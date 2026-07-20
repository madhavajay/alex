use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use alex_core::TraceRecord;
use alex_lar::{ArchiveReader, Limits, OpenPath};
use alex_store::{
    LarArtifactLocation, LarArtifactReadRequest, LarBodyArtifact, LarBodyStoreConfig,
    LarBodyStoreMode, LarLegacyImportOptions, LarLegacyResourceControls, Store, ToolCallRecord,
};
use flate2::write::GzEncoder;
use flate2::Compression;

static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn tmpdir(name: &str) -> PathBuf {
    let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "alex-lar-legacy-import-{name}-{}-{sequence}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn write_gzip(path: &Path, bytes: &[u8]) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let file = std::fs::File::create(path).unwrap();
    let mut encoder = GzEncoder::new(file, Compression::default());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap();
}

fn read_gzip(path: &Path) -> Vec<u8> {
    let file = std::fs::File::open(path).unwrap();
    let mut decoder = flate2::read::GzDecoder::new(file);
    let mut bytes = Vec::new();
    decoder.read_to_end(&mut bytes).unwrap();
    bytes
}

fn trace(id: &str, session_id: &str) -> TraceRecord {
    TraceRecord {
        id: id.into(),
        ts_request_ms: 1,
        session_id: Some(session_id.into()),
        ..TraceRecord::default()
    }
}

fn low_disk_probe(_: &Path) -> std::io::Result<u64> {
    Ok(512)
}

#[test]
fn low_disk_pauses_before_archive_writes_and_keeps_legacy_readable() {
    let data_dir = tmpdir("low-disk-pause");
    let store = Store::open(data_dir.clone()).unwrap();
    let source = store
        .write_body("trace-low-disk", "request.json", b"still legacy")
        .unwrap();
    let mut row = trace("trace-low-disk", "session-low-disk");
    row.req_body_path = Some(source.clone());
    store.insert_trace(&row).unwrap();

    let report = store
        .run_lar_legacy_import(&LarLegacyImportOptions {
            resources: LarLegacyResourceControls {
                min_free_disk_bytes: Some(1_024),
                ..Default::default()
            },
            disk_free_bytes_probe: Some(low_disk_probe),
            ..LarLegacyImportOptions::default()
        })
        .unwrap();
    assert!(report.claimed);
    assert_eq!(report.job_state, "pending");
    assert_eq!(report.free_disk_bytes, Some(512));
    assert!(report
        .paused_reason
        .as_deref()
        .unwrap()
        .contains("low_disk"));
    assert_eq!((report.attempted, report.migrated), (0, 0));
    assert!(!report.file_path.exists());
    assert!(Path::new(&source).is_file());
    assert!(matches!(
        store
            .lar_artifact_location("trace", "trace-low-disk", "client_request", None)
            .unwrap(),
        Some(LarArtifactLocation::Legacy { .. })
    ));
}

#[test]
fn worker_pool_and_progress_controls_are_observable() {
    let store = Store::open(tmpdir("worker-progress")).unwrap();
    for index in 0..4 {
        let id = format!("trace-worker-{index}");
        let mut row = trace(&id, "worker-session");
        row.ts_request_ms = index;
        row.req_body_path = Some(
            store
                .write_body(&id, "request.json", format!("body-{index}").as_bytes())
                .unwrap(),
        );
        store.insert_trace(&row).unwrap();
    }
    let report = store
        .run_lar_legacy_import(&LarLegacyImportOptions {
            batch_size: 4,
            resources: LarLegacyResourceControls {
                worker_count: 2,
                yield_every_artifacts: 1,
                ..Default::default()
            },
            ..LarLegacyImportOptions::default()
        })
        .unwrap();
    assert_eq!(report.configured_worker_count, 2);
    assert_eq!(report.workers_used, 2);
    assert_eq!(report.configured_batch_size, 4);
    assert_eq!(
        (
            report.total_items,
            report.completed_items,
            report.remaining_items
        ),
        (4, 4, 0)
    );
    assert_eq!(report.progress_percent, 100.0);
    assert_eq!(report.eta_seconds, None);
    assert_eq!(report.yield_count, 4);
    assert_eq!(report.last_error, None);
}

