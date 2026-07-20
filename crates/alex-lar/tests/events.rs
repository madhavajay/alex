use alex_lar::{
    ArchiveReader, ArchiveWriter, ChunkerConfig, Error, Exchange, ExchangeData, FileHeader,
    HeaderAtom, HeaderBlock, HeaderFidelity, Limits, OpenPath, ParsedFrame, RecordFrame,
    RecordType, Stage, StageData, StageKind, StreamFrameKind, StreamIndex, StreamParser,
    StreamRead, TokenUsage,
};
use std::io::Cursor;

fn limits() -> Limits {
    Limits {
        max_frame_payload: 1024 * 1024,
        max_chunk_uncompressed: 128,
        max_body_length: 1024 * 1024,
        ..Limits::default()
    }
}

fn writer() -> ArchiveWriter<Cursor<Vec<u8>>> {
    ArchiveWriter::create(
        Cursor::new(Vec::new()),
        FileHeader::standalone([0x45; 16], 1_000, b"event-tests".to_vec()),
        ChunkerConfig {
            min_size: 32,
            target_size: 64,
            max_size: 128,
        },
        limits(),
    )
    .unwrap()
}

fn headers(writer: &mut ArchiveWriter<Cursor<Vec<u8>>>) -> alex_lar::HeaderBlockId {
    writer
        .append_header_block(HeaderBlock::new(
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
        ))
        .unwrap()
}

fn all_stage_kinds() -> [StageKind; 18] {
    [
        StageKind::ClientRequest,
        StageKind::NormalizedRequest,
        StageKind::RouterDecision,
        StageKind::RetryDecision,
        StageKind::FailoverDecision,
        StageKind::UpstreamRequest,
        StageKind::UpstreamResponse,
        StageKind::UpstreamFailure,
        StageKind::ClientResponse,
        StageKind::ClientTrailers,
        StageKind::ToolCall,
        StageKind::ToolResult,
        StageKind::AuthRefresh,
        StageKind::AccountRouting,
        StageKind::DarioRequest,
        StageKind::DarioResponse,
        StageKind::InjectedResponse,
        StageKind::Cancellation,
    ]
}

