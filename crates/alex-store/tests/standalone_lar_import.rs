use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use alex_lar::{
    ArchiveReader, ArchiveWriter, ArtifactRangeRef, ChunkerConfig, ConversationEntry,
    ConversationEntryData, Exchange, ExchangeData, FileHeader, Generation, GenerationData,
    GenerationReason, HeaderAtom, HeaderBlock, HeaderFidelity, Limits, Stage, StageData, StageKind,
    TurnView, TurnViewData, REQUIRED_FEATURE_CONVERSATION_DAG,
};
use alex_store::{
    LarBodyArtifact, LarBodyStoreConfig, LarBodyStoreMode, LarStandaloneImportOptions, Store,
};

static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn tmpdir(name: &str) -> PathBuf {
    let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "alex-standalone-import-{name}-{}-{sequence}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

struct Fixture {
    request: Vec<u8>,
    response: Vec<u8>,
}

fn write_archive(path: &Path, sealed: bool) -> Fixture {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .read(true)
        .write(true)
        .open(path)
        .unwrap();
    let mut writer = ArchiveWriter::create(
        file,
        FileHeader::standalone([42; 16], 1_700_000_000_000_000_000, b"import-test".to_vec()),
        ChunkerConfig {
            min_size: 4,
            target_size: 4,
            max_size: 4,
        },
        Limits::default(),
    )
    .unwrap();
    let request = b"AAAABBBBCCCCDDDD".to_vec();
    let response = br#"{"ok":true}"#.to_vec();
    let request_id = writer.append_body(&request).unwrap();
    let response_id = writer.append_body(&response).unwrap();
    let headers = HeaderBlock::new(
        HeaderFidelity::Exact,
        vec![
            HeaderAtom {
                original_name: b"X-Test".to_vec(),
                value: b"one".to_vec(),
                flags: 0,
            },
            HeaderAtom {
                original_name: b"X-Test".to_vec(),
                value: b"two".to_vec(),
                flags: 0,
            },
        ],
    );
    let headers_id = writer.append_header_block(headers).unwrap();

    let mut client_request = StageData::new(StageKind::ClientRequest, 1_700_000_000_000_000_000);
    client_request.request_headers_ref = Some(headers_id);
    client_request.request_body_manifest_ref = Some(request_id);
    let client_request = writer.append_stage(Stage::new(client_request)).unwrap();

    let mut upstream_request =
        StageData::new(StageKind::UpstreamRequest, 1_700_000_000_010_000_000);
    upstream_request.attempt_number = Some(1);
    upstream_request.request_headers_ref = Some(headers_id);
    upstream_request.request_body_manifest_ref = Some(request_id);
    upstream_request.provider = Some(b"anthropic".to_vec());
    upstream_request.requested_model = Some(b"claude-test".to_vec());
    let upstream_request = writer.append_stage(Stage::new(upstream_request)).unwrap();

    let mut upstream_response =
        StageData::new(StageKind::UpstreamResponse, 1_700_000_000_020_000_000);
    upstream_response.attempt_number = Some(1);
    upstream_response.response_headers_ref = Some(headers_id);
    upstream_response.response_body_manifest_ref = Some(response_id);
    upstream_response.status_code = Some(200);
    let upstream_response = writer.append_stage(Stage::new(upstream_response)).unwrap();

    let mut client_response = StageData::new(StageKind::ClientResponse, 1_700_000_000_030_000_000);
    client_response.response_headers_ref = Some(headers_id);
    client_response.response_body_manifest_ref = Some(response_id);
    client_response.status_code = Some(200);
    let client_response = writer.append_stage(Stage::new(client_response)).unwrap();

    let mut exchange = ExchangeData::new(
        b"trace-standalone".to_vec(),
        7,
        1_700_000_000_000_000_000,
        vec![
            client_request,
            upstream_request,
            upstream_response,
            client_response,
        ],
    );
    exchange.session_id = Some(b"session-standalone".to_vec());
    exchange.run_id = Some(b"run-standalone".to_vec());
    writer.append_exchange(Exchange::new(exchange)).unwrap();
    if sealed {
        writer.seal().unwrap();
    } else {
        writer.checkpoint().unwrap();
    }
    writer.get_ref().sync_all().unwrap();
    Fixture { request, response }
}

fn write_body_archive(path: &Path, file_uuid: [u8; 16], chunk_size: usize, body: &[u8]) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .read(true)
        .write(true)
        .open(path)
        .unwrap();
    let mut writer = ArchiveWriter::create(
        file,
        FileHeader::standalone(
            file_uuid,
            1_700_000_000_000_000_000,
            b"body-import-test".to_vec(),
        ),
        ChunkerConfig {
            min_size: chunk_size,
            target_size: chunk_size,
            max_size: chunk_size,
        },
        Limits::default(),
    )
    .unwrap();
    writer.append_body(body).unwrap();
    writer.seal().unwrap();
    writer.get_ref().sync_all().unwrap();
}

