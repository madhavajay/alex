use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use alex_core::TraceRecord;
use alex_store::{
    LarBodyArtifact, LarBodyStoreConfig, LarBodyStoreMode, LarConversationEntryCapture,
    LarConversationEntryKind, LarConversationEvidence, LarConversationEvidenceSource,
    LarConversationGenerationEvent, LarConversationRawRange, LarConversationRole,
    LarConversationSemantics, LarConversationTurnCapture, LarLegacyImportOptions, Store,
};
use rusqlite::Connection;

static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn tmpdir(name: &str) -> PathBuf {
    let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "alex-lar-conversation-{name}-{}-{sequence}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn config() -> LarBodyStoreConfig {
    LarBodyStoreConfig {
        mode: LarBodyStoreMode::LarWithFallback,
        ..Default::default()
    }
}

fn insert_trace(store: &Store, id: &str, session_id: &str, timestamp: i64) {
    store
        .insert_trace(&TraceRecord {
            id: id.into(),
            session_id: Some(session_id.into()),
            ts_request_ms: timestamp,
            status: Some(200),
            ..Default::default()
        })
        .unwrap();
}

fn range(manifest_id: &str, body: &[u8], value: &[u8]) -> LarConversationRawRange {
    let offset = body
        .windows(value.len())
        .position(|window| window == value)
        .unwrap();
    LarConversationRawRange {
        manifest_id: manifest_id.into(),
        byte_offset: offset as u64,
        byte_length: value.len() as u64,
    }
}

fn known(
    source_format: &str,
    role: LarConversationRole,
    kind: LarConversationEntryKind,
    raw_range: LarConversationRawRange,
) -> LarConversationEntryCapture {
    LarConversationEntryCapture {
        semantics: LarConversationSemantics::Known {
            source_format: source_format.into(),
            role,
            kind,
            name: None,
            tool_call_id: None,
        },
        raw_ranges: vec![raw_range],
    }
}

fn evidence(kind: &str, id: &str) -> LarConversationEvidence {
    LarConversationEvidence {
        source: LarConversationEvidenceSource::Capture,
        kind: kind.into(),
        id: id.into(),
    }
}

fn physical_snapshot(root: &std::path::Path) -> (i64, i64, i64) {
    let conn = Connection::open(root.join("alexandria.sqlite3")).unwrap();
    (
        conn.query_row("SELECT COUNT(*) FROM lar_manifests", [], |row| row.get(0))
            .unwrap(),
        conn.query_row("SELECT COUNT(*) FROM lar_chunks", [], |row| row.get(0))
            .unwrap(),
        conn.query_row(
            "SELECT COALESCE(SUM(size_bytes), 0) FROM lar_files",
            [],
            |row| row.get(0),
        )
        .unwrap(),
    )
}

