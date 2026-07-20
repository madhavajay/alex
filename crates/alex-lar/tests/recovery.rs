use alex_lar::{
    read_file_header, ArchiveReader, ArchiveWriter, ChunkerConfig, Error, Exchange, ExchangeData,
    FileHeader, FrameRead, FrameReader, HeaderAtom, HeaderBlock, HeaderFidelity, Limits,
    RecordType, RecoveryStatus, Stage, StageData, StageKind, StreamIndex, StreamRead,
};
use std::io::Cursor;

#[derive(Clone, Copy)]
struct FrameBoundary {
    start: usize,
    end: usize,
    kind: RecordType,
}

fn limits() -> Limits {
    Limits {
        max_frame_payload: 1024 * 1024,
        max_chunk_uncompressed: 128,
        max_body_length: 1024 * 1024,
        ..Limits::default()
    }
}

fn fixture() -> (
    Vec<u8>,
    alex_lar::ManifestId,
    alex_lar::HeaderBlockId,
    alex_lar::StreamIndexId,
    alex_lar::StageId,
    alex_lar::ExchangeId,
) {
    let config = ChunkerConfig {
        min_size: 32,
        target_size: 64,
        max_size: 128,
    };
    let header = FileHeader::standalone([0x52; 16], 777, b"recovery-test".to_vec());
    let mut writer =
        ArchiveWriter::create(Cursor::new(Vec::new()), header, config, limits()).unwrap();
    let body: Vec<u8> = (0..513).map(|index| ((index * 37) % 251) as u8).collect();
    let manifest_id = writer.append_body(&body).unwrap();
    let header_block = HeaderBlock::new(
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
    let header_block_id = writer.append_header_block(header_block).unwrap();
    let stream_index_id = writer
        .append_stream_index(StreamIndex::new(
            manifest_id,
            vec![StreamRead {
                byte_offset: 0,
                byte_length: body.len() as u64,
                delta_from_first_byte_ns: 0,
            }],
            Vec::new(),
        ))
        .unwrap();
    let mut stage_data = StageData::new(StageKind::ClientResponse, 778);
    stage_data.response_headers_ref = Some(header_block_id);
    stage_data.response_body_manifest_ref = Some(manifest_id);
    stage_data.stream_index_ref = Some(stream_index_id);
    let stage_id = writer.append_stage(Stage::new(stage_data)).unwrap();
    let exchange_id = writer
        .append_exchange(Exchange::new(ExchangeData::new(
            b"recovery-trace".to_vec(),
            1,
            778,
            vec![stage_id],
        )))
        .unwrap();
    let bytes = writer.into_inner().unwrap().into_inner();
    (
        bytes,
        manifest_id,
        header_block_id,
        stream_index_id,
        stage_id,
        exchange_id,
    )
}

fn boundaries(bytes: &[u8]) -> (usize, Vec<FrameBoundary>) {
    let limits = limits();
    let mut cursor = Cursor::new(bytes);
    let (_, header_end) = read_file_header(&mut cursor, &limits).unwrap();
    let mut result = Vec::new();
    loop {
        let start = cursor.position() as usize;
        let next = {
            let mut reader = FrameReader::new(&mut cursor, &limits);
            reader.read_next().unwrap()
        };
        match next {
            (FrameRead::Frame, Some(frame)) => result.push(FrameBoundary {
                start,
                end: cursor.position() as usize,
                kind: frame.record_type,
            }),
            (FrameRead::CleanEof, None) => break,
            other => panic!("complete fixture produced {other:?}"),
        }
    }
    (header_end as usize, result)
}

#[test]
fn every_byte_truncation_boundary_exposes_only_complete_records() {
    let (bytes, manifest_id, header_block_id, stream_index_id, stage_id, exchange_id) = fixture();
    let body: Vec<u8> = (0..513).map(|index| ((index * 37) % 251) as u8).collect();
    let (header_end, frames) = boundaries(&bytes);
    assert!(
        frames.len() >= 3,
        "fixture must contain chunks, manifest, headers, stream, stage, and exchange"
    );
    assert_eq!(frames.first().unwrap().start, header_end);
    assert_eq!(frames.last().unwrap().end, bytes.len());

    // A partial file header is never a recoverable archive because no format,
    // limits, role, or feature contract can be trusted yet.
    for cut in 0..header_end {
        assert!(
            ArchiveReader::open(Cursor::new(&bytes[..cut]), limits()).is_err(),
            "partial header unexpectedly opened at byte {cut}"
        );
    }

    for cut in header_end..=bytes.len() {
        let completed: Vec<_> = frames.iter().filter(|frame| frame.end <= cut).collect();
        let expected_chunks = completed
            .iter()
            .filter(|frame| frame.kind == RecordType::Chunk)
            .count();
        let expected_manifests = completed
            .iter()
            .filter(|frame| frame.kind == RecordType::BodyManifest)
            .count();
        let expected_headers = completed
            .iter()
            .filter(|frame| frame.kind == RecordType::HeaderBlock)
            .count();
        let expected_stream_indexes = completed
            .iter()
            .filter(|frame| frame.kind == RecordType::StreamIndex)
            .count();
        let expected_stages = completed
            .iter()
            .filter(|frame| frame.kind == RecordType::Stage)
            .count();
        let expected_exchanges = completed
            .iter()
            .filter(|frame| frame.kind == RecordType::Exchange)
            .count();

        let mut reader = ArchiveReader::open(Cursor::new(&bytes[..cut]), limits())
            .unwrap_or_else(|error| panic!("prefix ending at {cut} failed: {error}"));
        assert_eq!(reader.record_count(), completed.len(), "cut={cut}");
        assert_eq!(reader.chunk_count(), expected_chunks, "cut={cut}");
        assert_eq!(reader.manifest_count(), expected_manifests, "cut={cut}");
        assert_eq!(reader.header_block_count(), expected_headers, "cut={cut}");
        assert_eq!(
            reader.stream_index_count(),
            expected_stream_indexes,
            "cut={cut}"
        );
        assert_eq!(reader.stage_count(), expected_stages, "cut={cut}");
        assert_eq!(reader.exchange_count(), expected_exchanges, "cut={cut}");

        let at_boundary = cut == header_end || frames.iter().any(|frame| frame.end == cut);
        if at_boundary {
            assert_eq!(reader.recovery_status(), RecoveryStatus::Clean, "cut={cut}");
        } else {
            let last_valid = completed.last().map_or(header_end, |frame| frame.end);
            assert_eq!(
                reader.recovery_status(),
                RecoveryStatus::TruncatedTail {
                    last_valid_offset: last_valid as u64,
                    tail_bytes: (cut - last_valid) as u64,
                },
                "cut={cut}"
            );
        }

        if expected_manifests == 1 {
            assert_eq!(reader.read_body(&manifest_id).unwrap(), body, "cut={cut}");
        } else {
            assert!(
                matches!(reader.read_body(&manifest_id), Err(Error::Missing(_))),
                "incomplete manifest exposed at cut={cut}"
            );
        }
        assert_eq!(
            reader.header_block(&header_block_id).is_some(),
            expected_headers == 1,
            "cut={cut}"
        );
        assert_eq!(
            reader.stream_index(&stream_index_id).is_some(),
            expected_stream_indexes == 1,
            "cut={cut}"
        );
        assert_eq!(
            reader.stage(&stage_id).is_some(),
            expected_stages == 1,
            "cut={cut}"
        );
        assert_eq!(
            reader.exchange(&exchange_id).is_some(),
            expected_exchanges == 1,
            "cut={cut}"
        );
    }
}
