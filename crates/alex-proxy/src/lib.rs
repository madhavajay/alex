use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use alex_auth::{
    now_ms, routing_reserve_blocked, routing_reserve_pct, routing_reset_selection, Account,
    AccountPolicy, AccountPolicyMode, RemovedAccount, Vault,
};
use alex_core::{
    compute_cost, conversation_root, parse_grpc_web_response, parse_since, parse_sse_usage,
    parse_trace_tags, parse_usage_api_response, route_model, usage_from_json,
    usage_to_limits_entry, validate_grpc_status_headers, window_label, ClientFormat, Provider,
    TraceIngestPayload, TraceRecord, GROK_CREDITS_ENDPOINT, GROK_CREDITS_REQUEST_BODY,
};
use alex_store::{KnownAccount, Store, TraceFilter};
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
const OPENROUTER_BASE: &str = "https://openrouter.ai/api/v1";
const CODEX_BASE: &str = "https://chatgpt.com/backend-api/codex";
const XAI_BASE: &str = "https://cli-chat-proxy.grok.com/v1";
const GROK_CLIENT_VERSION: &str = "0.2.77";
const ANTHROPIC_OAUTH_BETA: &str = "oauth-2025-04-20";
const GEMINI_CODE_ASSIST_BASE: &str = "https://cloudcode-pa.googleapis.com";
const GEMINI_CODE_ASSIST_VERSION: &str = "v1internal";
const GEMINI_API_BASE: &str = "https://generativelanguage.googleapis.com";
const CODEX_AFFINITY_TTL_MS: i64 = 30 * 24 * 60 * 60 * 1000;
const CODEX_AFFINITY_MAX_ENTRIES: usize = 10_000;

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

pub type UpdateApplyFuture =
    Pin<Box<dyn Future<Output = Result<Value, UpdateApplyError>> + Send + 'static>>;

#[derive(Debug)]
pub enum UpdateApplyError {
    Conflict(Value),
    Failed(String),
}

pub trait DaemonUpdater: Send + Sync {
    fn apply(&self) -> UpdateApplyFuture;
}

/// Shared request shape for the destructive reset control-plane operation.
/// Omitted booleans are false; omitted `dry_run` is true.
#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
pub struct ResetRequest {
    #[serde(default)]
    pub credentials: bool,
    #[serde(default)]
    pub settings: bool,
    #[serde(default)]
    pub traces: bool,
    #[serde(default)]
    pub harnesses: bool,
    #[serde(default)]
    pub cache: bool,
    #[serde(default = "reset_dry_run_default")]
    pub dry_run: bool,
}

fn reset_dry_run_default() -> bool {
    true
}

pub type ResetFuture = Pin<Box<dyn Future<Output = Result<Value>> + Send + 'static>>;

/// The executable owns configuration and harness-specific cleanup; the proxy
/// owns HTTP authentication and delegates reset execution through this hook.
pub trait ResetHandler: Send + Sync {
    fn reset(&self, state: Arc<AppState>, request: ResetRequest) -> ResetFuture;
}

fn suspect_dario(state: &AppState, account: &Account) {
    if account.kind != "dario" {
        return;
    }
    if let (Some(dario), Some(gen)) = (&state.dario, account.id.strip_prefix("dario:")) {
        dario.suspect(gen);
    }
}

fn routing_limits_from_headers(
    provider: Provider,
    headers: &reqwest::header::HeaderMap,
) -> Option<Value> {
    const OPENAI_LIMIT_HEADERS: &[&str] = &[
        "x-codex-primary-used-percent",
        "x-codex-primary-window-minutes",
        "x-codex-primary-reset-at",
        "x-codex-secondary-used-percent",
        "x-codex-secondary-window-minutes",
        "x-codex-secondary-reset-at",
        "x-codex-plan-type",
        "x-codex-active-limit",
        "x-codex-credits-balance",
        "x-codex-credits-has-credits",
        "x-codex-credits-unlimited",
    ];
    let mut safe = serde_json::Map::new();
    for (name, value) in headers {
        let name = name.as_str();
        let allowed = match provider {
            Provider::Openai => OPENAI_LIMIT_HEADERS.contains(&name),
            Provider::Anthropic => name.starts_with("anthropic-ratelimit-"),
            Provider::Xai => name.starts_with("x-ratelimit-"),
            Provider::Gemini | Provider::Openrouter | Provider::Amp => false,
        };
        if allowed {
            if let Ok(value) = value.to_str() {
                safe.insert(name.to_string(), json!(value));
            }
        }
    }
    if safe.is_empty() {
        return None;
    }
    let parsed = alex_core::parse_limit_headers(provider, &Value::Object(safe));
    let has_windows = parsed
        .get("windows")
        .and_then(Value::as_array)
        .map(|windows| !windows.is_empty())
        .unwrap_or(false);
    let has_plan = parsed.get("plan").is_some_and(|value| !value.is_null());
    (has_windows || has_plan).then_some(parsed)
}

pub struct AppState {
    pub local_key: Arc<std::sync::RwLock<String>>,
    pub vault: Arc<Vault>,
    pub store: Arc<Store>,
    pub http: reqwest::Client,
    pub dario: Option<Arc<dyn DarioRouter>>,
    pub in_flight: std::sync::atomic::AtomicI64,
    in_flight_requests: std::sync::Mutex<HashMap<String, InFlightRequest>>,
    upstream_stream_idle_timeout: Duration,
    pub started_ms: i64,
    pub base_url: String,
    pub anthropic_usage: std::sync::Mutex<UsageCache>,
    pub xai_usage: std::sync::Mutex<UsageCache>,
    pub amp_usage: std::sync::Mutex<UsageCache>,
    openrouter_models: std::sync::Mutex<Vec<String>>,
    pub logins: alex_auth::sessions::LoginManager,
    pub run_keys: std::sync::RwLock<HashMap<String, CachedRunKey>>,
    trace_ingest_lock: tokio::sync::Mutex<()>,
    pub update_status: Arc<tokio::sync::RwLock<Option<Value>>>,
    pub daemon_updater: std::sync::RwLock<Option<Arc<dyn DaemonUpdater>>>,
    pub reset_handler: std::sync::RwLock<Option<Arc<dyn ResetHandler>>>,
    codex_affinity: std::sync::Mutex<CodexAffinityCache>,
    codex_affinity_locks:
        std::sync::Mutex<HashMap<String, std::sync::Weak<tokio::sync::Mutex<()>>>>,
}

#[derive(Debug, Clone)]
pub struct CachedRunKey {
    pub kind: String,
    pub label: Option<String>,
    pub run_id: Option<String>,
    pub tags_json: Option<String>,
    pub expires_ms: Option<i64>,
}

#[derive(Debug, Clone)]
struct CodexAffinityEntry {
    account_id: String,
    expires_at_ms: i64,
}

#[derive(Debug)]
struct CodexAffinityCache {
    entries: HashMap<String, CodexAffinityEntry>,
    ttl_ms: i64,
    max_entries: usize,
}

impl Default for CodexAffinityCache {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            ttl_ms: CODEX_AFFINITY_TTL_MS,
            max_entries: CODEX_AFFINITY_MAX_ENTRIES,
        }
    }
}

impl CodexAffinityCache {
    fn preferred(&mut self, session_id: &str, now: i64) -> Option<String> {
        let entry = self.entries.get_mut(session_id)?;
        if entry.expires_at_ms <= now {
            self.entries.remove(session_id);
            return None;
        }
        // Sliding expiry keeps an active thread sticky without retaining
        // abandoned session IDs forever.
        entry.expires_at_ms = now.saturating_add(self.ttl_ms);
        Some(entry.account_id.clone())
    }

    fn bind(&mut self, session_id: &str, account_id: &str, now: i64) {
        self.entries.retain(|_, entry| entry.expires_at_ms > now);
        if !self.entries.contains_key(session_id) && self.entries.len() >= self.max_entries {
            if let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.expires_at_ms)
                .map(|(session, _)| session.clone())
            {
                self.entries.remove(&oldest);
            }
        }
        self.entries.insert(
            session_id.to_string(),
            CodexAffinityEntry {
                account_id: account_id.to_string(),
                expires_at_ms: now.saturating_add(self.ttl_ms),
            },
        );
    }

    fn unbind(&mut self, session_id: &str) {
        self.entries.remove(session_id);
    }
}

fn preferred_codex_account(state: &AppState, session_id: Option<&str>) -> Option<String> {
    let session_id = session_id.filter(|value| !value.is_empty())?;
    state
        .codex_affinity
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .preferred(session_id, now_ms())
}

fn codex_affinity_lock(state: &AppState, session_id: &str) -> Arc<tokio::sync::Mutex<()>> {
    let mut locks = state
        .codex_affinity_locks
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    locks.retain(|_, lock| lock.strong_count() > 0);
    if let Some(lock) = locks.get(session_id).and_then(std::sync::Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(tokio::sync::Mutex::new(()));
    locks.insert(session_id.to_string(), Arc::downgrade(&lock));
    lock
}

fn bind_codex_account(state: &AppState, session_id: Option<&str>, account: &Account) {
    let Some(session_id) = session_id.filter(|value| !value.is_empty()) else {
        return;
    };
    if account.provider != Provider::Openai {
        return;
    }
    let mut affinity = state
        .codex_affinity
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if account.kind == "oauth" {
        affinity.bind(session_id, &account.id, now_ms());
    } else {
        // An API-key fallback is not a Codex subscription affinity. Forget a
        // stale subscription binding so the failed account is not restored as
        // soon as its cooldown ends.
        affinity.unbind(session_id);
    }
}

#[derive(Debug, Clone)]
struct InFlightRequest {
    started_ms: i64,
    model: String,
    session_id: Option<String>,
    harness: Option<String>,
}

struct InFlight {
    state: Arc<AppState>,
    id: String,
}

impl InFlight {
    fn new(
        state: &Arc<AppState>,
        model: String,
        session_id: Option<String>,
        harness: Option<String>,
    ) -> Self {
        let id = uuid::Uuid::new_v4().to_string();
        state
            .in_flight_requests
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(
                id.clone(),
                InFlightRequest {
                    started_ms: now_ms(),
                    model,
                    session_id,
                    harness,
                },
            );
        state
            .in_flight
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Self {
            state: state.clone(),
            id,
        }
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        self.state
            .in_flight
            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        self.state
            .in_flight_requests
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&self.id);
    }
}

fn in_flight_requests(state: &AppState) -> Vec<Value> {
    let now = now_ms();
    let mut requests: Vec<_> = state
        .in_flight_requests
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .values()
        .map(|request| {
            json!({
                "age_s": (now.saturating_sub(request.started_ms)) / 1000,
                "model": request.model,
                "session_id": request.session_id,
                "harness": request.harness,
            })
        })
        .collect();
    requests.sort_by_key(|request| request["age_s"].as_i64().unwrap_or_default());
    requests
}

pub fn build_state(
    local_key: String,
    vault: Arc<Vault>,
    store: Arc<Store>,
    dario: Option<Arc<dyn DarioRouter>>,
    base_url: String,
    upstream_stream_idle_timeout: Duration,
) -> Arc<AppState> {
    // Import sidecars written by Vault::remove so a daemon restarted after a
    // terminal-side removal still exposes removed history to the Trace Browser.
    for removed in vault.removed_accounts() {
        if let Err(e) = store.tombstone_known_account(&known_removed_account(&removed), removed.removed_ms) {
            tracing::warn!(account = %removed.id, "failed to import account tombstone: {e}");
        }
    }
    for account in vault.list_cached() {
        if let Err(e) = store.upsert_known_account(&known_account(&account)) {
            tracing::warn!(account = %account.id, "failed to seed trace account catalogue: {e}");
        }
    }
    let http = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .build()
        .expect("reqwest client");
    Arc::new(AppState {
        local_key: Arc::new(std::sync::RwLock::new(local_key)),
        vault,
        store,
        http,
        dario,
        in_flight: std::sync::atomic::AtomicI64::new(0),
        in_flight_requests: std::sync::Mutex::new(HashMap::new()),
        upstream_stream_idle_timeout,
        started_ms: now_ms(),
        base_url,
        anthropic_usage: std::sync::Mutex::new(UsageCache::default()),
        xai_usage: std::sync::Mutex::new(UsageCache::default()),
        amp_usage: std::sync::Mutex::new(UsageCache::default()),
        openrouter_models: std::sync::Mutex::new(Vec::new()),
        logins: alex_auth::sessions::LoginManager::default(),
        run_keys: std::sync::RwLock::new(HashMap::new()),
        trace_ingest_lock: tokio::sync::Mutex::new(()),
        update_status: Arc::new(tokio::sync::RwLock::new(None)),
        daemon_updater: std::sync::RwLock::new(None),
        reset_handler: std::sync::RwLock::new(None),
        codex_affinity: std::sync::Mutex::new(CodexAffinityCache::default()),
        codex_affinity_locks: std::sync::Mutex::new(HashMap::new()),
    })
}

pub fn set_daemon_updater(state: &Arc<AppState>, updater: Arc<dyn DaemonUpdater>) {
    if let Ok(mut slot) = state.daemon_updater.write() {
        *slot = Some(updater);
    }
}

pub fn set_reset_handler(state: &Arc<AppState>, handler: Arc<dyn ResetHandler>) {
    if let Ok(mut slot) = state.reset_handler.write() {
        *slot = Some(handler);
    }
}

async fn require_local_key(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let ok = client_key(req.headers())
        .map(|k| state.local_key.read().map(|key| k == *key).unwrap_or(false))
        .unwrap_or(false);
    if !ok {
        return error_response(
            StatusCode::UNAUTHORIZED,
            "admin routes require x-api-key: <local_key>",
        );
    }
    next.run(req).await
}

async fn require_trace_ingest_key(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    match authenticate_trace_ingest(&state, req.headers()) {
        Ok(_) => next.run(req).await,
        Err(response) => response,
    }
}

async fn require_harness_event_key(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    match authenticate_harness_event(&state, req.headers()) {
        Ok(_) => next.run(req).await,
        Err(response) => response,
    }
}

pub fn router(state: Arc<AppState>) -> Router {
    // Control-plane + report routes: gated by the local key (sent as
    // `x-api-key: <key>` or `Authorization: Bearer <key>`) so a LAN/0.0.0.0
    // bind doesn't expose them. Run keys are NOT accepted here — a worker's
    // run key must not mint/revoke run keys or read the trace store.
    let gated = Router::new()
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
        .route("/admin/reset", post(admin_reset))
        .route("/admin/traces", get(admin_traces))
        .route("/admin/accounts", get(admin_accounts))
        .route("/admin/accounts/analytics", get(admin_account_analytics))
        .route(
            "/admin/routing/{provider}",
            get(admin_routing).put(admin_routing_update),
        )
        // Compatibility aliases for released macOS clients.
        .route(
            "/admin/codex-routing",
            get(admin_openai_routing).put(admin_openai_routing_update),
        )
        .route(
            "/admin/accounts/routing/openai",
            get(admin_openai_routing).put(admin_openai_routing_update),
        )
        .route(
            "/admin/accounts/{id}",
            axum::routing::delete(admin_account_remove).put(admin_account_update),
        )
        .route("/admin/auth/gemini-key", post(admin_auth_gemini_key))
        .route(
            "/admin/auth/openrouter-key",
            post(admin_auth_openrouter_key),
        )
        .route("/admin/health", get(admin_health))
        .route("/admin/analytics", get(admin_analytics))
        .route("/admin/limits", get(admin_limits))
        .route("/admin/update", get(admin_update).post(admin_update_apply))
        .route("/admin/dario", get(admin_dario))
        .route("/admin/dario/prompt-caches", get(admin_dario_prompt_caches))
        .route(
            "/admin/dario/prompt-caches/{key}",
            axum::routing::delete(admin_dario_prompt_cache_delete),
        )
        .route("/admin/auth/import", post(admin_auth_import))
        .route("/admin/auth/login/start", post(admin_auth_login_start))
        .route(
            "/admin/auth/login/complete",
            post(admin_auth_login_complete),
        )
        .route("/admin/auth/login/{id}", get(admin_auth_login_status))
        .route("/traces/search", get(traces_search))
        .route("/traces/accounts", get(traces_accounts))
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
        .route(
            "/traces/runs/{run_id}/export.ndjson",
            get(traces_run_export),
        )
        .route("/traces/runs/{run_id}/artifacts", get(traces_run_artifacts))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            require_local_key,
        ));
    // Authenticate ingest before Axum buffers/parses its potentially large
    // JSON body. The handler authenticates again to obtain ownership metadata.
    let ingest = Router::new()
        .route(
            "/traces/ingest",
            get(traces_ingest_status).post(traces_ingest),
        )
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            require_trace_ingest_key,
        ));
    // Harness hooks are authenticated before their JSON body is parsed. A
    // harness key is scoped to its label, which becomes the lineage namespace.
    let harness_events = Router::new()
        .route("/harness-events", post(harness_event))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            require_harness_event_key,
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
        .merge(ingest)
        .merge(harness_events)
        .merge(gated)
        .layer(axum::extract::DefaultBodyLimit::max(64 * 1024 * 1024))
        .with_state(state)
}

