//! Verified copy/switch/retire compaction for immutable LAR body packs.
//!
//! The durable plan covers both physical chunks and the complete reachable
//! canonical graph in combined packs. Catalog ownership is switched in one
//! immediate SQLite transaction only after the replacement graph and every
//! reconstructed body have been verified.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use alex_lar::{
    validate_selective_rewrite_source, ArchiveReader, ArchiveWriter, ChunkHash,
    ChunkRecordDescriptor, ChunkerConfig, ConversationEntryId, ExchangeId, GenerationId,
    HashAlgorithm, HeaderBlockId, Limits, ManifestId, RecoveryStatus, StageId, StreamIndexId,
    TurnViewId, REQUIRED_FEATURE_ARCHIVE_SET_BODY_REFS,
};
use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

use crate::{
    lar_archive_ops::{compute_lar_file_identity, record_lar_file_identity},
    Store,
};

static REPACK_RUN_SEQUENCE: AtomicU64 = AtomicU64::new(0);

const REPACK_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS lar_repack_runs (
  run_id                    TEXT PRIMARY KEY,
  archive_set_uuid          TEXT NOT NULL,
  source_file_uuid          TEXT NOT NULL,
  destination_file_uuid     TEXT NOT NULL,
  source_path               TEXT NOT NULL,
  destination_temp_path     TEXT NOT NULL,
  destination_path          TEXT NOT NULL,
  quarantine_path           TEXT NOT NULL,
  state                     TEXT NOT NULL
                            CHECK (state IN ('copying','copied','switched','complete','failed')),
  started_at_ms             INTEGER NOT NULL,
  updated_at_ms             INTEGER NOT NULL,
  completed_at_ms           INTEGER,
  source_size_bytes         INTEGER NOT NULL CHECK (source_size_bytes >= 0),
  source_identity_hash      BLOB CHECK (source_identity_hash IS NULL OR length(source_identity_hash) = 32),
  source_identity_size      INTEGER CHECK (source_identity_size IS NULL OR source_identity_size >= 0),
  destination_size_bytes    INTEGER NOT NULL DEFAULT 0 CHECK (destination_size_bytes >= 0),
  reachable_chunks          INTEGER NOT NULL DEFAULT 0,
  garbage_chunks            INTEGER NOT NULL DEFAULT 0,
  garbage_compressed_bytes  INTEGER NOT NULL DEFAULT 0,
  logical_bytes_reclaimed   INTEGER NOT NULL DEFAULT 0,
  physical_bytes_reclaimed  INTEGER NOT NULL DEFAULT 0,
  last_error                TEXT,
  FOREIGN KEY (archive_set_uuid) REFERENCES lar_archive_sets(archive_set_uuid),
  FOREIGN KEY (source_file_uuid) REFERENCES lar_files(file_uuid)
);
CREATE INDEX IF NOT EXISTS lar_repack_runs_state
  ON lar_repack_runs(state, started_at_ms);

CREATE TABLE IF NOT EXISTS lar_repack_chunks (
  run_id                   TEXT NOT NULL,
  ordinal                  INTEGER NOT NULL CHECK (ordinal >= 0),
  hash_algorithm           TEXT NOT NULL,
  chunk_hash               BLOB NOT NULL,
  uncompressed_length      INTEGER NOT NULL CHECK (uncompressed_length >= 0),
  source_compressed_length INTEGER NOT NULL CHECK (source_compressed_length >= 0),
  destination_offset       INTEGER,
  destination_compressed_length INTEGER,
  state                    TEXT NOT NULL DEFAULT 'planned'
                           CHECK (state IN ('planned','copied','switched')),
  PRIMARY KEY (run_id, ordinal),
  UNIQUE (run_id, hash_algorithm, chunk_hash),
  FOREIGN KEY (run_id) REFERENCES lar_repack_runs(run_id)
);

CREATE TABLE IF NOT EXISTS lar_repack_records (
  run_id      TEXT NOT NULL,
  record_kind TEXT NOT NULL CHECK (record_kind IN
              ('manifest','external_manifest','header','stream','stage','exchange',
               'conversation_entry','generation','turn_view')),
  record_id   BLOB NOT NULL CHECK (length(record_id) = 32),
  PRIMARY KEY (run_id, record_kind, record_id),
  FOREIGN KEY (run_id) REFERENCES lar_repack_runs(run_id)
);
"#;

const REACHABLE_CHUNKS_CTE: &str = r#"
WITH reachable_manifests(manifest_id) AS (
  SELECT manifest_id
    FROM lar_trace_artifacts
   WHERE validation_state='validated' AND manifest_id IS NOT NULL
  UNION
  SELECT request_body_manifest_ref
    FROM lar_stage_records
   WHERE request_body_manifest_ref IS NOT NULL
  UNION
  SELECT response_body_manifest_ref
    FROM lar_stage_records
   WHERE response_body_manifest_ref IS NOT NULL
  UNION
  SELECT manifest_id
    FROM lar_conversation_entry_ranges
  UNION
  SELECT destination_manifest_id
    FROM lar_migration_items
   WHERE destination_manifest_id IS NOT NULL
),
reachable_chunks(hash_algorithm, chunk_hash) AS (
  SELECT DISTINCT mc.hash_algorithm, mc.chunk_hash
    FROM lar_manifest_chunks mc
    JOIN reachable_manifests rm ON rm.manifest_id=mc.manifest_id
)
"#;

#[derive(Clone, Debug, PartialEq)]
pub struct LarRepackConfig {
    pub min_garbage_bytes: u64,
    pub min_garbage_ratio: f64,
}

impl Default for LarRepackConfig {
    fn default() -> Self {
        Self {
            min_garbage_bytes: 64 * 1024 * 1024,
            min_garbage_ratio: 0.25,
        }
    }
}

