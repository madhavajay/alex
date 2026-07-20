use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use alex_core::{TraceRecord, Usage};
use alex_lar::{
    ArchiveReader, ArchiveWriter, ChunkerConfig, Exchange, ExchangeData, ExchangeMetadataData,
    FileHeader, HeaderAtom, HeaderBlock, HeaderFidelity, Limits, ParsedFrame, Stage, StageData,
    StageKind, StreamFrameKind, StreamIndex, StreamParser, StreamRead, TokenUsage,
};
use alex_store::{
    write_archive_transaction, write_synthesized_legacy_transaction, LarStandaloneImportOptions,
    Store, LAR_TRANSACTION_ARTIFACT_PIECE_BYTES, LAR_TRANSACTION_FORMAT, LAR_TRANSACTION_VERSION,
};
use base64::Engine as _;
use serde_json::Value;

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn tmpdir(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "alex-lar-transaction-{name}-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&path).unwrap();
    path
}

fn records(bytes: &[u8]) -> Vec<Value> {
    bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| {
            assert_eq!(line[0], 0x1e);
            serde_json::from_slice(&line[1..]).unwrap()
        })
        .collect()
}

fn supplement_trace_id(harness: &str, session_id: &str, tool_call_id: &str, phase: &str) -> String {
    let mut hash = blake3::Hasher::new();
    hash.update(b"alex-lar-tool-supplement-v1\0");
    for value in [
        harness.as_bytes(),
        session_id.as_bytes(),
        tool_call_id.as_bytes(),
        phase.as_bytes(),
    ] {
        hash.update(&(value.len() as u64).to_le_bytes());
        hash.update(value);
    }
    format!("lar-tool-{}-{phase}", &hash.finalize().to_hex()[..32])
}