#[test]
fn small_memory_policy_streams_large_predecessor_candidates_exactly() {
    let store = Store::open(tmpdir("bounded-memory-stream")).unwrap();
    let first_body = vec![b'a'; 2 * 1024 * 1024];
    let mut second_body = first_body.clone();
    second_body.extend_from_slice(b"new turn");
    for (index, body) in [&first_body, &second_body].into_iter().enumerate() {
        let id = format!("trace-memory-{index}");
        let mut row = trace(&id, "memory-session");
        row.ts_request_ms = index as i64;
        row.req_body_path = Some(store.write_body(&id, "request.json", body).unwrap());
        store.insert_trace(&row).unwrap();
    }
    let report = store
        .run_lar_legacy_import(&LarLegacyImportOptions {
            batch_size: 2,
            resources: LarLegacyResourceControls {
                max_memory_bytes: 1024 * 1024,
                ..Default::default()
            },
            ..LarLegacyImportOptions::default()
        })
        .unwrap();
    assert_eq!(report.configured_max_memory_bytes, 1024 * 1024);
    assert_eq!((report.migrated, report.failed), (2, 0));
    assert_eq!(
        store
            .read_lar_or_legacy_artifact("trace", "trace-memory-1", "client_request", None,)
            .unwrap()
            .unwrap(),
        second_body
    );
}