fn write_shared_stage_archive(path: &Path) {
    let file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .read(true)
        .write(true)
        .open(path)
        .unwrap();
    let mut writer = ArchiveWriter::create(
        file,
        FileHeader::standalone(
            [81; 16],
            1_700_000_000_000_000_000,
            b"stage-sharing".to_vec(),
        ),
        ChunkerConfig::default(),
        Limits::default(),
    )
    .unwrap();
    let shared = writer
        .append_stage(Stage::new(StageData::new(
            StageKind::RouterDecision,
            1_700_000_000_000_000_000,
        )))
        .unwrap();
    for (trace, sequence, stages) in [
        ("trace-repeat", 1, vec![shared, shared]),
        ("trace-shared", 2, vec![shared]),
        ("trace-zero", 3, vec![]),
    ] {
        writer
            .append_exchange(Exchange::new(ExchangeData::new(
                trace.as_bytes(),
                sequence,
                1_700_000_000_000_000_000 + sequence,
                stages,
            )))
            .unwrap();
    }
    writer.seal().unwrap();
    writer.get_ref().sync_all().unwrap();
}

fn write_conversation_archive(path: &Path, body: &[u8]) {
    let file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .read(true)
        .write(true)
        .open(path)
        .unwrap();
    let mut header = FileHeader::standalone(
        [92; 16],
        1_700_000_000_000_000_000,
        b"conversation-rekey".to_vec(),
    );
    header.required_feature_bits |= REQUIRED_FEATURE_CONVERSATION_DAG;
    let mut writer = ArchiveWriter::create(
        file,
        header,
        ChunkerConfig {
            min_size: 4,
            target_size: 4,
            max_size: 4,
        },
        Limits::default(),
    )
    .unwrap();
    let manifest = writer.append_body(body).unwrap();
    let entry = writer
        .append_conversation_entry(ConversationEntry::new(ConversationEntryData::raw_only(
            vec![ArtifactRangeRef {
                manifest_id: manifest,
                byte_offset: 0,
                byte_length: body.len() as u64,
            }],
        )))
        .unwrap();
    let generation = writer
        .append_generation(Generation::new(GenerationData {
            parent_generation_id: None,
            entries: vec![entry],
            reason: GenerationReason::Initial,
        }))
        .unwrap();
    let mut stage = StageData::new(StageKind::ClientRequest, 1_700_000_000_000_000_000);
    stage.request_body_manifest_ref = Some(manifest);
    let stage = writer.append_stage(Stage::new(stage)).unwrap();
    let mut exchange = ExchangeData::new(
        b"trace-conversation-rekey",
        1,
        1_700_000_000_000_000_000,
        vec![stage],
    );
    exchange.session_id = Some(b"session-conversation-rekey".to_vec());
    writer.append_exchange(Exchange::new(exchange)).unwrap();
    writer
        .append_turn_view(TurnView::new(TurnViewData {
            trace_id: b"trace-conversation-rekey".to_vec(),
            generation_id: generation,
            upto_index: 0,
            response_entry_refs: Vec::new(),
        }))
        .unwrap();
    writer.seal().unwrap();
    writer.get_ref().sync_all().unwrap();
}