async fn admin_reset(
    State(state): State<Arc<AppState>>,
    body: Option<axum::Json<ResetRequest>>,
) -> Response {
    let request = body.map(|body| body.0).unwrap_or_default();
    let in_flight = state.in_flight.load(std::sync::atomic::Ordering::SeqCst);
    if !request.dry_run && in_flight > 0 {
        return error_response(
            StatusCode::CONFLICT,
            "cannot reset while routed requests are in flight; retry after they complete",
        );
    }
    let handler = state.reset_handler.read().ok().and_then(|slot| slot.clone());
    let Some(handler) = handler else {
        return error_response(
            StatusCode::NOT_IMPLEMENTED,
            "reset is not configured by this daemon",
        );
    };
    match handler.reset(state, request).await {
        Ok(plan) => axum::Json(plan).into_response(),
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
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

async fn admin_account_remove(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    let account = state.vault.list().await.into_iter().find(|a| a.id == id);
    let Some(account) = account else {
        return error_response(StatusCode::NOT_FOUND, &format!("unknown account '{id}'"));
    };
    if let Err(e) = state.store.tombstone_known_account(&known_account(&account), now_ms()) {
        // Do not remove credentials if we could not first preserve attribution.
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, &format!("could not preserve removed account history: {e}"));
    }
    match state.vault.remove(&id).await {
        Ok(true) => axum::Json(json!({"removed": id})).into_response(),
        Ok(false) => error_response(StatusCode::NOT_FOUND, &format!("unknown account '{id}'")),
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

fn known_account(account: &Account) -> KnownAccount {
    KnownAccount::new(
        account.id.clone(), account.provider.as_str(), account.name.clone(), account.kind.clone(),
        account.subscription_identity(), account.email(),
    )
}

fn known_removed_account(account: &RemovedAccount) -> KnownAccount {
    KnownAccount::new(
        account.id.clone(), account.provider.as_str(), account.name.clone(), account.kind.clone(),
        account.subscription_identity.clone(), account.email.clone(),
    )
}

fn bind_trace_account(store: &Store, trace: &mut TraceRecord, account: &Account) {
    trace.account_id = Some(account.id.clone());
    trace.subscription_identity = account.subscription_identity();
    if let Err(e) = store.upsert_known_account(&known_account(account)) {
        tracing::error!(account = %account.id, "failed to preserve trace account attribution: {e}");
    }
}

async fn admin_account_update(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    body: axum::Json<Value>,
) -> Response {
    let Some(paused) = body.0["paused"].as_bool() else {
        return error_response(StatusCode::BAD_REQUEST, "missing boolean 'paused'");
    };
    match state.vault.set_paused(&id, paused).await {
        Ok(()) => axum::Json(json!({"id": id, "paused": paused})).into_response(),
        Err(e) if e.to_string().starts_with("unknown account") => {
            error_response(StatusCode::NOT_FOUND, &e.to_string())
        }
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

async fn admin_auth_gemini_key(
    State(state): State<Arc<AppState>>,
    body: axum::Json<Value>,
) -> Response {
    let Some(key) = body.0["key"]
        .as_str()
        .map(str::trim)
        .filter(|k| !k.is_empty())
    else {
        return error_response(StatusCode::BAD_REQUEST, "missing 'key'");
    };
    let account = Account {
        id: alex_auth::named_account_id(Provider::Gemini, "api_key", "default"),
        provider: Provider::Gemini,
        kind: "api_key".into(),
        name: "default".into(),
        description: None,
        paused: false,
        label: Some("gemini (AI Studio key)".into()),
        access_token: None,
        refresh_token: None,
        id_token: None,
        api_key: Some(key.to_string()),
        expires_at_ms: None,
        last_refresh_ms: None,
        account_meta: Value::Null,
        cooldown_until_ms: None,
        status: "active".into(),
        path: None,
    };
    match state.vault.upsert(account).await {
        Ok(()) => axum::Json(json!({"saved": "gemini-api-key"})).into_response(),
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

async fn admin_auth_openrouter_key(
    State(state): State<Arc<AppState>>,
    body: axum::Json<Value>,
) -> Response {
    let remove = match body.0.get("remove") {
        Some(value) => match value.as_bool() {
            Some(value) => value,
            None => {
                return error_response(StatusCode::BAD_REQUEST, "'remove' must be boolean")
            }
        },
        None => false,
    };
    if remove {
        let has_configuration = ["key", "http_referer", "x_title"]
            .iter()
            .any(|field| body.0.get(*field).is_some_and(|value| !value.is_null()));
        if has_configuration {
            return error_response(
                StatusCode::BAD_REQUEST,
                "'remove' cannot be combined with 'key', 'http_referer', or 'x_title'",
            );
        }
        return match alex_auth::remove_openrouter_api_key(&state.vault).await {
            Ok(removed) => axum::Json(json!({
                "removed": removed.then_some("openrouter-api-key")
            }))
            .into_response(),
            Err(error) => {
                error_response(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string())
            }
        };
    }

    let Some(key) = body.0["key"]
        .as_str()
        .map(str::trim)
        .filter(|key| !key.is_empty())
    else {
        return error_response(StatusCode::BAD_REQUEST, "missing 'key'");
    };
    for field in ["http_referer", "x_title"] {
        if body
            .0
            .get(field)
            .is_some_and(|value| !value.is_null() && !value.is_string())
        {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("'{field}' must be a string"),
            );
        }
    }
    match alex_auth::save_openrouter_api_key(
        &state.vault,
        key,
        body.0["http_referer"].as_str(),
        body.0["x_title"].as_str(),
    )
    .await
    {
        Ok(id) => axum::Json(json!({"saved": id})).into_response(),
        Err(error) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
    }
}

async fn admin_auth_login_start(
    State(state): State<Arc<AppState>>,
    body: axum::Json<Value>,
) -> Response {
    let Some(provider) = body.0["provider"].as_str() else {
        return error_response(StatusCode::BAD_REQUEST, "missing 'provider'");
    };
    if body.0["auto_identity"].as_bool() == Some(true) {
        return match state.logins.start_auto(state.vault.clone(), provider).await {
            Ok(snapshot) => axum::Json(snapshot).into_response(),
            Err(e) => error_response(StatusCode::BAD_REQUEST, &e.to_string()),
        };
    }
    let name = body.0["name"].as_str().unwrap_or("default");
    match state.logins.start(state.vault.clone(), provider, name).await {
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
        "in_flight_requests": in_flight_requests(&state),
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
    let local_key = state.local_key.read().unwrap().clone();
    let (payload, exports) = connect_payload(&base, &local_key);
    if q.get("format").map(|f| f == "env").unwrap_or(false) {
        return Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "text/plain")
            .body(Body::from(exports))
            .unwrap_or_else(|e| error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()));
    }
    axum::Json(payload).into_response()
}

async fn refresh_openrouter_models(state: &AppState) {
    let Ok(account) = state.vault.account_for(Provider::Openrouter, false).await else {
        return;
    };
    let Ok(headers) = openrouter_auth_headers(&account) else {
        return;
    };
    let Ok(response) = state
        .http
        .get(format!("{OPENROUTER_BASE}/models"))
        .headers(headers)
        .timeout(Duration::from_secs(5))
        .send()
        .await
    else {
        return;
    };
    if !response.status().is_success() {
        return;
    }
    let Ok(payload) = response.json::<Value>().await else {
        return;
    };
    let models = alex_core::parse_openrouter_models_response(&payload);
    if let Ok(mut cached) = state.openrouter_models.lock() {
        *cached = models;
    }
}

async fn models(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    // OpenRouter is the sole dynamic provider catalog. Refresh only on an
    // explicit model-list request; Alexandria has no catalog refresh worker.
    refresh_openrouter_models(&state).await;
    let mut ids = state.store.pricing_models();
    if let Ok(models) = state.openrouter_models.lock() {
        ids.extend(models.iter().map(|id| format!("openrouter/{id}")));
    }
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

async fn admin_account_analytics(
    State(state): State<Arc<AppState>>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let minutes: i64 = q
        .get("since_minutes")
        .and_then(|s| s.parse().ok())
        .unwrap_or(24 * 60)
        .clamp(1, 30 * 24 * 60);
    let bucket_minutes: i64 = q
        .get("bucket_minutes")
        .and_then(|s| s.parse().ok())
        .unwrap_or(60)
        .clamp(1, 24 * 60);
    match state
        .store
        .account_analytics(now_ms() - minutes * 60_000, bucket_minutes * 60_000)
    {
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
    let account = state
        .vault
        .account_for(Provider::Anthropic, true)
        .await
        .ok()?;
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

const AMP_USAGE_URL: &str = "https://ampcode.com/api/internal?userDisplayBalanceInfo";

/// Amp Free / individual / workspace credits via the same API CodexBar uses.
async fn amp_usage_entry(state: &Arc<AppState>) -> Option<Value> {
    let account = state.vault.account_for(Provider::Amp, false).await.ok()?;
    let token = account
        .api_key
        .as_deref()
        .or(account.access_token.as_deref())?
        .to_string();
    {
        let cache = state.amp_usage.lock().unwrap();
        if cache.entry.is_some() && now_ms() < cache.fetched_at_ms + USAGE_CACHE_TTL_MS {
            return cache.entry.clone();
        }
        if now_ms() < cache.cooldown_until_ms {
            return cache.entry.clone();
        }
    }
    let body = json!({ "method": "userDisplayBalanceInfo", "params": {} });
    let result = state
        .http
        .post(AMP_USAGE_URL)
        .header("authorization", format!("Bearer {token}"))
        .header("accept", "application/json")
        .header("content-type", "application/json")
        .header("user-agent", "alexandria-amp-usage")
        .json(&body)
        .send()
        .await;
    match result {
        Ok(resp) if resp.status().is_success() => {
            let raw = resp.text().await.unwrap_or_default();
            match parse_usage_api_response(&raw) {
                Ok(snap) => {
                    let entry = usage_to_limits_entry(&snap, account.label.as_deref());
                    let mut cache = state.amp_usage.lock().unwrap();
                    cache.fetched_at_ms = now_ms();
                    cache.cooldown_until_ms = 0;
                    cache.failures = 0;
                    cache.entry = Some(entry.clone());
                    Some(entry)
                }
                Err(e) => {
                    tracing::debug!(error = %e, "amp usage parse failed");
                    let mut cache = state.amp_usage.lock().unwrap();
                    cache.failures += 1;
                    let cooldown = usage_backoff_ms(cache.failures, None);
                    cache.cooldown_until_ms = now_ms() + cooldown;
                    cache.entry.clone().or_else(|| {
                        Some(json!({
                            "provider": "amp",
                            "source": "amp usage API",
                            "error": e,
                            "plan": account.label,
                        }))
                    })
                }
            }
        }
        Ok(resp) => {
            let status = resp.status().as_u16();
            let mut cache = state.amp_usage.lock().unwrap();
            cache.failures += 1;
            let cooldown = usage_backoff_ms(cache.failures, None);
            cache.cooldown_until_ms = now_ms() + cooldown;
            tracing::debug!(status, "amp usage endpoint unavailable; backing off");
            cache.entry.clone().or_else(|| {
                Some(json!({
                    "provider": "amp",
                    "source": "amp usage API",
                    "error": format!("HTTP {status}"),
                    "plan": account.label,
                }))
            })
        }
        Err(e) => {
            let mut cache = state.amp_usage.lock().unwrap();
            cache.failures += 1;
            let cooldown = usage_backoff_ms(cache.failures, None);
            cache.cooldown_until_ms = now_ms() + cooldown;
            tracing::debug!(error = %e, "amp usage request failed");
            cache.entry.clone()
        }
    }
}

/// Fetch SuperGrok weekly credits from grok.com gRPC-web billing RPC.
/// Uses the vault's xAI OAuth access token. Degrades gracefully on any failure.
async fn xai_usage_entry(state: &Arc<AppState>) -> Option<Value> {
    let account = state.vault.account_for(Provider::Xai, true).await.ok()?;
    if account.kind != "oauth" {
        return None;
    }
    let token = account.access_token.as_deref()?.to_string();
    {
        let cache = state.xai_usage.lock().unwrap();
        if cache.entry.is_some() && now_ms() < cache.fetched_at_ms + USAGE_CACHE_TTL_MS {
            return cache.entry.clone();
        }
        if now_ms() < cache.cooldown_until_ms {
            return cache.entry.clone();
        }
    }

    let result = state
        .http
        .post(GROK_CREDITS_ENDPOINT)
        .header("authorization", format!("Bearer {token}"))
        .header("origin", "https://grok.com")
        .header("referer", "https://grok.com/?_s=usage")
        .header("accept", "*/*")
        .header("content-type", "application/grpc-web+proto")
        .header("x-grpc-web", "1")
        .header("x-user-agent", "connect-es/2.1.1")
        .header("user-agent", "Alexandria")
        .body(GROK_CREDITS_REQUEST_BODY.to_vec())
        .timeout(Duration::from_secs(15))
        .send()
        .await;

    match result {
        Ok(resp) if resp.status().is_success() => {
            let headers_for_grpc: Vec<(String, String)> = resp
                .headers()
                .iter()
                .filter_map(|(k, v)| {
                    let key = k.as_str();
                    if key.starts_with("grpc-") {
                        v.to_str()
                            .ok()
                            .map(|val| (key.to_string(), val.to_string()))
                    } else {
                        None
                    }
                })
                .collect();
            if let Err(e) = validate_grpc_status_headers(headers_for_grpc) {
                tracing::debug!(error = %e, "xai grok credits grpc header status failed");
                let mut cache = state.xai_usage.lock().unwrap();
                cache.failures += 1;
                let cooldown = usage_backoff_ms(cache.failures, None);
                cache.cooldown_until_ms = now_ms() + cooldown;
                return cache.entry.clone();
            }
            let body = match resp.bytes().await {
                Ok(b) => b,
                Err(e) => {
                    tracing::debug!(error = %e, "xai grok credits body read failed");
                    let mut cache = state.xai_usage.lock().unwrap();
                    cache.failures += 1;
                    let cooldown = usage_backoff_ms(cache.failures, None);
                    cache.cooldown_until_ms = now_ms() + cooldown;
                    return cache.entry.clone();
                }
            };
            let now_s = now_ms() / 1000;
            match parse_grpc_web_response(&body, now_s) {
                Ok(snap) => {
                    let label = window_label(snap.resets_at_s, now_s);
                    let mut window = json!({
                        "window": label,
                        "used_pct": snap.used_percent,
                    });
                    if let Some(ts) = snap.resets_at_s {
                        window["resets_at_s"] = json!(ts);
                    }
                    let entry = json!({
                        "provider": "xai",
                        "source": "grok web billing",
                        "plan": account.label,
                        "windows": [window],
                    });
                    let mut cache = state.xai_usage.lock().unwrap();
                    cache.fetched_at_ms = now_ms();
                    cache.cooldown_until_ms = 0;
                    cache.failures = 0;
                    cache.entry = Some(entry.clone());
                    Some(entry)
                }
                Err(e) => {
                    tracing::debug!(error = %e, "xai grok credits parse failed");
                    let mut cache = state.xai_usage.lock().unwrap();
                    cache.failures += 1;
                    let cooldown = usage_backoff_ms(cache.failures, None);
                    cache.cooldown_until_ms = now_ms() + cooldown;
                    cache.entry.clone()
                }
            }
        }
        Ok(resp) => {
            let status = resp.status().as_u16();
            let retry_after = resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<i64>().ok())
                .map(|s| s.clamp(30, 3600) * 1000);
            let mut cache = state.xai_usage.lock().unwrap();
            cache.failures += 1;
            let cooldown = usage_backoff_ms(cache.failures, retry_after);
            cache.cooldown_until_ms = now_ms() + cooldown;
            tracing::debug!(
                status,
                failures = cache.failures,
                cooldown_ms = cooldown,
                "xai grok web billing unavailable; backing off"
            );
            cache.entry.clone()
        }
        Err(e) => {
            tracing::debug!(error = %e, "xai grok web billing request failed");
            let mut cache = state.xai_usage.lock().unwrap();
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
    if let Some(entry) = xai_usage_entry(state).await {
        providers.push(entry);
    }
    if let Some(entry) = amp_usage_entry(state).await {
        providers.push(entry);
    }
    // Captured response headers outlive their credential. Do not turn that
    // historical data into a current provider card after the account is gone.
    let account_providers: HashSet<String> = state
        .vault
        .list()
        .await
        .into_iter()
        .map(|account| account.provider.as_str().to_string())
        .collect();
    for (provider_str, ts_ms, headers_json) in
        state.store.latest_provider_headers().unwrap_or_default()
    {
        if !account_providers.contains(&provider_str) {
            continue;
        }
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

async fn admin_update(State(state): State<Arc<AppState>>) -> Response {
    let stored = state.update_status.read().await.clone();
    let mut body = json!({
        "current": env!("CARGO_PKG_VERSION"),
        "latest": null,
        "update_available": false,
        "checked_at_ms": null,
    });
    if let Some(Value::Object(fields)) = stored {
        if let Some(obj) = body.as_object_mut() {
            obj.extend(fields);
            obj.insert("current".into(), json!(env!("CARGO_PKG_VERSION")));
        }
    }
    axum::Json(body).into_response()
}

async fn admin_update_apply(State(state): State<Arc<AppState>>) -> Response {
    let updater = state
        .daemon_updater
        .read()
        .ok()
        .and_then(|slot| slot.clone());
    let Some(updater) = updater else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "daemon updater is not configured",
        );
    };
    match updater.apply().await {
        Ok(body) => {
            let status = if body["applying"].as_bool() == Some(true) {
                StatusCode::ACCEPTED
            } else {
                StatusCode::OK
            };
            (status, axum::Json(body)).into_response()
        }
        Err(UpdateApplyError::Conflict(body)) => {
            (StatusCode::CONFLICT, axum::Json(body)).into_response()
        }
        Err(UpdateApplyError::Failed(message)) => {
            error_response(StatusCode::INTERNAL_SERVER_ERROR, &message)
        }
    }
}

async fn admin_dario(State(state): State<Arc<AppState>>) -> Response {
    match &state.dario {
        Some(d) => {
            let mut status = d.status();
            if let Some(obj) = status.as_object_mut() {
                obj.insert("prompt_caches".into(), json!(dario_prompt_caches(&state)));
            }
            axum::Json(status).into_response()
        }
        None => error_response(StatusCode::NOT_FOUND, "dario mode is not enabled"),
    }
}

fn dario_prompt_cache_dir(state: &AppState) -> PathBuf {
    state.store.data_dir.join("dario-prompt-cache")
}

fn dario_prompt_cache_key_valid(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 128
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

fn dario_prompt_cache_summary(path: PathBuf) -> Option<Value> {
    let raw = std::fs::read_to_string(&path).ok()?;
    let value: Value = serde_json::from_str(&raw).ok()?;
    let key = path.file_stem()?.to_string_lossy().to_string();
    let runs: Vec<Value> = value["runs"]
        .as_array()
        .map(|runs| runs.iter().rev().take(12).cloned().collect())
        .unwrap_or_default();
    Some(json!({
        "key": value["key"].as_str().unwrap_or(&key),
        "model": value["model"],
        "source": value["source"],
        "captured_at": value["captured_at"],
        "last_used_at": value["last_used_at"],
        "trace_id": value["trace_id"],
        "claude_bin": value["claude_bin"],
        "claude_version": value["claude_version"],
        "system_prompt_chars": value["system_prompt_chars"],
        "agent_identity_chars": value["agent_identity_chars"],
        "path": path.to_string_lossy(),
        "runs": runs,
    }))
}

fn dario_prompt_caches(state: &AppState) -> Vec<Value> {
    let dir = dario_prompt_cache_dir(state);
    let mut caches: Vec<Value> = std::fs::read_dir(dir)
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            (path.extension().and_then(|s| s.to_str()) == Some("json"))
                .then(|| dario_prompt_cache_summary(path))
                .flatten()
        })
        .collect();
    caches.sort_by(|a, b| {
        b["last_used_at"]
            .as_str()
            .unwrap_or("")
            .cmp(a["last_used_at"].as_str().unwrap_or(""))
    });
    caches
}

async fn admin_dario_prompt_caches(State(state): State<Arc<AppState>>) -> Response {
    axum::Json(json!({"prompt_caches": dario_prompt_caches(&state)})).into_response()
}

async fn admin_dario_prompt_cache_delete(
    State(state): State<Arc<AppState>>,
    Path(key): Path<String>,
) -> Response {
    if !dario_prompt_cache_key_valid(&key) {
        return error_response(StatusCode::BAD_REQUEST, "invalid prompt cache key");
    }
    let path = dario_prompt_cache_dir(&state).join(format!("{key}.json"));
    match std::fs::remove_file(&path) {
        Ok(()) => axum::Json(json!({"deleted": true, "key": key})).into_response(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            error_response(StatusCode::NOT_FOUND, "unknown prompt cache")
        }
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
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
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "'older_than_ms' must be an integer",
                )
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
        account_id: q.get("account_id").cloned(),
        account_ids: q.get("account_ids").map(|ids| ids.split(',').map(str::trim).filter(|id| !id.is_empty()).map(String::from).collect()).unwrap_or_default(),
        path: q.get("path").cloned(),
        harness: q.get("harness").cloned(),
        status: q.get("status").and_then(|s| s.parse().ok()),
        errors_only: q
            .get("errors")
            .map(|v| v == "1" || v == "true")
            .unwrap_or(false),
        key_fingerprint: q.get("key_fingerprint").cloned(),
        reasoning_effort: q.get("effort").cloned(),
        limit: q.get("limit").and_then(|s| s.parse().ok()).unwrap_or(200),
    }
}

fn wants_bodies(q: &HashMap<String, String>) -> bool {
    q.get("bodies")
        .map(|v| v == "1" || v == "true")
        .unwrap_or(false)
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

/// Trace Browser account selector API. The returned list deliberately includes
/// removed accounts with `removed: true`. Multi-select with
/// `/traces/search?account_ids=id1,id2`; matching durable identities are
/// included as well as the historical ids.
async fn traces_accounts(State(state): State<Arc<AppState>>) -> Response {
    match state.store.list_known_accounts() {
        Ok(accounts) => axum::Json(json!({"accounts": accounts})).into_response(),
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

fn body_date_dir_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(i, b)| i == 4 || i == 7 || b.is_ascii_digit())
}

fn dario_capture_suffix(kind: &str) -> Option<&'static str> {
    match kind {
        "dario-upstream-request" => Some("dario-upstream-request.json.gz"),
        "dario-upstream-response" => Some("dario-upstream-response.json.gz"),
        _ => None,
    }
}

fn is_dario_trace(row: &Value) -> bool {
    row["account_id"]
        .as_str()
        .map(|id| id.starts_with("dario:"))
        .unwrap_or(false)
}

fn find_dario_capture_path(state: &AppState, row: &Value, kind: &str) -> Option<String> {
    if !is_dario_trace(row) {
        return None;
    }
    let trace_id = row["id"].as_str()?;
    let suffix = dario_capture_suffix(kind)?;
    let filename = format!("{trace_id}.{suffix}");
    let bodies = state.store.data_dir.join("bodies");
    let mut days: Vec<PathBuf> = std::fs::read_dir(&bodies)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            (entry.path().is_dir() && body_date_dir_name(&name)).then(|| entry.path())
        })
        .collect();
    days.sort_by(|a, b| b.file_name().cmp(&a.file_name()));
    for day in days {
        let path = day.join(&filename);
        if path.is_file() {
            return Some(path.to_string_lossy().to_string());
        }
    }
    None
}

fn dario_capture_summary(state: &AppState, row: &Value) -> Option<Value> {
    let request_path = find_dario_capture_path(state, row, "dario-upstream-request");
    let response_path = find_dario_capture_path(state, row, "dario-upstream-response");
    if request_path.is_none() && response_path.is_none() {
        return None;
    }
    let prompt_cache = request_path
        .as_deref()
        .and_then(|path| read_gz_json(Some(path)))
        .and_then(|body| {
            body["prompt_cache"]
                .as_object()
                .map(|_| body["prompt_cache"].clone())
        });
    Some(json!({
        "request_available": request_path.is_some(),
        "response_available": response_path.is_some(),
        "request_path": request_path,
        "response_path": response_path,
        "prompt_cache": prompt_cache,
    }))
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

fn transcript_assistant_blocks(resp_text: &str) -> Vec<Value> {
    let Ok(response) = serde_json::from_str::<Value>(resp_text) else {
        return Vec::new();
    };
    let mut tool_calls = 0usize;
    response
        .pointer("/_alexandria/assistant_blocks")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|block| match block["type"].as_str() {
            Some("text") => block["text"].as_str().map(
                |text| json!({"type": "text", "text": truncate_chars(text.to_string(), 8000)}),
            ),
            Some("tool_call") if tool_calls < 24 => {
                let name = block["name"].as_str()?;
                tool_calls += 1;
                Some(json!({
                    "type": "tool_call",
                    "name": name,
                    "arguments": truncate_chars(
                        block["arguments"].as_str().unwrap_or_default().to_string(),
                        600,
                    ),
                }))
            }
            _ => None,
        })
        .take(64)
        .collect()
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
    let assistant_blocks = resp_text
        .as_deref()
        .map(transcript_assistant_blocks)
        .unwrap_or_default();
    json!({
        "trace_id": row["id"],
        "ts_request_ms": row["ts_request_ms"],
        "ts_response_ms": row["ts_response_ms"],
        "model": row["routed_model"],
        "provider": row["upstream_provider"],
        "status": row["status"],
        "input_tokens": row["input_tokens"],
        "output_tokens": row["output_tokens"],
        "reasoning_effort": row["reasoning_effort"],
        "thinking_budget": row["thinking_budget"],
        "cost_usd": row["cost_usd"],
        "account_id": row["account_id"],
        "error": row["error"],
        "user": user,
        "assistant": assistant,
        "tool_calls": tool_calls,
        "assistant_blocks": assistant_blocks,
    })
}

fn openai_responses_user_history_signature(request: &Value) -> Option<String> {
    let history = request["input"]
        .as_array()?
        .iter()
        .filter(|item| {
            item["type"].as_str().unwrap_or("message") == "message"
                && item["role"].as_str() == Some("user")
        })
        .map(|item| item["content"].clone())
        .collect::<Vec<_>>();
    (!history.is_empty())
        .then(|| serde_json::to_string(&history).ok())
        .flatten()
}

fn codex_user_history_signature(row: &Value) -> Option<String> {
    if row["harness"].as_str() != Some("codex")
        || row["client_format"].as_str() != Some("openai-responses")
    {
        return None;
    }
    openai_responses_user_history_signature(&read_gz_json(row["req_body_path"].as_str())?)
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
    let mut previous_codex_user_history: Option<String> = None;
    let turns: Vec<Value> = rows
        .iter()
        .take(limit)
        .map(|row| {
            let signature = codex_user_history_signature(row);
            let replayed_user = signature.is_some() && signature == previous_codex_user_history;
            if signature.is_some() {
                previous_codex_user_history = signature;
            }
            let mut turn = transcript_turn(row);
            if replayed_user {
                turn["user"] = Value::Null;
            }
            turn
        })
        .collect();
    axum::Json(json!({"session_id": session_id, "turns": turns})).into_response()
}

fn trace_reasoning_fields(req: &Value) -> (Option<String>, Option<i64>) {
    (
        req["reasoning"]["effort"]
            .as_str()
            .or_else(|| req["output_config"]["effort"].as_str())
            .map(String::from),
        req["thinking"]["budget_tokens"].as_i64(),
    )
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
    let (reasoning_effort, thinking_budget) = trace_reasoning_fields(req);
    json!({
        "reasoning_effort": reasoning_effort,
        "thinking_budget": thinking_budget,
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
            let mut extras = read_gz_json(row["req_body_path"].as_str())
                .map(|req| trace_extras(&req))
                .unwrap_or_else(|| json!({}));
            if let Some(summary) = dario_capture_summary(&state, &row) {
                if !extras.is_object() {
                    extras = json!({});
                }
                if let Some(obj) = extras.as_object_mut() {
                    obj.insert("dario_capture".into(), summary);
                }
            }
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
        Ok(None) => return error_response(StatusCode::NOT_FOUND, &format!("unknown trace '{id}'")),
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    let path = match kind.as_str() {
        "request" => row["req_body_path"].as_str().map(String::from),
        "upstream-request" => row["upstream_req_body_path"].as_str().map(String::from),
        "response" => row["resp_body_path"].as_str().map(String::from),
        "dario-upstream-request" | "dario-upstream-response" => {
            find_dario_capture_path(&state, &row, &kind)
        }
        _ => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "kind must be request|upstream-request|response|dario-upstream-request|dario-upstream-response",
            )
        }
    };
    match read_gz_text(path.as_deref()) {
        Some(text) => {
            let ct = if text.trim_start().starts_with('{') || text.trim_start().starts_with('[') {
                "application/json; charset=utf-8"
            } else {
                "text/plain; charset=utf-8"
            };
            Response::builder()
                .status(StatusCode::OK)
                .header("content-type", ct)
                .header("x-alexandria-body-path", path.as_deref().unwrap_or(""))
                .body(Body::from(text))
                .unwrap_or_else(|e| {
                    error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string())
                })
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
        Ok(None) => return error_response(StatusCode::NOT_FOUND, &format!("unknown trace '{id}'")),
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
        Ok(None) => return error_response(StatusCode::NOT_FOUND, &format!("unknown trace '{id}'")),
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
            let email = a.email();
            let policy = state.vault.policy(a.provider);
            let reserve_pct = routing_reserve_pct(&a, &policy);
            let routing = json!({
                "eligible": !policy.disabled.iter().any(|name| name == &a.name || name == &a.id),
                "priority": policy.order.iter().position(|name| name == &a.name),
                "reserve_pct": reserve_pct,
                "reserve_blocked": routing_reserve_blocked(&a, reserve_pct, now_ms() / 1000),
                "reset_selection": routing_reset_selection(&a, now_ms() / 1000),
            });
            let limits = a.account_meta
                .get("routing_limits")
                .or_else(|| a.account_meta.get("codex_limits"))
                .cloned()
                .unwrap_or(Value::Null);
            json!({
                "id": a.id,
                "provider": a.provider.as_str(),
                "name": a.name,
                "kind": a.kind,
                "label": a.label,
                "description": a.description,
                "email": email,
                "paused": a.paused,
                "path": a.path.map(|p| p.display().to_string()),
                "status": a.status,
                "expires_at_ms": a.expires_at_ms,
                "expires_in_s": a.expires_at_ms.map(|e| (e - now_ms()) / 1000),
                "routing": routing,
                "limits": limits,
            })
        })
        .collect();
    axum::Json(json!({"accounts": accounts}))
}

fn routing_strategy_name(mode: &AccountPolicyMode) -> &'static str {
    match mode {
        AccountPolicyMode::ResetFirst => "reset_first",
        AccountPolicyMode::RoundRobin => "round_robin",
        AccountPolicyMode::Priority | AccountPolicyMode::Threshold => "priority",
    }
}

async fn routing_snapshot(state: &Arc<AppState>, provider: Provider) -> Value {
    let policy = state.vault.policy(provider);
    let mut accounts: Vec<Account> = state
        .vault
        .list()
        .await
        .into_iter()
        .filter(|account| account.provider == provider)
        .collect();
    accounts.sort_by_key(|account| {
        (
            policy
                .order
                .iter()
                .position(|name| name == &account.name)
                .unwrap_or(usize::MAX / 2),
            account.name.clone(),
            account.id.clone(),
        )
    });
    let accounts: Vec<Value> = accounts
        .into_iter()
        .enumerate()
        .map(|(fallback_priority, account)| {
            let eligible = !policy
                .disabled
                .iter()
                .any(|name| name == &account.name || name == &account.id);
            let priority = policy
                .order
                .iter()
                .position(|name| name == &account.name)
                .unwrap_or(fallback_priority);
            let limits = account
                .account_meta
                .get("routing_limits")
                .or_else(|| account.account_meta.get("codex_limits"))
                .cloned()
                .unwrap_or_else(|| json!({}));
            let reserve_pct = routing_reserve_pct(&account, &policy);
            let reset_selection = routing_reset_selection(&account, now_ms() / 1000);
            json!({
                "account_id": account.id,
                "eligible": eligible,
                "priority": priority,
                "reserve_pct": reserve_pct,
                "reserve_blocked": routing_reserve_blocked(&account, reserve_pct, now_ms() / 1000),
                "reset_selection": reset_selection,
                "observed_at_ms": limits.get("observed_at_ms"),
                "plan": limits.get("plan"),
                "active_limit": limits.get("active_limit"),
                "windows": limits.get("windows").cloned().unwrap_or_else(|| json!([])),
                "credits": limits.get("credits").cloned().unwrap_or(Value::Null),
            })
        })
        .collect();
    json!({
        "provider": provider.as_str(),
        "strategy": routing_strategy_name(&policy.mode),
        "reserve_pct": policy.reserve_pct.unwrap_or(10).min(100),
        "allow_mid_thread_failover": policy.allow_mid_thread_failover,
        "reset_selection_rule": "highest_used_pct_then_earliest_reset",
        "accounts": accounts,
    })
}

async fn update_routing(
    state: Arc<AppState>,
    provider: Provider,
    body: Value,
) -> Response {
    let current_policy = state.vault.policy(provider);
    let mode = match body.get("strategy").and_then(Value::as_str) {
        Some("reset_first") => AccountPolicyMode::ResetFirst,
        Some("priority") => AccountPolicyMode::Priority,
        Some("round_robin") => AccountPolicyMode::RoundRobin,
        _ => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "strategy must be reset_first, priority, or round_robin",
            )
        }
    };
    let reserve_pct = match body.get("reserve_pct") {
        Some(value) => match value.as_u64() {
            Some(value) => value,
            None => return error_response(StatusCode::BAD_REQUEST, "reserve_pct must be an integer"),
        },
        None => current_policy.reserve_pct.unwrap_or(10) as u64,
    };
    if reserve_pct > 100 {
        return error_response(StatusCode::BAD_REQUEST, "reserve_pct must be between 0 and 100");
    }
    let allow_mid_thread_failover = match body.get("allow_mid_thread_failover") {
        Some(value) => match value.as_bool() {
            Some(value) => value,
            None => return error_response(
                StatusCode::BAD_REQUEST,
                "allow_mid_thread_failover must be boolean",
            ),
        },
        None => current_policy.allow_mid_thread_failover,
    };
    let Some(requested) = body.get("accounts").and_then(Value::as_array) else {
        return error_response(StatusCode::BAD_REQUEST, "accounts must be an array");
    };
    let provider_accounts: Vec<Account> = state
        .vault
        .list()
        .await
        .into_iter()
        .filter(|account| account.provider == provider)
        .collect();
    let by_id: HashMap<String, String> = provider_accounts
        .iter()
        .map(|account| (account.id.clone(), account.name.clone()))
        .collect();
    let mut seen = std::collections::HashSet::new();
    let mut ordered = Vec::new();
    for item in requested {
        let Some(account_id) = item.get("account_id").and_then(Value::as_str) else {
            return error_response(StatusCode::BAD_REQUEST, "each account needs account_id");
        };
        let Some(eligible) = item.get("eligible").and_then(Value::as_bool) else {
            return error_response(StatusCode::BAD_REQUEST, "each account needs boolean eligible");
        };
        let Some(priority) = item.get("priority").and_then(Value::as_u64) else {
            return error_response(StatusCode::BAD_REQUEST, "each account needs integer priority");
        };
        let account_reserve_pct = match item.get("reserve_pct") {
            Some(value) => match value.as_u64() {
                Some(value) if value <= 100 => value as u8,
                Some(_) => return error_response(
                    StatusCode::BAD_REQUEST,
                    "account reserve_pct must be between 0 and 100",
                ),
                None => return error_response(
                    StatusCode::BAD_REQUEST,
                    "account reserve_pct must be an integer",
                ),
            },
            None => reserve_pct as u8,
        };
        let Some(name) = by_id.get(account_id) else {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("unknown {} account '{account_id}'", provider.as_str()),
            );
        };
        if !seen.insert(account_id.to_string()) {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("duplicate {} account '{account_id}'", provider.as_str()),
            );
        }
        ordered.push((priority, name.clone(), eligible, account_reserve_pct));
    }
    if seen.len() != by_id.len() {
        return error_response(
            StatusCode::CONFLICT,
            "account list changed; refresh Settings and save again",
        );
    }
    ordered.sort_by_key(|(priority, name, _, _)| (*priority, name.clone()));
    let policy = AccountPolicy {
        order: ordered.iter().map(|(_, name, _, _)| name.clone()).collect(),
        mode,
        threshold_pct: None,
        reserve_pct: Some(reserve_pct as u8),
        account_reserve_pct: ordered
            .iter()
            .map(|(_, name, _, reserve_pct)| (name.clone(), *reserve_pct))
            .collect(),
        allow_mid_thread_failover,
        disabled: ordered
            .iter()
            .filter(|(_, _, eligible, _)| !*eligible)
            .map(|(_, name, _, _)| name.clone())
            .collect(),
    };
    match state
        .vault
        .set_policy_persisted(provider, policy)
        .await
    {
        Ok(()) => axum::Json(routing_snapshot(&state, provider).await).into_response(),
        Err(error) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
    }
}