#[test]
fn bodies_only_retention_drops_old_turn_ownership_but_keeps_shared_newer_entries() {
    let root = tmpdir("retention-shared-entry");
    let store = Store::open_with_lar_body_store(root.clone(), config()).unwrap();
    let session = "retention-session";
    insert_trace(&store, "old-turn", session, 100);
    insert_trace(&store, "new-turn", session, 1_000);

    let shared_body = b"shared conversation prefix";
    let shared_manifest = store
        .write_body_artifact(
            &LarBodyArtifact::trace("old-turn", "client_request"),
            "request.json",
            shared_body,
        )
        .unwrap()
        .manifest_id
        .unwrap();
    let old_response_body = b"old response only";
    let old_response_manifest = store
        .write_body_artifact(
            &LarBodyArtifact::trace("old-turn", "client_response"),
            "response.json",
            old_response_body,
        )
        .unwrap()
        .manifest_id
        .unwrap();
    let shared_entry = store
        .register_lar_conversation_entry(&known(
            "openai-chat",
            LarConversationRole::User,
            LarConversationEntryKind::Message,
            range(&shared_manifest, shared_body, shared_body),
        ))
        .unwrap();
    let old_response_entry = store
        .register_lar_conversation_entry(&known(
            "openai-chat",
            LarConversationRole::Assistant,
            LarConversationEntryKind::Message,
            range(&old_response_manifest, old_response_body, old_response_body),
        ))
        .unwrap();

    let old = store
        .record_lar_conversation_turn(&LarConversationTurnCapture {
            trace_id: "old-turn".into(),
            session_id: session.into(),
            event: LarConversationGenerationEvent::Initial,
            generation_entry_ids: vec![shared_entry.clone()],
            upto_index: 0,
            response_entry_ids: vec![old_response_entry.clone()],
        })
        .unwrap();
    let new = store
        .record_lar_conversation_turn(&LarConversationTurnCapture {
            trace_id: "new-turn".into(),
            session_id: session.into(),
            event: LarConversationGenerationEvent::Initial,
            generation_entry_ids: vec![shared_entry.clone()],
            upto_index: 0,
            response_entry_ids: Vec::new(),
        })
        .unwrap();
    assert_eq!(old.generation_id, new.generation_id);

    store.prune(500, true, false).unwrap();

    let page = store
        .lar_conversation_events_page(session, None, 10)
        .unwrap();
    assert_eq!(page.total_count, 1);
    assert_eq!(page.events[0].trace_id, "new-turn");
    assert_eq!(page.events[0].entries[0].entry_id, shared_entry);
    assert_eq!(
        store.read_lar_manifest_body(&shared_manifest).unwrap(),
        shared_body
    );

    let gc = store.plan_lar_gc().unwrap();
    assert_eq!(gc.reachable_manifests, 1);
    assert_eq!(gc.unreachable_manifests, 1);
    let conn = Connection::open(root.join("alexandria.sqlite3")).unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM lar_conversation_entries WHERE entry_id=?1",
            [&old_response_entry],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        0
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM lar_conversation_entry_ranges WHERE manifest_id=?1",
            [&old_response_manifest],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        0
    );
}

