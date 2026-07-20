use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use alex_core::TraceRecord;
use alex_lar::{
    ArchiveReader, ArchiveWriter, ChunkerConfig, Exchange, ExchangeData, FileHeader, Limits,
    ParsedFrame, Stage, StageData, StageKind, StreamFrameKind, StreamIndex, StreamParser,
    StreamRead,
};
use alex_store::{
    LarArchiveAvailability, LarArchiveUnavailableError, LarBodyArtifact, LarBodyStoreConfig,
    LarBodyStoreMode, LarExchangeBodyRefs, LarExchangeCapture, LarHeaderCapture,
    LarStandaloneImportOptions, LarStreamReadCapture, LarStreamReplayError,
    LarStreamReplayPageOptions, LarStreamReplaySource, LarUpstreamAttemptCapture, Store,
    MAX_STREAM_REPLAY_PAGE_BYTES, MAX_STREAM_REPLAY_PAGE_LIMIT,
};

static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn tmpdir(name: &str) -> PathBuf {
    let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "alex-lar-stream-replay-{name}-{}-{sequence}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn live_config() -> LarBodyStoreConfig {
    LarBodyStoreConfig {
        mode: LarBodyStoreMode::LarWithFallback,
        max_pack_bytes: 1,
        chunker: ChunkerConfig {
            min_size: 4,
            target_size: 4,
            max_size: 4,
        },
        ..Default::default()
    }
}

fn live_stream_fixture(root: &Path, trace_id: &str) -> (Store, String, Vec<u8>) {
    let store = Store::open_with_lar_body_store(root.to_path_buf(), live_config()).unwrap();
    let body = b"data: one\n\nmalformed\ndata: two\n\n".to_vec();
    let response = store
        .write_body_artifact(
            &LarBodyArtifact::trace(trace_id, "client_response"),
            "response.body",
            &body,
        )
        .unwrap();
    store
        .insert_trace(&TraceRecord {
            id: trace_id.into(),
            session_id: Some("stream-session".into()),
            ts_request_ms: 1_000,
            resp_body_path: Some(response.legacy_path),
            ..Default::default()
        })
        .unwrap();
    let reads = [
        (0usize, 12usize, 0u64),
        (12, 10, 2_000_000),
        (22, body.len() - 22, 9_000_000),
    ]
    .into_iter()
    .map(
        |(byte_offset, byte_length, delta_from_first_byte_ns)| LarStreamReadCapture {
            byte_offset: byte_offset as u64,
            byte_length: byte_length as u64,
            delta_from_first_byte_ns,
        },
    )
    .collect();
    store
        .write_lar_exchange_capture(
            &LarExchangeCapture {
                trace_id: trace_id.into(),
                session_id: Some("stream-session".into()),
                run_id: None,
                wall_time_ns: 1_000_000,
                client_request_headers: Some(LarHeaderCapture::observed([
                    ("Content-Type", "application/json"),
                    ("X-Repeated", "one"),
                    ("X-Repeated", "two"),
                ])),
                client_request_trailers: None,
                client_response_headers: Some(LarHeaderCapture::observed([(
                    "Content-Type",
                    "text/event-stream",
                )])),
                client_response_trailers: None,
                upstream_attempts: vec![LarUpstreamAttemptCapture {
                    attempt_number: 1,
                    wall_time_ns: 1_100_000,
                    request_headers: Some(LarHeaderCapture::observed([(
                        "Content-Type",
                        "application/json",
                    )])),
                    request_trailers: None,
                    response_headers: Some(LarHeaderCapture::observed([(
                        "Content-Type",
                        "text/event-stream",
                    )])),
                    response_trailers: None,
                    status_code: Some(200),
                    error_class: None,
                    error_message: None,
                }],
                upstream_stream_reads: Some(reads),
                provider: Some("test".into()),
                requested_model: None,
                routed_model: None,
                account_id: None,
                routing_reason: None,
                status_code: Some(200),
                error_class: None,
                error_message: None,
            },
            &LarExchangeBodyRefs {
                upstream_response_manifest_id: response.manifest_id.clone(),
                client_response_manifest_id: response.manifest_id,
                ..Default::default()
            },
        )
        .unwrap();
    let stage_id = store
        .lar_stages_for_traces(&[trace_id.to_string()])
        .unwrap()
        .remove(trace_id)
        .unwrap()
        .into_iter()
        .find(|stage| stage["kind"] == "upstream_response")
        .unwrap()["stage_id"]
        .as_str()
        .unwrap()
        .to_string();
    (store, stage_id, body)
}

