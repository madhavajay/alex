use crate::{Error, Result};
use crc32fast::Hasher;
use std::io::{Read, Seek, SeekFrom, Write};

pub const DEFAULT_CONTAINER_MAJOR: u16 = 1;
pub const DEFAULT_CONTAINER_MINOR: u16 = 0;
/// Stage body IDs may resolve through the archive-set catalog instead of a
/// manifest record in the same physical file. Standalone files never use it.
pub const REQUIRED_FEATURE_ARCHIVE_SET_BODY_REFS: u64 = 1 << 0;
/// Canonical conversation-entry, generation, and turn-view records are present.
pub const REQUIRED_FEATURE_CONVERSATION_DAG: u64 = 1 << 1;
const SUPPORTED_REQUIRED_FEATURE_BITS: u64 =
    REQUIRED_FEATURE_ARCHIVE_SET_BODY_REFS | REQUIRED_FEATURE_CONVERSATION_DAG;
const FILE_MAGIC: [u8; 4] = *b"LAR1";
const FRAME_MAGIC: [u8; 4] = *b"LREC";
const FRAME_PREFIX_LEN: usize = 4 + 2 + 2 + 4 + 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum FileRole {
    BodyPack = 1,
    EventLog = 2,
    Standalone = 3,
    SearchPack = 4,
    Dictionary = 5,
}

