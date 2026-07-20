//! Reference-aware retention and garbage accounting for the global LAR store.
//!
//! Body packs are immutable, so a sweep does not punch holes in an existing
//! pack. Instead it durably quarantines the exact manifest/chunk identities in
//! an audit run. A later copy-verify-switch-retire repack can omit identities
//! that are still unreachable. This keeps normal reads and content reuse safe
//! while making mark/sweep restartable.

use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

use crate::Store;

static GC_RUN_SEQUENCE: AtomicU64 = AtomicU64::new(0);

const GC_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS lar_gc_candidates (
  run_id             TEXT NOT NULL,
  object_kind        TEXT NOT NULL CHECK (object_kind IN ('manifest', 'chunk')),
  candidate_id       TEXT NOT NULL,
  manifest_id        TEXT,
  hash_algorithm     TEXT,
  chunk_hash         BLOB,
  file_uuid          TEXT,
  compressed_length  INTEGER NOT NULL DEFAULT 0 CHECK (compressed_length >= 0),
  state              TEXT NOT NULL DEFAULT 'marked'
                     CHECK (state IN ('marked', 'swept', 'retained')),
  reason             TEXT,
  PRIMARY KEY (run_id, object_kind, candidate_id),
  FOREIGN KEY (run_id) REFERENCES lar_gc_runs(run_id)
);
CREATE INDEX IF NOT EXISTS lar_gc_candidates_state
  ON lar_gc_candidates(run_id, state, object_kind);
"#;

const REACHABILITY_CTE: &str = r#"
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
),
reachable_chunks(hash_algorithm, chunk_hash) AS (
  SELECT DISTINCT mc.hash_algorithm, mc.chunk_hash
    FROM lar_manifest_chunks mc
    JOIN reachable_manifests rm ON rm.manifest_id=mc.manifest_id
)
"#;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct LarGcReport {
    pub run_id: Option<String>,
    pub archive_set_uuid: Option<String>,
    pub state: String,
    pub dry_run: bool,
    pub reachable_manifests: u64,
    pub reachable_chunks: u64,
    pub unreachable_manifests: u64,
    pub unreachable_chunks: u64,
    /// Compressed bytes occupied by unreachable chunk frames. These bytes are
    /// physically reclaimed only after a verified pack repack.
    pub garbage_compressed_bytes: u64,
    pub swept_manifests: u64,
    pub swept_chunks: u64,
    pub retained_after_recheck: u64,
    pub physical_bytes_reclaimed: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ReachabilityCounts {
    reachable_manifests: u64,
    reachable_chunks: u64,
    unreachable_manifests: u64,
    unreachable_chunks: u64,
    garbage_compressed_bytes: u64,
}

fn nonnegative(value: i64, field: &str) -> Result<u64> {
    value
        .try_into()
        .with_context(|| format!("negative LAR GC {field}: {value}"))
}

fn reachability_counts(conn: &Connection) -> Result<ReachabilityCounts> {
    let sql = format!(
        "{REACHABILITY_CTE}
         SELECT
           (SELECT COUNT(*) FROM lar_manifests m
             JOIN reachable_manifests rm ON rm.manifest_id=m.manifest_id
            WHERE m.state='ready'),
           (SELECT COUNT(*) FROM lar_chunks c
             JOIN reachable_chunks rc
               ON rc.hash_algorithm=c.hash_algorithm AND rc.chunk_hash=c.chunk_hash
            WHERE c.state='ready'),
           (SELECT COUNT(*) FROM lar_manifests m
            WHERE m.state='ready' AND NOT EXISTS
              (SELECT 1 FROM reachable_manifests rm WHERE rm.manifest_id=m.manifest_id)),
           (SELECT COUNT(*) FROM lar_chunks c
            WHERE c.state='ready' AND NOT EXISTS
              (SELECT 1 FROM reachable_chunks rc
                WHERE rc.hash_algorithm=c.hash_algorithm AND rc.chunk_hash=c.chunk_hash)),
           (SELECT COALESCE(SUM(c.compressed_length), 0) FROM lar_chunks c
            WHERE c.state='ready' AND NOT EXISTS
              (SELECT 1 FROM reachable_chunks rc
                WHERE rc.hash_algorithm=c.hash_algorithm AND rc.chunk_hash=c.chunk_hash))"
    );
    let values: (i64, i64, i64, i64, i64) = conn.query_row(&sql, [], |row| {
        Ok((
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
        ))
    })?;
    Ok(ReachabilityCounts {
        reachable_manifests: nonnegative(values.0, "reachable manifest count")?,
        reachable_chunks: nonnegative(values.1, "reachable chunk count")?,
        unreachable_manifests: nonnegative(values.2, "unreachable manifest count")?,
        unreachable_chunks: nonnegative(values.3, "unreachable chunk count")?,
        garbage_compressed_bytes: nonnegative(values.4, "garbage byte count")?,
    })
}

