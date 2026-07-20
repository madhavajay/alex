use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use alex_core::TraceRecord;
use alex_lar::{ArchiveReader, HeaderFidelity, Limits, OpenPath, StageKind};
use alex_store::{
    LarArtifactLocation, LarArtifactReadRequest, LarBodyArtifact, LarBodyStoreConfig,
    LarBodyStoreMode, LarLegacyImportBoundary, LarLegacyImportHook, LarLegacyImportOptions,
    LarLegacyResourceControls, Store, ToolCallRecord, LAR_HEADER_FLAG_REDACTED,
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
        (8, 8, 0)
    );
    assert_eq!(report.progress_percent, 100.0);
    assert_eq!(report.eta_seconds, None);
    assert_eq!(report.yield_count, 8);
    assert_eq!(report.last_error, None);
}

#[test]
fn migration_renews_its_lease_during_a_long_batch_boundary() {
    let store = Store::open(tmpdir("lease-heartbeat")).unwrap();
    let mut row = trace("trace-lease-heartbeat", "lease-heartbeat-session");
    row.req_body_path = Some(
        store
            .write_body(
                "trace-lease-heartbeat",
                "request.json",
                b"body whose migration remains exclusively leased",
            )
            .unwrap(),
    );
    store.insert_trace(&row).unwrap();
    let hook = LarLegacyImportHook::new(|boundary| {
        if boundary == LarLegacyImportBoundary::BodyAppended {
            std::thread::sleep(Duration::from_millis(180));
        }
        Ok(())
    });

    let report = store
        .run_lar_legacy_import(&LarLegacyImportOptions {
            batch_size: 1,
            lease_owner: "lease-heartbeat-worker".into(),
            lease_duration: Duration::from_millis(60),
            boundary_hook: Some(hook),
            ..LarLegacyImportOptions::default()
        })
        .unwrap();

    assert_eq!((report.migrated, report.failed), (1, 0));
    assert_eq!(report.job_state, "complete");
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
    assert_eq!(report.pack_sequence, 5);
    assert_eq!(report.packs_rotated, 5);

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
    assert_eq!(packs.len(), 6);
    assert_eq!(
        packs.iter().filter(|(_, state)| state == "sealed").count(),
        5
    );
    assert_eq!(
        packs.iter().filter(|(_, state)| state == "active").count(),
        1
    );
    for (index, (path, _)) in packs.into_iter().enumerate() {
        let reader =
            ArchiveReader::open(std::fs::File::open(path).unwrap(), Limits::default()).unwrap();
        assert_eq!(reader.manifest_count(), usize::from(index < 5));
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
    assert_eq!((resumed.migrated, resumed.pack_sequence), (1, 3));
    assert_eq!(resumed.packs_rotated, 2);
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
    assert_eq!(pack_count, 4);
    let sealed_count: i64 = catalog
        .query_row(
            "SELECT COUNT(*) FROM lar_files
             WHERE archive_set_uuid=?1 AND role='body-pack' AND state='sealed'",
            [&resumed.archive_set_uuid],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(sealed_count, 3);
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
    assert_eq!((report.migrated, report.pack_sequence), (2, 2));
    assert_eq!(report.packs_rotated, 2);
    let catalog = rusqlite::Connection::open(data_dir.join("alexandria.sqlite3")).unwrap();
    let pack_count: i64 = catalog
        .query_row(
            "SELECT COUNT(*) FROM lar_files WHERE archive_set_uuid=?1 AND role='body-pack'",
            [&report.archive_set_uuid],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(pack_count, 3);
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

#[test]
fn metadata_import_preserves_proven_headers_routes_tool_order_and_shared_bodies() {
    let data_dir = tmpdir("metadata-fidelity");
    let store = Store::open(data_dir.clone()).unwrap();
    let request = br#"{"messages":[{"role":"user","content":"hello"}]}"#;
    let request_path = store
        .write_body("trace-metadata", "request.json", request)
        .unwrap();
    let upstream_path = store
        .write_body("trace-metadata", "upstream-request.json", request)
        .unwrap();
    let response_path = store
        .write_body("trace-metadata", "response.body", br#"{"ok":true}"#)
        .unwrap();
    let mut row = trace("trace-metadata", "session-metadata");
    row.ts_request_ms = 100;
    row.ts_response_ms = Some(400);
    row.run_id = Some("run-metadata".into());
    row.harness = Some("pi".into());
    row.client_format = Some("openai-responses".into());
    row.upstream_format = Some("openai-chat".into());
    row.method = Some("POST".into());
    row.path = Some("/v1/responses".into());
    row.streamed = Some(true);
    row.req_body_path = Some(request_path.clone());
    row.upstream_req_body_path = Some(upstream_path.clone());
    row.resp_body_path = Some(response_path.clone());
    row.req_headers_json =
        Some(r#"[["X-Dupe","one"],["x-dupe","two"],["Authorization","secret"]]"#.into());
    row.resp_headers_json = Some(r#"{"Set-Cookie":["a=1","b=2"],"X-Response":"yes"}"#.into());
    row.upstream_provider = Some("openai".into());
    row.requested_model = Some("requested".into());
    row.routed_model = Some("served".into());
    row.status = Some(200);
    row.usage.input_tokens = Some(120);
    row.usage.cached_input_tokens = Some(80);
    row.usage.cache_creation_tokens = Some(12);
    row.usage.output_tokens = Some(30);
    row.usage.reasoning_tokens = Some(9);
    row.cost_usd = Some(0.000_123_456);
    row.billing_bucket = Some("standard".into());
    row.error_kind = Some("provider_error_type".into());
    row.error_code = Some("provider_error_code".into());
    row.subscription_identity = Some("subscription-stable".into());
    row.tags = Some(r#"{"feature":"lar"}"#.into());
    row.client_ip = Some("127.0.0.1".into());
    row.key_fingerprint = Some("key-fingerprint".into());
    row.reasoning_effort = Some("high".into());
    row.thinking_budget = Some(4096);
    row.substituted = true;
    row.original_model = Some("requested".into());
    row.served_model = Some("served".into());
    row.substitution_reason = Some("capacity".into());
    row.original_account_id = Some("account-a".into());
    row.served_account_id = Some("account-b".into());
    row.attempts = Some(
        r#"[{"account_id":"account-a","model":"requested","rung":"primary"},{"account_id":"account-b","model":"served","retry":1,"legacy_extra":"kept"}]"#.into(),
    );
    store.insert_trace(&row).unwrap();

    let args_path = store
        .write_body("tool-linked", "tool-args.json", br#"{"cmd":"pwd"}"#)
        .unwrap();
    let result_path = store
        .write_body("tool-linked", "tool-result.json", b"/tmp\n")
        .unwrap();
    store
        .upsert_tool_call(&ToolCallRecord {
            id: "tool-linked".into(),
            harness: "pi".into(),
            session_id: "session-metadata".into(),
            turn_id: Some("turn-linked".into()),
            tool_call_id: "call-linked".into(),
            trace_id: Some("trace-metadata".into()),
            tool_name: "bash".into(),
            ts_start_ms: 250,
            ts_end_ms: Some(300),
            is_error: Some(false),
            exit_status: Some(7),
            args_body_path: Some(args_path.clone()),
            result_body_path: Some(result_path.clone()),
        })
        .unwrap();

    let unlinked_result_path = store
        .write_body("tool-unlinked", "tool-result.json", b"orphan result")
        .unwrap();
    store
        .upsert_tool_call(&ToolCallRecord {
            id: "tool-unlinked".into(),
            harness: "pi".into(),
            session_id: "session-metadata".into(),
            turn_id: None,
            tool_call_id: "call-unlinked".into(),
            trace_id: Some("missing-trace".into()),
            tool_name: "read".into(),
            ts_start_ms: 500,
            ts_end_ms: Some(510),
            is_error: Some(true),
            exit_status: Some(1),
            args_body_path: None,
            result_body_path: Some(unlinked_result_path.clone()),
        })
        .unwrap();

    let first = store
        .run_lar_legacy_import(&LarLegacyImportOptions::default())
        .unwrap();
    assert_eq!((first.metadata_migrated, first.metadata_failed), (2, 0));
    assert!(first.metadata_unsupported >= 2);
    for path in [
        request_path,
        upstream_path,
        response_path,
        args_path,
        result_path,
        unlinked_result_path,
    ] {
        assert!(Path::new(&path).exists(), "legacy source changed: {path}");
    }

    let reader = ArchiveReader::open(
        std::fs::File::open(&first.file_path).unwrap(),
        Limits::default(),
    )
    .unwrap();
    let exchange = reader.exchange_by_trace(b"trace-metadata").unwrap();
    let companion = &reader.exchange_metadata(&exchange.id).unwrap().data;
    assert_eq!(companion.ts_request_ms, Some(100));
    assert_eq!(companion.ts_response_ms, Some(400));
    assert_eq!(companion.harness.as_deref(), Some(b"pi".as_slice()));
    assert_eq!(
        companion.client_format.as_deref(),
        Some(b"openai-responses".as_slice())
    );
    assert_eq!(
        companion.upstream_format.as_deref(),
        Some(b"openai-chat".as_slice())
    );
    assert_eq!(companion.method.as_deref(), Some(b"POST".as_slice()));
    assert_eq!(companion.path.as_deref(), Some(b"/v1/responses".as_slice()));
    assert_eq!(companion.streamed, Some(true));
    assert_eq!(companion.status, Some(200));
    assert_eq!(companion.cost_usd_bits, Some(0.000_123_456f64.to_bits()));
    assert_eq!(
        companion.billing_bucket.as_deref(),
        Some(b"standard".as_slice())
    );
    assert_eq!(
        companion.error_kind.as_deref(),
        Some(b"provider_error_type".as_slice())
    );
    assert_eq!(
        companion.error_code.as_deref(),
        Some(b"provider_error_code".as_slice())
    );
    assert_eq!(
        companion.subscription_identity.as_deref(),
        Some(b"subscription-stable".as_slice())
    );
    assert_eq!(
        companion.tags_json.as_deref(),
        Some(br#"{"feature":"lar"}"#.as_slice())
    );
    assert_eq!(
        companion.client_ip.as_deref(),
        Some(b"127.0.0.1".as_slice())
    );
    assert_eq!(
        companion.key_fingerprint.as_deref(),
        Some(b"key-fingerprint".as_slice())
    );
    assert_eq!(
        companion.reasoning_effort.as_deref(),
        Some(b"high".as_slice())
    );
    assert_eq!(companion.thinking_budget, Some(4096));
    assert_eq!(companion.input_tokens, Some(120));
    assert_eq!(companion.cached_input_tokens, Some(80));
    assert_eq!(companion.cache_creation_tokens, Some(12));
    assert_eq!(companion.output_tokens, Some(30));
    assert_eq!(companion.reasoning_tokens, Some(9));
    let stages = exchange
        .data
        .stages
        .iter()
        .map(|id| reader.stage(id).unwrap())
        .collect::<Vec<_>>();
    let kinds = stages
        .iter()
        .map(|stage| stage.data.kind)
        .collect::<Vec<_>>();
    let tool_call = kinds
        .iter()
        .position(|kind| *kind == StageKind::ToolCall)
        .unwrap();
    let tool_result = kinds
        .iter()
        .position(|kind| *kind == StageKind::ToolResult)
        .unwrap();
    assert_eq!(
        kinds
            .iter()
            .filter(|kind| **kind == StageKind::ToolCall)
            .count(),
        1
    );
    assert_eq!(
        kinds
            .iter()
            .filter(|kind| **kind == StageKind::ToolResult)
            .count(),
        1
    );
    let client_response = kinds
        .iter()
        .position(|kind| *kind == StageKind::ClientResponse)
        .unwrap();
    assert!(tool_call < tool_result && tool_result < client_response);
    let linked_provenance =
        std::str::from_utf8(stages[tool_call].data.routing_reason.as_deref().unwrap()).unwrap();
    assert!(linked_provenance.starts_with("legacy_tool_metadata_json:"));
    assert!(linked_provenance.contains(r#""harness":"pi""#));
    assert!(linked_provenance.contains(r#""turn_id":"turn-linked""#));
    assert!(linked_provenance.contains(r#""legacy_trace_id":"trace-metadata""#));
    let client_request = stages
        .iter()
        .find(|stage| stage.data.kind == StageKind::ClientRequest)
        .unwrap();
    let upstream_request = stages
        .iter()
        .find(|stage| stage.data.kind == StageKind::UpstreamRequest)
        .unwrap();
    assert_eq!(
        client_request.data.request_body_manifest_ref,
        upstream_request.data.request_body_manifest_ref,
        "unchanged upstream request must reference the one stored body manifest"
    );
    let request_headers = reader
        .header_block(&client_request.data.request_headers_ref.unwrap())
        .unwrap();
    assert_eq!(
        request_headers.fidelity,
        HeaderFidelity::LegacyCasingUnknown
    );
    assert_eq!(request_headers.atoms[0].original_name, b"X-Dupe");
    assert_eq!(request_headers.atoms[0].value, b"one");
    assert_eq!(request_headers.atoms[1].original_name, b"x-dupe");
    assert_eq!(request_headers.atoms[1].value, b"two");
    assert_eq!(request_headers.atoms[2].value, b"<redacted>");
    assert_ne!(request_headers.atoms[2].flags & LAR_HEADER_FLAG_REDACTED, 0);
    let attempts = stages
        .iter()
        .filter(|stage| {
            matches!(
                stage.data.kind,
                StageKind::AccountRouting | StageKind::RetryDecision | StageKind::FailoverDecision
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0].data.attempt_number, Some(1));
    assert_eq!(attempts[1].data.attempt_number, Some(2));
    assert_eq!(
        attempts[1].data.account_id.as_deref(),
        Some(b"account-b".as_slice())
    );
    assert_eq!(
        attempts[1].data.error_class.as_deref(),
        Some(b"legacy_opaque_metadata".as_slice())
    );
    assert_eq!(
        stages[tool_result].data.routing_reason.as_deref(),
        Some(b"legacy_tool_exit_status:7".as_slice())
    );
    assert_eq!(stages[tool_result].data.status_code, None);
    let response = stages
        .iter()
        .find(|stage| stage.data.kind == StageKind::UpstreamResponse)
        .unwrap();
    let usage = response.data.usage.as_ref().unwrap();
    assert_eq!(usage.input_tokens, 120);
    assert_eq!(usage.cached_tokens, 80);
    assert_eq!(usage.output_tokens, 30);
    assert_eq!(usage.reasoning_tokens, 9);
    assert_eq!(response.data.cost_nanos, Some(123_456));
    assert_eq!(
        response.data.cost_currency.as_deref(),
        Some(b"USD".as_slice())
    );

    let unlinked = reader
        .exchange_by_trace(b"legacy-tool:tool-unlinked")
        .unwrap();
    let unlinked_stages = unlinked
        .data
        .stages
        .iter()
        .map(|id| reader.stage(id).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(unlinked_stages.len(), 2);
    assert_eq!(unlinked_stages[0].data.kind, StageKind::ToolCall);
    assert_eq!(unlinked_stages[1].data.kind, StageKind::ToolResult);
    let unlinked_provenance =
        std::str::from_utf8(unlinked_stages[0].data.routing_reason.as_deref().unwrap()).unwrap();
    assert!(unlinked_provenance.contains(r#""harness":"pi""#));
    assert!(unlinked_provenance.contains(r#""turn_id":null"#));
    assert!(unlinked_provenance.contains(r#""legacy_trace_id":"missing-trace""#));
    assert_eq!(
        unlinked_stages[1].data.error_class.as_deref(),
        Some(b"tool_error".as_slice())
    );

    let catalog = rusqlite::Connection::open(data_dir.join("alexandria.sqlite3")).unwrap();
    let fidelity_detail: String = catalog
        .query_row(
            "SELECT h.fidelity_detail FROM lar_header_blocks h
             JOIN lar_stage_records s ON s.request_headers_ref=h.block_id
             WHERE s.trace_id='trace-metadata' AND s.kind='client_request'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(fidelity_detail, "legacy_casing_unknown");
    let before_counts: (i64, i64, i64) = catalog
        .query_row(
            "SELECT (SELECT COUNT(*) FROM lar_header_blocks),
                    (SELECT COUNT(*) FROM lar_stage_records),
                    (SELECT COUNT(*) FROM lar_migration_items
                       WHERE artifact_kind='exchange_metadata')",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    drop(catalog);
    let repeated = store
        .run_lar_legacy_import(&LarLegacyImportOptions::default())
        .unwrap();
    assert!(!repeated.claimed);
    let catalog = rusqlite::Connection::open(data_dir.join("alexandria.sqlite3")).unwrap();
    let after_counts: (i64, i64, i64) = catalog
        .query_row(
            "SELECT (SELECT COUNT(*) FROM lar_header_blocks),
                    (SELECT COUNT(*) FROM lar_stage_records),
                    (SELECT COUNT(*) FROM lar_migration_items
                       WHERE artifact_kind='exchange_metadata')",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(after_counts, before_counts);
    assert_eq!(
        catalog
            .query_row("SELECT COUNT(*) FROM lar_timeline_supplements", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        0,
        "legacy metadata stages must not also create live supplements"
    );
    drop(catalog);
    drop(store);
    let reopened = Store::open_with_lar_body_store(
        data_dir.clone(),
        LarBodyStoreConfig {
            mode: LarBodyStoreMode::LarWithFallback,
            ..LarBodyStoreConfig::default()
        },
    )
    .unwrap();
    drop(reopened);
    assert_eq!(
        rusqlite::Connection::open(data_dir.join("alexandria.sqlite3"))
            .unwrap()
            .query_row("SELECT COUNT(*) FROM lar_timeline_supplements", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        0,
        "startup recovery must ignore projection-only imported tool rows"
    );
}

#[test]
fn v2_metadata_upgrade_reuses_v1_pointer_after_legacy_gzip_cleanup() {
    let data_dir = tmpdir("v1-cleaned-upgrade");
    let store = Store::open(data_dir.clone()).unwrap();
    let legacy_path = store
        .write_body("trace-v1", "request.json", b"body retained only in LAR")
        .unwrap();
    let mut row = trace("trace-v1", "session-v1");
    row.req_body_path = Some(legacy_path.clone());
    row.req_headers_json = Some(r#"{"X-Legacy":"yes"}"#.into());
    store.insert_trace(&row).unwrap();

    // Stop after the one body item, matching a completed body-only v1
    // installation before exchange metadata existed.
    let body_only = store
        .run_lar_legacy_import(&LarLegacyImportOptions {
            limit: Some(1),
            ..LarLegacyImportOptions::default()
        })
        .unwrap();
    assert_eq!((body_only.migrated, body_only.metadata_migrated), (1, 0));
    let source_key = data_dir
        .join("alexandria.sqlite3")
        .to_string_lossy()
        .into_owned();
    let mut old_hasher = blake3::Hasher::new();
    old_hasher.update(b"alex-lar-legacy-job-v1");
    old_hasher.update(source_key.as_bytes());
    let old_job_id = format!(
        "legacy-{}",
        old_hasher.finalize().as_bytes()[..16]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
    let catalog = rusqlite::Connection::open(data_dir.join("alexandria.sqlite3")).unwrap();
    catalog
        .execute_batch("BEGIN IMMEDIATE; PRAGMA defer_foreign_keys=ON;")
        .unwrap();
    catalog
        .execute(
            "UPDATE lar_migration_jobs
                SET job_id=?2, source_version='legacy-gzip-v1', state='complete',
                    pending_count=0, failed_count=0, completed_at_ms=updated_at_ms,
                    lease_owner=NULL, lease_expires_at_ms=NULL
              WHERE job_id=?1",
            rusqlite::params![body_only.job_id, old_job_id],
        )
        .unwrap();
    catalog
        .execute(
            "UPDATE lar_migration_items SET job_id=?2 WHERE job_id=?1",
            rusqlite::params![body_only.job_id, old_job_id],
        )
        .unwrap();
    catalog.execute_batch("COMMIT;").unwrap();
    let before: (i64, i64) = catalog
        .query_row(
            "SELECT (SELECT COUNT(*) FROM lar_manifests),
                    (SELECT COUNT(*) FROM lar_chunks)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    drop(catalog);
    std::fs::remove_file(&legacy_path).unwrap();

    let upgraded = store
        .run_lar_legacy_import(&LarLegacyImportOptions::default())
        .unwrap();
    assert_ne!(upgraded.job_id, old_job_id);
    assert_eq!((upgraded.attempted, upgraded.metadata_migrated), (0, 1));
    assert_eq!(upgraded.job_state, "complete");
    assert_eq!(
        store
            .read_lar_or_legacy_artifact("trace", "trace-v1", "client_request", None)
            .unwrap()
            .unwrap(),
        b"body retained only in LAR"
    );
    let catalog = rusqlite::Connection::open(data_dir.join("alexandria.sqlite3")).unwrap();
    let after: (i64, i64) = catalog
        .query_row(
            "SELECT (SELECT COUNT(*) FROM lar_manifests),
                    (SELECT COUNT(*) FROM lar_chunks)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(after, before, "metadata upgrade recopied body bytes");
}