impl LarRepackConfig {
    fn validate(&self) -> Result<()> {
        if !self.min_garbage_ratio.is_finite() || !(0.0..=1.0).contains(&self.min_garbage_ratio) {
            bail!("LAR repack garbage ratio must be between 0 and 1");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct LarRepackCandidate {
    pub source_file_uuid: String,
    pub source_path: PathBuf,
    pub source_size_bytes: u64,
    pub total_chunks: u64,
    pub reachable_chunks: u64,
    pub garbage_chunks: u64,
    pub total_compressed_bytes: u64,
    pub garbage_compressed_bytes: u64,
    pub garbage_ratio: f64,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct LarRepackReport {
    pub run_id: String,
    pub state: String,
    pub source_file_uuid: String,
    pub destination_file_uuid: String,
    pub source_path: PathBuf,
    pub destination_path: PathBuf,
    pub quarantine_path: PathBuf,
    pub source_size_bytes: u64,
    pub destination_size_bytes: u64,
    pub reachable_chunks: u64,
    pub garbage_chunks: u64,
    pub garbage_compressed_bytes: u64,
    /// Size reduction represented by the replacement pack. The quarantined
    /// source remains on disk in v1, so this is not physical disk reclamation.
    pub logical_bytes_reclaimed: u64,
    pub physical_bytes_reclaimed: u64,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug)]
struct PlannedChunk {
    ordinal: u64,
    hash: ChunkHash,
    uncompressed_length: u64,
    source_compressed_length: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct CanonicalPlan {
    manifests: BTreeSet<ManifestId>,
    /// Reachable manifest catalog rows that remain logical archive-set
    /// objects but cannot be emitted locally without duplicating a chunk whose
    /// canonical physical location is another pack.
    external_manifests: BTreeSet<ManifestId>,
    headers: BTreeSet<HeaderBlockId>,
    streams: BTreeSet<StreamIndexId>,
    stages: BTreeSet<StageId>,
    exchanges: BTreeSet<ExchangeId>,
    conversation_entries: BTreeSet<ConversationEntryId>,
    generations: BTreeSet<GenerationId>,
    turn_views: BTreeSet<TurnViewId>,
}

#[derive(Clone, Debug)]
struct RepackPlan {
    chunks: Vec<PlannedChunk>,
    canonical: CanonicalPlan,
}

#[derive(Clone, Debug)]
struct RepackRun {
    run_id: String,
    archive_set_uuid: String,
    source_file_uuid: String,
    destination_file_uuid: String,
    source_path: PathBuf,
    destination_temp_path: PathBuf,
    destination_path: PathBuf,
    quarantine_path: PathBuf,
    state: String,
    source_identity: Option<crate::lar_archive_ops::LarFileIdentity>,
}

pub(crate) fn migrate(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute_batch(REPACK_SCHEMA)?;
    for (name, definition) in [
        ("source_identity_hash", "BLOB"),
        ("source_identity_size", "INTEGER"),
    ] {
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('lar_repack_runs') WHERE name=?1)",
            [name],
            |row| row.get(0),
        )?;
        if !exists {
            tx.execute_batch(&format!(
                "ALTER TABLE lar_repack_runs ADD COLUMN {name} {definition}"
            ))?;
        }
    }
    tx.execute(
        "UPDATE lar_repack_runs
            SET state='failed',
                last_error='repack plan predates durable source identity; start a new run'
          WHERE state IN ('copying','copied','switched')
            AND (source_identity_hash IS NULL OR source_identity_size IS NULL)",
        [],
    )?;
    tx.commit()?;
    Ok(())
}

fn nonnegative(value: i64, field: &str) -> Result<u64> {
    value
        .try_into()
        .with_context(|| format!("negative LAR repack {field}: {value}"))
}

fn as_i64(value: u64, field: &str) -> Result<i64> {
    value
        .try_into()
        .with_context(|| format!("LAR repack {field} exceeds SQLite integer range"))
}

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn run_identity(now_ms: i64) -> (String, [u8; 16], String) {
    let run_id = format!(
        "repack-{now_ms}-{}-{}",
        std::process::id(),
        REPACK_RUN_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    );
    let digest = blake3::hash(run_id.as_bytes());
    let mut uuid = [0; 16];
    uuid.copy_from_slice(&digest.as_bytes()[..16]);
    let file_uuid = hex(&uuid);
    (run_id, uuid, file_uuid)
}

fn digest32(bytes: Vec<u8>, field: &str) -> Result<[u8; 32]> {
    bytes.try_into().map_err(|bytes: Vec<u8>| {
        anyhow::anyhow!("{field} has length {}, expected 32", bytes.len())
    })
}

fn managed_lar_path(data_dir: &Path, path: &Path) -> Result<bool> {
    let root = data_dir.join("lar");
    let root = root
        .canonicalize()
        .with_context(|| format!("canonicalizing managed LAR root {}", root.display()))?;
    let path = path
        .canonicalize()
        .with_context(|| format!("canonicalizing LAR pack {}", path.display()))?;
    Ok(path.starts_with(root))
}

fn reachable_hashes_for_file(conn: &Connection, file_uuid: &str) -> Result<BTreeSet<[u8; 32]>> {
    let sql = format!(
        "{REACHABLE_CHUNKS_CTE}
         SELECT c.chunk_hash
           FROM lar_chunks c
           JOIN reachable_chunks rc
             ON rc.hash_algorithm=c.hash_algorithm AND rc.chunk_hash=c.chunk_hash
          WHERE c.file_uuid=?1 AND c.hash_algorithm='blake3' AND c.state='ready'
          ORDER BY c.chunk_hash"
    );
    let mut statement = conn.prepare(&sql)?;
    let rows = statement.query_map([file_uuid], |row| row.get::<_, Vec<u8>>(0))?;
    rows.map(|row| digest32(row?, "catalog chunk hash"))
        .collect()
}

fn query_record_ids(conn: &Connection, sql: &str, file_uuid: &str) -> Result<Vec<[u8; 32]>> {
    let mut statement = conn.prepare(sql)?;
    let rows = statement.query_map([file_uuid], |row| row.get::<_, String>(0))?;
    rows.map(|row| {
        let value = row?;
        digest32(
            hex_to_bytes(&value).with_context(|| format!("decoding catalog record ID {value}"))?,
            "catalog record ID",
        )
    })
    .collect()
}

fn build_canonical_plan<R: std::io::Read + std::io::Seek>(
    conn: &Connection,
    file_uuid: &str,
    reader: &ArchiveReader<R>,
) -> Result<CanonicalPlan> {
    let mut plan = CanonicalPlan::default();
    let source_owned_manifests = query_record_ids(
        conn,
        "SELECT manifest_id FROM lar_manifests
          WHERE file_uuid=?1 AND state='ready'",
        file_uuid,
    )?
    .into_iter()
    .map(ManifestId)
    .collect::<BTreeSet<_>>();

    for id in query_record_ids(
        conn,
        "SELECT exchange_id FROM lar_exchange_records WHERE file_uuid=?1
         UNION
         SELECT destination_exchange_id FROM lar_migration_items
          WHERE destination_file_uuid=?1 AND destination_exchange_id IS NOT NULL",
        file_uuid,
    )? {
        let id = ExchangeId(id);
        if reader.exchange(&id).is_none() {
            bail!("catalog-owned exchange {id} is missing from source LAR pack");
        }
        plan.exchanges.insert(id);
    }

    // Pre-v3 catalogs did not have explicit exchange ownership. Keep any
    // canonical stage that can still be resolved by its original record ID;
    // modern exchange roots below pull in repeated occurrence-scoped stages.
    for id in query_record_ids(
        conn,
        "SELECT record_id FROM lar_stage_records
          WHERE file_uuid=?1 AND record_id IS NOT NULL",
        file_uuid,
    )? {
        let id = StageId(id);
        if reader.stage(&id).is_some() {
            plan.stages.insert(id);
        }
    }

    for id in query_record_ids(
        conn,
        "SELECT manifest_id FROM lar_manifests
          WHERE file_uuid=?1 AND state='ready'",
        file_uuid,
    )? {
        let id = ManifestId(id);
        if reader.manifest(&id).is_none() {
            bail!("catalog-owned manifest {id} is missing from source LAR pack");
        }
        plan.manifests.insert(id);
    }

    for id in query_record_ids(
        conn,
        "SELECT block_id FROM lar_header_blocks WHERE file_uuid=?1",
        file_uuid,
    )? {
        let id = HeaderBlockId(id);
        if reader.header_block(&id).is_none() {
            bail!("catalog-owned header block {id} is missing from source LAR pack");
        }
        plan.headers.insert(id);
    }

    let catalog_turns = query_record_ids(
        conn,
        "SELECT turn_view_id FROM lar_conversation_turn_views
          WHERE EXISTS (SELECT 1 FROM lar_files WHERE file_uuid=?1)",
        file_uuid,
    )?
    .into_iter()
    .collect::<BTreeSet<_>>();
    for id in reader.turn_view_ids().copied() {
        if catalog_turns.contains(&id.0) {
            plan.turn_views.insert(id);
        }
    }
    let catalog_generations = query_record_ids(
        conn,
        "SELECT generation_id FROM lar_conversation_session_generations
          WHERE EXISTS (SELECT 1 FROM lar_files WHERE file_uuid=?1)",
        file_uuid,
    )?
    .into_iter()
    .collect::<BTreeSet<_>>();
    for id in reader.generation_ids().copied() {
        if catalog_generations.contains(&id.0) {
            plan.generations.insert(id);
        }
    }

    // Close the graph transitively. A valid source guarantees non-manifest
    // dependencies are local; archive-set body references may legitimately
    // resolve through a different pack and are therefore included only when
    // this source owns the manifest record.
    loop {
        let before = (
            plan.manifests.len(),
            plan.external_manifests.len(),
            plan.headers.len(),
            plan.streams.len(),
            plan.stages.len(),
            plan.exchanges.len(),
            plan.conversation_entries.len(),
            plan.generations.len(),
            plan.turn_views.len(),
        );
        for id in plan.exchanges.iter().copied().collect::<Vec<_>>() {
            let exchange = reader
                .exchange(&id)
                .with_context(|| format!("planned exchange {id} disappeared"))?;
            plan.stages.extend(exchange.data.stages.iter().copied());
        }
        for id in plan.stages.iter().copied().collect::<Vec<_>>() {
            let stage = reader
                .stage(&id)
                .with_context(|| format!("planned stage {id} is missing"))?;
            plan.headers.extend(
                [
                    stage.data.request_headers_ref,
                    stage.data.response_headers_ref,
                    stage.data.trailers_ref,
                ]
                .into_iter()
                .flatten(),
            );
            plan.streams.extend(stage.data.stream_index_ref);
            for manifest in [
                stage.data.request_body_manifest_ref,
                stage.data.response_body_manifest_ref,
            ]
            .into_iter()
            .flatten()
            {
                if source_owned_manifests.contains(&manifest) {
                    plan.manifests.insert(manifest);
                }
            }
        }
        for id in plan.streams.iter().copied().collect::<Vec<_>>() {
            let stream = reader
                .stream_index(&id)
                .with_context(|| format!("planned stream index {id} is missing"))?;
            if source_owned_manifests.contains(&stream.raw_body_manifest_id) {
                plan.manifests.insert(stream.raw_body_manifest_id);
            }
        }
        for id in plan.turn_views.iter().copied().collect::<Vec<_>>() {
            let turn = reader
                .turn_view(&id)
                .with_context(|| format!("planned turn view {id} is missing"))?;
            let exchange = reader
                .exchange_by_trace(&turn.data.trace_id)
                .with_context(|| format!("turn view {id} is missing its exchange"))?;
            plan.exchanges.insert(exchange.id);
            plan.generations.insert(turn.data.generation_id);
            plan.conversation_entries
                .extend(turn.data.response_entry_refs.iter().copied());
        }
        for id in plan.generations.iter().copied().collect::<Vec<_>>() {
            let generation = reader
                .generation(&id)
                .with_context(|| format!("planned generation {id} is missing"))?;
            plan.generations
                .extend(generation.data.parent_generation_id);
            plan.conversation_entries
                .extend(generation.data.entries.iter().copied());
        }
        for id in plan
            .conversation_entries
            .iter()
            .copied()
            .collect::<Vec<_>>()
        {
            let entry = reader
                .conversation_entry(&id)
                .with_context(|| format!("planned conversation entry {id} is missing"))?;
            for range in &entry.data.raw_ranges {
                if source_owned_manifests.contains(&range.manifest_id) {
                    plan.manifests.insert(range.manifest_id);
                }
            }
        }
        let after = (
            plan.manifests.len(),
            plan.external_manifests.len(),
            plan.headers.len(),
            plan.streams.len(),
            plan.stages.len(),
            plan.exchanges.len(),
            plan.conversation_entries.len(),
            plan.generations.len(),
            plan.turn_views.len(),
        );
        if after == before {
            break;
        }
    }

    let mut statement = conn.prepare(
        "SELECT chunk_hash FROM lar_chunks
          WHERE file_uuid=?1 AND hash_algorithm='blake3' AND state='ready'",
    )?;
    let source_chunks = statement
        .query_map([file_uuid], |row| row.get::<_, Vec<u8>>(0))?
        .map(|row| digest32(row?, "catalog chunk hash"))
        .collect::<Result<BTreeSet<_>>>()?;
    let owned = std::mem::take(&mut plan.manifests);
    for id in owned {
        let manifest = reader
            .manifest(&id)
            .with_context(|| format!("source-owned manifest {id} is missing"))?;
        if manifest
            .chunks
            .iter()
            .all(|reference| source_chunks.contains(&reference.chunk_hash.digest))
        {
            plan.manifests.insert(id);
        } else {
            plan.external_manifests.insert(id);
        }
    }
    Ok(plan)
}

fn has_catalog_records_outside_plan<R: std::io::Read + std::io::Seek>(
    conn: &Connection,
    file_uuid: &str,
    reader: &ArchiveReader<R>,
    plan: &CanonicalPlan,
) -> Result<bool> {
    let roots = build_canonical_plan(conn, file_uuid, reader)?;
    Ok(&roots != plan)
}

fn inspect_candidate(
    data_dir: &Path,
    conn: &Connection,
    file_uuid: &str,
    path: &Path,
    config: &LarRepackConfig,
) -> Result<Option<(LarRepackCandidate, RepackPlan)>> {
    if !managed_lar_path(data_dir, path)? {
        return Ok(None);
    }
    let source_size_bytes = fs::metadata(path)?.len();
    let mut compatibility_source = File::open(path)?;
    match validate_selective_rewrite_source(&mut compatibility_source, Limits::default()) {
        Ok(()) => {}
        Err(alex_lar::Error::Unsupported(_)) => return Ok(None),
        Err(error) => return Err(anyhow::Error::new(error)),
    }
    let mut reader =
        ArchiveReader::open(File::open(path)?, Limits::default()).map_err(anyhow::Error::new)?;
    if !reader.is_sealed() || reader.recovery_status() != RecoveryStatus::Clean {
        return Ok(None);
    }
    let canonical = build_canonical_plan(conn, file_uuid, &reader)?;
    let mut reachable = reachable_hashes_for_file(conn, file_uuid)?;
    for id in &canonical.manifests {
        let manifest = reader
            .manifest(id)
            .with_context(|| format!("planned manifest {id} disappeared"))?;
        reachable.extend(
            manifest
                .chunks
                .iter()
                .map(|reference| reference.chunk_hash.digest),
        );
    }
    let total_chunks = reader.chunk_count() as u64;
    let mut descriptors = reader.chunk_records().collect::<Vec<_>>();
    descriptors.sort_by_key(|descriptor| descriptor.hash.digest);
    let mut planned = Vec::new();
    let mut total_compressed_bytes = 0_u64;
    let mut garbage_compressed_bytes = 0_u64;
    for descriptor in descriptors {
        // Opening and reading every source chunk here verifies that selection
        // never treats a corrupt frame as reclaimable input.
        let bytes = reader
            .read_chunk(&descriptor.hash)
            .map_err(anyhow::Error::new)?;
        if ChunkHash::blake3(&bytes) != descriptor.hash {
            bail!("source LAR chunk failed hash verification");
        }
        total_compressed_bytes = total_compressed_bytes
            .checked_add(descriptor.compressed_length)
            .context("source compressed byte count overflow")?;
        if reachable.contains(&descriptor.hash.digest) {
            planned.push(PlannedChunk {
                ordinal: planned.len() as u64,
                hash: descriptor.hash,
                uncompressed_length: descriptor.uncompressed_length,
                source_compressed_length: descriptor.compressed_length,
            });
        } else {
            garbage_compressed_bytes = garbage_compressed_bytes
                .checked_add(descriptor.compressed_length)
                .context("garbage compressed byte count overflow")?;
        }
    }
    let reachable_chunks = planned.len() as u64;
    let garbage_chunks = total_chunks.saturating_sub(reachable_chunks);
    let garbage_ratio = if total_compressed_bytes == 0 {
        0.0
    } else {
        garbage_compressed_bytes as f64 / total_compressed_bytes as f64
    };
    if garbage_chunks == 0
        || garbage_compressed_bytes < config.min_garbage_bytes
        || garbage_ratio < config.min_garbage_ratio
    {
        return Ok(None);
    }
    Ok(Some((
        LarRepackCandidate {
            source_file_uuid: file_uuid.to_string(),
            source_path: path.to_path_buf(),
            source_size_bytes,
            total_chunks,
            reachable_chunks,
            garbage_chunks,
            total_compressed_bytes,
            garbage_compressed_bytes,
            garbage_ratio,
        },
        RepackPlan {
            chunks: planned,
            canonical,
        },
    )))
}

fn candidate_files(conn: &Connection) -> Result<Vec<(String, PathBuf)>> {
    let mut statement = conn.prepare(
        "SELECT file_uuid, path FROM lar_files
          WHERE role='body-pack' AND state='sealed'
          ORDER BY created_at_ms, file_uuid",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            PathBuf::from(row.get::<_, String>(1)?),
        ))
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn load_run(conn: &Connection, run_id: &str) -> Result<RepackRun> {
    conn.query_row(
        "SELECT run_id, archive_set_uuid, source_file_uuid, destination_file_uuid,
                source_path, destination_temp_path, destination_path,
                quarantine_path, state, source_identity_hash, source_identity_size
           FROM lar_repack_runs WHERE run_id=?1",
        [run_id],
        |row| {
            let identity_hash = row.get::<_, Option<Vec<u8>>>(9)?;
            let identity_size = row.get::<_, Option<i64>>(10)?;
            let source_identity = match (identity_hash, identity_size) {
                (Some(identity_hash), Some(identity_size)) => {
                    let identity_hash: [u8; 32] =
                        identity_hash.try_into().map_err(|value: Vec<u8>| {
                            rusqlite::Error::FromSqlConversionFailure(
                                9,
                                rusqlite::types::Type::Blob,
                                format!(
                                    "source identity hash has length {}, expected 32",
                                    value.len()
                                )
                                .into(),
                            )
                        })?;
                    Some(crate::lar_archive_ops::LarFileIdentity {
                        size: identity_size.try_into().map_err(|_| {
                            rusqlite::Error::IntegralValueOutOfRange(10, identity_size)
                        })?,
                        blake3: identity_hash,
                    })
                }
                (None, None) => None,
                _ => {
                    return Err(rusqlite::Error::InvalidColumnType(
                        9,
                        "source_identity_hash/source_identity_size".into(),
                        rusqlite::types::Type::Null,
                    ))
                }
            };
            Ok(RepackRun {
                run_id: row.get(0)?,
                archive_set_uuid: row.get(1)?,
                source_file_uuid: row.get(2)?,
                destination_file_uuid: row.get(3)?,
                source_path: PathBuf::from(row.get::<_, String>(4)?),
                destination_temp_path: PathBuf::from(row.get::<_, String>(5)?),
                destination_path: PathBuf::from(row.get::<_, String>(6)?),
                quarantine_path: PathBuf::from(row.get::<_, String>(7)?),
                state: row.get(8)?,
                source_identity,
            })
        },
    )
    .with_context(|| format!("loading LAR repack run {run_id}"))
}

fn verify_source_identity(run: &RepackRun) -> Result<()> {
    let expected = run
        .source_identity
        .as_ref()
        .context("LAR repack plan has no durable source identity")?;
    let current = compute_lar_file_identity(&run.source_path)?;
    if &current != expected {
        bail!("source LAR pack identity changed after repack planning; refusing to publish");
    }
    Ok(())
}

fn load_planned_chunks(conn: &Connection, run_id: &str) -> Result<Vec<PlannedChunk>> {
    let mut statement = conn.prepare(
        "SELECT ordinal, chunk_hash, uncompressed_length, source_compressed_length
           FROM lar_repack_chunks WHERE run_id=?1 ORDER BY ordinal",
    )?;
    let rows = statement.query_map([run_id], |row| {
        let digest = row.get::<_, Vec<u8>>(1)?;
        let digest: [u8; 32] = digest.try_into().map_err(|value: Vec<u8>| {
            rusqlite::Error::FromSqlConversionFailure(
                1,
                rusqlite::types::Type::Blob,
                format!("chunk hash has length {}, expected 32", value.len()).into(),
            )
        })?;
        Ok(PlannedChunk {
            ordinal: row.get(0)?,
            hash: ChunkHash {
                algorithm: HashAlgorithm::Blake3,
                digest,
            },
            uncompressed_length: row.get(2)?,
            source_compressed_length: row.get(3)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn load_planned_records(conn: &Connection, run_id: &str) -> Result<CanonicalPlan> {
    let mut statement = conn.prepare(
        "SELECT record_kind, record_id FROM lar_repack_records
          WHERE run_id=?1 ORDER BY record_kind, record_id",
    )?;
    let rows = statement.query_map([run_id], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
    })?;
    let mut plan = CanonicalPlan::default();
    for row in rows {
        let (kind, bytes) = row?;
        let id = digest32(bytes, "planned canonical record ID")?;
        match kind.as_str() {
            "manifest" => {
                plan.manifests.insert(ManifestId(id));
            }
            "external_manifest" => {
                plan.external_manifests.insert(ManifestId(id));
            }
            "header" => {
                plan.headers.insert(HeaderBlockId(id));
            }
            "stream" => {
                plan.streams.insert(StreamIndexId(id));
            }
            "stage" => {
                plan.stages.insert(StageId(id));
            }
            "exchange" => {
                plan.exchanges.insert(ExchangeId(id));
            }
            "conversation_entry" => {
                plan.conversation_entries.insert(ConversationEntryId(id));
            }
            "generation" => {
                plan.generations.insert(GenerationId(id));
            }
            "turn_view" => {
                plan.turn_views.insert(TurnViewId(id));
            }
            other => bail!("unknown planned LAR record kind {other}"),
        }
    }
    Ok(plan)
}

fn persist_planned_records(conn: &Connection, run_id: &str, plan: &CanonicalPlan) -> Result<()> {
    macro_rules! insert_ids {
        ($kind:literal, $values:expr) => {
            for id in $values {
                conn.execute(
                    "INSERT INTO lar_repack_records (run_id, record_kind, record_id)
                     VALUES (?1, ?2, ?3)",
                    params![run_id, $kind, id.0.as_slice()],
                )?;
            }
        };
    }
    insert_ids!("manifest", &plan.manifests);
    insert_ids!("external_manifest", &plan.external_manifests);
    insert_ids!("header", &plan.headers);
    insert_ids!("stream", &plan.streams);
    insert_ids!("stage", &plan.stages);
    insert_ids!("exchange", &plan.exchanges);
    insert_ids!("conversation_entry", &plan.conversation_entries);
    insert_ids!("generation", &plan.generations);
    insert_ids!("turn_view", &plan.turn_views);
    Ok(())
}

fn report_for_run(conn: &Connection, run_id: &str) -> Result<LarRepackReport> {
    conn.query_row(
        "SELECT run_id, state, source_file_uuid, destination_file_uuid,
                source_path, destination_path, quarantine_path,
                source_size_bytes, destination_size_bytes, reachable_chunks,
                garbage_chunks, garbage_compressed_bytes, logical_bytes_reclaimed,
                physical_bytes_reclaimed, last_error
           FROM lar_repack_runs WHERE run_id=?1",
        [run_id],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                PathBuf::from(row.get::<_, String>(4)?),
                PathBuf::from(row.get::<_, String>(5)?),
                PathBuf::from(row.get::<_, String>(6)?),
                row.get::<_, i64>(7)?,
                row.get::<_, i64>(8)?,
                row.get::<_, i64>(9)?,
                row.get::<_, i64>(10)?,
                row.get::<_, i64>(11)?,
                row.get::<_, i64>(12)?,
                row.get::<_, i64>(13)?,
                row.get::<_, Option<String>>(14)?,
            ))
        },
    )
    .with_context(|| format!("loading LAR repack report {run_id}"))
    .and_then(|row| {
        Ok(LarRepackReport {
            run_id: row.0,
            state: row.1,
            source_file_uuid: row.2,
            destination_file_uuid: row.3,
            source_path: row.4,
            destination_path: row.5,
            quarantine_path: row.6,
            source_size_bytes: nonnegative(row.7, "source size")?,
            destination_size_bytes: nonnegative(row.8, "destination size")?,
            reachable_chunks: nonnegative(row.9, "reachable chunk count")?,
            garbage_chunks: nonnegative(row.10, "garbage chunk count")?,
            garbage_compressed_bytes: nonnegative(row.11, "garbage compressed bytes")?,
            logical_bytes_reclaimed: nonnegative(row.12, "logical reclaimed bytes")?,
            physical_bytes_reclaimed: nonnegative(row.13, "physical reclaimed bytes")?,
            last_error: row.14,
        })
    })
}

fn record_error(store: &Store, run_id: &str, error: &anyhow::Error, now_ms: i64) {
    let detail = format!("{error:#}");
    if let Ok(conn) = store.conn.lock() {
        let _ = conn.execute(
            "UPDATE lar_repack_runs SET last_error=?2, updated_at_ms=?3 WHERE run_id=?1",
            params![run_id, detail, now_ms],
        );
    }
}

fn external_manifest_ids<R: std::io::Read + std::io::Seek>(
    source: &ArchiveReader<R>,
    plan: &CanonicalPlan,
) -> Result<BTreeSet<ManifestId>> {
    let mut ids = plan.external_manifests.clone();
    for id in &plan.streams {
        let stream = source
            .stream_index(id)
            .with_context(|| format!("planned stream index {id} is missing"))?;
        if !plan.manifests.contains(&stream.raw_body_manifest_id) {
            ids.insert(stream.raw_body_manifest_id);
        }
    }
    for id in &plan.stages {
        let stage = source
            .stage(id)
            .with_context(|| format!("planned stage {id} is missing"))?;
        for manifest in [
            stage.data.request_body_manifest_ref,
            stage.data.response_body_manifest_ref,
        ]
        .into_iter()
        .flatten()
        {
            if !plan.manifests.contains(&manifest) {
                ids.insert(manifest);
            }
        }
    }
    for id in &plan.conversation_entries {
        let entry = source
            .conversation_entry(id)
            .with_context(|| format!("planned conversation entry {id} is missing"))?;
        for range in &entry.data.raw_ranges {
            if !plan.manifests.contains(&range.manifest_id) {
                ids.insert(range.manifest_id);
            }
        }
    }
    Ok(ids)
}

fn catalog_manifest_lengths(
    conn: &Connection,
    wanted: &BTreeSet<ManifestId>,
) -> Result<HashMap<ManifestId, u64>> {
    if wanted.is_empty() {
        return Ok(HashMap::new());
    }
    let mut statement =
        conn.prepare("SELECT manifest_id, total_length FROM lar_manifests WHERE state='ready'")?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    let mut lengths = HashMap::with_capacity(wanted.len());
    for row in rows {
        let (value, length) = row?;
        let bytes = digest32(hex_to_bytes(&value)?, "catalog manifest ID")?;
        let id = ManifestId(bytes);
        if wanted.contains(&id) {
            lengths.insert(id, nonnegative(length, "manifest length")?);
        }
    }
    for id in wanted {
        if !lengths.contains_key(id) {
            bail!("external manifest {id} is missing from the ready catalog");
        }
    }
    Ok(lengths)
}

fn generation_order<R: std::io::Read + std::io::Seek>(
    source: &ArchiveReader<R>,
    plan: &CanonicalPlan,
) -> Result<Vec<GenerationId>> {
    fn visit<R: std::io::Read + std::io::Seek>(
        source: &ArchiveReader<R>,
        plan: &CanonicalPlan,
        id: GenerationId,
        visiting: &mut BTreeSet<GenerationId>,
        visited: &mut BTreeSet<GenerationId>,
        output: &mut Vec<GenerationId>,
    ) -> Result<()> {
        if visited.contains(&id) {
            return Ok(());
        }
        if !visiting.insert(id) {
            bail!("cycle in planned conversation generations at {id}");
        }
        let generation = source
            .generation(&id)
            .with_context(|| format!("planned generation {id} is missing"))?;
        if let Some(parent) = generation.data.parent_generation_id {
            if !plan.generations.contains(&parent) {
                bail!("planned generation {id} is missing parent {parent}");
            }
            visit(source, plan, parent, visiting, visited, output)?;
        }
        visiting.remove(&id);
        visited.insert(id);
        output.push(id);
        Ok(())
    }

    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();
    let mut output = Vec::with_capacity(plan.generations.len());
    for id in &plan.generations {
        visit(source, plan, *id, &mut visiting, &mut visited, &mut output)?;
    }
    Ok(output)
}

fn verify_pack(
    path: &Path,
    planned: &[PlannedChunk],
    canonical: &CanonicalPlan,
) -> Result<Vec<ChunkRecordDescriptor>> {
    let mut reader = ArchiveReader::open(
        File::open(path).with_context(|| format!("opening repack output {}", path.display()))?,
        Limits::default(),
    )
    .map_err(anyhow::Error::new)?;
    if !reader.is_sealed() || reader.recovery_status() != RecoveryStatus::Clean {
        bail!("repack output is not a clean sealed LAR archive");
    }
    let mut descriptors = reader.chunk_records().collect::<Vec<_>>();
    descriptors.sort_by_key(|descriptor| descriptor.hash.digest);
    let expected = planned
        .iter()
        .map(|chunk| (chunk.hash.digest, chunk.uncompressed_length))
        .collect::<BTreeMap<_, _>>();
    let actual = descriptors
        .iter()
        .map(|chunk| (chunk.hash.digest, chunk.uncompressed_length))
        .collect::<BTreeMap<_, _>>();
    if actual != expected || descriptors.len() != planned.len() {
        bail!("repack output chunk set does not match the committed copy plan");
    }
    for descriptor in &descriptors {
        let bytes = reader
            .read_chunk(&descriptor.hash)
            .map_err(anyhow::Error::new)?;
        if ChunkHash::blake3(&bytes) != descriptor.hash {
            bail!("repack output chunk failed hash verification");
        }
    }
    macro_rules! verify_ids {
        ($actual:expr, $expected:expr, $label:literal) => {{
            let actual = $actual.collect::<BTreeSet<_>>();
            if actual != *$expected {
                bail!(concat!(
                    "repack output ",
                    $label,
                    " set does not match the plan"
                ));
            }
        }};
    }
    verify_ids!(
        reader.manifest_ids().copied(),
        &canonical.manifests,
        "manifest"
    );
    verify_ids!(
        reader.header_block_ids().copied(),
        &canonical.headers,
        "header"
    );
    verify_ids!(
        reader.stream_index_ids().copied(),
        &canonical.streams,
        "stream"
    );
    verify_ids!(reader.stage_ids().copied(), &canonical.stages, "stage");
    verify_ids!(
        reader.exchange_ids().copied(),
        &canonical.exchanges,
        "exchange"
    );
    verify_ids!(
        reader.conversation_entry_ids().copied(),
        &canonical.conversation_entries,
        "conversation entry"
    );
    verify_ids!(
        reader.generation_ids().copied(),
        &canonical.generations,
        "generation"
    );
    verify_ids!(
        reader.turn_view_ids().copied(),
        &canonical.turn_views,
        "turn view"
    );
    for id in &canonical.manifests {
        reader.write_body(id, &mut std::io::sink())?;
    }
    Ok(descriptors)
}

fn verify_canonical_values(
    source_path: &Path,
    destination_path: &Path,
    canonical: &CanonicalPlan,
) -> Result<()> {
    let source = ArchiveReader::open(File::open(source_path)?, Limits::default())
        .map_err(anyhow::Error::new)?;
    let destination = ArchiveReader::open(File::open(destination_path)?, Limits::default())
        .map_err(anyhow::Error::new)?;
    macro_rules! verify_values {
        ($values:expr, $getter:ident, $label:literal) => {
            for id in $values {
                if source.$getter(id) != destination.$getter(id) {
                    bail!(concat!("repacked ", $label, " differs from source: {}"), id);
                }
            }
        };
    }
    verify_values!(&canonical.manifests, manifest, "manifest");
    verify_values!(&canonical.headers, header_block, "header block");
    verify_values!(&canonical.streams, stream_index, "stream index");
    verify_values!(&canonical.stages, stage, "stage");
    verify_values!(&canonical.exchanges, exchange, "exchange");
    verify_values!(
        &canonical.conversation_entries,
        conversation_entry,
        "conversation entry"
    );
    verify_values!(&canonical.generations, generation, "generation");
    verify_values!(&canonical.turn_views, turn_view, "turn view");
    for id in &canonical.exchanges {
        if source.exchange_metadata(id) != destination.exchange_metadata(id) {
            bail!("repacked exchange metadata differs from source: {id}");
        }
    }
    Ok(())
}

fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        File::open(path)?.sync_all()?;
    }
    Ok(())
}

fn quarantine_partial(store: &Store, run: &RepackRun, path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let directory = store
        .data_dir
        .join("lar")
        .join("quarantine")
        .join(format!("{}-partial", run.run_id));
    fs::create_dir_all(&directory)?;
    let base = directory.join(
        path.file_name()
            .context("partial repack path has no file name")?,
    );
    let destination = (0_u32..=u32::MAX)
        .map(|sequence| {
            if sequence == 0 {
                base.clone()
            } else {
                base.with_extension(format!("partial-{sequence}"))
            }
        })
        .find(|candidate| !candidate.exists())
        .context("exhausted partial repack quarantine names")?;
    fs::rename(path, &destination)?;
    sync_directory(&directory)?;
    Ok(())
}

impl Store {
    pub fn plan_lar_repacks(&self, config: &LarRepackConfig) -> Result<Vec<LarRepackCandidate>> {
        config.validate()?;
        let conn = self.conn.lock().unwrap();
        let mut candidates = Vec::new();
        for (file_uuid, path) in candidate_files(&conn)? {
            if let Some((candidate, _)) =
                inspect_candidate(&self.data_dir, &conn, &file_uuid, &path, config)?
            {
                candidates.push(candidate);
            }
        }
        candidates.sort_by(|left, right| {
            right
                .garbage_compressed_bytes
                .cmp(&left.garbage_compressed_bytes)
                .then_with(|| left.source_file_uuid.cmp(&right.source_file_uuid))
        });
        Ok(candidates)
    }