#[test]
fn explicit_generation_events_are_paged_and_share_exact_body_ranges() {
    let root = tmpdir("events");
    let store = Store::open_with_lar_body_store(root.clone(), config()).unwrap();
    let session = "conversation-session";
    for (index, trace_id) in ["initial", "append", "branch", "compact", "mutate"]
        .into_iter()
        .enumerate()
    {
        insert_trace(&store, trace_id, session, 1_000 + index as i64);
    }

    let body = b"wire:[USER-1][ASSISTANT-1][USER-2][ASSISTANT-2][BRANCH][SUMMARY][AFTER][MUTATED][\x00\xffRAW]";
    let written = store
        .write_body_artifact(
            &LarBodyArtifact::trace("body-source", "client_request"),
            "request.json",
            body,
        )
        .unwrap();
    let manifest_id = written.manifest_id.unwrap();
    let physical_before = physical_snapshot(&root);

    let user_1_capture = known(
        "anthropic-messages-v1",
        LarConversationRole::User,
        LarConversationEntryKind::Message,
        range(&manifest_id, body, b"[USER-1]"),
    );
    let user_1 = store
        .register_lar_conversation_entry(&user_1_capture)
        .unwrap();
    assert_eq!(
        store
            .register_lar_conversation_entry(&user_1_capture)
            .unwrap(),
        user_1,
        "the same body range and semantics must retain its canonical entry ID"
    );
    let assistant_1 = store
        .register_lar_conversation_entry(&known(
            "anthropic-messages-v1",
            LarConversationRole::Assistant,
            LarConversationEntryKind::Message,
            range(&manifest_id, body, b"[ASSISTANT-1]"),
        ))
        .unwrap();
    let user_2 = store
        .register_lar_conversation_entry(&known(
            "anthropic-messages-v1",
            LarConversationRole::User,
            LarConversationEntryKind::Message,
            range(&manifest_id, body, b"[USER-2]"),
        ))
        .unwrap();
    let assistant_2 = store
        .register_lar_conversation_entry(&known(
            "anthropic-messages-v1",
            LarConversationRole::Assistant,
            LarConversationEntryKind::Message,
            range(&manifest_id, body, b"[ASSISTANT-2]"),
        ))
        .unwrap();
    let branch_entry = store
        .register_lar_conversation_entry(&known(
            "anthropic-messages-v1",
            LarConversationRole::User,
            LarConversationEntryKind::Message,
            range(&manifest_id, body, b"[BRANCH]"),
        ))
        .unwrap();
    let summary = store
        .register_lar_conversation_entry(&known(
            "anthropic-messages-v1",
            LarConversationRole::Assistant,
            LarConversationEntryKind::Summary,
            range(&manifest_id, body, b"[SUMMARY]"),
        ))
        .unwrap();
    let after = store
        .register_lar_conversation_entry(&known(
            "anthropic-messages-v1",
            LarConversationRole::User,
            LarConversationEntryKind::Message,
            range(&manifest_id, body, b"[AFTER]"),
        ))
        .unwrap();
    let mutated = store
        .register_lar_conversation_entry(&known(
            "anthropic-messages-v1",
            LarConversationRole::System,
            LarConversationEntryKind::Message,
            range(&manifest_id, body, b"[MUTATED]"),
        ))
        .unwrap();
    let raw_range = range(&manifest_id, body, b"[\x00\xffRAW]");
    let raw = store
        .register_lar_conversation_entry(&LarConversationEntryCapture {
            semantics: LarConversationSemantics::RawOnly {
                source_format: "vendor/unknown-binary".into(),
            },
            raw_ranges: vec![raw_range.clone()],
        })
        .unwrap();

    let initial_capture = LarConversationTurnCapture {
        trace_id: "initial".into(),
        session_id: session.into(),
        event: LarConversationGenerationEvent::Initial,
        generation_entry_ids: vec![user_1.clone()],
        upto_index: 0,
        response_entry_ids: vec![assistant_1.clone()],
    };
    let initial = store
        .record_lar_conversation_turn(&initial_capture)
        .unwrap();
    assert_eq!(
        store
            .record_lar_conversation_turn(&initial_capture)
            .unwrap(),
        initial,
        "recording an identical turn must be idempotent and retain both IDs"
    );

    let append = store
        .record_lar_conversation_turn(&LarConversationTurnCapture {
            trace_id: "append".into(),
            session_id: session.into(),
            event: LarConversationGenerationEvent::Append {
                parent_generation_id: initial.generation_id.clone(),
            },
            generation_entry_ids: vec![user_1.clone(), assistant_1.clone(), user_2.clone()],
            upto_index: 2,
            response_entry_ids: vec![assistant_2],
        })
        .unwrap();

    store
        .record_lar_conversation_turn(&LarConversationTurnCapture {
            trace_id: "branch".into(),
            session_id: session.into(),
            event: LarConversationGenerationEvent::Branch {
                parent_generation_id: append.generation_id.clone(),
                evidence: evidence("harness_branch", "branch-17"),
            },
            generation_entry_ids: vec![user_1.clone(), assistant_1.clone(), branch_entry],
            upto_index: 2,
            response_entry_ids: vec![],
        })
        .unwrap();

    let missing_evidence = store
        .record_lar_conversation_turn(&LarConversationTurnCapture {
            trace_id: "compact".into(),
            session_id: session.into(),
            event: LarConversationGenerationEvent::Compaction {
                parent_generation_id: append.generation_id.clone(),
                evidence: evidence("", "compact-42"),
            },
            generation_entry_ids: vec![summary.clone(), after.clone()],
            upto_index: 1,
            response_entry_ids: vec![],
        })
        .unwrap_err();
    assert!(missing_evidence
        .to_string()
        .contains("must include kind and ID"));
    store
        .record_lar_conversation_turn(&LarConversationTurnCapture {
            trace_id: "compact".into(),
            session_id: session.into(),
            event: LarConversationGenerationEvent::Compaction {
                parent_generation_id: append.generation_id.clone(),
                evidence: evidence("context_compaction", "compact-42"),
            },
            generation_entry_ids: vec![summary, after],
            upto_index: 1,
            response_entry_ids: vec![],
        })
        .unwrap();

    let inferred_mutation = store
        .record_lar_conversation_turn(&LarConversationTurnCapture {
            trace_id: "mutate".into(),
            session_id: session.into(),
            event: LarConversationGenerationEvent::Append {
                parent_generation_id: append.generation_id.clone(),
            },
            generation_entry_ids: vec![user_1.clone(), mutated.clone(), raw.clone()],
            upto_index: 2,
            response_entry_ids: vec![],
        })
        .unwrap_err();
    assert!(
        inferred_mutation
            .to_string()
            .contains("must retain its parent's exact entry prefix"),
        "a changed prefix must not be guessed to be an append or mutation"
    );
    store
        .record_lar_conversation_turn(&LarConversationTurnCapture {
            trace_id: "mutate".into(),
            session_id: session.into(),
            event: LarConversationGenerationEvent::Mutation {
                parent_generation_id: append.generation_id.clone(),
                evidence: evidence("captured_history_rewrite", "rewrite-9"),
            },
            generation_entry_ids: vec![user_1, mutated, raw.clone()],
            upto_index: 2,
            response_entry_ids: vec![],
        })
        .unwrap();

    assert_eq!(
        physical_snapshot(&root),
        physical_before,
        "the graph catalog must add references without copying body manifests or chunks"
    );
    assert_eq!(store.read_lar_manifest_body(&manifest_id).unwrap(), body);

    let first = store
        .lar_conversation_events_page(session, None, 2)
        .unwrap();
    assert_eq!(first.total_count, 5);
    assert_eq!(first.events.len(), 2);
    assert!(first.has_more_after);
    assert_eq!(first.events[0].reason, "initial");
    assert_eq!(first.events[1].reason, "append");
    assert_eq!(
        first.events[1].parent_generation_id,
        Some(initial.generation_id)
    );

    let second = store
        .lar_conversation_events_page(session, first.next_after, 10)
        .unwrap();
    assert_eq!(
        second
            .events
            .iter()
            .map(|event| event.reason.as_str())
            .collect::<Vec<_>>(),
        ["branch", "compaction", "mutation"]
    );
    assert!(!second.has_more_after);
    assert_eq!(
        second.events[0].evidence.as_ref().unwrap().kind,
        "harness_branch"
    );
    assert_eq!(second.events[1].evidence.as_ref().unwrap().id, "compact-42");
    let raw_view = second.events[2]
        .entries
        .iter()
        .find(|entry| entry.entry_id == raw)
        .unwrap();
    assert_eq!(raw_view.semantic_schema, 0);
    assert_eq!(raw_view.role, "opaque");
    assert_eq!(raw_view.kind, "opaque");
    assert_eq!(raw_view.source_formats, ["vendor/unknown-binary"]);
    assert_eq!(raw_view.raw_ranges, [raw_range]);
}

