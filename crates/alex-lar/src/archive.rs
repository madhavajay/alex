use crate::chunker::{ChunkerConfig, StreamingChunker};
use crate::conversation::{
    ConversationEntry, ConversationEntryId, Generation, GenerationId, TurnView, TurnViewId,
};
use crate::event::{Exchange, ExchangeId, Stage, StageId, StreamIndex, StreamIndexId};
use crate::exchange_metadata::{ExchangeMetadata, ExchangeMetadataData};
use crate::format::{file_length, FrameRead};
use crate::index::{
    payload_hash, Checkpoint, CheckpointPointer, ChunkIndexEntry, IndexBlock, IndexBlockRef,
    IndexEntries, IndexKind, FOOTER_TRAILER_LEN, INDEX_SCHEMA_V1, LOCATOR_FRAME_LEN,
};
use crate::model::{
    ensure_end, put_hash, put_u64, read_array, read_hash, read_u64, BodyManifest, ChunkHash,
    ChunkRef, HeaderBlock, HeaderBlockId, ManifestId,
};
use crate::page::{push_inner_frame, MetadataPage, StoredDictionary};
use crate::range::{segment_against_predecessor, RangeMatchConfig, Segment};
use crate::{
    read_file_header, write_file_header, Error, FileHeader, FrameReader, Limits, RecordFrame,
    RecordType, Result, REQUIRED_FEATURE_ARCHIVE_SET_BODY_REFS, REQUIRED_FEATURE_CONVERSATION_DAG,
};
use std::collections::HashMap;
use std::io::{Cursor, Read, Seek, SeekFrom, Write};

const RECORD_SCHEMA_V1: u16 = 1;
const COMPRESSION_ZSTD: u8 = 1;
const METADATA_PAGE_TARGET: usize = 256 * 1024;

/// Result of a preservation-first rewrite to the latest supported v1
/// container. Canonical records are copied without decoding or re-encoding
/// their payloads; only derived indexes and the footer are regenerated.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArchiveUpgradeReport {
    pub source_container_major: u16,
    pub source_container_minor: u16,
    pub output_container_major: u16,
    pub output_container_minor: u16,
    pub file_role: crate::FileRole,
    pub source_uuid: [u8; 16],
    pub output_uuid: [u8; 16],
    pub source_created_at_ns: u64,
    pub output_created_at_ns: u64,
    pub canonical_records_copied: u64,
    pub derived_records_replaced: u64,
    pub manifests_verified: u64,
    pub chunks_verified: u64,
}

/// Prove that every physical source feature can be represented by Alex's
/// selective graph rewriter before it omits unreachable records.
///
/// This is intentionally stricter than ordinary reading: readers may skip
/// optional extensions, while a selective rewrite would silently lose them.
/// Dictionary-compressed metadata is safe because the graph rewriter decodes
/// canonical values and re-encodes them without depending on the source
/// dictionary.
pub fn validate_selective_rewrite_source<R>(source: &mut R, limits: Limits) -> Result<()>
where
    R: Read + Seek,
{
    let reader = ArchiveReader::open(&mut *source, limits.clone())?;
    if !reader.is_sealed() || reader.recovery_status() != RecoveryStatus::Clean {
        return Err(Error::Invalid(
            "selective rewrite source must be a clean, sealed archive",
        ));
    }
    let header = reader.header().clone();
    let data_offset = reader.data_offset();
    if header.container_major != crate::DEFAULT_CONTAINER_MAJOR
        || header.container_minor != crate::DEFAULT_CONTAINER_MINOR
    {
        return Err(Error::Unsupported(format!(
            "selective rewrite source container version {}.{}",
            header.container_major, header.container_minor
        )));
    }
    if header.optional_feature_bits != 0 {
        return Err(Error::Unsupported(format!(
            "optional feature bits {:#x} cannot be preserved by selective rewrite",
            header.optional_feature_bits
        )));
    }
    drop(reader);

    let mut canonical_header = Vec::new();
    write_file_header(&mut canonical_header, &header)?;
    if canonical_header.len() as u64 != data_offset {
        return Err(Error::Unsupported(
            "file header extension cannot be preserved by selective rewrite".into(),
        ));
    }

    let scan_end = file_length(source)?
        .checked_sub(FOOTER_TRAILER_LEN)
        .ok_or(Error::Invalid("sealed archive is shorter than its footer"))?;
    source.seek(SeekFrom::Start(data_offset))?;
    while source.stream_position()? < scan_end {
        let offset = source.stream_position()?;
        let frame = {
            let mut frame_reader = FrameReader::new(&mut *source, &limits);
            match frame_reader.read_next()? {
                (FrameRead::Frame, Some(frame)) => frame,
                _ => return Err(Error::Invalid("incomplete frame in sealed archive")),
            }
        };
        if source.stream_position()? > scan_end {
            return Err(Error::InvalidDetail(format!(
                "record at {offset} overlaps the sealed footer"
            )));
        }
        match frame.record_type {
            RecordType::Chunk
            | RecordType::BodyManifest
            | RecordType::HeaderBlock
            | RecordType::StreamIndex
            | RecordType::Stage
            | RecordType::Exchange
            | RecordType::DictionaryData
            | RecordType::MetadataPage
            | RecordType::ConversationEntry
            | RecordType::Generation
            | RecordType::TurnView => {
                if frame.schema_version != RECORD_SCHEMA_V1 || frame.flags != RecordFrame::REQUIRED
                {
                    return Err(Error::Unsupported(format!(
                        "canonical record type {} at {offset} uses schema {} or flags {:#x}",
                        frame.record_type.code(),
                        frame.schema_version,
                        frame.flags
                    )));
                }
            }
            RecordType::ExchangeMetadata => {
                if frame.schema_version != RECORD_SCHEMA_V1 || frame.flags != 0 {
                    return Err(Error::Unsupported(format!(
                        "exchange metadata at {offset} uses schema {} or flags {:#x}",
                        frame.schema_version, frame.flags
                    )));
                }
            }
            RecordType::IndexBlock | RecordType::Checkpoint | RecordType::CheckpointLocator => {
                if frame.schema_version != INDEX_SCHEMA_V1 || frame.flags != 0 {
                    return Err(Error::Unsupported(format!(
                        "derived record type {} at {offset} uses schema {} or flags {:#x}",
                        frame.record_type.code(),
                        frame.schema_version,
                        frame.flags
                    )));
                }
            }
            RecordType::Unknown(code) => {
                return Err(Error::Unsupported(format!(
                    "optional record type {code} cannot be preserved by selective rewrite"
                )));
            }
        }
    }
    if source.stream_position()? != scan_end {
        return Err(Error::Invalid("record stream does not meet sealed footer"));
    }
    Ok(())
}

/// Rewrite one clean, sealed archive into a newly sealed latest-v1 archive.
///
/// The source is never written. Supported canonical frames remain in their
/// original physical order with identical type, schema, flags, and payload.
/// Unknown optional outer records are copied byte-for-byte in stream order.
/// Header extensions, optional file-feature bits, and non-v1 canonical schemas
/// are rejected when they cannot be reproduced without interpretation.
pub fn upgrade_archive<R, W>(
    source: &mut R,
    mut output: W,
    output_uuid: [u8; 16],
    output_created_at_ns: u64,
    output_writer: Vec<u8>,
    limits: Limits,
) -> Result<(W, ArchiveUpgradeReport)>
where
    R: Read + Seek,
    W: Read + Write + Seek,
{
    let mut reader = ArchiveReader::open(&mut *source, limits.clone())?;
    if !reader.is_sealed() || reader.recovery_status() != RecoveryStatus::Clean {
        return Err(Error::Invalid(
            "upgrade source must be a clean, sealed archive",
        ));
    }
    let source_header = reader.header().clone();
    if source_header.container_major != crate::DEFAULT_CONTAINER_MAJOR
        || source_header.container_minor != crate::DEFAULT_CONTAINER_MINOR
    {
        return Err(Error::Unsupported(format!(
            "upgrade source container version {}.{}",
            source_header.container_major, source_header.container_minor
        )));
    }
    if source_header.optional_feature_bits != 0 {
        return Err(Error::Unsupported(format!(
            "optional feature bits {:#x} cannot be preserved safely",
            source_header.optional_feature_bits
        )));
    }
    if output_uuid == source_header.file_uuid {
        return Err(Error::Invalid(
            "upgraded archive must use a new physical file UUID",
        ));
    }

    // A current-version header must round-trip to exactly the same physical
    // length. Any extra bounded bytes are extensions this implementation does
    // not understand and therefore cannot promise to preserve.
    let mut canonical_source_header = Vec::new();
    write_file_header(&mut canonical_source_header, &source_header)?;
    if canonical_source_header.len() as u64 != reader.data_offset() {
        return Err(Error::Unsupported(
            "file header extension cannot be preserved safely".into(),
        ));
    }

    let manifest_ids: Vec<_> = reader.manifest_ids().copied().collect();
    for id in &manifest_ids {
        reader.write_body(id, &mut std::io::sink())?;
    }
    let chunk_hashes: Vec<_> = reader.chunk_records().map(|record| record.hash).collect();
    for hash in &chunk_hashes {
        reader.read_chunk(hash)?;
    }
    drop(reader);

    if file_length(&mut output)? != 0 {
        return Err(Error::Invalid("upgrade output is not empty"));
    }
    let source_end = file_length(source)?;
    let canonical_end = source_end
        .checked_sub(FOOTER_TRAILER_LEN)
        .ok_or(Error::Invalid("sealed archive is shorter than its footer"))?;
    let mut output_header = source_header.clone();
    output_header.container_major = crate::DEFAULT_CONTAINER_MAJOR;
    output_header.container_minor = crate::DEFAULT_CONTAINER_MINOR;
    output_header.file_uuid = output_uuid;
    output_header.created_at_ns = output_created_at_ns;
    output_header.writer = output_writer;
    output.seek(SeekFrom::Start(0))?;
    write_file_header(&mut output, &output_header)?;

    source.seek(SeekFrom::Start(canonical_source_header.len() as u64))?;
    let mut canonical_records_copied = 0u64;
    let mut derived_records_replaced = 0u64;
    let mut dictionary_index = 0usize;
    let mut saw_non_dictionary_record = false;
    while source.stream_position()? < canonical_end {
        let before = source.stream_position()?;
        let frame = {
            let mut frame_reader = FrameReader::new(&mut *source, &limits);
            match frame_reader.read_next()? {
                (FrameRead::Frame, Some(frame)) => frame,
                (FrameRead::Truncated, _) => {
                    return Err(Error::Invalid("truncated frame before sealed footer"))
                }
                (FrameRead::CleanEof, _) => {
                    return Err(Error::Invalid("unexpected EOF before sealed footer"))
                }
                _ => return Err(Error::Invalid("invalid frame read result")),
            }
        };
        if source.stream_position()? > canonical_end {
            return Err(Error::InvalidDetail(format!(
                "record at {before} overlaps the sealed footer"
            )));
        }
        match frame.record_type {
            RecordType::Chunk
            | RecordType::BodyManifest
            | RecordType::HeaderBlock
            | RecordType::StreamIndex
            | RecordType::Stage
            | RecordType::Exchange
            | RecordType::MetadataPage
            | RecordType::ConversationEntry
            | RecordType::Generation
            | RecordType::TurnView => {
                if frame.schema_version != RECORD_SCHEMA_V1 || frame.flags != RecordFrame::REQUIRED
                {
                    return Err(Error::Unsupported(format!(
                        "canonical record type {} at {before} uses schema {} or flags {:#x}",
                        frame.record_type.code(),
                        frame.schema_version,
                        frame.flags
                    )));
                }
                saw_non_dictionary_record = true;
                frame.write(&mut output)?;
                canonical_records_copied += 1;
            }
            RecordType::DictionaryData => {
                if frame.schema_version != RECORD_SCHEMA_V1
                    || frame.flags != RecordFrame::REQUIRED
                    || saw_non_dictionary_record
                {
                    return Err(Error::Unsupported(format!(
                        "dictionary record at {before} is not a contiguous canonical v1 record"
                    )));
                }
                let dictionary = StoredDictionary::decode(&frame.payload, &limits)?;
                let descriptor =
                    source_header
                        .dictionaries
                        .get(dictionary_index)
                        .ok_or(Error::Invalid(
                            "dictionary record lacks a header descriptor",
                        ))?;
                if descriptor.id != dictionary.id
                    || descriptor.uncompressed_length != dictionary.bytes.len() as u64
                {
                    return Err(Error::Invalid(
                        "dictionary record does not match its header descriptor",
                    ));
                }
                dictionary_index += 1;
                frame.write(&mut output)?;
                canonical_records_copied += 1;
            }
            RecordType::ExchangeMetadata => {
                if frame.flags & RecordFrame::REQUIRED != 0 {
                    return Err(Error::Unsupported(format!(
                        "exchange metadata record at {before} is unexpectedly required"
                    )));
                }
                saw_non_dictionary_record = true;
                frame.write(&mut output)?;
                canonical_records_copied += 1;
            }
            RecordType::IndexBlock | RecordType::Checkpoint | RecordType::CheckpointLocator => {
                if frame.schema_version != INDEX_SCHEMA_V1 || frame.flags != 0 {
                    return Err(Error::Unsupported(format!(
                        "derived record type {} at {before} uses schema {} or flags {:#x}",
                        frame.record_type.code(),
                        frame.schema_version,
                        frame.flags
                    )));
                }
                saw_non_dictionary_record = true;
                derived_records_replaced += 1;
            }
            RecordType::Unknown(_) => {
                // Optional outer records are opaque, independently framed,
                // checksummed, and bounded. Preserve them byte-for-byte in
                // stream order so future extensions survive a v1 upgrade.
                debug_assert_eq!(frame.flags & RecordFrame::REQUIRED, 0);
                saw_non_dictionary_record = true;
                frame.write(&mut output)?;
                canonical_records_copied += 1;
            }
        }
    }
    if source.stream_position()? != canonical_end {
        return Err(Error::Invalid("record stream does not meet sealed footer"));
    }
    if dictionary_index != source_header.dictionaries.len() {
        return Err(Error::Invalid(
            "file header dictionary descriptors do not map one-to-one to dictionary records",
        ));
    }

    // This is intentionally a forward-scan open of the canonical-only output.
    // It proves the copied graph before any derived indexes are generated.
    output.flush()?;
    output.seek(SeekFrom::Start(0))?;
    let max_chunk = usize::try_from(limits.max_chunk_uncompressed.min(8 * 1024))
        .map_err(|_| Error::Invalid("chunk limit exceeds address space"))?;
    if max_chunk == 0 {
        return Err(Error::Invalid("chunk limit must be non-zero"));
    }
    let target_chunk = max_chunk.min(2 * 1024);
    let rewrite_chunker = ChunkerConfig {
        min_size: target_chunk.min(512),
        target_size: target_chunk,
        max_size: max_chunk,
    };
    let mut writer = ArchiveWriter::open_append(output, rewrite_chunker, limits)?;
    writer.seal()?;
    let output = writer.into_inner()?;
    Ok((
        output,
        ArchiveUpgradeReport {
            source_container_major: source_header.container_major,
            source_container_minor: source_header.container_minor,
            output_container_major: output_header.container_major,
            output_container_minor: output_header.container_minor,
            file_role: source_header.file_role,
            source_uuid: source_header.file_uuid,
            output_uuid,
            source_created_at_ns: source_header.created_at_ns,
            output_created_at_ns,
            canonical_records_copied,
            derived_records_replaced,
            manifests_verified: manifest_ids.len() as u64,
            chunks_verified: chunk_hashes.len() as u64,
        },
    ))
}

/// Verify that an upgraded archive is a faithful physical rewrite of its
/// source. Header provenance fields may change, but every supported canonical
/// frame must be byte-identical and in the same order. The output footer path
/// and all local body bytes are verified as well.
pub fn verify_upgraded_archive<R1, R2>(
    source: &mut R1,
    output: &mut R2,
    limits: Limits,
) -> Result<()>
where
    R1: Read + Seek,
    R2: Read + Seek,
{
    let source_reader = ArchiveReader::open(&mut *source, limits.clone())?;
    let source_header = source_reader.header().clone();
    if !source_reader.is_sealed() || source_reader.recovery_status() != RecoveryStatus::Clean {
        return Err(Error::Invalid(
            "upgrade verification source is not clean and sealed",
        ));
    }
    let mut output_reader = ArchiveReader::open(&mut *output, limits.clone())?;
    let output_header = output_reader.header().clone();
    if !output_reader.is_sealed()
        || output_reader.recovery_status() != RecoveryStatus::Clean
        || output_reader.open_path() != OpenPath::Footer
    {
        return Err(Error::Invalid(
            "upgraded archive did not pass the sealed footer path",
        ));
    }
    if output_header.file_uuid == source_header.file_uuid {
        return Err(Error::Invalid("upgraded archive reused the source UUID"));
    }
    if output_header.container_major != crate::DEFAULT_CONTAINER_MAJOR
        || output_header.container_minor != crate::DEFAULT_CONTAINER_MINOR
        || output_header.file_role != source_header.file_role
        || output_header.required_feature_bits != source_header.required_feature_bits
        || output_header.optional_feature_bits != source_header.optional_feature_bits
        || output_header.default_hash_algorithm != source_header.default_hash_algorithm
        || output_header.zstd_level != source_header.zstd_level
        || output_header.dictionaries != source_header.dictionaries
    {
        return Err(Error::Invalid(
            "upgraded archive changed a preserved header field",
        ));
    }
    let output_manifest_ids: Vec<_> = output_reader.manifest_ids().copied().collect();
    for id in &output_manifest_ids {
        output_reader.write_body(id, &mut std::io::sink())?;
    }
    let output_chunks: Vec<_> = output_reader
        .chunk_records()
        .map(|record| record.hash)
        .collect();
    for hash in &output_chunks {
        output_reader.read_chunk(hash)?;
    }
    drop(source_reader);
    drop(output_reader);

    let source_end = file_length(source)?
        .checked_sub(FOOTER_TRAILER_LEN)
        .ok_or(Error::Invalid("source is shorter than its footer"))?;
    let output_end = file_length(output)?
        .checked_sub(FOOTER_TRAILER_LEN)
        .ok_or(Error::Invalid("output is shorter than its footer"))?;
    let (_, source_data_offset) = {
        source.seek(SeekFrom::Start(0))?;
        read_file_header(&mut *source, &limits)?
    };
    let (_, output_data_offset) = {
        output.seek(SeekFrom::Start(0))?;
        read_file_header(&mut *output, &limits)?
    };
    source.seek(SeekFrom::Start(source_data_offset))?;
    output.seek(SeekFrom::Start(output_data_offset))?;
    loop {
        let source_frame = next_canonical_upgrade_frame(source, source_end, &limits)?;
        let output_frame = next_canonical_upgrade_frame(output, output_end, &limits)?;
        match (source_frame, output_frame) {
            (None, None) => break,
            (Some(left), Some(right))
                if left.record_type == right.record_type
                    && left.schema_version == right.schema_version
                    && left.flags == right.flags
                    && left.payload == right.payload => {}
            _ => {
                return Err(Error::Invalid(
                    "upgraded archive canonical record sequence differs from source",
                ))
            }
        }
    }
    Ok(())
}

fn next_canonical_upgrade_frame<R: Read + Seek>(
    input: &mut R,
    scan_end: u64,
    limits: &Limits,
) -> Result<Option<RecordFrame>> {
    while input.stream_position()? < scan_end {
        let before = input.stream_position()?;
        let frame = {
            let mut reader = FrameReader::new(&mut *input, limits);
            match reader.read_next()? {
                (FrameRead::Frame, Some(frame)) => frame,
                _ => return Err(Error::Invalid("incomplete frame in sealed archive")),
            }
        };
        if input.stream_position()? > scan_end {
            return Err(Error::InvalidDetail(format!(
                "record at {before} overlaps the sealed footer"
            )));
        }
        match frame.record_type {
            RecordType::Chunk
            | RecordType::BodyManifest
            | RecordType::HeaderBlock
            | RecordType::StreamIndex
            | RecordType::Stage
            | RecordType::Exchange
            | RecordType::DictionaryData
            | RecordType::MetadataPage
            | RecordType::ConversationEntry
            | RecordType::Generation
            | RecordType::TurnView => {
                if frame.schema_version != RECORD_SCHEMA_V1 || frame.flags != RecordFrame::REQUIRED
                {
                    return Err(Error::Unsupported(format!(
                        "unsupported canonical record at {before}"
                    )));
                }
                return Ok(Some(frame));
            }
            RecordType::IndexBlock | RecordType::Checkpoint | RecordType::CheckpointLocator => {
                if frame.schema_version != INDEX_SCHEMA_V1 || frame.flags != 0 {
                    return Err(Error::Unsupported(format!(
                        "unsupported derived record at {before}"
                    )));
                }
            }
            RecordType::ExchangeMetadata | RecordType::Unknown(_) => {
                if frame.flags & RecordFrame::REQUIRED != 0 {
                    return Err(Error::Unsupported(format!(
                        "unsupported required extension record at {before}"
                    )));
                }
                return Ok(Some(frame));
            }
        }
    }
    if input.stream_position()? != scan_end {
        return Err(Error::Invalid("record stream does not meet sealed footer"));
    }
    Ok(None)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecoveryStatus {
    Clean,
    TruncatedTail {
        last_valid_offset: u64,
        tail_bytes: u64,
    },
    /// Canonical records were recovered after a corrupt derived checkpoint or
    /// footer was ignored. This is intentionally not `Clean`: callers must
    /// repair or rewrite the index before appending.
    CorruptIndexFallback {
        last_valid_offset: u64,
        tail_bytes: u64,
    },
}

/// Instrumentation for proving whether open used a persisted index or scanned
/// the record stream. A checkpoint/footer fast path still validates every
/// referenced metadata frame and canonical body reference.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OpenPath {
    ForwardScan,
    Checkpoint,
    Footer,
}

#[derive(Clone, Debug)]
struct ChunkLocation {
    frame_offset: u64,
    uncompressed_length: u64,
    compressed_length: u64,
}

#[derive(Clone, Debug)]
struct PendingMetadataRecord {
    record_type: RecordType,
    id: [u8; 32],
    payload: Vec<u8>,
}

/// Physical location and identity of one independently compressed chunk
/// record. Catalog-backed body stores persist this only after the containing
/// file has been flushed and synced.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChunkRecordDescriptor {
    pub hash: ChunkHash,
    pub frame_offset: u64,
    pub uncompressed_length: u64,
    pub compressed_length: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CheckpointRecordDescriptor {
    pub frame_offset: u64,
    pub frame_length: u64,
    pub payload_hash: [u8; 32],
    /// Durable append boundary immediately after the checkpoint locator.
    pub append_offset: u64,
}

/// Read and verify one chunk directly from its cataloged frame location.
///
/// Live archive sets already persist the physical offset only after the file
/// is synced. Using that descriptor avoids rebuilding an active pack's full
/// in-memory index just to read one newly written body. The record checksum,
/// schema, content hash, and both stored lengths are still verified.
pub fn read_chunk_record_at<R: Read + Seek>(
    io: &mut R,
    descriptor: &ChunkRecordDescriptor,
    limits: &Limits,
) -> Result<Vec<u8>> {
    let (frame, _) = read_frame_at(io, limits, descriptor.frame_offset)?;
    if frame.record_type != RecordType::Chunk
        || frame.schema_version != RECORD_SCHEMA_V1
        || frame.flags & RecordFrame::REQUIRED == 0
    {
        return Err(Error::Invalid(
            "chunk descriptor points to the wrong record",
        ));
    }
    let stored = StoredChunk::decode(&frame.payload, limits)?;
    if stored.hash != descriptor.hash
        || stored.uncompressed_length != descriptor.uncompressed_length
        || stored.compressed.len() as u64 != descriptor.compressed_length
    {
        return Err(Error::Invalid("chunk descriptor metadata mismatch"));
    }
    stored.decompress(limits)
}

#[derive(Clone, Debug)]
struct StoredChunk {
    hash: ChunkHash,
    uncompressed_length: u64,
    compressed: Vec<u8>,
}

impl StoredChunk {
    fn from_bytes(bytes: &[u8], level: i32) -> Result<Self> {
        let compressed = zstd::stream::encode_all(Cursor::new(bytes), level).map_err(Error::Io)?;
        Ok(Self {
            hash: ChunkHash::blake3(bytes),
            uncompressed_length: bytes.len() as u64,
            compressed,
        })
    }

    fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(50 + self.compressed.len());
        put_hash(&mut out, &self.hash);
        put_u64(&mut out, self.uncompressed_length);
        out.push(COMPRESSION_ZSTD);
        put_u64(&mut out, self.compressed.len() as u64);
        out.extend_from_slice(&self.compressed);
        out
    }

    fn decode(payload: &[u8], limits: &Limits) -> Result<Self> {
        let mut input = Cursor::new(payload);
        let hash = read_hash(&mut input)?;
        let uncompressed_length = read_u64(&mut input)?;
        if uncompressed_length > limits.max_chunk_uncompressed {
            return Err(Error::Limit {
                what: "uncompressed chunk",
                actual: uncompressed_length,
                limit: limits.max_chunk_uncompressed,
            });
        }
        if read_array::<1>(&mut input)?[0] != COMPRESSION_ZSTD {
            return Err(Error::Unsupported("chunk compression".into()));
        }
        let compressed_length = read_u64(&mut input)?;
        if compressed_length > limits.max_frame_payload {
            return Err(Error::Limit {
                what: "compressed chunk",
                actual: compressed_length,
                limit: limits.max_frame_payload,
            });
        }
        let start = usize::try_from(input.position())
            .map_err(|_| Error::Invalid("chunk position exceeds address space"))?;
        let compressed_size = usize::try_from(compressed_length).map_err(|_| Error::Limit {
            what: "compressed chunk address space",
            actual: compressed_length,
            limit: usize::MAX as u64,
        })?;
        let end = start
            .checked_add(compressed_size)
            .ok_or(Error::Invalid("chunk length overflow"))?;
        let compressed = payload
            .get(start..end)
            .ok_or(Error::Invalid("truncated compressed chunk"))?
            .to_vec();
        input.set_position(end as u64);
        ensure_end(&input, payload)?;
        Ok(Self {
            hash,
            uncompressed_length,
            compressed,
        })
    }

    fn decompress(&self, limits: &Limits) -> Result<Vec<u8>> {
        if self.uncompressed_length > limits.max_chunk_uncompressed {
            return Err(Error::Limit {
                what: "uncompressed chunk",
                actual: self.uncompressed_length,
                limit: limits.max_chunk_uncompressed,
            });
        }
        let decoder = zstd::stream::read::Decoder::new(Cursor::new(&self.compressed))?;
        let mut bounded = decoder.take(self.uncompressed_length.saturating_add(1));
        let mut bytes = Vec::with_capacity(self.uncompressed_length as usize);
        bounded.read_to_end(&mut bytes)?;
        if bytes.len() as u64 != self.uncompressed_length {
            return Err(Error::Invalid("decompressed chunk length mismatch"));
        }
        if ChunkHash::blake3(&bytes) != self.hash {
            return Err(Error::Invalid("decompressed chunk hash mismatch"));
        }
        Ok(bytes)
    }
}

/// Append-only archive writer. Duplicate content is verified byte-for-byte
/// against its existing frame before the existing logical ID is reused.
type BodyIdentity = (ChunkHash, u64, Option<Vec<u8>>, Option<Vec<u8>>);

pub struct ArchiveWriter<W> {
    io: W,
    header: FileHeader,
    limits: Limits,
    chunker_config: ChunkerConfig,
    chunks: HashMap<ChunkHash, ChunkLocation>,
    manifests: HashMap<ManifestId, BodyManifest>,
    manifest_offsets: HashMap<ManifestId, u64>,
    pending_metadata: Vec<PendingMetadataRecord>,
    pending_metadata_bytes: usize,
    metadata_page_batching: bool,
    metadata_dictionary: Option<StoredDictionary>,
    body_identities: HashMap<BodyIdentity, ManifestId>,
    header_blocks: HashMap<HeaderBlockId, HeaderBlock>,
    header_block_offsets: HashMap<HeaderBlockId, u64>,
    stream_indexes: HashMap<StreamIndexId, StreamIndex>,
    stream_index_offsets: HashMap<StreamIndexId, u64>,
    stages: HashMap<StageId, Stage>,
    stage_offsets: HashMap<StageId, u64>,
    exchanges: HashMap<ExchangeId, Exchange>,
    exchange_offsets: HashMap<ExchangeId, u64>,
    exchange_metadata: HashMap<ExchangeId, ExchangeMetadata>,
    traces: HashMap<Vec<u8>, ExchangeId>,
    sessions: HashMap<Vec<u8>, Vec<ExchangeId>>,
    conversation_entries: HashMap<ConversationEntryId, ConversationEntry>,
    conversation_entry_offsets: HashMap<ConversationEntryId, u64>,
    generations: HashMap<GenerationId, Generation>,
    generation_offsets: HashMap<GenerationId, u64>,
    turn_views: HashMap<TurnViewId, TurnView>,
    turn_view_offsets: HashMap<TurnViewId, u64>,
    turn_traces: HashMap<Vec<u8>, TurnViewId>,
    record_count: usize,
    sealed: bool,
}

impl<W: Read + Write + Seek> ArchiveWriter<W> {
    pub fn create(
        mut io: W,
        header: FileHeader,
        chunker_config: ChunkerConfig,
        limits: Limits,
    ) -> Result<Self> {
        chunker_config.validate()?;
        if chunker_config
            .for_body_length(limits.max_body_length)
            .max_size as u64
            > limits.max_chunk_uncompressed
        {
            return Err(Error::Limit {
                what: "chunker maximum",
                actual: chunker_config
                    .for_body_length(limits.max_body_length)
                    .max_size as u64,
                limit: limits.max_chunk_uncompressed,
            });
        }
        if file_length(&mut io)? != 0 {
            return Err(Error::Invalid("new archive output is not empty"));
        }
        io.seek(SeekFrom::Start(0))?;
        write_file_header(&mut io, &header)?;
        Ok(Self {
            io,
            header,
            limits,
            chunker_config,
            chunks: HashMap::new(),
            manifests: HashMap::new(),
            manifest_offsets: HashMap::new(),
            pending_metadata: Vec::new(),
            pending_metadata_bytes: 0,
            metadata_page_batching: false,
            metadata_dictionary: None,
            body_identities: HashMap::new(),
            header_blocks: HashMap::new(),
            header_block_offsets: HashMap::new(),
            stream_indexes: HashMap::new(),
            stream_index_offsets: HashMap::new(),
            stages: HashMap::new(),
            stage_offsets: HashMap::new(),
            exchanges: HashMap::new(),
            exchange_offsets: HashMap::new(),
            exchange_metadata: HashMap::new(),
            traces: HashMap::new(),
            sessions: HashMap::new(),
            conversation_entries: HashMap::new(),
            conversation_entry_offsets: HashMap::new(),
            generations: HashMap::new(),
            generation_offsets: HashMap::new(),
            turn_views: HashMap::new(),
            turn_view_offsets: HashMap::new(),
            turn_traces: HashMap::new(),
            record_count: 0,
            sealed: false,
        })
    }

    /// Create a self-contained archive whose metadata pages use a caller-
    /// supplied zstd dictionary. The dictionary descriptor is committed in the
    /// file header and the hash-verified bytes are written once as a required
    /// record before any page can reference them.
    pub fn create_with_metadata_dictionary(
        io: W,
        mut header: FileHeader,
        chunker_config: ChunkerConfig,
        limits: Limits,
        dictionary_bytes: Vec<u8>,
        dictionary_name: impl Into<Vec<u8>>,
    ) -> Result<Self> {
        if dictionary_bytes.len() as u64 > limits.max_field_length as u64 {
            return Err(Error::Limit {
                what: "compression dictionary",
                actual: dictionary_bytes.len() as u64,
                limit: limits.max_field_length as u64,
            });
        }
        let dictionary = StoredDictionary::new(dictionary_bytes);
        if header
            .dictionaries
            .iter()
            .any(|descriptor| descriptor.id == dictionary.id)
        {
            return Err(Error::Invalid("duplicate dictionary descriptor"));
        }
        header.dictionaries.push(crate::DictionaryDescriptor {
            id: dictionary.id,
            uncompressed_length: dictionary.bytes.len() as u64,
            name: dictionary_name.into(),
        });
        let mut writer = Self::create(io, header, chunker_config, limits)?;
        writer.append_frame(RecordType::DictionaryData, dictionary.encode()?)?;
        writer.metadata_page_batching = true;
        writer.metadata_dictionary = Some(dictionary);
        Ok(writer)
    }

    /// Reopen a clean archive for append after rebuilding and validating its
    /// in-memory indexes through the normal reader. Interrupted tails are
    /// rejected because a generic `Read + Write + Seek` value cannot be safely
    /// truncated; callers must recover/repair such a file before appending.
    pub fn open_append(mut io: W, chunker_config: ChunkerConfig, limits: Limits) -> Result<Self> {
        chunker_config.validate()?;
        if chunker_config
            .for_body_length(limits.max_body_length)
            .max_size as u64
            > limits.max_chunk_uncompressed
        {
            return Err(Error::Limit {
                what: "chunker maximum",
                actual: chunker_config
                    .for_body_length(limits.max_body_length)
                    .max_size as u64,
                limit: limits.max_chunk_uncompressed,
            });
        }
        let scanned = ArchiveReader::open(&mut io, limits.clone())?;
        if scanned.sealed {
            return Err(Error::Invalid(
                "sealed archive cannot be reopened for append",
            ));
        }
        match scanned.recovery {
            RecoveryStatus::Clean => {}
            RecoveryStatus::TruncatedTail {
                last_valid_offset,
                tail_bytes,
            } => {
                return Err(Error::InvalidDetail(format!(
                    "archive has a truncated tail ({tail_bytes} bytes after {last_valid_offset}); repair before append"
                )))
            }
            RecoveryStatus::CorruptIndexFallback {
                last_valid_offset,
                tail_bytes,
            } => {
                return Err(Error::InvalidDetail(format!(
                    "archive has a corrupt derived index ({tail_bytes} bytes after {last_valid_offset}); repair before append"
                )))
            }
        }
        let header = scanned.header.clone();
        let chunks = scanned.chunks.clone();
        let manifests = scanned.manifests.clone();
        let manifest_offsets = scanned.manifest_offsets.clone();
        let body_identities = scanned.body_identities.clone();
        let header_blocks = scanned.header_blocks.clone();
        let header_block_offsets = scanned.header_block_offsets.clone();
        let stream_indexes = scanned.stream_indexes.clone();
        let stream_index_offsets = scanned.stream_index_offsets.clone();
        let stages = scanned.stages.clone();
        let stage_offsets = scanned.stage_offsets.clone();
        let exchanges = scanned.exchanges.clone();
        let exchange_offsets = scanned.exchange_offsets.clone();
        let exchange_metadata = scanned.exchange_metadata.clone();
        let traces = scanned.traces.clone();
        let sessions = scanned.sessions.clone();
        let conversation_entries = scanned.conversation_entries.clone();
        let conversation_entry_offsets = scanned.conversation_entry_offsets.clone();
        let generations = scanned.generations.clone();
        let generation_offsets = scanned.generation_offsets.clone();
        let turn_views = scanned.turn_views.clone();
        let turn_view_offsets = scanned.turn_view_offsets.clone();
        let turn_traces = scanned.turn_traces.clone();
        let record_count = scanned.record_count;
        drop(scanned);
        io.seek(SeekFrom::End(0))?;
        Ok(Self {
            io,
            header,
            limits,
            chunker_config,
            chunks,
            manifests,
            manifest_offsets,
            pending_metadata: Vec::new(),
            pending_metadata_bytes: 0,
            metadata_page_batching: false,
            metadata_dictionary: None,
            body_identities,
            header_blocks,
            header_block_offsets,
            stream_indexes,
            stream_index_offsets,
            stages,
            stage_offsets,
            exchanges,
            exchange_offsets,
            exchange_metadata,
            traces,
            sessions,
            conversation_entries,
            conversation_entry_offsets,
            generations,
            generation_offsets,
            turn_views,
            turn_view_offsets,
            turn_traces,
            record_count,
            sealed: false,
        })
    }

    pub fn append_body(&mut self, bytes: &[u8]) -> Result<ManifestId> {
        if bytes.len() as u64 > self.limits.max_body_length {
            return Err(Error::Limit {
                what: "body length",
                actual: bytes.len() as u64,
                limit: self.limits.max_body_length,
            });
        }
        if let Some(id) = self.existing_body_id(bytes, None, None)? {
            return Ok(id);
        }
        let chunker_config = self.chunker_config.for_body_length(bytes.len() as u64);
        self.append_reader_with_config(Cursor::new(bytes), chunker_config)
    }

    /// Append body bytes while retaining the manifest-level media metadata.
    /// Chunks remain content-addressed, so two semantic manifests over the
    /// same bytes do not duplicate the actual body storage.
    pub fn append_body_with_metadata(
        &mut self,
        bytes: &[u8],
        media_type: Option<Vec<u8>>,
        content_encoding: Option<Vec<u8>>,
    ) -> Result<ManifestId> {
        if bytes.len() as u64 > self.limits.max_body_length {
            return Err(Error::Limit {
                what: "body length",
                actual: bytes.len() as u64,
                limit: self.limits.max_body_length,
            });
        }
        if let Some(id) =
            self.existing_body_id(bytes, media_type.as_deref(), content_encoding.as_deref())?
        {
            return Ok(id);
        }
        let chunker_config = self.chunker_config.for_body_length(bytes.len() as u64);
        self.append_reader_with_config_and_metadata(
            Cursor::new(bytes),
            chunker_config,
            media_type,
            content_encoding,
        )
    }

    /// Appends `bytes` while reusing byte ranges from one semantically related
    /// predecessor. The resulting manifest still references chunks directly;
    /// it never depends on the predecessor manifest during reconstruction.
    /// When the base is missing, unsuitable, or outside the matcher bounds,
    /// this safely falls back to ordinary content-defined chunking.
    pub fn append_body_with_predecessor(
        &mut self,
        bytes: &[u8],
        predecessor: ManifestId,
    ) -> Result<ManifestId> {
        self.append_body_with_predecessor_config(bytes, predecessor, RangeMatchConfig::default())
    }

    pub fn append_body_with_predecessor_config(
        &mut self,
        bytes: &[u8],
        predecessor: ManifestId,
        range_config: RangeMatchConfig,
    ) -> Result<ManifestId> {
        if bytes.len() as u64 > self.limits.max_body_length {
            return Err(Error::Limit {
                what: "body length",
                actual: bytes.len() as u64,
                limit: self.limits.max_body_length,
            });
        }
        if let Some(id) = self.existing_body_id(bytes, None, None)? {
            return Ok(id);
        }
        let Some(base_manifest) = self.manifests.get(&predecessor).cloned() else {
            return self.append_body(bytes);
        };
        let range_config = range_config.validate()?;
        if base_manifest.total_length > range_config.max_base_bytes as u64 {
            return self.append_body(bytes);
        }
        let base = self.read_manifest_bytes(&base_manifest)?;
        let Some(segments) = segment_against_predecessor(bytes, &base, range_config)? else {
            return self.append_body(bytes);
        };

        let mut refs = Vec::new();
        let mut logical_offset = 0u64;
        for segment in segments {
            match segment {
                Segment::Copy {
                    base_offset,
                    length,
                } => self.append_copy_refs(
                    &base_manifest,
                    base_offset as u64,
                    length as u64,
                    &mut logical_offset,
                    &mut refs,
                )?,
                Segment::Literal {
                    current_offset,
                    length,
                } => self.append_literal_refs(
                    &bytes[current_offset..current_offset + length],
                    &mut logical_offset,
                    &mut refs,
                )?,
            }
            if refs.len() > self.limits.max_manifest_chunks as usize {
                return Err(Error::Limit {
                    what: "manifest chunk count",
                    actual: refs.len() as u64,
                    limit: self.limits.max_manifest_chunks as u64,
                });
            }
        }
        let manifest = BodyManifest::new(
            bytes.len() as u64,
            ChunkHash::blake3(bytes),
            None,
            None,
            refs,
        );
        self.append_manifest(manifest)
    }

    pub fn append_reader<R: Read>(&mut self, mut input: R) -> Result<ManifestId> {
        let chunker_config = self.chunker_config;
        self.append_reader_with_config(&mut input, chunker_config)
    }

    fn append_reader_with_config<R: Read>(
        &mut self,
        input: R,
        chunker_config: ChunkerConfig,
    ) -> Result<ManifestId> {
        self.append_reader_with_config_and_metadata(input, chunker_config, None, None)
    }