impl TryFrom<u8> for FileRole {
    type Error = Error;
    fn try_from(value: u8) -> Result<Self> {
        match value {
            1 => Ok(Self::BodyPack),
            2 => Ok(Self::EventLog),
            3 => Ok(Self::Standalone),
            4 => Ok(Self::SearchPack),
            5 => Ok(Self::Dictionary),
            _ => Err(Error::Unsupported(format!("file role {value}"))),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[repr(u8)]
pub enum HashAlgorithm {
    Blake3 = 1,
}

impl TryFrom<u8> for HashAlgorithm {
    type Error = Error;
    fn try_from(value: u8) -> Result<Self> {
        match value {
            1 => Ok(Self::Blake3),
            _ => Err(Error::Unsupported(format!("hash algorithm {value}"))),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DictionaryDescriptor {
    pub id: [u8; 32],
    pub uncompressed_length: u64,
    pub name: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileHeader {
    pub container_major: u16,
    pub container_minor: u16,
    pub file_uuid: [u8; 16],
    pub file_role: FileRole,
    pub created_at_ns: u64,
    pub writer: Vec<u8>,
    pub required_feature_bits: u64,
    pub optional_feature_bits: u64,
    pub default_hash_algorithm: HashAlgorithm,
    pub zstd_level: i8,
    pub dictionaries: Vec<DictionaryDescriptor>,
}

impl FileHeader {
    pub fn body_pack(file_uuid: [u8; 16], created_at_ns: u64, writer: impl Into<Vec<u8>>) -> Self {
        let mut header = Self::standalone(file_uuid, created_at_ns, writer);
        header.file_role = FileRole::BodyPack;
        header
    }

    pub fn standalone(file_uuid: [u8; 16], created_at_ns: u64, writer: impl Into<Vec<u8>>) -> Self {
        Self {
            container_major: DEFAULT_CONTAINER_MAJOR,
            container_minor: DEFAULT_CONTAINER_MINOR,
            file_uuid,
            file_role: FileRole::Standalone,
            created_at_ns,
            writer: writer.into(),
            required_feature_bits: 0,
            optional_feature_bits: 0,
            default_hash_algorithm: HashAlgorithm::Blake3,
            zstd_level: 3,
            dictionaries: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Limits {
    pub max_header_length: u32,
    pub max_frame_payload: u64,
    pub max_chunk_uncompressed: u64,
    pub max_body_length: u64,
    pub max_manifest_chunks: u32,
    pub max_header_atoms: u32,
    pub max_field_length: u32,
    pub max_dictionaries: u32,
    pub max_identifier_length: u32,
    pub max_stream_reads: u32,
    pub max_stream_frames: u32,
    pub max_exchange_stages: u32,
    pub max_stream_indexes: u32,
    pub max_stages: u32,
    pub max_exchanges: u32,
    pub max_session_exchanges: u32,
    pub max_metadata_page_uncompressed: u64,
    pub max_conversation_entry_ranges: u32,
    pub max_generation_entries: u32,
    pub max_turn_response_entries: u32,
    pub max_conversation_entries: u32,
    pub max_generations: u32,
    pub max_turn_views: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_header_length: 1024 * 1024,
            max_frame_payload: 2 * 1024 * 1024,
            max_chunk_uncompressed: 128 * 1024,
            max_body_length: 16 * 1024 * 1024 * 1024,
            max_manifest_chunks: 2_000_000,
            max_header_atoms: 65_536,
            max_field_length: 1024 * 1024,
            max_dictionaries: 256,
            max_identifier_length: 4 * 1024,
            max_stream_reads: 4_000_000,
            max_stream_frames: 4_000_000,
            max_exchange_stages: 65_536,
            max_stream_indexes: 4_000_000,
            max_stages: 16_000_000,
            max_exchanges: 4_000_000,
            max_session_exchanges: 1_000_000,
            max_metadata_page_uncompressed: 2 * 1024 * 1024,
            max_conversation_entry_ranges: 65_536,
            max_generation_entries: 1_000_000,
            max_turn_response_entries: 65_536,
            max_conversation_entries: 16_000_000,
            max_generations: 4_000_000,
            max_turn_views: 4_000_000,
        }
    }
}

pub fn write_file_header<W: Write>(output: &mut W, header: &FileHeader) -> Result<u64> {
    let mut payload = Vec::new();
    payload.extend_from_slice(&header.file_uuid);
    payload.push(header.file_role as u8);
    payload.extend_from_slice(&header.created_at_ns.to_le_bytes());
    put_bytes_u16(&mut payload, &header.writer)?;
    payload.extend_from_slice(&header.required_feature_bits.to_le_bytes());
    payload.extend_from_slice(&header.optional_feature_bits.to_le_bytes());
    payload.push(header.default_hash_algorithm as u8);
    payload.push(header.zstd_level as u8);
    let dictionary_count = u16::try_from(header.dictionaries.len())
        .map_err(|_| Error::Invalid("too many dictionaries"))?;
    payload.extend_from_slice(&dictionary_count.to_le_bytes());
    for dictionary in &header.dictionaries {
        payload.extend_from_slice(&dictionary.id);
        payload.extend_from_slice(&dictionary.uncompressed_length.to_le_bytes());
        put_bytes_u16(&mut payload, &dictionary.name)?;
    }
    let payload_len =
        u32::try_from(payload.len()).map_err(|_| Error::Invalid("file header too large"))?;
    let mut prefix = Vec::with_capacity(12);
    prefix.extend_from_slice(&FILE_MAGIC);
    prefix.extend_from_slice(&header.container_major.to_le_bytes());
    prefix.extend_from_slice(&header.container_minor.to_le_bytes());
    prefix.extend_from_slice(&payload_len.to_le_bytes());
    let checksum = checksum_parts(&[&prefix, &payload]);
    output.write_all(&prefix)?;
    output.write_all(&payload)?;
    output.write_all(&checksum.to_le_bytes())?;
    Ok(prefix.len() as u64 + payload.len() as u64 + 4)
}

pub fn read_file_header<R: Read>(input: &mut R, limits: &Limits) -> Result<(FileHeader, u64)> {
    let mut prefix = [0u8; 12];
    input.read_exact(&mut prefix)?;
    if prefix[..4] != FILE_MAGIC {
        return Err(Error::Invalid("missing LAR1 magic"));
    }
    let major = u16::from_le_bytes(prefix[4..6].try_into().unwrap());
    let minor = u16::from_le_bytes(prefix[6..8].try_into().unwrap());
    if major != DEFAULT_CONTAINER_MAJOR {
        return Err(Error::Unsupported(format!(
            "container major version {major}"
        )));
    }
    let payload_len = u32::from_le_bytes(prefix[8..12].try_into().unwrap());
    if payload_len > limits.max_header_length {
        return Err(Error::Limit {
            what: "file header",
            actual: payload_len as u64,
            limit: limits.max_header_length as u64,
        });
    }
    let mut payload = vec![0; payload_len as usize];
    input.read_exact(&mut payload)?;
    let mut expected = [0; 4];
    input.read_exact(&mut expected)?;
    if checksum_parts(&[&prefix, &payload]) != u32::from_le_bytes(expected) {
        return Err(Error::Checksum { offset: 0 });
    }
    let mut cursor = 0usize;
    let file_uuid = take_array::<16>(&payload, &mut cursor)?;
    let file_role = FileRole::try_from(take_u8(&payload, &mut cursor)?)?;
    let created_at_ns = take_u64(&payload, &mut cursor)?;
    let writer = take_bytes_u16(&payload, &mut cursor, limits.max_field_length)?;
    let required_feature_bits = take_u64(&payload, &mut cursor)?;
    let unknown_required = required_feature_bits & !SUPPORTED_REQUIRED_FEATURE_BITS;
    if unknown_required != 0 {
        return Err(Error::Unsupported(format!(
            "required feature bits {unknown_required:#x}"
        )));
    }
    let optional_feature_bits = take_u64(&payload, &mut cursor)?;
    let default_hash_algorithm = HashAlgorithm::try_from(take_u8(&payload, &mut cursor)?)?;
    let zstd_level = take_u8(&payload, &mut cursor)? as i8;
    let count = take_u16(&payload, &mut cursor)? as u32;
    if count > limits.max_dictionaries {
        return Err(Error::Limit {
            what: "dictionary count",
            actual: count as u64,
            limit: limits.max_dictionaries as u64,
        });
    }
    let mut dictionaries = Vec::with_capacity(count as usize);
    for _ in 0..count {
        dictionaries.push(DictionaryDescriptor {
            id: take_array(&payload, &mut cursor)?,
            uncompressed_length: take_u64(&payload, &mut cursor)?,
            name: take_bytes_u16(&payload, &mut cursor, limits.max_field_length)?,
        });
    }
    // Minor versions may extend the bounded header payload; old readers skip
    // the extension after enforcing required feature bits.
    let header = FileHeader {
        container_major: major,
        container_minor: minor,
        file_uuid,
        file_role,
        created_at_ns,
        writer,
        required_feature_bits,
        optional_feature_bits,
        default_hash_algorithm,
        zstd_level,
        dictionaries,
    };
    Ok((header, 12 + payload_len as u64 + 4))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecordType {
    Chunk,
    BodyManifest,
    HeaderBlock,
    StreamIndex,
    Stage,
    Exchange,
    /// One bounded shard of a persisted archive index. Index records are
    /// derived metadata: an older reader may safely skip them and rebuild by
    /// scanning the canonical records.
    IndexBlock,
    /// Root record for a consistent set of index blocks.
    Checkpoint,
    /// Fixed-size pointer to the immediately preceding checkpoint. This is a
    /// normal record so later appends remain a valid forward-scannable stream.
    CheckpointLocator,
    /// Self-contained compression dictionary bytes referenced by later
    /// independently decompressible records.
    DictionaryData,
    /// Independently compressed canonical metadata records.
    MetadataPage,
    ConversationEntry,
    Generation,
    TurnView,
    Unknown(u16),
}
impl RecordType {
    pub fn code(self) -> u16 {
        match self {
            Self::Chunk => 1,
            Self::BodyManifest => 2,
            Self::HeaderBlock => 3,
            Self::StreamIndex => 4,
            Self::Stage => 5,
            Self::Exchange => 6,
            Self::IndexBlock => 7,
            Self::Checkpoint => 8,
            Self::CheckpointLocator => 9,
            Self::DictionaryData => 10,
            Self::MetadataPage => 11,
            Self::ConversationEntry => 12,
            Self::Generation => 13,
            Self::TurnView => 14,
            Self::Unknown(value) => value,
        }
    }
}
impl From<u16> for RecordType {
    fn from(value: u16) -> Self {
        match value {
            1 => Self::Chunk,
            2 => Self::BodyManifest,
            3 => Self::HeaderBlock,
            4 => Self::StreamIndex,
            5 => Self::Stage,
            6 => Self::Exchange,
            7 => Self::IndexBlock,
            8 => Self::Checkpoint,
            9 => Self::CheckpointLocator,
            10 => Self::DictionaryData,
            11 => Self::MetadataPage,
            12 => Self::ConversationEntry,
            13 => Self::Generation,
            14 => Self::TurnView,
            other => Self::Unknown(other),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecordFrame {
    pub record_type: RecordType,
    pub schema_version: u16,
    pub flags: u32,
    pub payload: Vec<u8>,
    pub offset: u64,
}

impl RecordFrame {
    pub const REQUIRED: u32 = 1;
    pub fn write<W: Write>(&self, output: &mut W) -> Result<u64> {
        let payload_len = self.payload.len() as u64;
        let mut prefix = Vec::with_capacity(FRAME_PREFIX_LEN);
        prefix.extend_from_slice(&FRAME_MAGIC);
        prefix.extend_from_slice(&self.record_type.code().to_le_bytes());
        prefix.extend_from_slice(&self.schema_version.to_le_bytes());
        prefix.extend_from_slice(&self.flags.to_le_bytes());
        prefix.extend_from_slice(&payload_len.to_le_bytes());
        let checksum = checksum_parts(&[&prefix, &self.payload]);
        output.write_all(&prefix)?;
        output.write_all(&self.payload)?;
        output.write_all(&checksum.to_le_bytes())?;
        Ok(prefix.len() as u64 + payload_len + 4)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameRead {
    Frame,
    CleanEof,
    Truncated,
}

pub struct FrameReader<'a, R> {
    input: &'a mut R,
    limits: &'a Limits,
}
impl<'a, R: Read + Seek> FrameReader<'a, R> {
    pub fn new(input: &'a mut R, limits: &'a Limits) -> Self {
        Self { input, limits }
    }
    pub fn read_next(&mut self) -> Result<(FrameRead, Option<RecordFrame>)> {
        let offset = self.input.stream_position()?;
        let mut prefix = [0u8; FRAME_PREFIX_LEN];
        let first = match self.input.read(&mut prefix[..1]) {
            Ok(0) => return Ok((FrameRead::CleanEof, None)),
            Ok(_) => true,
            Err(e) => return Err(e.into()),
        };
        debug_assert!(first);
        if let Err(e) = self.input.read_exact(&mut prefix[1..]) {
            if e.kind() == std::io::ErrorKind::UnexpectedEof {
                return Ok((FrameRead::Truncated, None));
            }
            return Err(e.into());
        }
        if prefix[..4] != FRAME_MAGIC {
            return Err(Error::InvalidDetail(format!(
                "missing record magic at byte {offset}"
            )));
        }
        let record_type = RecordType::from(u16::from_le_bytes(prefix[4..6].try_into().unwrap()));
        let schema_version = u16::from_le_bytes(prefix[6..8].try_into().unwrap());
        let flags = u32::from_le_bytes(prefix[8..12].try_into().unwrap());
        let payload_len = u64::from_le_bytes(prefix[12..20].try_into().unwrap());
        if payload_len > self.limits.max_frame_payload {
            return Err(Error::Limit {
                what: "record payload",
                actual: payload_len,
                limit: self.limits.max_frame_payload,
            });
        }
        let payload_size = usize::try_from(payload_len).map_err(|_| Error::Limit {
            what: "record payload address space",
            actual: payload_len,
            limit: usize::MAX as u64,
        })?;
        let mut payload = vec![0; payload_size];
        if let Err(e) = self.input.read_exact(&mut payload) {
            if e.kind() == std::io::ErrorKind::UnexpectedEof {
                return Ok((FrameRead::Truncated, None));
            }
            return Err(e.into());
        }
        let mut expected = [0; 4];
        if let Err(e) = self.input.read_exact(&mut expected) {
            if e.kind() == std::io::ErrorKind::UnexpectedEof {
                return Ok((FrameRead::Truncated, None));
            }
            return Err(e.into());
        }
        if checksum_parts(&[&prefix, &payload]) != u32::from_le_bytes(expected) {
            return Err(Error::Checksum { offset });
        }
        if matches!(record_type, RecordType::Unknown(_)) && flags & RecordFrame::REQUIRED != 0 {
            return Err(Error::Unsupported(format!(
                "required record type {}",
                record_type.code()
            )));
        }
        Ok((
            FrameRead::Frame,
            Some(RecordFrame {
                record_type,
                schema_version,
                flags,
                payload,
                offset,
            }),
        ))
    }
}

pub(crate) fn checksum_parts(parts: &[&[u8]]) -> u32 {
    let mut hasher = Hasher::new();
    for part in parts {
        hasher.update(part);
    }
    hasher.finalize()
}
fn put_bytes_u16(out: &mut Vec<u8>, value: &[u8]) -> Result<()> {
    let len = u16::try_from(value.len()).map_err(|_| Error::Invalid("header string too long"))?;
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(value);
    Ok(())
}
fn take_array<const N: usize>(data: &[u8], cursor: &mut usize) -> Result<[u8; N]> {
    let end = cursor
        .checked_add(N)
        .ok_or(Error::Invalid("header offset overflow"))?;
    let value = data
        .get(*cursor..end)
        .ok_or(Error::Invalid("truncated file header"))?
        .try_into()
        .unwrap();
    *cursor = end;
    Ok(value)
}
fn take_u8(data: &[u8], cursor: &mut usize) -> Result<u8> {
    Ok(take_array::<1>(data, cursor)?[0])
}
fn take_u16(data: &[u8], cursor: &mut usize) -> Result<u16> {
    Ok(u16::from_le_bytes(take_array(data, cursor)?))
}
fn take_u64(data: &[u8], cursor: &mut usize) -> Result<u64> {
    Ok(u64::from_le_bytes(take_array(data, cursor)?))
}
fn take_bytes_u16(data: &[u8], cursor: &mut usize, max: u32) -> Result<Vec<u8>> {
    let len = take_u16(data, cursor)? as u32;
    if len > max {
        return Err(Error::Limit {
            what: "header field",
            actual: len as u64,
            limit: max as u64,
        });
    }
    let end = cursor
        .checked_add(len as usize)
        .ok_or(Error::Invalid("header offset overflow"))?;
    let value = data
        .get(*cursor..end)
        .ok_or(Error::Invalid("truncated file header"))?
        .to_vec();
    *cursor = end;
    Ok(value)
}

pub(crate) fn file_length<S: Seek>(input: &mut S) -> Result<u64> {
    let current = input.stream_position()?;
    let end = input.seek(SeekFrom::End(0))?;
    input.seek(SeekFrom::Start(current))?;
    Ok(end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn header_round_trip_and_bad_checksum() {
        let header = FileHeader::standalone([7; 16], 42, b"alex-test".to_vec());
        let mut bytes = Vec::new();
        write_file_header(&mut bytes, &header).unwrap();
        let (decoded, used) =
            read_file_header(&mut Cursor::new(&bytes), &Limits::default()).unwrap();
        assert_eq!(decoded, header);
        assert_eq!(used, bytes.len() as u64);
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        assert!(matches!(
            read_file_header(&mut Cursor::new(bytes), &Limits::default()),
            Err(Error::Checksum { .. })
        ));
    }

    #[test]
    fn unknown_optional_frame_is_skippable() {
        let frame = RecordFrame {
            record_type: RecordType::Unknown(900),
            schema_version: 7,
            flags: 0,
            payload: vec![1, 2, 3],
            offset: 0,
        };
        let mut bytes = Vec::new();
        frame.write(&mut bytes).unwrap();
        let mut cursor = Cursor::new(bytes);
        let limits = Limits::default();
        let mut reader = FrameReader::new(&mut cursor, &limits);
        let (_, parsed) = reader.read_next().unwrap();
        assert_eq!(parsed.unwrap().record_type, RecordType::Unknown(900));
    }
}
