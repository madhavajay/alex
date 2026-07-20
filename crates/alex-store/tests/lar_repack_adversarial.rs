use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use alex_lar::{
    ArchiveReader, ArchiveWriter, BodyManifest, ChunkHash, ChunkRecordDescriptor, ChunkRef,
    ChunkerConfig, Exchange, ExchangeData, ExchangeId, ExchangeMetadataData, FileHeader, Limits,
    ManifestId, Stage, StageData, StageId, StageKind, UnknownExchangeMetadataAttribute,
    REQUIRED_FEATURE_ARCHIVE_SET_BODY_REFS,
};
use alex_store::{LarRepackConfig, Store};
use rusqlite::{params, Connection};

static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn tmpdir(name: &str) -> PathBuf {
    let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "alex-lar-repack-adversarial-{name}-{}-{sequence}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).unwrap();
    path
}

fn hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(output, "{byte:02x}").unwrap();
    }
    output
}

fn open_writer(path: &Path, file_uuid: [u8; 16], required_features: u64) -> ArchiveWriter<File> {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let file = OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .open(path)
        .unwrap();
    let mut header = FileHeader::body_pack(file_uuid, 1_000_000, b"repack-adversarial".to_vec());
    header.required_feature_bits |= required_features;
    ArchiveWriter::create(file, header, ChunkerConfig::default(), Limits::default()).unwrap()
}

fn seal(mut writer: ArchiveWriter<File>) {
    writer.seal().unwrap();
    writer.get_ref().sync_all().unwrap();
}

fn insert_archive_set(conn: &Connection) {
    conn.execute(
        "INSERT INTO lar_archive_sets
           (archive_set_uuid, created_at_ms, updated_at_ms, state, description)
         VALUES ('repack-adversarial-set', 1, 1, 'sealed', 'adversarial repack test')",
        [],
    )
    .unwrap();
}

fn insert_file(conn: &Connection, file_uuid: &str, path: &Path, created_at_ms: i64) {
    conn.execute(
        "INSERT INTO lar_files
           (file_uuid, archive_set_uuid, role, path, state, container_major,
            container_minor, required_feature_bits, optional_feature_bits,
            created_at_ms, sealed_at_ms, size_bytes)
         VALUES (?1, 'repack-adversarial-set', 'body-pack', ?2, 'sealed',
                 1, 0, 0, 0, ?3, ?3, ?4)",
        params![
            file_uuid,
            path.to_string_lossy(),
            created_at_ms,
            fs::metadata(path).unwrap().len(),
        ],
    )
    .unwrap();
}

fn insert_chunk(
    conn: &Connection,
    file_uuid: &str,
    descriptor: ChunkRecordDescriptor,
    created_at_ms: i64,
) {
    conn.execute(
        "INSERT INTO lar_chunks
           (hash_algorithm, chunk_hash, uncompressed_length, compression,
            compressed_length, file_uuid, record_id, page_offset, record_offset,
            checksum, created_at_ms, state)
         VALUES ('blake3', ?1, ?2, 'zstd', ?3, ?4, ?5, ?6, ?6, ?1, ?7, 'ready')",
        params![
            descriptor.hash.digest.as_slice(),
            descriptor.uncompressed_length,
            descriptor.compressed_length,
            file_uuid,
            format!("chunk:{}", hex(&descriptor.hash.digest)),
            descriptor.frame_offset,
            created_at_ms,
        ],
    )
    .unwrap();
}

fn insert_manifest(conn: &Connection, file_uuid: &str, manifest: &BodyManifest) {
    conn.execute(
        "INSERT INTO lar_manifests
           (manifest_id, total_length, hash_algorithm, whole_body_hash,
            file_uuid, record_id, created_at_ms, state)
         VALUES (?1, ?2, 'blake3', ?3, ?4, ?1, 1, 'ready')",
        params![
            manifest.id.to_string(),
            manifest.total_length,
            manifest.whole_body_hash.digest.as_slice(),
            file_uuid,
        ],
    )
    .unwrap();
    for (ordinal, reference) in manifest.chunks.iter().enumerate() {
        conn.execute(
            "INSERT INTO lar_manifest_chunks
               (manifest_id, ordinal, hash_algorithm, chunk_hash,
                logical_offset, chunk_offset, length)
             VALUES (?1, ?2, 'blake3', ?3, ?4, ?5, ?6)",
            params![
                manifest.id.to_string(),
                ordinal as u64,
                reference.chunk_hash.digest.as_slice(),
                reference.logical_offset,
                reference.chunk_offset,
                reference.length,
            ],
        )
        .unwrap();
    }
}