    fn append_reader_with_config_and_metadata<R: Read>(
        &mut self,
        mut input: R,
        chunker_config: ChunkerConfig,
        media_type: Option<Vec<u8>>,
        content_encoding: Option<Vec<u8>>,
    ) -> Result<ManifestId> {
        let mut whole_hash = blake3::Hasher::new();
        let mut chunker = StreamingChunker::new(chunker_config)?;
        let mut refs = Vec::new();
        let mut logical_offset = 0u64;
        let mut total_length = 0u64;
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let read = input.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            total_length = total_length
                .checked_add(read as u64)
                .ok_or(Error::Invalid("body length overflow"))?;
            if total_length > self.limits.max_body_length {
                return Err(Error::Limit {
                    what: "body length",
                    actual: total_length,
                    limit: self.limits.max_body_length,
                });
            }
            whole_hash.update(&buffer[..read]);
            chunker.push(&buffer[..read], |chunk| {
                let hash = self.append_chunk(chunk)?;
                refs.push(ChunkRef {
                    chunk_hash: hash,
                    chunk_offset: 0,
                    logical_offset,
                    length: chunk.len() as u64,
                });
                logical_offset += chunk.len() as u64;
                Ok(())
            })?;
        }
        chunker.finish(|chunk| {
            let hash = self.append_chunk(chunk)?;
            refs.push(ChunkRef {
                chunk_hash: hash,
                chunk_offset: 0,
                logical_offset,
                length: chunk.len() as u64,
            });
            logical_offset += chunk.len() as u64;
            Ok(())
        })?;
        let manifest = BodyManifest::new(
            total_length,
            ChunkHash {
                algorithm: crate::HashAlgorithm::Blake3,
                digest: *whole_hash.finalize().as_bytes(),
            },
            media_type,
            content_encoding,
            refs,
        );
        self.append_manifest(manifest)
    }

    pub fn append_header_block(&mut self, block: HeaderBlock) -> Result<HeaderBlockId> {
        let expected = HeaderBlock::new(block.fidelity, block.atoms.clone());
        if expected.id != block.id {
            return Err(Error::Invalid(
                "header block content ID does not match contents",
            ));
        }
        if let Some(existing) = self.header_blocks.get(&block.id) {
            if existing != &block {
                return Err(Error::Invalid("header block hash collision"));
            }
            return Ok(block.id);
        }
        let id = block.id;
        self.append_metadata_record(RecordType::HeaderBlock, id.0, block.encode())?;
        self.header_blocks.insert(id, block);
        Ok(id)
    }

    /// Appends compact range/timing metadata for a streamed body. The raw body
    /// manifest must already exist; no body bytes are copied into this record.
    pub fn append_stream_index(&mut self, index: StreamIndex) -> Result<StreamIndexId> {
        let body_length = self
            .manifests
            .get(&index.raw_body_manifest_id)
            .ok_or_else(|| Error::Missing(index.raw_body_manifest_id.to_string()))?
            .total_length;
        self.append_stream_index_with_external_manifest(index, body_length)
    }

    /// Appends timing metadata for a body manifest validated by the archive-set
    /// catalog. The body length is supplied separately so read coverage is
    /// still checked without copying the manifest or body into this file.
    pub fn append_stream_index_with_external_manifest(
        &mut self,
        index: StreamIndex,
        body_length: u64,
    ) -> Result<StreamIndexId> {
        match self.manifests.get(&index.raw_body_manifest_id) {
            Some(manifest) if manifest.total_length != body_length => {
                return Err(Error::Invalid("stream body length does not match manifest"));
            }
            None if self.header.required_feature_bits & REQUIRED_FEATURE_ARCHIVE_SET_BODY_REFS
                == 0 =>
            {
                return Err(Error::Unsupported(
                    "external stream manifests require the archive-set-body-refs feature".into(),
                ));
            }
            _ => {}
        }
        index.validate(body_length, &self.limits)?;
        let expected = StreamIndex::new(
            index.raw_body_manifest_id,
            index.reads.clone(),
            index.frames.clone(),
        );
        if expected.id != index.id {
            return Err(Error::Invalid("stream index ID does not match contents"));
        }
        if let Some(existing) = self.stream_indexes.get(&index.id) {
            if existing != &index {
                return Err(Error::Invalid("stream index hash collision"));
            }
            return Ok(index.id);
        }
        self.ensure_index_capacity(
            "stream index count",
            self.stream_indexes.len(),
            self.limits.max_stream_indexes,
        )?;
        let id = index.id;
        self.append_metadata_record(RecordType::StreamIndex, id.0, index.encode())?;
        self.stream_indexes.insert(id, index);
        Ok(id)
    }

    /// Appends one immutable capture stage after validating that every header,
    /// body, and stream reference resolves to an earlier record.
    pub fn append_stage(&mut self, stage: Stage) -> Result<StageId> {
        self.append_stage_with_external_manifests(stage, &[])
    }

    /// Appends a stage whose body manifests may live in another body pack in
    /// the same archive set. Header and stream records remain local. Callers
    /// must validate every supplied external manifest through their archive
    /// set catalog before using this API.
    pub fn append_stage_with_external_manifests(
        &mut self,
        stage: Stage,
        external_manifests: &[ManifestId],
    ) -> Result<StageId> {
        if !external_manifests.is_empty()
            && self.header.required_feature_bits & REQUIRED_FEATURE_ARCHIVE_SET_BODY_REFS == 0
        {
            return Err(Error::Unsupported(
                "external body manifests require the archive-set-body-refs feature".into(),
            ));
        }
        stage.validate_shape(&self.limits)?;
        self.validate_stage_references(&stage, external_manifests)?;
        let expected = Stage::new(stage.data.clone());
        if expected.id != stage.id {
            return Err(Error::Invalid("stage ID does not match contents"));
        }
        if let Some(existing) = self.stages.get(&stage.id) {
            if existing != &stage {
                return Err(Error::Invalid("stage hash collision"));
            }
            return Ok(stage.id);
        }
        self.ensure_index_capacity("stage count", self.stages.len(), self.limits.max_stages)?;
        let id = stage.id;
        self.append_metadata_record(RecordType::Stage, id.0, stage.encode())?;
        self.stages.insert(id, stage);
        Ok(id)
    }

    /// Appends an ordered exchange and updates the trace/session indexes. A
    /// trace ID names exactly one exchange in an archive.
    pub fn append_exchange(&mut self, exchange: Exchange) -> Result<ExchangeId> {
        exchange.validate_shape(&self.limits)?;
        for stage_id in &exchange.data.stages {
            if !self.stages.contains_key(stage_id) {
                return Err(Error::Missing(stage_id.to_string()));
            }
        }
        let expected = Exchange::new(exchange.data.clone());
        if expected.id != exchange.id {
            return Err(Error::Invalid("exchange ID does not match contents"));
        }
        if let Some(existing_id) = self.traces.get(&exchange.data.trace_id) {
            if *existing_id != exchange.id {
                return Err(Error::Invalid("duplicate exchange trace ID"));
            }
        }
        if let Some(existing) = self.exchanges.get(&exchange.id) {
            if existing != &exchange {
                return Err(Error::Invalid("exchange hash collision"));
            }
            return Ok(exchange.id);
        }
        self.ensure_index_capacity(
            "exchange count",
            self.exchanges.len(),
            self.limits.max_exchanges,
        )?;
        if let Some(session_id) = &exchange.data.session_id {
            let current = self.sessions.get(session_id).map_or(0, Vec::len);
            self.ensure_index_capacity(
                "session exchange count",
                current,
                self.limits.max_session_exchanges,
            )?;
        }
        let id = exchange.id;
        self.append_metadata_record(RecordType::Exchange, id.0, exchange.encode())?;
        self.traces.insert(exchange.data.trace_id.clone(), id);
        if let Some(session_id) = &exchange.data.session_id {
            self.sessions
                .entry(session_id.clone())
                .or_default()
                .push(id);
        }
        self.exchanges.insert(id, exchange);
        Ok(id)
    }

    /// Append an exchange and its optional transport-metadata companion as
    /// adjacent standalone frames. Adjacency is the compatibility-safe index:
    /// old readers skip the optional frame, while new checkpoint/footer readers
    /// find it from the existing Exchange offset without a new index kind.
    pub fn append_exchange_with_metadata(
        &mut self,
        exchange: Exchange,
        data: ExchangeMetadataData,
    ) -> Result<ExchangeId> {
        let metadata = ExchangeMetadata::new(exchange.id, data);
        let payload = metadata.encode(&self.limits)?;
        if let Some(existing) = self.exchanges.get(&exchange.id) {
            if existing != &exchange {
                return Err(Error::Invalid("exchange hash collision"));
            }
            return match self.exchange_metadata.get(&exchange.id) {
                Some(existing_metadata) if existing_metadata == &metadata => Ok(exchange.id),
                Some(_) => Err(Error::Invalid("conflicting exchange metadata")),
                None => Err(Error::Invalid(
                    "exchange metadata must be appended atomically with its exchange",
                )),
            };
        }

        // Never put this extension inside MetadataPage: shipped v1 readers
        // reject unknown inner record types even when their flags are optional.
        self.flush_metadata_page()?;
        let batching = self.metadata_page_batching;
        self.metadata_page_batching = false;
        let exchange_result = self.append_exchange(exchange);
        self.metadata_page_batching = batching;
        let id = exchange_result?;
        self.append_optional_frame(RecordType::ExchangeMetadata, payload)?;
        self.exchange_metadata.insert(id, metadata);
        Ok(id)
    }

    /// Append one normalized conversation entry. The record contains only
    /// semantic labels and ranges into existing raw manifests; it has no field
    /// capable of embedding prompt, message, or tool-result bytes.
    pub fn append_conversation_entry(
        &mut self,
        entry: ConversationEntry,
    ) -> Result<ConversationEntryId> {
        self.append_conversation_entry_with_external_manifests(entry, &[])
    }

    /// Append an entry whose ranges may address archive-set manifests. The
    /// supplied lengths must come from a validated catalog and are used only
    /// for bounds checking; no raw bytes or manifests are copied here.
    pub fn append_conversation_entry_with_external_manifests(
        &mut self,
        entry: ConversationEntry,
        external_manifests: &[(ManifestId, u64)],
    ) -> Result<ConversationEntryId> {
        self.require_conversation_feature()?;
        if !external_manifests.is_empty()
            && self.header.required_feature_bits & REQUIRED_FEATURE_ARCHIVE_SET_BODY_REFS == 0
        {
            return Err(Error::Unsupported(
                "external conversation ranges require the archive-set-body-refs feature".into(),
            ));
        }
        entry.validate_shape(&self.limits)?;
        for range in &entry.data.raw_ranges {
            let body_length = self
                .manifests
                .get(&range.manifest_id)
                .map(|manifest| manifest.total_length)
                .or_else(|| {
                    external_manifests
                        .iter()
                        .find(|(id, _)| *id == range.manifest_id)
                        .map(|(_, length)| *length)
                })
                .ok_or_else(|| Error::Missing(range.manifest_id.to_string()))?;
            let end = range
                .byte_offset
                .checked_add(range.byte_length)
                .ok_or(Error::Invalid("conversation artifact range overflow"))?;
            if end > body_length {
                return Err(Error::Invalid(
                    "conversation artifact range exceeds manifest",
                ));
            }
        }
        let expected = ConversationEntry::new(entry.data.clone());
        if expected.id != entry.id {
            return Err(Error::Invalid(
                "conversation entry ID does not match contents",
            ));
        }
        if let Some(existing) = self.conversation_entries.get(&entry.id) {
            if existing != &entry {
                return Err(Error::Invalid("conversation entry hash collision"));
            }
            return Ok(entry.id);
        }
        self.ensure_index_capacity(
            "conversation entry count",
            self.conversation_entries.len(),
            self.limits.max_conversation_entries,
        )?;
        let id = entry.id;
        self.append_metadata_record(RecordType::ConversationEntry, id.0, entry.encode())?;
        self.conversation_entries.insert(id, entry);
        Ok(id)
    }

    pub fn append_generation(&mut self, generation: Generation) -> Result<GenerationId> {
        self.require_conversation_feature()?;
        generation.validate_shape(&self.limits)?;
        if let Some(parent) = generation.data.parent_generation_id {
            if !self.generations.contains_key(&parent) {
                return Err(Error::Missing(parent.to_string()));
            }
        }
        for entry in &generation.data.entries {
            if !self.conversation_entries.contains_key(entry) {
                return Err(Error::Missing(entry.to_string()));
            }
        }
        let expected = Generation::new(generation.data.clone());
        if expected.id != generation.id {
            return Err(Error::Invalid("generation ID does not match contents"));
        }
        if let Some(existing) = self.generations.get(&generation.id) {
            if existing != &generation {
                return Err(Error::Invalid("generation hash collision"));
            }
            return Ok(generation.id);
        }
        self.ensure_index_capacity(
            "generation count",
            self.generations.len(),
            self.limits.max_generations,
        )?;
        let id = generation.id;
        self.append_metadata_record(RecordType::Generation, id.0, generation.encode())?;
        self.generations.insert(id, generation);
        Ok(id)
    }

    pub fn append_turn_view(&mut self, turn: TurnView) -> Result<TurnViewId> {
        self.require_conversation_feature()?;
        turn.validate_shape(&self.limits)?;
        if !self.traces.contains_key(&turn.data.trace_id) {
            return Err(Error::Missing(
                String::from_utf8_lossy(&turn.data.trace_id).into_owned(),
            ));
        }
        let generation = self
            .generations
            .get(&turn.data.generation_id)
            .ok_or_else(|| Error::Missing(turn.data.generation_id.to_string()))?;
        if turn.data.upto_index >= generation.data.entries.len() as u64 {
            return Err(Error::Invalid(
                "turn view upto index exceeds generation entries",
            ));
        }
        for entry in &turn.data.response_entry_refs {
            if !self.conversation_entries.contains_key(entry) {
                return Err(Error::Missing(entry.to_string()));
            }
        }
        let expected = TurnView::new(turn.data.clone());
        if expected.id != turn.id {
            return Err(Error::Invalid("turn view ID does not match contents"));
        }
        if let Some(existing) = self.turn_traces.get(&turn.data.trace_id) {
            if *existing != turn.id {
                return Err(Error::Invalid("duplicate turn view trace ID"));
            }
        }
        if let Some(existing) = self.turn_views.get(&turn.id) {
            if existing != &turn {
                return Err(Error::Invalid("turn view hash collision"));
            }
            return Ok(turn.id);
        }
        self.ensure_index_capacity(
            "turn view count",
            self.turn_views.len(),
            self.limits.max_turn_views,
        )?;
        let id = turn.id;
        self.append_metadata_record(RecordType::TurnView, id.0, turn.encode())?;
        self.turn_traces.insert(turn.data.trace_id.clone(), id);
        self.turn_views.insert(id, turn);
        Ok(id)
    }

    fn require_conversation_feature(&self) -> Result<()> {
        if self.header.required_feature_bits & REQUIRED_FEATURE_CONVERSATION_DAG == 0 {
            return Err(Error::Unsupported(
                "conversation DAG records require the conversation-dag feature".into(),
            ));
        }
        Ok(())
    }

    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }
    pub fn chunk_uncompressed_bytes(&self) -> u64 {
        self.chunks
            .values()
            .map(|location| location.uncompressed_length)
            .sum()
    }
    pub fn manifest_count(&self) -> usize {
        self.manifests.len()
    }
    pub fn header_block_count(&self) -> usize {
        self.header_blocks.len()
    }
    pub fn header_block(&self, id: &HeaderBlockId) -> Option<&HeaderBlock> {
        self.header_blocks.get(id)
    }
    pub fn stream_index_count(&self) -> usize {
        self.stream_indexes.len()
    }
    pub fn stage_count(&self) -> usize {
        self.stages.len()
    }
    pub fn stage(&self, id: &StageId) -> Option<&Stage> {
        self.stages.get(id)
    }
    pub fn exchange_count(&self) -> usize {
        self.exchanges.len()
    }
    pub fn exchange_metadata(&self, id: &ExchangeId) -> Option<&ExchangeMetadata> {
        self.exchange_metadata.get(id)
    }
    pub fn conversation_entry_count(&self) -> usize {
        self.conversation_entries.len()
    }
    pub fn generation_count(&self) -> usize {
        self.generations.len()
    }
    pub fn turn_view_count(&self) -> usize {
        self.turn_views.len()
    }

    /// Append one pre-chunked byte range, or return the verified local record
    /// when this pack already contains it. Cross-pack deduplication is a
    /// catalog concern; this method intentionally never copies external data.
    pub fn append_chunk_record(&mut self, bytes: &[u8]) -> Result<ChunkRecordDescriptor> {
        if bytes.len() as u64 > self.limits.max_chunk_uncompressed {
            return Err(Error::Limit {
                what: "uncompressed chunk",
                actual: bytes.len() as u64,
                limit: self.limits.max_chunk_uncompressed,
            });
        }
        let hash = ChunkHash::blake3(bytes);
        if let Some(location) = self.chunks.get(&hash).cloned() {
            let existing = self
                .read_chunk_at(location.frame_offset)?
                .decompress(&self.limits)?;
            if existing != bytes {
                return Err(Error::Invalid("BLAKE3 chunk collision"));
            }
            return Ok(ChunkRecordDescriptor {
                hash,
                frame_offset: location.frame_offset,
                uncompressed_length: location.uncompressed_length,
                compressed_length: location.compressed_length,
            });
        }
        let chunk = StoredChunk::from_bytes(bytes, self.header.zstd_level as i32)?;
        let location = ChunkLocation {
            frame_offset: self.append_frame(RecordType::Chunk, chunk.encode())?,
            uncompressed_length: chunk.uncompressed_length,
            compressed_length: chunk.compressed.len() as u64,
        };
        self.chunks.insert(hash, location.clone());
        Ok(ChunkRecordDescriptor {
            hash,
            frame_offset: location.frame_offset,
            uncompressed_length: location.uncompressed_length,
            compressed_length: location.compressed_length,
        })
    }
    pub fn header(&self) -> &FileHeader {
        &self.header
    }

    /// Batch subsequently appended manifests, header blocks, stream indexes,
    /// stages, and exchanges into independently compressed canonical metadata
    /// pages. Existing callers retain immediate one-record durability unless
    /// they explicitly enable batching and use `flush`, `checkpoint`, or
    /// `seal` as their commit boundary.
    pub fn enable_metadata_pages(&mut self) {
        self.metadata_page_batching = true;
    }

    /// Persist a complete derived index snapshot and a fixed-size locator as
    /// ordinary append-only records. Active files remain appendable: a later
    /// append simply follows the locator, and a later checkpoint supersedes
    /// it without rewriting any bytes.
    pub fn checkpoint(&mut self) -> Result<CheckpointRecordDescriptor> {
        let pointer = self.write_checkpoint(true)?;
        self.io.flush()?;
        let append_offset = self.io.seek(SeekFrom::End(0))?;
        Ok(CheckpointRecordDescriptor {
            frame_offset: pointer.frame_offset,
            frame_length: pointer.frame_length,
            payload_hash: pointer.payload_hash,
            append_offset,
        })
    }

    /// Write a clean footer and trailing `LAR1END!` magic. Sealing is
    /// irreversible for this writer and `open_append` rejects sealed files.
    pub fn seal(&mut self) -> Result<()> {
        let pointer = self.write_checkpoint(false)?;
        self.io.write_all(&pointer.footer_trailer())?;
        self.io.flush()?;
        self.sealed = true;
        Ok(())
    }

    pub fn flush(&mut self) -> Result<()> {
        self.flush_metadata_page()?;
        self.io.flush()?;
        Ok(())
    }
    pub fn get_ref(&self) -> &W {
        &self.io
    }
    pub fn get_mut(&mut self) -> &mut W {
        &mut self.io
    }
    pub fn into_inner(mut self) -> Result<W> {
        self.flush_metadata_page()?;
        self.io.flush()?;
        Ok(self.io)
    }

    fn append_chunk(&mut self, bytes: &[u8]) -> Result<ChunkHash> {
        Ok(self.append_chunk_record(bytes)?.hash)
    }

    fn write_checkpoint(&mut self, write_locator: bool) -> Result<CheckpointPointer> {
        if self.sealed {
            return Err(Error::Invalid("sealed archive cannot be checkpointed"));
        }
        self.flush_metadata_page()?;
        let indexed_end = self.io.seek(SeekFrom::End(0))?;
        let indexed_record_count = self.record_count as u64;
        let mut refs = Vec::new();
        for block in self.index_blocks()? {
            let kind = block.kind;
            let payload = block.encode()?;
            let hash = payload_hash(&payload);
            let (offset, length) = self.append_derived_frame(RecordType::IndexBlock, payload)?;
            refs.push(IndexBlockRef {
                kind,
                frame_offset: offset,
                frame_length: length,
                payload_hash: hash,
            });
        }
        let checkpoint = Checkpoint {
            indexed_end,
            record_count: indexed_record_count,
            blocks: refs,
        };
        let payload = checkpoint.encode()?;
        let hash = payload_hash(&payload);
        let (frame_offset, frame_length) =
            self.append_derived_frame(RecordType::Checkpoint, payload)?;
        let pointer = CheckpointPointer {
            frame_offset,
            frame_length,
            payload_hash: hash,
        };
        if write_locator {
            let (_, length) = self
                .append_derived_frame(RecordType::CheckpointLocator, pointer.locator_payload())?;
            if length != LOCATOR_FRAME_LEN {
                return Err(Error::Invalid("checkpoint locator frame has wrong size"));
            }
        }
        Ok(pointer)
    }

    fn index_blocks(&self) -> Result<Vec<IndexBlock>> {
        let max_payload =
            usize::try_from(self.limits.max_frame_payload).map_err(|_| Error::Limit {
                what: "index block address space",
                actual: self.limits.max_frame_payload,
                limit: usize::MAX as u64,
            })?;
        const BLOCK_HEADER: usize = 12;
        if max_payload < BLOCK_HEADER + 64 {
            return Err(Error::Limit {
                what: "index block payload",
                actual: max_payload as u64,
                limit: (BLOCK_HEADER + 64) as u64,
            });
        }
        let mut blocks = Vec::new();

        let mut chunks: Vec<_> = self
            .chunks
            .iter()
            .map(|(hash, location)| ChunkIndexEntry {
                hash: chunk_hash_bytes(hash),
                frame_offset: location.frame_offset,
                uncompressed_length: location.uncompressed_length,
                compressed_length: location.compressed_length,
            })
            .collect();
        chunks.sort_by_key(|entry| entry.hash);
        let chunk_capacity = (max_payload - BLOCK_HEADER) / 57;
        for part in chunks.chunks(chunk_capacity.max(1)) {
            blocks.push(IndexBlock {
                kind: IndexKind::Chunk,
                entries: IndexEntries::Chunks(part.to_vec()),
            });
        }

        macro_rules! id_offset_blocks {
            ($kind:expr, $map:expr) => {{
                let mut entries: Vec<_> = $map.iter().map(|(id, offset)| (id.0, *offset)).collect();
                entries.sort_by_key(|entry| entry.0);
                let capacity = ((max_payload - BLOCK_HEADER) / 40).max(1);
                for part in entries.chunks(capacity) {
                    blocks.push(IndexBlock {
                        kind: $kind,
                        entries: IndexEntries::IdOffsets(part.to_vec()),
                    });
                }
            }};
        }
        id_offset_blocks!(IndexKind::Manifest, self.manifest_offsets);
        id_offset_blocks!(IndexKind::HeaderBlock, self.header_block_offsets);
        id_offset_blocks!(IndexKind::StreamIndex, self.stream_index_offsets);
        id_offset_blocks!(IndexKind::Stage, self.stage_offsets);
        id_offset_blocks!(IndexKind::Exchange, self.exchange_offsets);
        let mut traces: Vec<_> = self
            .traces
            .iter()
            .map(|(trace, exchange)| (trace.clone(), exchange.0))
            .collect();
        traces.sort_by(|left, right| left.0.cmp(&right.0));
        push_variable_blocks(
            &mut blocks,
            IndexKind::Trace,
            traces,
            max_payload,
            IndexEntries::Traces,
            |entry| 4usize.saturating_add(entry.0.len()).saturating_add(32),
        )?;

        let mut sessions: Vec<_> = self.sessions.iter().collect();
        sessions.sort_by(|left, right| left.0.cmp(right.0));
        let mut session_parts = Vec::new();
        for (session, exchanges) in sessions {
            let fixed = 4usize
                .checked_add(session.len())
                .and_then(|value| value.checked_add(8))
                .ok_or(Error::Invalid("session index size overflow"))?;
            if fixed + 32 > max_payload - BLOCK_HEADER {
                return Err(Error::Limit {
                    what: "session index entry",
                    actual: (fixed + 32) as u64,
                    limit: (max_payload - BLOCK_HEADER) as u64,
                });
            }
            let per_part = ((max_payload - BLOCK_HEADER - fixed) / 32).max(1);
            for (part_index, part) in exchanges.chunks(per_part).enumerate() {
                let start = part_index
                    .checked_mul(per_part)
                    .ok_or(Error::Invalid("session index offset overflow"))?;
                session_parts.push((
                    session.clone(),
                    u32::try_from(start)
                        .map_err(|_| Error::Invalid("session index offset exceeds u32"))?,
                    part.iter().map(|id| id.0).collect::<Vec<_>>(),
                ));
            }
        }
        push_variable_blocks(
            &mut blocks,
            IndexKind::Session,
            session_parts,
            max_payload,
            IndexEntries::Sessions,
            |entry| {
                4usize
                    .saturating_add(entry.0.len())
                    .saturating_add(8)
                    .saturating_add(entry.2.len().saturating_mul(32))
            },
        )?;
        id_offset_blocks!(
            IndexKind::ConversationEntry,
            self.conversation_entry_offsets
        );
        id_offset_blocks!(IndexKind::Generation, self.generation_offsets);
        id_offset_blocks!(IndexKind::TurnView, self.turn_view_offsets);

        let mut turn_traces: Vec<_> = self
            .turn_traces
            .iter()
            .map(|(trace, turn)| (trace.clone(), turn.0))
            .collect();
        turn_traces.sort_by(|left, right| left.0.cmp(&right.0));
        push_variable_blocks(
            &mut blocks,
            IndexKind::TurnTrace,
            turn_traces,
            max_payload,
            IndexEntries::Traces,
            |entry| 4usize.saturating_add(entry.0.len()).saturating_add(32),
        )?;
        Ok(blocks)
    }

    fn append_derived_frame(&mut self, kind: RecordType, payload: Vec<u8>) -> Result<(u64, u64)> {
        if payload.len() as u64 > self.limits.max_frame_payload {
            return Err(Error::Limit {
                what: "derived index payload",
                actual: payload.len() as u64,
                limit: self.limits.max_frame_payload,
            });
        }
        let offset = self.io.seek(SeekFrom::End(0))?;
        let length = RecordFrame {
            record_type: kind,
            schema_version: INDEX_SCHEMA_V1,
            flags: 0,
            payload,
            offset,
        }
        .write(&mut self.io)?;
        self.record_count = self
            .record_count
            .checked_add(1)
            .ok_or(Error::Invalid("record count overflow"))?;
        Ok((offset, length))
    }

    fn existing_body_id(
        &mut self,
        bytes: &[u8],
        media_type: Option<&[u8]>,
        content_encoding: Option<&[u8]>,
    ) -> Result<Option<ManifestId>> {
        let identity = (
            ChunkHash::blake3(bytes),
            bytes.len() as u64,
            media_type.map(Vec::from),
            content_encoding.map(Vec::from),
        );
        let Some(id) = self.body_identities.get(&identity).copied() else {
            return Ok(None);
        };
        let manifest = self
            .manifests
            .get(&id)
            .cloned()
            .ok_or_else(|| Error::Missing(id.to_string()))?;
        if self.read_manifest_bytes(&manifest)? != bytes {
            return Err(Error::Invalid("BLAKE3 whole-body collision"));
        }
        Ok(Some(id))
    }

    fn append_literal_refs(
        &mut self,
        bytes: &[u8],
        logical_offset: &mut u64,
        refs: &mut Vec<ChunkRef>,
    ) -> Result<()> {
        let mut chunker = StreamingChunker::new(self.chunker_config)?;
        chunker.push(bytes, |chunk| {
            let hash = self.append_chunk(chunk)?;
            push_chunk_ref(
                refs,
                ChunkRef {
                    chunk_hash: hash,
                    chunk_offset: 0,
                    logical_offset: *logical_offset,
                    length: chunk.len() as u64,
                },
            );
            *logical_offset += chunk.len() as u64;
            Ok(())
        })?;
        chunker.finish(|chunk| {
            let hash = self.append_chunk(chunk)?;
            push_chunk_ref(
                refs,
                ChunkRef {
                    chunk_hash: hash,
                    chunk_offset: 0,
                    logical_offset: *logical_offset,
                    length: chunk.len() as u64,
                },
            );
            *logical_offset += chunk.len() as u64;
            Ok(())
        })
    }

    fn append_copy_refs(
        &self,
        manifest: &BodyManifest,
        base_offset: u64,
        length: u64,
        logical_offset: &mut u64,
        refs: &mut Vec<ChunkRef>,
    ) -> Result<()> {
        let end = base_offset
            .checked_add(length)
            .ok_or(Error::Invalid("predecessor copy range overflow"))?;
        if end > manifest.total_length {
            return Err(Error::Invalid("predecessor copy range exceeds body"));
        }
        let mut covered = 0u64;
        for prior in &manifest.chunks {
            let prior_end = prior
                .logical_offset
                .checked_add(prior.length)
                .ok_or(Error::Invalid("predecessor chunk range overflow"))?;
            let start = base_offset.max(prior.logical_offset);
            let overlap_end = end.min(prior_end);
            if start >= overlap_end {
                continue;
            }
            let overlap = overlap_end - start;
            let stored = self
                .chunks
                .get(&prior.chunk_hash)
                .ok_or_else(|| Error::Missing(format!("chunk {:?}", prior.chunk_hash)))?;
            let chunk_offset = prior
                .chunk_offset
                .checked_add(start - prior.logical_offset)
                .ok_or(Error::Invalid("predecessor chunk range overflow"))?;
            let chunk_end = chunk_offset
                .checked_add(overlap)
                .ok_or(Error::Invalid("predecessor chunk range overflow"))?;
            if chunk_end > stored.uncompressed_length {
                return Err(Error::Invalid("predecessor range exceeds stored chunk"));
            }
            push_chunk_ref(
                refs,
                ChunkRef {
                    chunk_hash: prior.chunk_hash,
                    chunk_offset,
                    logical_offset: *logical_offset,
                    length: overlap,
                },
            );
            covered += overlap;
            *logical_offset += overlap;
        }
        if covered != length {
            return Err(Error::Invalid(
                "predecessor manifest does not cover requested copy range",
            ));
        }
        Ok(())
    }

    fn read_manifest_bytes(&mut self, manifest: &BodyManifest) -> Result<Vec<u8>> {
        manifest.validate()?;
        let capacity = usize::try_from(manifest.total_length).map_err(|_| Error::Limit {
            what: "predecessor body address space",
            actual: manifest.total_length,
            limit: usize::MAX as u64,
        })?;
        let mut body = Vec::with_capacity(capacity);
        for reference in &manifest.chunks {
            let location = self
                .chunks
                .get(&reference.chunk_hash)
                .cloned()
                .ok_or_else(|| Error::Missing(format!("chunk {:?}", reference.chunk_hash)))?;
            let chunk = self
                .read_chunk_at(location.frame_offset)?
                .decompress(&self.limits)?;
            let start = usize::try_from(reference.chunk_offset)
                .map_err(|_| Error::Invalid("predecessor chunk range overflow"))?;
            let end = usize::try_from(
                reference
                    .chunk_offset
                    .checked_add(reference.length)
                    .ok_or(Error::Invalid("predecessor chunk range overflow"))?,
            )
            .map_err(|_| Error::Invalid("predecessor chunk range overflow"))?;
            body.extend_from_slice(
                chunk
                    .get(start..end)
                    .ok_or(Error::Invalid("predecessor range exceeds chunk"))?,
            );
        }
        if body.len() as u64 != manifest.total_length
            || ChunkHash::blake3(&body) != manifest.whole_body_hash
        {
            return Err(Error::Invalid("predecessor body identity mismatch"));
        }
        Ok(body)
    }

    fn verify_manifest_body(&mut self, manifest: &BodyManifest) -> Result<()> {
        manifest.validate()?;
        let mut written = 0u64;
        let mut hasher = blake3::Hasher::new();
        for reference in &manifest.chunks {
            let location = self
                .chunks
                .get(&reference.chunk_hash)
                .cloned()
                .ok_or_else(|| Error::Missing(format!("chunk {:?}", reference.chunk_hash)))?;
            let chunk = self
                .read_chunk_at(location.frame_offset)?
                .decompress(&self.limits)?;
            let start = usize::try_from(reference.chunk_offset)
                .map_err(|_| Error::Invalid("manifest chunk range overflow"))?;
            let end = usize::try_from(
                reference
                    .chunk_offset
                    .checked_add(reference.length)
                    .ok_or(Error::Invalid("manifest chunk range overflow"))?,
            )
            .map_err(|_| Error::Invalid("manifest chunk range overflow"))?;
            let bytes = chunk
                .get(start..end)
                .ok_or(Error::Invalid("manifest range exceeds chunk"))?;
            hasher.update(bytes);
            written = written
                .checked_add(bytes.len() as u64)
                .ok_or(Error::Invalid("manifest body length overflow"))?;
        }
        if written != manifest.total_length
            || hasher.finalize().as_bytes() != &manifest.whole_body_hash.digest
        {
            return Err(Error::Invalid("manifest body identity mismatch"));
        }
        Ok(())
    }

    /// Append an already chunked manifest without changing its chunk ranges or
    /// logical identity. Every referenced chunk must already exist in this
    /// physical archive and is revalidated before the canonical manifest is
    /// published. This is used by graph-preserving archive-set operations such
    /// as repack; ordinary capture callers should prefer [`Self::append_body`]
    /// or [`Self::append_reader`].
    pub fn append_manifest_record(&mut self, manifest: BodyManifest) -> Result<ManifestId> {
        manifest.validate()?;
        let expected = BodyManifest::new(
            manifest.total_length,
            manifest.whole_body_hash,
            manifest.media_type.clone(),
            manifest.content_encoding.clone(),
            manifest.chunks.clone(),
        );
        if expected.id != manifest.id {
            return Err(Error::Invalid("manifest ID does not match contents"));
        }
        if manifest.chunks.len() > self.limits.max_manifest_chunks as usize {
            return Err(Error::Limit {
                what: "manifest chunk count",
                actual: manifest.chunks.len() as u64,
                limit: self.limits.max_manifest_chunks as u64,
            });
        }
        validate_manifest_references(&manifest, &self.chunks)?;
        if let Some(existing) = self.manifests.get(&manifest.id) {
            if existing != &manifest {
                return Err(Error::Invalid("manifest hash collision"));
            }
            return Ok(manifest.id);
        }
        // Exact-record insertion must not collapse distinct manifests that
        // reconstruct the same bytes through different chunk ranges. Their
        // IDs are part of Stage and ConversationEntry identities. Verify the
        // complete body before publishing the caller-supplied identity.
        self.verify_manifest_body(&manifest)?;
        let identity = (
            manifest.whole_body_hash,
            manifest.total_length,
            manifest.media_type.clone(),
            manifest.content_encoding.clone(),
        );
        let payload = manifest.encode();
        let id = manifest.id;
        self.append_metadata_record(RecordType::BodyManifest, id.0, payload)?;
        self.body_identities.entry(identity).or_insert(id);
        self.manifests.insert(id, manifest);
        Ok(id)
    }

    fn append_manifest(&mut self, manifest: BodyManifest) -> Result<ManifestId> {
        let identity = (
            manifest.whole_body_hash,
            manifest.total_length,
            manifest.media_type.clone(),
            manifest.content_encoding.clone(),
        );
        if let Some(existing_id) = self.body_identities.get(&identity).copied() {
            let existing = self
                .manifests
                .get(&existing_id)
                .cloned()
                .ok_or_else(|| Error::Missing(existing_id.to_string()))?;
            if self.read_manifest_bytes(&existing)? != self.read_manifest_bytes(&manifest)? {
                return Err(Error::Invalid("BLAKE3 whole-body collision"));
            }
            return Ok(existing_id);
        }
        self.append_manifest_record(manifest)
    }

    fn append_metadata_record(
        &mut self,
        record_type: RecordType,
        id: [u8; 32],
        payload: Vec<u8>,
    ) -> Result<()> {
        if payload.len() as u64 > self.limits.max_frame_payload {
            return Err(Error::Limit {
                what: "metadata record payload",
                actual: payload.len() as u64,
                limit: self.limits.max_frame_payload,
            });
        }
        if !self.metadata_page_batching {
            let offset = self.append_frame(record_type, payload)?;
            return self.record_metadata_offset(record_type, id, offset);
        }
        let framed_length = payload
            .len()
            .checked_add(24)
            .ok_or(Error::Invalid("metadata page size overflow"))?;
        if framed_length as u64 > self.limits.max_metadata_page_uncompressed {
            self.flush_metadata_page()?;
            let offset = self.append_frame(record_type, payload)?;
            return self.record_metadata_offset(record_type, id, offset);
        }
        if self.pending_metadata_bytes.saturating_add(framed_length) as u64
            > self.limits.max_metadata_page_uncompressed
        {
            self.flush_metadata_page()?;
        }
        self.pending_metadata_bytes = self
            .pending_metadata_bytes
            .checked_add(framed_length)
            .ok_or(Error::Invalid("metadata page size overflow"))?;
        self.pending_metadata.push(PendingMetadataRecord {
            record_type,
            id,
            payload,
        });
        if self.pending_metadata_bytes >= METADATA_PAGE_TARGET {
            self.flush_metadata_page()?;
        }
        Ok(())
    }

    fn flush_metadata_page(&mut self) -> Result<()> {
        if self.pending_metadata.is_empty() {
            return Ok(());
        }
        let mut inner = Vec::with_capacity(self.pending_metadata_bytes);
        for record in &self.pending_metadata {
            push_inner_frame(&mut inner, record.record_type, record.payload.clone())?;
        }
        if inner.len() as u64 > self.limits.max_metadata_page_uncompressed {
            return Err(Error::Limit {
                what: "metadata page uncompressed bytes",
                actual: inner.len() as u64,
                limit: self.limits.max_metadata_page_uncompressed,
            });
        }
        let count = u32::try_from(self.pending_metadata.len())
            .map_err(|_| Error::Invalid("metadata page record count exceeds u32"))?;
        let payload = if let Some(dictionary) = self.metadata_dictionary.as_ref() {
            MetadataPage::encode_with_dictionary(
                inner,
                count,
                self.header.zstd_level as i32,
                Some(dictionary),
            )?
        } else {
            MetadataPage::encode(inner, count, self.header.zstd_level as i32)?
        };
        let offset = self.append_frame(RecordType::MetadataPage, payload)?;
        let pending = std::mem::take(&mut self.pending_metadata);
        for record in pending {
            self.record_metadata_offset(record.record_type, record.id, offset)?;
        }
        self.pending_metadata_bytes = 0;
        Ok(())
    }

    fn record_metadata_offset(
        &mut self,
        record_type: RecordType,
        id: [u8; 32],
        offset: u64,
    ) -> Result<()> {
        let previous = match record_type {
            RecordType::BodyManifest => self.manifest_offsets.insert(ManifestId(id), offset),
            RecordType::HeaderBlock => self.header_block_offsets.insert(HeaderBlockId(id), offset),
            RecordType::StreamIndex => self.stream_index_offsets.insert(StreamIndexId(id), offset),
            RecordType::Stage => self.stage_offsets.insert(StageId(id), offset),
            RecordType::Exchange => self.exchange_offsets.insert(ExchangeId(id), offset),
            RecordType::ConversationEntry => self
                .conversation_entry_offsets
                .insert(ConversationEntryId(id), offset),
            RecordType::Generation => self.generation_offsets.insert(GenerationId(id), offset),
            RecordType::TurnView => self.turn_view_offsets.insert(TurnViewId(id), offset),
            _ => {
                return Err(Error::Invalid(
                    "non-metadata record queued in metadata page",
                ))
            }
        };
        if previous.is_some() {
            return Err(Error::Invalid("duplicate metadata record offset"));
        }
        Ok(())
    }

    fn append_frame(&mut self, kind: RecordType, payload: Vec<u8>) -> Result<u64> {
        self.append_frame_with_flags(kind, RecordFrame::REQUIRED, payload)
    }

    fn append_optional_frame(&mut self, kind: RecordType, payload: Vec<u8>) -> Result<u64> {
        self.append_frame_with_flags(kind, 0, payload)
    }

    fn append_frame_with_flags(
        &mut self,
        kind: RecordType,
        flags: u32,
        payload: Vec<u8>,
    ) -> Result<u64> {
        if self.sealed {
            return Err(Error::Invalid("sealed archive cannot be appended"));
        }
        if payload.len() as u64 > self.limits.max_frame_payload {
            return Err(Error::Limit {
                what: "record payload",
                actual: payload.len() as u64,
                limit: self.limits.max_frame_payload,
            });
        }
        let offset = self.io.seek(SeekFrom::End(0))?;
        RecordFrame {
            record_type: kind,
            schema_version: RECORD_SCHEMA_V1,
            flags,
            payload,
            offset,
        }
        .write(&mut self.io)?;
        self.record_count = self
            .record_count
            .checked_add(1)
            .ok_or(Error::Invalid("record count overflow"))?;
        Ok(offset)
    }

    fn read_chunk_at(&mut self, offset: u64) -> Result<StoredChunk> {
        let append_position = self.io.seek(SeekFrom::End(0))?;
        self.io.seek(SeekFrom::Start(offset))?;
        let result = {
            let mut reader = FrameReader::new(&mut self.io, &self.limits);
            match reader.read_next()? {
                (FrameRead::Frame, Some(frame)) if frame.record_type == RecordType::Chunk => {
                    StoredChunk::decode(&frame.payload, &self.limits)
                }
                _ => Err(Error::Invalid("chunk index points to non-chunk frame")),
            }
        };
        self.io.seek(SeekFrom::Start(append_position))?;
        result
    }

    fn ensure_index_capacity(&self, what: &'static str, current: usize, max: u32) -> Result<()> {
        let actual = current as u64 + 1;
        if actual > max as u64 {
            return Err(Error::Limit {
                what,
                actual,
                limit: max as u64,
            });
        }
        Ok(())
    }

    fn validate_stage_references(
        &self,
        stage: &Stage,
        external_manifests: &[ManifestId],
    ) -> Result<()> {
        for id in [
            stage.data.request_headers_ref,
            stage.data.response_headers_ref,
            stage.data.trailers_ref,
        ]
        .into_iter()
        .flatten()
        {
            if !self.header_blocks.contains_key(&id) {
                return Err(Error::Missing(id.to_string()));
            }
        }
        for id in [
            stage.data.request_body_manifest_ref,
            stage.data.response_body_manifest_ref,
        ]
        .into_iter()
        .flatten()
        {
            if !self.manifests.contains_key(&id) && !external_manifests.contains(&id) {
                return Err(Error::Missing(id.to_string()));
            }
        }
        if let Some(stream_id) = stage.data.stream_index_ref {
            let stream = self
                .stream_indexes
                .get(&stream_id)
                .ok_or_else(|| Error::Missing(stream_id.to_string()))?;
            if stage.data.response_body_manifest_ref != Some(stream.raw_body_manifest_id) {
                return Err(Error::Invalid(
                    "stream index and response body must reference the same manifest",
                ));
            }
        }
        Ok(())
    }
}

