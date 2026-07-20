use alex_lar::{
    ArchiveReader, ArchiveWriter, ArtifactRangeRef, ChunkerConfig, ConversationEntry,
    ConversationEntryData, ConversationEntryKind, ConversationRole, Exchange, ExchangeData,
    FileHeader, Generation, GenerationData, GenerationReason, Limits, OpenPath, RecoveryStatus,
    TurnView, TurnViewData, REQUIRED_FEATURE_CONVERSATION_DAG, SEMANTIC_SCHEMA_V1,
};
use std::io::Cursor;

fn chunker() -> ChunkerConfig {
    ChunkerConfig {
        min_size: 64,
        target_size: 128,
        max_size: 256,
    }
}

fn dag_writer() -> ArchiveWriter<Cursor<Vec<u8>>> {
    let mut header = FileHeader::standalone([0x61; 16], 100, b"conversation-dag-test".to_vec());
    header.required_feature_bits |= REQUIRED_FEATURE_CONVERSATION_DAG;
    let mut writer = ArchiveWriter::create(
        Cursor::new(Vec::new()),
        header,
        chunker(),
        Limits::default(),
    )
    .unwrap();
    writer.enable_metadata_pages();
    writer
}

fn entry(
    manifest_id: alex_lar::ManifestId,
    length: usize,
    role: ConversationRole,
    kind: ConversationEntryKind,
) -> ConversationEntry {
    ConversationEntry::new(ConversationEntryData {
        semantic_schema: SEMANTIC_SCHEMA_V1,
        role,
        kind,
        raw_ranges: vec![ArtifactRangeRef {
            manifest_id,
            byte_offset: 0,
            byte_length: length as u64,
        }],
        name: None,
        tool_call_id: None,
    })
}