fn write_fixture(path: &Path) -> (String, String, Vec<u8>, Vec<u8>, String, String, String) {
    let file = fs::OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .open(path)
        .unwrap();
    let mut writer = ArchiveWriter::create(
        file,
        FileHeader::standalone([31; 16], 1_000, b"transaction-test".to_vec()),
        ChunkerConfig {
            min_size: 16 * 1024,
            target_size: 32 * 1024,
            max_size: 64 * 1024,
        },
        Limits::default(),
    )
    .unwrap();
    let mut binary = (0..(LAR_TRANSACTION_ARTIFACT_PIECE_BYTES * 3 + 17))
        .map(|index| (index % 251) as u8)
        .collect::<Vec<_>>();
    binary[7] = 0;
    binary[8] = 0xff;
    let stream_body = b"data: one\n\ndata: two\n\n".to_vec();
    let binary_manifest = writer.append_body(&binary).unwrap();
    let stream_manifest = writer.append_body(&stream_body).unwrap();
    let duplicate_headers = writer
        .append_header_block(HeaderBlock::new(
            HeaderFidelity::Exact,
            vec![
                HeaderAtom {
                    original_name: b"x-duplicate".to_vec(),
                    value: b"one".to_vec(),
                    flags: 0,
                },
                HeaderAtom {
                    original_name: b"x-duplicate".to_vec(),
                    value: b"two".to_vec(),
                    flags: 0,
                },
            ],
        ))
        .unwrap();
    let trailers = writer
        .append_header_block(HeaderBlock::new(
            HeaderFidelity::Exact,
            vec![HeaderAtom {
                original_name: b"x-checksum".to_vec(),
                value: vec![0, 0xff, 42],
                flags: 0,
            }],
        ))
        .unwrap();
    let stream = writer
        .append_stream_index(StreamIndex::new(
            stream_manifest,
            vec![
                StreamRead {
                    byte_offset: 0,
                    byte_length: 11,
                    delta_from_first_byte_ns: 0,
                },
                StreamRead {
                    byte_offset: 11,
                    byte_length: 11,
                    delta_from_first_byte_ns: 20_000_000,
                },
            ],
            vec![
                ParsedFrame {
                    byte_offset: 0,
                    byte_length: 11,
                    delta_from_first_byte_ns: 0,
                    parser: StreamParser::Sse,
                    frame_kind: StreamFrameKind::SseEvent,
                },
                ParsedFrame {
                    byte_offset: 11,
                    byte_length: 11,
                    delta_from_first_byte_ns: 20_000_000,
                    parser: StreamParser::Sse,
                    frame_kind: StreamFrameKind::SseEvent,
                },
            ],
        ))
        .unwrap();

    let mut client_request = StageData::new(StageKind::ClientRequest, 1_000);
    client_request.request_headers_ref = Some(duplicate_headers);
    client_request.request_body_manifest_ref = Some(binary_manifest);
    client_request.requested_model = Some(b"requested-model".to_vec());
    let client_request = writer.append_stage(Stage::new(client_request)).unwrap();

    let mut failed = StageData::new(StageKind::UpstreamFailure, 1_010);
    failed.attempt_number = Some(1);
    failed.request_body_manifest_ref = Some(binary_manifest);
    failed.error_class = Some(b"rate_limit".to_vec());
    failed.error_message = Some(b"retry me".to_vec());
    let failed = writer.append_stage(Stage::new(failed)).unwrap();

    let mut retry = StageData::new(StageKind::UpstreamRequest, 1_020);
    retry.attempt_number = Some(2);
    retry.request_body_manifest_ref = Some(binary_manifest);
    retry.provider = Some(b"provider".to_vec());
    retry.routing_reason = Some(b"failover".to_vec());
    let retry = writer.append_stage(Stage::new(retry)).unwrap();

    let mut response = StageData::new(StageKind::ClientResponse, 1_030);
    response.response_headers_ref = Some(duplicate_headers);
    response.response_body_manifest_ref = Some(stream_manifest);
    response.trailers_ref = Some(trailers);
    response.stream_index_ref = Some(stream);
    response.status_code = Some(200);
    response.usage = Some(TokenUsage {
        input_tokens: 10,
        output_tokens: 20,
        cached_tokens: 3,
        reasoning_tokens: 4,
    });
    response.cost_nanos = Some(1234);
    response.cost_currency = Some(b"USD".to_vec());
    let response = writer.append_stage(Stage::new(response)).unwrap();

    let trace_id = "transaction-trace".to_string();
    let mut exchange = ExchangeData::new(
        trace_id.as_bytes(),
        7,
        1_000,
        vec![client_request, failed, retry, retry, response],
    );
    exchange.session_id = Some(b"transaction-session".to_vec());
    let mut metadata = ExchangeMetadataData::default();
    metadata.method = Some(b"POST".to_vec());
    metadata.path = Some(b"/v1/messages?shape=exact".to_vec());
    metadata.client_format = Some(b"anthropic".to_vec());
    metadata.upstream_format = Some(b"openai".to_vec());
    writer
        .append_exchange_with_metadata(Exchange::new(exchange), metadata)
        .unwrap();

    let tool_call_id = "transaction-tool-call";
    let supplement_trace = supplement_trace_id("pi", "transaction-session", tool_call_id, "end");
    let mut tool_result = StageData::new(StageKind::ToolResult, 2_000_000);
    tool_result.response_body_manifest_ref = Some(binary_manifest);
    tool_result.routing_reason = Some(
        serde_json::to_vec(&serde_json::json!({
            "schema": "alex.tool-supplement.v1",
            "phase": "end",
            "tool_id": "transaction-tool",
            "harness": "pi",
            "turn_id": "turn-1",
            "tool_call_id": tool_call_id,
            "tool_name": "bash",
            "source_trace_id": trace_id,
            "ts_start_ms": 1,
            "ts_end_ms": 2,
            "is_error": false,
            "exit_status": 0,
        }))
        .unwrap(),
    );
    let supplement_stage = writer.append_stage(Stage::new(tool_result)).unwrap();
    let mut supplement_exchange = ExchangeData::new(
        supplement_trace.as_bytes(),
        8,
        2_000_000,
        vec![supplement_stage],
    );
    supplement_exchange.session_id = Some(b"transaction-session".to_vec());
    supplement_exchange.parent_trace_id = Some(trace_id.as_bytes().to_vec());
    let supplement_exchange_id = writer
        .append_exchange(Exchange::new(supplement_exchange))
        .unwrap();

    let unrelated_stage = writer
        .append_stage(Stage::new(StageData::new(StageKind::ClientRequest, 1_050)))
        .unwrap();
    let mut unrelated_exchange =
        ExchangeData::new(b"ordinary-child", 9, 1_050, vec![unrelated_stage]);
    unrelated_exchange.session_id = Some(b"transaction-session".to_vec());
    unrelated_exchange.parent_trace_id = Some(trace_id.as_bytes().to_vec());
    writer
        .append_exchange(Exchange::new(unrelated_exchange))
        .unwrap();
    writer.seal().unwrap();
    writer.into_inner().unwrap().sync_all().unwrap();
    (
        trace_id,
        stream.to_string(),
        binary,
        stream_body,
        supplement_stage.to_string(),
        supplement_exchange_id.to_string(),
        unrelated_stage.to_string(),
    )
}