fn push_chunk_ref(refs: &mut Vec<ChunkRef>, next: ChunkRef) {
    if next.length == 0 {
        return;
    }
    if let Some(previous) = refs.last_mut() {
        if previous.chunk_hash == next.chunk_hash
            && previous.chunk_offset + previous.length == next.chunk_offset
            && previous.logical_offset + previous.length == next.logical_offset
        {
            previous.length += next.length;
            return;
        }
    }
    refs.push(next);
}

fn chunk_hash_bytes(hash: &ChunkHash) -> [u8; 33] {
    let mut bytes = [0; 33];
    bytes[0] = hash.algorithm as u8;
    bytes[1..].copy_from_slice(&hash.digest);
    bytes
}

fn chunk_hash_from_bytes(bytes: [u8; 33]) -> Result<ChunkHash> {
    Ok(ChunkHash {
        algorithm: crate::HashAlgorithm::try_from(bytes[0])?,
        digest: bytes[1..].try_into().unwrap(),
    })
}

fn push_variable_blocks<T, F, S>(
    blocks: &mut Vec<IndexBlock>,
    kind: IndexKind,
    entries: Vec<T>,
    max_payload: usize,
    wrap: F,
    size: S,
) -> Result<()>
where
    T: Clone,
    F: Fn(Vec<T>) -> IndexEntries,
    S: Fn(&T) -> usize,
{
    const BLOCK_HEADER: usize = 12;
    let available = max_payload.saturating_sub(BLOCK_HEADER);
    let mut current = Vec::new();
    let mut current_size = 0usize;
    for entry in entries {
        let entry_size = size(&entry);
        if entry_size > available {
            return Err(Error::Limit {
                what: "variable index entry",
                actual: entry_size as u64,
                limit: available as u64,
            });
        }
        if !current.is_empty() && current_size.saturating_add(entry_size) > available {
            blocks.push(IndexBlock {
                kind,
                entries: wrap(std::mem::take(&mut current)),
            });
            current_size = 0;
        }
        current_size = current_size
            .checked_add(entry_size)
            .ok_or(Error::Invalid("index block size overflow"))?;
        current.push(entry);
    }
    if !current.is_empty() {
        blocks.push(IndexBlock {
            kind,
            entries: wrap(current),
        });
    }
    Ok(())
}

fn validate_manifest_references(
    manifest: &BodyManifest,
    chunks: &HashMap<ChunkHash, ChunkLocation>,
) -> Result<()> {
    for reference in &manifest.chunks {
        let location = chunks
            .get(&reference.chunk_hash)
            .ok_or_else(|| Error::Missing(format!("chunk {:?}", reference.chunk_hash)))?;
        let end = reference
            .chunk_offset
            .checked_add(reference.length)
            .ok_or(Error::Invalid("manifest chunk range overflow"))?;
        if end > location.uncompressed_length {
            return Err(Error::Invalid("manifest range exceeds chunk"));
        }
    }
    Ok(())
}

fn untrusted_footer_pointer(trailer: &[u8]) -> Option<CheckpointPointer> {
    if trailer.len() != FOOTER_TRAILER_LEN as usize {
        return None;
    }
    Some(CheckpointPointer {
        frame_offset: u64::from_le_bytes(trailer[12..20].try_into().ok()?),
        frame_length: u64::from_le_bytes(trailer[20..28].try_into().ok()?),
        payload_hash: trailer[28..60].try_into().ok()?,
    })
}

fn read_frame_at<R: Read + Seek>(
    io: &mut R,
    limits: &Limits,
    offset: u64,
) -> Result<(RecordFrame, u64)> {
    io.seek(SeekFrom::Start(offset))?;
    let frame = {
        let mut reader = FrameReader::new(io, limits);
        match reader.read_next()? {
            (FrameRead::Frame, Some(frame)) => frame,
            _ => return Err(Error::Invalid("index points to an incomplete frame")),
        }
    };
    let end = io.stream_position()?;
    Ok((frame, end - offset))
}

#[derive(Clone, Debug)]
struct IndexedMetadataRecords {
    from_page: bool,
    frames: Vec<RecordFrame>,
}

fn read_indexed_metadata_payload<R: Read + Seek>(
    io: &mut R,
    limits: &Limits,
    dictionaries: &HashMap<[u8; 32], Vec<u8>>,
    cache: &mut HashMap<u64, IndexedMetadataRecords>,
    offset: u64,
    expected: RecordType,
    expected_id: [u8; 32],
) -> Result<Vec<u8>> {
    if !cache.contains_key(&offset) {
        let (frame, _) = read_frame_at(io, limits, offset)?;
        if frame.schema_version != RECORD_SCHEMA_V1 || frame.flags & RecordFrame::REQUIRED == 0 {
            return Err(Error::Invalid(
                "persisted metadata index points to an unsupported record",
            ));
        }
        let (from_page, frames) = if frame.record_type == RecordType::MetadataPage {
            let page = if dictionaries.is_empty() {
                MetadataPage::decode(&frame.payload, limits)?
            } else {
                MetadataPage::decode_with_dictionaries(&frame.payload, limits, dictionaries)?
            };
            let frames = page.frames(limits)?;
            for inner in &frames {
                if inner.schema_version != RECORD_SCHEMA_V1
                    || inner.flags & RecordFrame::REQUIRED == 0
                    || inner.payload.len() < 32
                {
                    return Err(Error::Invalid(
                        "metadata page contains an unsupported canonical record",
                    ));
                }
            }
            (true, frames)
        } else if matches!(
            frame.record_type,
            RecordType::BodyManifest
                | RecordType::HeaderBlock
                | RecordType::StreamIndex
                | RecordType::Stage
                | RecordType::Exchange
                | RecordType::ConversationEntry
                | RecordType::Generation
                | RecordType::TurnView
        ) {
            (false, vec![frame])
        } else {
            return Err(Error::Invalid(
                "persisted metadata index points to the wrong record",
            ));
        };
        cache.insert(offset, IndexedMetadataRecords { from_page, frames });
    }
    let records = cache.get(&offset).expect("indexed metadata was cached");
    let mut matches = records.frames.iter().filter(|frame| {
        frame.record_type == expected && frame.payload.get(..32) == Some(expected_id.as_slice())
    });
    let payload = matches
        .next()
        .ok_or(Error::Invalid(
            "metadata record is missing from indexed page",
        ))?
        .payload
        .clone();
    if matches.next().is_some() {
        return Err(Error::Invalid("duplicate metadata record in indexed page"));
    }
    Ok(payload)
}

fn read_adjacent_exchange_metadata<R: Read + Seek>(
    io: &mut R,
    limits: &Limits,
    exchange_offset: u64,
    indexed_end: u64,
    expected_id: ExchangeId,
) -> Result<Option<ExchangeMetadata>> {
    let (exchange_frame, exchange_length) = read_frame_at(io, limits, exchange_offset)?;
    if exchange_frame.record_type == RecordType::MetadataPage {
        // The composite writer always forces its Exchange out of a page. A
        // paged exchange therefore has no companion and must not be probed
        // past the page boundary.
        return Ok(None);
    }
    if exchange_frame.record_type != RecordType::Exchange
        || exchange_frame.schema_version != RECORD_SCHEMA_V1
        || exchange_frame.flags != RecordFrame::REQUIRED
    {
        return Err(Error::Invalid(
            "exchange index does not point to a canonical exchange frame",
        ));
    }
    let adjacent_offset = exchange_offset
        .checked_add(exchange_length)
        .ok_or(Error::Invalid("exchange companion offset overflow"))?;
    if adjacent_offset >= indexed_end {
        return Ok(None);
    }
    let (frame, _) = read_frame_at(io, limits, adjacent_offset)?;
    if frame.record_type != RecordType::ExchangeMetadata {
        return Ok(None);
    }
    if frame.flags & RecordFrame::REQUIRED != 0 {
        return Err(Error::Unsupported(
            "required exchange metadata companion is unsupported".into(),
        ));
    }
    if frame.schema_version != RECORD_SCHEMA_V1 || frame.flags != 0 {
        // Optional future schema/flag combinations retain archive
        // readability. Upgrade copies the opaque frame byte-for-byte.
        return Ok(None);
    }
    let metadata = ExchangeMetadata::decode(&frame.payload, limits)?;
    if metadata.exchange_id != expected_id {
        return Err(Error::Invalid(
            "exchange metadata companion identity mismatch",
        ));
    }
    Ok(Some(metadata))
}

fn indexed_metadata_inner_order(
    cache: &HashMap<u64, IndexedMetadataRecords>,
    offset: u64,
    expected: RecordType,
    expected_id: [u8; 32],
) -> Result<usize> {
    let records = cache
        .get(&offset)
        .ok_or(Error::Invalid("indexed metadata was not cached"))?;
    let mut matches = records.frames.iter().enumerate().filter(|(_, frame)| {
        frame.record_type == expected && frame.payload.get(..32) == Some(expected_id.as_slice())
    });
    let (inner_order, _) = matches.next().ok_or(Error::Invalid(
        "metadata record is missing from indexed page",
    ))?;
    if matches.next().is_some() {
        return Err(Error::Invalid("duplicate metadata record in indexed page"));
    }
    Ok(inner_order)
}

fn validate_indexed_metadata_page_coverage(
    cache: &HashMap<u64, IndexedMetadataRecords>,
    manifest_offsets: &HashMap<ManifestId, u64>,
    header_block_offsets: &HashMap<HeaderBlockId, u64>,
    stream_index_offsets: &HashMap<StreamIndexId, u64>,
    stage_offsets: &HashMap<StageId, u64>,
    exchange_offsets: &HashMap<ExchangeId, u64>,
    conversation_entry_offsets: &HashMap<ConversationEntryId, u64>,
    generation_offsets: &HashMap<GenerationId, u64>,
    turn_view_offsets: &HashMap<TurnViewId, u64>,
) -> Result<()> {
    for (offset, records) in cache.iter().filter(|(_, records)| records.from_page) {
        for frame in &records.frames {
            let id: [u8; 32] = frame.payload[..32].try_into().unwrap();
            let indexed_offset = match frame.record_type {
                RecordType::BodyManifest => manifest_offsets.get(&ManifestId(id)),
                RecordType::HeaderBlock => header_block_offsets.get(&HeaderBlockId(id)),
                RecordType::StreamIndex => stream_index_offsets.get(&StreamIndexId(id)),
                RecordType::Stage => stage_offsets.get(&StageId(id)),
                RecordType::Exchange => exchange_offsets.get(&ExchangeId(id)),
                RecordType::ConversationEntry => {
                    conversation_entry_offsets.get(&ConversationEntryId(id))
                }
                RecordType::Generation => generation_offsets.get(&GenerationId(id)),
                RecordType::TurnView => turn_view_offsets.get(&TurnViewId(id)),
                _ => unreachable!("MetadataPage::frames filters record types"),
            };
            if indexed_offset != Some(offset) {
                return Err(Error::Invalid(
                    "persisted index does not cover every metadata page record",
                ));
            }
        }
    }
    Ok(())
}

fn validate_indexed_record_offset(offset: u64, start: u64, end: u64) -> Result<()> {
    if offset < start || offset >= end {
        return Err(Error::Invalid("persisted record offset is out of range"));
    }
    Ok(())
}

fn ensure_loaded_count(what: &'static str, count: usize, max: u32) -> Result<()> {
    if count as u64 > max as u64 {
        return Err(Error::Limit {
            what,
            actual: count as u64,
            limit: max as u64,
        });
    }
    Ok(())
}

fn sorted_offsets<K>(map: &HashMap<K, u64>) -> Vec<(K, u64)>
where
    K: Copy + Eq + std::hash::Hash,
{
    let mut entries: Vec<_> = map.iter().map(|(id, offset)| (*id, *offset)).collect();
    entries.sort_by_key(|entry| entry.1);
    entries
}

struct LoadedIndex {
    checkpoint: Checkpoint,
    chunks: HashMap<ChunkHash, ChunkLocation>,
    manifests: HashMap<ManifestId, BodyManifest>,
    manifest_offsets: HashMap<ManifestId, u64>,
    body_identities: HashMap<BodyIdentity, ManifestId>,
    header_blocks: HashMap<HeaderBlockId, HeaderBlock>,
    header_block_offsets: HashMap<HeaderBlockId, u64>,
    stream_indexes: HashMap<StreamIndexId, StreamIndex>,
    stream_index_offsets: HashMap<StreamIndexId, u64>,
    stages: HashMap<StageId, Stage>,
    stage_offsets: HashMap<StageId, u64>,
    exchanges: HashMap<ExchangeId, Exchange>,
    exchange_offsets: HashMap<ExchangeId, u64>,
    exchange_metadata: HashMap<ExchangeId, ExchangeMetadata>,
    traces: HashMap<Vec<u8>, ExchangeId>,
    sessions: HashMap<Vec<u8>, Vec<ExchangeId>>,
    conversation_entries: HashMap<ConversationEntryId, ConversationEntry>,
    conversation_entry_offsets: HashMap<ConversationEntryId, u64>,
    generations: HashMap<GenerationId, Generation>,
    generation_offsets: HashMap<GenerationId, u64>,
    turn_views: HashMap<TurnViewId, TurnView>,
    turn_view_offsets: HashMap<TurnViewId, u64>,
    turn_traces: HashMap<Vec<u8>, TurnViewId>,
}

/// Indexed reader rebuilt by a bounded forward scan. It remains usable when
/// the final record was interrupted; all complete preceding records survive.
pub struct ArchiveReader<R> {
    io: R,
    header: FileHeader,
    limits: Limits,
    data_offset: u64,
    recovery: RecoveryStatus,
    record_count: usize,
    open_path: OpenPath,
    sealed: bool,
    chunks: HashMap<ChunkHash, ChunkLocation>,
    manifests: HashMap<ManifestId, BodyManifest>,
    manifest_offsets: HashMap<ManifestId, u64>,
    body_identities: HashMap<BodyIdentity, ManifestId>,
    header_blocks: HashMap<HeaderBlockId, HeaderBlock>,
    header_block_offsets: HashMap<HeaderBlockId, u64>,
    stream_indexes: HashMap<StreamIndexId, StreamIndex>,
    stream_index_offsets: HashMap<StreamIndexId, u64>,
    stages: HashMap<StageId, Stage>,
    stage_offsets: HashMap<StageId, u64>,
    exchanges: HashMap<ExchangeId, Exchange>,
    exchange_offsets: HashMap<ExchangeId, u64>,
    exchange_metadata: HashMap<ExchangeId, ExchangeMetadata>,
    traces: HashMap<Vec<u8>, ExchangeId>,
    sessions: HashMap<Vec<u8>, Vec<ExchangeId>>,
    conversation_entries: HashMap<ConversationEntryId, ConversationEntry>,
    conversation_entry_offsets: HashMap<ConversationEntryId, u64>,
    generations: HashMap<GenerationId, Generation>,
    generation_offsets: HashMap<GenerationId, u64>,
    turn_views: HashMap<TurnViewId, TurnView>,
    turn_view_offsets: HashMap<TurnViewId, u64>,
    turn_traces: HashMap<Vec<u8>, TurnViewId>,
}

