use alex_lar::{
    read_file_header, upgrade_archive, verify_upgraded_archive, ArchiveReader, ArchiveWriter,
    ChunkerConfig, Error, Exchange, ExchangeData, ExchangeMetadataData, FileHeader, FrameRead,
    FrameReader, Limits, OpenPath, RecordFrame, RecordType, UnknownExchangeMetadataAttribute,
};
use std::io::{Cursor, Seek};

fn limits() -> Limits {
    Limits {
        max_frame_payload: 1024 * 1024,
        max_chunk_uncompressed: 128,
        max_body_length: 1024 * 1024,
        ..Limits::default()
    }
}

fn config() -> ChunkerConfig {
    ChunkerConfig {
        min_size: 32,
        target_size: 64,
        max_size: 128,
    }
}

fn writer() -> ArchiveWriter<Cursor<Vec<u8>>> {
    ArchiveWriter::create(
        Cursor::new(Vec::new()),
        FileHeader::standalone([0x6d; 16], 1, b"exchange-metadata-tests".to_vec()),
        config(),
        limits(),
    )
    .unwrap()
}

fn complete_data() -> ExchangeMetadataData {
    ExchangeMetadataData {
        ts_request_ms: Some(-123),
        ts_response_ms: Some(456),
        harness: Some(b"pi".to_vec()),
        client_format: Some(b"openai-chat".to_vec()),
        upstream_format: Some(b"anthropic-messages".to_vec()),
        method: Some(b"POST".to_vec()),
        path: Some(b"/v1/chat/completions?raw=1".to_vec()),
        streamed: Some(false),
        status: Some(70_000),
        cost_usd_bits: Some((-0.0f64).to_bits()),
        billing_bucket: Some(b"subscription".to_vec()),
        error_kind: Some(b"quota".to_vec()),
        error_code: Some(b"access_terminated_error".to_vec()),
        substituted: true,
        original_model: Some(b"model-a".to_vec()),
        served_model: Some(b"model-b".to_vec()),
        substitution_reason: Some(b"quota".to_vec()),
        injected: true,
        fixture_name: Some(b"onboarding".to_vec()),
        attempts_json: Some(br#"[{"account":"one"}]"#.to_vec()),
        original_account_id: Some(b"account-one".to_vec()),
        served_account_id: Some(b"account-two".to_vec()),
        subscription_identity: Some(b"sub-identity".to_vec()),
        via_dario: true,
        dario_generation: Some(b"generation-7".to_vec()),
        tags_json: Some(br#"{"run":"bench"}"#.to_vec()),
        client_ip: Some(b"127.0.0.1".to_vec()),
        key_fingerprint: Some(b"sha256:abc".to_vec()),
        reasoning_effort: Some(b"high".to_vec()),
        thinking_budget: Some(-1),
        input_tokens: Some(-2),
        cached_input_tokens: Some(0),
        cache_creation_tokens: Some(17),
        output_tokens: Some(18),
        reasoning_tokens: Some(19),
        unknown_attributes: vec![UnknownExchangeMetadataAttribute {
            key: b"x.future.attribute".to_vec(),
            value: b"preserve me".to_vec(),
        }],
    }
}

#[derive(Clone)]
struct PhysicalFrame {
    start: usize,
    end: usize,
    frame: RecordFrame,
}

fn frames(bytes: &[u8]) -> (usize, Vec<PhysicalFrame>) {
    let mut cursor = Cursor::new(bytes);
    let test_limits = limits();
    let (_, data_offset) = read_file_header(&mut cursor, &test_limits).unwrap();
    let mut result = Vec::new();
    loop {
        let start = cursor.position() as usize;
        let next = {
            let mut reader = FrameReader::new(&mut cursor, &test_limits);
            reader.read_next()
        };
        match next {
            Ok((FrameRead::Frame, Some(frame))) => result.push(PhysicalFrame {
                start,
                end: cursor.position() as usize,
                frame,
            }),
            Ok((FrameRead::CleanEof, _)) | Ok((FrameRead::Truncated, _)) | Err(_) => break,
            _ => unreachable!(),
        }
    }
    (data_offset as usize, result)
}

#[test]
fn companion_round_trips_through_forward_checkpoint_and_footer_paths() {
    let mut writer = writer();
    writer.enable_metadata_pages();
    writer
        .append_exchange(Exchange::new(ExchangeData::new(
            b"paged-without-metadata".to_vec(),
            1,
            1,
            vec![],
        )))
        .unwrap();
    let exchange = Exchange::new(ExchangeData::new(
        b"trace-with-metadata".to_vec(),
        2,
        2,
        vec![],
    ));
    let expected = complete_data();
    let id = writer
        .append_exchange_with_metadata(exchange.clone(), expected.clone())
        .unwrap();
    assert_eq!(writer.exchange_metadata(&id).unwrap().data, expected);

    // The first exchange is flushed as a page; the companion exchange is a
    // direct required frame followed immediately by the optional extension.
    let active = writer.get_mut().get_ref().clone();
    let (_, active_frames) = frames(&active);
    let kinds: Vec<_> = active_frames
        .iter()
        .map(|value| value.frame.record_type)
        .collect();
    assert_eq!(
        kinds,
        vec![
            RecordType::MetadataPage,
            RecordType::Exchange,
            RecordType::ExchangeMetadata
        ]
    );
    assert_eq!(active_frames[2].frame.flags, 0);

    let forward = ArchiveReader::open(Cursor::new(active), limits()).unwrap();
    assert_eq!(forward.open_path(), OpenPath::ForwardScan);
    assert_eq!(forward.exchange_metadata(&id).unwrap().data, expected);
    assert_eq!(
        forward
            .exchange_metadata_by_trace(b"trace-with-metadata")
            .unwrap()
            .data,
        expected
    );

    writer.checkpoint().unwrap();
    let checkpoint_bytes = writer.get_mut().get_ref().clone();
    let checkpoint = ArchiveReader::open(Cursor::new(checkpoint_bytes), limits()).unwrap();
    assert_eq!(checkpoint.open_path(), OpenPath::Checkpoint);
    assert_eq!(checkpoint.exchange_metadata(&id).unwrap().data, expected);

    writer.seal().unwrap();
    let sealed = writer.into_inner().unwrap().into_inner();
    let footer = ArchiveReader::open(Cursor::new(sealed), limits()).unwrap();
    assert_eq!(footer.open_path(), OpenPath::Footer);
    assert_eq!(footer.exchange(&id), Some(&exchange));
    assert_eq!(footer.exchange_metadata(&id).unwrap().data, expected);
}

#[test]
fn old_v1_dispatcher_can_skip_the_standalone_optional_record() {
    let mut writer = writer();
    let exchange = Exchange::new(ExchangeData::new(b"legacy-skip".to_vec(), 1, 1, vec![]));
    writer
        .append_exchange_with_metadata(exchange, ExchangeMetadataData::default())
        .unwrap();
    let bytes = writer.into_inner().unwrap().into_inner();
    let (_, frames) = frames(&bytes);
    let mut legacy_known = 0;
    let mut legacy_skipped = 0;
    for physical in frames {
        match physical.frame.record_type.code() {
            1..=14 => legacy_known += 1,
            _ => {
                assert_eq!(physical.frame.flags & RecordFrame::REQUIRED, 0);
                legacy_skipped += 1;
            }
        }
    }
    assert_eq!(legacy_known, 1);
    assert_eq!(legacy_skipped, 1);
}

#[test]
fn forward_scan_rejects_orphan_mismatched_and_duplicate_companions() {
    fn capture(trace: &[u8]) -> (Vec<u8>, usize, PhysicalFrame, PhysicalFrame) {
        let mut writer = writer();
        let exchange = Exchange::new(ExchangeData::new(trace.to_vec(), 1, 1, vec![]));
        writer
            .append_exchange_with_metadata(exchange, ExchangeMetadataData::default())
            .unwrap();
        let bytes = writer.into_inner().unwrap().into_inner();
        let (header_end, frames) = frames(&bytes);
        (bytes, header_end, frames[0].clone(), frames[1].clone())
    }

    let (first, header_end, exchange, metadata) = capture(b"first");
    let (second, _, _, other_metadata) = capture(b"second");

    let mut orphan = first[..header_end].to_vec();
    orphan.extend_from_slice(&first[metadata.start..metadata.end]);
    assert!(matches!(
        ArchiveReader::open(Cursor::new(orphan), limits()),
        Err(Error::Invalid("orphan exchange metadata companion"))
    ));

    let mut mismatched = first[..header_end].to_vec();
    mismatched.extend_from_slice(&first[exchange.start..exchange.end]);
    mismatched.extend_from_slice(&second[other_metadata.start..other_metadata.end]);
    assert!(matches!(
        ArchiveReader::open(Cursor::new(mismatched), limits()),
        Err(Error::Invalid(
            "exchange metadata companion identity mismatch"
        ))
    ));

    let mut duplicate = first[..header_end].to_vec();
    duplicate.extend_from_slice(&first[exchange.start..exchange.end]);
    duplicate.extend_from_slice(&first[metadata.start..metadata.end]);
    duplicate.extend_from_slice(&first[metadata.start..metadata.end]);
    assert!(ArchiveReader::open(Cursor::new(duplicate), limits()).is_err());
}

#[test]
fn footer_skips_and_upgrade_preserves_future_optional_companion_schema() {
    let mut writer = writer();
    let exchange = Exchange::new(ExchangeData::new(b"future-schema".to_vec(), 1, 1, vec![]));
    let id = writer
        .append_exchange_with_metadata(exchange, complete_data())
        .unwrap();
    writer.seal().unwrap();
    let mut source = writer.into_inner().unwrap().into_inner();
    let (_, physical) = frames(&source);
    let metadata = physical
        .iter()
        .find(|value| value.frame.record_type == RecordType::ExchangeMetadata)
        .unwrap();
    let future_payload = metadata.frame.payload.clone();
    let mut replacement = Vec::new();
    RecordFrame {
        record_type: RecordType::ExchangeMetadata,
        schema_version: 77,
        flags: 2,
        payload: future_payload.clone(),
        offset: metadata.start as u64,
    }
    .write(&mut replacement)
    .unwrap();
    assert_eq!(replacement.len(), metadata.end - metadata.start);
    source.splice(metadata.start..metadata.end, replacement);

    let reader = ArchiveReader::open(Cursor::new(source.clone()), limits()).unwrap();
    assert_eq!(reader.open_path(), OpenPath::Footer);
    assert!(reader.exchange(&id).is_some());
    assert!(reader.exchange_metadata(&id).is_none());

    let mut source_cursor = Cursor::new(source);
    let (upgraded, _) = upgrade_archive(
        &mut source_cursor,
        Cursor::new(Vec::new()),
        [0x77; 16],
        77,
        b"future-upgrade".to_vec(),
        limits(),
    )
    .unwrap();
    let upgraded_bytes = upgraded.into_inner();
    let (_, upgraded_frames) = frames(&upgraded_bytes);
    let preserved = upgraded_frames
        .iter()
        .find(|value| value.frame.record_type == RecordType::ExchangeMetadata)
        .unwrap();
    assert_eq!(preserved.frame.schema_version, 77);
    assert_eq!(preserved.frame.flags, 2);
    assert_eq!(preserved.frame.payload, future_payload);

    source_cursor.rewind().unwrap();
    let mut upgraded_cursor = Cursor::new(upgraded_bytes);
    verify_upgraded_archive(&mut source_cursor, &mut upgraded_cursor, limits()).unwrap();
}

#[test]
fn upgrade_preserves_unknown_optional_outer_record_in_order() {
    let mut writer = writer();
    writer
        .append_exchange(Exchange::new(ExchangeData::new(
            b"unknown-extension".to_vec(),
            1,
            1,
            vec![],
        )))
        .unwrap();
    RecordFrame {
        record_type: RecordType::Unknown(9_000),
        schema_version: 41,
        flags: 2,
        payload: b"opaque future extension".to_vec(),
        offset: 0,
    }
    .write(writer.get_mut())
    .unwrap();
    writer.seal().unwrap();
    let source = writer.into_inner().unwrap().into_inner();
    let mut source_cursor = Cursor::new(source.clone());
    let (upgraded, _) = upgrade_archive(
        &mut source_cursor,
        Cursor::new(Vec::new()),
        [0x90; 16],
        90,
        b"unknown-upgrade".to_vec(),
        limits(),
    )
    .unwrap();
    let upgraded = upgraded.into_inner();
    let (_, output_frames) = frames(&upgraded);
    let unknown = output_frames
        .iter()
        .find(|value| value.frame.record_type == RecordType::Unknown(9_000))
        .unwrap();
    assert_eq!(unknown.frame.schema_version, 41);
    assert_eq!(unknown.frame.flags, 2);
    assert_eq!(unknown.frame.payload, b"opaque future extension");

    source_cursor.rewind().unwrap();
    let mut output_cursor = Cursor::new(upgraded);
    verify_upgraded_archive(&mut source_cursor, &mut output_cursor, limits()).unwrap();
}
