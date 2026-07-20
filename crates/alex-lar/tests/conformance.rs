use alex_lar::{
    read_file_header, write_file_header, ArchiveReader, Error, FileHeader, FrameReader, Limits,
    RecordFrame, RecordType, RecoveryStatus,
};
use crc32fast::Hasher;
use std::io::Cursor;

fn crc32(parts: &[&[u8]]) -> u32 {
    let mut hasher = Hasher::new();
    for part in parts {
        hasher.update(part);
    }
    hasher.finalize()
}

fn decode_hex(input: &str) -> Vec<u8> {
    let compact: String = input.chars().filter(|c| !c.is_whitespace()).collect();
    assert_eq!(compact.len() % 2, 0);
    (0..compact.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&compact[at..at + 2], 16).unwrap())
        .collect()
}

fn header() -> FileHeader {
    FileHeader::standalone([0x41; 16], 1_234_567, b"conformance".to_vec())
}

fn short_header() -> FileHeader {
    FileHeader::standalone([0x41; 16], 1_234_567, b"x".to_vec())
}

fn encoded_header(value: &FileHeader) -> Vec<u8> {
    let mut bytes = Vec::new();
    write_file_header(&mut bytes, value).unwrap();
    bytes
}

fn push_frame(
    bytes: &mut Vec<u8>,
    record_type: RecordType,
    schema_version: u16,
    flags: u32,
    payload: Vec<u8>,
) {
    RecordFrame {
        record_type,
        schema_version,
        flags,
        payload,
        offset: bytes.len() as u64,
    }
    .write(bytes)
    .unwrap();
}

/// Adds bytes to the bounded v1 header payload and updates its length and CRC.
/// This models a newer minor writer without depending on its implementation.
fn add_minor_header_extension(mut bytes: Vec<u8>, extension: &[u8]) -> Vec<u8> {
    let payload_len = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
    assert_eq!(bytes.len(), 12 + payload_len + 4);
    bytes.truncate(12 + payload_len);
    bytes.extend_from_slice(extension);
    let extended_len = (payload_len + extension.len()) as u32;
    bytes[8..12].copy_from_slice(&extended_len.to_le_bytes());
    let checksum = crc32(&[&bytes]);
    bytes.extend_from_slice(&checksum.to_le_bytes());
    bytes
}