impl<R: Read + Seek> ArchiveReader<R> {
    pub fn open(mut io: R, limits: Limits) -> Result<Self> {
        let end = file_length(&mut io)?;

        // A sealed footer has an unmistakable fixed trailer and is always the
        // preferred path. If either magic survives but its checksum/index is
        // corrupt, recover only canonical records and report a non-clean
        // status instead of pretending the bad footer is a normal EOF.
        if end >= FOOTER_TRAILER_LEN {
            let mut trailer = vec![0; FOOTER_TRAILER_LEN as usize];
            io.seek(SeekFrom::Start(end - FOOTER_TRAILER_LEN))?;
            io.read_exact(&mut trailer)?;
            let footer_candidate =
                &trailer[..8] == b"LARFOOT1" || CheckpointPointer::has_trailing_magic(&trailer);
            if footer_candidate {
                if let Ok(pointer) = CheckpointPointer::from_footer_trailer(&trailer) {
                    io.seek(SeekFrom::Start(0))?;
                    let (header, data_offset) = read_file_header(&mut io, &limits)?;
                    if let Ok(loaded) = Self::load_index(
                        &mut io,
                        &limits,
                        data_offset,
                        pointer,
                        header.required_feature_bits,
                    ) {
                        return Self::from_loaded_index(
                            io,
                            header,
                            data_offset,
                            limits,
                            loaded,
                            OpenPath::Footer,
                            true,
                            0,
                        );
                    }
                    let scan_end = Self::untrusted_indexed_end(&mut io, &limits, pointer)
                        .unwrap_or(pointer.frame_offset)
                        .min(end - FOOTER_TRAILER_LEN);
                    return Self::open_forward(
                        io,
                        limits,
                        Some(scan_end),
                        Some(RecoveryStatus::CorruptIndexFallback {
                            last_valid_offset: scan_end,
                            tail_bytes: end.saturating_sub(scan_end),
                        }),
                    );
                }

                let pointer = untrusted_footer_pointer(&trailer);
                let scan_end = pointer
                    .and_then(|pointer| Self::untrusted_indexed_end(&mut io, &limits, pointer))
                    .unwrap_or(end - FOOTER_TRAILER_LEN)
                    .min(end - FOOTER_TRAILER_LEN);
                return Self::open_forward(
                    io,
                    limits,
                    Some(scan_end),
                    Some(RecoveryStatus::CorruptIndexFallback {
                        last_valid_offset: scan_end,
                        tail_bytes: end.saturating_sub(scan_end),
                    }),
                );
            }
        }

        // Active checkpoints end in one fixed-size ordinary LREC frame. The
        // locator can remain in the middle of the stream after later appends;
        // only a locator at the current EOF is authoritative.
        if end >= LOCATOR_FRAME_LEN {
            let locator_offset = end - LOCATOR_FRAME_LEN;
            io.seek(SeekFrom::Start(locator_offset))?;
            let mut raw = vec![0; LOCATOR_FRAME_LEN as usize];
            io.read_exact(&mut raw)?;
            let locator_candidate = &raw[..4] == b"LREC"
                && u16::from_le_bytes(raw[4..6].try_into().unwrap())
                    == RecordType::CheckpointLocator.code();
            if locator_candidate {
                io.seek(SeekFrom::Start(locator_offset))?;
                let parsed = {
                    let mut reader = FrameReader::new(&mut io, &limits);
                    reader.read_next()
                };
                if let Ok((FrameRead::Frame, Some(frame))) = parsed {
                    if frame.record_type == RecordType::CheckpointLocator
                        && frame.schema_version == INDEX_SCHEMA_V1
                        && frame.flags == 0
                    {
                        if let Ok(pointer) = CheckpointPointer::from_locator_payload(&frame.payload)
                        {
                            io.seek(SeekFrom::Start(0))?;
                            let (header, data_offset) = read_file_header(&mut io, &limits)?;
                            if let Ok(loaded) = Self::load_index(
                                &mut io,
                                &limits,
                                data_offset,
                                pointer,
                                header.required_feature_bits,
                            ) {
                                return Self::from_loaded_index(
                                    io,
                                    header,
                                    data_offset,
                                    limits,
                                    loaded,
                                    OpenPath::Checkpoint,
                                    false,
                                    1,
                                );
                            }
                        }
                    }
                }

                let pointer = CheckpointPointer::from_locator_payload(
                    &raw[20..20 + crate::index::LOCATOR_PAYLOAD_LEN],
                )
                .ok();
                let scan_end = pointer
                    .and_then(|pointer| Self::untrusted_indexed_end(&mut io, &limits, pointer))
                    .unwrap_or(locator_offset)
                    .min(locator_offset);
                return Self::open_forward(
                    io,
                    limits,
                    Some(scan_end),
                    Some(RecoveryStatus::CorruptIndexFallback {
                        last_valid_offset: scan_end,
                        tail_bytes: end.saturating_sub(scan_end),
                    }),
                );
            }
        }

        Self::open_forward(io, limits, None, None)
    }

    #[allow(clippy::too_many_arguments)]
    fn from_loaded_index(
        io: R,
        header: FileHeader,
        data_offset: u64,
        limits: Limits,
        loaded: LoadedIndex,
        open_path: OpenPath,
        sealed: bool,
        locator_records: usize,
    ) -> Result<Self> {
        let record_count = usize::try_from(loaded.checkpoint.record_count)
            .map_err(|_| Error::Invalid("checkpoint record count exceeds address space"))?
            .checked_add(loaded.checkpoint.blocks.len())
            .and_then(|value| value.checked_add(1 + locator_records))
            .ok_or(Error::Invalid("checkpoint record count overflow"))?;
        Ok(Self {
            io,
            header,
            limits,
            data_offset,
            recovery: RecoveryStatus::Clean,
            record_count,
            open_path,
            sealed,
            chunks: loaded.chunks,
            manifests: loaded.manifests,
            manifest_offsets: loaded.manifest_offsets,
            body_identities: loaded.body_identities,
            header_blocks: loaded.header_blocks,
            header_block_offsets: loaded.header_block_offsets,
            stream_indexes: loaded.stream_indexes,
            stream_index_offsets: loaded.stream_index_offsets,
            stages: loaded.stages,
            stage_offsets: loaded.stage_offsets,
            exchanges: loaded.exchanges,
            exchange_offsets: loaded.exchange_offsets,
            exchange_metadata: loaded.exchange_metadata,
            traces: loaded.traces,
            sessions: loaded.sessions,
            conversation_entries: loaded.conversation_entries,
            conversation_entry_offsets: loaded.conversation_entry_offsets,
            generations: loaded.generations,
            generation_offsets: loaded.generation_offsets,
            turn_views: loaded.turn_views,
            turn_view_offsets: loaded.turn_view_offsets,
            turn_traces: loaded.turn_traces,
        })
    }

    fn load_index(
        io: &mut R,
        limits: &Limits,
        data_offset: u64,
        pointer: CheckpointPointer,
        required_feature_bits: u64,
    ) -> Result<LoadedIndex> {
        let allow_external_manifests =
            required_feature_bits & REQUIRED_FEATURE_ARCHIVE_SET_BODY_REFS != 0;
        let (frame, frame_length) = read_frame_at(io, limits, pointer.frame_offset)?;
        if frame_length != pointer.frame_length
            || frame.record_type != RecordType::Checkpoint
            || frame.schema_version != INDEX_SCHEMA_V1
            || frame.flags != 0
            || payload_hash(&frame.payload) != pointer.payload_hash
        {
            return Err(Error::Invalid("checkpoint pointer identity mismatch"));
        }
        let max_blocks = u32::try_from(
            (limits.max_frame_payload / 52)
                .saturating_add(1)
                .min(u32::MAX as u64),
        )
        .unwrap();
        let checkpoint = Checkpoint::decode(&frame.payload, max_blocks)?;
        if checkpoint.indexed_end < data_offset || checkpoint.indexed_end > pointer.frame_offset {
            return Err(Error::Invalid("checkpoint indexed range is invalid"));
        }

        // Self-contained dictionaries are emitted contiguously immediately
        // after the header, so a fast open needs only a bounded prefix probe.
        let mut dictionaries = HashMap::new();
        let mut dictionary_offset = data_offset;
        for _ in 0..limits.max_dictionaries {
            if dictionary_offset >= checkpoint.indexed_end {
                break;
            }
            let (dictionary_frame, length) = read_frame_at(io, limits, dictionary_offset)?;
            if dictionary_frame.record_type != RecordType::DictionaryData {
                break;
            }
            let dictionary = StoredDictionary::decode(&dictionary_frame.payload, limits)?;
            if dictionaries
                .insert(dictionary.id, dictionary.bytes)
                .is_some()
            {
                return Err(Error::Invalid("duplicate dictionary record"));
            }
            dictionary_offset = dictionary_offset
                .checked_add(length)
                .ok_or(Error::Invalid("dictionary record range overflow"))?;
        }

        let mut chunks = HashMap::new();
        let mut manifest_offsets = HashMap::new();
        let mut header_block_offsets = HashMap::new();
        let mut stream_index_offsets = HashMap::new();
        let mut stage_offsets = HashMap::new();
        let mut exchange_offsets = HashMap::new();
        let mut conversation_entry_offsets = HashMap::new();
        let mut generation_offsets = HashMap::new();
        let mut turn_view_offsets = HashMap::new();
        let mut traces = HashMap::new();
        let mut turn_traces = HashMap::new();
        let mut session_parts = Vec::new();
        let mut previous_block_end = checkpoint.indexed_end;
        let max_entries = limits
            .max_stages
            .max(limits.max_exchanges)
            .max(limits.max_stream_indexes)
            .max(limits.max_manifest_chunks)
            .max(limits.max_conversation_entries)
            .max(limits.max_generations)
            .max(limits.max_turn_views)
            .max(limits.max_generation_entries);
        let mut previous_kind = None;
        for reference in &checkpoint.blocks {
            if previous_kind.is_some_and(|kind| kind > reference.kind) {
                return Err(Error::Invalid(
                    "checkpoint blocks are not canonically ordered",
                ));
            }
            previous_kind = Some(reference.kind);
            let block_end = reference
                .frame_offset
                .checked_add(reference.frame_length)
                .ok_or(Error::Invalid("index block range overflow"))?;
            if reference.frame_offset < previous_block_end || block_end > pointer.frame_offset {
                return Err(Error::Invalid("index block range is invalid"));
            }
            let (block_frame, block_length) = read_frame_at(io, limits, reference.frame_offset)?;
            if block_length != reference.frame_length
                || block_frame.record_type != RecordType::IndexBlock
                || block_frame.schema_version != INDEX_SCHEMA_V1
                || block_frame.flags != 0
                || payload_hash(&block_frame.payload) != reference.payload_hash
            {
                return Err(Error::Invalid("index block pointer identity mismatch"));
            }
            let block = IndexBlock::decode(
                &block_frame.payload,
                max_entries,
                limits.max_identifier_length,
            )?;
            if block.kind != reference.kind {
                return Err(Error::Invalid("index block kind mismatch"));
            }
            match block.entries {
                IndexEntries::Chunks(entries) => {
                    for entry in entries {
                        let hash = chunk_hash_from_bytes(entry.hash)?;
                        validate_indexed_record_offset(
                            entry.frame_offset,
                            data_offset,
                            checkpoint.indexed_end,
                        )?;
                        if entry.uncompressed_length > limits.max_chunk_uncompressed
                            || entry.compressed_length > limits.max_frame_payload
                        {
                            return Err(Error::Invalid("chunk index length is out of bounds"));
                        }
                        if chunks
                            .insert(
                                hash,
                                ChunkLocation {
                                    frame_offset: entry.frame_offset,
                                    uncompressed_length: entry.uncompressed_length,
                                    compressed_length: entry.compressed_length,
                                },
                            )
                            .is_some()
                        {
                            return Err(Error::Invalid("duplicate chunk index entry"));
                        }
                    }
                }
                IndexEntries::IdOffsets(entries) => {
                    for (id, offset) in entries {
                        validate_indexed_record_offset(
                            offset,
                            data_offset,
                            checkpoint.indexed_end,
                        )?;
                        let duplicate = match block.kind {
                            IndexKind::Manifest => {
                                manifest_offsets.insert(ManifestId(id), offset).is_some()
                            }
                            IndexKind::HeaderBlock => header_block_offsets
                                .insert(HeaderBlockId(id), offset)
                                .is_some(),
                            IndexKind::StreamIndex => stream_index_offsets
                                .insert(StreamIndexId(id), offset)
                                .is_some(),
                            IndexKind::Stage => stage_offsets.insert(StageId(id), offset).is_some(),
                            IndexKind::Exchange => {
                                exchange_offsets.insert(ExchangeId(id), offset).is_some()
                            }
                            IndexKind::ConversationEntry => conversation_entry_offsets
                                .insert(ConversationEntryId(id), offset)
                                .is_some(),
                            IndexKind::Generation => generation_offsets
                                .insert(GenerationId(id), offset)
                                .is_some(),
                            IndexKind::TurnView => {
                                turn_view_offsets.insert(TurnViewId(id), offset).is_some()
                            }
                            _ => return Err(Error::Invalid("invalid ID-offset index kind")),
                        };
                        if duplicate {
                            return Err(Error::Invalid("duplicate ID-offset index entry"));
                        }
                    }
                }
                IndexEntries::Traces(entries) => {
                    for (trace, id) in entries {
                        let duplicate = match block.kind {
                            IndexKind::Trace => traces.insert(trace, ExchangeId(id)).is_some(),
                            IndexKind::TurnTrace => {
                                turn_traces.insert(trace, TurnViewId(id)).is_some()
                            }
                            _ => return Err(Error::Invalid("invalid trace index kind")),
                        };
                        if duplicate {
                            return Err(Error::Invalid("duplicate trace index entry"));
                        }
                    }
                }
                IndexEntries::Sessions(entries) => session_parts.extend(entries),
            }
            previous_block_end = block_end;
        }

        ensure_loaded_count(
            "stream index count",
            stream_index_offsets.len(),
            limits.max_stream_indexes,
        )?;
        ensure_loaded_count("stage count", stage_offsets.len(), limits.max_stages)?;
        ensure_loaded_count(
            "exchange count",
            exchange_offsets.len(),
            limits.max_exchanges,
        )?;
        ensure_loaded_count(
            "conversation entry count",
            conversation_entry_offsets.len(),
            limits.max_conversation_entries,
        )?;
        ensure_loaded_count(
            "generation count",
            generation_offsets.len(),
            limits.max_generations,
        )?;
        ensure_loaded_count(
            "turn view count",
            turn_view_offsets.len(),
            limits.max_turn_views,
        )?;
        if !conversation_entry_offsets.is_empty()
            || !generation_offsets.is_empty()
            || !turn_view_offsets.is_empty()
        {
            require_conversation_feature_bits(required_feature_bits)?;
        }

        let mut metadata_cache = HashMap::new();
        let mut manifests = HashMap::new();
        let mut body_identities = HashMap::new();
        for (id, offset) in sorted_offsets(&manifest_offsets) {
            let payload = read_indexed_metadata_payload(
                io,
                limits,
                &dictionaries,
                &mut metadata_cache,
                offset,
                RecordType::BodyManifest,
                id.0,
            )?;
            let value = BodyManifest::decode(&payload, limits)?;
            if value.id != id {
                return Err(Error::Invalid("manifest index identity mismatch"));
            }
            validate_manifest_references(&value, &chunks)?;
            body_identities
                .entry((
                    value.whole_body_hash,
                    value.total_length,
                    value.media_type.clone(),
                    value.content_encoding.clone(),
                ))
                .or_insert(value.id);
            manifests.insert(value.id, value);
        }

        let mut header_blocks = HashMap::new();
        for (id, offset) in sorted_offsets(&header_block_offsets) {
            let payload = read_indexed_metadata_payload(
                io,
                limits,
                &dictionaries,
                &mut metadata_cache,
                offset,
                RecordType::HeaderBlock,
                id.0,
            )?;
            let value = HeaderBlock::decode(&payload, limits)?;
            if value.id != id {
                return Err(Error::Invalid("header block index identity mismatch"));
            }
            header_blocks.insert(value.id, value);
        }

        let mut stream_indexes = HashMap::new();
        for (id, offset) in sorted_offsets(&stream_index_offsets) {
            let payload = read_indexed_metadata_payload(
                io,
                limits,
                &dictionaries,
                &mut metadata_cache,
                offset,
                RecordType::StreamIndex,
                id.0,
            )?;
            let value = StreamIndex::decode(&payload, limits)?;
            if value.id != id {
                return Err(Error::Invalid("stream index identity mismatch"));
            }
            let body_length = match manifests.get(&value.raw_body_manifest_id) {
                Some(body) => body.total_length,
                None if allow_external_manifests => stream_observed_body_length(&value)?,
                None => return Err(Error::Missing(value.raw_body_manifest_id.to_string())),
            };
            value.validate(body_length, limits)?;
            stream_indexes.insert(value.id, value);
        }

        let mut stages = HashMap::new();
        for (id, offset) in sorted_offsets(&stage_offsets) {
            let payload = read_indexed_metadata_payload(
                io,
                limits,
                &dictionaries,
                &mut metadata_cache,
                offset,
                RecordType::Stage,
                id.0,
            )?;
            let value = Stage::decode(&payload, limits)?;
            if value.id != id {
                return Err(Error::Invalid("stage index identity mismatch"));
            }
            validate_stage_references(
                &value,
                &manifests,
                &header_blocks,
                &stream_indexes,
                allow_external_manifests,
            )?;
            stages.insert(value.id, value);
        }

        let mut exchanges = HashMap::new();
        for (id, offset) in sorted_offsets(&exchange_offsets) {
            let payload = read_indexed_metadata_payload(
                io,
                limits,
                &dictionaries,
                &mut metadata_cache,
                offset,
                RecordType::Exchange,
                id.0,
            )?;
            let value = Exchange::decode(&payload, limits)?;
            if value.id != id {
                return Err(Error::Invalid("exchange index identity mismatch"));
            }
            if value.data.stages.iter().any(|id| !stages.contains_key(id)) {
                return Err(Error::Invalid("exchange index has a missing stage"));
            }
            exchanges.insert(value.id, value);
        }

        let mut exchange_metadata = HashMap::new();
        for (id, offset) in sorted_offsets(&exchange_offsets) {
            if let Some(value) =
                read_adjacent_exchange_metadata(io, limits, offset, checkpoint.indexed_end, id)?
            {
                exchange_metadata.insert(id, value);
            }
        }

        let mut conversation_entries = HashMap::new();
        for (id, offset) in sorted_offsets(&conversation_entry_offsets) {
            let payload = read_indexed_metadata_payload(
                io,
                limits,
                &dictionaries,
                &mut metadata_cache,
                offset,
                RecordType::ConversationEntry,
                id.0,
            )?;
            let value = ConversationEntry::decode(&payload, limits)?;
            if value.id != id {
                return Err(Error::Invalid("conversation entry index identity mismatch"));
            }
            validate_conversation_entry_references(&value, &manifests, allow_external_manifests)?;
            conversation_entries.insert(value.id, value);
        }

        let mut decoded_generations = Vec::with_capacity(generation_offsets.len());
        for (id, offset) in sorted_offsets(&generation_offsets) {
            let payload = read_indexed_metadata_payload(
                io,
                limits,
                &dictionaries,
                &mut metadata_cache,
                offset,
                RecordType::Generation,
                id.0,
            )?;
            let value = Generation::decode(&payload, limits)?;
            if value.id != id {
                return Err(Error::Invalid("generation index identity mismatch"));
            }
            let inner_order = indexed_metadata_inner_order(
                &metadata_cache,
                offset,
                RecordType::Generation,
                id.0,
            )?;
            decoded_generations.push((offset, inner_order, value));
        }
        decoded_generations.sort_by_key(|(offset, inner_order, _)| (*offset, *inner_order));

        let mut generations = HashMap::new();
        for (_, _, value) in decoded_generations {
            validate_generation_references(&value, &conversation_entries, &generations)?;
            generations.insert(value.id, value);
        }

        let mut turn_views = HashMap::new();
        for (id, offset) in sorted_offsets(&turn_view_offsets) {
            let payload = read_indexed_metadata_payload(
                io,
                limits,
                &dictionaries,
                &mut metadata_cache,
                offset,
                RecordType::TurnView,
                id.0,
            )?;
            let value = TurnView::decode(&payload, limits)?;
            if value.id != id {
                return Err(Error::Invalid("turn view index identity mismatch"));
            }
            validate_turn_view_references(&value, &traces, &generations, &conversation_entries)?;
            turn_views.insert(value.id, value);
        }
        validate_indexed_metadata_page_coverage(
            &metadata_cache,
            &manifest_offsets,
            &header_block_offsets,
            &stream_index_offsets,
            &stage_offsets,
            &exchange_offsets,
            &conversation_entry_offsets,
            &generation_offsets,
            &turn_view_offsets,
        )?;

        for (trace, exchange_id) in &traces {
            let exchange = exchanges
                .get(exchange_id)
                .ok_or_else(|| Error::Missing(exchange_id.to_string()))?;
            if exchange.data.trace_id != *trace {
                return Err(Error::Invalid("trace index identity mismatch"));
            }
        }
        if traces.len() != exchanges.len() {
            return Err(Error::Invalid("trace index does not cover all exchanges"));
        }
        for (trace, turn_id) in &turn_traces {
            let turn = turn_views
                .get(turn_id)
                .ok_or_else(|| Error::Missing(turn_id.to_string()))?;
            if turn.data.trace_id != *trace {
                return Err(Error::Invalid("turn trace index identity mismatch"));
            }
        }
        if turn_traces.len() != turn_views.len() {
            return Err(Error::Invalid(
                "turn trace index does not cover all turn views",
            ));
        }

        session_parts.sort_by(|left, right| (&left.0, left.1).cmp(&(&right.0, right.1)));
        let mut sessions: HashMap<Vec<u8>, Vec<ExchangeId>> = HashMap::new();
        for (session, start, ids) in session_parts {
            let target = sessions.entry(session.clone()).or_default();
            if target.len() != start as usize {
                return Err(Error::Invalid("session index parts have a gap or overlap"));
            }
            for id in ids {
                let id = ExchangeId(id);
                let exchange = exchanges
                    .get(&id)
                    .ok_or_else(|| Error::Missing(id.to_string()))?;
                if exchange.data.session_id.as_deref() != Some(session.as_slice()) {
                    return Err(Error::Invalid("session index identity mismatch"));
                }
                target.push(id);
            }
            ensure_loaded_count(
                "session exchange count",
                target.len(),
                limits.max_session_exchanges,
            )?;
        }
        let expected_session_exchanges = exchanges
            .values()
            .filter(|exchange| exchange.data.session_id.is_some())
            .count();
        let indexed_session_exchanges: usize = sessions.values().map(Vec::len).sum();
        if expected_session_exchanges != indexed_session_exchanges {
            return Err(Error::Invalid(
                "session index does not cover all session exchanges",
            ));
        }

        Ok(LoadedIndex {
            checkpoint,
            chunks,
            manifests,
            manifest_offsets,
            body_identities,
            header_blocks,
            header_block_offsets,
            stream_indexes,
            stream_index_offsets,
            stages,
            stage_offsets,
            exchanges,
            exchange_offsets,
            exchange_metadata,
            traces,
            sessions,
            conversation_entries,
            conversation_entry_offsets,
            generations,
            generation_offsets,
            turn_views,
            turn_view_offsets,
            turn_traces,
        })
    }

    fn untrusted_indexed_end(
        io: &mut R,
        limits: &Limits,
        pointer: CheckpointPointer,
    ) -> Option<u64> {
        io.seek(SeekFrom::Start(pointer.frame_offset)).ok()?;
        let mut prefix = [0; 20];
        io.read_exact(&mut prefix).ok()?;
        if &prefix[..4] != b"LREC"
            || u16::from_le_bytes(prefix[4..6].try_into().ok()?) != RecordType::Checkpoint.code()
        {
            return None;
        }
        let payload_len = u64::from_le_bytes(prefix[12..20].try_into().ok()?);
        if payload_len > limits.max_frame_payload
            || payload_len.saturating_add(24) != pointer.frame_length
        {
            return None;
        }
        let mut payload = vec![0; usize::try_from(payload_len).ok()?];
        io.read_exact(&mut payload).ok()?;
        Checkpoint::decode(
            &payload,
            u32::try_from((payload_len / 52).saturating_add(1)).ok()?,
        )
        .ok()
        .map(|checkpoint| checkpoint.indexed_end)
    }

