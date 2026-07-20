use std::io::{self, Cursor, Read};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use alex_core::TraceRecord;
use alex_store::{LarBodyStoreConfig, LarBodyStoreMode, LarJsonlImportOptions, Store};
use base64::Engine as _;
use serde_json::{json, Value};

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

fn tmpdir(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "alex-jsonl-import-{label}-{}-{}",
        std::process::id(),
        NEXT_DIR.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn import_store(path: PathBuf) -> Store {
    Store::open_with_lar_body_store(
        path,
        LarBodyStoreConfig {
            mode: LarBodyStoreMode::LarWithFallback,
            ..LarBodyStoreConfig::default()
        },
    )
    .unwrap()
}

fn encoded(body: Option<&[u8]>) -> Value {
    body.map_or(Value::Null, |body| {
        json!({
            "encoding": "base64",
            "length": body.len(),
            "blake3": blake3::hash(body).to_hex().to_string(),
            "data": base64::engine::general_purpose::STANDARD.encode(body),
        })
    })
}

fn exported_trace(id: &str, request: Option<&[u8]>, response: Option<&[u8]>) -> Value {
    let dir = tmpdir("metadata-source");
    let store = Store::open(dir.clone()).unwrap();
    store
        .insert_trace(&TraceRecord {
            id: id.into(),
            ts_request_ms: 1_700_000_000_000,
            ts_response_ms: Some(1_700_000_000_025),
            session_id: Some("session-jsonl".into()),
            harness: Some("test".into()),
            upstream_provider: Some("openai".into()),
            requested_model: Some("requested-model".into()),
            routed_model: Some("routed-model".into()),
            status: Some(200),
            req_headers_json: Some(r#"[["Content-Type","application/json"]]"#.into()),
            resp_headers_json: Some(r#"{"content-type":"application/json"}"#.into()),
            ..TraceRecord::default()
        })
        .unwrap();
    let mut metadata = store.export_trace_backup_rows().unwrap().traces.remove(0);
    let object = metadata.as_object_mut().unwrap();
    object.remove("req_body_path");
    object.remove("upstream_req_body_path");
    object.remove("resp_body_path");
    std::fs::remove_dir_all(dir).unwrap();
    json!({
        "type": "alex.trace",
        "metadata": metadata,
        "headers": {
            "request": [{"name": "Content-Type", "value": "application/json"}],
            "response": [{"name": "content-type", "value": "application/json"}],
            "fidelity": "legacy_order_and_casing_unknown",
        },
        "artifacts": {
            "client_request": encoded(request),
            "upstream_request": Value::Null,
            "client_response": encoded(response),
        },
    })
}

fn jsonl(records: &[Value]) -> Vec<u8> {
    let mut out = serde_json::to_vec(&json!({
        "type": "alex.lar.export.manifest",
        "version": 1,
        "format": "jsonl",
        "loss_report": ["legacy header casing unavailable"],
    }))
    .unwrap();
    out.push(b'\n');
    for record in records {
        serde_json::to_writer(&mut out, record).unwrap();
        out.push(b'\n');
    }
    out
}

#[test]
fn imports_exact_bodies_metadata_and_is_idempotent() {
    let dir = tmpdir("roundtrip");
    let store = import_store(dir.clone());
    let request = br#"{"messages":[{"role":"user","content":"hello"}]}"#;
    let response = br#"{"choices":[{"message":{"content":"hi"}}]}"#;
    let bytes = jsonl(&[exported_trace("trace-jsonl", Some(request), Some(response))]);

    let first = store
        .import_lar_jsonl(Cursor::new(&bytes), &LarJsonlImportOptions::default())
        .unwrap();
    assert_eq!(first.traces_imported, 1);
    assert_eq!(first.traces_skipped, 0);
    assert_eq!(first.bodies_written, 2);
    assert_eq!(
        first.source_loss_report,
        ["legacy header casing unavailable"]
    );
    assert_eq!(
        store
            .read_lar_or_legacy_artifact("trace", "trace-jsonl", "client_request", None)
            .unwrap()
            .as_deref(),
        Some(request.as_slice())
    );
    assert_eq!(
        store
            .read_lar_or_legacy_artifact("trace", "trace-jsonl", "client_response", None)
            .unwrap()
            .as_deref(),
        Some(response.as_slice())
    );

    let repeated = store
        .import_lar_jsonl(Cursor::new(&bytes), &LarJsonlImportOptions::default())
        .unwrap();
    assert_eq!(repeated.traces_imported, 0);
    assert_eq!(repeated.traces_skipped, 1);
    assert_eq!(store.export_trace_backup_rows().unwrap().traces.len(), 1);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn rejects_malformed_base64_hash_types_and_duplicate_trace_ids() {
    let cases: [(&str, fn(&mut Value), &str); 4] = [
        (
            "base64",
            |record: &mut Value| record["artifacts"]["client_request"]["data"] = json!("***"),
            "base64",
        ),
        (
            "hash",
            |record: &mut Value| {
                record["artifacts"]["client_request"]["blake3"] = json!("00".repeat(32))
            },
            "BLAKE3",
        ),
        (
            "type",
            |record: &mut Value| record["metadata"]["status"] = json!("200"),
            "status must be an integer",
        ),
        (
            "headers",
            |record: &mut Value| record["headers"]["request"][0]["value"] = json!("text/plain"),
            "headers do not match",
        ),
    ];
    for (label, mutate, expected) in cases {
        let dir = tmpdir(label);
        let store = import_store(dir.clone());
        let mut record = exported_trace("trace-bad", Some(b"request"), None);
        mutate(&mut record);
        let error = store
            .import_lar_jsonl(
                Cursor::new(jsonl(&[record])),
                &LarJsonlImportOptions::default(),
            )
            .unwrap_err();
        assert!(format!("{error:#}").contains(expected), "{error:#}");
        assert!(store.export_trace_backup_rows().unwrap().traces.is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    let dir = tmpdir("duplicate");
    let store = import_store(dir.clone());
    let record = exported_trace("trace-duplicate", Some(b"same"), None);
    let error = store
        .import_lar_jsonl(
            Cursor::new(jsonl(&[record.clone(), record])),
            &LarJsonlImportOptions::default(),
        )
        .unwrap_err();
    assert!(format!("{error:#}").contains("duplicates trace ID"));
    assert_eq!(store.export_trace_backup_rows().unwrap().traces.len(), 1);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn hard_limits_reject_oversized_lines_and_decoded_bodies_before_trace_publish() {
    let record = exported_trace("trace-large", Some(&vec![7; 256]), None);
    let bytes = jsonl(&[record]);

    let dir = tmpdir("line-limit");
    let store = import_store(dir.clone());
    let mut options = LarJsonlImportOptions::default();
    options.max_line_bytes = 128;
    let error = store
        .import_lar_jsonl(Cursor::new(&bytes), &options)
        .unwrap_err();
    assert!(format!("{error:#}").contains("line exceeds"));
    assert!(store.export_trace_backup_rows().unwrap().traces.is_empty());
    std::fs::remove_dir_all(dir).unwrap();

    let dir = tmpdir("body-limit");
    let store = import_store(dir.clone());
    let mut options = LarJsonlImportOptions::default();
    options.max_body_bytes = 32;
    let error = store
        .import_lar_jsonl(Cursor::new(&bytes), &options)
        .unwrap_err();
    assert!(format!("{error:#}").contains("decoded-body limit"));
    assert!(store.export_trace_backup_rows().unwrap().traces.is_empty());
    std::fs::remove_dir_all(dir).unwrap();
}

struct InterruptingReader {
    inner: Cursor<Vec<u8>>,
    fail_at: u64,
}

impl Read for InterruptingReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.inner.position() >= self.fail_at {
            return Err(io::Error::other("injected interruption"));
        }
        let remaining = (self.fail_at - self.inner.position()) as usize;
        let read_length = buffer.len().min(remaining);
        self.inner.read(&mut buffer[..read_length])
    }
}

#[test]
fn interrupted_stream_publishes_only_complete_lines_and_restart_resumes_idempotently() {
    let first = exported_trace("trace-first", Some(b"first"), None);
    let second = exported_trace("trace-second", Some(b"second"), None);
    let bytes = jsonl(&[first, second]);
    let second_start = bytes
        .windows(b"trace-second".len())
        .position(|window| window == b"trace-second")
        .unwrap() as u64;
    let dir = tmpdir("interrupted");
    let store = import_store(dir.clone());
    let error = store
        .import_lar_jsonl(
            InterruptingReader {
                inner: Cursor::new(bytes.clone()),
                fail_at: second_start + 4,
            },
            &LarJsonlImportOptions::default(),
        )
        .unwrap_err();
    assert!(format!("{error:#}").contains("injected interruption"));
    let rows = store.export_trace_backup_rows().unwrap().traces;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["id"], "trace-first");

    let resumed = store
        .import_lar_jsonl(Cursor::new(bytes), &LarJsonlImportOptions::default())
        .unwrap();
    assert_eq!(resumed.traces_skipped, 1);
    assert_eq!(resumed.traces_imported, 1);
    assert_eq!(store.export_trace_backup_rows().unwrap().traces.len(), 2);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn existing_conflicting_trace_is_rejected_without_overwrite() {
    let dir = tmpdir("conflict");
    let store = import_store(dir.clone());
    let original = exported_trace("trace-conflict", Some(b"original"), None);
    store
        .import_lar_jsonl(
            Cursor::new(jsonl(&[original])),
            &LarJsonlImportOptions::default(),
        )
        .unwrap();
    let conflicting = exported_trace("trace-conflict", Some(b"different"), None);
    let error = store
        .import_lar_jsonl(
            Cursor::new(jsonl(&[conflicting])),
            &LarJsonlImportOptions::default(),
        )
        .unwrap_err();
    assert!(format!("{error:#}").contains("conflicting client_request bytes"));
    assert_eq!(
        store
            .read_lar_or_legacy_artifact("trace", "trace-conflict", "client_request", None)
            .unwrap()
            .as_deref(),
        Some(b"original".as_slice())
    );
    std::fs::remove_dir_all(dir).unwrap();
}
