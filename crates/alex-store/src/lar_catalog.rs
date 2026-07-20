//! SQLite catalog and migration state for LAR artifacts.
//!
//! This module is deliberately independent of the physical LAR reader/writer.
//! The catalog records durable object identities and never treats file offsets
//! as public IDs. Legacy body paths remain authoritative until an imported
//! manifest has been reconstructed and validated through the normal reader.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde_json::{json, Value};

use crate::Store;

const LAR_CATALOG_SCHEMA_VERSION: i64 = 3;

const LAR_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS lar_schema_versions (
  version       INTEGER PRIMARY KEY,
  applied_at_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS lar_archive_sets (
  archive_set_uuid TEXT PRIMARY KEY,
  created_at_ms    INTEGER NOT NULL,
  updated_at_ms    INTEGER NOT NULL,
  state            TEXT NOT NULL DEFAULT 'active'
                   CHECK (state IN ('active', 'sealed', 'offline', 'retired')),
  description      TEXT,
  catalog_revision INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS lar_files (
  file_uuid             TEXT PRIMARY KEY,
  archive_set_uuid      TEXT NOT NULL,
  role                   TEXT NOT NULL
                         CHECK (role IN ('body-pack', 'event-log', 'standalone', 'search-pack', 'dictionary')),
  path                   TEXT NOT NULL,
  state                  TEXT NOT NULL DEFAULT 'active'
                         CHECK (state IN ('active', 'sealed', 'offline', 'repairing', 'retired')),
  container_major        INTEGER NOT NULL,
  container_minor        INTEGER NOT NULL,
  required_feature_bits  INTEGER NOT NULL DEFAULT 0,
  optional_feature_bits  INTEGER NOT NULL DEFAULT 0,
  created_at_ms          INTEGER NOT NULL,
  sealed_at_ms           INTEGER,
  size_bytes             INTEGER,
  footer_offset          INTEGER,
  UNIQUE (archive_set_uuid, path),
  FOREIGN KEY (archive_set_uuid) REFERENCES lar_archive_sets(archive_set_uuid)
);
CREATE INDEX IF NOT EXISTS lar_files_archive_state
  ON lar_files(archive_set_uuid, state, role);

CREATE TABLE IF NOT EXISTS lar_checkpoints (
  file_uuid          TEXT NOT NULL,
  checkpoint_sequence INTEGER NOT NULL,
  record_id          TEXT NOT NULL,
  frame_offset       INTEGER,
  frame_length       INTEGER,
  append_offset      INTEGER NOT NULL,
  created_at_ms      INTEGER NOT NULL,
  checksum           BLOB NOT NULL,
  PRIMARY KEY (file_uuid, checkpoint_sequence),
  FOREIGN KEY (file_uuid) REFERENCES lar_files(file_uuid)
);

CREATE TABLE IF NOT EXISTS lar_chunks (
  hash_algorithm       TEXT NOT NULL,
  chunk_hash           BLOB NOT NULL,
  uncompressed_length  INTEGER NOT NULL CHECK (uncompressed_length >= 0),
  compression          TEXT NOT NULL,
  dictionary_id        TEXT,
  compressed_length    INTEGER NOT NULL CHECK (compressed_length >= 0),
  file_uuid             TEXT NOT NULL,
  record_id             TEXT NOT NULL,
  page_offset           INTEGER NOT NULL,
  record_offset         INTEGER,
  checksum              BLOB NOT NULL,
  created_at_ms         INTEGER NOT NULL,
  state                 TEXT NOT NULL DEFAULT 'ready'
                        CHECK (state IN ('ready', 'quarantined', 'unreachable')),
  PRIMARY KEY (hash_algorithm, chunk_hash),
  UNIQUE (file_uuid, record_id),
  FOREIGN KEY (file_uuid) REFERENCES lar_files(file_uuid)
);
CREATE INDEX IF NOT EXISTS lar_chunks_file ON lar_chunks(file_uuid, page_offset);

CREATE TABLE IF NOT EXISTS lar_manifests (
  manifest_id         TEXT PRIMARY KEY,
  total_length        INTEGER NOT NULL CHECK (total_length >= 0),
  hash_algorithm      TEXT NOT NULL,
  whole_body_hash     BLOB NOT NULL,
  media_type          TEXT,
  content_encoding    TEXT,
  file_uuid            TEXT,
  record_id            TEXT,
  created_at_ms        INTEGER NOT NULL,
  state                TEXT NOT NULL DEFAULT 'ready'
                       CHECK (state IN ('ready', 'quarantined', 'unreachable')),
  UNIQUE (file_uuid, record_id),
  FOREIGN KEY (file_uuid) REFERENCES lar_files(file_uuid)
);
CREATE INDEX IF NOT EXISTS lar_manifests_hash
  ON lar_manifests(hash_algorithm, whole_body_hash, total_length);

CREATE TABLE IF NOT EXISTS lar_manifest_chunks (
  manifest_id          TEXT NOT NULL,
  ordinal              INTEGER NOT NULL CHECK (ordinal >= 0),
  hash_algorithm       TEXT NOT NULL,
  chunk_hash           BLOB NOT NULL,
  logical_offset       INTEGER NOT NULL CHECK (logical_offset >= 0),
  chunk_offset         INTEGER NOT NULL DEFAULT 0 CHECK (chunk_offset >= 0),
  length               INTEGER NOT NULL CHECK (length >= 0),
  PRIMARY KEY (manifest_id, ordinal),
  FOREIGN KEY (manifest_id) REFERENCES lar_manifests(manifest_id),
  FOREIGN KEY (hash_algorithm, chunk_hash) REFERENCES lar_chunks(hash_algorithm, chunk_hash)
);
CREATE INDEX IF NOT EXISTS lar_manifest_chunks_chunk
  ON lar_manifest_chunks(hash_algorithm, chunk_hash, manifest_id);

CREATE TABLE IF NOT EXISTS lar_header_atoms (
  atom_id               TEXT PRIMARY KEY,
  original_name_bytes   BLOB NOT NULL,
  value_bytes           BLOB NOT NULL,
  flags                  INTEGER NOT NULL DEFAULT 0,
  created_at_ms          INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS lar_header_blocks (
  block_id               TEXT PRIMARY KEY,
  fidelity               TEXT NOT NULL
                         CHECK (fidelity IN ('observed_ordered', 'legacy_normalized', 'derived')),
  fidelity_detail        TEXT,
  atom_count             INTEGER NOT NULL CHECK (atom_count >= 0),
  file_uuid              TEXT,
  record_id              TEXT,
  created_at_ms          INTEGER NOT NULL,
  UNIQUE (file_uuid, record_id),
  FOREIGN KEY (file_uuid) REFERENCES lar_files(file_uuid)
);

CREATE TABLE IF NOT EXISTS lar_header_block_atoms (
  block_id               TEXT NOT NULL,
  ordinal                INTEGER NOT NULL CHECK (ordinal >= 0),
  atom_id                TEXT NOT NULL,
  PRIMARY KEY (block_id, ordinal),
  FOREIGN KEY (block_id) REFERENCES lar_header_blocks(block_id),
  FOREIGN KEY (atom_id) REFERENCES lar_header_atoms(atom_id)
);
CREATE INDEX IF NOT EXISTS lar_header_block_atoms_atom
  ON lar_header_block_atoms(atom_id, block_id);

CREATE TABLE IF NOT EXISTS lar_stage_records (
  stage_id                   TEXT PRIMARY KEY,
  trace_id                   TEXT NOT NULL,
  capture_sequence           INTEGER NOT NULL,
  kind                       TEXT NOT NULL,
  attempt_number             INTEGER,
  wall_time_ns               INTEGER,
  monotonic_delta_ns         INTEGER,
  request_headers_ref        TEXT,
  request_body_manifest_ref  TEXT,
  response_headers_ref       TEXT,
  response_body_manifest_ref TEXT,
  trailers_ref               TEXT,
  stream_index_ref           TEXT,
  file_uuid                  TEXT,
  record_id                  TEXT,
  fidelity                   TEXT NOT NULL DEFAULT 'captured',
  UNIQUE (trace_id, capture_sequence),
  UNIQUE (file_uuid, record_id)
);
CREATE INDEX IF NOT EXISTS lar_stage_records_trace
  ON lar_stage_records(trace_id, capture_sequence);

-- Trace-to-exchange ownership is explicit rather than inferred from stages:
-- an exchange may contain zero stages, and content-addressed stages may be
-- repeated or shared by multiple exchanges.
CREATE TABLE IF NOT EXISTS lar_exchange_records (
  trace_id          TEXT PRIMARY KEY,
  exchange_id       TEXT NOT NULL,
  capture_sequence  INTEGER NOT NULL,
  stage_count       INTEGER NOT NULL CHECK (stage_count >= 0),
  file_uuid         TEXT NOT NULL,
  fidelity          TEXT NOT NULL DEFAULT 'captured',
  UNIQUE (file_uuid, exchange_id),
  FOREIGN KEY (file_uuid) REFERENCES lar_files(file_uuid)
);
CREATE INDEX IF NOT EXISTS lar_exchange_records_file
  ON lar_exchange_records(file_uuid, capture_sequence);

CREATE TABLE IF NOT EXISTS lar_trace_artifacts (
  owner_kind          TEXT NOT NULL CHECK (owner_kind IN ('trace', 'tool_call')),
  owner_id            TEXT NOT NULL,
  artifact_kind       TEXT NOT NULL,
  stage_id            TEXT NOT NULL DEFAULT '',
  manifest_id         TEXT,
  header_block_id     TEXT,
  legacy_path         TEXT,
  source_fingerprint  TEXT,
  fidelity            TEXT NOT NULL,
  validation_state    TEXT NOT NULL CHECK (validation_state = 'validated'),
  validated_at_ms     INTEGER NOT NULL,
  pointer_revision    INTEGER NOT NULL DEFAULT 1,
  CHECK (manifest_id IS NOT NULL OR header_block_id IS NOT NULL),
  PRIMARY KEY (owner_kind, owner_id, artifact_kind, stage_id),
  FOREIGN KEY (manifest_id) REFERENCES lar_manifests(manifest_id),
  FOREIGN KEY (header_block_id) REFERENCES lar_header_blocks(block_id)
);
CREATE INDEX IF NOT EXISTS lar_trace_artifacts_manifest
  ON lar_trace_artifacts(manifest_id, owner_kind, owner_id);
CREATE INDEX IF NOT EXISTS lar_trace_artifacts_owner
  ON lar_trace_artifacts(owner_kind, owner_id, artifact_kind);

CREATE TABLE IF NOT EXISTS lar_session_revisions (
  session_id       TEXT PRIMARY KEY,
  revision         INTEGER NOT NULL DEFAULT 0,
  updated_at_ms    INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS lar_migration_jobs (
  job_id                    TEXT PRIMARY KEY,
  format_version            INTEGER NOT NULL,
  source_version            TEXT NOT NULL,
  source_key                TEXT NOT NULL,
  state                     TEXT NOT NULL DEFAULT 'pending'
                            CHECK (state IN ('pending', 'running', 'paused', 'complete', 'failed')),
  lease_owner               TEXT,
  lease_expires_at_ms       INTEGER,
  started_at_ms             INTEGER,
  created_at_ms             INTEGER NOT NULL,
  updated_at_ms             INTEGER NOT NULL,
  completed_at_ms           INTEGER,
  last_committed_cursor     TEXT,
  discovered_count          INTEGER NOT NULL DEFAULT 0,
  pending_count             INTEGER NOT NULL DEFAULT 0,
  migrated_count            INTEGER NOT NULL DEFAULT 0,
  skipped_count             INTEGER NOT NULL DEFAULT 0,
  failed_count              INTEGER NOT NULL DEFAULT 0,
  bytes_read                INTEGER NOT NULL DEFAULT 0,
  unique_bytes_written      INTEGER NOT NULL DEFAULT 0,
  bytes_deduplicated        INTEGER NOT NULL DEFAULT 0,
  last_error                TEXT,
  UNIQUE (format_version, source_version, source_key)
);
CREATE INDEX IF NOT EXISTS lar_migration_jobs_state_lease
  ON lar_migration_jobs(state, lease_expires_at_ms);

CREATE TABLE IF NOT EXISTS lar_migration_items (
  item_id                   TEXT PRIMARY KEY,
  job_id                    TEXT NOT NULL,
  owner_kind                TEXT NOT NULL CHECK (owner_kind IN ('trace', 'tool_call')),
  owner_id                  TEXT NOT NULL,
  artifact_kind             TEXT NOT NULL,
  stage_id                  TEXT NOT NULL DEFAULT '',
  source_path               TEXT,
  source_size               INTEGER,
  source_mtime_ms           INTEGER,
  source_fingerprint        TEXT NOT NULL,
  fidelity                  TEXT NOT NULL,
  state                     TEXT NOT NULL DEFAULT 'pending'
                            CHECK (state IN ('pending', 'migrating', 'migrated', 'skipped', 'failed')),
  destination_manifest_id   TEXT,
  destination_exchange_id   TEXT,
  destination_file_uuid     TEXT,
  metadata_stage_count      INTEGER NOT NULL DEFAULT 0 CHECK (metadata_stage_count >= 0),
  metadata_header_count     INTEGER NOT NULL DEFAULT 0 CHECK (metadata_header_count >= 0),
  metadata_unsupported_count INTEGER NOT NULL DEFAULT 0 CHECK (metadata_unsupported_count >= 0),
  source_length             INTEGER,
  source_hash_algorithm     TEXT,
  source_hash               BLOB,
  validation_state          TEXT NOT NULL DEFAULT 'pending'
                            CHECK (validation_state IN ('pending', 'validated', 'failed')),
  error_kind                TEXT,
  validation_error          TEXT,
  bytes_read                INTEGER NOT NULL DEFAULT 0,
  unique_bytes_written      INTEGER NOT NULL DEFAULT 0,
  bytes_deduplicated        INTEGER NOT NULL DEFAULT 0,
  cleanup_eligible          INTEGER NOT NULL DEFAULT 0 CHECK (cleanup_eligible IN (0, 1)),
  created_at_ms             INTEGER NOT NULL,
  updated_at_ms             INTEGER NOT NULL,
  completed_at_ms           INTEGER,
  UNIQUE (job_id, owner_kind, owner_id, artifact_kind, stage_id, source_fingerprint),
  FOREIGN KEY (job_id) REFERENCES lar_migration_jobs(job_id),
  FOREIGN KEY (destination_manifest_id) REFERENCES lar_manifests(manifest_id),
  FOREIGN KEY (destination_file_uuid) REFERENCES lar_files(file_uuid)
);
CREATE INDEX IF NOT EXISTS lar_migration_items_job_state
  ON lar_migration_items(job_id, state, updated_at_ms);
CREATE INDEX IF NOT EXISTS lar_migration_items_owner
  ON lar_migration_items(owner_kind, owner_id, artifact_kind);

CREATE TABLE IF NOT EXISTS lar_gc_runs (
  run_id                   TEXT PRIMARY KEY,
  archive_set_uuid         TEXT NOT NULL,
  state                    TEXT NOT NULL
                           CHECK (state IN ('marking', 'sweeping', 'repacking', 'complete', 'failed')),
  started_at_ms            INTEGER NOT NULL,
  updated_at_ms            INTEGER NOT NULL,
  completed_at_ms          INTEGER,
  reachable_manifests      INTEGER NOT NULL DEFAULT 0,
  reachable_chunks         INTEGER NOT NULL DEFAULT 0,
  unreachable_chunks       INTEGER NOT NULL DEFAULT 0,
  bytes_reclaimed          INTEGER NOT NULL DEFAULT 0,
  last_error               TEXT,
  FOREIGN KEY (archive_set_uuid) REFERENCES lar_archive_sets(archive_set_uuid)
);
"#;

pub(crate) fn migrate(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute_batch(LAR_SCHEMA)?;
    let manifest_schema: String = tx.query_row(
        "SELECT sql FROM sqlite_master WHERE type='table' AND name='lar_manifests'",
        [],
        |row| row.get(0),
    )?;
    if manifest_schema.contains("UNIQUE (hash_algorithm, whole_body_hash, total_length)") {
        // Catalog v2 conflated byte identity with manifest identity. Rebuild
        // the small manifest table so media/encoding variants and alternate
        // chunk topology can share chunks without losing their own IDs.
        tx.execute_batch(
            "PRAGMA defer_foreign_keys=ON;
             CREATE TABLE lar_manifests_v3 (
               manifest_id TEXT PRIMARY KEY,
               total_length INTEGER NOT NULL CHECK (total_length >= 0),
               hash_algorithm TEXT NOT NULL,
               whole_body_hash BLOB NOT NULL,
               media_type TEXT,
               content_encoding TEXT,
               file_uuid TEXT,
               record_id TEXT,
               created_at_ms INTEGER NOT NULL,
               state TEXT NOT NULL DEFAULT 'ready'
                 CHECK (state IN ('ready', 'quarantined', 'unreachable')),
               UNIQUE (file_uuid, record_id),
               FOREIGN KEY (file_uuid) REFERENCES lar_files(file_uuid)
             );
             INSERT INTO lar_manifests_v3
               (manifest_id, total_length, hash_algorithm, whole_body_hash,
                media_type, content_encoding, file_uuid, record_id,
                created_at_ms, state)
             SELECT manifest_id, total_length, hash_algorithm, whole_body_hash,
                    media_type, content_encoding, file_uuid, record_id,
                    created_at_ms, state
               FROM lar_manifests;
             DROP TABLE lar_manifests;
             ALTER TABLE lar_manifests_v3 RENAME TO lar_manifests;
             CREATE INDEX lar_manifests_hash
               ON lar_manifests(hash_algorithm, whole_body_hash, total_length);",
        )?;
    }
    for (name, declaration) in [
        ("destination_exchange_id", "TEXT"),
        ("destination_file_uuid", "TEXT"),
        (
            "metadata_stage_count",
            "INTEGER NOT NULL DEFAULT 0 CHECK (metadata_stage_count >= 0)",
        ),
        (
            "metadata_header_count",
            "INTEGER NOT NULL DEFAULT 0 CHECK (metadata_header_count >= 0)",
        ),
        (
            "metadata_unsupported_count",
            "INTEGER NOT NULL DEFAULT 0 CHECK (metadata_unsupported_count >= 0)",
        ),
    ] {
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('lar_migration_items') WHERE name=?1)",
            [name],
            |row| row.get(0),
        )?;
        if !exists {
            tx.execute_batch(&format!(
                "ALTER TABLE lar_migration_items ADD COLUMN {name} {declaration}"
            ))?;
        }
    }
    let has_header_fidelity_detail: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('lar_header_blocks') WHERE name='fidelity_detail')",
        [],
        |row| row.get(0),
    )?;
    if !has_header_fidelity_detail {
        tx.execute_batch("ALTER TABLE lar_header_blocks ADD COLUMN fidelity_detail TEXT")?;
    }
    tx.execute(
        "INSERT OR IGNORE INTO lar_schema_versions (version, applied_at_ms)
         VALUES (?1, CAST(strftime('%s', 'now') AS INTEGER) * 1000)",
        [LAR_CATALOG_SCHEMA_VERSION],
    )?;
    tx.commit()?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LarMigrationJobSpec {
    pub job_id: String,
    pub format_version: i64,
    pub source_version: String,
    pub source_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LarMigrationJob {
    pub job_id: String,
    pub state: String,
    pub lease_owner: Option<String>,
    pub lease_expires_at_ms: Option<i64>,
    pub discovered_count: u64,
    pub pending_count: u64,
    pub migrated_count: u64,
    pub skipped_count: u64,
    pub failed_count: u64,
    pub bytes_read: u64,
    pub unique_bytes_written: u64,
    pub bytes_deduplicated: u64,
    pub last_committed_cursor: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LarMigrationItem {
    pub item_id: String,
    pub job_id: String,
    pub owner_kind: String,
    pub owner_id: String,
    pub artifact_kind: String,
    pub stage_id: Option<String>,
    pub source_path: Option<String>,
    pub source_size: Option<u64>,
    pub source_mtime_ms: Option<i64>,
    pub source_fingerprint: String,
    pub fidelity: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LarManifestRegistration {
    pub manifest_id: String,
    pub total_length: u64,
    pub hash_algorithm: String,
    pub whole_body_hash: Vec<u8>,
    pub media_type: Option<String>,
    pub content_encoding: Option<String>,
    pub file_uuid: Option<String>,
    pub record_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LarValidation {
    pub source_length: u64,
    pub source_hash_algorithm: String,
    pub source_hash: Vec<u8>,
    pub reconstructed_length: u64,
    pub reconstructed_hash: Vec<u8>,
    pub bytes_read: u64,
    pub unique_bytes_written: u64,
    pub bytes_deduplicated: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LarPointerSwitch {
    Switched { manifest_id: String },
    AlreadySwitched { manifest_id: String },
    ValidationFailed { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LarArtifactLocation {
    Lar {
        manifest_id: String,
        total_length: u64,
        hash_algorithm: String,
        whole_body_hash: Vec<u8>,
        fidelity: String,
    },
    Legacy {
        path: String,
        migration_error: Option<LarArtifactError>,
    },
    Unavailable {
        source_path: Option<String>,
        error: LarArtifactError,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LarArtifactError {
    pub kind: String,
    pub detail: String,
}

struct MigrationSourceRow {
    path: Option<String>,
    state: String,
    error_kind: Option<String>,
    error_detail: Option<String>,
}

fn require_nonempty(value: &str, name: &str) -> Result<()> {
    if value.is_empty() {
        bail!("{name} must not be empty");
    }
    Ok(())
}

fn require_owner_kind(owner_kind: &str) -> Result<()> {
    match owner_kind {
        "trace" | "tool_call" => Ok(()),
        _ => bail!("unsupported LAR artifact owner kind: {owner_kind}"),
    }
}

fn u64_to_i64(value: u64, field: &str) -> Result<i64> {
    i64::try_from(value).with_context(|| format!("{field} exceeds SQLite's integer range"))
}

fn nonnegative_u64(value: i64, field: &str) -> rusqlite::Result<u64> {
    u64::try_from(value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Integer,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("negative {field}: {error}"),
            )),
        )
    })
}

fn migration_job_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<LarMigrationJob> {
    Ok(LarMigrationJob {
        job_id: row.get(0)?,
        state: row.get(1)?,
        lease_owner: row.get(2)?,
        lease_expires_at_ms: row.get(3)?,
        discovered_count: nonnegative_u64(row.get(4)?, "discovered_count")?,
        pending_count: nonnegative_u64(row.get(5)?, "pending_count")?,
        migrated_count: nonnegative_u64(row.get(6)?, "migrated_count")?,
        skipped_count: nonnegative_u64(row.get(7)?, "skipped_count")?,
        failed_count: nonnegative_u64(row.get(8)?, "failed_count")?,
        bytes_read: nonnegative_u64(row.get(9)?, "bytes_read")?,
        unique_bytes_written: nonnegative_u64(row.get(10)?, "unique_bytes_written")?,
        bytes_deduplicated: nonnegative_u64(row.get(11)?, "bytes_deduplicated")?,
        last_committed_cursor: row.get(12)?,
        last_error: row.get(13)?,
    })
}

fn recount_job(tx: &rusqlite::Transaction<'_>, job_id: &str, now_ms: i64) -> Result<()> {
    tx.execute(
        "UPDATE lar_migration_jobs SET
           discovered_count=(SELECT COUNT(*) FROM lar_migration_items WHERE job_id=?1),
           pending_count=(SELECT COUNT(*) FROM lar_migration_items WHERE job_id=?1 AND state IN ('pending','migrating')),
           migrated_count=(SELECT COUNT(*) FROM lar_migration_items WHERE job_id=?1 AND state='migrated'),
           skipped_count=(SELECT COUNT(*) FROM lar_migration_items WHERE job_id=?1 AND state='skipped'),
           failed_count=(SELECT COUNT(*) FROM lar_migration_items WHERE job_id=?1 AND state='failed'),
           bytes_read=COALESCE((SELECT SUM(bytes_read) FROM lar_migration_items WHERE job_id=?1), 0),
           unique_bytes_written=COALESCE((SELECT SUM(unique_bytes_written) FROM lar_migration_items WHERE job_id=?1), 0),
           bytes_deduplicated=COALESCE((SELECT SUM(bytes_deduplicated) FROM lar_migration_items WHERE job_id=?1), 0),
           updated_at_ms=?2
         WHERE job_id=?1",
        params![job_id, now_ms],
    )?;
    Ok(())
}

fn require_live_lease(
    tx: &rusqlite::Transaction<'_>,
    job_id: &str,
    lease_owner: &str,
    now_ms: i64,
) -> Result<()> {
    let owns: bool = tx
        .query_row(
            "SELECT lease_owner=?2 AND lease_expires_at_ms>?3 AND state='running'
             FROM lar_migration_jobs WHERE job_id=?1",
            params![job_id, lease_owner, now_ms],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or(false);
    if !owns {
        bail!("migration job {job_id} does not have a live lease for {lease_owner}");
    }
    Ok(())
}

impl Store {
    /// Return the latest installed LAR catalog schema version.
    pub fn lar_catalog_schema_version(&self) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        Ok(conn.query_row(
            "SELECT COALESCE(MAX(version), 0) FROM lar_schema_versions",
            [],
            |row| row.get(0),
        )?)
    }

    /// Create a migration job once and return the durable job for this source.
    /// Repeated startup calls reuse the same job even if the caller generated a
    /// different candidate `job_id`.
    pub fn ensure_lar_migration_job(
        &self,
        spec: &LarMigrationJobSpec,
        now_ms: i64,
    ) -> Result<LarMigrationJob> {
        require_nonempty(&spec.job_id, "job_id")?;
        require_nonempty(&spec.source_version, "source_version")?;
        require_nonempty(&spec.source_key, "source_key")?;
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO lar_migration_jobs
               (job_id, format_version, source_version, source_key, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5)
             ON CONFLICT(format_version, source_version, source_key) DO NOTHING",
            params![
                spec.job_id,
                spec.format_version,
                spec.source_version,
                spec.source_key,
                now_ms
            ],
        )?;
        conn.query_row(
            "SELECT job_id, state, lease_owner, lease_expires_at_ms,
                    discovered_count, pending_count, migrated_count, skipped_count, failed_count,
                    bytes_read, unique_bytes_written, bytes_deduplicated,
                    last_committed_cursor, last_error
             FROM lar_migration_jobs
             WHERE format_version=?1 AND source_version=?2 AND source_key=?3",
            params![spec.format_version, spec.source_version, spec.source_key],
            migration_job_row,
        )
        .map_err(Into::into)
    }

    /// Acquire or recover a migration lease. A lease held by another owner is
    /// never stolen before expiry.
    pub fn claim_lar_migration_job(
        &self,
        job_id: &str,
        lease_owner: &str,
        now_ms: i64,
        lease_duration: Duration,
    ) -> Result<bool> {
        require_nonempty(lease_owner, "lease_owner")?;
        let lease_ms = i64::try_from(lease_duration.as_millis())
            .context("lease duration exceeds SQLite's integer range")?;
        if lease_ms <= 0 {
            bail!("lease duration must be positive");
        }
        let lease_expires_at_ms = now_ms
            .checked_add(lease_ms)
            .context("lease expiry overflows i64")?;
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = tx.execute(
            "UPDATE lar_migration_jobs
             SET state='running', lease_owner=?2, lease_expires_at_ms=?3,
                 started_at_ms=COALESCE(started_at_ms, ?4), updated_at_ms=?4
             WHERE job_id=?1 AND state IN ('pending','running','failed')
               AND (lease_owner IS NULL OR lease_owner=?2 OR lease_expires_at_ms<=?4)",
            params![job_id, lease_owner, lease_expires_at_ms, now_ms],
        )?;
        tx.commit()?;
        Ok(changed == 1)
    }

    pub fn renew_lar_migration_lease(
        &self,
        job_id: &str,
        lease_owner: &str,
        now_ms: i64,
        lease_duration: Duration,
    ) -> Result<bool> {
        let lease_ms = i64::try_from(lease_duration.as_millis())
            .context("lease duration exceeds SQLite's integer range")?;
        let expires = now_ms
            .checked_add(lease_ms)
            .context("lease expiry overflows i64")?;
        let conn = self.conn.lock().unwrap();
        Ok(conn.execute(
            "UPDATE lar_migration_jobs SET lease_expires_at_ms=?3, updated_at_ms=?4
             WHERE job_id=?1 AND lease_owner=?2 AND state='running'
               AND lease_expires_at_ms>?4",
            params![job_id, lease_owner, expires, now_ms],
        )? == 1)
    }

    pub fn lar_migration_job(&self, job_id: &str) -> Result<Option<LarMigrationJob>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT job_id, state, lease_owner, lease_expires_at_ms,
                    discovered_count, pending_count, migrated_count, skipped_count, failed_count,
                    bytes_read, unique_bytes_written, bytes_deduplicated,
                    last_committed_cursor, last_error
             FROM lar_migration_jobs WHERE job_id=?1",
            [job_id],
            migration_job_row,
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn list_lar_migration_jobs(&self) -> Result<Vec<LarMigrationJob>> {
        let conn = self.conn.lock().unwrap();
        let mut statement = conn.prepare(
            "SELECT job_id, state, lease_owner, lease_expires_at_ms,
                    discovered_count, pending_count, migrated_count, skipped_count, failed_count,
                    bytes_read, unique_bytes_written, bytes_deduplicated,
                    last_committed_cursor, last_error
             FROM lar_migration_jobs ORDER BY updated_at_ms DESC, created_at_ms DESC",
        )?;
        let rows = statement.query_map([], migration_job_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Pause is an operator action and releases the worker lease. Resume puts
    /// the job back in the claimable pending state; a startup/background worker
    /// still has to acquire a lease before processing it.
    pub fn set_lar_migration_paused(
        &self,
        job_id: &str,
        paused: bool,
        now_ms: i64,
    ) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let changed = if paused {
            conn.execute(
                "UPDATE lar_migration_jobs
                 SET state='paused', lease_owner=NULL, lease_expires_at_ms=NULL, updated_at_ms=?2
                 WHERE job_id=?1 AND state IN ('pending','running','failed')",
                params![job_id, now_ms],
            )?
        } else {
            conn.execute(
                "UPDATE lar_migration_jobs SET state='pending', updated_at_ms=?2
                 WHERE job_id=?1 AND state='paused'",
                params![job_id, now_ms],
            )?
        };
        Ok(changed == 1)
    }

    /// Persist the last fully committed inventory/batch cursor. Callers must
    /// checkpoint only after the corresponding item and pointer transactions.
    pub fn checkpoint_lar_migration_job(
        &self,
        job_id: &str,
        lease_owner: &str,
        cursor: &str,
        now_ms: i64,
    ) -> Result<()> {
        require_nonempty(cursor, "migration cursor")?;
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        require_live_lease(&tx, job_id, lease_owner, now_ms)?;
        tx.execute(
            "UPDATE lar_migration_jobs
             SET last_committed_cursor=?2, updated_at_ms=?3 WHERE job_id=?1",
            params![job_id, cursor, now_ms],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Seal a job only when every discovered artifact has a terminal successful
    /// state. Failed items require repair/retry or an explicit skip first.
    pub fn complete_lar_migration_job(
        &self,
        job_id: &str,
        lease_owner: &str,
        now_ms: i64,
    ) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        require_live_lease(&tx, job_id, lease_owner, now_ms)?;
        recount_job(&tx, job_id, now_ms)?;
        let unfinished: (i64, i64) = tx.query_row(
            "SELECT pending_count, failed_count FROM lar_migration_jobs WHERE job_id=?1",
            [job_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        if unfinished != (0, 0) {
            bail!(
                "migration job {job_id} cannot complete with {} pending and {} failed items",
                unfinished.0,
                unfinished.1
            );
        }
        tx.execute(
            "UPDATE lar_migration_jobs
             SET state='complete', completed_at_ms=?2, updated_at_ms=?2,
                 lease_owner=NULL, lease_expires_at_ms=NULL, last_error=NULL
             WHERE job_id=?1",
            params![job_id, now_ms],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Persist an inventory item without resetting completed work. The unique
    /// source identity makes discovery safe to repeat on every startup.
    pub fn discover_lar_migration_item(&self, item: &LarMigrationItem, now_ms: i64) -> Result<()> {
        require_owner_kind(&item.owner_kind)?;
        require_nonempty(&item.item_id, "item_id")?;
        require_nonempty(&item.source_fingerprint, "source_fingerprint")?;
        require_nonempty(&item.artifact_kind, "artifact_kind")?;
        let source_size = item
            .source_size
            .map(|value| u64_to_i64(value, "source_size"))
            .transpose()?;
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT INTO lar_migration_items
               (item_id, job_id, owner_kind, owner_id, artifact_kind, stage_id,
                source_path, source_size, source_mtime_ms, source_fingerprint, fidelity,
                created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?12)
             ON CONFLICT(job_id, owner_kind, owner_id, artifact_kind, stage_id, source_fingerprint)
             DO UPDATE SET source_path=excluded.source_path,
                           source_size=excluded.source_size,
                           source_mtime_ms=excluded.source_mtime_ms,
                           updated_at_ms=excluded.updated_at_ms
             WHERE lar_migration_items.state NOT IN ('migrated','skipped')",
            params![
                item.item_id,
                item.job_id,
                item.owner_kind,
                item.owner_id,
                item.artifact_kind,
                item.stage_id.as_deref().unwrap_or(""),
                item.source_path,
                source_size,
                item.source_mtime_ms,
                item.source_fingerprint,
                item.fidelity,
                now_ms,
            ],
        )?;
        // A repaired/replaced source has a new fingerprint and therefore a new
        // durable item. Preserve the old failed row as provenance, but remove
        // it from the job's blocking failure count so the validated replacement
        // can eventually complete the job.
        tx.execute(
            "UPDATE lar_migration_items
             SET state='skipped', cleanup_eligible=0,
                 validation_error=COALESCE(validation_error, '') ||
                   CASE WHEN validation_error IS NULL OR validation_error='' THEN '' ELSE '; ' END ||
                   'superseded by source fingerprint ' || ?7,
                 updated_at_ms=?8
             WHERE job_id=?1 AND owner_kind=?2 AND owner_id=?3 AND artifact_kind=?4
               AND stage_id=?5 AND item_id!=?6 AND state='failed'",
            params![
                item.job_id,
                item.owner_kind,
                item.owner_id,
                item.artifact_kind,
                item.stage_id.as_deref().unwrap_or(""),
                item.item_id,
                item.source_fingerprint,
                now_ms,
            ],
        )?;
        recount_job(&tx, &item.job_id, now_ms)?;
        tx.commit()?;
        Ok(())
    }

    /// Persist a source-side inventory/read failure without discarding its
    /// provenance. `missing`, `corrupt`, and `unsupported` are kept distinct so
    /// operators and later repair passes do not mistake them for validation
    /// failures in newly written LAR data.
    pub fn record_lar_migration_item_failure(
        &self,
        job_id: &str,
        item_id: &str,
        lease_owner: &str,
        error_kind: &str,
        detail: &str,
        now_ms: i64,
    ) -> Result<()> {
        require_nonempty(error_kind, "error_kind")?;
        require_nonempty(detail, "failure detail")?;
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        require_live_lease(&tx, job_id, lease_owner, now_ms)?;
        let changed = tx.execute(
            "UPDATE lar_migration_items SET state='failed', validation_state='failed',
               error_kind=?3, validation_error=?4, cleanup_eligible=0, updated_at_ms=?5
             WHERE item_id=?1 AND job_id=?2 AND state!='migrated'",
            params![item_id, job_id, error_kind, detail, now_ms],
        )?;
        if changed != 1 {
            bail!("migration item {item_id} was not pending in job {job_id}");
        }
        tx.execute(
            "UPDATE lar_migration_jobs SET last_error=?2 WHERE job_id=?1",
            params![job_id, detail],
        )?;
        recount_job(&tx, job_id, now_ms)?;
        tx.commit()?;
        Ok(())
    }

    /// Register a manifest after its physical bytes are recoverably appended.
    /// Content identity is checked on every retry; a manifest ID can never be
    /// rebound to different bytes.
    pub fn register_lar_manifest(
        &self,
        manifest: &LarManifestRegistration,
        now_ms: i64,
    ) -> Result<()> {
        require_nonempty(&manifest.manifest_id, "manifest_id")?;
        require_nonempty(&manifest.hash_algorithm, "hash_algorithm")?;
        let total_length = u64_to_i64(manifest.total_length, "total_length")?;
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO lar_manifests
               (manifest_id, total_length, hash_algorithm, whole_body_hash,
                media_type, content_encoding, file_uuid, record_id, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(manifest_id) DO NOTHING",
            params![
                manifest.manifest_id,
                total_length,
                manifest.hash_algorithm,
                manifest.whole_body_hash,
                manifest.media_type,
                manifest.content_encoding,
                manifest.file_uuid,
                manifest.record_id,
                now_ms,
            ],
        )?;
        let stored: (i64, String, Vec<u8>) = conn.query_row(
            "SELECT total_length, hash_algorithm, whole_body_hash
             FROM lar_manifests WHERE manifest_id=?1",
            [&manifest.manifest_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        if stored
            != (
                total_length,
                manifest.hash_algorithm.clone(),
                manifest.whole_body_hash.clone(),
            )
        {
            bail!(
                "manifest {} is already bound to different content",
                manifest.manifest_id
            );
        }
        Ok(())
    }

    /// Atomically mark an item validated and publish its artifact pointer.
    /// A validation or catalog mismatch is persisted on the migration item and
    /// leaves the existing legacy path as the readable source.
    #[allow(clippy::too_many_arguments)]
    pub fn switch_validated_lar_artifact(
        &self,
        job_id: &str,
        item_id: &str,
        lease_owner: &str,
        manifest_id: &str,
        validation: &LarValidation,
        session_id: Option<&str>,
        now_ms: i64,
    ) -> Result<LarPointerSwitch> {
        let source_length = u64_to_i64(validation.source_length, "source_length")?;
        let reconstructed_length =
            u64_to_i64(validation.reconstructed_length, "reconstructed_length")?;
        let bytes_read = u64_to_i64(validation.bytes_read, "bytes_read")?;
        let unique_bytes_written =
            u64_to_i64(validation.unique_bytes_written, "unique_bytes_written")?;
        let bytes_deduplicated = u64_to_i64(validation.bytes_deduplicated, "bytes_deduplicated")?;

        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        require_live_lease(&tx, job_id, lease_owner, now_ms)?;

        #[allow(clippy::type_complexity)]
        let item: Option<(
            String,
            String,
            String,
            String,
            Option<String>,
            String,
            String,
        )> = tx
            .query_row(
                "SELECT owner_kind, owner_id, artifact_kind, stage_id, source_path,
                        source_fingerprint, fidelity
                 FROM lar_migration_items WHERE item_id=?1 AND job_id=?2",
                params![item_id, job_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                    ))
                },
            )
            .optional()?;
        let Some((
            owner_kind,
            owner_id,
            artifact_kind,
            stage_id,
            source_path,
            fingerprint,
            fidelity,
        )) = item
        else {
            bail!("migration item {item_id} does not belong to job {job_id}");
        };

        let existing_manifest: Option<String> = tx
            .query_row(
                "SELECT destination_manifest_id FROM lar_migration_items
                 WHERE item_id=?1 AND state='migrated' AND validation_state='validated'",
                [item_id],
                |row| row.get(0),
            )
            .optional()?
            .flatten();
        if let Some(existing_manifest) = existing_manifest {
            if existing_manifest != manifest_id {
                bail!("migration item {item_id} is already bound to manifest {existing_manifest}");
            }
            tx.commit()?;
            return Ok(LarPointerSwitch::AlreadySwitched {
                manifest_id: existing_manifest,
            });
        }

        let mut failure = None;
        if source_length != reconstructed_length {
            failure = Some(format!(
                "length mismatch: source {source_length}, reconstructed {reconstructed_length}"
            ));
        } else if validation.source_hash != validation.reconstructed_hash {
            failure = Some("hash mismatch between source and reconstructed bytes".to_string());
        }

        let manifest: Option<(i64, String, Vec<u8>, String)> = tx
            .query_row(
                "SELECT total_length, hash_algorithm, whole_body_hash, state
                 FROM lar_manifests WHERE manifest_id=?1",
                [manifest_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?;
        match manifest {
            None => failure = Some(format!("manifest {manifest_id} is not cataloged")),
            Some((_, _, _, state)) if state != "ready" => {
                failure = Some(format!("manifest {manifest_id} is {state}"));
            }
            Some((length, algorithm, hash, _))
                if length != reconstructed_length
                    || algorithm != validation.source_hash_algorithm
                    || hash != validation.reconstructed_hash =>
            {
                failure = Some(format!(
                    "manifest {manifest_id} content identity does not match validation"
                ));
            }
            Some(_) => {}
        }

        if let Some(reason) = failure {
            tx.execute(
                "UPDATE lar_migration_items SET state='failed', validation_state='failed',
                   source_length=?2, source_hash_algorithm=?3, source_hash=?4,
                   error_kind='validation', validation_error=?5, bytes_read=?6, unique_bytes_written=?7,
                   bytes_deduplicated=?8, updated_at_ms=?9, cleanup_eligible=0
                 WHERE item_id=?1",
                params![
                    item_id,
                    source_length,
                    validation.source_hash_algorithm,
                    validation.source_hash,
                    reason,
                    bytes_read,
                    unique_bytes_written,
                    bytes_deduplicated,
                    now_ms,
                ],
            )?;
            tx.execute(
                "UPDATE lar_migration_jobs SET last_error=?2 WHERE job_id=?1",
                params![job_id, reason],
            )?;
            recount_job(&tx, job_id, now_ms)?;
            tx.commit()?;
            return Ok(LarPointerSwitch::ValidationFailed { reason });
        }

        tx.execute(
            "INSERT INTO lar_trace_artifacts
               (owner_kind, owner_id, artifact_kind, stage_id, manifest_id, legacy_path,
                source_fingerprint, fidelity, validation_state, validated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'validated', ?9)
             ON CONFLICT(owner_kind, owner_id, artifact_kind, stage_id) DO UPDATE SET
               manifest_id=excluded.manifest_id,
               legacy_path=COALESCE(lar_trace_artifacts.legacy_path, excluded.legacy_path),
               source_fingerprint=excluded.source_fingerprint,
               fidelity=excluded.fidelity,
               validation_state='validated',
               validated_at_ms=excluded.validated_at_ms,
               pointer_revision=lar_trace_artifacts.pointer_revision+1",
            params![
                owner_kind,
                owner_id,
                artifact_kind,
                stage_id,
                manifest_id,
                source_path,
                fingerprint,
                fidelity,
                now_ms,
            ],
        )?;
        tx.execute(
            "UPDATE lar_migration_items SET state='migrated', validation_state='validated',
               destination_manifest_id=?2, source_length=?3, source_hash_algorithm=?4,
               source_hash=?5, error_kind=NULL, validation_error=NULL, bytes_read=?6,
               unique_bytes_written=?7, bytes_deduplicated=?8, cleanup_eligible=1,
               updated_at_ms=?9, completed_at_ms=?9
             WHERE item_id=?1",
            params![
                item_id,
                manifest_id,
                source_length,
                validation.source_hash_algorithm,
                validation.source_hash,
                bytes_read,
                unique_bytes_written,
                bytes_deduplicated,
                now_ms,
            ],
        )?;
        if let Some(session_id) = session_id.filter(|value| !value.is_empty()) {
            tx.execute(
                "INSERT INTO lar_session_revisions (session_id, revision, updated_at_ms)
                 VALUES (?1, 1, ?2)
                 ON CONFLICT(session_id) DO UPDATE SET
                   revision=lar_session_revisions.revision+1,
                   updated_at_ms=excluded.updated_at_ms",
                params![session_id, now_ms],
            )?;
        }
        recount_job(&tx, job_id, now_ms)?;
        tx.commit()?;
        Ok(LarPointerSwitch::Switched {
            manifest_id: manifest_id.to_string(),
        })
    }

    /// Publish a legacy exchange only after its combined-pack records have
    /// been synced and validated through the normal archive reader. The
    /// caller's catalog closure and the durable migration receipt commit in
    /// one transaction, so a completed item can never name partially
    /// published headers or stages.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn publish_lar_migration_exchange<F>(
        &self,
        job_id: &str,
        item_id: &str,
        lease_owner: &str,
        exchange_id: &str,
        file_uuid: &str,
        session_id: Option<&str>,
        stage_count: u64,
        header_count: u64,
        unsupported_count: u64,
        now_ms: i64,
        publish_catalog: F,
    ) -> Result<bool>
    where
        F: FnOnce(&rusqlite::Transaction<'_>) -> Result<()>,
    {
        require_nonempty(exchange_id, "exchange_id")?;
        require_nonempty(file_uuid, "exchange file UUID")?;
        let stage_count = u64_to_i64(stage_count, "metadata_stage_count")?;
        let header_count = u64_to_i64(header_count, "metadata_header_count")?;
        let unsupported_count = u64_to_i64(unsupported_count, "metadata_unsupported_count")?;
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        require_live_lease(&tx, job_id, lease_owner, now_ms)?;

        let item: Option<(String, String, Option<String>, Option<String>)> = tx
            .query_row(
                "SELECT state, artifact_kind, destination_exchange_id, destination_file_uuid
                   FROM lar_migration_items WHERE item_id=?1 AND job_id=?2",
                params![item_id, job_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?;
        let Some((state, artifact_kind, existing_exchange, existing_file)) = item else {
            bail!("migration metadata item {item_id} does not belong to job {job_id}");
        };
        if artifact_kind != "exchange_metadata" {
            bail!("migration item {item_id} is not exchange metadata");
        }
        if state == "migrated" {
            if existing_exchange.as_deref() != Some(exchange_id)
                || existing_file.as_deref() != Some(file_uuid)
            {
                bail!("migration metadata item {item_id} is already bound to another exchange");
            }
            tx.commit()?;
            return Ok(false);
        }

        let file_ready: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM lar_files WHERE file_uuid=?1 AND state IN ('active','sealed'))",
            [file_uuid],
            |row| row.get(0),
        )?;
        if !file_ready {
            bail!("metadata exchange destination pack {file_uuid} is not available");
        }
        publish_catalog(&tx)?;
        let changed = tx.execute(
            "UPDATE lar_migration_items
                SET state='migrated', validation_state='validated',
                    destination_exchange_id=?2, destination_file_uuid=?3,
                    metadata_stage_count=?4, metadata_header_count=?5,
                    metadata_unsupported_count=?6, error_kind=NULL,
                    validation_error=NULL, cleanup_eligible=0,
                    updated_at_ms=?7, completed_at_ms=?7
              WHERE item_id=?1 AND job_id=?8 AND state IN ('pending','migrating','failed')",
            params![
                item_id,
                exchange_id,
                file_uuid,
                stage_count,
                header_count,
                unsupported_count,
                now_ms,
                job_id,
            ],
        )?;
        if changed != 1 {
            bail!("migration metadata item {item_id} was not publishable");
        }
        if let Some(session_id) = session_id.filter(|value| !value.is_empty()) {
            tx.execute(
                "INSERT INTO lar_session_revisions (session_id, revision, updated_at_ms)
                 VALUES (?1, 1, ?2)
                 ON CONFLICT(session_id) DO UPDATE SET
                   revision=lar_session_revisions.revision+1,
                   updated_at_ms=excluded.updated_at_ms",
                params![session_id, now_ms],
            )?;
        }
        recount_job(&tx, job_id, now_ms)?;
        tx.commit()?;
        Ok(true)
    }

    /// Resolve a body for mixed-mode reads. Only validated LAR pointers win;
    /// otherwise the original trace/tool path remains available.
    pub fn lar_artifact_location(
        &self,
        owner_kind: &str,
        owner_id: &str,
        artifact_kind: &str,
        stage_id: Option<&str>,
    ) -> Result<Option<LarArtifactLocation>> {
        require_owner_kind(owner_kind)?;
        let conn = self.conn.lock().unwrap();
        let lar = conn
            .query_row(
                "SELECT a.manifest_id, m.total_length, m.hash_algorithm,
                        m.whole_body_hash, a.fidelity
                 FROM lar_trace_artifacts a
                 JOIN lar_manifests m ON m.manifest_id=a.manifest_id
                 WHERE a.owner_kind=?1 AND a.owner_id=?2 AND a.artifact_kind=?3
                   AND a.stage_id=?4 AND a.validation_state='validated' AND m.state='ready'",
                params![owner_kind, owner_id, artifact_kind, stage_id.unwrap_or("")],
                |row| {
                    let length: i64 = row.get(1)?;
                    Ok(LarArtifactLocation::Lar {
                        manifest_id: row.get(0)?,
                        total_length: nonnegative_u64(length, "manifest length")?,
                        hash_algorithm: row.get(2)?,
                        whole_body_hash: row.get(3)?,
                        fidelity: row.get(4)?,
                    })
                },
            )
            .optional()?;
        if lar.is_some() {
            return Ok(lar);
        }

        let migration_source: Option<MigrationSourceRow> = conn
            .query_row(
                "SELECT source_path, state, error_kind, validation_error
                 FROM lar_migration_items
                 WHERE owner_kind=?1 AND owner_id=?2 AND artifact_kind=?3 AND stage_id=?4
                 ORDER BY updated_at_ms DESC LIMIT 1",
                params![owner_kind, owner_id, artifact_kind, stage_id.unwrap_or("")],
                |row| {
                    Ok(MigrationSourceRow {
                        path: row.get(0)?,
                        state: row.get(1)?,
                        error_kind: row.get(2)?,
                        error_detail: row.get(3)?,
                    })
                },
            )
            .optional()?;
        if let Some(source) = migration_source {
            let error = source
                .error_kind
                .zip(source.error_detail)
                .map(|(kind, detail)| LarArtifactError { kind, detail });
            if source.state == "failed"
                && error.as_ref().is_some_and(|value| {
                    matches!(value.kind.as_str(), "missing" | "corrupt" | "unsupported")
                })
            {
                return Ok(Some(LarArtifactLocation::Unavailable {
                    source_path: source.path,
                    error: error.expect("failed source error was checked"),
                }));
            }
            if let Some(path) = source.path {
                return Ok(Some(LarArtifactLocation::Legacy {
                    path,
                    migration_error: error,
                }));
            }
        }

        let legacy_path: Option<String> = match (owner_kind, artifact_kind) {
            ("trace", "client_request") => conn
                .query_row(
                    "SELECT req_body_path FROM traces WHERE id=?1",
                    [owner_id],
                    |row| row.get(0),
                )
                .optional()?
                .flatten(),
            ("trace", "upstream_request") => conn
                .query_row(
                    "SELECT upstream_req_body_path FROM traces WHERE id=?1",
                    [owner_id],
                    |row| row.get(0),
                )
                .optional()?
                .flatten(),
            ("trace", "client_response") => conn
                .query_row(
                    "SELECT resp_body_path FROM traces WHERE id=?1",
                    [owner_id],
                    |row| row.get(0),
                )
                .optional()?
                .flatten(),
            ("tool_call", "tool_arguments") => conn
                .query_row(
                    "SELECT args_body_path FROM tool_calls WHERE id=?1",
                    [owner_id],
                    |row| row.get(0),
                )
                .optional()?
                .flatten(),
            ("tool_call", "tool_result") => conn
                .query_row(
                    "SELECT result_body_path FROM tool_calls WHERE id=?1",
                    [owner_id],
                    |row| row.get(0),
                )
                .optional()?
                .flatten(),
            _ => None,
        };
        Ok(legacy_path.map(|path| LarArtifactLocation::Legacy {
            path,
            migration_error: None,
        }))
    }

    pub fn lar_session_revision(&self, session_id: &str) -> Result<u64> {
        let conn = self.conn.lock().unwrap();
        let revision: Option<i64> = conn
            .query_row(
                "SELECT revision FROM lar_session_revisions WHERE session_id=?1",
                [session_id],
                |row| row.get(0),
            )
            .optional()?;
        Ok(match revision {
            Some(value) => nonnegative_u64(value, "session revision")?,
            None => 0,
        })
    }

    /// Fetch an entire transcript page's ordered transport/routing stages in
    /// one catalog query. This keeps Trace Browser paging O(1) queries rather
    /// than opening archives or issuing one query per turn.
    pub fn lar_stages_for_traces(
        &self,
        trace_ids: &[String],
    ) -> Result<HashMap<String, Vec<Value>>> {
        if trace_ids.is_empty() {
            return Ok(HashMap::new());
        }
        const MAX_TRACE_IDS: usize = 500;
        if trace_ids.len() > MAX_TRACE_IDS {
            bail!(
                "LAR stage lookup requested {} traces, limit is {MAX_TRACE_IDS}",
                trace_ids.len()
            );
        }
        let placeholders = std::iter::repeat("?")
            .take(trace_ids.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT trace_id, stage_id, capture_sequence, kind, attempt_number,
                    wall_time_ns, monotonic_delta_ns, request_headers_ref,
                    request_body_manifest_ref, response_headers_ref,
                    response_body_manifest_ref, trailers_ref, stream_index_ref,
                    fidelity
               FROM lar_stage_records
              WHERE trace_id IN ({placeholders})
              ORDER BY trace_id, capture_sequence"
        );
        let conn = self.conn.lock().unwrap();
        let mut statement = conn.prepare(&sql)?;
        let mut rows = statement.query(rusqlite::params_from_iter(trace_ids.iter()))?;
        let mut by_trace = HashMap::<String, Vec<Value>>::new();
        while let Some(row) = rows.next()? {
            let trace_id = row.get::<_, String>(0)?;
            by_trace.entry(trace_id).or_default().push(json!({
                "stage_id": row.get::<_, String>(1)?,
                "capture_sequence": row.get::<_, u64>(2)?,
                "kind": row.get::<_, String>(3)?,
                "attempt_number": row.get::<_, Option<u64>>(4)?,
                "wall_time_ns": row.get::<_, Option<u64>>(5)?,
                "monotonic_delta_ns": row.get::<_, Option<u64>>(6)?,
                "request_headers_ref": row.get::<_, Option<String>>(7)?,
                "request_body_manifest_ref": row.get::<_, Option<String>>(8)?,
                "response_headers_ref": row.get::<_, Option<String>>(9)?,
                "response_body_manifest_ref": row.get::<_, Option<String>>(10)?,
                "trailers_ref": row.get::<_, Option<String>>(11)?,
                "stream_index_ref": row.get::<_, Option<String>>(12)?,
                "fidelity": row.get::<_, String>(13)?,
            }));
        }
        Ok(by_trace)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use rusqlite::Connection;

    use super::*;

    static TEST_DIR_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    fn tmpdir(name: &str) -> PathBuf {
        let sequence = TEST_DIR_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "alex-lar-catalog-{name}-{}-{sequence}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn migration_job(job_id: &str) -> LarMigrationJobSpec {
        LarMigrationJobSpec {
            job_id: job_id.into(),
            format_version: 1,
            source_version: "legacy-gzip-v1".into(),
            source_key: "alexandria.sqlite3+bodies".into(),
        }
    }

    fn item(item_id: &str, artifact_kind: &str, source_path: &str) -> LarMigrationItem {
        LarMigrationItem {
            item_id: item_id.into(),
            job_id: "job-1".into(),
            owner_kind: "trace".into(),
            owner_id: "trace-1".into(),
            artifact_kind: artifact_kind.into(),
            stage_id: None,
            source_path: Some(source_path.into()),
            source_size: Some(3),
            source_mtime_ms: Some(5),
            source_fingerprint: format!("fingerprint-{artifact_kind}"),
            fidelity: "legacy_exact_body".into(),
        }
    }

    fn insert_legacy_trace(store: &Store, request_path: &str) {
        let conn = store.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO traces (id, ts_request_ms, session_id, req_body_path)
             VALUES ('trace-1', 1, 'session-1', ?1)",
            [request_path],
        )
        .unwrap();
    }

    #[test]
    fn legacy_database_upgrade_is_additive_and_idempotent() {
        let data_dir = tmpdir("schema-upgrade");
        let db_path = data_dir.join("alexandria.sqlite3");
        Connection::open(&db_path)
            .unwrap()
            .execute_batch(crate::SCHEMA)
            .unwrap();

        let store = Store::open(data_dir.clone()).unwrap();
        assert_eq!(store.lar_catalog_schema_version().unwrap(), 3);
        {
            let conn = store.conn.lock().unwrap();
            let lar_tables: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master
                     WHERE type='table' AND name IN
                       ('lar_archive_sets','lar_files','lar_checkpoints','lar_chunks',
                        'lar_manifests','lar_manifest_chunks','lar_header_atoms',
                        'lar_header_blocks','lar_trace_artifacts','lar_stage_records',
                        'lar_migration_jobs','lar_migration_items','lar_gc_runs')",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(lar_tables, 13);
            let legacy_path_columns: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('traces')
                     WHERE name IN ('req_body_path','upstream_req_body_path','resp_body_path')",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(
                legacy_path_columns, 3,
                "legacy fallback columns must remain"
            );
        }
        drop(store);

        let reopened = Store::open(data_dir).unwrap();
        assert_eq!(reopened.lar_catalog_schema_version().unwrap(), 3);
        let conn = reopened.conn.lock().unwrap();
        let version_rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM lar_schema_versions", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(version_rows, 1, "startup migration must be repeatable");
    }

    #[test]
    fn v2_manifest_identity_upgrade_preserves_rows_and_allows_metadata_variants() {
        let data_dir = tmpdir("manifest-v2-upgrade");
        let db_path = data_dir.join("alexandria.sqlite3");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(crate::SCHEMA).unwrap();
        conn.execute_batch(
            "CREATE TABLE lar_manifests (
               manifest_id TEXT PRIMARY KEY,
               total_length INTEGER NOT NULL CHECK (total_length >= 0),
               hash_algorithm TEXT NOT NULL,
               whole_body_hash BLOB NOT NULL,
               media_type TEXT,
               content_encoding TEXT,
               file_uuid TEXT,
               record_id TEXT,
               created_at_ms INTEGER NOT NULL,
               state TEXT NOT NULL DEFAULT 'ready'
                 CHECK (state IN ('ready', 'quarantined', 'unreachable')),
               UNIQUE (hash_algorithm, whole_body_hash, total_length),
               UNIQUE (file_uuid, record_id)
             );",
        )
        .unwrap();
        let digest = vec![7u8; 32];
        conn.execute(
            "INSERT INTO lar_manifests
               (manifest_id, total_length, hash_algorithm, whole_body_hash,
                media_type, content_encoding, created_at_ms, state)
             VALUES ('manifest-v2', 4, 'blake3', ?1, NULL, NULL, 1, 'ready')",
            [&digest],
        )
        .unwrap();
        drop(conn);

        let store = Store::open(data_dir).unwrap();
        assert_eq!(store.lar_catalog_schema_version().unwrap(), 3);
        let conn = store.conn.lock().unwrap();
        let preserved: String = conn
            .query_row(
                "SELECT manifest_id FROM lar_manifests WHERE whole_body_hash=?1",
                [&digest],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(preserved, "manifest-v2");
        conn.execute(
            "INSERT INTO lar_manifests
               (manifest_id, total_length, hash_algorithm, whole_body_hash,
                media_type, content_encoding, created_at_ms, state)
             VALUES ('manifest-v3', 4, 'blake3', ?1,
                     'application/json', NULL, 2, 'ready')",
            [&digest],
        )
        .unwrap();
        let variants: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM lar_manifests WHERE whole_body_hash=?1",
                [&digest],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(variants, 2);
    }

    #[test]
    fn pointer_switch_waits_for_readback_validation_and_is_idempotent() {
        let store = Store::open(tmpdir("validated-switch")).unwrap();
        let legacy_path = "/data/bodies/2026-07-20/trace-1.request.json.gz";
        insert_legacy_trace(&store, legacy_path);

        let first = store
            .ensure_lar_migration_job(&migration_job("job-1"), 10)
            .unwrap();
        let repeated = store
            .ensure_lar_migration_job(&migration_job("different-candidate-id"), 11)
            .unwrap();
        assert_eq!(first.job_id, "job-1");
        assert_eq!(repeated.job_id, "job-1");
        assert!(store
            .claim_lar_migration_job("job-1", "worker-a", 12, Duration::from_secs(30))
            .unwrap());

        store
            .discover_lar_migration_item(&item("item-1", "client_request", legacy_path), 13)
            .unwrap();
        store
            .discover_lar_migration_item(
                &item("duplicate-item-id", "client_request", legacy_path),
                14,
            )
            .unwrap();
        assert_eq!(
            store
                .lar_migration_job("job-1")
                .unwrap()
                .unwrap()
                .discovered_count,
            1
        );
        assert_eq!(
            store
                .lar_artifact_location("trace", "trace-1", "client_request", None)
                .unwrap(),
            Some(LarArtifactLocation::Legacy {
                path: legacy_path.into(),
                migration_error: None,
            })
        );

        let source_hash = vec![1, 2, 3];
        store
            .register_lar_manifest(
                &LarManifestRegistration {
                    manifest_id: "manifest-1".into(),
                    total_length: 3,
                    hash_algorithm: "blake3".into(),
                    whole_body_hash: source_hash.clone(),
                    media_type: Some("application/json".into()),
                    content_encoding: None,
                    file_uuid: None,
                    record_id: None,
                },
                15,
            )
            .unwrap();

        let failed = store
            .switch_validated_lar_artifact(
                "job-1",
                "item-1",
                "worker-a",
                "manifest-1",
                &LarValidation {
                    source_length: 3,
                    source_hash_algorithm: "blake3".into(),
                    source_hash: source_hash.clone(),
                    reconstructed_length: 3,
                    reconstructed_hash: vec![9, 9, 9],
                    bytes_read: 3,
                    unique_bytes_written: 3,
                    bytes_deduplicated: 0,
                },
                Some("session-1"),
                16,
            )
            .unwrap();
        assert!(matches!(failed, LarPointerSwitch::ValidationFailed { .. }));
        assert!(matches!(
            store
                .lar_artifact_location("trace", "trace-1", "client_request", None)
                .unwrap(),
            Some(LarArtifactLocation::Legacy {
                migration_error: Some(LarArtifactError { ref kind, .. }),
                ..
            }) if kind == "validation"
        ));
        assert_eq!(store.lar_session_revision("session-1").unwrap(), 0);

        let valid = LarValidation {
            source_length: 3,
            source_hash_algorithm: "blake3".into(),
            source_hash: source_hash.clone(),
            reconstructed_length: 3,
            reconstructed_hash: source_hash.clone(),
            bytes_read: 3,
            unique_bytes_written: 3,
            bytes_deduplicated: 0,
        };
        assert_eq!(
            store
                .switch_validated_lar_artifact(
                    "job-1",
                    "item-1",
                    "worker-a",
                    "manifest-1",
                    &valid,
                    Some("session-1"),
                    17,
                )
                .unwrap(),
            LarPointerSwitch::Switched {
                manifest_id: "manifest-1".into()
            }
        );
        assert!(matches!(
            store
                .lar_artifact_location("trace", "trace-1", "client_request", None)
                .unwrap(),
            Some(LarArtifactLocation::Lar { ref manifest_id, .. }) if manifest_id == "manifest-1"
        ));
        assert_eq!(store.lar_session_revision("session-1").unwrap(), 1);

        assert!(matches!(
            store
                .switch_validated_lar_artifact(
                    "job-1",
                    "item-1",
                    "worker-a",
                    "manifest-1",
                    &valid,
                    Some("session-1"),
                    18,
                )
                .unwrap(),
            LarPointerSwitch::AlreadySwitched { .. }
        ));
        assert_eq!(store.lar_session_revision("session-1").unwrap(), 1);
        let job = store.lar_migration_job("job-1").unwrap().unwrap();
        assert_eq!(
            (job.discovered_count, job.pending_count, job.migrated_count),
            (1, 0, 1)
        );
        store
            .checkpoint_lar_migration_job("job-1", "worker-a", "trace-1:client_request", 19)
            .unwrap();
        store
            .complete_lar_migration_job("job-1", "worker-a", 20)
            .unwrap();
        let completed = store.list_lar_migration_jobs().unwrap();
        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0].state, "complete");
        assert_eq!(
            completed[0].last_committed_cursor.as_deref(),
            Some("trace-1:client_request")
        );

        let conn = store.conn.lock().unwrap();
        let legacy_path_after: String = conn
            .query_row(
                "SELECT req_body_path FROM traces WHERE id='trace-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            legacy_path_after, legacy_path,
            "pointer switch must preserve rollback data"
        );
    }

    #[test]
    fn generic_artifacts_keep_provenance_and_explicit_source_errors() {
        let store = Store::open(tmpdir("generic-source-error")).unwrap();
        insert_legacy_trace(&store, "/unused/request.gz");
        store
            .ensure_lar_migration_job(&migration_job("job-1"), 10)
            .unwrap();
        assert!(store
            .claim_lar_migration_job("job-1", "worker-a", 11, Duration::from_secs(30))
            .unwrap());
        let dario_path = "/data/bodies/2026-07-20/trace-1.dario-upstream-request.json.gz";
        store
            .discover_lar_migration_item(
                &item("item-dario", "dario_upstream_request", dario_path),
                12,
            )
            .unwrap();
        store
            .record_lar_migration_item_failure(
                "job-1",
                "item-dario",
                "worker-a",
                "missing",
                "legacy Dario capture disappeared before import",
                13,
            )
            .unwrap();

        assert_eq!(
            store
                .lar_artifact_location("trace", "trace-1", "dario_upstream_request", None)
                .unwrap(),
            Some(LarArtifactLocation::Unavailable {
                source_path: Some(dario_path.into()),
                error: LarArtifactError {
                    kind: "missing".into(),
                    detail: "legacy Dario capture disappeared before import".into(),
                },
            })
        );
        let job = store.lar_migration_job("job-1").unwrap().unwrap();
        assert_eq!((job.discovered_count, job.failed_count), (1, 1));
    }

    #[test]
    fn migration_lease_is_exclusive_but_stale_owners_can_be_recovered() {
        let store = Store::open(tmpdir("lease-recovery")).unwrap();
        store
            .ensure_lar_migration_job(&migration_job("job-1"), 1)
            .unwrap();
        assert!(store
            .claim_lar_migration_job("job-1", "worker-a", 10, Duration::from_millis(10))
            .unwrap());
        store
            .checkpoint_lar_migration_job("job-1", "worker-a", "cursor-a", 11)
            .unwrap();
        assert!(store.set_lar_migration_paused("job-1", true, 12).unwrap());
        assert!(!store
            .claim_lar_migration_job("job-1", "worker-b", 13, Duration::from_millis(10))
            .unwrap());
        assert!(store.set_lar_migration_paused("job-1", false, 14).unwrap());
        assert!(store
            .claim_lar_migration_job("job-1", "worker-b", 15, Duration::from_millis(10))
            .unwrap());
        assert!(!store
            .claim_lar_migration_job("job-1", "worker-a", 24, Duration::from_millis(10))
            .unwrap());
        assert!(store
            .claim_lar_migration_job("job-1", "worker-a", 25, Duration::from_millis(10))
            .unwrap());
    }
}
