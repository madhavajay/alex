use crate::{Error, FrameRead, FrameReader, Limits, RecordFrame, RecordType, Result};
use std::collections::HashMap;
use std::io::{Cursor, Read};

const PAGE_MAGIC: &[u8; 4] = b"LMP1";
const DICTIONARY_MAGIC: &[u8; 4] = b"LDI1";
const PAGE_COMPRESSION_ZSTD: u8 = 1;
const PAGE_COMPRESSION_ZSTD_DICTIONARY: u8 = 2;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StoredDictionary {
    pub id: [u8; 32],
    pub bytes: Vec<u8>,
}

impl StoredDictionary {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self {
            id: *blake3::hash(&bytes).as_bytes(),
            bytes,
        }
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        let length = u32::try_from(self.bytes.len())
            .map_err(|_| Error::Invalid("compression dictionary exceeds u32"))?;
        let mut out = Vec::with_capacity(40 + self.bytes.len());
        out.extend_from_slice(DICTIONARY_MAGIC);
        out.extend_from_slice(&self.id);
        out.extend_from_slice(&length.to_le_bytes());
        out.extend_from_slice(&self.bytes);
        Ok(out)
    }

    pub fn decode(payload: &[u8], limits: &Limits) -> Result<Self> {
        if payload.len() < 40 || &payload[..4] != DICTIONARY_MAGIC {
            return Err(Error::Invalid("invalid dictionary record"));
        }
        let id = payload[4..36].try_into().unwrap();
        let length = u32::from_le_bytes(payload[36..40].try_into().unwrap());
        if length > limits.max_field_length {
            return Err(Error::Limit {
                what: "compression dictionary",
                actual: length as u64,
                limit: limits.max_field_length as u64,
            });
        }
        if payload.len() != 40 + length as usize {
            return Err(Error::Invalid("dictionary length mismatch"));
        }
        let value = Self {
            id,
            bytes: payload[40..].to_vec(),
        };
        if *blake3::hash(&value.bytes).as_bytes() != value.id {
            return Err(Error::Invalid("dictionary content ID mismatch"));
        }
        Ok(value)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct MetadataPage {
    pub inner_count: u32,
    pub uncompressed: Vec<u8>,
}

impl MetadataPage {
    pub fn encode(uncompressed: Vec<u8>, inner_count: u32, level: i32) -> Result<Vec<u8>> {
        Self::encode_with_dictionary(uncompressed, inner_count, level, None)
    }

    pub fn encode_with_dictionary(
        uncompressed: Vec<u8>,
        inner_count: u32,
        level: i32,
        dictionary: Option<&StoredDictionary>,
    ) -> Result<Vec<u8>> {
        let compressed = if let Some(dictionary) = dictionary {
            let mut encoder =
                zstd::stream::Encoder::with_dictionary(Vec::new(), level, &dictionary.bytes)?;
            std::io::Write::write_all(&mut encoder, &uncompressed)?;
            encoder.finish()?
        } else {
            zstd::stream::encode_all(Cursor::new(&uncompressed), level).map_err(Error::Io)?
        };
        let dictionary_bytes = if dictionary.is_some() { 32 } else { 0 };
        let mut out = Vec::with_capacity(28 + dictionary_bytes + compressed.len());
        out.extend_from_slice(PAGE_MAGIC);
        out.push(if dictionary.is_some() {
            PAGE_COMPRESSION_ZSTD_DICTIONARY
        } else {
            PAGE_COMPRESSION_ZSTD
        });
        out.extend_from_slice(&[0; 3]);
        out.extend_from_slice(&(uncompressed.len() as u64).to_le_bytes());
        out.extend_from_slice(&(compressed.len() as u64).to_le_bytes());
        out.extend_from_slice(&inner_count.to_le_bytes());
        if let Some(dictionary) = dictionary {
            out.extend_from_slice(&dictionary.id);
        }
        out.extend_from_slice(&compressed);
        Ok(out)
    }

    pub fn decode(payload: &[u8], limits: &Limits) -> Result<Self> {
        Self::decode_with_dictionaries(payload, limits, &HashMap::new())
    }

    pub fn decode_with_dictionaries(
        payload: &[u8],
        limits: &Limits,
        dictionaries: &HashMap<[u8; 32], Vec<u8>>,
    ) -> Result<Self> {
        if payload.len() < 28 || &payload[..4] != PAGE_MAGIC {
            return Err(Error::Invalid("invalid metadata page"));
        }
        let compression = payload[4];
        if !matches!(
            compression,
            PAGE_COMPRESSION_ZSTD | PAGE_COMPRESSION_ZSTD_DICTIONARY
        ) || payload[5..8] != [0; 3]
        {
            return Err(Error::Unsupported("metadata page compression".into()));
        }
        let uncompressed_length = u64::from_le_bytes(payload[8..16].try_into().unwrap());
        if uncompressed_length > limits.max_metadata_page_uncompressed {
            return Err(Error::Limit {
                what: "metadata page uncompressed bytes",
                actual: uncompressed_length,
                limit: limits.max_metadata_page_uncompressed,
            });
        }
        let compressed_length = u64::from_le_bytes(payload[16..24].try_into().unwrap());
        let data_offset = if compression == PAGE_COMPRESSION_ZSTD_DICTIONARY {
            60
        } else {
            28
        };
        if compressed_length > limits.max_frame_payload
            || payload.len() as u64 != data_offset + compressed_length
        {
            return Err(Error::Invalid("metadata page compressed length mismatch"));
        }
        let inner_count = u32::from_le_bytes(payload[24..28].try_into().unwrap());
        let mut dictionary = None;
        if compression == PAGE_COMPRESSION_ZSTD_DICTIONARY {
            let id: [u8; 32] = payload
                .get(28..60)
                .ok_or(Error::Invalid("truncated metadata page dictionary ID"))?
                .try_into()
                .unwrap();
            dictionary = Some(
                dictionaries
                    .get(&id)
                    .ok_or_else(|| Error::Missing(format!("dictionary {}", hex_id(&id))))?,
            );
        }
        let decoder: Box<dyn Read + '_> = if let Some(dictionary) = dictionary {
            Box::new(zstd::stream::read::Decoder::with_dictionary(
                Cursor::new(&payload[data_offset as usize..]),
                dictionary,
            )?)
        } else {
            Box::new(zstd::stream::read::Decoder::new(Cursor::new(
                &payload[data_offset as usize..],
            ))?)
        };
        let mut bounded = decoder.take(uncompressed_length.saturating_add(1));
        let mut uncompressed = Vec::with_capacity(uncompressed_length as usize);
        bounded.read_to_end(&mut uncompressed)?;
        if uncompressed.len() as u64 != uncompressed_length {
            return Err(Error::Invalid("metadata page uncompressed length mismatch"));
        }
        Ok(Self {
            inner_count,
            uncompressed,
        })
    }

    pub fn frames(&self, limits: &Limits) -> Result<Vec<RecordFrame>> {
        let mut cursor = Cursor::new(self.uncompressed.as_slice());
        let mut frames = Vec::with_capacity(self.inner_count as usize);
        loop {
            let next = {
                let mut reader = FrameReader::new(&mut cursor, limits);
                reader.read_next()?
            };
            match next {
                (FrameRead::CleanEof, None) => break,
                (FrameRead::Frame, Some(frame)) => {
                    if !matches!(
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
                        return Err(Error::Invalid(
                            "metadata page contains a non-metadata record",
                        ));
                    }
                    frames.push(frame);
                }
                (FrameRead::Truncated, _) => {
                    return Err(Error::Invalid("truncated record inside metadata page"))
                }
                _ => return Err(Error::Invalid("invalid metadata page frame state")),
            }
        }
        if frames.len() != self.inner_count as usize {
            return Err(Error::Invalid("metadata page record count mismatch"));
        }
        Ok(frames)
    }
}

