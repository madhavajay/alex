use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use alex_lar::{
    ArchiveReader, ArchiveWriter, ArtifactRangeRef, ChunkerConfig, ConversationEntry,
    ConversationEntryData, ConversationEntryKind, ConversationRole, Exchange, ExchangeData,
    ExchangeMetadataData, FileHeader, FileRole, Generation, GenerationData, GenerationReason,
    HeaderAtom, HeaderBlock, HeaderFidelity, Limits, Stage, StageData, StageKind, StreamIndex,
    StreamRead, TurnView, TurnViewData, REQUIRED_FEATURE_CONVERSATION_DAG, SEMANTIC_SCHEMA_V1,
};
use alex_store::{
    LarConversationEntryCapture, LarConversationEntryKind, LarConversationGenerationEvent,
    LarConversationRawRange, LarConversationRole, LarConversationSemantics,
    LarConversationTurnCapture, LarRepackConfig, LarStageContentOptions,
    LarStandaloneImportOptions, LarStreamReplayPageOptions, Store,
};
use rusqlite::{params, Connection};

static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn tmpdir(name: &str) -> PathBuf {
    let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "alex-lar-repack-combined-{name}-{}-{sequence}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).unwrap();
    path
}

#[derive(Clone)]
struct CombinedFixture {
    request: Vec<u8>,
    response: Vec<u8>,
    request_manifest: String,
    response_manifest: String,
    response_stage: String,
    exchange: String,
    user_entry: String,
    assistant_entry: String,
    generation: String,
    turn_view: String,
}

fn garbage_bytes() -> Vec<u8> {
    let mut state = 0x15_07_20_26_c0_ff_ee_u64;
    (0..192 * 1024)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        })
        .collect()
}