    /// Select one eligible sealed body pack, commit its exact reachable chunk
    /// set and canonical graph, and copy it to a new sealed pack. No catalog
    /// location changes here.
    pub fn start_lar_repack(
        &self,
        config: &LarRepackConfig,
        now_ms: i64,
    ) -> Result<Option<LarRepackReport>> {
        config.validate()?;
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let active: Option<String> = tx
            .query_row(
                "SELECT run_id FROM lar_repack_runs
                  WHERE state IN ('copying','copied','switched')
                  ORDER BY started_at_ms, run_id LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(run_id) = active {
            tx.commit()?;
            return Ok(Some(report_for_run(&conn, &run_id)?));
        }
        let mut selected = None;
        for (file_uuid, path) in candidate_files(&tx)? {
            let identity_before = compute_lar_file_identity(&path)?;
            if let Some((candidate, planned)) =
                inspect_candidate(&self.data_dir, &tx, &file_uuid, &path, config)?
            {
                let identity_after = compute_lar_file_identity(&path)?;
                if identity_before != identity_after
                    || candidate.source_size_bytes != identity_after.size
                {
                    bail!("source LAR pack changed while its repack plan was being built");
                }
                selected = Some((candidate, planned, identity_after));
                break;
            }
        }
        let Some((candidate, planned, source_identity)) = selected else {
            tx.commit()?;
            return Ok(None);
        };
        let archive_set_uuid: String = tx.query_row(
            "SELECT archive_set_uuid FROM lar_files WHERE file_uuid=?1 AND state='sealed'",
            [&candidate.source_file_uuid],
            |row| row.get(0),
        )?;
        let (run_id, _destination_uuid_bytes, destination_file_uuid) = run_identity(now_ms);
        let destination_dir = self.data_dir.join("lar").join("repacked");
        let destination_path = destination_dir.join(format!("body-{destination_file_uuid}.lar"));
        let destination_temp_path =
            destination_dir.join(format!(".body-{destination_file_uuid}.tmp"));
        let quarantine_path = self
            .data_dir
            .join("lar")
            .join("quarantine")
            .join(&run_id)
            .join(
                candidate
                    .source_path
                    .file_name()
                    .context("source LAR pack has no file name")?,
            );
        tx.execute(
            "INSERT INTO lar_repack_runs
               (run_id, archive_set_uuid, source_file_uuid, destination_file_uuid,
                source_path, destination_temp_path, destination_path, quarantine_path,
                state, started_at_ms, updated_at_ms, source_size_bytes,
                source_identity_hash, source_identity_size,
                reachable_chunks, garbage_chunks, garbage_compressed_bytes)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'copying', ?9, ?9,
                     ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                run_id,
                archive_set_uuid,
                candidate.source_file_uuid,
                destination_file_uuid,
                candidate.source_path.to_string_lossy(),
                destination_temp_path.to_string_lossy(),
                destination_path.to_string_lossy(),
                quarantine_path.to_string_lossy(),
                now_ms,
                as_i64(candidate.source_size_bytes, "source size")?,
                source_identity.blake3.as_slice(),
                as_i64(source_identity.size, "source identity size")?,
                as_i64(candidate.reachable_chunks, "reachable chunks")?,
                as_i64(candidate.garbage_chunks, "garbage chunks")?,
                as_i64(
                    candidate.garbage_compressed_bytes,
                    "garbage compressed bytes"
                )?,
            ],
        )?;
        for chunk in planned.chunks {
            tx.execute(
                "INSERT INTO lar_repack_chunks
                   (run_id, ordinal, hash_algorithm, chunk_hash,
                    uncompressed_length, source_compressed_length)
                 VALUES (?1, ?2, 'blake3', ?3, ?4, ?5)",
                params![
                    run_id,
                    chunk.ordinal,
                    chunk.hash.digest.as_slice(),
                    chunk.uncompressed_length,
                    chunk.source_compressed_length,
                ],
            )?;
        }
        persist_planned_records(&tx, &run_id, &planned.canonical)?;
        tx.commit()?;
        drop(conn);
        self.copy_lar_repack(&run_id, now_ms).map(Some)
    }

