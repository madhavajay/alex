use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use alex_core::TraceRecord;
use alex_lar::{ArchiveReader, ArchiveWriter, ChunkerConfig, FileHeader, Limits, StageKind};
use alex_store::{
    LarBodyStoreConfig, LarBodyStoreMode, LarExchangeBodyRefs, LarExchangeCapture, LarRepackConfig,
    LarStandaloneImportOptions, Store, ToolCallRecord,
};
use rusqlite::Connection;

static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn tmpdir(name: &str) -> PathBuf {
    let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "alex-lar-tool-timeline-{name}-{}-{sequence}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn config(max_pack_bytes: u64) -> LarBodyStoreConfig {
    LarBodyStoreConfig {
        mode: LarBodyStoreMode::LarWithFallback,
        max_pack_bytes,
        ..LarBodyStoreConfig::default()
    }
}

fn database(root: &Path) -> Connection {
    Connection::open(root.join("alexandria.sqlite3")).unwrap()
}

fn seed_trace(store: &Store, trace_id: &str, session_id: &str, wall_time_ms: i64) {
    store
        .insert_trace(&TraceRecord {
            id: trace_id.into(),
            session_id: Some(session_id.into()),
            harness: Some("pi".into()),
            ts_request_ms: wall_time_ms,
            status: Some(200),
            ..TraceRecord::default()
        })
        .unwrap();
    store
        .write_lar_exchange_capture(
            &LarExchangeCapture {
                trace_id: trace_id.into(),
                session_id: Some(session_id.into()),
                run_id: None,
                wall_time_ns: (wall_time_ms as u64) * 1_000_000,
                client_request_headers: None,
                client_request_trailers: None,
                client_response_headers: None,
                client_response_trailers: None,
                upstream_attempts: Vec::new(),
                upstream_stream_reads: None,
                provider: None,
                requested_model: None,
                routed_model: None,
                account_id: None,
                routing_reason: None,
                status_code: Some(200),
                error_class: None,
                error_message: None,
            },
            &LarExchangeBodyRefs::default(),
        )
        .unwrap()
        .unwrap();
}

fn tool(
    id: &str,
    call_id: &str,
    trace_id: Option<&str>,
    start: i64,
    end: Option<i64>,
    args: Option<String>,
    result: Option<String>,
) -> ToolCallRecord {
    ToolCallRecord {
        id: id.into(),
        harness: "pi".into(),
        session_id: "session-tools".into(),
        turn_id: Some("turn-1".into()),
        tool_call_id: call_id.into(),
        trace_id: trace_id.map(str::to_string),
        tool_name: "bash".into(),
        ts_start_ms: start,
        ts_end_ms: end,
        is_error: Some(false),
        exit_status: end.map(|_| 0),
        args_body_path: args,
        result_body_path: result,
    }
}

