use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use alex_auth::{now_ms, Account, Vault};
use alex_core::{
    compute_cost, conversation_root, parse_since, parse_sse_usage, parse_trace_tags, route_model,
    usage_from_json, ClientFormat, Provider, TraceRecord,
};
use alex_store::{Store, TraceFilter};
use axum::body::{Body, Bytes};
use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use futures_util::StreamExt;
use serde_json::{json, Value};
use tokio_stream::wrappers::ReceiverStream;

const ANTHROPIC_BASE: &str = "https://api.anthropic.com";
const OPENAI_BASE: &str = "https://api.openai.com";
const CODEX_BASE: &str = "https://chatgpt.com/backend-api/codex";
const XAI_BASE: &str = "https://cli-chat-proxy.grok.com/v1";
const GROK_CLIENT_VERSION: &str = "0.2.77";
const ANTHROPIC_OAUTH_BETA: &str = "oauth-2025-04-20";
const GEMINI_CODE_ASSIST_BASE: &str = "https://cloudcode-pa.googleapis.com";
const GEMINI_CODE_ASSIST_VERSION: &str = "v1internal";
const GEMINI_API_BASE: &str = "https://generativelanguage.googleapis.com";

#[derive(Debug, Clone)]
pub struct DarioActive {
    pub generation_id: String,
    pub base_url: String,
    pub api_key: String,
}

pub trait DarioRouter: Send + Sync {
    fn active(&self) -> Option<DarioActive>;
    fn begin(&self, generation_id: &str) -> Option<Box<dyn std::any::Any + Send>>;
    fn status(&self) -> Value;
    fn suspect(&self, generation_id: &str);
}

fn suspect_dario(state: &AppState, account: &Account) {
    if account.kind != "dario" {
        return;
    }
    if let (Some(dario), Some(gen)) = (&state.dario, account.id.strip_prefix("dario:")) {
        dario.suspect(gen);
    }
}

pub struct AppState {
    pub local_key: String,
    pub vault: Arc<Vault>,
    pub store: Arc<Store>,
    pub http: reqwest::Client,
    pub dario: Option<Arc<dyn DarioRouter>>,
    pub in_flight: std::sync::atomic::AtomicI64,
    pub started_ms: i64,
    pub base_url: String,
    pub anthropic_usage: std::sync::Mutex<UsageCache>,
    pub logins: alex_auth::sessions::LoginManager,
    pub run_keys: std::sync::RwLock<HashMap<String, CachedRunKey>>,
}

#[derive(Debug, Clone)]
pub struct CachedRunKey {
    pub run_id: Option<String>,
    pub tags_json: Option<String>,
    pub expires_ms: Option<i64>,
}

struct InFlight(Arc<AppState>);

impl InFlight {
    fn new(state: &Arc<AppState>) -> Self {
        state
            .in_flight
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Self(state.clone())
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        self.0
            .in_flight
            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

pub fn build_state(
    local_key: String,
    vault: Arc<Vault>,
    store: Arc<Store>,
    dario: Option<Arc<dyn DarioRouter>>,
    base_url: String,
) -> Arc<AppState> {
    let http = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .build()
        .expect("reqwest client");
    Arc::new(AppState {
        local_key,
        vault,
        store,
        http,
        dario,
        in_flight: std::sync::atomic::AtomicI64::new(0),
        started_ms: now_ms(),
        base_url,
        anthropic_usage: std::sync::Mutex::new(UsageCache::default()),
        logins: alex_auth::sessions::LoginManager::default(),
        run_keys: std::sync::RwLock::new(HashMap::new()),
    })
}

async fn require_local_key(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let ok = client_key(req.headers())
        .map(|k| k == state.local_key)
        .unwrap_or(false);
    if !ok {
        return error_response(
            StatusCode::UNAUTHORIZED,
            "admin routes require x-api-key: <local_key>",
        );
    }
    next.run(req).await
}

pub fn router(state: Arc<AppState>) -> Router {
    // Control-plane routes: gated by the local key so a LAN/0.0.0.0 bind
    // doesn't expose them. Run keys are NOT accepted here — a worker's run
    // key must not be able to mint or revoke run keys.
    let admin = Router::new()
        .route(
            "/admin/run-keys",
            get(admin_run_keys_list).post(admin_run_keys_create),
        )
        .route(
            "/admin/run-keys/{id}",
            axum::routing::delete(admin_run_keys_revoke),
        )
        .route("/admin/storage", get(admin_storage))
        .route("/admin/storage/prune", post(admin_storage_prune))
        .route("/admin/traces", get(admin_traces))
        .route("/admin/accounts", get(admin_accounts))
        .route("/admin/health", get(admin_health))
        .route("/admin/analytics", get(admin_analytics))
        .route("/admin/limits", get(admin_limits))
        .route("/admin/dario", get(admin_dario))
        .route("/admin/auth/import", post(admin_auth_import))
        .route("/admin/auth/login/start", post(admin_auth_login_start))
        .route("/admin/auth/login/complete", post(admin_auth_login_complete))
        .route("/admin/auth/login/{id}", get(admin_auth_login_status))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            require_local_key,
        ));

    Router::new()
        .route("/health", get(health))
        .route("/connect", get(connect_info))
        .route("/v1/models", get(models))
        .route("/v1/messages", post(anthropic_messages))
        .route("/v1/chat/completions", post(openai_chat))
        .route("/v1/responses", post(openai_responses))
        .route("/chat/completions", post(openai_chat))
        .route("/responses", post(openai_responses))
        .route("/v1beta/models/{model_action}", post(gemini_generate))
        .route("/traces/search", get(traces_search))
        .route("/traces/export.ndjson", get(traces_export))
        .route("/traces/sessions", get(traces_sessions))
        .route(
            "/traces/sessions/{session_id}/transcript",
            get(traces_session_transcript),
        )
        .route("/traces/{id}", get(trace_get).delete(trace_delete))
        .route("/traces/{id}/reply.md", get(trace_reply_md))
        .route("/traces/{id}/body/{kind}", get(trace_body))
        .route("/traces/runs/{run_id}", get(traces_run_summary))
        .route("/traces/runs/{run_id}/events", get(traces_run_events))
        .route("/traces/runs/{run_id}/export.ndjson", get(traces_run_export))
        .route("/traces/runs/{run_id}/artifacts", get(traces_run_artifacts))
        .merge(admin)
        .layer(axum::extract::DefaultBodyLimit::max(64 * 1024 * 1024))
        .with_state(state)
}

async fn admin_auth_import(
    State(state): State<Arc<AppState>>,
    body: Option<axum::Json<Value>>,
) -> Response {
    let source = body
        .as_ref()
        .and_then(|b| b.0["source"].as_str().or(b.0["provider"].as_str()))
        .unwrap_or("all")
        .to_string();
    match alex_auth::import_all(&state.vault, &source).await {
        Ok(outcomes) => {
            let items: Vec<Value> = outcomes
                .iter()
                .map(|o| {
                    json!({
                        "source": o.source,
                        "imported": o.imported,
                        "note": o.note,
                    })
                })
                .collect();
            axum::Json(json!({"outcomes": items})).into_response()
        }
        Err(e) => error_response(StatusCode::BAD_REQUEST, &e.to_string()),
    }
}

async fn admin_auth_login_start(
    State(state): State<Arc<AppState>>,
    body: axum::Json<Value>,
) -> Response {
    let Some(provider) = body.0["provider"].as_str() else {
        return error_response(StatusCode::BAD_REQUEST, "missing 'provider'");
    };
    match state.logins.start(state.vault.clone(), provider).await {
        Ok(snapshot) => axum::Json(snapshot).into_response(),
        Err(e) => error_response(StatusCode::BAD_REQUEST, &e.to_string()),
    }
}

async fn admin_auth_login_complete(
    State(state): State<Arc<AppState>>,
    body: axum::Json<Value>,
) -> Response {
    let (Some(id), Some(input)) = (body.0["login_id"].as_str(), body.0["input"].as_str()) else {
        return error_response(StatusCode::BAD_REQUEST, "missing 'login_id' or 'input'");
    };
    match state.logins.complete(state.vault.clone(), id, input).await {
        Ok(snapshot) => axum::Json(snapshot).into_response(),
        Err(e) => error_response(StatusCode::BAD_REQUEST, &e.to_string()),
    }
}

async fn admin_auth_login_status(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Response {
    match state.logins.status(&id).await {
        Some(snapshot) => axum::Json(snapshot).into_response(),
        None => error_response(StatusCode::NOT_FOUND, "unknown or expired login session"),
    }
}

async fn health(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    axum::Json(json!({
        "status": "ok",
        "service": "alexandria",
        "version": env!("CARGO_PKG_VERSION"),
        "in_flight": state.in_flight.load(std::sync::atomic::Ordering::SeqCst),
        "uptime_s": (now_ms() - state.started_ms) / 1000,
        "dario": state.dario.is_some(),
    }))
}

fn rewrite_host(base_url: &str, host: &str) -> String {
    let port = base_url.rsplit(':').next().unwrap_or("4100");
    format!("http://{host}:{port}")
}

pub fn connect_payload(base_url: &str, local_key: &str) -> (Value, String) {
    let base = base_url.trim_end_matches('/').to_string();
    let v1 = format!("{base}/v1");
    let exports = format!(
        "export ANTHROPIC_BASE_URL={base}\nexport ANTHROPIC_API_KEY={local_key}\n\
         export OPENAI_BASE_URL={v1}\nexport OPENAI_API_KEY={local_key}\n\
         export XAI_API_KEY={local_key}\nexport GROK_MODELS_BASE_URL={v1}\n\
         export GOOGLE_GEMINI_BASE_URL={base}\nexport GOOGLE_GENAI_API_VERSION=v1beta\n\
         export GEMINI_API_KEY={local_key}\nexport GEMINI_API_KEY_AUTH_MECHANISM=bearer\n\
         export GOOGLE_GENAI_USE_GCA=false\n"
    );
    let payload = json!({
        "service": "alexandria",
        "base_url": base,
        "api_key": local_key,
        "anthropic": {"base_url": base, "env": {"ANTHROPIC_BASE_URL": base, "ANTHROPIC_API_KEY": local_key}},
        "openai": {"base_url": v1, "env": {"OPENAI_BASE_URL": v1, "OPENAI_API_KEY": local_key}},
        "xai": {"base_url": v1, "env": {"XAI_API_KEY": local_key, "GROK_MODELS_BASE_URL": v1}},
        "gemini": {"base_url": base, "env": {
            "GOOGLE_GEMINI_BASE_URL": base,
            "GOOGLE_GENAI_API_VERSION": "v1beta",
            "GEMINI_API_KEY": local_key,
            "GEMINI_API_KEY_AUTH_MECHANISM": "bearer",
            "GOOGLE_GENAI_USE_GCA": "false",
        }},
        "exports": exports,
    });
    (payload, exports)
}

async fn connect_info(
    State(state): State<Arc<AppState>>,
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<std::net::SocketAddr>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    if !peer.ip().is_loopback() {
        return error_response(
            StatusCode::FORBIDDEN,
            "connection info is only served to loopback clients",
        );
    }
    let base = match q.get("host").map(String::as_str) {
        Some("docker") => rewrite_host(&state.base_url, "host.docker.internal"),
        Some(host) if !host.is_empty() => rewrite_host(&state.base_url, host),
        _ => state.base_url.clone(),
    };
    let (payload, exports) = connect_payload(&base, &state.local_key);
    if q.get("format").map(|f| f == "env").unwrap_or(false) {
        return Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "text/plain")
            .body(Body::from(exports))
            .unwrap_or_else(|e| error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()));
    }
    axum::Json(payload).into_response()
}