pub(crate) fn migrate(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute_batch(GC_SCHEMA)?;
    tx.commit()?;
    Ok(())
}

fn selected_archive_set(conn: &Connection) -> Result<Option<String>> {
    conn.query_row(
        "SELECT archive_set_uuid FROM lar_archive_sets
         ORDER BY CASE state WHEN 'active' THEN 0 WHEN 'sealed' THEN 1 ELSE 2 END,
                  updated_at_ms DESC, archive_set_uuid
         LIMIT 1",
        [],
        |row| row.get(0),
    )
    .optional()
    .map_err(Into::into)
}

fn new_run_id(now_ms: i64) -> String {
    format!(
        "gc-{now_ms}-{}-{}",
        std::process::id(),
        GC_RUN_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )
}

fn mark_candidates(conn: &Connection, run_id: &str) -> Result<ReachabilityCounts> {
    let counts = reachability_counts(conn)?;
    let manifests = format!(
        "{REACHABILITY_CTE}
         INSERT INTO lar_gc_candidates
           (run_id, object_kind, candidate_id, manifest_id, file_uuid,
            compressed_length, state, reason)
         SELECT ?1, 'manifest', m.manifest_id, m.manifest_id, m.file_uuid,
                0, 'marked', 'no retained artifact or stage reference'
           FROM lar_manifests m
          WHERE m.state='ready' AND NOT EXISTS
            (SELECT 1 FROM reachable_manifests rm WHERE rm.manifest_id=m.manifest_id)
         ON CONFLICT(run_id, object_kind, candidate_id) DO NOTHING"
    );
    conn.execute(&manifests, [run_id])?;

    let chunks = format!(
        "{REACHABILITY_CTE}
         INSERT INTO lar_gc_candidates
           (run_id, object_kind, candidate_id, hash_algorithm, chunk_hash,
            file_uuid, compressed_length, state, reason)
         SELECT ?1, 'chunk', c.hash_algorithm || ':' || lower(hex(c.chunk_hash)),
                c.hash_algorithm, c.chunk_hash, c.file_uuid, c.compressed_length,
                'marked', 'not reachable through a retained manifest'
           FROM lar_chunks c
          WHERE c.state='ready' AND NOT EXISTS
            (SELECT 1 FROM reachable_chunks rc
              WHERE rc.hash_algorithm=c.hash_algorithm AND rc.chunk_hash=c.chunk_hash)
         ON CONFLICT(run_id, object_kind, candidate_id) DO NOTHING"
    );
    conn.execute(&chunks, [run_id])?;
    Ok(counts)
}

fn report_for_run(conn: &Connection, run_id: &str) -> Result<LarGcReport> {
    let run: Option<(String, String, i64, i64, i64)> = conn
        .query_row(
            "SELECT archive_set_uuid, state, reachable_manifests,
                    reachable_chunks, unreachable_chunks
               FROM lar_gc_runs WHERE run_id=?1",
            [run_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .optional()?;
    let Some((
        archive_set_uuid,
        state,
        reachable_manifests,
        reachable_chunks,
        recorded_unreachable_chunks,
    )) = run
    else {
        bail!("unknown LAR GC run: {run_id}");
    };
    let candidates: (i64, i64, i64, i64, i64, i64) = conn.query_row(
        "SELECT
           COALESCE(SUM(CASE WHEN object_kind='manifest' THEN 1 ELSE 0 END), 0),
           COALESCE(SUM(CASE WHEN object_kind='chunk' THEN 1 ELSE 0 END), 0),
           COALESCE(SUM(CASE WHEN object_kind='chunk' THEN compressed_length ELSE 0 END), 0),
           COALESCE(SUM(CASE WHEN object_kind='manifest' AND state='swept' THEN 1 ELSE 0 END), 0),
           COALESCE(SUM(CASE WHEN object_kind='chunk' AND state='swept' THEN 1 ELSE 0 END), 0),
           COALESCE(SUM(CASE WHEN state='retained' THEN 1 ELSE 0 END), 0)
         FROM lar_gc_candidates WHERE run_id=?1",
        [run_id],
        |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
            ))
        },
    )?;
    Ok(LarGcReport {
        run_id: Some(run_id.to_string()),
        archive_set_uuid: Some(archive_set_uuid),
        state,
        dry_run: false,
        reachable_manifests: nonnegative(reachable_manifests, "reachable manifest count")?,
        reachable_chunks: nonnegative(reachable_chunks, "reachable chunk count")?,
        unreachable_manifests: nonnegative(candidates.0, "candidate manifest count")?,
        unreachable_chunks: nonnegative(recorded_unreachable_chunks, "candidate chunk count")?,
        garbage_compressed_bytes: nonnegative(candidates.2, "garbage byte count")?,
        swept_manifests: nonnegative(candidates.3, "swept manifest count")?,
        swept_chunks: nonnegative(candidates.4, "swept chunk count")?,
        retained_after_recheck: nonnegative(candidates.5, "retained candidate count")?,
        physical_bytes_reclaimed: 0,
    })
}