fn page_options(cursor: u64, limit: usize) -> LarStreamReplayPageOptions {
    LarStreamReplayPageOptions {
        cursor,
        limit,
        ..Default::default()
    }
}

#[test]
fn active_and_sealed_raw_pages_concatenate_exactly_and_enforce_bounds() {
    let root = tmpdir("live");
    let (store, stage_id, expected) = live_stream_fixture(&root, "trace-live");

    let first = store
        .lar_stream_replay_page("trace-live", &stage_id, &page_options(0, 2))
        .unwrap();
    assert_eq!(first.archive_state, "active");
    assert_eq!(first.total_events, 3);
    assert_eq!(first.next_cursor, Some(2));
    assert_eq!(first.events[0].observed_delta_ns, 0);
    assert_eq!(first.events[1].observed_delta_ns, 2_000_000);
    assert!(first.events.iter().all(|event| event.parser.is_none()));
    let second = store
        .lar_stream_replay_page("trace-live", &stage_id, &page_options(2, 2))
        .unwrap();
    assert_eq!(second.next_cursor, None);
    let reconstructed = first
        .events
        .iter()
        .chain(&second.events)
        .flat_map(|event| event.bytes.iter().copied())
        .collect::<Vec<_>>();
    assert_eq!(reconstructed, expected);

    // The next body rotates the event-bearing pack. The identical cursor now
    // resolves through a sealed stage archive while retaining exact bytes.
    store
        .write_body_artifact(
            &LarBodyArtifact::trace("rotation", "client_request"),
            "request.json",
            b"rotate",
        )
        .unwrap();
    let sealed = store
        .lar_stream_replay_page("trace-live", &stage_id, &page_options(0, 3))
        .unwrap();
    assert_eq!(sealed.archive_state, "sealed");
    assert_eq!(
        sealed
            .events
            .iter()
            .flat_map(|event| event.bytes.iter().copied())
            .collect::<Vec<_>>(),
        expected
    );

    let parsed = store
        .lar_stream_replay_page(
            "trace-live",
            &stage_id,
            &LarStreamReplayPageOptions {
                source: LarStreamReplaySource::ParsedFrames,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(
        parsed.total_events, 0,
        "malformed/unparsed input is explicit"
    );
    assert!(parsed.events.is_empty());

    for options in [
        LarStreamReplayPageOptions {
            limit: 0,
            ..Default::default()
        },
        LarStreamReplayPageOptions {
            limit: MAX_STREAM_REPLAY_PAGE_LIMIT + 1,
            ..Default::default()
        },
        LarStreamReplayPageOptions {
            max_page_bytes: MAX_STREAM_REPLAY_PAGE_BYTES + 1,
            ..Default::default()
        },
    ] {
        let error = store
            .lar_stream_replay_page("trace-live", &stage_id, &options)
            .unwrap_err();
        assert_eq!(
            error.downcast_ref::<LarStreamReplayError>().unwrap().code(),
            "replay_invalid_request"
        );
    }
    let cursor_error = store
        .lar_stream_replay_page("trace-live", &stage_id, &page_options(4, 1))
        .unwrap_err();
    assert_eq!(
        cursor_error
            .downcast_ref::<LarStreamReplayError>()
            .unwrap()
            .code(),
        "replay_cursor_out_of_range"
    );
    let byte_error = store
        .lar_stream_replay_page(
            "trace-live",
            &stage_id,
            &LarStreamReplayPageOptions {
                max_page_bytes: 1,
                ..Default::default()
            },
        )
        .unwrap_err();
    assert_eq!(
        byte_error
            .downcast_ref::<LarStreamReplayError>()
            .unwrap()
            .code(),
        "replay_event_too_large"
    );

    let client_stage = store
        .lar_stages_for_traces(&["trace-live".into()])
        .unwrap()
        .remove("trace-live")
        .unwrap()
        .into_iter()
        .find(|stage| stage["kind"] == "client_response")
        .unwrap()["stage_id"]
        .as_str()
        .unwrap()
        .to_string();
    let no_index = store
        .lar_stream_replay_page("trace-live", &client_stage, &page_options(0, 1))
        .unwrap_err();
    assert_eq!(
        no_index
            .downcast_ref::<LarStreamReplayError>()
            .unwrap()
            .code(),
        "replay_not_captured"
    );

    let file_uuid = sealed.archive_file_uuid;
    store.detach_lar_archive(&file_uuid).unwrap();
    let offline = store
        .lar_stream_replay_page("trace-live", &stage_id, &page_options(0, 1))
        .unwrap_err();
    let offline = offline
        .downcast_ref::<LarArchiveUnavailableError>()
        .unwrap();
    assert_eq!(
        offline.availability,
        LarArchiveAvailability::ArchivedOffline
    );
    assert_eq!(offline.file_uuid, file_uuid);
}

#[test]
fn bodies_only_retention_removes_header_and_stream_graph_from_export() {
    let root = tmpdir("retention-export");
    let (store, _stage_id, _body) = live_stream_fixture(&root, "trace-retained-metadata");

    store.prune(2_000, true, false).unwrap();
    let interchange = store
        .lar_interchange_trace("trace-retained-metadata")
        .unwrap()
        .unwrap();
    assert!(interchange.bodies.is_empty());
    assert!(interchange.stages.iter().all(|stage| {
        stage.data.request_headers_ref.is_none()
            && stage.data.request_body_manifest_ref.is_none()
            && stage.data.response_headers_ref.is_none()
            && stage.data.response_body_manifest_ref.is_none()
            && stage.data.trailers_ref.is_none()
            && stage.data.stream_index_ref.is_none()
    }));

    let export_path = root.join("retained-metadata.lar");
    let file = OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .open(&export_path)
        .unwrap();
    let mut writer = ArchiveWriter::create(
        file,
        FileHeader::standalone([0x94; 16], 2_000_000_000, b"retention-export".to_vec()),
        ChunkerConfig::default(),
        Limits::default(),
    )
    .unwrap();
    assert!(store
        .append_exact_trace_to_standalone(&mut writer, "trace-retained-metadata")
        .unwrap());
    writer.seal().unwrap();
    drop(writer);

    let exported =
        ArchiveReader::open(File::open(export_path).unwrap(), Limits::default()).unwrap();
    assert_eq!(exported.manifest_count(), 0);
    assert_eq!(exported.header_block_count(), 0);
    assert_eq!(exported.stream_index_count(), 0);
}

fn write_parsed_archive(path: &Path) -> (String, Vec<u8>, Vec<u8>) {
    let file = OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .open(path)
        .unwrap();
    let mut writer = ArchiveWriter::create(
        file,
        FileHeader::standalone([0x93; 16], 1_000_000, b"replay-pages".to_vec()),
        ChunkerConfig {
            min_size: 4,
            target_size: 4,
            max_size: 4,
        },
        Limits::default(),
    )
    .unwrap();
    let body = b"data: one\n\nbad\ndata: two\n\n{\"n\":3}\n".to_vec();
    let manifest_id = writer.append_body(&body).unwrap();
    let stream_id = writer
        .append_stream_index(StreamIndex::new(
            manifest_id,
            vec![StreamRead {
                byte_offset: 0,
                byte_length: body.len() as u64,
                delta_from_first_byte_ns: 0,
            }],
            vec![
                ParsedFrame {
                    byte_offset: 0,
                    byte_length: 11,
                    delta_from_first_byte_ns: 0,
                    parser: StreamParser::Sse,
                    frame_kind: StreamFrameKind::SseEvent,
                },
                ParsedFrame {
                    byte_offset: 15,
                    byte_length: 11,
                    delta_from_first_byte_ns: 5_000_000,
                    parser: StreamParser::Sse,
                    frame_kind: StreamFrameKind::SseEvent,
                },
                ParsedFrame {
                    byte_offset: 26,
                    byte_length: 8,
                    delta_from_first_byte_ns: 9_000_000,
                    parser: StreamParser::Ndjson,
                    frame_kind: StreamFrameKind::NdjsonRecord,
                },
            ],
        ))
        .unwrap();
    let mut stage = StageData::new(StageKind::UpstreamResponse, 1_100_000);
    stage.attempt_number = Some(1);
    stage.response_body_manifest_ref = Some(manifest_id);
    stage.stream_index_ref = Some(stream_id);
    let stage_id = writer.append_stage(Stage::new(stage)).unwrap();
    writer
        .append_exchange(Exchange::new(ExchangeData::new(
            b"trace-parsed".to_vec(),
            1,
            1_000_000,
            vec![stage_id],
        )))
        .unwrap();
    writer.seal().unwrap();
    writer.get_ref().sync_all().unwrap();
    let parsed_bytes = [&body[0..11], &body[15..26], &body[26..34]].concat();
    (stage_id.to_string(), body, parsed_bytes)
}

#[test]
fn parsed_frames_page_from_sealed_archive_and_missing_is_typed() {
    let root = tmpdir("parsed");
    let archive = root.join("parsed.lar");
    let (stage_id, raw_body, parsed_bytes) = write_parsed_archive(&archive);
    let store = Store::open(root.clone()).unwrap();
    let report = store
        .import_sealed_lar_archive(
            &archive,
            &LarStandaloneImportOptions {
                insert_trace_rows: true,
                ..Default::default()
            },
        )
        .unwrap();
    let parsed_options = |cursor, limit| LarStreamReplayPageOptions {
        source: LarStreamReplaySource::ParsedFrames,
        cursor,
        limit,
        ..Default::default()
    };
    let first = store
        .lar_stream_replay_page("trace-parsed", &stage_id, &parsed_options(0, 2))
        .unwrap();
    let second = store
        .lar_stream_replay_page("trace-parsed", &stage_id, &parsed_options(2, 2))
        .unwrap();
    assert_eq!(first.archive_state, "sealed");
    assert_eq!(first.next_cursor, Some(2));
    assert_eq!(second.next_cursor, None);
    assert_eq!(first.events[0].parser.as_deref(), Some("sse"));
    assert_eq!(first.events[0].frame_kind.as_deref(), Some("sse_event"));
    assert_eq!(second.events[0].parser.as_deref(), Some("ndjson"));
    assert_eq!(
        second.events[0].frame_kind.as_deref(),
        Some("ndjson_record")
    );
    assert_eq!(
        first
            .events
            .iter()
            .chain(&second.events)
            .flat_map(|event| event.bytes.iter().copied())
            .collect::<Vec<_>>(),
        parsed_bytes
    );
    let raw = store
        .lar_stream_replay_page(
            "trace-parsed",
            &stage_id,
            &LarStreamReplayPageOptions::default(),
        )
        .unwrap();
    assert_eq!(raw.events[0].bytes, raw_body);

    let missing_path = root.join("parsed.missing");
    std::fs::rename(&archive, &missing_path).unwrap();
    let missing = store
        .lar_stream_replay_page("trace-parsed", &stage_id, &parsed_options(0, 1))
        .unwrap_err();
    let missing = missing
        .downcast_ref::<LarArchiveUnavailableError>()
        .unwrap();
    assert_eq!(
        missing.availability,
        LarArchiveAvailability::ArchivedMissing
    );
    assert_eq!(missing.file_uuid, report.file_uuid);
}