    /// Copy/recover the precommitted plan and verify the sealed result. A
    /// crash before this phase commits leaves the old pack authoritative.
    pub fn copy_lar_repack(&self, run_id: &str, now_ms: i64) -> Result<LarRepackReport> {
        let (run, planned, canonical) = {
            let conn = self.conn.lock().unwrap();
            (
                load_run(&conn, run_id)?,
                load_planned_chunks(&conn, run_id)?,
                load_planned_records(&conn, run_id)?,
            )
        };
        if run.state != "copying" {
            let conn = self.conn.lock().unwrap();
            return report_for_run(&conn, run_id);
        }
        let copy = (|| -> Result<Vec<ChunkRecordDescriptor>> {
            verify_source_identity(&run)?;
            fs::create_dir_all(
                run.destination_path
                    .parent()
                    .context("repack destination has no parent")?,
            )?;
            if run.destination_path.exists() {
                verify_canonical_values(&run.source_path, &run.destination_path, &canonical)?;
                return verify_pack(&run.destination_path, &planned, &canonical);
            }
            if run.destination_temp_path.exists() {
                if verify_pack(&run.destination_temp_path, &planned, &canonical).is_ok()
                    && verify_canonical_values(
                        &run.source_path,
                        &run.destination_temp_path,
                        &canonical,
                    )
                    .is_ok()
                {
                    fs::rename(&run.destination_temp_path, &run.destination_path)?;
                    sync_directory(run.destination_path.parent().unwrap())?;
                    return verify_pack(&run.destination_path, &planned, &canonical);
                }
                quarantine_partial(self, &run, &run.destination_temp_path)?;
            }

            let source_file = File::open(&run.source_path)
                .with_context(|| format!("opening source pack {}", run.source_path.display()))?;
            let mut source =
                ArchiveReader::open(source_file, Limits::default()).map_err(anyhow::Error::new)?;
            if !source.is_sealed() || source.recovery_status() != RecoveryStatus::Clean {
                bail!("source pack is not a clean sealed LAR archive");
            }
            let output = OpenOptions::new()
                .create_new(true)
                .read(true)
                .write(true)
                .open(&run.destination_temp_path)?;
            let file_uuid: [u8; 16] = hex_to_bytes(&run.destination_file_uuid)?
                .try_into()
                .map_err(|value: Vec<u8>| {
                    anyhow::anyhow!(
                        "destination file UUID has length {}, expected 16",
                        value.len()
                    )
                })?;
            let external_ids = external_manifest_ids(&source, &canonical)?;
            let external_lengths = {
                let conn = self.conn.lock().unwrap();
                catalog_manifest_lengths(&conn, &external_ids)?
            };
            let mut header = source.header().clone();
            header.file_uuid = file_uuid;
            header.created_at_ns = now_ns();
            header.writer = b"alex-store/repack-v2".to_vec();
            header.dictionaries.clear();
            header.optional_feature_bits = 0;
            if !external_ids.is_empty() {
                header.required_feature_bits |= REQUIRED_FEATURE_ARCHIVE_SET_BODY_REFS;
            }
            let mut writer =
                ArchiveWriter::create(output, header, ChunkerConfig::default(), Limits::default())
                    .map_err(anyhow::Error::new)?;
            writer.enable_metadata_pages();
            for chunk in &planned {
                let bytes = source.read_chunk(&chunk.hash).map_err(anyhow::Error::new)?;
                if ChunkHash::blake3(&bytes) != chunk.hash {
                    bail!("source chunk changed during repack copy");
                }
                let descriptor = writer
                    .append_chunk_record(&bytes)
                    .map_err(anyhow::Error::new)?;
                if descriptor.hash != chunk.hash
                    || descriptor.uncompressed_length != chunk.uncompressed_length
                {
                    bail!("copied chunk identity changed during repack");
                }
            }
            for id in &canonical.manifests {
                let value = source
                    .manifest(id)
                    .cloned()
                    .with_context(|| format!("source is missing planned manifest {id}"))?;
                let copied = writer
                    .append_manifest_record(value)
                    .map_err(anyhow::Error::new)?;
                if copied != *id {
                    bail!("copied manifest identity changed during repack");
                }
            }
            for id in &canonical.headers {
                let value = source
                    .header_block(id)
                    .cloned()
                    .with_context(|| format!("source is missing planned header block {id}"))?;
                if writer
                    .append_header_block(value)
                    .map_err(anyhow::Error::new)?
                    != *id
                {
                    bail!("copied header identity changed during repack");
                }
            }
            for id in &canonical.streams {
                let value = source
                    .stream_index(id)
                    .cloned()
                    .with_context(|| format!("source is missing planned stream index {id}"))?;
                let copied = if canonical.manifests.contains(&value.raw_body_manifest_id) {
                    writer.append_stream_index(value)
                } else {
                    let length = *external_lengths
                        .get(&value.raw_body_manifest_id)
                        .with_context(|| {
                            format!(
                                "stream {id} references unknown external manifest {}",
                                value.raw_body_manifest_id
                            )
                        })?;
                    writer.append_stream_index_with_external_manifest(value, length)
                }
                .map_err(anyhow::Error::new)?;
                if copied != *id {
                    bail!("copied stream identity changed during repack");
                }
            }
            for id in &canonical.stages {
                let value = source
                    .stage(id)
                    .cloned()
                    .with_context(|| format!("source is missing planned stage {id}"))?;
                let external = [
                    value.data.request_body_manifest_ref,
                    value.data.response_body_manifest_ref,
                ]
                .into_iter()
                .flatten()
                .filter(|manifest| !canonical.manifests.contains(manifest))
                .collect::<Vec<_>>();
                if writer
                    .append_stage_with_external_manifests(value, &external)
                    .map_err(anyhow::Error::new)?
                    != *id
                {
                    bail!("copied stage identity changed during repack");
                }
            }
            for id in &canonical.exchanges {
                let value = source
                    .exchange(id)
                    .cloned()
                    .with_context(|| format!("source is missing planned exchange {id}"))?;
                let copied = if let Some(metadata) = source.exchange_metadata(id) {
                    writer.append_exchange_with_metadata(value, metadata.data.clone())
                } else {
                    writer.append_exchange(value)
                }
                .map_err(anyhow::Error::new)?;
                if copied != *id {
                    bail!("copied exchange identity changed during repack");
                }
            }
            for id in &canonical.conversation_entries {
                let value = source.conversation_entry(id).cloned().with_context(|| {
                    format!("source is missing planned conversation entry {id}")
                })?;
                let external = value
                    .data
                    .raw_ranges
                    .iter()
                    .filter(|range| !canonical.manifests.contains(&range.manifest_id))
                    .map(|range| {
                        external_lengths
                            .get(&range.manifest_id)
                            .copied()
                            .map(|length| (range.manifest_id, length))
                            .with_context(|| {
                                format!(
                                    "entry {id} references unknown external manifest {}",
                                    range.manifest_id
                                )
                            })
                    })
                    .collect::<Result<Vec<_>>>()?;
                if writer
                    .append_conversation_entry_with_external_manifests(value, &external)
                    .map_err(anyhow::Error::new)?
                    != *id
                {
                    bail!("copied conversation entry identity changed during repack");
                }
            }
            for id in generation_order(&source, &canonical)? {
                let value = source
                    .generation(&id)
                    .cloned()
                    .with_context(|| format!("source is missing planned generation {id}"))?;
                if writer
                    .append_generation(value)
                    .map_err(anyhow::Error::new)?
                    != id
                {
                    bail!("copied generation identity changed during repack");
                }
            }
            for id in &canonical.turn_views {
                let value = source
                    .turn_view(id)
                    .cloned()
                    .with_context(|| format!("source is missing planned turn view {id}"))?;
                if writer.append_turn_view(value).map_err(anyhow::Error::new)? != *id {
                    bail!("copied turn identity changed during repack");
                }
            }
            writer.seal().map_err(anyhow::Error::new)?;
            writer.get_ref().sync_all()?;
            drop(writer);
            verify_pack(&run.destination_temp_path, &planned, &canonical)?;
            verify_canonical_values(&run.source_path, &run.destination_temp_path, &canonical)?;
            fs::rename(&run.destination_temp_path, &run.destination_path)?;
            sync_directory(run.destination_path.parent().unwrap())?;
            verify_pack(&run.destination_path, &planned, &canonical)
        })();
        let descriptors = match copy {
            Ok(descriptors) => descriptors,
            Err(error) => {
                record_error(self, run_id, &error, now_ms);
                return Err(error);
            }
        };
        let by_hash = descriptors
            .into_iter()
            .map(|descriptor| (descriptor.hash.digest, descriptor))
            .collect::<BTreeMap<_, _>>();
        verify_source_identity(&run)?;
        let destination_size = fs::metadata(&run.destination_path)?.len();
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for chunk in &planned {
            let descriptor = by_hash
                .get(&chunk.hash.digest)
                .context("verified repack output lost a planned chunk")?;
            tx.execute(
                "UPDATE lar_repack_chunks SET destination_offset=?3,
                        destination_compressed_length=?4, state='copied'
                  WHERE run_id=?1 AND ordinal=?2",
                params![
                    run_id,
                    chunk.ordinal,
                    descriptor.frame_offset,
                    descriptor.compressed_length,
                ],
            )?;
        }
        tx.execute(
            "UPDATE lar_repack_runs SET state='copied', updated_at_ms=?2,
                    destination_size_bytes=?3, last_error=NULL
              WHERE run_id=?1 AND state='copying'",
            params![run_id, now_ms, destination_size],
        )?;
        tx.commit()?;
        report_for_run(&conn, run_id)
    }

