use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use alex_core::TraceRecord;
use alex_store::{
    LarArchiveReattachOptions, LarBodyArtifact, LarBodyStoreConfig, LarBodyStoreMode,
    LarExchangeBodyRefs, LarExchangeCapture, LarHeaderCapture, LarStageContentOptions,
    LarUpstreamAttemptCapture, Store, LAR_HEADER_FLAG_REDACTED, MAX_STAGE_CONTENT_LIMIT,
};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn tmpdir(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "alex-stage-content-{name}-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn config(max_pack_bytes: u64) -> LarBodyStoreConfig {
    let mut config = LarBodyStoreConfig {
        mode: LarBodyStoreMode::LarWithFallback,
        max_pack_bytes,
        ..Default::default()
    };
    config.chunker.min_size = 4;
    config.chunker.target_size = 4;
    config.chunker.max_size = 8;
    config
}

fn capture() -> LarExchangeCapture {
    LarExchangeCapture {
        trace_id: "trace-stage-content".into(),
        session_id: Some("session-stage-content".into()),
        run_id: Some("run-stage-content".into()),
        wall_time_ns: 1_000,
        client_request_headers: Some(LarHeaderCapture::observed([
            ("X-Repeat", "one"),
            ("X-Repeat", "two"),
            ("Authorization", "must-not-survive"),
        ])),
        client_response_headers: Some(LarHeaderCapture::legacy_normalized([
            ("content-type", "application/json"),
            ("x-response", "client"),
        ])),
        upstream_attempts: vec![
            LarUpstreamAttemptCapture {
                attempt_number: 1,
                wall_time_ns: 1_100,
                request_headers: Some(LarHeaderCapture::observed([
                    ("x-retry", "first"),
                    ("x-repeat", "one"),
                    ("x-repeat", "two"),
                ])),
                response_headers: Some(LarHeaderCapture::observed([("retry-after", "1")])),
                status_code: Some(429),
                error_class: Some("capacity".into()),
                error_message: Some("retry".into()),
            },
            LarUpstreamAttemptCapture {
                attempt_number: 2,
                wall_time_ns: 1_200,
                request_headers: Some(LarHeaderCapture::observed([
                    ("x-retry", "second"),
                    ("x-repeat", "one"),
                    ("x-repeat", "two"),
                ])),
                response_headers: Some(LarHeaderCapture::observed([
                    ("content-type", "application/json"),
                    ("x-response", "upstream"),
                ])),
                status_code: Some(200),
                error_class: None,
                error_message: None,
            },
        ],
        upstream_stream_reads: None,
        provider: Some("test".into()),
        requested_model: Some("requested".into()),
        routed_model: Some("routed".into()),
        account_id: Some("account".into()),
        routing_reason: Some("retry".into()),
        status_code: Some(200),
        error_class: None,
        error_message: None,
    }
}

fn populated_store(root: &PathBuf, max_pack_bytes: u64) -> (Store, u64) {
    let store = Store::open_with_lar_body_store(root.clone(), config(max_pack_bytes)).unwrap();
    store
        .insert_trace(&TraceRecord {
            id: "trace-stage-content".into(),
            session_id: Some("session-stage-content".into()),
            ts_request_ms: 1,
            ..Default::default()
        })
        .unwrap();
    let request = br#"{"messages":[{"role":"user","content":"hello"}]}"#;
    let upstream_response = br#"{"answer":"upstream"}"#;
    let client_response = br#"{"answer":"translated"}"#;
    let request_written = store
        .write_body_artifact(
            &LarBodyArtifact::trace("trace-stage-content", "client_request"),
            "request.json",
            request,
        )
        .unwrap();
    let upstream_request_written = store
        .write_body_artifact(
            &LarBodyArtifact::trace("trace-stage-content", "upstream_request"),
            "upstream-request.json",
            request,
        )
        .unwrap();
    let upstream_response_written = store
        .write_body_artifact(
            &LarBodyArtifact::trace("trace-stage-content", "upstream_response"),
            "upstream-response.body",
            upstream_response,
        )
        .unwrap();
    let client_response_written = store
        .write_body_artifact(
            &LarBodyArtifact::trace("trace-stage-content", "client_response"),
            "response.body",
            client_response,
        )
        .unwrap();
    assert_eq!(
        request_written.manifest_id, upstream_request_written.manifest_id,
        "identical stage bodies must share a manifest"
    );
    store
        .write_lar_exchange_capture(
            &capture(),
            &LarExchangeBodyRefs {
                client_request_manifest_id: request_written.manifest_id,
                upstream_request_manifest_id: upstream_request_written.manifest_id,
                upstream_response_manifest_id: upstream_response_written.manifest_id,
                client_response_manifest_id: client_response_written.manifest_id,
            },
        )
        .unwrap();
    (
        store,
        (request.len() + upstream_response.len() + client_response.len()) as u64,
    )
}