fn insert_stage_occurrence(
    conn: &Connection,
    occurrence_id: &str,
    trace_id: &str,
    sequence: u64,
    file_uuid: &str,
    canonical_id: StageId,
    request_manifest: Option<ManifestId>,
) {
    conn.execute(
        "INSERT INTO lar_stage_records
           (stage_id, trace_id, capture_sequence, kind, wall_time_ns,
            request_body_manifest_ref, file_uuid, record_id, fidelity)
         VALUES (?1, ?2, ?3, 'client_request', 1, ?4, ?5, ?6, 'captured')",
        params![
            occurrence_id,
            trace_id,
            sequence,
            request_manifest.map(|id| id.to_string()),
            file_uuid,
            canonical_id.to_string(),
        ],
    )
    .unwrap();
}

fn insert_exchange(
    conn: &Connection,
    trace_id: &str,
    exchange_id: ExchangeId,
    sequence: u64,
    stage_count: u64,
    file_uuid: &str,
) {
    conn.execute(
        "INSERT INTO lar_exchange_records
           (trace_id, exchange_id, capture_sequence, stage_count, file_uuid, fidelity)
         VALUES (?1, ?2, ?3, ?4, ?5, 'captured')",
        params![
            trace_id,
            exchange_id.to_string(),
            sequence,
            stage_count,
            file_uuid,
        ],
    )
    .unwrap();
}

fn repack_config() -> LarRepackConfig {
    LarRepackConfig {
        min_garbage_bytes: 1,
        min_garbage_ratio: 0.000_001,
    }
}