    fn open_forward(
        mut io: R,
        limits: Limits,
        scan_end: Option<u64>,
        recovery_override: Option<RecoveryStatus>,
    ) -> Result<Self> {
        io.seek(SeekFrom::Start(0))?;
        let (header, data_offset) = read_file_header(&mut io, &limits)?;
        let physical_end = file_length(&mut io)?;
        let mut chunks = HashMap::new();
        let mut dictionaries: HashMap<[u8; 32], Vec<u8>> = HashMap::new();
        let mut manifests = HashMap::new();
        let mut manifest_offsets = HashMap::new();
        let mut body_identities = HashMap::new();
        let mut header_blocks = HashMap::new();
        let mut header_block_offsets = HashMap::new();
        let mut stream_indexes = HashMap::new();
        let mut stream_index_offsets = HashMap::new();
        let mut stages = HashMap::new();
        let mut stage_offsets = HashMap::new();
        let mut exchanges = HashMap::new();
        let mut exchange_offsets = HashMap::new();
        let mut exchange_metadata = HashMap::new();
        let mut traces = HashMap::new();
        let mut sessions: HashMap<Vec<u8>, Vec<ExchangeId>> = HashMap::new();
        let mut conversation_entries = HashMap::new();
        let mut conversation_entry_offsets = HashMap::new();
        let mut generations = HashMap::new();
        let mut generation_offsets = HashMap::new();
        let mut turn_views = HashMap::new();
        let mut turn_view_offsets = HashMap::new();
        let mut turn_traces = HashMap::new();
        let mut record_count = 0;
        let mut derived_start = None;
        let mut dangling_checkpoint = false;
        let mut adjacent_direct_exchange = None;
        let (last_valid, truncated) = loop {
            let before = io.stream_position()?;
            if scan_end.is_some_and(|end| before >= end) {
                break (before, false);
            }
            if scan_end.is_none() && before < physical_end {
                let remaining = (physical_end - before).min(8) as usize;
                let mut peek = [0; 8];
                io.read_exact(&mut peek[..remaining])?;
                io.seek(SeekFrom::Start(before))?;
                if b"LARFOOT1".starts_with(&peek[..remaining]) {
                    break (before, true);
                }
            }
            let next = {
                let mut frame_reader = FrameReader::new(&mut io, &limits);
                frame_reader.read_next()?
            };
            match next {
                (FrameRead::CleanEof, _) => break (before, false),
                (FrameRead::Truncated, _) => break (before, true),
                (FrameRead::Frame, Some(frame)) => {
                    let previous_direct_exchange = adjacent_direct_exchange.take();
                    if frame.schema_version != RECORD_SCHEMA_V1 {
                        if frame.flags & RecordFrame::REQUIRED != 0 {
                            return Err(Error::Unsupported(format!(
                                "required record schema {}",
                                frame.schema_version
                            )));
                        }
                        continue;
                    }
                    match frame.record_type {
                        RecordType::Chunk => {
                            derived_start = None;
                            dangling_checkpoint = false;
                            let chunk = StoredChunk::decode(&frame.payload, &limits)?;
                            if let Some(previous) = chunks.insert(
                                chunk.hash,
                                ChunkLocation {
                                    frame_offset: frame.offset,
                                    uncompressed_length: chunk.uncompressed_length,
                                    compressed_length: chunk.compressed.len() as u64,
                                },
                            ) {
                                return Err(Error::InvalidDetail(format!(
                                    "duplicate chunk record at {} (previous {})",
                                    frame.offset, previous.frame_offset
                                )));
                            }
                        }
                        RecordType::BodyManifest => {
                            derived_start = None;
                            dangling_checkpoint = false;
                            let value = BodyManifest::decode(&frame.payload, &limits)?;
                            validate_manifest_references(&value, &chunks)?;
                            body_identities
                                .entry((
                                    value.whole_body_hash,
                                    value.total_length,
                                    value.media_type.clone(),
                                    value.content_encoding.clone(),
                                ))
                                .or_insert(value.id);
                            manifest_offsets.insert(value.id, frame.offset);
                            if manifests.insert(value.id, value).is_some() {
                                return Err(Error::Invalid("duplicate manifest record"));
                            }
                        }
                        RecordType::HeaderBlock => {
                            derived_start = None;
                            dangling_checkpoint = false;
                            let value = HeaderBlock::decode(&frame.payload, &limits)?;
                            header_block_offsets.insert(value.id, frame.offset);
                            if header_blocks.insert(value.id, value).is_some() {
                                return Err(Error::Invalid("duplicate header block record"));
                            }
                        }
                        RecordType::StreamIndex => {
                            derived_start = None;
                            dangling_checkpoint = false;
                            ensure_scan_capacity(
                                "stream index count",
                                stream_indexes.len(),
                                limits.max_stream_indexes,
                            )?;
                            let value = StreamIndex::decode(&frame.payload, &limits)?;
                            let body_length = match manifests.get(&value.raw_body_manifest_id) {
                                Some(body) => body.total_length,
                                None if header.required_feature_bits
                                    & REQUIRED_FEATURE_ARCHIVE_SET_BODY_REFS
                                    != 0 =>
                                {
                                    stream_observed_body_length(&value)?
                                }
                                None => {
                                    return Err(Error::Missing(
                                        value.raw_body_manifest_id.to_string(),
                                    ))
                                }
                            };
                            value.validate(body_length, &limits)?;
                            stream_index_offsets.insert(value.id, frame.offset);
                            if stream_indexes.insert(value.id, value).is_some() {
                                return Err(Error::Invalid("duplicate stream index record"));
                            }
                        }
                        RecordType::Stage => {
                            derived_start = None;
                            dangling_checkpoint = false;
                            ensure_scan_capacity("stage count", stages.len(), limits.max_stages)?;
                            let value = Stage::decode(&frame.payload, &limits)?;
                            validate_stage_references(
                                &value,
                                &manifests,
                                &header_blocks,
                                &stream_indexes,
                                header.required_feature_bits
                                    & REQUIRED_FEATURE_ARCHIVE_SET_BODY_REFS
                                    != 0,
                            )?;
                            stage_offsets.insert(value.id, frame.offset);
                            if stages.insert(value.id, value).is_some() {
                                return Err(Error::Invalid("duplicate stage record"));
                            }
                        }
                        RecordType::Exchange => {
                            derived_start = None;
                            dangling_checkpoint = false;
                            ensure_scan_capacity(
                                "exchange count",
                                exchanges.len(),
                                limits.max_exchanges,
                            )?;
                            let value = Exchange::decode(&frame.payload, &limits)?;
                            for stage_id in &value.data.stages {
                                if !stages.contains_key(stage_id) {
                                    return Err(Error::Missing(stage_id.to_string()));
                                }
                            }
                            if let Some(previous) =
                                traces.insert(value.data.trace_id.clone(), value.id)
                            {
                                return Err(Error::InvalidDetail(format!(
                                    "duplicate exchange trace ID (previous {previous})"
                                )));
                            }
                            if let Some(session_id) = &value.data.session_id {
                                let session = sessions.entry(session_id.clone()).or_default();
                                ensure_scan_capacity(
                                    "session exchange count",
                                    session.len(),
                                    limits.max_session_exchanges,
                                )?;
                                session.push(value.id);
                            }
                            exchange_offsets.insert(value.id, frame.offset);
                            adjacent_direct_exchange = Some(value.id);
                            if exchanges.insert(value.id, value).is_some() {
                                return Err(Error::Invalid("duplicate exchange record"));
                            }
                        }
                        RecordType::ExchangeMetadata => {
                            derived_start = None;
                            dangling_checkpoint = false;
                            if frame.flags & RecordFrame::REQUIRED != 0 {
                                return Err(Error::Unsupported(
                                    "required exchange metadata companion is unsupported".into(),
                                ));
                            }
                            if frame.flags != 0 {
                                // Unknown optional flags are safe to skip. The
                                // adjacency slot remains consumed.
                            } else {
                                let value = ExchangeMetadata::decode(&frame.payload, &limits)?;
                                let expected = previous_direct_exchange
                                    .ok_or(Error::Invalid("orphan exchange metadata companion"))?;
                                if value.exchange_id != expected {
                                    return Err(Error::Invalid(
                                        "exchange metadata companion identity mismatch",
                                    ));
                                }
                                if !exchanges.contains_key(&value.exchange_id) {
                                    return Err(Error::Invalid(
                                        "exchange metadata companion references a missing exchange",
                                    ));
                                }
                                if exchange_metadata.insert(value.exchange_id, value).is_some() {
                                    return Err(Error::Invalid(
                                        "duplicate exchange metadata companion",
                                    ));
                                }
                            }
                        }
                        RecordType::ConversationEntry => {
                            derived_start = None;
                            dangling_checkpoint = false;
                            require_conversation_feature_bits(header.required_feature_bits)?;
                            ensure_scan_capacity(
                                "conversation entry count",
                                conversation_entries.len(),
                                limits.max_conversation_entries,
                            )?;
                            let value = ConversationEntry::decode(&frame.payload, &limits)?;
                            validate_conversation_entry_references(
                                &value,
                                &manifests,
                                header.required_feature_bits
                                    & REQUIRED_FEATURE_ARCHIVE_SET_BODY_REFS
                                    != 0,
                            )?;
                            conversation_entry_offsets.insert(value.id, frame.offset);
                            if conversation_entries.insert(value.id, value).is_some() {
                                return Err(Error::Invalid("duplicate conversation entry record"));
                            }
                        }
                        RecordType::Generation => {
                            derived_start = None;
                            dangling_checkpoint = false;
                            require_conversation_feature_bits(header.required_feature_bits)?;
                            ensure_scan_capacity(
                                "generation count",
                                generations.len(),
                                limits.max_generations,
                            )?;
                            let value = Generation::decode(&frame.payload, &limits)?;
                            validate_generation_references(
                                &value,
                                &conversation_entries,
                                &generations,
                            )?;
                            generation_offsets.insert(value.id, frame.offset);
                            if generations.insert(value.id, value).is_some() {
                                return Err(Error::Invalid("duplicate generation record"));
                            }
                        }
                        RecordType::TurnView => {
                            derived_start = None;
                            dangling_checkpoint = false;
                            require_conversation_feature_bits(header.required_feature_bits)?;
                            ensure_scan_capacity(
                                "turn view count",
                                turn_views.len(),
                                limits.max_turn_views,
                            )?;
                            let value = TurnView::decode(&frame.payload, &limits)?;
                            validate_turn_view_references(
                                &value,
                                &traces,
                                &generations,
                                &conversation_entries,
                            )?;
                            if let Some(previous) =
                                turn_traces.insert(value.data.trace_id.clone(), value.id)
                            {
                                return Err(Error::InvalidDetail(format!(
                                    "duplicate turn view trace ID (previous {previous})"
                                )));
                            }
                            turn_view_offsets.insert(value.id, frame.offset);
                            if turn_views.insert(value.id, value).is_some() {
                                return Err(Error::Invalid("duplicate turn view record"));
                            }
                        }
                        RecordType::DictionaryData => {
                            let dictionary = StoredDictionary::decode(&frame.payload, &limits)?;
                            if dictionaries
                                .insert(dictionary.id, dictionary.bytes)
                                .is_some()
                            {
                                return Err(Error::Invalid("duplicate dictionary record"));
                            }
                            derived_start = None;
                            dangling_checkpoint = false;
                        }
                        RecordType::MetadataPage => {
                            derived_start = None;
                            dangling_checkpoint = false;
                            let page = if dictionaries.is_empty() {
                                MetadataPage::decode(&frame.payload, &limits)?
                            } else {
                                MetadataPage::decode_with_dictionaries(
                                    &frame.payload,
                                    &limits,
                                    &dictionaries,
                                )?
                            };
                            for inner in page.frames(&limits)? {
                                if inner.schema_version != RECORD_SCHEMA_V1
                                    || inner.flags & RecordFrame::REQUIRED == 0
                                {
                                    return Err(Error::Invalid(
                                        "metadata page contains an unsupported canonical record",
                                    ));
                                }
                                match inner.record_type {
                                    RecordType::BodyManifest => {
                                        let value = BodyManifest::decode(&inner.payload, &limits)?;
                                        validate_manifest_references(&value, &chunks)?;
                                        body_identities
                                            .entry((
                                                value.whole_body_hash,
                                                value.total_length,
                                                value.media_type.clone(),
                                                value.content_encoding.clone(),
                                            ))
                                            .or_insert(value.id);
                                        manifest_offsets.insert(value.id, frame.offset);
                                        if manifests.insert(value.id, value).is_some() {
                                            return Err(Error::Invalid(
                                                "duplicate manifest in metadata page",
                                            ));
                                        }
                                    }
                                    RecordType::HeaderBlock => {
                                        let value = HeaderBlock::decode(&inner.payload, &limits)?;
                                        header_block_offsets.insert(value.id, frame.offset);
                                        if header_blocks.insert(value.id, value).is_some() {
                                            return Err(Error::Invalid(
                                                "duplicate header block in metadata page",
                                            ));
                                        }
                                    }
                                    RecordType::StreamIndex => {
                                        ensure_scan_capacity(
                                            "stream index count",
                                            stream_indexes.len(),
                                            limits.max_stream_indexes,
                                        )?;
                                        let value = StreamIndex::decode(&inner.payload, &limits)?;
                                        let body_length =
                                            match manifests.get(&value.raw_body_manifest_id) {
                                                Some(body) => body.total_length,
                                                None if header.required_feature_bits
                                                    & REQUIRED_FEATURE_ARCHIVE_SET_BODY_REFS
                                                    != 0 =>
                                                {
                                                    stream_observed_body_length(&value)?
                                                }
                                                None => {
                                                    return Err(Error::Missing(
                                                        value.raw_body_manifest_id.to_string(),
                                                    ))
                                                }
                                            };
                                        value.validate(body_length, &limits)?;
                                        stream_index_offsets.insert(value.id, frame.offset);
                                        if stream_indexes.insert(value.id, value).is_some() {
                                            return Err(Error::Invalid(
                                                "duplicate stream index in metadata page",
                                            ));
                                        }
                                    }
                                    RecordType::Stage => {
                                        ensure_scan_capacity(
                                            "stage count",
                                            stages.len(),
                                            limits.max_stages,
                                        )?;
                                        let value = Stage::decode(&inner.payload, &limits)?;
                                        validate_stage_references(
                                            &value,
                                            &manifests,
                                            &header_blocks,
                                            &stream_indexes,
                                            header.required_feature_bits
                                                & REQUIRED_FEATURE_ARCHIVE_SET_BODY_REFS
                                                != 0,
                                        )?;
                                        stage_offsets.insert(value.id, frame.offset);
                                        if stages.insert(value.id, value).is_some() {
                                            return Err(Error::Invalid(
                                                "duplicate stage in metadata page",
                                            ));
                                        }
                                    }
                                    RecordType::Exchange => {
                                        ensure_scan_capacity(
                                            "exchange count",
                                            exchanges.len(),
                                            limits.max_exchanges,
                                        )?;
                                        let value = Exchange::decode(&inner.payload, &limits)?;
                                        for stage_id in &value.data.stages {
                                            if !stages.contains_key(stage_id) {
                                                return Err(Error::Missing(stage_id.to_string()));
                                            }
                                        }
                                        if let Some(previous) =
                                            traces.insert(value.data.trace_id.clone(), value.id)
                                        {
                                            return Err(Error::InvalidDetail(format!(
                                                "duplicate exchange trace ID (previous {previous})"
                                            )));
                                        }
                                        if let Some(session_id) = &value.data.session_id {
                                            let session =
                                                sessions.entry(session_id.clone()).or_default();
                                            ensure_scan_capacity(
                                                "session exchange count",
                                                session.len(),
                                                limits.max_session_exchanges,
                                            )?;
                                            session.push(value.id);
                                        }
                                        exchange_offsets.insert(value.id, frame.offset);
                                        if exchanges.insert(value.id, value).is_some() {
                                            return Err(Error::Invalid(
                                                "duplicate exchange in metadata page",
                                            ));
                                        }
                                    }
                                    RecordType::ConversationEntry => {
                                        require_conversation_feature_bits(
                                            header.required_feature_bits,
                                        )?;
                                        ensure_scan_capacity(
                                            "conversation entry count",
                                            conversation_entries.len(),
                                            limits.max_conversation_entries,
                                        )?;
                                        let value =
                                            ConversationEntry::decode(&inner.payload, &limits)?;
                                        validate_conversation_entry_references(
                                            &value,
                                            &manifests,
                                            header.required_feature_bits
                                                & REQUIRED_FEATURE_ARCHIVE_SET_BODY_REFS
                                                != 0,
                                        )?;
                                        conversation_entry_offsets.insert(value.id, frame.offset);
                                        if conversation_entries.insert(value.id, value).is_some() {
                                            return Err(Error::Invalid(
                                                "duplicate conversation entry in metadata page",
                                            ));
                                        }
                                    }
                                    RecordType::Generation => {
                                        require_conversation_feature_bits(
                                            header.required_feature_bits,
                                        )?;
                                        ensure_scan_capacity(
                                            "generation count",
                                            generations.len(),
                                            limits.max_generations,
                                        )?;
                                        let value = Generation::decode(&inner.payload, &limits)?;
                                        validate_generation_references(
                                            &value,
                                            &conversation_entries,
                                            &generations,
                                        )?;
                                        generation_offsets.insert(value.id, frame.offset);
                                        if generations.insert(value.id, value).is_some() {
                                            return Err(Error::Invalid(
                                                "duplicate generation in metadata page",
                                            ));
                                        }
                                    }
                                    RecordType::TurnView => {
                                        require_conversation_feature_bits(
                                            header.required_feature_bits,
                                        )?;
                                        ensure_scan_capacity(
                                            "turn view count",
                                            turn_views.len(),
                                            limits.max_turn_views,
                                        )?;
                                        let value = TurnView::decode(&inner.payload, &limits)?;
                                        validate_turn_view_references(
                                            &value,
                                            &traces,
                                            &generations,
                                            &conversation_entries,
                                        )?;
                                        if let Some(previous) = turn_traces
                                            .insert(value.data.trace_id.clone(), value.id)
                                        {
                                            return Err(Error::InvalidDetail(format!(
                                                "duplicate turn view trace ID (previous {previous})"
                                            )));
                                        }
                                        turn_view_offsets.insert(value.id, frame.offset);
                                        if turn_views.insert(value.id, value).is_some() {
                                            return Err(Error::Invalid(
                                                "duplicate turn view in metadata page",
                                            ));
                                        }
                                    }
                                    _ => unreachable!("MetadataPage::frames filters record types"),
                                }
                            }
                        }
                        RecordType::IndexBlock => {
                            derived_start.get_or_insert(frame.offset);
                        }
                        RecordType::Checkpoint => {
                            derived_start.get_or_insert(frame.offset);
                            dangling_checkpoint = true;
                        }
                        RecordType::CheckpointLocator => {
                            derived_start = None;
                            dangling_checkpoint = false;
                            // Optional derived indexes are not part of the
                            // authoritative forward-scan reconstruction path.
                        }
                        RecordType::Unknown(_) => {}
                    }
                    record_count += 1;
                }
                _ => unreachable!(),
            }
        };
        let end = file_length(&mut io)?;
        let recovery = if let Some(recovery) = recovery_override {
            recovery
        } else if dangling_checkpoint || derived_start.is_some() {
            let last_valid_offset = derived_start.unwrap_or(last_valid);
            RecoveryStatus::CorruptIndexFallback {
                last_valid_offset,
                tail_bytes: end.saturating_sub(last_valid_offset),
            }
        } else if truncated {
            RecoveryStatus::TruncatedTail {
                last_valid_offset: last_valid,
                tail_bytes: end.saturating_sub(last_valid),
            }
        } else {
            RecoveryStatus::Clean
        };
        Ok(Self {
            io,
            header,
            limits,
            data_offset,
            recovery,
            record_count,
            open_path: OpenPath::ForwardScan,
            sealed: false,
            chunks,
            manifests,
            manifest_offsets,
            body_identities,
            header_blocks,
            header_block_offsets,
            stream_indexes,
            stream_index_offsets,
            stages,
            stage_offsets,
            exchanges,
            exchange_offsets,
            exchange_metadata,
            traces,
            sessions,
            conversation_entries,
            conversation_entry_offsets,
            generations,
            generation_offsets,
            turn_views,
            turn_view_offsets,
            turn_traces,
        })
    }

    pub fn header(&self) -> &FileHeader {
        &self.header
    }
    pub fn recovery_status(&self) -> RecoveryStatus {
        self.recovery
    }
    pub fn open_path(&self) -> OpenPath {
        self.open_path
    }
    pub fn is_sealed(&self) -> bool {
        self.sealed
    }
    pub fn data_offset(&self) -> u64 {
        self.data_offset
    }
    pub fn record_count(&self) -> usize {
        self.record_count
    }
    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }
    pub fn chunk_records(&self) -> impl Iterator<Item = ChunkRecordDescriptor> + '_ {
        self.chunks
            .iter()
            .map(|(hash, location)| ChunkRecordDescriptor {
                hash: *hash,
                frame_offset: location.frame_offset,
                uncompressed_length: location.uncompressed_length,
                compressed_length: location.compressed_length,
            })
    }
    pub fn manifest_count(&self) -> usize {
        self.manifests.len()
    }
    pub fn header_block_count(&self) -> usize {
        self.header_blocks.len()
    }
    pub fn stream_index_count(&self) -> usize {
        self.stream_indexes.len()
    }
    pub fn stage_count(&self) -> usize {
        self.stages.len()
    }
    pub fn exchange_count(&self) -> usize {
        self.exchanges.len()
    }
    pub fn conversation_entry_count(&self) -> usize {
        self.conversation_entries.len()
    }
    pub fn generation_count(&self) -> usize {
        self.generations.len()
    }
    pub fn turn_view_count(&self) -> usize {
        self.turn_views.len()
    }
    pub fn manifest_ids(&self) -> impl Iterator<Item = &ManifestId> {
        self.manifests.keys()
    }
    pub fn header_block_ids(&self) -> impl Iterator<Item = &HeaderBlockId> {
        self.header_blocks.keys()
    }
    pub fn manifest(&self, id: &ManifestId) -> Option<&BodyManifest> {
        self.manifests.get(id)
    }
    pub fn header_block(&self, id: &HeaderBlockId) -> Option<&HeaderBlock> {
        self.header_blocks.get(id)
    }
    pub fn stream_index(&self, id: &StreamIndexId) -> Option<&StreamIndex> {
        self.stream_indexes.get(id)
    }
    pub fn stage(&self, id: &StageId) -> Option<&Stage> {
        self.stages.get(id)
    }
    pub fn exchange(&self, id: &ExchangeId) -> Option<&Exchange> {
        self.exchanges.get(id)
    }
    pub fn exchange_metadata(&self, id: &ExchangeId) -> Option<&ExchangeMetadata> {
        self.exchange_metadata.get(id)
    }
    pub fn conversation_entry(&self, id: &ConversationEntryId) -> Option<&ConversationEntry> {
        self.conversation_entries.get(id)
    }
    pub fn generation(&self, id: &GenerationId) -> Option<&Generation> {
        self.generations.get(id)
    }
    pub fn turn_view(&self, id: &TurnViewId) -> Option<&TurnView> {
        self.turn_views.get(id)
    }
    pub fn exchange_by_trace(&self, trace_id: &[u8]) -> Option<&Exchange> {
        self.traces
            .get(trace_id)
            .and_then(|id| self.exchanges.get(id))
    }
    pub fn exchange_metadata_by_trace(&self, trace_id: &[u8]) -> Option<&ExchangeMetadata> {
        self.traces
            .get(trace_id)
            .and_then(|id| self.exchange_metadata.get(id))
    }
    pub fn exchanges_for_session(&self, session_id: &[u8]) -> Option<&[ExchangeId]> {
        self.sessions.get(session_id).map(Vec::as_slice)
    }
    pub fn turn_view_by_trace(&self, trace_id: &[u8]) -> Option<&TurnView> {
        self.turn_traces
            .get(trace_id)
            .and_then(|id| self.turn_views.get(id))
    }
    pub fn stream_index_ids(&self) -> impl Iterator<Item = &StreamIndexId> {
        self.stream_indexes.keys()
    }
    pub fn stage_ids(&self) -> impl Iterator<Item = &StageId> {
        self.stages.keys()
    }
    pub fn exchange_ids(&self) -> impl Iterator<Item = &ExchangeId> {
        self.exchanges.keys()
    }
    pub fn conversation_entry_ids(&self) -> impl Iterator<Item = &ConversationEntryId> {
        self.conversation_entries.keys()
    }
    pub fn generation_ids(&self) -> impl Iterator<Item = &GenerationId> {
        self.generations.keys()
    }
    pub fn turn_view_ids(&self) -> impl Iterator<Item = &TurnViewId> {
        self.turn_views.keys()
    }

    pub fn read_body(&mut self, id: &ManifestId) -> Result<Vec<u8>> {
        let length = self
            .manifests
            .get(id)
            .ok_or_else(|| Error::Missing(id.to_string()))?
            .total_length;
        let capacity = usize::try_from(length).map_err(|_| Error::Limit {
            what: "in-memory body",
            actual: length,
            limit: usize::MAX as u64,
        })?;
        let mut output = Vec::with_capacity(capacity);
        self.write_body(id, &mut output)?;
        Ok(output)
    }

    /// Reconstruct only the requested logical body ranges.
    ///
    /// The manifest structure and every touched chunk are verified, while
    /// untouched chunks are neither read nor decompressed. Callers that need a
    /// whole-body BLAKE3 verification must use [`Self::read_body`]. Repeated or
    /// overlapping ranges share a per-call chunk cache.
    pub fn read_body_ranges(
        &mut self,
        id: &ManifestId,
        ranges: &[(u64, u64)],
    ) -> Result<Vec<Vec<u8>>> {
        let manifest = self
            .manifests
            .get(id)
            .cloned()
            .ok_or_else(|| Error::Missing(id.to_string()))?;
        manifest.validate()?;

        let mut outputs = Vec::with_capacity(ranges.len());
        let mut ends = Vec::with_capacity(ranges.len());
        for &(offset, length) in ranges {
            let end = offset
                .checked_add(length)
                .ok_or(Error::Invalid("body range overflow"))?;
            if end > manifest.total_length {
                return Err(Error::Invalid("body range exceeds manifest"));
            }
            let capacity = usize::try_from(length).map_err(|_| Error::Limit {
                what: "in-memory body range",
                actual: length,
                limit: usize::MAX as u64,
            })?;
            outputs.push(Vec::with_capacity(capacity));
            ends.push(end);
        }

        let mut chunks = HashMap::<ChunkHash, Vec<u8>>::new();
        for (index, &(range_start, range_length)) in ranges.iter().enumerate() {
            if range_length == 0 {
                continue;
            }
            let range_end = ends[index];
            for reference in &manifest.chunks {
                let reference_end = reference
                    .logical_offset
                    .checked_add(reference.length)
                    .ok_or(Error::Invalid("manifest range overflow"))?;
                let overlap_start = range_start.max(reference.logical_offset);
                let overlap_end = range_end.min(reference_end);
                if overlap_start >= overlap_end {
                    continue;
                }
                if !chunks.contains_key(&reference.chunk_hash) {
                    let bytes = self.read_chunk(&reference.chunk_hash)?;
                    chunks.insert(reference.chunk_hash, bytes);
                }
                let chunk = chunks
                    .get(&reference.chunk_hash)
                    .expect("requested chunk was cached");
                let within_reference = overlap_start - reference.logical_offset;
                let chunk_start = reference
                    .chunk_offset
                    .checked_add(within_reference)
                    .ok_or(Error::Invalid("body range chunk offset overflow"))?;
                let chunk_end = chunk_start
                    .checked_add(overlap_end - overlap_start)
                    .ok_or(Error::Invalid("body range chunk end overflow"))?;
                let chunk_start = usize::try_from(chunk_start)
                    .map_err(|_| Error::Invalid("body range chunk offset overflow"))?;
                let chunk_end = usize::try_from(chunk_end)
                    .map_err(|_| Error::Invalid("body range chunk end overflow"))?;
                outputs[index].extend_from_slice(
                    chunk
                        .get(chunk_start..chunk_end)
                        .ok_or(Error::Invalid("body range exceeds chunk"))?,
                );
            }
            if outputs[index].len() as u64 != range_length {
                return Err(Error::Invalid("reconstructed body range length mismatch"));
            }
        }
        Ok(outputs)
    }

    pub fn read_body_range(
        &mut self,
        id: &ManifestId,
        offset: u64,
        length: u64,
    ) -> Result<Vec<u8>> {
        self.read_body_ranges(id, &[(offset, length)])?
            .pop()
            .ok_or(Error::Invalid("body range result missing"))
    }

    /// Reconstruct a captured stream once and build a timed zero-copy replay
    /// schedule over either the observed HTTP reads or parsed provider frames.
    pub fn read_stream_replay(
        &mut self,
        id: &StreamIndexId,
        source: crate::StreamReplaySource,
        timing: crate::StreamReplayTiming,
    ) -> Result<crate::StreamReplay> {
        let index = self
            .stream_indexes
            .get(id)
            .cloned()
            .ok_or_else(|| Error::Missing(id.to_string()))?;
        let body = self.read_body(&index.raw_body_manifest_id)?;
        crate::StreamReplay::from_index(&index, body, source, timing)
    }

    pub fn write_body<W: Write>(&mut self, id: &ManifestId, mut output: W) -> Result<u64> {
        let manifest = self
            .manifests
            .get(id)
            .cloned()
            .ok_or_else(|| Error::Missing(id.to_string()))?;
        manifest.validate()?;
        let mut body_hash = blake3::Hasher::new();
        let mut written = 0u64;
        for reference in &manifest.chunks {
            let chunk = self.read_chunk(&reference.chunk_hash)?;
            let start = usize::try_from(reference.chunk_offset)
                .map_err(|_| Error::Invalid("chunk range overflow"))?;
            let end_u64 = reference
                .chunk_offset
                .checked_add(reference.length)
                .ok_or(Error::Invalid("chunk range overflow"))?;
            let end =
                usize::try_from(end_u64).map_err(|_| Error::Invalid("chunk range overflow"))?;
            let range = chunk
                .get(start..end)
                .ok_or(Error::Invalid("manifest range exceeds chunk"))?;
            output.write_all(range)?;
            body_hash.update(range);
            written += range.len() as u64;
        }
        if written != manifest.total_length {
            return Err(Error::Invalid("reconstructed body length mismatch"));
        }
        let actual = ChunkHash {
            algorithm: crate::HashAlgorithm::Blake3,
            digest: *body_hash.finalize().as_bytes(),
        };
        if actual != manifest.whole_body_hash {
            return Err(Error::Invalid("reconstructed body hash mismatch"));
        }
        Ok(written)
    }

    /// Read and verify one chunk independently of any local manifest. This is
    /// the primitive used by catalogs to reconstruct cross-pack manifests.
    pub fn read_chunk(&mut self, hash: &ChunkHash) -> Result<Vec<u8>> {
        let location = self
            .chunks
            .get(hash)
            .cloned()
            .ok_or_else(|| Error::Missing(format!("chunk {hash:?}")))?;
        self.io.seek(SeekFrom::Start(location.frame_offset))?;
        let frame = {
            let mut reader = FrameReader::new(&mut self.io, &self.limits);
            match reader.read_next()? {
                (FrameRead::Frame, Some(frame)) if frame.record_type == RecordType::Chunk => frame,
                _ => return Err(Error::Invalid("chunk index points to non-chunk frame")),
            }
        };
        let stored = StoredChunk::decode(&frame.payload, &self.limits)?;
        if stored.hash != *hash
            || stored.uncompressed_length != location.uncompressed_length
            || stored.compressed.len() as u64 != location.compressed_length
        {
            return Err(Error::Invalid("chunk index identity mismatch"));
        }
        stored.decompress(&self.limits)
    }
}