#[test]
fn sealed_and_live_transaction_exports_preserve_complete_canonical_bytes_and_order() {
    let dir = tmpdir("canonical");
    let archive = dir.join("source.lar");
    let (
        trace_id,
        stream_id,
        binary,
        stream_body,
        supplement_stage_id,
        supplement_exchange_id,
        unrelated_stage_id,
    ) = write_fixture(&archive);

    let mut reader =
        ArchiveReader::open(fs::File::open(&archive).unwrap(), Limits::default()).unwrap();
    let mut sealed = Vec::new();
    let report = write_archive_transaction(&mut reader, &trace_id, &mut sealed).unwrap();
    assert_eq!(report.format, LAR_TRANSACTION_FORMAT);
    assert_eq!(report.version, LAR_TRANSACTION_VERSION);
    assert_eq!(report.exchanges, 2);
    assert_eq!(report.stages, 6);
    assert_eq!(
        report.artifacts, 2,
        "shared request body must be emitted once"
    );
    assert_eq!(report.stream_indexes, 1);
    assert!(report.max_source_chunk_bytes <= 64 * 1024);
    assert_eq!(
        report.output_piece_bytes,
        LAR_TRANSACTION_ARTIFACT_PIECE_BYTES
    );

    let parsed = records(&sealed);
    let exchange = parsed
        .iter()
        .find(|value| value["type"] == "transaction_timeline")
        .unwrap();
    assert_eq!(exchange["metadata"]["method"], "POST");
    assert_eq!(exchange["metadata"]["path"], "/v1/messages?shape=exact");
    assert_eq!(
        exchange["supplements"][0]["exchange_content_id"],
        supplement_exchange_id
    );
    let stages = parsed
        .iter()
        .filter(|value| value["type"] == "stage")
        .collect::<Vec<_>>();
    assert_eq!(stages.len(), 6);
    assert_eq!(stages[1]["attempt_number"], 1);
    assert_eq!(stages[1]["error_class"], "rate_limit");
    assert_eq!(stages[1]["error_message"], "retry me");
    assert_eq!(stages[2]["attempt_number"], 2);
    assert_eq!(stages[2]["provider"], "provider");
    assert_eq!(stages[2]["routing_reason"], "failover");
    assert_eq!(stages[2]["content_id"], stages[3]["content_id"]);
    assert_ne!(stages[2]["occurrence_id"], stages[3]["occurrence_id"]);
    assert_eq!(
        stages[0]["request_body_content_id"],
        stages[1]["request_body_content_id"]
    );
    assert_eq!(
        stages[1]["request_body_content_id"],
        stages[2]["request_body_content_id"]
    );
    assert_eq!(
        stages[2]["request_body_content_id"],
        stages[3]["request_body_content_id"]
    );
    assert_eq!(stages[4]["status_code"], 200);
    assert_eq!(stages[4]["usage"]["reasoning_tokens"], 4);
    assert_eq!(stages[4]["cost_nanos"], 1234);
    assert_eq!(stages[4]["cost_currency"], "USD");
    assert!(stages[4]["trailers_content_id"].is_string());
    assert_eq!(stages[5]["content_id"], supplement_stage_id);
    assert_eq!(stages[5]["exchange_content_id"], supplement_exchange_id);
    assert_eq!(stages[5]["exchange_ordinal"], 1);
    assert_eq!(stages[5]["ordinal_within_exchange"], 0);
    assert_eq!(stages[5]["timeline_ordinal"], 5);
    assert_eq!(stages[5]["tool_id"], "transaction-tool");
    assert_eq!(stages[5]["tool_phase"], "end");
    assert!(stages
        .iter()
        .all(|stage| stage["content_id"] != unrelated_stage_id));

    let header = parsed
        .iter()
        .find(|value| {
            value["type"] == "header_block" && value["atoms"].as_array().unwrap().len() == 2
        })
        .unwrap();
    assert_eq!(header["atoms"][0]["name"], "x-duplicate");
    assert_eq!(header["atoms"][0]["value"], "one");
    assert_eq!(header["atoms"][1]["value"], "two");
    let trailer = parsed
        .iter()
        .find(|value| {
            value["type"] == "header_block" && value["atoms"].as_array().unwrap().len() == 1
        })
        .unwrap();
    assert_eq!(trailer["atoms"][0]["value"]["base64"], "AP8q");

    let stream = parsed
        .iter()
        .find(|value| value["type"] == "stream_index")
        .unwrap();
    assert_eq!(stream["content_id"], stream_id);
    assert_eq!(
        stream["observed_reads"][1]["delta_from_first_byte_ns"],
        20_000_000
    );
    assert_eq!(
        stream["parsed_frames"][1]["delta_from_first_byte_ns"],
        20_000_000
    );

    let mut reconstructed = std::collections::HashMap::<String, Vec<u8>>::new();
    for record in &parsed {
        if record["type"] == "artifact_bytes" {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(record["data_base64"].as_str().unwrap())
                .unwrap();
            assert!(bytes.len() <= LAR_TRANSACTION_ARTIFACT_PIECE_BYTES);
            reconstructed
                .entry(record["content_id"].as_str().unwrap().into())
                .or_default()
                .extend(bytes);
        }
    }
    assert_eq!(
        reconstructed
            .get(stages[0]["request_body_content_id"].as_str().unwrap())
            .unwrap(),
        &binary
    );
    assert_eq!(
        reconstructed
            .get(stages[4]["response_body_content_id"].as_str().unwrap())
            .unwrap(),
        &stream_body
    );

    let mut ordinary_child = Vec::new();
    let ordinary_report =
        write_archive_transaction(&mut reader, "ordinary-child", &mut ordinary_child).unwrap();
    assert_eq!(ordinary_report.exchanges, 1);
    assert_eq!(ordinary_report.stages, 1);
    let ordinary_records = records(&ordinary_child);
    let ordinary_stages = ordinary_records
        .iter()
        .filter(|record| record["type"] == "stage")
        .collect::<Vec<_>>();
    assert_eq!(ordinary_stages[0]["content_id"], unrelated_stage_id);

    let live_dir = dir.join("live");
    let live = Store::open(live_dir.clone()).unwrap();
    live.import_sealed_lar_archive(&archive, &LarStandaloneImportOptions::default())
        .unwrap();
    let mut catalog = Vec::new();
    let live_report = live
        .write_lar_transaction(&trace_id, &mut catalog)
        .unwrap()
        .unwrap();
    assert_eq!(live_report.artifact_bytes, report.artifact_bytes);
    assert_eq!(
        blake3::hash(&catalog),
        blake3::hash(&sealed),
        "live and sealed canonical transaction records differ"
    );
}

