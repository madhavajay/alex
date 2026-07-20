//! Rebuildable provider-neutral text search over LAR-backed artifacts.
//!
//! Raw request/response/tool bytes remain authoritative. This module stores
//! only bounded normalized text and reverse references in SQLite. The entire
//! index may be deleted and deterministically rebuilt from catalog artifacts.

use std::collections::HashSet;

use anyhow::{bail, Result};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde_json::Value;

use crate::{
    LarArtifactBatchRead, LarArtifactReadRequest, LarBodyArtifact, LarBodyOwnerKind, Store,
};

pub const LAR_NORMALIZED_INDEX_SCHEMA_VERSION: i64 = 1;
const EXTRACTOR_VERSION: &str = "provider-neutral-json-v1";
const MAX_JSON_DEPTH: usize = 64;
const MAX_JSON_NODES: usize = 100_000;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS lar_normalized_index_meta (
  singleton         INTEGER PRIMARY KEY CHECK (singleton = 1),
  schema_version    INTEGER NOT NULL,
  extractor_version TEXT NOT NULL,
  state             TEXT NOT NULL CHECK (state IN ('ready','rebuilding','needs_rebuild')),
  updated_at_ms     INTEGER NOT NULL,
  last_error        TEXT
);

CREATE TABLE IF NOT EXISTS lar_normalized_entries (
  entry_id          TEXT PRIMARY KEY,
  schema_version    INTEGER NOT NULL,
  entry_kind        TEXT NOT NULL,
  normalized_text   TEXT NOT NULL,
  text_bytes        INTEGER NOT NULL CHECK (text_bytes >= 0),
  content_hash      BLOB NOT NULL,
  created_at_ms     INTEGER NOT NULL,
  UNIQUE (schema_version, entry_kind, content_hash)
);

CREATE VIRTUAL TABLE IF NOT EXISTS lar_normalized_entries_fts USING fts5(
  entry_id UNINDEXED,
  entry_kind UNINDEXED,
  normalized_text,
  tokenize = 'unicode61 remove_diacritics 2'
);

CREATE TABLE IF NOT EXISTS lar_normalized_entry_refs (
  entry_id          TEXT NOT NULL,
  trace_id          TEXT,
  session_id        TEXT,
  ts_request_ms     INTEGER,
  owner_kind        TEXT NOT NULL CHECK (owner_kind IN ('trace','tool_call')),
  owner_id          TEXT NOT NULL,
  artifact_kind     TEXT NOT NULL,
  stage_id          TEXT NOT NULL DEFAULT '',
  manifest_id       TEXT NOT NULL DEFAULT '',
  created_at_ms     INTEGER NOT NULL,
  PRIMARY KEY (entry_id, owner_kind, owner_id, artifact_kind, stage_id),
  FOREIGN KEY (entry_id) REFERENCES lar_normalized_entries(entry_id)
);
CREATE INDEX IF NOT EXISTS lar_normalized_refs_trace
  ON lar_normalized_entry_refs(trace_id, ts_request_ms, entry_id);
CREATE INDEX IF NOT EXISTS lar_normalized_refs_session
  ON lar_normalized_entry_refs(session_id, ts_request_ms, trace_id);
CREATE INDEX IF NOT EXISTS lar_normalized_refs_stage
  ON lar_normalized_entry_refs(stage_id, trace_id);
CREATE INDEX IF NOT EXISTS lar_normalized_refs_manifest
  ON lar_normalized_entry_refs(manifest_id, trace_id);

-- Records successful extraction even when an artifact contains no searchable
-- semantic text. The proxy uses this to avoid reopening gzip compatibility
-- bodies for traces already covered by the derived index.
CREATE TABLE IF NOT EXISTS lar_normalized_artifact_state (
  owner_kind        TEXT NOT NULL CHECK (owner_kind IN ('trace','tool_call')),
  owner_id          TEXT NOT NULL,
  artifact_kind     TEXT NOT NULL,
  stage_id          TEXT NOT NULL DEFAULT '',
  manifest_id       TEXT NOT NULL DEFAULT '',
  schema_version    INTEGER NOT NULL,
  status            TEXT NOT NULL CHECK (status IN ('indexed','no_text','skipped_limit')),
  indexed_at_ms     INTEGER NOT NULL,
  PRIMARY KEY (owner_kind, owner_id, artifact_kind, stage_id)
);
CREATE INDEX IF NOT EXISTS lar_normalized_artifact_state_owner
  ON lar_normalized_artifact_state(owner_kind, owner_id, artifact_kind, status);
"#;