fn require_conversation_feature_bits(required_feature_bits: u64) -> Result<()> {
    if required_feature_bits & REQUIRED_FEATURE_CONVERSATION_DAG == 0 {
        return Err(Error::Unsupported(
            "conversation DAG record without required feature bit".into(),
        ));
    }
    Ok(())
}

fn validate_conversation_entry_references(
    entry: &ConversationEntry,
    manifests: &HashMap<ManifestId, BodyManifest>,
    allow_external_manifests: bool,
) -> Result<()> {
    for range in &entry.data.raw_ranges {
        let Some(manifest) = manifests.get(&range.manifest_id) else {
            if allow_external_manifests {
                continue;
            }
            return Err(Error::Missing(range.manifest_id.to_string()));
        };
        let end = range
            .byte_offset
            .checked_add(range.byte_length)
            .ok_or(Error::Invalid("conversation artifact range overflow"))?;
        if end > manifest.total_length {
            return Err(Error::Invalid(
                "conversation artifact range exceeds manifest",
            ));
        }
    }
    Ok(())
}

fn validate_generation_references(
    generation: &Generation,
    entries: &HashMap<ConversationEntryId, ConversationEntry>,
    generations: &HashMap<GenerationId, Generation>,
) -> Result<()> {
    if let Some(parent) = generation.data.parent_generation_id {
        if !generations.contains_key(&parent) {
            return Err(Error::Missing(parent.to_string()));
        }
    }
    for entry in &generation.data.entries {
        if !entries.contains_key(entry) {
            return Err(Error::Missing(entry.to_string()));
        }
    }
    Ok(())
}

fn validate_turn_view_references(
    turn: &TurnView,
    traces: &HashMap<Vec<u8>, ExchangeId>,
    generations: &HashMap<GenerationId, Generation>,
    entries: &HashMap<ConversationEntryId, ConversationEntry>,
) -> Result<()> {
    if !traces.contains_key(&turn.data.trace_id) {
        return Err(Error::Invalid(
            "turn view references a missing exchange trace",
        ));
    }
    let generation = generations
        .get(&turn.data.generation_id)
        .ok_or_else(|| Error::Missing(turn.data.generation_id.to_string()))?;
    if turn.data.upto_index >= generation.data.entries.len() as u64 {
        return Err(Error::Invalid(
            "turn view upto index exceeds generation entries",
        ));
    }
    for entry in &turn.data.response_entry_refs {
        if !entries.contains_key(entry) {
            return Err(Error::Missing(entry.to_string()));
        }
    }
    Ok(())
}

fn ensure_scan_capacity(what: &'static str, current: usize, max: u32) -> Result<()> {
    let actual = current as u64 + 1;
    if actual > max as u64 {
        return Err(Error::Limit {
            what,
            actual,
            limit: max as u64,
        });
    }
    Ok(())
}

fn stream_observed_body_length(index: &StreamIndex) -> Result<u64> {
    index
        .reads
        .last()
        .map(|read| {
            read.byte_offset
                .checked_add(read.byte_length)
                .ok_or(Error::Invalid("stream read range overflow"))
        })
        .unwrap_or(Ok(0))
}

