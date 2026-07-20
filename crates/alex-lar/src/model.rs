use crate::{Error, HashAlgorithm, Limits, Result};
use std::io::{Cursor, Read};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ChunkHash {
    pub algorithm: HashAlgorithm,
    pub digest: [u8; 32],
}

impl ChunkHash {
    pub fn blake3(bytes: &[u8]) -> Self {
        Self {
            algorithm: HashAlgorithm::Blake3,
            digest: *blake3::hash(bytes).as_bytes(),
        }
    }
}

macro_rules! logical_id {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(pub [u8; 32]);

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                for byte in self.0 {
                    write!(f, "{byte:02x}")?;
                }
                Ok(())
            }
        }

        impl std::str::FromStr for $name {
            type Err = Error;

            fn from_str(value: &str) -> Result<Self> {
                if value.len() != 64 {
                    return Err(Error::InvalidDetail(format!(
                        "{} must contain exactly 64 hexadecimal characters",
                        stringify!($name)
                    )));
                }
                let mut bytes = [0u8; 32];
                for (index, byte) in bytes.iter_mut().enumerate() {
                    let start = index * 2;
                    *byte = u8::from_str_radix(&value[start..start + 2], 16).map_err(|_| {
                        Error::InvalidDetail(format!(
                            "{} contains non-hexadecimal characters",
                            stringify!($name)
                        ))
                    })?;
                }
                Ok(Self(bytes))
            }
        }
    };
}

