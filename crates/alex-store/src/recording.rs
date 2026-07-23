use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{bail, Context, Result};
use chrono::{SecondsFormat, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::{Store, TraceBodyKind, TraceFilter};

const MAX_FIXTURE_BODY_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct RecordFixturesOptions {
    pub provider: String,
    pub trace_ids: Vec<String>,
    pub since_ms: Option<i64>,
    pub limit: usize,
    pub out_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RecordedFixture {
    pub trace_id: String,
    pub metadata_path: PathBuf,
    pub request_body_path: PathBuf,
    pub response_body_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RecordFixturesReport {
    pub fixtures: Vec<RecordedFixture>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FixtureRequest {
    method: String,
    path: String,
    headers: Value,
    body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FixtureResponse {
    body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FixtureMetadata {
    status: u16,
    headers: Value,
    provider: String,
    format: String,
    recorded_at: String,
    provenance: String,
    trace_id: String,
    surface: String,
    outcome: String,
    request: FixtureRequest,
    response: FixtureResponse,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, PartialOrd, Ord)]
struct InventoryRow {
    provider: String,
    surface: String,
    outcome: String,
    provenance: String,
}

pub fn record_fixtures(
    store: &Store,
    options: &RecordFixturesOptions,
) -> Result<RecordFixturesReport> {
    let provider = fixture_provider(&options.provider);
    let upstream_provider = store_provider(&options.provider);
    if provider.is_empty()
        || !provider
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        bail!("provider must contain only letters, numbers, '-' or '_'");
    }
    if options.trace_ids.is_empty() && options.limit == 0 {
        bail!("fixture trace limit must be positive");
    }
    let traces = if options.trace_ids.is_empty() {
        store.search_traces(&TraceFilter {
            since_ms: options.since_ms,
            provider: Some(upstream_provider.clone()),
            limit: options.limit,
            ..TraceFilter::default()
        })?
    } else {
        let mut traces = Vec::with_capacity(options.trace_ids.len());
        for id in &options.trace_ids {
            let trace = store
                .get_trace(id)?
                .with_context(|| format!("trace {id} was not found"))?;
            let actual = trace["upstream_provider"].as_str().unwrap_or_default();
            if actual != upstream_provider {
                bail!(
                    "trace {id} belongs to provider {actual}, not {}",
                    options.provider
                );
            }
            traces.push(trace);
        }
        traces
    };

    if traces.is_empty() {
        bail!("no matching traces for provider {}", options.provider);
    }

    let mut fixtures = Vec::with_capacity(traces.len());
    for trace in traces {
        fixtures.push(export_trace(store, &trace, &provider, &options.out_dir)?);
    }
    Ok(RecordFixturesReport { fixtures })
}

fn export_trace(
    store: &Store,
    trace: &Value,
    provider: &str,
    out: &Path,
) -> Result<RecordedFixture> {
    let id = trace["id"].as_str().context("trace row omitted id")?;
    let request_body = read_request_body(store, trace, id)?;
    let response_body =
        match store.read_trace_body(id, TraceBodyKind::Response, MAX_FIXTURE_BODY_BYTES)? {
            Some(body) => body.bytes,
            None if trace["resp_body_path"].is_null() && trace["resp_body_lar"].is_null() => {
                Vec::new()
            }
            None => bail!("trace {id} has an unreadable captured response body"),
        };
    let request_headers = sanitize_headers(&parse_headers(&trace["req_headers_json"])?)?;
    let response_headers = sanitize_headers(&parse_headers(&trace["resp_headers_json"])?)?;
    let response_content_type = header_value(&response_headers, "content-type");
    let request_content_type = header_value(&request_headers, "content-type");
    let streamed = trace["streamed"].as_bool().unwrap_or(false)
        || response_content_type
            .as_deref()
            .is_some_and(|value| value.to_ascii_lowercase().contains("text/event-stream"));
    let (request_body, request_extension) =
        sanitize_wire_body(&request_body, request_content_type.as_deref(), false)?;
    let (response_body, response_extension) =
        sanitize_wire_body(&response_body, response_content_type.as_deref(), streamed)?;
    let path = trace["path"].as_str().unwrap_or("/");
    let surface = classify_surface(path);
    let status = trace["status"]
        .as_u64()
        .and_then(|value| u16::try_from(value).ok())
        .context("trace row omitted a valid response status")?;
    let outcome = classify_outcome(status, streamed, &response_body);
    let trace_id = stable_token("trace", id);
    let stem = format!("{}-{}", outcome, short_hash(id).to_ascii_lowercase());
    let fixture_dir = out.join(provider).join(&surface);
    fs::create_dir_all(&fixture_dir)
        .with_context(|| format!("creating fixture directory {}", fixture_dir.display()))?;
    let request_name = format!("{stem}.request.{request_extension}");
    let response_name = format!("{stem}.response.{response_extension}");
    let metadata_name = format!("{stem}.meta.json");
    let request_body_path = fixture_dir.join(&request_name);
    let response_body_path = fixture_dir.join(&response_name);
    let metadata_path = fixture_dir.join(&metadata_name);
    fs::write(&request_body_path, request_body)
        .with_context(|| format!("writing {}", request_body_path.display()))?;
    fs::write(&response_body_path, response_body)
        .with_context(|| format!("writing {}", response_body_path.display()))?;

    let recorded_at = trace["ts_request_ms"]
        .as_i64()
        .and_then(chrono::DateTime::<Utc>::from_timestamp_millis)
        .unwrap_or_else(Utc::now)
        .to_rfc3339_opts(SecondsFormat::Millis, true);
    let metadata = FixtureMetadata {
        status,
        headers: response_headers,
        provider: provider.to_string(),
        format: trace["upstream_format"]
            .as_str()
            .or_else(|| trace["client_format"].as_str())
            .unwrap_or("unknown")
            .to_string(),
        recorded_at,
        provenance: "recorded".into(),
        trace_id: trace_id.clone(),
        surface,
        outcome,
        request: FixtureRequest {
            method: trace["method"].as_str().unwrap_or("POST").to_string(),
            path: sanitize_path(path),
            headers: request_headers,
            body: request_name,
        },
        response: FixtureResponse {
            body: response_name,
        },
    };
    fs::write(&metadata_path, serde_json::to_vec_pretty(&metadata)?)
        .with_context(|| format!("writing {}", metadata_path.display()))?;
    Ok(RecordedFixture {
        trace_id,
        metadata_path,
        request_body_path,
        response_body_path,
    })
}

fn read_request_body(store: &Store, trace: &Value, id: &str) -> Result<Vec<u8>> {
    if !trace["upstream_req_body_path"].is_null() || !trace["upstream_req_body_lar"].is_null() {
        if let Some(body) =
            store.read_trace_body(id, TraceBodyKind::UpstreamRequest, MAX_FIXTURE_BODY_BYTES)?
        {
            return Ok(body.bytes);
        }
    }
    match store.read_trace_body(id, TraceBodyKind::Request, MAX_FIXTURE_BODY_BYTES)? {
        Some(body) => Ok(body.bytes),
        None if trace["req_body_path"].is_null() && trace["req_body_lar"].is_null() => {
            Ok(Vec::new())
        }
        None => bail!("trace {id} has an unreadable captured request body"),
    }
}

fn parse_headers(value: &Value) -> Result<Value> {
    match value {
        Value::String(raw) => serde_json::from_str(raw).context("decoding stored trace headers"),
        Value::Null => Ok(json!({})),
        other => Ok(other.clone()),
    }
}

pub fn sanitize_headers(headers: &Value) -> Result<Value> {
    let object = headers
        .as_object()
        .context("stored trace headers must be a JSON object")?;
    let mut sanitized = Map::new();
    for (name, value) in object {
        let normalized = name.to_ascii_lowercase();
        let value = match value {
            Value::String(value) => Value::String(sanitize_header_value(&normalized, value)),
            Value::Array(values) => Value::Array(
                values
                    .iter()
                    .map(|value| {
                        value
                            .as_str()
                            .map(|value| Value::String(sanitize_header_value(&normalized, value)))
                            .unwrap_or(Value::Null)
                    })
                    .collect(),
            ),
            _ => continue,
        };
        sanitized.insert(normalized, value);
    }
    Ok(Value::Object(sanitized))
}

fn sanitize_header_value(name: &str, value: &str) -> String {
    match classify_sensitive_name(name) {
        Some("authorization") => format!("Bearer {}", stable_token("sk", value)),
        Some(kind) => stable_token(kind, value),
        None => sanitize_text(value),
    }
}

fn classify_sensitive_name(name: &str) -> Option<&'static str> {
    let name = name.to_ascii_lowercase().replace('_', "-");
    if name == "authorization" || name == "proxy-authorization" {
        Some("authorization")
    } else if name == "x-api-key"
        || name == "x-goog-api-key"
        || name.ends_with("-token")
        || name == "api-key"
    {
        Some("sk")
    } else if name == "cookie" || name == "set-cookie" {
        Some("cookie")
    } else if name.contains("organization") || name == "org-id" || name == "x-org-id" {
        Some("org")
    } else if name.contains("account-id") || name == "account" {
        Some("acct")
    } else if name.contains("request-id") || name == "request-id" {
        Some("req")
    } else if name == "email" || name.ends_with("-email") {
        Some("email")
    } else {
        None
    }
}

pub fn sanitize_json_body(body: &[u8]) -> Result<Vec<u8>> {
    let mut value: Value = serde_json::from_slice(body).context("decoding JSON fixture body")?;
    sanitize_json_value(&mut value, None);
    Ok(serde_json::to_vec_pretty(&value)?)
}

fn sanitize_json_value(value: &mut Value, field: Option<&str>) {
    if let Some(kind) = field.and_then(classify_json_field) {
        if !matches!(value, Value::Object(_) | Value::Array(_) | Value::Null) {
            let original = value
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| value.to_string());
            *value = Value::String(stable_token(kind, &original));
            return;
        }
    }
    match value {
        Value::Object(object) => {
            let original = std::mem::take(object);
            for (key, mut value) in original {
                sanitize_json_value(&mut value, Some(&key));
                object.insert(sanitize_text(&key), value);
            }
        }
        Value::Array(values) => {
            for value in values {
                sanitize_json_value(value, field);
            }
        }
        Value::String(string) => {
            *string = sanitize_text(string);
        }
        _ => {}
    }
}

fn classify_json_field(field: &str) -> Option<&'static str> {
    let field = field.to_ascii_lowercase().replace('-', "_");
    if [
        "authorization",
        "api_key",
        "apikey",
        "access_token",
        "refresh_token",
        "client_secret",
        "password",
        "secret",
    ]
    .contains(&field.as_str())
    {
        Some("sk")
    } else if field == "cookie" || field == "set_cookie" {
        Some("cookie")
    } else if field == "email" || field.ends_with("_email") {
        Some("email")
    } else if field.contains("organization_id") || field == "org_id" {
        Some("org")
    } else if field.contains("account_id") || field == "account" {
        Some("acct")
    } else if field.contains("request_id") {
        Some("req")
    } else {
        None
    }
}

pub fn sanitize_sse_body(body: &[u8]) -> Result<Vec<u8>> {
    let text = std::str::from_utf8(body).context("SSE fixture is not UTF-8")?;
    let mut output = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let newline = if line.ends_with("\r\n") {
            "\r\n"
        } else if line.ends_with('\n') {
            "\n"
        } else {
            ""
        };
        let content = line.strip_suffix(newline).unwrap_or(line);
        if let Some(payload) = content.strip_prefix("data:") {
            let spaces = payload.len() - payload.trim_start_matches([' ', '\t']).len();
            let prefix_len = "data:".len() + spaces;
            let data = &content[prefix_len..];
            output.push_str(&content[..prefix_len]);
            if data == "[DONE]" || data.is_empty() {
                output.push_str(data);
            } else if let Ok(mut value) = serde_json::from_str::<Value>(data) {
                sanitize_json_value(&mut value, None);
                output.push_str(&serde_json::to_string(&value)?);
            } else {
                output.push_str(&sanitize_text(data));
            }
        } else {
            output.push_str(&sanitize_text(content));
        }
        output.push_str(newline);
    }
    Ok(output.into_bytes())
}