fn write_combined_archive(path: &Path, body_pack: bool) -> CombinedFixture {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    let file = OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .open(path)
        .unwrap();
    let file_uuid = [0x71; 16];
    let mut header = if body_pack {
        FileHeader::body_pack(file_uuid, 1_000_000, b"combined-repack-safety".to_vec())
    } else {
        FileHeader::standalone(file_uuid, 1_000_000, b"combined-repack-safety".to_vec())
    };
    if body_pack {
        header.required_feature_bits |= REQUIRED_FEATURE_CONVERSATION_DAG;
    }
    let mut writer = ArchiveWriter::create(
        file,
        header,
        ChunkerConfig {
            min_size: 32 * 1024,
            target_size: 32 * 1024,
            max_size: 32 * 1024,
        },
        Limits::default(),
    )
    .unwrap();

    let request = br#"{"messages":[{"role":"user","content":"hello"}]}"#.to_vec();
    let response = b"data: one\n\ndata: two\n\n".to_vec();
    let request_manifest = writer.append_body(&request).unwrap();
    let response_manifest = writer.append_body(&response).unwrap();
    // This manifest has no trace, stage, or conversation root. It makes the
    // pack exceed the normal repack thresholds so metadata safety is the only
    // reason it remains ineligible.
    writer.append_body(&garbage_bytes()).unwrap();

    let headers = HeaderBlock::new(
        HeaderFidelity::Exact,
        vec![HeaderAtom {
            original_name: b"Content-Type".to_vec(),
            value: b"text/event-stream".to_vec(),
            flags: 0,
        }],
    );
    let headers_id = writer.append_header_block(headers).unwrap();
    let stream_id = writer
        .append_stream_index(StreamIndex::new(
            response_manifest,
            vec![
                StreamRead {
                    byte_offset: 0,
                    byte_length: 11,
                    delta_from_first_byte_ns: 0,
                },
                StreamRead {
                    byte_offset: 11,
                    byte_length: response.len() as u64 - 11,
                    delta_from_first_byte_ns: 5_000_000,
                },
            ],
            Vec::new(),
        ))
        .unwrap();

    let mut request_stage = StageData::new(StageKind::ClientRequest, 1_000_000);
    request_stage.request_headers_ref = Some(headers_id);
    request_stage.request_body_manifest_ref = Some(request_manifest);
    let request_stage = writer.append_stage(Stage::new(request_stage)).unwrap();
    let mut response_stage = StageData::new(StageKind::UpstreamResponse, 2_000_000);
    response_stage.attempt_number = Some(1);
    response_stage.response_headers_ref = Some(headers_id);
    response_stage.response_body_manifest_ref = Some(response_manifest);
    response_stage.stream_index_ref = Some(stream_id);
    response_stage.status_code = Some(200);
    let response_stage = writer.append_stage(Stage::new(response_stage)).unwrap();

    let mut exchange = ExchangeData::new(
        b"trace-combined".to_vec(),
        1,
        1_000_000,
        vec![request_stage, response_stage],
    );
    exchange.session_id = Some(b"session-combined".to_vec());
    let exchange = writer
        .append_exchange_with_metadata(
            Exchange::new(exchange),
            ExchangeMetadataData {
                ts_request_ms: Some(1),
                ts_response_ms: Some(2),
                harness: Some(b"pi".to_vec()),
                client_format: Some(b"openai-chat".to_vec()),
                upstream_format: Some(b"openai-chat".to_vec()),
                method: Some(b"POST".to_vec()),
                path: Some(b"/v1/chat/completions".to_vec()),
                streamed: Some(true),
                status: Some(200),
                ..ExchangeMetadataData::default()
            },
        )
        .unwrap();

    let user = ConversationEntry::new(ConversationEntryData {
        semantic_schema: SEMANTIC_SCHEMA_V1,
        role: ConversationRole::User,
        kind: ConversationEntryKind::Message,
        raw_ranges: vec![ArtifactRangeRef {
            manifest_id: request_manifest,
            byte_offset: 0,
            byte_length: request.len() as u64,
        }],
        name: None,
        tool_call_id: None,
    });
    let assistant = ConversationEntry::new(ConversationEntryData {
        semantic_schema: SEMANTIC_SCHEMA_V1,
        role: ConversationRole::Assistant,
        kind: ConversationEntryKind::Message,
        raw_ranges: vec![ArtifactRangeRef {
            manifest_id: response_manifest,
            byte_offset: 0,
            byte_length: response.len() as u64,
        }],
        name: None,
        tool_call_id: None,
    });
    let generation = Generation::new(GenerationData {
        parent_generation_id: None,
        entries: vec![user.id],
        reason: GenerationReason::Initial,
    });
    let turn = TurnView::new(TurnViewData {
        trace_id: b"trace-combined".to_vec(),
        generation_id: generation.id,
        upto_index: 0,
        response_entry_refs: vec![assistant.id],
    });
    if body_pack {
        writer.append_conversation_entry(user.clone()).unwrap();
        writer.append_conversation_entry(assistant.clone()).unwrap();
        writer.append_generation(generation.clone()).unwrap();
        writer.append_turn_view(turn.clone()).unwrap();
    }
    writer.seal().unwrap();
    writer.get_ref().sync_all().unwrap();

    CombinedFixture {
        request,
        response,
        request_manifest: request_manifest.to_string(),
        response_manifest: response_manifest.to_string(),
        response_stage: response_stage.to_string(),
        exchange: exchange.to_string(),
        user_entry: user.id.to_string(),
        assistant_entry: assistant.id.to_string(),
        generation: generation.id.to_string(),
        turn_view: turn.id.to_string(),
    }
}

