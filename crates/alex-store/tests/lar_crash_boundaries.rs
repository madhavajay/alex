use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use alex_core::TraceRecord;
use alex_lar::{ArchiveReader, ArchiveWriter, ChunkerConfig, Limits, RecoveryStatus};
use alex_store::{LarBodyArtifact, LarBodyStoreConfig, LarBodyStoreMode, LarRepackConfig, Store};
use rusqlite::Connection;

static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn tmpdir(name: &str) -> PathBuf {
    let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "alex-lar-crash-{name}-{}-{sequence}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn config(max_pack_bytes: u64) -> LarBodyStoreConfig {
    LarBodyStoreConfig {
        mode: LarBodyStoreMode::LarWithFallback,
        max_pack_bytes,
        ..LarBodyStoreConfig::default()
    }
}

fn database(root: &Path) -> Connection {
    Connection::open(root.join("alexandria.sqlite3")).unwrap()
}

fn active_pack(root: &Path) -> (String, PathBuf) {
    database(root)
        .query_row(
            "SELECT file_uuid, path FROM lar_files WHERE state='active'
             ORDER BY created_at_ms DESC, file_uuid DESC LIMIT 1",
            [],
            |row| Ok((row.get(0)?, PathBuf::from(row.get::<_, String>(1)?))),
        )
        .unwrap()
}

fn insert_trace(store: &Store, id: &str, legacy_path: String, timestamp: i64) {
    store
        .insert_trace(&TraceRecord {
            id: id.into(),
            session_id: Some(format!("session-{id}")),
            ts_request_ms: timestamp,
            req_body_path: Some(legacy_path),
            status: Some(200),
            ..TraceRecord::default()
        })
        .unwrap();
}

fn deterministic_bytes(seed: u64, length: usize) -> Vec<u8> {
    let mut state = seed;
    (0..length)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        })
        .collect()
}

/// Every published LAR body pointer must resolve through its catalog manifest
/// and hash-verified physical chunks. Reading by manifest deliberately avoids
/// the legacy fallback, so corrupt or missing LAR bytes cannot be masked.
fn assert_every_published_lar_pointer_is_readable(store: &Store, root: &Path) {
    let conn = database(root);
    let mut statement = conn
        .prepare(
            "SELECT manifest_id FROM lar_trace_artifacts
              WHERE validation_state='validated' AND manifest_id IS NOT NULL
             UNION
             SELECT request_body_manifest_ref FROM lar_stage_records
              WHERE request_body_manifest_ref IS NOT NULL
             UNION
             SELECT response_body_manifest_ref FROM lar_stage_records
              WHERE response_body_manifest_ref IS NOT NULL
             ORDER BY 1",
        )
        .unwrap();
    let ids = statement
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    drop(statement);
    drop(conn);
    for id in ids {
        store
            .read_lar_manifest_body(&id)
            .unwrap_or_else(|error| panic!("published manifest {id} is unreadable: {error:#}"));
    }
}

fn manifest_pack_uuids(root: &Path, manifest_id: &str) -> Vec<String> {
    let conn = database(root);
    let mut statement = conn
        .prepare(
            "SELECT DISTINCT c.file_uuid
               FROM lar_manifest_chunks mc
               JOIN lar_chunks c
                 ON c.hash_algorithm=mc.hash_algorithm AND c.chunk_hash=mc.chunk_hash
              WHERE mc.manifest_id=?1
              ORDER BY c.file_uuid",
        )
        .unwrap();
    let rows = statement
        .query_map([manifest_id], |row| row.get(0))
        .unwrap();
    rows.collect::<rusqlite::Result<Vec<_>>>().unwrap()
}

