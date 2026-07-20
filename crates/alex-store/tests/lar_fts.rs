use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use alex_core::TraceRecord;
use alex_store::{
    LarBodyArtifact, LarBodyStoreConfig, LarBodyStoreMode, LarExchangeBodyRefs, LarExchangeCapture,
    LarFtsRebuildOptions, Store, ToolCallRecord, TraceFilter,
};
use rusqlite::Connection;

static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn tmpdir(name: &str) -> PathBuf {
    let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "alex-lar-fts-{name}-{}-{sequence}",
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
        ..Default::default()
    }
}

fn trace(id: &str, session_id: &str, timestamp: i64, body_path: String) -> TraceRecord {
    TraceRecord {
        id: id.into(),
        session_id: Some(session_id.into()),
        ts_request_ms: timestamp,
        req_body_path: Some(body_path),
        status: Some(200),
        ..Default::default()
    }
}

fn text_filter(text: &str) -> TraceFilter {
    TraceFilter {
        text: Some(text.into()),
        ..Default::default()
    }
}

fn minimal_capture(trace_id: &str, session_id: &str) -> LarExchangeCapture {
    LarExchangeCapture {
        trace_id: trace_id.into(),
        session_id: Some(session_id.into()),
        run_id: None,
        wall_time_ns: 1_000_000,
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
    }
}

#[test]
fn search_resolves_active_and_sealed_lar_bodies_with_trace_and_stage_anchors() {
    let root = tmpdir("mixed-packs");
    // Every subsequent write rotates the previous non-empty pack, producing a
    // deterministic sealed + active mix without large fixture bodies.
    let store = Store::open_with_lar_body_store(root.clone(), config(1)).unwrap();

    let sealed = store
        .write_body_artifact(
            &LarBodyArtifact::trace("trace-sealed", "client_request"),
            "request.json",
            br#"{"messages":[{"role":"user","content":"sealed celestial widget"}]}"#,
        )
        .unwrap();
    store
        .insert_trace(&trace(
            "trace-sealed",
            "session-sealed",
            1_000,
            sealed.legacy_path.clone(),
        ))
        .unwrap();

    let active = store
        .write_body_artifact(
            &LarBodyArtifact::trace("trace-active", "client_response"),
            "response.body",
            br#"{"choices":[{"message":{"role":"assistant","content":"active ocean widget"}}]}"#,
        )
        .unwrap();
    store
        .insert_trace(&trace(
            "trace-active",
            "session-active",
            2_000,
            active.legacy_path.clone(),
        ))
        .unwrap();

    // Force reads/search to rely on LAR rather than the compatibility gzip.
    std::fs::remove_file(&sealed.legacy_path).unwrap();
    std::fs::remove_file(&active.legacy_path).unwrap();

    let sealed_rows = store.search_traces(&text_filter("celestial")).unwrap();
    assert_eq!(sealed_rows.len(), 1);
    assert_eq!(sealed_rows[0]["id"], "trace-sealed");
    assert_eq!(sealed_rows[0]["session_id"], "session-sealed");
    assert_eq!(sealed_rows[0]["ts_request_ms"], 1_000);

    let active_rows = store.search_traces(&text_filter("ocean")).unwrap();
    assert_eq!(active_rows.len(), 1);
    assert_eq!(active_rows[0]["id"], "trace-active");
    assert_eq!(active_rows[0]["session_id"], "session-active");
    assert_eq!(active_rows[0]["ts_request_ms"], 2_000);
    let coverage = store
        .lar_normalized_indexed_artifacts(&["trace-sealed".into(), "trace-active".into()])
        .unwrap();
    assert!(coverage.contains(&("trace-sealed".into(), "client_request".into())));
    assert!(coverage.contains(&("trace-active".into(), "client_response".into())));

    let sealed_manifest = sealed.manifest_id.unwrap();
    store
        .write_lar_exchange_capture(
            &minimal_capture("trace-sealed", "session-sealed"),
            &LarExchangeBodyRefs {
                client_request_manifest_id: Some(sealed_manifest.clone()),
                ..Default::default()
            },
        )
        .unwrap();

    let conn = Connection::open(root.join("alexandria.sqlite3")).unwrap();
    let (sealed_files, active_files): (i64, i64) = conn
        .query_row(
            "SELECT
               SUM(CASE WHEN state='sealed' THEN 1 ELSE 0 END),
               SUM(CASE WHEN state='active' THEN 1 ELSE 0 END)
             FROM lar_files",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert!(sealed_files >= 1);
    assert!(active_files >= 1);
    let staged_refs: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM lar_normalized_entry_refs
             WHERE trace_id='trace-sealed' AND stage_id<>'' AND manifest_id=?1",
            [&sealed_manifest],
            |row| row.get(0),
        )
        .unwrap();
    assert!(staged_refs >= 1);
}