async fn models(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let mut ids = state.store.pricing_models();
    for (alias, _) in alex_core::model_aliases() {
        ids.push((*alias).to_string());
    }
    for id in ids.clone() {
        ids.push(format!("alexandria/{id}"));
    }
    let data: Vec<Value> = ids
        .into_iter()
        .map(|m| json!({"id": m, "object": "model", "owned_by": "alexandria"}))
        .collect();
    axum::Json(json!({"object": "list", "data": data}))
}

async fn admin_analytics(
    State(state): State<Arc<AppState>>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let minutes: i64 = q
        .get("since_minutes")
        .and_then(|s| s.parse().ok())
        .unwrap_or(60);
    match state.store.analytics(now_ms() - minutes * 60_000) {
        Ok(v) => axum::Json(v).into_response(),
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

const USAGE_CACHE_TTL_MS: i64 = 300_000;
const USAGE_BACKOFF_BASE_MS: i64 = 60_000;
const USAGE_BACKOFF_MAX_MS: i64 = 3_600_000;

#[derive(Default)]
pub struct UsageCache {
    fetched_at_ms: i64,
    entry: Option<Value>,
    cooldown_until_ms: i64,
    failures: u32,
}

fn usage_backoff_ms(failures: u32, retry_after_ms: Option<i64>) -> i64 {
    let exp = USAGE_BACKOFF_BASE_MS
        .saturating_mul(1i64 << failures.saturating_sub(1).min(6))
        .min(USAGE_BACKOFF_MAX_MS);
    exp.max(retry_after_ms.unwrap_or(0))
}

async fn anthropic_usage_entry(state: &Arc<AppState>) -> Option<Value> {
    let account = state.vault.account_for(Provider::Anthropic, true).await.ok()?;
    if account.kind != "oauth" {
        return None;
    }
    let token = account.access_token.as_deref()?.to_string();
    {
        let cache = state.anthropic_usage.lock().unwrap();
        if cache.entry.is_some() && now_ms() < cache.fetched_at_ms + USAGE_CACHE_TTL_MS {
            return cache.entry.clone();
        }
        if now_ms() < cache.cooldown_until_ms {
            return cache.entry.clone();
        }
    }
    let result = state
        .http
        .get(format!("{ANTHROPIC_BASE}/api/oauth/usage"))
        .header("authorization", format!("Bearer {token}"))
        .header("anthropic-beta", ANTHROPIC_OAUTH_BETA)
        .header("accept", "application/json")
        .header("user-agent", "claude-cli/2.1.202 (external, cli)")
        .send()
        .await;
    match result {
        Ok(resp) if resp.status().is_success() => {
            let raw: Value = resp.json().await.unwrap_or(Value::Null);
            let mut windows = Vec::new();
            for (name, key) in [
                ("5h", "five_hour"),
                ("7d", "seven_day"),
                ("7d opus", "seven_day_opus"),
                ("7d sonnet", "seven_day_sonnet"),
            ] {
                let w = &raw[key];
                if w.is_object() {
                    windows.push(json!({
                        "window": name,
                        "used_pct": w["utilization"],
                        "resets_at": w["resets_at"],
                    }));
                }
            }
            let entry = json!({
                "provider": "anthropic",
                "source": "oauth usage endpoint",
                "plan": account.label,
                "windows": windows,
                "extra_usage": raw["extra_usage"],
            });
            let mut cache = state.anthropic_usage.lock().unwrap();
            cache.fetched_at_ms = now_ms();
            cache.cooldown_until_ms = 0;
            cache.failures = 0;
            cache.entry = Some(entry.clone());
            Some(entry)
        }
        Ok(resp) => {
            let retry_after = resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<i64>().ok())
                .map(|s| s.clamp(30, 3600) * 1000);
            let mut cache = state.anthropic_usage.lock().unwrap();
            cache.failures += 1;
            let cooldown = usage_backoff_ms(cache.failures, retry_after);
            cache.cooldown_until_ms = now_ms() + cooldown;
            tracing::debug!(
                status = resp.status().as_u16(),
                failures = cache.failures,
                cooldown_ms = cooldown,
                "anthropic usage endpoint unavailable; backing off"
            );
            cache.entry.clone()
        }
        Err(_) => {
            let mut cache = state.anthropic_usage.lock().unwrap();
            cache.failures += 1;
            let cooldown = usage_backoff_ms(cache.failures, None);
            cache.cooldown_until_ms = now_ms() + cooldown;
            cache.entry.clone()
        }
    }
}

pub async fn limits_snapshot(state: &Arc<AppState>) -> Value {
    let mut providers: Vec<Value> = Vec::new();
    if let Some(entry) = anthropic_usage_entry(state).await {
        providers.push(entry);
    }
    for (provider_str, ts_ms, headers_json) in
        state.store.latest_provider_headers().unwrap_or_default()
    {
        if providers
            .iter()
            .any(|p| p["provider"].as_str() == Some(&provider_str))
        {
            continue;
        }
        let Some(provider) = Provider::from_str_loose(&provider_str) else {
            continue;
        };
        let headers: Value = serde_json::from_str(&headers_json).unwrap_or(Value::Null);
        let mut parsed = alex_core::parse_limit_headers(provider, &headers);
        if let Some(o) = parsed.as_object_mut() {
            o.insert("provider".into(), json!(provider_str));
            o.insert("source".into(), json!("captured response headers"));
            o.insert("observed_at_ms".into(), json!(ts_ms));
            providers.push(parsed);
        }
    }
    providers.sort_by_key(|p| p["provider"].as_str().unwrap_or("").to_string());
    json!({"providers": providers})
}

async fn admin_limits(State(state): State<Arc<AppState>>) -> Response {
    axum::Json(limits_snapshot(&state).await).into_response()
}

async fn admin_dario(State(state): State<Arc<AppState>>) -> Response {
    match &state.dario {
        Some(d) => axum::Json(d.status()).into_response(),
        None => error_response(StatusCode::NOT_FOUND, "dario mode is not enabled"),
    }
}

async fn admin_storage(State(state): State<Arc<AppState>>) -> Response {
    match state.store.disk_usage() {
        Ok(v) => axum::Json(v).into_response(),
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

async fn admin_storage_prune(
    State(state): State<Arc<AppState>>,
    body: axum::Json<Value>,
) -> Response {
    let now = now_ms();
    let cutoff = match &body["older_than_ms"] {
        Value::Null => match body["older_than"].as_str() {
            Some(s) => match parse_since(s, now) {
                Some(ms) => ms,
                None => {
                    return error_response(
                        StatusCode::BAD_REQUEST,
                        &format!("invalid 'older_than' '{s}' (use 45s, 30m, 24h, 7d, or RFC3339)"),
                    )
                }
            },
            None => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "missing 'older_than_ms' or 'older_than'",
                )
            }
        },
        v => match v.as_i64() {
            Some(ms) => ms,
            None => {
                return error_response(StatusCode::BAD_REQUEST, "'older_than_ms' must be an integer")
            }
        },
    };
    if cutoff > now {
        return error_response(StatusCode::BAD_REQUEST, "cutoff is in the future");
    }
    let bodies_only = body["bodies_only"].as_bool().unwrap_or(true);
    let dry_run = body["dry_run"].as_bool().unwrap_or(false);
    let store = state.store.clone();
    let report =
        tokio::task::spawn_blocking(move || store.prune(cutoff, bodies_only, dry_run)).await;
    match report {
        Ok(Ok(r)) => axum::Json(serde_json::to_value(r).unwrap_or_default()).into_response(),
        Ok(Err(e)) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

async fn admin_traces(
    State(state): State<Arc<AppState>>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let limit = q.get("limit").and_then(|s| s.parse().ok()).unwrap_or(50);
    match state.store.list_traces(
        limit,
        q.get("session").map(|s| s.as_str()),
        q.get("model").map(|s| s.as_str()),
    ) {
        Ok(rows) => axum::Json(json!({"traces": rows})).into_response(),
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

fn filter_from_query(q: &HashMap<String, String>) -> TraceFilter {
    let now = now_ms();
    TraceFilter {
        since_ms: q.get("since").and_then(|s| parse_since(s, now)),
        until_ms: q.get("until").and_then(|s| parse_since(s, now)),
        run_id: q.get("run_id").cloned(),
        session: q.get("session").cloned(),
        model: q.get("model").cloned(),
        provider: q.get("provider").cloned(),
        path: q.get("path").cloned(),
        harness: q.get("harness").cloned(),
        status: q.get("status").and_then(|s| s.parse().ok()),
        errors_only: q
            .get("errors")
            .map(|v| v == "1" || v == "true")
            .unwrap_or(false),
        key_fingerprint: q.get("key_fingerprint").cloned(),
        limit: q.get("limit").and_then(|s| s.parse().ok()).unwrap_or(200),
    }
}

fn wants_bodies(q: &HashMap<String, String>) -> bool {
    q.get("bodies").map(|v| v == "1" || v == "true").unwrap_or(false)
}

fn read_gz_file(path: &str) -> Option<Vec<u8>> {
    let file = std::fs::File::open(path).ok()?;
    let mut decoder = flate2::read::GzDecoder::new(file);
    let mut buf = Vec::new();
    std::io::Read::read_to_end(&mut decoder, &mut buf).ok()?;
    Some(buf)
}

fn inline_row_bodies(row: &mut Value) {
    use base64::Engine;
    for (path_key, out_key) in [
        ("req_body_path", "req_body_b64"),
        ("upstream_req_body_path", "upstream_req_body_b64"),
        ("resp_body_path", "resp_body_b64"),
    ] {
        let Some(buf) = row[path_key].as_str().and_then(read_gz_file) else {
            continue;
        };
        row[out_key] = json!(base64::engine::general_purpose::STANDARD.encode(&buf));
    }
}

fn ndjson_response(mut rows: Vec<Value>, inline_bodies: bool) -> Response {
    rows.sort_by_key(|r| r["ts_request_ms"].as_i64().unwrap_or(0));
    let mut out = String::new();
    for mut row in rows {
        if inline_bodies {
            inline_row_bodies(&mut row);
        }
        out.push_str(&serde_json::to_string(&row).unwrap_or_default());
        out.push('\n');
    }
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/x-ndjson")
        .body(Body::from(out))
        .unwrap_or_else(|e| error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))
}

const TEXT_SCAN_CAP: usize = 300;

fn trace_matches_text(row: &Value, needle: &str) -> bool {
    for key in ["req_body_path", "resp_body_path"] {
        if let Some(path) = row.get(key).and_then(|v| v.as_str()) {
            if let Some(bytes) = read_gz_file(path) {
                if String::from_utf8_lossy(&bytes)
                    .to_lowercase()
                    .contains(needle)
                {
                    return true;
                }
            }
        }
    }
    false
}

async fn traces_search(
    State(state): State<Arc<AppState>>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let mut filter = filter_from_query(&q);
    let text = q
        .get("text")
        .or_else(|| q.get("q"))
        .map(|t| t.trim().to_lowercase())
        .filter(|t| !t.is_empty());
    if text.is_some() {
        filter.limit = TEXT_SCAN_CAP;
    }
    match state.store.search_traces(&filter) {
        Ok(rows) => match text {
            Some(needle) => {
                let scanned = rows.len();
                let rows = tokio::task::spawn_blocking(move || {
                    rows.into_iter()
                        .filter(|r| trace_matches_text(r, &needle))
                        .collect::<Vec<_>>()
                })
                .await
                .unwrap_or_default();
                axum::Json(json!({"traces": rows, "scanned": scanned, "scan_cap": TEXT_SCAN_CAP}))
                    .into_response()
            }
            None => axum::Json(json!({"traces": rows})).into_response(),
        },
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

async fn traces_export(
    State(state): State<Arc<AppState>>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    match state.store.search_traces(&filter_from_query(&q)) {
        Ok(rows) => ndjson_response(rows, wants_bodies(&q)),
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

fn truncate_chars(s: String, max: usize) -> String {
    if s.chars().count() <= max {
        s
    } else {
        s.chars().take(max).collect()
    }
}

fn read_gz_json(path: Option<&str>) -> Option<Value> {
    let buf = path.and_then(read_gz_file)?;
    serde_json::from_slice(&buf).ok()
}

fn read_gz_text(path: Option<&str>) -> Option<String> {
    let buf = path.and_then(read_gz_file)?;
    Some(String::from_utf8_lossy(&buf).to_string())
}

async fn traces_sessions(
    State(state): State<Arc<AppState>>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let since = q.get("since").and_then(|s| parse_since(s, now_ms()));
    let limit = q.get("limit").and_then(|s| s.parse().ok()).unwrap_or(0);
    match state.store.sessions(since, limit) {
        Ok(rows) => axum::Json(json!({"sessions": rows})).into_response(),
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

fn transcript_turn(row: &Value) -> Value {
    use alex_core::translate;
    let user = read_gz_json(row["req_body_path"].as_str())
        .and_then(|req| {
            translate::last_user_text(row["client_format"].as_str().unwrap_or(""), &req)
        })
        .map(|s| truncate_chars(s, 8000));
    let resp_text = read_gz_text(row["resp_body_path"].as_str());
    let fmt = row["upstream_format"]
        .as_str()
        .or(row["client_format"].as_str())
        .unwrap_or("")
        .to_string();
    let assistant = resp_text
        .as_deref()
        .and_then(|text| translate::assistant_reply_text(&fmt, text))
        .map(|s| truncate_chars(s, 8000));
    let tool_calls: Vec<Value> = resp_text
        .as_deref()
        .map(|text| translate::assistant_tool_calls(&fmt, text))
        .unwrap_or_default()
        .into_iter()
        .take(24)
        .map(|mut c| {
            if let Some(a) = c["arguments"].as_str() {
                let t = truncate_chars(a.to_string(), 600);
                c["arguments"] = json!(t);
            }
            c
        })
        .collect();
    json!({
        "trace_id": row["id"],
        "ts_request_ms": row["ts_request_ms"],
        "ts_response_ms": row["ts_response_ms"],
        "model": row["routed_model"],
        "status": row["status"],
        "input_tokens": row["input_tokens"],
        "output_tokens": row["output_tokens"],
        "cost_usd": row["cost_usd"],
        "error": row["error"],
        "user": user,
        "assistant": assistant,
        "tool_calls": tool_calls,
    })
}

async fn traces_session_transcript(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let since = q
        .get("since_ms")
        .and_then(|s| s.parse::<i64>().ok())
        .or_else(|| q.get("since").and_then(|s| parse_since(s, now_ms())));
    let limit: usize = q.get("limit").and_then(|s| s.parse().ok()).unwrap_or(500);
    let rows = match state.store.session_traces(&session_id, since) {
        Ok(rows) => rows,
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    let turns: Vec<Value> = rows.iter().take(limit).map(transcript_turn).collect();
    axum::Json(json!({"session_id": session_id, "turns": turns})).into_response()
}

fn trace_extras(req: &Value) -> Value {
    let system_text: Option<String> = match &req["system"] {
        Value::String(s) => Some(s.clone()),
        Value::Array(parts) => {
            let joined: Vec<&str> = parts.iter().filter_map(|p| p["text"].as_str()).collect();
            (!joined.is_empty()).then(|| joined.join("\n\n"))
        }
        _ => req["instructions"].as_str().map(String::from).or_else(|| {
            let joined: Vec<String> = req["messages"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|m| matches!(m["role"].as_str(), Some("system") | Some("developer")))
                .map(|m| match &m["content"] {
                    Value::String(s) => s.clone(),
                    Value::Array(parts) => parts
                        .iter()
                        .filter_map(|p| p["text"].as_str())
                        .collect::<Vec<_>>()
                        .join("\n"),
                    _ => String::new(),
                })
                .filter(|s| !s.is_empty())
                .collect();
            (!joined.is_empty()).then(|| joined.join("\n\n"))
        }),
    };
    let system_chars = system_text.as_ref().map(|s| s.chars().count());
    let system_prompt = system_text.map(|s| truncate_chars(s, 64_000));
    json!({
        "reasoning_effort": req["reasoning"]["effort"],
        "thinking_budget": req["thinking"]["budget_tokens"],
        "max_tokens": req["max_tokens"].as_i64().or(req["max_output_tokens"].as_i64()),
        "temperature": req["temperature"],
        "message_count": req["messages"].as_array().or(req["input"].as_array()).map(|a| a.len()),
        "system_chars": system_chars,
        "system_prompt": system_prompt,
    })
}

async fn trace_get(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    match state.store.get_trace(&id) {
        Ok(Some(row)) => {
            let extras = read_gz_json(row["req_body_path"].as_str())
                .map(|req| trace_extras(&req))
                .unwrap_or_else(|| json!({}));
            axum::Json(json!({"trace": row, "extras": extras})).into_response()
        }
        Ok(None) => error_response(StatusCode::NOT_FOUND, &format!("unknown trace '{id}'")),
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

async fn trace_body(
    State(state): State<Arc<AppState>>,
    Path((id, kind)): Path<(String, String)>,
) -> Response {
    let row = match state.store.get_trace(&id) {
        Ok(Some(row)) => row,
        Ok(None) => {
            return error_response(StatusCode::NOT_FOUND, &format!("unknown trace '{id}'"))
        }
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    let column = match kind.as_str() {
        "request" => "req_body_path",
        "upstream-request" => "upstream_req_body_path",
        "response" => "resp_body_path",
        _ => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "kind must be request|upstream-request|response",
            )
        }
    };
    match read_gz_text(row[column].as_str()) {
        Some(text) => {
            let ct = if text.trim_start().starts_with('{') || text.trim_start().starts_with('[') {
                "application/json; charset=utf-8"
            } else {
                "text/plain; charset=utf-8"
            };
            Response::builder()
                .status(StatusCode::OK)
                .header("content-type", ct)
                .header("x-alexandria-body-path", row[column].as_str().unwrap_or(""))
                .body(Body::from(text))
                .unwrap_or_else(|e| error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))
        }
        None => error_response(
            StatusCode::NOT_FOUND,
            &format!("no {kind} body stored for trace '{id}'"),
        ),
    }
}

async fn trace_reply_md(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    use alex_core::translate;
    let row = match state.store.get_trace(&id) {
        Ok(Some(row)) => row,
        Ok(None) => {
            return error_response(StatusCode::NOT_FOUND, &format!("unknown trace '{id}'"))
        }
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    let fmt = row["upstream_format"]
        .as_str()
        .or(row["client_format"].as_str())
        .unwrap_or("");
    let reply = read_gz_text(row["resp_body_path"].as_str())
        .and_then(|text| translate::assistant_reply_text(fmt, &text));
    match reply {
        Some(md) => Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "text/markdown; charset=utf-8")
            .body(Body::from(md))
            .unwrap_or_else(|e| error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string())),
        None => error_response(
            StatusCode::NOT_FOUND,
            &format!("no assistant reply available for trace '{id}'"),
        ),
    }
}

async fn trace_delete(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    match state.store.get_trace(&id) {
        Ok(Some(_)) => {}
        Ok(None) => {
            return error_response(StatusCode::NOT_FOUND, &format!("unknown trace '{id}'"))
        }
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
    match state.store.delete_trace(&id) {
        Ok(paths) => {
            let removed = paths
                .iter()
                .filter(|p| std::fs::remove_file(p).is_ok())
                .count();
            axum::Json(json!({"deleted": true, "files_removed": removed})).into_response()
        }
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

async fn traces_run_summary(
    State(state): State<Arc<AppState>>,
    Path(run_id): Path<String>,
) -> Response {
    match state.store.run_summary(&run_id) {
        Ok(v) if v["trace_count"].as_i64().unwrap_or(0) == 0 => error_response(
            StatusCode::NOT_FOUND,
            &format!("no traces for run '{run_id}'"),
        ),
        Ok(v) => axum::Json(v).into_response(),
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

async fn traces_run_events(
    State(state): State<Arc<AppState>>,
    Path(run_id): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let filter = TraceFilter {
        run_id: Some(run_id),
        limit: q.get("limit").and_then(|s| s.parse().ok()).unwrap_or(1000),
        ..Default::default()
    };
    match state.store.search_traces(&filter) {
        Ok(rows) => axum::Json(json!({"traces": rows})).into_response(),
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

async fn traces_run_export(
    State(state): State<Arc<AppState>>,
    Path(run_id): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let filter = TraceFilter {
        run_id: Some(run_id),
        limit: q.get("limit").and_then(|s| s.parse().ok()).unwrap_or(5000),
        ..Default::default()
    };
    match state.store.search_traces(&filter) {
        Ok(rows) => ndjson_response(rows, wants_bodies(&q)),
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

async fn traces_run_artifacts(
    State(state): State<Arc<AppState>>,
    Path(run_id): Path<String>,
) -> Response {
    match state.store.run_artifacts(&run_id) {
        Ok(artifacts) => axum::Json(json!({
            "run_id": run_id,
            "data_dir": state.store.data_dir.to_string_lossy(),
            "artifacts": artifacts,
        }))
        .into_response(),
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

async fn admin_accounts(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let accounts: Vec<Value> = state
        .vault
        .list()
        .await
        .into_iter()
        .map(|a| {
            json!({
                "id": a.id,
                "provider": a.provider.as_str(),
                "kind": a.kind,
                "label": a.label,
                "status": a.status,
                "expires_at_ms": a.expires_at_ms,
                "expires_in_s": a.expires_at_ms.map(|e| (e - now_ms()) / 1000),
            })
        })
        .collect();
    axum::Json(json!({"accounts": accounts}))
}

async fn admin_health(State(state): State<Arc<AppState>>) -> Response {
    let heartbeats = state.store.last_heartbeats().unwrap_or_default();
    let accounts: Vec<Value> = state
        .vault
        .list()
        .await
        .into_iter()
        .map(|a| {
            let last = heartbeats
                .iter()
                .find(|h| h["provider"].as_str() == Some(a.provider.as_str()))
                .cloned();
            json!({
                "id": a.id,
                "provider": a.provider.as_str(),
                "kind": a.kind,
                "status": a.status,
                "token_expires_in_s": a.expires_at_ms.map(|e| (e - now_ms()) / 1000),
                "last_heartbeat": last,
            })
        })
        .collect();
    axum::Json(json!({"accounts": accounts})).into_response()
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PingResult {
    pub provider: &'static str,
    pub account_id: Option<String>,
    pub ok: bool,
    pub status: Option<u16>,
    pub latency_ms: i64,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct PingModels {
    pub anthropic: String,
    pub openai: String,
    pub xai: String,
    pub gemini: String,
}

pub async fn ping_provider(
    state: &Arc<AppState>,
    provider: Provider,
    models: &PingModels,
) -> PingResult {
    let start = now_ms();
    let prompt = "Health check: what time is it? If you cannot know, just reply: creds ok";
    let (format, path, body) = match provider {
        Provider::Anthropic => (
            ClientFormat::AnthropicMessages,
            "/v1/messages",
            json!({
                "model": models.anthropic,
                "max_tokens": 64,
                "system": "You are Claude Code, Anthropic's official CLI for Claude.",
                "messages": [{"role": "user", "content": prompt}],
            }),
        ),
        Provider::Openai => (
            ClientFormat::OpenaiResponses,
            "/v1/responses",
            json!({
                "model": models.openai,
                "stream": true,
                "store": false,
                "instructions": "You are a helpful assistant.",
                "input": [{"type": "message", "role": "user",
                           "content": [{"type": "input_text", "text": prompt}]}],
            }),
        ),
        Provider::Xai => (
            ClientFormat::OpenaiChat,
            "/v1/chat/completions",
            json!({
                "model": models.xai,
                "stream": false,
                "messages": [{"role": "user", "content": prompt}],
            }),
        ),
        Provider::Gemini => (
            ClientFormat::GeminiGenerate,
            "/v1beta/models/:generateContent",
            json!({
                "model": models.gemini,
                "contents": [{"role": "user", "parts": [{"text": prompt}]}],
                "generationConfig": {"maxOutputTokens": 64},
            }),
        ),
    };
    let account_id = state
        .vault
        .account_for(provider, true)
        .await
        .ok()
        .map(|a| a.id);
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-api-key",
        HeaderValue::from_str(&state.local_key).expect("key header"),
    );
    headers.insert("user-agent", HeaderValue::from_static("alexandria-ping"));
    let resp = proxy(
        state.clone(),
        format,
        path,
        headers,
        Bytes::from(serde_json::to_vec(&body).expect("ping body")),
        None,
    )
    .await;
    let status = resp.status().as_u16();
    let bytes = axum::body::to_bytes(resp.into_body(), 8 * 1024 * 1024)
        .await
        .unwrap_or_default();
    let text = String::from_utf8_lossy(&bytes);
    let message = extract_reply(&text).unwrap_or_else(|| snippet(&text));
    PingResult {
        provider: provider.as_str(),
        account_id,
        ok: (200..300).contains(&status),
        status: Some(status),
        latency_ms: now_ms() - start,
        message,
    }
}

fn extract_reply(text: &str) -> Option<String> {
    if let Ok(v) = serde_json::from_str::<Value>(text) {
        if let Some(t) = v["content"][0]["text"].as_str() {
            return Some(t.to_string());
        }
        if let Some(t) = v["choices"][0]["message"]["content"].as_str() {
            return Some(t.to_string());
        }
        if let Some(t) = v["error"]["message"].as_str() {
            return Some(t.to_string());
        }
        if let Some(t) = v["detail"].as_str() {
            return Some(t.to_string());
        }
    }
    for line in text.lines() {
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<Value>(data.trim()) else {
            continue;
        };
        if v["type"].as_str() == Some("response.output_text.done") {
            if let Some(t) = v["text"].as_str() {
                return Some(t.to_string());
            }
        }
    }
    None
}

fn snippet(text: &str) -> String {
    let t: String = text.chars().take(200).collect();
    t.replace('\n', " ")
}

pub async fn heartbeat_once(state: &Arc<AppState>, models: &PingModels) -> Vec<PingResult> {
    let providers: Vec<Provider> = {
        let mut seen = Vec::new();
        for a in state.vault.list().await {
            if a.status == "active"
                && matches!(
                    a.provider,
                    Provider::Anthropic | Provider::Openai | Provider::Xai | Provider::Gemini
                )
                && !seen.contains(&a.provider)
            {
                seen.push(a.provider);
            }
        }
        seen
    };
    let mut results = Vec::new();
    for provider in providers {
        let r = ping_provider(state, provider, models).await;
        if let Err(e) = state.store.insert_heartbeat(
            now_ms(),
            r.provider,
            r.account_id.as_deref(),
            r.ok,
            r.status.map(|s| s as i64),
            r.latency_ms,
            &r.message,
        ) {
            tracing::warn!("failed to record heartbeat: {e}");
        }
        tracing::info!(
            provider = r.provider,
            ok = r.ok,
            status = r.status,
            latency_ms = r.latency_ms,
            reply = %r.message,
            "heartbeat"
        );
        results.push(r);
    }
    results
}

async fn anthropic_messages(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    proxy(
        state,
        ClientFormat::AnthropicMessages,
        "/v1/messages",
        headers,
        body,
        Some(peer),
    )
    .await
}

async fn openai_chat(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    proxy(
        state,
        ClientFormat::OpenaiChat,
        "/v1/chat/completions",
        headers,
        body,
        Some(peer),
    )
    .await
}

async fn openai_responses(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    proxy(
        state,
        ClientFormat::OpenaiResponses,
        "/v1/responses",
        headers,
        body,
        Some(peer),
    )
    .await
}

async fn gemini_generate(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>,
    axum::extract::Path(model_action): axum::extract::Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let (model, method) = model_action
        .split_once(':')
        .unwrap_or((model_action.as_str(), "generateContent"));
    let stream = method == "streamGenerateContent";
    if method != "generateContent" && !stream {
        return error_response(
            StatusCode::NOT_FOUND,
            &format!("unsupported gemini method '{method}' (expected generateContent or streamGenerateContent)"),
        );
    }
    if model.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "missing model in path");
    }
    let mut v: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            return error_response(StatusCode::BAD_REQUEST, &format!("invalid JSON body: {e}"))
        }
    };
    v["model"] = json!(model);
    if stream {
        v["stream"] = json!(true);
    }
    let body = Bytes::from(serde_json::to_vec(&v).unwrap_or_default());
    let path = if stream {
        "/v1beta/models/:streamGenerateContent"
    } else {
        "/v1beta/models/:generateContent"
    };
    proxy(
        state,
        ClientFormat::GeminiGenerate,
        path,
        headers,
        body,
        Some(peer),
    )
    .await
}

fn error_response(status: StatusCode, message: &str) -> Response {
    (
        status,
        axum::Json(json!({"error": {"type": "alexandria", "message": message}})),
    )
        .into_response()
}

fn client_key(headers: &HeaderMap) -> Option<String> {
    if let Some(v) = headers.get("x-api-key").and_then(|v| v.to_str().ok()) {
        return Some(v.to_string());
    }
    if let Some(v) = headers.get("x-goog-api-key").and_then(|v| v.to_str().ok()) {
        return Some(v.to_string());
    }
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(String::from)
}

fn redacted_headers(headers: &HeaderMap) -> String {
    let map: HashMap<String, String> = headers
        .iter()
        .map(|(k, v)| {
            let key = k.as_str().to_lowercase();
            let val = if ["authorization", "x-api-key", "cookie", "chatgpt-account-id"]
                .contains(&key.as_str())
            {
                "<redacted>".to_string()
            } else {
                v.to_str().unwrap_or("<binary>").to_string()
            };
            (key, val)
        })
        .collect();
    serde_json::to_string(&map).unwrap_or_default()
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum RespondAs {
    Anthropic,
    OpenaiChat,
    OpenaiResponses,
    Gemini,
}

struct UpstreamPlan {
    url: String,
    account: Account,
    body: Vec<u8>,
    upstream_format: &'static str,
    destream: bool,
    respond_as: Option<RespondAs>,
    client_stream: bool,
    extra_headers: Vec<(String, String)>,
    dario_guard: Option<Box<dyn std::any::Any + Send>>,
}

fn dario_account(active: &DarioActive) -> Account {
    Account {
        id: format!("dario:{}", active.generation_id),
        provider: Provider::Anthropic,
        kind: "dario".into(),
        label: Some("dario generational proxy".into()),
        access_token: None,
        refresh_token: None,
        id_token: None,
        api_key: Some(active.api_key.clone()),
        expires_at_ms: None,
        last_refresh_ms: None,
        account_meta: Value::Null,
        cooldown_until_ms: None,
        status: "active".into(),
    }
}

async fn ensure_gemini_project(
    state: &AppState,
    account: &Account,
) -> Result<String, (StatusCode, String)> {
    if let Some(p) = account.account_meta.get("project_id").and_then(|v| v.as_str()) {
        if !p.is_empty() {
            return Ok(p.to_string());
        }
    }
    if let Ok(env_p) = std::env::var("GOOGLE_CLOUD_PROJECT") {
        if !env_p.is_empty() {
            let _ = state
                .vault
                .set_account_meta(&account.id, "project_id", json!(env_p))
                .await;
            return Ok(env_p);
        }
    }
    let token = account
        .access_token
        .as_deref()
        .ok_or_else(|| (StatusCode::BAD_GATEWAY, "gemini account has no access token".into()))?;
    let load_url = format!("{GEMINI_CODE_ASSIST_BASE}/{GEMINI_CODE_ASSIST_VERSION}:loadCodeAssist");
    let load_body = json!({
        "cloudaicompanionProject": null,
        "metadata": {"ideType": "IDE_UNSPECIFIED", "platform": "PLATFORM_UNSPECIFIED", "pluginType": "GEMINI"},
    });
    let resp = state
        .http
        .post(&load_url)
        .header("authorization", format!("Bearer {token}"))
        .json(&load_body)
        .send()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("loadCodeAssist failed: {e}")))?;
    let load: Value = resp.json().await.unwrap_or(Value::Null);
    let extract = |v: &Value| -> Option<String> {
        for key in ["cloudaicompanionProject", "projectId", "project"] {
            match &v[key] {
                Value::String(s) if !s.is_empty() => return Some(s.clone()),
                obj if obj["id"].is_string() => {
                    return obj["id"].as_str().map(String::from)
                }
                _ => {}
            }
        }
        None
    };
    if let Some(p) = extract(&load) {
        let _ = state
            .vault
            .set_account_meta(&account.id, "project_id", json!(p))
            .await;
        return Ok(p);
    }
    if let Some(free) = load["ineligibleTiers"]
        .as_array()
        .and_then(|t| t.iter().find(|t| t["tierId"] == json!("free-tier")))
    {
        let reason = free["reasonCode"].as_str().unwrap_or("");
        let msg = free["reasonMessage"].as_str().unwrap_or("");
        let hint = if let Some(url) = free["validationUrl"].as_str() {
            format!(
                "gemini account needs verification before the free Code Assist tier works — \
                 open {url} to verify, then retry (or sign in with another personal Google \
                 account, or set gemini_project to a GCP project)"
            )
        } else if reason == "DASHER_USER" {
            "this is a Google Workspace account, which cannot use the free Code Assist tier — \
             set gemini_project to a GCP project (Code Assist API enabled), or sign in with a \
             personal Google account"
                .to_string()
        } else {
            format!("gemini account is not eligible for the free Code Assist tier: {msg}")
        };
        return Err((StatusCode::BAD_GATEWAY, hint));
    }
    let tier = load["allowedTiers"]
        .as_array()
        .and_then(|tiers| tiers.iter().find(|t| t["isDefault"] == json!(true)))
        .and_then(|t| t["id"].as_str())
        .unwrap_or("free-tier")
        .to_string();
    let onboard_url =
        format!("{GEMINI_CODE_ASSIST_BASE}/{GEMINI_CODE_ASSIST_VERSION}:onboardUser");
    let onboard_body = json!({
        "tierId": tier,
        "cloudaicompanionProject": load["cloudaicompanionProject"],
        "metadata": {"ideType": "IDE_UNSPECIFIED", "platform": "PLATFORM_UNSPECIFIED", "pluginType": "GEMINI"},
    });
    for _ in 0..5 {
        let resp = state
            .http
            .post(&onboard_url)
            .header("authorization", format!("Bearer {token}"))
            .json(&onboard_body)
            .send()
            .await
            .map_err(|e| (StatusCode::BAD_GATEWAY, format!("onboardUser failed: {e}")))?;
        let lro: Value = resp.json().await.unwrap_or(Value::Null);
        if lro["done"] == json!(true) {
            if let Some(p) = extract(&lro["response"]) {
                let _ = state
                    .vault
                    .set_account_meta(&account.id, "project_id", json!(p))
                    .await;
                return Ok(p);
            }
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    Err((
        StatusCode::BAD_GATEWAY,
        "gemini Code Assist onboarding did not return a project id (set GOOGLE_CLOUD_PROJECT to override)".into(),
    ))
}

async fn plan_upstream(
    state: &AppState,
    format: ClientFormat,
    provider: Provider,
    routed_model: &str,
    body_json: &mut Value,
    original_body: &[u8],
    trace_id: &str,
) -> Result<UpstreamPlan, (StatusCode, String)> {
    use alex_core::translate;
    let client_stream = body_json["stream"].as_bool().unwrap_or(false);
    match provider {
        Provider::Anthropic => {
            let converted = match format {
                ClientFormat::AnthropicMessages => None,
                ClientFormat::OpenaiChat => Some((
                    translate::openai_chat_to_anthropic(body_json),
                    RespondAs::OpenaiChat,
                )),
                ClientFormat::OpenaiResponses => Some((
                    translate::openai_responses_to_anthropic(body_json),
                    RespondAs::OpenaiResponses,
                )),
                ClientFormat::GeminiGenerate => Some((
                    translate::gemini_to_anthropic(body_json),
                    RespondAs::Gemini,
                )),
            };
            let dario_active = state.dario.as_ref().and_then(|d| d.active());
            let (base, account, dario_guard) = match (&state.dario, dario_active) {
                (Some(dario), Some(active)) => (
                    active.base_url.trim_end_matches('/').to_string(),
                    dario_account(&active),
                    dario.begin(&active.generation_id),
                ),
                _ => {
                    let account = state
                        .vault
                        .account_for(provider, true)
                        .await
                        .map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?;
                    (ANTHROPIC_BASE.to_string(), account, None)
                }
            };
            let (body, respond_as) = match converted {
                None => {
                    body_json["model"] = json!(routed_model);
                    (
                        serde_json::to_vec(body_json).unwrap_or_else(|_| original_body.to_vec()),
                        None,
                    )
                }
                Some((mut converted, respond_as)) => {
                    converted["model"] = json!(routed_model);
                    converted["stream"] = json!(false);
                    (
                        serde_json::to_vec(&converted).unwrap_or_else(|_| original_body.to_vec()),
                        Some(respond_as),
                    )
                }
            };
            Ok(UpstreamPlan {
                url: format!("{base}/v1/messages"),
                account,
                body,
                upstream_format: "anthropic",
                destream: false,
                respond_as,
                client_stream,
                extra_headers: vec![],
                dario_guard,
            })
        }
        Provider::Openai => {
            let prefer_oauth = format != ClientFormat::OpenaiChat;
            let account = state
                .vault
                .account_for(provider, prefer_oauth)
                .await
                .map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?;
            let oauth = account.kind == "oauth";
            match format {
                ClientFormat::OpenaiChat if !oauth => {
                    body_json["model"] = json!(routed_model);
                    let body =
                        serde_json::to_vec(body_json).unwrap_or_else(|_| original_body.to_vec());
                    Ok(UpstreamPlan {
                        url: format!("{OPENAI_BASE}/v1/chat/completions"),
                        account,
                        body,
                        upstream_format: "openai-chat",
                        destream: false,
                        respond_as: None,
                        client_stream,
                        extra_headers: vec![],
                        dario_guard: None,
                    })
                }
                ClientFormat::OpenaiChat => {
                    let pivot = translate::openai_chat_to_anthropic(body_json);
                    let mut converted = translate::anthropic_to_openai_responses(&pivot);
                    converted["model"] = json!(routed_model);
                    translate::normalize_codex_request(&mut converted);
                    let body = serde_json::to_vec(&converted)
                        .unwrap_or_else(|_| original_body.to_vec());
                    Ok(UpstreamPlan {
                        url: format!("{CODEX_BASE}/responses"),
                        account,
                        body,
                        upstream_format: "openai-responses",
                        destream: false,
                        respond_as: Some(RespondAs::OpenaiChat),
                        client_stream,
                        extra_headers: vec![],
                        dario_guard: None,
                    })
                }
                ClientFormat::OpenaiResponses => {
                    body_json["model"] = json!(routed_model);
                    let mut destream = false;
                    let url = if oauth {
                        if body_json["stream"].as_bool() != Some(true) {
                            destream = true;
                        }
                        translate::normalize_codex_request(body_json);
                        format!("{CODEX_BASE}/responses")
                    } else {
                        format!("{OPENAI_BASE}/v1/responses")
                    };
                    let body =
                        serde_json::to_vec(body_json).unwrap_or_else(|_| original_body.to_vec());
                    Ok(UpstreamPlan {
                        url,
                        account,
                        body,
                        upstream_format: "openai-responses",
                        destream,
                        respond_as: None,
                        client_stream,
                        extra_headers: vec![],
                        dario_guard: None,
                    })
                }
                ClientFormat::AnthropicMessages => {
                    let mut converted = translate::anthropic_to_openai_responses(body_json);
                    converted["model"] = json!(routed_model);
                    let url = if oauth {
                        translate::normalize_codex_request(&mut converted);
                        format!("{CODEX_BASE}/responses")
                    } else {
                        converted["stream"] = json!(false);
                        format!("{OPENAI_BASE}/v1/responses")
                    };
                    let body = serde_json::to_vec(&converted)
                        .unwrap_or_else(|_| original_body.to_vec());
                    Ok(UpstreamPlan {
                        url,
                        account,
                        body,
                        upstream_format: "openai-responses",
                        destream: false,
                        respond_as: Some(RespondAs::Anthropic),
                        client_stream,
                        extra_headers: vec![],
                        dario_guard: None,
                    })
                }
                ClientFormat::GeminiGenerate => {
                    let pivot = translate::gemini_to_anthropic(body_json);
                    let mut converted = translate::anthropic_to_openai_responses(&pivot);
                    converted["model"] = json!(routed_model);
                    let url = if oauth {
                        translate::normalize_codex_request(&mut converted);
                        format!("{CODEX_BASE}/responses")
                    } else {
                        converted["stream"] = json!(false);
                        format!("{OPENAI_BASE}/v1/responses")
                    };
                    let body = serde_json::to_vec(&converted)
                        .unwrap_or_else(|_| original_body.to_vec());
                    Ok(UpstreamPlan {
                        url,
                        account,
                        body,
                        upstream_format: "openai-responses",
                        destream: false,
                        respond_as: Some(RespondAs::Gemini),
                        client_stream,
                        extra_headers: vec![],
                        dario_guard: None,
                    })
                }
            }
        }
        Provider::Xai => {
            if format != ClientFormat::OpenaiChat {
                return Err((
                    StatusCode::NOT_IMPLEMENTED,
                    "the xai/grok upstream speaks OpenAI chat completions; POST to /v1/chat/completions".to_string(),
                ));
            }
            let account = state
                .vault
                .account_for(provider, true)
                .await
                .map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?;
            body_json["model"] = json!(routed_model);
            let body = serde_json::to_vec(body_json).unwrap_or_else(|_| original_body.to_vec());
            Ok(UpstreamPlan {
                url: format!("{XAI_BASE}/chat/completions"),
                account,
                body,
                upstream_format: "openai-chat",
                destream: false,
                respond_as: None,
                client_stream,
                extra_headers: vec![
                    ("x-grok-model-override".into(), routed_model.to_string()),
                    ("x-grok-conv-id".into(), trace_id.to_string()),
                ],
                dario_guard: None,
            })
        }
        Provider::Gemini => {
            // Prefer an AI Studio API key over the OAuth/Code-Assist path.
            let account = state
                .vault
                .account_for(provider, false)
                .await
                .map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?;
            let (gemini_req, respond_as) = match format {
                ClientFormat::GeminiGenerate => {
                    let mut g = body_json.clone();
                    if let Some(o) = g.as_object_mut() {
                        o.remove("model");
                        o.remove("stream");
                    }
                    (g, None)
                }
                ClientFormat::AnthropicMessages => (
                    translate::anthropic_to_gemini_request(body_json),
                    Some(RespondAs::Anthropic),
                ),
                ClientFormat::OpenaiChat => (
                    translate::anthropic_to_gemini_request(&translate::openai_chat_to_anthropic(
                        body_json,
                    )),
                    Some(RespondAs::OpenaiChat),
                ),
                ClientFormat::OpenaiResponses => (
                    translate::anthropic_to_gemini_request(
                        &translate::openai_responses_to_anthropic(body_json),
                    ),
                    Some(RespondAs::OpenaiResponses),
                ),
            };
            let method = if client_stream {
                "streamGenerateContent"
            } else {
                "generateContent"
            };
            let (url, body) = if account.kind == "api_key" {
                // AI Studio: plain request, model in the path, x-goog-api-key header.
                let mut url = format!(
                    "{GEMINI_API_BASE}/v1beta/models/{routed_model}:{method}"
                );
                if client_stream {
                    url.push_str("?alt=sse");
                }
                let body =
                    serde_json::to_vec(&gemini_req).unwrap_or_else(|_| original_body.to_vec());
                (url, body)
            } else {
                // Code Assist (OAuth): wrapped envelope + project.
                let project = ensure_gemini_project(state, &account).await?;
                let mut url = format!(
                    "{GEMINI_CODE_ASSIST_BASE}/{GEMINI_CODE_ASSIST_VERSION}:{method}"
                );
                if client_stream {
                    url.push_str("?alt=sse");
                }
                let envelope = json!({
                    "model": routed_model,
                    "project": project,
                    "request": gemini_req,
                });
                let body =
                    serde_json::to_vec(&envelope).unwrap_or_else(|_| original_body.to_vec());
                (url, body)
            };
            Ok(UpstreamPlan {
                url,
                account,
                body,
                upstream_format: "gemini",
                destream: false,
                respond_as: respond_as.or(Some(RespondAs::Gemini)),
                client_stream,
                extra_headers: vec![],
                dario_guard: None,
            })
        }
    }
}

#[cfg(test)]
mod trace_api_tests {
    use super::{session_from_metadata, trace_extras, transcript_turn, truncate_chars};
    use serde_json::json;

    #[test]
    fn metadata_session_variants() {
        let claude = json!({"metadata": {"user_id":
            "{\"device_id\":\"d1\",\"session_id\":\"ses_inner\"}"}});
        assert_eq!(session_from_metadata(&claude), Some("ses_inner".into()));
        let plain = json!({"metadata": {"user_id": "user-123"}});
        assert_eq!(session_from_metadata(&plain), Some("user-123".into()));
        let json_no_session = json!({"metadata": {"user_id": "{\"device_id\":\"d1\"}"}});
        assert_eq!(
            session_from_metadata(&json_no_session),
            Some("{\"device_id\":\"d1\"}".into())
        );
        assert_eq!(session_from_metadata(&json!({})), None);
    }

    #[test]
    fn extras_per_format() {
        let anthropic = json!({
            "system": [{"type": "text", "text": "abcd"}],
            "messages": [{"role": "user", "content": "hi"}],
            "max_tokens": 100,
            "temperature": 0.5,
            "thinking": {"type": "enabled", "budget_tokens": 4096},
        });
        let e = trace_extras(&anthropic);
        assert_eq!(e["thinking_budget"], 4096);
        assert_eq!(e["max_tokens"], 100);
        assert_eq!(e["temperature"], 0.5);
        assert_eq!(e["message_count"], 1);
        assert_eq!(e["system_chars"], 4);
        assert_eq!(e["reasoning_effort"], serde_json::Value::Null);
        let responses = json!({
            "instructions": "abc",
            "input": [{"type": "message"}, {"type": "message"}],
            "max_output_tokens": 200,
            "reasoning": {"effort": "high"},
        });
        let e = trace_extras(&responses);
        assert_eq!(e["reasoning_effort"], "high");
        assert_eq!(e["max_tokens"], 200);
        assert_eq!(e["message_count"], 2);
        assert_eq!(e["system_chars"], 3);
        assert_eq!(e["thinking_budget"], serde_json::Value::Null);
    }

    #[test]
    fn truncates_on_char_boundaries() {
        assert_eq!(truncate_chars("abc".into(), 8000), "abc");
        assert_eq!(truncate_chars("héllo".repeat(2000), 8000).chars().count(), 8000);
    }

    #[test]
    fn transcript_turn_missing_bodies_are_null() {
        let row = json!({
            "id": "t1", "ts_request_ms": 1, "ts_response_ms": 2,
            "routed_model": "m", "status": 200,
            "input_tokens": 10, "output_tokens": 5, "cost_usd": 0.01, "error": null,
            "req_body_path": "/nonexistent/x.gz", "resp_body_path": null,
            "client_format": "anthropic", "upstream_format": "anthropic",
        });
        let turn = transcript_turn(&row);
        assert_eq!(turn["trace_id"], "t1");
        assert_eq!(turn["user"], serde_json::Value::Null);
        assert_eq!(turn["assistant"], serde_json::Value::Null);
        assert_eq!(turn["model"], "m");
    }
}

#[cfg(test)]
mod run_key_tests {
    use super::{generate_run_key, key_fingerprint, key_hash_hex, merge_run_key_tags};

    #[test]
    fn merge_header_tags_win_per_key() {
        let merged = merge_run_key_tags(
            Some(r#"{"task":"demo","suite":"swebench"}"#),
            Some(r#"{"task":"override","case":"x1"}"#),
        )
        .unwrap();
        let v: serde_json::Value = serde_json::from_str(&merged).unwrap();
        assert_eq!(v["task"], "override");
        assert_eq!(v["suite"], "swebench");
        assert_eq!(v["case"], "x1");
    }

    #[test]
    fn merge_handles_missing_sides() {
        assert_eq!(merge_run_key_tags(None, None), None);
        assert_eq!(
            merge_run_key_tags(Some(r#"{"a":"1"}"#), None).unwrap(),
            r#"{"a":"1"}"#
        );
        assert_eq!(
            merge_run_key_tags(None, Some(r#"{"b":"2"}"#)).unwrap(),
            r#"{"b":"2"}"#
        );
        assert_eq!(merge_run_key_tags(Some("not json"), None), None);
    }

    #[test]
    fn run_key_shape_and_fingerprint() {
        let key = generate_run_key();
        assert!(key.starts_with("alxk-"));
        assert_eq!(key.len(), 5 + 64);
        assert_ne!(key, generate_run_key());
        let hash = key_hash_hex(&key);
        assert_eq!(hash.len(), 64);
        assert_eq!(hash[..16], key_fingerprint(&key));
    }
}

#[cfg(test)]
mod usage_tests {
    use super::{key_fingerprint, usage_backoff_ms};

    #[test]
    fn fingerprint_is_first_8_sha256_bytes_hex() {
        assert_eq!(key_fingerprint("abc"), "ba7816bf8f01cfea");
        assert_eq!(key_fingerprint("abc").len(), 16);
        assert_ne!(key_fingerprint("abc"), key_fingerprint("abd"));
    }

    #[test]
    fn backoff_grows_and_caps() {
        assert_eq!(usage_backoff_ms(1, None), 60_000);
        assert_eq!(usage_backoff_ms(2, None), 120_000);
        assert_eq!(usage_backoff_ms(3, None), 240_000);
        assert_eq!(usage_backoff_ms(10, None), 3_600_000);
    }

    #[test]
    fn retry_after_wins_when_larger() {
        assert_eq!(usage_backoff_ms(1, Some(600_000)), 600_000);
        assert_eq!(usage_backoff_ms(5, Some(1_000)), 960_000);
    }
}

fn upstream_headers(
    account: &Account,
    client_headers: &HeaderMap,
) -> Result<reqwest::header::HeaderMap, (StatusCode, String)> {
    let mut h = reqwest::header::HeaderMap::new();
    h.insert("content-type", HeaderValue::from_static("application/json"));
    h.insert("accept", HeaderValue::from_static("*/*"));
    h.insert("accept-encoding", HeaderValue::from_static("identity"));
    if let Some(ua) = client_headers.get("user-agent") {
        h.insert("user-agent", ua.clone());
    }
    match (account.provider, account.kind.as_str()) {
        (Provider::Anthropic, "oauth") => {
            let token = account.access_token.as_deref().ok_or((
                StatusCode::BAD_GATEWAY,
                "anthropic oauth account has no access token".to_string(),
            ))?;
            h.insert(
                "authorization",
                HeaderValue::from_str(&format!("Bearer {token}"))
                    .map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?,
            );
            let mut beta = ANTHROPIC_OAUTH_BETA.to_string();
            if let Some(client_beta) = client_headers
                .get("anthropic-beta")
                .and_then(|v| v.to_str().ok())
            {
                if !client_beta.contains(ANTHROPIC_OAUTH_BETA) {
                    beta = format!("{ANTHROPIC_OAUTH_BETA},{client_beta}");
                } else {
                    beta = client_beta.to_string();
                }
            }
            h.insert(
                "anthropic-beta",
                HeaderValue::from_str(&beta)
                    .map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?,
            );
            insert_anthropic_version(&mut h, client_headers);
        }
        (Provider::Anthropic, _) => {
            let key = account
                .api_key
                .as_deref()
                .or(account.access_token.as_deref())
                .ok_or((
                    StatusCode::BAD_GATEWAY,
                    "anthropic account has no api key".to_string(),
                ))?;
            h.insert(
                "x-api-key",
                HeaderValue::from_str(key).map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?,
            );
            if let Some(client_beta) = client_headers.get("anthropic-beta") {
                h.insert("anthropic-beta", client_beta.clone());
            }
            insert_anthropic_version(&mut h, client_headers);
        }
        (Provider::Openai, "oauth") => {
            let token = account.access_token.as_deref().ok_or((
                StatusCode::BAD_GATEWAY,
                "openai oauth account has no access token".to_string(),
            ))?;
            h.insert(
                "authorization",
                HeaderValue::from_str(&format!("Bearer {token}"))
                    .map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?,
            );
            if let Some(acct) = account.chatgpt_account_id() {
                h.insert(
                    "chatgpt-account-id",
                    HeaderValue::from_str(&acct)
                        .map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?,
                );
            }
            h.insert(
                "openai-beta",
                HeaderValue::from_static("responses=experimental"),
            );
            h.insert("originator", HeaderValue::from_static("codex_cli_rs"));
            let session = client_headers
                .get("session_id")
                .and_then(|v| v.to_str().ok())
                .map(String::from)
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            h.insert(
                "session_id",
                HeaderValue::from_str(&session)
                    .map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?,
            );
        }
        (Provider::Openai, _) => {
            let key = account.api_key.as_deref().ok_or((
                StatusCode::BAD_GATEWAY,
                "openai account has no api key".to_string(),
            ))?;
            h.insert(
                "authorization",
                HeaderValue::from_str(&format!("Bearer {key}"))
                    .map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?,
            );
        }
        (Provider::Xai, _) => {
            let token = account
                .access_token
                .as_deref()
                .or(account.api_key.as_deref())
                .ok_or((
                    StatusCode::BAD_GATEWAY,
                    "xai account has no access token".to_string(),
                ))?;
            h.insert(
                "authorization",
                HeaderValue::from_str(&format!("Bearer {token}"))
                    .map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?,
            );
            h.insert("x-xai-token-auth", HeaderValue::from_static("xai-grok-cli"));
            h.insert(
                "x-grok-client-version",
                HeaderValue::from_static(GROK_CLIENT_VERSION),
            );
            h.insert("user-agent", HeaderValue::from_static("xai-grok-workspace"));
            h.insert("accept", HeaderValue::from_static("text/event-stream"));
        }
        (Provider::Gemini, "api_key") => {
            let key = account.api_key.as_deref().ok_or((
                StatusCode::BAD_GATEWAY,
                "gemini api_key account has no key".to_string(),
            ))?;
            h.insert(
                "x-goog-api-key",
                HeaderValue::from_str(key)
                    .map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?,
            );
        }
        (Provider::Gemini, _) => {
            let token = account.access_token.as_deref().ok_or((
                StatusCode::BAD_GATEWAY,
                "gemini account has no access token".to_string(),
            ))?;
            h.insert(
                "authorization",
                HeaderValue::from_str(&format!("Bearer {token}"))
                    .map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?,
            );
        }
    }
    Ok(h)
}

fn insert_anthropic_version(h: &mut reqwest::header::HeaderMap, client_headers: &HeaderMap) {
    match client_headers.get("anthropic-version") {
        Some(v) => {
            h.insert("anthropic-version", v.clone());
        }
        None => {
            h.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
        }
    }
}

fn key_fingerprint(key: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(key.as_bytes());
    digest[..8].iter().map(|b| format!("{b:02x}")).collect()
}

const RUN_KEY_PREFIX: &str = "alxk-";
const RUN_KEY_DEFAULT_TTL_S: i64 = 86_400;
const RUN_KEY_MAX_TTL_S: i64 = 604_800;

pub fn generate_run_key() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    format!(
        "{RUN_KEY_PREFIX}{}",
        bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
    )
}

fn key_hash_hex(key: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(key.as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

fn merge_run_key_tags(key_tags: Option<&str>, header_tags: Option<&str>) -> Option<String> {
    let parse = |s: Option<&str>| {
        s.and_then(|s| serde_json::from_str::<Value>(s).ok())
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default()
    };
    let mut merged = parse(key_tags);
    merged.extend(parse(header_tags));
    if merged.is_empty() {
        None
    } else {
        serde_json::to_string(&Value::Object(merged)).ok()
    }
}

fn run_key_entry(state: &AppState, key_hash: &str) -> Option<CachedRunKey> {
    let now = now_ms();
    let cached = state.run_keys.read().unwrap().get(key_hash).cloned();
    if let Some(entry) = cached {
        if entry.expires_ms.map(|e| e > now).unwrap_or(true) {
            return Some(entry);
        }
        state.run_keys.write().unwrap().remove(key_hash);
        return None;
    }
    let row = state.store.lookup_run_key(key_hash, now).ok().flatten()?;
    let entry = CachedRunKey {
        run_id: row["run_id"].as_str().map(String::from),
        tags_json: row["tags"]
            .as_object()
            .filter(|o| !o.is_empty())
            .and_then(|o| serde_json::to_string(o).ok()),
        expires_ms: row["expires_ms"].as_i64(),
    };
    state
        .run_keys
        .write()
        .unwrap()
        .insert(key_hash.to_string(), entry.clone());
    Some(entry)
}

async fn admin_run_keys_create(
    State(state): State<Arc<AppState>>,
    body: Option<axum::Json<Value>>,
) -> Response {
    let body = body.map(|b| b.0).unwrap_or_else(|| json!({}));
    let tags = match &body["tags"] {
        Value::Null => None,
        Value::Object(o) => Some(o.clone()),
        _ => return error_response(StatusCode::BAD_REQUEST, "'tags' must be an object"),
    };
    let ttl_seconds = body["ttl_seconds"].as_i64().unwrap_or(RUN_KEY_DEFAULT_TTL_S);
    if ttl_seconds <= 0 {
        return error_response(StatusCode::BAD_REQUEST, "'ttl_seconds' must be positive");
    }
    let ttl_seconds = ttl_seconds.min(RUN_KEY_MAX_TTL_S);
    let run_id = body["run_id"].as_str().map(String::from);
    let label = body["label"].as_str().map(String::from);
    let key = generate_run_key();
    let key_hash = key_hash_hex(&key);
    let id = format!("rk-{}", &key_hash[..8]);
    let created_ms = now_ms();
    let expires_ms = created_ms + ttl_seconds * 1000;
    let tags_json = tags
        .as_ref()
        .filter(|o| !o.is_empty())
        .and_then(|o| serde_json::to_string(o).ok());
    match state.store.insert_run_key(
        &id,
        &key_hash,
        run_id.as_deref(),
        tags_json.as_deref(),
        label.as_deref(),
        created_ms,
        Some(expires_ms),
    ) {
        Ok(()) => {
            let (_, exports) = connect_payload(&state.base_url, &key);
            (
                StatusCode::CREATED,
                axum::Json(json!({
                    "id": id,
                    "key": key,
                    "run_id": run_id,
                    "tags": tags.map(Value::Object).unwrap_or_else(|| json!({})),
                    "expires_ms": expires_ms,
                    "exports": exports,
                })),
            )
                .into_response()
        }
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

async fn admin_run_keys_list(
    State(state): State<Arc<AppState>>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let all = q.get("all").map(|v| v == "1" || v == "true").unwrap_or(false);
    match state.store.list_run_keys(all) {
        Ok(rows) => axum::Json(json!({"run_keys": rows})).into_response(),
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

async fn admin_run_keys_revoke(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    match state.store.revoke_run_key(&id) {
        Ok(true) => {
            state.run_keys.write().unwrap().clear();
            axum::Json(json!({"revoked": true})).into_response()
        }
        Ok(false) => error_response(StatusCode::NOT_FOUND, &format!("unknown run key '{id}'")),
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

fn session_from_metadata(body_json: &Value) -> Option<String> {
    let raw = body_json["metadata"]["user_id"].as_str()?;
    if let Ok(v) = serde_json::from_str::<Value>(raw) {
        if let Some(inner) = v["session_id"].as_str() {
            return Some(inner.to_string());
        }
    }
    Some(raw.to_string())
}

const METADATA_HEADERS: &[(&str, &str)] = &[
    ("x-alexandria-harness", "harness"),
    ("x-alexandria-task", "task"),
    ("x-alexandria-model", "model"),
    ("x-alexandria-job", "job"),
    ("x-alexandria-phase", "phase"),
    ("x-alexandria-kind", "kind"),
];

fn trace_tags_json(headers: &HeaderMap) -> Option<String> {
    let values: Vec<&str> = headers
        .get_all("x-alexandria-trace-tag")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .collect();
    let mut tags = parse_trace_tags(&values);
    if let Some(o) = tags.as_object_mut() {
        for (header, key) in METADATA_HEADERS {
            if o.contains_key(*key) {
                continue;
            }
            if let Some(v) = headers.get(*header).and_then(|v| v.to_str().ok()) {
                let v = v.trim();
                if !v.is_empty() {
                    o.insert((*key).to_string(), json!(v));
                }
            }
        }
    }
    if tags.as_object().map(|o| o.is_empty()).unwrap_or(true) {
        return None;
    }
    serde_json::to_string(&tags).ok()
}

async fn proxy(
    state: Arc<AppState>,
    format: ClientFormat,
    path: &'static str,
    headers: HeaderMap,
    body: Bytes,
    peer: Option<std::net::SocketAddr>,
) -> Response {
    let mut run_key: Option<CachedRunKey> = None;
    let client_fingerprint = match client_key(&headers) {
        Some(k) if k == state.local_key => key_fingerprint(&k),
        Some(k) => {
            let key_hash = key_hash_hex(&k);
            match run_key_entry(&state, &key_hash) {
                Some(entry) => {
                    if let Err(e) = state.store.touch_run_key(&key_hash, now_ms()) {
                        tracing::warn!("failed to touch run key: {e}");
                    }
                    run_key = Some(entry);
                    key_hash.chars().take(16).collect()
                }
                None if k.starts_with(RUN_KEY_PREFIX) => {
                    return error_response(
                        StatusCode::UNAUTHORIZED,
                        "run key expired or revoked",
                    )
                }
                None => {
                    return error_response(
                        StatusCode::UNAUTHORIZED,
                        "bad or missing local key (x-api-key / Authorization: Bearer)",
                    )
                }
            }
        }
        None => {
            return error_response(
                StatusCode::UNAUTHORIZED,
                "bad or missing local key (x-api-key / Authorization: Bearer)",
            )
        }
    };

    let in_flight = InFlight::new(&state);
    let trace_id = uuid::Uuid::new_v4().to_string();
    let mut trace = TraceRecord {
        id: trace_id.clone(),
        ts_request_ms: now_ms(),
        method: Some("POST".into()),
        path: Some(path.into()),
        client_format: Some(format.as_str().into()),
        harness: headers
            .get("user-agent")
            .and_then(|v| v.to_str().ok())
            .map(String::from),
        req_headers_json: Some(redacted_headers(&headers)),
        run_id: headers
            .get("x-alexandria-run-id")
            .and_then(|v| v.to_str().ok())
            .map(String::from)
            .or_else(|| run_key.as_ref().and_then(|k| k.run_id.clone())),
        tags: merge_run_key_tags(
            run_key.as_ref().and_then(|k| k.tags_json.as_deref()),
            trace_tags_json(&headers).as_deref(),
        ),
        client_ip: peer.map(|p| p.ip().to_string()),
        key_fingerprint: Some(client_fingerprint),
        ..Default::default()
    };

    let mut body_json: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            return error_response(StatusCode::BAD_REQUEST, &format!("body is not JSON: {e}"))
        }
    };

    trace.session_id = headers
        .get("x-session-id")
        .and_then(|v| v.to_str().ok())
        .map(String::from)
        .or_else(|| {
            headers
                .get("session_id")
                .and_then(|v| v.to_str().ok())
                .map(String::from)
        })
        .or_else(|| session_from_metadata(&body_json))
        .or_else(|| {
            conversation_root(format, &body_json).map(|root| {
                let ua = headers
                    .get("user-agent")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("");
                let ip = peer.map(|p| p.ip().to_string()).unwrap_or_default();
                format!("auto-{}", key_fingerprint(&format!("{root}{ua}{ip}")))
            })
        });

    let requested_model = body_json["model"].as_str().unwrap_or("").to_string();
    if requested_model.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "missing 'model' in request body");
    }
    let (routed_provider, routed_model) = route_model(&requested_model);
    let provider = routed_provider.unwrap_or_else(|| format.default_provider());
    trace.requested_model = Some(requested_model.clone());
    trace.routed_model = Some(routed_model.clone());
    trace.upstream_provider = Some(provider.as_str().into());
    trace.streamed = Some(body_json["stream"].as_bool().unwrap_or(false));

    let mut plan = match plan_upstream(
        &state,
        format,
        provider,
        &routed_model,
        &mut body_json,
        &body,
        &trace_id,
    )
    .await
    {
        Ok(p) => p,
        Err((status, msg)) => {
            trace.status = Some(status.as_u16() as i64);
            trace.error = Some(msg.clone());
            finalize_trace(&state, trace, &body, None, None);
            return error_response(status, &msg);
        }
    };
    trace.upstream_format = Some(plan.upstream_format.into());
    trace.account_id = Some(plan.account.id.clone());
    trace.billing_bucket = Some(
        if plan.account.kind == "oauth" || plan.account.kind == "dario" {
            "subscription"
        } else {
            "api"
        }
        .into(),
    );

    tracing::info!(
        trace_id,
        model = %routed_model,
        provider = provider.as_str(),
        account = %plan.account.id,
        url = %plan.url,
        "proxying request"
    );

    let mut account = plan.account.clone();
    let mut upstream_resp = None;
    for attempt in 0..2 {
        let mut up_headers = match upstream_headers(&account, &headers) {
            Ok(h) => h,
            Err((status, msg)) => {
                trace.status = Some(status.as_u16() as i64);
                trace.error = Some(msg.clone());
                finalize_trace(&state, trace, &body, Some(&plan.body), None);
                return error_response(status, &msg);
            }
        };
        for (k, v) in &plan.extra_headers {
            if let (Ok(name), Ok(value)) = (
                reqwest::header::HeaderName::from_bytes(k.as_bytes()),
                HeaderValue::from_str(v),
            ) {
                up_headers.insert(name, value);
            }
        }
        let resp = match state
            .http
            .post(&plan.url)
            .headers(up_headers)
            .body(plan.body.clone())
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                let msg = format!("upstream request failed: {e}");
                suspect_dario(&state, &account);
                trace.status = Some(502);
                trace.error = Some(msg.clone());
                finalize_trace(&state, trace, &body, Some(&plan.body), None);
                return error_response(StatusCode::BAD_GATEWAY, &msg);
            }
        };
        if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS && account.kind != "dario" {
            let retry_after_s = resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<i64>().ok())
                .unwrap_or(60)
                .clamp(1, 3600);
            tracing::warn!(
                account = %account.id,
                retry_after_s,
                "upstream returned 429; cooling account down"
            );
            if let Err(e) = state
                .vault
                .mark_cooldown(&account.id, now_ms() + retry_after_s * 1000)
                .await
            {
                tracing::warn!("failed to mark cooldown: {e}");
            }
        }
        if resp.status() == reqwest::StatusCode::UNAUTHORIZED
            && account.kind == "oauth"
            && attempt == 0
        {
            tracing::warn!(
                account = %account.id,
                "upstream returned 401 for oauth account; forcing token refresh and retrying"
            );
            match state.vault.refresh(&account.id, true).await {
                Ok(fresh) => {
                    account = fresh;
                    continue;
                }
                Err(e) => {
                    tracing::warn!("forced refresh failed: {e}");
                }
            }
        }
        upstream_resp = Some(resp);
        break;
    }
    let upstream_resp = upstream_resp.expect("upstream response after retry loop");

    let status = upstream_resp.status();
    trace.status = Some(status.as_u16() as i64);
    let resp_headers = upstream_resp.headers().clone();
    let content_type = resp_headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();
    trace.resp_headers_json = Some(redacted_headers(&resp_headers));
    let is_sse = content_type.starts_with("text/event-stream");

    if let Some(target) = plan.respond_as {
        use alex_core::translate;
        let buf = match upstream_resp.bytes().await {
            Ok(b) => b.to_vec(),
            Err(e) => {
                let msg = format!("upstream body read failed: {e}");
                suspect_dario(&state, &account);
                trace.status = Some(502);
                trace.error = Some(msg.clone());
                finalize_trace(&state, trace, &body, Some(&plan.body), None);
                return error_response(StatusCode::BAD_GATEWAY, &msg);
            }
        };
        drop(plan.dario_guard.take());
        trace.ts_response_ms = Some(now_ms());
        fill_usage_and_cost(&state, &mut trace, &buf, is_sse);
        let text = String::from_utf8_lossy(&buf).to_string();
        if !status.is_success() {
            finalize_trace(&state, trace, &body, Some(&plan.body), Some(&buf));
            return Response::builder()
                .status(status)
                .header("content-type", "application/json")
                .header("x-alexandria-trace-id", &trace_id)
                .body(Body::from(buf))
                .unwrap_or_else(|e| {
                    error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string())
                });
        }
        let trimmed = text.trim_start();
        let looks_sse = trimmed.starts_with("event:") || trimmed.starts_with("data:");
        let upstream_final = if plan.upstream_format == "anthropic" {
            if is_sse || looks_sse {
                translate::parse_anthropic_sse_to_message(&text)
            } else {
                serde_json::from_str(&text).ok()
            }
        } else if plan.upstream_format == "gemini" {
            translate::parse_gemini_upstream_final(&text)
        } else if is_sse || looks_sse {
            translate::parse_responses_sse_final(&text)
        } else {
            serde_json::from_str(&text).ok()
        };
        let Some(upstream_final) = upstream_final else {
            let msg = "could not reassemble upstream response for translation";
            trace.error = Some(msg.to_string());
            finalize_trace(&state, trace, &body, Some(&plan.body), Some(&buf));
            return error_response(StatusCode::BAD_GATEWAY, msg);
        };
        let out = match (target, plan.upstream_format) {
            (RespondAs::Gemini, "gemini") => upstream_final.clone(),
            (RespondAs::Anthropic, "gemini") => {
                translate::gemini_response_to_anthropic(&upstream_final, &requested_model)
            }
            (RespondAs::OpenaiChat, "gemini") => translate::anthropic_response_to_openai_chat(
                &translate::gemini_response_to_anthropic(&upstream_final, &requested_model),
                &requested_model,
            ),
            (RespondAs::OpenaiResponses, "gemini") => {
                translate::anthropic_response_to_openai_responses(
                    &translate::gemini_response_to_anthropic(&upstream_final, &requested_model),
                    &requested_model,
                )
            }
            (RespondAs::OpenaiChat, "anthropic") => {
                translate::anthropic_response_to_openai_chat(&upstream_final, &requested_model)
            }
            (RespondAs::OpenaiResponses, "anthropic") => translate::anthropic_response_to_openai_responses(
                &upstream_final,
                &requested_model,
            ),
            (RespondAs::Gemini, "anthropic") => {
                translate::anthropic_response_to_gemini(&upstream_final, &requested_model)
            }
            (RespondAs::Gemini, _) => translate::anthropic_response_to_gemini(
                &translate::responses_final_to_anthropic(&upstream_final, &requested_model),
                &requested_model,
            ),
            (RespondAs::Anthropic, "anthropic") | (RespondAs::OpenaiResponses, _) => {
                upstream_final.clone()
            }
            (RespondAs::Anthropic, _) => {
                translate::responses_final_to_anthropic(&upstream_final, &requested_model)
            }
            (RespondAs::OpenaiChat, _) => {
                translate::responses_final_to_openai_chat(&upstream_final, &requested_model)
            }
        };
        let (out_ct, out_body) = if plan.client_stream {
            let sse = match target {
                RespondAs::Anthropic => translate::synth_anthropic_sse(&out),
                RespondAs::OpenaiChat => translate::synth_openai_chat_sse(&out),
                RespondAs::OpenaiResponses => translate::synth_openai_responses_sse(&out),
                RespondAs::Gemini => translate::synth_gemini_sse(&out),
            };
            ("text/event-stream", sse.into_bytes())
        } else {
            (
                "application/json",
                serde_json::to_vec(&out).unwrap_or_default(),
            )
        };
        finalize_trace(&state, trace, &body, Some(&plan.body), Some(&buf));
        return Response::builder()
            .status(StatusCode::OK)
            .header("content-type", out_ct)
            .header("x-alexandria-trace-id", &trace_id)
            .body(Body::from(out_body))
            .unwrap_or_else(|e| error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()));
    }

    if plan.destream {
        let buf = match upstream_resp.bytes().await {
            Ok(b) => b.to_vec(),
            Err(e) => {
                let msg = format!("upstream body read failed: {e}");
                trace.status = Some(502);
                trace.error = Some(msg.clone());
                finalize_trace(&state, trace, &body, Some(&plan.body), None);
                return error_response(StatusCode::BAD_GATEWAY, &msg);
            }
        };
        trace.ts_response_ms = Some(now_ms());
        fill_usage_and_cost(&state, &mut trace, &buf, is_sse);
        let text = String::from_utf8_lossy(&buf).to_string();
        let (out_status, out_body) = if status.is_success() {
            match extract_final_response(&text) {
                Some(v) => (StatusCode::OK, serde_json::to_vec(&v).unwrap_or_default()),
                None => {
                    let msg = "upstream stream ended without a response.completed event";
                    trace.error = Some(msg.to_string());
                    (
                        StatusCode::BAD_GATEWAY,
                        serde_json::to_vec(
                            &json!({"error": {"type": "alexandria", "message": msg}}),
                        )
                        .unwrap_or_default(),
                    )
                }
            }
        } else {
            (status, buf.clone())
        };
        finalize_trace(&state, trace, &body, Some(&plan.body), Some(&buf));
        return Response::builder()
            .status(out_status)
            .header("content-type", "application/json")
            .header("x-alexandria-trace-id", &trace_id)
            .body(Body::from(out_body))
            .unwrap_or_else(|e| error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()));
    }

    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(64);
    let mut upstream_stream = upstream_resp.bytes_stream();
    let state2 = state.clone();
    let client_body = body.clone();
    let upstream_body = plan.body.clone();
    let dario_guard = plan.dario_guard.take();
    tokio::spawn(async move {
        let _dario_guard = dario_guard;
        let _in_flight = in_flight;
        let mut buf: Vec<u8> = Vec::new();
        let mut stream_error: Option<String> = None;
        while let Some(chunk) = upstream_stream.next().await {
            match chunk {
                Ok(b) => {
                    buf.extend_from_slice(&b);
                    let _ = tx.send(Ok(b)).await;
                }
                Err(e) => {
                    stream_error = Some(format!("upstream stream error: {e}"));
                    let _ = tx
                        .send(Err(std::io::Error::new(
                            std::io::ErrorKind::Other,
                            e.to_string(),
                        )))
                        .await;
                    break;
                }
            }
        }
        drop(tx);
        trace.ts_response_ms = Some(now_ms());
        if trace.error.is_none() {
            trace.error = stream_error;
        }
        if trace.error.is_some() {
            if let (Some(dario), Some(gen)) = (
                &state2.dario,
                trace
                    .account_id
                    .as_deref()
                    .and_then(|id| id.strip_prefix("dario:")),
            ) {
                dario.suspect(gen);
            }
        }

        fill_usage_and_cost(&state2, &mut trace, &buf, is_sse);
        finalize_trace(
            &state2,
            trace,
            &client_body,
            Some(&upstream_body),
            Some(&buf),
        );
    });

    let mut response = Response::builder().status(status);
    for (k, v) in resp_headers.iter() {
        let key = k.as_str().to_lowercase();
        if [
            "transfer-encoding",
            "connection",
            "content-encoding",
            "content-length",
        ]
        .contains(&key.as_str())
        {
            continue;
        }
        response = response.header(k, v);
    }
    response = response.header("x-alexandria-trace-id", &trace_id);
    response
        .body(Body::from_stream(ReceiverStream::new(rx)))
        .unwrap_or_else(|e| error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))
}

fn fill_usage_and_cost(state: &AppState, trace: &mut TraceRecord, buf: &[u8], is_sse: bool) {
    let text = String::from_utf8_lossy(buf);
    let trimmed = text.trim_start();
    let looks_sse = trimmed.starts_with("event:") || trimmed.starts_with("data:");
    trace.usage = if is_sse || looks_sse {
        parse_sse_usage(&text)
    } else {
        serde_json::from_str::<Value>(&text)
            .map(|v| usage_from_json(&v))
            .unwrap_or_default()
    };
    if !trace.usage.is_empty() {
        if let Some(pricing) = trace
            .routed_model
            .as_deref()
            .and_then(|m| state.store.pricing_for(m))
        {
            let input_includes_cached = trace
                .upstream_format
                .as_deref()
                .map(|f| f.starts_with("openai"))
                .unwrap_or(false);
            trace.cost_usd = Some(compute_cost(&trace.usage, &pricing, input_includes_cached));
        }
    }
}

fn extract_final_response(text: &str) -> Option<Value> {
    let mut last: Option<Value> = None;
    for line in text.lines() {
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<Value>(data.trim()) else {
            continue;
        };
        if matches!(
            v["type"].as_str(),
            Some("response.completed") | Some("response.incomplete") | Some("response.failed")
        ) && v["response"].is_object()
        {
            last = Some(v["response"].clone());
        }
    }
    last
}

fn finalize_trace(
    state: &AppState,
    mut trace: TraceRecord,
    client_body: &[u8],
    upstream_body: Option<&[u8]>,
    resp_body: Option<&[u8]>,
) {
    let store = &state.store;
    match store.write_body(&trace.id, "request.json", client_body) {
        Ok(p) => trace.req_body_path = Some(p),
        Err(e) => tracing::warn!("failed to write request body: {e}"),
    }
    if let Some(up) = upstream_body {
        if up != client_body {
            match store.write_body(&trace.id, "upstream-request.json", up) {
                Ok(p) => trace.upstream_req_body_path = Some(p),
                Err(e) => tracing::warn!("failed to write upstream request body: {e}"),
            }
        }
    }
    if let Some(resp) = resp_body {
        match store.write_body(&trace.id, "response.body", resp) {
            Ok(p) => trace.resp_body_path = Some(p),
            Err(e) => tracing::warn!("failed to write response body: {e}"),
        }
    }
    if let Err(e) = store.insert_trace(&trace) {
        tracing::error!("failed to insert trace {}: {e}", trace.id);
    } else {
        tracing::info!(
            trace_id = %trace.id,
            status = trace.status,
            input = trace.usage.input_tokens,
            output = trace.usage.output_tokens,
            cost = trace.cost_usd,
            "trace recorded"
        );
    }
}
