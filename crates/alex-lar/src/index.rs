//! Persisted, derived indexes for fast archive open.
//!
//! Index records never own canonical capture bytes. They are deliberately
//! optional records: readers that do not understand this schema can ignore
//! them and recover the same archive state by scanning canonical records.

use crate::{Error, Result};
use crc32fast::Hasher as Crc32;
use std::io::{Cursor, Read};

pub(crate) const INDEX_SCHEMA_V1: u16 = 1;
pub(crate) const LOCATOR_PAYLOAD_LEN: usize = 56;
pub(crate) const LOCATOR_FRAME_LEN: u64 = 20 + LOCATOR_PAYLOAD_LEN as u64 + 4;
pub(crate) const FOOTER_TRAILER_LEN: u64 = 72;

const BLOCK_MAGIC: &[u8; 4] = b"LIDX";
const COMPRESSED_BLOCK_MAGIC: &[u8; 4] = b"LIZ2";
const CHECKPOINT_MAGIC: &[u8; 4] = b"LCP1";
const LOCATOR_MAGIC: &[u8; 4] = b"LCPT";
const FOOTER_MAGIC: &[u8; 8] = b"LARFOOT1";
const TRAILING_MAGIC: &[u8; 8] = b"LAR1END!";
const MAX_INDEX_PAGE_UNCOMPRESSED: u64 = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(u8)]
pub(crate) enum IndexKind {
    Chunk = 1,
    Manifest = 2,
    HeaderBlock = 3,
    StreamIndex = 4,
    Stage = 5,
    Exchange = 6,
    Trace = 7,
    Session = 8,
    ConversationEntry = 9,
    Generation = 10,
    TurnView = 11,
    TurnTrace = 12,
}

impl TryFrom<u8> for IndexKind {
    type Error = Error;