    /// Verify the replacement once more, recheck reachability under an
    /// immediate transaction, and atomically publish every new chunk offset.
    pub fn switch_lar_repack(&self, run_id: &str, now_ms: i64) -> Result<LarRepackReport> {
        let (run, planned, canonical) = {
            let conn = self.conn.lock().unwrap();
            (
                load_run(&conn, run_id)?,
                load_planned_chunks(&conn, run_id)?,
                load_planned_records(&conn, run_id)?,
            )
        };
        if run.state != "copied" {
            let conn = self.conn.lock().unwrap();
            return report_for_run(&conn, run_id);
        }
        verify_source_identity(&run)?;
        let descriptors =
            match verify_pack(&run.destination_path, &planned, &canonical).and_then(|descriptors| {
                verify_canonical_values(&run.source_path, &run.destination_path, &canonical)?;
                Ok(descriptors)
            }) {
                Ok(descriptors) => descriptors,
                Err(error) => {
                    record_error(self, run_id, &error, now_ms);
                    return Err(error);
                }
            };
        File::open(&run.destination_path)?.sync_all()?;
        sync_directory(run.destination_path.parent().unwrap())?;
        let destination_identity = compute_lar_file_identity(&run.destination_path)?;
        let source_identity = compute_lar_file_identity(&run.source_path)?;
        let destination_header =
            ArchiveReader::open(File::open(&run.destination_path)?, Limits::default())
                .map_err(anyhow::Error::new)?
                .header()
                .clone();
        let by_hash = descriptors
            .into_iter()
            .map(|descriptor| (descriptor.hash.digest, descriptor))
            .collect::<BTreeMap<_, _>>();
        let planned_hashes = planned
            .iter()
            .map(|chunk| chunk.hash.digest)
            .collect::<BTreeSet<_>>();

        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let source_reader = ArchiveReader::open(
            File::open(&run.source_path).with_context(|| {
                format!(
                    "reopening source pack {} for final metadata check",
                    run.source_path.display()
                )
            })?,
            Limits::default(),
        )
        .map_err(anyhow::Error::new)?;
        verify_source_identity(&run)?;
        if has_catalog_records_outside_plan(&tx, &run.source_file_uuid, &source_reader, &canonical)?
        {
            bail!("source LAR canonical reachability changed during repack");
        }
        let current_hashes = reachable_hashes_for_file(&tx, &run.source_file_uuid)?;
        if current_hashes != planned_hashes {
            bail!("LAR reachability changed during repack; the copied pack was not published");
        }
        let source: Option<(String, String)> = tx
            .query_row(
                "SELECT path, state FROM lar_files WHERE file_uuid=?1",
                [&run.source_file_uuid],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if source.as_ref()
            != Some(&(
                run.source_path.to_string_lossy().into_owned(),
                "sealed".into(),
            ))
        {
            bail!("source LAR pack moved or changed state before catalog switch");
        }
        let destination_size = fs::metadata(&run.destination_path)?.len();
        tx.execute(
            "INSERT INTO lar_files
               (file_uuid, archive_set_uuid, role, path, state, container_major,
                container_minor, required_feature_bits, optional_feature_bits,
                created_at_ms, sealed_at_ms, size_bytes)
             VALUES (?1, ?2, 'body-pack', ?3, 'sealed', 1, 0, ?4, ?5, ?6, ?6, ?7)",
            params![
                run.destination_file_uuid,
                run.archive_set_uuid,
                run.destination_path.to_string_lossy(),
                destination_header.required_feature_bits,
                destination_header.optional_feature_bits,
                now_ms,
                destination_size,
            ],
        )?;
        record_lar_file_identity(
            &tx,
            &run.destination_file_uuid,
            &destination_identity,
            "repack_output",
            now_ms,
        )?;
        record_lar_file_identity(
            &tx,
            &run.source_file_uuid,
            &source_identity,
            "repack_source",
            now_ms,
        )?;
        for chunk in &planned {
            let descriptor = by_hash
                .get(&chunk.hash.digest)
                .context("verified replacement is missing a planned chunk")?;
            let changed = tx.execute(
                "UPDATE lar_chunks SET file_uuid=?3, record_id=?4,
                        page_offset=?5, record_offset=?5,
                        compressed_length=?6, checksum=?2
                  WHERE hash_algorithm='blake3' AND chunk_hash=?2
                    AND file_uuid=?1 AND state='ready'",
                params![
                    run.source_file_uuid,
                    chunk.hash.digest.as_slice(),
                    run.destination_file_uuid,
                    format!("chunk:{}", hex(&chunk.hash.digest)),
                    descriptor.frame_offset,
                    descriptor.compressed_length,
                ],
            )?;
            if changed != 1 {
                bail!("a planned source chunk changed before catalog switch");
            }
            tx.execute(
                "UPDATE lar_repack_chunks SET state='switched' WHERE run_id=?1 AND ordinal=?2",
                params![run_id, chunk.ordinal],
            )?;
        }
        for id in &canonical.manifests {
            let changed = tx.execute(
                "UPDATE lar_manifests SET file_uuid=?3, record_id=?2
                  WHERE manifest_id=?2 AND file_uuid=?1 AND state='ready'",
                params![
                    run.source_file_uuid,
                    id.to_string(),
                    run.destination_file_uuid,
                ],
            )?;
            if changed != 1 {
                bail!("planned local manifest {id} changed before catalog switch");
            }
        }
        for id in &canonical.external_manifests {
            let changed = tx.execute(
                "UPDATE lar_manifests SET file_uuid=NULL, record_id=NULL
                  WHERE manifest_id=?1 AND file_uuid=?2 AND state='ready'",
                params![id.to_string(), run.source_file_uuid],
            )?;
            if changed != 1 {
                bail!("planned external manifest {id} changed before catalog switch");
            }
        }
        for id in &canonical.headers {
            let changed = tx.execute(
                "UPDATE lar_header_blocks SET file_uuid=?3, record_id=?2
                  WHERE block_id=?2 AND file_uuid=?1",
                params![
                    run.source_file_uuid,
                    id.to_string(),
                    run.destination_file_uuid,
                ],
            )?;
            if changed != 1 {
                bail!("planned header block {id} changed before catalog switch");
            }
        }
        tx.execute(
            "UPDATE lar_stage_records SET file_uuid=?2
              WHERE file_uuid=?1",
            params![run.source_file_uuid, run.destination_file_uuid],
        )?;
        for id in &canonical.exchanges {
            let changed = tx.execute(
                "UPDATE lar_exchange_records SET file_uuid=?3
                  WHERE exchange_id=?2 AND file_uuid=?1",
                params![
                    run.source_file_uuid,
                    id.to_string(),
                    run.destination_file_uuid,
                ],
            )?;
            if changed > 1 {
                bail!("exchange {id} has duplicate catalog ownership");
            }
        }
        tx.execute(
            "UPDATE lar_timeline_supplements SET file_uuid=?2
              WHERE file_uuid=?1 AND supplement_trace_id IN
                (SELECT trace_id FROM lar_exchange_records WHERE file_uuid=?2)",
            params![run.source_file_uuid, run.destination_file_uuid],
        )?;
        tx.execute(
            "UPDATE lar_migration_items SET destination_file_uuid=NULL
              WHERE destination_file_uuid=?1",
            [&run.source_file_uuid],
        )?;
        for id in &canonical.manifests {
            tx.execute(
                "UPDATE lar_migration_items SET destination_file_uuid=?2
                  WHERE destination_manifest_id=?1",
                params![id.to_string(), run.destination_file_uuid],
            )?;
        }
        for id in &canonical.exchanges {
            tx.execute(
                "UPDATE lar_migration_items SET destination_file_uuid=?2
                  WHERE destination_exchange_id=?1",
                params![id.to_string(), run.destination_file_uuid],
            )?;
        }
        tx.execute(
            "UPDATE lar_chunks SET state='unreachable'
              WHERE file_uuid=?1 AND state='ready'",
            [&run.source_file_uuid],
        )?;
        tx.execute(
            "DELETE FROM lar_checkpoints WHERE file_uuid=?1",
            [&run.source_file_uuid],
        )?;
        let dangling: bool = tx.query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM lar_chunks WHERE file_uuid=?1 AND state='ready'
                 UNION ALL SELECT 1 FROM lar_manifests WHERE file_uuid=?1 AND state='ready'
                 UNION ALL SELECT 1 FROM lar_header_blocks WHERE file_uuid=?1
                 UNION ALL SELECT 1 FROM lar_stage_records WHERE file_uuid=?1
                 UNION ALL SELECT 1 FROM lar_exchange_records WHERE file_uuid=?1
                 UNION ALL SELECT 1 FROM lar_timeline_supplements WHERE file_uuid=?1
                 UNION ALL SELECT 1 FROM lar_migration_items WHERE destination_file_uuid=?1
             )",
            [&run.source_file_uuid],
            |row| row.get(0),
        )?;
        if dangling {
            bail!("catalog switch left authoritative references on the source pack");
        }
        tx.execute(
            "UPDATE lar_files SET state='retired' WHERE file_uuid=?1 AND state='sealed'",
            [&run.source_file_uuid],
        )?;
        tx.execute(
            "UPDATE lar_archive_sets SET catalog_revision=catalog_revision+1,
                    updated_at_ms=?2 WHERE archive_set_uuid=?1",
            params![run.archive_set_uuid, now_ms],
        )?;
        let source_size = fs::metadata(&run.source_path)?.len();
        let logical_reclaimed = source_size.saturating_sub(destination_size);
        tx.execute(
            "UPDATE lar_repack_runs SET state='switched', updated_at_ms=?2,
                    destination_size_bytes=?3, logical_bytes_reclaimed=?4,
                    physical_bytes_reclaimed=0, last_error=NULL
              WHERE run_id=?1 AND state='copied'",
            params![run_id, now_ms, destination_size, logical_reclaimed],
        )?;
        tx.commit()?;
        report_for_run(&conn, run_id)
    }