#[test]
fn actual_stage_content_preserves_duplicate_headers_retries_and_unique_body_budget() {
    let root = tmpdir("actual");
    let (store, unique_body_bytes) = populated_store(&root, u64::MAX);
    let page = store
        .lar_stage_content_page(
            "trace-stage-content",
            &LarStageContentOptions {
                body_byte_budget: unique_body_bytes,
                header_byte_budget: 64 * 1024,
                ..Default::default()
            },
        )
        .unwrap();

    assert_eq!(page.total_stages, 7);
    assert!(!page.stages_truncated);
    assert_eq!(
        page.stages
            .iter()
            .map(|stage| (stage.kind.as_str(), stage.attempt_number))
            .collect::<Vec<_>>(),
        [
            ("client_request", None),
            ("router_decision", None),
            ("upstream_request", Some(1)),
            ("upstream_response", Some(1)),
            ("upstream_request", Some(2)),
            ("upstream_response", Some(2)),
            ("client_response", None),
        ]
    );
    let client = &page.stages[0];
    let headers = page
        .header_blocks
        .iter()
        .find(|block| {
            Some(block.content_id.as_str()) == client.request_headers_content_id.as_deref()
        })
        .unwrap();
    assert_eq!(headers.state, "available");
    assert_eq!(headers.fidelity.as_deref(), Some("legacy_casing_unknown"));
    assert_eq!(
        headers
            .atoms
            .iter()
            .map(|atom| (
                atom.original_name.as_slice(),
                atom.value.as_slice(),
                atom.flags
            ))
            .collect::<Vec<_>>(),
        [
            (b"X-Repeat".as_slice(), b"one".as_slice(), 0),
            (b"X-Repeat".as_slice(), b"two".as_slice(), 0),
            (
                b"Authorization".as_slice(),
                b"<redacted>".as_slice(),
                LAR_HEADER_FLAG_REDACTED,
            ),
        ]
    );
    let client_response = page
        .stages
        .iter()
        .find(|stage| stage.kind == "client_response")
        .unwrap();
    let response_headers = page
        .header_blocks
        .iter()
        .find(|block| {
            Some(block.content_id.as_str())
                == client_response.response_headers_content_id.as_deref()
        })
        .unwrap();
    assert_eq!(
        response_headers.fidelity.as_deref(),
        Some("legacy_order_and_casing_unknown")
    );
    let retries = page
        .stages
        .iter()
        .filter(|stage| stage.kind == "upstream_request")
        .collect::<Vec<_>>();
    assert_eq!(retries.len(), 2);
    assert_ne!(
        retries[0].request_headers_content_id, retries[1].request_headers_content_id,
        "actual retry header blocks differ"
    );
    assert_eq!(
        page.stages[0].request_body_content_id, page.stages[4].request_body_content_id,
        "shared client/upstream bytes use one response content record"
    );
    assert_ne!(
        page.stages[5].response_body_content_id,
        page.stages[6].response_body_content_id
    );
    assert_eq!(page.bodies.len(), 3);
    assert_eq!(page.body_bytes_loaded, unique_body_bytes);
    assert!(page.bodies.iter().all(|body| body.state == "available"));

    let bounded = store
        .lar_stage_content_page(
            "trace-stage-content",
            &LarStageContentOptions {
                stage_limit: 3,
                body_byte_budget: 0,
                header_byte_budget: 0,
                after_capture_sequence: None,
                after_stage_id: None,
            },
        )
        .unwrap();
    assert_eq!(bounded.stages.len(), 3);
    assert!(bounded.stages_truncated);
    assert_eq!(bounded.body_bytes_loaded, 0);
    assert_eq!(bounded.header_bytes_loaded, 0);
    assert!(bounded.bodies.iter().all(|body| body.state == "truncated"));
    assert!(bounded
        .header_blocks
        .iter()
        .all(|headers| headers.state == "truncated"));

    let mut cursor = None;
    let mut traversed = Vec::new();
    loop {
        let page = store
            .lar_stage_content_page(
                "trace-stage-content",
                &LarStageContentOptions {
                    stage_limit: 2,
                    body_byte_budget: unique_body_bytes,
                    header_byte_budget: 64 * 1024,
                    after_capture_sequence: cursor
                        .as_ref()
                        .map(|cursor: &alex_store::LarStageContentCursor| cursor.capture_sequence),
                    after_stage_id: cursor
                        .as_ref()
                        .map(|cursor: &alex_store::LarStageContentCursor| cursor.stage_id.clone()),
                },
            )
            .unwrap();
        traversed.extend(page.stages.iter().map(|stage| stage.stage_id.clone()));
        if !page.has_more {
            assert!(page.next_cursor.is_none());
            break;
        }
        cursor = page.next_cursor;
    }
    assert_eq!(traversed.len(), 7);
    assert_eq!(
        traversed
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        traversed.len(),
        "pagination must neither repeat nor skip stages"
    );

    let invalid = store
        .lar_stage_content_page(
            "trace-stage-content",
            &LarStageContentOptions {
                stage_limit: MAX_STAGE_CONTENT_LIMIT + 1,
                ..Default::default()
            },
        )
        .unwrap_err();
    assert_eq!(
        invalid
            .downcast_ref::<alex_store::LarStageContentError>()
            .unwrap()
            .code(),
        "stage_content_invalid_request"
    );
}