logical_id!(ManifestId);
logical_id!(HeaderBlockId);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChunkRef {
    pub chunk_hash: ChunkHash,
    pub chunk_offset: u64,
    pub logical_offset: u64,
    pub length: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BodyManifest {
    pub id: ManifestId,
    pub total_length: u64,
    pub whole_body_hash: ChunkHash,
    pub media_type: Option<Vec<u8>>,
    pub content_encoding: Option<Vec<u8>>,
    pub chunks: Vec<ChunkRef>,
}

impl BodyManifest {
    /// Build a content-addressed manifest from already durable chunk ranges.
    ///
    /// Body-pack coordinators use this constructor when the referenced chunks
    /// live in more than one physical pack. The manifest remains independent
    /// of file locations; a catalog is responsible for resolving each hash.
    pub fn new(
        total_length: u64,
        whole_body_hash: ChunkHash,
        media_type: Option<Vec<u8>>,
        content_encoding: Option<Vec<u8>>,
        chunks: Vec<ChunkRef>,
    ) -> Self {
        let mut value = Self {
            id: ManifestId([0; 32]),
            total_length,
            whole_body_hash,
            media_type,
            content_encoding,
            chunks,
        };
        value.id = ManifestId(*blake3::hash(&value.canonical_bytes()).as_bytes());
        value
    }

    pub(crate) fn canonical_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put_u64(&mut out, self.total_length);
        put_hash(&mut out, &self.whole_body_hash);
        put_opt_bytes(&mut out, self.media_type.as_deref());
        put_opt_bytes(&mut out, self.content_encoding.as_deref());
        put_u32(&mut out, self.chunks.len() as u32);
        for item in &self.chunks {
            put_hash(&mut out, &item.chunk_hash);
            put_u64(&mut out, item.chunk_offset);
            put_u64(&mut out, item.logical_offset);
            put_u64(&mut out, item.length);
        }
        out
    }

    pub(crate) fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(32 + self.canonical_bytes().len());
        out.extend_from_slice(&self.id.0);
        out.extend_from_slice(&self.canonical_bytes());
        out
    }

    pub(crate) fn decode(bytes: &[u8], limits: &Limits) -> Result<Self> {
        let mut input = Cursor::new(bytes);
        let id = ManifestId(read_array(&mut input)?);
        let total_length = read_u64(&mut input)?;
        if total_length > limits.max_body_length {
            return Err(Error::Limit {
                what: "body length",
                actual: total_length,
                limit: limits.max_body_length,
            });
        }
        let whole_body_hash = read_hash(&mut input)?;
        let media_type = read_opt_bytes(&mut input, limits.max_field_length)?;
        let content_encoding = read_opt_bytes(&mut input, limits.max_field_length)?;
        let count = read_u32(&mut input)? as u64;
        if count > limits.max_manifest_chunks as u64 {
            return Err(Error::Limit {
                what: "manifest chunk count",
                actual: count,
                limit: limits.max_manifest_chunks as u64,
            });
        }
        let mut chunks = Vec::with_capacity(count as usize);
        for _ in 0..count {
            chunks.push(ChunkRef {
                chunk_hash: read_hash(&mut input)?,
                chunk_offset: read_u64(&mut input)?,
                logical_offset: read_u64(&mut input)?,
                length: read_u64(&mut input)?,
            });
        }
        ensure_end(&input, bytes)?;
        let value = Self {
            id,
            total_length,
            whole_body_hash,
            media_type,
            content_encoding,
            chunks,
        };
        if ManifestId(*blake3::hash(&value.canonical_bytes()).as_bytes()) != value.id {
            return Err(Error::Invalid(
                "manifest content ID does not match contents",
            ));
        }
        value.validate()?;
        Ok(value)
    }

    pub fn validate(&self) -> Result<()> {
        let mut expected = 0u64;
        for item in &self.chunks {
            if item.length == 0 {
                return Err(Error::Invalid("manifest ranges must not be empty"));
            }
            if item.logical_offset != expected {
                return Err(Error::Invalid(
                    "manifest ranges must be contiguous and ordered",
                ));
            }
            expected = expected
                .checked_add(item.length)
                .ok_or(Error::Invalid("manifest length overflow"))?;
        }
        if expected != self.total_length {
            return Err(Error::Invalid("manifest ranges do not cover total length"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HeaderAtom {
    pub original_name: Vec<u8>,
    pub value: Vec<u8>,
    pub flags: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum HeaderFidelity {
    Exact = 0,
    LegacyOrderUnknown = 1,
    LegacyCasingUnknown = 2,
    LegacyOrderAndCasingUnknown = 3,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HeaderBlock {
    pub id: HeaderBlockId,
    pub fidelity: HeaderFidelity,
    pub atoms: Vec<HeaderAtom>,
}

impl HeaderBlock {
    pub fn new(fidelity: HeaderFidelity, atoms: Vec<HeaderAtom>) -> Self {
        let mut value = Self {
            id: HeaderBlockId([0; 32]),
            fidelity,
            atoms,
        };
        value.id = HeaderBlockId(*blake3::hash(&value.canonical_bytes()).as_bytes());
        value
    }

    fn canonical_bytes(&self) -> Vec<u8> {
        let mut out = vec![self.fidelity as u8];
        put_u32(&mut out, self.atoms.len() as u32);
        for atom in &self.atoms {
            put_bytes(&mut out, &atom.original_name);
            put_bytes(&mut out, &atom.value);
            put_u32(&mut out, atom.flags);
        }
        out
    }

    pub(crate) fn encode(&self) -> Vec<u8> {
        let mut out = self.id.0.to_vec();
        out.extend_from_slice(&self.canonical_bytes());
        out
    }

    pub(crate) fn decode(bytes: &[u8], limits: &Limits) -> Result<Self> {
        let mut input = Cursor::new(bytes);
        let id = HeaderBlockId(read_array(&mut input)?);
        let fidelity = match read_u8(&mut input)? {
            0 => HeaderFidelity::Exact,
            1 => HeaderFidelity::LegacyOrderUnknown,
            2 => HeaderFidelity::LegacyCasingUnknown,
            3 => HeaderFidelity::LegacyOrderAndCasingUnknown,
            _ => return Err(Error::Invalid("unknown header fidelity")),
        };
        let count = read_u32(&mut input)? as u64;
        if count > limits.max_header_atoms as u64 {
            return Err(Error::Limit {
                what: "header atom count",
                actual: count,
                limit: limits.max_header_atoms as u64,
            });
        }
        let mut atoms = Vec::with_capacity(count as usize);
        for _ in 0..count {
            atoms.push(HeaderAtom {
                original_name: read_bytes(&mut input, limits.max_field_length)?,
                value: read_bytes(&mut input, limits.max_field_length)?,
                flags: read_u32(&mut input)?,
            });
        }
        ensure_end(&input, bytes)?;
        let value = Self {
            id,
            fidelity,
            atoms,
        };
        if HeaderBlockId(*blake3::hash(&value.canonical_bytes()).as_bytes()) != value.id {
            return Err(Error::Invalid("header block ID does not match contents"));
        }
        Ok(value)
    }
}

pub(crate) fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}
pub(crate) fn put_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}
pub(crate) fn put_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    put_u32(out, bytes.len() as u32);
    out.extend_from_slice(bytes);
}
fn put_opt_bytes(out: &mut Vec<u8>, value: Option<&[u8]>) {
    match value {
        Some(v) => {
            out.push(1);
            put_bytes(out, v);
        }
        None => out.push(0),
    }
}
pub(crate) fn put_hash(out: &mut Vec<u8>, hash: &ChunkHash) {
    out.push(hash.algorithm as u8);
    out.extend_from_slice(&hash.digest);
}

pub(crate) fn read_u8(input: &mut Cursor<&[u8]>) -> Result<u8> {
    Ok(read_array::<1>(input)?[0])
}
pub(crate) fn read_u32(input: &mut Cursor<&[u8]>) -> Result<u32> {
    Ok(u32::from_le_bytes(read_array(input)?))
}
pub(crate) fn read_u64(input: &mut Cursor<&[u8]>) -> Result<u64> {
    Ok(u64::from_le_bytes(read_array(input)?))
}
pub(crate) fn read_array<const N: usize>(input: &mut Cursor<&[u8]>) -> Result<[u8; N]> {
    let mut out = [0; N];
    input
        .read_exact(&mut out)
        .map_err(|_| Error::Invalid("truncated record payload"))?;
    Ok(out)
}
pub(crate) fn read_bytes(input: &mut Cursor<&[u8]>, max: u32) -> Result<Vec<u8>> {
    let len = read_u32(input)?;
    if len > max {
        return Err(Error::Limit {
            what: "field length",
            actual: len as u64,
            limit: max as u64,
        });
    }
    let mut out = vec![0; len as usize];
    input
        .read_exact(&mut out)
        .map_err(|_| Error::Invalid("truncated record payload"))?;
    Ok(out)
}
fn read_opt_bytes(input: &mut Cursor<&[u8]>, max: u32) -> Result<Option<Vec<u8>>> {
    match read_u8(input)? {
        0 => Ok(None),
        1 => Ok(Some(read_bytes(input, max)?)),
        _ => Err(Error::Invalid("invalid optional field tag")),
    }
}
pub(crate) fn read_hash(input: &mut Cursor<&[u8]>) -> Result<ChunkHash> {
    let algorithm = HashAlgorithm::try_from(read_u8(input)?)?;
    Ok(ChunkHash {
        algorithm,
        digest: read_array(input)?,
    })
}
pub(crate) fn ensure_end(input: &Cursor<&[u8]>, bytes: &[u8]) -> Result<()> {
    if input.position() != bytes.len() as u64 {
        return Err(Error::Invalid("trailing record payload bytes"));
    }
    Ok(())
}