fn sanitize_wire_body(
    body: &[u8],
    content_type: Option<&str>,
    streamed: bool,
) -> Result<(Vec<u8>, &'static str)> {
    let content_type = content_type.unwrap_or_default().to_ascii_lowercase();
    if body.is_empty() {
        return Ok((Vec::new(), "txt"));
    }
    let looks_like_sse = body.starts_with(b"data:")
        || body.starts_with(b"event:")
        || body.windows(6).any(|window| window == b"\ndata:");
    if content_type.contains("text/event-stream") || (streamed && looks_like_sse) {
        return Ok((sanitize_sse_body(body)?, "sse"));
    }
    if content_type.contains("json") || serde_json::from_slice::<Value>(body).is_ok() {
        return Ok((sanitize_json_body(body)?, "json"));
    }
    let text = std::str::from_utf8(body).context("text fixture body is not UTF-8")?;
    Ok((sanitize_text(text).into_bytes(), "txt"))
}

fn sanitize_text(input: &str) -> String {
    let mut output = replace_matches(input, email_regex(), "email");
    output = replace_matches(&output, account_regex(), "acct");
    output = replace_matches(&output, organization_regex(), "org");
    output = replace_matches(&output, request_regex(), "req");
    output = replace_matches(&output, api_key_regex(), "sk");
    output
}