fn file_bytes(path: &Path) -> Vec<u8> {
    let mut bytes = Vec::new();
    File::open(path).unwrap().read_to_end(&mut bytes).unwrap();
    bytes
}

fn catalog_count(data_dir: &Path, table: &str) -> i64 {
    let conn = rusqlite::Connection::open(data_dir.join("alexandria.sqlite3")).unwrap();
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
        row.get(0)
    })
    .unwrap()
}

#[test]
fn imports_sealed_archive_into_empty_store_and_reconstructs_anchors() {
    let root = tmpdir("empty");
    let archive = root.join("source.lar");
    let fixture = write_archive(&archive, true);
    let source_before = file_bytes(&archive);
    let data_dir = root.join("store");
    let store = Store::open(data_dir.clone()).unwrap();

    let report = store
        .import_sealed_lar_archive(&archive, &LarStandaloneImportOptions::default())
        .unwrap();
    assert!(!report.already_attached);
    assert_eq!(
        (report.exchanges, report.stages, report.header_blocks),
        (1, 4, 1)
    );
    assert_eq!(report.traces_inserted, 1);
    assert_eq!(
        file_bytes(&archive),
        source_before,
        "import mutated its source"
    );

    let trace = store.get_trace("trace-standalone").unwrap().unwrap();
    assert_eq!(trace["session_id"], "session-standalone");
    assert_eq!(trace["run_id"], "run-standalone");
    assert_eq!(store.lar_session_revision("session-standalone").unwrap(), 7);
    assert_eq!(
        store
            .read_lar_or_legacy_artifact("trace", "trace-standalone", "client_request", None,)
            .unwrap()
            .unwrap(),
        fixture.request
    );
    assert_eq!(
        store
            .read_lar_or_legacy_artifact("trace", "trace-standalone", "client_response", None,)
            .unwrap()
            .unwrap(),
        fixture.response
    );
    assert_eq!(catalog_count(&data_dir, "lar_stage_records"), 4);
    assert_eq!(catalog_count(&data_dir, "lar_header_blocks"), 1);
    assert_eq!(catalog_count(&data_dir, "lar_header_block_atoms"), 2);
}

#[test]
fn repeated_import_is_idempotent_and_reuses_every_logical_body() {
    let root = tmpdir("idempotent");
    let archive = root.join("source.lar");
    write_archive(&archive, true);
    let data_dir = root.join("store");
    let store = Store::open(data_dir.clone()).unwrap();
    let first = store
        .import_sealed_lar_archive(&archive, &LarStandaloneImportOptions::default())
        .unwrap();
    let counts = (
        catalog_count(&data_dir, "lar_files"),
        catalog_count(&data_dir, "lar_chunks"),
        catalog_count(&data_dir, "lar_manifests"),
        catalog_count(&data_dir, "lar_trace_artifacts"),
    );

    let repeated = store
        .import_sealed_lar_archive(&archive, &LarStandaloneImportOptions::default())
        .unwrap();
    assert!(repeated.already_attached);
    assert!(!repeated.relocated);
    assert_eq!(repeated.chunks_reused, first.chunks);
    assert_eq!(repeated.manifests_reused, first.manifests);
    assert_eq!(
        counts,
        (
            catalog_count(&data_dir, "lar_files"),
            catalog_count(&data_dir, "lar_chunks"),
            catalog_count(&data_dir, "lar_manifests"),
            catalog_count(&data_dir, "lar_trace_artifacts"),
        )
    );
}

