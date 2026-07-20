use std::fs::File;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use alex_lar::{ArchiveReader, HeaderFidelity, Limits, StageKind};
use alex_store::{
    LarBodyArtifact, LarBodyStoreConfig, LarBodyStoreMode, LarExchangeBodyRefs, LarExchangeCapture,
    LarHeaderCapture, LarStreamReadCapture, LarUpstreamAttemptCapture, Store,
    LAR_HEADER_FLAG_REDACTED,
};
use rusqlite::Connection;

static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn tmpdir(name: &str) -> PathBuf {
    let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "alex-lar-exchange-{name}-{}-{sequence}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn config() -> LarBodyStoreConfig {
    LarBodyStoreConfig {
        mode: LarBodyStoreMode::LarWithFallback,
        max_pack_bytes: 16 * 1024 * 1024,
        ..Default::default()
    }
}

#[test]
fn ordered_headers_trailers_stages_and_raw_body_identity_are_preserved() {
    let root = tmpdir("fidelity");
    let store = Store::open_with_lar_body_store(root.clone(), config()).unwrap();
    let request = store
        .write_body_artifact(
            &LarBodyArtifact::trace("trace-ordered", "client_request"),
            "request.json",
            br#"{"message":"hello"}"#,
        )
        .unwrap();
    let first_read = b"data: {broken\n\n";
    let second_read = b"\xffopaque";
    let raw_response_body = [first_read.as_slice(), second_read.as_slice()].concat();
    let response = store
        .write_body_artifact(
            &LarBodyArtifact::trace("trace-ordered", "client_response"),
            "response.body",
            &raw_response_body,
        )
        .unwrap();

    let client_headers = LarHeaderCapture::observed([
        ("x-repeat", "first"),
        ("x-repeat", "second"),
        ("x-goog-api-key", "google-secret"),
        ("authorization", "Bearer secret"),
        ("cookie", "session=secret"),
    ]);
    let upstream_headers = LarHeaderCapture::observed([
        ("content-type", "application/json"),
        ("x-api-key", "provider-secret"),
    ]);
    let upstream_response_headers = LarHeaderCapture::observed([
        ("content-type", "text/event-stream"),
        ("set-cookie", "provider-session=secret"),
    ]);
    let client_request_trailers = LarHeaderCapture::observed([
        ("x-client-request-trailer", "first"),
        ("x-client-request-trailer", "second"),
    ]);
    let client_response_trailers =
        LarHeaderCapture::observed([("x-client-response-trailer", "complete")]);
    let upstream_request_trailers =
        LarHeaderCapture::observed([("x-upstream-request-trailer", "sent")]);
    let upstream_response_trailers = LarHeaderCapture::observed([
        ("x-upstream-response-trailer", "received"),
        ("authorization", "Bearer trailer-secret"),
    ]);
    let capture = LarExchangeCapture {
        trace_id: "trace-ordered".into(),
        session_id: Some("session-ordered".into()),
        run_id: Some("run-ordered".into()),
        wall_time_ns: 1_000_000,
        client_request_headers: Some(client_headers),
        client_request_trailers: Some(client_request_trailers),
        client_response_headers: Some(LarHeaderCapture::observed([
            ("content-type", "text/event-stream"),
            ("x-alexandria-trace-id", "trace-ordered"),
        ])),
        client_response_trailers: Some(client_response_trailers),
        upstream_attempts: vec![LarUpstreamAttemptCapture {
            attempt_number: 1,
            wall_time_ns: 1_100_000,
            request_headers: Some(upstream_headers),
            request_trailers: Some(upstream_request_trailers),
            response_headers: Some(upstream_response_headers),
            response_trailers: Some(upstream_response_trailers),
            status_code: Some(200),
            error_class: None,
            error_message: None,
        }],
        upstream_stream_reads: Some(vec![
            LarStreamReadCapture {
                byte_offset: 0,
                byte_length: first_read.len() as u64,
                delta_from_first_byte_ns: 0,
            },
            LarStreamReadCapture {
                byte_offset: first_read.len() as u64,
                byte_length: second_read.len() as u64,
                delta_from_first_byte_ns: 4_000_000,
            },
        ]),
        provider: Some("gemini".into()),
        requested_model: Some("alex/gemini".into()),
        routed_model: Some("gemini".into()),
        account_id: Some("account-redacted-from-headers-only".into()),
        routing_reason: None,
        status_code: Some(200),
        error_class: None,
        error_message: None,
    };
    let refs = LarExchangeBodyRefs {
        client_request_manifest_id: request.manifest_id.clone(),
        // Passthrough request identity: the upstream stage reuses the same
        // manifest instead of storing a second copy of identical bytes.
        upstream_request_manifest_id: request.manifest_id.clone(),
        upstream_response_manifest_id: response.manifest_id.clone(),
        client_response_manifest_id: response.manifest_id.clone(),
    };
    store
        .write_lar_exchange_capture(&capture, &refs)
        .unwrap()
        .unwrap();

    let conn = Connection::open(root.join("alexandria.sqlite3")).unwrap();
    let archive_path: String = conn
        .query_row(
            "SELECT path FROM lar_files WHERE state='active' ORDER BY created_at_ms DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let reader = ArchiveReader::open(File::open(archive_path).unwrap(), Limits::default()).unwrap();
    let exchange = reader.exchange_by_trace(b"trace-ordered").unwrap();
    let stages = exchange
        .data
        .stages
        .iter()
        .map(|id| reader.stage(id).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        stages
            .iter()
            .map(|stage| stage.data.kind)
            .collect::<Vec<_>>(),
        vec![
            StageKind::ClientRequest,
            StageKind::RouterDecision,
            StageKind::UpstreamRequest,
            StageKind::UpstreamResponse,
            StageKind::ClientResponse,
        ]
    );
    let catalog_stages = store
        .lar_stages_for_traces(&["trace-ordered".to_owned()])
        .unwrap()
        .remove("trace-ordered")
        .unwrap();
    assert_eq!(catalog_stages.len(), 5);
    assert_eq!(catalog_stages[0]["kind"], "client_request");
    assert_eq!(catalog_stages[2]["kind"], "upstream_request");
    assert_eq!(catalog_stages[2]["attempt_number"], 1);
    assert_eq!(catalog_stages[3]["kind"], "upstream_response");
    assert!(catalog_stages[3]["stream_index_ref"].is_string());
    for stage in [
        &catalog_stages[0],
        &catalog_stages[2],
        &catalog_stages[3],
        &catalog_stages[4],
    ] {
        assert!(stage["trailers_ref"].is_string());
    }
    let cataloged_header_blocks: i64 = conn
        .query_row("SELECT COUNT(*) FROM lar_header_blocks", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(cataloged_header_blocks, 8);
    assert_eq!(
        catalog_stages[3]["response_body_manifest_ref"],
        catalog_stages[4]["response_body_manifest_ref"]
    );

    let client_block = reader
        .header_block(&stages[0].data.request_headers_ref.unwrap())
        .unwrap();
    assert_eq!(client_block.fidelity, HeaderFidelity::LegacyCasingUnknown);
    assert_eq!(client_block.atoms[0].value, b"first");
    assert_eq!(client_block.atoms[1].value, b"second");
    assert_eq!(client_block.atoms[0].original_name, b"x-repeat");
    assert_eq!(client_block.atoms[1].original_name, b"x-repeat");
    for secret_name in [b"x-goog-api-key".as_slice(), b"authorization", b"cookie"] {
        let atom = client_block
            .atoms
            .iter()
            .find(|atom| atom.original_name == secret_name)
            .unwrap();
        assert_eq!(atom.value, b"<redacted>");
        assert_eq!(
            atom.flags & LAR_HEADER_FLAG_REDACTED,
            LAR_HEADER_FLAG_REDACTED
        );
    }
    let upstream_response_block = reader
        .header_block(&stages[3].data.response_headers_ref.unwrap())
        .unwrap();
    assert_eq!(
        upstream_response_block
            .atoms
            .iter()
            .find(|atom| atom.original_name == b"set-cookie")
            .unwrap()
            .value,
        b"<redacted>"
    );
    let client_request_trailer_block = reader
        .header_block(&stages[0].data.trailers_ref.unwrap())
        .unwrap();
    assert_eq!(client_request_trailer_block.atoms.len(), 2);
    assert_eq!(
        client_request_trailer_block
            .atoms
            .iter()
            .map(|atom| atom.value.as_slice())
            .collect::<Vec<_>>(),
        vec![b"first".as_slice(), b"second".as_slice()]
    );
    let upstream_request_trailer_block = reader
        .header_block(&stages[2].data.trailers_ref.unwrap())
        .unwrap();
    assert_eq!(upstream_request_trailer_block.atoms[0].value, b"sent");
    let upstream_response_trailer_block = reader
        .header_block(&stages[3].data.trailers_ref.unwrap())
        .unwrap();
    assert_eq!(upstream_response_trailer_block.atoms[0].value, b"received");
    assert_eq!(
        upstream_response_trailer_block.atoms[1].value,
        b"<redacted>"
    );
    assert_eq!(
        upstream_response_trailer_block.atoms[1].flags & LAR_HEADER_FLAG_REDACTED,
        LAR_HEADER_FLAG_REDACTED
    );
    let client_response_trailer_block = reader
        .header_block(&stages[4].data.trailers_ref.unwrap())
        .unwrap();
    assert_eq!(client_response_trailer_block.atoms[0].value, b"complete");
    let stream_index = reader
        .stream_index(&stages[3].data.stream_index_ref.unwrap())
        .unwrap();
    assert_eq!(stream_index.reads.len(), 2);
    assert_eq!(stream_index.reads[0].byte_offset, 0);
    assert_eq!(stream_index.reads[0].byte_length, first_read.len() as u64);
    assert_eq!(stream_index.reads[0].delta_from_first_byte_ns, 0);
    assert_eq!(stream_index.reads[1].byte_offset, first_read.len() as u64);
    assert_eq!(stream_index.reads[1].byte_length, second_read.len() as u64);
    assert_eq!(stream_index.reads[1].delta_from_first_byte_ns, 4_000_000);
    assert!(
        stream_index.frames.is_empty(),
        "malformed SSE remains opaque"
    );
    assert!(stages[4].data.stream_index_ref.is_none());

    let raw_response = response.manifest_id.unwrap();
    assert_eq!(
        stages[3]
            .data
            .response_body_manifest_ref
            .unwrap()
            .to_string(),
        raw_response
    );
    assert_eq!(
        stages[4]
            .data
            .response_body_manifest_ref
            .unwrap()
            .to_string(),
        raw_response
    );
    let response_manifest_rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM lar_manifests WHERE manifest_id=?1",
            [raw_response.clone()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        response_manifest_rows, 1,
        "raw SSE bytes have one body identity"
    );
    assert_eq!(
        store.read_lar_manifest_body(&raw_response).unwrap(),
        raw_response_body,
        "timing metadata never changes the raw body bytes"
    );
}

#[test]
fn normalized_legacy_headers_are_never_labeled_exact() {
    let root = tmpdir("legacy-fidelity");
    let store = Store::open_with_lar_body_store(root.clone(), config()).unwrap();
    let request = store
        .write_body_artifact(
            &LarBodyArtifact::trace("legacy-header-trace", "client_request"),
            "request.json",
            b"{}",
        )
        .unwrap();
    let capture = LarExchangeCapture {
        trace_id: "legacy-header-trace".into(),
        session_id: None,
        run_id: None,
        wall_time_ns: 2_000_000,
        client_request_headers: Some(LarHeaderCapture::legacy_normalized([
            ("x-api-key", "old-secret"),
            ("content-type", "application/json"),
        ])),
        client_request_trailers: None,
        client_response_headers: None,
        client_response_trailers: None,
        upstream_attempts: vec![],
        upstream_stream_reads: None,
        provider: None,
        requested_model: None,
        routed_model: None,
        account_id: None,
        routing_reason: None,
        status_code: None,
        error_class: None,
        error_message: None,
    };
    store
        .write_lar_exchange_capture(
            &capture,
            &LarExchangeBodyRefs {
                client_request_manifest_id: request.manifest_id,
                ..Default::default()
            },
        )
        .unwrap();
    let conn = Connection::open(root.join("alexandria.sqlite3")).unwrap();
    let fidelity: String = conn
        .query_row(
            "SELECT fidelity FROM lar_header_blocks LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(fidelity, "legacy_normalized");
}

#[test]
fn non_stream_response_does_not_emit_a_stream_index() {
    let root = tmpdir("non-stream");
    let store = Store::open_with_lar_body_store(root.clone(), config()).unwrap();
    let response = store
        .write_body_artifact(
            &LarBodyArtifact::trace("trace-non-stream", "client_response"),
            "response.body",
            br#"{"message":"complete"}"#,
        )
        .unwrap()
        .manifest_id;
    store
        .write_lar_exchange_capture(
            &LarExchangeCapture {
                trace_id: "trace-non-stream".into(),
                session_id: None,
                run_id: None,
                wall_time_ns: 2_500_000,
                client_request_headers: None,
                client_request_trailers: None,
                client_response_headers: None,
                client_response_trailers: None,
                upstream_attempts: vec![LarUpstreamAttemptCapture {
                    attempt_number: 1,
                    wall_time_ns: 2_600_000,
                    request_headers: None,
                    request_trailers: None,
                    response_headers: None,
                    response_trailers: None,
                    status_code: Some(200),
                    error_class: None,
                    error_message: None,
                }],
                upstream_stream_reads: None,
                provider: Some("openai".into()),
                requested_model: None,
                routed_model: None,
                account_id: None,
                routing_reason: None,
                status_code: Some(200),
                error_class: None,
                error_message: None,
            },
            &LarExchangeBodyRefs {
                upstream_response_manifest_id: response.clone(),
                client_response_manifest_id: response,
                ..Default::default()
            },
        )
        .unwrap();

    let conn = Connection::open(root.join("alexandria.sqlite3")).unwrap();
    let archive_path: String = conn
        .query_row(
            "SELECT path FROM lar_files WHERE state='active' ORDER BY created_at_ms DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let reader = ArchiveReader::open(File::open(archive_path).unwrap(), Limits::default()).unwrap();
    assert_eq!(reader.stream_index_count(), 0);
    let exchange = reader.exchange_by_trace(b"trace-non-stream").unwrap();
    assert!(exchange.data.stages.iter().all(|stage| reader
        .stage(stage)
        .unwrap()
        .data
        .stream_index_ref
        .is_none()));
}

#[test]
fn translated_client_bytes_get_their_own_manifest_and_stage_reference() {
    let root = tmpdir("translated");
    let store = Store::open_with_lar_body_store(root.clone(), config()).unwrap();
    let upstream_body = b"event: message\ndata: {\"wire\":\"upstream\"}\n\n";
    let upstream = store
        .write_body_artifact(
            &LarBodyArtifact::trace("trace-translated", "upstream_response"),
            "upstream-response.body",
            upstream_body,
        )
        .unwrap()
        .manifest_id
        .unwrap();
    let client = store
        .write_body_artifact(
            &LarBodyArtifact::trace("trace-translated", "client_response"),
            "response.body",
            br#"{"translated":"client-wire"}"#,
        )
        .unwrap()
        .manifest_id
        .unwrap();
    assert_ne!(upstream, client);

    let capture = LarExchangeCapture {
        trace_id: "trace-translated".into(),
        session_id: Some("translated-session".into()),
        run_id: None,
        wall_time_ns: 3_000_000,
        client_request_headers: Some(LarHeaderCapture::observed([(
            "content-type",
            "application/json",
        )])),
        client_request_trailers: None,
        client_response_headers: Some(LarHeaderCapture::observed([(
            "content-type",
            "application/json",
        )])),
        client_response_trailers: None,
        upstream_attempts: vec![LarUpstreamAttemptCapture {
            attempt_number: 1,
            wall_time_ns: 3_100_000,
            request_headers: None,
            request_trailers: None,
            response_headers: Some(LarHeaderCapture::observed([(
                "content-type",
                "text/event-stream",
            )])),
            response_trailers: None,
            status_code: Some(200),
            error_class: None,
            error_message: None,
        }],
        upstream_stream_reads: Some(vec![LarStreamReadCapture {
            byte_offset: 0,
            byte_length: upstream_body.len() as u64,
            delta_from_first_byte_ns: 0,
        }]),
        provider: Some("anthropic".into()),
        requested_model: Some("foreign-client-model".into()),
        routed_model: Some("claude".into()),
        account_id: None,
        routing_reason: None,
        status_code: Some(200),
        error_class: None,
        error_message: None,
    };
    store
        .write_lar_exchange_capture(
            &capture,
            &LarExchangeBodyRefs {
                upstream_response_manifest_id: Some(upstream.clone()),
                client_response_manifest_id: Some(client.clone()),
                ..Default::default()
            },
        )
        .unwrap();

    assert_eq!(
        store.read_lar_manifest_body(&upstream).unwrap(),
        upstream_body
    );
    assert_eq!(
        store.read_lar_manifest_body(&client).unwrap(),
        br#"{"translated":"client-wire"}"#
    );
    let conn = Connection::open(root.join("alexandria.sqlite3")).unwrap();
    let archive_path: String = conn
        .query_row(
            "SELECT path FROM lar_files WHERE state='active' ORDER BY created_at_ms DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let reader = ArchiveReader::open(File::open(archive_path).unwrap(), Limits::default()).unwrap();
    let exchange = reader.exchange_by_trace(b"trace-translated").unwrap();
    let stages = exchange
        .data
        .stages
        .iter()
        .map(|id| reader.stage(id).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        stages[3]
            .data
            .response_body_manifest_ref
            .unwrap()
            .to_string(),
        upstream
    );
    assert_eq!(
        stages[4]
            .data
            .response_body_manifest_ref
            .unwrap()
            .to_string(),
        client
    );
    let stream = reader
        .stream_index(&stages[3].data.stream_index_ref.unwrap())
        .unwrap();
    assert_eq!(stream.frames.len(), 1);
    assert_eq!(stream.frames[0].byte_offset, 0);
    assert_eq!(stream.frames[0].byte_length, upstream_body.len() as u64);
    assert_eq!(stream.frames[0].delta_from_first_byte_ns, 0);
    assert_eq!(stream.frames[0].parser, alex_lar::StreamParser::Sse);
    assert_eq!(
        stream.frames[0].frame_kind,
        alex_lar::StreamFrameKind::SseEvent
    );
    assert!(stages[4].data.stream_index_ref.is_none());
}