fn validate_stage_references(
    stage: &Stage,
    manifests: &HashMap<ManifestId, BodyManifest>,
    header_blocks: &HashMap<HeaderBlockId, HeaderBlock>,
    stream_indexes: &HashMap<StreamIndexId, StreamIndex>,
    allow_external_manifests: bool,
) -> Result<()> {
    for id in [
        stage.data.request_headers_ref,
        stage.data.response_headers_ref,
        stage.data.trailers_ref,
    ]
    .into_iter()
    .flatten()
    {
        if !header_blocks.contains_key(&id) {
            return Err(Error::Missing(id.to_string()));
        }
    }
    for id in [
        stage.data.request_body_manifest_ref,
        stage.data.response_body_manifest_ref,
    ]
    .into_iter()
    .flatten()
    {
        if !manifests.contains_key(&id) && !allow_external_manifests {
            return Err(Error::Missing(id.to_string()));
        }
    }
    if let Some(stream_id) = stage.data.stream_index_ref {
        let stream = stream_indexes
            .get(&stream_id)
            .ok_or_else(|| Error::Missing(stream_id.to_string()))?;
        if stage.data.response_body_manifest_ref != Some(stream.raw_body_manifest_id) {
            return Err(Error::Invalid(
                "stream index and response body must reference the same manifest",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ArtifactRangeRef, ConversationEntryData, ExchangeData, GenerationData, GenerationReason,
        HeaderAtom, HeaderFidelity, ParsedFrame, StageData, StageKind, StreamFrameKind,
        StreamParser, StreamRead, TurnViewData,
    };
    use proptest::prelude::*;

    fn header() -> FileHeader {
        FileHeader::standalone([9; 16], 123, b"alex-lar-test".to_vec())
    }
    fn small_config() -> ChunkerConfig {
        ChunkerConfig {
            min_size: 64,
            target_size: 128,
            max_size: 256,
        }
    }
    fn limits() -> Limits {
        Limits {
            max_chunk_uncompressed: 256,
            max_frame_payload: 1024 * 1024,
            max_body_length: 2_000_000,
            ..Limits::default()
        }
    }

    fn append_mixed_metadata<W: Read + Write + Seek>(
        writer: &mut ArchiveWriter<W>,
        trace_id: &[u8],
    ) -> (
        ManifestId,
        HeaderBlockId,
        StreamIndexId,
        StageId,
        ExchangeId,
    ) {
        let body = b"data: {\"type\":\"done\"}\n\n";
        let manifest = writer.append_body(body).unwrap();
        let headers = HeaderBlock::new(
            HeaderFidelity::Exact,
            vec![HeaderAtom {
                original_name: b"Content-Type".to_vec(),
                value: b"text/event-stream".to_vec(),
                flags: 0,
            }],
        );
        let header_id = writer.append_header_block(headers).unwrap();
        let stream = StreamIndex::new(
            manifest,
            vec![StreamRead {
                byte_offset: 0,
                byte_length: body.len() as u64,
                delta_from_first_byte_ns: 0,
            }],
            Vec::new(),
        );
        let stream_id = writer.append_stream_index(stream).unwrap();
        let mut stage_data = StageData::new(StageKind::UpstreamResponse, 123);
        stage_data.attempt_number = Some(1);
        stage_data.response_headers_ref = Some(header_id);
        stage_data.response_body_manifest_ref = Some(manifest);
        stage_data.stream_index_ref = Some(stream_id);
        let stage_id = writer.append_stage(Stage::new(stage_data)).unwrap();
        let mut exchange_data = ExchangeData::new(trace_id, 1, 123, vec![stage_id]);
        exchange_data.session_id = Some(b"mixed-session".to_vec());
        let exchange_id = writer
            .append_exchange(Exchange::new(exchange_data))
            .unwrap();
        (manifest, header_id, stream_id, stage_id, exchange_id)
    }

    #[test]
    fn deduplicates_and_reconstructs_body_and_headers() {
        let cursor = Cursor::new(Vec::new());
        let mut writer = ArchiveWriter::create(cursor, header(), small_config(), limits()).unwrap();
        let body: Vec<u8> = (0..20_000).map(|n| (n % 251) as u8).collect();
        let first = writer.append_body(&body).unwrap();
        let chunks = writer.chunk_count();
        let second = writer.append_body(&body).unwrap();
        assert_eq!(first, second);
        assert_eq!(writer.chunk_count(), chunks);
        assert_eq!(writer.manifest_count(), 1);
        let block = HeaderBlock::new(
            HeaderFidelity::Exact,
            vec![
                HeaderAtom {
                    original_name: b"Set-Cookie".to_vec(),
                    value: b"a=1".to_vec(),
                    flags: 0,
                },
                HeaderAtom {
                    original_name: b"Set-Cookie".to_vec(),
                    value: b"b=2".to_vec(),
                    flags: 0,
                },
            ],
        );
        let block_id = writer.append_header_block(block.clone()).unwrap();
        writer.append_header_block(block.clone()).unwrap();
        let bytes = writer.into_inner().unwrap().into_inner();
        let mut reader = ArchiveReader::open(Cursor::new(bytes), limits()).unwrap();
        assert_eq!(reader.read_body(&first).unwrap(), body);
        assert_eq!(reader.header_block(&block_id), Some(&block));
        assert_eq!(reader.header_block_count(), 1);
        assert_eq!(reader.recovery_status(), RecoveryStatus::Clean);
    }

    #[test]
    fn manifest_metadata_variants_share_the_same_physical_chunks() {
        let cursor = Cursor::new(Vec::new());
        let mut writer = ArchiveWriter::create(cursor, header(), small_config(), limits()).unwrap();
        let body = br#"{"same":"bytes"}"#;
        let plain = writer.append_body(body).unwrap();
        let chunks = writer.chunk_count();
        let json = writer
            .append_body_with_metadata(body, Some(b"application/json".to_vec()), None)
            .unwrap();
        assert_ne!(plain, json);
        assert_eq!(writer.chunk_count(), chunks);
        assert_eq!(writer.manifest_count(), 2);
        writer.seal().unwrap();
        let bytes = writer.into_inner().unwrap().into_inner();
        let mut reader = ArchiveReader::open(Cursor::new(bytes), limits()).unwrap();
        assert_eq!(reader.chunk_count(), chunks);
        assert_eq!(reader.manifest_count(), 2);
        assert_eq!(reader.read_body(&plain).unwrap(), body);
        assert_eq!(reader.read_body(&json).unwrap(), body);
        assert_eq!(
            reader.manifest(&json).unwrap().media_type.as_deref(),
            Some(b"application/json".as_slice())
        );
    }

    #[test]
    fn exact_manifest_copy_preserves_distinct_chunk_topologies() {
        let cursor = Cursor::new(Vec::new());
        let mut writer = ArchiveWriter::create(cursor, header(), small_config(), limits()).unwrap();
        let body = b"abcdefgh";
        let whole = writer.append_chunk_record(body).unwrap().hash;
        let left = writer.append_chunk_record(&body[..4]).unwrap().hash;
        let right = writer.append_chunk_record(&body[4..]).unwrap().hash;
        let one_range = BodyManifest::new(
            body.len() as u64,
            ChunkHash::blake3(body),
            None,
            None,
            vec![ChunkRef {
                chunk_hash: whole,
                chunk_offset: 0,
                logical_offset: 0,
                length: body.len() as u64,
            }],
        );
        let two_ranges = BodyManifest::new(
            body.len() as u64,
            ChunkHash::blake3(body),
            None,
            None,
            vec![
                ChunkRef {
                    chunk_hash: left,
                    chunk_offset: 0,
                    logical_offset: 0,
                    length: 4,
                },
                ChunkRef {
                    chunk_hash: right,
                    chunk_offset: 0,
                    logical_offset: 4,
                    length: 4,
                },
            ],
        );
        assert_ne!(one_range.id, two_ranges.id);
        let first = writer.append_manifest_record(one_range).unwrap();
        let second = writer.append_manifest_record(two_ranges).unwrap();
        assert_ne!(first, second);
        assert_eq!(writer.manifest_count(), 2);
        writer.seal().unwrap();

        let mut reader = ArchiveReader::open(writer.into_inner().unwrap(), limits()).unwrap();
        assert_eq!(reader.read_body(&first).unwrap(), body);
        assert_eq!(reader.read_body(&second).unwrap(), body);
    }

    #[test]
    fn chunk_identity_verification_rejects_hash_collision() {
        let cursor = Cursor::new(Vec::new());
        let mut writer = ArchiveWriter::create(cursor, header(), small_config(), limits()).unwrap();
        let existing = writer.append_chunk_record(b"existing bytes").unwrap();
        let colliding_bytes = b"different bytes";
        let colliding_hash = ChunkHash::blake3(colliding_bytes);
        let location = writer.chunks.get(&existing.hash).unwrap().clone();
        writer.chunks.insert(colliding_hash, location);

        let error = writer.append_chunk_record(colliding_bytes).unwrap_err();
        assert!(matches!(error, Error::Invalid("BLAKE3 chunk collision")));
    }

    #[test]
    fn catalog_descriptor_reads_one_chunk_without_an_archive_scan() {
        let cursor = Cursor::new(Vec::new());
        let mut writer = ArchiveWriter::create(cursor, header(), small_config(), limits()).unwrap();
        let descriptor = writer.append_chunk_record(b"direct catalog read").unwrap();
        let mut cursor = writer.into_inner().unwrap();

        assert_eq!(
            read_chunk_record_at(&mut cursor, &descriptor, &limits()).unwrap(),
            b"direct catalog read"
        );

        let mut wrong = descriptor;
        wrong.uncompressed_length += 1;
        assert!(matches!(
            read_chunk_record_at(&mut cursor, &wrong, &limits()),
            Err(Error::Invalid("chunk descriptor metadata mismatch"))
        ));
    }

    #[test]
    fn empty_body_uses_no_chunks() {
        let mut writer =
            ArchiveWriter::create(Cursor::new(Vec::new()), header(), small_config(), limits())
                .unwrap();
        let id = writer.append_body(&[]).unwrap();
        assert_eq!(writer.chunk_count(), 0);
        let mut reader = ArchiveReader::open(writer.into_inner().unwrap(), limits()).unwrap();
        assert_eq!(reader.read_body(&id).unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn clean_archive_reopens_for_append_and_reuses_existing_content() {
        let mut writer =
            ArchiveWriter::create(Cursor::new(Vec::new()), header(), small_config(), limits())
                .unwrap();
        let first = writer.append_body(b"first body").unwrap();
        let bytes = writer.into_inner().unwrap().into_inner();
        let mut writer =
            ArchiveWriter::open_append(Cursor::new(bytes), small_config(), limits()).unwrap();
        let before = writer.chunk_count();
        assert_eq!(writer.append_body(b"first body").unwrap(), first);
        assert_eq!(writer.chunk_count(), before);
        let second = writer.append_body(b"second body").unwrap();
        let mut reader = ArchiveReader::open(writer.into_inner().unwrap(), limits()).unwrap();
        assert_eq!(reader.read_body(&first).unwrap(), b"first body");
        assert_eq!(reader.read_body(&second).unwrap(), b"second body");
    }

    #[test]
    fn append_rejects_an_interrupted_tail() {
        let mut writer =
            ArchiveWriter::create(Cursor::new(Vec::new()), header(), small_config(), limits())
                .unwrap();
        writer.append_body(b"complete body").unwrap();
        let mut bytes = writer.into_inner().unwrap().into_inner();
        bytes.extend_from_slice(b"LRE");
        assert!(matches!(
            ArchiveWriter::open_append(Cursor::new(bytes), small_config(), limits()),
            Err(Error::InvalidDetail(detail)) if detail.contains("truncated tail")
        ));
    }

    #[test]
    fn complete_records_survive_truncated_tail() {
        let mut writer =
            ArchiveWriter::create(Cursor::new(Vec::new()), header(), small_config(), limits())
                .unwrap();
        let first = writer
            .append_body(b"the complete body before the interrupted append")
            .unwrap();
        let complete_len = writer.io.get_ref().len();
        writer.append_body(&vec![77; 1000]).unwrap();
        let mut bytes = writer.into_inner().unwrap().into_inner();
        bytes.truncate(complete_len + 11);
        let mut reader = ArchiveReader::open(Cursor::new(bytes), limits()).unwrap();
        assert!(matches!(
            reader.recovery_status(),
            RecoveryStatus::TruncatedTail { tail_bytes: 11, .. }
        ));
        assert_eq!(
            reader.read_body(&first).unwrap(),
            b"the complete body before the interrupted append"
        );
    }

    #[test]
    fn corruption_is_not_treated_as_truncation() {
        let mut writer =
            ArchiveWriter::create(Cursor::new(Vec::new()), header(), small_config(), limits())
                .unwrap();
        writer.append_body(b"checksum me").unwrap();
        let mut bytes = writer.into_inner().unwrap().into_inner();
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        assert!(matches!(
            ArchiveReader::open(Cursor::new(bytes), limits()),
            Err(Error::Checksum { .. })
        ));
    }

    #[test]
    fn active_checkpoint_uses_fast_path_and_remains_appendable() {
        let mut writer =
            ArchiveWriter::create(Cursor::new(Vec::new()), header(), small_config(), limits())
                .unwrap();
        let first = writer.append_body(b"checkpointed body").unwrap();
        writer.checkpoint().unwrap();
        let bytes = writer.into_inner().unwrap().into_inner();

        let mut reader = ArchiveReader::open(Cursor::new(bytes.clone()), limits()).unwrap();
        assert_eq!(reader.open_path(), OpenPath::Checkpoint);
        assert!(!reader.is_sealed());
        assert_eq!(reader.read_body(&first).unwrap(), b"checkpointed body");

        let mut writer =
            ArchiveWriter::open_append(Cursor::new(bytes), small_config(), limits()).unwrap();
        let second = writer.append_body(b"body after checkpoint").unwrap();
        writer.checkpoint().unwrap();
        let mut reader = ArchiveReader::open(writer.into_inner().unwrap(), limits()).unwrap();
        assert_eq!(reader.open_path(), OpenPath::Checkpoint);
        assert_eq!(reader.read_body(&first).unwrap(), b"checkpointed body");
        assert_eq!(reader.read_body(&second).unwrap(), b"body after checkpoint");
    }

    #[test]
    fn sealed_footer_uses_fast_path_and_rejects_append() {
        let mut writer =
            ArchiveWriter::create(Cursor::new(Vec::new()), header(), small_config(), limits())
                .unwrap();
        let id = writer.append_body(b"sealed body").unwrap();
        writer.seal().unwrap();
        assert!(matches!(
            writer.append_body(b"too late"),
            Err(Error::Invalid("sealed archive cannot be appended"))
        ));
        let bytes = writer.into_inner().unwrap().into_inner();
        assert_eq!(&bytes[bytes.len() - 8..], b"LAR1END!");
        let mut reader = ArchiveReader::open(Cursor::new(bytes.clone()), limits()).unwrap();
        assert_eq!(reader.open_path(), OpenPath::Footer);
        assert!(reader.is_sealed());
        assert_eq!(reader.read_body(&id).unwrap(), b"sealed body");
        assert!(matches!(
            ArchiveWriter::open_append(Cursor::new(bytes), small_config(), limits()),
            Err(Error::Invalid(
                "sealed archive cannot be reopened for append"
            ))
        ));
    }

    #[test]
    fn metadata_pages_round_trip_through_checkpoint_and_reduce_manifest_bytes() {
        fn bodies() -> Vec<Vec<u8>> {
            (0..80u32)
                .map(|index| {
                    let mut body = vec![b'a'; 1_024];
                    body[..4].copy_from_slice(&index.to_le_bytes());
                    body
                })
                .collect()
        }

        let mut plain =
            ArchiveWriter::create(Cursor::new(Vec::new()), header(), small_config(), limits())
                .unwrap();
        for body in bodies() {
            plain.append_body(&body).unwrap();
        }
        let plain_len = plain.into_inner().unwrap().get_ref().len();

        let mut paged =
            ArchiveWriter::create(Cursor::new(Vec::new()), header(), small_config(), limits())
                .unwrap();
        paged.enable_metadata_pages();
        let mut expected = Vec::new();
        for body in bodies() {
            expected.push((paged.append_body(&body).unwrap(), body));
        }
        paged.checkpoint().unwrap();
        let paged_bytes = paged.into_inner().unwrap().into_inner();
        assert!(
            paged_bytes.len() < plain_len,
            "paged={} plain={plain_len}",
            paged_bytes.len()
        );
        let mut reader = ArchiveReader::open(Cursor::new(paged_bytes), limits()).unwrap();
        assert_eq!(reader.open_path(), OpenPath::Checkpoint);
        for (id, body) in expected {
            assert_eq!(reader.read_body(&id).unwrap(), body);
        }
    }

    #[test]
    fn mixed_metadata_page_round_trips_through_footer_and_forward_recovery() {
        let mut writer =
            ArchiveWriter::create(Cursor::new(Vec::new()), header(), small_config(), limits())
                .unwrap();
        writer.enable_metadata_pages();
        let (manifest, headers, stream, stage, exchange) =
            append_mixed_metadata(&mut writer, b"mixed-trace");
        writer.seal().unwrap();
        let sealed = writer.into_inner().unwrap().into_inner();

        let mut physical = Cursor::new(sealed.as_slice());
        read_file_header(&mut physical, &limits()).unwrap();
        let mixed_types = loop {
            let (_, frame) = FrameReader::new(&mut physical, &limits())
                .read_next()
                .unwrap();
            let frame = frame.expect("metadata page exists before the footer");
            if frame.record_type == RecordType::MetadataPage {
                break MetadataPage::decode(&frame.payload, &limits())
                    .unwrap()
                    .frames(&limits())
                    .unwrap()
                    .into_iter()
                    .map(|inner| inner.record_type)
                    .collect::<Vec<_>>();
            }
        };
        assert_eq!(
            mixed_types,
            vec![
                RecordType::BodyManifest,
                RecordType::HeaderBlock,
                RecordType::StreamIndex,
                RecordType::Stage,
                RecordType::Exchange,
            ]
        );

        let mut footer_reader = ArchiveReader::open(Cursor::new(sealed.clone()), limits()).unwrap();
        assert_eq!(footer_reader.open_path(), OpenPath::Footer);
        assert_eq!(
            footer_reader.read_body(&manifest).unwrap(),
            b"data: {\"type\":\"done\"}\n\n"
        );
        assert!(footer_reader.header_block(&headers).is_some());
        assert!(footer_reader.stream_index(&stream).is_some());
        assert!(footer_reader.stage(&stage).is_some());
        assert_eq!(
            footer_reader
                .exchange_by_trace(b"mixed-trace")
                .map(|value| value.id),
            Some(exchange)
        );

        let mut without_footer = sealed;
        without_footer.truncate(without_footer.len() - FOOTER_TRAILER_LEN as usize);
        let mut recovered = ArchiveReader::open(Cursor::new(without_footer), limits()).unwrap();
        assert_eq!(recovered.open_path(), OpenPath::ForwardScan);
        assert_eq!(
            recovered.read_body(&manifest).unwrap(),
            b"data: {\"type\":\"done\"}\n\n"
        );
        assert!(recovered.header_block(&headers).is_some());
        assert!(recovered.stream_index(&stream).is_some());
        assert!(recovered.stage(&stage).is_some());
        assert_eq!(
            recovered
                .exchange_by_trace(b"mixed-trace")
                .map(|value| value.id),
            Some(exchange)
        );
    }

    #[test]
    fn mixed_metadata_page_round_trips_through_active_checkpoint_and_append() {
        let mut writer =
            ArchiveWriter::create(Cursor::new(Vec::new()), header(), small_config(), limits())
                .unwrap();
        writer.enable_metadata_pages();
        let (_, headers, stream, stage, first_exchange) =
            append_mixed_metadata(&mut writer, b"checkpoint-mixed-1");
        writer.checkpoint().unwrap();
        let bytes = writer.into_inner().unwrap().into_inner();
        let reader = ArchiveReader::open(Cursor::new(bytes.clone()), limits()).unwrap();
        assert_eq!(reader.open_path(), OpenPath::Checkpoint);
        assert!(reader.header_block(&headers).is_some());
        assert!(reader.stream_index(&stream).is_some());
        assert!(reader.stage(&stage).is_some());
        assert_eq!(
            reader
                .exchange_by_trace(b"checkpoint-mixed-1")
                .map(|value| value.id),
            Some(first_exchange)
        );

        let mut writer =
            ArchiveWriter::open_append(Cursor::new(bytes), small_config(), limits()).unwrap();
        writer.enable_metadata_pages();
        let (_, _, _, _, second_exchange) =
            append_mixed_metadata(&mut writer, b"checkpoint-mixed-2");
        writer.checkpoint().unwrap();
        let reader = ArchiveReader::open(writer.into_inner().unwrap(), limits()).unwrap();
        assert_eq!(reader.open_path(), OpenPath::Checkpoint);
        assert_eq!(
            reader
                .exchange_by_trace(b"checkpoint-mixed-1")
                .map(|value| value.id),
            Some(first_exchange)
        );
        assert_eq!(
            reader
                .exchange_by_trace(b"checkpoint-mixed-2")
                .map(|value| value.id),
            Some(second_exchange)
        );
    }

    #[test]
    fn mixed_pages_compress_repeated_header_and_stage_metadata() {
        fn write(paged: bool) -> usize {
            let mut writer =
                ArchiveWriter::create(Cursor::new(Vec::new()), header(), small_config(), limits())
                    .unwrap();
            if paged {
                writer.enable_metadata_pages();
            }
            for index in 0..400u64 {
                let block = HeaderBlock::new(
                    HeaderFidelity::Exact,
                    vec![
                        HeaderAtom {
                            original_name: b"Content-Type".to_vec(),
                            value: b"application/json; charset=utf-8".to_vec(),
                            flags: 0,
                        },
                        HeaderAtom {
                            original_name: b"X-Alexandria-Trace-Id".to_vec(),
                            value: format!("trace-{index:08}").into_bytes(),
                            flags: 0,
                        },
                    ],
                );
                let header_id = writer.append_header_block(block).unwrap();
                let mut stage = StageData::new(StageKind::ClientResponse, index);
                stage.response_headers_ref = Some(header_id);
                stage.provider = Some(b"repeated-provider-name".to_vec());
                stage.routed_model = Some(b"repeated-model-name".to_vec());
                writer.append_stage(Stage::new(stage)).unwrap();
            }
            writer.into_inner().unwrap().get_ref().len()
        }

        let plain = write(false);
        let paged = write(true);
        assert!(paged < plain / 2, "paged={paged} plain={plain}");
    }

    #[test]
    fn mixed_page_recovery_rejects_dependency_reordering() {
        let block = HeaderBlock::new(
            HeaderFidelity::Exact,
            vec![HeaderAtom {
                original_name: b"Content-Type".to_vec(),
                value: b"application/json".to_vec(),
                flags: 0,
            }],
        );
        let mut stage_data = StageData::new(StageKind::ClientRequest, 1);
        stage_data.request_headers_ref = Some(block.id);
        let stage = Stage::new(stage_data);
        let mut inner = Vec::new();
        push_inner_frame(&mut inner, RecordType::Stage, stage.encode()).unwrap();
        push_inner_frame(&mut inner, RecordType::HeaderBlock, block.encode()).unwrap();
        let page = MetadataPage::encode(inner, 2, 3).unwrap();
        let mut file = Cursor::new(Vec::new());
        write_file_header(&mut file, &header()).unwrap();
        RecordFrame {
            record_type: RecordType::MetadataPage,
            schema_version: RECORD_SCHEMA_V1,
            flags: RecordFrame::REQUIRED,
            payload: page,
            offset: file.position(),
        }
        .write(&mut file)
        .unwrap();
        file.set_position(0);

        assert!(matches!(
            ArchiveReader::open(file, limits()),
            Err(Error::Missing(id)) if id == block.id.to_string()
        ));
    }

    #[test]
    fn mixed_page_recovery_rejects_unsupported_inner_schema() {
        let block = HeaderBlock::new(HeaderFidelity::Exact, Vec::new());
        let mut inner = Vec::new();
        RecordFrame {
            record_type: RecordType::HeaderBlock,
            schema_version: RECORD_SCHEMA_V1 + 1,
            flags: RecordFrame::REQUIRED,
            payload: block.encode(),
            offset: 0,
        }
        .write(&mut inner)
        .unwrap();
        let page = MetadataPage::encode(inner, 1, 3).unwrap();
        let mut file = Cursor::new(Vec::new());
        write_file_header(&mut file, &header()).unwrap();
        RecordFrame {
            record_type: RecordType::MetadataPage,
            schema_version: RECORD_SCHEMA_V1,
            flags: RecordFrame::REQUIRED,
            payload: page,
            offset: file.position(),
        }
        .write(&mut file)
        .unwrap();
        file.set_position(0);

        assert!(matches!(
            ArchiveReader::open(file, limits()),
            Err(Error::Invalid(
                "metadata page contains an unsupported canonical record"
            ))
        ));
    }

    #[test]
    fn self_contained_dictionary_page_keeps_fast_random_access() {
        let dictionary = b"manifest json messages role content tool result headers".repeat(32);
        let mut writer = ArchiveWriter::create_with_metadata_dictionary(
            Cursor::new(Vec::new()),
            header(),
            small_config(),
            limits(),
            dictionary,
            b"test-dictionary".to_vec(),
        )
        .unwrap();
        let id = writer.append_body(&vec![b'x'; 4_096]).unwrap();
        writer.checkpoint().unwrap();
        let mut reader = ArchiveReader::open(writer.into_inner().unwrap(), limits()).unwrap();
        assert_eq!(reader.open_path(), OpenPath::Checkpoint);
        assert_eq!(reader.read_body(&id).unwrap(), vec![b'x'; 4_096]);
        assert_eq!(reader.header().dictionaries.len(), 1);
    }

    #[test]
    fn corrupt_footer_and_index_fall_back_without_a_clean_status() {
        let mut writer =
            ArchiveWriter::create(Cursor::new(Vec::new()), header(), small_config(), limits())
                .unwrap();
        let id = writer.append_body(b"recover canonical body").unwrap();
        writer.seal().unwrap();
        let original = writer.into_inner().unwrap().into_inner();

        let mut bad_footer = original.clone();
        let footer_start = bad_footer.len() - FOOTER_TRAILER_LEN as usize;
        bad_footer[footer_start + 20] ^= 1;
        let mut reader = ArchiveReader::open(Cursor::new(bad_footer), limits()).unwrap();
        assert_eq!(reader.open_path(), OpenPath::ForwardScan);
        assert!(matches!(
            reader.recovery_status(),
            RecoveryStatus::CorruptIndexFallback { .. }
        ));
        assert_eq!(reader.read_body(&id).unwrap(), b"recover canonical body");

        let pointer = CheckpointPointer::from_footer_trailer(
            &original[original.len() - FOOTER_TRAILER_LEN as usize..],
        )
        .unwrap();
        let mut indexed = Cursor::new(original.clone());
        let (checkpoint_frame, _) =
            read_frame_at(&mut indexed, &limits(), pointer.frame_offset).unwrap();
        let checkpoint = Checkpoint::decode(&checkpoint_frame.payload, 10_000).unwrap();
        let mut bad_index = original;
        let block_payload = checkpoint.blocks[0].frame_offset as usize + 20;
        bad_index[block_payload] ^= 1;
        let mut reader = ArchiveReader::open(Cursor::new(bad_index), limits()).unwrap();
        assert_eq!(reader.open_path(), OpenPath::ForwardScan);
        assert!(matches!(
            reader.recovery_status(),
            RecoveryStatus::CorruptIndexFallback { .. }
        ));
        assert_eq!(reader.read_body(&id).unwrap(), b"recover canonical body");
    }

    #[test]
    fn every_footer_truncation_boundary_recovers_canonical_records() {
        let mut writer =
            ArchiveWriter::create(Cursor::new(Vec::new()), header(), small_config(), limits())
                .unwrap();
        let id = writer.append_body(b"survives every footer cut").unwrap();
        let canonical_end = writer.io.get_ref().len();
        writer.seal().unwrap();
        let complete = writer.into_inner().unwrap().into_inner();
        for cut in canonical_end + 1..complete.len() {
            let mut bytes = complete.clone();
            bytes.truncate(cut);
            let mut reader = ArchiveReader::open(Cursor::new(bytes), limits())
                .unwrap_or_else(|error| panic!("cut={cut}: {error}"));
            assert_ne!(reader.recovery_status(), RecoveryStatus::Clean, "cut={cut}");
            assert_eq!(
                reader.read_body(&id).unwrap(),
                b"survives every footer cut",
                "cut={cut}"
            );
        }
    }

    #[test]
    fn predecessor_ranges_store_only_literals_and_reconstruct_normally() {
        let mut writer =
            ArchiveWriter::create(Cursor::new(Vec::new()), header(), small_config(), limits())
                .unwrap();
        let mut base = b"{\"messages\":[".to_vec();
        base.extend((0..20_000).map(|index| (index % 251) as u8));
        base.extend_from_slice(b"]}");
        let first = writer.append_body(&base).unwrap();
        let before = writer.chunk_uncompressed_bytes();
        let insertion = b",{\"role\":\"user\",\"content\":\"one genuinely new turn\"}";
        let split = base.len() - 2;
        let mut current = base[..split].to_vec();
        current.extend_from_slice(insertion);
        current.extend_from_slice(&base[split..]);
        let second = writer
            .append_body_with_predecessor(&current, first)
            .unwrap();
        let chunk_count = writer.chunk_count();
        assert_eq!(writer.append_body(&current).unwrap(), second);
        assert_eq!(writer.chunk_count(), chunk_count);
        let newly_stored = writer.chunk_uncompressed_bytes() - before;
        assert!(
            newly_stored <= insertion.len() as u64 + 256,
            "{newly_stored}"
        );
        let manifest = writer.manifests.get(&second).unwrap();
        assert!(manifest
            .chunks
            .iter()
            .any(|reference| reference.chunk_offset != 0));
        assert!(manifest
            .chunks
            .iter()
            .all(|reference| writer.chunks.contains_key(&reference.chunk_hash)));

        let mut reader = ArchiveReader::open(writer.into_inner().unwrap(), limits()).unwrap();
        assert_eq!(reader.read_body(&first).unwrap(), base);
        assert_eq!(reader.read_body(&second).unwrap(), current);
    }

    #[test]
    fn predecessor_range_append_survives_reopen() {
        let mut writer =
            ArchiveWriter::create(Cursor::new(Vec::new()), header(), small_config(), limits())
                .unwrap();
        let base = vec![b'a'; 20_000];
        let first = writer.append_body(&base).unwrap();
        let bytes = writer.into_inner().unwrap().into_inner();
        let mut writer =
            ArchiveWriter::open_append(Cursor::new(bytes), small_config(), limits()).unwrap();
        let mut current = base;
        current.splice(10_000..10_000, b"new middle".iter().copied());
        let second = writer
            .append_body_with_predecessor(&current, first)
            .unwrap();
        let mut reader = ArchiveReader::open(writer.into_inner().unwrap(), limits()).unwrap();
        assert_eq!(reader.read_body(&second).unwrap(), current);
    }

    #[test]
    fn malformed_predecessor_range_is_rejected_before_write() {
        let mut writer =
            ArchiveWriter::create(Cursor::new(Vec::new()), header(), small_config(), limits())
                .unwrap();
        let id = writer.append_body(&vec![1; 1024]).unwrap();
        let original = writer.manifests.get(&id).unwrap().clone();
        let mut chunks = original.chunks;
        chunks[0].chunk_offset = 10_000;
        let malformed = BodyManifest::new(
            original.total_length,
            original.whole_body_hash,
            None,
            None,
            chunks,
        );
        assert!(matches!(
            writer.append_manifest(malformed),
            Err(Error::Invalid("manifest range exceeds chunk"))
                | Err(Error::Invalid("predecessor range exceeds chunk"))
        ));
    }

    #[test]
    fn upgrade_preserves_full_canonical_graph_and_rebuilds_footer() {
        let mut source_header = header();
        source_header.required_feature_bits |= REQUIRED_FEATURE_CONVERSATION_DAG;
        let dictionary = br#"{"role":"assistant","content":"tool"}"#.to_vec();
        let mut writer = ArchiveWriter::create_with_metadata_dictionary(
            Cursor::new(Vec::new()),
            source_header,
            small_config(),
            limits(),
            dictionary,
            b"fixture-json".to_vec(),
        )
        .unwrap();
        let body = b"data: one\n\ndata: two\n\n";
        let body_id = writer.append_body(body).unwrap();
        let headers = HeaderBlock::new(
            HeaderFidelity::Exact,
            vec![
                HeaderAtom {
                    original_name: b"Set-Cookie".to_vec(),
                    value: b"a=1".to_vec(),
                    flags: 7,
                },
                HeaderAtom {
                    original_name: b"Set-Cookie".to_vec(),
                    value: b"b=2".to_vec(),
                    flags: 9,
                },
            ],
        );
        let header_id = writer.append_header_block(headers.clone()).unwrap();
        let stream = StreamIndex::new(
            body_id,
            vec![
                StreamRead {
                    byte_offset: 0,
                    byte_length: 11,
                    delta_from_first_byte_ns: 0,
                },
                StreamRead {
                    byte_offset: 11,
                    byte_length: 11,
                    delta_from_first_byte_ns: 500,
                },
            ],
            vec![ParsedFrame {
                byte_offset: 0,
                byte_length: 11,
                delta_from_first_byte_ns: 0,
                parser: StreamParser::Sse,
                frame_kind: StreamFrameKind::SseEvent,
            }],
        );
        let stream_id = writer.append_stream_index(stream.clone()).unwrap();
        let mut stage_data = StageData::new(StageKind::UpstreamResponse, 456);
        stage_data.attempt_number = Some(1);
        stage_data.response_headers_ref = Some(header_id);
        stage_data.response_body_manifest_ref = Some(body_id);
        stage_data.stream_index_ref = Some(stream_id);
        let stage = Stage::new(stage_data);
        let stage_id = writer.append_stage(stage.clone()).unwrap();
        let mut exchange_data =
            ExchangeData::new(b"upgrade-trace".to_vec(), 77, 456, vec![stage_id]);
        exchange_data.session_id = Some(b"upgrade-session".to_vec());
        let exchange = Exchange::new(exchange_data);
        let exchange_id = writer.append_exchange(exchange.clone()).unwrap();
        let entry =
            ConversationEntry::new(ConversationEntryData::raw_only(vec![ArtifactRangeRef {
                manifest_id: body_id,
                byte_offset: 0,
                byte_length: 11,
            }]));
        let entry_id = writer.append_conversation_entry(entry.clone()).unwrap();
        let generation = Generation::new(GenerationData {
            parent_generation_id: None,
            entries: vec![entry_id],
            reason: GenerationReason::Initial,
        });
        let generation_id = writer.append_generation(generation.clone()).unwrap();
        let turn = TurnView::new(TurnViewData {
            trace_id: b"upgrade-trace".to_vec(),
            generation_id,
            upto_index: 0,
            response_entry_refs: vec![entry_id],
        });
        let turn_id = writer.append_turn_view(turn.clone()).unwrap();
        writer.seal().unwrap();
        let source_bytes = writer.into_inner().unwrap().into_inner();

        let mut source = Cursor::new(source_bytes);
        let (output, report) = upgrade_archive(
            &mut source,
            Cursor::new(Vec::new()),
            [8; 16],
            999,
            b"upgrade-test".to_vec(),
            limits(),
        )
        .unwrap();
        assert_eq!(report.source_uuid, [9; 16]);
        assert_eq!(report.output_uuid, [8; 16]);
        assert_eq!(report.manifests_verified, 1);
        assert!(report.canonical_records_copied >= 3);
        assert!(report.derived_records_replaced >= 2);

        let mut output = Cursor::new(output.into_inner());
        source.rewind().unwrap();
        verify_upgraded_archive(&mut source, &mut output, limits()).unwrap();
        output.rewind().unwrap();
        let mut reader = ArchiveReader::open(&mut output, limits()).unwrap();
        assert_eq!(reader.open_path(), OpenPath::Footer);
        assert_eq!(reader.header().file_role, crate::FileRole::Standalone);
        assert_eq!(reader.header().file_uuid, [8; 16]);
        assert_eq!(reader.header().created_at_ns, 999);
        assert_eq!(reader.header().writer, b"upgrade-test");
        assert_eq!(reader.header_block(&header_id), Some(&headers));
        assert_eq!(reader.stream_index(&stream_id), Some(&stream));
        assert_eq!(reader.stage(&stage_id), Some(&stage));
        assert_eq!(reader.exchange(&exchange_id), Some(&exchange));
        assert_eq!(reader.conversation_entry(&entry_id), Some(&entry));
        assert_eq!(reader.generation(&generation_id), Some(&generation));
        assert_eq!(reader.turn_view(&turn_id), Some(&turn));
        assert_eq!(reader.read_body(&body_id).unwrap(), body);
        assert_eq!(
            reader.exchanges_for_session(b"upgrade-session"),
            Some([exchange_id].as_slice())
        );
    }

    #[test]
    fn upgrade_rejects_unsealed_future_and_unpreservable_optional_data() {
        let mut unsealed =
            ArchiveWriter::create(Cursor::new(Vec::new()), header(), small_config(), limits())
                .unwrap();
        unsealed.append_body(b"body").unwrap();
        let mut unsealed = unsealed.into_inner().unwrap();
        assert!(upgrade_archive(
            &mut unsealed,
            Cursor::new(Vec::new()),
            [1; 16],
            1,
            b"test".to_vec(),
            limits(),
        )
        .is_err());

        let mut future_header = header();
        future_header.container_minor = crate::DEFAULT_CONTAINER_MINOR + 1;
        let mut future = ArchiveWriter::create(
            Cursor::new(Vec::new()),
            future_header,
            small_config(),
            limits(),
        )
        .unwrap();
        future.seal().unwrap();
        let mut future = future.into_inner().unwrap();
        assert!(matches!(
            upgrade_archive(
                &mut future,
                Cursor::new(Vec::new()),
                [1; 16],
                1,
                b"test".to_vec(),
                limits(),
            ),
            Err(Error::Unsupported(_))
        ));

        let mut optional_header = header();
        optional_header.optional_feature_bits = 1;
        let mut optional = ArchiveWriter::create(
            Cursor::new(Vec::new()),
            optional_header,
            small_config(),
            limits(),
        )
        .unwrap();
        optional.seal().unwrap();
        let mut optional = optional.into_inner().unwrap();
        assert!(matches!(
            upgrade_archive(
                &mut optional,
                Cursor::new(Vec::new()),
                [1; 16],
                1,
                b"test".to_vec(),
                limits(),
            ),
            Err(Error::Unsupported(_))
        ));
    }

    #[test]
    fn upgrade_preserves_unknown_optional_frames_and_rejects_dictionary_mismatch() {
        let mut writer =
            ArchiveWriter::create(Cursor::new(Vec::new()), header(), small_config(), limits())
                .unwrap();
        RecordFrame {
            record_type: RecordType::Unknown(900),
            schema_version: RECORD_SCHEMA_V1,
            flags: 0,
            payload: b"future".to_vec(),
            offset: 0,
        }
        .write(writer.get_mut())
        .unwrap();
        writer.seal().unwrap();
        let mut unknown = writer.into_inner().unwrap();
        assert!(matches!(
            validate_selective_rewrite_source(&mut unknown, limits()),
            Err(Error::Unsupported(_))
        ));
        let (upgraded, _) = upgrade_archive(
            &mut unknown,
            Cursor::new(Vec::new()),
            [1; 16],
            1,
            b"test".to_vec(),
            limits(),
        )
        .unwrap();
        let mut upgraded = Cursor::new(upgraded.into_inner());
        unknown.rewind().unwrap();
        verify_upgraded_archive(&mut unknown, &mut upgraded, limits()).unwrap();

        let mut writer =
            ArchiveWriter::create(Cursor::new(Vec::new()), header(), small_config(), limits())
                .unwrap();
        RecordFrame {
            record_type: RecordType::HeaderBlock,
            schema_version: RECORD_SCHEMA_V1 + 1,
            flags: 0,
            payload: b"future schema".to_vec(),
            offset: 0,
        }
        .write(writer.get_mut())
        .unwrap();
        writer.seal().unwrap();
        let mut future_schema = writer.into_inner().unwrap();
        assert!(matches!(
            upgrade_archive(
                &mut future_schema,
                Cursor::new(Vec::new()),
                [1; 16],
                1,
                b"test".to_vec(),
                limits(),
            ),
            Err(Error::Unsupported(_))
        ));

        let bytes = b"dictionary bytes".to_vec();
        let dictionary = StoredDictionary::new(bytes.clone());
        let mut mismatched_header = header();
        mismatched_header
            .dictionaries
            .push(crate::DictionaryDescriptor {
                id: dictionary.id,
                uncompressed_length: bytes.len() as u64,
                name: b"missing".to_vec(),
            });
        let mut mismatch = ArchiveWriter::create(
            Cursor::new(Vec::new()),
            mismatched_header,
            small_config(),
            limits(),
        )
        .unwrap();
        mismatch.seal().unwrap();
        let mut mismatch = mismatch.into_inner().unwrap();
        assert!(matches!(
            upgrade_archive(
                &mut mismatch,
                Cursor::new(Vec::new()),
                [1; 16],
                1,
                b"test".to_vec(),
                limits(),
            ),
            Err(Error::Invalid(_))
        ));
    }

    #[test]
    fn selective_rewrite_accepts_supported_dictionary_pages() {
        let dictionary = b"manifest messages roles tool results headers".repeat(32);
        let mut writer = ArchiveWriter::create_with_metadata_dictionary(
            Cursor::new(Vec::new()),
            header(),
            small_config(),
            limits(),
            dictionary,
            b"selective-rewrite-test".to_vec(),
        )
        .unwrap();
        writer.append_body(&vec![b'x'; 8_192]).unwrap();
        writer.seal().unwrap();
        let mut source = writer.into_inner().unwrap();
        validate_selective_rewrite_source(&mut source, limits()).unwrap();
    }

    #[test]
    fn upgrade_rejects_even_current_minor_header_extensions() {
        let mut bytes = Vec::new();
        write_file_header(&mut bytes, &header()).unwrap();
        let old_payload_len = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
        bytes.truncate(12 + old_payload_len);
        bytes.push(0xa5);
        bytes[8..12].copy_from_slice(&((old_payload_len + 1) as u32).to_le_bytes());
        let checksum = crate::format::checksum_parts(&[&bytes[..12], &bytes[12..]]);
        bytes.extend_from_slice(&checksum.to_le_bytes());
        let mut writer =
            ArchiveWriter::open_append(Cursor::new(bytes), small_config(), limits()).unwrap();
        writer.append_body(b"header extension fixture").unwrap();
        writer.seal().unwrap();
        let mut extended = writer.into_inner().unwrap();
        assert!(matches!(
            upgrade_archive(
                &mut extended,
                Cursor::new(Vec::new()),
                [1; 16],
                1,
                b"test".to_vec(),
                limits(),
            ),
            Err(Error::Unsupported(_))
        ));
    }

    proptest! {
        #[test]
        fn arbitrary_bodies_round_trip(data in proptest::collection::vec(any::<u8>(), 0..100_000)) {
            let mut writer=ArchiveWriter::create(Cursor::new(Vec::new()), header(), small_config(), limits()).unwrap();
            let id=writer.append_body(&data).unwrap(); let mut reader=ArchiveReader::open(writer.into_inner().unwrap(), limits()).unwrap();
            prop_assert_eq!(reader.read_body(&id).unwrap(), data);
        }


        #[test]
        fn predecessor_mutations_round_trip(
            base in proptest::collection::vec(any::<u8>(), 512..20_000),
            insertion in proptest::collection::vec(any::<u8>(), 0..512),
            split_seed in any::<usize>(),
        ) {
            let split = split_seed % (base.len() + 1);
            let mut current = base[..split].to_vec();
            current.extend_from_slice(&insertion);
            current.extend_from_slice(&base[split..]);
            let mut writer = ArchiveWriter::create(
                Cursor::new(Vec::new()),
                header(),
                small_config(),
                limits(),
            ).unwrap();
            let first = writer.append_body(&base).unwrap();
            let second = writer.append_body_with_predecessor(&current, first).unwrap();
            let mut reader = ArchiveReader::open(writer.into_inner().unwrap(), limits()).unwrap();
            prop_assert_eq!(reader.read_body(&second).unwrap(), current);
        }
    }
}
