use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use alex_lar::{
    ArchiveWriter, ChunkerConfig, Exchange, ExchangeData, FileHeader, FileRole, Limits, Stage,
    StageData, StageKind, REQUIRED_FEATURE_ARCHIVE_SET_BODY_REFS,
};
use alex_store::{
    LarArchiveAvailability, LarArchiveReattachOptions, LarArchiveUnavailableError,
    LarArtifactBatchRead, LarArtifactReadRequest, LarBodyArtifact, LarBodyStoreConfig,
    LarBodyStoreMode, LarStandaloneImportOptions, Store,
};

static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn tmpdir(name: &str) -> PathBuf {
    let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "alex-archive-ops-{name}-{}-{sequence}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

struct Fixture {
    body: Vec<u8>,
    manifest_id: String,
}

fn write_archive(path: &Path, file_uuid: [u8; 16], body: &[u8]) -> Fixture {
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
            b"archive-ops-test".to_vec(),
        ),
        ChunkerConfig {
            min_size: 4,
            target_size: 4,
            max_size: 4,
        },
        Limits::default(),
    )
    .unwrap();
    let manifest_id = writer.append_body(body).unwrap();
    let mut request = StageData::new(StageKind::ClientRequest, 1_700_000_000_000_000_000);
    request.request_body_manifest_ref = Some(manifest_id);
    let request = writer.append_stage(Stage::new(request)).unwrap();
    let mut exchange = ExchangeData::new(
        b"trace-archive-ops".to_vec(),
        1,
        1_700_000_000_000_000_000,
        vec![request],
    );
    exchange.session_id = Some(b"session-archive-ops".to_vec());
    writer.append_exchange(Exchange::new(exchange)).unwrap();
    writer.seal().unwrap();
    writer.get_ref().sync_all().unwrap();
    Fixture {
        body: body.to_vec(),
        manifest_id: manifest_id.to_string(),
    }
}

fn catalog_count(data_dir: &Path, table: &str) -> i64 {
    let conn = rusqlite::Connection::open(data_dir.join("alexandria.sqlite3")).unwrap();
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
        row.get(0)
    })
    .unwrap()
}

fn live_config() -> LarBodyStoreConfig {
    LarBodyStoreConfig {
        mode: LarBodyStoreMode::LarWithFallback,
        chunker: ChunkerConfig {
            min_size: 4,
            target_size: 4,
            max_size: 4,
        },
        ..LarBodyStoreConfig::default()
    }
}

fn uuid_bytes(value: &str) -> [u8; 16] {
    let bytes: Vec<u8> = value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect();
    bytes.try_into().unwrap()
}

fn write_body_pack(path: &Path, file_uuid: [u8; 16], body: &[u8]) {
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
    let mut header = FileHeader::body_pack(
        file_uuid,
        1_700_000_000_000_000_000,
        b"archive-ops-pack-test".to_vec(),
    );
    header.required_feature_bits |= REQUIRED_FEATURE_ARCHIVE_SET_BODY_REFS;
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
    writer.append_body(body).unwrap();
    writer.seal().unwrap();
    writer.get_ref().sync_all().unwrap();
}

fn write_event_log(path: &Path, file_uuid: [u8; 16], body: &[u8]) {
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
    let mut header = FileHeader::standalone(
        file_uuid,
        1_700_000_000_000_000_000,
        b"archive-ops-event-test".to_vec(),
    );
    header.file_role = FileRole::EventLog;
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
    writer.append_body(body).unwrap();
    writer.seal().unwrap();
    writer.get_ref().sync_all().unwrap();
}