fn sanitize_path(path: &str) -> String {
    let (path, query) = path.split_once('?').unwrap_or((path, ""));
    let path = sanitize_text(path);
    if query.is_empty() {
        return path;
    }
    let query = query
        .split('&')
        .map(|part| {
            let (name, value) = part.split_once('=').unwrap_or((part, ""));
            let query_name = name.to_ascii_lowercase().replace('_', "-");
            let sanitized = classify_sensitive_name(name)
                .or_else(|| {
                    matches!(
                        query_name.as_str(),
                        "key" | "token" | "access-token" | "refresh-token" | "api-key"
                    )
                    .then_some("sk")
                })
                .map(|kind| stable_token(kind, value))
                .unwrap_or_else(|| sanitize_text(value));
            if part.contains('=') {
                format!("{name}={sanitized}")
            } else {
                sanitize_text(name)
            }
        })
        .collect::<Vec<_>>()
        .join("&");
    format!("{path}?{query}")
}

fn replace_matches(input: &str, regex: &Regex, kind: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut end = 0;
    for found in regex.find_iter(input) {
        output.push_str(&input[end..found.start()]);
        output.push_str(&stable_token(kind, found.as_str()));
        end = found.end();
    }
    output.push_str(&input[end..]);
    output
}

fn stable_token(kind: &str, input: &str) -> String {
    let hash = short_hash(input);
    match kind {
        "sk" | "authorization" => format!("sk-FAKE_{hash}"),
        "acct" => format!("acct_FAKE_{hash}"),
        "org" => format!("org_FAKE_{hash}"),
        "req" | "trace" => format!("{kind}_FAKE_{hash}"),
        "email" => format!("email_FAKE_{}@example.invalid", hash.to_ascii_lowercase()),
        "cookie" => format!("cookie_FAKE_{hash}"),
        _ => format!("FAKE_{hash}"),
    }
}