fn routing_provider(value: &str) -> Result<Provider, Response> {
    Provider::from_str_loose(value).ok_or_else(|| {
        error_response(
            StatusCode::BAD_REQUEST,
            &format!("unknown provider '{value}'"),
        )
    })
}

async fn admin_routing(
    State(state): State<Arc<AppState>>,
    Path(provider): Path<String>,
) -> Response {
    match routing_provider(&provider) {
        Ok(provider) => axum::Json(routing_snapshot(&state, provider).await).into_response(),
        Err(response) => response,
    }
}

async fn admin_routing_update(
    State(state): State<Arc<AppState>>,
    Path(provider): Path<String>,
    body: axum::Json<Value>,
) -> Response {
    match routing_provider(&provider) {
        Ok(provider) => update_routing(state, provider, body.0).await,
        Err(response) => response,
    }
}

async fn admin_openai_routing(State(state): State<Arc<AppState>>) -> Response {
    axum::Json(routing_snapshot(&state, Provider::Openai).await).into_response()
}

async fn admin_openai_routing_update(
    State(state): State<Arc<AppState>>,
    body: axum::Json<Value>,
) -> Response {
    update_routing(state, Provider::Openai, body.0).await
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
                "name": a.name,
                "kind": a.kind,
                "status": a.status,
                "paused": a.paused,
                "path": a.path.map(|p| p.display().to_string()),
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
    pub openrouter: String,
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
        Provider::Openrouter => (
            ClientFormat::OpenaiChat,
            "/v1/chat/completions",
            json!({
                "model": format!("openrouter/{}", models.openrouter),
                "stream": false,
                "messages": [{"role": "user", "content": prompt}],
            }),
        ),
        Provider::Amp => {
            let account_id = state
                .vault
                .account_for(Provider::Amp, false)
                .await
                .ok()
                .map(|a| a.id);
            let entry = amp_usage_entry(state).await;
            let ok = entry
                .as_ref()
                .map(|e| e.get("error").is_none())
                .unwrap_or(false);
            let message = entry
                .as_ref()
                .and_then(|e| {
                    e.get("display_text")
                        .and_then(|v| v.as_str())
                        .map(|s| s.lines().next().unwrap_or(s).to_string())
                        .or_else(|| e.get("error").and_then(|v| v.as_str()).map(String::from))
                })
                .unwrap_or_else(|| "no amp credentials".into());
            return PingResult {
                provider: "amp",
                account_id,
                ok,
                status: if ok { Some(200) } else { None },
                latency_ms: now_ms() - start,
                message,
            };
        }
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
        HeaderValue::from_str(&state.local_key.read().unwrap()).expect("key header"),
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
                    Provider::Anthropic
                        | Provider::Openai
                        | Provider::Xai
                        | Provider::Gemini
                        | Provider::Openrouter
                        | Provider::Amp
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

const CLAUDE_CODE_PASSTHROUGH_HEADERS: &[&str] = &[
    "accept",
    "x-app",
    "x-claude-code-session-id",
    "x-stainless-arch",
    "x-stainless-lang",
    "x-stainless-os",
    "x-stainless-package-version",
    "x-stainless-retry-count",
    "x-stainless-runtime",
    "x-stainless-runtime-version",
    "x-stainless-timeout",
    "anthropic-dangerous-direct-browser-access",
];

fn is_genuine_claude_code_request(
    format: ClientFormat,
    headers: &HeaderMap,
    body: &Value,
) -> bool {
    if format != ClientFormat::AnthropicMessages {
        return false;
    }

    for harness in headers.get_all("x-alexandria-harness").iter() {
        let Ok(harness) = harness.to_str() else {
            return false;
        };
        let harness = harness.trim().to_ascii_lowercase();
        if !matches!(harness.as_str(), "claude" | "claude-code") {
            return false;
        }
    }

    let claude_user_agent = headers
        .get("user-agent")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.to_ascii_lowercase().starts_with("claude-cli/"));
    let cli_app = headers
        .get("x-app")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("cli"));
    let has_session = headers
        .get("x-claude-code-session-id")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| !value.trim().is_empty());
    let has_billing_header = body["system"]
        .as_array()
        .and_then(|blocks| blocks.first())
        .and_then(|block| block["text"].as_str())
        .is_some_and(|text| text.starts_with("x-anthropic-billing-header:"));

    claude_user_agent && cli_app && has_session && has_billing_header
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
        name: active.generation_id.clone(),
        description: None,
        paused: false,
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
        path: None,
    }
}