#[test]
fn synthesized_legacy_transaction_is_never_labeled_canonical() {
    let dir = tmpdir("legacy-label");
    let archive = dir.join("source.lar");
    let (trace_id, _, _, _, _, _, _) = write_fixture(&archive);
    let mut reader =
        ArchiveReader::open(fs::File::open(archive).unwrap(), Limits::default()).unwrap();
    let mut output = Vec::new();
    let report = write_synthesized_legacy_transaction(&mut reader, &trace_id, &mut output).unwrap();
    assert_eq!(report.fidelity, "synthesized_legacy");
    let parsed = records(&output);
    assert_eq!(parsed[0]["fidelity"], "synthesized_legacy");
    assert!(parsed[0]["limitations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|value| value.as_str().unwrap().contains("synthesized")));
}

#[test]
fn legacy_store_transaction_streams_large_binary_bodies_and_deduplicates_shared_bytes() {
    let dir = tmpdir("legacy-streaming");
    let store = Store::open(dir).unwrap();
    let mut shared = (0..(LAR_TRANSACTION_ARTIFACT_PIECE_BYTES * 5 + 37))
        .map(|index| (index % 253) as u8)
        .collect::<Vec<_>>();
    shared[0] = 0;
    shared[1] = 0xff;
    let response = b"legacy\0response\xff".to_vec();
    let request_path = store
        .write_body("legacy-transaction", "request.json", &shared)
        .unwrap();
    let upstream_path = store
        .write_body("legacy-transaction", "upstream-request.json", &shared)
        .unwrap();
    let response_path = store
        .write_body("legacy-transaction", "response.body", &response)
        .unwrap();
    store
        .insert_trace(&TraceRecord {
            id: "legacy-transaction".into(),
            ts_request_ms: 10,
            ts_response_ms: Some(20),
            session_id: Some("legacy-session".into()),
            method: Some("POST".into()),
            path: Some("/v1/legacy".into()),
            upstream_provider: Some("legacy-provider".into()),
            requested_model: Some("requested".into()),
            routed_model: Some("routed".into()),
            status: Some(201),
            usage: Usage {
                input_tokens: Some(10),
                output_tokens: Some(20),
                cached_input_tokens: Some(3),
                cache_creation_tokens: Some(4),
                reasoning_tokens: Some(5),
            },
            cost_usd: Some(0.25),
            req_body_path: Some(request_path),
            upstream_req_body_path: Some(upstream_path),
            resp_body_path: Some(response_path),
            req_headers_json: Some(r#"[["x-duplicate","one"],["x-duplicate","two"]]"#.into()),
            resp_headers_json: Some(r#"[["x-response","yes"]]"#.into()),
            ..Default::default()
        })
        .unwrap();

    let mut output = Vec::new();
    let report = store
        .write_legacy_transaction("legacy-transaction", &mut output)
        .unwrap()
        .unwrap();
    assert_eq!(report.fidelity, "synthesized_legacy");
    assert_eq!(report.artifacts, 2, "identical request bodies emit once");
    assert_eq!(
        report.artifact_bytes,
        (shared.len() + response.len()) as u64
    );
    let parsed = records(&output);
    assert_eq!(parsed[0]["fidelity"], "synthesized_legacy");
    let timeline = parsed
        .iter()
        .find(|record| record["type"] == "transaction_timeline")
        .unwrap();
    assert_eq!(timeline["metadata"]["method"], "POST");
    assert_eq!(timeline["metadata"]["path"], "/v1/legacy");
    let duplicate_headers = parsed
        .iter()
        .find(|record| {
            record["type"] == "header_block"
                && record["atoms"]
                    .as_array()
                    .is_some_and(|atoms| atoms.len() == 2)
        })
        .unwrap();
    assert_eq!(duplicate_headers["atoms"][0]["value"], "one");
    assert_eq!(duplicate_headers["atoms"][1]["value"], "two");

    let mut reconstructed = std::collections::HashMap::<String, Vec<u8>>::new();
    for record in &parsed {
        if record["type"] != "artifact_bytes" {
            continue;
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(record["data_base64"].as_str().unwrap())
            .unwrap();
        assert!(bytes.len() <= LAR_TRANSACTION_ARTIFACT_PIECE_BYTES);
        reconstructed
            .entry(record["content_id"].as_str().unwrap().into())
            .or_default()
            .extend(bytes);
    }
    assert!(reconstructed.values().any(|bytes| bytes == &shared));
    assert!(reconstructed.values().any(|bytes| bytes == &response));
}