#[test]
fn canonical_entry_and_turn_ids_survive_store_restart() {
    let root = tmpdir("restart");
    let body = b"stable entry bytes";
    let (entry_capture, turn_capture, expected_entry, expected_turn) = {
        let store = Store::open_with_lar_body_store(root.clone(), config()).unwrap();
        insert_trace(&store, "stable-trace", "stable-session", 1);
        let written = store
            .write_body_artifact(
                &LarBodyArtifact::trace("stable-trace", "client_request"),
                "request.json",
                body,
            )
            .unwrap();
        let entry_capture = known(
            "openai-responses-v1",
            LarConversationRole::User,
            LarConversationEntryKind::Message,
            LarConversationRawRange {
                manifest_id: written.manifest_id.unwrap(),
                byte_offset: 0,
                byte_length: body.len() as u64,
            },
        );
        let expected_entry = store
            .register_lar_conversation_entry(&entry_capture)
            .unwrap();
        let turn_capture = LarConversationTurnCapture {
            trace_id: "stable-trace".into(),
            session_id: "stable-session".into(),
            event: LarConversationGenerationEvent::Initial,
            generation_entry_ids: vec![expected_entry.clone()],
            upto_index: 0,
            response_entry_ids: vec![],
        };
        let expected_turn = store.record_lar_conversation_turn(&turn_capture).unwrap();
        (entry_capture, turn_capture, expected_entry, expected_turn)
    };

    let reopened = Store::open_with_lar_body_store(root, config()).unwrap();
    assert_eq!(
        reopened
            .register_lar_conversation_entry(&entry_capture)
            .unwrap(),
        expected_entry
    );
    assert_eq!(
        reopened
            .record_lar_conversation_turn(&turn_capture)
            .unwrap(),
        expected_turn
    );
}

