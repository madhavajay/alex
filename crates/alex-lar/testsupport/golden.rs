use alex_lar::{
    write_file_header, ArchiveWriter, ArtifactRangeRef, ChunkerConfig, ConversationEntry,
    ConversationEntryData, ConversationEntryKind, ConversationRole, Exchange, ExchangeData,
    ExchangeMetadataData, FileHeader, Generation, GenerationData, GenerationReason, HeaderAtom,
    HeaderBlock, HeaderFidelity, Limits, RecordFrame, RecordType, Stage, StageData, StageKind,
    StreamIndex, StreamRead, TurnView, TurnViewData, UnknownExchangeMetadataAttribute,
    REQUIRED_FEATURE_CONVERSATION_DAG, SEMANTIC_SCHEMA_V1,
};
use std::io::Cursor;

pub const FULL_BODY: &[u8] =
    b"event: message\ndata: {\"type\":\"message\",\"text\":\"golden body\"}\n\n";

pub fn v1_0_full_archive() -> Vec<u8> {
    let header = FileHeader::standalone(
        [
            0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d,
            0x1e, 0x1f,
        ],
        1_725_000_000_123_456_789,
        b"alex-lar-golden-v1.0".to_vec(),
    );
    let chunker = ChunkerConfig {
        min_size: 64,
        target_size: 128,
        max_size: 256,
    };
    let mut writer =
        ArchiveWriter::create(Cursor::new(Vec::new()), header, chunker, Limits::default()).unwrap();
    writer.enable_metadata_pages();
    let manifest = writer.append_body(FULL_BODY).unwrap();
    let headers = HeaderBlock::new(
        HeaderFidelity::Exact,
        vec![
            HeaderAtom {
                original_name: b"Content-Type".to_vec(),
                value: b"text/event-stream".to_vec(),
                flags: 0,
            },
            HeaderAtom {
                original_name: b"X-Golden-Version".to_vec(),
                value: b"1.0".to_vec(),
                flags: 0,
            },
        ],
    );
    let header_id = writer.append_header_block(headers).unwrap();
    let stream = StreamIndex::new(
        manifest,
        vec![StreamRead {
            byte_offset: 0,
            byte_length: FULL_BODY.len() as u64,
            delta_from_first_byte_ns: 0,
        }],
        Vec::new(),
    );
    let stream_id = writer.append_stream_index(stream).unwrap();
    let mut stage = StageData::new(StageKind::UpstreamResponse, 1_725_000_000_123_456_789);
    stage.attempt_number = Some(1);
    stage.response_headers_ref = Some(header_id);
    stage.response_body_manifest_ref = Some(manifest);
    stage.stream_index_ref = Some(stream_id);
    stage.provider = Some(b"golden-provider".to_vec());
    stage.routed_model = Some(b"golden-model-v1".to_vec());
    stage.status_code = Some(200);
    let stage_id = writer.append_stage(Stage::new(stage)).unwrap();
    let mut exchange = ExchangeData::new(
        b"golden-trace-v1.0",
        7,
        1_725_000_000_123_456_789,
        vec![stage_id],
    );
    exchange.session_id = Some(b"golden-session-v1.0".to_vec());
    exchange.run_id = Some(b"golden-run-v1.0".to_vec());
    writer.append_exchange(Exchange::new(exchange)).unwrap();
    writer.seal().unwrap();
    writer.into_inner().unwrap().into_inner()
}

pub fn v1_future_minor_optional_archive() -> Vec<u8> {
    let mut header = FileHeader::standalone(
        [0x23; 16],
        1_725_000_000_987_654_321,
        b"synthetic-future-minor".to_vec(),
    );
    header.container_minor = 23;
    header.optional_feature_bits = 0x8000_0000_0000_0042;
    let mut bytes = Vec::new();
    write_file_header(&mut bytes, &header).unwrap();
    for (record_type, schema_version, payload) in [
        (RecordType::Unknown(900), 1, b"optional-type".to_vec()),
        (
            RecordType::HeaderBlock,
            99,
            b"optional-future-header-schema".to_vec(),
        ),
    ] {
        RecordFrame {
            record_type,
            schema_version,
            flags: 0,
            payload,
            offset: bytes.len() as u64,
        }
        .write(&mut bytes)
        .unwrap();
    }
    bytes
}