    /// Move the old immutable pack into recoverable quarantine only after the
    /// catalog switch committed, then mark it retired. No bytes are deleted.
    pub fn finish_lar_repack(&self, run_id: &str, now_ms: i64) -> Result<LarRepackReport> {
        let run = {
            let conn = self.conn.lock().unwrap();
            load_run(&conn, run_id)?
        };
        if run.state == "complete" {
            let conn = self.conn.lock().unwrap();
            return report_for_run(&conn, run_id);
        }
        if run.state != "switched" {
            bail!("LAR repack run {run_id} is not ready to retire its source pack");
        }
        verify_source_identity(&run)?;
        if let Some(parent) = run.quarantine_path.parent() {
            fs::create_dir_all(parent)?;
        }
        match (run.source_path.exists(), run.quarantine_path.exists()) {
            (true, false) => {
                fs::rename(&run.source_path, &run.quarantine_path)?;
                sync_directory(run.quarantine_path.parent().unwrap())?;
                if let Some(source_parent) = run.source_path.parent() {
                    sync_directory(source_parent)?;
                }
            }
            (false, true) => {}
            (true, true) => bail!("both source and quarantine packs exist; refusing overwrite"),
            (false, false) => bail!("source pack disappeared before recoverable retirement"),
        }
        let source_size = fs::metadata(&run.quarantine_path)?.len();
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = tx.execute(
            "UPDATE lar_files SET path=?2, state='retired', size_bytes=?3
              WHERE file_uuid=?1 AND state='retired'",
            params![
                run.source_file_uuid,
                run.quarantine_path.to_string_lossy(),
                source_size,
            ],
        )?;
        if changed != 1 {
            bail!("source pack catalog state changed before retirement finished");
        }
        tx.execute(
            "UPDATE lar_repack_runs SET state='complete', updated_at_ms=?2,
                    completed_at_ms=?2, physical_bytes_reclaimed=0, last_error=NULL
              WHERE run_id=?1 AND state='switched'",
            params![run_id, now_ms],
        )?;
        tx.commit()?;
        report_for_run(&conn, run_id)
    }