#[test]
fn malformed_known_format_uses_a_full_manifest_raw_only_turn() {
    let root = tmpdir("malformed-raw");
    let store = Store::open_with_lar_body_store(root, config()).unwrap();
    let body = br#"{"messages":[{"role":"user","content":"unterminated}] }"#;
    let written = store
        .write_body_artifact(
            &LarBodyArtifact::trace("malformed", "client_request"),
            "request.json",
            body,
        )
        .unwrap();
    let manifest_id = written.manifest_id.unwrap();
    store
        .insert_trace(&TraceRecord {
            id: "malformed".into(),
            session_id: Some("malformed-session".into()),
            ts_request_ms: 1,
            client_format: Some("anthropic".into()),
            ..Default::default()
        })
        .unwrap();

    store
        .populate_lar_conversation_for_trace("malformed", LarConversationEvidenceSource::Capture)
        .unwrap()
        .unwrap();
    let page = store
        .lar_conversation_events_page("malformed-session", None, 10)
        .unwrap();
    assert_eq!(page.events.len(), 1);
    assert_eq!(page.events[0].reason, "initial");
    let entry = &page.events[0].entries[0];
    assert_eq!(entry.semantic_schema, 0);
    assert_eq!(entry.role, "opaque");
    assert_eq!(
        entry.raw_ranges,
        [LarConversationRawRange {
            manifest_id: manifest_id.clone(),
            byte_offset: 0,
            byte_length: body.len() as u64,
        }]
    );
    assert_eq!(store.read_lar_manifest_body(&manifest_id).unwrap(), body);

    let changed = br#"{"messages":[{"role":"user","content":"different"}]}"#;
    store
        .write_body_artifact(
            &LarBodyArtifact::trace("changed", "client_request"),
            "request.json",
            changed,
        )
        .unwrap();
    store
        .insert_trace(&TraceRecord {
            id: "changed".into(),
            session_id: Some("malformed-session".into()),
            ts_request_ms: 2,
            client_format: Some("anthropic".into()),
            ..Default::default()
        })
        .unwrap();
    store
        .populate_lar_conversation_for_trace("changed", LarConversationEvidenceSource::Capture)
        .unwrap()
        .unwrap();
    let page = store
        .lar_conversation_events_page("malformed-session", None, 10)
        .unwrap();
    assert_eq!(page.events[1].reason, "mutation");
    assert_eq!(
        page.events[1].evidence.as_ref().unwrap().kind,
        "request_history_diverged"
    );
    assert!(page.events.iter().all(|event| event.reason != "compaction"));
}