#[test]
fn v1_envelope_encoding_matches_frozen_golden_bytes() {
    let mut value = FileHeader::standalone(
        [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
        0x0102_0304_0506_0708,
        b"golden".to_vec(),
    );
    value.optional_feature_bits = 1 << 63;

    let mut actual = encoded_header(&value);
    push_frame(
        &mut actual,
        RecordType::Unknown(0x1234),
        7,
        0,
        vec![0xde, 0xad, 0xbe, 0xef],
    );

    let expected = decode_hex(include_str!("../testdata/v1-envelope.hex"));
    assert_eq!(actual, expected, "v1 envelope bytes changed");

    let reader = ArchiveReader::open(Cursor::new(expected), Limits::default()).unwrap();
    assert_eq!(reader.recovery_status(), RecoveryStatus::Clean);
    assert_eq!(reader.record_count(), 0, "newer optional schema is skipped");
}

#[test]
fn identical_public_writes_are_deterministic() {
    let make = || {
        let mut bytes = encoded_header(&header());
        push_frame(
            &mut bytes,
            RecordType::Unknown(777),
            1,
            0,
            b"same input".to_vec(),
        );
        bytes
    };
    assert_eq!(make(), make());
}

#[test]
fn newer_minor_header_and_optional_records_are_skipped() {
    let mut value = header();
    value.container_minor = 23;
    value.optional_feature_bits = 0x8000_0000_0000_0042;
    let mut bytes = add_minor_header_extension(encoded_header(&value), b"future-minor-fields");

    // An unknown type using schema v1 is scanned but has no exposed index.
    push_frame(
        &mut bytes,
        RecordType::Unknown(900),
        1,
        0,
        b"optional type".to_vec(),
    );
    // A known type with a future optional schema must be skipped without
    // attempting to parse its deliberately invalid payload.
    push_frame(
        &mut bytes,
        RecordType::HeaderBlock,
        99,
        0,
        b"future schema".to_vec(),
    );

    let reader = ArchiveReader::open(Cursor::new(bytes), Limits::default()).unwrap();
    assert_eq!(reader.header().container_minor, 23);
    assert_eq!(reader.header().optional_feature_bits, 0x8000_0000_0000_0042);
    assert_eq!(reader.record_count(), 1);
    assert_eq!(reader.header_block_count(), 0);
    assert_eq!(reader.recovery_status(), RecoveryStatus::Clean);
}

#[test]
fn unknown_required_type_and_schema_are_rejected() {
    let mut unknown_type = encoded_header(&header());
    push_frame(
        &mut unknown_type,
        RecordType::Unknown(901),
        1,
        RecordFrame::REQUIRED,
        Vec::new(),
    );
    assert!(matches!(
        ArchiveReader::open(Cursor::new(unknown_type), Limits::default()),
        Err(Error::Unsupported(message)) if message.contains("required record type 901")
    ));

    let mut unknown_schema = encoded_header(&header());
    push_frame(
        &mut unknown_schema,
        RecordType::HeaderBlock,
        2,
        RecordFrame::REQUIRED,
        Vec::new(),
    );
    assert!(matches!(
        ArchiveReader::open(Cursor::new(unknown_schema), Limits::default()),
        Err(Error::Unsupported(message)) if message.contains("required record schema 2")
    ));
}

#[test]
fn unknown_required_header_features_are_rejected() {
    let mut value = header();
    value.required_feature_bits = 4;
    assert!(matches!(
        ArchiveReader::open(Cursor::new(encoded_header(&value)), Limits::default()),
        Err(Error::Unsupported(message)) if message.contains("required feature bits 0x4")
    ));
}

#[test]
fn declared_header_and_frame_lengths_are_bounded_before_allocation() {
    let header_limits = Limits {
        max_header_length: 32,
        ..Limits::default()
    };

    let mut oversized_header = b"LAR1".to_vec();
    oversized_header.extend_from_slice(&1u16.to_le_bytes());
    oversized_header.extend_from_slice(&0u16.to_le_bytes());
    oversized_header.extend_from_slice(&33u32.to_le_bytes());
    assert!(matches!(
        read_file_header(&mut Cursor::new(oversized_header), &header_limits),
        Err(Error::Limit {
            what: "file header",
            actual: 33,
            limit: 32
        })
    ));

    let frame_limits = Limits {
        max_frame_payload: 8,
        ..Limits::default()
    };
    let mut oversized_frame = encoded_header(&header());
    oversized_frame.extend_from_slice(b"LREC");
    oversized_frame.extend_from_slice(&900u16.to_le_bytes());
    oversized_frame.extend_from_slice(&1u16.to_le_bytes());
    oversized_frame.extend_from_slice(&0u32.to_le_bytes());
    oversized_frame.extend_from_slice(&9u64.to_le_bytes());
    assert!(matches!(
        ArchiveReader::open(Cursor::new(oversized_frame), frame_limits),
        Err(Error::Limit {
            what: "record payload",
            actual: 9,
            limit: 8
        })
    ));
}

#[test]
fn nested_record_lengths_are_bounded_before_payload_allocation() {
    let limits = Limits {
        max_chunk_uncompressed: 8,
        max_body_length: 8,
        max_field_length: 4,
        ..Limits::default()
    };

    let mut chunk_archive = encoded_header(&short_header());
    let mut chunk = vec![1]; // BLAKE3 algorithm
    chunk.extend_from_slice(&[0; 32]);
    chunk.extend_from_slice(&9u64.to_le_bytes());
    push_frame(
        &mut chunk_archive,
        RecordType::Chunk,
        1,
        RecordFrame::REQUIRED,
        chunk,
    );
    assert!(matches!(
        ArchiveReader::open(Cursor::new(chunk_archive), limits.clone()),
        Err(Error::Limit {
            what: "uncompressed chunk",
            actual: 9,
            limit: 8
        })
    ));

    let mut manifest_archive = encoded_header(&short_header());
    let mut manifest = vec![0; 32]; // content ID; length is checked first
    manifest.extend_from_slice(&9u64.to_le_bytes());
    push_frame(
        &mut manifest_archive,
        RecordType::BodyManifest,
        1,
        RecordFrame::REQUIRED,
        manifest,
    );
    assert!(matches!(
        ArchiveReader::open(Cursor::new(manifest_archive), limits.clone()),
        Err(Error::Limit {
            what: "body length",
            actual: 9,
            limit: 8
        })
    ));

    let mut headers_archive = encoded_header(&short_header());
    let mut header_block = vec![0; 32]; // content ID
    header_block.push(0); // exact fidelity
    header_block.extend_from_slice(&1u32.to_le_bytes()); // one atom
    header_block.extend_from_slice(&5u32.to_le_bytes()); // name length
    push_frame(
        &mut headers_archive,
        RecordType::HeaderBlock,
        1,
        RecordFrame::REQUIRED,
        header_block,
    );
    assert!(matches!(
        ArchiveReader::open(Cursor::new(headers_archive), limits),
        Err(Error::Limit {
            what: "field length",
            actual: 5,
            limit: 4
        })
    ));
}

#[test]
fn checksum_corruption_is_not_reported_as_recoverable_truncation() {
    let mut valid = encoded_header(&header());
    let frame_offset = valid.len();
    push_frame(
        &mut valid,
        RecordType::Unknown(777),
        1,
        0,
        b"payload".to_vec(),
    );

    let mut corrupt = valid.clone();
    corrupt[frame_offset + 20] ^= 0x80;
    assert!(matches!(
        ArchiveReader::open(Cursor::new(corrupt), Limits::default()),
        Err(Error::Checksum { offset }) if offset == frame_offset as u64
    ));

    valid.pop();
    let reader = ArchiveReader::open(Cursor::new(valid.clone()), Limits::default()).unwrap();
    assert_eq!(
        reader.recovery_status(),
        RecoveryStatus::TruncatedTail {
            last_valid_offset: frame_offset as u64,
            tail_bytes: (valid.len() - frame_offset) as u64,
        }
    );
    assert_eq!(reader.record_count(), 0);
}

#[test]
fn frame_reader_reports_clean_eof_only_at_a_record_boundary() {
    let mut bytes = Vec::new();
    push_frame(&mut bytes, RecordType::Unknown(42), 1, 0, Vec::new());
    let limits = Limits::default();
    let mut cursor = Cursor::new(bytes);
    let mut reader = FrameReader::new(&mut cursor, &limits);
    assert!(matches!(
        reader.read_next(),
        Ok((alex_lar::FrameRead::Frame, Some(_)))
    ));
    assert!(matches!(
        reader.read_next(),
        Ok((alex_lar::FrameRead::CleanEof, None))
    ));
}