#[test]
fn cross_pack_manifest_stays_external_and_shared_chunk_is_not_copied() {
    let root = tmpdir("cross-pack");
    let store = Store::open(root.clone()).unwrap();
    let external_uuid_bytes = [0x31; 16];
    let source_uuid_bytes = [0x32; 16];
    let external_uuid = hex(&external_uuid_bytes);
    let source_uuid = hex(&source_uuid_bytes);
    let external_path = root.join("lar/external/body-external.lar");
    let source_path = root.join("lar/combined/body-source.lar");

    let shared_bytes = vec![b's'; 48 * 1024];
    let owned_bytes = vec![b'o'; 48 * 1024];
    let garbage_bytes = vec![b'g'; 96 * 1024];

    let mut external_writer = open_writer(&external_path, external_uuid_bytes, 0);
    let external_shared = external_writer.append_chunk_record(&shared_bytes).unwrap();
    seal(external_writer);

    let mut source_writer = open_writer(&source_path, source_uuid_bytes, 0);
    let source_shared = source_writer.append_chunk_record(&shared_bytes).unwrap();
    assert_eq!(source_shared.hash, external_shared.hash);
    let source_owned = source_writer.append_chunk_record(&owned_bytes).unwrap();
    let source_garbage = source_writer.append_chunk_record(&garbage_bytes).unwrap();
    let mut body = shared_bytes.clone();
    body.extend_from_slice(&owned_bytes);
    let manifest = BodyManifest::new(
        body.len() as u64,
        ChunkHash::blake3(&body),
        None,
        None,
        vec![
            ChunkRef {
                chunk_hash: source_shared.hash,
                chunk_offset: 0,
                logical_offset: 0,
                length: shared_bytes.len() as u64,
            },
            ChunkRef {
                chunk_hash: source_owned.hash,
                chunk_offset: 0,
                logical_offset: shared_bytes.len() as u64,
                length: owned_bytes.len() as u64,
            },
        ],
    );
    source_writer
        .append_manifest_record(manifest.clone())
        .unwrap();
    let mut stage_data = StageData::new(StageKind::ClientRequest, 1);
    stage_data.request_body_manifest_ref = Some(manifest.id);
    let stage_id = source_writer.append_stage(Stage::new(stage_data)).unwrap();
    let exchange_id = source_writer
        .append_exchange(Exchange::new(ExchangeData::new(
            b"trace-cross-pack".to_vec(),
            1,
            1,
            vec![stage_id],
        )))
        .unwrap();
    seal(source_writer);

    let catalog_path = root.join("alexandria.sqlite3");
    let conn = Connection::open(&catalog_path).unwrap();
    insert_archive_set(&conn);
    insert_file(&conn, &external_uuid, &external_path, 1);
    insert_file(&conn, &source_uuid, &source_path, 2);
    insert_chunk(&conn, &external_uuid, external_shared, 1);
    insert_chunk(&conn, &source_uuid, source_owned, 2);
    insert_chunk(&conn, &source_uuid, source_garbage, 2);
    insert_manifest(&conn, &source_uuid, &manifest);
    insert_stage_occurrence(
        &conn,
        "cross-pack-occurrence",
        "trace-cross-pack",
        0,
        &source_uuid,
        stage_id,
        Some(manifest.id),
    );
    insert_exchange(&conn, "trace-cross-pack", exchange_id, 1, 1, &source_uuid);
    conn.execute(
        "INSERT INTO traces (id, ts_request_ms) VALUES ('trace-cross-pack', 1)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO lar_trace_artifacts
           (owner_kind, owner_id, artifact_kind, stage_id, manifest_id,
            fidelity, validation_state, validated_at_ms)
         VALUES ('trace', 'trace-cross-pack', 'client_request', '', ?1,
                 'captured', 'validated', 1)",
        [manifest.id.to_string()],
    )
    .unwrap();
    drop(conn);

    let candidates = store.plan_lar_repacks(&repack_config()).unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].source_file_uuid, source_uuid);
    let report = store.run_lar_repack(&repack_config(), 10).unwrap().unwrap();
    assert_eq!(report.state, "complete");

    let replacement = ArchiveReader::open(
        File::open(&report.destination_path).unwrap(),
        Limits::default(),
    )
    .unwrap();
    let replacement_hashes = replacement
        .chunk_records()
        .map(|value| value.hash)
        .collect::<Vec<_>>();
    assert_eq!(replacement_hashes, vec![source_owned.hash]);
    assert_eq!(replacement.manifest_count(), 0);
    assert!(
        replacement.header().required_feature_bits & REQUIRED_FEATURE_ARCHIVE_SET_BODY_REFS != 0
    );
    assert!(replacement.stage(&stage_id).is_some());
    assert!(replacement.exchange(&exchange_id).is_some());
    assert_eq!(
        store
            .read_lar_or_legacy_artifact("trace", "trace-cross-pack", "client_request", None,)
            .unwrap()
            .unwrap(),
        body
    );
    let verification = store.verify_lar_migration().unwrap();
    assert!(verification.valid, "{:?}", verification.issues);
    assert_eq!(verification.manifests_checked, 1);
    assert_eq!(verification.bytes_reconstructed, body.len() as u64);

    let conn = Connection::open(catalog_path).unwrap();
    let shared_location: String = conn
        .query_row(
            "SELECT file_uuid FROM lar_chunks
              WHERE hash_algorithm='blake3' AND chunk_hash=?1 AND state='ready'",
            [external_shared.hash.digest.as_slice()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(shared_location, external_uuid);
    let owned_location: String = conn
        .query_row(
            "SELECT file_uuid FROM lar_chunks
              WHERE hash_algorithm='blake3' AND chunk_hash=?1 AND state='ready'",
            [source_owned.hash.digest.as_slice()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(owned_location, report.destination_file_uuid);
    let manifest_location: (Option<String>, Option<String>) = conn
        .query_row(
            "SELECT file_uuid, record_id FROM lar_manifests WHERE manifest_id=?1",
            [manifest.id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(manifest_location, (None, None));
}

#[test]
fn repeated_shared_stage_and_metadata_companion_states_survive_repack() {
    let root = tmpdir("stage-occurrences-metadata");
    let store = Store::open(root.clone()).unwrap();
    let source_uuid_bytes = [0x41; 16];
    let source_uuid = hex(&source_uuid_bytes);
    let source_path = root.join("lar/combined/body-occurrences.lar");
    let mut writer = open_writer(&source_path, source_uuid_bytes, 0);
    let garbage = writer.append_chunk_record(&vec![b'x'; 128 * 1024]).unwrap();
    let canonical_stage = writer
        .append_stage(Stage::new(StageData::new(StageKind::ClientRequest, 1)))
        .unwrap();

    let repeated = Exchange::new(ExchangeData::new(
        b"trace-repeat".to_vec(),
        1,
        1,
        vec![canonical_stage, canonical_stage],
    ));
    let rich_metadata = ExchangeMetadataData {
        ts_request_ms: Some(11),
        ts_response_ms: Some(12),
        harness: Some(b"pi".to_vec()),
        status: Some(201),
        unknown_attributes: vec![UnknownExchangeMetadataAttribute {
            key: b"x.repack.future".to_vec(),
            value: b"preserve-exactly".to_vec(),
        }],
        ..ExchangeMetadataData::default()
    };
    let repeated_id = writer
        .append_exchange_with_metadata(repeated, rich_metadata.clone())
        .unwrap();
    let shared_id = writer
        .append_exchange(Exchange::new(ExchangeData::new(
            b"trace-shared".to_vec(),
            2,
            2,
            vec![canonical_stage],
        )))
        .unwrap();
    let zero_id = writer
        .append_exchange_with_metadata(
            Exchange::new(ExchangeData::new(b"trace-zero".to_vec(), 3, 3, Vec::new())),
            ExchangeMetadataData::default(),
        )
        .unwrap();
    seal(writer);

    let catalog_path = root.join("alexandria.sqlite3");
    let conn = Connection::open(&catalog_path).unwrap();
    insert_archive_set(&conn);
    insert_file(&conn, &source_uuid, &source_path, 1);
    insert_chunk(&conn, &source_uuid, garbage, 1);
    insert_stage_occurrence(
        &conn,
        "occurrence-repeat-0",
        "trace-repeat",
        0,
        &source_uuid,
        canonical_stage,
        None,
    );
    insert_stage_occurrence(
        &conn,
        "occurrence-repeat-1",
        "trace-repeat",
        1,
        &source_uuid,
        canonical_stage,
        None,
    );
    insert_stage_occurrence(
        &conn,
        "occurrence-shared-0",
        "trace-shared",
        0,
        &source_uuid,
        canonical_stage,
        None,
    );
    insert_exchange(&conn, "trace-repeat", repeated_id, 1, 2, &source_uuid);
    insert_exchange(&conn, "trace-shared", shared_id, 2, 1, &source_uuid);
    insert_exchange(&conn, "trace-zero", zero_id, 3, 0, &source_uuid);
    drop(conn);

    let report = store.run_lar_repack(&repack_config(), 20).unwrap().unwrap();
    assert_eq!(report.state, "complete");
    let replacement = ArchiveReader::open(
        File::open(&report.destination_path).unwrap(),
        Limits::default(),
    )
    .unwrap();
    assert_eq!(replacement.chunk_count(), 0);
    assert_eq!(replacement.stage_count(), 1);
    assert_eq!(replacement.exchange_count(), 3);
    assert_eq!(
        replacement.exchange(&repeated_id).unwrap().data.stages,
        vec![canonical_stage, canonical_stage]
    );
    assert_eq!(
        replacement.exchange(&shared_id).unwrap().data.stages,
        vec![canonical_stage]
    );
    assert!(replacement
        .exchange(&zero_id)
        .unwrap()
        .data
        .stages
        .is_empty());
    assert_eq!(
        replacement.exchange_metadata(&repeated_id).unwrap().data,
        rich_metadata
    );
    assert!(replacement.exchange_metadata(&shared_id).is_none());
    assert_eq!(
        replacement.exchange_metadata(&zero_id).unwrap().data,
        ExchangeMetadataData::default(),
        "an explicitly present default companion must not collapse to absence"
    );

    let conn = Connection::open(catalog_path).unwrap();
    let mut statement = conn
        .prepare(
            "SELECT stage_id, file_uuid, record_id FROM lar_stage_records
              ORDER BY trace_id, capture_sequence",
        )
        .unwrap();
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(
        rows.iter()
            .map(|row| row.0.as_str())
            .collect::<std::collections::BTreeSet<_>>(),
        std::collections::BTreeSet::from([
            "occurrence-repeat-0",
            "occurrence-repeat-1",
            "occurrence-shared-0",
        ])
    );
    assert!(rows.iter().all(|row| row.1 == report.destination_file_uuid));
    assert!(rows.iter().all(|row| row.2 == canonical_stage.to_string()));
}