#[test]
fn existing_logical_body_is_reused_even_when_archive_chunking_differs() {
    let root = tmpdir("logical-dedupe");
    let first_archive = root.join("one-chunk.lar");
    let second_archive = root.join("four-chunks.lar");
    let body = b"AAAABBBBCCCCDDDD";
    write_body_archive(&first_archive, [51; 16], 16, body);
    write_body_archive(&second_archive, [52; 16], 4, body);
    let data_dir = root.join("store");
    let store = Store::open(data_dir.clone()).unwrap();

    let first = store
        .import_sealed_lar_archive(&first_archive, &LarStandaloneImportOptions::default())
        .unwrap();
    assert_eq!((first.chunks, first.manifests), (1, 1));
    let counts = (
        catalog_count(&data_dir, "lar_chunks"),
        catalog_count(&data_dir, "lar_manifests"),
        catalog_count(&data_dir, "lar_manifest_chunks"),
    );

    let second = store
        .import_sealed_lar_archive(&second_archive, &LarStandaloneImportOptions::default())
        .unwrap();
    assert_eq!((second.chunks, second.manifests), (4, 1));
    assert_eq!(second.chunks_reused, 4);
    assert_eq!(second.manifests_reused, 1);
    assert_eq!(
        counts,
        (
            catalog_count(&data_dir, "lar_chunks"),
            catalog_count(&data_dir, "lar_manifests"),
            catalog_count(&data_dir, "lar_manifest_chunks"),
        ),
        "alternate physical chunking duplicated a known logical body"
    );
}

#[test]
fn reused_file_uuid_cannot_rebind_catalog_to_different_archive_bytes() {
    let root = tmpdir("uuid-rebind");
    let original = root.join("original.lar");
    let impostor = root.join("impostor.lar");
    write_body_archive(&original, [61; 16], 4, b"original body");
    write_body_archive(&impostor, [61; 16], 4, b"different body");
    let data_dir = root.join("store");
    let store = Store::open(data_dir.clone()).unwrap();
    store
        .import_sealed_lar_archive(&original, &LarStandaloneImportOptions::default())
        .unwrap();
    let counts = (
        catalog_count(&data_dir, "lar_files"),
        catalog_count(&data_dir, "lar_chunks"),
        catalog_count(&data_dir, "lar_manifests"),
    );

    let error = store
        .import_sealed_lar_archive(&impostor, &LarStandaloneImportOptions::default())
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("different immutable archive bytes"));
    assert_eq!(
        counts,
        (
            catalog_count(&data_dir, "lar_files"),
            catalog_count(&data_dir, "lar_chunks"),
            catalog_count(&data_dir, "lar_manifests"),
        ),
        "rejected rebind partially changed the catalog"
    );
    let conn = rusqlite::Connection::open(data_dir.join("alexandria.sqlite3")).unwrap();
    let attached_path: String = conn
        .query_row("SELECT path FROM lar_files", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        attached_path,
        original.canonicalize().unwrap().to_str().unwrap()
    );
}

#[test]
fn corrupt_and_unsealed_sources_publish_nothing() {
    let root = tmpdir("reject");
    let corrupt = root.join("corrupt.lar");
    write_archive(&corrupt, true);
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&corrupt)
        .unwrap();
    file.seek(SeekFrom::Start(0)).unwrap();
    file.write_all(b"NOPE").unwrap();
    file.sync_all().unwrap();
    let unsealed = root.join("unsealed.lar");
    write_archive(&unsealed, false);
    let data_dir = root.join("store");
    let store = Store::open(data_dir.clone()).unwrap();

    assert!(store
        .import_sealed_lar_archive(&corrupt, &LarStandaloneImportOptions::default())
        .unwrap_err()
        .to_string()
        .contains("opening standalone"));
    assert!(store
        .import_sealed_lar_archive(&unsealed, &LarStandaloneImportOptions::default())
        .unwrap_err()
        .to_string()
        .contains("sealed footer"));
    for table in [
        "lar_files",
        "lar_chunks",
        "lar_manifests",
        "lar_stage_records",
        "traces",
    ] {
        assert_eq!(
            catalog_count(&data_dir, table),
            0,
            "partial rows in {table}"
        );
    }
}