pub fn v1_conversation_dag_archive() -> Vec<u8> {
    let mut header = FileHeader::standalone(
        [0x31; 16],
        1_725_000_001_123_456_789,
        b"alex-lar-golden-conversation-v1".to_vec(),
    );
    header.required_feature_bits |= REQUIRED_FEATURE_CONVERSATION_DAG;
    let mut writer = ArchiveWriter::create(
        Cursor::new(Vec::new()),
        header,
        ChunkerConfig {
            min_size: 64,
            target_size: 128,
            max_size: 256,
        },
        Limits::default(),
    )
    .unwrap();
    writer.enable_metadata_pages();

    let user_body = b"{\"role\":\"user\",\"content\":\"golden DAG request\"}";
    let assistant_body = b"{\"role\":\"assistant\",\"content\":\"golden DAG response\"}";
    let user_manifest = writer.append_body(user_body).unwrap();
    let assistant_manifest = writer.append_body(assistant_body).unwrap();
    let user = writer
        .append_conversation_entry(ConversationEntry::new(ConversationEntryData {
            semantic_schema: SEMANTIC_SCHEMA_V1,
            role: ConversationRole::User,
            kind: ConversationEntryKind::Message,
            raw_ranges: vec![ArtifactRangeRef {
                manifest_id: user_manifest,
                byte_offset: 0,
                byte_length: user_body.len() as u64,
            }],
            name: None,
            tool_call_id: None,
        }))
        .unwrap();
    let assistant = writer
        .append_conversation_entry(ConversationEntry::new(ConversationEntryData {
            semantic_schema: SEMANTIC_SCHEMA_V1,
            role: ConversationRole::Assistant,
            kind: ConversationEntryKind::Message,
            raw_ranges: vec![ArtifactRangeRef {
                manifest_id: assistant_manifest,
                byte_offset: 0,
                byte_length: assistant_body.len() as u64,
            }],
            name: None,
            tool_call_id: None,
        }))
        .unwrap();
    let generation = writer
        .append_generation(Generation::new(GenerationData {
            parent_generation_id: None,
            entries: vec![user],
            reason: GenerationReason::Initial,
        }))
        .unwrap();
    let trace_id = b"golden-dag-trace-v1";
    writer
        .append_exchange(Exchange::new(ExchangeData::new(
            trace_id,
            1,
            1_725_000_001_123_456_789,
            Vec::new(),
        )))
        .unwrap();
    writer
        .append_turn_view(TurnView::new(TurnViewData {
            trace_id: trace_id.to_vec(),
            generation_id: generation,
            upto_index: 0,
            response_entry_refs: vec![assistant],
        }))
        .unwrap();
    writer.seal().unwrap();
    writer.into_inner().unwrap().into_inner()
}

pub fn v1_exchange_metadata_data() -> ExchangeMetadataData {
    ExchangeMetadataData {
        ts_request_ms: Some(-123),
        ts_response_ms: Some(456),
        harness: Some(b"pi".to_vec()),
        client_format: Some(b"openai-chat".to_vec()),
        upstream_format: Some(b"anthropic-messages".to_vec()),
        method: Some(b"POST".to_vec()),
        path: Some(b"/v1/chat/completions?raw=1".to_vec()),
        streamed: Some(false),
        status: Some(70_000),
        cost_usd_bits: Some((-0.0f64).to_bits()),
        billing_bucket: Some(b"subscription".to_vec()),
        error_kind: Some(b"quota".to_vec()),
        error_code: Some(b"access_terminated_error".to_vec()),
        substituted: true,
        original_model: Some(b"model-a".to_vec()),
        served_model: Some(b"model-b".to_vec()),
        substitution_reason: Some(b"quota".to_vec()),
        injected: true,
        fixture_name: Some(b"onboarding".to_vec()),
        attempts_json: Some(br#"[{"account":"one"}]"#.to_vec()),
        original_account_id: Some(b"account-one".to_vec()),
        served_account_id: Some(b"account-two".to_vec()),
        subscription_identity: Some(b"sub-identity".to_vec()),
        via_dario: true,
        dario_generation: Some(b"generation-7".to_vec()),
        tags_json: Some(br#"{"run":"bench"}"#.to_vec()),
        client_ip: Some(b"127.0.0.1".to_vec()),
        key_fingerprint: Some(b"sha256:abc".to_vec()),
        reasoning_effort: Some(b"high".to_vec()),
        thinking_budget: Some(-1),
        input_tokens: Some(-2),
        cached_input_tokens: Some(0),
        cache_creation_tokens: Some(17),
        output_tokens: Some(18),
        reasoning_tokens: Some(19),
        unknown_attributes: vec![UnknownExchangeMetadataAttribute {
            key: b"x.future.attribute".to_vec(),
            value: b"preserve me".to_vec(),
        }],
    }
}

pub fn v1_exchange_metadata_archive() -> Vec<u8> {
    let header = FileHeader::standalone(
        [0x4d; 16],
        1_725_000_002_123_456_789,
        b"alex-lar-golden-exchange-metadata-v1".to_vec(),
    );
    let mut writer = ArchiveWriter::create(
        Cursor::new(Vec::new()),
        header,
        ChunkerConfig {
            min_size: 64,
            target_size: 128,
            max_size: 256,
        },
        Limits::default(),
    )
    .unwrap();
    writer.enable_metadata_pages();
    let mut exchange = ExchangeData::new(
        b"golden-exchange-metadata-v1",
        1,
        1_725_000_002_123_456_789,
        Vec::new(),
    );
    exchange.session_id = Some(b"golden-exchange-metadata-session-v1".to_vec());
    writer
        .append_exchange_with_metadata(Exchange::new(exchange), v1_exchange_metadata_data())
        .unwrap();
    writer.seal().unwrap();
    writer.into_inner().unwrap().into_inner()
}