impl Store {
    /// Compute global reachability without writing an audit run or changing any
    /// catalog/object state.
    pub fn plan_lar_gc(&self) -> Result<LarGcReport> {
        let conn = self.conn.lock().unwrap();
        let archive_set_uuid = selected_archive_set(&conn)?;
        let counts = reachability_counts(&conn)?;
        Ok(LarGcReport {
            run_id: None,
            archive_set_uuid,
            state: "dry-run".into(),
            dry_run: true,
            reachable_manifests: counts.reachable_manifests,
            reachable_chunks: counts.reachable_chunks,
            unreachable_manifests: counts.unreachable_manifests,
            unreachable_chunks: counts.unreachable_chunks,
            garbage_compressed_bytes: counts.garbage_compressed_bytes,
            swept_manifests: 0,
            swept_chunks: 0,
            retained_after_recheck: 0,
            physical_bytes_reclaimed: 0,
        })
    }

    /// Durably mark a reachability snapshot. Committing the complete candidate
    /// set before sweeping gives restart recovery an unambiguous boundary.
    pub fn start_lar_gc(&self, now_ms: i64) -> Result<LarGcReport> {
        let mut conn = self.conn.lock().unwrap();
        let archive_set_uuid = selected_archive_set(&conn)?
            .context("cannot start LAR GC before an archive set is cataloged")?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let active: Option<String> = tx
            .query_row(
                "SELECT run_id FROM lar_gc_runs
                 WHERE state IN ('marking','sweeping','repacking')
                 ORDER BY started_at_ms, run_id LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(run_id) = active {
            tx.commit()?;
            return report_for_run(&conn, &run_id);
        }
        let run_id = new_run_id(now_ms);
        tx.execute(
            "INSERT INTO lar_gc_runs
               (run_id, archive_set_uuid, state, started_at_ms, updated_at_ms)
             VALUES (?1, ?2, 'marking', ?3, ?3)",
            params![run_id, archive_set_uuid, now_ms],
        )?;
        let counts = mark_candidates(&tx, &run_id)?;
        tx.execute(
            "UPDATE lar_gc_runs SET state='sweeping', updated_at_ms=?2,
                    reachable_manifests=?3, reachable_chunks=?4,
                    unreachable_chunks=?5
              WHERE run_id=?1",
            params![
                run_id,
                now_ms,
                counts.reachable_manifests,
                counts.reachable_chunks,
                counts.unreachable_chunks,
            ],
        )?;
        tx.commit()?;
        report_for_run(&conn, &run_id)
    }

    /// Recheck every marked identity against current roots, then quarantine the
    /// still-unreachable candidates in the durable audit. This is idempotent:
    /// completed runs can be resumed after a process restart without changes.
    pub fn resume_lar_gc(&self, run_id: &str, now_ms: i64) -> Result<LarGcReport> {
        let mut conn = self.conn.lock().unwrap();
        let state: Option<String> = conn
            .query_row(
                "SELECT state FROM lar_gc_runs WHERE run_id=?1",
                [run_id],
                |row| row.get(0),
            )
            .optional()?;
        let Some(state) = state else {
            bail!("unknown LAR GC run: {run_id}");
        };
        if state == "complete" {
            return report_for_run(&conn, run_id);
        }
        if state == "failed" || state == "repacking" {
            bail!("LAR GC run {run_id} cannot be resumed from state {state}");
        }

        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if state == "marking" {
            // A v1 mark normally commits atomically with the `sweeping` state,
            // but rebuilding makes recovery safe for imported/older catalogs.
            tx.execute("DELETE FROM lar_gc_candidates WHERE run_id=?1", [run_id])?;
            let counts = mark_candidates(&tx, run_id)?;
            tx.execute(
                "UPDATE lar_gc_runs SET state='sweeping', updated_at_ms=?2,
                        reachable_manifests=?3, reachable_chunks=?4,
                        unreachable_chunks=?5 WHERE run_id=?1",
                params![
                    run_id,
                    now_ms,
                    counts.reachable_manifests,
                    counts.reachable_chunks,
                    counts.unreachable_chunks,
                ],
            )?;
        }

        let retain_manifests = format!(
            "{REACHABILITY_CTE}
             UPDATE lar_gc_candidates
                SET state='retained', reason='became reachable after mark'
              WHERE run_id=?1 AND state='marked' AND object_kind='manifest'
                AND EXISTS (SELECT 1 FROM reachable_manifests rm
                             WHERE rm.manifest_id=lar_gc_candidates.manifest_id)"
        );
        tx.execute(&retain_manifests, [run_id])?;
        let retain_chunks = format!(
            "{REACHABILITY_CTE}
             UPDATE lar_gc_candidates
                SET state='retained', reason='became reachable after mark'
              WHERE run_id=?1 AND state='marked' AND object_kind='chunk'
                AND EXISTS (SELECT 1 FROM reachable_chunks rc
                             WHERE rc.hash_algorithm=lar_gc_candidates.hash_algorithm
                               AND rc.chunk_hash=lar_gc_candidates.chunk_hash)"
        );
        tx.execute(&retain_chunks, [run_id])?;
        tx.execute(
            "UPDATE lar_gc_candidates
                SET state='swept', reason='quarantined for verified repack'
              WHERE run_id=?1 AND state='marked'",
            [run_id],
        )?;
        tx.execute(
            "UPDATE lar_gc_runs SET state='complete', updated_at_ms=?2,
                    completed_at_ms=?2, bytes_reclaimed=0, last_error=NULL
              WHERE run_id=?1",
            params![run_id, now_ms],
        )?;
        tx.commit()?;
        report_for_run(&conn, run_id)
    }

    /// Convenience apply path; callers that need an explicit crash boundary
    /// can persist `start_lar_gc` and invoke `resume_lar_gc` after restart.
    pub fn run_lar_gc(&self, now_ms: i64) -> Result<LarGcReport> {
        let marked = self.start_lar_gc(now_ms)?;
        let run_id = marked
            .run_id
            .as_deref()
            .context("persisted LAR GC run did not return an ID")?;
        self.resume_lar_gc(run_id, now_ms)
    }
}

/// Remove only the roots owned by one trace. Chunks and manifests are never
/// deleted here; globally shared content remains reachable through other roots.
pub(crate) fn delete_trace_references(conn: &Connection, trace_id: &str) -> Result<()> {
    crate::lar_fts::delete_trace_references(conn, trace_id)?;
    // Explicit deletion is a durable disposition, not a transient projection
    // loss. Prevent startup recovery from recreating this parent's children.
    conn.execute(
        "UPDATE tool_calls SET canonical_timeline=0 WHERE id IN
           (SELECT tool_id FROM lar_timeline_supplements
             WHERE parent_trace_id=?1 OR display_trace_id=?1)",
        [trace_id],
    )?;
    conn.execute(
        "INSERT OR IGNORE INTO lar_timeline_supplement_tombstones
           (tool_id, phase, supplement_trace_id, parent_trace_id, deleted_at_ms)
         SELECT tool_id, phase, supplement_trace_id, parent_trace_id,
                CAST(strftime('%s','now') AS INTEGER) * 1000
           FROM lar_timeline_supplements
          WHERE parent_trace_id=?1 OR display_trace_id=?1",
        [trace_id],
    )?;
    conn.execute(
        "DELETE FROM lar_stage_records WHERE stage_id IN
           (SELECT stage_id FROM lar_timeline_supplements
             WHERE parent_trace_id=?1 OR display_trace_id=?1)",
        [trace_id],
    )?;
    conn.execute(
        "DELETE FROM lar_exchange_records WHERE trace_id IN
           (SELECT supplement_trace_id FROM lar_timeline_supplements
             WHERE parent_trace_id=?1 OR display_trace_id=?1)",
        [trace_id],
    )?;
    conn.execute(
        "DELETE FROM lar_timeline_supplements
          WHERE parent_trace_id=?1 OR display_trace_id=?1",
        [trace_id],
    )?;
    conn.execute(
        "DELETE FROM lar_trace_artifacts WHERE owner_kind='trace' AND owner_id=?1",
        [trace_id],
    )?;
    conn.execute(
        "DELETE FROM lar_stage_records WHERE trace_id=?1",
        [trace_id],
    )?;
    conn.execute(
        "DELETE FROM lar_exchange_records WHERE trace_id=?1",
        [trace_id],
    )?;
    delete_trace_conversation_references(conn, trace_id)?;
    delete_orphan_transport_metadata(conn)?;
    Ok(())
}

pub(crate) fn clear_all_trace_references(conn: &Connection) -> Result<()> {
    crate::lar_fts::clear_all_references(conn)?;
    conn.execute("DELETE FROM lar_trace_artifacts", [])?;
    conn.execute("DELETE FROM lar_timeline_supplements", [])?;
    conn.execute("DELETE FROM lar_timeline_supplement_tombstones", [])?;
    conn.execute("DELETE FROM lar_stage_records", [])?;
    conn.execute("DELETE FROM lar_exchange_records", [])?;
    conn.execute("DELETE FROM lar_conversation_turn_responses", [])?;
    conn.execute("DELETE FROM lar_conversation_turn_views", [])?;
    conn.execute("DELETE FROM lar_conversation_session_generations", [])?;
    conn.execute("DELETE FROM lar_conversation_generation_entries", [])?;
    conn.execute("DELETE FROM lar_conversation_generations", [])?;
    conn.execute("DELETE FROM lar_conversation_entry_ranges", [])?;
    conn.execute("DELETE FROM lar_conversation_entry_formats", [])?;
    conn.execute("DELETE FROM lar_conversation_entry_fingerprints", [])?;
    conn.execute("DELETE FROM lar_conversation_entries", [])?;
    Ok(())
}

/// Apply trace/tool retention to LAR roots in the same SQLite transaction as
/// the owning row mutation. A bodies-only pass preserves stage/header history.
pub(crate) fn prune_references(
    conn: &Connection,
    older_than_ms: i64,
    bodies_only: bool,
) -> Result<()> {
    crate::lar_fts::prune_references(conn, older_than_ms)?;
    if bodies_only {
        conn.execute(
            "DELETE FROM lar_trace_artifacts WHERE
                    (owner_kind='trace' AND owner_id IN
                       (SELECT id FROM traces WHERE ts_request_ms < ?1)) OR
                     (owner_kind='tool_call' AND owner_id IN
                       (SELECT id FROM tool_calls WHERE ts_start_ms < ?1))",
            [older_than_ms],
        )?;
        conn.execute(
            "UPDATE lar_stage_records
                SET request_headers_ref=NULL, request_body_manifest_ref=NULL,
                    response_headers_ref=NULL, response_body_manifest_ref=NULL,
                    trailers_ref=NULL, stream_index_ref=NULL
              WHERE trace_id IN (SELECT id FROM traces WHERE ts_request_ms < ?1)
                 OR stage_id IN (
                    SELECT s.stage_id FROM lar_timeline_supplements s
                    LEFT JOIN tool_calls t ON t.id=s.tool_id
                    LEFT JOIN traces p ON p.id=s.display_trace_id
                    WHERE t.ts_start_ms < ?1 OR p.ts_request_ms < ?1)",
            [older_than_ms],
        )?;
        conn.execute(
            "UPDATE lar_timeline_supplements SET manifest_id=NULL
              WHERE tool_id IN (SELECT id FROM tool_calls WHERE ts_start_ms < ?1)
                 OR display_trace_id IN (SELECT id FROM traces WHERE ts_request_ms < ?1)",
            [older_than_ms],
        )?;
        prune_conversation_references(conn, older_than_ms)?;
        delete_orphan_transport_metadata(conn)?;
    } else {
        conn.execute(
            "UPDATE tool_calls SET canonical_timeline=0
              WHERE id IN (
                SELECT s.tool_id FROM lar_timeline_supplements s
                LEFT JOIN tool_calls t ON t.id=s.tool_id
                LEFT JOIN traces p ON p.id=s.display_trace_id
                WHERE t.ts_start_ms < ?1 OR p.ts_request_ms < ?1)",
            [older_than_ms],
        )?;
        conn.execute(
            "INSERT OR IGNORE INTO lar_timeline_supplement_tombstones
               (tool_id, phase, supplement_trace_id, parent_trace_id, deleted_at_ms)
             SELECT s.tool_id, s.phase, s.supplement_trace_id, s.parent_trace_id, ?1
               FROM lar_timeline_supplements s
               LEFT JOIN tool_calls t ON t.id=s.tool_id
               LEFT JOIN traces p ON p.id=s.display_trace_id
              WHERE t.ts_start_ms < ?1 OR p.ts_request_ms < ?1",
            [older_than_ms],
        )?;
        conn.execute(
            "DELETE FROM lar_trace_artifacts WHERE
                 (owner_kind='trace' AND owner_id IN
                    (SELECT id FROM traces WHERE ts_request_ms < ?1)) OR
                 (owner_kind='tool_call' AND owner_id IN
                    (SELECT id FROM tool_calls WHERE ts_start_ms < ?1))",
            [older_than_ms],
        )?;
        conn.execute(
            "DELETE FROM lar_stage_records WHERE stage_id IN
               (SELECT s.stage_id FROM lar_timeline_supplements s
                LEFT JOIN tool_calls t ON t.id=s.tool_id
                LEFT JOIN traces p ON p.id=s.display_trace_id
                WHERE t.ts_start_ms < ?1 OR p.ts_request_ms < ?1)",
            [older_than_ms],
        )?;
        conn.execute(
            "DELETE FROM lar_exchange_records WHERE trace_id IN
               (SELECT s.supplement_trace_id FROM lar_timeline_supplements s
                LEFT JOIN tool_calls t ON t.id=s.tool_id
                LEFT JOIN traces p ON p.id=s.display_trace_id
                WHERE t.ts_start_ms < ?1 OR p.ts_request_ms < ?1)",
            [older_than_ms],
        )?;
        conn.execute(
            "DELETE FROM lar_timeline_supplements
              WHERE tool_id IN (SELECT id FROM tool_calls WHERE ts_start_ms < ?1)
                 OR display_trace_id IN (SELECT id FROM traces WHERE ts_request_ms < ?1)",
            [older_than_ms],
        )?;
        conn.execute(
            "DELETE FROM lar_stage_records
              WHERE trace_id IN (SELECT id FROM traces WHERE ts_request_ms < ?1)",
            [older_than_ms],
        )?;
        conn.execute(
            "DELETE FROM lar_exchange_records
              WHERE trace_id IN (SELECT id FROM traces WHERE ts_request_ms < ?1)",
            [older_than_ms],
        )?;
        prune_conversation_references(conn, older_than_ms)?;
        delete_orphan_transport_metadata(conn)?;
    }
    Ok(())
}

fn delete_trace_conversation_references(conn: &Connection, trace_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM lar_conversation_session_generations
          WHERE generation_id IN
            (SELECT generation_id FROM lar_conversation_turn_views WHERE trace_id=?1)
            AND NOT EXISTS (
              SELECT 1 FROM lar_conversation_turn_views other
               WHERE other.generation_id=lar_conversation_session_generations.generation_id
                 AND other.trace_id!=?1)",
        [trace_id],
    )?;
    conn.execute(
        "DELETE FROM lar_conversation_turn_responses WHERE turn_view_id IN
           (SELECT turn_view_id FROM lar_conversation_turn_views WHERE trace_id=?1)",
        [trace_id],
    )?;
    conn.execute(
        "DELETE FROM lar_conversation_turn_views WHERE trace_id=?1",
        [trace_id],
    )?;
    delete_orphan_conversation_graph(conn)
}