#[test]
fn restart_during_chunk_append_drops_partial_tail_before_publication() {
    let root = tmpdir("chunk-append");
    let body_before = b"complete body before interrupted chunk append".to_vec();
    let body_during = b"different body whose chunk append is interrupted".to_vec();
    let store = Store::open_with_lar_body_store(root.clone(), config(16 * 1024 * 1024)).unwrap();
    let before = store
        .write_body_artifact(
            &LarBodyArtifact::trace("chunk-before", "client_request"),
            "request.json",
            &body_before,
        )
        .unwrap();
    insert_trace(&store, "chunk-before", before.legacy_path, 1);
    let manifest_before = before.manifest_id.unwrap();
    let (_, pack_path) = active_pack(&root);
    let valid_length = std::fs::metadata(&pack_path).unwrap().len();

    store.inject_lar_disk_full_during_append_once();
    let interrupted = store
        .write_body_artifact(
            &LarBodyArtifact::trace("chunk-interrupted", "client_request"),
            "request.json",
            &body_during,
        )
        .unwrap();
    assert!(interrupted.manifest_id.is_none());
    assert!(interrupted.lar_error.is_some());
    assert!(std::fs::metadata(&pack_path).unwrap().len() > valid_length);
    assert!(matches!(
        ArchiveReader::open(File::open(&pack_path).unwrap(), Limits::default())
            .unwrap()
            .recovery_status(),
        RecoveryStatus::TruncatedTail { .. }
    ));
    drop(store);

    let reopened = Store::open_with_lar_body_store(root.clone(), config(16 * 1024 * 1024)).unwrap();
    assert_eq!(std::fs::metadata(&pack_path).unwrap().len(), valid_length);
    assert_eq!(
        reopened.read_lar_manifest_body(&manifest_before).unwrap(),
        body_before
    );
    let unpublished: i64 = database(&root)
        .query_row(
            "SELECT COUNT(*) FROM lar_trace_artifacts WHERE owner_id='chunk-interrupted'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(unpublished, 0);
    assert_every_published_lar_pointer_is_readable(&reopened, &root);

    let retry = reopened
        .write_body_artifact(
            &LarBodyArtifact::trace("chunk-after-restart", "client_request"),
            "request.json",
            &body_during,
        )
        .unwrap();
    assert!(retry.lar_error.is_none());
    assert_eq!(
        reopened
            .read_lar_manifest_body(retry.manifest_id.as_deref().unwrap())
            .unwrap(),
        body_during
    );
}

#[test]
fn restart_during_manifest_append_keeps_only_complete_published_manifests() {
    let root = tmpdir("manifest-append");
    let original_body = b"body published before interrupted manifest".to_vec();
    let interrupted_body = deterministic_bytes(17, 400);
    let store = Store::open_with_lar_body_store(root.clone(), config(16 * 1024 * 1024)).unwrap();
    let original = store
        .write_body_artifact(
            &LarBodyArtifact::trace("manifest-before", "client_request"),
            "request.json",
            &original_body,
        )
        .unwrap();
    insert_trace(&store, "manifest-before", original.legacy_path, 1);
    let original_manifest = original.manifest_id.unwrap();
    let (_, pack_path) = active_pack(&root);
    drop(store);

    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&pack_path)
        .unwrap();
    let mut writer =
        ArchiveWriter::open_append(file, ChunkerConfig::default(), Limits::default()).unwrap();
    writer.append_chunk_record(&interrupted_body).unwrap();
    writer.flush().unwrap();
    writer.get_ref().sync_all().unwrap();
    let manifest_start = writer.get_mut().seek(SeekFrom::End(0)).unwrap();
    let interrupted_manifest = writer.append_body(&interrupted_body).unwrap();
    writer.flush().unwrap();
    writer.get_ref().sync_all().unwrap();
    let manifest_end = writer.get_mut().seek(SeekFrom::End(0)).unwrap();
    assert!(manifest_end > manifest_start + 4);
    drop(writer);

    let file = OpenOptions::new().write(true).open(&pack_path).unwrap();
    file.set_len(manifest_end - 4).unwrap();
    file.sync_all().unwrap();
    drop(file);
    assert!(matches!(
        ArchiveReader::open(File::open(&pack_path).unwrap(), Limits::default())
            .unwrap()
            .recovery_status(),
        RecoveryStatus::TruncatedTail { .. }
    ));

    let reopened = Store::open_with_lar_body_store(root.clone(), config(16 * 1024 * 1024)).unwrap();
    assert_eq!(std::fs::metadata(&pack_path).unwrap().len(), manifest_start);
    assert_eq!(
        reopened.read_lar_manifest_body(&original_manifest).unwrap(),
        original_body
    );
    let physically_interrupted_was_never_published: i64 = database(&root)
        .query_row(
            "SELECT COUNT(*) FROM lar_manifests WHERE manifest_id=?1",
            [interrupted_manifest.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(physically_interrupted_was_never_published, 0);
    assert_every_published_lar_pointer_is_readable(&reopened, &root);

    let retry = reopened
        .write_body_artifact(
            &LarBodyArtifact::trace("manifest-after-restart", "client_request"),
            "request.json",
            &interrupted_body,
        )
        .unwrap();
    assert_eq!(
        reopened
            .read_lar_manifest_body(retry.manifest_id.as_deref().unwrap())
            .unwrap(),
        interrupted_body
    );
}

#[test]
fn restart_during_seal_recovers_active_tail_without_truncating_a_sealed_pack() {
    let root = tmpdir("seal");
    let sealed_body = b"body in an immutable sealed pack".to_vec();
    let active_body = b"body in the pack whose seal is interrupted".to_vec();

    let initial = Store::open_with_lar_body_store(root.clone(), config(16 * 1024 * 1024)).unwrap();
    let sealed = initial
        .write_body_artifact(
            &LarBodyArtifact::trace("seal-immutable", "client_request"),
            "request.json",
            &sealed_body,
        )
        .unwrap();
    insert_trace(&initial, "seal-immutable", sealed.legacy_path, 1);
    let sealed_manifest = sealed.manifest_id.unwrap();
    drop(initial);

    // Reopening with a one-byte rotation threshold seals the previous pack
    // before writing the next body, giving this test an immutable control pack.
    let rotating = Store::open_with_lar_body_store(root.clone(), config(1)).unwrap();
    let active = rotating
        .write_body_artifact(
            &LarBodyArtifact::trace("seal-interrupted", "client_request"),
            "request.json",
            &active_body,
        )
        .unwrap();
    insert_trace(&rotating, "seal-interrupted", active.legacy_path, 2);
    let active_manifest = active.manifest_id.unwrap();
    let conn = database(&root);
    let sealed_path: PathBuf = conn
        .query_row(
            "SELECT path FROM lar_files WHERE state='sealed' ORDER BY created_at_ms LIMIT 1",
            [],
            |row| Ok(PathBuf::from(row.get::<_, String>(0)?)),
        )
        .unwrap();
    drop(conn);
    let sealed_bytes = std::fs::read(&sealed_path).unwrap();
    let (_, active_path) = active_pack(&root);
    drop(rotating);

    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&active_path)
        .unwrap();
    let mut writer =
        ArchiveWriter::open_append(file, ChunkerConfig::default(), Limits::default()).unwrap();
    writer.seal().unwrap();
    writer.get_ref().sync_all().unwrap();
    let complete_sealed_length = writer.get_mut().seek(SeekFrom::End(0)).unwrap();
    drop(writer);

    // Leave all seal index records but only a prefix of the fixed footer. This
    // is the last possible interruption point before seal() becomes durable.
    let partial_seal_length = complete_sealed_length - 4;
    let file = OpenOptions::new().write(true).open(&active_path).unwrap();
    file.set_len(partial_seal_length).unwrap();
    file.sync_all().unwrap();
    drop(file);
    let partial =
        ArchiveReader::open(File::open(&active_path).unwrap(), Limits::default()).unwrap();
    assert!(!partial.is_sealed());
    assert_ne!(partial.recovery_status(), RecoveryStatus::Clean);
    drop(partial);

    let reopened = Store::open_with_lar_body_store(root.clone(), config(1)).unwrap();
    assert!(std::fs::metadata(&active_path).unwrap().len() < partial_seal_length);
    let recovered =
        ArchiveReader::open(File::open(&active_path).unwrap(), Limits::default()).unwrap();
    assert!(!recovered.is_sealed());
    assert_eq!(recovered.recovery_status(), RecoveryStatus::Clean);
    drop(recovered);
    assert_eq!(std::fs::read(&sealed_path).unwrap(), sealed_bytes);
    let immutable =
        ArchiveReader::open(File::open(&sealed_path).unwrap(), Limits::default()).unwrap();
    assert!(immutable.is_sealed());
    assert_eq!(immutable.recovery_status(), RecoveryStatus::Clean);
    assert_eq!(
        reopened.read_lar_manifest_body(&sealed_manifest).unwrap(),
        sealed_body
    );
    assert_eq!(
        reopened.read_lar_manifest_body(&active_manifest).unwrap(),
        active_body
    );
    assert_every_published_lar_pointer_is_readable(&reopened, &root);
}

#[test]
fn restart_during_repack_rebuilds_partial_output_before_atomic_switch() {
    let root = tmpdir("repack");
    let keep_body = deterministic_bytes(101, 96 * 1024);
    let garbage_body = deterministic_bytes(202, 128 * 1024);
    let rotate_body = deterministic_bytes(303, 16 * 1024);

    let initial = Store::open_with_lar_body_store(root.clone(), config(16 * 1024 * 1024)).unwrap();
    let keep = initial
        .write_body_artifact(
            &LarBodyArtifact::trace("repack-keep", "client_request"),
            "request.json",
            &keep_body,
        )
        .unwrap();
    insert_trace(&initial, "repack-keep", keep.legacy_path, 1);
    let keep_manifest = keep.manifest_id.unwrap();
    let garbage = initial
        .write_body_artifact(
            &LarBodyArtifact::trace("repack-garbage", "client_request"),
            "request.json",
            &garbage_body,
        )
        .unwrap();
    insert_trace(&initial, "repack-garbage", garbage.legacy_path, 2);
    drop(initial);

    let rotating = Store::open_with_lar_body_store(root.clone(), config(1)).unwrap();
    let rotate = rotating
        .write_body_artifact(
            &LarBodyArtifact::trace("repack-rotate", "client_request"),
            "request.json",
            &rotate_body,
        )
        .unwrap();
    insert_trace(&rotating, "repack-rotate", rotate.legacy_path, 3);
    rotating.delete_trace("repack-garbage").unwrap();

    let repack_config = LarRepackConfig {
        min_garbage_bytes: 1,
        min_garbage_ratio: 0.01,
    };
    let copied = rotating
        .start_lar_repack(&repack_config, 10)
        .unwrap()
        .unwrap();
    assert_eq!(copied.state, "copied");
    assert_eq!(
        rotating.read_lar_manifest_body(&keep_manifest).unwrap(),
        keep_body
    );
    let source_bytes = std::fs::read(&copied.source_path).unwrap();
    let source_uuid = copied.source_file_uuid.clone();
    let destination_uuid = copied.destination_file_uuid.clone();
    let run_id = copied.run_id.clone();
    let destination_path = copied.destination_path.clone();
    drop(rotating);

    // Rewind the durable state to the pre-copy boundary and replace the clean
    // output with a partial first chunk frame. This models process death while
    // copying without adding a test-only production code path.
    let destination_temp_path: PathBuf = database(&root)
        .query_row(
            "SELECT destination_temp_path FROM lar_repack_runs WHERE run_id=?1",
            [&run_id],
            |row| Ok(PathBuf::from(row.get::<_, String>(0)?)),
        )
        .unwrap();
    let reader =
        ArchiveReader::open(File::open(&destination_path).unwrap(), Limits::default()).unwrap();
    let first_chunk = reader.chunk_records().next().unwrap();
    let partial_length = first_chunk.frame_offset + 20 + first_chunk.compressed_length / 2;
    assert!(partial_length < std::fs::metadata(&destination_path).unwrap().len());
    drop(reader);
    std::fs::rename(&destination_path, &destination_temp_path).unwrap();
    let file = OpenOptions::new()
        .write(true)
        .open(&destination_temp_path)
        .unwrap();
    file.set_len(partial_length).unwrap();
    file.sync_all().unwrap();
    drop(file);
    let partial = ArchiveReader::open(
        File::open(&destination_temp_path).unwrap(),
        Limits::default(),
    )
    .unwrap();
    assert!(matches!(
        partial.recovery_status(),
        RecoveryStatus::TruncatedTail { .. }
    ));
    drop(partial);
    let conn = database(&root);
    conn.execute(
        "UPDATE lar_repack_runs
            SET state='copying', destination_size_bytes=0, last_error=NULL
          WHERE run_id=?1",
        [&run_id],
    )
    .unwrap();
    conn.execute(
        "UPDATE lar_repack_chunks
            SET state='planned', destination_offset=NULL,
                destination_compressed_length=NULL
          WHERE run_id=?1",
        [&run_id],
    )
    .unwrap();
    drop(conn);

    let restarted = Store::open_with_lar_body_store(root.clone(), config(1)).unwrap();
    assert_eq!(
        restarted.read_lar_manifest_body(&keep_manifest).unwrap(),
        keep_body
    );
    assert_eq!(
        manifest_pack_uuids(&root, &keep_manifest),
        vec![source_uuid.clone()]
    );
    assert_eq!(std::fs::read(&copied.source_path).unwrap(), source_bytes);
    assert_every_published_lar_pointer_is_readable(&restarted, &root);
    let recopied = restarted.resume_lar_repack(&run_id, 11).unwrap();
    assert_eq!(recopied.state, "copied");
    assert_eq!(
        manifest_pack_uuids(&root, &keep_manifest),
        vec![source_uuid.clone()]
    );
    assert_eq!(std::fs::read(&copied.source_path).unwrap(), source_bytes);
    let replacement =
        ArchiveReader::open(File::open(&destination_path).unwrap(), Limits::default()).unwrap();
    assert!(replacement.is_sealed());
    assert_eq!(replacement.recovery_status(), RecoveryStatus::Clean);
    drop(replacement);
    drop(restarted);

    let restarted = Store::open_with_lar_body_store(root.clone(), config(1)).unwrap();
    let switched = restarted.resume_lar_repack(&run_id, 12).unwrap();
    assert_eq!(switched.state, "switched");
    assert_eq!(
        manifest_pack_uuids(&root, &keep_manifest),
        vec![destination_uuid]
    );
    assert_eq!(
        restarted.read_lar_manifest_body(&keep_manifest).unwrap(),
        keep_body
    );
    assert_eq!(std::fs::read(&copied.source_path).unwrap(), source_bytes);
    assert_every_published_lar_pointer_is_readable(&restarted, &root);
    drop(restarted);

    let restarted = Store::open_with_lar_body_store(root.clone(), config(1)).unwrap();
    let complete = restarted.resume_lar_repack(&run_id, 13).unwrap();
    assert_eq!(complete.state, "complete");
    assert_eq!(
        std::fs::read(&complete.quarantine_path).unwrap(),
        source_bytes
    );
    assert_every_published_lar_pointer_is_readable(&restarted, &root);
}
