use std::collections::BTreeSet;
use std::fs::File;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use alex_core::TraceRecord;
use alex_lar::{ArchiveReader, Limits};
use alex_store::{
    LarArtifactReadRequest, LarBodyArtifact, LarBodyStoreConfig, LarBodyStoreMode,
    LarExchangeBodyRefs, LarExchangeCapture, Store,
};
use rusqlite::Connection;

static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn tmpdir() -> PathBuf {
    let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "alex-lar-session-rotation-{}-{sequence}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn capture(trace_id: &str, wall_time_ns: u64) -> LarExchangeCapture {
    LarExchangeCapture {
        trace_id: trace_id.into(),
        session_id: Some("rotating-session".into()),
        run_id: Some("rotating-run".into()),
        wall_time_ns,
        client_request_headers: None,
        client_request_trailers: None,
        client_response_headers: None,
        client_response_trailers: None,
        upstream_attempts: Vec::new(),
        upstream_stream_reads: None,
        provider: Some("test".into()),
        requested_model: Some("test/model".into()),
        routed_model: Some("test/model".into()),
        account_id: None,
        routing_reason: Some("rotation-test".into()),
        status_code: Some(200),
        error_class: None,
        error_message: None,
    }
}

#[test]
fn one_session_remains_pageable_and_exact_across_body_and_event_rotations() {
    let root = tmpdir();
    let store = Store::open_with_lar_body_store(
        root.clone(),
        LarBodyStoreConfig {
            mode: LarBodyStoreMode::LarWithFallback,
            max_pack_bytes: 1,
            ..Default::default()
        },
    )
    .unwrap();
    let mut expected = Vec::new();
    for index in 0..6 {
        let trace_id = format!("rotation-trace-{index}");
        let body = serde_json::to_vec(&serde_json::json!({
            "messages": [{"role": "user", "content": format!("rotation turn {index}")}]
        }))
        .unwrap();
        let written = store
            .write_body_artifact(
                &LarBodyArtifact::trace(&trace_id, "client_request"),
                "request.json",
                &body,
            )
            .unwrap();
        let manifest = written.manifest_id.clone().unwrap();
        store
            .insert_trace(&TraceRecord {
                id: trace_id.clone(),
                session_id: Some("rotating-session".into()),
                run_id: Some("rotating-run".into()),
                ts_request_ms: 1_000 + index,
                req_body_path: Some(written.legacy_path.clone()),
                status: Some(200),
                ..Default::default()
            })
            .unwrap();
        store
            .write_lar_exchange_capture(
                &capture(&trace_id, (1_000 + index) as u64 * 1_000_000),
                &LarExchangeBodyRefs {
                    client_request_manifest_id: Some(manifest),
                    ..Default::default()
                },
            )
            .unwrap()
            .unwrap();
        std::fs::remove_file(written.legacy_path).unwrap();
        expected.push((trace_id, body));
    }

    let conn = Connection::open(root.join("alexandria.sqlite3")).unwrap();
    let files = conn
        .prepare("SELECT path, state FROM lar_files ORDER BY created_at_ms, file_uuid")
        .unwrap()
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert!(files.iter().any(|(_, state)| state == "sealed"));
    assert!(files.iter().any(|(_, state)| state == "active"));
    let mut archived_traces = BTreeSet::new();
    let mut files_with_session_events = 0usize;
    for (path, _) in &files {
        let reader = ArchiveReader::open(File::open(path).unwrap(), Limits::default()).unwrap();
        if let Some(exchange_ids) = reader.exchanges_for_session(b"rotating-session") {
            if !exchange_ids.is_empty() {
                files_with_session_events += 1;
            }
            for id in exchange_ids {
                let trace = &reader.exchange(id).unwrap().data.trace_id;
                archived_traces.insert(String::from_utf8(trace.clone()).unwrap());
            }
        }
    }
    assert!(files_with_session_events >= 2);
    assert_eq!(archived_traces.len(), expected.len());

    let mut after = None;
    let mut seen = Vec::new();
    loop {
        let page = store
            .session_traces_page("rotating-session", after.clone(), None, 2, false)
            .unwrap();
        if page.rows.is_empty() {
            break;
        }
        let requests = page
            .rows
            .iter()
            .map(|row| {
                LarArtifactReadRequest::new("trace", row["id"].as_str().unwrap(), "client_request")
            })
            .collect::<Vec<_>>();
        let bodies = store.read_lar_or_legacy_artifact_batch(&requests).unwrap();
        for (row, body) in page.rows.iter().zip(bodies) {
            seen.push((
                row["id"].as_str().unwrap().to_owned(),
                body.expect("LAR-only request body remains readable"),
            ));
        }
        let last = page.rows.last().unwrap();
        after = Some((
            last["ts_request_ms"].as_i64().unwrap(),
            last["id"].as_str().unwrap().to_owned(),
        ));
        if !page.has_more_after {
            break;
        }
    }
    assert_eq!(seen, expected);
}