#[test]
fn repack_preserves_a_complete_combined_canonical_graph() {
    let root = tmpdir("all-records");
    let store = Store::open(root.clone()).unwrap();
    let seed_path = root.join("seed-standalone.lar");
    let body_path = root.join("lar/combined/body-combined.lar");
    let seed = write_combined_archive(&seed_path, false);
    let fixture = write_combined_archive(&body_path, true);
    assert_eq!(fixture.request_manifest, seed.request_manifest);
    assert_eq!(fixture.response_manifest, seed.response_manifest);
    assert_eq!(fixture.response_stage, seed.response_stage);
    assert_eq!(fixture.exchange, seed.exchange);

    let imported = store
        .import_sealed_lar_archive(&seed_path, &LarStandaloneImportOptions::default())
        .unwrap();
    let catalog_path = root.join("alexandria.sqlite3");
    let conn = Connection::open(&catalog_path).unwrap();
    conn.execute(
        "UPDATE lar_files
            SET role='body-pack', path=?2, required_feature_bits=?3, size_bytes=?4
          WHERE file_uuid=?1",
        params![
            imported.file_uuid,
            body_path.to_string_lossy(),
            REQUIRED_FEATURE_CONVERSATION_DAG,
            fs::metadata(&body_path).unwrap().len(),
        ],
    )
    .unwrap();
    conn.execute(
        "UPDATE lar_manifests SET state='unreachable'
          WHERE manifest_id NOT IN (?1, ?2)",
        params![fixture.request_manifest, fixture.response_manifest],
    )
    .unwrap();
    conn.execute(
        "DELETE FROM lar_file_identities WHERE file_uuid=?1",
        [&imported.file_uuid],
    )
    .unwrap();
    drop(conn);

    let user_id = store
        .register_lar_conversation_entry(&LarConversationEntryCapture {
            semantics: LarConversationSemantics::Known {
                source_format: "openai-chat".into(),
                role: LarConversationRole::User,
                kind: LarConversationEntryKind::Message,
                name: None,
                tool_call_id: None,
            },
            raw_ranges: vec![LarConversationRawRange {
                manifest_id: fixture.request_manifest.clone(),
                byte_offset: 0,
                byte_length: fixture.request.len() as u64,
            }],
        })
        .unwrap();
    let assistant_id = store
        .register_lar_conversation_entry(&LarConversationEntryCapture {
            semantics: LarConversationSemantics::Known {
                source_format: "openai-chat".into(),
                role: LarConversationRole::Assistant,
                kind: LarConversationEntryKind::Message,
                name: None,
                tool_call_id: None,
            },
            raw_ranges: vec![LarConversationRawRange {
                manifest_id: fixture.response_manifest.clone(),
                byte_offset: 0,
                byte_length: fixture.response.len() as u64,
            }],
        })
        .unwrap();
    assert_eq!(user_id, fixture.user_entry);
    assert_eq!(assistant_id, fixture.assistant_entry);
    let turn = store
        .record_lar_conversation_turn(&LarConversationTurnCapture {
            trace_id: "trace-combined".into(),
            session_id: "session-combined".into(),
            event: LarConversationGenerationEvent::Initial,
            generation_entry_ids: vec![user_id],
            upto_index: 0,
            response_entry_ids: vec![assistant_id],
        })
        .unwrap();
    assert_eq!(turn.generation_id, fixture.generation);
    assert_eq!(turn.turn_view_id, fixture.turn_view);

    let before = ArchiveReader::open(File::open(&body_path).unwrap(), Limits::default()).unwrap();
    assert_eq!(before.header().file_role, FileRole::BodyPack);
    assert!(before.manifest_count() >= 3);
    assert_eq!(before.header_block_count(), 1);
    assert_eq!(before.stream_index_count(), 1);
    assert_eq!(before.stage_count(), 2);
    assert_eq!(before.exchange_count(), 1);
    assert_eq!(before.conversation_entry_count(), 2);
    assert_eq!(before.generation_count(), 1);
    assert_eq!(before.turn_view_count(), 1);
    let exchange = before
        .exchange_by_trace(b"trace-combined")
        .expect("combined exchange remains indexed");
    assert_eq!(exchange.id.to_string(), fixture.exchange);
    assert_eq!(
        before
            .exchange_metadata(&exchange.id)
            .unwrap()
            .data
            .harness
            .as_deref(),
        Some(b"pi".as_slice())
    );
    drop(before);

    let config = LarRepackConfig {
        min_garbage_bytes: 1,
        min_garbage_ratio: 0.001,
    };
    let conn = Connection::open(&catalog_path).unwrap();
    let (total_chunks, reachable_chunks): (i64, i64) = conn
        .query_row(
            "WITH reachable_manifests(manifest_id) AS (
                 SELECT manifest_id FROM lar_trace_artifacts
                  WHERE validation_state='validated' AND manifest_id IS NOT NULL
                 UNION
                 SELECT request_body_manifest_ref FROM lar_stage_records
                  WHERE request_body_manifest_ref IS NOT NULL
                 UNION
                 SELECT response_body_manifest_ref FROM lar_stage_records
                  WHERE response_body_manifest_ref IS NOT NULL
             ), reachable_chunks(chunk_hash) AS (
                 SELECT DISTINCT mc.chunk_hash FROM lar_manifest_chunks mc
                   JOIN reachable_manifests rm ON rm.manifest_id=mc.manifest_id
             )
             SELECT
                 (SELECT COUNT(*) FROM lar_chunks WHERE file_uuid=?1),
                 (SELECT COUNT(*) FROM lar_chunks c JOIN reachable_chunks r
                    ON r.chunk_hash=c.chunk_hash WHERE c.file_uuid=?1)",
            [&imported.file_uuid],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert!(
        total_chunks > reachable_chunks,
        "fixture must cross the repack garbage threshold"
    );
    drop(conn);
    let candidates = store.plan_lar_repacks(&config).unwrap();
    assert_eq!(candidates.len(), 1);
    let report = store.run_lar_repack(&config, 10).unwrap().unwrap();
    assert_eq!(report.state, "complete");
    assert!(report.destination_path.is_file());
    assert!(report.quarantine_path.is_file());
    let mut replacement = ArchiveReader::open(
        File::open(&report.destination_path).unwrap(),
        Limits::default(),
    )
    .unwrap();
    assert_eq!(replacement.manifest_count(), 2);
    assert_eq!(replacement.header_block_count(), 1);
    assert_eq!(replacement.stream_index_count(), 1);
    assert_eq!(replacement.stage_count(), 2);
    assert_eq!(replacement.exchange_count(), 1);
    assert_eq!(replacement.conversation_entry_count(), 2);
    assert_eq!(replacement.generation_count(), 1);
    assert_eq!(replacement.turn_view_count(), 1);
    assert_eq!(
        replacement
            .read_body(&fixture.request_manifest.parse().unwrap())
            .unwrap(),
        fixture.request
    );

    assert_eq!(
        store
            .read_lar_or_legacy_artifact("trace", "trace-combined", "client_request", None,)
            .unwrap()
            .unwrap(),
        fixture.request
    );
    let stages = store
        .lar_stage_content_page("trace-combined", &LarStageContentOptions::default())
        .unwrap();
    assert_eq!(stages.total_stages, 2);
    assert_eq!(stages.header_blocks.len(), 1);
    assert_eq!(stages.bodies.len(), 2);
    assert!(stages
        .header_blocks
        .iter()
        .all(|block| block.state == "available"));
    assert!(stages.bodies.iter().all(|body| body.state == "available"));

    let replay = store
        .lar_stream_replay_page(
            "trace-combined",
            &fixture.response_stage,
            &LarStreamReplayPageOptions::default(),
        )
        .unwrap();
    assert_eq!(
        replay
            .events
            .iter()
            .flat_map(|event| event.bytes.iter().copied())
            .collect::<Vec<_>>(),
        fixture.response
    );
    let conversation = store
        .lar_conversation_events_page("session-combined", None, 10)
        .unwrap();
    assert_eq!(conversation.total_count, 1);
    assert_eq!(
        conversation.events[0].entries[0].entry_id,
        fixture.user_entry
    );
    assert_eq!(
        conversation.events[0].response_entries[0].entry_id,
        fixture.assistant_entry
    );

    let conn = Connection::open(catalog_path).unwrap();
    let source_state: String = conn
        .query_row(
            "SELECT state FROM lar_files WHERE file_uuid=?1",
            [&imported.file_uuid],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(source_state, "retired");
    assert!(!body_path.exists());
    let refs_to_retired: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM (
                 SELECT c.file_uuid FROM lar_chunks c JOIN lar_files f USING(file_uuid)
                  WHERE f.state='retired' AND c.state='ready'
                 UNION ALL
                 SELECT m.file_uuid FROM lar_manifests m JOIN lar_files f USING(file_uuid)
                  WHERE f.state='retired' AND m.state='ready'
                 UNION ALL
                 SELECT h.file_uuid FROM lar_header_blocks h JOIN lar_files f USING(file_uuid)
                  WHERE f.state='retired'
                 UNION ALL
                 SELECT s.file_uuid FROM lar_stage_records s JOIN lar_files f USING(file_uuid)
                  WHERE f.state='retired'
                 UNION ALL
                 SELECT i.destination_file_uuid FROM lar_migration_items i
                   JOIN lar_files f ON f.file_uuid=i.destination_file_uuid
                  WHERE f.state='retired'
             )",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(refs_to_retired, 0);
}
