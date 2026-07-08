use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use alex_core::{Pricing, TraceRecord};
use anyhow::{Context, Result};
use chrono::Utc;
use flate2::write::GzEncoder;
use flate2::Compression;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

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
  account_id        TEXT,
  run_id            TEXT,
  tags_json         TEXT,
  client_ip         TEXT,
  key_fingerprint   TEXT
);
CREATE INDEX IF NOT EXISTS traces_session ON traces(session_id);
CREATE INDEX IF NOT EXISTS traces_ts ON traces(ts_request_ms);
CREATE INDEX IF NOT EXISTS traces_model ON traces(routed_model);

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
"#;

const TRACE_COLS: &str = "id, ts_request_ms, ts_response_ms, harness, client_format, upstream_provider,
     requested_model, routed_model, status, streamed,
     input_tokens, cached_input_tokens, cache_creation_tokens, output_tokens, reasoning_tokens,
     cost_usd, billing_bucket, error, session_id, resp_body_path,
     upstream_format, req_body_path, upstream_req_body_path, req_headers_json, resp_headers_json,
     account_id, run_id, tags_json, client_ip, key_fingerprint";

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
        "upstream_format": r.get::<_, Option<String>>(20)?,
        "req_body_path": r.get::<_, Option<String>>(21)?,
        "upstream_req_body_path": r.get::<_, Option<String>>(22)?,
        "req_headers_json": r.get::<_, Option<String>>(23)?,
        "resp_headers_json": r.get::<_, Option<String>>(24)?,
        "account_id": r.get::<_, Option<String>>(25)?,
        "run_id": r.get::<_, Option<String>>(26)?,
        "tags_json": r.get::<_, Option<String>>(27)?,
        "client_ip": r.get::<_, Option<String>>(28)?,
        "key_fingerprint": r.get::<_, Option<String>>(29)?,
        "latency_ms": ts_response_ms.map(|t| t - ts_request_ms),
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
    ] {
        if let Err(e) = conn.execute_batch(&format!("ALTER TABLE traces ADD COLUMN {col}")) {
            if !e.to_string().contains("duplicate column name") {
                return Err(e.into());
            }
        }
    }
    conn.execute_batch("CREATE INDEX IF NOT EXISTS traces_run ON traces(run_id)")?;
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
    pub path: Option<String>,
    pub harness: Option<String>,
    pub status: Option<i64>,
    pub errors_only: bool,
    pub key_fingerprint: Option<String>,
    pub limit: usize,
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
            path: None,
            harness: None,
            status: None,
            errors_only: false,
            key_fingerprint: None,
            limit: DEFAULT_SEARCH_LIMIT,
        }
    }
}

pub struct Store {
    conn: Mutex<Connection>,
    pub data_dir: PathBuf,
}