#[test]
fn stages_stream_and_exchange_round_trip_in_capture_order() {
    let mut writer = writer();
    let raw_stream = b"data: a\n\ndata: b\n\n";
    let body_id = writer.append_body(raw_stream).unwrap();
    assert_eq!(writer.append_body(raw_stream).unwrap(), body_id);
    assert_eq!(
        writer.chunk_uncompressed_bytes(),
        raw_stream.len() as u64,
        "referencing the stream more than once must not duplicate raw bytes"
    );
    let header_id = headers(&mut writer);
    let stream = StreamIndex::new(
        body_id,
        vec![
            StreamRead {
                byte_offset: 0,
                byte_length: 9,
                delta_from_first_byte_ns: 0,
            },
            StreamRead {
                byte_offset: 9,
                byte_length: 9,
                delta_from_first_byte_ns: 2_000_000,
            },
        ],
        vec![
            ParsedFrame {
                byte_offset: 0,
                byte_length: 9,
                delta_from_first_byte_ns: 0,
                parser: StreamParser::Sse,
                frame_kind: StreamFrameKind::SseEvent,
            },
            ParsedFrame {
                byte_offset: 9,
                byte_length: 9,
                delta_from_first_byte_ns: 2_000_000,
                parser: StreamParser::Sse,
                frame_kind: StreamFrameKind::SseEvent,
            },
        ],
    );
    let stream_id = writer.append_stream_index(stream.clone()).unwrap();
    assert_eq!(
        writer.append_stream_index(stream.clone()).unwrap(),
        stream_id
    );

    let mut stage_ids = Vec::new();
    let mut expected_stages = Vec::new();
    for (sequence, kind) in all_stage_kinds().into_iter().enumerate() {
        let mut data = StageData::new(kind, 10_000 + sequence as u64);
        if matches!(
            kind,
            StageKind::UpstreamRequest | StageKind::UpstreamResponse | StageKind::UpstreamFailure
        ) {
            data.attempt_number = Some(2);
        }
        data.request_headers_ref = Some(header_id);
        data.request_body_manifest_ref = Some(body_id);
        data.response_headers_ref = Some(header_id);
        data.response_body_manifest_ref = Some(body_id);
        data.trailers_ref = Some(header_id);
        data.provider = Some(b"xai".to_vec());
        data.requested_model = Some(b"grok-code-fast-1".to_vec());
        data.routed_model = Some(b"grok-code-fast-1".to_vec());
        data.account_id = Some(b"account-1".to_vec());
        data.routing_reason = Some(b"configured".to_vec());
        data.status_code = Some(200);
        data.usage = Some(TokenUsage {
            input_tokens: 20,
            output_tokens: 5,
            cached_tokens: 10,
            reasoning_tokens: 2,
        });
        data.cost_nanos = Some(42);
        data.cost_currency = Some(b"USD".to_vec());
        if kind == StageKind::ClientResponse {
            data.stream_index_ref = Some(stream_id);
            data.first_byte_delta_ns = Some(100);
            data.last_byte_delta_ns = Some(2_000_100);
        }
        if kind == StageKind::UpstreamFailure {
            data.error_class = Some(b"transport".to_vec());
            data.error_message = Some(b"connection reset".to_vec());
        }
        let stage = Stage::new(data);
        let id = writer.append_stage(stage.clone()).unwrap();
        assert_eq!(writer.append_stage(stage.clone()).unwrap(), id);
        stage_ids.push(id);
        expected_stages.push(stage);
    }

    let mut exchange_data = ExchangeData::new(b"trace-1".to_vec(), 77, 10_000, stage_ids.clone());
    exchange_data.session_id = Some(b"session-1".to_vec());
    exchange_data.run_id = Some(b"run-1".to_vec());
    exchange_data.parent_trace_id = Some(b"parent-trace".to_vec());
    exchange_data.monotonic_delta_ns = Some(500);
    exchange_data.clock_id = Some(b"boot-1".to_vec());
    let exchange = Exchange::new(exchange_data);
    let exchange_id = writer.append_exchange(exchange.clone()).unwrap();
    assert_eq!(
        writer.append_exchange(exchange.clone()).unwrap(),
        exchange_id
    );

    assert_eq!(writer.manifest_count(), 1);
    assert_eq!(writer.header_block_count(), 1);
    assert_eq!(writer.stream_index_count(), 1);
    assert_eq!(writer.stage_count(), all_stage_kinds().len());
    assert_eq!(writer.exchange_count(), 1);

    writer.seal().unwrap();
    let mut reader = ArchiveReader::open(writer.into_inner().unwrap(), limits()).unwrap();
    assert_eq!(reader.open_path(), OpenPath::Footer);
    assert_eq!(reader.read_body(&body_id).unwrap(), raw_stream);
    assert_eq!(reader.stream_index(&stream_id), Some(&stream));
    for (id, expected) in stage_ids.iter().zip(&expected_stages) {
        let actual = reader.stage(id).unwrap();
        assert_eq!(actual, expected);
        assert_eq!(actual.data.request_headers_ref, Some(header_id));
        assert_eq!(actual.data.request_body_manifest_ref, Some(body_id));
        assert_eq!(actual.data.response_headers_ref, Some(header_id));
        assert_eq!(actual.data.response_body_manifest_ref, Some(body_id));
        assert_eq!(actual.data.trailers_ref, Some(header_id));
    }
    assert_eq!(reader.exchange(&exchange_id), Some(&exchange));
    assert_eq!(reader.exchange_by_trace(b"trace-1"), Some(&exchange));
    assert_eq!(
        reader.exchanges_for_session(b"session-1"),
        Some(&[exchange_id][..])
    );
    assert_eq!(
        reader.exchange(&exchange_id).unwrap().data.stages,
        stage_ids
    );
}