fn short_hash(input: &str) -> String {
    let digest = Sha256::digest(input.as_bytes());
    digest[..6]
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect()
}

fn email_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?i)[a-z0-9.!#$%&'*+/=?^_`{|}~-]+@[a-z0-9-]+(?:\.[a-z0-9-]+)+").unwrap()
    })
}

fn account_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"(?i)\b(?:acct|account|acc)[_-][a-z0-9_-]{4,}\b").unwrap())
}

fn organization_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"(?i)\borg[_-][a-z0-9_-]{4,}\b").unwrap())
}

fn request_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"(?i)\b(?:req|request)[_-][a-z0-9_-]{4,}\b").unwrap())
}

fn api_key_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"(?i)\bsk-[a-z0-9_-]{4,}\b").unwrap())
}

fn header_value(headers: &Value, name: &str) -> Option<String> {
    match headers.get(name)? {
        Value::String(value) => Some(value.clone()),
        Value::Array(values) => values.first()?.as_str().map(str::to_string),
        _ => None,
    }
}

fn classify_surface(path: &str) -> String {
    let lower = path.to_ascii_lowercase();
    if lower.contains("usage")
        || lower.contains("usages")
        || lower.contains("credits")
        || lower.contains("balance")
    {
        "usage".into()
    } else if lower.contains("models") {
        "models".into()
    } else if lower.contains("oauth/token")
        || lower.contains("deviceauth/token")
        || lower.contains("device_authorization")
    {
        "oauth-token".into()
    } else if lower.contains("profile")
        || lower.contains("userinfo")
        || lower.contains("loadcodeassist")
        || lower.contains("onboarduser")
    {
        "profile".into()
    } else {
        "model".into()
    }
}