#[test]
fn stage_content_surfaces_offline_archives_and_legacy_fallback() {
    let root = tmpdir("availability");
    let (store, _) = populated_store(&root, 1);
    let sealed = store
        .lar_archive_file_statuses()
        .unwrap()
        .into_iter()
        .find(|file| file.role == "body-pack" && file.catalog_state == "sealed")
        .unwrap();
    let original_path = PathBuf::from(&sealed.resolved_path);
    store.detach_lar_archive(&sealed.file_uuid).unwrap();
    let offline = store
        .lar_stage_content_page("trace-stage-content", &LarStageContentOptions::default())
        .unwrap();
    assert!(offline.bodies.iter().any(|body| {
        body.state == "archived_offline"
            && body.archive_file_uuid.as_deref() == Some(sealed.file_uuid.as_str())
    }));

    store
        .reattach_lar_archive(
            &sealed.file_uuid,
            &original_path,
            &LarArchiveReattachOptions::default(),
        )
        .unwrap();
    let parked = original_path.with_extension("parked.lar");
    std::fs::rename(&original_path, &parked).unwrap();
    let missing = store
        .lar_stage_content_page("trace-stage-content", &LarStageContentOptions::default())
        .unwrap();
    assert!(
        missing.bodies.iter().any(|body| {
            body.state == "archived_missing"
                && body.archive_file_uuid.as_deref() == Some(sealed.file_uuid.as_str())
        }),
        "missing outcomes: {:?}",
        missing.bodies
    );

    let legacy_root = tmpdir("legacy");
    let legacy_store = Store::open(legacy_root).unwrap();
    let legacy_bytes = br#"{"legacy":true}"#;
    let legacy_path = legacy_store
        .write_body("legacy-stage", "request.json", legacy_bytes)
        .unwrap();
    let upstream_path = legacy_store
        .write_body("legacy-stage", "upstream-request.json", legacy_bytes)
        .unwrap();
    let response_path = legacy_store
        .write_body("legacy-stage", "response.body", br#"{"legacy":"response"}"#)
        .unwrap();
    legacy_store
        .insert_trace(&TraceRecord {
            id: "legacy-stage".into(),
            session_id: Some("legacy-session".into()),
            ts_request_ms: 1,
            req_body_path: Some(legacy_path),
            upstream_req_body_path: Some(upstream_path),
            resp_body_path: Some(response_path),
            req_headers_json: Some(r#"{"X-Legacy":"yes"}"#.into()),
            resp_headers_json: Some(r#"{"X-Response":"yes"}"#.into()),
            ..Default::default()
        })
        .unwrap();
    let legacy = legacy_store
        .lar_stage_content_page("legacy-stage", &LarStageContentOptions::default())
        .unwrap();
    assert_eq!(legacy.total_stages, 3);
    assert_eq!(
        legacy
            .stages
            .iter()
            .map(|stage| stage.kind.as_str())
            .collect::<Vec<_>>(),
        ["client_request", "upstream_request", "client_response"]
    );
    assert!(legacy.stages.iter().all(|stage| {
        stage.fidelity == "legacy"
            && stage.attempt_number.is_none()
            && !stage.limitations.is_empty()
    }));
    let request = legacy
        .stages
        .iter()
        .find(|stage| stage.kind == "client_request")
        .unwrap();
    let body = legacy
        .bodies
        .iter()
        .find(|body| Some(body.content_id.as_str()) == request.request_body_content_id.as_deref())
        .unwrap();
    assert_eq!(body.state, "available");
    assert_eq!(body.fidelity, "legacy");
    assert_eq!(body.bytes.as_deref(), Some(legacy_bytes.as_slice()));
    let headers = legacy
        .header_blocks
        .iter()
        .find(|block| {
            Some(block.content_id.as_str()) == request.request_headers_content_id.as_deref()
        })
        .unwrap();
    assert_eq!(headers.fidelity.as_deref(), Some("legacy_normalized"));
    assert_eq!(headers.atoms[0].original_name, b"X-Legacy");
}