#[test]
fn append_branch_mutation_and_compaction_share_raw_entries_and_recover() {
    let mut writer = dag_writer();
    let raw_bodies: [&[u8]; 7] = [
        b"system: preserve exact wire instructions",
        b"user: inspect the repository",
        b"tool-result: a very large result is represented once",
        b"assistant: initial answer",
        b"user: branch request",
        b"user: edited request",
        b"summary: compacted earlier conversation",
    ];
    let manifests = raw_bodies
        .iter()
        .map(|body| writer.append_body(body).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(writer.manifest_count(), raw_bodies.len());

    let system = writer
        .append_conversation_entry(entry(
            manifests[0],
            raw_bodies[0].len(),
            ConversationRole::System,
            ConversationEntryKind::Message,
        ))
        .unwrap();
    let user = writer
        .append_conversation_entry(entry(
            manifests[1],
            raw_bodies[1].len(),
            ConversationRole::User,
            ConversationEntryKind::Message,
        ))
        .unwrap();
    let tool = writer
        .append_conversation_entry(entry(
            manifests[2],
            raw_bodies[2].len(),
            ConversationRole::Tool,
            ConversationEntryKind::ToolResult,
        ))
        .unwrap();
    let assistant = writer
        .append_conversation_entry(entry(
            manifests[3],
            raw_bodies[3].len(),
            ConversationRole::Assistant,
            ConversationEntryKind::Message,
        ))
        .unwrap();
    let branch_user = writer
        .append_conversation_entry(entry(
            manifests[4],
            raw_bodies[4].len(),
            ConversationRole::User,
            ConversationEntryKind::Message,
        ))
        .unwrap();
    let edited_user = writer
        .append_conversation_entry(entry(
            manifests[5],
            raw_bodies[5].len(),
            ConversationRole::User,
            ConversationEntryKind::Message,
        ))
        .unwrap();
    let summary = writer
        .append_conversation_entry(entry(
            manifests[6],
            raw_bodies[6].len(),
            ConversationRole::System,
            ConversationEntryKind::Summary,
        ))
        .unwrap();
    assert_eq!(writer.conversation_entry_count(), 7);
    assert_eq!(writer.manifest_count(), raw_bodies.len());

    let initial = writer
        .append_generation(Generation::new(GenerationData {
            parent_generation_id: None,
            entries: vec![system, user, tool],
            reason: GenerationReason::Initial,
        }))
        .unwrap();
    let appended = writer
        .append_generation(Generation::new(GenerationData {
            parent_generation_id: Some(initial),
            entries: vec![system, user, tool, assistant],
            reason: GenerationReason::Append,
        }))
        .unwrap();
    let branch = writer
        .append_generation(Generation::new(GenerationData {
            parent_generation_id: Some(appended),
            entries: vec![system, user, branch_user],
            reason: GenerationReason::Branch,
        }))
        .unwrap();
    let mutation = writer
        .append_generation(Generation::new(GenerationData {
            parent_generation_id: Some(appended),
            entries: vec![system, edited_user, tool, assistant],
            reason: GenerationReason::Mutation,
        }))
        .unwrap();
    let compaction = writer
        .append_generation(Generation::new(GenerationData {
            parent_generation_id: Some(mutation),
            entries: vec![summary, assistant],
            reason: GenerationReason::Compaction,
        }))
        .unwrap();
    assert_eq!(writer.generation_count(), 5);
    assert_eq!(writer.manifest_count(), raw_bodies.len());

    let trace_id = b"dag-trace";
    writer
        .append_exchange(Exchange::new(ExchangeData::new(trace_id, 1, 100, vec![])))
        .unwrap();
    let turn = writer
        .append_turn_view(TurnView::new(TurnViewData {
            trace_id: trace_id.to_vec(),
            generation_id: appended,
            upto_index: 2,
            response_entry_refs: vec![assistant],
        }))
        .unwrap();
    let duplicate_turn = TurnView::new(TurnViewData {
        trace_id: trace_id.to_vec(),
        generation_id: appended,
        upto_index: 2,
        response_entry_refs: vec![assistant],
    });
    assert_eq!(writer.append_turn_view(duplicate_turn).unwrap(), turn);
    assert_eq!(writer.turn_view_count(), 1);

    writer.seal().unwrap();
    let sealed = writer.into_inner().unwrap().into_inner();
    let reader = ArchiveReader::open(Cursor::new(sealed.clone()), Limits::default()).unwrap();
    assert_eq!(reader.open_path(), OpenPath::Footer);
    assert_eq!(reader.conversation_entry_count(), 7);
    assert_eq!(reader.generation_count(), 5);
    assert_eq!(reader.turn_view_count(), 1);
    assert_eq!(reader.turn_view_by_trace(trace_id).unwrap().id, turn);
    assert_eq!(
        reader.generation(&branch).unwrap().data.entries[..2],
        [system, user]
    );
    assert_eq!(
        reader.generation(&compaction).unwrap().data.entries,
        vec![summary, assistant]
    );
    for (entry_id, expected_manifest) in [
        (system, manifests[0]),
        (tool, manifests[2]),
        (assistant, manifests[3]),
        (summary, manifests[6]),
    ] {
        let stored = reader.conversation_entry(&entry_id).unwrap();
        assert_eq!(stored.data.raw_ranges.len(), 1);
        assert_eq!(stored.data.raw_ranges[0].manifest_id, expected_manifest);
    }

    let mut forward = sealed;
    forward.truncate(forward.len() - 72);
    let recovered = ArchiveReader::open(Cursor::new(forward), Limits::default()).unwrap();
    assert_eq!(recovered.open_path(), OpenPath::ForwardScan);
    assert!(matches!(
        recovered.recovery_status(),
        RecoveryStatus::CorruptIndexFallback { .. }
    ));
    assert_eq!(recovered.turn_view_by_trace(trace_id).unwrap().id, turn);
    assert_eq!(
        recovered.generation(&compaction).unwrap().data.entries[1],
        assistant
    );
}

#[test]
fn unknown_provider_can_remain_raw_only_without_core_parsing() {
    let mut writer = dag_writer();
    let opaque = b"\xff\x00provider-specific\x80wire";
    let manifest = writer.append_body(opaque).unwrap();
    let raw = ConversationEntry::new(ConversationEntryData::raw_only(vec![ArtifactRangeRef {
        manifest_id: manifest,
        byte_offset: 0,
        byte_length: opaque.len() as u64,
    }]));
    let raw_id = writer.append_conversation_entry(raw.clone()).unwrap();
    assert_eq!(writer.append_conversation_entry(raw).unwrap(), raw_id);
    let generation = writer
        .append_generation(Generation::new(GenerationData {
            parent_generation_id: None,
            entries: vec![raw_id],
            reason: GenerationReason::Import,
        }))
        .unwrap();
    let bytes = writer.into_inner().unwrap().into_inner();
    let mut reader = ArchiveReader::open(Cursor::new(bytes), Limits::default()).unwrap();
    let stored = reader.conversation_entry(&raw_id).unwrap();
    assert_eq!(stored.data.semantic_schema, 0);
    assert_eq!(stored.data.role, ConversationRole::Opaque);
    assert_eq!(stored.data.kind, ConversationEntryKind::Opaque);
    assert_eq!(
        reader.generation(&generation).unwrap().data.entries,
        vec![raw_id]
    );
    assert_eq!(reader.read_body(&manifest).unwrap(), opaque);
}

#[test]
fn active_checkpoint_indexes_the_conversation_graph() {
    let mut writer = dag_writer();
    let body = b"checkpointed prompt";
    let manifest = writer.append_body(body).unwrap();
    let entry = writer
        .append_conversation_entry(entry(
            manifest,
            body.len(),
            ConversationRole::User,
            ConversationEntryKind::Message,
        ))
        .unwrap();
    let generation = writer
        .append_generation(Generation::new(GenerationData {
            parent_generation_id: None,
            entries: vec![entry],
            reason: GenerationReason::Initial,
        }))
        .unwrap();
    let trace_id = b"checkpointed-turn";
    writer
        .append_exchange(Exchange::new(ExchangeData::new(trace_id, 1, 100, vec![])))
        .unwrap();
    let turn = writer
        .append_turn_view(TurnView::new(TurnViewData {
            trace_id: trace_id.to_vec(),
            generation_id: generation,
            upto_index: 0,
            response_entry_refs: vec![],
        }))
        .unwrap();

    writer.checkpoint().unwrap();
    let bytes = writer.into_inner().unwrap().into_inner();
    let reader = ArchiveReader::open(Cursor::new(bytes), Limits::default()).unwrap();
    assert_eq!(reader.open_path(), OpenPath::Checkpoint);
    assert_eq!(reader.conversation_entry(&entry).unwrap().id, entry);
    assert_eq!(reader.generation(&generation).unwrap().id, generation);
    assert_eq!(reader.turn_view_by_trace(trace_id).unwrap().id, turn);
}

#[test]
fn writer_requires_the_conversation_feature_and_bounds_ranges() {
    let header = FileHeader::standalone([0x62; 16], 101, b"no-dag-feature".to_vec());
    let mut writer = ArchiveWriter::create(
        Cursor::new(Vec::new()),
        header,
        chunker(),
        Limits::default(),
    )
    .unwrap();
    let body = writer.append_body(b"raw").unwrap();
    let raw = ConversationEntry::new(ConversationEntryData::raw_only(vec![ArtifactRangeRef {
        manifest_id: body,
        byte_offset: 0,
        byte_length: 3,
    }]));
    assert!(matches!(
        writer.append_conversation_entry(raw),
        Err(alex_lar::Error::Unsupported(message)) if message.contains("conversation-dag")
    ));

    let mut writer = dag_writer();
    let body = writer.append_body(b"raw").unwrap();
    let out_of_bounds =
        ConversationEntry::new(ConversationEntryData::raw_only(vec![ArtifactRangeRef {
            manifest_id: body,
            byte_offset: 2,
            byte_length: 2,
        }]));
    assert!(matches!(
        writer.append_conversation_entry(out_of_bounds),
        Err(alex_lar::Error::Invalid(
            "conversation artifact range exceeds manifest"
        ))
    ));
}

#[test]
fn reader_rejects_conversation_records_without_the_required_feature() {
    let mut writer = dag_writer();
    let body = writer.append_body(b"raw").unwrap();
    writer
        .append_conversation_entry(ConversationEntry::new(ConversationEntryData::raw_only(
            vec![ArtifactRangeRef {
                manifest_id: body,
                byte_offset: 0,
                byte_length: 3,
            }],
        )))
        .unwrap();
    let mut bytes = writer.into_inner().unwrap().into_inner();

    let header_payload_length = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
    let writer_length = u16::from_le_bytes(bytes[37..39].try_into().unwrap()) as usize;
    let required_bits_offset = 39 + writer_length;
    let required_bits = u64::from_le_bytes(
        bytes[required_bits_offset..required_bits_offset + 8]
            .try_into()
            .unwrap(),
    ) & !REQUIRED_FEATURE_CONVERSATION_DAG;
    bytes[required_bits_offset..required_bits_offset + 8]
        .copy_from_slice(&required_bits.to_le_bytes());
    let checksum_offset = 12 + header_payload_length;
    let checksum = crc32fast::hash(&bytes[..checksum_offset]);
    bytes[checksum_offset..checksum_offset + 4].copy_from_slice(&checksum.to_le_bytes());

    assert!(matches!(
        ArchiveReader::open(Cursor::new(bytes), Limits::default()),
        Err(alex_lar::Error::Unsupported(message))
            if message.contains("conversation DAG record without required feature bit")
    ));
}

#[test]
fn generation_entry_count_is_bounded_while_decoding() {
    let mut writer = dag_writer();
    let first_body = writer.append_body(b"first").unwrap();
    let second_body = writer.append_body(b"second").unwrap();
    let first = writer
        .append_conversation_entry(entry(
            first_body,
            5,
            ConversationRole::User,
            ConversationEntryKind::Message,
        ))
        .unwrap();
    let second = writer
        .append_conversation_entry(entry(
            second_body,
            6,
            ConversationRole::Assistant,
            ConversationEntryKind::Message,
        ))
        .unwrap();
    writer
        .append_generation(Generation::new(GenerationData {
            parent_generation_id: None,
            entries: vec![first, second],
            reason: GenerationReason::Initial,
        }))
        .unwrap();

    let bytes = writer.into_inner().unwrap().into_inner();
    let limits = Limits {
        max_generation_entries: 1,
        ..Limits::default()
    };
    assert!(matches!(
        ArchiveReader::open(Cursor::new(bytes), limits),
        Err(alex_lar::Error::Limit {
            what: "generation entry count",
            actual: 2,
            limit: 1,
        })
    ));
}