fn classify_outcome(status: u16, streamed: bool, body: &[u8]) -> String {
    let text = String::from_utf8_lossy(body).to_ascii_lowercase();
    if text.contains("tool_use") || text.contains("tool_calls") || text.contains("function_call") {
        "tool-call".into()
    } else if matches!(status, 401 | 403) {
        "401".into()
    } else if status == 429 {
        "429".into()
    } else if status >= 500 {
        "5xx".into()
    } else if streamed {
        "ok-sse".into()
    } else if (200..400).contains(&status) {
        "ok".into()
    } else {
        status.to_string()
    }
}

fn store_provider(provider: &str) -> String {
    match provider.to_ascii_lowercase().as_str() {
        "openai-api" | "codex" | "codex-oauth" => "openai".into(),
        "gemini-api" | "gemini-code-assist" => "gemini".into(),
        "grok" => "xai".into(),
        other => other.to_string(),
    }
}

fn fixture_provider(provider: &str) -> String {
    provider.to_ascii_lowercase()
}

pub fn write_inventory(dir: &Path) -> Result<PathBuf> {
    fs::create_dir_all(dir)
        .with_context(|| format!("creating fixture directory {}", dir.display()))?;
    let mut actual: BTreeMap<(String, String, String), BTreeSet<String>> = BTreeMap::new();
    for path in metadata_files(dir)? {
        let metadata: Value = serde_json::from_slice(&fs::read(&path)?)
            .with_context(|| format!("decoding fixture sidecar {}", path.display()))?;
        let provider = metadata["provider"]
            .as_str()
            .context("fixture sidecar omitted provider")?;
        let surface = metadata["surface"]
            .as_str()
            .context("fixture sidecar omitted surface")?;
        let outcome = metadata["outcome"]
            .as_str()
            .context("fixture sidecar omitted outcome")?;
        let provenance = metadata["provenance"]
            .as_str()
            .context("fixture sidecar omitted provenance")?;
        if !matches!(provenance, "recorded" | "synthetic") {
            bail!(
                "fixture sidecar {} has unsupported provenance {provenance}",
                path.display()
            );
        }
        actual
            .entry((provider.into(), surface.into(), outcome.into()))
            .or_default()
            .insert(provenance.into());
    }

    let mut rows = BTreeSet::new();
    for ((provider, surface, outcome), provenance) in &actual {
        rows.insert(InventoryRow {
            provider: provider.clone(),
            surface: surface.clone(),
            outcome: outcome.clone(),
            provenance: provenance.iter().cloned().collect::<Vec<_>>().join(", "),
        });
    }
    for (provider, surface, outcome) in expected_inventory() {
        let key = (
            provider.to_string(),
            surface.to_string(),
            outcome.to_string(),
        );
        if !actual.contains_key(&key) {
            rows.insert(InventoryRow {
                provider: provider.into(),
                surface: surface.into(),
                outcome: outcome.into(),
                provenance: "missing".into(),
            });
        }
    }
    let mut markdown = String::from(
        "# Fixture inventory\n\n| Provider | Surface | Outcome | Provenance |\n| --- | --- | --- | --- |\n",
    );
    for row in rows {
        markdown.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            row.provider, row.surface, row.outcome, row.provenance
        ));
    }
    let path = dir.join("INVENTORY.md");
    fs::write(&path, markdown).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

fn metadata_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut pending = vec![dir.to_path_buf()];
    let mut paths = Vec::new();
    while let Some(current) = pending.pop() {
        for entry in fs::read_dir(&current)
            .with_context(|| format!("scanning fixture directory {}", current.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                pending.push(path);
            } else if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".meta.json"))
            {
                paths.push(path);
            }
        }
    }
    paths.sort();
    Ok(paths)
}

fn expected_inventory() -> Vec<(&'static str, &'static str, &'static str)> {
    const ALL: &[&str] = &["ok", "ok-sse", "tool-call", "401", "429", "5xx"];
    let specs: &[(&str, &[&str], &[&str])] = &[
        ("anthropic", ALL, &["usage", "oauth-token", "profile"]),
        ("openai-api", ALL, &["models", "oauth-token", "profile"]),
        ("codex-oauth", ALL, &["usage", "oauth-token", "profile"]),
        ("gemini-api", ALL, &["models", "oauth-token"]),
        ("gemini-code-assist", ALL, &["oauth-token", "profile"]),
        ("grok", ALL, &["usage", "oauth-token", "profile"]),
        ("kimi", ALL, &["usage", "oauth-token"]),
        ("openrouter", ALL, &["models"]),
        ("amp", &["401", "5xx"], &["usage"]),
        ("exo", &["ok", "ok-sse", "tool-call", "5xx"], &["models"]),
        (
            "cliproxyapi",
            &["ok", "ok-sse", "tool-call", "401", "5xx"],
            &["models"],
        ),
    ];
    let mut expected = Vec::new();
    for (provider, outcomes, surfaces) in specs {
        for outcome in *outcomes {
            expected.push((*provider, "model", *outcome));
        }
        for surface in *surfaces {
            expected.push((*provider, *surface, "ok"));
        }
    }
    expected
}