#[test]
fn event_counts_and_nested_vectors_are_bounded_while_scanning() {
    let mut writer = writer();
    let body_id = writer.append_body(b"ab").unwrap();
    let stream_id = writer
        .append_stream_index(StreamIndex::new(
            body_id,
            vec![
                StreamRead {
                    byte_offset: 0,
                    byte_length: 1,
                    delta_from_first_byte_ns: 0,
                },
                StreamRead {
                    byte_offset: 1,
                    byte_length: 1,
                    delta_from_first_byte_ns: 1,
                },
            ],
            Vec::new(),
        ))
        .unwrap();
    let mut response = StageData::new(StageKind::ClientResponse, 1);
    response.response_body_manifest_ref = Some(body_id);
    response.stream_index_ref = Some(stream_id);
    let first = writer.append_stage(Stage::new(response)).unwrap();
    let second = writer
        .append_stage(Stage::new(StageData::new(StageKind::Cancellation, 2)))
        .unwrap();
    let mut first_exchange = ExchangeData::new(b"bounded".to_vec(), 1, 1, vec![first, second]);
    first_exchange.session_id = Some(b"bounded-session".to_vec());
    writer
        .append_exchange(Exchange::new(first_exchange))
        .unwrap();
    let mut second_exchange = ExchangeData::new(b"bounded-2".to_vec(), 2, 2, vec![first, second]);
    second_exchange.session_id = Some(b"bounded-session".to_vec());
    writer
        .append_exchange(Exchange::new(second_exchange))
        .unwrap();
    let bytes = writer.into_inner().unwrap().into_inner();

    assert!(matches!(
        ArchiveReader::open(
            Cursor::new(bytes.clone()),
            Limits {
                max_stream_reads: 1,
                ..limits()
            }
        ),
        Err(Error::Limit {
            what: "stream read count",
            actual: 2,
            limit: 1
        })
    ));
    assert!(matches!(
        ArchiveReader::open(
            Cursor::new(bytes.clone()),
            Limits {
                max_identifier_length: 3,
                ..limits()
            }
        ),
        Err(Error::Limit {
            what: "event field length",
            actual: 7,
            limit: 3
        })
    ));
    assert!(matches!(
        ArchiveReader::open(
            Cursor::new(bytes.clone()),
            Limits {
                max_session_exchanges: 1,
                ..limits()
            }
        ),
        Err(Error::Limit {
            what: "session exchange count",
            actual: 2,
            limit: 1
        })
    ));
    assert!(matches!(
        ArchiveReader::open(
            Cursor::new(bytes.clone()),
            Limits {
                max_stages: 1,
                ..limits()
            }
        ),
        Err(Error::Limit {
            what: "stage count",
            actual: 2,
            limit: 1
        })
    ));
    assert!(matches!(
        ArchiveReader::open(
            Cursor::new(bytes),
            Limits {
                max_exchange_stages: 1,
                ..limits()
            }
        ),
        Err(Error::Limit {
            what: "exchange stage count",
            actual: 2,
            limit: 1
        })
    ));
}

fn deterministic_event_archive() -> Vec<u8> {
    let mut writer = writer();
    let body_id = writer.append_body(b"xy").unwrap();
    let stream_id = writer
        .append_stream_index(StreamIndex::new(
            body_id,
            vec![StreamRead {
                byte_offset: 0,
                byte_length: 2,
                delta_from_first_byte_ns: 3,
            }],
            vec![ParsedFrame {
                byte_offset: 0,
                byte_length: 2,
                delta_from_first_byte_ns: 3,
                parser: StreamParser::Unknown(77),
                frame_kind: StreamFrameKind::Unknown(88),
            }],
        ))
        .unwrap();
    let mut stage = StageData::new(StageKind::Unknown(99), 123);
    stage.response_body_manifest_ref = Some(body_id);
    stage.stream_index_ref = Some(stream_id);
    let stage_id = writer.append_stage(Stage::new(stage)).unwrap();
    writer
        .append_exchange(Exchange::new(ExchangeData::new(
            b"deterministic".to_vec(),
            9,
            123,
            vec![stage_id],
        )))
        .unwrap();
    writer.into_inner().unwrap().into_inner()
}