#[test]
fn relocated_archive_is_revalidated_and_reattached_with_safe_relative_path() {
    let root = tmpdir("relocate");
    let archive = root.join("source.lar");
    let fixture = write_archive(&archive, true);
    let data_dir = root.join("store");
    let store = Store::open(data_dir.clone()).unwrap();
    let first = store
        .import_sealed_lar_archive(&archive, &LarStandaloneImportOptions::default())
        .unwrap();

    let moved = data_dir.join("archives/moved.lar");
    std::fs::create_dir_all(moved.parent().unwrap()).unwrap();
    std::fs::rename(&archive, &moved).unwrap();
    let attached = store
        .import_sealed_lar_archive(&moved, &LarStandaloneImportOptions::default())
        .unwrap();
    assert_eq!(attached.file_uuid, first.file_uuid);
    assert!(attached.relocated);
    assert_eq!(attached.catalog_path, "archives/moved.lar");
    assert_eq!(catalog_count(&data_dir, "lar_files"), 1);
    assert_eq!(
        store
            .read_lar_or_legacy_artifact("trace", "trace-standalone", "client_request", None,)
            .unwrap()
            .unwrap(),
        fixture.request
    );
}

#[test]
fn relative_standalone_archive_donates_chunks_to_live_write_and_read() {
    let root = tmpdir("relative-donor");
    let data_dir = root.join("store");
    let archive = data_dir.join("archives/donor.lar");
    let body = b"AAAABBBBCCCCDDDD";
    write_body_archive(&archive, [71; 16], 4, body);
    let store = Store::open_with_lar_body_store(
        data_dir.clone(),
        LarBodyStoreConfig {
            mode: LarBodyStoreMode::LarWithFallback,
            chunker: ChunkerConfig {
                min_size: 4,
                target_size: 4,
                max_size: 4,
            },
            ..LarBodyStoreConfig::default()
        },
    )
    .unwrap();
    let imported = store
        .import_sealed_lar_archive(&archive, &LarStandaloneImportOptions::default())
        .unwrap();
    assert_eq!(imported.catalog_path, "archives/donor.lar");
    let chunks_before = catalog_count(&data_dir, "lar_chunks");

    let written = store
        .write_body_artifact(
            &LarBodyArtifact::trace("trace-live", "client_request"),
            "request.json",
            body,
        )
        .unwrap();
    assert_eq!(written.lar_error, None);
    assert!(written.manifest_id.is_some());
    assert_eq!(catalog_count(&data_dir, "lar_chunks"), chunks_before);
    assert_eq!(
        store
            .read_lar_or_legacy_artifact("trace", "trace-live", "client_request", None)
            .unwrap()
            .unwrap(),
        body
    );
}

#[test]
fn repeated_import_rejects_incompatible_stage_catalog_binding() {
    let root = tmpdir("stage-collision");
    let archive = root.join("source.lar");
    write_archive(&archive, true);
    let data_dir = root.join("store");
    let store = Store::open(data_dir.clone()).unwrap();
    store
        .import_sealed_lar_archive(&archive, &LarStandaloneImportOptions::default())
        .unwrap();
    let conn = rusqlite::Connection::open(data_dir.join("alexandria.sqlite3")).unwrap();
    conn.execute(
        "UPDATE lar_stage_records SET trace_id='wrong-trace' WHERE capture_sequence=0",
        [],
    )
    .unwrap();
    drop(conn);

    let error = store
        .import_sealed_lar_archive(&archive, &LarStandaloneImportOptions::default())
        .unwrap_err();
    assert!(error.to_string().contains("incompatible"));
    assert_eq!(catalog_count(&data_dir, "lar_files"), 1);
}