fn stage_kinds(store: &Store, trace_id: &str) -> Vec<String> {
    store
        .lar_stages_for_traces(&[trace_id.to_string()])
        .unwrap()
        .remove(trace_id)
        .unwrap_or_default()
        .into_iter()
        .map(|value| value["kind"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn live_phases_are_ordered_idempotent_and_reuse_one_body_identity() {
    let root = tmpdir("live");
    let store = Store::open_with_lar_body_store(root.clone(), config(16 * 1024 * 1024)).unwrap();
    seed_trace(&store, "trace-tools", "session-tools", 1_000);
    let shared = br#"{"command":"echo hi"}"#;
    let args = store
        .write_body("tool-1", "tool-args.json", shared)
        .unwrap();
    let result = store
        .write_body("tool-1", "tool-result.json", shared)
        .unwrap();

    // A lone/end-first event proves both phases even when it is the first
    // callback; retrying it cannot append a second occurrence. Its timestamps
    // deliberately predate the base exchange: the browser must keep the exact
    // base stage order first, then append canonical supplements.
    let end_first = tool(
        "tool-1",
        "call-1",
        Some("trace-tools"),
        900,
        Some(950),
        Some(args),
        Some(result),
    );
    store
        .upsert_live_tool_call_with_timeline(&end_first)
        .unwrap();
    store
        .upsert_live_tool_call_with_timeline(&end_first)
        .unwrap();
    assert_eq!(
        stage_kinds(&store, "trace-tools"),
        vec![
            "client_request",
            "router_decision",
            "client_response",
            "tool_call",
            "tool_result",
        ]
    );

    let conn = database(&root);
    let supplements: i64 = conn
        .query_row("SELECT COUNT(*) FROM lar_timeline_supplements", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(supplements, 2);
    let manifests: Vec<String> = conn
        .prepare(
            "SELECT manifest_id FROM lar_timeline_supplements
              WHERE tool_id='tool-1' ORDER BY phase",
        )
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(manifests.len(), 2);
    assert_eq!(manifests[0], manifests[1]);
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM lar_manifests", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM lar_chunks", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    drop(conn);

    // Restart performs a rescan/recovery but must remain occurrence-idempotent.
    drop(store);
    let reopened = Store::open_with_lar_body_store(root.clone(), config(16 * 1024 * 1024)).unwrap();
    assert_eq!(
        stage_kinds(&reopened, "trace-tools"),
        vec![
            "client_request",
            "router_decision",
            "client_response",
            "tool_call",
            "tool_result",
        ]
    );

    // A result with no captured arguments still gets a body-less call before
    // its result, preserving the semantic occurrence order.
    let result_only_body = reopened
        .write_body("tool-2", "tool-result.json", b"result only")
        .unwrap();
    reopened
        .upsert_live_tool_call_with_timeline(&tool(
            "tool-2",
            "call-2",
            Some("trace-tools"),
            1_300,
            Some(1_400),
            None,
            Some(result_only_body),
        ))
        .unwrap();
    assert_eq!(
        stage_kinds(&reopened, "trace-tools"),
        vec![
            "client_request",
            "router_decision",
            "client_response",
            "tool_call",
            "tool_result",
            "tool_call",
            "tool_result",
        ]
    );
}

#[test]
fn standalone_round_trip_and_catalog_rescan_restore_tool_identity() {
    let source_root = tmpdir("export-source");
    let source =
        Store::open_with_lar_body_store(source_root.clone(), config(16 * 1024 * 1024)).unwrap();
    seed_trace(&source, "trace-export", "session-tools", 2_000);
    source
        .upsert_live_tool_call_with_timeline(&tool(
            "tool-export",
            "call-export",
            Some("trace-export"),
            2_100,
            Some(2_200),
            None,
            None,
        ))
        .unwrap();
    let args = source
        .write_body("tool-export", "tool-args.json", br#"{"command":"pwd"}"#)
        .unwrap();
    let result = source
        .write_body("tool-export", "tool-result.json", b"/tmp")
        .unwrap();
    source
        .upsert_live_tool_call_with_timeline(&tool(
            "tool-export",
            "call-export",
            Some("trace-export"),
            2_100,
            Some(2_200),
            Some(args),
            Some(result),
        ))
        .unwrap();

    let archive_path = source_root.join("trace-export.lar");
    let file = OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .open(&archive_path)
        .unwrap();
    let mut writer = ArchiveWriter::create(
        file,
        FileHeader::standalone([91; 16], 2_000_000_000, b"tool-export-test".to_vec()),
        ChunkerConfig::default(),
        Limits::default(),
    )
    .unwrap();
    assert!(source
        .append_exact_trace_to_standalone(&mut writer, "trace-export")
        .unwrap());
    writer.seal().unwrap();
    writer.get_ref().sync_all().unwrap();
    drop(writer);

    let exported =
        ArchiveReader::open(File::open(&archive_path).unwrap(), Limits::default()).unwrap();
    assert_eq!(exported.exchange_lineage_by_trace(b"trace-export").len(), 5);
    assert_eq!(
        exported
            .stage_lineage_by_trace(b"trace-export")
            .iter()
            .filter(|stage| matches!(stage.data.kind, StageKind::ToolCall | StageKind::ToolResult))
            .count(),
        4
    );
    drop(exported);

    let imported_root = tmpdir("export-destination");
    let imported = Store::open(imported_root.clone()).unwrap();
    let report = imported
        .import_sealed_lar_archive(&archive_path, &LarStandaloneImportOptions::default())
        .unwrap();
    assert_eq!(
        stage_kinds(&imported, "trace-export"),
        vec![
            "client_request",
            "router_decision",
            "client_response",
            "tool_call",
            "tool_call",
            "tool_result",
            "tool_result",
        ]
    );
    let tools = imported.session_tool_calls("session-tools").unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["id"], "tool-export");
    assert_eq!(tools[0]["trace_id"], "trace-export");

    // Delete only the derived projections, detach, and reattach the same
    // immutable bytes. Provenance must rebuild both tables exactly.
    {
        let conn = database(&imported_root);
        conn.execute("DELETE FROM lar_timeline_supplements", [])
            .unwrap();
        conn.execute("DELETE FROM tool_calls", []).unwrap();
    }
    imported.detach_lar_archive(&report.file_uuid).unwrap();
    imported
        .reattach_lar_archive(
            &report.file_uuid,
            &archive_path,
            &alex_store::LarArchiveReattachOptions::default(),
        )
        .unwrap();
    assert_eq!(
        database(&imported_root)
            .query_row("SELECT COUNT(*) FROM lar_timeline_supplements", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        4
    );
    assert_eq!(
        imported.session_tool_calls("session-tools").unwrap()[0]["id"],
        "tool-export"
    );
}

#[test]
fn gc_repack_and_parent_deletion_keep_supplement_ownership_consistent() {
    let root = tmpdir("retention");
    let store = Store::open_with_lar_body_store(root.clone(), config(16 * 1024 * 1024)).unwrap();
    seed_trace(&store, "trace-retain", "session-tools", 3_000);
    let args = store
        .write_body("tool-retain", "tool-args.json", b"reachable arguments")
        .unwrap();
    let result = store
        .write_body("tool-retain", "tool-result.json", b"reachable result")
        .unwrap();
    store
        .upsert_live_tool_call_with_timeline(&tool(
            "tool-retain",
            "call-retain",
            Some("trace-retain"),
            3_100,
            Some(3_200),
            Some(args),
            Some(result),
        ))
        .unwrap();
    let garbage = store
        .write_body("garbage-owner", "request.json", b"unreachable repack bytes")
        .unwrap();
    let garbage_manifest: String = database(&root)
        .query_row(
            "SELECT manifest_id FROM lar_trace_artifacts
              WHERE owner_id='garbage-owner'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    database(&root)
        .execute(
            "DELETE FROM lar_trace_artifacts WHERE owner_id='garbage-owner'",
            [],
        )
        .unwrap();
    {
        let conn = database(&root);
        conn.execute(
            "DELETE FROM lar_manifest_chunks WHERE manifest_id=?1",
            [&garbage_manifest],
        )
        .unwrap();
        conn.execute(
            "DELETE FROM lar_manifests WHERE manifest_id=?1",
            [&garbage_manifest],
        )
        .unwrap();
    }
    assert!(!garbage.is_empty());
    assert!(store.plan_lar_gc().unwrap().reachable_manifests >= 2);
    let source_supplement_file: String = database(&root)
        .query_row(
            "SELECT file_uuid FROM lar_timeline_supplements LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();

    // Reopen with a tiny rotation threshold and force the source pack sealed,
    // then repack away the deliberately unreachable body.
    drop(store);
    let rotating = Store::open_with_lar_body_store(root.clone(), config(1)).unwrap();
    rotating
        .write_body("rotation", "request.json", b"rotate")
        .unwrap();
    let report = rotating
        .run_lar_repack(
            &LarRepackConfig {
                min_garbage_bytes: 0,
                min_garbage_ratio: 0.0,
            },
            4_000,
        )
        .unwrap()
        .expect("the unreferenced body makes a repack candidate");
    assert_eq!(report.state, "complete");
    assert_ne!(
        database(&root)
            .query_row(
                "SELECT file_uuid FROM lar_timeline_supplements LIMIT 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        source_supplement_file
    );
    assert_eq!(
        stage_kinds(&rotating, "trace-retain"),
        vec![
            "client_request",
            "router_decision",
            "client_response",
            "tool_call",
            "tool_result",
        ]
    );

    rotating.delete_trace("trace-retain").unwrap();
    drop(rotating);
    let reopened = Store::open_with_lar_body_store(root.clone(), config(1)).unwrap();
    assert!(stage_kinds(&reopened, "trace-retain").is_empty());
    let conn = database(&root);
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM lar_timeline_supplements", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap(),
        0
    );
    assert_eq!(
        conn.query_row(
            "SELECT canonical_timeline FROM tool_calls WHERE id='tool-retain'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        0
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM lar_timeline_supplement_tombstones",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        2
    );
}

#[test]
fn ordinary_startup_does_not_scan_sealed_historical_archives() {
    let root = tmpdir("startup-bounded");
    let store = Store::open_with_lar_body_store(root.clone(), config(1)).unwrap();
    seed_trace(&store, "trace-sealed", "session-tools", 5_000);
    store
        .write_body("new-active", "request.json", b"force rotation")
        .unwrap();
    let sealed_path = PathBuf::from(
        database(&root)
            .query_row(
                "SELECT path FROM lar_files WHERE state='sealed'
              ORDER BY created_at_ms LIMIT 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
    );
    let hidden_path = sealed_path.with_extension("lar.hidden-for-startup-test");
    drop(store);
    std::fs::rename(&sealed_path, &hidden_path).unwrap();

    // If startup performed a full-corpus supplement rescan this would fail on
    // the deliberately unavailable sealed file. Active-pack recovery remains
    // bounded and the catalog opens normally.
    let reopened = Store::open_with_lar_body_store(root.clone(), config(1)).unwrap();
    drop(reopened);
    std::fs::rename(hidden_path, sealed_path).unwrap();
}

#[test]
fn full_retention_prune_does_not_resurrect_archived_supplements() {
    let root = tmpdir("prune-tombstone");
    let store = Store::open_with_lar_body_store(root.clone(), config(16 * 1024 * 1024)).unwrap();
    seed_trace(&store, "trace-pruned", "session-tools", 6_000);
    let result = store
        .write_body("tool-pruned", "tool-result.json", b"old result")
        .unwrap();
    store
        .upsert_live_tool_call_with_timeline(&tool(
            "tool-pruned",
            "call-pruned",
            Some("trace-pruned"),
            6_100,
            Some(6_200),
            None,
            Some(result),
        ))
        .unwrap();
    assert_eq!(
        database(&root)
            .query_row("SELECT COUNT(*) FROM lar_timeline_supplements", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        2
    );
    store.prune(7_000, false, false).unwrap();
    drop(store);

    let reopened = Store::open_with_lar_body_store(root.clone(), config(16 * 1024 * 1024)).unwrap();
    drop(reopened);
    let conn = database(&root);
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM lar_timeline_supplements", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap(),
        0
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM lar_timeline_supplement_tombstones",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        2
    );
}

#[test]
fn bodies_only_prune_removes_tool_bytes_from_catalog_and_standalone_export() {
    let root = tmpdir("bodies-only");
    let store = Store::open_with_lar_body_store(root.clone(), config(16 * 1024 * 1024)).unwrap();
    seed_trace(&store, "trace-body-pruned", "session-tools", 7_000);
    let args = store
        .write_body("tool-body-pruned", "tool-args.json", b"secret arguments")
        .unwrap();
    let result = store
        .write_body("tool-body-pruned", "tool-result.json", b"secret result")
        .unwrap();
    store
        .upsert_live_tool_call_with_timeline(&tool(
            "tool-body-pruned",
            "call-body-pruned",
            Some("trace-body-pruned"),
            7_100,
            Some(7_200),
            Some(args),
            Some(result),
        ))
        .unwrap();
    store.prune(8_000, true, false).unwrap();
    let interchange = store
        .lar_interchange_trace("trace-body-pruned")
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
    assert!(store.plan_lar_gc().unwrap().unreachable_manifests >= 2);
    store.run_lar_gc(8_001).unwrap();

    let path = root.join("body-pruned-export.lar");
    let file = OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    let mut writer = ArchiveWriter::create(
        file,
        FileHeader::standalone([92; 16], 8_000_000_000, b"pruned-export".to_vec()),
        ChunkerConfig::default(),
        Limits::default(),
    )
    .unwrap();
    assert!(store
        .append_exact_trace_to_standalone(&mut writer, "trace-body-pruned")
        .unwrap());
    writer.seal().unwrap();
    drop(writer);
    let exported = ArchiveReader::open(File::open(path).unwrap(), Limits::default()).unwrap();
    assert_eq!(exported.manifest_ids().count(), 0);
    assert_eq!(exported.stream_index_count(), 0);
}