    /// Resume one durable boundary. Repeated calls eventually reach complete;
    /// callers may inspect each phase independently for operator visibility.
    pub fn resume_lar_repack(&self, run_id: &str, now_ms: i64) -> Result<LarRepackReport> {
        let state = {
            let conn = self.conn.lock().unwrap();
            load_run(&conn, run_id)?.state
        };
        match state.as_str() {
            "copying" => self.copy_lar_repack(run_id, now_ms),
            "copied" => self.switch_lar_repack(run_id, now_ms),
            "switched" => self.finish_lar_repack(run_id, now_ms),
            "complete" => {
                let conn = self.conn.lock().unwrap();
                report_for_run(&conn, run_id)
            }
            _ => bail!("LAR repack run {run_id} cannot resume from state {state}"),
        }
    }

    pub fn run_lar_repack(
        &self,
        config: &LarRepackConfig,
        now_ms: i64,
    ) -> Result<Option<LarRepackReport>> {
        let Some(mut report) = self.start_lar_repack(config, now_ms)? else {
            return Ok(None);
        };
        while report.state != "complete" {
            report = self.resume_lar_repack(&report.run_id, now_ms)?;
        }
        Ok(Some(report))
    }
}

fn hex_to_bytes(value: &str) -> Result<Vec<u8>> {
    if value.len() % 2 != 0 {
        bail!("hex value has odd length");
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let pair = std::str::from_utf8(pair)?;
            u8::from_str_radix(pair, 16).map_err(Into::into)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use alex_lar::{BodyManifest, ChunkRef, FileHeader};

    use super::*;

    static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct SeededPack {
        source_file_uuid: String,
        source_path: PathBuf,
        manifest_id: String,
        reachable_bytes: Vec<u8>,
        reachable_hash: ChunkHash,
        garbage_hash: ChunkHash,
    }

    fn tmpdir(name: &str) -> PathBuf {
        let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "alex-lar-repack-{name}-{}-{sequence}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn bytes(seed: u64, length: usize) -> Vec<u8> {
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

    fn seed_pack(store: &Store) -> SeededPack {
        let source_uuid = [7_u8; 16];
        let source_file_uuid = hex(&source_uuid);
        let source_dir = store.data_dir.join("lar").join("seed");
        fs::create_dir_all(&source_dir).unwrap();
        let source_path = source_dir.join("body-source.lar");
        let output = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&source_path)
            .unwrap();
        let mut writer = ArchiveWriter::create(
            output,
            FileHeader::body_pack(source_uuid, 1, b"repack-test".to_vec()),
            ChunkerConfig::default(),
            Limits::default(),
        )
        .unwrap();
        let reachable_bytes = bytes(11, 96 * 1024);
        let garbage_bytes = bytes(29, 128 * 1024);
        let reachable_descriptor = writer.append_chunk_record(&reachable_bytes).unwrap();
        let garbage_descriptor = writer.append_chunk_record(&garbage_bytes).unwrap();
        writer.seal().unwrap();
        writer.get_ref().sync_all().unwrap();
        drop(writer);

        let manifest = BodyManifest::new(
            reachable_bytes.len() as u64,
            ChunkHash::blake3(&reachable_bytes),
            None,
            None,
            vec![ChunkRef {
                chunk_hash: reachable_descriptor.hash,
                chunk_offset: 0,
                logical_offset: 0,
                length: reachable_bytes.len() as u64,
            }],
        );
        let manifest_id = manifest.id.to_string();
        let conn = store.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO lar_archive_sets
               (archive_set_uuid, created_at_ms, updated_at_ms, state, description)
             VALUES ('repack-set', 1, 1, 'sealed', 'repack test')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO lar_files
               (file_uuid, archive_set_uuid, role, path, state, container_major,
                container_minor, created_at_ms, sealed_at_ms, size_bytes)
             VALUES (?1, 'repack-set', 'body-pack', ?2, 'sealed', 1, 0, 1, 1, ?3)",
            params![
                source_file_uuid,
                source_path.to_string_lossy(),
                fs::metadata(&source_path).unwrap().len(),
            ],
        )
        .unwrap();
        for descriptor in [reachable_descriptor, garbage_descriptor] {
            conn.execute(
                "INSERT INTO lar_chunks
                   (hash_algorithm, chunk_hash, uncompressed_length, compression,
                    compressed_length, file_uuid, record_id, page_offset,
                    record_offset, checksum, created_at_ms, state)
                 VALUES ('blake3', ?1, ?2, 'zstd', ?3, ?4, ?5, ?6, ?6, ?1, 1, 'ready')",
                params![
                    descriptor.hash.digest.as_slice(),
                    descriptor.uncompressed_length,
                    descriptor.compressed_length,
                    source_file_uuid,
                    format!("chunk:{}", hex(&descriptor.hash.digest)),
                    descriptor.frame_offset,
                ],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO lar_manifests
               (manifest_id, total_length, hash_algorithm, whole_body_hash,
                created_at_ms, state)
             VALUES (?1, ?2, 'blake3', ?3, 1, 'ready')",
            params![
                manifest_id,
                manifest.total_length,
                manifest.whole_body_hash.digest.as_slice(),
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO lar_manifest_chunks
               (manifest_id, ordinal, hash_algorithm, chunk_hash,
                logical_offset, chunk_offset, length)
             VALUES (?1, 0, 'blake3', ?2, 0, 0, ?3)",
            params![
                manifest_id,
                reachable_descriptor.hash.digest.as_slice(),
                reachable_bytes.len() as u64,
            ],
        )
        .unwrap();
        for (trace_id, timestamp) in [("trace-a", 1_i64), ("trace-b", 2_i64)] {
            conn.execute(
                "INSERT INTO traces (id, ts_request_ms, session_id) VALUES (?1, ?2, ?3)",
                params![trace_id, timestamp, format!("session-{trace_id}")],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO lar_trace_artifacts
                   (owner_kind, owner_id, artifact_kind, stage_id, manifest_id,
                    fidelity, validation_state, validated_at_ms)
                 VALUES ('trace', ?1, 'client_request', '', ?2,
                         'captured', 'validated', 1)",
                params![trace_id, manifest_id],
            )
            .unwrap();
        }
        SeededPack {
            source_file_uuid,
            source_path,
            manifest_id,
            reachable_bytes,
            reachable_hash: reachable_descriptor.hash,
            garbage_hash: garbage_descriptor.hash,
        }
    }

    fn config() -> LarRepackConfig {
        LarRepackConfig {
            min_garbage_bytes: 1,
            min_garbage_ratio: 0.01,
        }
    }

    #[test]
    fn shared_reachable_chunk_is_copied_once_and_reads_after_switch() {
        let store = Store::open(tmpdir("shared-read")).unwrap();
        let seeded = seed_pack(&store);
        assert!(store
            .plan_lar_repacks(&LarRepackConfig {
                min_garbage_bytes: u64::MAX,
                min_garbage_ratio: 1.0,
            })
            .unwrap()
            .is_empty());
        let candidates = store.plan_lar_repacks(&config()).unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(
            (candidates[0].reachable_chunks, candidates[0].garbage_chunks),
            (1, 1)
        );

        let copied = store.start_lar_repack(&config(), 10).unwrap().unwrap();
        assert_eq!(copied.state, "copied");
        store.delete_trace("trace-a").unwrap();
        let switched = store.switch_lar_repack(&copied.run_id, 11).unwrap();
        assert_eq!(switched.state, "switched");
        assert_eq!(
            store
                .read_lar_or_legacy_artifact("trace", "trace-b", "client_request", None)
                .unwrap()
                .unwrap(),
            seeded.reachable_bytes
        );
        let conn = store.conn.lock().unwrap();
        let reachable_location: String = conn
            .query_row(
                "SELECT file_uuid FROM lar_chunks WHERE chunk_hash=?1",
                [seeded.reachable_hash.digest.as_slice()],
                |row| row.get(0),
            )
            .unwrap();
        let garbage_location: String = conn
            .query_row(
                "SELECT file_uuid FROM lar_chunks WHERE chunk_hash=?1",
                [seeded.garbage_hash.digest.as_slice()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(reachable_location, switched.destination_file_uuid);
        assert_eq!(garbage_location, seeded.source_file_uuid);
        assert_eq!(
            conn.query_row(
                "SELECT manifest_id FROM lar_trace_artifacts WHERE owner_id='trace-b'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
            seeded.manifest_id,
            "repacking must preserve stable logical manifest identities"
        );
    }

    #[test]
    fn finish_reports_logical_reclaim_but_keeps_source_quarantined() {
        let store = Store::open(tmpdir("accounting")).unwrap();
        let seeded = seed_pack(&store);
        let complete = store.run_lar_repack(&config(), 20).unwrap().unwrap();
        assert_eq!(complete.state, "complete");
        assert!(complete.logical_bytes_reclaimed > 0);
        assert_eq!(complete.physical_bytes_reclaimed, 0);
        assert!(!seeded.source_path.exists());
        assert!(complete.destination_path.is_file());
        assert!(complete.quarantine_path.is_file());
        assert_eq!(
            fs::metadata(&complete.quarantine_path).unwrap().len(),
            complete.source_size_bytes
        );
        let conn = store.conn.lock().unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT state FROM lar_files WHERE file_uuid=?1",
                [seeded.source_file_uuid],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
            "retired"
        );
    }

    #[test]
    fn corrupt_replacement_fails_before_catalog_switch() {
        let store = Store::open(tmpdir("failure-before-switch")).unwrap();
        let seeded = seed_pack(&store);
        let copied = store.start_lar_repack(&config(), 30).unwrap().unwrap();
        OpenOptions::new()
            .write(true)
            .open(&copied.destination_path)
            .unwrap()
            .set_len(16)
            .unwrap();
        assert!(store.switch_lar_repack(&copied.run_id, 31).is_err());
        let conn = store.conn.lock().unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT file_uuid FROM lar_chunks WHERE chunk_hash=?1",
                [seeded.reachable_hash.digest.as_slice()],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
            seeded.source_file_uuid
        );
        assert_eq!(
            conn.query_row(
                "SELECT state FROM lar_files WHERE file_uuid=?1",
                [seeded.source_file_uuid],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
            "sealed"
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM lar_files WHERE file_uuid=?1",
                [copied.destination_file_uuid],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            0
        );
    }

    #[test]
    fn changed_source_identity_fails_before_catalog_switch() {
        let store = Store::open(tmpdir("changed-source-before-switch")).unwrap();
        let seeded = seed_pack(&store);
        let copied = store.start_lar_repack(&config(), 35).unwrap().unwrap();
        OpenOptions::new()
            .append(true)
            .open(&seeded.source_path)
            .unwrap()
            .write_all(b"changed after planning")
            .unwrap();

        let error = store
            .switch_lar_repack(&copied.run_id, 36)
            .unwrap_err()
            .to_string();
        assert!(error.contains("identity changed"), "{error}");
        let conn = store.conn.lock().unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT file_uuid FROM lar_chunks WHERE chunk_hash=?1",
                [seeded.reachable_hash.digest.as_slice()],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
            seeded.source_file_uuid
        );
        assert_eq!(
            conn.query_row(
                "SELECT state FROM lar_files WHERE file_uuid=?1",
                [seeded.source_file_uuid],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
            "sealed"
        );
    }

    #[test]
    fn pre_identity_active_plan_is_failed_during_schema_upgrade() {
        let data_dir = tmpdir("pre-identity-plan");
        {
            let store = Store::open(data_dir.clone()).unwrap();
            let seeded = seed_pack(&store);
            let conn = store.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO lar_repack_runs
                   (run_id, archive_set_uuid, source_file_uuid,
                    destination_file_uuid, source_path, destination_temp_path,
                    destination_path, quarantine_path, state, started_at_ms,
                    updated_at_ms, source_size_bytes)
                 VALUES ('old-run', 'repack-set', ?1, 'old-destination', ?2,
                         'old.tmp', 'old.lar', 'old.quarantine', 'copied',
                         1, 1, ?3)",
                params![
                    seeded.source_file_uuid,
                    seeded.source_path.to_string_lossy(),
                    fs::metadata(&seeded.source_path).unwrap().len(),
                ],
            )
            .unwrap();
        }

        let reopened = Store::open(data_dir).unwrap();
        let conn = reopened.conn.lock().unwrap();
        let row: (String, String) = conn
            .query_row(
                "SELECT state, last_error FROM lar_repack_runs WHERE run_id='old-run'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(row.0, "failed");
        assert!(row.1.contains("predates durable source identity"));
    }

    #[test]
    fn copied_and_switched_boundaries_resume_after_restart() {
        let data_dir = tmpdir("restart");
        let (run_id, expected) = {
            let store = Store::open(data_dir.clone()).unwrap();
            let seeded = seed_pack(&store);
            let copied = store.start_lar_repack(&config(), 40).unwrap().unwrap();
            assert_eq!(copied.state, "copied");
            (copied.run_id, seeded.reachable_bytes)
        };

        {
            let reopened = Store::open(data_dir.clone()).unwrap();
            let switched = reopened.resume_lar_repack(&run_id, 41).unwrap();
            assert_eq!(switched.state, "switched");
            assert_eq!(
                reopened
                    .read_lar_or_legacy_artifact("trace", "trace-a", "client_request", None)
                    .unwrap()
                    .unwrap(),
                expected
            );
        }

        let reopened = Store::open(data_dir).unwrap();
        let complete = reopened.resume_lar_repack(&run_id, 42).unwrap();
        assert_eq!(complete.state, "complete");
        assert_eq!(reopened.resume_lar_repack(&run_id, 43).unwrap(), complete);
    }

    #[test]
    fn final_recheck_refuses_to_publish_a_stale_reachable_set() {
        let store = Store::open(tmpdir("final-recheck")).unwrap();
        let seeded = seed_pack(&store);
        let copied = store.start_lar_repack(&config(), 50).unwrap().unwrap();
        store.delete_trace("trace-a").unwrap();
        store.delete_trace("trace-b").unwrap();
        assert!(store.switch_lar_repack(&copied.run_id, 51).is_err());
        let conn = store.conn.lock().unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT file_uuid FROM lar_chunks WHERE chunk_hash=?1",
                [seeded.reachable_hash.digest.as_slice()],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
            seeded.source_file_uuid
        );
        assert_eq!(seeded.manifest_id.len(), 64);
    }

    #[test]
    fn final_recheck_refuses_new_non_chunk_catalog_ownership() {
        let store = Store::open(tmpdir("final-metadata-recheck")).unwrap();
        let seeded = seed_pack(&store);
        let copied = store.start_lar_repack(&config(), 60).unwrap().unwrap();
        {
            let conn = store.conn.lock().unwrap();
            conn.execute(
                "UPDATE lar_manifests SET file_uuid=?2, record_id=?3
                  WHERE manifest_id=?1",
                params![
                    &seeded.manifest_id,
                    &seeded.source_file_uuid,
                    format!("manifest:{}", seeded.manifest_id),
                ],
            )
            .unwrap();
        }

        let error = store.switch_lar_repack(&copied.run_id, 61).unwrap_err();
        assert!(
            format!("{error:#}").contains("catalog-owned manifest"),
            "unexpected error: {error:#}"
        );
        let conn = store.conn.lock().unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT file_uuid FROM lar_chunks WHERE chunk_hash=?1",
                [seeded.reachable_hash.digest.as_slice()],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
            seeded.source_file_uuid
        );
        assert_eq!(
            conn.query_row(
                "SELECT state FROM lar_files WHERE file_uuid=?1",
                [seeded.source_file_uuid],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
            "sealed"
        );
    }
}