fn prune_conversation_references(conn: &Connection, older_than_ms: i64) -> Result<()> {
    conn.execute(
        "DELETE FROM lar_conversation_session_generations
          WHERE generation_id IN (
            SELECT tv.generation_id FROM lar_conversation_turn_views tv
            JOIN traces t ON t.id=tv.trace_id
            WHERE t.ts_request_ms < ?1)
            AND NOT EXISTS (
            SELECT 1 FROM lar_conversation_turn_views tv
            JOIN traces t ON t.id=tv.trace_id
            WHERE tv.generation_id=lar_conversation_session_generations.generation_id
              AND t.ts_request_ms >= ?1)",
        [older_than_ms],
    )?;
    conn.execute(
        "DELETE FROM lar_conversation_turn_responses WHERE turn_view_id IN
           (SELECT tv.turn_view_id FROM lar_conversation_turn_views tv
            JOIN traces t ON t.id=tv.trace_id WHERE t.ts_request_ms < ?1)",
        [older_than_ms],
    )?;
    conn.execute(
        "DELETE FROM lar_conversation_turn_views WHERE trace_id IN
           (SELECT id FROM traces WHERE ts_request_ms < ?1)",
        [older_than_ms],
    )?;
    delete_orphan_conversation_graph(conn)
}

fn delete_orphan_conversation_graph(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "WITH RECURSIVE retained(generation_id) AS (
           SELECT generation_id FROM lar_conversation_turn_views
           UNION SELECT generation_id FROM lar_conversation_session_generations
           UNION
           SELECT g.parent_generation_id FROM lar_conversation_generations g
           JOIN retained r ON r.generation_id=g.generation_id
           WHERE g.parent_generation_id IS NOT NULL
         )
         DELETE FROM lar_conversation_generation_entries
          WHERE generation_id NOT IN (SELECT generation_id FROM retained);
         WITH RECURSIVE retained(generation_id) AS (
           SELECT generation_id FROM lar_conversation_turn_views
           UNION SELECT generation_id FROM lar_conversation_session_generations
           UNION
           SELECT g.parent_generation_id FROM lar_conversation_generations g
           JOIN retained r ON r.generation_id=g.generation_id
           WHERE g.parent_generation_id IS NOT NULL
         )
         DELETE FROM lar_conversation_generations
          WHERE generation_id NOT IN (SELECT generation_id FROM retained);
         DELETE FROM lar_conversation_entry_ranges WHERE entry_id NOT IN (
           SELECT entry_id FROM lar_conversation_generation_entries
           UNION SELECT entry_id FROM lar_conversation_turn_responses);
         DELETE FROM lar_conversation_entry_formats WHERE entry_id NOT IN (
           SELECT entry_id FROM lar_conversation_generation_entries
           UNION SELECT entry_id FROM lar_conversation_turn_responses);
         DELETE FROM lar_conversation_entry_fingerprints WHERE entry_id NOT IN (
           SELECT entry_id FROM lar_conversation_generation_entries
           UNION SELECT entry_id FROM lar_conversation_turn_responses);
         DELETE FROM lar_conversation_entries WHERE entry_id NOT IN (
           SELECT entry_id FROM lar_conversation_generation_entries
           UNION SELECT entry_id FROM lar_conversation_turn_responses);",
    )?;
    Ok(())
}