#[test]
fn detach_move_and_validated_reattach_restore_live_reads_without_new_chunks() {
    let root = tmpdir("detach-reattach");
    let data_dir = root.join("store");
    let source = data_dir.join("archives/source.lar");
    let fixture = write_archive(&source, [81; 16], b"AAAABBBBCCCCDDDD");
    let store = Store::open_with_lar_body_store(data_dir.clone(), live_config()).unwrap();
    let attached = store
        .import_sealed_lar_archive(&source, &LarStandaloneImportOptions::default())
        .unwrap();
    let initial = store
        .lar_archive_file_status(&attached.file_uuid)
        .unwrap()
        .unwrap();
    assert_eq!(initial.availability, LarArchiveAvailability::Online);
    assert_eq!(initial.catalog_path, "archives/source.lar");
    assert_eq!(
        store.read_lar_manifest_body(&fixture.manifest_id).unwrap(),
        fixture.body
    );

    let detached = store.detach_lar_archive(&attached.file_uuid).unwrap();
    assert!(!detached.already_offline);
    assert_eq!(
        detached.file.availability,
        LarArchiveAvailability::ArchivedOffline
    );
    assert!(detached.file.exists, "detach must not delete archive bytes");
    let repeated = store.detach_lar_archive(&attached.file_uuid).unwrap();
    assert!(repeated.already_offline);
    let error = store
        .read_lar_manifest_body(&fixture.manifest_id)
        .unwrap_err();
    assert_eq!(
        error
            .downcast_ref::<LarArchiveUnavailableError>()
            .unwrap()
            .availability,
        LarArchiveAvailability::ArchivedOffline
    );
    assert!(matches!(
        &store.read_lar_or_legacy_artifact_batch_bounded(
            &[LarArtifactReadRequest::new(
                "trace",
                "trace-archive-ops",
                "client_request",
            )],
            u64::MAX,
        )[0],
        LarArtifactBatchRead::ArchiveUnavailable(error)
            if error.availability == LarArchiveAvailability::ArchivedOffline
    ));

    let moved = data_dir.join("cold/moved.lar");
    std::fs::create_dir_all(moved.parent().unwrap()).unwrap();
    std::fs::rename(&source, &moved).unwrap();
    let restored = store
        .reattach_lar_archive(
            &attached.file_uuid,
            &moved,
            &LarArchiveReattachOptions::default(),
        )
        .unwrap();
    assert!(restored.relocated);
    assert_eq!(restored.file_uuid, attached.file_uuid);
    assert_eq!(restored.catalog_path, "cold/moved.lar");
    let online = store
        .lar_archive_file_status(&attached.file_uuid)
        .unwrap()
        .unwrap();
    assert_eq!(online.availability, LarArchiveAvailability::Online);
    assert_eq!(
        store.read_lar_manifest_body(&fixture.manifest_id).unwrap(),
        fixture.body
    );

    let chunks_before = catalog_count(&data_dir, "lar_chunks");
    let live = store
        .write_body_artifact(
            &LarBodyArtifact::trace("trace-live-after-reattach", "client_request"),
            "request.json",
            &fixture.body,
        )
        .unwrap();
    assert_eq!(live.lar_error, None);
    assert_eq!(
        live.manifest_id.as_deref(),
        Some(fixture.manifest_id.as_str())
    );
    assert_eq!(catalog_count(&data_dir, "lar_chunks"), chunks_before);
    assert_eq!(
        store
            .read_lar_or_legacy_artifact(
                "trace",
                "trace-live-after-reattach",
                "client_request",
                None,
            )
            .unwrap()
            .unwrap(),
        fixture.body
    );
}

#[test]
fn missing_archive_is_file_level_state_and_can_be_reattached() {
    let root = tmpdir("missing");
    let data_dir = root.join("store");
    let source = data_dir.join("archives/source.lar");
    let fixture = write_archive(&source, [82; 16], b"missing archive body");
    let store = Store::open(data_dir.clone()).unwrap();
    let attached = store
        .import_sealed_lar_archive(&source, &LarStandaloneImportOptions::default())
        .unwrap();
    let parked = root.join("parked.lar");
    std::fs::rename(&source, &parked).unwrap();

    let missing = store
        .lar_archive_file_status(&attached.file_uuid)
        .unwrap()
        .unwrap();
    assert_eq!(missing.catalog_state, "sealed");
    assert_eq!(
        missing.availability,
        LarArchiveAvailability::ArchivedMissing
    );
    let error = store
        .read_lar_manifest_body(&fixture.manifest_id)
        .unwrap_err();
    assert_eq!(
        error
            .downcast_ref::<LarArchiveUnavailableError>()
            .unwrap()
            .availability,
        LarArchiveAvailability::ArchivedMissing
    );
    assert!(matches!(
        &store.read_lar_or_legacy_artifact_batch_bounded(
            &[LarArtifactReadRequest::new(
                "trace",
                "trace-archive-ops",
                "client_request",
            )],
            u64::MAX,
        )[0],
        LarArtifactBatchRead::ArchiveUnavailable(error)
            if error.availability == LarArchiveAvailability::ArchivedMissing
    ));

    store
        .reattach_lar_archive(
            &attached.file_uuid,
            &parked,
            &LarArchiveReattachOptions::default(),
        )
        .unwrap();
    assert_eq!(
        store.read_lar_manifest_body(&fixture.manifest_id).unwrap(),
        fixture.body
    );
    let repeated = store
        .reattach_lar_archive(
            &attached.file_uuid,
            &parked,
            &LarArchiveReattachOptions::default(),
        )
        .unwrap();
    assert!(repeated.already_attached);
    assert!(!repeated.relocated);
}