#[test]
fn event_encoding_is_deterministic_and_unknown_enum_codes_round_trip() {
    let first = deterministic_event_archive();
    assert_eq!(first, deterministic_event_archive());
    let reader = ArchiveReader::open(Cursor::new(first), limits()).unwrap();
    let exchange = reader.exchange_by_trace(b"deterministic").unwrap();
    let stage = reader.stage(&exchange.data.stages[0]).unwrap();
    assert_eq!(stage.data.kind, StageKind::Unknown(99));
    let stream = reader
        .stream_index(&stage.data.stream_index_ref.unwrap())
        .unwrap();
    assert_eq!(stream.frames[0].parser, StreamParser::Unknown(77));
    assert_eq!(stream.frames[0].frame_kind, StreamFrameKind::Unknown(88));
}

#[test]
fn event_indexes_survive_reopen_for_append() {
    let bytes = deterministic_event_archive();
    let mut writer = ArchiveWriter::open_append(
        Cursor::new(bytes),
        ChunkerConfig {
            min_size: 32,
            target_size: 64,
            max_size: 128,
        },
        limits(),
    )
    .unwrap();
    assert_eq!(writer.manifest_count(), 1);
    assert_eq!(writer.stream_index_count(), 1);
    assert_eq!(writer.stage_count(), 1);
    assert_eq!(writer.exchange_count(), 1);

    let existing_stage = {
        let reader =
            ArchiveReader::open(Cursor::new(writer.get_ref().get_ref().clone()), limits()).unwrap();
        reader
            .exchange_by_trace(b"deterministic")
            .unwrap()
            .data
            .stages[0]
    };
    writer
        .append_exchange(Exchange::new(ExchangeData::new(
            b"after-reopen".to_vec(),
            10,
            124,
            vec![existing_stage],
        )))
        .unwrap();
    let reader = ArchiveReader::open(writer.into_inner().unwrap(), limits()).unwrap();
    assert!(reader.exchange_by_trace(b"deterministic").is_some());
    assert!(reader.exchange_by_trace(b"after-reopen").is_some());
}

#[test]
fn references_must_exist_and_stream_must_share_the_response_manifest() {
    let mut writer = writer();
    let body_id = writer.append_body(b"one").unwrap();
    let other_body_id = writer.append_body(b"two").unwrap();
    let stream_id = writer
        .append_stream_index(StreamIndex::new(
            body_id,
            vec![StreamRead {
                byte_offset: 0,
                byte_length: 3,
                delta_from_first_byte_ns: 0,
            }],
            Vec::new(),
        ))
        .unwrap();

    let mut mismatch = StageData::new(StageKind::ClientResponse, 1);
    mismatch.response_body_manifest_ref = Some(other_body_id);
    mismatch.stream_index_ref = Some(stream_id);
    assert!(matches!(
        writer.append_stage(Stage::new(mismatch)),
        Err(Error::Invalid(
            "stream index and response body must reference the same manifest"
        ))
    ));

    let exchange = Exchange::new(ExchangeData::new(
        b"missing-stage".to_vec(),
        1,
        1,
        vec![alex_lar::StageId([0x99; 32])],
    ));
    assert!(matches!(
        writer.append_exchange(exchange),
        Err(Error::Missing(_))
    ));
}

#[test]
fn future_required_event_schema_is_rejected_and_optional_is_skipped() {
    let clean = writer().into_inner().unwrap().into_inner();

    let mut required = clean.clone();
    RecordFrame {
        record_type: RecordType::Exchange,
        schema_version: 2,
        flags: RecordFrame::REQUIRED,
        payload: b"future".to_vec(),
        offset: required.len() as u64,
    }
    .write(&mut required)
    .unwrap();
    assert!(matches!(
        ArchiveReader::open(Cursor::new(required), limits()),
        Err(Error::Unsupported(message)) if message.contains("required record schema 2")
    ));

    let mut optional = clean;
    RecordFrame {
        record_type: RecordType::Exchange,
        schema_version: 2,
        flags: 0,
        payload: b"future".to_vec(),
        offset: optional.len() as u64,
    }
    .write(&mut optional)
    .unwrap();
    let reader = ArchiveReader::open(Cursor::new(optional), limits()).unwrap();
    assert_eq!(reader.exchange_count(), 0);
    assert_eq!(reader.record_count(), 0);
}