#[test]
fn imports_all_five_catalog_body_columns_and_reads_exact_bytes() {
    let data_dir = tmpdir("all-columns");
    let store = Store::open(data_dir).unwrap();
    let request = store
        .write_body("trace-1", "request.json", b"request bytes")
        .unwrap();
    let upstream = store
        .write_body("trace-1", "upstream-request.json", b"upstream bytes")
        .unwrap();
    let response = store
        .write_body("trace-1", "response.body", b"response\0bytes")
        .unwrap();
    let mut row = trace("trace-1", "session-1");
    row.req_body_path = Some(request.clone());
    row.upstream_req_body_path = Some(upstream.clone());
    row.resp_body_path = Some(response.clone());
    store.insert_trace(&row).unwrap();

    let arguments = store
        .write_body("tool-1", "tool-args.json", br#"{"command":"pwd"}"#)
        .unwrap();
    let result = store
        .write_body("tool-1", "tool-result.json", b"/tmp\n")
        .unwrap();
    store
        .upsert_tool_call(&ToolCallRecord {
            id: "tool-1".into(),
            harness: "pi".into(),
            session_id: "session-1".into(),
            turn_id: Some("turn-1".into()),
            tool_call_id: "call-1".into(),
            trace_id: Some("trace-1".into()),
            tool_name: "bash".into(),
            ts_start_ms: 2,
            ts_end_ms: Some(3),
            is_error: Some(false),
            exit_status: Some(0),
            args_body_path: Some(arguments.clone()),
            result_body_path: Some(result.clone()),
        })
        .unwrap();

    let report = store
        .run_lar_legacy_import(&LarLegacyImportOptions::default())
        .unwrap();
    assert_eq!(
        (report.attempted, report.migrated, report.failed),
        (5, 5, 0)
    );
    assert_eq!(report.job_state, "complete");
    assert_eq!(
        store
            .read_lar_or_legacy_artifact("trace", "trace-1", "client_request", None)
            .unwrap()
            .unwrap(),
        b"request bytes"
    );
    assert_eq!(
        store
            .read_lar_or_legacy_artifact("trace", "trace-1", "upstream_request", None)
            .unwrap()
            .unwrap(),
        b"upstream bytes"
    );
    assert_eq!(
        store
            .read_lar_or_legacy_artifact("trace", "trace-1", "client_response", None)
            .unwrap()
            .unwrap(),
        b"response\0bytes"
    );
    assert_eq!(
        store
            .read_lar_or_legacy_artifact("tool_call", "tool-1", "tool_arguments", None)
            .unwrap()
            .unwrap(),
        br#"{"command":"pwd"}"#
    );
    assert_eq!(
        store
            .read_lar_or_legacy_artifact("tool_call", "tool-1", "tool_result", None)
            .unwrap()
            .unwrap(),
        b"/tmp\n"
    );
    let batch = store
        .read_lar_or_legacy_artifact_batch(&[
            LarArtifactReadRequest::new("trace", "trace-1", "client_request"),
            LarArtifactReadRequest::new("trace", "trace-1", "client_response"),
            LarArtifactReadRequest::new("tool_call", "tool-1", "tool_result"),
        ])
        .unwrap();
    assert_eq!(batch[0].as_deref(), Some(b"request bytes".as_slice()));
    assert_eq!(batch[1].as_deref(), Some(b"response\0bytes".as_slice()));
    assert_eq!(batch[2].as_deref(), Some(b"/tmp\n".as_slice()));
    for path in [request, upstream, response, arguments, result] {
        assert!(Path::new(&path).is_file(), "import must not delete {path}");
    }
}

#[test]
fn identical_bodies_share_one_manifest_and_repeat_is_idempotent() {
    let data_dir = tmpdir("dedupe-idempotent");
    let store = Store::open(data_dir).unwrap();
    let body = vec![b'x'; 300_000];
    let request = store.write_body("trace-1", "request.json", &body).unwrap();
    let upstream = store
        .write_body("trace-1", "upstream-request.json", &body)
        .unwrap();
    let mut row = trace("trace-1", "session-1");
    row.req_body_path = Some(request);
    row.upstream_req_body_path = Some(upstream);
    store.insert_trace(&row).unwrap();

    let first = store
        .run_lar_legacy_import(&LarLegacyImportOptions {
            batch_size: 1,
            ..LarLegacyImportOptions::default()
        })
        .unwrap();
    assert_eq!((first.migrated, first.failed), (2, 0));
    assert!(first.unique_bytes_written <= body.len() as u64);
    assert!(first.bytes_deduplicated >= body.len() as u64);
    assert_eq!(
        first.unique_bytes_written + first.bytes_deduplicated,
        (body.len() * 2) as u64
    );
    let file_size = std::fs::metadata(&first.file_path).unwrap().len();
    let reader = ArchiveReader::open(
        std::fs::File::open(&first.file_path).unwrap(),
        Limits::default(),
    )
    .unwrap();
    assert_eq!(reader.manifest_count(), 1);
    assert_eq!(reader.open_path(), OpenPath::Checkpoint);

    let repeated = store
        .run_lar_legacy_import(&LarLegacyImportOptions::default())
        .unwrap();
    assert!(!repeated.claimed, "a completed migration is a no-op");
    assert_eq!((repeated.attempted, repeated.migrated), (0, 0));
    assert_eq!(
        std::fs::metadata(&first.file_path).unwrap().len(),
        file_size
    );
}

#[test]
fn missing_and_corrupt_sources_are_explicit_failures_and_keep_paths() {
    let data_dir = tmpdir("source-failures");
    let store = Store::open(data_dir.clone()).unwrap();
    let missing = data_dir.join("bodies/2026-01-01/missing.request.json.gz");
    let corrupt = data_dir.join("bodies/2026-01-01/corrupt.response.body.gz");
    std::fs::create_dir_all(corrupt.parent().unwrap()).unwrap();
    std::fs::write(&corrupt, b"not a gzip stream").unwrap();
    let mut row = trace("trace-1", "session-1");
    row.req_body_path = Some(missing.to_string_lossy().into_owned());
    row.resp_body_path = Some(corrupt.to_string_lossy().into_owned());
    store.insert_trace(&row).unwrap();

    let report = store
        .run_lar_legacy_import(&LarLegacyImportOptions::default())
        .unwrap();
    assert_eq!((report.migrated, report.failed), (0, 2));
    assert_eq!(report.job_state, "failed");
    assert!(report
        .errors
        .iter()
        .any(|error| error.error_kind == "missing"));
    assert!(report
        .errors
        .iter()
        .any(|error| error.error_kind == "corrupt"));
    assert!(matches!(
        store
            .lar_artifact_location("trace", "trace-1", "client_request", None)
            .unwrap(),
        Some(LarArtifactLocation::Unavailable { error, .. }) if error.kind == "missing"
    ));
    assert!(matches!(
        store
            .lar_artifact_location("trace", "trace-1", "client_response", None)
            .unwrap(),
        Some(LarArtifactLocation::Unavailable { error, .. }) if error.kind == "corrupt"
    ));
    let stored = store.get_trace("trace-1").unwrap().unwrap();
    assert_eq!(stored["req_body_path"].as_str(), missing.to_str());
    assert_eq!(stored["resp_body_path"].as_str(), corrupt.to_str());
    assert!(corrupt.is_file());

    write_gzip(&missing, b"repaired request");
    write_gzip(&corrupt, b"repaired response");
    let repaired = store
        .run_lar_legacy_import(&LarLegacyImportOptions::default())
        .unwrap();
    assert_eq!((repaired.migrated, repaired.failed), (2, 0));
    assert_eq!(repaired.job_state, "complete");
    assert_eq!(
        store
            .read_lar_or_legacy_artifact("trace", "trace-1", "client_request", None)
            .unwrap()
            .unwrap(),
        b"repaired request"
    );
}

#[test]
fn bounded_limit_resumes_and_archive_tail_must_validate_before_switch() {
    let data_dir = tmpdir("limit-validation");
    let store = Store::open(data_dir).unwrap();
    let source = store
        .write_body("trace-1", "request.json", b"safe source")
        .unwrap();
    let mut row = trace("trace-1", "session-1");
    row.req_body_path = Some(source.clone());
    store.insert_trace(&row).unwrap();

    let zero = store
        .run_lar_legacy_import(&LarLegacyImportOptions {
            limit: Some(0),
            ..LarLegacyImportOptions::default()
        })
        .unwrap();
    assert_eq!(zero.attempted, 0);
    assert!(zero.limit_reached);
    let mut archive = std::fs::OpenOptions::new()
        .append(true)
        .open(&zero.file_path)
        .unwrap();
    archive.write_all(b"interrupted-tail").unwrap();
    archive.sync_all().unwrap();

    let error = store
        .run_lar_legacy_import(&LarLegacyImportOptions {
            limit: Some(1),
            ..LarLegacyImportOptions::default()
        })
        .unwrap_err();
    assert!(error.to_string().contains("truncated tail"));
    assert_eq!(
        store
            .lar_artifact_location("trace", "trace-1", "client_request", None)
            .unwrap(),
        Some(LarArtifactLocation::Legacy {
            path: source,
            migration_error: None,
        })
    );
}

#[test]
fn bounded_limit_resumes_remaining_items_on_the_same_archive() {
    let data_dir = tmpdir("limit-resume");
    let store = Store::open(data_dir).unwrap();
    let mut row = trace("trace-1", "session-1");
    row.req_body_path = Some(store.write_body("trace-1", "request", b"one").unwrap());
    row.upstream_req_body_path = Some(
        store
            .write_body("trace-1", "upstream-request", b"two")
            .unwrap(),
    );
    row.resp_body_path = Some(store.write_body("trace-1", "response", b"three").unwrap());
    store.insert_trace(&row).unwrap();

    let first = store
        .run_lar_legacy_import(&LarLegacyImportOptions {
            limit: Some(1),
            ..LarLegacyImportOptions::default()
        })
        .unwrap();
    assert_eq!((first.attempted, first.migrated), (1, 1));
    assert!(first.limit_reached);
    assert_eq!(first.job_state, "pending");

    let resumed = store
        .run_lar_legacy_import(&LarLegacyImportOptions::default())
        .unwrap();
    assert_eq!((resumed.attempted, resumed.migrated), (2, 2));
    assert_eq!(resumed.file_uuid, first.file_uuid);
    assert_eq!(resumed.file_path, first.file_path);
    assert_eq!(resumed.job_state, "complete");
}

#[test]
fn pack_index_cap_rotates_between_validated_artifacts_and_bounds_each_reader() {
    let data_dir = tmpdir("bounded-pack-index");
    let store = Store::open(data_dir.clone()).unwrap();
    let mut expected = Vec::new();
    for index in 0..5 {
        let id = format!("trace-pack-{index}");
        let body = format!("unique body {index}").into_bytes();
        let mut row = trace(&id, "pack-session");
        row.req_body_path = Some(store.write_body(&id, "request.json", &body).unwrap());
        store.insert_trace(&row).unwrap();
        expected.push((id, body));
    }

    let report = store
        .run_lar_legacy_import(&LarLegacyImportOptions {
            batch_size: 5,
            resources: LarLegacyResourceControls {
                max_pack_bytes: u64::MAX,
                // One small body contributes one chunk plus one manifest.
                max_pack_index_entries: 2,
                ..Default::default()
            },
            ..LarLegacyImportOptions::default()
        })
        .unwrap();
    assert_eq!((report.migrated, report.failed), (5, 0));
    assert_eq!(report.effective_max_pack_index_entries, 2);
    assert_eq!(report.pack_sequence, 4);
    assert_eq!(report.packs_rotated, 4);

    let catalog = rusqlite::Connection::open(data_dir.join("alexandria.sqlite3")).unwrap();
    let packs: Vec<(String, String)> = catalog
        .prepare(
            "SELECT path, state FROM lar_files
             WHERE archive_set_uuid=?1 AND role='body-pack' ORDER BY path",
        )
        .unwrap()
        .query_map([&report.archive_set_uuid], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(packs.len(), 5);
    assert_eq!(
        packs.iter().filter(|(_, state)| state == "sealed").count(),
        4
    );
    assert_eq!(
        packs.iter().filter(|(_, state)| state == "active").count(),
        1
    );
    for (path, _) in packs {
        let reader =
            ArchiveReader::open(std::fs::File::open(path).unwrap(), Limits::default()).unwrap();
        assert_eq!(reader.manifest_count(), 1);
    }
    for (id, body) in expected {
        assert_eq!(
            store
                .read_lar_or_legacy_artifact("trace", &id, "client_request", None)
                .unwrap()
                .unwrap(),
            body
        );
    }
}

#[test]
fn capped_pack_continuation_resumes_on_a_new_deterministic_pack() {
    let data_dir = tmpdir("bounded-pack-resume");
    let store = Store::open(data_dir.clone()).unwrap();
    for index in 0..3 {
        let id = format!("trace-pack-resume-{index}");
        let mut row = trace(&id, "pack-resume-session");
        row.req_body_path = Some(
            store
                .write_body(&id, "request.json", format!("resume {index}").as_bytes())
                .unwrap(),
        );
        store.insert_trace(&row).unwrap();
    }
    let resources = LarLegacyResourceControls {
        max_pack_bytes: u64::MAX,
        max_pack_index_entries: 2,
        ..Default::default()
    };
    let first = store
        .run_lar_legacy_import(&LarLegacyImportOptions {
            limit: Some(2),
            batch_size: 3,
            resources: resources.clone(),
            ..LarLegacyImportOptions::default()
        })
        .unwrap();
    assert_eq!((first.migrated, first.pack_sequence), (2, 1));
    assert_eq!(first.job_state, "pending");

    let resumed = store
        .run_lar_legacy_import(&LarLegacyImportOptions {
            batch_size: 3,
            resources,
            ..LarLegacyImportOptions::default()
        })
        .unwrap();
    assert_eq!((resumed.migrated, resumed.pack_sequence), (1, 2));
    assert_eq!(resumed.packs_rotated, 1);
    assert_eq!(resumed.job_id, first.job_id);
    assert_eq!(resumed.job_state, "complete");

    let catalog = rusqlite::Connection::open(data_dir.join("alexandria.sqlite3")).unwrap();
    let pack_count: i64 = catalog
        .query_row(
            "SELECT COUNT(*) FROM lar_files WHERE archive_set_uuid=?1 AND role='body-pack'",
            [&resumed.archive_set_uuid],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(pack_count, 3);
    let sealed_count: i64 = catalog
        .query_row(
            "SELECT COUNT(*) FROM lar_files
             WHERE archive_set_uuid=?1 AND role='body-pack' AND state='sealed'",
            [&resumed.archive_set_uuid],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(sealed_count, 2);
}

#[test]
fn pack_byte_cap_rotates_after_the_artifact_that_crosses_it() {
    let data_dir = tmpdir("bounded-pack-bytes");
    let store = Store::open(data_dir.clone()).unwrap();
    for index in 0..2 {
        let id = format!("trace-pack-bytes-{index}");
        let mut row = trace(&id, "pack-byte-session");
        row.req_body_path = Some(
            store
                .write_body(&id, "request.json", format!("byte cap {index}").as_bytes())
                .unwrap(),
        );
        store.insert_trace(&row).unwrap();
    }
    let report = store
        .run_lar_legacy_import(&LarLegacyImportOptions {
            batch_size: 2,
            resources: LarLegacyResourceControls {
                // The empty header is allowed; each completed non-empty pack
                // crosses this soft cap and rotates before the next artifact.
                max_pack_bytes: 1,
                max_pack_index_entries: usize::MAX,
                ..Default::default()
            },
            ..LarLegacyImportOptions::default()
        })
        .unwrap();
    assert_eq!((report.migrated, report.pack_sequence), (2, 1));
    assert_eq!(report.packs_rotated, 1);
    let catalog = rusqlite::Connection::open(data_dir.join("alexandria.sqlite3")).unwrap();
    let pack_count: i64 = catalog
        .query_row(
            "SELECT COUNT(*) FROM lar_files WHERE archive_set_uuid=?1 AND role='body-pack'",
            [&report.archive_set_uuid],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(pack_count, 2);
}

#[test]
fn legacy_dual_and_fallback_writes_survive_every_supported_mode_reopen() {
    let data_dir = tmpdir("mode-upgrade-downgrade");
    let scenarios = [
        ("legacy", LarBodyStoreMode::Legacy),
        ("dual", LarBodyStoreMode::DualWriteValidated),
        ("fallback", LarBodyStoreMode::LarWithFallback),
    ];
    let mut expected = Vec::new();
    for (name, mode) in scenarios {
        let store = Store::open_with_lar_body_store(
            data_dir.clone(),
            LarBodyStoreConfig {
                mode,
                ..Default::default()
            },
        )
        .unwrap();
        let trace_id = format!("trace-mode-{name}");
        let body = format!("body written in {name}").into_bytes();
        let result = store
            .write_body_artifact(
                &LarBodyArtifact::trace(&trace_id, "client_request"),
                "request.json",
                &body,
            )
            .unwrap();
        let mut row = trace(&trace_id, "mode-session");
        row.req_body_path = Some(result.legacy_path.clone());
        store.insert_trace(&row).unwrap();
        assert_eq!(read_gzip(Path::new(&result.legacy_path)), body);
        let location = store
            .lar_artifact_location("trace", &trace_id, "client_request", None)
            .unwrap()
            .unwrap();
        match mode {
            LarBodyStoreMode::Legacy | LarBodyStoreMode::DualWriteValidated => {
                assert!(matches!(location, LarArtifactLocation::Legacy { .. }));
            }
            LarBodyStoreMode::LarWithFallback => {
                assert!(matches!(location, LarArtifactLocation::Lar { .. }));
            }
        }
        expected.push((trace_id, body, result.legacy_path));
    }

    for mode in [
        LarBodyStoreMode::Legacy,
        LarBodyStoreMode::DualWriteValidated,
        LarBodyStoreMode::LarWithFallback,
    ] {
        let store = Store::open_with_lar_body_store(
            data_dir.clone(),
            LarBodyStoreConfig {
                mode,
                ..Default::default()
            },
        )
        .unwrap();
        for (trace_id, body, legacy_path) in &expected {
            assert_eq!(
                store
                    .read_lar_or_legacy_artifact("trace", trace_id, "client_request", None,)
                    .unwrap()
                    .unwrap(),
                *body
            );
            assert_eq!(read_gzip(Path::new(legacy_path)), *body);
        }
    }
}

#[test]
fn sequential_session_requests_reuse_predecessor_ranges_across_restart() {
    let data_dir = tmpdir("predecessor-restart");
    let store = Store::open(data_dir).unwrap();
    let base: Vec<u8> = (0..200_000)
        .map(|index| ((index * 67 + index / 251) % 256) as u8)
        .collect();
    let mut bodies = Vec::new();
    for turn in 0..3 {
        let mut body = base.clone();
        for previous in 0..=turn {
            body.extend_from_slice(
                format!("\nnew-session-turn-{previous}:{}", "x".repeat(300)).as_bytes(),
            );
        }
        let id = format!("trace-{turn}");
        let path = store.write_body(&id, "request.json", &body).unwrap();
        let mut row = trace(&id, "growing-session");
        row.ts_request_ms = 1_000 + turn as i64;
        row.req_body_path = Some(path);
        store.insert_trace(&row).unwrap();
        bodies.push((id, body));
    }

    let first = store
        .run_lar_legacy_import(&LarLegacyImportOptions {
            limit: Some(1),
            ..LarLegacyImportOptions::default()
        })
        .unwrap();
    assert_eq!((first.migrated, first.failed), (1, 0));
    assert!(first.limit_reached);

    let resumed = store
        .run_lar_legacy_import(&LarLegacyImportOptions::default())
        .unwrap();
    assert_eq!((resumed.migrated, resumed.failed), (2, 0));
    assert!(
        resumed.unique_bytes_written < 4_000,
        "only genuinely appended turn bytes should be new, got {}",
        resumed.unique_bytes_written
    );
    for (id, expected) in bodies {
        assert_eq!(
            store
                .read_lar_or_legacy_artifact("trace", &id, "client_request", None)
                .unwrap()
                .unwrap(),
            expected
        );
    }
}

#[test]
fn dario_suffix_records_are_imported_through_generic_inventory_hook() {
    let data_dir = tmpdir("dario-suffix");
    let store = Store::open(data_dir.clone()).unwrap();
    let request = store
        .write_body("trace-1", "request.json", b"client")
        .unwrap();
    let mut row = trace("trace-1", "session-1");
    row.req_body_path = Some(request);
    row.via_dario = true;
    store.insert_trace(&row).unwrap();
    let dario = data_dir.join("bodies/2026-01-01/trace-1.dario-upstream-request.json.gz");
    write_gzip(&dario, b"dario exact bytes");

    let report = store
        .run_lar_legacy_import(&LarLegacyImportOptions::default())
        .unwrap();
    assert_eq!(report.migrated, 2);
    assert_eq!(
        store
            .read_lar_or_legacy_artifact("trace", "trace-1", "dario_upstream_request", None,)
            .unwrap()
            .unwrap(),
        b"dario exact bytes"
    );
}

#[test]
fn imported_chunks_enter_global_catalog_and_live_writer_reuses_them() {
    let data_dir = tmpdir("import-global-chunks");
    let body = vec![b'x'; 4_096];
    let store = Store::open(data_dir.clone()).unwrap();
    let path = store
        .write_body("trace-old", "request.json", &body)
        .unwrap();
    let mut row = trace("trace-old", "session-old");
    row.req_body_path = Some(path);
    store.insert_trace(&row).unwrap();
    let report = store
        .run_lar_legacy_import(&LarLegacyImportOptions::default())
        .unwrap();
    assert_eq!((report.migrated, report.failed), (1, 0));

    let catalog = rusqlite::Connection::open(data_dir.join("alexandria.sqlite3")).unwrap();
    let imported_chunks: i64 = catalog
        .query_row("SELECT COUNT(*) FROM lar_chunks", [], |row| row.get(0))
        .unwrap();
    let imported_edges: i64 = catalog
        .query_row("SELECT COUNT(*) FROM lar_manifest_chunks", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert!(imported_chunks > 0);
    assert!(imported_edges > 0);
    catalog
        .execute("DELETE FROM lar_manifest_chunks", [])
        .unwrap();
    catalog.execute("DELETE FROM lar_chunks", []).unwrap();
    drop(catalog);
    let backfill = store
        .run_lar_legacy_import(&LarLegacyImportOptions::default())
        .unwrap();
    assert_eq!(backfill.job_state, "complete");
    let catalog = rusqlite::Connection::open(data_dir.join("alexandria.sqlite3")).unwrap();
    assert_eq!(
        catalog
            .query_row("SELECT COUNT(*) FROM lar_chunks", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        imported_chunks
    );
    drop(catalog);
    drop(store);

    let live = Store::open_with_lar_body_store(
        data_dir.clone(),
        LarBodyStoreConfig {
            mode: LarBodyStoreMode::DualWriteValidated,
            ..Default::default()
        },
    )
    .unwrap();
    let result = live
        .write_body_artifact(
            &LarBodyArtifact::trace("trace-new", "client_request"),
            "request.json",
            &body,
        )
        .unwrap();
    assert!(result.lar_error.is_none());
    let catalog = rusqlite::Connection::open(data_dir.join("alexandria.sqlite3")).unwrap();
    let after: i64 = catalog
        .query_row("SELECT COUNT(*) FROM lar_chunks", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        after, imported_chunks,
        "live write copied an imported chunk"
    );
}

#[test]
fn completed_import_starts_one_deterministic_generation_for_new_legacy_writes() {
    let data_dir = tmpdir("completed-generation");
    let store = Store::open(data_dir.clone()).unwrap();

    let mut first_trace = trace("trace-first", "session-generation");
    first_trace.req_body_path = Some(
        store
            .write_body("trace-first", "request.json", b"first generation")
            .unwrap(),
    );
    store.insert_trace(&first_trace).unwrap();

    let first = store
        .run_lar_legacy_import(&LarLegacyImportOptions::default())
        .unwrap();
    assert_eq!((first.migrated, first.failed), (1, 0));
    assert_eq!(first.job_state, "complete");

    let mut second_trace = trace("trace-second", "session-generation");
    second_trace.req_body_path = Some(
        store
            .write_body("trace-second", "request.json", b"second generation")
            .unwrap(),
    );
    store.insert_trace(&second_trace).unwrap();

    let second = store
        .run_lar_legacy_import(&LarLegacyImportOptions::default())
        .unwrap();
    assert_ne!(second.job_id, first.job_id);
    assert_eq!(second.file_uuid, first.file_uuid);
    assert_eq!(second.file_path, first.file_path);
    assert_eq!((second.migrated, second.failed), (1, 0));
    assert_eq!(
        second.skipped, 1,
        "the completed generation was re-imported"
    );
    assert_eq!(second.job_state, "complete");
    assert_eq!(
        store
            .read_lar_or_legacy_artifact("trace", "trace-second", "client_request", None)
            .unwrap()
            .unwrap(),
        b"second generation"
    );

    let repeated = store
        .run_lar_legacy_import(&LarLegacyImportOptions::default())
        .unwrap();
    assert_eq!(repeated.job_id, second.job_id);
    assert!(!repeated.claimed);
    assert_eq!((repeated.attempted, repeated.migrated), (0, 0));

    let catalog = rusqlite::Connection::open(data_dir.join("alexandria.sqlite3")).unwrap();
    let job_count: i64 = catalog
        .query_row("SELECT COUNT(*) FROM lar_migration_jobs", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(job_count, 2, "an empty rerun created another generation");
}