async fn ensure_gemini_project(
    state: &AppState,
    account: &Account,
) -> Result<String, (StatusCode, String)> {
    if let Some(p) = account
        .account_meta
        .get("project_id")
        .and_then(|v| v.as_str())
    {
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
    let token = account.access_token.as_deref().ok_or_else(|| {
        (
            StatusCode::BAD_GATEWAY,
            "gemini account has no access token".into(),
        )
    })?;
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
        .map_err(|e| {
            (
                StatusCode::BAD_GATEWAY,
                format!("loadCodeAssist failed: {e}"),
            )
        })?;
    let load: Value = resp.json().await.unwrap_or(Value::Null);
    let extract = |v: &Value| -> Option<String> {
        for key in ["cloudaicompanionProject", "projectId", "project"] {
            match &v[key] {
                Value::String(s) if !s.is_empty() => return Some(s.clone()),
                obj if obj["id"].is_string() => return obj["id"].as_str().map(String::from),
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
    let onboard_url = format!("{GEMINI_CODE_ASSIST_BASE}/{GEMINI_CODE_ASSIST_VERSION}:onboardUser");
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
    excluded_accounts: &HashSet<String>,
    affinity_session: Option<&str>,
    client_headers: &HeaderMap,
) -> Result<UpstreamPlan, (StatusCode, String)> {
    use alex_core::translate;
    let client_stream = body_json["stream"].as_bool().unwrap_or(false);
    let genuine_claude_code =
        is_genuine_claude_code_request(format, client_headers, body_json);
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
            let (base, account, dario_guard, dario_capture) = if genuine_claude_code {
                let account = state
                    .vault
                    .account_for_excluding(provider, true, excluded_accounts)
                    .await
                    .map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?;
                (ANTHROPIC_BASE.to_string(), account, None, false)
            } else {
                let dario_active = state.dario.as_ref().and_then(|dario| dario.active());
                match (&state.dario, dario_active) {
                    (Some(dario), Some(active)) => {
                        let guard = dario.begin(&active.generation_id).ok_or_else(|| {
                            (
                                StatusCode::SERVICE_UNAVAILABLE,
                                "Dario became unavailable while routing the Anthropic request"
                                    .to_string(),
                            )
                        })?;
                        (
                            active.base_url.trim_end_matches('/').to_string(),
                            dario_account(&active),
                            Some(guard),
                            true,
                        )
                    }
                    (Some(_), None) => {
                        return Err((
                            StatusCode::SERVICE_UNAVAILABLE,
                            "Dario is configured but no healthy generation is available"
                                .to_string(),
                        ));
                    }
                    (None, None) => {
                        let account = state
                            .vault
                            .account_for_excluding(provider, true, excluded_accounts)
                            .await
                            .map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?;
                        (ANTHROPIC_BASE.to_string(), account, None, false)
                    }
                    (None, Some(_)) => unreachable!("Dario cannot be active when it is disabled"),
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
                extra_headers: if dario_capture {
                    vec![
                        ("x-dario-capture-id".into(), trace_id.into()),
                        ("x-dario-capture-model".into(), routed_model.into()),
                    ]
                } else {
                    vec![]
                },
                dario_guard,
            })
        }
        Provider::Openai => {
            // Serialize only the account-planning step for the same thread.
            // This closes the first-request race without blocking unrelated
            // sessions or holding a lock during the upstream model request.
            let _affinity_guard = match affinity_session.filter(|value| !value.is_empty()) {
                Some(session_id) => Some(
                    codex_affinity_lock(state, session_id)
                        .lock_owned()
                        .await,
                ),
                None => None,
            };
            let prefer_oauth = format != ClientFormat::OpenaiChat;
            let policy = state.vault.policy(Provider::Openai);
            let preferred = preferred_codex_account(state, affinity_session);
            let account = state
                .vault
                .account_for_excluding_preferred_mode(
                    provider,
                    prefer_oauth,
                    excluded_accounts,
                    preferred.as_deref(),
                    preferred.is_some() && !policy.allow_mid_thread_failover,
                )
                .await
                .map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?;
            let oauth = account.kind == "oauth";
            bind_codex_account(state, affinity_session, &account);
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
            let account = state
                .vault
                .account_for_excluding(provider, true, excluded_accounts)
                .await
                .map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?;
            let extra_headers = vec![
                ("x-grok-model-override".into(), routed_model.to_string()),
                ("x-grok-conv-id".into(), trace_id.to_string()),
            ];
            match format {
                ClientFormat::OpenaiChat => {
                    body_json["model"] = json!(routed_model);
                    let body =
                        serde_json::to_vec(body_json).unwrap_or_else(|_| original_body.to_vec());
                    Ok(UpstreamPlan {
                        url: format!("{XAI_BASE}/chat/completions"),
                        account,
                        body,
                        upstream_format: "openai-chat",
                        destream: false,
                        respond_as: None,
                        client_stream,
                        extra_headers,
                        dario_guard: None,
                    })
                }
                ClientFormat::AnthropicMessages => {
                    // pivot (anthropic) → chat completions for xAI/Grok upstream
                    let mut converted = translate::anthropic_to_openai_chat(body_json);
                    converted["model"] = json!(routed_model);
                    // buffer full upstream body then re-synth client dialect
                    converted["stream"] = json!(false);
                    let body = serde_json::to_vec(&converted)
                        .unwrap_or_else(|_| original_body.to_vec());
                    Ok(UpstreamPlan {
                        url: format!("{XAI_BASE}/chat/completions"),
                        account,
                        body,
                        upstream_format: "openai-chat",
                        destream: false,
                        respond_as: Some(RespondAs::Anthropic),
                        client_stream,
                        extra_headers,
                        dario_guard: None,
                    })
                }
                ClientFormat::OpenaiResponses | ClientFormat::GeminiGenerate => Err((
                    StatusCode::NOT_IMPLEMENTED,
                    "the xai/grok upstream speaks OpenAI chat completions; POST to /v1/chat/completions or /v1/messages"
                        .to_string(),
                )),
            }
        }
        Provider::Openrouter => {
            let account = state
                .vault
                .account_for_excluding(provider, false, excluded_accounts)
                .await
                .map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?;
            match format {
                ClientFormat::OpenaiChat => {
                    body_json["model"] = json!(routed_model);
                    let body =
                        serde_json::to_vec(body_json).unwrap_or_else(|_| original_body.to_vec());
                    Ok(UpstreamPlan {
                        url: format!("{OPENROUTER_BASE}/chat/completions"),
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
                ClientFormat::AnthropicMessages => {
                    let mut converted = translate::anthropic_to_openai_chat(body_json);
                    converted["model"] = json!(routed_model);
                    converted["stream"] = json!(false);
                    let body = serde_json::to_vec(&converted)
                        .unwrap_or_else(|_| original_body.to_vec());
                    Ok(UpstreamPlan {
                        url: format!("{OPENROUTER_BASE}/chat/completions"),
                        account,
                        body,
                        upstream_format: "openai-chat",
                        destream: false,
                        respond_as: Some(RespondAs::Anthropic),
                        client_stream,
                        extra_headers: vec![],
                        dario_guard: None,
                    })
                }
                ClientFormat::OpenaiResponses | ClientFormat::GeminiGenerate => Err((
                    StatusCode::NOT_IMPLEMENTED,
                    "the OpenRouter upstream speaks OpenAI chat completions; POST to /v1/chat/completions or /v1/messages"
                        .to_string(),
                )),
            }
        }
        Provider::Amp => Err((
            StatusCode::NOT_IMPLEMENTED,
            "amp is wrap/billing-only (no /v1 upstream). Use `alex wrap amp` for harness capture and `alex limits` for credits.".into(),
        )),
        Provider::Gemini => {
            // Prefer an AI Studio API key over the OAuth/Code-Assist path.
            let account = state
                .vault
                .account_for_excluding(provider, false, excluded_accounts)
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
    use super::{
        openai_responses_user_history_signature, session_from_metadata, trace_extras,
        trace_harness, trace_reasoning_fields, transcript_assistant_blocks, transcript_turn,
        truncate_chars,
    };
    use axum::http::{HeaderMap, HeaderValue};
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
    fn explicit_harness_header_wins_over_sdk_user_agent() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "user-agent",
            HeaderValue::from_static("Anthropic/JS 0.91.1"),
        );
        assert_eq!(trace_harness(&headers).as_deref(), Some("Anthropic/JS 0.91.1"));
        headers.insert(
            "x-alexandria-harness",
            HeaderValue::from_static("pi"),
        );
        assert_eq!(trace_harness(&headers).as_deref(), Some("pi"));
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
        assert_eq!(trace_reasoning_fields(&anthropic), (None, Some(4096)));
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
        assert_eq!(
            trace_reasoning_fields(&responses),
            (Some("high".into()), None)
        );
        assert_eq!(e["max_tokens"], 200);
        assert_eq!(e["message_count"], 2);
        assert_eq!(e["system_chars"], 3);
        assert_eq!(e["thinking_budget"], serde_json::Value::Null);
    }

    #[test]
    fn truncates_on_char_boundaries() {
        assert_eq!(truncate_chars("abc".into(), 8000), "abc");
        assert_eq!(
            truncate_chars("héllo".repeat(2000), 8000).chars().count(),
            8000
        );
    }

    #[test]
    fn transcript_turn_missing_bodies_are_null() {
        let row = json!({
            "id": "t1", "ts_request_ms": 1, "ts_response_ms": 2,
            "routed_model": "m", "status": 200,
            "input_tokens": 10, "output_tokens": 5, "cost_usd": 0.01, "error": null,
            "reasoning_effort": "minimal", "thinking_budget": null,
            "req_body_path": "/nonexistent/x.gz", "resp_body_path": null,
            "client_format": "anthropic", "upstream_format": "anthropic",
        });
        let turn = transcript_turn(&row);
        assert_eq!(turn["trace_id"], "t1");
        assert_eq!(turn["user"], serde_json::Value::Null);
        assert_eq!(turn["assistant"], serde_json::Value::Null);
        assert_eq!(turn["model"], "m");
        assert_eq!(turn["reasoning_effort"], "minimal");
        assert_eq!(turn["thinking_budget"], serde_json::Value::Null);
    }

    #[test]
    fn transcript_assistant_blocks_preserve_text_tool_text_order() {
        let response = json!({
            "_alexandria": {"assistant_blocks": [
                {"type": "text", "text": "Listing the workspace."},
                {"type": "tool_call", "name": "Shell", "arguments": "{\"command\":\"ls\"}"},
                {"type": "text", "text": "Here are the files."},
            ]}
        });
        let blocks = transcript_assistant_blocks(&response.to_string());
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0]["type"], "text");
        assert_eq!(blocks[0]["text"], "Listing the workspace.");
        assert_eq!(blocks[1]["type"], "tool_call");
        assert_eq!(blocks[1]["name"], "Shell");
        assert_eq!(blocks[2]["type"], "text");
        assert_eq!(blocks[2]["text"], "Here are the files.");
    }

    #[test]
    fn responses_user_history_signature_changes_only_for_a_new_user_message() {
        let request = |extra: Option<&str>| {
            let mut input = vec![
                json!({"type": "message", "role": "user", "content": [{"type": "input_text", "text": "context"}]}),
                json!({"type": "message", "role": "user", "content": [{"type": "input_text", "text": "help me"}]}),
                json!({"type": "custom_tool_call_output", "call_id": "call-1", "output": "done"}),
            ];
            if let Some(text) = extra {
                input.push(json!({"type": "message", "role": "user", "content": [{"type": "input_text", "text": text}]}));
            }
            json!({"input": input})
        };
        let first = openai_responses_user_history_signature(&request(None)).unwrap();
        let tool_loop = openai_responses_user_history_signature(&request(None)).unwrap();
        let next = openai_responses_user_history_signature(&request(Some("again"))).unwrap();
        assert_eq!(first, tool_loop);
        assert_ne!(first, next);
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
    genuine_claude_code: bool,
) -> Result<reqwest::header::HeaderMap, (StatusCode, String)> {
    let mut h = reqwest::header::HeaderMap::new();
    h.insert("content-type", HeaderValue::from_static("application/json"));
    h.insert("accept", HeaderValue::from_static("*/*"));
    h.insert("accept-encoding", HeaderValue::from_static("identity"));
    if account.provider != Provider::Openrouter {
        if let Some(ua) = client_headers.get("user-agent") {
            h.insert("user-agent", ua.clone());
        }
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
        (Provider::Openrouter, _) => {
            h.extend(openrouter_auth_headers(account)?);
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
                HeaderValue::from_str(key).map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?,
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
        (Provider::Amp, _) => {
            return Err((
                StatusCode::NOT_IMPLEMENTED,
                "amp is wrap/billing-only".to_string(),
            ));
        }
    }
    if genuine_claude_code && account.provider == Provider::Anthropic {
        for name in CLAUDE_CODE_PASSTHROUGH_HEADERS {
            if let Some(value) = client_headers.get(*name) {
                h.insert(*name, value.clone());
            }
        }
    }
    Ok(h)
}

/// OpenRouter credentials and attribution come only from the selected vault
/// account. This deliberately has no access to inbound client headers.
fn openrouter_auth_headers(
    account: &Account,
) -> Result<reqwest::header::HeaderMap, (StatusCode, String)> {
    let key = account.api_key.as_deref().ok_or((
        StatusCode::BAD_GATEWAY,
        "openrouter account has no api key".to_string(),
    ))?;
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        "authorization",
        HeaderValue::from_str(&format!("Bearer {key}"))
            .map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?,
    );
    for (meta_key, header_name) in [
        ("http_referer", "http-referer"),
        ("x_title", "x-title"),
    ] {
        if let Some(value) = account.account_meta.get(meta_key).and_then(Value::as_str) {
            headers.insert(
                header_name,
                HeaderValue::from_str(value)
                    .map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?,
            );
        }
    }
    Ok(headers)
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
        kind: row["kind"].as_str().unwrap_or("run").to_string(),
        label: row["label"].as_str().map(String::from),
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
    let kind = body["kind"].as_str().unwrap_or("run");
    if kind != "run" && kind != "harness" && kind != "wrap" {
        return error_response(
            StatusCode::BAD_REQUEST,
            "'kind' must be 'run', 'harness', or 'wrap'",
        );
    }
    let run_id = body["run_id"].as_str().map(String::from);
    let label = body["label"].as_str().map(String::from);
    if (kind == "harness" || kind == "wrap") && label.as_deref().unwrap_or("").trim().is_empty() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "'label' is required for harness and wrap keys",
        );
    }
    let created_ms = now_ms();
    let expires_ms = if kind == "harness" || kind == "wrap" {
        None
    } else {
        let ttl_seconds = body["ttl_seconds"]
            .as_i64()
            .unwrap_or(RUN_KEY_DEFAULT_TTL_S);
        if ttl_seconds <= 0 {
            return error_response(StatusCode::BAD_REQUEST, "'ttl_seconds' must be positive");
        }
        Some(created_ms + ttl_seconds.min(RUN_KEY_MAX_TTL_S) * 1000)
    };
    let key = generate_run_key();
    let key_hash = key_hash_hex(&key);
    let id = format!("rk-{}", &key_hash[..8]);
    let tags_json = tags
        .as_ref()
        .filter(|o| !o.is_empty())
        .and_then(|o| serde_json::to_string(o).ok());
    match state.store.insert_run_key(
        &id,
        &key_hash,
        kind,
        run_id.as_deref(),
        tags_json.as_deref(),
        label.as_deref(),
        created_ms,
        expires_ms,
    ) {
        Ok(()) => {
            let exports = if kind == "wrap" {
                format!(
                    "export ALEXANDRIA_TRACE_URL={}\nexport ALEXANDRIA_TRACE_KEY={}\n",
                    state.base_url.trim_end_matches('/'),
                    key
                )
            } else {
                connect_payload(&state.base_url, &key).1
            };
            (
                StatusCode::CREATED,
                axum::Json(json!({
                    "id": id,
                    "key": key,
                    "kind": kind,
                    "run_id": run_id,
                    "label": label,
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
    let all = q
        .get("all")
        .map(|v| v == "1" || v == "true")
        .unwrap_or(false);
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

const MAX_INGEST_BODY_BYTES: usize = 16 * 1024 * 1024;

fn valid_ingest_trace_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 200
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn decode_ingest_body(encoded: Option<&str>, field: &str) -> Result<Option<Vec<u8>>, String> {
    use base64::Engine;

    let Some(encoded) = encoded else {
        return Ok(None);
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|e| format!("'{field}' is not valid base64: {e}"))?;
    if bytes.len() > MAX_INGEST_BODY_BYTES {
        return Err(format!(
            "'{field}' exceeds the {} byte limit",
            MAX_INGEST_BODY_BYTES
        ));
    }
    Ok(Some(bytes))
}

fn authenticate_trace_ingest(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<(String, Option<CachedRunKey>, bool), Response> {
    let Some(key) = client_key(headers) else {
        return Err(error_response(
            StatusCode::UNAUTHORIZED,
            "trace ingest requires an Alexandria wrap key",
        ));
    };
    if state.local_key.read().map(|local| key == *local).unwrap_or(false) {
        return Ok((key_fingerprint(&key), None, true));
    }
    let key_hash = key_hash_hex(&key);
    let Some(entry) = run_key_entry(state, &key_hash) else {
        return Err(error_response(
            StatusCode::UNAUTHORIZED,
            "wrap key expired, revoked, or unknown",
        ));
    };
    if entry.kind != "wrap" {
        return Err(error_response(
            StatusCode::FORBIDDEN,
            "trace ingest requires a key minted with --kind wrap",
        ));
    }
    Ok((key_hash.chars().take(16).collect(), Some(entry), false))
}

fn authenticate_harness_event(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<(String, CachedRunKey), Response> {
    let Some(key) = client_key(headers) else {
        return Err(error_response(
            StatusCode::UNAUTHORIZED,
            "harness events require an Alexandria harness key",
        ));
    };
    let key_hash = key_hash_hex(&key);
    let Some(entry) = run_key_entry(state, &key_hash) else {
        return Err(error_response(
            StatusCode::UNAUTHORIZED,
            "harness key expired, revoked, or unknown",
        ));
    };
    if entry.kind != "harness" {
        return Err(error_response(
            StatusCode::FORBIDDEN,
            "harness events require a key minted with --kind harness",
        ));
    }
    if entry.label.as_deref().unwrap_or("").trim().is_empty() {
        return Err(error_response(
            StatusCode::FORBIDDEN,
            "harness key is missing its harness label",
        ));
    }
    Ok((key_hash, entry))
}

async fn harness_event(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::Json(event): axum::Json<Value>,
) -> Response {
    let (key_hash, key) = match authenticate_harness_event(&state, &headers) {
        Ok(auth) => auth,
        Err(response) => return response,
    };
    if let Err(error) = state.store.touch_run_key(&key_hash, now_ms()) {
        tracing::warn!("failed to touch harness key: {error}");
    }
    let harness = key.label.as_deref().unwrap_or_default();
    let event_name = event["hook_event_name"]
        .as_str()
        .or_else(|| event["hookEventName"].as_str())
        .unwrap_or_default();
    if !matches!(
        event_name,
        "SessionStart" | "SubagentStart" | "SubagentStop" | "Stop"
    ) {
        return error_response(StatusCode::BAD_REQUEST, "unsupported harness hook event");
    }
    for field in ["session_id", "turn_id", "agent_id"] {
        if let Some(id) = event[field].as_str() {
            if !valid_ingest_trace_id(id) {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    &format!("'{field}' must be a safe 1-200 character identifier"),
                );
            }
        }
    }
    if matches!(event_name, "SubagentStart" | "SubagentStop")
        && (event["session_id"].as_str().is_none() || event["agent_id"].as_str().is_none())
    {
        return error_response(
            StatusCode::BAD_REQUEST,
            "subagent events require session_id and agent_id",
        );
    }
    match state.store.record_harness_event(harness, &event, now_ms()) {
        Ok(lineage_updated) => axum::Json(json!({
            "ok": true,
            "harness": harness,
            "lineage_updated": lineage_updated,
        }))
        .into_response(),
        Err(error) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
    }
}

fn touch_trace_ingest_key(state: &AppState, key: Option<&CachedRunKey>, headers: &HeaderMap) {
    if key.is_none() {
        return;
    }
    let Some(raw_key) = client_key(headers) else {
        return;
    };
    let key_hash = key_hash_hex(&raw_key);
    if let Err(error) = state.store.touch_run_key(&key_hash, now_ms()) {
        tracing::warn!("failed to touch wrap key: {error}");
    }
}

async fn traces_ingest_status(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    match authenticate_trace_ingest(&state, &headers) {
        Ok((_, key, _)) => {
            touch_trace_ingest_key(&state, key.as_ref(), &headers);
            axum::Json(json!({
                "ok": true,
                "capability": "trace-ingest-v1",
                "max_body_bytes": MAX_INGEST_BODY_BYTES,
            }))
            .into_response()
        }
        Err(response) => response,
    }
}

async fn traces_ingest(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::Json(mut payload): axum::Json<TraceIngestPayload>,
) -> Response {
    let (fingerprint, key, local_admin) = match authenticate_trace_ingest(&state, &headers) {
        Ok(auth) => auth,
        Err(response) => return response,
    };
    touch_trace_ingest_key(&state, key.as_ref(), &headers);
    if !valid_ingest_trace_id(&payload.trace.id) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "trace.id must be 1-200 characters using only letters, numbers, '.', '_', or '-'",
        );
    }
    if payload.trace.ts_request_ms <= 0 {
        return error_response(
            StatusCode::BAD_REQUEST,
            "trace.ts_request_ms must be positive",
        );
    }
    let request = match decode_ingest_body(payload.request_body_b64.as_deref(), "request_body_b64")
    {
        Ok(body) => body,
        Err(error) => return error_response(StatusCode::BAD_REQUEST, &error),
    };
    let upstream_request = match decode_ingest_body(
        payload.upstream_request_body_b64.as_deref(),
        "upstream_request_body_b64",
    ) {
        Ok(body) => body,
        Err(error) => return error_response(StatusCode::BAD_REQUEST, &error),
    };
    let response =
        match decode_ingest_body(payload.response_body_b64.as_deref(), "response_body_b64") {
            Ok(body) => body,
            Err(error) => return error_response(StatusCode::BAD_REQUEST, &error),
        };

    // Keep ownership check, body replacement, and row upsert together. This
    // prevents two wrap credentials racing an unused trace id and replacing
    // one another's bodies between the check and insert.
    let _ingest_guard = state.trace_ingest_lock.lock().await;
    let existing = match state.store.get_trace(&payload.trace.id) {
        Ok(row) => row,
        Err(error) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
    };
    if let Some(row) = &existing {
        let owner = row["key_fingerprint"].as_str();
        if !local_admin && owner != Some(fingerprint.as_str()) {
            return error_response(
                StatusCode::CONFLICT,
                "trace id already belongs to another credential",
            );
        }
    }

    payload.trace.run_id = payload
        .trace
        .run_id
        .or_else(|| key.as_ref().and_then(|entry| entry.run_id.clone()));
    payload.trace.tags = merge_run_key_tags(
        key.as_ref().and_then(|entry| entry.tags_json.as_deref()),
        payload.trace.tags.as_deref(),
    );
    payload.trace.key_fingerprint = Some(fingerprint);
    payload.trace.client_ip = None;
    payload.trace.req_body_path = existing
        .as_ref()
        .and_then(|row| row["req_body_path"].as_str().map(String::from));
    payload.trace.upstream_req_body_path = existing
        .as_ref()
        .and_then(|row| row["upstream_req_body_path"].as_str().map(String::from));
    payload.trace.resp_body_path = existing
        .as_ref()
        .and_then(|row| row["resp_body_path"].as_str().map(String::from));

    if let Some(body) = request {
        match state
            .store
            .write_body(&payload.trace.id, "request.json", &body)
        {
            Ok(path) => payload.trace.req_body_path = Some(path),
            Err(error) => {
                return error_response(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string())
            }
        }
    }
    if let Some(body) = upstream_request {
        match state
            .store
            .write_body(&payload.trace.id, "upstream-request.json", &body)
        {
            Ok(path) => payload.trace.upstream_req_body_path = Some(path),
            Err(error) => {
                return error_response(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string())
            }
        }
    }
    if let Some(body) = response {
        match state
            .store
            .write_body(&payload.trace.id, "response.body", &body)
        {
            Ok(path) => payload.trace.resp_body_path = Some(path),
            Err(error) => {
                return error_response(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string())
            }
        }
    }
    if let Err(error) = state.store.insert_trace(&payload.trace) {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string());
    }
    let outcome = if existing.is_some() {
        "updated"
    } else {
        "inserted"
    };
    (
        if existing.is_some() {
            StatusCode::OK
        } else {
            StatusCode::CREATED
        },
        axum::Json(json!({"id": payload.trace.id, "outcome": outcome})),
    )
        .into_response()
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

fn trace_harness(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-alexandria-harness")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(String::from)
        .or_else(|| {
            headers
                .get("user-agent")
                .and_then(|value| value.to_str().ok())
                .map(String::from)
        })
}

const METADATA_HEADERS: &[(&str, &str)] = &[
    ("x-alexandria-harness", "harness"),
    ("x-alexandria-harness-version", "harness_version"),
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

fn retryable_failover_status(status: reqwest::StatusCode) -> bool {
    status == reqwest::StatusCode::TOO_MANY_REQUESTS
        || status == reqwest::StatusCode::UNAUTHORIZED
        || status == reqwest::StatusCode::FORBIDDEN
        || status.is_server_error()
}

fn retry_failover_allowed(
    provider: Provider,
    thread_was_affined: bool,
    allow_mid_thread_failover: bool,
) -> bool {
    provider != Provider::Openai || !thread_was_affined || allow_mid_thread_failover
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
        Some(k) if state.local_key.read().map(|local| k == *local).unwrap_or(false) => key_fingerprint(&k),
        Some(k) => {
            let key_hash = key_hash_hex(&k);
            match run_key_entry(&state, &key_hash) {
                Some(entry) if entry.kind == "wrap" => {
                    return error_response(
                        StatusCode::FORBIDDEN,
                        "wrap keys may only post to /traces/ingest",
                    )
                }
                Some(entry) => {
                    if let Err(e) = state.store.touch_run_key(&key_hash, now_ms()) {
                        tracing::warn!("failed to touch run key: {e}");
                    }
                    run_key = Some(entry);
                    key_hash.chars().take(16).collect()
                }
                None if k.starts_with(RUN_KEY_PREFIX) => {
                    return error_response(StatusCode::UNAUTHORIZED, "run key expired or revoked")
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

    let trace_id = uuid::Uuid::new_v4().to_string();
    let mut trace = TraceRecord {
        id: trace_id.clone(),
        ts_request_ms: now_ms(),
        method: Some("POST".into()),
        path: Some(path.into()),
        client_format: Some(format.as_str().into()),
        harness: trace_harness(&headers),
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
    let (reasoning_effort, thinking_budget) = trace_reasoning_fields(&body_json);
    trace.reasoning_effort = reasoning_effort;
    trace.thinking_budget = thinking_budget;
    let genuine_claude_code = is_genuine_claude_code_request(format, &headers, &body_json);

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
        .or_else(|| {
            headers
                .get("x-claude-code-session-id")
                .and_then(|v| v.to_str().ok())
                .map(String::from)
        })
        .or_else(|| session_from_metadata(&body_json))
        .or_else(|| {
            body_json
                .get("prompt_cache_key")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(String::from)
        })
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
    let in_flight = InFlight::new(
        &state,
        routed_model.clone(),
        trace.session_id.clone(),
        trace.harness.clone(),
    );

    // Codex emits SubagentStart before the child's first request. Resolve that
    // child to the lineage root so every descendant prefers the same upstream
    // account while retaining its own session id in traces.
    let affinity_session_id = trace.session_id.as_ref().map(|session_id| {
        if trace.harness.as_deref() == Some("codex") {
            state
                .store
                .session_lineage_root("codex", session_id)
                .unwrap_or_else(|error| {
                    tracing::warn!(%error, %session_id, "could not resolve Codex session lineage");
                    session_id.clone()
                })
        } else {
            session_id.clone()
        }
    });

    // Capture this before planning binds a brand-new session. The policy only
    // suppresses failover for a thread that arrived with an existing affinity;
    // an unaffined first request may still fail over and then pin its winner.
    let codex_thread_was_affined = provider == Provider::Openai
        && preferred_codex_account(&state, affinity_session_id.as_deref()).is_some();
    let allow_mid_thread_failover = state
        .vault
        .policy(Provider::Openai)
        .allow_mid_thread_failover;

    let original_body_json = body_json.clone();
    let mut attempted_accounts = HashSet::new();
    let mut plan = match plan_upstream(
        &state,
        format,
        provider,
        &routed_model,
        &mut body_json,
        &body,
        &trace_id,
        &attempted_accounts,
        affinity_session_id.as_deref(),
        &headers,
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
    bind_trace_account(&state.store, &mut trace, &plan.account);
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
        genuine_claude_code,
        "proxying request"
    );

    let mut account = plan.account.clone();
    let mut upstream_resp = None;
    'accounts: loop {
        if !attempted_accounts.insert(account.id.clone()) {
            tracing::error!(account = %account.id, "refusing to retry an already-attempted account");
            break;
        }

        let mut forced_oauth_refresh = false;
        loop {
            let mut up_headers =
                match upstream_headers(&account, &headers, genuine_claude_code) {
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

            if account.kind == "oauth" {
                if let Some(snapshot) = routing_limits_from_headers(account.provider, resp.headers()) {
                    if let Err(error) = state
                        .vault
                        .record_routing_limits(&account.id, snapshot)
                        .await
                    {
                        tracing::warn!(account = %account.id, %error, "could not persist routing limit snapshot");
                    }
                }
            }
            if resp.status() == reqwest::StatusCode::UNAUTHORIZED
                && account.kind == "oauth"
                && !forced_oauth_refresh
            {
                tracing::warn!(
                    account = %account.id,
                    "upstream returned 401 for oauth account; forcing token refresh and retrying"
                );
                forced_oauth_refresh = true;
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

            if retryable_failover_status(resp.status()) && account.kind != "dario" {
                let retry_after_s = resp
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<i64>().ok())
                    .unwrap_or(60)
                    .clamp(1, 3600);
                tracing::warn!(account = %account.id, retry_after_s, status = %resp.status(), "upstream failed; cooling account down");
                if let Err(e) = state
                    .vault
                    .mark_cooldown(&account.id, now_ms() + retry_after_s * 1000)
                    .await
                {
                    tracing::warn!("failed to mark cooldown: {e}");
                }

                if !retry_failover_allowed(
                    provider,
                    codex_thread_was_affined,
                    allow_mid_thread_failover,
                ) {
                    tracing::warn!(
                        account = %account.id,
                        status = %resp.status(),
                        "returning retryable Codex error without switching an affined thread"
                    );
                    upstream_resp = Some(resp);
                    break 'accounts;
                }

                let mut retry_body_json = original_body_json.clone();
                match plan_upstream(
                    &state,
                    format,
                    provider,
                    &routed_model,
                    &mut retry_body_json,
                    &body,
                    &trace_id,
                    &attempted_accounts,
                    affinity_session_id.as_deref(),
                    &headers,
                )
                .await
                {
                    Ok(next_plan) if !attempted_accounts.contains(&next_plan.account.id) => {
                        plan = next_plan;
                        account = plan.account.clone();
                        bind_trace_account(&state.store, &mut trace, &account);
                        trace.upstream_format = Some(plan.upstream_format.into());
                        trace.billing_bucket = Some(
                            if account.kind == "oauth" || account.kind == "dario" {
                                "subscription"
                            } else {
                                "api"
                            }
                            .into(),
                        );
                        tracing::warn!(
                            account = %account.id,
                            attempted = attempted_accounts.len(),
                            "retrying with failover account"
                        );
                        continue 'accounts;
                    }
                    Ok(next_plan) => {
                        tracing::error!(
                            account = %next_plan.account.id,
                            "account selector returned an already-attempted failover account"
                        );
                    }
                    Err((status, msg)) => tracing::warn!(
                        status = %status,
                        error = %msg,
                        attempted = attempted_accounts.len(),
                        "no untried failover account available"
                    ),
                }
            }

            upstream_resp = Some(resp);
            break 'accounts;
        }
    }
    let upstream_resp = upstream_resp.expect("upstream response after retry loop");
    bind_trace_account(&state.store, &mut trace, &account);

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
        } else if plan.upstream_format == "openai-chat" {
            if is_sse || looks_sse {
                translate::parse_openai_chat_sse_final(&text)
            } else {
                serde_json::from_str(&text).ok()
            }
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
            (RespondAs::OpenaiResponses, "anthropic") => {
                translate::anthropic_response_to_openai_responses(&upstream_final, &requested_model)
            }
            (RespondAs::Gemini, "anthropic") => {
                translate::anthropic_response_to_gemini(&upstream_final, &requested_model)
            }
            (RespondAs::Anthropic, "openai-chat") => {
                translate::openai_chat_response_to_anthropic(&upstream_final, &requested_model)
            }
            (RespondAs::OpenaiChat, "openai-chat") => upstream_final.clone(),
            (RespondAs::OpenaiResponses, "openai-chat") => {
                translate::anthropic_response_to_openai_responses(
                    &translate::openai_chat_response_to_anthropic(
                        &upstream_final,
                        &requested_model,
                    ),
                    &requested_model,
                )
            }
            (RespondAs::Gemini, "openai-chat") => translate::anthropic_response_to_gemini(
                &translate::openai_chat_response_to_anthropic(&upstream_final, &requested_model),
                &requested_model,
            ),
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
        let mut sse_error_observer = is_sse.then(|| SseErrorObserver::new(plan.upstream_format));
        let stream_error = forward_upstream_stream(
            &mut upstream_stream,
            &tx,
            state2.upstream_stream_idle_timeout,
            |chunk| {
                buf.extend_from_slice(chunk);
                if let Some(observer) = sse_error_observer.as_mut() {
                    observer.observe(chunk);
                }
            },
        )
        .await;
        drop(tx);
        trace.ts_response_ms = Some(now_ms());
        if trace.error.is_none() {
            trace.error = sse_error_observer
                .as_mut()
                .and_then(|observer| {
                    observer.finish();
                    observer.error()
                })
                .or(stream_error);
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

/// Forward an upstream response while enforcing a maximum quiet period between
/// chunks.  A disconnected downstream client deliberately stops the upstream:
/// there is no recipient for further output, and retaining a detached drain can
/// otherwise hold an in-flight guard forever.
async fn forward_upstream_stream<S, E, F>(
    upstream_stream: &mut S,
    tx: &tokio::sync::mpsc::Sender<Result<Bytes, std::io::Error>>,
    idle_timeout: Duration,
    mut observe: F,
) -> Option<String>
where
    S: futures_util::Stream<Item = Result<Bytes, E>> + Unpin,
    E: std::fmt::Display,
    F: FnMut(&Bytes),
{
    loop {
        match tokio::time::timeout(idle_timeout, upstream_stream.next()).await {
            Ok(Some(Ok(chunk))) => {
                observe(&chunk);
                if tx.send(Ok(chunk)).await.is_err() {
                    return Some("client disconnected before upstream stream completed".into());
                }
            }
            Ok(Some(Err(error))) => {
                let message = error.to_string();
                let _ = tx
                    .send(Err(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        message.clone(),
                    )))
                    .await;
                return Some(format!("upstream stream error: {message}"));
            }
            Ok(None) => return None,
            Err(_) => {
                return Some(format!(
                    "upstream stream idle timeout after {} seconds",
                    idle_timeout.as_secs()
                ));
            }
        }
    }
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct UpstreamSseError {
    kind: String,
    message: String,
}

impl UpstreamSseError {
    fn trace_message(&self) -> String {
        format!("upstream stream error: {}: {}", self.kind, self.message)
    }
}

/// Passively observes one SSE stream. It retains only the current SSE event so
/// an event split across transport chunks can still be decoded; it never holds
/// back, changes, or coalesces the bytes sent to the client.
struct SseErrorObserver {
    upstream_format: &'static str,
    pending_line: Vec<u8>,
    event_name: Option<String>,
    event_data: Vec<String>,
    error: Option<UpstreamSseError>,
}

impl SseErrorObserver {
    fn new(upstream_format: &'static str) -> Self {
        Self {
            upstream_format,
            pending_line: Vec::new(),
            event_name: None,
            event_data: Vec::new(),
            error: None,
        }
    }

    fn observe(&mut self, chunk: &[u8]) {
        self.pending_line.extend_from_slice(chunk);
        let mut consumed = 0;
        for index in 0..self.pending_line.len() {
            if self.pending_line[index] != b'\n' {
                continue;
            }
            let line = String::from_utf8_lossy(&self.pending_line[consumed..index]).into_owned();
            self.observe_line(line.strip_suffix('\r').unwrap_or(&line));
            consumed = index + 1;
        }
        if consumed > 0 {
            self.pending_line.drain(..consumed);
        }
    }

    fn finish(&mut self) {
        if !self.pending_line.is_empty() {
            let line = String::from_utf8_lossy(&self.pending_line).into_owned();
            self.observe_line(line.strip_suffix('\r').unwrap_or(&line));
            self.pending_line.clear();
        }
        self.finish_event();
    }

    fn error(&self) -> Option<String> {
        self.error.as_ref().map(UpstreamSseError::trace_message)
    }

    fn observe_line(&mut self, line: &str) {
        if line.is_empty() {
            self.finish_event();
            return;
        }
        if line.starts_with(':') {
            return;
        }
        let Some((field, value)) = line.split_once(':') else {
            return;
        };
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "event" => self.event_name = Some(value.to_string()),
            "data" => self.event_data.push(value.to_string()),
            _ => {}
        }
    }

    fn finish_event(&mut self) {
        if self.error.is_none() && !self.event_data.is_empty() && self.event_may_be_an_error() {
            let data = self.event_data.join("\n");
            if let Ok(value) = serde_json::from_str::<Value>(&data) {
                self.error = upstream_sse_error(
                    self.upstream_format,
                    self.event_name.as_deref(),
                    &value,
                );
            }
        }
        self.event_name = None;
        self.event_data.clear();
    }

    /// Cheap pre-filter so we don't JSON-parse every content delta of every
    /// stream just to learn it isn't an error. Every arm of `upstream_sse_error`
    /// needs one of these markers, so anything without them cannot match. A
    /// false positive here costs one parse and still yields no error.
    fn event_may_be_an_error(&self) -> bool {
        matches!(
            self.event_name.as_deref(),
            Some("error") | Some("response.failed")
        ) || self
            .event_data
            .iter()
            .any(|line| line.contains("error") || line.contains("failed"))
    }
}

fn error_details(error: &Value, fallback_kind: &str) -> UpstreamSseError {
    let kind = error["type"]
        .as_str()
        .or_else(|| error["code"].as_str())
        .or_else(|| error["status"].as_str())
        .unwrap_or(fallback_kind)
        .to_string();
    let message = error["message"]
        .as_str()
        .unwrap_or("upstream returned an error event")
        .to_string();
    UpstreamSseError { kind, message }
}

fn upstream_sse_error(
    upstream_format: &str,
    event_name: Option<&str>,
    value: &Value,
) -> Option<UpstreamSseError> {
    let event_is_error = event_name == Some("error");
    match upstream_format {
        // Anthropic's documented stream error is `event: error` with a
        // `{type: "error", error: {type, message}}` payload.
        "anthropic" if event_is_error || value["type"] == "error" => {
            Some(error_details(&value["error"], "error"))
        }
        // OpenAI chat streams can carry a normal API error object as an SSE
        // frame. Responses additionally exposes the terminal
        // `response.failed` event used by its stream reassembler above.
        "openai-chat"
            if event_is_error || value["type"] == "error" || value["error"].is_object() =>
        {
            let error = if value["error"].is_object() {
                &value["error"]
            } else {
                value
            };
            Some(error_details(error, "error"))
        }
        "openai-responses"
            if event_is_error
                || value["type"] == "error"
                || event_name == Some("response.failed")
                || value["type"] == "response.failed" =>
        {
            let error = if value["response"]["error"].is_object() {
                &value["response"]["error"]
            } else if value["error"].is_object() {
                &value["error"]
            } else {
                value
            };
            Some(error_details(error, "response_failed"))
        }
        // Gemini's stream reassembler accepts both direct and code-assist
        // `response`-wrapped frames. Google error frames use the same shape.
        "gemini"
            if event_is_error
                || value["error"].is_object()
                || value["response"]["error"].is_object() =>
        {
            let error = if value["response"]["error"].is_object() {
                &value["response"]["error"]
            } else if value["error"].is_object() {
                &value["error"]
            } else {
                value
            };
            Some(error_details(error, "error"))
        }
        _ => None,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tmpdir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "alex-proxy-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn test_state(name: &str) -> Arc<AppState> {
        test_state_with_dario(name, None)
    }

    fn test_state_with_dario(
        name: &str,
        dario: Option<Arc<dyn DarioRouter>>,
    ) -> Arc<AppState> {
        let dir = tmpdir(name);
        let store = Arc::new(Store::open(dir.join("store")).unwrap());
        let vault = Arc::new(Vault::open(dir.join("vault")).unwrap());
        build_state(
            "alx-local".into(),
            vault,
            store,
            dario,
            "http://127.0.0.1:4100".into(),
            Duration::from_secs(15 * 60),
        )
    }

    #[tokio::test]
    async fn health_reports_in_flight_age_model_session_and_harness() {
        let state = test_state("in-flight-registry");
        let _guard = InFlight::new(
            &state,
            "gpt-5.5".into(),
            Some("session-123".into()),
            Some("codex".into()),
        );
        state
            .in_flight_requests
            .lock()
            .unwrap()
            .values_mut()
            .next()
            .unwrap()
            .started_ms = now_ms() - 61_000;

        let response = health(State(state.clone())).await.into_response();
        let (status, body) = response_json(response).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["in_flight"], 1);
        assert!(body["in_flight_requests"][0]["age_s"].as_i64().unwrap() >= 61);
        assert_eq!(body["in_flight_requests"][0]["model"], "gpt-5.5");
        assert_eq!(body["in_flight_requests"][0]["session_id"], "session-123");
        assert_eq!(body["in_flight_requests"][0]["harness"], "codex");
    }

    #[tokio::test(start_paused = true)]
    async fn stalled_upstream_releases_the_in_flight_guard_after_idle_timeout() {
        let state = test_state("in-flight-idle-timeout");
        let guard = InFlight::new(&state, "gpt-5.5".into(), None, None);
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        let task = tokio::spawn(async move {
            let _in_flight = guard;
            let mut stream = futures_util::stream::pending::<Result<Bytes, std::io::Error>>();
            forward_upstream_stream(&mut stream, &tx, Duration::from_secs(5), |_| {}).await
        });

        tokio::task::yield_now().await;
        assert_eq!(state.in_flight.load(std::sync::atomic::Ordering::SeqCst), 1);
        tokio::time::advance(Duration::from_secs(5)).await;
        assert_eq!(
            task.await.unwrap(),
            Some("upstream stream idle timeout after 5 seconds".into())
        );
        assert_eq!(state.in_flight.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert!(in_flight_requests(&state).is_empty());
    }

    #[tokio::test]
    async fn in_flight_guard_is_released_when_the_stream_task_panics() {
        let state = test_state("in-flight-panic");
        let guard = InFlight::new(&state, "gpt-5.5".into(), None, None);
        let task = tokio::spawn(async move {
            let _in_flight = guard;
            panic!("fake streaming task panic");
        });

        assert!(task.await.is_err());
        assert_eq!(state.in_flight.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert!(in_flight_requests(&state).is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn slow_but_progressing_upstream_does_not_hit_idle_timeout() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        let mut stream = futures_util::stream::unfold(0, |index| async move {
            if index == 2 {
                None
            } else {
                tokio::time::sleep(Duration::from_secs(4)).await;
                Some((Ok::<_, std::io::Error>(Bytes::from_static(b"token")), index + 1))
            }
        })
        .boxed();
        let task = tokio::spawn(async move {
            forward_upstream_stream(&mut stream, &tx, Duration::from_secs(5), |_| {}).await
        });

        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(4)).await;
        tokio::task::yield_now().await;
        assert_eq!(rx.recv().await.unwrap().unwrap(), Bytes::from_static(b"token"));
        tokio::time::advance(Duration::from_secs(4)).await;
        tokio::task::yield_now().await;
        assert_eq!(rx.recv().await.unwrap().unwrap(), Bytes::from_static(b"token"));
        assert_eq!(task.await.unwrap(), None);
    }

    fn anthropic_account() -> Account {
        Account {
            id: "anthropic:test".into(),
            provider: Provider::Anthropic,
            kind: "oauth".into(),
            name: "test".into(),
            description: None,
            paused: false,
            label: None,
            access_token: Some("direct-token".into()),
            refresh_token: None,
            id_token: None,
            api_key: None,
            expires_at_ms: Some(now_ms() + 3_600_000),
            last_refresh_ms: None,
            account_meta: Value::Null,
            cooldown_until_ms: None,
            status: "active".into(),
            path: None,
        }
    }

    fn openrouter_account() -> Account {
        Account {
            id: "openrouter-api-key".into(),
            provider: Provider::Openrouter,
            kind: "api_key".into(),
            name: "default".into(),
            description: None,
            paused: false,
            label: None,
            access_token: None,
            refresh_token: None,
            id_token: None,
            api_key: Some("openrouter-secret".into()),
            expires_at_ms: None,
            last_refresh_ms: None,
            account_meta: json!({
                "http_referer": "https://alexandria.example",
                "x_title": "Alexandria",
            }),
            cooldown_until_ms: None,
            status: "active".into(),
            path: None,
        }
    }

    struct FakeDario {
        active: Option<DarioActive>,
        begin_succeeds: bool,
    }

    impl DarioRouter for FakeDario {
        fn active(&self) -> Option<DarioActive> {
            self.active.clone()
        }

        fn begin(&self, _generation_id: &str) -> Option<Box<dyn std::any::Any + Send>> {
            self.begin_succeeds
                .then(|| Box::new(()) as Box<dyn std::any::Any + Send>)
        }

        fn status(&self) -> Value {
            json!({"active": self.active.is_some()})
        }

        fn suspect(&self, _generation_id: &str) {}
    }

    fn active_dario() -> Arc<dyn DarioRouter> {
        Arc::new(FakeDario {
            active: Some(DarioActive {
                generation_id: "test-generation".into(),
                base_url: "http://127.0.0.1:9191".into(),
                api_key: "dario-key".into(),
            }),
            begin_succeeds: true,
        })
    }

    fn claude_code_request() -> (HeaderMap, Value) {
        let mut headers = HeaderMap::new();
        headers.insert("user-agent", HeaderValue::from_static("claude-cli/2.1.0"));
        headers.insert("x-app", HeaderValue::from_static("cli"));
        headers.insert(
            "x-claude-code-session-id",
            HeaderValue::from_static("session-123"),
        );
        let body = json!({
            "model": "claude-sonnet-4-5",
            "system": [{
                "type": "text",
                "text": "x-anthropic-billing-header: cc_version=2.1.0"
            }],
            "messages": []
        });
        (headers, body)
    }

    #[test]
    fn claude_code_detection_requires_the_complete_signature() {
        let (headers, body) = claude_code_request();
        assert!(is_genuine_claude_code_request(
            ClientFormat::AnthropicMessages,
            &headers,
            &body
        ));
        assert!(!is_genuine_claude_code_request(
            ClientFormat::OpenaiChat,
            &headers,
            &body
        ));

        for required in ["user-agent", "x-app", "x-claude-code-session-id"] {
            let mut missing = headers.clone();
            missing.remove(required);
            assert!(
                !is_genuine_claude_code_request(
                    ClientFormat::AnthropicMessages,
                    &missing,
                    &body
                ),
                "request without {required} must not bypass Dario"
            );
        }

        let mut missing_billing = body.clone();
        missing_billing["system"] = json!([]);
        assert!(!is_genuine_claude_code_request(
            ClientFormat::AnthropicMessages,
            &headers,
            &missing_billing
        ));

        let mut explicit_other_harness = headers;
        explicit_other_harness.insert(
            "x-alexandria-harness",
            HeaderValue::from_static("pi"),
        );
        assert!(!is_genuine_claude_code_request(
            ClientFormat::AnthropicMessages,
            &explicit_other_harness,
            &body
        ));

        let (mut conflicting_harness, body) = claude_code_request();
        conflicting_harness.append(
            "x-alexandria-harness",
            HeaderValue::from_static("claude-code"),
        );
        conflicting_harness.append(
            "x-alexandria-harness",
            HeaderValue::from_static("codex"),
        );
        assert!(!is_genuine_claude_code_request(
            ClientFormat::AnthropicMessages,
            &conflicting_harness,
            &body
        ));

        let (mut malformed_harness, body) = claude_code_request();
        malformed_harness.insert(
            "x-alexandria-harness",
            HeaderValue::from_bytes(b"\x80").unwrap(),
        );
        assert!(!is_genuine_claude_code_request(
            ClientFormat::AnthropicMessages,
            &malformed_harness,
            &body
        ));
    }

    #[tokio::test]
    async fn anthropic_routing_bypasses_dario_only_for_claude_code() {
        let state = test_state_with_dario("dario-routing", Some(active_dario()));
        state.vault.upsert(anthropic_account()).await.unwrap();
        let (claude_headers, request) = claude_code_request();
        let original = serde_json::to_vec(&request).unwrap();
        let excluded = HashSet::new();

        let mut direct_body = request.clone();
        let direct = plan_upstream(
            &state,
            ClientFormat::AnthropicMessages,
            Provider::Anthropic,
            "claude-sonnet-4-5",
            &mut direct_body,
            &original,
            "trace-direct",
            &excluded,
            None,
            &claude_headers,
        )
        .await
        .unwrap();
        assert_eq!(direct.url, "https://api.anthropic.com/v1/messages");
        assert_eq!(direct.account.id, "anthropic:test");
        assert!(direct.extra_headers.is_empty());

        let mut harness_headers = HeaderMap::new();
        harness_headers.insert("x-alexandria-harness", HeaderValue::from_static("pi"));
        let mut dario_body = request;
        let dario = plan_upstream(
            &state,
            ClientFormat::AnthropicMessages,
            Provider::Anthropic,
            "claude-sonnet-4-5",
            &mut dario_body,
            &original,
            "trace-dario",
            &excluded,
            None,
            &harness_headers,
        )
        .await
        .unwrap();
        assert_eq!(dario.url, "http://127.0.0.1:9191/v1/messages");
        assert_eq!(dario.account.kind, "dario");
        assert!(dario
            .extra_headers
            .contains(&("x-dario-capture-id".into(), "trace-dario".into())));
    }

    #[tokio::test]
    async fn configured_dario_fails_closed_when_unhealthy() {
        let state = test_state_with_dario(
            "dario-unhealthy",
            Some(Arc::new(FakeDario {
                active: None,
                begin_succeeds: false,
            })),
        );
        state.vault.upsert(anthropic_account()).await.unwrap();
        let (_, mut request) = claude_code_request();
        let client_headers = HeaderMap::new();
        let excluded = HashSet::new();
        let original = serde_json::to_vec(&request).unwrap();

        let result = plan_upstream(
            &state,
            ClientFormat::AnthropicMessages,
            Provider::Anthropic,
            "claude-sonnet-4-5",
            &mut request,
            &original,
            "trace-unhealthy",
            &excluded,
            None,
            &client_headers,
        )
        .await;
        match result {
            Err((status, message)) => {
                assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
                assert!(message.contains("no healthy generation"));
            }
            Ok(_) => panic!("an unhealthy configured Dario must not fall back to direct traffic"),
        }
    }

    #[tokio::test]
    async fn dario_generation_race_fails_closed() {
        let state = test_state_with_dario(
            "dario-generation-race",
            Some(Arc::new(FakeDario {
                active: Some(DarioActive {
                    generation_id: "vanished-generation".into(),
                    base_url: "http://127.0.0.1:9191".into(),
                    api_key: "dario-key".into(),
                }),
                begin_succeeds: false,
            })),
        );
        let (_, mut request) = claude_code_request();
        let original = serde_json::to_vec(&request).unwrap();
        let client_headers = HeaderMap::new();
        let excluded = HashSet::new();

        let result = plan_upstream(
            &state,
            ClientFormat::AnthropicMessages,
            Provider::Anthropic,
            "claude-sonnet-4-5",
            &mut request,
            &original,
            "trace-race",
            &excluded,
            None,
            &client_headers,
        )
        .await;
        match result {
            Err((status, message)) => {
                assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
                assert!(message.contains("became unavailable"));
            }
            Ok(_) => panic!("a vanished Dario generation must not fall back to direct traffic"),
        }
    }

    #[test]
    fn direct_claude_code_headers_are_allowlisted() {
        let (mut client_headers, _) = claude_code_request();
        for name in CLAUDE_CODE_PASSTHROUGH_HEADERS {
            client_headers.insert(*name, HeaderValue::from_static("safe-client-value"));
        }
        client_headers.insert(
            "authorization",
            HeaderValue::from_static("Bearer client-secret"),
        );
        client_headers.insert("x-api-key", HeaderValue::from_static("client-secret"));
        client_headers.insert("host", HeaderValue::from_static("attacker.invalid"));
        client_headers.insert("content-length", HeaderValue::from_static("999"));
        client_headers.insert("connection", HeaderValue::from_static("close"));
        client_headers.insert("accept-encoding", HeaderValue::from_static("br"));
        client_headers.insert(
            "x-alexandria-harness",
            HeaderValue::from_static("claude"),
        );
        client_headers.insert(
            "x-dario-capture-id",
            HeaderValue::from_static("spoofed"),
        );

        let direct = upstream_headers(&anthropic_account(), &client_headers, true).unwrap();
        for name in CLAUDE_CODE_PASSTHROUGH_HEADERS {
            assert_eq!(direct[*name], "safe-client-value", "missing {name}");
        }
        assert_eq!(direct["authorization"], "Bearer direct-token");
        assert!(direct.get("x-api-key").is_none());
        assert!(direct.get("host").is_none());
        assert!(direct.get("content-length").is_none());
        assert!(direct.get("connection").is_none());
        assert_eq!(direct["accept-encoding"], "identity");
        assert!(direct.get("x-alexandria-harness").is_none());
        assert!(direct.get("x-dario-capture-id").is_none());

        let non_claude = upstream_headers(&anthropic_account(), &client_headers, false).unwrap();
        assert!(non_claude.get("x-app").is_none());
        assert!(non_claude.get("x-claude-code-session-id").is_none());
        assert_eq!(non_claude["authorization"], "Bearer direct-token");
    }

    #[test]
    fn openrouter_headers_use_only_vault_credentials_and_attribution() {
        let mut client_headers = HeaderMap::new();
        client_headers.insert(
            "authorization",
            HeaderValue::from_static("Bearer caller-secret"),
        );
        client_headers.insert("x-api-key", HeaderValue::from_static("another-provider-key"));
        client_headers.insert(
            "http-referer",
            HeaderValue::from_static("https://caller.example"),
        );
        client_headers.insert("x-title", HeaderValue::from_static("Caller title"));
        client_headers.insert("user-agent", HeaderValue::from_static("caller-agent"));

        let headers = upstream_headers(&openrouter_account(), &client_headers, false).unwrap();
        assert_eq!(headers["authorization"], "Bearer openrouter-secret");
        assert_eq!(headers["http-referer"], "https://alexandria.example");
        assert_eq!(headers["x-title"], "Alexandria");
        assert!(headers.get("x-api-key").is_none());
        assert!(headers.get("user-agent").is_none());
        assert_ne!(headers["authorization"], "Bearer caller-secret");
        assert_ne!(headers["http-referer"], "https://caller.example");
    }

    async fn response_json(resp: Response) -> (StatusCode, Value) {
        let status = resp.status();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        (status, value)
    }

    fn record_synthetic_sse_trace(
        state: &Arc<AppState>,
        id: &str,
        upstream_format: &'static str,
        chunks: &[&[u8]],
    ) -> Vec<Vec<u8>> {
        let mut observer = SseErrorObserver::new(upstream_format);
        let mut forwarded = Vec::new();
        for chunk in chunks {
            // This mirrors the production ordering: inspect the chunk, then
            // pass that exact chunk through unchanged.
            observer.observe(chunk);
            forwarded.push(chunk.to_vec());
        }
        observer.finish();

        let mut trace = TraceRecord {
            id: id.into(),
            ts_request_ms: now_ms(),
            status: Some(StatusCode::OK.as_u16() as i64),
            streamed: Some(true),
            upstream_format: Some(upstream_format.into()),
            ..Default::default()
        };
        trace.error = observer.error();
        let response: Vec<u8> = forwarded.iter().flatten().copied().collect();
        finalize_trace(state, trace, b"{}", None, Some(&response));
        forwarded
    }

    #[test]
    fn anthropic_sse_error_trace_keeps_the_http_200_stream_unchanged() {
        let state = test_state("anthropic-sse-error");
        let stream = b"event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}\n\n";

        let forwarded = record_synthetic_sse_trace(
            &state,
            "anthropic-sse-error",
            "anthropic",
            &[stream.as_slice()],
        );

        assert_eq!(forwarded, vec![stream.to_vec()]);
        let trace = state
            .store
            .get_trace("anthropic-sse-error")
            .unwrap()
            .unwrap();
        assert_eq!(trace["status"], StatusCode::OK.as_u16());
        assert_eq!(
            trace["error"],
            "upstream stream error: overloaded_error: Overloaded"
        );
    }

    #[test]
    fn split_responses_sse_error_trace_keeps_each_forwarded_chunk_unchanged() {
        let state = test_state("split-responses-sse-error");
        let first = b"event: response.failed\ndata: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"overloaded_error\",\"mes";
        let second = b"sage\":\"try again later\"}}}\n\n";

        let forwarded = record_synthetic_sse_trace(
            &state,
            "split-responses-sse-error",
            "openai-responses",
            &[first.as_slice(), second.as_slice()],
        );

        assert_eq!(forwarded, vec![first.to_vec(), second.to_vec()]);
        let trace = state
            .store
            .get_trace("split-responses-sse-error")
            .unwrap()
            .unwrap();
        assert_eq!(trace["status"], StatusCode::OK.as_u16());
        assert_eq!(
            trace["error"],
            "upstream stream error: overloaded_error: try again later"
        );
    }

    #[test]
    fn sse_error_observer_recognizes_openai_chat_and_gemini_error_frames() {
        let cases = [
            (
                "openai-chat",
                b"data: {\"error\":{\"type\":\"server_error\",\"message\":\"temporarily unavailable\"}}\n\n".as_slice(),
                "upstream stream error: server_error: temporarily unavailable",
            ),
            (
                "gemini",
                b"data: {\"response\":{\"error\":{\"status\":\"RESOURCE_EXHAUSTED\",\"message\":\"quota exhausted\"}}}\n\n".as_slice(),
                "upstream stream error: RESOURCE_EXHAUSTED: quota exhausted",
            ),
        ];
        for (upstream_format, frame, expected) in cases {
            let mut observer = SseErrorObserver::new(upstream_format);
            observer.observe(frame);
            observer.finish();
            assert_eq!(observer.error().as_deref(), Some(expected));
        }
    }

    struct FakeUpdater {
        result: Result<Value, UpdateApplyError>,
    }

    impl DaemonUpdater for FakeUpdater {
        fn apply(&self) -> UpdateApplyFuture {
            let result = match &self.result {
                Ok(body) => Ok(body.clone()),
                Err(UpdateApplyError::Conflict(body)) => {
                    Err(UpdateApplyError::Conflict(body.clone()))
                }
                Err(UpdateApplyError::Failed(message)) => {
                    Err(UpdateApplyError::Failed(message.clone()))
                }
            };
            Box::pin(async move { result })
        }
    }

    fn test_openai_account(name: &str) -> Account {
        Account {
            id: if name == "default" {
                "openai-oauth".into()
            } else {
                format!("openai-oauth-{name}")
            },
            provider: Provider::Openai,
            kind: "oauth".into(),
            name: name.into(),
            description: None,
            paused: false,
            label: Some("codex (test)".into()),
            access_token: Some(format!("token-{name}")),
            refresh_token: Some(format!("refresh-{name}")),
            id_token: None,
            api_key: None,
            expires_at_ms: Some(now_ms() + 3_600_000),
            last_refresh_ms: Some(now_ms()),
            account_meta: json!({"account_id": format!("chatgpt-{name}")}),
            cooldown_until_ms: None,
            status: "active".into(),
            path: None,
        }
    }

    #[test]
    fn codex_affinity_cache_expires_and_evicts_oldest_session() {
        let mut cache = CodexAffinityCache {
            entries: HashMap::new(),
            ttl_ms: 100,
            max_entries: 2,
        };
        cache.bind("session-a", "account-a", 0);
        cache.bind("session-b", "account-b", 10);
        assert_eq!(cache.preferred("session-a", 20).as_deref(), Some("account-a"));

        // session-a's lookup extends its expiry, so session-b is oldest.
        cache.bind("session-c", "account-c", 30);
        assert!(cache.preferred("session-b", 30).is_none());
        assert_eq!(cache.preferred("session-a", 30).as_deref(), Some("account-a"));
        assert_eq!(cache.preferred("session-c", 30).as_deref(), Some("account-c"));
        assert!(cache.preferred("session-a", 131).is_none());
    }

    #[test]
    fn captures_only_safe_routing_limit_headers() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("authorization", HeaderValue::from_static("Bearer secret"));
        headers.insert("x-codex-plan-type", HeaderValue::from_static("plus"));
        headers.insert(
            "x-codex-primary-used-percent",
            HeaderValue::from_static("42"),
        );
        headers.insert(
            "x-codex-primary-window-minutes",
            HeaderValue::from_static("300"),
        );
        headers.insert(
            "x-codex-primary-reset-at",
            HeaderValue::from_static("1800000000"),
        );
        let snapshot = routing_limits_from_headers(Provider::Openai, &headers).unwrap();
        assert_eq!(snapshot["plan"], "plus");
        assert_eq!(snapshot["windows"][0]["window"], "5h");
        assert_eq!(snapshot["windows"][0]["used_pct"], 42.0);
        assert!(!snapshot.to_string().contains("secret"));
    }

    #[tokio::test]
    async fn routing_api_updates_eligibility_order_and_reserve() {
        let state = test_state("codex-routing");
        state
            .vault
            .upsert(test_openai_account("default"))
            .await
            .unwrap();
        state
            .vault
            .upsert(test_openai_account("work"))
            .await
            .unwrap();
        let (status, body) = response_json(
            update_routing(
                state.clone(),
                Provider::Openai,
                json!({
                    "strategy": "priority",
                    "reserve_pct": 15,
                    "allow_mid_thread_failover": false,
                    "accounts": [
                        {"account_id": "openai-oauth-work", "eligible": true, "priority": 0, "reserve_pct": 22},
                        {"account_id": "openai-oauth", "eligible": false, "priority": 1}
                    ]
                }),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["strategy"], "priority");
        assert_eq!(body["reserve_pct"], 15);
        assert_eq!(body["allow_mid_thread_failover"], false);
        assert_eq!(body["accounts"][0]["account_id"], "openai-oauth-work");
        assert_eq!(body["accounts"][0]["eligible"], true);
        assert_eq!(body["accounts"][0]["reserve_pct"], 22);
        assert_eq!(body["accounts"][1]["eligible"], false);
        assert_eq!(body["accounts"][1]["reserve_pct"], 15);
        assert_eq!(
            state
                .vault
                .policy(Provider::Openai)
                .account_reserve_pct["work"],
            22
        );
        assert_eq!(
            state
                .vault
                .account_for(Provider::Openai, true)
                .await
                .unwrap()
                .name,
            "work"
        );
    }

    #[tokio::test]
    async fn provider_routing_endpoint_updates_a_non_codex_provider() {
        let state = test_state("anthropic-routing");
        let mut personal = anthropic_account();
        personal.id = "anthropic:personal".into();
        personal.name = "personal".into();
        let mut work = anthropic_account();
        work.id = "anthropic:work".into();
        work.name = "work".into();
        state.vault.upsert(personal).await.unwrap();
        state.vault.upsert(work).await.unwrap();

        let (status, body) = response_json(
            admin_routing_update(
                State(state.clone()),
                Path("anthropic".into()),
                axum::Json(json!({
                    "strategy": "round_robin",
                    "reserve_pct": 7,
                    "accounts": [
                        {"account_id": "anthropic:work", "eligible": true, "priority": 0, "reserve_pct": 3},
                        {"account_id": "anthropic:personal", "eligible": true, "priority": 1}
                    ]
                })),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["provider"], "anthropic");
        assert_eq!(body["strategy"], "round_robin");
        assert_eq!(body["reserve_pct"], 7);
        assert_eq!(body["accounts"][0]["reserve_pct"], 3);
        assert_eq!(state.vault.policy(Provider::Anthropic).account_reserve_pct["work"], 3);
    }

    #[tokio::test]
    async fn openrouter_key_endpoint_sets_attribution_removes_and_validates() {
        let state = test_state("admin-openrouter-key");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server_state = state.clone();
        let server = tokio::spawn(async move {
            axum::serve(listener, router(server_state)).await.unwrap();
        });
        let client = reqwest::Client::new();
        let endpoint = format!("http://{address}/admin/auth/openrouter-key");

        let unauthorized = client
            .post(&endpoint)
            .json(&json!({"key": "or-secret"}))
            .send()
            .await
            .unwrap();
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

        let missing = client
            .post(&endpoint)
            .header("x-api-key", "alx-local")
            .json(&json!({}))
            .send()
            .await
            .unwrap();
        assert_eq!(missing.status(), StatusCode::BAD_REQUEST);
        let missing_body: Value = missing.json().await.unwrap();
        assert_eq!(missing_body["error"]["type"], "alexandria");
        assert_eq!(missing_body["error"]["message"], "missing 'key'");

        let saved: Value = client
            .post(&endpoint)
            .header("x-api-key", "alx-local")
            .json(&json!({"key": "  or-secret  "}))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(saved["saved"], "openrouter-api-key");
        let account = state
            .vault
            .account_for(Provider::Openrouter, false)
            .await
            .unwrap();
        assert_eq!(account.api_key.as_deref(), Some("or-secret"));

        let attributed: Value = client
            .post(&endpoint)
            .header("x-api-key", "alx-local")
            .json(&json!({
                "key": "or-secret",
                "http_referer": " https://alexandria.example ",
                "x_title": " Alexandria "
            }))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(attributed["saved"], "openrouter-api-key");
        let account = state
            .vault
            .account_for(Provider::Openrouter, false)
            .await
            .unwrap();
        assert_eq!(
            account.account_meta["http_referer"],
            "https://alexandria.example"
        );
        assert_eq!(account.account_meta["x_title"], "Alexandria");

        let removed: Value = client
            .post(&endpoint)
            .header("x-api-key", "alx-local")
            .json(&json!({"remove": true}))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(removed["removed"], "openrouter-api-key");
        assert!(state
            .vault
            .list()
            .await
            .iter()
            .all(|account| account.provider != Provider::Openrouter));

        server.abort();
    }

    #[tokio::test]
    async fn codex_routing_endpoint_keeps_the_legacy_shape() {
        let state = test_state("codex-routing-compatibility");
        state.vault.upsert(test_openai_account("default")).await.unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, router(state)).await.unwrap();
        });
        let client = reqwest::Client::new();
        let legacy: Value = client
            .get(format!("http://{address}/admin/codex-routing"))
            .header("x-api-key", "alx-local")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let general: Value = client
            .get(format!("http://{address}/admin/routing/openai"))
            .header("x-api-key", "alx-local")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        server.abort();
        assert_eq!(legacy, general);
        let fields = legacy.as_object().unwrap();
        for field in [
            "provider",
            "strategy",
            "reserve_pct",
            "allow_mid_thread_failover",
            "reset_selection_rule",
            "accounts",
        ] {
            assert!(fields.contains_key(field), "missing legacy field {field}");
        }
        let account = legacy["accounts"][0].as_object().unwrap();
        for field in [
            "account_id",
            "eligible",
            "priority",
            "reserve_pct",
            "reserve_blocked",
            "reset_selection",
            "observed_at_ms",
            "plan",
            "active_limit",
            "windows",
            "credits",
        ] {
            assert!(account.contains_key(field), "missing legacy account field {field}");
        }
    }

    #[tokio::test]
    async fn admin_update_apply_response_shapes() {
        let state = test_state("admin-update-apply");
        set_daemon_updater(
            &state,
            Arc::new(FakeUpdater {
                result: Ok(json!({
                    "applying": true,
                    "current": "0.1.0",
                    "latest": "0.2.0",
                    "update_available": true,
                })),
            }),
        );
        let (status, body) = response_json(admin_update_apply(State(state.clone())).await).await;
        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(body["applying"], true);
        assert_eq!(body["latest"], "0.2.0");

        set_daemon_updater(
            &state,
            Arc::new(FakeUpdater {
                result: Ok(json!({
                    "applying": false,
                    "current": "0.2.0",
                    "latest": "0.2.0",
                    "update_available": false,
                })),
            }),
        );
        let (status, body) = response_json(admin_update_apply(State(state.clone())).await).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["update_available"], false);

        set_daemon_updater(
            &state,
            Arc::new(FakeUpdater {
                result: Err(UpdateApplyError::Conflict(json!({
                    "applying": false,
                    "current": "0.1.0",
                    "latest": "0.2.0",
                    "update_available": true,
                    "reason": "alex is managed by Homebrew - run `brew upgrade alex`",
                }))),
            }),
        );
        let (status, body) = response_json(admin_update_apply(State(state)).await).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(
            body["reason"],
            "alex is managed by Homebrew - run `brew upgrade alex`"
        );
    }

    #[tokio::test]
    async fn harness_run_key_mints_lists_validates_and_revokes() {
        let state = test_state("harness-run-key");
        let (_status, created) = response_json(
            admin_run_keys_create(
                State(state.clone()),
                Some(axum::Json(json!({
                    "kind": "harness",
                    "label": "pi",
                    "ttl_seconds": 1,
                    "tags": {"harness": "pi"},
                }))),
            )
            .await,
        )
        .await;
        assert_eq!(_status, StatusCode::CREATED);
        assert_eq!(created["kind"], "harness");
        assert_eq!(created["label"], "pi");
        assert_eq!(created["expires_ms"], Value::Null);
        let id = created["id"].as_str().unwrap().to_string();
        let key = created["key"].as_str().unwrap();
        let hash = key_hash_hex(key);

        let entry = run_key_entry(&state, &hash).unwrap();
        assert_eq!(entry.expires_ms, None);
        assert_eq!(entry.label.as_deref(), Some("pi"));
        assert_eq!(entry.tags_json.as_deref(), Some(r#"{"harness":"pi"}"#));

        let (_status, listed) =
            response_json(admin_run_keys_list(State(state.clone()), Query(HashMap::new())).await)
                .await;
        assert_eq!(_status, StatusCode::OK);
        let rows = listed["run_keys"].as_array().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["id"], id);
        assert_eq!(rows[0]["kind"], "harness");
        assert_eq!(rows[0]["label"], "pi");
        assert_eq!(rows[0]["expires_ms"], Value::Null);

        let (_status, revoked) =
            response_json(admin_run_keys_revoke(State(state.clone()), Path(id)).await).await;
        assert_eq!(_status, StatusCode::OK);
        assert_eq!(revoked["revoked"], true);
        assert!(run_key_entry(&state, &hash).is_none());
    }

    #[tokio::test]
    async fn harness_run_key_requires_label() {
        let state = test_state("harness-run-key-label");
        let (status, body) = response_json(
            admin_run_keys_create(
                State(state),
                Some(axum::Json(json!({
                    "kind": "harness",
                    "tags": {"harness": "pi"},
                }))),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body["error"]["message"].as_str().unwrap().contains("label"));
    }

    #[tokio::test]
    async fn codex_harness_event_records_authenticated_lineage() {
        let state = test_state("codex-harness-event");
        let (status, created) = response_json(
            admin_run_keys_create(
                State(state.clone()),
                Some(axum::Json(json!({"kind": "harness", "label": "codex"}))),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            HeaderValue::from_str(&format!(
                "Bearer {}",
                created["key"].as_str().unwrap()
            ))
            .unwrap(),
        );
        let (status, body) = response_json(
            harness_event(
                State(state.clone()),
                headers,
                axum::Json(json!({
                    "hook_event_name": "SubagentStart",
                    "session_id": "parent-session",
                    "turn_id": "turn-1",
                    "agent_id": "child-session",
                    "agent_type": "default",
                })),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["harness"], "codex");
        assert_eq!(body["lineage_updated"], true);
        assert_eq!(
            state
                .store
                .session_lineage_root("codex", "child-session")
                .unwrap(),
            "parent-session"
        );
    }

    #[tokio::test]
    async fn wrap_key_ingests_and_updates_trace_bodies() {
        use base64::Engine;

        let state = test_state("wrap-trace-ingest");
        let (status, created) = response_json(
            admin_run_keys_create(
                State(state.clone()),
                Some(axum::Json(json!({
                    "kind": "wrap",
                    "label": "remote-mac",
                    "tags": {"machine": "remote-mac"},
                }))),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(created["kind"], "wrap");
        assert_eq!(created["expires_ms"], Value::Null);
        let key = created["key"].as_str().unwrap();
        assert!(created["exports"]
            .as_str()
            .unwrap()
            .contains("ALEXANDRIA_TRACE_KEY"));
        assert!(!created["exports"]
            .as_str()
            .unwrap()
            .contains("OPENAI_API_KEY"));
        let mut headers = HeaderMap::new();
        headers.insert("x-api-key", HeaderValue::from_str(key).unwrap());
        let response_body = |text: &str| {
            base64::engine::general_purpose::STANDARD.encode(
                json!({"choices":[{"message":{"role":"assistant","content":text}}]}).to_string(),
            )
        };
        let payload = |text: &str| TraceIngestPayload {
            trace: TraceRecord {
                id: "agent-remote-session-1".into(),
                ts_request_ms: 1000,
                ts_response_ms: Some(2000),
                session_id: Some("remote-session".into()),
                harness: Some("agent".into()),
                upstream_provider: Some("cursor".into()),
                routed_model: Some("cursor-agent".into()),
                tags: Some(r#"{"stream":"dialogue"}"#.into()),
                ..Default::default()
            },
            request_body_b64: Some(
                base64::engine::general_purpose::STANDARD.encode(br#"{"messages":[]}"#),
            ),
            upstream_request_body_b64: None,
            response_body_b64: Some(response_body(text)),
        };

        let (status, body) = response_json(
            traces_ingest(
                State(state.clone()),
                headers.clone(),
                axum::Json(payload("progress")),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(body["outcome"], "inserted");
        let row = state
            .store
            .get_trace("agent-remote-session-1")
            .unwrap()
            .unwrap();
        assert_eq!(row["key_fingerprint"], &key_hash_hex(key)[..16]);
        assert_eq!(
            row["tags_json"].as_str().unwrap().contains("remote-mac"),
            true
        );
        assert!(row["req_body_path"].as_str().is_some());
        assert!(read_gz_text(row["resp_body_path"].as_str())
            .unwrap()
            .contains("progress"));

        let (status, body) = response_json(
            traces_ingest(
                State(state.clone()),
                headers,
                axum::Json(payload("final answer")),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["outcome"], "updated");
        let row = state
            .store
            .get_trace("agent-remote-session-1")
            .unwrap()
            .unwrap();
        assert!(read_gz_text(row["resp_body_path"].as_str())
            .unwrap()
            .contains("final answer"));
    }

    #[tokio::test]
    async fn trace_ingest_rejects_non_wrap_key() {
        let state = test_state("trace-ingest-kind");
        let (status, created) = response_json(
            admin_run_keys_create(
                State(state.clone()),
                Some(axum::Json(json!({"kind": "run", "label": "not-wrap"}))),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-api-key",
            HeaderValue::from_str(created["key"].as_str().unwrap()).unwrap(),
        );
        let (status, _) = response_json(
            traces_ingest(
                State(state),
                headers,
                axum::Json(TraceIngestPayload {
                    trace: TraceRecord {
                        id: "remote-1".into(),
                        ts_request_ms: 1,
                        ..Default::default()
                    },
                    request_body_b64: None,
                    upstream_request_body_b64: None,
                    response_body_b64: None,
                }),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn trace_ingest_rejects_updates_from_a_different_wrap_key() {
        let state = test_state("trace-ingest-owner");
        let mint = |label: &str| {
            admin_run_keys_create(
                State(state.clone()),
                Some(axum::Json(json!({"kind": "wrap", "label": label}))),
            )
        };
        let (_, first) = response_json(mint("first").await).await;
        let (_, second) = response_json(mint("second").await).await;
        let headers = |key: &str| {
            let mut headers = HeaderMap::new();
            headers.insert("x-api-key", HeaderValue::from_str(key).unwrap());
            headers
        };
        let payload = || TraceIngestPayload {
            trace: TraceRecord {
                id: "owned-trace-1".into(),
                ts_request_ms: 1,
                ..Default::default()
            },
            request_body_b64: None,
            upstream_request_body_b64: None,
            response_body_b64: None,
        };

        let (status, _) = response_json(
            traces_ingest(
                State(state.clone()),
                headers(first["key"].as_str().unwrap()),
                axum::Json(payload()),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let (status, body) = response_json(
            traces_ingest(
                State(state),
                headers(second["key"].as_str().unwrap()),
                axum::Json(payload()),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert!(body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("another credential"));
    }

    #[tokio::test]
    async fn wrap_key_cannot_invoke_models() {
        let state = test_state("wrap-key-inference");
        let (_, created) = response_json(
            admin_run_keys_create(
                State(state.clone()),
                Some(axum::Json(json!({"kind": "wrap", "label": "remote"}))),
            )
            .await,
        )
        .await;
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-api-key",
            HeaderValue::from_str(created["key"].as_str().unwrap()).unwrap(),
        );
        let response = proxy(
            state,
            ClientFormat::OpenaiChat,
            "/v1/chat/completions",
            headers,
            Bytes::from_static(br#"{"model":"gpt-5.5","messages":[]}"#),
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let (_, body) = response_json(response).await;
        assert!(body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("only post to /traces/ingest"));
    }

    #[test]
    fn failover_statuses_are_limited_to_auth_rate_limit_and_server_errors() {
        for status in [
            reqwest::StatusCode::UNAUTHORIZED,
            reqwest::StatusCode::FORBIDDEN,
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            reqwest::StatusCode::BAD_GATEWAY,
            reqwest::StatusCode::SERVICE_UNAVAILABLE,
        ] {
            assert!(
                retryable_failover_status(status),
                "{status} should fail over"
            );
        }
        for status in [
            reqwest::StatusCode::OK,
            reqwest::StatusCode::BAD_REQUEST,
            reqwest::StatusCode::NOT_FOUND,
            reqwest::StatusCode::UNPROCESSABLE_ENTITY,
        ] {
            assert!(
                !retryable_failover_status(status),
                "{status} should be returned without failover"
            );
        }
    }

    #[test]
    fn disabled_mid_thread_failover_only_pins_existing_codex_threads() {
        assert!(!retry_failover_allowed(Provider::Openai, true, false));
        assert!(retry_failover_allowed(Provider::Openai, false, false));
        assert!(retry_failover_allowed(Provider::Openai, true, true));
        assert!(retry_failover_allowed(Provider::Anthropic, true, false));
    }

    #[tokio::test]
    async fn codex_planning_excludes_every_account_already_attempted() {
        let state = test_state("codex-plan-exclusions");
        for (id, name) in [
            ("openai-oauth-a", "a"),
            ("openai-oauth-b", "b"),
            ("openai-oauth-c", "c"),
        ] {
            state
                .vault
                .upsert(Account {
                    id: id.into(),
                    provider: Provider::Openai,
                    kind: "oauth".into(),
                    name: name.into(),
                    description: None,
                    paused: false,
                    label: None,
                    access_token: Some(format!("access-{name}")),
                    refresh_token: Some(format!("refresh-{name}")),
                    id_token: None,
                    api_key: None,
                    expires_at_ms: Some(now_ms() + 3_600_000),
                    last_refresh_ms: Some(now_ms()),
                    account_meta: json!({"account_id": format!("chatgpt-{name}")}),
                    cooldown_until_ms: None,
                    status: "active".into(),
                    path: None,
                })
                .await
                .unwrap();
        }

        let original = json!({"model": "gpt-5.3-codex", "stream": true});
        let raw = serde_json::to_vec(&original).unwrap();
        let client_headers = HeaderMap::new();
        let mut excluded = HashSet::new();
        let mut selected = Vec::new();
        for _ in 0..3 {
            let mut body = original.clone();
            let plan = plan_upstream(
                &state,
                ClientFormat::OpenaiResponses,
                Provider::Openai,
                "gpt-5.3-codex",
                &mut body,
                &raw,
                "trace-test",
                &excluded,
                None,
                &client_headers,
            )
            .await
            .unwrap();
            assert!(excluded.insert(plan.account.id.clone()));
            selected.push(plan.account.id);
        }

        assert_eq!(
            selected,
            vec!["openai-oauth-a", "openai-oauth-b", "openai-oauth-c"]
        );
        let mut body = original;
        assert!(plan_upstream(
            &state,
            ClientFormat::OpenaiResponses,
            Provider::Openai,
            "gpt-5.3-codex",
            &mut body,
            &raw,
            "trace-test",
            &excluded,
            None,
            &client_headers,
        )
        .await
        .is_err());
    }

    #[tokio::test]
    async fn codex_sessions_stay_affined_and_failover_rebinds() {
        let state = test_state("codex-session-affinity");
        state
            .vault
            .upsert(test_openai_account("a"))
            .await
            .unwrap();
        state
            .vault
            .upsert(test_openai_account("b"))
            .await
            .unwrap();
        state
            .vault
            .set_policies(vec![(
                Provider::Openai,
                AccountPolicy {
                    order: vec!["a".into(), "b".into()],
                    mode: AccountPolicyMode::RoundRobin,
                    reserve_pct: Some(0),
                    ..AccountPolicy::default()
                },
            )])
            .await;

        let request = json!({"model": "gpt-5.3-codex", "stream": true});
        let raw = serde_json::to_vec(&request).unwrap();
        let headers = HeaderMap::new();
        let none_excluded = HashSet::new();

        let mut body = request.clone();
        let session_a_first = plan_upstream(
            &state,
            ClientFormat::OpenaiResponses,
            Provider::Openai,
            "gpt-5.3-codex",
            &mut body,
            &raw,
            "trace-a-1",
            &none_excluded,
            Some("session-a"),
            &headers,
        )
        .await
        .unwrap()
        .account
        .id;

        let mut body = request.clone();
        let session_b_first = plan_upstream(
            &state,
            ClientFormat::OpenaiResponses,
            Provider::Openai,
            "gpt-5.3-codex",
            &mut body,
            &raw,
            "trace-b-1",
            &none_excluded,
            Some("session-b"),
            &headers,
        )
        .await
        .unwrap()
        .account
        .id;
        assert_ne!(session_a_first, session_b_first);

        let mut body = request.clone();
        let session_a_again = plan_upstream(
            &state,
            ClientFormat::OpenaiResponses,
            Provider::Openai,
            "gpt-5.3-codex",
            &mut body,
            &raw,
            "trace-a-2",
            &none_excluded,
            Some("session-a"),
            &headers,
        )
        .await
        .unwrap()
        .account
        .id;
        assert_eq!(session_a_again, session_a_first);

        let excluded = HashSet::from([session_a_first.clone()]);
        let mut body = request.clone();
        let failover = plan_upstream(
            &state,
            ClientFormat::OpenaiResponses,
            Provider::Openai,
            "gpt-5.3-codex",
            &mut body,
            &raw,
            "trace-a-failover",
            &excluded,
            Some("session-a"),
            &headers,
        )
        .await
        .unwrap()
        .account
        .id;
        assert_eq!(failover, session_b_first);

        let mut body = request;
        let after_failover = plan_upstream(
            &state,
            ClientFormat::OpenaiResponses,
            Provider::Openai,
            "gpt-5.3-codex",
            &mut body,
            &raw,
            "trace-a-3",
            &none_excluded,
            Some("session-a"),
            &headers,
        )
        .await
        .unwrap()
        .account
        .id;
        assert_eq!(after_failover, failover);
    }
}