#[test]
fn first_import_rejects_an_existing_legacy_trace_without_rebinding_its_body() {
    let root = tmpdir("legacy-trace-collision");
    let archive = root.join("source.lar");
    write_archive(&archive, true);
    let data_dir = root.join("store");
    let store = Store::open(data_dir.clone()).unwrap();
    let original = b"legacy body must remain authoritative";
    let path = store
        .write_body("trace-standalone", "request.json", original)
        .unwrap();
    store
        .insert_trace(&alex_core::TraceRecord {
            id: "trace-standalone".into(),
            req_body_path: Some(path),
            ..Default::default()
        })
        .unwrap();

    let error = store
        .import_sealed_lar_archive(&archive, &LarStandaloneImportOptions::default())
        .unwrap_err();
    assert!(error.to_string().contains("already exists"));
    assert_eq!(catalog_count(&data_dir, "lar_files"), 0);
    assert_eq!(catalog_count(&data_dir, "lar_trace_artifacts"), 0);
    assert_eq!(
        store
            .read_lar_or_legacy_artifact("trace", "trace-standalone", "client_request", None,)
            .unwrap()
            .unwrap(),
        original
    );
}

#[test]
fn shared_repeated_and_zero_stage_exchanges_keep_explicit_ownership() {
    let root = tmpdir("stage-occurrences");
    let archive = root.join("source.lar");
    write_shared_stage_archive(&archive);
    let data_dir = root.join("store");
    let store = Store::open(data_dir.clone()).unwrap();
    let report = store
        .import_sealed_lar_archive(&archive, &LarStandaloneImportOptions::default())
        .unwrap();
    assert_eq!(report.exchanges, 3);
    assert_eq!(report.stages, 3);
    assert_eq!(catalog_count(&data_dir, "lar_stage_records"), 3);
    assert_eq!(catalog_count(&data_dir, "lar_exchange_records"), 3);

    let exported = root.join("zero-export.lar");
    let file = OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .open(&exported)
        .unwrap();
    let mut writer = ArchiveWriter::create(
        file,
        FileHeader::standalone([82; 16], 1_700_000_000_100_000_000, b"zero-export".to_vec()),
        ChunkerConfig::default(),
        Limits::default(),
    )
    .unwrap();
    assert!(store
        .append_exact_trace_to_standalone(&mut writer, "trace-zero")
        .unwrap());
    writer.seal().unwrap();
    let reader = ArchiveReader::open(File::open(exported).unwrap(), Limits::default()).unwrap();
    assert!(reader
        .exchange_by_trace(b"trace-zero")
        .unwrap()
        .data
        .stages
        .is_empty());
}

#[test]
fn logical_body_reuse_transitively_rekeys_and_reexports_conversation_ids() {
    let root = tmpdir("conversation-rekey");
    let body = b"AAAABBBBCCCCDDDD";
    let donor = root.join("donor.lar");
    let source = root.join("source.lar");
    write_body_archive(&donor, [91; 16], 16, body);
    write_conversation_archive(&source, body);
    let data_dir = root.join("store");
    let store = Store::open(data_dir).unwrap();
    store
        .import_sealed_lar_archive(&donor, &LarStandaloneImportOptions::default())
        .unwrap();
    let imported = store
        .import_sealed_lar_archive(&source, &LarStandaloneImportOptions::default())
        .unwrap();
    assert_eq!(imported.manifests_reused, 1);

    let destination = root.join("roundtrip.lar");
    let file = OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .open(&destination)
        .unwrap();
    let mut header = FileHeader::standalone(
        [93; 16],
        1_700_000_000_100_000_000,
        b"conversation-roundtrip".to_vec(),
    );
    header.required_feature_bits |= REQUIRED_FEATURE_CONVERSATION_DAG;
    let mut writer =
        ArchiveWriter::create(file, header, ChunkerConfig::default(), Limits::default()).unwrap();
    assert!(store
        .append_exact_trace_to_standalone(&mut writer, "trace-conversation-rekey")
        .unwrap());
    writer.seal().unwrap();
    let mut reader =
        ArchiveReader::open(File::open(destination).unwrap(), Limits::default()).unwrap();
    let turn = reader
        .turn_view_by_trace(b"trace-conversation-rekey")
        .unwrap();
    let generation = reader.generation(&turn.data.generation_id).unwrap();
    let manifest_id = reader
        .conversation_entry(&generation.data.entries[0])
        .unwrap()
        .data
        .raw_ranges[0]
        .manifest_id;
    assert_eq!(reader.read_body(&manifest_id).unwrap(), body);
}