#[test]
fn index_is_deduplicated_rebuildable_and_cleans_reverse_references() {
    let root = tmpdir("rebuild-delete");
    let store = Store::open_with_lar_body_store(root.clone(), config(1024 * 1024)).unwrap();
    let body = br#"{"messages":[{"role":"user","content":"shared aurora phrase"}]}"#;

    for (id, session, timestamp) in [
        ("trace-one", "session-one", 1_000),
        ("trace-two", "session-two", 2_000),
    ] {
        let written = store
            .write_body_artifact(
                &LarBodyArtifact::trace(id, "client_request"),
                "request.json",
                body,
            )
            .unwrap();
        store
            .insert_trace(&trace(id, session, timestamp, written.legacy_path.clone()))
            .unwrap();
        std::fs::remove_file(written.legacy_path).unwrap();
    }

    let raw_before = store
        .read_lar_or_legacy_artifact("trace", "trace-one", "client_request", None)
        .unwrap()
        .unwrap();
    assert_eq!(raw_before, body);
    assert_eq!(
        store.search_traces(&text_filter("aurora")).unwrap().len(),
        2
    );

    let conn = Connection::open(root.join("alexandria.sqlite3")).unwrap();
    let (entries, references): (i64, i64) = conn
        .query_row(
            "SELECT
               (SELECT COUNT(*) FROM lar_normalized_entries),
               (SELECT COUNT(*) FROM lar_normalized_entry_refs)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(entries, 1);
    assert_eq!(references, 2);
    drop(conn);

    store.clear_lar_normalized_index().unwrap();
    assert!(store
        .search_traces(&text_filter("aurora"))
        .unwrap()
        .is_empty());
    let bounded_report = store
        .rebuild_lar_normalized_index(&LarFtsRebuildOptions {
            max_artifacts: 1,
            ..Default::default()
        })
        .unwrap();
    assert!(bounded_report.limit_reached);
    assert_eq!(bounded_report.artifacts_seen, 1);
    let conn = Connection::open(root.join("alexandria.sqlite3")).unwrap();
    let state: String = conn
        .query_row(
            "SELECT state FROM lar_normalized_index_meta WHERE singleton=1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(state, "needs_rebuild");
    drop(conn);
    let report = store
        .rebuild_lar_normalized_index(&LarFtsRebuildOptions::default())
        .unwrap();
    assert_eq!(report.artifacts_indexed, 2);
    assert_eq!(report.entries, 1);
    assert_eq!(report.reverse_references, 2);
    assert_eq!(
        store.search_traces(&text_filter("aurora")).unwrap().len(),
        2
    );
    assert_eq!(
        store
            .read_lar_or_legacy_artifact("trace", "trace-one", "client_request", None)
            .unwrap()
            .unwrap(),
        raw_before
    );
    let conn = Connection::open(root.join("alexandria.sqlite3")).unwrap();
    let state: String = conn
        .query_row(
            "SELECT state FROM lar_normalized_index_meta WHERE singleton=1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(state, "ready");
    drop(conn);

    store.delete_trace("trace-one").unwrap();
    let rows = store.search_traces(&text_filter("aurora")).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["id"], "trace-two");
    store.delete_trace("trace-two").unwrap();

    let conn = Connection::open(root.join("alexandria.sqlite3")).unwrap();
    let (entries, references): (i64, i64) = conn
        .query_row(
            "SELECT
               (SELECT COUNT(*) FROM lar_normalized_entries),
               (SELECT COUNT(*) FROM lar_normalized_entry_refs)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!((entries, references), (0, 0));
}

#[test]
fn tool_results_gain_trace_session_and_timestamp_reverse_anchors() {
    let root = tmpdir("tool-anchor");
    let store = Store::open_with_lar_body_store(root.clone(), config(1024 * 1024)).unwrap();
    store
        .insert_trace(&TraceRecord {
            id: "trace-tool".into(),
            session_id: Some("session-tool".into()),
            ts_request_ms: 4_200,
            ..Default::default()
        })
        .unwrap();

    // Pi can report body bytes before the tool lifecycle row arrives. The
    // subsequent upsert must enrich the existing reverse reference.
    let written = store
        .write_body_artifact(
            &LarBodyArtifact::tool_call("tool-one", "tool_result"),
            "tool-result.json",
            br#"{"result":"nebula tool output","authorization":"never searchable"}"#,
        )
        .unwrap();
    std::fs::remove_file(&written.legacy_path).unwrap();
    store
        .upsert_tool_call(&ToolCallRecord {
            id: "tool-one".into(),
            harness: "pi".into(),
            session_id: "session-tool".into(),
            turn_id: Some("turn-one".into()),
            tool_call_id: "call-one".into(),
            trace_id: Some("trace-tool".into()),
            tool_name: "shell".into(),
            ts_start_ms: 4_250,
            ts_end_ms: Some(4_300),
            is_error: Some(false),
            exit_status: Some(0),
            args_body_path: None,
            result_body_path: None,
        })
        .unwrap();

    let rows = store.search_traces(&text_filter("nebula")).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["id"], "trace-tool");
    assert!(store
        .search_traces(&text_filter("searchable"))
        .unwrap()
        .is_empty());

    let conn = Connection::open(root.join("alexandria.sqlite3")).unwrap();
    let anchor: (String, String, i64, String) = conn
        .query_row(
            "SELECT trace_id, session_id, ts_request_ms, manifest_id
             FROM lar_normalized_entry_refs
             WHERE owner_kind='tool_call' AND owner_id='tool-one'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(anchor.0, "trace-tool");
    assert_eq!(anchor.1, "session-tool");
    assert_eq!(anchor.2, 4_200);
    assert_eq!(Some(anchor.3), written.manifest_id);
}

#[test]
fn selected_redacted_headers_are_indexed_and_rebuilt_without_secrets() {
    let root = tmpdir("selected-headers");
    let store = Store::open_with_lar_body_store(root.clone(), config(1024 * 1024)).unwrap();
    store
        .insert_trace(&TraceRecord {
            id: "trace-headers".into(),
            session_id: Some("session-headers".into()),
            ts_request_ms: 9_001,
            req_headers_json: Some(
                r#"{"content-type":"application/safeheaderneedle","authorization":"Bearer leakedheaderneedle","x-private-note":"privateheaderneedle"}"#.into(),
            ),
            resp_headers_json: Some(r#"[["x-request-id","requestheaderneedle"]]"#.into()),
            ..Default::default()
        })
        .unwrap();

    for needle in ["safeheaderneedle", "requestheaderneedle"] {
        let rows = store.search_traces(&text_filter(needle)).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["id"], "trace-headers");
        assert_eq!(rows[0]["ts_request_ms"], 9_001);
    }
    for secret in ["leakedheaderneedle", "privateheaderneedle"] {
        assert!(store
            .search_traces(&text_filter(secret))
            .unwrap()
            .is_empty());
    }

    store.clear_lar_normalized_index().unwrap();
    assert!(store
        .search_traces(&text_filter("safeheaderneedle"))
        .unwrap()
        .is_empty());
    store
        .rebuild_lar_normalized_index(&LarFtsRebuildOptions::default())
        .unwrap();
    assert_eq!(
        store
            .search_traces(&text_filter("safeheaderneedle"))
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn long_messages_are_segmented_and_truncation_never_claims_full_coverage() {
    let root = tmpdir("long-segments");
    let store = Store::open_with_lar_body_store(root.clone(), config(4 * 1024 * 1024)).unwrap();
    let long_searchable = format!(
        "{{\"messages\":[{{\"role\":\"user\",\"content\":\"{} latebodyneedle\"}}]}}",
        "filler ".repeat(12_000)
    );
    let written = store
        .write_body_artifact(
            &LarBodyArtifact::trace("trace-long-complete", "client_request"),
            "request.json",
            long_searchable.as_bytes(),
        )
        .unwrap();
    store
        .insert_trace(&trace(
            "trace-long-complete",
            "session-long",
            10_000,
            written.legacy_path,
        ))
        .unwrap();
    assert_eq!(
        store.search_traces(&text_filter("latebodyneedle")).unwrap()[0]["id"],
        "trace-long-complete"
    );

    let unique_filler = (0..140_000)
        .map(|index| format!("word{index} "))
        .collect::<String>();
    let oversized = format!(
        "{{\"messages\":[{{\"role\":\"user\",\"content\":\"{} beyondindexneedle\"}}]}}",
        unique_filler
    );
    let written = store
        .write_body_artifact(
            &LarBodyArtifact::trace("trace-long-partial", "client_request"),
            "request.json",
            oversized.as_bytes(),
        )
        .unwrap();
    store
        .insert_trace(&trace(
            "trace-long-partial",
            "session-long",
            11_000,
            written.legacy_path,
        ))
        .unwrap();
    let coverage = store
        .lar_normalized_indexed_artifacts(&["trace-long-partial".into()])
        .unwrap();
    assert!(!coverage.contains(&("trace-long-partial".into(), "client_request".into())));
    let conn = Connection::open(root.join("alexandria.sqlite3")).unwrap();
    let status: String = conn
        .query_row(
            "SELECT status FROM lar_normalized_artifact_state
             WHERE owner_kind='trace' AND owner_id='trace-long-partial'
               AND artifact_kind='client_request'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(status, "skipped_limit");
}

#[test]
fn fallback_cursor_is_stable_when_trace_timestamps_are_equal() {
    let root = tmpdir("stable-cursor");
    let store = Store::open(root).unwrap();
    for id in ["trace-c", "trace-a", "trace-b"] {
        store
            .insert_trace(&TraceRecord {
                id: id.into(),
                ts_request_ms: 42,
                ..Default::default()
            })
            .unwrap();
    }
    let first = store
        .search_traces(&TraceFilter {
            limit: 2,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(first[0]["id"], "trace-c");
    assert_eq!(first[1]["id"], "trace-b");
    let second = store
        .search_traces(&TraceFilter {
            before: Some((42, "trace-b".into())),
            limit: 2,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(second.len(), 1);
    assert_eq!(second[0]["id"], "trace-a");
}