    fn try_from(value: u8) -> Result<Self> {
        match value {
            1 => Ok(Self::Chunk),
            2 => Ok(Self::Manifest),
            3 => Ok(Self::HeaderBlock),
            4 => Ok(Self::StreamIndex),
            5 => Ok(Self::Stage),
            6 => Ok(Self::Exchange),
            7 => Ok(Self::Trace),
            8 => Ok(Self::Session),
            9 => Ok(Self::ConversationEntry),
            10 => Ok(Self::Generation),
            11 => Ok(Self::TurnView),
            12 => Ok(Self::TurnTrace),
            _ => Err(Error::Unsupported(format!("index kind {value}"))),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ChunkIndexEntry {
    pub hash: [u8; 33],
    pub frame_offset: u64,
    pub uncompressed_length: u64,
    pub compressed_length: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum IndexEntries {
    Chunks(Vec<ChunkIndexEntry>),
    IdOffsets(Vec<([u8; 32], u64)>),
    Traces(Vec<(Vec<u8>, [u8; 32])>),
    /// A session may span blocks. Parts are sorted by `(session_id, start)`
    /// and must join without gaps.
    Sessions(Vec<(Vec<u8>, u32, Vec<[u8; 32]>)>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct IndexBlock {
    pub kind: IndexKind,
    pub entries: IndexEntries,
}

impl IndexBlock {
    pub fn encode(&self) -> Result<Vec<u8>> {
        let plain = self.encode_plain()?;
        let compressed =
            zstd::stream::encode_all(std::io::Cursor::new(&plain), 3).map_err(Error::Io)?;
        if compressed.len().saturating_add(20) >= plain.len() {
            return Ok(plain);
        }
        let mut out = Vec::with_capacity(20 + compressed.len());
        out.extend_from_slice(COMPRESSED_BLOCK_MAGIC);
        out.extend_from_slice(&(plain.len() as u64).to_le_bytes());
        out.extend_from_slice(&(compressed.len() as u64).to_le_bytes());
        out.extend_from_slice(&compressed);
        Ok(out)
    }

    fn encode_plain(&self) -> Result<Vec<u8>> {
        validate_entries_kind(self.kind, &self.entries)?;
        let mut out = Vec::new();
        out.extend_from_slice(BLOCK_MAGIC);
        out.extend_from_slice(&INDEX_SCHEMA_V1.to_le_bytes());
        out.push(self.kind as u8);
        out.push(0);
        match &self.entries {
            IndexEntries::Chunks(entries) => {
                put_count(&mut out, entries.len())?;
                for entry in entries {
                    out.extend_from_slice(&entry.hash);
                    out.extend_from_slice(&entry.frame_offset.to_le_bytes());
                    out.extend_from_slice(&entry.uncompressed_length.to_le_bytes());
                    out.extend_from_slice(&entry.compressed_length.to_le_bytes());
                }
            }
            IndexEntries::IdOffsets(entries) => {
                put_count(&mut out, entries.len())?;
                for (id, offset) in entries {
                    out.extend_from_slice(id);
                    out.extend_from_slice(&offset.to_le_bytes());
                }
            }
            IndexEntries::Traces(entries) => {
                put_count(&mut out, entries.len())?;
                for (trace, exchange) in entries {
                    put_bytes(&mut out, trace)?;
                    out.extend_from_slice(exchange);
                }
            }
            IndexEntries::Sessions(entries) => {
                put_count(&mut out, entries.len())?;
                for (session, start, exchanges) in entries {
                    put_bytes(&mut out, session)?;
                    out.extend_from_slice(&start.to_le_bytes());
                    put_count(&mut out, exchanges.len())?;
                    for exchange in exchanges {
                        out.extend_from_slice(exchange);
                    }
                }
            }
        }
        Ok(out)
    }

    pub fn decode(payload: &[u8], max_entries: u32, max_identifier: u32) -> Result<Self> {
        if payload.starts_with(COMPRESSED_BLOCK_MAGIC) {
            if payload.len() < 20 {
                return Err(Error::Invalid("truncated compressed index page"));
            }
            let uncompressed_length = u64::from_le_bytes(payload[4..12].try_into().unwrap());
            let compressed_length = u64::from_le_bytes(payload[12..20].try_into().unwrap());
            if uncompressed_length > MAX_INDEX_PAGE_UNCOMPRESSED {
                return Err(Error::Limit {
                    what: "index page uncompressed bytes",
                    actual: uncompressed_length,
                    limit: MAX_INDEX_PAGE_UNCOMPRESSED,
                });
            }
            if payload.len() as u64 != 20 + compressed_length {
                return Err(Error::Invalid("compressed index page length mismatch"));
            }
            let decoder = zstd::stream::read::Decoder::new(std::io::Cursor::new(&payload[20..]))?;
            let mut bounded = decoder.take(uncompressed_length.saturating_add(1));
            let mut plain = Vec::with_capacity(uncompressed_length as usize);
            bounded.read_to_end(&mut plain)?;
            if plain.len() as u64 != uncompressed_length {
                return Err(Error::Invalid("index page uncompressed length mismatch"));
            }
            return Self::decode_plain(&plain, max_entries, max_identifier);
        }
        Self::decode_plain(payload, max_entries, max_identifier)
    }

    fn decode_plain(payload: &[u8], max_entries: u32, max_identifier: u32) -> Result<Self> {
        let mut input = Cursor::new(payload);
        if read_array::<4>(&mut input)? != *BLOCK_MAGIC {
            return Err(Error::Invalid("missing index block magic"));
        }
        let schema = read_u16(&mut input)?;
        if schema != INDEX_SCHEMA_V1 {
            return Err(Error::Unsupported(format!("index block schema {schema}")));
        }
        let kind = IndexKind::try_from(read_u8(&mut input)?)?;
        if read_u8(&mut input)? != 0 {
            return Err(Error::Invalid("index block reserved byte is nonzero"));
        }
        let count = read_count(&mut input, max_entries, "index entry count")?;
        let entries = match kind {
            IndexKind::Chunk => {
                let mut values = Vec::with_capacity(count);
                for _ in 0..count {
                    values.push(ChunkIndexEntry {
                        hash: read_array(&mut input)?,
                        frame_offset: read_u64(&mut input)?,
                        uncompressed_length: read_u64(&mut input)?,
                        compressed_length: read_u64(&mut input)?,
                    });
                }
                IndexEntries::Chunks(values)
            }
            IndexKind::Manifest
            | IndexKind::HeaderBlock
            | IndexKind::StreamIndex
            | IndexKind::Stage
            | IndexKind::Exchange
            | IndexKind::ConversationEntry
            | IndexKind::Generation
            | IndexKind::TurnView => {
                let mut values = Vec::with_capacity(count);
                for _ in 0..count {
                    values.push((read_array(&mut input)?, read_u64(&mut input)?));
                }
                IndexEntries::IdOffsets(values)
            }
            IndexKind::Trace | IndexKind::TurnTrace => {
                let mut values = Vec::with_capacity(count);
                for _ in 0..count {
                    values.push((
                        read_bytes(&mut input, max_identifier)?,
                        read_array(&mut input)?,
                    ));
                }
                IndexEntries::Traces(values)
            }
            IndexKind::Session => {
                let mut values = Vec::with_capacity(count);
                for _ in 0..count {
                    let session = read_bytes(&mut input, max_identifier)?;
                    let start = read_u32(&mut input)?;
                    let exchange_count =
                        read_count(&mut input, max_entries, "session exchange index count")?;
                    let mut exchanges = Vec::with_capacity(exchange_count);
                    for _ in 0..exchange_count {
                        exchanges.push(read_array(&mut input)?);
                    }
                    values.push((session, start, exchanges));
                }
                IndexEntries::Sessions(values)
            }
        };
        ensure_end(&input, payload)?;
        let block = Self { kind, entries };
        validate_entries_kind(kind, &block.entries)?;
        Ok(block)
    }
}

fn validate_entries_kind(kind: IndexKind, entries: &IndexEntries) -> Result<()> {
    let valid = matches!(
        (kind, entries),
        (IndexKind::Chunk, IndexEntries::Chunks(_))
            | (
                IndexKind::Manifest
                    | IndexKind::HeaderBlock
                    | IndexKind::StreamIndex
                    | IndexKind::Stage
                    | IndexKind::Exchange
                    | IndexKind::ConversationEntry
                    | IndexKind::Generation
                    | IndexKind::TurnView,
                IndexEntries::IdOffsets(_)
            )
            | (
                IndexKind::Trace | IndexKind::TurnTrace,
                IndexEntries::Traces(_)
            )
            | (IndexKind::Session, IndexEntries::Sessions(_))
    );
    if valid {
        Ok(())
    } else {
        Err(Error::Invalid("index kind and entry encoding differ"))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct IndexBlockRef {
    pub kind: IndexKind,
    pub frame_offset: u64,
    pub frame_length: u64,
    pub payload_hash: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Checkpoint {
    /// End of canonical data represented by this snapshot. Index block/root
    /// records follow this point and are never included in their own index.
    pub indexed_end: u64,
    pub record_count: u64,
    pub blocks: Vec<IndexBlockRef>,
}

impl Checkpoint {
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        out.extend_from_slice(CHECKPOINT_MAGIC);
        out.extend_from_slice(&INDEX_SCHEMA_V1.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&self.indexed_end.to_le_bytes());
        out.extend_from_slice(&self.record_count.to_le_bytes());
        put_count(&mut out, self.blocks.len())?;
        for block in &self.blocks {
            out.push(block.kind as u8);
            out.extend_from_slice(&[0; 3]);
            out.extend_from_slice(&block.frame_offset.to_le_bytes());
            out.extend_from_slice(&block.frame_length.to_le_bytes());
            out.extend_from_slice(&block.payload_hash);
        }
        Ok(out)
    }

    pub fn decode(payload: &[u8], max_blocks: u32) -> Result<Self> {
        let mut input = Cursor::new(payload);
        if read_array::<4>(&mut input)? != *CHECKPOINT_MAGIC {
            return Err(Error::Invalid("missing checkpoint magic"));
        }
        let schema = read_u16(&mut input)?;
        if schema != INDEX_SCHEMA_V1 {
            return Err(Error::Unsupported(format!("checkpoint schema {schema}")));
        }
        if read_u16(&mut input)? != 0 {
            return Err(Error::Invalid("checkpoint flags are nonzero"));
        }
        let indexed_end = read_u64(&mut input)?;
        let record_count = read_u64(&mut input)?;
        let block_count = read_count(&mut input, max_blocks, "checkpoint block count")?;
        let mut blocks = Vec::with_capacity(block_count);
        for _ in 0..block_count {
            let kind = IndexKind::try_from(read_u8(&mut input)?)?;
            if read_array::<3>(&mut input)? != [0; 3] {
                return Err(Error::Invalid(
                    "checkpoint block reserved bytes are nonzero",
                ));
            }
            blocks.push(IndexBlockRef {
                kind,
                frame_offset: read_u64(&mut input)?,
                frame_length: read_u64(&mut input)?,
                payload_hash: read_array(&mut input)?,
            });
        }
        ensure_end(&input, payload)?;
        Ok(Self {
            indexed_end,
            record_count,
            blocks,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CheckpointPointer {
    pub frame_offset: u64,
    pub frame_length: u64,
    pub payload_hash: [u8; 32],
}

impl CheckpointPointer {
    pub fn locator_payload(self) -> Vec<u8> {
        let mut out = Vec::with_capacity(LOCATOR_PAYLOAD_LEN);
        out.extend_from_slice(LOCATOR_MAGIC);
        out.extend_from_slice(&INDEX_SCHEMA_V1.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&self.frame_offset.to_le_bytes());
        out.extend_from_slice(&self.frame_length.to_le_bytes());
        out.extend_from_slice(&self.payload_hash);
        debug_assert_eq!(out.len(), LOCATOR_PAYLOAD_LEN);
        out
    }

    pub fn from_locator_payload(payload: &[u8]) -> Result<Self> {
        if payload.len() != LOCATOR_PAYLOAD_LEN {
            return Err(Error::Invalid("checkpoint locator has wrong size"));
        }
        let mut input = Cursor::new(payload);
        if read_array::<4>(&mut input)? != *LOCATOR_MAGIC {
            return Err(Error::Invalid("missing checkpoint locator magic"));
        }
        let schema = read_u16(&mut input)?;
        if schema != INDEX_SCHEMA_V1 {
            return Err(Error::Unsupported(format!(
                "checkpoint locator schema {schema}"
            )));
        }
        if read_u16(&mut input)? != 0 {
            return Err(Error::Invalid("checkpoint locator flags are nonzero"));
        }
        let pointer = Self {
            frame_offset: read_u64(&mut input)?,
            frame_length: read_u64(&mut input)?,
            payload_hash: read_array(&mut input)?,
        };
        ensure_end(&input, payload)?;
        Ok(pointer)
    }

    pub fn footer_trailer(self) -> [u8; FOOTER_TRAILER_LEN as usize] {
        let mut out = [0u8; FOOTER_TRAILER_LEN as usize];
        out[..8].copy_from_slice(FOOTER_MAGIC);
        out[8..10].copy_from_slice(&INDEX_SCHEMA_V1.to_le_bytes());
        out[10..12].copy_from_slice(&0u16.to_le_bytes());
        out[12..20].copy_from_slice(&self.frame_offset.to_le_bytes());
        out[20..28].copy_from_slice(&self.frame_length.to_le_bytes());
        out[28..60].copy_from_slice(&self.payload_hash);
        let mut crc = Crc32::new();
        crc.update(&out[..60]);
        out[60..64].copy_from_slice(&crc.finalize().to_le_bytes());
        out[64..].copy_from_slice(TRAILING_MAGIC);
        out
    }

    pub fn from_footer_trailer(trailer: &[u8]) -> Result<Self> {
        if trailer.len() != FOOTER_TRAILER_LEN as usize {
            return Err(Error::Invalid("footer trailer has wrong size"));
        }
        if &trailer[..8] != FOOTER_MAGIC || &trailer[64..] != TRAILING_MAGIC {
            return Err(Error::Invalid("missing footer trailer magic"));
        }
        let schema = u16::from_le_bytes(trailer[8..10].try_into().unwrap());
        if schema != INDEX_SCHEMA_V1 {
            return Err(Error::Unsupported(format!(
                "footer trailer schema {schema}"
            )));
        }
        if trailer[10..12] != [0; 2] {
            return Err(Error::Invalid("footer trailer flags are nonzero"));
        }
        let mut crc = Crc32::new();
        crc.update(&trailer[..60]);
        if crc.finalize() != u32::from_le_bytes(trailer[60..64].try_into().unwrap()) {
            return Err(Error::Invalid("footer trailer checksum mismatch"));
        }
        Ok(Self {
            frame_offset: u64::from_le_bytes(trailer[12..20].try_into().unwrap()),
            frame_length: u64::from_le_bytes(trailer[20..28].try_into().unwrap()),
            payload_hash: trailer[28..60].try_into().unwrap(),
        })
    }

    pub fn has_trailing_magic(trailer: &[u8]) -> bool {
        trailer.len() == FOOTER_TRAILER_LEN as usize && &trailer[64..] == TRAILING_MAGIC
    }
}

pub(crate) fn payload_hash(payload: &[u8]) -> [u8; 32] {
    *blake3::hash(payload).as_bytes()
}

fn put_count(out: &mut Vec<u8>, count: usize) -> Result<()> {
    let count = u32::try_from(count).map_err(|_| Error::Invalid("index count exceeds u32"))?;
    out.extend_from_slice(&count.to_le_bytes());
    Ok(())
}

fn put_bytes(out: &mut Vec<u8>, bytes: &[u8]) -> Result<()> {
    put_count(out, bytes.len())?;
    out.extend_from_slice(bytes);
    Ok(())
}

fn read_array<const N: usize>(input: &mut Cursor<&[u8]>) -> Result<[u8; N]> {
    let mut value = [0; N];
    input.read_exact(&mut value)?;
    Ok(value)
}

fn read_u8(input: &mut Cursor<&[u8]>) -> Result<u8> {
    Ok(read_array::<1>(input)?[0])
}

fn read_u16(input: &mut Cursor<&[u8]>) -> Result<u16> {
    Ok(u16::from_le_bytes(read_array(input)?))
}

fn read_u32(input: &mut Cursor<&[u8]>) -> Result<u32> {
    Ok(u32::from_le_bytes(read_array(input)?))
}

fn read_u64(input: &mut Cursor<&[u8]>) -> Result<u64> {
    Ok(u64::from_le_bytes(read_array(input)?))
}

fn read_count(input: &mut Cursor<&[u8]>, max: u32, what: &'static str) -> Result<usize> {
    let value = read_u32(input)?;
    if value > max {
        return Err(Error::Limit {
            what,
            actual: value as u64,
            limit: max as u64,
        });
    }
    Ok(value as usize)
}

fn read_bytes(input: &mut Cursor<&[u8]>, max: u32) -> Result<Vec<u8>> {
    let len = read_count(input, max, "index identifier length")?;
    let mut bytes = vec![0; len];
    input.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn ensure_end(input: &Cursor<&[u8]>, bytes: &[u8]) -> Result<()> {
    if input.position() != bytes.len() as u64 {
        return Err(Error::Invalid("trailing bytes in index payload"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locator_and_footer_are_fixed_and_checked() {
        let pointer = CheckpointPointer {
            frame_offset: 123,
            frame_length: 456,
            payload_hash: [7; 32],
        };
        let locator = pointer.locator_payload();
        assert_eq!(locator.len(), LOCATOR_PAYLOAD_LEN);
        assert_eq!(
            CheckpointPointer::from_locator_payload(&locator).unwrap(),
            pointer
        );
        let footer = pointer.footer_trailer();
        assert_eq!(footer.len(), FOOTER_TRAILER_LEN as usize);
        assert_eq!(
            CheckpointPointer::from_footer_trailer(&footer).unwrap(),
            pointer
        );
        let mut corrupt = footer;
        corrupt[20] ^= 1;
        assert!(CheckpointPointer::from_footer_trailer(&corrupt).is_err());
    }

    #[test]
    fn index_blocks_round_trip() {
        let blocks = [
            IndexBlock {
                kind: IndexKind::Chunk,
                entries: IndexEntries::Chunks(vec![ChunkIndexEntry {
                    hash: [1; 33],
                    frame_offset: 2,
                    uncompressed_length: 3,
                    compressed_length: 4,
                }]),
            },
            IndexBlock {
                kind: IndexKind::Trace,
                entries: IndexEntries::Traces(vec![(b"trace".to_vec(), [4; 32])]),
            },
            IndexBlock {
                kind: IndexKind::Session,
                entries: IndexEntries::Sessions(vec![(
                    b"session".to_vec(),
                    0,
                    vec![[5; 32], [6; 32]],
                )]),
            },
        ];
        for block in blocks {
            let encoded = block.encode().unwrap();
            assert_eq!(IndexBlock::decode(&encoded, 100, 100).unwrap(), block);
        }
    }

    #[test]
    fn large_index_blocks_are_independently_compressed_and_bounded() {
        let block = IndexBlock {
            kind: IndexKind::Manifest,
            entries: IndexEntries::IdOffsets(
                (0..2_000u64)
                    .map(|index| {
                        let mut id = [0; 32];
                        id[..8].copy_from_slice(&index.to_le_bytes());
                        (id, 1_000 + index * 64)
                    })
                    .collect(),
            ),
        };
        let encoded = block.encode().unwrap();
        assert!(encoded.starts_with(COMPRESSED_BLOCK_MAGIC));
        assert_eq!(IndexBlock::decode(&encoded, 3_000, 100).unwrap(), block);

        let mut bomb = encoded;
        bomb[4..12].copy_from_slice(&(MAX_INDEX_PAGE_UNCOMPRESSED + 1).to_le_bytes());
        assert!(matches!(
            IndexBlock::decode(&bomb, 3_000, 100),
            Err(Error::Limit {
                what: "index page uncompressed bytes",
                ..
            })
        ));
    }
}