fn delete_orphan_transport_metadata(conn: &Connection) -> Result<()> {
    conn.execute(
        "DELETE FROM lar_header_block_atoms WHERE block_id NOT IN (
           SELECT request_headers_ref FROM lar_stage_records WHERE request_headers_ref IS NOT NULL
           UNION SELECT response_headers_ref FROM lar_stage_records WHERE response_headers_ref IS NOT NULL
           UNION SELECT trailers_ref FROM lar_stage_records WHERE trailers_ref IS NOT NULL
           UNION SELECT header_block_id FROM lar_trace_artifacts WHERE header_block_id IS NOT NULL)",
        [],
    )?;
    conn.execute(
        "DELETE FROM lar_header_blocks WHERE block_id NOT IN (
           SELECT request_headers_ref FROM lar_stage_records WHERE request_headers_ref IS NOT NULL
           UNION SELECT response_headers_ref FROM lar_stage_records WHERE response_headers_ref IS NOT NULL
           UNION SELECT trailers_ref FROM lar_stage_records WHERE trailers_ref IS NOT NULL
           UNION SELECT header_block_id FROM lar_trace_artifacts WHERE header_block_id IS NOT NULL)",
        [],
    )?;
    conn.execute(
        "DELETE FROM lar_header_atoms WHERE atom_id NOT IN
           (SELECT atom_id FROM lar_header_block_atoms)",
        [],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    fn tmpdir(name: &str) -> PathBuf {
        let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "alex-lar-gc-{name}-{}-{sequence}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn seed_object(store: &Store, suffix: u8) -> (String, Vec<u8>) {
        let manifest = format!("manifest-{suffix}");
        let hash = vec![suffix; 32];
        let conn = store.conn.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO lar_archive_sets
               (archive_set_uuid, created_at_ms, updated_at_ms, state)
             VALUES ('set-1', 1, 1, 'active')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO lar_files
               (file_uuid, archive_set_uuid, role, path, state, container_major,
                container_minor, created_at_ms)
             VALUES ('file-1', 'set-1', 'body-pack', '/tmp/test.lar', 'sealed', 1, 0, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO lar_chunks
               (hash_algorithm, chunk_hash, uncompressed_length, compression,
                compressed_length, file_uuid, record_id, page_offset, checksum,
                created_at_ms, state)
             VALUES ('blake3', ?1, 100, 'zstd', 40, 'file-1', ?2, ?3, ?1, 1, 'ready')",
            params![hash, format!("chunk-{suffix}"), u64::from(suffix)],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO lar_manifests
               (manifest_id, total_length, hash_algorithm, whole_body_hash,
                created_at_ms, state)
             VALUES (?1, 100, 'blake3', ?2, 1, 'ready')",
            params![manifest, vec![suffix.wrapping_add(100); 32]],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO lar_manifest_chunks
               (manifest_id, ordinal, hash_algorithm, chunk_hash,
                logical_offset, chunk_offset, length)
             VALUES (?1, 0, 'blake3', ?2, 0, 0, 100)",
            params![manifest, hash],
        )
        .unwrap();
        (manifest, hash)
    }

    fn seed_trace_root(store: &Store, trace_id: &str, manifest_id: &str, ts: i64) {
        let conn = store.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO traces (id, ts_request_ms, session_id) VALUES (?1, ?2, ?3)",
            params![trace_id, ts, format!("session-{trace_id}")],
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

    #[test]
    fn deleting_one_trace_never_collects_a_shared_manifest_or_chunk() {
        let store = Store::open(tmpdir("shared")).unwrap();
        let (manifest, _) = seed_object(&store, 1);
        seed_trace_root(&store, "trace-a", &manifest, 1);
        seed_trace_root(&store, "trace-b", &manifest, 2);

        store.delete_trace("trace-a").unwrap();
        let retained = store.plan_lar_gc().unwrap();
        assert_eq!(retained.reachable_manifests, 1);
        assert_eq!(retained.reachable_chunks, 1);
        assert_eq!(retained.unreachable_manifests, 0);
        assert_eq!(retained.unreachable_chunks, 0);

        store.delete_trace("trace-b").unwrap();
        let orphaned = store.plan_lar_gc().unwrap();
        assert_eq!(orphaned.unreachable_manifests, 1);
        assert_eq!(orphaned.unreachable_chunks, 1);
    }

    #[test]
    fn dry_run_reports_orphans_without_mutating_catalog_or_audit() {
        let store = Store::open(tmpdir("dry-run")).unwrap();
        seed_object(&store, 2);
        let before = {
            let conn = store.conn.lock().unwrap();
            (
                conn.query_row("SELECT COUNT(*) FROM lar_gc_runs", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
                conn.query_row("SELECT COUNT(*) FROM lar_gc_candidates", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            )
        };
        let report = store.plan_lar_gc().unwrap();
        assert!(report.dry_run);
        assert_eq!(report.unreachable_manifests, 1);
        assert_eq!(report.unreachable_chunks, 1);
        assert_eq!(report.garbage_compressed_bytes, 40);
        let conn = store.conn.lock().unwrap();
        assert_eq!(
            before,
            (
                conn.query_row("SELECT COUNT(*) FROM lar_gc_runs", [], |row| row
                    .get::<_, i64>(0))
                    .unwrap(),
                conn.query_row("SELECT COUNT(*) FROM lar_gc_candidates", [], |row| row
                    .get::<_, i64>(0))
                    .unwrap(),
            )
        );
        assert_eq!(
            conn.query_row("SELECT state FROM lar_manifests", [], |row| row
                .get::<_, String>(0))
                .unwrap(),
            "ready"
        );
    }

    #[test]
    fn apply_quarantines_orphans_for_repack_but_preserves_immutable_pack() {
        let store = Store::open(tmpdir("apply")).unwrap();
        seed_object(&store, 3);
        let report = store.run_lar_gc(10).unwrap();
        assert_eq!(report.state, "complete");
        assert_eq!(report.swept_manifests, 1);
        assert_eq!(report.swept_chunks, 1);
        assert_eq!(report.garbage_compressed_bytes, 40);
        assert_eq!(report.physical_bytes_reclaimed, 0);
        let conn = store.conn.lock().unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM lar_gc_candidates WHERE state='swept'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            2
        );
        assert_eq!(
            conn.query_row("SELECT state FROM lar_chunks", [], |row| row
                .get::<_, String>(0))
                .unwrap(),
            "ready",
            "sweep must not mutate an immutable body pack's reusable catalog entry"
        );
    }

    #[test]
    fn marked_run_resumes_after_restart_and_is_idempotent() {
        let data_dir = tmpdir("restart");
        let run_id = {
            let store = Store::open(data_dir.clone()).unwrap();
            seed_object(&store, 4);
            let marked = store.start_lar_gc(20).unwrap();
            assert_eq!(marked.state, "sweeping");
            marked.run_id.unwrap()
        };
        let reopened = Store::open(data_dir).unwrap();
        let complete = reopened.resume_lar_gc(&run_id, 21).unwrap();
        assert_eq!(complete.state, "complete");
        assert_eq!((complete.swept_manifests, complete.swept_chunks), (1, 1));
        assert_eq!(reopened.resume_lar_gc(&run_id, 22).unwrap(), complete);
    }

    #[test]
    fn sweep_rechecks_roots_added_after_mark() {
        let store = Store::open(tmpdir("recheck")).unwrap();
        let (manifest, _) = seed_object(&store, 5);
        let marked = store.start_lar_gc(30).unwrap();
        seed_trace_root(&store, "late-trace", &manifest, 31);
        let complete = store
            .resume_lar_gc(marked.run_id.as_deref().unwrap(), 32)
            .unwrap();
        assert_eq!(complete.retained_after_recheck, 2);
        assert_eq!((complete.swept_manifests, complete.swept_chunks), (0, 0));
    }

    #[test]
    fn retention_and_backup_keep_chunks_shared_with_a_newer_trace() {
        let store = Store::open(tmpdir("retention-shared")).unwrap();
        let (manifest, _) = seed_object(&store, 6);
        seed_trace_root(&store, "old-trace", &manifest, 10);
        seed_trace_root(&store, "new-trace", &manifest, 100);

        let _backup = store.export_trace_backup_rows().unwrap();
        store.prune(50, true, false).unwrap();

        let report = store.plan_lar_gc().unwrap();
        assert_eq!(report.reachable_manifests, 1);
        assert_eq!(report.reachable_chunks, 1);
        assert_eq!(report.unreachable_manifests, 0);
        assert_eq!(report.unreachable_chunks, 0);
        let complete = store.run_lar_gc(200).unwrap();
        assert_eq!((complete.swept_manifests, complete.swept_chunks), (0, 0));
        let conn = store.conn.lock().unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM lar_trace_artifacts WHERE owner_id='old-trace'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM lar_trace_artifacts WHERE owner_id='new-trace'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            1
        );
    }
}