#[test]
fn invalid_candidate_never_switches_offline_catalog_identity_or_path() {
    let root = tmpdir("invalid-candidate");
    let data_dir = root.join("store");
    let source = data_dir.join("archives/source.lar");
    write_archive(&source, [83; 16], b"authoritative body");
    let wrong_uuid = root.join("wrong-uuid.lar");
    write_archive(&wrong_uuid, [84; 16], b"authoritative body");
    let reused_uuid = root.join("reused-uuid.lar");
    write_archive(&reused_uuid, [83; 16], b"different archive bytes");
    let store = Store::open(data_dir.clone()).unwrap();
    let attached = store
        .import_sealed_lar_archive(&source, &LarStandaloneImportOptions::default())
        .unwrap();
    store.detach_lar_archive(&attached.file_uuid).unwrap();
    let before = store
        .lar_archive_file_status(&attached.file_uuid)
        .unwrap()
        .unwrap();

    let wrong = store
        .reattach_lar_archive(
            &attached.file_uuid,
            &wrong_uuid,
            &LarArchiveReattachOptions::default(),
        )
        .unwrap_err();
    assert!(wrong.to_string().contains("header does not match"));
    assert_eq!(catalog_count(&data_dir, "lar_files"), 1);
    assert_eq!(
        store
            .lar_archive_file_status(&attached.file_uuid)
            .unwrap()
            .unwrap(),
        before
    );

    let changed = store
        .reattach_lar_archive(
            &attached.file_uuid,
            &reused_uuid,
            &LarArchiveReattachOptions::default(),
        )
        .unwrap_err();
    assert!(changed.to_string().contains("differs from the cataloged"));
    assert_eq!(
        store
            .lar_archive_file_status(&attached.file_uuid)
            .unwrap()
            .unwrap(),
        before
    );
}

#[test]
fn sealed_live_pack_detaches_rejects_changed_bytes_and_reattaches() {
    let root = tmpdir("sealed-live-pack");
    let data_dir = root.join("store");
    let mut config = live_config();
    config.max_pack_bytes = 1;
    let store = Store::open_with_lar_body_store(data_dir.clone(), config).unwrap();
    let first_body = b"first sealed live pack body";
    let first = store
        .write_body_artifact(
            &LarBodyArtifact::trace("trace-pack-one", "client_request"),
            "request.json",
            first_body,
        )
        .unwrap();
    assert_eq!(first.lar_error, None);
    store
        .write_body_artifact(
            &LarBodyArtifact::trace("trace-pack-two", "client_request"),
            "request.json",
            b"second active pack body",
        )
        .unwrap();

    let statuses = store.lar_archive_file_statuses().unwrap();
    let sealed = statuses
        .iter()
        .find(|file| file.role == "body-pack" && file.catalog_state == "sealed")
        .unwrap()
        .clone();
    let active = statuses
        .iter()
        .find(|file| file.role == "body-pack" && file.catalog_state == "active")
        .unwrap();
    assert!(sealed.identity_validated);
    assert!(store.detach_lar_archive(&active.file_uuid).is_err());
    store.detach_lar_archive(&sealed.file_uuid).unwrap();
    let offline_error = store
        .read_lar_manifest_body(first.manifest_id.as_deref().unwrap())
        .unwrap_err();
    assert_eq!(
        offline_error
            .downcast_ref::<LarArchiveUnavailableError>()
            .unwrap()
            .availability,
        LarArchiveAvailability::ArchivedOffline
    );

    let original = PathBuf::from(&sealed.resolved_path);
    let moved = root.join("cold/sealed-live.lar");
    std::fs::create_dir_all(moved.parent().unwrap()).unwrap();
    std::fs::rename(&original, &moved).unwrap();
    let changed = root.join("changed-same-uuid.lar");
    write_body_pack(
        &changed,
        uuid_bytes(&sealed.file_uuid),
        b"changed immutable bytes",
    );
    let rejected = store
        .reattach_lar_archive(
            &sealed.file_uuid,
            &changed,
            &LarArchiveReattachOptions::default(),
        )
        .unwrap_err();
    assert!(rejected.to_string().contains("differs from the cataloged"));
    assert_eq!(
        store
            .lar_archive_file_status(&sealed.file_uuid)
            .unwrap()
            .unwrap()
            .availability,
        LarArchiveAvailability::ArchivedOffline
    );

    let restored = store
        .reattach_lar_archive(
            &sealed.file_uuid,
            &moved,
            &LarArchiveReattachOptions::default(),
        )
        .unwrap();
    assert!(restored.relocated);
    assert_eq!(restored.file.role, "body-pack");
    assert_eq!(
        store
            .read_lar_manifest_body(first.manifest_id.as_deref().unwrap())
            .unwrap(),
        first_body
    );
}