#[test]
fn legacy_import_populates_and_bounded_complete_job_backfills_without_lar_writes() {
    let root = tmpdir("legacy-population");
    let store = Store::open(root.clone()).unwrap();
    let first_request = br#"{"messages":[{"role":"user","content":"one"}]}"#;
    let second_request = br#"{"messages":[{"role":"user","content":"one"},{"role":"assistant","content":"two"},{"role":"user","content":"three"}]}"#;
    let response = br#"{"choices":[{"message":{"role":"assistant","content":"ok"}}]}"#;
    for (id, timestamp, request) in [
        ("legacy-first", 10, first_request.as_slice()),
        ("legacy-second", 20, second_request.as_slice()),
    ] {
        let request_path = store.write_body(id, "request.json", request).unwrap();
        let response_path = store.write_body(id, "response.body", response).unwrap();
        store
            .insert_trace(&TraceRecord {
                id: id.into(),
                session_id: Some("legacy-conversation".into()),
                ts_request_ms: timestamp,
                client_format: Some("openai-chat".into()),
                req_body_path: Some(request_path),
                resp_body_path: Some(response_path),
                ..Default::default()
            })
            .unwrap();
    }

    let report = store
        .run_lar_legacy_import(&LarLegacyImportOptions::default())
        .unwrap();
    assert_eq!(report.failed, 0);
    let original = store
        .lar_conversation_events_page("legacy-conversation", None, 10)
        .unwrap();
    assert_eq!(original.events.len(), 2);
    assert_eq!(original.events[0].reason, "initial");
    assert_eq!(original.events[1].reason, "append");
    assert_eq!(
        original.events[0].entries[0].entry_id, original.events[1].entries[0].entry_id,
        "the repeated lexical prefix must reuse the first manifest span"
    );
    let exact = &original.events[1].entries[2].raw_ranges[0];
    let manifest = store.read_lar_manifest_body(&exact.manifest_id).unwrap();
    assert_eq!(
        &manifest[exact.byte_offset as usize..(exact.byte_offset + exact.byte_length) as usize],
        br#"{"role":"user","content":"three"}"#
    );
    let stable_ids = original
        .events
        .iter()
        .map(|event| (event.generation_id.clone(), event.turn_view_id.clone()))
        .collect::<Vec<_>>();
    let physical = physical_snapshot(&root);

    let conn = Connection::open(root.join("alexandria.sqlite3")).unwrap();
    conn.execute_batch(
        "DELETE FROM lar_conversation_turn_responses;
         DELETE FROM lar_conversation_turn_views;
         DELETE FROM lar_conversation_session_generations;
         DELETE FROM lar_conversation_generation_entries;
         DELETE FROM lar_conversation_generations;
         DELETE FROM lar_conversation_entry_fingerprints;
         DELETE FROM lar_conversation_entry_formats;
         DELETE FROM lar_conversation_entry_ranges;
         DELETE FROM lar_conversation_entries;",
    )
    .unwrap();
    drop(conn);

    let options = LarLegacyImportOptions {
        limit: Some(1),
        ..LarLegacyImportOptions::default()
    };
    let first_backfill = store.run_lar_legacy_import(&options).unwrap();
    assert_eq!(first_backfill.job_state, "complete");
    assert!(first_backfill.limit_reached);
    assert_eq!(
        store
            .lar_conversation_events_page("legacy-conversation", None, 10)
            .unwrap()
            .events
            .len(),
        1
    );
    assert_eq!(physical_snapshot(&root), physical);

    let final_backfill = store.run_lar_legacy_import(&options).unwrap();
    assert!(!final_backfill.limit_reached);
    let rebuilt = store
        .lar_conversation_events_page("legacy-conversation", None, 10)
        .unwrap();
    assert_eq!(
        rebuilt
            .events
            .iter()
            .map(|event| (event.generation_id.clone(), event.turn_view_id.clone()))
            .collect::<Vec<_>>(),
        stable_ids
    );
    assert_eq!(physical_snapshot(&root), physical);
}