#[cfg(test)]
mod tests {
    use super::*;
    use alex_core::TraceRecord;

    fn temp_dir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "alex-recording-{name}-{}-{}",
            std::process::id(),
            short_hash(name)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn sanitizer_replaces_sensitive_headers_and_keeps_wire_headers() {
        let headers = json!({
            "Authorization": "Bearer sk-live-secret",
            "X-Api-Key": "key-secret",
            "Cookie": "session=private",
            "Set-Cookie": "session=private; Secure",
            "OpenAI-Organization": "org_private123",
            "ChatGPT-Account-Id": "acct_private123",
            "X-Request-Id": "req_private123",
            "Content-Type": "application/json",
            "Anthropic-Version": "2023-06-01",
            "Anthropic-Ratelimit-Unified-5h-Utilization": "0.5",
            "X-Model": "claude-sonnet-4-5"
        });
        let first = sanitize_headers(&headers).unwrap();
        let second = sanitize_headers(&headers).unwrap();
        assert_eq!(first, second);
        let text = first.to_string();
        assert!(!text.contains("live-secret"));
        assert!(!text.contains("private123"));
        assert_eq!(first["content-type"], "application/json");
        assert_eq!(first["anthropic-version"], "2023-06-01");
        assert_eq!(first["anthropic-ratelimit-unified-5h-utilization"], "0.5");
        assert_eq!(first["x-model"], "claude-sonnet-4-5");
    }

    #[test]
    fn sanitizer_replaces_accounts_and_emails_in_json_bodies() {
        let body = br#"{"account_id":"acct_private123","email":"person@example.com","nested":{"message":"contact person@example.com for org_private123"},"model":"gpt-5.6"}"#;
        let first = sanitize_json_body(body).unwrap();
        let second = sanitize_json_body(body).unwrap();
        assert_eq!(first, second);
        let text = String::from_utf8(first).unwrap();
        assert!(!text.contains("person@example.com"));
        assert!(!text.contains("private123"));
        assert!(text.contains("acct_FAKE_"));
        assert!(text.contains("email_FAKE_"));
        assert!(text.contains("gpt-5.6"));
    }

    #[test]
    fn sse_sanitization_preserves_frame_boundaries() {
        let body = b"event: message_start\r\ndata: {\"request_id\":\"req_private123\",\"email\":\"person@example.com\"}\r\n\r\ndata: [DONE]\n\n";
        let sanitized = sanitize_sse_body(body).unwrap();
        let original_boundaries: Vec<_> = body
            .windows(2)
            .enumerate()
            .filter_map(|(index, pair)| (pair == b"\n\n").then_some(index))
            .collect();
        let sanitized_boundaries: Vec<_> = sanitized
            .windows(2)
            .enumerate()
            .filter_map(|(index, pair)| (pair == b"\n\n").then_some(index))
            .collect();
        assert_eq!(original_boundaries.len(), sanitized_boundaries.len());
        assert_eq!(
            body.iter().filter(|byte| **byte == b'\n').count(),
            sanitized.iter().filter(|byte| **byte == b'\n').count()
        );
        let text = String::from_utf8(sanitized).unwrap();
        assert!(text.contains("event: message_start\r\n"));
        assert!(text.ends_with("data: [DONE]\n\n"));
        assert!(!text.contains("person@example.com"));
        assert!(!text.contains("req_private123"));
    }

