#![recursion_limit = "256"]

use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use alex_core::{route_model, Pricing, Provider, TraceRecord};
use anyhow::{Context, Result};
use chrono::Utc;
use flate2::write::GzEncoder;
use flate2::Compression;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

mod lar_archive_ops;
mod lar_catalog;
mod lar_conversation;
mod lar_conversation_adapter;
mod lar_fts;
mod lar_gc;
mod lar_grep;
mod lar_jsonl_import;
mod lar_repack;
mod lar_stream_replay;
mod lar_verify;
mod legacy_import;
mod live_body_store;
mod standalone_import;

pub use lar_archive_ops::{
    LarArchiveAvailability, LarArchiveDetachReport, LarArchiveFileStatus,
    LarArchiveReattachOptions, LarArchiveReattachReport, LarArchiveUnavailableError,
};
pub use lar_catalog::{
    LarArtifactError, LarArtifactLocation, LarManifestRegistration, LarMigrationItem,
    LarMigrationJob, LarMigrationJobSpec, LarPointerSwitch, LarValidation,
};
pub use lar_conversation::{
    LarConversationBackfillReport, LarConversationEntryCapture, LarConversationEntryKind,
    LarConversationEntryView, LarConversationEventPage, LarConversationEventView,
    LarConversationEvidence, LarConversationEvidenceSource, LarConversationGenerationEvent,
    LarConversationRawRange, LarConversationRole, LarConversationSemantics,
    LarConversationTurnCapture, LarConversationTurnIds,
};
pub use lar_fts::{LarFtsRebuildOptions, LarFtsRebuildReport, LAR_NORMALIZED_INDEX_SCHEMA_VERSION};
pub use lar_gc::LarGcReport;
pub use lar_grep::{LarCatalogGrepMatch, LarCatalogGrepReport};
pub use lar_jsonl_import::{LarJsonlImportOptions, LarJsonlImportReport};
pub use lar_repack::{LarRepackCandidate, LarRepackConfig, LarRepackReport};
pub use lar_stream_replay::{
    LarStreamReplayError, LarStreamReplayPage, LarStreamReplayPageEvent,
    LarStreamReplayPageOptions, LarStreamReplaySource, DEFAULT_STREAM_REPLAY_PAGE_BYTES,
    DEFAULT_STREAM_REPLAY_PAGE_LIMIT, MAX_STREAM_REPLAY_PAGE_BYTES, MAX_STREAM_REPLAY_PAGE_LIMIT,
};
pub use lar_verify::{LarMigrationVerificationIssue, LarMigrationVerificationReport};
pub use legacy_import::{
    LarArtifactBatchRead, LarArtifactReadRequest, LarLegacyArtifact, LarLegacyImportBoundary,
    LarLegacyImportError, LarLegacyImportHook, LarLegacyImportOptions, LarLegacyImportReport,
    LarLegacyResourceControls, LarLegacySuffixArtifact,
};
pub use live_body_store::{
    LarBodyArtifact, LarBodyOwnerKind, LarBodyStoreConfig, LarBodyStoreMode, LarBodyWriteResult,
    LarExchangeBodyRefs, LarExchangeCapture, LarHeaderCapture, LarStreamReadCapture,
    LarUpstreamAttemptCapture, LAR_HEADER_FLAG_REDACTED,
};
pub use standalone_import::{
    LarBackupArtifactRef, LarStandaloneImportOptions, LarStandaloneImportReport,
};