#[derive(Clone, Debug)]
pub struct LarFtsRebuildOptions {
    pub max_artifacts: usize,
    pub max_body_bytes: u64,
    pub max_total_body_bytes: u64,
    pub max_entries_per_artifact: usize,
    pub max_entry_chars: usize,
    pub max_total_chars_per_artifact: usize,
}

impl Default for LarFtsRebuildOptions {
    fn default() -> Self {
        Self {
            max_artifacts: 100_000,
            max_body_bytes: 4 * 1024 * 1024,
            max_total_body_bytes: 1024 * 1024 * 1024,
            max_entries_per_artifact: 4096,
            max_entry_chars: 64 * 1024,
            max_total_chars_per_artifact: 1024 * 1024,
        }
    }
}

impl LarFtsRebuildOptions {
    fn validate(&self) -> Result<()> {
        if self.max_artifacts == 0
            || self.max_body_bytes == 0
            || self.max_total_body_bytes == 0
            || self.max_entries_per_artifact == 0
            || self.max_entry_chars == 0
            || self.max_total_chars_per_artifact == 0
        {
            bail!("normalized index rebuild bounds must all be positive");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Serialize)]
pub struct LarFtsRebuildReport {
    pub schema_version: i64,
    pub artifacts_seen: u64,
    pub artifacts_indexed: u64,
    pub artifacts_skipped: u64,
    pub body_bytes_read: u64,
    pub entries: u64,
    pub reverse_references: u64,
    pub limit_reached: bool,
}

#[derive(Clone)]
struct Candidate {
    owner_kind: String,
    owner_id: String,
    artifact_kind: String,
    stage_id: String,
    manifest_id: String,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct NormalizedEntry {
    kind: String,
    text: String,
}

#[derive(Clone, Copy)]
struct ExtractionLimits {
    max_entries: usize,
    max_entry_chars: usize,
    max_total_chars: usize,
}

pub(crate) fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch(SCHEMA)?;
    let current: Option<(i64, String)> = conn
        .query_row(
            "SELECT schema_version, extractor_version
             FROM lar_normalized_index_meta WHERE singleton=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let now = chrono::Utc::now().timestamp_millis();
    match current {
        None => {
            let has_existing_artifacts: bool = conn.query_row(
                "SELECT EXISTS(
                   SELECT 1 FROM lar_trace_artifacts
                   WHERE validation_state='validated' AND manifest_id IS NOT NULL
                 )",
                [],
                |row| row.get(0),
            )?;
            conn.execute(
                "INSERT INTO lar_normalized_index_meta
                   (singleton, schema_version, extractor_version, state, updated_at_ms)
                 VALUES (1, ?1, ?2, ?3, ?4)",
                params![
                    LAR_NORMALIZED_INDEX_SCHEMA_VERSION,
                    EXTRACTOR_VERSION,
                    if has_existing_artifacts {
                        "needs_rebuild"
                    } else {
                        "ready"
                    },
                    now
                ],
            )?;
        }
        Some((version, extractor))
            if version != LAR_NORMALIZED_INDEX_SCHEMA_VERSION || extractor != EXTRACTOR_VERSION =>
        {
            clear_index(conn)?;
            conn.execute(
                "UPDATE lar_normalized_index_meta SET schema_version=?1,
                   extractor_version=?2, state='needs_rebuild', updated_at_ms=?3,
                   last_error=NULL WHERE singleton=1",
                params![LAR_NORMALIZED_INDEX_SCHEMA_VERSION, EXTRACTOR_VERSION, now],
            )?;
        }
        Some(_) => {}
    }
    Ok(())
}

pub(crate) fn fts_match_query(query: &str) -> Option<String> {
    let terms = query
        .split_whitespace()
        .filter_map(|term| {
            let term = term.trim_matches(|character: char| character.is_ascii_punctuation());
            (!term.is_empty()).then_some(term)
        })
        .take(16)
        .map(|term| {
            let term = term.chars().take(128).collect::<String>();
            format!("\"{}\"*", term.replace('"', "\"\""))
        })
        .collect::<Vec<_>>();
    (!terms.is_empty()).then(|| terms.join(" AND "))
}

pub(crate) fn index_artifact_bytes(
    conn: &Connection,
    artifact: &LarBodyArtifact,
    manifest_id: &str,
    bytes: &[u8],
    created_at_ms: i64,
) -> Result<(u64, u64)> {
    let defaults = LarFtsRebuildOptions::default();
    let owner_kind = match artifact.owner_kind {
        LarBodyOwnerKind::Trace => "trace",
        LarBodyOwnerKind::ToolCall => "tool_call",
    };
    let candidate = Candidate {
        owner_kind: owner_kind.into(),
        owner_id: artifact.owner_id.clone(),
        artifact_kind: artifact.artifact_kind.clone(),
        stage_id: artifact.stage_id.clone().unwrap_or_default(),
        manifest_id: manifest_id.into(),
    };
    if bytes.len() as u64 > defaults.max_body_bytes {
        reset_artifact_derived(conn, &candidate)?;
        record_artifact_state(conn, &candidate, "skipped_limit", created_at_ms)?;
        return Ok((0, 0));
    }
    index_bytes(
        conn,
        &candidate,
        bytes,
        ExtractionLimits {
            max_entries: defaults.max_entries_per_artifact,
            max_entry_chars: defaults.max_entry_chars,
            max_total_chars: defaults.max_total_chars_per_artifact,
        },
        created_at_ms,
    )
}

pub(crate) fn refresh_trace_anchor(
    conn: &Connection,
    trace_id: &str,
    session_id: Option<&str>,
    ts_request_ms: i64,
) -> Result<()> {
    conn.execute(
        "UPDATE lar_normalized_entry_refs SET trace_id=?1, session_id=?2,
           ts_request_ms=?3 WHERE owner_kind='trace' AND owner_id=?1",
        params![trace_id, session_id, ts_request_ms],
    )?;
    conn.execute(
        "UPDATE lar_normalized_entry_refs SET session_id=COALESCE(session_id, ?2),
           ts_request_ms=COALESCE(ts_request_ms, ?3)
         WHERE trace_id=?1",
        params![trace_id, session_id, ts_request_ms],
    )?;
    Ok(())
}

pub(crate) fn refresh_tool_anchor(conn: &Connection, tool_id: &str) -> Result<()> {
    conn.execute(
        "UPDATE lar_normalized_entry_refs SET
           trace_id=(SELECT trace_id FROM tool_calls WHERE id=?1),
           session_id=(SELECT session_id FROM tool_calls WHERE id=?1),
           ts_request_ms=COALESCE(
             (SELECT t.ts_request_ms FROM tool_calls c JOIN traces t ON t.id=c.trace_id
              WHERE c.id=?1),
             (SELECT ts_start_ms FROM tool_calls WHERE id=?1))
         WHERE owner_kind='tool_call' AND owner_id=?1",
        [tool_id],
    )?;
    Ok(())
}

pub(crate) fn attach_stage_manifest_refs(
    conn: &Connection,
    trace_id: &str,
    stage_id: &str,
    manifest_id: &str,
    created_at_ms: i64,
) -> Result<u64> {
    if manifest_id.is_empty() {
        return Ok(0);
    }
    Ok(conn.execute(
        "INSERT INTO lar_normalized_entry_refs
           (entry_id, trace_id, session_id, ts_request_ms, owner_kind, owner_id,
            artifact_kind, stage_id, manifest_id, created_at_ms)
         SELECT entry_id, trace_id, session_id, ts_request_ms, owner_kind, owner_id,
                artifact_kind, ?2, manifest_id, ?4
         FROM lar_normalized_entry_refs
         WHERE trace_id=?1 AND manifest_id=?3 AND stage_id=''
         ON CONFLICT(entry_id, owner_kind, owner_id, artifact_kind, stage_id)
         DO NOTHING",
        params![trace_id, stage_id, manifest_id, created_at_ms],
    )? as u64)
}

pub(crate) fn delete_trace_references(conn: &Connection, trace_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM lar_normalized_entry_refs
         WHERE trace_id=?1 OR (owner_kind='trace' AND owner_id=?1)",
        [trace_id],
    )?;
    conn.execute(
        "DELETE FROM lar_normalized_artifact_state
         WHERE owner_kind='trace' AND owner_id=?1",
        [trace_id],
    )?;
    delete_orphan_entries(conn)
}