    #[test]
    fn records_a_complete_transaction_from_the_trace_store() {
        let root = temp_dir("end-to-end");
        let store = Store::open(root.join("store")).unwrap();
        let request_path = store
            .write_body(
                "trace-secret-id",
                "request.json",
                br#"{"model":"claude-sonnet-4-5","account_id":"acct_private123","email":"person@example.com"}"#,
            )
            .unwrap();
        let upstream_request_path = store
            .write_body(
                "trace-secret-id",
                "upstream-request.json",
                br#"{"model":"claude-sonnet-4-5","account_id":"acct_upstream123"}"#,
            )
            .unwrap();
        let response_path = store
            .write_body(
                "trace-secret-id",
                "response.sse",
                b"data: {\"type\":\"message_start\",\"request_id\":\"req_private123\"}\n\ndata: [DONE]\n\n",
            )
            .unwrap();
        store
            .insert_trace(&TraceRecord {
                id: "trace-secret-id".into(),
                ts_request_ms: 1_700_000_000_000,
                ts_response_ms: Some(1_700_000_000_100),
                upstream_provider: Some("anthropic".into()),
                upstream_format: Some("anthropic".into()),
                method: Some("POST".into()),
                path: Some(
                    "/v1/messages?beta=true&key=sk-live-query&account_id=acct_query123".into(),
                ),
                status: Some(200),
                streamed: Some(true),
                req_body_path: Some(request_path),
                upstream_req_body_path: Some(upstream_request_path),
                resp_body_path: Some(response_path),
                req_headers_json: Some(
                    json!({
                        "authorization": "Bearer sk-live-secret",
                        "content-type": "application/json",
                        "anthropic-version": "2023-06-01"
                    })
                    .to_string(),
                ),
                resp_headers_json: Some(
                    json!({
                        "content-type": "text/event-stream",
                        "x-request-id": "req_private123"
                    })
                    .to_string(),
                ),
                ..TraceRecord::default()
            })
            .unwrap();

        let out = root.join("fixtures");
        let report = record_fixtures(
            &store,
            &RecordFixturesOptions {
                provider: "anthropic".into(),
                trace_ids: vec!["trace-secret-id".into()],
                since_ms: None,
                limit: 20,
                out_dir: out.clone(),
            },
        )
        .unwrap();
        assert_eq!(report.fixtures.len(), 1);
        let fixture = &report.fixtures[0];
        assert!(fixture
            .request_body_path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .ends_with(".request.json"));
        assert!(fixture
            .response_body_path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .ends_with(".response.sse"));
        assert!(fixture
            .metadata_path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .ends_with(".meta.json"));
        let request = fs::read_to_string(&fixture.request_body_path).unwrap();
        assert!(request.contains("claude-sonnet-4-5"));
        assert!(!request.contains("acct_upstream123"));
        assert!(!request.contains("person@example.com"));
        let response = fs::read_to_string(&fixture.response_body_path).unwrap();
        assert!(response.ends_with("data: [DONE]\n\n"));
        assert!(!response.contains("req_private123"));
        let metadata: Value =
            serde_json::from_slice(&fs::read(&fixture.metadata_path).unwrap()).unwrap();
        assert_eq!(metadata["status"], 200);
        assert_eq!(metadata["provider"], "anthropic");
        assert_eq!(metadata["format"], "anthropic");
        assert_eq!(metadata["provenance"], "recorded");
        assert_eq!(metadata["request"]["method"], "POST");
        let path = metadata["request"]["path"].as_str().unwrap();
        assert!(path.starts_with("/v1/messages?beta=true&key=sk-FAKE_"));
        assert!(path.contains("account_id=acct_FAKE_"));
        assert!(!path.contains("live-query"));
        assert!(!path.contains("query123"));
        assert_eq!(
            metadata["request"]["headers"]["anthropic-version"],
            "2023-06-01"
        );
        assert!(metadata["trace_id"]
            .as_str()
            .unwrap()
            .starts_with("trace_FAKE_"));
        assert!(!serde_json::to_string(&metadata)
            .unwrap()
            .contains("trace-secret-id"));
        assert_eq!(
            fs::read_dir(out.join("anthropic/model")).unwrap().count(),
            3
        );
    }

    #[test]
    fn inventory_marks_expected_absences_as_missing() {
        let root = temp_dir("inventory");
        let fixture = root.join("anthropic/model/ok.meta.json");
        fs::create_dir_all(fixture.parent().unwrap()).unwrap();
        fs::write(
            fixture,
            serde_json::to_vec(&json!({
                "provider": "anthropic",
                "surface": "model",
                "outcome": "ok",
                "provenance": "recorded"
            }))
            .unwrap(),
        )
        .unwrap();
        let inventory = write_inventory(&root).unwrap();
        let markdown = fs::read_to_string(inventory).unwrap();
        assert!(markdown.contains("| anthropic | model | ok | recorded |"));
        assert!(markdown.contains("| anthropic | model | 429 | missing |"));
    }
}