#[test]
fn legacy_sealed_identity_upgrades_only_from_original_online_path() {
    let root = tmpdir("identity-upgrade");
    let data_dir = root.join("store");
    let source = data_dir.join("archives/source.lar");
    write_archive(&source, [85; 16], b"upgrade from original");
    let store = Store::open(data_dir.clone()).unwrap();
    let attached = store
        .import_sealed_lar_archive(&source, &LarStandaloneImportOptions::default())
        .unwrap();
    let conn = rusqlite::Connection::open(data_dir.join("alexandria.sqlite3")).unwrap();
    conn.execute(
        "DELETE FROM lar_file_identities WHERE file_uuid=?1",
        [&attached.file_uuid],
    )
    .unwrap();
    drop(conn);
    assert!(
        !store
            .lar_archive_file_status(&attached.file_uuid)
            .unwrap()
            .unwrap()
            .identity_validated
    );
    let detached = store.detach_lar_archive(&attached.file_uuid).unwrap();
    assert!(detached.file.identity_validated);

    let missing_root = tmpdir("identity-missing");
    let missing_data = missing_root.join("store");
    let missing_source = missing_data.join("archives/source.lar");
    write_archive(&missing_source, [86; 16], b"never trust replacement");
    let missing_store = Store::open(missing_data.clone()).unwrap();
    let missing_attached = missing_store
        .import_sealed_lar_archive(&missing_source, &LarStandaloneImportOptions::default())
        .unwrap();
    let conn = rusqlite::Connection::open(missing_data.join("alexandria.sqlite3")).unwrap();
    conn.execute(
        "DELETE FROM lar_file_identities WHERE file_uuid=?1",
        [&missing_attached.file_uuid],
    )
    .unwrap();
    drop(conn);
    let replacement = missing_root.join("replacement.lar");
    std::fs::rename(&missing_source, &replacement).unwrap();
    let error = missing_store
        .reattach_lar_archive(
            &missing_attached.file_uuid,
            &replacement,
            &LarArchiveReattachOptions::default(),
        )
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("original catalog path is missing"));
    let unchanged = missing_store
        .lar_archive_file_status(&missing_attached.file_uuid)
        .unwrap()
        .unwrap();
    assert_eq!(unchanged.catalog_path, "archives/source.lar");
    assert_eq!(
        unchanged.availability,
        LarArchiveAvailability::ArchivedMissing
    );
    assert!(!unchanged.identity_validated);
}

#[test]
fn sealed_event_log_uses_the_same_stable_identity_detach_and_reattach_path() {
    let root = tmpdir("event-log-identity");
    let data_dir = root.join("store");
    let source = data_dir.join("events/source.lar");
    let file_uuid = "57575757575757575757575757575757";
    write_event_log(&source, uuid_bytes(file_uuid), b"event log body bytes");
    let store = Store::open(data_dir.clone()).unwrap();
    {
        let conn = rusqlite::Connection::open(data_dir.join("alexandria.sqlite3")).unwrap();
        conn.execute(
            "INSERT INTO lar_archive_sets
               (archive_set_uuid, created_at_ms, updated_at_ms, state, description)
             VALUES ('event-set', 1, 1, 'sealed', 'event log test')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO lar_files
               (file_uuid, archive_set_uuid, role, path, state, container_major,
                container_minor, created_at_ms, sealed_at_ms, size_bytes)
             VALUES (?1, 'event-set', 'event-log', 'events/source.lar', 'sealed',
                     1, 0, 1, 1, ?2)",
            rusqlite::params![file_uuid, std::fs::metadata(&source).unwrap().len()],
        )
        .unwrap();
    }
    assert!(
        !store
            .lar_archive_file_status(file_uuid)
            .unwrap()
            .unwrap()
            .identity_validated
    );
    let detached = store.detach_lar_archive(file_uuid).unwrap();
    assert_eq!(detached.file.role, "event-log");
    assert!(detached.file.identity_validated);
    assert_eq!(
        detached.file.availability,
        LarArchiveAvailability::ArchivedOffline
    );

    let moved = root.join("cold/event-log.lar");
    std::fs::create_dir_all(moved.parent().unwrap()).unwrap();
    std::fs::rename(&source, &moved).unwrap();
    let restored = store
        .reattach_lar_archive(file_uuid, &moved, &LarArchiveReattachOptions::default())
        .unwrap();
    assert_eq!(restored.file.role, "event-log");
    assert_eq!(restored.file.availability, LarArchiveAvailability::Online);
    assert!(restored.relocated);
}