pub(crate) fn clear_all_references(conn: &Connection) -> Result<()> {
    clear_index(conn)
}

pub(crate) fn prune_references(conn: &Connection, older_than_ms: i64) -> Result<()> {
    conn.execute(
        "DELETE FROM lar_normalized_entry_refs
         WHERE trace_id IN (SELECT id FROM traces WHERE ts_request_ms < ?1)
            OR (owner_kind='trace' AND owner_id IN
                  (SELECT id FROM traces WHERE ts_request_ms < ?1))
            OR (owner_kind='tool_call' AND owner_id IN
                  (SELECT id FROM tool_calls WHERE ts_start_ms < ?1))",
        [older_than_ms],
    )?;
    conn.execute(
        "DELETE FROM lar_normalized_artifact_state
         WHERE (owner_kind='trace' AND owner_id IN
                  (SELECT id FROM traces WHERE ts_request_ms < ?1))
            OR (owner_kind='tool_call' AND owner_id IN
                  (SELECT id FROM tool_calls WHERE ts_start_ms < ?1))",
        [older_than_ms],
    )?;
    delete_orphan_entries(conn)
}

impl Store {
    /// Return trace/artifact slots completely covered by the normalized index.
    /// Callers can use this bounded batch result to avoid compatibility gzip
    /// reads while still scanning genuinely legacy or skipped artifacts.
    pub fn lar_normalized_indexed_artifacts(
        &self,
        trace_ids: &[String],
    ) -> Result<HashSet<(String, String)>> {
        if trace_ids.is_empty() {
            return Ok(HashSet::new());
        }
        let placeholders = std::iter::repeat("?")
            .take(trace_ids.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT owner_id, artifact_kind FROM lar_normalized_artifact_state
             WHERE owner_kind='trace' AND status IN ('indexed','no_text')
               AND artifact_kind IN ('client_request','client_response')
               AND owner_id IN ({placeholders})"
        );
        let conn = self.conn.lock().unwrap();
        let mut statement = conn.prepare(&sql)?;
        let rows = statement
            .query_map(rusqlite::params_from_iter(trace_ids.iter()), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<HashSet<_>>>()?;
        Ok(rows)
    }

    pub fn clear_lar_normalized_index(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        clear_index(&conn)?;
        conn.execute(
            "UPDATE lar_normalized_index_meta SET state='needs_rebuild',
               updated_at_ms=?1, last_error=NULL WHERE singleton=1",
            [chrono::Utc::now().timestamp_millis()],
        )?;
        Ok(())
    }

    pub fn rebuild_lar_normalized_index(
        &self,
        options: &LarFtsRebuildOptions,
    ) -> Result<LarFtsRebuildReport> {
        options.validate()?;
        {
            let mut conn = self.conn.lock().unwrap();
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            clear_index(&tx)?;
            tx.execute(
                "UPDATE lar_normalized_index_meta SET state='rebuilding',
                   updated_at_ms=?1, last_error=NULL WHERE singleton=1",
                [chrono::Utc::now().timestamp_millis()],
            )?;
            tx.commit()?;
        }
        let result = (|| -> Result<LarFtsRebuildReport> {
            let candidates = {
                let conn = self.conn.lock().unwrap();
                let mut statement = conn.prepare(
                    "SELECT owner_kind, owner_id, artifact_kind, stage_id,
                        COALESCE(manifest_id, '')
                 FROM lar_trace_artifacts
                 WHERE validation_state='validated' AND artifact_kind IN
                   ('client_request','upstream_request','client_response','upstream_response',
                    'tool_arguments','tool_result','dario_upstream_request',
                    'dario_upstream_response')
                 ORDER BY owner_kind, owner_id, artifact_kind, stage_id
                 LIMIT ?1",
                )?;
                let candidates = statement
                    .query_map(
                        [i64::try_from(options.max_artifacts.saturating_add(1))?],
                        |row| {
                            Ok(Candidate {
                                owner_kind: row.get(0)?,
                                owner_id: row.get(1)?,
                                artifact_kind: row.get(2)?,
                                stage_id: row.get(3)?,
                                manifest_id: row.get(4)?,
                            })
                        },
                    )?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                candidates
            };
            let mut report = LarFtsRebuildReport {
                schema_version: LAR_NORMALIZED_INDEX_SCHEMA_VERSION,
                limit_reached: candidates.len() > options.max_artifacts,
                ..Default::default()
            };
            let limits = ExtractionLimits {
                max_entries: options.max_entries_per_artifact,
                max_entry_chars: options.max_entry_chars,
                max_total_chars: options.max_total_chars_per_artifact,
            };
            for candidate in candidates.into_iter().take(options.max_artifacts) {
                report.artifacts_seen += 1;
                let request = LarArtifactReadRequest {
                    owner_kind: candidate.owner_kind.clone(),
                    owner_id: candidate.owner_id.clone(),
                    artifact_kind: candidate.artifact_kind.clone(),
                    stage_id: (!candidate.stage_id.is_empty()).then(|| candidate.stage_id.clone()),
                };
                let outcome = self
                    .read_lar_or_legacy_artifact_batch_bounded(&[request], options.max_body_bytes)
                    .into_iter()
                    .next()
                    .unwrap_or(LarArtifactBatchRead::Missing);
                let LarArtifactBatchRead::Read(bytes) = outcome else {
                    report.artifacts_skipped += 1;
                    continue;
                };
                let body_length = bytes.len() as u64;
                if report.body_bytes_read.saturating_add(body_length) > options.max_total_body_bytes
                {
                    report.limit_reached = true;
                    break;
                }
                report.body_bytes_read += body_length;
                let (entries, refs) = {
                    let mut conn = self.conn.lock().unwrap();
                    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
                    let counts = index_bytes(
                        &tx,
                        &candidate,
                        &bytes,
                        limits,
                        chrono::Utc::now().timestamp_millis(),
                    )?;
                    tx.commit()?;
                    counts
                };
                report.artifacts_indexed += 1;
                report.entries += entries;
                report.reverse_references += refs;
            }
            {
                let conn = self.conn.lock().unwrap();
                attach_all_stage_references(&conn)?;
                let (entries, refs): (i64, i64) = conn.query_row(
                    "SELECT (SELECT COUNT(*) FROM lar_normalized_entries),
                        (SELECT COUNT(*) FROM lar_normalized_entry_refs)",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )?;
                report.entries = entries.max(0) as u64;
                report.reverse_references = refs.max(0) as u64;
                let state = if report.limit_reached || report.artifacts_skipped > 0 {
                    "needs_rebuild"
                } else {
                    "ready"
                };
                conn.execute(
                    "UPDATE lar_normalized_index_meta SET state=?1, updated_at_ms=?2,
                   last_error=NULL WHERE singleton=1",
                    params![state, chrono::Utc::now().timestamp_millis()],
                )?;
            }
            Ok(report)
        })();
        if let Err(error) = &result {
            let detail = format!("{error:#}").chars().take(2048).collect::<String>();
            let conn = self.conn.lock().unwrap();
            // Preserve the original rebuild failure if even this best-effort
            // status update cannot be written.
            let _ = conn.execute(
                "UPDATE lar_normalized_index_meta SET state='needs_rebuild',
                   updated_at_ms=?1, last_error=?2 WHERE singleton=1",
                params![chrono::Utc::now().timestamp_millis(), detail],
            );
        }
        result
    }
}

fn index_bytes(
    conn: &Connection,
    candidate: &Candidate,
    bytes: &[u8],
    limits: ExtractionLimits,
    created_at_ms: i64,
) -> Result<(u64, u64)> {
    reset_artifact_derived(conn, candidate)?;
    let entries = extract_entries(&candidate.artifact_kind, bytes, limits);
    record_artifact_state(
        conn,
        candidate,
        if entries.is_empty() {
            "no_text"
        } else {
            "indexed"
        },
        created_at_ms,
    )?;
    let (trace_id, session_id, ts_request_ms) = resolve_anchor(conn, candidate)?;
    let mut new_entries = 0u64;
    let mut new_refs = 0u64;
    for entry in entries {
        let entry_id = entry_id(&entry);
        let digest = blake3::hash(entry.text.as_bytes());
        let entry_kind = &entry.kind;
        let normalized_text = &entry.text;
        let inserted = conn.execute(
            "INSERT INTO lar_normalized_entries
               (entry_id, schema_version, entry_kind, normalized_text, text_bytes,
                content_hash, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(entry_id) DO NOTHING",
            params![
                entry_id,
                LAR_NORMALIZED_INDEX_SCHEMA_VERSION,
                entry_kind,
                normalized_text,
                normalized_text.len() as u64,
                digest.as_bytes().as_slice(),
                created_at_ms,
            ],
        )?;
        if inserted > 0 {
            conn.execute(
                "INSERT INTO lar_normalized_entries_fts
                   (entry_id, entry_kind, normalized_text) VALUES (?1, ?2, ?3)",
                params![entry_id, entry_kind, normalized_text],
            )?;
            new_entries += 1;
        }
        new_refs += conn.execute(
            "INSERT INTO lar_normalized_entry_refs
               (entry_id, trace_id, session_id, ts_request_ms, owner_kind, owner_id,
                artifact_kind, stage_id, manifest_id, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(entry_id, owner_kind, owner_id, artifact_kind, stage_id)
             DO UPDATE SET trace_id=excluded.trace_id, session_id=excluded.session_id,
               ts_request_ms=excluded.ts_request_ms, manifest_id=excluded.manifest_id",
            params![
                entry_id,
                trace_id,
                session_id,
                ts_request_ms,
                candidate.owner_kind,
                candidate.owner_id,
                candidate.artifact_kind,
                candidate.stage_id,
                candidate.manifest_id,
                created_at_ms,
            ],
        )? as u64;
    }
    if let Some(trace_id) = trace_id.as_deref() {
        attach_matching_stages(conn, trace_id, &candidate.manifest_id, created_at_ms)?;
    }
    Ok((new_entries, new_refs))
}

fn reset_artifact_derived(conn: &Connection, candidate: &Candidate) -> Result<()> {
    // One owner/artifact slot points at one authoritative body. Remove all of
    // its old derived references before indexing a replacement; stage refs are
    // reattached below from the authoritative stage catalog.
    conn.execute(
        "DELETE FROM lar_normalized_entry_refs
         WHERE owner_kind=?1 AND owner_id=?2 AND artifact_kind=?3",
        params![
            candidate.owner_kind,
            candidate.owner_id,
            candidate.artifact_kind,
        ],
    )?;
    conn.execute(
        "DELETE FROM lar_normalized_artifact_state
         WHERE owner_kind=?1 AND owner_id=?2 AND artifact_kind=?3",
        params![
            candidate.owner_kind,
            candidate.owner_id,
            candidate.artifact_kind,
        ],
    )?;
    delete_orphan_entries(conn)
}

fn record_artifact_state(
    conn: &Connection,
    candidate: &Candidate,
    status: &str,
    indexed_at_ms: i64,
) -> Result<()> {
    conn.execute(
        "INSERT INTO lar_normalized_artifact_state
           (owner_kind, owner_id, artifact_kind, stage_id, manifest_id,
            schema_version, status, indexed_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            candidate.owner_kind,
            candidate.owner_id,
            candidate.artifact_kind,
            candidate.stage_id,
            candidate.manifest_id,
            LAR_NORMALIZED_INDEX_SCHEMA_VERSION,
            status,
            indexed_at_ms,
        ],
    )?;
    Ok(())
}

fn resolve_anchor(
    conn: &Connection,
    candidate: &Candidate,
) -> Result<(Option<String>, Option<String>, Option<i64>)> {
    match candidate.owner_kind.as_str() {
        "trace" => {
            let anchor: Option<(Option<String>, i64)> = conn
                .query_row(
                    "SELECT session_id, ts_request_ms FROM traces WHERE id=?1",
                    [&candidate.owner_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            Ok(match anchor {
                Some((session, timestamp)) => {
                    (Some(candidate.owner_id.clone()), session, Some(timestamp))
                }
                None => (Some(candidate.owner_id.clone()), None, None),
            })
        }
        "tool_call" => Ok(conn
            .query_row(
                "SELECT trace_id, session_id,
                        COALESCE((SELECT ts_request_ms FROM traces WHERE id=tool_calls.trace_id),
                                 ts_start_ms)
                 FROM tool_calls WHERE id=?1",
                [&candidate.owner_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?
            .unwrap_or((None, None, None))),
        other => bail!("unsupported normalized index owner kind {other}"),
    }
}

fn attach_matching_stages(
    conn: &Connection,
    trace_id: &str,
    manifest_id: &str,
    created_at_ms: i64,
) -> Result<()> {
    if manifest_id.is_empty() {
        return Ok(());
    }
    let mut statement = conn.prepare(
        "SELECT stage_id FROM lar_stage_records WHERE trace_id=?1 AND
           (request_body_manifest_ref=?2 OR response_body_manifest_ref=?2)",
    )?;
    let stages = statement
        .query_map(params![trace_id, manifest_id], |row| {
            row.get::<_, String>(0)
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(statement);
    for stage in stages {
        attach_stage_manifest_refs(conn, trace_id, &stage, manifest_id, created_at_ms)?;
    }
    Ok(())
}

fn attach_all_stage_references(conn: &Connection) -> Result<()> {
    let stages = conn
        .prepare(
            "SELECT trace_id, stage_id, request_body_manifest_ref,
                    response_body_manifest_ref FROM lar_stage_records",
        )?
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let now = chrono::Utc::now().timestamp_millis();
    for (trace, stage, request, response) in stages {
        for manifest in [request, response].into_iter().flatten() {
            attach_stage_manifest_refs(conn, &trace, &stage, &manifest, now)?;
        }
    }
    Ok(())
}

fn delete_orphan_entries(conn: &Connection) -> Result<()> {
    conn.execute(
        "DELETE FROM lar_normalized_entries_fts WHERE entry_id IN
           (SELECT e.entry_id FROM lar_normalized_entries e
            WHERE NOT EXISTS (SELECT 1 FROM lar_normalized_entry_refs r
                              WHERE r.entry_id=e.entry_id))",
        [],
    )?;
    conn.execute(
        "DELETE FROM lar_normalized_entries WHERE NOT EXISTS
           (SELECT 1 FROM lar_normalized_entry_refs r
            WHERE r.entry_id=lar_normalized_entries.entry_id)",
        [],
    )?;
    Ok(())
}

fn clear_index(conn: &Connection) -> Result<()> {
    conn.execute("DELETE FROM lar_normalized_entry_refs", [])?;
    conn.execute("DELETE FROM lar_normalized_entries_fts", [])?;
    conn.execute("DELETE FROM lar_normalized_entries", [])?;
    conn.execute("DELETE FROM lar_normalized_artifact_state", [])?;
    Ok(())
}

fn entry_id(entry: &NormalizedEntry) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"lar-normalized-entry-v1\0");
    hasher.update(entry.kind.as_bytes());
    hasher.update(b"\0");
    hasher.update(entry.text.as_bytes());
    hasher.finalize().to_hex().to_string()
}

fn extract_entries(
    artifact_kind: &str,
    bytes: &[u8],
    limits: ExtractionLimits,
) -> Vec<NormalizedEntry> {
    if !supported_artifact(artifact_kind) {
        return Vec::new();
    }
    let mut extractor = Extractor {
        artifact_kind,
        limits,
        entries: Vec::new(),
        seen: HashSet::new(),
        nodes: 0,
        total_chars: 0,
    };
    if let Ok(value) = serde_json::from_slice::<Value>(bytes) {
        extractor.walk(&value, None, None, false, 0);
    } else if let Ok(text) = std::str::from_utf8(bytes) {
        for line in text.lines() {
            let Some(data) = line.trim().strip_prefix("data:") else {
                continue;
            };
            let data = data.trim();
            if data == "[DONE]" {
                continue;
            }
            if let Ok(value) = serde_json::from_str::<Value>(data) {
                extractor.walk(&value, None, None, false, 0);
            }
        }
    }
    extractor.entries
}

fn supported_artifact(kind: &str) -> bool {
    matches!(
        kind,
        "client_request"
            | "upstream_request"
            | "client_response"
            | "upstream_response"
            | "tool_arguments"
            | "tool_result"
            | "dario_upstream_request"
            | "dario_upstream_response"
    )
}

struct Extractor<'a> {
    artifact_kind: &'a str,
    limits: ExtractionLimits,
    entries: Vec<NormalizedEntry>,
    seen: HashSet<NormalizedEntry>,
    nodes: usize,
    total_chars: usize,
}

impl Extractor<'_> {
    fn walk(
        &mut self,
        value: &Value,
        parent_key: Option<&str>,
        role: Option<&str>,
        tool_context: bool,
        depth: usize,
    ) {
        if depth > MAX_JSON_DEPTH
            || self.nodes >= MAX_JSON_NODES
            || self.entries.len() >= self.limits.max_entries
            || self.total_chars >= self.limits.max_total_chars
        {
            return;
        }
        let role = role.or_else(|| {
            parent_key.and_then(|key| match canonical_key(key).as_str() {
                "system" | "instructions" => Some("system"),
                _ => None,
            })
        });
        self.nodes += 1;
        match value {
            Value::Object(object) => {
                let local_role = object
                    .get("role")
                    .or_else(|| object.get("author"))
                    .and_then(Value::as_str)
                    .or(role);
                let local_tool = tool_context
                    || local_role.is_some_and(|role| {
                        matches!(role.to_ascii_lowercase().as_str(), "tool" | "function")
                    })
                    || object.keys().any(|key| tool_key(key));
                for (key, child) in object {
                    if sensitive_key(key) || metadata_only_key(key) {
                        continue;
                    }
                    self.walk(
                        child,
                        Some(key),
                        local_role,
                        local_tool || tool_key(key),
                        depth + 1,
                    );
                }
            }
            Value::Array(values) => {
                for child in values {
                    self.walk(child, parent_key, role, tool_context, depth + 1);
                }
            }
            Value::String(text) if self.should_index(parent_key, tool_context) => {
                let kind = classify_entry(self.artifact_kind, parent_key, role, tool_context);
                self.push(kind, text);
            }
            _ => {}
        }
    }

    fn should_index(&self, parent_key: Option<&str>, tool_context: bool) -> bool {
        if self.artifact_kind.contains("tool_") || tool_context {
            return !parent_key.is_some_and(metadata_only_key);
        }
        parent_key.is_some_and(|key| {
            matches!(
                canonical_key(key).as_str(),
                "text"
                    | "content"
                    | "inputtext"
                    | "outputtext"
                    | "prompt"
                    | "system"
                    | "instructions"
                    | "description"
                    | "reasoning"
                    | "reasoningcontent"
                    | "thinking"
                    | "summary"
                    | "arguments"
                    | "result"
                    | "command"
                    | "query"
                    | "message"
                    | "error"
                    | "value"
            )
        })
    }

    fn push(&mut self, kind: String, text: &str) {
        let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if text.is_empty() {
            return;
        }
        let remaining = self.limits.max_total_chars.saturating_sub(self.total_chars);
        let limit = self.limits.max_entry_chars.min(remaining);
        let text = text.chars().take(limit).collect::<String>();
        if text.is_empty() {
            return;
        }
        let entry = NormalizedEntry { kind, text };
        if self.seen.insert(entry.clone()) {
            self.total_chars = self.total_chars.saturating_add(entry.text.chars().count());
            self.entries.push(entry);
        }
    }
}

fn classify_entry(
    artifact_kind: &str,
    parent_key: Option<&str>,
    role: Option<&str>,
    tool_context: bool,
) -> String {
    let key = parent_key.map(canonical_key).unwrap_or_default();
    if matches!(
        key.as_str(),
        "reasoning" | "reasoningcontent" | "thinking" | "summary"
    ) {
        return "reasoning".into();
    }
    if tool_context
        || artifact_kind.contains("tool_")
        || matches!(key.as_str(), "arguments" | "result" | "command")
    {
        return "tool".into();
    }
    match role.map(str::to_ascii_lowercase).as_deref() {
        Some("system") | Some("developer") => "system".into(),
        Some("user") => "user".into(),
        Some("assistant") | Some("model") => "assistant".into(),
        Some("tool") | Some("function") => "tool".into(),
        _ if artifact_kind.contains("request") => "user".into(),
        _ if artifact_kind.contains("response") => "assistant".into(),
        _ => "text".into(),
    }
}

fn canonical_key(key: &str) -> String {
    key.chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(|character| character.to_lowercase())
        .collect()
}

fn sensitive_key(key: &str) -> bool {
    let key = canonical_key(key);
    matches!(
        key.as_str(),
        "authorization"
            | "proxyauthorization"
            | "apikey"
            | "accesskey"
            | "accesstoken"
            | "refreshtoken"
            | "authtoken"
            | "sessiontoken"
            | "password"
            | "secret"
            | "clientsecret"
            | "cookie"
            | "setcookie"
            | "headers"
            | "signature"
            | "privatekey"
    ) || key.contains("credential")
        || key.ends_with("token")
}

fn metadata_only_key(key: &str) -> bool {
    matches!(
        canonical_key(key).as_str(),
        "id" | "role"
            | "type"
            | "model"
            | "object"
            | "status"
            | "finishreason"
            | "stopreason"
            | "created"
            | "createdat"
            | "timestamp"
            | "usage"
    )
}

fn tool_key(key: &str) -> bool {
    matches!(
        canonical_key(key).as_str(),
        "tool"
            | "tools"
            | "toolcall"
            | "toolcalls"
            | "tooluse"
            | "toolresult"
            | "function"
            | "functioncall"
            | "functionresponse"
            | "arguments"
            | "result"
            | "command"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extraction_is_semantic_bounded_and_redacts_sensitive_keys() {
        let bytes = br#"{
          "system":"be concise",
          "messages":[
            {"role":"user","content":"find the lunar widget", "authorization":"secret"},
            {"role":"assistant","content":[{"type":"text","text":"widget found"}],
             "reasoning_content":"private chain summary"},
            {"role":"tool","content":"tool output", "api_key":"never index me"}
          ]
        }"#;
        let entries = extract_entries(
            "client_request",
            bytes,
            ExtractionLimits {
                max_entries: 32,
                max_entry_chars: 1024,
                max_total_chars: 4096,
            },
        );
        assert!(entries
            .iter()
            .any(|entry| entry.kind == "user" && entry.text.contains("lunar widget")));
        assert!(entries
            .iter()
            .any(|entry| entry.kind == "system" && entry.text == "be concise"));
        assert!(entries
            .iter()
            .any(|entry| entry.kind == "assistant" && entry.text == "widget found"));
        assert!(entries
            .iter()
            .any(|entry| entry.kind == "reasoning" && entry.text.contains("chain summary")));
        assert!(entries
            .iter()
            .any(|entry| entry.kind == "tool" && entry.text == "tool output"));
        assert!(!entries.iter().any(|entry| entry.text.contains("secret")));
        assert!(!entries
            .iter()
            .any(|entry| entry.text.contains("never index")));
    }

    #[test]
    fn fts_query_is_literal_bounded_and_prefix_searchable() {
        assert_eq!(
            fts_match_query(" lunar   widget ").as_deref(),
            Some("\"lunar\"* AND \"widget\"*")
        );
        assert_eq!(fts_match_query("---"), None);
    }
}