fn hex_id(id: &[u8; 32]) -> String {
    let mut value = String::with_capacity(64);
    for byte in id {
        use std::fmt::Write as _;
        let _ = write!(value, "{byte:02x}");
    }
    value
}

pub(crate) fn push_inner_frame(
    output: &mut Vec<u8>,
    record_type: RecordType,
    payload: Vec<u8>,
) -> Result<()> {
    RecordFrame {
        record_type,
        schema_version: 1,
        flags: RecordFrame::REQUIRED,
        payload,
        offset: output.len() as u64,
    }
    .write(output)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dictionary_and_page_round_trip() {
        let dictionary = StoredDictionary::new(b"json http dictionary".to_vec());
        assert_eq!(
            StoredDictionary::decode(&dictionary.encode().unwrap(), &Limits::default()).unwrap(),
            dictionary
        );
        let mut inner = Vec::new();
        push_inner_frame(&mut inner, RecordType::BodyManifest, vec![1, 2, 3]).unwrap();
        let encoded = MetadataPage::encode(inner.clone(), 1, 3).unwrap();
        let page = MetadataPage::decode(&encoded, &Limits::default()).unwrap();
        assert_eq!(page.uncompressed, inner);
        let frames = page.frames(&Limits::default()).unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].payload, vec![1, 2, 3]);

        let encoded =
            MetadataPage::encode_with_dictionary(inner.clone(), 1, 3, Some(&dictionary)).unwrap();
        let dictionaries = HashMap::from([(dictionary.id, dictionary.bytes.clone())]);
        let page =
            MetadataPage::decode_with_dictionaries(&encoded, &Limits::default(), &dictionaries)
                .unwrap();
        assert_eq!(page.uncompressed, inner);
    }

    #[test]
    fn metadata_page_rejects_unknown_inner_record_types() {
        let mut inner = Vec::new();
        RecordFrame {
            record_type: RecordType::Unknown(4_000),
            schema_version: 1,
            flags: 0,
            payload: vec![0; 32],
            offset: 0,
        }
        .write(&mut inner)
        .unwrap();
        let encoded = MetadataPage::encode(inner, 1, 3).unwrap();
        let page = MetadataPage::decode(&encoded, &Limits::default()).unwrap();
        assert!(matches!(
            page.frames(&Limits::default()),
            Err(Error::Invalid(
                "metadata page contains a non-metadata record"
            ))
        ));
    }
}