impl Store {
    pub fn open(data_dir: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&data_dir)?;
        let db_path = data_dir.join("alexandria.sqlite3");
        let conn =
            Connection::open(&db_path).with_context(|| format!("opening sqlite at {db_path:?}"))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.execute_batch(SCHEMA)?;
        migrate_traces(&conn)?;
        seed_pricing(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            data_dir,
        })
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

    pub fn insert_trace(&self, t: &TraceRecord) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            r#"INSERT OR REPLACE INTO traces (
                id, ts_request_ms, ts_response_ms, session_id, harness, client_format,
                upstream_provider, upstream_format, requested_model, routed_model,
                method, path, status, streamed,
                input_tokens, cached_input_tokens, cache_creation_tokens, output_tokens, reasoning_tokens,
                cost_usd, billing_bucket,
                req_body_path, upstream_req_body_path, resp_body_path,
                req_headers_json, resp_headers_json, error, account_id,
                run_id, tags_json, client_ip, key_fingerprint
            ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25,?26,?27,?28,?29,?30,?31,?32)"#,
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
                t.run_id,
                t.tags,
                t.client_ip,
                t.key_fingerprint,
            ],
        )?;
        Ok(())
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
        if let Some(k) = &f.key_fingerprint {
            sql.push_str(" AND key_fingerprint = ?");
            args.push(k.clone());
        }
        sql.push_str(" ORDER BY ts_request_ms DESC LIMIT ?");
        args.push(effective_limit(f.limit).to_string());
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(args.iter()), trace_row_json)?;
        Ok(rows.filter_map(|r| r.ok()).collect())
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
                    GROUP_CONCAT(tags_json, char(31))
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
            Ok(json!({
                "session_id": r.get::<_, String>(0)?,
                "run_id": r.get::<_, Option<String>>(1)?,
                "first_ts_ms": r.get::<_, Option<i64>>(2)?,
                "last_ts_ms": r.get::<_, Option<i64>>(3)?,
                "trace_count": r.get::<_, i64>(4)?,
                "models": models,
                "harness": r.get::<_, Option<String>>(6)?,
                "total_input_tokens": r.get::<_, i64>(7)?,
                "total_output_tokens": r.get::<_, i64>(8)?,
                "total_cost_usd": r.get::<_, f64>(9)?,
                "errors": r.get::<_, i64>(10)?,
                "last_status": r.get::<_, Option<i64>>(11)?,
                "tags": tags,
            }))
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
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
        let conn = self.conn.lock().unwrap();
        let paths: Option<(Option<String>, Option<String>, Option<String>)> = conn
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
        conn.execute("DELETE FROM traces WHERE id = ?1", params![id])?;
        Ok([req, upstream, resp].into_iter().flatten().collect())
    }

    pub fn run_summary(&self, run_id: &str) -> Result<Value> {
        let conn = self.conn.lock().unwrap();
        let (trace_count, first_ts_ms, last_ts_ms, total_input, total_output, total_cost, errors) =
            conn.query_row(
                "SELECT COUNT(*), MIN(ts_request_ms), MAX(ts_request_ms),
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
                        r.get::<_, i64>(3)?,
                        r.get::<_, i64>(4)?,
                        r.get::<_, f64>(5)?,
                        r.get::<_, i64>(6)?,
                    ))
                },
            )?;
        let mut status_counts = serde_json::Map::new();
        let mut stmt = conn.prepare(
            "SELECT status, COUNT(*) FROM traces WHERE run_id = ?1 GROUP BY status",
        )?;
        let pairs = stmt.query_map(params![run_id], |r| {
            Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, i64>(1)?))
        })?;
        for pair in pairs.flatten() {
            let key = pair.0.map(|s| s.to_string()).unwrap_or_else(|| "none".into());
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
            let bucket = row["billing_bucket"].as_str().unwrap_or("unknown").to_string();
            *buckets.entry(bucket).or_default() += row["cost_usd"].as_f64().unwrap_or(0.0);
        }
        Ok(json!({
            "since_ms": since_ms,
            "totals": {"requests": requests, "cost_usd": cost, "errors": errors, "cost_by_bucket": buckets},
            "by_model": rows,
        }))
    }

    pub fn write_body(&self, trace_id: &str, kind: &str, bytes: &[u8]) -> Result<String> {
        let date = Utc::now().format("%Y-%m-%d").to_string();
        let dir = self.data_dir.join("bodies").join(&date);
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{trace_id}.{kind}.gz"));
        let file = std::fs::File::create(&path)?;
        let mut enc = GzEncoder::new(file, Compression::default());
        enc.write_all(bytes)?;
        enc.finish()?;
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
        assert_eq!(
            s["models"],
            json!(["claude-haiku-4-5", "gpt-5.5"])
        );
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
        let mut b = trace("b", 2000, None);
        b.session_id = Some("ses_1".into());
        b.status = Some(500);
        b.error = Some("boom".into());
        b.routed_model = Some("gpt-5.5".into());
        b.tags = Some(r#"{"case":"x1"}"#.into());
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
        assert_eq!(s1["total_input_tokens"], 20);
        assert_eq!(s1["total_output_tokens"], 10);
        assert_eq!(s1["errors"], 1);
        assert_eq!(s1["last_status"], 500);
        assert_eq!(s1["tags"]["suite"], "swebench");
        assert_eq!(s1["tags"]["case"], "x1");
        let models: Vec<String> = s1["models"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m.as_str().unwrap().to_string())
            .collect();
        assert!(models.contains(&"claude-haiku-4-5".to_string()));
        assert!(models.contains(&"gpt-5.5".to_string()));
        let recent = store.sessions(Some(3000), 0).unwrap();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0]["session_id"], "ses_2");
        let limited = store.sessions(None, 1).unwrap();
        assert_eq!(limited.len(), 1);
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
    fn reopen_keeps_working() {
        let dir = tmpdir("reopen");
        {
            let store = Store::open(dir.clone()).unwrap();
            store.insert_trace(&trace("a", 1000, Some("run-1"))).unwrap();
        }
        let store = Store::open(dir).unwrap();
        store.insert_trace(&trace("b", 2000, Some("run-1"))).unwrap();
        let s = store.run_summary("run-1").unwrap();
        assert_eq!(s["trace_count"], 2);
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