static BODY_TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Display-only fields shared by the trace browser clients. Keeping this
/// derivation beside the session aggregate means every client gets the same
/// cheap, stable presentation values without having to reshape hundreds of
/// rows on its UI thread.
pub fn session_display_fields(row: &Value) -> Value {
    let session_id = row["session_id"].as_str().unwrap_or_default();
    let short_id = if session_id.chars().count() > 22 {
        format!(
            "{}…{}",
            session_id.chars().take(10).collect::<String>(),
            session_id
                .chars()
                .rev()
                .take(8)
                .collect::<String>()
                .chars()
                .rev()
                .collect::<String>()
        )
    } else {
        session_id.to_string()
    };
    let first = row["first_ts_ms"].as_i64().unwrap_or(0);
    let last = row["last_ts_ms"].as_i64().unwrap_or(first);
    let tags_summary = row["tags"]
        .as_object()
        .map(|tags| {
            let mut pairs: Vec<_> = tags
                .iter()
                .filter_map(|(key, value)| {
                    value.as_str().filter(|v| !v.is_empty()).map(|v| (key, v))
                })
                .collect();
            pairs.sort_by(|a, b| a.0.cmp(b.0));
            pairs
                .into_iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default();
    let status = row["last_status"].as_i64();
    let errors = row["errors"].as_i64().unwrap_or(0);
    let status_label = if errors > 0 || status.is_some_and(|s| s >= 400) {
        "Error"
    } else if status.is_some_and(|s| (200..400).contains(&s)) {
        "Done"
    } else {
        "Running"
    };
    json!({
        "short_id": short_id,
        "duration_ms": (last - first).max(0),
        "providers": row["providers"].clone(),
        "tags_summary": tags_summary,
        "status_label": status_label,
    })
}

/// Anthropic models are derived from the same embedded pricing catalogue used
/// to seed the store.  Keeping this here prevents Dario (or any other caller)
/// from growing a second, eventually-stale Claude list.
pub fn anthropic_catalog_models() -> Vec<String> {
    let models: Vec<Value> = serde_json::from_str(include_str!("models.json"))
        .expect("embedded models.json must be valid");
    let mut result: Vec<String> = models
        .into_iter()
        .filter_map(|entry| entry["model"].as_str().map(str::to_string))
        .filter_map(|model| {
            let (provider, routed) = route_model(&model);
            (provider == Some(Provider::Anthropic)).then_some(routed)
        })
        .collect();
    result.sort();
    result.dedup();
    result
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS traces (
  id                TEXT PRIMARY KEY,
  ts_request_ms     INTEGER NOT NULL,
  ts_response_ms    INTEGER,
  session_id        TEXT,
  harness           TEXT,
  client_format     TEXT,
  upstream_provider TEXT,
  upstream_format   TEXT,
  requested_model   TEXT,
  routed_model      TEXT,
  method            TEXT,
  path              TEXT,
  status            INTEGER,
  streamed          INTEGER,
  input_tokens      INTEGER,
  cached_input_tokens INTEGER,
  cache_creation_tokens INTEGER,
  output_tokens     INTEGER,
  reasoning_tokens  INTEGER,
  cost_usd          REAL,
  billing_bucket    TEXT,
  req_body_path     TEXT,
  upstream_req_body_path TEXT,
  resp_body_path    TEXT,
  req_headers_json  TEXT,
  resp_headers_json TEXT,
  error             TEXT,
  error_kind        TEXT,
  error_code        TEXT,
  error_class       TEXT,
  substituted       INTEGER NOT NULL DEFAULT 0,
  original_model    TEXT,
  served_model      TEXT,
  substitution_reason TEXT,
  attempts          TEXT,
  injected          INTEGER NOT NULL DEFAULT 0,
  fixture_name      TEXT,
  original_account_id TEXT,
  served_account_id TEXT,
  account_id        TEXT,
  run_id            TEXT,
  tags_json         TEXT,
  client_ip         TEXT,
  key_fingerprint   TEXT,
  reasoning_effort  TEXT,
  thinking_budget   INTEGER,
  subscription_identity TEXT,
  via_dario         INTEGER NOT NULL DEFAULT 0,
  dario_generation  TEXT
);
CREATE INDEX IF NOT EXISTS traces_session ON traces(session_id);
CREATE INDEX IF NOT EXISTS traces_session_ts ON traces(session_id, ts_request_ms, id);
CREATE INDEX IF NOT EXISTS traces_ts ON traces(ts_request_ms);
CREATE INDEX IF NOT EXISTS traces_model ON traces(routed_model);

-- A durable local catalogue, including removed accounts.  Trace rows retain
-- their historical account_id; this table lets them be attributed after the
-- local account file has gone away.
CREATE TABLE IF NOT EXISTS known_accounts (
  account_id TEXT PRIMARY KEY,
  provider TEXT NOT NULL,
  name TEXT NOT NULL,
  kind TEXT NOT NULL,
  subscription_identity TEXT,
  email TEXT,
  removed_ms INTEGER,
  last_seen_ms INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS known_accounts_identity ON known_accounts(subscription_identity);

CREATE TABLE IF NOT EXISTS pricing (
  model TEXT PRIMARY KEY,
  input_per_m REAL, cached_input_per_m REAL,
  cache_creation_per_m REAL, output_per_m REAL
);

CREATE TABLE IF NOT EXISTS heartbeats (
  ts_ms      INTEGER NOT NULL,
  provider   TEXT NOT NULL,
  account_id TEXT,
  ok         INTEGER,
  status     INTEGER,
  latency_ms INTEGER,
  message    TEXT
);
CREATE INDEX IF NOT EXISTS heartbeats_ts ON heartbeats(ts_ms);

CREATE TABLE IF NOT EXISTS run_keys (
  id TEXT PRIMARY KEY,
  key_hash TEXT UNIQUE NOT NULL,
  kind TEXT NOT NULL DEFAULT 'run',
  run_id TEXT,
  tags_json TEXT,
  label TEXT,
  created_ms INTEGER NOT NULL,
  expires_ms INTEGER,
  revoked INTEGER DEFAULT 0,
  use_count INTEGER DEFAULT 0,
  last_used_ms INTEGER
);

-- Verified parent/child session edges reported by harness lifecycle hooks.
-- The child id is also the request-side session key for current Codex
-- subagents, so this table joins lifecycle data to ordinary trace rows.
CREATE TABLE IF NOT EXISTS session_lineage (
  harness TEXT NOT NULL,
  child_session_id TEXT NOT NULL,
  parent_session_id TEXT NOT NULL,
  turn_id TEXT,
  agent_type TEXT,
  started_ms INTEGER NOT NULL,
  stopped_ms INTEGER,
  PRIMARY KEY (harness, child_session_id)
);
CREATE INDEX IF NOT EXISTS session_lineage_parent
  ON session_lineage(harness, parent_session_id);

-- Tool activity is deliberately separate from model traces: Pi reports it
-- asynchronously, and a model turn can have zero or many tool calls.
CREATE TABLE IF NOT EXISTS tool_calls (
  id                 TEXT PRIMARY KEY,
  harness            TEXT NOT NULL,
  session_id         TEXT NOT NULL,
  turn_id            TEXT,
  tool_call_id       TEXT NOT NULL,
  trace_id           TEXT,
  tool_name          TEXT NOT NULL,
  ts_start_ms        INTEGER NOT NULL,
  ts_end_ms          INTEGER,
  is_error           INTEGER,
  exit_status        INTEGER,
  args_body_path     TEXT,
  result_body_path   TEXT,
  UNIQUE(harness, session_id, tool_call_id)
);
CREATE INDEX IF NOT EXISTS tool_calls_session_ts ON tool_calls(session_id, ts_start_ms);
CREATE INDEX IF NOT EXISTS tool_calls_session_turn
  ON tool_calls(session_id, turn_id, ts_start_ms);
CREATE INDEX IF NOT EXISTS tool_calls_trace ON tool_calls(trace_id);
"#;

const RUN_KEY_COLS: &str =
    "id, key_hash, run_id, tags_json, label, created_ms, expires_ms, revoked, use_count, last_used_ms, kind";

fn run_key_row_json(r: &rusqlite::Row) -> rusqlite::Result<Value> {
    let key_hash = r.get::<_, String>(1)?;
    let tags = r
        .get::<_, Option<String>>(3)?
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .filter(|v| v.is_object())
        .unwrap_or_else(|| json!({}));
    Ok(json!({
        "id": r.get::<_, String>(0)?,
        "key_fingerprint": key_hash.chars().take(16).collect::<String>(),
        "run_id": r.get::<_, Option<String>>(2)?,
        "tags": tags,
        "label": r.get::<_, Option<String>>(4)?,
        "created_ms": r.get::<_, i64>(5)?,
        "expires_ms": r.get::<_, Option<i64>>(6)?,
        "revoked": r.get::<_, i64>(7)? != 0,
        "use_count": r.get::<_, i64>(8)?,
        "last_used_ms": r.get::<_, Option<i64>>(9)?,
        "kind": r.get::<_, String>(10)?,
    }))
}

const TRACE_COLS: &str =
    "id, ts_request_ms, ts_response_ms, harness, client_format, upstream_provider,
     requested_model, routed_model, status, streamed,
     input_tokens, cached_input_tokens, cache_creation_tokens, output_tokens, reasoning_tokens,
     cost_usd, billing_bucket, error, session_id, resp_body_path,
     error_kind, error_code, error_class,
     substituted, original_model, served_model, substitution_reason, attempts, injected, fixture_name,
     original_account_id, served_account_id,
     upstream_format, req_body_path, upstream_req_body_path, req_headers_json, resp_headers_json,
     account_id, run_id, tags_json, client_ip, key_fingerprint, reasoning_effort, thinking_budget,
     method, path, subscription_identity, via_dario, dario_generation";

const BACKUP_TRACE_COLS: &[&str] = &[
    "id",
    "ts_request_ms",
    "ts_response_ms",
    "session_id",
    "harness",
    "client_format",
    "upstream_provider",
    "upstream_format",
    "requested_model",
    "routed_model",
    "method",
    "path",
    "status",
    "streamed",
    "input_tokens",
    "cached_input_tokens",
    "cache_creation_tokens",
    "output_tokens",
    "reasoning_tokens",
    "cost_usd",
    "billing_bucket",
    "req_body_path",
    "upstream_req_body_path",
    "resp_body_path",
    "req_headers_json",
    "resp_headers_json",
    "error",
    "error_kind",
    "error_code",
    "error_class",
    "substituted",
    "original_model",
    "served_model",
    "substitution_reason",
    "attempts",
    "injected",
    "fixture_name",
    "original_account_id",
    "served_account_id",
    "account_id",
    "run_id",
    "tags_json",
    "client_ip",
    "key_fingerprint",
    "reasoning_effort",
    "thinking_budget",
    "subscription_identity",
    "via_dario",
    "dario_generation",
];
const BACKUP_TOOL_CALL_COLS: &[&str] = &[
    "id",
    "harness",
    "session_id",
    "turn_id",
    "tool_call_id",
    "trace_id",
    "tool_name",
    "ts_start_ms",
    "ts_end_ms",
    "is_error",
    "exit_status",
    "args_body_path",
    "result_body_path",
];
const BACKUP_HEARTBEAT_COLS: &[&str] = &[
    "ts_ms",
    "provider",
    "account_id",
    "ok",
    "status",
    "latency_ms",
    "message",
];

#[derive(Debug, Clone, Default, PartialEq)]
pub struct TraceBackupRows {
    pub traces: Vec<Value>,
    pub tool_calls: Vec<Value>,
    pub heartbeats: Vec<Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct TraceImportCounts {
    pub traces_imported: u64,
    pub traces_skipped: u64,
    pub tool_calls_imported: u64,
    pub tool_calls_skipped: u64,
    pub heartbeats_imported: u64,
    pub heartbeats_skipped: u64,
}

fn sqlite_row_json(row: &rusqlite::Row<'_>, columns: &[&str]) -> rusqlite::Result<Value> {
    use rusqlite::types::ValueRef;

    let mut object = serde_json::Map::with_capacity(columns.len());
    for (index, column) in columns.iter().enumerate() {
        let value = match row.get_ref(index)? {
            ValueRef::Null => Value::Null,
            ValueRef::Integer(value) => json!(value),
            ValueRef::Real(value) => json!(value),
            ValueRef::Text(value) => Value::String(String::from_utf8_lossy(value).into_owned()),
            ValueRef::Blob(_) => {
                return Err(rusqlite::Error::InvalidColumnType(
                    index,
                    (*column).to_string(),
                    rusqlite::types::Type::Blob,
                ))
            }
        };
        object.insert((*column).to_string(), value);
    }
    Ok(Value::Object(object))
}

fn export_table_rows(
    conn: &Connection,
    table: &str,
    columns: &[&str],
    order: &str,
) -> Result<Vec<Value>> {
    let sql = format!(
        "SELECT {} FROM {table} ORDER BY {order}",
        columns.join(", ")
    );
    let mut statement = conn.prepare(&sql)?;
    let rows = statement.query_map([], |row| sqlite_row_json(row, columns))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn archive_body_path(data_dir: &Path, value: &mut Value) {
    let Some(path) = value.as_str() else { return };
    let body_root = data_dir.join("bodies");
    if let Ok(relative) = Path::new(path).strip_prefix(&body_root) {
        // Archive paths are always forward-slash so an export is portable
        // across platforms (Windows would otherwise emit `bodies\…`).
        let mut portable = String::from("bodies");
        for component in relative.components() {
            portable.push('/');
            portable.push_str(&component.as_os_str().to_string_lossy());
        }
        *value = Value::String(portable);
    }
}

fn restored_body_path(data_dir: &Path, value: &mut Value) {
    let Some(path) = value.as_str() else { return };
    let path = Path::new(path);
    let mut components = path.components();
    if components.next() != Some(Component::Normal("bodies".as_ref()))
        || !components.all(|component| matches!(component, Component::Normal(_)))
    {
        return;
    }
    *value = Value::String(data_dir.join(path).to_string_lossy().into_owned());
}

fn checked_sql_values(
    row: &Value,
    table: &str,
    columns: &[&str],
) -> Result<Vec<rusqlite::types::Value>> {
    use rusqlite::types::Value as SqlValue;

    let object = row
        .as_object()
        .with_context(|| format!("{table} backup row must be a JSON object"))?;
    if object.len() != columns.len() || columns.iter().any(|column| !object.contains_key(*column)) {
        anyhow::bail!("{table} backup row has an unexpected set of columns");
    }
    columns
        .iter()
        .map(|column| {
            let value = &object[*column];
            match value {
                Value::Null => Ok(SqlValue::Null),
                Value::Bool(value) => Ok(SqlValue::Integer(*value as i64)),
                Value::Number(value) => {
                    if let Some(value) = value.as_i64() {
                        Ok(SqlValue::Integer(value))
                    } else if let Some(value) = value.as_u64() {
                        i64::try_from(value)
                            .map(SqlValue::Integer)
                            .with_context(|| {
                                format!("{table}.{column} is outside SQLite's integer range")
                            })
                    } else {
                        value.as_f64().map(SqlValue::Real).with_context(|| {
                            format!("{table}.{column} is not a finite JSON number")
                        })
                    }
                }
                Value::String(value) => Ok(SqlValue::Text(value.clone())),
                Value::Array(_) | Value::Object(_) => {
                    anyhow::bail!("{table}.{column} must be a JSON scalar")
                }
            }
        })
        .collect()
}

fn trace_row_json(r: &rusqlite::Row) -> rusqlite::Result<Value> {
    let ts_request_ms = r.get::<_, i64>(1)?;
    let ts_response_ms = r.get::<_, Option<i64>>(2)?;
    Ok(json!({
        "id": r.get::<_, String>(0)?,
        "ts_request_ms": ts_request_ms,
        "ts_response_ms": ts_response_ms,
        "harness": r.get::<_, Option<String>>(3)?,
        "client_format": r.get::<_, Option<String>>(4)?,
        "upstream_provider": r.get::<_, Option<String>>(5)?,
        "requested_model": r.get::<_, Option<String>>(6)?,
        "routed_model": r.get::<_, Option<String>>(7)?,
        "status": r.get::<_, Option<i64>>(8)?,
        "streamed": r.get::<_, Option<i64>>(9)?,
        "input_tokens": r.get::<_, Option<i64>>(10)?,
        "cached_input_tokens": r.get::<_, Option<i64>>(11)?,
        "cache_creation_tokens": r.get::<_, Option<i64>>(12)?,
        "output_tokens": r.get::<_, Option<i64>>(13)?,
        "reasoning_tokens": r.get::<_, Option<i64>>(14)?,
        "cost_usd": r.get::<_, Option<f64>>(15)?,
        "billing_bucket": r.get::<_, Option<String>>(16)?,
        "error": r.get::<_, Option<String>>(17)?,
        "session_id": r.get::<_, Option<String>>(18)?,
        "resp_body_path": r.get::<_, Option<String>>(19)?,
        "error_kind": r.get::<_, Option<String>>(20)?,
        "error_code": r.get::<_, Option<String>>(21)?,
        "error_class": r.get::<_, Option<String>>(22)?,
        "substituted": r.get::<_, i64>(23)? != 0,
        "original_model": r.get::<_, Option<String>>(24)?,
        "served_model": r.get::<_, Option<String>>(25)?,
        "substitution_reason": r.get::<_, Option<String>>(26)?,
        "attempts": r.get::<_, Option<String>>(27)?.and_then(|v| serde_json::from_str::<Value>(&v).ok()).unwrap_or(Value::Null),
        "injected": r.get::<_, i64>(28)? != 0,
        "fixture_name": r.get::<_, Option<String>>(29)?,
        "original_account_id": r.get::<_, Option<String>>(30)?,
        "served_account_id": r.get::<_, Option<String>>(31)?,
        "upstream_format": r.get::<_, Option<String>>(32)?,
        "req_body_path": r.get::<_, Option<String>>(33)?,
        "upstream_req_body_path": r.get::<_, Option<String>>(34)?,
        "req_headers_json": r.get::<_, Option<String>>(35)?,
        "resp_headers_json": r.get::<_, Option<String>>(36)?,
        "account_id": r.get::<_, Option<String>>(37)?,
        "run_id": r.get::<_, Option<String>>(38)?,
        "tags_json": r.get::<_, Option<String>>(39)?,
        "client_ip": r.get::<_, Option<String>>(40)?,
        "key_fingerprint": r.get::<_, Option<String>>(41)?,
        "reasoning_effort": r.get::<_, Option<String>>(42)?,
        "thinking_budget": r.get::<_, Option<i64>>(43)?,
        "method": r.get::<_, Option<String>>(44)?,
        "path": r.get::<_, Option<String>>(45)?,
        "subscription_identity": r.get::<_, Option<String>>(46)?,
        "via_dario": r.get::<_, i64>(47)? != 0,
        "dario_generation": r.get::<_, Option<String>>(48)?,
        "latency_ms": ts_response_ms.map(|t| t - ts_request_ms),
    }))
}

fn annotate_trace_accounts(conn: &Connection, rows: &mut [Value]) -> Result<()> {
    for row in rows {
        let identity = row["subscription_identity"].as_str();
        let account_id = row["account_id"].as_str();
        // Prefer an active account sharing the durable identity. That is the
        // automatic re-link after a user re-adds the subscription under a new
        // nickname. Fall back to the original account tombstone.
        let account = if let Some(identity) = identity {
            conn.query_row(
                "SELECT account_id, provider, name, kind, email, removed_ms FROM known_accounts
                 WHERE subscription_identity=?1 AND removed_ms IS NULL ORDER BY last_seen_ms DESC LIMIT 1",
                [identity], account_json_row,
            ).optional()?
        } else { None }.or_else(|| {
            account_id.and_then(|id| conn.query_row(
                "SELECT account_id, provider, name, kind, email, removed_ms FROM known_accounts WHERE account_id=?1",
                [id], account_json_row,
            ).optional().ok().flatten())
        });
        if let Some(account) = account {
            row["account"] = account;
        }
    }
    Ok(())
}

fn account_json_row(r: &rusqlite::Row) -> rusqlite::Result<Value> {
    let removed_ms = r.get::<_, Option<i64>>(5)?;
    Ok(json!({
        "id": r.get::<_, String>(0)?, "provider": r.get::<_, String>(1)?,
        "name": r.get::<_, String>(2)?, "kind": r.get::<_, String>(3)?,
        "email": r.get::<_, Option<String>>(4)?, "removed": removed_ms.is_some(), "removed_ms": removed_ms,
    }))
}

const DEFAULT_SEARCH_LIMIT: usize = 200;
const MAX_SEARCH_LIMIT: usize = 5000;

fn effective_limit(limit: usize) -> usize {
    if limit == 0 {
        DEFAULT_SEARCH_LIMIT
    } else {
        limit.min(MAX_SEARCH_LIMIT)
    }
}

fn migrate_traces(conn: &Connection) -> Result<()> {
    for col in [
        "run_id TEXT",
        "tags_json TEXT",
        "client_ip TEXT",
        "key_fingerprint TEXT",
        "reasoning_effort TEXT",
        "thinking_budget INTEGER",
        "subscription_identity TEXT",
        "via_dario INTEGER NOT NULL DEFAULT 0",
        "dario_generation TEXT",
        "error_kind TEXT",
        "error_code TEXT",
        "error_class TEXT",
        "substituted INTEGER NOT NULL DEFAULT 0",
        "original_model TEXT",
        "served_model TEXT",
        "substitution_reason TEXT",
        "injected INTEGER NOT NULL DEFAULT 0",
        "fixture_name TEXT",
        "attempts TEXT",
        "original_account_id TEXT",
        "served_account_id TEXT",
    ] {
        if let Err(e) = conn.execute_batch(&format!("ALTER TABLE traces ADD COLUMN {col}")) {
            if !e.to_string().contains("duplicate column name") {
                return Err(e.into());
            }
        }
    }
    conn.execute_batch("CREATE INDEX IF NOT EXISTS traces_run ON traces(run_id); CREATE INDEX IF NOT EXISTS traces_subscription_identity ON traces(subscription_identity)")?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownAccount {
    pub account_id: String,
    pub provider: String,
    pub name: String,
    pub kind: String,
    pub subscription_identity: Option<String>,
    pub email: Option<String>,
}

impl KnownAccount {
    pub fn new(
        account_id: impl Into<String>,
        provider: impl Into<String>,
        name: impl Into<String>,
        kind: impl Into<String>,
        subscription_identity: Option<String>,
        email: Option<String>,
    ) -> Self {
        Self {
            account_id: account_id.into(),
            provider: provider.into(),
            name: name.into(),
            kind: kind.into(),
            subscription_identity,
            email,
        }
    }
}

fn migrate_run_keys(conn: &Connection) -> Result<()> {
    if let Err(e) =
        conn.execute_batch("ALTER TABLE run_keys ADD COLUMN kind TEXT NOT NULL DEFAULT 'run'")
    {
        if !e.to_string().contains("duplicate column name") {
            return Err(e.into());
        }
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct TraceFilter {
    pub since_ms: Option<i64>,
    pub until_ms: Option<i64>,
    pub run_id: Option<String>,
    pub session: Option<String>,
    pub model: Option<String>,
    pub provider: Option<String>,
    /// Matches the historical account id and, where available, its durable
    /// subscription identity so a removed account selection remains useful.
    pub account_id: Option<String>,
    pub account_ids: Vec<String>,
    pub path: Option<String>,
    pub harness: Option<String>,
    pub status: Option<i64>,
    pub errors_only: bool,
    pub error_class: Option<String>,
    pub key_fingerprint: Option<String>,
    pub reasoning_effort: Option<String>,
    /// Provider-neutral body text, resolved through the disposable LAR FTS
    /// index. Raw archive bytes remain authoritative.
    pub text: Option<String>,
    pub limit: usize,
}

/// A stable cursor page from one session. Rows are always returned in
/// chronological order, even when the query reads the newest tail or walks
/// backward from an older cursor.
#[derive(Debug, Clone)]
pub struct SessionTracePage {
    pub rows: Vec<Value>,
    pub total_count: usize,
    pub has_more_before: bool,
    pub has_more_after: bool,
    /// Timestamp of the first trace after this page. This is the exclusive
    /// time boundary for unlinked harness tool activity.
    pub next_ts_ms: Option<i64>,
}

/// Normalized harness tool activity. Payload bytes live in `bodies/`, just as
/// trace request and response bytes do, so all existing retention and reset
/// operations apply to tools too.
#[derive(Debug, Clone)]
pub struct ToolCallRecord {
    pub id: String,
    pub harness: String,
    pub session_id: String,
    pub turn_id: Option<String>,
    pub tool_call_id: String,
    pub trace_id: Option<String>,
    pub tool_name: String,
    pub ts_start_ms: i64,
    pub ts_end_ms: Option<i64>,
    pub is_error: Option<bool>,
    pub exit_status: Option<i64>,
    pub args_body_path: Option<String>,
    pub result_body_path: Option<String>,
}

impl Default for TraceFilter {
    fn default() -> Self {
        Self {
            since_ms: None,
            until_ms: None,
            run_id: None,
            session: None,
            model: None,
            provider: None,
            account_id: None,
            account_ids: vec![],
            path: None,
            harness: None,
            status: None,
            errors_only: false,
            error_class: None,
            key_fingerprint: None,
            reasoning_effort: None,
            text: None,
            limit: DEFAULT_SEARCH_LIMIT,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct PruneReport {
    pub bodies_deleted: u64,
    pub bytes_freed: u64,
    pub rows_affected: u64,
    pub rows_deleted: u64,
    pub dirs_removed: u64,
}

/// Rows re-keyed when a duplicate account is merged into a survivor. Every
/// count is a reassignment, never a deletion, except `known_accounts_removed`
/// which drops the now-redundant catalogue row for the dup.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct MergeCounts {
    pub traces_account_id: u64,
    pub traces_served_account_id: u64,
    pub traces_original_account_id: u64,
    pub heartbeats: u64,
    pub known_accounts_removed: u64,
}

fn date_dir_name(name: &str) -> bool {
    name.len() == 10
        && name.bytes().enumerate().all(|(i, b)| match i {
            4 | 7 => b == b'-',
            _ => b.is_ascii_digit(),
        })
}

pub struct Store {
    conn: Mutex<Connection>,
    pub data_dir: PathBuf,
    live_lar: Mutex<live_body_store::LiveLarCoordinator>,
    live_lar_mode: LarBodyStoreMode,
    live_lar_lock_timeout: std::time::Duration,
}

#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct ResetCounts {
    pub traces: u64,
    pub heartbeats: u64,
    pub run_keys: u64,
    pub pricing: u64,
    pub body_files: u64,
    pub body_bytes: u64,
    pub dario_prompt_cache_files: u64,
    pub dario_prompt_cache_bytes: u64,
}

impl Store {
    pub fn open(data_dir: PathBuf) -> Result<Self> {
        Self::open_inner(data_dir, LarBodyStoreConfig::default())
    }

    fn open_inner(data_dir: PathBuf, lar_config: LarBodyStoreConfig) -> Result<Self> {
        let live_lar_mode = lar_config.mode;
        let live_lar_lock_timeout = lar_config.writer_lock_timeout;
        std::fs::create_dir_all(&data_dir)?;
        let db_path = data_dir.join("alexandria.sqlite3");
        let mut conn =
            Connection::open(&db_path).with_context(|| format!("opening sqlite at {db_path:?}"))?;
        // Migrations and account tombstones share the daemon's WAL database;
        // wait for an in-flight writer instead of assuming exclusive startup.
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.execute_batch(SCHEMA)?;
        migrate_traces(&conn)?;
        migrate_run_keys(&conn)?;
        lar_catalog::migrate(&mut conn)?;
        lar_archive_ops::migrate(&conn)?;
        lar_conversation::migrate(&conn)?;
        lar_fts::migrate(&conn)?;
        lar_gc::migrate(&mut conn)?;
        lar_repack::migrate(&mut conn)?;
        seed_pricing(&conn)?;
        let store = Self {
            conn: Mutex::new(conn),
            data_dir,
            live_lar: Mutex::new(live_body_store::LiveLarCoordinator::new(lar_config)?),
            live_lar_mode,
            live_lar_lock_timeout,
        };
        if live_lar_mode != LarBodyStoreMode::Legacy {
            if let Err(error) = store.recover_lar_body_store_orphans() {
                tracing::warn!(
                    "live LAR startup recovery failed; legacy fallbacks remain available: {error:#}"
                );
            }
        }
        Ok(store)
    }

    pub fn pricing_for(&self, model: &str) -> Option<Pricing> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT model, input_per_m, cached_input_per_m, cache_creation_per_m, output_per_m FROM pricing")
            .ok()?;
        let rows: Vec<(String, Pricing)> = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    Pricing {
                        input_per_m: r.get(1)?,
                        cached_input_per_m: r.get(2)?,
                        cache_creation_per_m: r.get(3)?,
                        output_per_m: r.get(4)?,
                    },
                ))
            })
            .ok()?
            .filter_map(|r| r.ok())
            .collect();
        rows.iter()
            .filter(|(key, _)| model.starts_with(key.as_str()))
            .max_by_key(|(key, _)| key.len())
            .map(|(_, p)| p.clone())
    }

    pub fn pricing_models(&self) -> Vec<String> {
        let conn = self.conn.lock().unwrap();
        let Ok(mut stmt) = conn.prepare("SELECT model FROM pricing ORDER BY model") else {
            return vec![];
        };
        stmt.query_map([], |r| r.get::<_, String>(0))
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
    }

    /// Counts the data owned by resettable store categories.  This deliberately
    /// does not include `known_accounts`: trace attribution survives resets.
    pub fn reset_counts(&self) -> Result<ResetCounts> {
        fn body_usage(path: &std::path::Path) -> Result<(u64, u64)> {
            let mut files = 0;
            let mut bytes = 0;
            if !path.exists() {
                return Ok((files, bytes));
            }
            for entry in std::fs::read_dir(path)? {
                let entry = entry?;
                let ty = entry.file_type()?;
                if ty.is_dir() {
                    let (nested_files, nested_bytes) = body_usage(&entry.path())?;
                    files += nested_files;
                    bytes += nested_bytes;
                } else if ty.is_file() {
                    files += 1;
                    bytes += entry.metadata()?.len();
                }
            }
            Ok((files, bytes))
        }

        let (body_files, body_bytes) = body_usage(&self.data_dir.join("bodies"))?;
        let (dario_prompt_cache_files, dario_prompt_cache_bytes) =
            body_usage(&self.data_dir.join("dario-prompt-cache"))?;
        let conn = self.conn.lock().unwrap();
        let count = |table: &str| -> Result<u64> {
            Ok(conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))?)
        };
        Ok(ResetCounts {
            traces: count("traces")?,
            heartbeats: count("heartbeats")?,
            run_keys: conn.query_row(
                "SELECT COUNT(*) FROM run_keys WHERE revoked = 0",
                [],
                |r| r.get(0),
            )?,
            pricing: count("pricing")?,
            body_files,
            body_bytes,
            dario_prompt_cache_files,
            dario_prompt_cache_bytes,
        })
    }

    /// Revokes every still-active key without deleting the audit rows.
    pub fn revoke_all_run_keys(&self) -> Result<u64> {
        let conn = self.conn.lock().unwrap();
        Ok(conn.execute("UPDATE run_keys SET revoked = 1 WHERE revoked = 0", [])? as u64)
    }

    /// Permanently removes revoked run-key audit rows, leaving active keys intact.
    pub fn delete_revoked_run_keys(&self) -> Result<u64> {
        let conn = self.conn.lock().unwrap();
        Ok(conn.execute("DELETE FROM run_keys WHERE revoked = 1", [])? as u64)
    }

    /// Deletes trace rows and heartbeats and removes every Alex-owned captured
    /// body/archive file. External standalone archives are detached from the
    /// local catalog but are never removed. `known_accounts` is intentionally
    /// not touched.
    pub fn clear_traces_and_bodies(&self) -> Result<()> {
        // Writer -> SQLite is the normal capture lock order. Holding the
        // coordinator across reset prevents a new pack publication racing the
        // catalog clear or directory removal.
        let mut live_lar = self.live_lar.lock().unwrap();
        live_lar.reset()?;
        let mut conn = self.conn.lock().unwrap();
        let transaction = conn.transaction()?;
        lar_gc::clear_all_trace_references(&transaction)?;
        transaction.execute_batch(
            "DELETE FROM lar_normalized_entry_refs;
             DELETE FROM lar_normalized_entries_fts;
             DELETE FROM lar_normalized_entries;
             DELETE FROM lar_normalized_index_meta;
             DELETE FROM lar_repack_chunks;
             DELETE FROM lar_repack_runs;
             DELETE FROM lar_gc_candidates;
             DELETE FROM lar_gc_runs;
             DELETE FROM lar_migration_items;
             DELETE FROM lar_migration_jobs;
             DELETE FROM lar_session_revisions;
             DELETE FROM lar_manifest_chunks;
             DELETE FROM lar_file_identities;
             DELETE FROM lar_checkpoints;
             DELETE FROM lar_manifests;
             DELETE FROM lar_chunks;
             DELETE FROM lar_header_block_atoms;
             DELETE FROM lar_header_blocks;
             DELETE FROM lar_header_atoms;
             DELETE FROM lar_files;
             DELETE FROM lar_archive_sets;",
        )?;
        transaction
            .execute_batch("DELETE FROM tool_calls; DELETE FROM traces; DELETE FROM heartbeats;")?;
        transaction.commit()?;
        drop(conn);

        // Files are removed only after every SQLite owner/reference is gone.
        // A failed filesystem cleanup therefore leaves harmless unreferenced
        // bytes which a repeated reset can reclaim; it never leaves a catalog
        // pointer to missing shared content.
        for directory in [self.data_dir.join("bodies"), self.data_dir.join("lar")] {
            if directory.exists() {
                std::fs::remove_dir_all(&directory).with_context(|| {
                    format!("removing captured trace storage at {}", directory.display())
                })?;
            }
        }
        Ok(())
    }

    /// Export the three trace-history tables as lossless JSON values. Body
    /// paths owned by this store are made portable relative to `data_dir`.
    pub fn export_trace_backup_rows(&self) -> Result<TraceBackupRows> {
        let mut conn = self.conn.lock().unwrap();
        let transaction = conn.transaction()?;
        let mut rows = TraceBackupRows {
            traces: export_table_rows(
                &transaction,
                "traces",
                BACKUP_TRACE_COLS,
                "ts_request_ms, id",
            )?,
            tool_calls: export_table_rows(
                &transaction,
                "tool_calls",
                BACKUP_TOOL_CALL_COLS,
                "ts_start_ms, id",
            )?,
            heartbeats: export_table_rows(
                &transaction,
                "heartbeats",
                BACKUP_HEARTBEAT_COLS,
                "ts_ms, provider",
            )?,
        };
        transaction.commit()?;
        for row in &mut rows.traces {
            for column in ["req_body_path", "upstream_req_body_path", "resp_body_path"] {
                archive_body_path(&self.data_dir, &mut row[column]);
            }
        }
        for row in &mut rows.tool_calls {
            for column in ["args_body_path", "result_body_path"] {
                archive_body_path(&self.data_dir, &mut row[column]);
            }
        }
        Ok(rows)
    }

    /// Restore trace-history rows without modifying rows already present.
    /// Trace and tool-call uniqueness is enforced by SQLite; heartbeats use
    /// equality across their complete row because that table has no key.
    pub fn import_trace_backup_rows(&self, rows: &TraceBackupRows) -> Result<TraceImportCounts> {
        let mut rows = rows.clone();
        for row in &mut rows.traces {
            for column in ["req_body_path", "upstream_req_body_path", "resp_body_path"] {
                restored_body_path(&self.data_dir, &mut row[column]);
            }
        }
        for row in &mut rows.tool_calls {
            for column in ["args_body_path", "result_body_path"] {
                restored_body_path(&self.data_dir, &mut row[column]);
            }
        }

        let mut conn = self.conn.lock().unwrap();
        let transaction = conn.transaction()?;
        let mut counts = TraceImportCounts::default();

        let insert_rows = |table: &str,
                           columns: &[&str],
                           source: &[Value],
                           imported: &mut u64,
                           skipped: &mut u64|
         -> Result<()> {
            let placeholders = (1..=columns.len())
                .map(|index| format!("?{index}"))
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "INSERT INTO {table} ({}) VALUES ({placeholders}) ON CONFLICT DO NOTHING",
                columns.join(", ")
            );
            let mut statement = transaction.prepare(&sql)?;
            for row in source {
                let values = checked_sql_values(row, table, columns)?;
                let changed = statement.execute(rusqlite::params_from_iter(values.iter()))?;
                if changed == 0 {
                    *skipped += 1;
                } else {
                    *imported += 1;
                }
            }
            Ok(())
        };

        insert_rows(
            "traces",
            BACKUP_TRACE_COLS,
            &rows.traces,
            &mut counts.traces_imported,
            &mut counts.traces_skipped,
        )?;
        insert_rows(
            "tool_calls",
            BACKUP_TOOL_CALL_COLS,
            &rows.tool_calls,
            &mut counts.tool_calls_imported,
            &mut counts.tool_calls_skipped,
        )?;

        let heartbeat_match = format!(
            "SELECT 1 FROM heartbeats WHERE {} LIMIT 1",
            BACKUP_HEARTBEAT_COLS
                .iter()
                .enumerate()
                .map(|(index, column)| format!("{column} IS ?{}", index + 1))
                .collect::<Vec<_>>()
                .join(" AND ")
        );
        let heartbeat_insert = format!(
            "INSERT INTO heartbeats ({}) VALUES ({})",
            BACKUP_HEARTBEAT_COLS.join(", "),
            (1..=BACKUP_HEARTBEAT_COLS.len())
                .map(|index| format!("?{index}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
        for row in &rows.heartbeats {
            let values = checked_sql_values(row, "heartbeats", BACKUP_HEARTBEAT_COLS)?;
            let exists = transaction
                .query_row(
                    &heartbeat_match,
                    rusqlite::params_from_iter(values.iter()),
                    |_| Ok(()),
                )
                .optional()?
                .is_some();
            if exists {
                counts.heartbeats_skipped += 1;
            } else {
                transaction
                    .execute(&heartbeat_insert, rusqlite::params_from_iter(values.iter()))?;
                counts.heartbeats_imported += 1;
            }
        }
        transaction.commit()?;
        Ok(counts)
    }

    /// Clears learned price data and immediately re-seeds the bundled catalog.
    ///
    /// Re-seeding is not optional. `pricing` is also the model catalog that
    /// `/v1/models` serves and that the harness config writer installs into each
    /// harness. Seeding otherwise only happens on store open, so clearing this on
    /// a *running* daemon left the catalog empty until the next restart -- the
    /// harness injection then silently fell back to a stale hardcoded list and
    /// models such as claude-fable-5 vanished from the harnesses.
    pub fn clear_pricing(&self) -> Result<u64> {
        let conn = self.conn.lock().unwrap();
        let removed = conn.execute("DELETE FROM pricing", [])? as u64;
        seed_pricing(&conn)?;
        Ok(removed)
    }

    /// Clears all derived caches currently persisted by the local store.
    pub fn clear_derived_cache(&self) -> Result<u64> {
        let prompt_cache = self.data_dir.join("dario-prompt-cache");
        if prompt_cache.exists() {
            std::fs::remove_dir_all(&prompt_cache).with_context(|| {
                format!("removing dario prompt cache at {}", prompt_cache.display())
            })?;
        }
        self.clear_pricing()
    }

    pub fn insert_trace(&self, t: &TraceRecord) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        // Some older remote/wrap clients deserialize a TraceRecord without
        // the new field and later update the same trace id. INSERT OR REPLACE
        // must not erase an identity that was already recorded.
        let subscription_identity = match &t.subscription_identity {
            Some(identity) => Some(identity.clone()),
            None => conn
                .query_row(
                    "SELECT subscription_identity FROM traces WHERE id=?1",
                    [&t.id],
                    |r| r.get::<_, Option<String>>(0),
                )
                .optional()?
                .flatten(),
        };
        conn.execute(
            r#"INSERT OR REPLACE INTO traces (
                id, ts_request_ms, ts_response_ms, session_id, harness, client_format,
                upstream_provider, upstream_format, requested_model, routed_model,
                method, path, status, streamed,
                input_tokens, cached_input_tokens, cache_creation_tokens, output_tokens, reasoning_tokens,
                cost_usd, billing_bucket,
                req_body_path, upstream_req_body_path, resp_body_path,
                req_headers_json, resp_headers_json, error, account_id,
                error_kind, error_code, error_class,
                substituted, original_model, served_model, substitution_reason, attempts, injected, fixture_name,
                original_account_id, served_account_id,
                run_id, tags_json, client_ip, key_fingerprint, reasoning_effort, thinking_budget,
                subscription_identity, via_dario, dario_generation
            ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25,?26,?27,?28,?29,?30,?31,?32,?33,?34,?35,?36,?37,?38,?39,?40,?41,?42,?43,?44,?45,?46,?47,?48,?49)"#,
            params![
                t.id,
                t.ts_request_ms,
                t.ts_response_ms,
                t.session_id,
                t.harness,
                t.client_format,
                t.upstream_provider,
                t.upstream_format,
                t.requested_model,
                t.routed_model,
                t.method,
                t.path,
                t.status,
                t.streamed.map(|b| b as i64),
                t.usage.input_tokens,
                t.usage.cached_input_tokens,
                t.usage.cache_creation_tokens,
                t.usage.output_tokens,
                t.usage.reasoning_tokens,
                t.cost_usd,
                t.billing_bucket,
                t.req_body_path,
                t.upstream_req_body_path,
                t.resp_body_path,
                t.req_headers_json,
                t.resp_headers_json,
                t.error,
                t.account_id,
                t.error_kind,
                t.error_code,
                t.error_class,
                t.substituted as i64,
                t.original_model,
                t.served_model,
                t.substitution_reason,
                t.attempts,
                t.injected as i64,
                t.fixture_name,
                t.original_account_id,
                t.served_account_id,
                t.run_id,
                t.tags,
                t.client_ip,
                t.key_fingerprint,
                t.reasoning_effort,
                t.thinking_budget,
                subscription_identity,
                t.via_dario as i64,
                t.dario_generation,
            ],
        )?;
        lar_fts::refresh_trace_anchor(&conn, &t.id, t.session_id.as_deref(), t.ts_request_ms)?;
        Ok(())
    }

    /// Refresh non-payload billing attribution on an already-imported trace.
    /// Optional account fields are additive so an importer that cannot resolve
    /// identity never erases attribution recorded by an earlier pass.
    pub fn update_trace_billing_metadata(
        &self,
        trace_id: &str,
        requested_model: &str,
        routed_model: &str,
        billing_bucket: &str,
        account_id: Option<&str>,
        subscription_identity: Option<&str>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE traces SET requested_model=?2, routed_model=?3, billing_bucket=?4,
               account_id=COALESCE(?5, account_id),
               subscription_identity=COALESCE(?6, subscription_identity)
             WHERE id=?1",
            params![
                trace_id,
                requested_model,
                routed_model,
                billing_bucket,
                account_id,
                subscription_identity
            ],
        )?;
        Ok(())
    }

    /// Record an active account. This is an upsert so it is safe to call on
    /// every routing decision and does not require vault/database exclusivity.
    pub fn upsert_known_account(&self, account: &KnownAccount) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO known_accounts (account_id, provider, name, kind, subscription_identity, email, removed_ms, last_seen_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, ?7)
             ON CONFLICT(account_id) DO UPDATE SET provider=excluded.provider, name=excluded.name,
               kind=excluded.kind, subscription_identity=excluded.subscription_identity,
               email=excluded.email, removed_ms=NULL, last_seen_ms=excluded.last_seen_ms",
            params![account.account_id, account.provider, account.name, account.kind,
                account.subscription_identity, account.email, Utc::now().timestamp_millis()],
        )?;
        Ok(())
    }

    /// Keep account metadata after credential removal. No trace row is deleted
    /// or changed by this operation.
    pub fn tombstone_known_account(&self, account: &KnownAccount, removed_ms: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO known_accounts (account_id, provider, name, kind, subscription_identity, email, removed_ms, last_seen_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(account_id) DO UPDATE SET provider=excluded.provider, name=excluded.name,
               kind=excluded.kind, subscription_identity=excluded.subscription_identity,
               email=excluded.email, removed_ms=excluded.removed_ms, last_seen_ms=excluded.last_seen_ms",
            params![account.account_id, account.provider, account.name, account.kind,
                account.subscription_identity, account.email, removed_ms, removed_ms],
        )?;
        Ok(())
    }

    /// Accounts for the Trace Browser. Removed rows remain present and are
    /// explicitly marked, so callers can offer them as filterable selections.
    pub fn list_known_accounts(&self) -> Result<Vec<Value>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT a.account_id, a.provider, a.name, a.kind, a.subscription_identity, a.email,
                    a.removed_ms, a.last_seen_ms, COUNT(t.id)
             FROM known_accounts a LEFT JOIN traces t ON t.account_id = a.account_id
                OR (a.subscription_identity IS NOT NULL AND t.subscription_identity = a.subscription_identity)
             GROUP BY a.account_id ORDER BY a.removed_ms IS NOT NULL, a.provider, a.name",
        )?;
        let rows = stmt.query_map([], |r| Ok(json!({
            "id": r.get::<_, String>(0)?, "provider": r.get::<_, String>(1)?,
            "name": r.get::<_, String>(2)?, "kind": r.get::<_, String>(3)?,
            "subscription_identity": r.get::<_, Option<String>>(4)?,
            "email": r.get::<_, Option<String>>(5)?, "removed": r.get::<_, Option<i64>>(6)?.is_some(),
            "removed_ms": r.get::<_, Option<i64>>(6)?, "last_seen_ms": r.get::<_, i64>(7)?,
            "trace_count": r.get::<_, i64>(8)?,
        })))?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// Legacy trace groups that cannot currently resolve to an active account.
    pub fn orphaned_trace_groups(&self) -> Result<Vec<Value>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT t.account_id, MAX(t.upstream_provider), GROUP_CONCAT(DISTINCT t.routed_model),
                    MIN(t.ts_request_ms), MAX(t.ts_request_ms), COUNT(*)
             FROM traces t
             WHERE t.account_id IS NOT NULL AND t.subscription_identity IS NULL
               AND NOT EXISTS (SELECT 1 FROM known_accounts a WHERE a.account_id=t.account_id AND a.removed_ms IS NULL)
             GROUP BY t.account_id ORDER BY MAX(t.ts_request_ms) DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(json!({
                "account_id": r.get::<_, String>(0)?, "provider": r.get::<_, Option<String>>(1)?,
                "models": r.get::<_, Option<String>>(2)?, "first_ts_ms": r.get::<_, i64>(3)?,
                "last_ts_ms": r.get::<_, i64>(4)?, "count": r.get::<_, i64>(5)?,
            }))
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// Attach only untagged legacy traces to a selected account identity. The
    /// caller must present the plan first; `confirmed=false` is a strict no-op.
    pub fn reattach_orphaned_traces(
        &self,
        orphan_account_id: &str,
        target: &KnownAccount,
        confirmed: bool,
    ) -> Result<u64> {
        if !confirmed {
            return Ok(0);
        }
        let Some(identity) = target.subscription_identity.as_deref() else {
            anyhow::bail!("target account has no durable subscription identity");
        };
        self.upsert_known_account(target)?;
        let conn = self.conn.lock().unwrap();
        Ok(conn.execute(
            "UPDATE traces SET subscription_identity=?1 WHERE account_id=?2 AND subscription_identity IS NULL",
            params![identity, orphan_account_id],
        )? as u64)
    }

    /// Reassign every trace/heartbeat/catalogue reference from `from_id` to
    /// `into_id` in a single transaction, unifying both histories under the
    /// surviving id. No trace row is ever deleted — request and token history
    /// from both accounts is preserved and simply re-keyed. The dup's
    /// `known_accounts` catalogue row is dropped because its traces now belong
    /// to the survivor (leaving it would double-count via the identity join in
    /// `list_known_accounts`). Idempotent: re-running after a completed merge
    /// reassigns zero rows. The whole operation commits or rolls back atomically.
    pub fn merge_accounts(&self, from_id: &str, into_id: &str) -> Result<MergeCounts> {
        if from_id == into_id {
            anyhow::bail!("cannot merge account '{from_id}' into itself");
        }
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let traces_account_id = tx.execute(
            "UPDATE traces SET account_id=?2 WHERE account_id=?1",
            params![from_id, into_id],
        )? as u64;
        let traces_served_account_id = tx.execute(
            "UPDATE traces SET served_account_id=?2 WHERE served_account_id=?1",
            params![from_id, into_id],
        )? as u64;
        let traces_original_account_id = tx.execute(
            "UPDATE traces SET original_account_id=?2 WHERE original_account_id=?1",
            params![from_id, into_id],
        )? as u64;
        let heartbeats = tx.execute(
            "UPDATE heartbeats SET account_id=?2 WHERE account_id=?1",
            params![from_id, into_id],
        )? as u64;
        let known_accounts_removed = tx.execute(
            "DELETE FROM known_accounts WHERE account_id=?1",
            params![from_id],
        )? as u64;
        tx.commit()?;
        Ok(MergeCounts {
            traces_account_id,
            traces_served_account_id,
            traces_original_account_id,
            heartbeats,
            known_accounts_removed,
        })
    }

    pub fn list_traces(
        &self,
        limit: usize,
        session: Option<&str>,
        model: Option<&str>,
    ) -> Result<Vec<Value>> {
        let filter = TraceFilter {
            session: session.map(String::from),
            model: model.map(String::from),
            limit,
            ..Default::default()
        };
        self.search_traces(&filter)
    }

    pub fn search_traces(&self, f: &TraceFilter) -> Result<Vec<Value>> {
        let conn = self.conn.lock().unwrap();
        let mut sql = format!("SELECT {TRACE_COLS} FROM traces WHERE 1=1");
        let mut args: Vec<String> = vec![];
        if let Some(since) = f.since_ms {
            sql.push_str(" AND ts_request_ms >= ?");
            args.push(since.to_string());
        }
        if let Some(until) = f.until_ms {
            sql.push_str(" AND ts_request_ms <= ?");
            args.push(until.to_string());
        }
        if let Some(r) = &f.run_id {
            sql.push_str(" AND run_id = ?");
            args.push(r.clone());
        }
        if let Some(s) = &f.session {
            sql.push_str(" AND session_id = ?");
            args.push(s.clone());
        }
        if let Some(m) = &f.model {
            sql.push_str(" AND routed_model LIKE ?");
            args.push(format!("%{m}%"));
        }
        if let Some(p) = &f.provider {
            sql.push_str(" AND upstream_provider = ?");
            args.push(p.clone());
        }
        if let Some(account_id) = &f.account_id {
            sql.push_str(" AND (account_id = ? OR subscription_identity = (SELECT subscription_identity FROM known_accounts WHERE account_id = ?))");
            args.push(account_id.clone());
            args.push(account_id.clone());
        }
        if !f.account_ids.is_empty() {
            let placeholders = std::iter::repeat("?")
                .take(f.account_ids.len())
                .collect::<Vec<_>>()
                .join(",");
            sql.push_str(&format!(" AND (account_id IN ({placeholders}) OR subscription_identity IN (SELECT subscription_identity FROM known_accounts WHERE account_id IN ({placeholders})))"));
            args.extend(f.account_ids.iter().cloned());
            args.extend(f.account_ids.iter().cloned());
        }
        if let Some(p) = &f.path {
            sql.push_str(" AND path = ?");
            args.push(p.clone());
        }
        if let Some(h) = &f.harness {
            sql.push_str(" AND harness LIKE ?");
            args.push(format!("%{h}%"));
        }
        if let Some(s) = f.status {
            sql.push_str(" AND status = ?");
            args.push(s.to_string());
        }
        if f.errors_only {
            sql.push_str(" AND error IS NOT NULL");
        }
        if let Some(class) = &f.error_class {
            sql.push_str(" AND error_class = ?");
            args.push(class.clone());
        }
        if let Some(k) = &f.key_fingerprint {
            sql.push_str(" AND key_fingerprint = ?");
            args.push(k.clone());
        }
        if let Some(e) = &f.reasoning_effort {
            sql.push_str(" AND reasoning_effort = ?");
            args.push(e.clone());
        }
        if let Some(text) = f.text.as_deref() {
            let Some(match_query) = lar_fts::fts_match_query(text) else {
                return Ok(Vec::new());
            };
            sql.push_str(
                " AND id IN (
                    SELECT DISTINCT refs.trace_id
                    FROM lar_normalized_entries_fts
                    JOIN lar_normalized_entry_refs refs
                      ON refs.entry_id=lar_normalized_entries_fts.entry_id
                    WHERE lar_normalized_entries_fts MATCH ?
                      AND refs.trace_id IS NOT NULL
                  )",
            );
            args.push(match_query);
        }
        sql.push_str(" ORDER BY ts_request_ms DESC LIMIT ?");
        args.push(effective_limit(f.limit).to_string());
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(args.iter()), trace_row_json)?;
        let mut rows: Vec<Value> = rows.filter_map(|r| r.ok()).collect();
        annotate_trace_accounts(&conn, &mut rows)?;
        Ok(rows)
    }

    pub fn sessions(&self, since_ms: Option<i64>, limit: usize) -> Result<Vec<Value>> {
        let conn = self.conn.lock().unwrap();
        let mut sql = String::from(
            "SELECT session_id, MAX(run_id), MIN(ts_request_ms), MAX(ts_request_ms), COUNT(*),
                    GROUP_CONCAT(DISTINCT routed_model), MAX(harness),
                    COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0),
                    COALESCE(SUM(cost_usd),0.0),
                    COALESCE(SUM(CASE WHEN error IS NOT NULL OR status >= 400 THEN 1 ELSE 0 END),0),
                    (SELECT t2.status FROM traces t2 WHERE t2.session_id = traces.session_id
                     ORDER BY t2.ts_request_ms DESC LIMIT 1),
                    GROUP_CONCAT(tags_json, char(31)),
                    GROUP_CONCAT(DISTINCT reasoning_effort),
                    GROUP_CONCAT(DISTINCT account_id),
                    GROUP_CONCAT(DISTINCT upstream_provider),
                    COALESCE(SUM(CASE WHEN error_class = 'auth' THEN 1 ELSE 0 END),0),
                    COALESCE(SUM(CASE WHEN error_class = 'capacity' THEN 1 ELSE 0 END),0),
                    COALESCE(SUM(CASE WHEN error_class = 'bad_request' THEN 1 ELSE 0 END),0),
                    COALESCE(SUM(CASE WHEN error_class = 'server' THEN 1 ELSE 0 END),0),
                    COALESCE(SUM(CASE WHEN error_class = 'client_disconnect' THEN 1 ELSE 0 END),0),
                    COALESCE(SUM(CASE WHEN error_class = 'network' THEN 1 ELSE 0 END),0),
                    COALESCE(SUM(CASE WHEN error_class = 'other' THEN 1 ELSE 0 END),0)
             FROM traces WHERE session_id IS NOT NULL",
        );
        let mut args: Vec<String> = vec![];
        if let Some(since) = since_ms {
            sql.push_str(" AND ts_request_ms >= ?");
            args.push(since.to_string());
        }
        sql.push_str(" GROUP BY session_id ORDER BY MAX(ts_request_ms) DESC LIMIT ?");
        let limit = if limit == 0 {
            DEFAULT_SEARCH_LIMIT
        } else {
            limit.min(1000)
        };
        args.push(limit.to_string());
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(args.iter()), |r| {
            let models: Vec<String> = r
                .get::<_, Option<String>>(5)?
                .map(|s| s.split(',').map(str::to_string).collect())
                .unwrap_or_default();
            let mut tags = serde_json::Map::new();
            if let Some(raw) = r.get::<_, Option<String>>(12)? {
                for piece in raw.split('\u{1f}') {
                    if let Ok(Value::Object(o)) = serde_json::from_str::<Value>(piece) {
                        tags.extend(o);
                    }
                }
            }
            let efforts: Vec<String> = r
                .get::<_, Option<String>>(13)?
                .map(|s| s.split(',').map(str::to_string).collect())
                .unwrap_or_default();
            let account_ids: Vec<String> = r
                .get::<_, Option<String>>(14)?
                .map(|s| s.split(',').map(str::to_string).collect())
                .unwrap_or_default();
            let providers: Vec<String> = r
                .get::<_, Option<String>>(15)?
                .map(|s| s.split(',').map(str::to_string).collect())
                .unwrap_or_default();
            let error_class_counts: serde_json::Map<String, Value> = [
                ("auth", 17),
                ("capacity", 18),
                ("bad_request", 19),
                ("server", 20),
                ("client_disconnect", 21),
                ("network", 22),
                ("other", 23),
            ]
            .into_iter()
            .filter_map(|(class, index)| {
                let count = r.get::<_, i64>(index).ok()?;
                (count > 0).then(|| (class.to_string(), json!(count)))
            })
            .collect();
            Ok(json!({
                "session_id": r.get::<_, String>(0)?,
                "run_id": r.get::<_, Option<String>>(1)?,
                "first_ts_ms": r.get::<_, Option<i64>>(2)?,
                "last_ts_ms": r.get::<_, Option<i64>>(3)?,
                "trace_count": r.get::<_, i64>(4)?,
                "models": models,
                "providers": providers,
                "harness": r.get::<_, Option<String>>(6)?,
                "total_input_tokens": r.get::<_, i64>(7)?,
                "total_output_tokens": r.get::<_, i64>(8)?,
                "total_cost_usd": r.get::<_, f64>(9)?,
                "errors": r.get::<_, i64>(10)?,
                "last_status": r.get::<_, Option<i64>>(11)?,
                "tags": tags,
                "efforts": efforts,
                "account_ids": account_ids,
                "error_class_counts": error_class_counts,
            }))
        })?;
        let mut rows: Vec<Value> = rows.filter_map(|r| r.ok()).collect();
        drop(stmt);
        for row in &mut rows {
            let Some(session_id) = row["session_id"].as_str().map(String::from) else {
                continue;
            };
            let harness = row["harness"].as_str().map(String::from);
            let lineage = conn
                .query_row(
                    "SELECT parent_session_id, turn_id, agent_type, started_ms, stopped_ms
                     FROM session_lineage
                     WHERE child_session_id = ?1 AND (?2 IS NULL OR harness = ?2)
                     ORDER BY started_ms DESC LIMIT 1",
                    params![session_id, harness],
                    |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, Option<String>>(1)?,
                            r.get::<_, Option<String>>(2)?,
                            r.get::<_, i64>(3)?,
                            r.get::<_, Option<i64>>(4)?,
                        ))
                    },
                )
                .optional()?;
            if let Some((parent, turn_id, agent_type, started_ms, stopped_ms)) = lineage {
                row["parent_session_id"] = json!(parent);
                row["lineage_turn_id"] = json!(turn_id);
                row["agent_type"] = json!(agent_type);
                row["subagent_started_ms"] = json!(started_ms);
                row["subagent_stopped_ms"] = json!(stopped_ms);
            }
            let child_count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM session_lineage
                 WHERE parent_session_id = ?1 AND (?2 IS NULL OR harness = ?2)",
                params![session_id, harness],
                |r| r.get(0),
            )?;
            row["child_count"] = json!(child_count);
            if let Some(display) = session_display_fields(row).as_object() {
                for (key, value) in display {
                    row[key] = value.clone();
                }
            }
        }
        Ok(rows)
    }

    /// Record a normalized lifecycle event. Returns true when it created or
    /// updated a durable parent/child edge.
    pub fn record_harness_event(
        &self,
        harness: &str,
        event: &Value,
        received_ms: i64,
    ) -> Result<bool> {
        let event_name = event["hook_event_name"]
            .as_str()
            .or_else(|| event["hookEventName"].as_str())
            .unwrap_or_default();
        if !matches!(event_name, "SubagentStart" | "SubagentStop") {
            return Ok(false);
        }
        let Some(parent) = event["session_id"].as_str().filter(|id| !id.is_empty()) else {
            return Ok(false);
        };
        let Some(child) = event["agent_id"].as_str().filter(|id| !id.is_empty()) else {
            return Ok(false);
        };
        // Hooks provide their own event clock.  Retain it when present so a
        // start/stop pair remains meaningful even if delivery is delayed.
        let event_ms = event["timestamp_ms"]
            .as_i64()
            .filter(|timestamp_ms| *timestamp_ms > 0)
            .unwrap_or(received_ms);
        let turn_id = event["turn_id"].as_str();
        let agent_type = event["agent_type"].as_str();
        let stopped_ms = (event_name == "SubagentStop").then_some(event_ms);
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO session_lineage
                (harness, child_session_id, parent_session_id, turn_id, agent_type,
                 started_ms, stopped_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(harness, child_session_id) DO UPDATE SET
                parent_session_id = excluded.parent_session_id,
                turn_id = COALESCE(excluded.turn_id, session_lineage.turn_id),
                agent_type = COALESCE(excluded.agent_type, session_lineage.agent_type),
                started_ms = MIN(session_lineage.started_ms, excluded.started_ms),
                stopped_ms = COALESCE(excluded.stopped_ms, session_lineage.stopped_ms)",
            params![harness, child, parent, turn_id, agent_type, event_ms, stopped_ms],
        )?;
        Ok(true)
    }

    /// Resolve a session to the root of its verified harness lineage. Cycles
    /// and pathological depth are bounded defensively.
    pub fn session_lineage_root(&self, harness: &str, session_id: &str) -> Result<String> {
        let conn = self.conn.lock().unwrap();
        let mut current = session_id.to_string();
        let mut seen = std::collections::HashSet::new();
        for _ in 0..32 {
            if !seen.insert(current.clone()) {
                break;
            }
            let parent = conn
                .query_row(
                    "SELECT parent_session_id FROM session_lineage
                     WHERE harness = ?1 AND child_session_id = ?2",
                    params![harness, current],
                    |r| r.get::<_, String>(0),
                )
                .optional()?;
            let Some(parent) = parent else { break };
            current = parent;
        }
        Ok(current)
    }

    pub fn session_traces(&self, session_id: &str, since_ms: Option<i64>) -> Result<Vec<Value>> {
        let conn = self.conn.lock().unwrap();
        let mut sql = format!("SELECT {TRACE_COLS} FROM traces WHERE session_id = ?");
        let mut args = vec![session_id.to_string()];
        if let Some(since) = since_ms {
            sql.push_str(" AND ts_request_ms >= ?");
            args.push(since.to_string());
        }
        sql.push_str(" ORDER BY ts_request_ms ASC");
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(args.iter()), trace_row_json)?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// Read a bounded chronological page without materializing and sorting an
    /// entire long-running session. Cursors are `(ts_request_ms, trace_id)` so
    /// simultaneous traces cannot be skipped or duplicated at page boundaries.
    pub fn session_traces_page(
        &self,
        session_id: &str,
        after: Option<(i64, String)>,
        before: Option<(i64, String)>,
        limit: usize,
        tail: bool,
    ) -> Result<SessionTracePage> {
        if after.is_some() && before.is_some() {
            anyhow::bail!("session trace page cannot use both after and before cursors");
        }
        let limit = limit.clamp(1, 500);
        let conn = self.conn.lock().unwrap();
        let mut sql = format!("SELECT {TRACE_COLS} FROM traces WHERE session_id = ?");
        let mut args = vec![session_id.to_string()];
        let descending = before.is_some() || (after.is_none() && tail);
        if let Some((ts, id)) = &after {
            sql.push_str(" AND (ts_request_ms > ? OR (ts_request_ms = ? AND id > ?))");
            args.extend([ts.to_string(), ts.to_string(), id.clone()]);
        } else if let Some((ts, id)) = &before {
            sql.push_str(" AND (ts_request_ms < ? OR (ts_request_ms = ? AND id < ?))");
            args.extend([ts.to_string(), ts.to_string(), id.clone()]);
        }
        sql.push_str(if descending {
            " ORDER BY ts_request_ms DESC, id DESC LIMIT ?"
        } else {
            " ORDER BY ts_request_ms ASC, id ASC LIMIT ?"
        });
        args.push(limit.to_string());
        let mut stmt = conn.prepare(&sql)?;
        let mapped = stmt.query_map(rusqlite::params_from_iter(args.iter()), trace_row_json)?;
        let mut rows: Vec<Value> = mapped.filter_map(|row| row.ok()).collect();
        if descending {
            rows.reverse();
        }

        let total_count = conn.query_row(
            "SELECT COUNT(*) FROM traces WHERE session_id=?1",
            params![session_id],
            |row| row.get::<_, i64>(0),
        )? as usize;
        let cursor = |row: &Value| {
            (
                row["ts_request_ms"].as_i64().unwrap_or_default(),
                row["id"].as_str().unwrap_or_default().to_string(),
            )
        };
        let has_more_before = if let Some(first) = rows.first() {
            let (ts, id) = cursor(first);
            conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM traces WHERE session_id=?1
                 AND (ts_request_ms < ?2 OR (ts_request_ms = ?2 AND id < ?3)))",
                params![session_id, ts, id],
                |row| row.get::<_, i64>(0),
            )? != 0
        } else {
            false
        };
        let next_ts_ms = if let Some(last) = rows.last() {
            let (ts, id) = cursor(last);
            conn.query_row(
                "SELECT ts_request_ms FROM traces WHERE session_id=?1
                 AND (ts_request_ms > ?2 OR (ts_request_ms = ?2 AND id > ?3))
                 ORDER BY ts_request_ms ASC, id ASC LIMIT 1",
                params![session_id, ts, id],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
        } else {
            None
        };
        Ok(SessionTracePage {
            rows,
            total_count,
            has_more_before,
            has_more_after: next_ts_ms.is_some(),
            next_ts_ms,
        })
    }

    pub fn get_trace(&self, id: &str) -> Result<Option<Value>> {
        let conn = self.conn.lock().unwrap();
        let row = conn
            .query_row(
                &format!("SELECT {TRACE_COLS} FROM traces WHERE id = ?1"),
                params![id],
                trace_row_json,
            )
            .optional()?;
        Ok(row)
    }

    pub fn delete_trace(&self, id: &str) -> Result<Vec<String>> {
        let mut conn = self.conn.lock().unwrap();
        let transaction = conn.transaction()?;
        let paths: Option<(Option<String>, Option<String>, Option<String>)> = transaction
            .query_row(
                "SELECT req_body_path, upstream_req_body_path, resp_body_path
                 FROM traces WHERE id = ?1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let Some((req, upstream, resp)) = paths else {
            anyhow::bail!("trace not found: {id}");
        };
        lar_gc::delete_trace_references(&transaction, id)?;
        transaction.execute("DELETE FROM traces WHERE id = ?1", params![id])?;
        transaction.commit()?;
        Ok([req, upstream, resp].into_iter().flatten().collect())
    }

    pub fn upsert_tool_call(&self, tool: &ToolCallRecord) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO tool_calls (id, harness, session_id, turn_id, tool_call_id, trace_id, tool_name, ts_start_ms, ts_end_ms, is_error, exit_status, args_body_path, result_body_path)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
             ON CONFLICT(harness, session_id, tool_call_id) DO UPDATE SET
               turn_id=COALESCE(excluded.turn_id, tool_calls.turn_id),
               trace_id=COALESCE(excluded.trace_id, tool_calls.trace_id),
               tool_name=excluded.tool_name,
               ts_start_ms=MIN(tool_calls.ts_start_ms, excluded.ts_start_ms),
               ts_end_ms=COALESCE(excluded.ts_end_ms, tool_calls.ts_end_ms),
               is_error=COALESCE(excluded.is_error, tool_calls.is_error),
               exit_status=COALESCE(excluded.exit_status, tool_calls.exit_status),
               args_body_path=COALESCE(excluded.args_body_path, tool_calls.args_body_path),
               result_body_path=COALESCE(excluded.result_body_path, tool_calls.result_body_path)",
            params![tool.id, tool.harness, tool.session_id, tool.turn_id, tool.tool_call_id,
                tool.trace_id, tool.tool_name, tool.ts_start_ms, tool.ts_end_ms,
                tool.is_error.map(|v| v as i64), tool.exit_status, tool.args_body_path,
                tool.result_body_path],
        )?;
        lar_fts::refresh_tool_anchor(&conn, &tool.id)?;
        Ok(())
    }

    pub fn session_tool_calls(&self, session_id: &str) -> Result<Vec<Value>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, session_id, turn_id, tool_call_id, trace_id, tool_name, ts_start_ms,
                    ts_end_ms, is_error, exit_status, args_body_path, result_body_path
             FROM tool_calls WHERE session_id=?1 ORDER BY ts_start_ms ASC",
        )?;
        let rows = stmt.query_map(params![session_id], |r| {
            Ok(json!({
                "id": r.get::<_, String>(0)?, "session_id": r.get::<_, String>(1)?,
                "turn_id": r.get::<_, Option<String>>(2)?, "tool_call_id": r.get::<_, String>(3)?,
                "trace_id": r.get::<_, Option<String>>(4)?, "tool_name": r.get::<_, String>(5)?,
                "ts_start_ms": r.get::<_, i64>(6)?, "ts_end_ms": r.get::<_, Option<i64>>(7)?,
                "is_error": r.get::<_, Option<i64>>(8)?.map(|v| v != 0),
                "exit_status": r.get::<_, Option<i64>>(9)?,
                "args_body_path": r.get::<_, Option<String>>(10)?,
                "result_body_path": r.get::<_, Option<String>>(11)?,
            }))
        })?;
        Ok(rows.filter_map(|row| row.ok()).collect())
    }

    /// Load the tool activity needed to render one trace page. Explicit
    /// trace links always win even if the tool starts after the following
    /// request; unlinked activity is restricted to this page's time window.
    pub fn session_tool_calls_page(
        &self,
        session_id: &str,
        trace_ids: &[String],
        start_ms: i64,
        end_before_ms: Option<i64>,
    ) -> Result<Vec<Value>> {
        use rusqlite::types::Value as SqlValue;

        let conn = self.conn.lock().unwrap();
        let mut sql = String::from(
            "SELECT id, session_id, turn_id, tool_call_id, trace_id, tool_name, ts_start_ms,
                    ts_end_ms, is_error, exit_status, args_body_path, result_body_path
             FROM tool_calls WHERE session_id=? AND
             ((trace_id IS NULL AND ts_start_ms>=?",
        );
        let mut args = vec![
            SqlValue::Text(session_id.to_string()),
            SqlValue::Integer(start_ms),
        ];
        if let Some(end) = end_before_ms {
            sql.push_str(" AND ts_start_ms<?");
            args.push(SqlValue::Integer(end));
        }
        sql.push(')');
        if !trace_ids.is_empty() {
            sql.push_str(" OR trace_id IN (");
            sql.push_str(&vec!["?"; trace_ids.len()].join(","));
            sql.push(')');
            args.extend(trace_ids.iter().cloned().map(SqlValue::Text));
        }
        sql.push_str(") ORDER BY ts_start_ms ASC");
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(args.iter()), |r| {
            Ok(json!({
                "id": r.get::<_, String>(0)?, "session_id": r.get::<_, String>(1)?,
                "turn_id": r.get::<_, Option<String>>(2)?, "tool_call_id": r.get::<_, String>(3)?,
                "trace_id": r.get::<_, Option<String>>(4)?, "tool_name": r.get::<_, String>(5)?,
                "ts_start_ms": r.get::<_, i64>(6)?, "ts_end_ms": r.get::<_, Option<i64>>(7)?,
                "is_error": r.get::<_, Option<i64>>(8)?.map(|v| v != 0),
                "exit_status": r.get::<_, Option<i64>>(9)?,
                "args_body_path": r.get::<_, Option<String>>(10)?,
                "result_body_path": r.get::<_, Option<String>>(11)?,
            }))
        })?;
        Ok(rows.filter_map(|row| row.ok()).collect())
    }

    pub fn get_tool_call(&self, id: &str) -> Result<Option<Value>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, args_body_path, result_body_path FROM tool_calls WHERE id=?1", params![id],
            |r| Ok(json!({"id": r.get::<_, String>(0)?, "args_body_path": r.get::<_, Option<String>>(1)?, "result_body_path": r.get::<_, Option<String>>(2)?})),
        ).optional().map_err(Into::into)
    }

    pub fn run_summary(&self, run_id: &str) -> Result<Value> {
        let conn = self.conn.lock().unwrap();
        #[allow(clippy::type_complexity)]
        let (
            trace_count,
            first_ts_ms,
            last_ts_ms,
            last_response_ms,
            pending,
            total_input,
            total_output,
            total_cost,
            errors,
        ) = conn.query_row(
            "SELECT COUNT(*), MIN(ts_request_ms), MAX(ts_request_ms), MAX(ts_response_ms),
                    COALESCE(SUM(CASE WHEN ts_response_ms IS NULL THEN 1 ELSE 0 END),0),
                    COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0),
                    COALESCE(SUM(cost_usd),0.0),
                    COALESCE(SUM(CASE WHEN error IS NOT NULL THEN 1 ELSE 0 END),0)
             FROM traces WHERE run_id = ?1",
            params![run_id],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, Option<i64>>(1)?,
                    r.get::<_, Option<i64>>(2)?,
                    r.get::<_, Option<i64>>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, i64>(5)?,
                    r.get::<_, i64>(6)?,
                    r.get::<_, f64>(7)?,
                    r.get::<_, i64>(8)?,
                ))
            },
        )?;
        let mut status_counts = serde_json::Map::new();
        let mut stmt =
            conn.prepare("SELECT status, COUNT(*) FROM traces WHERE run_id = ?1 GROUP BY status")?;
        let pairs = stmt.query_map(params![run_id], |r| {
            Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, i64>(1)?))
        })?;
        for pair in pairs.flatten() {
            let key = pair
                .0
                .map(|s| s.to_string())
                .unwrap_or_else(|| "none".into());
            status_counts.insert(key, json!(pair.1));
        }
        let distinct = |col: &str| -> Result<Vec<String>> {
            let mut stmt = conn.prepare(&format!(
                "SELECT DISTINCT {col} FROM traces WHERE run_id = ?1 AND {col} IS NOT NULL ORDER BY {col}"
            ))?;
            let vals = stmt
                .query_map(params![run_id], |r| r.get::<_, String>(0))?
                .filter_map(|r| r.ok())
                .collect();
            Ok(vals)
        };
        let models = distinct("routed_model")?;
        let providers = distinct("upstream_provider")?;
        let mut tags = serde_json::Map::new();
        let mut stmt = conn.prepare(
            "SELECT tags_json FROM traces WHERE run_id = ?1 AND tags_json IS NOT NULL ORDER BY ts_request_ms",
        )?;
        let tag_rows = stmt.query_map(params![run_id], |r| r.get::<_, String>(0))?;
        for raw in tag_rows.flatten() {
            if let Ok(Value::Object(o)) = serde_json::from_str::<Value>(&raw) {
                tags.extend(o);
            }
        }
        Ok(json!({
            "run_id": run_id,
            "trace_count": trace_count,
            "first_ts_ms": first_ts_ms,
            "last_ts_ms": last_ts_ms,
            "last_request_ms": last_ts_ms,
            "last_response_ms": last_response_ms,
            "last_activity_ms": last_response_ms.max(last_ts_ms),
            "pending": pending,
            "status_counts": status_counts,
            "models": models,
            "providers": providers,
            "total_input_tokens": total_input,
            "total_output_tokens": total_output,
            "total_cost_usd": total_cost,
            "tags": tags,
            "errors": errors,
        }))
    }

    pub fn run_artifacts(&self, run_id: &str) -> Result<Vec<Value>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, req_body_path, upstream_req_body_path, resp_body_path
             FROM traces WHERE run_id = ?1 ORDER BY ts_request_ms",
        )?;
        let rows: Vec<(String, Option<String>, Option<String>, Option<String>)> = stmt
            .query_map(params![run_id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })?
            .filter_map(|r| r.ok())
            .collect();
        let mut out = Vec::new();
        for (trace_id, req, upstream_req, resp) in rows {
            for (kind, path) in [
                ("request", req),
                ("upstream-request", upstream_req),
                ("response", resp),
            ] {
                let Some(path) = path else { continue };
                let size_bytes = std::fs::metadata(&path).ok().map(|m| m.len());
                out.push(json!({
                    "trace_id": trace_id,
                    "kind": kind,
                    "path": path,
                    "exists": size_bytes.is_some(),
                    "size_bytes": size_bytes,
                }));
            }
        }
        Ok(out)
    }

    pub fn run_trace_ids(&self, run_id: &str) -> Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT id FROM traces WHERE run_id = ?1 ORDER BY ts_request_ms, id")?;
        let rows = stmt.query_map(params![run_id], |r| r.get::<_, String>(0))?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn insert_heartbeat(
        &self,
        ts_ms: i64,
        provider: &str,
        account_id: Option<&str>,
        ok: bool,
        status: Option<i64>,
        latency_ms: i64,
        message: &str,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO heartbeats (ts_ms, provider, account_id, ok, status, latency_ms, message)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![ts_ms, provider, account_id, ok as i64, status, latency_ms, message],
        )?;
        Ok(())
    }

    pub fn last_heartbeats(&self) -> Result<Vec<Value>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT h.ts_ms, h.provider, h.account_id, h.ok, h.status, h.latency_ms, h.message
             FROM heartbeats h
             JOIN (SELECT provider, MAX(ts_ms) AS ts FROM heartbeats GROUP BY provider) latest
               ON h.provider = latest.provider AND h.ts_ms = latest.ts",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(json!({
                "ts_ms": r.get::<_, i64>(0)?,
                "provider": r.get::<_, String>(1)?,
                "account_id": r.get::<_, Option<String>>(2)?,
                "ok": r.get::<_, i64>(3)? == 1,
                "status": r.get::<_, Option<i64>>(4)?,
                "latency_ms": r.get::<_, i64>(5)?,
                "message": r.get::<_, String>(6)?,
            }))
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn latest_provider_headers(&self) -> Result<Vec<(String, i64, String)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT t.upstream_provider, t.ts_request_ms, t.resp_headers_json
             FROM traces t
             JOIN (SELECT upstream_provider p, MAX(ts_request_ms) ts FROM traces
                   WHERE status >= 200 AND status < 300
                     AND resp_headers_json IS NOT NULL AND upstream_provider IS NOT NULL
                   GROUP BY upstream_provider) latest
               ON t.upstream_provider = latest.p AND t.ts_request_ms = latest.ts
             WHERE t.resp_headers_json IS NOT NULL",
        )?;
        let rows: Vec<(String, i64, String)> = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?
            .filter_map(|r| r.ok())
            .collect();
        let mut seen = std::collections::HashMap::new();
        for row in rows {
            seen.entry(row.0.clone()).or_insert(row);
        }
        Ok(seen.into_values().collect())
    }

    pub fn analytics(&self, since_ms: i64) -> Result<Value> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT routed_model, upstream_provider, billing_bucket, COUNT(*),
                    COALESCE(SUM(input_tokens),0), COALESCE(SUM(cached_input_tokens),0),
                    COALESCE(SUM(output_tokens),0), COALESCE(SUM(cost_usd),0.0),
                    SUM(CASE WHEN status >= 200 AND status < 300 THEN 0 ELSE 1 END),
                    AVG(CASE WHEN ts_response_ms IS NOT NULL THEN ts_response_ms - ts_request_ms END)
             FROM traces WHERE ts_request_ms >= ?1
             GROUP BY routed_model, upstream_provider, billing_bucket
             ORDER BY SUM(cost_usd) DESC",
        )?;
        let rows: Vec<Value> = stmt
            .query_map(params![since_ms], |r| {
                Ok(json!({
                    "routed_model": r.get::<_, Option<String>>(0)?,
                    "upstream_provider": r.get::<_, Option<String>>(1)?,
                    "billing_bucket": r.get::<_, Option<String>>(2)?,
                    "requests": r.get::<_, i64>(3)?,
                    "input_tokens": r.get::<_, i64>(4)?,
                    "cached_input_tokens": r.get::<_, i64>(5)?,
                    "output_tokens": r.get::<_, i64>(6)?,
                    "cost_usd": r.get::<_, f64>(7)?,
                    "errors": r.get::<_, Option<i64>>(8)?,
                    "avg_latency_ms": r.get::<_, Option<f64>>(9)?,
                }))
            })?
            .filter_map(|r| r.ok())
            .collect();
        let (mut requests, mut cost, mut errors) = (0i64, 0f64, 0i64);
        let mut buckets: std::collections::HashMap<String, f64> = Default::default();
        for row in &rows {
            requests += row["requests"].as_i64().unwrap_or(0);
            cost += row["cost_usd"].as_f64().unwrap_or(0.0);
            errors += row["errors"].as_i64().unwrap_or(0);
            let bucket = row["billing_bucket"]
                .as_str()
                .unwrap_or("unknown")
                .to_string();
            *buckets.entry(bucket).or_default() += row["cost_usd"].as_f64().unwrap_or(0.0);
        }
        Ok(json!({
            "since_ms": since_ms,
            "totals": {"requests": requests, "cost_usd": cost, "errors": errors, "cost_by_bucket": buckets},
            "by_model": rows,
        }))
    }

    pub fn account_analytics(&self, since_ms: i64, bucket_ms: i64) -> Result<Value> {
        let conn = self.conn.lock().unwrap();
        let mut accounts = conn.prepare(
            "SELECT account_id, upstream_provider, COUNT(*),
                    COALESCE(SUM(input_tokens), 0), COALESCE(SUM(output_tokens), 0),
                    COALESCE(SUM(cost_usd), 0.0),
                    SUM(CASE WHEN status >= 200 AND status < 300 THEN 0 ELSE 1 END),
                    MAX(ts_request_ms)
             FROM traces
             WHERE ts_request_ms >= ?1 AND account_id IS NOT NULL
             GROUP BY account_id, upstream_provider
             ORDER BY MAX(ts_request_ms) DESC",
        )?;
        let by_account: Vec<Value> = accounts
            .query_map(params![since_ms], |r| {
                Ok(json!({
                    "account_id": r.get::<_, String>(0)?,
                    "provider": r.get::<_, Option<String>>(1)?,
                    "requests": r.get::<_, i64>(2)?,
                    "input_tokens": r.get::<_, i64>(3)?,
                    "output_tokens": r.get::<_, i64>(4)?,
                    "cost_usd": r.get::<_, f64>(5)?,
                    "errors": r.get::<_, Option<i64>>(6)?,
                    "last_ts_ms": r.get::<_, Option<i64>>(7)?,
                }))
            })?
            .filter_map(|r| r.ok())
            .collect();

        let mut series = conn.prepare(
            "SELECT ((ts_request_ms / ?2) * ?2) AS bucket_ms, account_id,
                    COUNT(*), COALESCE(SUM(input_tokens), 0), COALESCE(SUM(output_tokens), 0),
                    COALESCE(SUM(cost_usd), 0.0),
                    SUM(CASE WHEN status >= 200 AND status < 300 THEN 0 ELSE 1 END)
             FROM traces
             WHERE ts_request_ms >= ?1 AND account_id IS NOT NULL
             GROUP BY bucket_ms, account_id
             ORDER BY bucket_ms ASC, account_id ASC",
        )?;
        let series: Vec<Value> = series
            .query_map(params![since_ms, bucket_ms.max(60_000)], |r| {
                Ok(json!({
                    "bucket_ms": r.get::<_, i64>(0)?,
                    "account_id": r.get::<_, String>(1)?,
                    "requests": r.get::<_, i64>(2)?,
                    "input_tokens": r.get::<_, i64>(3)?,
                    "output_tokens": r.get::<_, i64>(4)?,
                    "cost_usd": r.get::<_, f64>(5)?,
                    "errors": r.get::<_, Option<i64>>(6)?,
                }))
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(
            json!({"since_ms": since_ms, "bucket_ms": bucket_ms.max(60_000), "by_account": by_account, "series": series}),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn insert_run_key(
        &self,
        id: &str,
        key_hash: &str,
        kind: &str,
        run_id: Option<&str>,
        tags_json: Option<&str>,
        label: Option<&str>,
        created_ms: i64,
        expires_ms: Option<i64>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO run_keys (id, key_hash, kind, run_id, tags_json, label, created_ms, expires_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![id, key_hash, kind, run_id, tags_json, label, created_ms, expires_ms],
        )?;
        Ok(())
    }

    pub fn lookup_run_key(&self, key_hash: &str, now_ms: i64) -> Result<Option<Value>> {
        let conn = self.conn.lock().unwrap();
        let row = conn
            .query_row(
                &format!(
                    "SELECT {RUN_KEY_COLS} FROM run_keys
                     WHERE key_hash = ?1 AND revoked = 0
                       AND (expires_ms IS NULL OR expires_ms > ?2)"
                ),
                params![key_hash, now_ms],
                run_key_row_json,
            )
            .optional()?;
        Ok(row)
    }

    pub fn touch_run_key(&self, key_hash: &str, now_ms: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE run_keys SET use_count = use_count + 1, last_used_ms = ?2 WHERE key_hash = ?1",
            params![key_hash, now_ms],
        )?;
        Ok(())
    }

    pub fn list_run_keys(&self, include_inactive: bool) -> Result<Vec<Value>> {
        let conn = self.conn.lock().unwrap();
        let mut sql = format!("SELECT {RUN_KEY_COLS} FROM run_keys");
        let mut args: Vec<String> = vec![];
        if !include_inactive {
            sql.push_str(" WHERE revoked = 0 AND (expires_ms IS NULL OR expires_ms > ?1)");
            args.push(Utc::now().timestamp_millis().to_string());
        }
        sql.push_str(" ORDER BY created_ms DESC");
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(args.iter()), run_key_row_json)?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn revoke_run_key(&self, id_or_prefix: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "UPDATE run_keys SET revoked = 1 WHERE id = ?1 OR id LIKE ?1 || '%'",
            params![id_or_prefix],
        )?;
        Ok(changed > 0)
    }

    pub fn prune(
        &self,
        older_than_ms: i64,
        bodies_only: bool,
        dry_run: bool,
    ) -> Result<PruneReport> {
        let mut report = PruneReport::default();
        // Tool payloads share `bodies/`, so include them in the existing
        // retention pass rather than creating a parallel retention policy.
        let tool_rows: Vec<(String, Option<String>, Option<String>)> = {
            let conn = self.conn.lock().unwrap();
            let mut stmt = conn.prepare(
                "SELECT id, args_body_path, result_body_path FROM tool_calls
                 WHERE ts_start_ms < ?1 AND (args_body_path IS NOT NULL OR result_body_path IS NOT NULL)",
            )?;
            let rows = stmt
                .query_map(params![older_than_ms], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                })?
                .filter_map(|r| r.ok())
                .collect();
            rows
        };
        for (id, args, result) in tool_rows {
            for path in [args, result].into_iter().flatten() {
                if let Ok(meta) = std::fs::metadata(&path) {
                    report.bytes_freed += meta.len();
                    report.bodies_deleted += 1;
                    if !dry_run {
                        std::fs::remove_file(&path)
                            .with_context(|| format!("deleting body file {path}"))?;
                    }
                }
            }
            if !dry_run {
                self.conn.lock().unwrap().execute(
                    "UPDATE tool_calls SET args_body_path=NULL, result_body_path=NULL WHERE id=?1",
                    params![id],
                )?;
            }
            report.rows_affected += 1;
        }
        let rows: Vec<(String, Option<String>, Option<String>, Option<String>)> = {
            let conn = self.conn.lock().unwrap();
            let mut stmt = conn.prepare(
                "SELECT id, req_body_path, upstream_req_body_path, resp_body_path
                 FROM traces WHERE ts_request_ms < ?1
                   AND (req_body_path IS NOT NULL OR upstream_req_body_path IS NOT NULL
                        OR resp_body_path IS NOT NULL OR req_headers_json IS NOT NULL
                        OR resp_headers_json IS NOT NULL)",
            )?;
            let rows = stmt
                .query_map(params![older_than_ms], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
                })?
                .filter_map(|r| r.ok())
                .collect();
            rows
        };
        for (id, req, upstream, resp) in rows {
            for path in [req, upstream, resp].into_iter().flatten() {
                let Ok(meta) = std::fs::metadata(&path) else {
                    continue;
                };
                report.bytes_freed += meta.len();
                report.bodies_deleted += 1;
                if !dry_run {
                    std::fs::remove_file(&path)
                        .with_context(|| format!("deleting body file {path}"))?;
                }
            }
            if !dry_run {
                let conn = self.conn.lock().unwrap();
                conn.execute(
                    "UPDATE traces SET req_body_path = NULL, upstream_req_body_path = NULL,
                            resp_body_path = NULL, req_headers_json = NULL, resp_headers_json = NULL
                     WHERE id = ?1",
                    params![id],
                )?;
            }
            report.rows_affected += 1;
        }
        if dry_run {
            if !bodies_only {
                let conn = self.conn.lock().unwrap();
                report.rows_deleted = conn.query_row(
                    "SELECT COUNT(*) FROM traces WHERE ts_request_ms < ?1",
                    params![older_than_ms],
                    |r| r.get::<_, i64>(0),
                )? as u64;
            }
        } else {
            let mut conn = self.conn.lock().unwrap();
            let transaction = conn.transaction()?;
            lar_gc::prune_references(&transaction, older_than_ms, bodies_only)?;
            if !bodies_only {
                report.rows_deleted = transaction.execute(
                    "DELETE FROM traces WHERE ts_request_ms < ?1",
                    params![older_than_ms],
                )? as u64;
                report.rows_deleted += transaction.execute(
                    "DELETE FROM tool_calls WHERE ts_start_ms < ?1",
                    params![older_than_ms],
                )? as u64;
            }
            transaction.commit()?;
            if report.rows_deleted > 0 {
                if let Err(e) = conn.execute_batch("VACUUM") {
                    tracing::warn!("prune: VACUUM skipped: {e}");
                }
            }
        }
        if !dry_run {
            let bodies = self.data_dir.join("bodies");
            if let Ok(entries) = std::fs::read_dir(&bodies) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    let name = entry.file_name().to_string_lossy().to_string();
                    if !path.is_dir() || !date_dir_name(&name) {
                        continue;
                    }
                    let empty = std::fs::read_dir(&path)
                        .map(|mut d| d.next().is_none())
                        .unwrap_or(false);
                    if empty && std::fs::remove_dir(&path).is_ok() {
                        report.dirs_removed += 1;
                    }
                }
            }
        }
        Ok(report)
    }

    pub fn disk_usage(&self) -> Result<Value> {
        let mut sqlite_bytes = 0u64;
        for suffix in ["", "-wal", "-shm"] {
            let path = self.data_dir.join(format!("alexandria.sqlite3{suffix}"));
            if let Ok(m) = std::fs::metadata(&path) {
                sqlite_bytes += m.len();
            }
        }
        let mut bodies_bytes = 0u64;
        let mut days: Vec<Value> = vec![];
        if let Ok(entries) = std::fs::read_dir(self.data_dir.join("bodies")) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if !entry.path().is_dir() || !date_dir_name(&name) {
                    continue;
                }
                let (mut files, mut bytes) = (0u64, 0u64);
                if let Ok(inner) = std::fs::read_dir(entry.path()) {
                    for f in inner.flatten() {
                        if let Ok(m) = f.metadata() {
                            if m.is_file() {
                                files += 1;
                                bytes += m.len();
                            }
                        }
                    }
                }
                bodies_bytes += bytes;
                days.push(json!({"date": name, "files": files, "bytes": bytes}));
            }
        }
        days.sort_by(|a, b| b["date"].as_str().cmp(&a["date"].as_str()));
        let conn = self.conn.lock().unwrap();
        let (trace_rows, oldest_ts_ms, newest_ts_ms) = conn.query_row(
            "SELECT COUNT(*), MIN(ts_request_ms), MAX(ts_request_ms) FROM traces",
            [],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, Option<i64>>(1)?,
                    r.get::<_, Option<i64>>(2)?,
                ))
            },
        )?;
        let mut lar_files = Vec::new();
        let mut lar_catalog_bytes = 0_u64;
        let mut lar_physical_bytes = 0_u64;
        let mut lar_missing_files = 0_u64;
        {
            let mut statement = conn.prepare(
                "SELECT role, state, COUNT(*), COALESCE(SUM(size_bytes), 0)
                   FROM lar_files GROUP BY role, state ORDER BY role, state",
            )?;
            let rows = statement.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, u64>(2)?,
                    row.get::<_, u64>(3)?,
                ))
            })?;
            for row in rows {
                let (role, state, files, bytes) = row?;
                lar_catalog_bytes = lar_catalog_bytes.saturating_add(bytes);
                lar_files.push(json!({
                    "role": role,
                    "state": state,
                    "files": files,
                    "catalog_bytes": bytes,
                }));
            }
        }
        {
            let mut statement = conn.prepare("SELECT path FROM lar_files")?;
            let paths = statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for path in paths {
                let path = std::path::PathBuf::from(path);
                let path = if path.is_absolute() {
                    path
                } else {
                    self.data_dir.join(path)
                };
                match std::fs::metadata(path) {
                    Ok(metadata) if metadata.is_file() => {
                        lar_physical_bytes = lar_physical_bytes.saturating_add(metadata.len());
                    }
                    _ => lar_missing_files = lar_missing_files.saturating_add(1),
                }
            }
        }
        let (
            lar_chunks,
            lar_unique_uncompressed_bytes,
            lar_chunk_compressed_bytes,
            lar_unreachable_chunks,
        ) = conn.query_row(
            "SELECT COUNT(*), COALESCE(SUM(uncompressed_length), 0),
                    COALESCE(SUM(compressed_length), 0),
                    COALESCE(SUM(CASE WHEN state='unreachable' THEN 1 ELSE 0 END), 0)
               FROM lar_chunks",
            [],
            |row| {
                Ok((
                    row.get::<_, u64>(0)?,
                    row.get::<_, u64>(1)?,
                    row.get::<_, u64>(2)?,
                    row.get::<_, u64>(3)?,
                ))
            },
        )?;
        let (lar_manifests, lar_unique_manifest_bytes): (u64, u64) = conn.query_row(
            "SELECT COUNT(*), COALESCE(SUM(total_length), 0)
               FROM lar_manifests WHERE state='ready'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let (lar_artifact_refs, lar_referenced_body_bytes): (u64, u64) = conn.query_row(
            "SELECT COUNT(*), COALESCE(SUM(m.total_length), 0)
               FROM lar_trace_artifacts a
               JOIN lar_manifests m ON m.manifest_id=a.manifest_id
              WHERE a.validation_state='validated' AND m.state='ready'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let (lar_header_blocks, lar_header_atoms, lar_stages): (u64, u64, u64) = conn.query_row(
            "SELECT (SELECT COUNT(*) FROM lar_header_blocks),
                    (SELECT COUNT(*) FROM lar_header_atoms),
                    (SELECT COUNT(*) FROM lar_stage_records)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        let (lar_checkpoints, lar_latest_checkpoint_ms): (u64, Option<i64>) = conn.query_row(
            "SELECT COUNT(*), MAX(created_at_ms) FROM lar_checkpoints",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let referenced_to_unique_ratio = if lar_unique_uncompressed_bytes == 0 {
            1.0
        } else {
            lar_referenced_body_bytes as f64 / lar_unique_uncompressed_bytes as f64
        };
        let chunk_compression_ratio = if lar_chunk_compressed_bytes == 0 {
            1.0
        } else {
            lar_unique_uncompressed_bytes as f64 / lar_chunk_compressed_bytes as f64
        };
        Ok(json!({
            "sqlite_bytes": sqlite_bytes,
            "bodies_bytes": bodies_bytes,
            "days": days,
            "trace_rows": trace_rows,
            "oldest_ts_ms": oldest_ts_ms,
            "newest_ts_ms": newest_ts_ms,
            "lar": {
                "files": lar_files,
                "catalog_bytes": lar_catalog_bytes,
                "physical_bytes": lar_physical_bytes,
                "missing_or_offline_files": lar_missing_files,
                "chunks": lar_chunks,
                "unreachable_chunks": lar_unreachable_chunks,
                "unique_uncompressed_bytes": lar_unique_uncompressed_bytes,
                "compressed_chunk_bytes": lar_chunk_compressed_bytes,
                "manifests": lar_manifests,
                "unique_manifest_bytes": lar_unique_manifest_bytes,
                "artifact_references": lar_artifact_refs,
                "referenced_body_bytes": lar_referenced_body_bytes,
                "referenced_to_unique_ratio": referenced_to_unique_ratio,
                "chunk_compression_ratio": chunk_compression_ratio,
                "header_blocks": lar_header_blocks,
                "header_atoms": lar_header_atoms,
                "stages": lar_stages,
                "checkpoints": lar_checkpoints,
                "latest_checkpoint_ms": lar_latest_checkpoint_ms,
            },
        }))
    }

    pub fn write_body(&self, trace_id: &str, kind: &str, bytes: &[u8]) -> Result<String> {
        self.write_body_through_configured_store(trace_id, kind, bytes)
    }

    #[cfg(test)]
    fn write_body_dated(
        &self,
        date: &str,
        trace_id: &str,
        kind: &str,
        bytes: &[u8],
    ) -> Result<String> {
        self.write_legacy_body_dated(date, trace_id, kind, bytes)
    }

    pub(crate) fn write_legacy_body_dated(
        &self,
        date: &str,
        trace_id: &str,
        kind: &str,
        bytes: &[u8],
    ) -> Result<String> {
        let dir = self.data_dir.join("bodies").join(date);
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{trace_id}.{kind}.gz"));
        let sequence = BODY_TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let temp = dir.join(format!(
            ".{trace_id}.{kind}.{}.{sequence}.tmp",
            std::process::id()
        ));
        let result = (|| -> Result<()> {
            let file = std::fs::File::create(&temp)?;
            let mut enc = GzEncoder::new(file, Compression::default());
            enc.write_all(bytes)?;
            let file = enc.finish()?;
            file.sync_all()?;
            #[cfg(windows)]
            if path.exists() {
                std::fs::remove_file(&path)?;
            }
            std::fs::rename(&temp, &path)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temp);
        }
        result?;
        Ok(path.to_string_lossy().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "alexandria-store-test-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn trace(id: &str, ts: i64, run: Option<&str>) -> TraceRecord {
        TraceRecord {
            id: id.into(),
            ts_request_ms: ts,
            ts_response_ms: Some(ts + 250),
            status: Some(200),
            routed_model: Some("claude-haiku-4-5".into()),
            upstream_provider: Some("anthropic".into()),
            run_id: run.map(String::from),
            usage: alex_core::Usage {
                input_tokens: Some(10),
                output_tokens: Some(5),
                ..Default::default()
            },
            cost_usd: Some(0.001),
            ..Default::default()
        }
    }

    #[test]
    fn session_display_fields_are_stable_and_precomputed() {
        let fields = session_display_fields(&json!({
            "session_id": "abcdefghijklmno-pqrstuvwxyz-0123456789",
            "first_ts_ms": 100, "last_ts_ms": 2_600,
            "providers": ["openai", "anthropic"],
            "tags": {"z": "last", "a": "first", "empty": ""},
            "last_status": 500, "errors": 1,
        }));
        assert_eq!(fields["short_id"], "abcdefghij…23456789");
        assert_eq!(fields["duration_ms"], 2500);
        assert_eq!(fields["providers"], json!(["openai", "anthropic"]));
        assert_eq!(fields["tags_summary"], "a=first z=last");
        assert_eq!(fields["status_label"], "Error");
    }

    #[test]
    fn run_summary_aggregates() {
        let store = Store::open(tmpdir("summary")).unwrap();
        let mut a = trace("a", 1000, Some("run-1"));
        a.tags = Some(r#"{"suite":"swebench"}"#.into());
        let mut b = trace("b", 2000, Some("run-1"));
        b.tags = Some(r#"{"case":"astropy-1"}"#.into());
        b.status = Some(500);
        b.error = Some("boom".into());
        b.routed_model = Some("gpt-5.5".into());
        b.upstream_provider = Some("openai".into());
        let c = trace("c", 3000, Some("run-2"));
        for t in [&a, &b, &c] {
            store.insert_trace(t).unwrap();
        }
        let s = store.run_summary("run-1").unwrap();
        assert_eq!(s["trace_count"], 2);
        assert_eq!(s["first_ts_ms"], 1000);
        assert_eq!(s["last_ts_ms"], 2000);
        assert_eq!(s["status_counts"]["200"], 1);
        assert_eq!(s["status_counts"]["500"], 1);
        assert_eq!(s["models"], json!(["claude-haiku-4-5", "gpt-5.5"]));
        assert_eq!(s["providers"], json!(["anthropic", "openai"]));
        assert_eq!(s["total_input_tokens"], 20);
        assert_eq!(s["total_output_tokens"], 10);
        assert_eq!(s["tags"]["suite"], "swebench");
        assert_eq!(s["tags"]["case"], "astropy-1");
        assert_eq!(s["errors"], 1);
        let missing = store.run_summary("nope").unwrap();
        assert_eq!(missing["trace_count"], 0);
    }

    #[test]
    fn search_traces_filters() {
        let store = Store::open(tmpdir("search")).unwrap();
        let mut a = trace("a", 1000, Some("run-1"));
        a.key_fingerprint = Some("deadbeefdeadbeef".into());
        a.reasoning_effort = Some("high".into());
        a.thinking_budget = Some(16_384);
        let mut b = trace("b", 2000, Some("run-1"));
        b.status = Some(429);
        b.error = Some("rate limited".into());
        let c = trace("c", 3000, None);
        for t in [&a, &b, &c] {
            store.insert_trace(t).unwrap();
        }
        let all = store.search_traces(&TraceFilter::default()).unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0]["id"], "c");
        assert_eq!(all[0]["latency_ms"], 250);
        assert_eq!(all[2]["reasoning_effort"], "high");
        assert_eq!(all[2]["thinking_budget"], 16_384);
        let window = store
            .search_traces(&TraceFilter {
                since_ms: Some(1500),
                until_ms: Some(2500),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(window.len(), 1);
        assert_eq!(window[0]["id"], "b");
        let by_run = store
            .search_traces(&TraceFilter {
                run_id: Some("run-1".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(by_run.len(), 2);
        let by_status = store
            .search_traces(&TraceFilter {
                status: Some(429),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(by_status.len(), 1);
        let errors = store
            .search_traces(&TraceFilter {
                errors_only: true,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0]["id"], "b");
        let by_key = store
            .search_traces(&TraceFilter {
                key_fingerprint: Some("deadbeefdeadbeef".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(by_key.len(), 1);
        assert_eq!(by_key[0]["id"], "a");
        let by_effort = store
            .search_traces(&TraceFilter {
                reasoning_effort: Some("high".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(by_effort.len(), 1);
        assert_eq!(by_effort[0]["id"], "a");
        let limited = store
            .search_traces(&TraceFilter {
                limit: 1,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(limited.len(), 1);
    }

    #[test]
    fn sessions_aggregate_and_order() {
        let store = Store::open(tmpdir("sessions")).unwrap();
        let mut a = trace("a", 1000, Some("run-1"));
        a.session_id = Some("ses_1".into());
        a.tags = Some(r#"{"suite":"swebench"}"#.into());
        a.harness = Some("codex".into());
        a.reasoning_effort = Some("high".into());
        a.account_id = Some("openai-oauth-personal".into());
        let mut b = trace("b", 2000, None);
        b.session_id = Some("ses_1".into());
        b.status = Some(500);
        b.error = Some("boom".into());
        b.routed_model = Some("gpt-5.5".into());
        b.tags = Some(r#"{"case":"x1"}"#.into());
        b.reasoning_effort = Some("minimal".into());
        b.account_id = Some("openai-oauth-work".into());
        let mut c = trace("c", 5000, None);
        c.session_id = Some("ses_2".into());
        let d = trace("d", 9000, None);
        for t in [&a, &b, &c, &d] {
            store.insert_trace(t).unwrap();
        }
        let sessions = store.sessions(None, 0).unwrap();
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0]["session_id"], "ses_2");
        let s1 = &sessions[1];
        assert_eq!(s1["session_id"], "ses_1");
        assert_eq!(s1["run_id"], "run-1");
        assert_eq!(s1["first_ts_ms"], 1000);
        assert_eq!(s1["last_ts_ms"], 2000);
        assert_eq!(s1["trace_count"], 2);
        assert_eq!(s1["harness"], "codex");
        assert_eq!(s1["providers"], json!(["anthropic"]));
        assert_eq!(s1["total_input_tokens"], 20);
        assert_eq!(s1["total_output_tokens"], 10);
        assert_eq!(s1["errors"], 1);
        assert_eq!(s1["last_status"], 500);
        assert_eq!(s1["tags"]["suite"], "swebench");
        assert_eq!(s1["tags"]["case"], "x1");
        let efforts: Vec<String> = s1["efforts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m.as_str().unwrap().to_string())
            .collect();
        assert!(efforts.contains(&"high".to_string()));
        assert!(efforts.contains(&"minimal".to_string()));
        let models: Vec<String> = s1["models"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m.as_str().unwrap().to_string())
            .collect();
        assert!(models.contains(&"claude-haiku-4-5".to_string()));
        assert!(models.contains(&"gpt-5.5".to_string()));
        let account_ids: Vec<String> = s1["account_ids"]
            .as_array()
            .unwrap()
            .iter()
            .map(|account| account.as_str().unwrap().to_string())
            .collect();
        assert!(account_ids.contains(&"openai-oauth-personal".to_string()));
        assert!(account_ids.contains(&"openai-oauth-work".to_string()));
        let recent = store.sessions(Some(3000), 0).unwrap();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0]["session_id"], "ses_2");
        let limited = store.sessions(None, 1).unwrap();
        assert_eq!(limited.len(), 1);
    }

    #[test]
    fn harness_events_persist_session_lineage_and_annotate_sessions() {
        let store = Store::open(tmpdir("session-lineage")).unwrap();
        for (id, session_id, ts) in [
            ("root-trace", "root-session", 1000),
            ("child-trace", "child-session", 2000),
            ("grandchild-trace", "grandchild-session", 3000),
        ] {
            let mut row = trace(id, ts, None);
            row.session_id = Some(session_id.into());
            row.harness = Some("codex".into());
            store.insert_trace(&row).unwrap();
        }
        assert!(store
            .record_harness_event(
                "codex",
                &json!({
                    "hook_event_name": "SubagentStart",
                    "session_id": "root-session",
                    "turn_id": "turn-1",
                    "agent_id": "child-session",
                    "agent_type": "default",
                }),
                1500,
            )
            .unwrap());
        assert!(store
            .record_harness_event(
                "codex",
                &json!({
                    "hook_event_name": "SubagentStart",
                    "session_id": "child-session",
                    "turn_id": "turn-2",
                    "agent_id": "grandchild-session",
                }),
                2500,
            )
            .unwrap());
        assert!(store
            .record_harness_event(
                "codex",
                &json!({
                    "hook_event_name": "SubagentStop",
                    "session_id": "root-session",
                    "agent_id": "child-session",
                }),
                3500,
            )
            .unwrap());
        assert_eq!(
            store
                .session_lineage_root("codex", "grandchild-session")
                .unwrap(),
            "root-session"
        );
        let sessions = store.sessions(None, 0).unwrap();
        let root = sessions
            .iter()
            .find(|row| row["session_id"] == "root-session")
            .unwrap();
        assert_eq!(root["child_count"], 1);
        let child = sessions
            .iter()
            .find(|row| row["session_id"] == "child-session")
            .unwrap();
        assert_eq!(child["parent_session_id"], "root-session");
        assert_eq!(child["lineage_turn_id"], "turn-1");
        assert_eq!(child["agent_type"], "default");
        assert_eq!(child["subagent_started_ms"], 1500);
        assert_eq!(child["subagent_stopped_ms"], 3500);
        assert_eq!(child["child_count"], 1);
    }

    #[test]
    fn session_traces_ascending() {
        let store = Store::open(tmpdir("session-traces")).unwrap();
        for (id, ts) in [("a", 3000i64), ("b", 1000), ("c", 2000)] {
            let mut t = trace(id, ts, None);
            t.session_id = Some("ses_1".into());
            t.upstream_format = Some("anthropic".into());
            t.req_body_path = Some(format!("/bodies/{id}.request.json.gz"));
            store.insert_trace(&t).unwrap();
        }
        let rows = store.session_traces("ses_1", None).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0]["id"], "b");
        assert_eq!(rows[2]["id"], "a");
        assert_eq!(rows[0]["upstream_format"], "anthropic");
        assert_eq!(rows[0]["client_format"], Value::Null);
        assert_eq!(rows[0]["req_body_path"], "/bodies/b.request.json.gz");
        let windowed = store.session_traces("ses_1", Some(1500)).unwrap();
        assert_eq!(windowed.len(), 2);
        assert_eq!(windowed[0]["id"], "c");
        assert!(store.session_traces("nope", None).unwrap().is_empty());
    }

    #[test]
    fn session_trace_pages_scale_to_synthetic_long_session() {
        let store = Store::open(tmpdir("session-trace-pages-long")).unwrap();
        // The real regression session had 1,277 traces. Use 1,500, including
        // duplicate millisecond timestamps, so cursor correctness and index
        // behavior remain covered as sessions and subagents grow.
        for index in 0..1_500 {
            let id = format!("trace-{index:04}");
            let mut row = trace(&id, 1_000 + (index / 3) as i64, None);
            row.session_id = Some("long-session".into());
            store.insert_trace(&row).unwrap();
        }

        let started = std::time::Instant::now();
        let latest = store
            .session_traces_page("long-session", None, None, 50, true)
            .unwrap();
        let elapsed = started.elapsed();
        eprintln!("synthetic 1,500-trace tail page: {elapsed:?}");
        assert!(elapsed < std::time::Duration::from_millis(500));
        assert_eq!(latest.total_count, 1_500);
        assert_eq!(latest.rows.len(), 50);
        assert_eq!(latest.rows.first().unwrap()["id"], "trace-1450");
        assert_eq!(latest.rows.last().unwrap()["id"], "trace-1499");
        assert!(latest.has_more_before);
        assert!(!latest.has_more_after);

        let first = latest.rows.first().unwrap();
        let older = store
            .session_traces_page(
                "long-session",
                None,
                Some((
                    first["ts_request_ms"].as_i64().unwrap(),
                    first["id"].as_str().unwrap().to_string(),
                )),
                50,
                false,
            )
            .unwrap();
        assert_eq!(older.rows.first().unwrap()["id"], "trace-1400");
        assert_eq!(older.rows.last().unwrap()["id"], "trace-1449");
        assert!(older.has_more_before);
        assert!(older.has_more_after);

        let last = latest.rows.last().unwrap();
        let after = store
            .session_traces_page(
                "long-session",
                Some((
                    last["ts_request_ms"].as_i64().unwrap(),
                    last["id"].as_str().unwrap().to_string(),
                )),
                None,
                50,
                false,
            )
            .unwrap();
        assert!(after.rows.is_empty());
        assert_eq!(after.total_count, 1_500);
    }

    #[test]
    fn tool_calls_join_sessions_and_share_reset_body_accounting() {
        let store = Store::open(tmpdir("tool-calls")).unwrap();
        let args = store
            .write_body(
                "tool-session-call",
                "tool-args.json",
                br#"{"argv":["echo","ok"]}"#,
            )
            .unwrap();
        let result = store
            .write_body("tool-session-call", "tool-result.json", b"large output")
            .unwrap();
        store
            .upsert_tool_call(&ToolCallRecord {
                id: "tool-session-call".into(),
                harness: "pi".into(),
                session_id: "session".into(),
                turn_id: Some("2".into()),
                tool_call_id: "call".into(),
                trace_id: Some("trace".into()),
                tool_name: "bash".into(),
                ts_start_ms: 10,
                ts_end_ms: Some(20),
                is_error: Some(false),
                exit_status: Some(0),
                args_body_path: Some(args),
                result_body_path: Some(result),
            })
            .unwrap();
        let calls = store.session_tool_calls("session").unwrap();
        assert_eq!(calls[0]["trace_id"], "trace");
        assert_eq!(calls[0]["turn_id"], "2");
        // Explicit links remain with their trace page even when execution
        // starts beyond the unlinked time boundary.
        assert_eq!(
            store
                .session_tool_calls_page("session", &["trace".into()], 0, Some(5))
                .unwrap()
                .len(),
            1
        );
        assert!(store
            .session_tool_calls_page("session", &[], 0, Some(5))
            .unwrap()
            .is_empty());
        assert!(store.reset_counts().unwrap().body_files >= 2);
        store.clear_traces_and_bodies().unwrap();
        assert!(store.session_tool_calls("session").unwrap().is_empty());
        assert_eq!(store.reset_counts().unwrap().body_files, 0);
    }

    #[test]
    fn trace_backup_rows_round_trip_and_are_idempotent() {
        let dir = tmpdir("trace-backup-round-trip");
        let store = Store::open(dir.clone()).unwrap();
        let request_path = store
            .write_body("backup-trace", "request.json", br#"{"model":"test"}"#)
            .unwrap();
        let tool_path = store
            .write_body("backup-tool", "tool-result.json", b"tool output")
            .unwrap();
        let mut row = trace("backup-trace", 1_000, Some("backup-run"));
        row.req_body_path = Some(request_path);
        store.insert_trace(&row).unwrap();
        store
            .upsert_tool_call(&ToolCallRecord {
                id: "backup-tool".into(),
                harness: "pi".into(),
                session_id: "backup-session".into(),
                turn_id: Some("turn-1".into()),
                tool_call_id: "call-1".into(),
                trace_id: Some("backup-trace".into()),
                tool_name: "bash".into(),
                ts_start_ms: 1_100,
                ts_end_ms: Some(1_200),
                is_error: Some(false),
                exit_status: Some(0),
                args_body_path: None,
                result_body_path: Some(tool_path),
            })
            .unwrap();
        store
            .insert_heartbeat(900, "anthropic", Some("account"), true, Some(200), 12, "ok")
            .unwrap();

        let backup = store.export_trace_backup_rows().unwrap();
        assert_eq!(backup.traces.len(), 1);
        assert_eq!(backup.tool_calls.len(), 1);
        assert_eq!(backup.heartbeats.len(), 1);
        assert!(backup.traces[0]["req_body_path"]
            .as_str()
            .unwrap()
            .starts_with("bodies/"));
        assert!(backup.tool_calls[0]["result_body_path"]
            .as_str()
            .unwrap()
            .starts_with("bodies/"));

        store.clear_traces_and_bodies().unwrap();
        let imported = store.import_trace_backup_rows(&backup).unwrap();
        assert_eq!(imported.traces_imported, 1);
        assert_eq!(imported.tool_calls_imported, 1);
        assert_eq!(imported.heartbeats_imported, 1);
        assert_eq!(store.reset_counts().unwrap().traces, 1);
        assert_eq!(store.reset_counts().unwrap().heartbeats, 1);
        assert!(
            store.get_trace("backup-trace").unwrap().unwrap()["req_body_path"]
                .as_str()
                .unwrap()
                .starts_with(dir.to_string_lossy().as_ref())
        );

        let repeated = store.import_trace_backup_rows(&backup).unwrap();
        assert_eq!(repeated.traces_skipped, 1);
        assert_eq!(repeated.tool_calls_skipped, 1);
        assert_eq!(repeated.heartbeats_skipped, 1);
    }

    #[test]
    fn trace_backup_import_skips_existing_rows_and_keeps_newer_history() {
        let source = Store::open(tmpdir("trace-backup-source")).unwrap();
        source
            .insert_trace(&trace("existing", 1_000, None))
            .unwrap();
        source
            .insert_trace(&trace("from-backup", 2_000, None))
            .unwrap();
        source
            .upsert_tool_call(&ToolCallRecord {
                id: "existing-tool".into(),
                harness: "pi".into(),
                session_id: "session".into(),
                turn_id: None,
                tool_call_id: "call".into(),
                trace_id: Some("existing".into()),
                tool_name: "old-name".into(),
                ts_start_ms: 1_100,
                ts_end_ms: None,
                is_error: None,
                exit_status: None,
                args_body_path: None,
                result_body_path: None,
            })
            .unwrap();
        let backup = source.export_trace_backup_rows().unwrap();

        let destination = Store::open(tmpdir("trace-backup-destination")).unwrap();
        let mut existing = trace("existing", 9_000, None);
        existing.routed_model = Some("newer-model".into());
        destination.insert_trace(&existing).unwrap();
        destination
            .insert_trace(&trace("newer", 10_000, None))
            .unwrap();
        destination
            .upsert_tool_call(&ToolCallRecord {
                id: "existing-tool".into(),
                harness: "pi".into(),
                session_id: "session".into(),
                turn_id: None,
                tool_call_id: "call".into(),
                trace_id: Some("existing".into()),
                tool_name: "new-name".into(),
                ts_start_ms: 9_100,
                ts_end_ms: None,
                is_error: None,
                exit_status: None,
                args_body_path: None,
                result_body_path: None,
            })
            .unwrap();

        let imported = destination.import_trace_backup_rows(&backup).unwrap();
        assert_eq!(imported.traces_imported, 1);
        assert_eq!(imported.traces_skipped, 1);
        assert_eq!(imported.tool_calls_imported, 0);
        assert_eq!(imported.tool_calls_skipped, 1);
        assert_eq!(destination.reset_counts().unwrap().traces, 3);
        assert_eq!(
            destination.get_trace("existing").unwrap().unwrap()["routed_model"],
            "newer-model"
        );
        assert_eq!(
            destination.session_tool_calls("session").unwrap()[0]["tool_name"],
            "new-name"
        );
    }

    #[test]
    fn get_and_delete_trace() {
        let store = Store::open(tmpdir("delete")).unwrap();
        let mut t = trace("a", 1000, None);
        t.req_body_path = Some(
            store
                .write_body("a", "request.json", b"{\"model\":\"x\"}")
                .unwrap(),
        );
        t.resp_body_path = Some("/nonexistent/a.response.body.gz".into());
        store.insert_trace(&t).unwrap();
        let row = store.get_trace("a").unwrap().unwrap();
        assert_eq!(row["id"], "a");
        assert_eq!(row["resp_body_path"], "/nonexistent/a.response.body.gz");
        assert!(store.get_trace("missing").unwrap().is_none());
        let paths = store.delete_trace("a").unwrap();
        assert_eq!(paths.len(), 2);
        assert!(store.get_trace("a").unwrap().is_none());
        assert!(store.delete_trace("a").is_err());
    }

    fn seed_dated_trace(store: &Store, id: &str, ts: i64, date: &str) {
        let mut t = trace(id, ts, None);
        t.req_body_path = Some(
            store
                .write_body_dated(date, id, "request.json", b"{\"model\":\"x\"}")
                .unwrap(),
        );
        t.resp_body_path = Some(
            store
                .write_body_dated(date, id, "response.body", b"response bytes here")
                .unwrap(),
        );
        t.req_headers_json = Some(r#"{"authorization":"[redacted]"}"#.into());
        t.resp_headers_json = Some(r#"{"content-type":"application/json"}"#.into());
        store.insert_trace(&t).unwrap();
    }

    #[test]
    fn date_dir_name_shape() {
        assert!(date_dir_name("2024-01-31"));
        assert!(!date_dir_name("2024-1-31"));
        assert!(!date_dir_name("20240131"));
        assert!(!date_dir_name("accounts"));
        assert!(!date_dir_name("2024-01-31x"));
    }

    #[test]
    fn prune_bodies_dry_run_then_real() {
        let store = Store::open(tmpdir("prune-bodies")).unwrap();
        seed_dated_trace(&store, "a", 1000, "2024-01-01");
        seed_dated_trace(&store, "b", 2000, "2024-01-02");
        seed_dated_trace(&store, "c", 10_000, "2024-01-03");
        let scratch = store.data_dir.join("bodies").join("scratch");
        std::fs::create_dir_all(&scratch).unwrap();
        let dry = store.prune(5000, true, true).unwrap();
        assert_eq!(dry.bodies_deleted, 4);
        assert!(dry.bytes_freed > 0);
        assert_eq!(dry.rows_affected, 2);
        assert_eq!(dry.rows_deleted, 0);
        assert_eq!(dry.dirs_removed, 0);
        let before = store.get_trace("a").unwrap().unwrap();
        assert!(before["req_body_path"].is_string());
        assert!(store.data_dir.join("bodies").join("2024-01-01").exists());
        let real = store.prune(5000, true, false).unwrap();
        assert_eq!(real.bodies_deleted, dry.bodies_deleted);
        assert_eq!(real.bytes_freed, dry.bytes_freed);
        assert_eq!(real.rows_affected, dry.rows_affected);
        assert_eq!(real.rows_deleted, 0);
        assert_eq!(real.dirs_removed, 2);
        assert!(!store.data_dir.join("bodies").join("2024-01-01").exists());
        assert!(!store.data_dir.join("bodies").join("2024-01-02").exists());
        assert!(store.data_dir.join("bodies").join("2024-01-03").exists());
        assert!(scratch.exists());
        assert_eq!(
            store.search_traces(&TraceFilter::default()).unwrap().len(),
            3
        );
        let pruned = store.get_trace("a").unwrap().unwrap();
        for key in [
            "req_body_path",
            "upstream_req_body_path",
            "resp_body_path",
            "req_headers_json",
            "resp_headers_json",
        ] {
            assert_eq!(pruned[key], Value::Null, "{key} not nulled");
        }
        let kept = store.get_trace("c").unwrap().unwrap();
        assert!(kept["req_body_path"].is_string());
        assert!(kept["req_headers_json"].is_string());
        assert_eq!(
            store.prune(5000, true, false).unwrap(),
            PruneReport::default()
        );
    }

    #[test]
    fn prune_rows_deletes_and_du_reflects() {
        let store = Store::open(tmpdir("prune-rows")).unwrap();
        seed_dated_trace(&store, "a", 1000, "2024-01-01");
        seed_dated_trace(&store, "b", 2000, "2024-01-02");
        seed_dated_trace(&store, "c", 10_000, "2024-01-03");
        let before = store.disk_usage().unwrap();
        assert_eq!(before["trace_rows"], 3);
        assert_eq!(before["oldest_ts_ms"], 1000);
        assert_eq!(before["newest_ts_ms"], 10_000);
        assert_eq!(before["days"].as_array().unwrap().len(), 3);
        assert_eq!(before["days"][0]["date"], "2024-01-03");
        assert_eq!(before["days"][0]["files"], 2);
        assert!(before["bodies_bytes"].as_u64().unwrap() > 0);
        assert!(before["sqlite_bytes"].as_u64().unwrap() > 0);
        let dry = store.prune(5000, false, true).unwrap();
        assert_eq!(dry.rows_deleted, 2);
        assert_eq!(dry.rows_affected, 2);
        assert_eq!(store.disk_usage().unwrap()["trace_rows"], 3);
        let real = store.prune(5000, false, false).unwrap();
        assert_eq!(real.bodies_deleted, 4);
        assert_eq!(real.rows_affected, 2);
        assert_eq!(real.rows_deleted, 2);
        assert_eq!(real.dirs_removed, 2);
        assert_eq!(real.bytes_freed, dry.bytes_freed);
        let after = store.disk_usage().unwrap();
        assert_eq!(after["trace_rows"], 1);
        assert_eq!(after["oldest_ts_ms"], 10_000);
        assert_eq!(after["newest_ts_ms"], 10_000);
        assert_eq!(after["days"].as_array().unwrap().len(), 1);
        assert_eq!(after["days"][0]["date"], "2024-01-03");
        assert_eq!(
            after["bodies_bytes"].as_u64().unwrap(),
            before["bodies_bytes"].as_u64().unwrap() - real.bytes_freed
        );
        assert!(store.get_trace("a").unwrap().is_none());
        assert!(store.get_trace("c").unwrap().is_some());
    }

    #[test]
    fn disk_usage_reports_live_lar_dedup_and_archive_health() {
        let data_dir = tmpdir("lar-disk-usage");
        let store = Store::open_with_lar_body_store(
            data_dir,
            LarBodyStoreConfig {
                mode: LarBodyStoreMode::LarWithFallback,
                ..Default::default()
            },
        )
        .unwrap();
        let body = br#"{"messages":[{"role":"user","content":"repeated body"}]}"#;
        for trace_id in ["lar-du-a", "lar-du-b"] {
            store
                .write_body_artifact(
                    &LarBodyArtifact::trace(trace_id, "client_request"),
                    "request.json",
                    body,
                )
                .unwrap();
            store
                .insert_trace(&trace(trace_id, 1000, Some("lar-du-session")))
                .unwrap();
        }

        let usage = store.disk_usage().unwrap();
        assert_eq!(usage["lar"]["manifests"], 1);
        assert_eq!(usage["lar"]["artifact_references"], 2);
        assert_eq!(usage["lar"]["referenced_body_bytes"], body.len() * 2);
        assert!(usage["lar"]["chunks"].as_u64().unwrap() > 0);
        assert!(usage["lar"]["physical_bytes"].as_u64().unwrap() > 0);
        assert_eq!(usage["lar"]["missing_or_offline_files"], 0);
        assert!(usage["lar"]["referenced_to_unique_ratio"].as_f64().unwrap() > 1.0);
    }

    #[test]
    fn limit_defaults_and_caps() {
        assert_eq!(effective_limit(0), 200);
        assert_eq!(effective_limit(50), 50);
        assert_eq!(effective_limit(9000), 5000);
    }

    #[test]
    fn run_artifacts_reports_files() {
        let dir = tmpdir("artifacts");
        let store = Store::open(dir).unwrap();
        let mut t = trace("a", 1000, Some("run-1"));
        t.req_body_path = Some(
            store
                .write_body("a", "request.json", b"{\"model\":\"x\"}")
                .unwrap(),
        );
        t.resp_body_path = Some("/nonexistent/a.response.body.gz".into());
        store.insert_trace(&t).unwrap();
        let arts = store.run_artifacts("run-1").unwrap();
        assert_eq!(arts.len(), 2);
        assert_eq!(arts[0]["kind"], "request");
        assert_eq!(arts[0]["exists"], true);
        assert!(arts[0]["size_bytes"].as_u64().unwrap() > 0);
        assert_eq!(arts[1]["kind"], "response");
        assert_eq!(arts[1]["exists"], false);
        assert_eq!(arts[1]["size_bytes"], Value::Null);
        assert!(store.run_artifacts("nope").unwrap().is_empty());
    }

    #[test]
    fn run_trace_ids_include_bodyless_traces_once_in_order() {
        let store = Store::open(tmpdir("run-trace-ids")).unwrap();
        store
            .insert_trace(&trace("second", 2000, Some("run-1")))
            .unwrap();
        store
            .insert_trace(&trace("first", 1000, Some("run-1")))
            .unwrap();
        store
            .insert_trace(&trace("other", 500, Some("run-2")))
            .unwrap();

        assert_eq!(store.run_trace_ids("run-1").unwrap(), ["first", "second"]);
        assert!(store.run_trace_ids("missing").unwrap().is_empty());
    }

    #[test]
    fn reopen_keeps_working() {
        let dir = tmpdir("reopen");
        {
            let store = Store::open(dir.clone()).unwrap();
            store
                .insert_trace(&trace("a", 1000, Some("run-1")))
                .unwrap();
        }
        let store = Store::open(dir).unwrap();
        store
            .insert_trace(&trace("b", 2000, Some("run-1")))
            .unwrap();
        let s = store.run_summary("run-1").unwrap();
        assert_eq!(s["trace_count"], 2);
    }

    #[test]
    fn run_keys_lifecycle() {
        let store = Store::open(tmpdir("run-keys")).unwrap();
        store
            .insert_run_key(
                "rk-aaaa1111",
                "aaaa1111bbbb2222cccc",
                "run",
                Some("run-1"),
                Some(r#"{"task":"demo"}"#),
                Some("demo job"),
                1000,
                Some(10_000),
            )
            .unwrap();
        store
            .insert_run_key(
                "rk-dddd4444",
                "dddd4444eeee5555ffff",
                "run",
                None,
                None,
                None,
                2000,
                None,
            )
            .unwrap();
        let k = store
            .lookup_run_key("aaaa1111bbbb2222cccc", 5000)
            .unwrap()
            .unwrap();
        assert_eq!(k["id"], "rk-aaaa1111");
        assert_eq!(k["run_id"], "run-1");
        assert_eq!(k["tags"]["task"], "demo");
        assert_eq!(k["label"], "demo job");
        assert_eq!(k["key_fingerprint"], "aaaa1111bbbb2222");
        assert_eq!(k["revoked"], false);
        assert!(store
            .lookup_run_key("aaaa1111bbbb2222cccc", 10_000)
            .unwrap()
            .is_none());
        assert!(store
            .lookup_run_key("dddd4444eeee5555ffff", 10_000)
            .unwrap()
            .is_some());
        assert!(store
            .lookup_run_key("unknown-hash", 5000)
            .unwrap()
            .is_none());
        store.touch_run_key("aaaa1111bbbb2222cccc", 3000).unwrap();
        store.touch_run_key("aaaa1111bbbb2222cccc", 4000).unwrap();
        let all = store.list_run_keys(true).unwrap();
        assert_eq!(all.len(), 2);
        let touched = all.iter().find(|r| r["id"] == "rk-aaaa1111").unwrap();
        assert_eq!(touched["use_count"], 2);
        assert_eq!(touched["last_used_ms"], 4000);
        assert_eq!(touched["tags"], json!({"task": "demo"}));
        assert_eq!(touched["kind"], "run");
        assert!(store.revoke_run_key("rk-aaaa").unwrap());
        assert!(!store.revoke_run_key("rk-zzzz").unwrap());
        assert!(store
            .lookup_run_key("aaaa1111bbbb2222cccc", 5000)
            .unwrap()
            .is_none());
        let active = store.list_run_keys(false).unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0]["id"], "rk-dddd4444");
        assert!(store
            .insert_run_key(
                "rk-ffff6666",
                "aaaa1111bbbb2222cccc",
                "run",
                None,
                None,
                None,
                1,
                None
            )
            .is_err());
    }

    #[test]
    fn delete_revoked_run_keys_keeps_active_rows() {
        let store = Store::open(tmpdir("delete-revoked-run-keys")).unwrap();
        for (id, hash) in [
            ("rk-active", "active1111bbbb2222cccc"),
            ("rk-revoked-1", "revoked111bbbb2222cccc"),
            ("rk-revoked-2", "revoked222bbbb2222cccc"),
        ] {
            store
                .insert_run_key(id, hash, "run", None, None, None, 1_000, None)
                .unwrap();
        }
        assert!(store.revoke_run_key("rk-revoked-1").unwrap());
        assert!(store.revoke_run_key("rk-revoked-2").unwrap());

        assert_eq!(store.delete_revoked_run_keys().unwrap(), 2);
        let remaining = store.list_run_keys(true).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0]["id"], "rk-active");
        assert_eq!(store.delete_revoked_run_keys().unwrap(), 0);
    }

    #[test]
    fn harness_run_keys_never_expire_and_revoke() {
        let store = Store::open(tmpdir("run-keys-harness")).unwrap();
        store
            .insert_run_key(
                "rk-harness1",
                "hhhh1111bbbb2222cccc",
                "harness",
                None,
                Some(r#"{"harness":"pi"}"#),
                Some("pi"),
                1000,
                None,
            )
            .unwrap();
        let k = store
            .lookup_run_key("hhhh1111bbbb2222cccc", i64::MAX)
            .unwrap()
            .unwrap();
        assert_eq!(k["kind"], "harness");
        assert_eq!(k["label"], "pi");
        assert_eq!(k["expires_ms"], Value::Null);
        assert_eq!(k["tags"], json!({"harness": "pi"}));
        let active = store.list_run_keys(false).unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0]["id"], "rk-harness1");
        assert!(store.revoke_run_key("rk-harness1").unwrap());
        assert!(store
            .lookup_run_key("hhhh1111bbbb2222cccc", i64::MAX)
            .unwrap()
            .is_none());
        assert!(store.list_run_keys(false).unwrap().is_empty());
    }

    #[test]
    fn run_keys_table_added_to_existing_db() {
        let dir = tmpdir("run-keys-migrate");
        let db_path = dir.join("alexandria.sqlite3");
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(
                "CREATE TABLE traces (id TEXT PRIMARY KEY, ts_request_ms INTEGER NOT NULL,
                   session_id TEXT, routed_model TEXT);",
            )
            .unwrap();
        }
        let store = Store::open(dir).unwrap();
        store
            .insert_run_key("rk-11112222", "hash-x", "run", None, None, None, 1000, None)
            .unwrap();
        assert_eq!(store.list_run_keys(true).unwrap().len(), 1);
    }

    #[test]
    fn migrates_old_schema() {
        let dir = tmpdir("migrate");
        let db_path = dir.join("alexandria.sqlite3");
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(
                "CREATE TABLE traces (
                   id TEXT PRIMARY KEY,
                   ts_request_ms INTEGER NOT NULL,
                   ts_response_ms INTEGER,
                   session_id TEXT, harness TEXT, client_format TEXT,
                   upstream_provider TEXT, upstream_format TEXT,
                   requested_model TEXT, routed_model TEXT,
                   method TEXT, path TEXT, status INTEGER, streamed INTEGER,
                   input_tokens INTEGER, cached_input_tokens INTEGER,
                   cache_creation_tokens INTEGER, output_tokens INTEGER,
                   reasoning_tokens INTEGER, cost_usd REAL, billing_bucket TEXT,
                   req_body_path TEXT, upstream_req_body_path TEXT, resp_body_path TEXT,
                   req_headers_json TEXT, resp_headers_json TEXT,
                   error TEXT, account_id TEXT
                 );
                 INSERT INTO traces (id, ts_request_ms) VALUES ('old', 500);",
            )
            .unwrap();
        }
        let store = Store::open(dir).unwrap();
        let mut t = trace("new", 1000, Some("run-1"));
        t.tags = Some(r#"{"k":"v"}"#.into());
        t.client_ip = Some("127.0.0.1".into());
        t.key_fingerprint = Some("deadbeefdeadbeef".into());
        store.insert_trace(&t).unwrap();
        let rows = store.search_traces(&TraceFilter::default()).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["run_id"], "run-1");
        assert_eq!(rows[1]["id"], "old");
        assert_eq!(rows[1]["run_id"], Value::Null);
        assert_eq!(rows[1]["reasoning_effort"], Value::Null);
        assert_eq!(rows[1]["thinking_budget"], Value::Null);
    }

    #[test]
    fn reasoning_effort_migration_is_idempotent_and_preserves_old_rows() {
        let dir = tmpdir("reasoning-effort-migrate");
        let db_path = dir.join("alexandria.sqlite3");
        Connection::open(&db_path)
            .unwrap()
            .execute_batch(
                "CREATE TABLE traces (
                   id TEXT PRIMARY KEY,
                   ts_request_ms INTEGER NOT NULL,
                   ts_response_ms INTEGER,
                   session_id TEXT, harness TEXT, client_format TEXT,
                   upstream_provider TEXT, upstream_format TEXT,
                   requested_model TEXT, routed_model TEXT,
                   method TEXT, path TEXT, status INTEGER, streamed INTEGER,
                   input_tokens INTEGER, cached_input_tokens INTEGER,
                   cache_creation_tokens INTEGER, output_tokens INTEGER,
                   reasoning_tokens INTEGER, cost_usd REAL, billing_bucket TEXT,
                   req_body_path TEXT, upstream_req_body_path TEXT, resp_body_path TEXT,
                   req_headers_json TEXT, resp_headers_json TEXT,
                   error TEXT, account_id TEXT
                 );
                 INSERT INTO traces (id, ts_request_ms, session_id, routed_model)
                   VALUES ('historic-effort', 100, 'historic-session', 'gpt-5');",
            )
            .unwrap();

        let store = Store::open(dir.clone()).unwrap();
        let historic = store.get_trace("historic-effort").unwrap().unwrap();
        assert_eq!(historic["reasoning_effort"], Value::Null);
        drop(store);

        let store = Store::open(dir).unwrap();
        let conn = store.conn.lock().unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('traces') WHERE name='reasoning_effort'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "reopening must not add reasoning_effort twice");
    }

    #[test]
    fn migration_is_idempotent_and_existing_trace_history_stays_readable() {
        let dir = tmpdir("subscription-identity-migrate");
        let db_path = dir.join("alexandria.sqlite3");
        // This is the pre-change traces schema as written by the current
        // released binary (all of its then-current trace columns, but no new
        // subscription_identity column).
        Connection::open(&db_path)
            .unwrap()
            .execute_batch(
                "CREATE TABLE traces (id TEXT PRIMARY KEY, ts_request_ms INTEGER NOT NULL,
              ts_response_ms INTEGER, session_id TEXT, harness TEXT, client_format TEXT,
              upstream_provider TEXT, upstream_format TEXT, requested_model TEXT, routed_model TEXT,
              method TEXT, path TEXT, status INTEGER, streamed INTEGER, input_tokens INTEGER,
              cached_input_tokens INTEGER, cache_creation_tokens INTEGER, output_tokens INTEGER,
              reasoning_tokens INTEGER, cost_usd REAL, billing_bucket TEXT, req_body_path TEXT,
              upstream_req_body_path TEXT, resp_body_path TEXT, req_headers_json TEXT,
              resp_headers_json TEXT, error TEXT, account_id TEXT, run_id TEXT, tags_json TEXT,
              client_ip TEXT, key_fingerprint TEXT, reasoning_effort TEXT, thinking_budget INTEGER);
             INSERT INTO traces (id, ts_request_ms, upstream_provider, routed_model, account_id)
              VALUES ('historic', 100, 'openai', 'gpt-5', 'openai-oauth-old');",
            )
            .unwrap();
        let store = Store::open(dir.clone()).unwrap();
        let old = KnownAccount::new(
            "openai-oauth-old",
            "openai",
            "old",
            "oauth",
            Some("openai:chatgpt-account:acct_123".into()),
            Some("madhava@example.com".into()),
        );
        store.tombstone_known_account(&old, 200).unwrap();
        let rows = store.search_traces(&TraceFilter::default()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["id"], "historic");
        assert_eq!(rows[0]["account"]["name"], "old");
        assert_eq!(rows[0]["account"]["removed"], true);
        drop(store);
        let store = Store::open(dir).unwrap();
        let conn = store.conn.lock().unwrap();
        let identity_cols: i64 = conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('traces') WHERE name='subscription_identity'", [], |r| r.get(0)
        ).unwrap();
        assert_eq!(
            identity_cols, 1,
            "reopening must not alter the schema again"
        );
    }

    #[test]
    fn removed_account_trace_reattaches_when_readded_under_a_new_nickname() {
        let store = Store::open(tmpdir("readd-identity")).unwrap();
        let identity = Some("openai:chatgpt-account:acct_123".into());
        let old = KnownAccount::new(
            "openai-oauth-personal",
            "openai",
            "personal",
            "oauth",
            identity.clone(),
            Some("madhava@example.com".into()),
        );
        store.upsert_known_account(&old).unwrap();
        let mut historic = trace("historic", 100, None);
        historic.account_id = Some(old.account_id.clone());
        historic.subscription_identity = identity.clone();
        store.insert_trace(&historic).unwrap();
        store.tombstone_known_account(&old, 200).unwrap();
        let removed = store.search_traces(&TraceFilter::default()).unwrap();
        assert_eq!(removed[0]["account"]["name"], "personal");
        assert_eq!(removed[0]["account"]["removed"], true);
        let readded = KnownAccount::new(
            "openai-api_key-work",
            "openai",
            "work",
            "api_key",
            identity,
            Some("madhava@example.com".into()),
        );
        store.upsert_known_account(&readded).unwrap();
        let rows = store.search_traces(&TraceFilter::default()).unwrap();
        assert_eq!(rows[0]["account"]["id"], "openai-api_key-work");
        assert_eq!(rows[0]["account"]["name"], "work");
        assert_eq!(rows[0]["account"]["removed"], false);
        let accounts = store.list_known_accounts().unwrap();
        assert!(accounts
            .iter()
            .any(|a| a["id"] == "openai-oauth-personal" && a["removed"] == true));
    }

    #[test]
    fn merging_same_email_duplicates_unifies_history_with_counts_adding_up() {
        let store = Store::open(tmpdir("merge-accounts")).unwrap();
        let identity = Some("anthropic:email:me@madhavajay.com".into());
        // The survivor: the pre-existing "default" claude account.
        let survivor = KnownAccount::new(
            "anthropic-oauth",
            "anthropic",
            "default",
            "oauth",
            identity.clone(),
            Some("me@madhavajay.com".into()),
        );
        // The duplicate: a second id created when the user re-authed the same
        // Anthropic subscription under a different local name.
        let dup = KnownAccount::new(
            "anthropic-oauth-2",
            "anthropic",
            "reauth",
            "oauth",
            identity.clone(),
            Some("me@madhavajay.com".into()),
        );
        store.upsert_known_account(&survivor).unwrap();
        store.upsert_known_account(&dup).unwrap();

        // Two traces on the survivor, three on the duplicate — history is split.
        for id in ["s1", "s2"] {
            let mut t = trace(id, 1_000, None);
            t.account_id = Some("anthropic-oauth".into());
            t.subscription_identity = identity.clone();
            store.insert_trace(&t).unwrap();
        }
        for id in ["d1", "d2", "d3"] {
            let mut t = trace(id, 2_000, None);
            t.account_id = Some("anthropic-oauth-2".into());
            t.subscription_identity = identity.clone();
            store.insert_trace(&t).unwrap();
        }
        // A failover trace on the duplicate exercises served/original columns.
        let mut failover = trace("d-failover", 2_500, None);
        failover.account_id = Some("anthropic-oauth-2".into());
        failover.served_account_id = Some("anthropic-oauth-2".into());
        failover.original_account_id = Some("anthropic-oauth-2".into());
        failover.subscription_identity = identity.clone();
        store.insert_trace(&failover).unwrap();
        // Heartbeats on both accounts.
        store
            .insert_heartbeat(
                1_000,
                "anthropic",
                Some("anthropic-oauth"),
                true,
                Some(200),
                5,
                "ok",
            )
            .unwrap();
        store
            .insert_heartbeat(
                2_000,
                "anthropic",
                Some("anthropic-oauth-2"),
                true,
                Some(200),
                6,
                "ok",
            )
            .unwrap();

        let before = store.account_analytics(0, 60_000).unwrap();
        let total_requests_before: i64 = before["by_account"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["requests"].as_i64().unwrap())
            .sum();
        assert_eq!(total_requests_before, 6);

        let counts = store
            .merge_accounts("anthropic-oauth-2", "anthropic-oauth")
            .unwrap();
        assert_eq!(counts.traces_account_id, 4, "4 dup traces re-keyed");
        assert_eq!(counts.traces_served_account_id, 1);
        assert_eq!(counts.traces_original_account_id, 1);
        assert_eq!(counts.heartbeats, 1);
        assert_eq!(counts.known_accounts_removed, 1);

        // Every trace now points at the survivor — nothing left on the dup.
        let conn = store.conn.lock().unwrap();
        let dup_traces: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM traces WHERE account_id='anthropic-oauth-2'
                   OR served_account_id='anthropic-oauth-2' OR original_account_id='anthropic-oauth-2'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(dup_traces, 0, "no trace references the merged-away id");
        let survivor_traces: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM traces WHERE account_id='anthropic-oauth'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(survivor_traces, 6, "both histories now sit on the survivor");
        let dup_heartbeats: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM heartbeats WHERE account_id='anthropic-oauth-2'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(dup_heartbeats, 0);
        drop(conn);

        // The catalogue lists only the survivor, and its unified trace count is
        // the sum of both accounts — no orphaned/double-counted rows.
        let known = store.list_known_accounts().unwrap();
        assert_eq!(known.len(), 1);
        assert_eq!(known[0]["id"], "anthropic-oauth");
        assert_eq!(known[0]["trace_count"], 6);
        assert!(store.orphaned_trace_groups().unwrap().is_empty());

        // Usage rollups now attribute every request to the single survivor.
        let after = store.account_analytics(0, 60_000).unwrap();
        let by_account = after["by_account"].as_array().unwrap();
        assert_eq!(by_account.len(), 1);
        assert_eq!(by_account[0]["account_id"], "anthropic-oauth");
        assert_eq!(by_account[0]["requests"], 6);

        // Idempotent: a second merge of the same pair moves nothing.
        let again = store
            .merge_accounts("anthropic-oauth-2", "anthropic-oauth")
            .unwrap();
        assert_eq!(again, MergeCounts::default());
    }

    #[test]
    fn merging_an_account_into_itself_is_rejected() {
        let store = Store::open(tmpdir("merge-self")).unwrap();
        assert!(store
            .merge_accounts("anthropic-oauth", "anthropic-oauth")
            .is_err());
    }

    #[test]
    fn traces_reattach_plan_is_a_noop_without_confirmation() {
        let store = Store::open(tmpdir("reattach-confirmation")).unwrap();
        let mut orphan = trace("orphan", 100, None);
        orphan.account_id = Some("openai-oauth-removed".into());
        store.insert_trace(&orphan).unwrap();
        let target = KnownAccount::new(
            "openai-oauth-new",
            "openai",
            "new",
            "oauth",
            Some("openai:chatgpt-account:acct_456".into()),
            Some("new@example.com".into()),
        );
        assert_eq!(
            store.orphaned_trace_groups().unwrap()[0]["account_id"],
            "openai-oauth-removed"
        );
        assert_eq!(
            store
                .reattach_orphaned_traces("openai-oauth-removed", &target, false)
                .unwrap(),
            0
        );
        assert!(
            store.search_traces(&TraceFilter::default()).unwrap()[0]["subscription_identity"]
                .is_null()
        );
        assert_eq!(
            store
                .reattach_orphaned_traces("openai-oauth-removed", &target, true)
                .unwrap(),
            1
        );
        assert_eq!(
            store.search_traces(&TraceFilter::default()).unwrap()[0]["account"]["id"],
            "openai-oauth-new"
        );
    }
}

fn seed_pricing(conn: &Connection) -> Result<()> {
    let models: Vec<Value> = serde_json::from_str(include_str!("models.json"))?;
    for m in models {
        conn.execute(
            "INSERT OR IGNORE INTO pricing (model, input_per_m, cached_input_per_m, cache_creation_per_m, output_per_m)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                m["model"].as_str(),
                m["input_per_m"].as_f64(),
                m["cached_input_per_m"].as_f64(),
                m["cache_creation_per_m"].as_f64(),
                m["output_per_m"].as_f64(),
            ],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod catalog_tests {
    use super::*;

    #[test]
    fn anthropic_models_follow_the_shared_catalogue() {
        let catalogue: Vec<Value> = serde_json::from_str(include_str!("models.json")).unwrap();
        let expected: Vec<String> = catalogue
            .into_iter()
            .filter_map(|entry| entry["model"].as_str().map(str::to_string))
            .filter_map(|model| {
                let (provider, routed) = route_model(&model);
                (provider == Some(Provider::Anthropic)).then_some(routed)
            })
            .collect();
        let actual = anthropic_catalog_models();
        for model in expected {
            assert!(actual.contains(&model), "missing catalogue model {model}");
        }
        assert!(actual.contains(&"claude-fable-5".to_string()));
    }
}
