use std::collections::{BTreeMap, HashMap, HashSet};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

mod plugins;
pub use plugins::{PluginManager, PluginManifest};
pub mod notify;

use alex_auth::{
    encrypt_bundle, export_bundle, harness_cred_paths, now_ms, routing_reserve_blocked,
    routing_reserve_pct, routing_reset_selection, Account, AccountPolicy, AccountPolicyMode,
    BundleSelection, RemovedAccount, Vault,
};
use alex_core::{
    compute_cost, conversation_root, parse_grpc_web_response, parse_since, parse_sse_usage,
    parse_trace_tags, parse_usage_api_response, quota_state, route_model, usage_from_json,
    usage_to_limits_entry, validate_grpc_status_headers, window_label, ClientFormat, Provider,
    TraceIngestPayload, TraceRecord, GROK_CREDITS_ENDPOINT, GROK_CREDITS_REQUEST_BODY,
};
use alex_store::{KnownAccount, Store, ToolCallRecord, TraceFilter};
use anyhow::Result;
use axum::body::{Body, Bytes};
use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::Router;
use chrono::{TimeZone, Utc};
use futures_util::StreamExt;
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio_stream::wrappers::ReceiverStream;

const ANTHROPIC_BASE: &str = "https://api.anthropic.com";
const OPENAI_BASE: &str = "https://api.openai.com";
const OPENROUTER_BASE: &str = "https://openrouter.ai/api/v1";
/// Kimi Code (Moonshot AI) OpenAI-compatible coding endpoint.
const KIMI_BASE: &str = "https://api.kimi.com/coding/v1";
const CODEX_BASE: &str = "https://chatgpt.com/backend-api/codex";
const XAI_BASE: &str = "https://cli-chat-proxy.grok.com/v1";
const GROK_CLIENT_VERSION: &str = "0.2.77";
const ANTHROPIC_OAUTH_BETA: &str = "oauth-2025-04-20";
const GEMINI_CODE_ASSIST_BASE: &str = "https://cloudcode-pa.googleapis.com";
const GEMINI_CODE_ASSIST_VERSION: &str = "v1internal";
const GEMINI_API_BASE: &str = "https://generativelanguage.googleapis.com";
const CODEX_AFFINITY_TTL_MS: i64 = 30 * 24 * 60 * 60 * 1000;
const CODEX_AFFINITY_MAX_ENTRIES: usize = 10_000;

/// Cross-model substitution is deliberately opt-in. Same-provider account
/// failover is independent of this setting and remains enabled by default.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SubstitutionConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub fallbacks: BTreeMap<String, Vec<String>>,
}

/// Opt-in escalation policy. It deliberately lives beside the legacy
/// substitution list so existing `fallbacks` configurations keep working.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtectionPolicy {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub reroute_on_auth: bool,
    #[serde(default = "default_protection_retries")]
    pub retries: u32,
    #[serde(default = "default_auto_return")]
    pub auto_return: bool,
    #[serde(default)]
    pub equivalencies: BTreeMap<String, BTreeMap<String, String>>,
}

/// Persists a protection-policy update owned by the daemon configuration.
/// The proxy deliberately does not know the daemon's config format or path.
pub trait ProtectionPolicyPersister: Send + Sync {
    fn persist(&self, policy: &ProtectionPolicy) -> std::result::Result<(), String>;
}

/// Local Exo endpoint settings. Exo is account-less: its OpenAI-compatible
/// server accepts the dummy bearer token used for requests.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExoConfig {
    #[serde(default = "default_exo_url")]
    pub url: String,
    #[serde(default)]
    pub enabled_models: Vec<String>,
}

fn default_exo_url() -> String {
    "http://localhost:52415".into()
}

impl Default for ExoConfig {
    fn default() -> Self {
        Self {
            url: default_exo_url(),
            enabled_models: Vec::new(),
        }
    }
}

/// Persists an Exo settings update owned by the daemon configuration.
pub trait ExoConfigPersister: Send + Sync {
    fn persist(&self, config: &ExoConfig) -> std::result::Result<(), String>;
}

/// Persists notification settings owned by the daemon configuration. Keeping
/// this boundary in the binary lets the proxy hot-apply channels without
/// knowing where config.toml lives.
pub trait NotificationConfigPersister: Send + Sync {
    fn persist(&self, settings: &notify::NotificationSettings) -> std::result::Result<(), String>;
}

impl Default for ProtectionPolicy {
    fn default() -> Self {
        Self {
            enabled: false,
            reroute_on_auth: false,
            retries: default_protection_retries(),
            auto_return: default_auto_return(),
            equivalencies: BTreeMap::new(),
        }
    }
}

fn default_protection_retries() -> u32 {
    1
}
fn default_auto_return() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    UpstreamToClient,
    ClientToProxy,
}
impl Default for Direction {
    fn default() -> Self {
        Self::UpstreamToClient
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorFixture {
    pub name: String,
    pub provider: String,
    pub status: u16,
    pub error_kind: String,
    pub body: String,
    #[serde(default)]
    pub direction: Direction,
    pub created_ms: i64,
    #[serde(default)]
    pub source_trace_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PendingInjection {
    #[serde(flatten)]
    fixture: ErrorFixture,
    count: u32,
}

#[derive(Debug, Clone)]
pub struct DarioActive {
    pub generation_id: String,
    pub base_url: String,
    pub api_key: String,
}

pub trait DarioRouter: Send + Sync {
    fn routes_requests(&self) -> bool {
        true
    }
    fn active(&self) -> Option<DarioActive>;
    /// Repair a missing generation before a request is sent. Implementations
    /// must return promptly with an error rather than leaving clients waiting.
    fn ensure_active(&self) -> DarioEnsureFuture {
        let active = self.active();
        Box::pin(async move { active.ok_or_else(|| "no healthy Dario generation".into()) })
    }
    fn begin(&self, generation_id: &str) -> Option<Box<dyn std::any::Any + Send>>;
    fn prepare_model(&self, _model: &str) -> DarioPrepareFuture {
        Box::pin(async { DarioPrepare::ServeThroughDario })
    }
    fn probe(&self, _model: &str) -> DarioProbeFuture {
        Box::pin(async { Err("through-Dario probe is not implemented".into()) })
    }
    fn status(&self) -> Value;
    fn suspect(&self, generation_id: &str);
}

pub type DarioPrepareFuture = Pin<Box<dyn Future<Output = DarioPrepare> + Send + 'static>>;
pub type DarioEnsureFuture =
    Pin<Box<dyn Future<Output = Result<DarioActive, String>> + Send + 'static>>;
pub type DarioProbeFuture = Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'static>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DarioPrepare {
    ServeThroughDario,
    DirectFallback { reason: String },
    Unavailable { reason: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DarioHealthState {
    NotApplicable,
    Down,
    Healthy,
}

pub fn dario_health_state(
    anthropic_credentials_present: bool,
    generation_ready: bool,
    through_dario_probe_succeeds: bool,
) -> DarioHealthState {
    if !anthropic_credentials_present {
        DarioHealthState::NotApplicable
    } else if generation_ready && through_dario_probe_succeeds {
        DarioHealthState::Healthy
    } else {
        DarioHealthState::Down
    }
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

pub type UpdateChannelSetFuture = Pin<
    Box<dyn Future<Output = std::result::Result<SetChannelOutcome, UpdateChannelError>> + Send + 'static>,
>;

/// Outcome of persisting a new daemon update channel.
#[derive(Debug, Clone)]
pub struct SetChannelOutcome {
    /// The channel that is now persisted (normalized: "stable" | "beta").
    pub channel: String,
    /// The daemon update status recomputed against the new channel, when it
    /// could be checked. `None` when the check could not run (e.g. offline);
    /// the channel is still persisted.
    pub status: Option<Value>,
}

#[derive(Debug)]
pub enum UpdateChannelError {
    /// The requested channel is not a recognized value; maps to 400.
    Invalid(String),
    /// Persistence or another internal step failed; maps to 500.
    Failed(String),
}

/// Reads and writes the persisted daemon update channel (config.toml
/// `update_channel`). Injected by the binary so the proxy can hot-apply a
/// channel change through the exact persistence the CLI `--set-channel` uses,
/// without the proxy needing to know where config.toml lives. Keeping this the
/// only writer path is what stops the app picker and the daemon from diverging.
pub trait UpdateChannelController: Send + Sync {
    /// The channel currently persisted in config.toml ("stable" | "beta").
    fn current(&self) -> String;
    /// Validate, persist, and recompute the update status against `channel`.
    fn set(&self, channel: String) -> UpdateChannelSetFuture;
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
            Provider::Gemini
            | Provider::Openrouter
            | Provider::Exo
            | Provider::Amp
            | Provider::Kimi => false,
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
    exo: std::sync::RwLock<ExoConfig>,
    exo_persister: std::sync::RwLock<Option<Arc<dyn ExoConfigPersister>>>,
    pub logins: alex_auth::sessions::LoginManager,
    pub run_keys: std::sync::RwLock<HashMap<String, CachedRunKey>>,
    trace_ingest_lock: tokio::sync::Mutex<()>,
    pub update_status: Arc<tokio::sync::RwLock<Option<Value>>>,
    pub daemon_updater: std::sync::RwLock<Option<Arc<dyn DaemonUpdater>>>,
    update_channel_controller: std::sync::RwLock<Option<Arc<dyn UpdateChannelController>>>,
    pub reset_handler: std::sync::RwLock<Option<Arc<dyn ResetHandler>>>,
    pub plugins: Arc<PluginManager>,
    pub notifications: Arc<std::sync::RwLock<notify::NotificationDispatcher>>,
    notification_settings: std::sync::RwLock<notify::NotificationSettings>,
    notification_persister: std::sync::RwLock<Option<Arc<dyn NotificationConfigPersister>>>,
    telegram_base: std::sync::RwLock<String>,
    codex_affinity: std::sync::Mutex<CodexAffinityCache>,
    codex_affinity_locks:
        std::sync::Mutex<HashMap<String, std::sync::Weak<tokio::sync::Mutex<()>>>>,
    substitution: SubstitutionConfig,
    protection: std::sync::RwLock<ProtectionPolicy>,
    protection_persister: std::sync::RwLock<Option<Arc<dyn ProtectionPolicyPersister>>>,
    fixture_dir: std::sync::RwLock<Option<PathBuf>>,
    pending_injections: std::sync::Mutex<HashMap<String, Vec<PendingInjection>>>,
    /// Deliberately transient provider-wide fault injection for exercising
    /// failover and re-auth notifications. It is intentionally not persisted.
    paused_providers: std::sync::Mutex<HashMap<String, PauseMode>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PauseMode {
    Down,
    LoggedOut,
}

impl PauseMode {
    fn status(self) -> u16 {
        match self {
            Self::Down => 503,
            Self::LoggedOut => 401,
        }
    }

    fn error_class(self) -> ErrorClass {
        match self {
            Self::Down => ErrorClass::Server,
            Self::LoggedOut => ErrorClass::Auth,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Down => "down",
            Self::LoggedOut => "logged_out",
        }
    }
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
    build_state_with_substitution(
        local_key,
        vault,
        store,
        dario,
        base_url,
        upstream_stream_idle_timeout,
        SubstitutionConfig::default(),
    )
}

pub fn build_state_with_substitution(
    local_key: String,
    vault: Arc<Vault>,
    store: Arc<Store>,
    dario: Option<Arc<dyn DarioRouter>>,
    base_url: String,
    upstream_stream_idle_timeout: Duration,
    substitution: SubstitutionConfig,
) -> Arc<AppState> {
    // Import sidecars written by Vault::remove so a daemon restarted after a
    // terminal-side removal still exposes removed history to the Trace Browser.
    for removed in vault.removed_accounts() {
        if let Err(e) =
            store.tombstone_known_account(&known_removed_account(&removed), removed.removed_ms)
        {
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
        exo: std::sync::RwLock::new(ExoConfig::default()),
        exo_persister: std::sync::RwLock::new(None),
        logins: alex_auth::sessions::LoginManager::default(),
        run_keys: std::sync::RwLock::new(HashMap::new()),
        trace_ingest_lock: tokio::sync::Mutex::new(()),
        update_status: Arc::new(tokio::sync::RwLock::new(None)),
        daemon_updater: std::sync::RwLock::new(None),
        update_channel_controller: std::sync::RwLock::new(None),
        reset_handler: std::sync::RwLock::new(None),
        plugins: Arc::new(PluginManager::empty()),
        notifications: Arc::new(std::sync::RwLock::new(
            notify::NotificationDispatcher::default(),
        )),
        notification_settings: std::sync::RwLock::new(notify::NotificationSettings::default()),
        notification_persister: std::sync::RwLock::new(None),
        telegram_base: std::sync::RwLock::new("https://api.telegram.org".into()),
        codex_affinity: std::sync::Mutex::new(CodexAffinityCache::default()),
        codex_affinity_locks: std::sync::Mutex::new(HashMap::new()),
        substitution,
        protection: std::sync::RwLock::new(ProtectionPolicy::default()),
        protection_persister: std::sync::RwLock::new(None),
        fixture_dir: std::sync::RwLock::new(None),
        pending_injections: std::sync::Mutex::new(HashMap::new()),
        paused_providers: std::sync::Mutex::new(HashMap::new()),
    })
}

pub fn set_protection_policy(state: &Arc<AppState>, policy: ProtectionPolicy) {
    if let Ok(mut slot) = state.protection.write() {
        *slot = policy;
    }
}

/// Installs the daemon-owned config persistence hook for admin policy writes.
pub fn set_protection_policy_persister(
    state: &Arc<AppState>,
    persister: Arc<dyn ProtectionPolicyPersister>,
) {
    if let Ok(mut slot) = state.protection_persister.write() {
        *slot = Some(persister);
    }
}

/// Replaces Exo settings used for both routing and the published catalog.
pub fn set_exo_config(state: &Arc<AppState>, config: ExoConfig) {
    if let Ok(mut slot) = state.exo.write() {
        *slot = config;
    }
}

/// Installs the daemon-owned config persistence hook for Exo admin writes.
pub fn set_exo_config_persister(state: &Arc<AppState>, persister: Arc<dyn ExoConfigPersister>) {
    if let Ok(mut slot) = state.exo_persister.write() {
        *slot = Some(persister);
    }
}

/// Model identifiers exposed by enabled Exo models, including the explicit
/// provider prefix and Alexandria's convenient local alias.
pub fn exo_catalog_models(state: &AppState) -> Vec<String> {
    state
        .exo
        .read()
        .map(|config| {
            config
                .enabled_models
                .iter()
                .flat_map(|model| [format!("exo/{model}"), format!("alex/{model}")])
                .collect()
        })
        .unwrap_or_default()
}

/// Kimi Code models Alexandria routes to `https://api.kimi.com/coding/v1`.
/// Advertised in `/v1/models` only when a Kimi account is present, so a harness
/// pointed at Alex can select `kimi/k3` etc. from its model picker.
pub const KIMI_CATALOG_MODELS: &[&str] = &[
    "kimi/k3",
    "kimi/kimi-for-coding",
    "kimi/kimi-for-coding-highspeed",
];

pub async fn kimi_catalog_models(state: &AppState) -> Vec<String> {
    let has_account = state
        .vault
        .list()
        .await
        .into_iter()
        .any(|a| a.provider == Provider::Kimi && a.status == "active");
    if !has_account {
        return Vec::new();
    }
    KIMI_CATALOG_MODELS
        .iter()
        .map(|id| (*id).to_string())
        .collect()
}

/// Kimi's usage/quota endpoint (discovered in the kimi node binary:
/// `managedUsageUrl` -> `${base}/usages`, GET with a Bearer token).
fn kimi_usage_url() -> String {
    format!("{KIMI_BASE}/usages")
}

fn kimi_usage_num(raw: &Value, key: &str) -> Option<i64> {
    raw.get(key).and_then(|v| {
        v.as_i64()
            .or_else(|| v.as_f64().map(|f| f as i64))
            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
    })
}

/// Convert one Kimi usage row (`{limit, used, remaining, name|title, reset_at}`)
/// into an Alexandria rate-window entry. Mirrors the kimi CLI's `toUsageRow`.
fn kimi_usage_row(raw: &Value, default_label: &str) -> Option<Value> {
    let limit = kimi_usage_num(raw, "limit");
    let mut used = kimi_usage_num(raw, "used");
    if used.is_none() {
        if let (Some(remaining), Some(limit)) = (kimi_usage_num(raw, "remaining"), limit) {
            used = Some(limit - remaining);
        }
    }
    if used.is_none() && limit.is_none() {
        return None;
    }
    let label = raw
        .get("name")
        .and_then(Value::as_str)
        .or_else(|| raw.get("title").and_then(Value::as_str))
        .unwrap_or(default_label)
        .to_string();
    let used_pct = match (used, limit) {
        (Some(u), Some(l)) if l > 0 => Some((u as f64 / l as f64) * 100.0),
        _ => None,
    };
    let resets_at = ["reset_at", "resetAt", "reset_time", "resetTime"]
        .iter()
        .find_map(|k| raw.get(*k).and_then(Value::as_str))
        .map(String::from);
    Some(json!({
        "label": label,
        "used": used,
        "limit": limit,
        "remaining": kimi_usage_num(raw, "remaining"),
        "used_pct": used_pct,
        "resets_at": resets_at,
    }))
}

/// Parse the `/usages` payload into a routing-limits snapshot. Pure + tolerant so
/// it can be unit-tested and degrades gracefully on an unexpected shape.
pub fn parse_kimi_usage_payload(payload: &Value) -> Value {
    let mut windows = Vec::new();
    if let Some(row) = payload
        .get("usage")
        .and_then(|u| kimi_usage_row(u, "Weekly limit"))
    {
        windows.push(row);
    }
    if let Some(limits) = payload.get("limits").and_then(Value::as_array) {
        for (idx, item) in limits.iter().enumerate() {
            let detail = item.get("detail").filter(|d| d.is_object()).unwrap_or(item);
            let default_label = format!("Limit #{}", idx + 1);
            if let Some(row) = kimi_usage_row(detail, &default_label) {
                windows.push(row);
            }
        }
    }
    let credits = payload
        .get("boosterWallet")
        .filter(|w| w.is_object())
        .cloned()
        .unwrap_or(Value::Null);
    json!({
        "provider": "kimi",
        "source": "kimi /usages",
        "windows": windows,
        "credits": credits,
    })
}

/// Fetch and record Kimi usage for the active account. Returns a short human
/// summary for the health line. Degrades gracefully when usage is unavailable.
async fn kimi_usage_probe(state: &Arc<AppState>) -> (bool, Option<String>, String) {
    let account = match state.vault.account_for(Provider::Kimi, true).await {
        Ok(account) => account,
        Err(e) => return (false, None, e.to_string()),
    };
    let Some(token) = account.access_token.clone() else {
        return (
            false,
            Some(account.id.clone()),
            "kimi account has no access token".into(),
        );
    };
    let resp = state
        .http
        .get(kimi_usage_url())
        .header("authorization", format!("Bearer {token}"))
        .header("accept", "application/json")
        .send()
        .await;
    match resp {
        Ok(resp) => {
            let status = resp.status();
            if status == reqwest::StatusCode::NOT_FOUND {
                return (
                    true,
                    Some(account.id.clone()),
                    "creds ok (usage not reported)".into(),
                );
            }
            let body: Value = resp.json().await.unwrap_or(Value::Null);
            if !status.is_success() {
                return (
                    false,
                    Some(account.id.clone()),
                    format!("usage HTTP {status}"),
                );
            }
            let snapshot = parse_kimi_usage_payload(&body);
            let summary = snapshot["windows"]
                .as_array()
                .and_then(|w| w.first())
                .map(|row| {
                    let label = row["label"].as_str().unwrap_or("limit");
                    match (row["used"].as_i64(), row["limit"].as_i64()) {
                        (Some(u), Some(l)) => format!("{label}: {u}/{l}"),
                        _ => format!("{label} ok"),
                    }
                })
                .unwrap_or_else(|| "creds ok".into());
            let _ = state
                .vault
                .record_routing_limits(&account.id, snapshot)
                .await;
            (true, Some(account.id.clone()), summary)
        }
        Err(e) => (
            false,
            Some(account.id.clone()),
            format!("usage endpoint unreachable: {e}"),
        ),
    }
}

fn exo_model_enabled(state: &AppState, model: &str) -> bool {
    state
        .exo
        .read()
        .map(|config| config.enabled_models.iter().any(|id| id == model))
        .unwrap_or(false)
}

pub fn set_fixture_dir(state: &Arc<AppState>, dir: PathBuf) {
    // First-run seeding is deliberately idempotent and never overwrites an
    // operator's library.
    if let Err(error) = starter_fixtures(&dir) {
        tracing::warn!(%error, "could not seed error fixtures");
    }
    if let Ok(mut slot) = state.fixture_dir.write() {
        *slot = Some(dir);
    }
}

/// Replaces the daemon's notification channels after config has been loaded.
/// This is intentionally separate from `build_state` to keep its existing
/// callers source-compatible (offline commands and tests have no channels).
pub fn set_notifications(state: &Arc<AppState>, mut settings: notify::NotificationSettings) {
    // Legacy TOML channels predate runtime IDs. Give them deterministic IDs in
    // memory; the next admin save persists the migration without requiring a
    // restart or a config rewrite at boot.
    for (index, channel) in settings.channels.iter_mut().enumerate() {
        if channel.id.as_deref().unwrap_or_default().trim().is_empty() {
            channel.id = Some(format!("channel-{index}"));
        }
    }
    let dispatcher = notify::NotificationDispatcher::from_settings(settings.clone());
    if let Ok(mut notifications) = state.notifications.write() {
        *notifications = dispatcher;
    }
    if let Ok(mut current) = state.notification_settings.write() {
        *current = settings;
    }
}

/// Installs the daemon-owned config persistence hook for runtime notification
/// channel updates.
pub fn set_notification_config_persister(
    state: &Arc<AppState>,
    persister: Arc<dyn NotificationConfigPersister>,
) {
    if let Ok(mut slot) = state.notification_persister.write() {
        *slot = Some(persister);
    }
}

/// Overrides Telegram's API root. This is intentionally only an in-process
/// setting so tests can use a local stub; it has no admin endpoint.
pub fn set_telegram_base(state: &Arc<AppState>, base: impl Into<String>) {
    if let Ok(mut current) = state.telegram_base.write() {
        *current = base.into();
    }
}

pub fn set_daemon_updater(state: &Arc<AppState>, updater: Arc<dyn DaemonUpdater>) {
    if let Ok(mut slot) = state.daemon_updater.write() {
        *slot = Some(updater);
    }
}

/// Installs the daemon-owned control for reading and persisting the update
/// channel. Shared with the CLI `--set-channel` path so the app picker and the
/// daemon can never end up on different channels.
pub fn set_update_channel_controller(
    state: &Arc<AppState>,
    controller: Arc<dyn UpdateChannelController>,
) {
    if let Ok(mut slot) = state.update_channel_controller.write() {
        *slot = Some(controller);
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
        .route(
            "/admin/fixtures",
            get(admin_fixtures).post(admin_fixture_save),
        )
        .route(
            "/admin/fixtures/{name}",
            get(admin_fixture_get).delete(admin_fixture_delete),
        )
        .route(
            "/admin/sessions/{session_id}/inject",
            post(admin_session_inject),
        )
        .route(
            "/admin/sessions/{session_id}/injections",
            get(admin_session_injections).delete(admin_session_injections_clear),
        )
        .route("/admin/accounts", get(admin_accounts))
        .route("/admin/providers", get(admin_providers))
        .route(
            "/admin/providers/{provider}/pause",
            post(admin_provider_pause),
        )
        .route(
            "/admin/providers/{provider}/resume",
            post(admin_provider_resume),
        )
        .route("/admin/accounts/analytics", get(admin_account_analytics))
        .route("/admin/accounts/merge", post(admin_account_merge))
        .route(
            "/admin/routing/{provider}",
            get(admin_routing).put(admin_routing_update),
        )
        .route(
            "/admin/protection",
            get(admin_protection).put(admin_protection_update),
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
        .route("/admin/exo", get(admin_exo).put(admin_exo_update))
        .route("/admin/exo/status", get(admin_exo_status))
        .route("/admin/exo/models", get(admin_exo_models))
        .route(
            "/admin/notifications",
            get(admin_notifications).post(admin_notifications_save),
        )
        .route(
            "/admin/notifications/{id}",
            delete(admin_notifications_delete),
        )
        .route(
            "/admin/notifications/validate",
            post(admin_notifications_validate),
        )
        .route(
            "/admin/notifications/discover-chat",
            post(admin_notifications_discover_chat),
        )
        .route("/admin/notifications/test", post(admin_notifications_test))
        .route("/admin/analytics", get(admin_analytics))
        .route("/admin/limits", get(admin_limits))
        .route("/admin/update", get(admin_update).post(admin_update_apply))
        .route(
            "/admin/update/channel",
            get(admin_update_channel).post(admin_update_channel_set),
        )
        .route("/admin/dario", get(admin_dario))
        .route("/admin/dario/ping", post(admin_dario_ping))
        .route("/admin/dario/prompt-caches", get(admin_dario_prompt_caches))
        .route(
            "/admin/dario/prompt-caches/{key}",
            axum::routing::delete(admin_dario_prompt_cache_delete),
        )
        .route("/admin/auth/import", post(admin_auth_import))
        .route("/admin/vault/export", post(admin_vault_export))
        .route("/admin/credentials", get(admin_credentials))
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
        .route("/tools/{id}/body/{kind}", get(tool_body))
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
    // Pi's authoritative extension hooks use the same harness capability as
    // lifecycle events. Authentication happens before JSON buffering.
    let tool_events = Router::new()
        .route("/tool-events", post(tool_event))
        .layer(axum::extract::DefaultBodyLimit::max(MAX_INGEST_BODY_BYTES))
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
        .merge(tool_events)
        .merge(gated)
        .layer(axum::extract::DefaultBodyLimit::max(64 * 1024 * 1024))
        .with_state(state)
}

async fn admin_notifications(State(state): State<Arc<AppState>>) -> Response {
    let view = state
        .notifications
        .read()
        .map(|notifications| notifications.admin_view())
        .unwrap_or_else(|_| json!({"channels": [], "error": "notification status unavailable"}));
    axum::Json(view).into_response()
}

#[derive(Debug, Deserialize)]
struct NotificationChannelRequest {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    format: notify::WebhookFormat,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    token: Option<String>,
    #[serde(default)]
    chat_id: Option<String>,
    #[serde(default)]
    min_level: notify::NotificationLevel,
    #[serde(default)]
    categories: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct TelegramTokenRequest {
    format: notify::WebhookFormat,
    token: String,
}

fn notification_channel_from_request(
    request: NotificationChannelRequest,
    require_delivery_target: bool,
) -> std::result::Result<notify::NotificationChannelConfig, String> {
    let kind = request.kind.unwrap_or_else(|| "webhook".into());
    if kind != "webhook" {
        return Err("only webhook notification channels are supported".into());
    }
    let token = request
        .token
        .filter(|value| !value.trim().is_empty())
        .map(|value| value.trim().to_owned());
    let url = request.url.unwrap_or_default().trim().to_owned();
    if matches!(request.format, notify::WebhookFormat::Telegram) {
        if token.is_none() {
            return Err("telegram notification channels require token".into());
        }
        if require_delivery_target
            && request
                .chat_id
                .as_deref()
                .unwrap_or_default()
                .trim()
                .is_empty()
        {
            return Err("telegram notification channels require chat_id".into());
        }
    } else if require_delivery_target {
        if url.is_empty() {
            return Err("webhook notification channels require url".into());
        }
        let parsed = reqwest::Url::parse(&url).map_err(|_| "webhook url is invalid")?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err("webhook url must use http or https".into());
        }
    }
    Ok(notify::NotificationChannelConfig {
        id: request
            .id
            .filter(|value| !value.trim().is_empty())
            .map(|value| value.trim().to_owned()),
        kind,
        format: request.format,
        url,
        token,
        bot_username: None,
        chat_id: request
            .chat_id
            .filter(|value| !value.trim().is_empty())
            .map(|value| value.trim().to_owned()),
        min_level: request.min_level,
        categories: request.categories,
    })
}

fn notification_persister(state: &AppState) -> Option<Arc<dyn NotificationConfigPersister>> {
    state
        .notification_persister
        .read()
        .ok()
        .and_then(|slot| slot.clone())
}

fn persist_and_apply_notifications(
    state: &Arc<AppState>,
    settings: notify::NotificationSettings,
) -> std::result::Result<(), Response> {
    let Some(persister) = notification_persister(state) else {
        return Err(error_response(
            StatusCode::NOT_IMPLEMENTED,
            "notification persistence is not configured",
        ));
    };
    if let Err(error) = persister.persist(&settings) {
        return Err(error_response(StatusCode::INTERNAL_SERVER_ERROR, &error));
    }
    set_notifications(state, settings);
    Ok(())
}

fn telegram_endpoint(base: &str, token: &str, method: &str) -> String {
    format!("{}/bot{token}/{method}", base.trim_end_matches('/'))
}

async fn telegram_get_me(
    state: &AppState,
    token: &str,
) -> std::result::Result<(String, String), ()> {
    let base = state
        .telegram_base
        .read()
        .map(|base| base.clone())
        .unwrap_or_else(|_| "https://api.telegram.org".into());
    let response = state
        .http
        .get(telegram_endpoint(&base, token, "getMe"))
        .timeout(Duration::from_secs(8))
        .send()
        .await
        .map_err(|_| ())?;
    if !response.status().is_success() {
        return Err(());
    }
    let body: Value = response.json().await.map_err(|_| ())?;
    if body["ok"] != Value::Bool(true) {
        return Err(());
    }
    let username = body["result"]["username"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    if username.is_empty() {
        return Err(());
    }
    let name = body["result"]["first_name"]
        .as_str()
        .unwrap_or(&username)
        .to_owned();
    Ok((username, name))
}

async fn admin_notifications_save(
    State(state): State<Arc<AppState>>,
    axum::Json(request): axum::Json<NotificationChannelRequest>,
) -> Response {
    let mut channel = match notification_channel_from_request(request, true) {
        Ok(channel) => channel,
        Err(error) => return error_response(StatusCode::BAD_REQUEST, &error),
    };
    if matches!(channel.format, notify::WebhookFormat::Telegram) {
        let token = channel.token.as_deref().expect("validated above");
        match telegram_get_me(&state, token).await {
            Ok((username, _)) => channel.bot_username = Some(username),
            Err(()) => {
                return axum::Json(json!({"ok": false, "error": "telegram validation failed"}))
                    .into_response()
            }
        }
    }
    let mut settings = match state.notification_settings.read() {
        Ok(settings) => settings.clone(),
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "notification settings unavailable",
            )
        }
    };
    let id = channel
        .id
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().simple().to_string());
    channel.id = Some(id.clone());
    if let Some(index) = settings
        .channels
        .iter()
        .position(|existing| existing.id.as_deref() == Some(id.as_str()))
    {
        settings.channels[index] = channel;
    } else {
        settings.channels.push(channel);
    }
    if let Err(response) = persist_and_apply_notifications(&state, settings) {
        return response;
    }
    let view = state
        .notifications
        .read()
        .ok()
        .map(|dispatcher| dispatcher.admin_view())
        .and_then(|view| view["channels"].as_array().cloned())
        .and_then(|channels| channels.into_iter().find(|saved| saved["id"] == id));
    axum::Json(json!({"ok": true, "channel": view})).into_response()
}

async fn admin_notifications_delete(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    let mut settings = match state.notification_settings.read() {
        Ok(settings) => settings.clone(),
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "notification settings unavailable",
            )
        }
    };
    let Some(index) = settings
        .channels
        .iter()
        .position(|channel| channel.id.as_deref() == Some(id.as_str()))
    else {
        return error_response(StatusCode::NOT_FOUND, "notification channel not found");
    };
    settings.channels.remove(index);
    if let Err(response) = persist_and_apply_notifications(&state, settings) {
        return response;
    }
    axum::Json(json!({"ok": true, "id": id})).into_response()
}

async fn admin_notifications_validate(
    State(state): State<Arc<AppState>>,
    axum::Json(request): axum::Json<TelegramTokenRequest>,
) -> Response {
    if !matches!(request.format, notify::WebhookFormat::Telegram) || request.token.trim().is_empty()
    {
        return error_response(
            StatusCode::BAD_REQUEST,
            "validate requires format telegram and token",
        );
    }
    match telegram_get_me(&state, request.token.trim()).await {
        Ok((bot_username, bot_name)) => {
            axum::Json(json!({"ok": true, "bot_username": bot_username, "bot_name": bot_name}))
                .into_response()
        }
        Err(()) => {
            axum::Json(json!({"ok": false, "error": "telegram validation failed"})).into_response()
        }
    }
}

async fn admin_notifications_discover_chat(
    State(state): State<Arc<AppState>>,
    axum::Json(request): axum::Json<TelegramTokenRequest>,
) -> Response {
    if !matches!(request.format, notify::WebhookFormat::Telegram) || request.token.trim().is_empty()
    {
        return error_response(
            StatusCode::BAD_REQUEST,
            "discover-chat requires format telegram and token",
        );
    }
    let base = state
        .telegram_base
        .read()
        .map(|base| base.clone())
        .unwrap_or_else(|_| "https://api.telegram.org".into());
    let response = state
        .http
        .get(telegram_endpoint(&base, request.token.trim(), "getUpdates"))
        .timeout(Duration::from_secs(8))
        .send()
        .await;
    let Ok(response) = response else {
        return axum::Json(json!({"ok": false, "error": "telegram discovery failed"}))
            .into_response();
    };
    if !response.status().is_success() {
        return axum::Json(json!({"ok": false, "error": "telegram discovery failed"}))
            .into_response();
    }
    let Ok(body) = response.json::<Value>().await else {
        return axum::Json(json!({"ok": false, "error": "telegram discovery failed"}))
            .into_response();
    };
    let mut chats = Vec::new();
    let mut seen = HashSet::new();
    for update in body["result"].as_array().into_iter().flatten() {
        for key in ["message", "edited_message", "channel_post"] {
            let chat = &update[key]["chat"];
            let Some(chat_id) = chat["id"].as_i64() else {
                continue;
            };
            if !seen.insert(chat_id) {
                continue;
            }
            let chat_name = chat["title"]
                .as_str()
                .or_else(|| chat["username"].as_str())
                .map(str::to_owned)
                .or_else(|| {
                    let first = chat["first_name"].as_str()?;
                    let last = chat["last_name"].as_str().unwrap_or_default();
                    Some(format!("{first} {last}").trim().to_owned())
                })
                .unwrap_or_else(|| chat_id.to_string());
            chats.push(json!({"chat_id": chat_id.to_string(), "chat_name": chat_name}));
        }
    }
    axum::Json(json!({"ok": true, "chats": chats})).into_response()
}

async fn admin_notifications_test(
    State(state): State<Arc<AppState>>,
    body: axum::Json<Value>,
) -> Response {
    let channel = match body.0.get("channel") {
        None | Some(Value::Null) => None,
        Some(value) => match value.as_u64().and_then(|value| usize::try_from(value).ok()) {
            Some(index) => Some(index),
            None => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "channel must be a non-negative index",
                )
            }
        },
    };
    let inline = body.0.get("format").is_some()
        || body.0.get("url").is_some()
        || body.0.get("token").is_some();
    if inline {
        let request = match serde_json::from_value::<NotificationChannelRequest>(body.0) {
            Ok(request) => request,
            Err(_) => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "invalid inline notification channel",
                )
            }
        };
        let channel = match notification_channel_from_request(request, true) {
            Ok(channel) => channel,
            Err(error) => return error_response(StatusCode::BAD_REQUEST, &error),
        };
        let dispatcher =
            notify::NotificationDispatcher::from_settings(notify::NotificationSettings {
                channels: vec![channel],
                ..Default::default()
            });
        return axum::Json(json!({"channels": dispatcher.test(None, now_ms()).await}))
            .into_response();
    }
    let dispatcher = match state.notifications.read() {
        Ok(notifications) => notifications.clone(),
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "notification dispatcher unavailable",
            )
        }
    };
    axum::Json(json!({"channels": dispatcher.test(channel, now_ms()).await})).into_response()
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
    let handler = state
        .reset_handler
        .read()
        .ok()
        .and_then(|slot| slot.clone());
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

#[derive(serde::Deserialize)]
struct VaultExportRequest {
    passphrase: Option<String>,
    #[serde(default)]
    selection: BundleSelection,
}

async fn admin_vault_export(
    State(state): State<Arc<AppState>>,
    body: Option<axum::Json<VaultExportRequest>>,
) -> Response {
    let Some(request) = body.map(|v| v.0) else {
        return error_response(StatusCode::BAD_REQUEST, "passphrase is required");
    };
    let Some(passphrase) = request.passphrase.filter(|p| !p.is_empty()) else {
        return error_response(StatusCode::BAD_REQUEST, "passphrase is required");
    };
    match export_bundle(&state.vault, request.selection)
        .await
        .and_then(|bundle| encrypt_bundle(&bundle, &passphrase))
    {
        Ok(blob) => axum::Json(blob).into_response(),
        Err(e) => error_response(StatusCode::BAD_REQUEST, &e.to_string()),
    }
}

async fn admin_credentials(State(state): State<Arc<AppState>>) -> Response {
    let now = now_ms();
    let mut outbound = Vec::new();
    for account in state.vault.list().await {
        let present = account
            .access_token
            .as_deref()
            .is_some_and(|v| !v.is_empty())
            || account
                .refresh_token
                .as_deref()
                .is_some_and(|v| !v.is_empty())
            || account.api_key.as_deref().is_some_and(|v| !v.is_empty());
        let active = present
            && !account.paused
            && account.status == "active"
            && account
                .cooldown_until_ms
                .map(|until| until <= now)
                .unwrap_or(true)
            && (account.kind != "oauth"
                || account
                    .expires_at_ms
                    .map(|expires| expires > now)
                    .unwrap_or(true));
        outbound.push(json!({"kind": account.kind, "id": account.id, "provider": account.provider.as_str(), "present": present, "active": active, "identity": account.email(), "expires_at_ms": account.expires_at_ms, "source": "vault"}));
    }
    for harness in alex_auth::vault_bundle::HARNESS_NAMES {
        let paths = harness_cred_paths(harness);
        let present = !paths.is_empty() && paths.iter().all(|(_, path)| path.exists());
        outbound.push(json!({"kind": "harness_login", "name": harness, "present": present, "active": present, "identity": Value::Null, "expires_at_ms": Value::Null, "source": "harness_file"}));
    }
    let run_keys = match state.store.list_run_keys(true) {
        Ok(keys) => keys,
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    axum::Json(json!({"outbound": outbound, "inbound": {"admin_key": {"present": !state.local_key.read().map(|v| v.is_empty()).unwrap_or(true)}, "local_key": {"present": !state.local_key.read().map(|v| v.is_empty()).unwrap_or(true)}, "run_keys": run_keys}})).into_response()
}

async fn admin_account_remove(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    let account = state.vault.list().await.into_iter().find(|a| a.id == id);
    let Some(account) = account else {
        return error_response(StatusCode::NOT_FOUND, &format!("unknown account '{id}'"));
    };
    if let Err(e) = state
        .store
        .tombstone_known_account(&known_account(&account), now_ms())
    {
        // Do not remove credentials if we could not first preserve attribution.
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("could not preserve removed account history: {e}"),
        );
    }
    match state.vault.remove(&id).await {
        Ok(true) => axum::Json(json!({"removed": id})).into_response(),
        Ok(false) => error_response(StatusCode::NOT_FOUND, &format!("unknown account '{id}'")),
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

#[derive(serde::Deserialize)]
struct AccountMergeRequest {
    from: String,
    into: String,
    #[serde(default)]
    allow_mismatch: bool,
}

/// Unify a duplicate same-email account into a survivor, keeping BOTH histories.
///
/// Order matters for atomicity: validate first (read-only), then re-key the
/// trace database in one transaction, then move the credential and tombstone the
/// dup. If the credential step ever failed after the DB step, the survivor
/// already owns every re-keyed row and re-running the merge is a safe, idempotent
/// no-op on the database that finishes the credential move.
async fn admin_account_merge(
    State(state): State<Arc<AppState>>,
    body: axum::Json<AccountMergeRequest>,
) -> Response {
    let AccountMergeRequest {
        from,
        into,
        allow_mismatch,
    } = body.0;
    if from.trim().is_empty() || into.trim().is_empty() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "both 'from' and 'into' are required",
        );
    }
    // 1. Validate credentials-side before touching any history.
    let (_from_account, into_account) = match state
        .vault
        .validate_merge(&from, &into, allow_mismatch)
        .await
    {
        Ok(pair) => pair,
        Err(e) => {
            let status = if e.to_string().starts_with("unknown account") {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::BAD_REQUEST
            };
            return error_response(status, &e.to_string());
        }
    };
    // Ensure the survivor has a durable catalogue row so re-keyed traces resolve.
    if let Err(e) = state
        .store
        .upsert_known_account(&known_account(&into_account))
    {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    // 2. Re-key every trace/heartbeat/catalogue reference in one transaction.
    let counts = match state.store.merge_accounts(&from, &into) {
        Ok(counts) => counts,
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("could not reassign account history: {e}"),
            )
        }
    };
    // 3. Move the surviving credential and tombstone the duplicate login.
    let outcome = match state.vault.merge_accounts(&from, &into, true).await {
        Ok(outcome) => outcome,
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!(
                    "history was re-keyed to '{into}' but removing the duplicate login failed: {e}"
                ),
            )
        }
    };
    let rows = serde_json::to_value(&counts).unwrap_or(Value::Null);
    axum::Json(json!({
        "merged_into": outcome.survivor_id,
        "removed": outcome.removed_id,
        "adopted_credentials_from": outcome.adopted_credentials_from,
        "rows": rows,
    }))
    .into_response()
}

fn known_account(account: &Account) -> KnownAccount {
    KnownAccount::new(
        account.id.clone(),
        account.provider.as_str(),
        account.name.clone(),
        account.kind.clone(),
        account.subscription_identity(),
        account.email(),
    )
}

fn known_removed_account(account: &RemovedAccount) -> KnownAccount {
    KnownAccount::new(
        account.id.clone(),
        account.provider.as_str(),
        account.name.clone(),
        account.kind.clone(),
        account.subscription_identity.clone(),
        account.email.clone(),
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
            None => return error_response(StatusCode::BAD_REQUEST, "'remove' must be boolean"),
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
            Err(error) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
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
    match state
        .logins
        .start(state.vault.clone(), provider, name)
        .await
    {
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
        "dario": state.dario.as_ref().and_then(|dario| dario.active()).is_some(),
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

fn normalize_exo_config(mut config: ExoConfig) -> Result<ExoConfig, String> {
    let parsed = reqwest::Url::parse(config.url.trim())
        .map_err(|_| "Exo URL must be an absolute http:// or https:// URL".to_string())?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(
            "Exo URL must be an absolute http:// or https:// endpoint without query or fragment"
                .into(),
        );
    }
    config.url = config.url.trim_end_matches('/').to_string();
    let mut seen = HashSet::new();
    config.enabled_models = config
        .enabled_models
        .into_iter()
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty() && id.len() <= 512 && seen.insert(id.clone()))
        .collect();
    if config.enabled_models.len() > 500 {
        return Err("at most 500 Exo models may be enabled".into());
    }
    Ok(config)
}

async fn exo_model_payload(state: &AppState) -> Result<Value, String> {
    let config = state
        .exo
        .read()
        .map_err(|_| "Exo settings are unavailable".to_string())?
        .clone();
    let response = state
        .http
        .get(format!("{}/v1/models", config.url))
        .header("authorization", "Bearer x")
        .timeout(Duration::from_secs(3))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err(format!("Exo returned HTTP {}", response.status()));
    }
    response
        .json::<Value>()
        .await
        .map_err(|error| format!("invalid Exo models response: {error}"))
}

fn exo_models_array(payload: &Value) -> Vec<Value> {
    payload
        .get("data")
        .or_else(|| payload.get("models"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

async fn admin_exo(State(state): State<Arc<AppState>>) -> Response {
    match state.exo.read() {
        Ok(config) => axum::Json(config.clone()).into_response(),
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Exo settings are unavailable",
        ),
    }
}

async fn admin_exo_update(
    State(state): State<Arc<AppState>>,
    axum::Json(body): axum::Json<ExoConfig>,
) -> Response {
    let config = match normalize_exo_config(body) {
        Ok(config) => config,
        Err(error) => return error_response(StatusCode::BAD_REQUEST, &error),
    };
    let persister = state
        .exo_persister
        .read()
        .ok()
        .and_then(|slot| slot.clone());
    let Some(persister) = persister else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "Exo settings persistence is unavailable",
        );
    };
    if let Err(error) = persister.persist(&config) {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, &error);
    }
    set_exo_config(&state, config.clone());
    axum::Json(config).into_response()
}

async fn admin_exo_status(State(state): State<Arc<AppState>>) -> Response {
    let config = match state.exo.read() {
        Ok(config) => config.clone(),
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Exo settings are unavailable",
            )
        }
    };
    match exo_model_payload(&state).await {
        Ok(payload) => axum::Json(json!({"running": true, "url": config.url, "model_count": exo_models_array(&payload).len()})).into_response(),
        Err(error) => axum::Json(json!({"running": false, "url": config.url, "model_count": 0, "error": error})).into_response(),
    }
}

async fn admin_exo_models(State(state): State<Arc<AppState>>) -> Response {
    let enabled = state
        .exo
        .read()
        .map(|config| config.enabled_models.clone())
        .unwrap_or_default();
    match exo_model_payload(&state).await {
        Ok(payload) => {
            // Some Exo builds list every downloadable model and publish the
            // currently loaded subset separately. Preserve that distinction
            // when it is present; otherwise `running` remains omitted.
            let running_ids: Option<HashSet<String>> = payload
                .get("running_models")
                .or_else(|| payload.get("loaded_models"))
                .and_then(Value::as_array)
                .map(|models| {
                    models
                        .iter()
                        .filter_map(|model| {
                            model
                                .as_str()
                                .or_else(|| model.get("id").and_then(Value::as_str))
                                .map(String::from)
                        })
                        .collect()
                });
            let models: Vec<Value> = exo_models_array(&payload)
                .into_iter()
                .filter_map(|model| {
                    let id = model.get("id").and_then(Value::as_str)?.to_string();
                    let running = model
                        .get("running")
                        .and_then(Value::as_bool)
                        .or_else(|| {
                            model
                                .get("status")
                                .and_then(Value::as_str)
                                .map(|status| status.eq_ignore_ascii_case("running"))
                        })
                        .or_else(|| running_ids.as_ref().map(|ids| ids.contains(&id)));
                    let mut out = serde_json::Map::new();
                    out.insert("id".into(), json!(id));
                    out.insert(
                        "name".into(),
                        json!(model
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_else(|| model["id"].as_str().unwrap_or(""))),
                    );
                    for (output, candidates) in [
                        ("family", &["family", "architecture"][..]),
                        ("quantization", &["quantization", "quantization_level"][..]),
                        (
                            "context_length",
                            &["context_length", "context_window", "max_context_length"][..],
                        ),
                    ] {
                        if let Some(value) = candidates
                            .iter()
                            .find_map(|key| model.get(*key))
                            .filter(|value| !value.is_null())
                        {
                            if output == "context_length" {
                                if let Some(length) = value
                                    .as_u64()
                                    .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
                                {
                                    out.insert(output.into(), json!(length));
                                }
                            } else {
                                out.insert(output.into(), value.clone());
                            }
                        }
                    }
                    out.insert(
                        "enabled".into(),
                        json!(enabled.iter().any(|value| value == &id)),
                    );
                    if let Some(running) = running {
                        out.insert("running".into(), json!(running));
                    }
                    Some(Value::Object(out))
                })
                .collect();
            axum::Json(json!({"models": models})).into_response()
        }
        Err(error) => error_response(StatusCode::BAD_GATEWAY, &error),
    }
}

async fn models(State(state): State<Arc<AppState>>, headers: HeaderMap) -> impl IntoResponse {
    // OpenRouter is the sole dynamic provider catalog. Refresh only on an
    // explicit model-list request; Alexandria has no catalog refresh worker.
    refresh_openrouter_models(&state).await;
    let mut ids = state.store.pricing_models();
    if let Ok(models) = state.openrouter_models.lock() {
        ids.extend(models.iter().map(|id| format!("openrouter/{id}")));
    }
    ids.extend(exo_catalog_models(&state));
    ids.extend(kimi_catalog_models(&state).await);
    for (alias, _) in alex_core::model_aliases() {
        ids.push((*alias).to_string());
    }
    let claude_gateway = headers
        .get_all("x-alexandria-harness")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .any(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "claude" | "claude-code"
            )
        });
    if !claude_gateway {
        for id in ids.clone() {
            ids.push(format!("alexandria/{id}"));
        }
    }
    let mut seen = HashSet::new();
    let data: Vec<Value> = ids
        .into_iter()
        .filter(|id| seen.insert(id.clone()))
        .map(|m| {
            if claude_gateway {
                json!({
                    "id": format!("claude-alex/{m}"),
                    "display_name": format!("alex/{m}"),
                    "object": "model",
                    "owned_by": "alexandria",
                })
            } else {
                json!({"id": m, "object": "model", "owned_by": "alexandria"})
            }
        })
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
    let since_ms = now_ms() - minutes * 60_000;
    let bucket_ms = bucket_minutes * 60_000;
    match state.store.account_analytics(since_ms, bucket_ms) {
        Ok(mut v) => {
            // Keep the legacy sparse point list in `series`; `plot_series`
            // is deliberately additive for older menu-bar clients. New
            // clients can draw it directly without grouping or zero-filling.
            let (plot_series, x_labels, bucket_count) =
                account_plot_series(&v, since_ms, bucket_ms.max(60_000), now_ms());
            v["plot_series"] = plot_series;
            v["x_labels"] = json!(x_labels);
            v["bucket_count"] = json!(bucket_count);
            axum::Json(v).into_response()
        }
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

/// Convert sparse aggregate points into the exact rectangular data shape a
/// chart needs. This is intentionally pure so the boundary/zero-fill behavior
/// is covered by unit tests and does not drift back into SwiftUI view bodies.
fn account_plot_series(
    response: &Value,
    since_ms: i64,
    bucket_ms: i64,
    now_ms: i64,
) -> (Value, Vec<String>, usize) {
    let first = (since_ms / bucket_ms) * bucket_ms;
    let last = (now_ms / bucket_ms) * bucket_ms;
    let buckets: Vec<i64> = if last < first {
        Vec::new()
    } else {
        (first..=last).step_by(bucket_ms as usize).collect()
    };
    let x_labels = buckets
        .iter()
        .map(|ms| {
            let date = Utc.timestamp_millis_opt(*ms).single().unwrap_or_default();
            if bucket_ms <= 60 * 60 * 1_000 {
                date.format("%H:%M").to_string()
            } else {
                date.format("%b %-d").to_string()
            }
        })
        .collect::<Vec<_>>();
    let indices: HashMap<i64, usize> = buckets
        .iter()
        .enumerate()
        .map(|(index, bucket)| (*bucket, index))
        .collect();
    let mut names: HashMap<String, String> = response["by_account"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|account| {
            let id = account["account_id"].as_str()?.to_string();
            Some((id.clone(), id))
        })
        .collect();
    let mut values: HashMap<String, Vec<f64>> = HashMap::new();
    for point in response["series"].as_array().into_iter().flatten() {
        let Some(account_id) = point["account_id"].as_str() else {
            continue;
        };
        let Some(bucket) = point["bucket_ms"].as_i64() else {
            continue;
        };
        let Some(index) = indices.get(&bucket) else {
            continue;
        };
        names
            .entry(account_id.to_string())
            .or_insert_with(|| account_id.to_string());
        let entry = values
            .entry(account_id.to_string())
            .or_insert_with(|| vec![0.0; buckets.len()]);
        entry[*index] = point["input_tokens"].as_f64().unwrap_or(0.0)
            + point["output_tokens"].as_f64().unwrap_or(0.0);
    }
    let mut ids: Vec<_> = values.keys().cloned().collect();
    ids.sort();
    let plot_series = ids
        .into_iter()
        .map(|account_id| {
            json!({
                "account_id": account_id,
                "name": names.remove(&account_id).unwrap_or(account_id.clone()),
                "values": values.remove(&account_id).unwrap_or_default(),
            })
        })
        .collect::<Vec<_>>();
    (json!(plot_series), x_labels, buckets.len())
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
                        "credits": {
                            "has_credits": snap.used_percent < 100.0,
                            "unlimited": false,
                            "used_pct": snap.used_percent,
                        },
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
    for entry in &mut providers {
        if let Some(provider) = entry["provider"]
            .as_str()
            .and_then(Provider::from_str_loose)
        {
            let quota = quota_state(provider, entry);
            if let Some(object) = entry.as_object_mut() {
                object.insert("quota".into(), quota);
            }
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

async fn admin_update_channel(State(state): State<Arc<AppState>>) -> Response {
    let controller = state
        .update_channel_controller
        .read()
        .ok()
        .and_then(|slot| slot.clone());
    let Some(controller) = controller else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "update channel control is unavailable",
        );
    };
    axum::Json(json!({ "channel": controller.current() })).into_response()
}

#[derive(Deserialize)]
struct UpdateChannelRequest {
    channel: String,
}

async fn admin_update_channel_set(
    State(state): State<Arc<AppState>>,
    axum::Json(body): axum::Json<UpdateChannelRequest>,
) -> Response {
    let controller = state
        .update_channel_controller
        .read()
        .ok()
        .and_then(|slot| slot.clone());
    let Some(controller) = controller else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "update channel control is unavailable",
        );
    };
    match controller.set(body.channel).await {
        Ok(outcome) => {
            let mut response = json!({ "channel": outcome.channel });
            if let Some(status) = outcome.status {
                // Hot-apply: the next `/admin/update` reflects the new channel
                // immediately, without waiting for the periodic background check.
                *state.update_status.write().await = Some(status.clone());
                if let (Some(obj), Value::Object(fields)) = (response.as_object_mut(), status) {
                    obj.extend(fields);
                }
            }
            axum::Json(response).into_response()
        }
        Err(UpdateChannelError::Invalid(message)) => {
            error_response(StatusCode::BAD_REQUEST, &message)
        }
        Err(UpdateChannelError::Failed(message)) => {
            error_response(StatusCode::INTERNAL_SERVER_ERROR, &message)
        }
    }
}

async fn admin_dario(State(state): State<Arc<AppState>>) -> Response {
    match &state.dario {
        Some(d) => {
            let mut status = d.status();
            let anthropic_credentials_present =
                state.vault.list().await.into_iter().any(|account| {
                    account.provider == Provider::Anthropic && account.status == "active"
                });
            let anthropic_oauth_present = state.vault.list().await.into_iter().any(|account| {
                account.provider == Provider::Anthropic
                    && account.status == "active"
                    && account.kind == "oauth"
            });
            let generation_ready = d.active().is_some();
            let probe_succeeds =
                status["generations"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|generation| {
                        generation["id"].as_str() == status["active_generation_id"].as_str()
                            && generation["last_probe"]["ok"].as_bool() == Some(true)
                    });
            let prompt_cache_degraded = !status["health_reason"].is_null();
            let probe_returned_401 = dario_probe_returned_401(&status);
            let should_be_healthy =
                status["route_enabled"].as_bool().unwrap_or(false) && anthropic_oauth_present;
            if let Some(obj) = status.as_object_mut() {
                obj.insert("prompt_caches".into(), json!(dario_prompt_caches(&state)));
                let generation_health = dario_health_state(
                    anthropic_credentials_present,
                    generation_ready,
                    probe_succeeds,
                );
                // A failed prompt capture is actionable even if the child
                // socket/probe remains healthy; do not overwrite it with the
                // coarser connection health state.
                if !prompt_cache_degraded {
                    obj.insert("health".into(), json!(generation_health));
                }
                obj.insert("generation_health".into(), json!(generation_health));
                obj.insert(
                    "anthropic_credentials_present".into(),
                    json!(anthropic_credentials_present),
                );
                obj.insert("should_be_healthy".into(), json!(should_be_healthy));
                if probe_returned_401 {
                    obj.insert(
                        "issue".into(),
                        json!({"code": "reauth", "message": "Claude Code login needs re-auth", "fixable": true}),
                    );
                } else if !anthropic_oauth_present {
                    obj.insert(
                        "issue".into(),
                        json!({"code": "no_anthropic_creds", "message": "no active Anthropic OAuth credentials", "fixable": false}),
                    );
                }
                obj.insert("ping_path".into(), json!("/admin/dario/ping"));
            }
            axum::Json(status).into_response()
        }
        None => error_response(StatusCode::NOT_FOUND, "dario mode is not enabled"),
    }
}

fn dario_probe_returned_401(status: &Value) -> bool {
    let active_id = status["active_generation_id"].as_str();
    status["generations"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|generation| {
            generation["id"].as_str() == active_id
                && generation["last_probe"]["status"].as_u64() == Some(401)
        })
}

async fn admin_dario_ping(State(state): State<Arc<AppState>>) -> Response {
    let (health, ping) = ping_dario(&state, "claude-haiku-4-5").await;
    let status = if health == DarioHealthState::Down {
        StatusCode::BAD_GATEWAY
    } else {
        StatusCode::OK
    };
    (
        status,
        axum::Json(json!({
            "health": health,
            "generation_ready": state.dario.as_ref().and_then(|dario| dario.active()).is_some(),
            "through_dario": ping,
        })),
    )
        .into_response()
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
    // Keep the small `/admin/traces` fixture endpoint useful to harness
    // certification: a scoped run key gives every request a unique run id.
    // `list_traces` predates run keys, so delegate only run-id queries to the
    // richer filter API while retaining its established response shape.
    if q.contains_key("run_id") || q.contains_key("error_class") || q.contains_key("errors") {
        let mut filter = filter_from_query(&q);
        filter.limit = limit;
        return match state.store.search_traces(&filter) {
            Ok(rows) => {
                axum::Json(json!({"traces": trace_rows_with_display_fields(rows)})).into_response()
            }
            Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
        };
    }
    match state.store.list_traces(
        limit,
        q.get("session").map(|s| s.as_str()),
        q.get("model").map(|s| s.as_str()),
    ) {
        Ok(rows) => {
            axum::Json(json!({"traces": trace_rows_with_display_fields(rows)})).into_response()
        }
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

fn fixture_root(state: &AppState) -> Result<PathBuf, String> {
    state
        .fixture_dir
        .read()
        .ok()
        .and_then(|slot| slot.clone())
        .ok_or_else(|| "fixture storage is not configured".into())
}

fn fixture_path(root: &std::path::Path, name: &str) -> Result<PathBuf, String> {
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err("fixture name may contain only letters, digits, '-' and '_'".into());
    }
    Ok(root.join(format!("{name}.json")))
}

fn starter_fixtures(root: &std::path::Path) -> Result<(), String> {
    std::fs::create_dir_all(root).map_err(|e| e.to_string())?;
    if std::fs::read_dir(root)
        .map_err(|e| e.to_string())?
        .next()
        .is_some()
    {
        return Ok(());
    }
    let rows = [
        (
            "anthropic-relogin-401",
            "anthropic",
            401,
            "authentication_error",
            r#"{"type":"error","error":{"type":"authentication_error","message":"Invalid authentication credentials"}}"#,
        ),
        (
            "anthropic-overloaded-429",
            "anthropic",
            429,
            "rate_limit_error",
            r#"{"type":"error","error":{"type":"rate_limit_error","message":"Overloaded"}}"#,
        ),
        (
            "upstream-503",
            "unknown",
            503,
            "http_status_503",
            r#"{"error":{"type":"http_status_503","message":"Upstream unavailable"}}"#,
        ),
        (
            "openai-capacity-429",
            "openai",
            429,
            "rate_limit_error",
            r#"{"error":{"type":"rate_limit_error","message":"Capacity exceeded"}}"#,
        ),
    ];
    for (name, provider, status, error_kind, body) in rows {
        let fixture = ErrorFixture {
            name: name.into(),
            provider: provider.into(),
            status,
            error_kind: error_kind.into(),
            body: body.into(),
            direction: Direction::UpstreamToClient,
            created_ms: now_ms(),
            source_trace_id: None,
        };
        let path = fixture_path(root, name)?;
        std::fs::write(
            path,
            serde_json::to_vec_pretty(&fixture).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn load_fixture(state: &AppState, name: &str) -> Result<ErrorFixture, String> {
    let root = fixture_root(state)?;
    starter_fixtures(&root)?;
    let path = fixture_path(&root, name)?;
    serde_json::from_slice(&std::fs::read(path).map_err(|_| format!("fixture '{name}' not found"))?)
        .map_err(|e| e.to_string())
}

async fn admin_fixtures(State(state): State<Arc<AppState>>) -> Response {
    let root = match fixture_root(&state).and_then(|root| {
        starter_fixtures(&root)?;
        Ok(root)
    }) {
        Ok(root) => root,
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e),
    };
    let mut fixtures = Vec::new();
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    for entry in entries.flatten() {
        if entry.path().extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        if let Ok(fixture) =
            serde_json::from_slice::<ErrorFixture>(&std::fs::read(entry.path()).unwrap_or_default())
        {
            fixtures.push(fixture);
        }
    }
    fixtures.sort_by(|a, b| a.name.cmp(&b.name));
    axum::Json(json!({"fixtures": fixtures})).into_response()
}

async fn admin_fixture_get(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Response {
    match load_fixture(&state, &name) {
        Ok(fixture) => axum::Json(fixture).into_response(),
        Err(e) if e.contains("not found") => error_response(StatusCode::NOT_FOUND, &e),
        Err(e) => error_response(StatusCode::BAD_REQUEST, &e),
    }
}

async fn admin_fixture_save(
    State(state): State<Arc<AppState>>,
    axum::Json(body): axum::Json<Value>,
) -> Response {
    let name = match body["name"].as_str() {
        Some(name) => name.to_string(),
        None => return error_response(StatusCode::BAD_REQUEST, "missing fixture name"),
    };
    let root = match fixture_root(&state).and_then(|root| {
        std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        Ok(root)
    }) {
        Ok(root) => root,
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e),
    };
    let fixture = if let Some(trace_id) = body["from_trace_id"].as_str() {
        let kind = body["kind"].as_str().unwrap_or("resp");
        if kind != "resp" {
            return error_response(StatusCode::BAD_REQUEST, "only kind='resp' is supported");
        }
        let row = match state.store.get_trace(trace_id) {
            Ok(Some(row)) => row,
            Ok(None) => return error_response(StatusCode::NOT_FOUND, "source trace not found"),
            Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
        };
        let bytes = match row["resp_body_path"].as_str().and_then(read_gz_file) {
            Some(body) => body,
            None => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "source trace has no captured response body",
                )
            }
        };
        ErrorFixture {
            name: name.clone(),
            provider: row["upstream_provider"]
                .as_str()
                .unwrap_or("unknown")
                .into(),
            status: row["status"].as_u64().unwrap_or(500) as u16,
            error_kind: row["error_kind"]
                .as_str()
                .unwrap_or("http_status_500")
                .into(),
            body: String::from_utf8_lossy(&bytes).into_owned(),
            direction: Direction::UpstreamToClient,
            created_ms: now_ms(),
            source_trace_id: Some(trace_id.into()),
        }
    } else {
        let status = match body["status"]
            .as_u64()
            .and_then(|n| u16::try_from(n).ok())
            .filter(|n| (400..600).contains(n))
        {
            Some(status) => status,
            None => return error_response(StatusCode::BAD_REQUEST, "status must be 400-599"),
        };
        ErrorFixture {
            name: name.clone(),
            provider: body["provider"].as_str().unwrap_or("unknown").into(),
            status,
            error_kind: body["error_kind"].as_str().unwrap_or("http_status").into(),
            body: match body["body"].as_str() {
                Some(body) => body.into(),
                None => return error_response(StatusCode::BAD_REQUEST, "missing fixture body"),
            },
            direction: serde_json::from_value(body["direction"].clone()).unwrap_or_default(),
            created_ms: now_ms(),
            source_trace_id: None,
        }
    };
    let path = match fixture_path(&root, &name) {
        Ok(path) => path,
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &e),
    };
    match serde_json::to_vec_pretty(&fixture)
        .map_err(|e| e.to_string())
        .and_then(|bytes| std::fs::write(path, bytes).map_err(|e| e.to_string()))
    {
        Ok(()) => (StatusCode::CREATED, axum::Json(fixture)).into_response(),
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e),
    }
}

async fn admin_fixture_delete(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Response {
    let result = fixture_root(&state)
        .and_then(|root| fixture_path(&root, &name))
        .and_then(|path| std::fs::remove_file(path).map_err(|_| "fixture not found".into()));
    match result {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) if e == "fixture not found" => error_response(StatusCode::NOT_FOUND, &e),
        Err(e) => error_response(StatusCode::BAD_REQUEST, &e),
    }
}

async fn admin_session_inject(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    axum::Json(body): axum::Json<Value>,
) -> Response {
    let fixture = if let Some(name) = body["fixture"].as_str() {
        match load_fixture(&state, name) {
            Ok(fixture) => fixture,
            Err(e) => return error_response(StatusCode::BAD_REQUEST, &e),
        }
    } else if let Some(inline) = body.get("inline") {
        let status = match inline["status"]
            .as_u64()
            .and_then(|n| u16::try_from(n).ok())
            .filter(|n| (400..600).contains(n))
        {
            Some(status) => status,
            None => {
                return error_response(StatusCode::BAD_REQUEST, "inline.status must be 400-599")
            }
        };
        ErrorFixture {
            name: "inline".into(),
            provider: inline["provider"].as_str().unwrap_or("unknown").into(),
            status,
            error_kind: inline["error_kind"]
                .as_str()
                .unwrap_or("http_status")
                .into(),
            body: match inline["body"].as_str() {
                Some(value) => value.into(),
                None => return error_response(StatusCode::BAD_REQUEST, "missing inline.body"),
            },
            direction: serde_json::from_value(body["direction"].clone()).unwrap_or_default(),
            created_ms: now_ms(),
            source_trace_id: None,
        }
    } else {
        return error_response(StatusCode::BAD_REQUEST, "supply fixture or inline");
    };
    let count = body["count"]
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .unwrap_or(1)
        .max(1);
    let pending = PendingInjection { fixture, count };
    state
        .pending_injections
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .entry(session_id.clone())
        .or_default()
        .push(pending.clone());
    (
        StatusCode::CREATED,
        axum::Json(json!({"session_id": session_id, "pending": pending})),
    )
        .into_response()
}

async fn admin_session_injections(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
) -> Response {
    let pending = state
        .pending_injections
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(&session_id)
        .cloned()
        .unwrap_or_default();
    axum::Json(json!({"session_id": session_id, "injections": pending})).into_response()
}

async fn admin_session_injections_clear(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
) -> Response {
    state
        .pending_injections
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .remove(&session_id);
    StatusCode::NO_CONTENT.into_response()
}

/// `/admin/traces` and `/traces/search` are also consumed as session-like
/// lists by external clients. Add the same display primitives as the grouped
/// sessions endpoint without changing any persisted trace semantics.
fn trace_rows_with_display_fields(rows: Vec<Value>) -> Vec<Value> {
    rows.into_iter()
        .map(|mut row| {
            let id = row["session_id"]
                .as_str()
                .or_else(|| row["id"].as_str())
                .unwrap_or_default();
            let short_id = if id.chars().count() > 22 {
                format!(
                    "{}…{}",
                    id.chars().take(10).collect::<String>(),
                    id.chars()
                        .rev()
                        .take(8)
                        .collect::<String>()
                        .chars()
                        .rev()
                        .collect::<String>()
                )
            } else {
                id.to_string()
            };
            let status = row["status"].as_i64().unwrap_or_default();
            row["short_id"] = json!(short_id);
            row["duration_ms"] = json!(row["latency_ms"].as_i64().unwrap_or(0).max(0));
            row["providers"] = json!(row["upstream_provider"]
                .as_str()
                .into_iter()
                .collect::<Vec<_>>());
            row["status_label"] = json!(if row["error"].is_string() || status >= 400 {
                "Error"
            } else if (200..400).contains(&status) {
                "Done"
            } else {
                "Running"
            });
            row
        })
        .collect()
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
        account_ids: q
            .get("account_ids")
            .map(|ids| {
                ids.split(',')
                    .map(str::trim)
                    .filter(|id| !id.is_empty())
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default(),
        path: q.get("path").cloned(),
        harness: q.get("harness").cloned(),
        status: q.get("status").and_then(|s| s.parse().ok()),
        errors_only: q
            .get("errors")
            .map(|v| v == "1" || v == "true")
            .unwrap_or(false),
        error_class: q.get("error_class").cloned(),
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
                axum::Json(json!({"traces": trace_rows_with_display_fields(rows), "scanned": scanned, "scan_cap": TEXT_SCAN_CAP}))
                    .into_response()
            }
            None => {
                axum::Json(json!({"traces": trace_rows_with_display_fields(rows)})).into_response()
            }
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
    row["via_dario"].as_bool().unwrap_or(false)
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
                let mut item = json!({
                    "type": "tool_call",
                    "name": name,
                    "arguments": truncate_chars(
                        block["arguments"].as_str().unwrap_or_default().to_string(),
                        600,
                    ),
                });
                if let Some(id) = block["id"]
                    .as_str()
                    .or_else(|| block["call_id"].as_str())
                    .filter(|id| !id.is_empty())
                {
                    item["id"] = json!(id);
                }
                Some(item)
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
        "billing_bucket": row["billing_bucket"],
        "account_id": row["account_id"],
        "error": row["error"],
        "error_kind": row["error_kind"],
        "error_code": row["error_code"],
        "error_class": row["error_class"],
        "user": user,
        "assistant": assistant,
        "tool_calls": tool_calls,
        "assistant_blocks": assistant_blocks,
    })
}

/// Counts are sent with the transcript so tab labels never need to rescan a
/// large response while the user is typing a filter. They intentionally count
/// displayable message halves, matching the client filter's All/User/Model
/// rules; tool/agent data is additive and harmless to older clients.
fn transcript_tab_counts(turns: &[Value]) -> Value {
    let mut all = 0usize;
    let mut user = 0usize;
    let mut model = 0usize;
    let mut tools = 0usize;
    let agents = 0usize;
    for turn in turns {
        let has_user = turn["user"].as_str().is_some_and(|text| !text.is_empty());
        let has_tools = turn["tool_calls"]
            .as_array()
            .is_some_and(|calls| !calls.is_empty())
            || turn["assistant_blocks"].as_array().is_some_and(|blocks| {
                blocks
                    .iter()
                    .any(|block| block["type"].as_str() == Some("tool_call"))
            })
            || turn["executed_tools"]
                .as_array()
                .is_some_and(|calls| !calls.is_empty());
        let has_model = turn["assistant"]
            .as_str()
            .is_some_and(|text| !text.is_empty())
            || has_tools
            || turn["error"].as_str().is_some_and(|text| !text.is_empty());
        if has_user {
            user += 1;
            all += 1;
        }
        if has_model {
            model += 1;
            all += 1;
            if has_tools {
                tools += 1;
            }
        }
    }
    json!({"all": all, "user": user, "model": model, "tools": tools, "agents": agents})
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
    let tools = match state.store.session_tool_calls(&session_id) {
        Ok(rows) => rows,
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    let mut previous_codex_user_history: Option<String> = None;
    let turns: Vec<Value> = rows
        .iter()
        .take(limit)
        .enumerate()
        .map(|(index, row)| {
            let signature = codex_user_history_signature(row);
            let replayed_user = signature.is_some() && signature == previous_codex_user_history;
            if signature.is_some() {
                previous_codex_user_history = signature;
            }
            let mut turn = transcript_turn(row);
            // Pi emits a tool start after the model response that requested it
            // and before the next provider request. Associate by explicit
            // trace_id when available, otherwise by that session-local time
            // interval. `turn_id` remains on each tool row for Pi-level joins.
            let next_request = rows
                .get(index + 1)
                .and_then(|next| next["ts_request_ms"].as_i64());
            let trace_id = row["id"].as_str();
            let started = row["ts_request_ms"].as_i64().unwrap_or_default();
            let executed: Vec<Value> = tools
                .iter()
                .filter(|tool| {
                    tool["trace_id"].as_str() == trace_id
                        || (tool["trace_id"].is_null()
                            && tool["ts_start_ms"].as_i64().is_some_and(|ts| {
                                ts >= started && next_request.is_none_or(|next| ts < next)
                            }))
                })
                .cloned()
                .collect();
            turn["executed_tools"] = json!(executed);
            if replayed_user {
                turn["user"] = Value::Null;
            }
            turn
        })
        .collect();
    let tab_counts = transcript_tab_counts(&turns);
    axum::Json(json!({"session_id": session_id, "turns": turns, "tab_counts": tab_counts}))
        .into_response()
}

fn trace_reasoning_fields(req: &Value) -> (Option<String>, Option<i64>) {
    let thinking_budget = req["thinking"]
        .as_object()
        .filter(|thinking| thinking.get("type").and_then(Value::as_str) == Some("enabled"))
        .and_then(|thinking| thinking.get("budget_tokens").and_then(Value::as_i64));
    (
        req["reasoning"]["effort"]
            .as_str()
            .or_else(|| req["reasoning_effort"].as_str())
            .or_else(|| req["output_config"]["effort"].as_str())
            .map(String::from)
            .or_else(|| thinking_budget.map(|budget| format!("budget:{budget}"))),
        thinking_budget,
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

async fn tool_body(
    State(state): State<Arc<AppState>>,
    Path((id, kind)): Path<(String, String)>,
) -> Response {
    let row = match state.store.get_tool_call(&id) {
        Ok(Some(row)) => row,
        Ok(None) => {
            return error_response(StatusCode::NOT_FOUND, &format!("unknown tool call '{id}'"))
        }
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    let path = match kind.as_str() {
        "args" => row["args_body_path"].as_str(),
        "result" => row["result_body_path"].as_str(),
        _ => return error_response(StatusCode::BAD_REQUEST, "kind must be args or result"),
    };
    match read_gz_text(path) {
        Some(text) => Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "application/json; charset=utf-8")
            .body(Body::from(text))
            .unwrap_or_else(|e| error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string())),
        None => error_response(
            StatusCode::NOT_FOUND,
            &format!("no {kind} body stored for tool call '{id}'"),
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

const PAUSEABLE_PROVIDERS: [Provider; 8] = [
    Provider::Anthropic,
    Provider::Openai,
    Provider::Gemini,
    Provider::Xai,
    Provider::Openrouter,
    Provider::Exo,
    Provider::Amp,
    Provider::Kimi,
];

#[derive(Debug, Deserialize)]
struct ProviderPauseRequest {
    mode: PauseMode,
}

fn paused_provider_mode(state: &AppState, provider: Provider) -> Option<PauseMode> {
    state
        .paused_providers
        .lock()
        .ok()
        .and_then(|paused| paused.get(provider.as_str()).copied())
}

fn provider_pause_view(provider: Provider, mode: Option<PauseMode>) -> Value {
    let mut view = json!({
        "provider": provider.as_str(),
        "paused": mode.is_some(),
    });
    if let Some(mode) = mode {
        view["mode"] = json!(mode.as_str());
    }
    view
}

async fn admin_providers(State(state): State<Arc<AppState>>) -> Response {
    let providers = PAUSEABLE_PROVIDERS
        .into_iter()
        .map(|provider| provider_pause_view(provider, paused_provider_mode(&state, provider)))
        .collect::<Vec<_>>();
    axum::Json(json!({"providers": providers})).into_response()
}

async fn admin_provider_pause(
    State(state): State<Arc<AppState>>,
    Path(provider): Path<String>,
    axum::Json(request): axum::Json<ProviderPauseRequest>,
) -> Response {
    let provider = match routing_provider(&provider) {
        Ok(provider) => provider,
        Err(response) => return response,
    };
    let Ok(mut paused) = state.paused_providers.lock() else {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "provider pause state unavailable",
        );
    };
    paused.insert(provider.as_str().into(), request.mode);
    axum::Json(provider_pause_view(provider, Some(request.mode))).into_response()
}

async fn admin_provider_resume(
    State(state): State<Arc<AppState>>,
    Path(provider): Path<String>,
) -> Response {
    let provider = match routing_provider(&provider) {
        Ok(provider) => provider,
        Err(response) => return response,
    };
    let Ok(mut paused) = state.paused_providers.lock() else {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "provider pause state unavailable",
        );
    };
    paused.remove(provider.as_str());
    axum::Json(provider_pause_view(provider, None)).into_response()
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
            let mut limits = a
                .account_meta
                .get("routing_limits")
                .or_else(|| a.account_meta.get("codex_limits"))
                .cloned()
                .unwrap_or(Value::Null);
            let quota = quota_state(a.provider, &limits);
            if let Some(object) = limits.as_object_mut() {
                object.insert("quota".into(), quota);
            }
            json!({
                "id": a.id,
                "provider": a.provider.as_str(),
                "name": a.name,
                "kind": a.kind,
                "label": a.label,
                "description": a.description,
                "email": email,
                "paused": a.paused,
                "path": a.path.as_ref().map(|p| p.display().to_string()),
                "status": a.status,
                // Reachability derived from the last heartbeat/ping, distinct
                // from `status` (credential presence). The UI dot must follow
                // this, not `status`, so a failing-ping provider reads red.
                "health": account_health(&a),
                "last_probe": a.account_meta.get("last_probe").cloned().unwrap_or(Value::Null),
                "needs_reauth": a.needs_reauth(),
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
            let mut limits = account
                .account_meta
                .get("routing_limits")
                .or_else(|| account.account_meta.get("codex_limits"))
                .cloned()
                .unwrap_or_else(|| json!({}));
            let quota = quota_state(provider, &limits);
            if let Some(object) = limits.as_object_mut() {
                object.insert("quota".into(), quota);
            }
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
                "quota": limits.get("quota").cloned().unwrap_or_else(|| quota_state(provider, &limits)),
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

async fn update_routing(state: Arc<AppState>, provider: Provider, body: Value) -> Response {
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
            None => {
                return error_response(StatusCode::BAD_REQUEST, "reserve_pct must be an integer")
            }
        },
        None => current_policy.reserve_pct.unwrap_or(10) as u64,
    };
    if reserve_pct > 100 {
        return error_response(
            StatusCode::BAD_REQUEST,
            "reserve_pct must be between 0 and 100",
        );
    }
    let allow_mid_thread_failover = match body.get("allow_mid_thread_failover") {
        Some(value) => match value.as_bool() {
            Some(value) => value,
            None => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "allow_mid_thread_failover must be boolean",
                )
            }
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
            return error_response(
                StatusCode::BAD_REQUEST,
                "each account needs boolean eligible",
            );
        };
        let Some(priority) = item.get("priority").and_then(Value::as_u64) else {
            return error_response(
                StatusCode::BAD_REQUEST,
                "each account needs integer priority",
            );
        };
        let account_reserve_pct = match item.get("reserve_pct") {
            Some(value) => match value.as_u64() {
                Some(value) if value <= 100 => value as u8,
                Some(_) => {
                    return error_response(
                        StatusCode::BAD_REQUEST,
                        "account reserve_pct must be between 0 and 100",
                    )
                }
                None => {
                    return error_response(
                        StatusCode::BAD_REQUEST,
                        "account reserve_pct must be an integer",
                    )
                }
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
    match state.vault.set_policy_persisted(provider, policy).await {
        Ok(()) => axum::Json(routing_snapshot(&state, provider).await).into_response(),
        Err(error) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
    }
}

fn validate_protection_policy(policy: &ProtectionPolicy) -> std::result::Result<(), String> {
    if policy.retries > 10 {
        return Err("retries must be between 0 and 10".into());
    }
    for (requested_model, equivalents) in &policy.equivalencies {
        if requested_model.trim().is_empty() {
            return Err("equivalency model names must not be empty".into());
        }
        if equivalents.is_empty() {
            return Err(format!(
                "equivalency for '{requested_model}' must contain a provider/model pair"
            ));
        }
        for (provider, model) in equivalents {
            if !matches!(
                provider.as_str(),
                "anthropic" | "openai" | "xai" | "gemini" | "openrouter"
            ) {
                return Err(format!("unknown equivalency provider '{provider}'"));
            }
            if model.trim().is_empty() {
                return Err(format!(
                    "equivalency model for provider '{provider}' must not be empty"
                ));
            }
        }
    }
    Ok(())
}

async fn admin_protection(State(state): State<Arc<AppState>>) -> Response {
    match state.protection.read() {
        Ok(policy) => axum::Json(policy.clone()).into_response(),
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "protection policy unavailable",
        ),
    }
}

async fn admin_protection_update(
    State(state): State<Arc<AppState>>,
    axum::Json(policy): axum::Json<ProtectionPolicy>,
) -> Response {
    if let Err(error) = validate_protection_policy(&policy) {
        return error_response(StatusCode::BAD_REQUEST, &error);
    }
    let persister = state
        .protection_persister
        .read()
        .ok()
        .and_then(|slot| slot.clone());
    let Some(persister) = persister else {
        return error_response(
            StatusCode::NOT_IMPLEMENTED,
            "protection policy persistence is not configured",
        );
    };
    if let Err(error) = persister.persist(&policy) {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, &error);
    }
    set_protection_policy(&state, policy.clone());
    axum::Json(policy).into_response()
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
    pub kimi: String,
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
            // Sonnet costs roughly 10x Haiku per ping, but it verifies that a
            // subscription can actually use premium models. Immediately after
            // Dario is enabled its first Sonnet ping may warm the prompt cache
            // (or briefly use the direct fallback and receive one 429) before
            // subsequent routed checks become green.
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
        Provider::Exo => match exo_model_payload(state).await {
            Ok(payload) => {
                return PingResult {
                    provider: "exo",
                    account_id: Some("exo-local".into()),
                    ok: true,
                    status: Some(200),
                    latency_ms: now_ms() - start,
                    message: format!("{} models available", exo_models_array(&payload).len()),
                }
            }
            Err(error) => {
                return PingResult {
                    provider: "exo",
                    account_id: Some("exo-local".into()),
                    ok: false,
                    status: None,
                    latency_ms: now_ms() - start,
                    message: error,
                }
            }
        },
        Provider::Kimi => {
            // Kimi's usage endpoint doubles as a cheap credential check: it needs
            // a valid Bearer token and returns the subscription's quota windows.
            let (ok, account_id, message) = kimi_usage_probe(state).await;
            return PingResult {
                provider: "kimi",
                account_id,
                ok,
                status: if ok { Some(200) } else { None },
                latency_ms: now_ms() - start,
                message,
            };
        }
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
    let message = annotate_anthropic_ping_message(
        provider,
        state
            .dario
            .as_ref()
            .is_some_and(|dario| dario.routes_requests() && dario.active().is_none()),
        extract_reply(&text).unwrap_or_else(|| snippet(&text)),
    );
    PingResult {
        provider: provider.as_str(),
        account_id,
        ok: (200..300).contains(&status),
        status: Some(status),
        latency_ms: now_ms() - start,
        message,
    }
}

fn annotate_anthropic_ping_message(
    provider: Provider,
    dario_down: bool,
    message: String,
) -> String {
    if provider == Provider::Anthropic && dario_down {
        "degraded — serving via direct fallback, Dario down".into()
    } else {
        message
    }
}

/// Probe the local Dario generation with a tiny Haiku completion.  The vault
/// check is deliberately first: without Anthropic credentials Dario is not an
/// applicable health target, rather than a failed one.
pub async fn ping_dario(state: &Arc<AppState>, model: &str) -> (DarioHealthState, PingResult) {
    let start = now_ms();
    let account_id = state
        .vault
        .list()
        .await
        .into_iter()
        .find(|account| account.provider == Provider::Anthropic && account.status == "active")
        .map(|account| account.id);
    let Some(account_id) = account_id else {
        return (
            dario_health_state(false, false, false),
            PingResult {
                provider: "dario",
                account_id: None,
                ok: true,
                status: None,
                latency_ms: now_ms() - start,
                message: "not applicable: no Anthropic credentials".into(),
            },
        );
    };
    let generation_ready = state
        .dario
        .as_ref()
        .and_then(|dario| dario.active())
        .is_some();
    let result = match &state.dario {
        Some(dario) if generation_ready => dario.probe(model).await,
        Some(_) => Err("no healthy Dario generation".into()),
        None => Err("Dario mode is not enabled".into()),
    };
    let probe_ok = result.is_ok();
    let health = dario_health_state(true, generation_ready, probe_ok);
    let message = match result {
        Ok(()) => "through-Dario probe succeeded".into(),
        Err(error) => error,
    };
    (
        health,
        PingResult {
            provider: "dario",
            account_id: Some(account_id),
            ok: health == DarioHealthState::Healthy,
            status: generation_ready.then_some(if probe_ok { 200 } else { 502 }),
            latency_ms: now_ms() - start,
            message,
        },
    )
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
                        | Provider::Kimi
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
        if r.ok {
            if let Some(account_id) = r.account_id.as_deref() {
                if let Err(error) = state.vault.clear_cooldown(account_id).await {
                    tracing::warn!(%error, %account_id, "could not clear recovered account cooldown");
                }
            }
        }
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
        // Reachability, not credential-presence, is what the status dot must
        // reflect: persist the probe outcome and — for an auth-class failure —
        // reuse the proactive re-auth dispatcher (same cooldown as the request
        // path and the idle-expiry watchdog).
        record_probe_outcome(state, &r).await;
        results.push(r);
    }
    results
}

/// Health an account's last probe implies. This is reachability, distinct from
/// the credential-presence `status` field: a live credential that fails its
/// ping is not "healthy". `Unknown` is never recorded — it is the read-side
/// default for an account that has never been probed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProbeHealth {
    Healthy,
    Unreachable,
    AuthFailed,
}

impl ProbeHealth {
    fn as_str(self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Unreachable => "unreachable",
            Self::AuthFailed => "auth_failed",
        }
    }
}

/// Classify a probe result into a health state. The don't-cry-wolf boundary is
/// the shared [`ErrorClass::Auth`] taxonomy: only a genuine auth failure
/// (401/403/invalid token) is `AuthFailed`; every other failure — 5xx, 429,
/// timeout, network, unknown — is `Unreachable` (a failover/down condition, not
/// a re-auth condition). A ping travels through the proxy, which already forces
/// one token refresh on a 401 before returning, so a 401 reaching here is a
/// confirmed logout rather than a merely-stale token.
fn probe_health(result: &PingResult) -> ProbeHealth {
    if result.ok {
        return ProbeHealth::Healthy;
    }
    match classify_error("", result.status, None) {
        ErrorClass::Auth => ProbeHealth::AuthFailed,
        _ => ProbeHealth::Unreachable,
    }
}

/// Persist a probe outcome on its account and drive the auth-failure re-auth
/// alert. Recording the last probe is what lets `/admin/accounts` (and the
/// Swift status dot) report reachability instead of credential presence.
///
/// On an auth-class failure this reuses the exact proactive-reauth machinery
/// (`mark_account_needs_reauth` + `emit_reauth_notification_for_account`, whose
/// dispatcher cooldown debounces to one alert per provider per window). A
/// transient/unreachable failure deliberately leaves the re-auth flag untouched
/// — it is a failover condition, not a logout. A healthy probe clears a prior
/// logout flag so a recovered account stops showing red and the next genuine
/// logout alerts again.
async fn record_probe_outcome(state: &AppState, result: &PingResult) {
    let Some(account_id) = result.account_id.as_deref() else {
        return;
    };
    let health = probe_health(result);
    let last_probe = json!({
        "ok": result.ok,
        "status": result.status,
        "latency_ms": result.latency_ms,
        "health": health.as_str(),
        "checked_at_ms": now_ms(),
    });
    if let Err(error) = state
        .vault
        .set_account_meta(account_id, "last_probe", last_probe)
        .await
    {
        tracing::warn!(account = %account_id, %error, "could not persist probe health");
    }
    match health {
        ProbeHealth::AuthFailed => {
            mark_account_needs_reauth(state, account_id, true).await;
            // Only managed OAuth logins have a re-auth alert; the emitter itself
            // guards on kind, but fetch the fresh account so its provider/label
            // drive the (secret-free) notification.
            if let Some(account) = state
                .vault
                .list()
                .await
                .into_iter()
                .find(|account| account.id == account_id)
            {
                emit_reauth_notification_for_account(state, &account);
            }
        }
        ProbeHealth::Healthy => {
            // A recovered account: clear any stale logout flag so it stops
            // reading red and a future logout alerts afresh.
            if state
                .vault
                .list()
                .await
                .into_iter()
                .any(|account| account.id == account_id && account.needs_reauth())
            {
                mark_account_needs_reauth(state, account_id, false).await;
            }
        }
        // Unreachable/down is a failover condition, never a re-auth: leave the
        // flag exactly as it is so a network blip never clears nor raises it.
        ProbeHealth::Unreachable => {}
    }
}

/// The reachability health `/admin/accounts` reports for an account. A confirmed
/// logout (`needs_reauth`, set by either the probe path or the idle-expiry
/// watchdog) always wins so the dot is red regardless of the last ping; failing
/// that it echoes the last probe's health, defaulting to `unknown` when the
/// account has never been probed (never claim green without evidence).
fn account_health(account: &Account) -> String {
    if account.needs_reauth() {
        return "auth_failed".to_string();
    }
    account
        .account_meta
        .get("last_probe")
        .and_then(|probe| probe.get("health"))
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string()
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
    "x-claude-code-agent-id",
    "x-claude-code-parent-agent-id",
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

fn is_genuine_claude_code_request(format: ClientFormat, headers: &HeaderMap, body: &Value) -> bool {
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
    /// The account displayed and aggregated in traces. For Dario this is the
    /// real Anthropic subscription, never the synthetic local Dario key.
    account: Account,
    /// Credentials used to make the HTTP connection when they differ from
    /// trace attribution (the Dario child key).
    connection_account: Option<Account>,
    body: Vec<u8>,
    upstream_format: &'static str,
    destream: bool,
    respond_as: Option<RespondAs>,
    client_stream: bool,
    extra_headers: Vec<(String, String)>,
    dario_guard: Option<Box<dyn std::any::Any + Send>>,
    dario_fallback_reason: Option<String>,
    via_dario: bool,
    dario_generation: Option<String>,
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

fn exo_account() -> Account {
    Account {
        id: "exo-local".into(),
        provider: Provider::Exo,
        kind: "local".into(),
        name: "Exo local cluster".into(),
        description: None,
        paused: false,
        label: Some("Exo local inference".into()),
        access_token: None,
        refresh_token: None,
        id_token: None,
        api_key: Some("x".into()),
        expires_at_ms: None,
        last_refresh_ms: None,
        account_meta: Value::Null,
        cooldown_until_ms: None,
        status: "active".into(),
        path: None,
    }
}

fn direct_anthropic_plan(
    account: Account,
    body_json: &mut Value,
    original_body: &[u8],
    routed_model: &str,
    converted: Option<(Value, RespondAs)>,
    client_stream: bool,
    dario_fallback_reason: String,
) -> UpstreamPlan {
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
    UpstreamPlan {
        url: format!("{ANTHROPIC_BASE}/v1/messages"),
        account,
        connection_account: None,
        body,
        upstream_format: "anthropic",
        destream: false,
        respond_as,
        client_stream,
        extra_headers: vec![],
        dario_guard: None,
        dario_fallback_reason: Some(dario_fallback_reason),
        via_dario: false,
        dario_generation: None,
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
    let genuine_claude_code = is_genuine_claude_code_request(format, client_headers, body_json);
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
            let (base, account, connection_account, dario_guard, dario_capture, dario_fallback_reason, via_dario, dario_generation) = if genuine_claude_code {
                let account = state
                    .vault
                    .account_for_excluding(provider, true, excluded_accounts)
                    .await
                    .map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?;
                (ANTHROPIC_BASE.to_string(), account, None, None, false, None, false, None)
            } else {
                let route_via_dario = state
                    .dario
                    .as_ref()
                    .map(|dario| dario.routes_requests())
                    .unwrap_or(false);
                if !route_via_dario {
                    let account = state
                        .vault
                        .account_for_excluding(provider, true, excluded_accounts)
                        .await
                        .map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?;
                    (ANTHROPIC_BASE.to_string(), account, None, None, false, None, false, None)
                } else {
                    let dario_active = match state.dario.as_ref() {
                        Some(dario) => match dario.active() {
                            Some(active) => Some(active),
                            None => match dario.ensure_active().await {
                                Ok(active) => Some(active),
                                Err(reason) => {
                                    let account = state
                                        .vault
                                        .account_for_excluding(provider, true, excluded_accounts)
                                        .await
                                        .map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?;
                                    tracing::warn!(%reason, "Dario unavailable after on-demand repair; using direct Anthropic fallback");
                                    return Ok(direct_anthropic_plan(
                                        account, body_json, original_body, routed_model, converted,
                                        client_stream, format!("dario repair failed: {reason}"),
                                    ));
                                }
                            },
                        },
                        None => None,
                    };
                    match (&state.dario, dario_active) {
                        (Some(dario), Some(active)) => {
                            match dario.prepare_model(routed_model).await {
                                DarioPrepare::ServeThroughDario => {}
                                DarioPrepare::DirectFallback { reason } => {
                                    let account = state
                                        .vault
                                        .account_for_excluding(provider, true, excluded_accounts)
                                        .await
                                        .map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?;
                                    return Ok(direct_anthropic_plan(
                                        account,
                                        body_json,
                                        original_body,
                                        routed_model,
                                        converted,
                                        client_stream,
                                        reason,
                                    ));
                                }
                                DarioPrepare::Unavailable { reason } => {
                                    return Err((StatusCode::SERVICE_UNAVAILABLE, reason));
                                }
                            }
                            let Some(guard) = dario.begin(&active.generation_id) else {
                                let account = state.vault.account_for_excluding(provider, true, excluded_accounts).await
                                    .map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?;
                                let reason = "Dario became unavailable while routing the Anthropic request".to_string();
                                tracing::warn!(%reason, "using direct Anthropic fallback");
                                return Ok(direct_anthropic_plan(account, body_json, original_body, routed_model, converted, client_stream, reason));
                            };
                            let attribution_account = state
                                .vault
                                .account_for_excluding(provider, true, excluded_accounts)
                                .await
                                .map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?;
                            (
                                active.base_url.trim_end_matches('/').to_string(),
                                attribution_account,
                                Some(dario_account(&active)),
                                Some(guard),
                                true,
                                None,
                                true,
                                Some(active.generation_id),
                            )
                        }
                        (Some(_), None) => {
                            let account = state.vault.account_for_excluding(provider, true, excluded_accounts).await
                                .map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?;
                            let reason = "Dario has no healthy generation after on-demand repair".to_string();
                            tracing::warn!(%reason, "using direct Anthropic fallback");
                            return Ok(direct_anthropic_plan(account, body_json, original_body, routed_model, converted, client_stream, reason));
                        }
                        (None, None) => unreachable!("Dario routing requires a Dario router"),
                        (None, Some(_)) => {
                            unreachable!("Dario cannot be active when it is disabled")
                        }
                    }
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
                connection_account,
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
                dario_fallback_reason,
                via_dario,
                dario_generation,
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
                        connection_account: None,
                        body,
                        upstream_format: "openai-chat",
                        destream: false,
                        respond_as: None,
                        client_stream,
                        extra_headers: vec![],
                        dario_guard: None,
                        dario_fallback_reason: None,
                        via_dario: false,
                        dario_generation: None,
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
                        connection_account: None,
                        body,
                        upstream_format: "openai-responses",
                        destream: false,
                        respond_as: Some(RespondAs::OpenaiChat),
                        client_stream,
                        extra_headers: vec![],
                        dario_guard: None,
                        dario_fallback_reason: None,
                        via_dario: false,
                        dario_generation: None,
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
                        connection_account: None,
                        body,
                        upstream_format: "openai-responses",
                        destream,
                        respond_as: None,
                        client_stream,
                        extra_headers: vec![],
                        dario_guard: None,
                        dario_fallback_reason: None,
                        via_dario: false,
                        dario_generation: None,
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
                        connection_account: None,
                        body,
                        upstream_format: "openai-responses",
                        destream: false,
                        respond_as: Some(RespondAs::Anthropic),
                        client_stream,
                        extra_headers: vec![],
                        dario_guard: None,
                        dario_fallback_reason: None,
                        via_dario: false,
                        dario_generation: None,
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
                        connection_account: None,
                        body,
                        upstream_format: "openai-responses",
                        destream: false,
                        respond_as: Some(RespondAs::Gemini),
                        client_stream,
                        extra_headers: vec![],
                        dario_guard: None,
                        dario_fallback_reason: None,
                        via_dario: false,
                        dario_generation: None,
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
                        connection_account: None,
                        body,
                        upstream_format: "openai-chat",
                        destream: false,
                        respond_as: None,
                        client_stream,
                        extra_headers,
                        dario_guard: None,
                        dario_fallback_reason: None,
                        via_dario: false,
                        dario_generation: None,
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
                        connection_account: None,
                        body,
                        upstream_format: "openai-chat",
                        destream: false,
                        respond_as: Some(RespondAs::Anthropic),
                        client_stream,
                        extra_headers,
                        dario_guard: None,
                        dario_fallback_reason: None,
                        via_dario: false,
                        dario_generation: None,
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
                        connection_account: None,
                        body,
                        upstream_format: "openai-chat",
                        destream: false,
                        respond_as: None,
                        client_stream,
                        extra_headers: vec![],
                        dario_guard: None,
                        dario_fallback_reason: None,
                        via_dario: false,
                        dario_generation: None,
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
                        connection_account: None,
                        body,
                        upstream_format: "openai-chat",
                        destream: false,
                        respond_as: Some(RespondAs::Anthropic),
                        client_stream,
                        extra_headers: vec![],
                        dario_guard: None,
                        dario_fallback_reason: None,
                        via_dario: false,
                        dario_generation: None,
                    })
                }
                ClientFormat::OpenaiResponses | ClientFormat::GeminiGenerate => Err((
                    StatusCode::NOT_IMPLEMENTED,
                    "the OpenRouter upstream speaks OpenAI chat completions; POST to /v1/chat/completions or /v1/messages"
                        .to_string(),
                )),
            }
        }
        Provider::Exo => {
            let url = state.exo.read().map_err(|_| (StatusCode::SERVICE_UNAVAILABLE, "Exo settings are unavailable".to_string()))?.url.clone();
            let account = exo_account();
            match format {
                ClientFormat::OpenaiChat => {
                    body_json["model"] = json!(routed_model);
                    let body = serde_json::to_vec(body_json).unwrap_or_else(|_| original_body.to_vec());
                    Ok(UpstreamPlan { url: format!("{url}/v1/chat/completions"), account, connection_account: None, body,
                        upstream_format: "openai-chat", destream: false, respond_as: None, client_stream,
                        extra_headers: vec![], dario_guard: None, dario_fallback_reason: None, via_dario: false, dario_generation: None })
                }
                ClientFormat::AnthropicMessages => {
                    let mut converted = translate::anthropic_to_openai_chat(body_json);
                    converted["model"] = json!(routed_model);
                    converted["stream"] = json!(false);
                    let body = serde_json::to_vec(&converted).unwrap_or_else(|_| original_body.to_vec());
                    Ok(UpstreamPlan { url: format!("{url}/v1/chat/completions"), account, connection_account: None, body,
                        upstream_format: "openai-chat", destream: false, respond_as: Some(RespondAs::Anthropic), client_stream,
                        extra_headers: vec![], dario_guard: None, dario_fallback_reason: None, via_dario: false, dario_generation: None })
                }
                ClientFormat::OpenaiResponses | ClientFormat::GeminiGenerate => Err((
                    StatusCode::NOT_IMPLEMENTED,
                    "the Exo upstream speaks OpenAI chat completions; POST to /v1/chat/completions or /v1/messages".to_string(),
                )),
            }
        }
        Provider::Kimi => {
            let account = state
                .vault
                .account_for_excluding(provider, true, excluded_accounts)
                .await
                .map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?;
            match format {
                ClientFormat::OpenaiChat => {
                    body_json["model"] = json!(routed_model);
                    let body =
                        serde_json::to_vec(body_json).unwrap_or_else(|_| original_body.to_vec());
                    Ok(UpstreamPlan {
                        url: format!("{KIMI_BASE}/chat/completions"),
                        account,
                        connection_account: None,
                        body,
                        upstream_format: "openai-chat",
                        destream: false,
                        respond_as: None,
                        client_stream,
                        extra_headers: vec![],
                        dario_guard: None,
                        dario_fallback_reason: None,
                        via_dario: false,
                        dario_generation: None,
                    })
                }
                ClientFormat::AnthropicMessages => {
                    let mut converted = translate::anthropic_to_openai_chat(body_json);
                    converted["model"] = json!(routed_model);
                    converted["stream"] = json!(false);
                    let body = serde_json::to_vec(&converted)
                        .unwrap_or_else(|_| original_body.to_vec());
                    Ok(UpstreamPlan {
                        url: format!("{KIMI_BASE}/chat/completions"),
                        account,
                        connection_account: None,
                        body,
                        upstream_format: "openai-chat",
                        destream: false,
                        respond_as: Some(RespondAs::Anthropic),
                        client_stream,
                        extra_headers: vec![],
                        dario_guard: None,
                        dario_fallback_reason: None,
                        via_dario: false,
                        dario_generation: None,
                    })
                }
                ClientFormat::OpenaiResponses | ClientFormat::GeminiGenerate => Err((
                    StatusCode::NOT_IMPLEMENTED,
                    "the Kimi upstream speaks OpenAI chat completions; POST to /v1/chat/completions or /v1/messages".to_string(),
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
                connection_account: None,
                body,
                upstream_format: "gemini",
                destream: false,
                respond_as: respond_as.or(Some(RespondAs::Gemini)),
                client_stream,
                extra_headers: vec![],
                dario_guard: None,
                dario_fallback_reason: None,
                via_dario: false,
                dario_generation: None,
            })
        }
    }
}

#[cfg(test)]
mod trace_api_tests {
    use super::{
        account_plot_series, openai_responses_user_history_signature, session_from_metadata,
        trace_extras, trace_harness, trace_reasoning_fields, transcript_assistant_blocks,
        transcript_tab_counts, transcript_turn, truncate_chars,
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
    fn account_plot_series_zero_fills_canonical_buckets() {
        let response = json!({
            "by_account": [{"account_id": "a"}, {"account_id": "b"}],
            "series": [
                {"bucket_ms": 0, "account_id": "a", "input_tokens": 2, "output_tokens": 3},
                {"bucket_ms": 2_000, "account_id": "a", "input_tokens": 7, "output_tokens": 0},
                {"bucket_ms": 1_000, "account_id": "b", "input_tokens": 1, "output_tokens": 1}
            ]
        });
        let (series, labels, count) = account_plot_series(&response, 100, 1_000, 2_999);
        assert_eq!(count, 3);
        assert_eq!(labels, vec!["00:00", "00:00", "00:00"]);
        assert_eq!(series[0]["account_id"], "a");
        assert_eq!(series[0]["values"], json!([5.0, 0.0, 7.0]));
        assert_eq!(series[1]["values"], json!([0.0, 2.0, 0.0]));
    }

    #[test]
    fn transcript_tab_counts_count_displayable_halves_once() {
        let turns = vec![
            json!({"user": "hello", "assistant": "world", "tool_calls": []}),
            json!({"user": null, "assistant": null, "tool_calls": [{"name": "ls"}]}),
        ];
        assert_eq!(
            transcript_tab_counts(&turns),
            json!({"all": 3, "user": 1, "model": 2, "tools": 1, "agents": 0})
        );
    }

    #[test]
    fn explicit_harness_header_wins_over_sdk_user_agent() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "user-agent",
            HeaderValue::from_static("Anthropic/JS 0.91.1"),
        );
        assert_eq!(
            trace_harness(&headers).as_deref(),
            Some("Anthropic/JS 0.91.1")
        );
        headers.insert("x-alexandria-harness", HeaderValue::from_static("pi"));
        assert_eq!(trace_harness(&headers).as_deref(), Some("pi"));
    }

    #[test]
    fn proxy_reasoning_effort_capture_for_anthropic_thinking() {
        let anthropic = json!({
            "system": [{"type": "text", "text": "abcd"}],
            "messages": [{"role": "user", "content": "hi"}],
            "max_tokens": 100,
            "temperature": 0.5,
            "thinking": {"type": "enabled", "budget_tokens": 4096},
        });
        let e = trace_extras(&anthropic);
        assert_eq!(e["thinking_budget"], 4096);
        assert_eq!(
            trace_reasoning_fields(&anthropic),
            (Some("budget:4096".into()), Some(4096))
        );
        assert_eq!(e["max_tokens"], 100);
        assert_eq!(e["temperature"], 0.5);
        assert_eq!(e["message_count"], 1);
        assert_eq!(e["system_chars"], 4);
        assert_eq!(e["reasoning_effort"], "budget:4096");

        let anthropic_without_thinking = json!({
            "model": "claude-sonnet-4-5",
            "messages": [],
        });
        assert_eq!(
            trace_reasoning_fields(&anthropic_without_thinking),
            (None, None)
        );
    }

    #[test]
    fn proxy_reasoning_effort_capture_for_openai_responses() {
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
    fn proxy_reasoning_effort_capture_for_openai_chat_completions() {
        let chat = json!({
            "model": "gpt-5",
            "messages": [],
            "reasoning_effort": "medium",
        });
        assert_eq!(trace_reasoning_fields(&chat), (Some("medium".into()), None));
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
            "billing_bucket": "subscription",
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
        assert_eq!(turn["billing_bucket"], "subscription");
        assert_eq!(turn["thinking_budget"], serde_json::Value::Null);
    }

    #[test]
    fn transcript_assistant_blocks_preserve_text_tool_text_order() {
        let response = json!({
            "_alexandria": {"assistant_blocks": [
                {"type": "text", "text": "Listing the workspace."},
                {"type": "tool_call", "id": "call-1", "name": "Shell", "arguments": "{\"command\":\"ls\"}"},
                {"type": "text", "text": "Here are the files."},
            ]}
        });
        let blocks = transcript_assistant_blocks(&response.to_string());
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0]["type"], "text");
        assert_eq!(blocks[0]["text"], "Listing the workspace.");
        assert_eq!(blocks[1]["type"], "tool_call");
        assert_eq!(blocks[1]["id"], "call-1");
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
        (Provider::Exo, _) => {
            h.insert("authorization", HeaderValue::from_static("Bearer x"));
        }
        (Provider::Kimi, _) => {
            let token = account
                .access_token
                .as_deref()
                .or(account.api_key.as_deref())
                .ok_or((
                    StatusCode::BAD_GATEWAY,
                    "kimi account has no access token".to_string(),
                ))?;
            h.insert(
                "authorization",
                HeaderValue::from_str(&format!("Bearer {token}"))
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
    for (meta_key, header_name) in [("http_referer", "http-referer"), ("x_title", "x-title")] {
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

fn merge_trace_note(tags: Option<String>, key: &str, value: &str) -> Option<String> {
    let mut object = tags
        .as_deref()
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    object.insert(key.to_string(), Value::String(value.to_string()));
    serde_json::to_string(&Value::Object(object)).ok()
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

// Tool call bodies use an id derived from session_id and tool_call_id. Dots are
// legal in trace ids, but the body filename sanitizer maps them to underscores.
// Keep that broader trace-id contract and reject dots only for tool identities.
fn valid_tool_event_id(id: &str) -> bool {
    valid_ingest_trace_id(id) && !id.contains('.')
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
    if state
        .local_key
        .read()
        .map(|local| key == *local)
        .unwrap_or(false)
    {
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
    axum::Json(mut event): axum::Json<Value>,
) -> Response {
    let (key_hash, key) = match authenticate_harness_event(&state, &headers) {
        Ok(auth) => auth,
        Err(response) => return response,
    };
    if let Err(error) = state.store.touch_run_key(&key_hash, now_ms()) {
        tracing::warn!("failed to touch harness key: {error}");
    }
    let harness = key.label.as_deref().unwrap_or_default();
    normalize_harness_event(&mut event);
    let event_name = event["hook_event_name"].as_str().unwrap_or_default();
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

fn normalize_harness_event(event: &mut Value) {
    let Some(object) = event.as_object_mut() else {
        return;
    };
    let raw_event = object
        .get("hook_event_name")
        .or_else(|| object.get("hookEventName"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let compact = raw_event
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase();
    let canonical = match compact.as_str() {
        "sessionstart" => Some("SessionStart"),
        "subagentstart" => Some("SubagentStart"),
        "subagentstop" | "subagentend" => Some("SubagentStop"),
        "stop" | "sessionend" => Some("Stop"),
        _ => None,
    };
    if let Some(canonical) = canonical {
        object.insert("hook_event_name".into(), Value::String(canonical.into()));
    }

    for (canonical, aliases) in [
        (
            "session_id",
            &["sessionId", "conversation_id", "conversationId"][..],
        ),
        ("turn_id", &["turnId"][..]),
        (
            "agent_id",
            &[
                "agentId",
                "subagent_id",
                "subagentId",
                "subagent_session_id",
                "subagentSessionId",
                "child_session_id",
                "childSessionId",
            ][..],
        ),
        (
            "agent_type",
            &["agentType", "subagent_type", "subagentType"][..],
        ),
    ] {
        if object.get(canonical).and_then(Value::as_str).is_some() {
            continue;
        }
        if let Some(value) = aliases
            .iter()
            .find_map(|alias| object.get(*alias).and_then(Value::as_str))
        {
            object.insert(canonical.into(), Value::String(value.to_string()));
        }
    }
}

static TOOL_AUTH_HEADER_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?i)(\bauthorization\s*:\s*(?:bearer|basic)\s+)([^\s'"\\]+)"#).unwrap()
});
static TOOL_API_KEY_HEADER_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?i)(\bx-api-key\s*:\s*)([^\s'"\\]+)"#).unwrap());
static TOOL_ENV_ASSIGNMENT_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?i)(\b(?:export\s+)?(?:pgpassword|aws_secret_access_key|token|api[_-]?key|authorization|password|secret)\s*=\s*)(?:'[^']*'|"[^"]*"|[^\s;]+)"#).unwrap()
});
static TOOL_FLAG_ASSIGNMENT_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?i)(--(?:token|api-key|apikey|password|secret|key|bearer|auth)\s*=\s*)(?:'[^']*'|"[^"]*"|[^\s;]+)"#).unwrap()
});
static TOOL_FLAG_VALUE_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?i)(--(?:token|api-key|apikey|password|secret|key|bearer|auth)\s+)(?:'[^']*'|"[^"]*"|[^\s;]+)"#).unwrap()
});
static TOOL_URL_PASSWORD_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)(https?://[^\s/:@]+:)([^@\s/]+)(@)").unwrap());
static TOOL_STANDALONE_KEY_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(?:sk_live_[a-z0-9_-]+|sk-[a-z0-9_-]+|ghp_[a-z0-9]+|xoxb-[a-z0-9-]+|akia[0-9a-z]{16})\b").unwrap()
});

/// Mask well-known secret formats embedded in otherwise useful free text.
///
/// This deliberately favors explicit credential syntax over entropy-based
/// detection: command context remains useful without masking ordinary prose.
fn scrub_secret_string(input: &str) -> Option<String> {
    let mut scrubbed = input.to_string();
    for (regex, replacement) in [
        (&*TOOL_AUTH_HEADER_RE, "$1<redacted>"),
        (&*TOOL_API_KEY_HEADER_RE, "$1<redacted>"),
        (&*TOOL_ENV_ASSIGNMENT_RE, "$1<redacted>"),
        (&*TOOL_FLAG_ASSIGNMENT_RE, "$1<redacted>"),
        (&*TOOL_FLAG_VALUE_RE, "$1<redacted>"),
        (&*TOOL_URL_PASSWORD_RE, "$1<redacted>$3"),
        (&*TOOL_STANDALONE_KEY_RE, "<redacted>"),
    ] {
        scrubbed = regex.replace_all(&scrubbed, replacement).into_owned();
    }
    (scrubbed != input).then_some(scrubbed)
}

fn redact_tool_value(value: &mut Value) {
    const SECRET_KEYS: &[&str] = &[
        "authorization",
        "token",
        "secret",
        "password",
        "api_key",
        "apikey",
        "cookie",
        "env",
        "environment",
    ];
    match value {
        Value::Object(object) => {
            for (key, value) in object.iter_mut() {
                let lower = key.to_ascii_lowercase();
                if SECRET_KEYS.iter().any(|needle| lower.contains(needle)) {
                    *value = Value::String("<redacted>".into());
                } else {
                    redact_tool_value(value);
                }
            }
        }
        Value::Array(values) => {
            let mut redact_next = false;
            for value in values {
                if redact_next {
                    *value = Value::String("<redacted>".into());
                    redact_next = false;
                    continue;
                }
                if let Some(argument) = value.as_str() {
                    let lower = argument.to_ascii_lowercase();
                    const SECRET_FLAGS: &[&str] = &[
                        "--token",
                        "--api-key",
                        "--apikey",
                        "--password",
                        "--secret",
                        "--key",
                        "--bearer",
                        "--auth",
                        "-h",
                        "--header",
                    ];
                    if let Some((flag, _)) = lower.split_once('=') {
                        if SECRET_FLAGS.iter().any(|secret_flag| flag == *secret_flag) {
                            let (key, _) = argument.split_once('=').unwrap();
                            *value = Value::String(format!("{key}=<redacted>"));
                            continue;
                        }
                    }
                    if SECRET_FLAGS.iter().any(|flag| lower == *flag) {
                        redact_next = true;
                    } else if let Some(scrubbed) = scrub_secret_string(argument) {
                        *value = Value::String(scrubbed);
                    }
                } else {
                    redact_tool_value(value);
                }
            }
        }
        Value::String(string) => {
            if let Some(scrubbed) = scrub_secret_string(string) {
                *string = scrubbed;
            }
        }
        _ => {}
    }
}

fn tool_event_body(value: Option<Value>) -> Option<Vec<u8>> {
    value.map(|mut value| {
        redact_tool_value(&mut value);
        serde_json::to_vec(&value).unwrap_or_default()
    })
}

/// Translate the native Claude Code/Codex hook payload into the tool ingest
/// contract. Hooks deliberately send their stdin unchanged so this stays the
/// single compatibility boundary for harness-specific payloads.
fn normalize_tool_event(event: &mut Value) {
    let Some(object) = event.as_object_mut() else {
        return;
    };
    if object.get("phase").and_then(Value::as_str).is_none() {
        let phase = match object.get("hook_event_name").and_then(Value::as_str) {
            Some("PreToolUse") => Some("start"),
            Some("PostToolUse") | Some("PostToolUseFailure") => Some("end"),
            _ => None,
        };
        if let Some(phase) = phase {
            object.insert("phase".into(), Value::String(phase.into()));
        }
    }
    if object.get("hook_event_name").and_then(Value::as_str) == Some("PostToolUseFailure") {
        object.insert("is_error".into(), Value::Bool(true));
    }
    for (canonical, aliases) in [
        (
            "tool_call_id",
            &["tool_use_id", "toolUseId", "tool_useID"][..],
        ),
        ("session_id", &["sessionId"][..]),
    ] {
        if object.get(canonical).and_then(Value::as_str).is_some() {
            continue;
        }
        if let Some(value) = aliases
            .iter()
            .find_map(|alias| object.get(*alias).and_then(Value::as_str))
        {
            object.insert(canonical.into(), Value::String(value.to_string()));
        }
    }
    for (canonical, native) in [("args", "tool_input"), ("result", "tool_response")] {
        if !object.contains_key(canonical) {
            if let Some(value) = object.get(native).cloned() {
                object.insert(canonical.into(), value);
            }
        }
    }
}

async fn tool_event(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::Json(mut event): axum::Json<Value>,
) -> Response {
    let (key_hash, key) = match authenticate_harness_event(&state, &headers) {
        Ok(auth) => auth,
        Err(response) => return response,
    };
    if let Err(error) = state.store.touch_run_key(&key_hash, now_ms()) {
        tracing::warn!(%error, "failed to touch harness key");
    }
    let harness = key.label.as_deref().unwrap_or_default();
    normalize_tool_event(&mut event);
    let phase = event["phase"].as_str().unwrap_or_default();
    if !matches!(
        phase,
        "start" | "end" | "turn_start" | "turn_end" | "agent_start" | "agent_end"
    ) {
        return error_response(StatusCode::BAD_REQUEST, "tool event phase is unsupported");
    }
    let session_id = match event["session_id"]
        .as_str()
        .filter(|id| valid_tool_event_id(id))
    {
        Some(id) => id.to_string(),
        None => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "tool event requires a safe session_id without '.'",
            )
        }
    };
    // Pi 0.80.6's AgentStartEvent and AgentEndEvent have no parent/child id;
    // acknowledge them without fabricating a session_lineage edge.
    if matches!(
        phase,
        "turn_start" | "turn_end" | "agent_start" | "agent_end"
    ) {
        return axum::Json(json!({"ok": true, "session_id": session_id, "lineage_updated": false}))
            .into_response();
    }
    let tool_call_id = match event["tool_call_id"]
        .as_str()
        .filter(|id| valid_tool_event_id(id))
    {
        Some(id) => id.to_string(),
        None => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "tool event requires a safe tool_call_id without '.'",
            )
        }
    };
    let tool_name = match event["tool_name"]
        .as_str()
        .filter(|name| !name.is_empty() && name.len() <= 200)
    {
        Some(name) => name.to_string(),
        None => return error_response(StatusCode::BAD_REQUEST, "tool event requires tool_name"),
    };
    let ts_ms = event["timestamp_ms"]
        .as_i64()
        .filter(|ts| *ts > 0)
        .unwrap_or_else(now_ms);
    let id = format!("tool-{}-{}", session_id, tool_call_id).replace(
        |c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_',
        "_",
    );
    let args_body_path = match tool_event_body(event.get("args").cloned()) {
        Some(bytes) => match state.store.write_body(&id, "tool-args.json", &bytes) {
            Ok(path) => Some(path),
            Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
        },
        None => None,
    };
    let result_body_path = match tool_event_body(event.get("result").cloned()) {
        Some(bytes) => match state.store.write_body(&id, "tool-result.json", &bytes) {
            Ok(path) => Some(path),
            Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
        },
        None => None,
    };
    let record = ToolCallRecord {
        id,
        harness: harness.to_string(),
        session_id,
        turn_id: event["turn_id"].as_str().map(String::from),
        tool_call_id,
        trace_id: event["trace_id"].as_str().map(String::from),
        tool_name,
        ts_start_ms: if phase == "start" {
            ts_ms
        } else {
            event["started_ms"].as_i64().unwrap_or(ts_ms)
        },
        ts_end_ms: (phase == "end").then_some(ts_ms),
        is_error: event["is_error"].as_bool(),
        exit_status: event["exit_status"].as_i64(),
        args_body_path,
        result_body_path,
    };
    match state.store.upsert_tool_call(&record) {
        Ok(()) => axum::Json(json!({"ok": true})).into_response(),
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

fn reroutable_error_class(class: ErrorClass) -> bool {
    matches!(class, ErrorClass::Capacity | ErrorClass::Server)
}

#[cfg(test)]
fn retryable_failover_status(status: reqwest::StatusCode) -> bool {
    reroutable_error_class(classify_error("unknown", Some(status.as_u16()), None))
}

fn no_substitute(headers: &HeaderMap) -> bool {
    headers
        .get("x-alexandria-no-substitute")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.trim() == "1")
}

fn configured_fallback(
    config: &SubstitutionConfig,
    requested_model: &str,
    current_model: &str,
    attempted_models: &HashSet<String>,
) -> Option<(Provider, String)> {
    if !config.enabled {
        return None;
    }
    config
        .fallbacks
        .get(requested_model)
        .or_else(|| config.fallbacks.get(current_model))?
        .iter()
        .find_map(|candidate| {
            (!attempted_models.contains(candidate)).then(|| route_model(candidate))
        })
        .and_then(|(provider, model)| provider.map(|provider| (provider, model)))
}

fn policy_covers(class: ErrorClass, policy: &ProtectionPolicy) -> bool {
    policy.enabled
        && (reroutable_error_class(class) || (class == ErrorClass::Auth && policy.reroute_on_auth))
}

/// Canonical names for the model aliases accepted by the Protection UI.
/// This only affects matching a policy key; the configured equivalent is still
/// routed exactly as written.
fn canonical_model_alias(model: &str) -> &str {
    // Clients/harnesses route by prefixed model ids (codex selects "alex/claude-fable-5",
    // some catalogs expose "claude-alex/…", and cross-format calls carry "openai/"/"anthropic/").
    // Strip the routing prefix before aliasing so equivalency keys match regardless of prefix.
    let model = model
        .strip_prefix("alex/")
        .or_else(|| model.strip_prefix("claude-alex/"))
        .or_else(|| model.strip_prefix("openai/"))
        .or_else(|| model.strip_prefix("anthropic/"))
        .unwrap_or(model);
    match model {
        "fable-5" | "claude-fable-5" => "claude-fable-5",
        "sol" | "gpt-5.6-sol" => "gpt-5.6-sol",
        "terra" | "gpt-5.6-terra" => "gpt-5.6-terra",
        _ => model,
    }
}

fn protection_equivalent(
    policy: &ProtectionPolicy,
    requested_model: &str,
    current_provider: Provider,
    attempted_models: &HashSet<String>,
) -> Option<(Provider, String)> {
    if !policy.enabled {
        return None;
    }
    let equivalents = policy.equivalencies.get(requested_model).or_else(|| {
        let requested_alias = canonical_model_alias(requested_model);
        policy
            .equivalencies
            .iter()
            .find(|(model, _)| canonical_model_alias(model) == requested_alias)
            .map(|(_, equivalents)| equivalents)
    })?;
    equivalents.iter().find_map(|(provider, model)| {
        let candidate_provider = match provider.as_str() {
            "anthropic" => Provider::Anthropic,
            "openai" => Provider::Openai,
            "xai" => Provider::Xai,
            "gemini" => Provider::Gemini,
            "openrouter" => Provider::Openrouter,
            "exo" => Provider::Exo,
            "kimi" => Provider::Kimi,
            _ => return None,
        };
        (candidate_provider != current_provider && !attempted_models.contains(model))
            .then(|| (candidate_provider, model.clone()))
    })
}

fn take_pending_injection(
    state: &AppState,
    session: Option<&str>,
    run_id: Option<&str>,
) -> Option<PendingInjection> {
    let mut map = state
        .pending_injections
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let key = session.or(run_id)?;
    let queue = map.get_mut(key)?;
    let mut item = queue.first()?.clone();
    if item.count > 1 {
        queue[0].count -= 1;
    } else {
        queue.remove(0);
    }
    if queue.is_empty() {
        map.remove(key);
    }
    item.count = item.count.min(1);
    Some(item)
}

/// `Vault` deliberately has a degraded-mode selector for ordinary requests
/// (it can pick the soonest-expiring cooldown when every account is down).
/// A retry must not use that escape hatch: failover only moves to a genuinely
/// ready, non-reserve-blocked account.
fn retry_account_eligible(state: &AppState, account: &Account) -> bool {
    let now = now_ms();
    if account.cooldown_until_ms.is_some_and(|until| until > now) {
        return false;
    }
    let policy = state.vault.policy(account.provider);
    !routing_reserve_blocked(account, routing_reserve_pct(account, &policy), now / 1000)
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
    let mut local_key_request = false;
    let client_fingerprint = match client_key(&headers) {
        Some(k)
            if state
                .local_key
                .read()
                .map(|local| k == *local)
                .unwrap_or(false) =>
        {
            local_key_request = true;
            key_fingerprint(&k)
        }
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
    let plugin_headers: serde_json::Map<String, Value> = headers
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.as_str().to_string(), Value::String(value.to_string())))
        })
        .collect();
    let plugin_request = state.plugins.invoke(
        "on_request",
        json!({"headers": plugin_headers, "body": body_json}),
    );
    if let Some(mutated) = plugin_request.get("body").filter(|value| value.is_object()) {
        body_json = mutated.clone();
    }
    let (reasoning_effort, thinking_budget) = trace_reasoning_fields(&body_json);
    trace.reasoning_effort = reasoning_effort;
    trace.thinking_budget = thinking_budget;
    let genuine_claude_code = is_genuine_claude_code_request(format, &headers, &body_json);

    let claude_root_session = headers
        .get("x-claude-code-session-id")
        .and_then(|v| v.to_str().ok())
        .filter(|value| !value.is_empty())
        .map(String::from);
    let claude_agent_id = headers
        .get("x-claude-code-agent-id")
        .and_then(|v| v.to_str().ok())
        .filter(|value| !value.is_empty())
        .map(String::from);
    if trace.harness.as_deref().is_some_and(|harness| {
        matches!(
            harness.to_ascii_lowercase().as_str(),
            "claude" | "claude-code"
        )
    }) {
        if let (Some(agent_id), Some(parent_id)) = (
            claude_agent_id.as_deref(),
            headers
                .get("x-claude-code-parent-agent-id")
                .and_then(|v| v.to_str().ok())
                .filter(|value| !value.is_empty())
                .or(claude_root_session.as_deref()),
        ) {
            let event = json!({
                "hook_event_name": "SubagentStart",
                "session_id": parent_id,
                "agent_id": agent_id,
            });
            if let Err(error) = state.store.record_harness_event("claude", &event, now_ms()) {
                tracing::warn!(%error, %agent_id, %parent_id, "could not record Claude request lineage");
            }
        }
    }

    trace.session_id = claude_agent_id
        .or_else(|| {
            headers
                .get("x-session-id")
                .and_then(|v| v.to_str().ok())
                .map(String::from)
        })
        .or_else(|| {
            headers
                .get("session_id")
                .and_then(|v| v.to_str().ok())
                .map(String::from)
        })
        .or(claude_root_session)
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
    // `alex/<model>` is the local alias published for an enabled Exo model.
    // An explicit `exo/<model>` must also be enabled; this prevents an
    // arbitrary caller from using Alexandria as an unintended LAN proxy.
    let provider = match routed_provider {
        Some(Provider::Exo) if !exo_model_enabled(&state, &routed_model) => {
            return error_response(StatusCode::NOT_FOUND, "Exo model is not enabled")
        }
        Some(provider) => provider,
        None if exo_model_enabled(&state, &routed_model) => Provider::Exo,
        None => format.default_provider(),
    };
    trace.requested_model = Some(requested_model.clone());
    trace.routed_model = Some(routed_model.clone());
    trace.upstream_provider = Some(provider.as_str().into());
    trace.streamed = Some(body_json["stream"].as_bool().unwrap_or(false));
    let substitution_disabled = no_substitute(&headers);
    let paused_mode = paused_provider_mode(&state, provider);
    // A provider pause has precedence over a one-shot fixture: it represents
    // the operator taking the provider out of service, while fixtures remain
    // queued for after it is resumed.
    let simulation = if let Some(mode) = paused_mode {
        Some(Ok((
            mode.status(),
            "provider_paused".to_string(),
            None,
            None,
            Some(mode),
        )))
    } else {
        let pending_injection =
            take_pending_injection(&state, trace.session_id.as_deref(), trace.run_id.as_deref());
        headers
            .get("x-alexandria-simulate-error")
            .and_then(|value| value.to_str().ok())
            .map(|value| {
                parse_simulated_error(value).map(|(status, kind)| (status, kind, None, None, None))
            })
            .or_else(|| {
                pending_injection.map(|pending| {
                    Ok((
                        pending.fixture.status,
                        pending.fixture.error_kind,
                        Some(pending.fixture.body.into_bytes()),
                        Some(pending.fixture.name),
                        None,
                    ))
                })
            })
    };
    if let Some(simulation) = simulation {
        if headers.get("x-alexandria-simulate-error").is_some()
            && !local_key_request
            && run_key.as_ref().map_or(true, |key| key.kind != "harness")
        {
            return error_response(
                StatusCode::FORBIDDEN,
                "x-alexandria-simulate-error requires a local or harness run key",
            );
        }
        let (status, kind, injected_body, fixture_name, pause_mode) = match simulation {
            Ok(value) => value,
            Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
        };
        let message = if let Some(mode) = pause_mode {
            format!("provider deliberately paused in {} mode", mode.as_str())
        } else {
            format!("simulated upstream error: {kind}")
        };
        trace.status = Some(status as i64);
        trace.error_kind = Some(kind.clone());
        trace.error_code = Some(status.to_string());
        trace.error_class = Some(
            pause_mode
                .map(PauseMode::error_class)
                .unwrap_or_else(|| classify_error(provider.as_str(), Some(status), Some(&kind)))
                .as_str()
                .into(),
        );
        trace.error = Some(format!("{kind}: {message}"));
        trace.tags = merge_trace_note(
            trace.tags.take(),
            if pause_mode.is_some() {
                "provider_paused"
            } else {
                "simulated"
            },
            pause_mode.map(PauseMode::as_str).unwrap_or("true"),
        );
        if fixture_name.is_some() {
            trace.injected = true;
            trace.fixture_name = fixture_name;
            trace.tags = merge_trace_note(trace.tags.take(), "injected", "true");
        }
        // Simulation deliberately never dispatches upstream traffic, but it
        // follows the real account/model selection policy so failover can be
        // tested with local or harness credentials alone.
        let class = pause_mode
            .map(PauseMode::error_class)
            .unwrap_or_else(|| classify_error(provider.as_str(), Some(status), Some(&kind)));
        // This deliberately calls the same account-scoped re-auth dispatcher
        // used while handling a real managed OAuth failure. The pause itself
        // never contacts upstream or mutates credentials.
        if pause_mode == Some(PauseMode::LoggedOut) {
            if let Some(account) = state.vault.list_cached().into_iter().find(|account| {
                account.provider == provider
                    && account.kind == "oauth"
                    && account.status == "active"
                    && !account.paused
            }) {
                emit_reauth_notification_for_account(&state, &account);
            }
        }
        let protection = state
            .protection
            .read()
            .map(|policy| policy.clone())
            .unwrap_or_default();
        if (reroutable_error_class(class) || policy_covers(class, &protection))
            && !substitution_disabled
        {
            let mut attempted_accounts = HashSet::new();
            let mut attempted_models = HashSet::from([routed_model.clone()]);
            let mut attempts = Vec::<Value>::new();
            let mut current_provider = provider;
            let mut current_model = routed_model.clone();
            // A forced simulation never opens an upstream socket, but retain
            // the policy's retry rung in the trace so the lab can prove the
            // exact escalation plan deterministically.
            if protection.enabled {
                for retry in 0..protection.retries {
                    attempts.push(
                        json!({"rung": "retry_same", "retry": retry + 1, "model": current_model}),
                    );
                }
            }
            let prefer_oauth = format != ClientFormat::OpenaiChat;
            let mut account = state
                .vault
                .account_for_excluding(current_provider, prefer_oauth, &attempted_accounts)
                .await
                .ok();
            while let Some(current_account) = account {
                attempted_accounts.insert(current_account.id.clone());
                attempts.push(json!({"account_id": current_account.id, "model": current_model}));
                // Cross-provider auth protection must not hide the fact that
                // the original managed subscription needs a fresh login.
                if class == ErrorClass::Auth {
                    emit_reauth_notification_for_account(&state, &current_account);
                }
                let _ = state
                    .vault
                    .mark_cooldown(&current_account.id, now_ms() + 60_000)
                    .await;
                let next = state
                    .vault
                    .account_for_excluding(current_provider, prefer_oauth, &attempted_accounts)
                    .await
                    .ok()
                    .filter(|candidate| {
                        !attempted_accounts.contains(&candidate.id)
                            && retry_account_eligible(&state, candidate)
                    });
                if let Some(next) = next {
                    trace.substituted = true;
                    trace
                        .original_model
                        .get_or_insert_with(|| requested_model.clone());
                    trace
                        .original_account_id
                        .get_or_insert_with(|| current_account.id.clone());
                    trace.substitution_reason = Some(class.as_str().into());
                    trace.served_model = Some(current_model.clone());
                    trace.served_account_id = Some(next.id.clone());
                    bind_trace_account(&state.store, &mut trace, &next);
                    account = Some(next);
                    continue;
                }
                let fallback = if reroutable_error_class(class) {
                    configured_fallback(
                        &state.substitution,
                        &requested_model,
                        &current_model,
                        &attempted_models,
                    )
                } else {
                    None
                }
                .or_else(|| {
                    protection_equivalent(
                        &protection,
                        &requested_model,
                        current_provider,
                        &attempted_models,
                    )
                });
                let Some((fallback_provider, fallback_model)) = fallback else {
                    bind_trace_account(&state.store, &mut trace, &current_account);
                    break;
                };
                current_provider = fallback_provider;
                current_model = fallback_model;
                attempted_models.insert(current_model.clone());
                account = state
                    .vault
                    .account_for_excluding(
                        current_provider,
                        format != ClientFormat::OpenaiChat,
                        &attempted_accounts,
                    )
                    .await
                    .ok()
                    .filter(|account| retry_account_eligible(&state, account));
                if let Some(next) = account.as_ref() {
                    trace.substituted = true;
                    trace
                        .original_model
                        .get_or_insert_with(|| requested_model.clone());
                    trace
                        .original_account_id
                        .get_or_insert_with(|| current_account.id.clone());
                    trace.substitution_reason = Some(class.as_str().into());
                    trace.served_model = Some(current_model.clone());
                    trace.served_account_id = Some(next.id.clone());
                    trace.routed_model = Some(current_model.clone());
                    trace.upstream_provider = Some(current_provider.as_str().into());
                    bind_trace_account(&state.store, &mut trace, next);
                }
            }
            if trace.substituted {
                trace.attempts = serde_json::to_string(&attempts).ok();
            }
        }
        trace.ts_response_ms = Some(now_ms());
        let response_body =
            injected_body.unwrap_or_else(|| simulated_error_body(format, status, &kind, &message));
        finalize_trace(&state, trace, &body, None, Some(&response_body));
        let mut response = Response::builder()
            .status(StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR))
            .header("content-type", "application/json")
            .header("x-alexandria-trace-id", &trace_id);
        if let Some(mode) = pause_mode {
            response = response.header(
                "x-alexandria-paused",
                format!("{}:{}", provider.as_str(), mode.as_str()),
            );
        }
        return response
            .body(Body::from(response_body))
            .unwrap_or_else(|e| error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()));
    }
    let in_flight = InFlight::new(
        &state,
        routed_model.clone(),
        trace.session_id.clone(),
        trace.harness.clone(),
    );

    // Resolve verified Codex/Claude children to the lineage root so every
    // descendant prefers the same upstream account while retaining its own
    // session id in traces.
    let affinity_session_id = trace.session_id.as_ref().map(|session_id| {
        if trace.harness.as_deref().is_some_and(|harness| {
            matches!(harness, "codex" | "claude" | "claude-code")
        }) {
            let lineage_harness = if trace.harness.as_deref() == Some("codex") {
                "codex"
            } else {
                "claude"
            };
            state
                .store
                .session_lineage_root(lineage_harness, session_id)
                .unwrap_or_else(|error| {
                    tracing::warn!(%error, %session_id, %lineage_harness, "could not resolve harness session lineage");
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
    let mut attempted_models = HashSet::from([routed_model.clone()]);
    let mut attempt_records = Vec::<Value>::new();
    let mut current_provider = provider;
    let mut current_model = routed_model.clone();
    let mut plan = match plan_upstream(
        &state,
        format,
        current_provider,
        &current_model,
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
    trace.via_dario = plan.via_dario;
    trace.dario_generation = plan.dario_generation.clone();
    if let Some(reason) = &plan.dario_fallback_reason {
        trace.tags = merge_trace_note(trace.tags.take(), "dario_fallback", reason);
    }
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

    // Dario's child key is deliberately connection-only. Trace attribution,
    // known-account upserts, and billing continue to use plan.account.
    let mut account = plan
        .connection_account
        .clone()
        .unwrap_or_else(|| plan.account.clone());
    let mut upstream_resp = None;
    'accounts: loop {
        if !attempted_accounts.insert(account.id.clone()) {
            tracing::error!(account = %account.id, "refusing to retry an already-attempted account");
            break;
        }
        attempt_records.push(json!({"account_id": plan.account.id, "model": current_model}));

        let mut forced_oauth_refresh = false;
        loop {
            let mut up_headers = match upstream_headers(&account, &headers, genuine_claude_code) {
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
                    trace.error_kind = Some("upstream_unreachable".into());
                    trace.error_class = Some(ErrorClass::Network.as_str().into());
                    finalize_trace(&state, trace, &body, Some(&plan.body), None);
                    return error_response(StatusCode::BAD_GATEWAY, &msg);
                }
            };

            if account.kind == "oauth" {
                if let Some(snapshot) =
                    routing_limits_from_headers(account.provider, resp.headers())
                {
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
                        // The refresh path has the managed account in scope.
                        // Send now; final trace classification will attempt
                        // the same event for the retained 401, and the
                        // dispatcher coalesces that duplicate.
                        emit_reauth_notification_for_account(&state, &account);
                    }
                }
            }

            let error_class = classify_error(
                current_provider.as_str(),
                Some(resp.status().as_u16()),
                None,
            );
            let protection = state
                .protection
                .read()
                .map(|policy| policy.clone())
                .unwrap_or_default();
            if (reroutable_error_class(error_class) || policy_covers(error_class, &protection))
                && !substitution_disabled
                && account.kind != "dario"
            {
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
                    current_provider,
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
                    current_provider,
                    &current_model,
                    &mut retry_body_json,
                    &body,
                    &trace_id,
                    &attempted_accounts,
                    affinity_session_id.as_deref(),
                    &headers,
                )
                .await
                {
                    Ok(next_plan)
                        if !attempted_accounts.contains(&next_plan.account.id)
                            && retry_account_eligible(&state, &next_plan.account) =>
                    {
                        trace.substituted = true;
                        trace
                            .original_model
                            .get_or_insert_with(|| requested_model.clone());
                        trace
                            .original_account_id
                            .get_or_insert_with(|| plan.account.id.clone());
                        trace.substitution_reason = Some(error_class.as_str().into());
                        plan = next_plan;
                        account = plan
                            .connection_account
                            .clone()
                            .unwrap_or_else(|| plan.account.clone());
                        bind_trace_account(&state.store, &mut trace, &plan.account);
                        trace.upstream_format = Some(plan.upstream_format.into());
                        trace.via_dario = plan.via_dario;
                        trace.dario_generation = plan.dario_generation.clone();
                        trace.served_model = Some(current_model.clone());
                        trace.served_account_id = Some(plan.account.id.clone());
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
                let fallback = if reroutable_error_class(error_class) {
                    configured_fallback(
                        &state.substitution,
                        &requested_model,
                        &current_model,
                        &attempted_models,
                    )
                } else {
                    None
                }
                .or_else(|| {
                    protection_equivalent(
                        &protection,
                        &requested_model,
                        current_provider,
                        &attempted_models,
                    )
                });
                if let Some((fallback_provider, fallback_model)) = fallback {
                    let mut retry_body_json = original_body_json.clone();
                    match plan_upstream(
                        &state,
                        format,
                        fallback_provider,
                        &fallback_model,
                        &mut retry_body_json,
                        &body,
                        &trace_id,
                        &attempted_accounts,
                        affinity_session_id.as_deref(),
                        &headers,
                    )
                    .await
                    {
                        Ok(next_plan)
                            if !attempted_accounts.contains(&next_plan.account.id)
                                && retry_account_eligible(&state, &next_plan.account) =>
                        {
                            trace.substituted = true;
                            trace
                                .original_model
                                .get_or_insert_with(|| requested_model.clone());
                            trace
                                .original_account_id
                                .get_or_insert_with(|| plan.account.id.clone());
                            trace.substitution_reason = Some(error_class.as_str().into());
                            current_provider = fallback_provider;
                            current_model = fallback_model;
                            attempted_models.insert(current_model.clone());
                            plan = next_plan;
                            account = plan
                                .connection_account
                                .clone()
                                .unwrap_or_else(|| plan.account.clone());
                            bind_trace_account(&state.store, &mut trace, &plan.account);
                            trace.upstream_provider = Some(current_provider.as_str().into());
                            trace.routed_model = Some(current_model.clone());
                            trace.upstream_format = Some(plan.upstream_format.into());
                            trace.via_dario = plan.via_dario;
                            trace.dario_generation = plan.dario_generation.clone();
                            trace.served_model = Some(current_model.clone());
                            trace.served_account_id = Some(plan.account.id.clone());
                            trace.billing_bucket = Some(
                                if account.kind == "oauth" || account.kind == "dario" {
                                    "subscription"
                                } else {
                                    "api"
                                }
                                .into(),
                            );
                            tracing::warn!(account = %account.id, model = %current_model, "retrying with configured model fallback");
                            continue 'accounts;
                        }
                        Ok(next_plan) => {
                            tracing::warn!(account = %next_plan.account.id, "configured fallback selected an attempted account")
                        }
                        Err((status, msg)) => {
                            tracing::warn!(status = %status, error = %msg, model = %fallback_model, "configured fallback has no eligible account")
                        }
                    }
                }
            }

            upstream_resp = Some(resp);
            break 'accounts;
        }
    }
    let upstream_resp = upstream_resp.expect("upstream response after retry loop");
    if trace.substituted {
        trace.served_model = Some(current_model.clone());
        trace.served_account_id = Some(plan.account.id.clone());
        trace.attempts = serde_json::to_string(&attempt_records).ok();
    }
    bind_trace_account(&state.store, &mut trace, &plan.account);

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
                trace.error_kind = Some("upstream_unreachable".into());
                trace.error_class = Some(ErrorClass::Network.as_str().into());
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
                trace.error_kind = Some("upstream_unreachable".into());
                trace.error_class = Some(ErrorClass::Network.as_str().into());
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
        let sse_error = sse_error_observer.as_mut().and_then(|observer| {
            observer.finish();
            observer.upstream_error()
        });
        if trace.error.is_none() {
            trace.error = sse_error
                .as_ref()
                .map(UpstreamSseError::trace_message)
                .or(stream_error.clone());
        }
        if trace.error_kind.is_none() {
            trace.error_kind = sse_error
                .as_ref()
                .map(|error| error.kind.clone())
                .or_else(|| {
                    stream_error.as_deref().map(|message| {
                        if message.starts_with("client disconnected") {
                            "client_disconnect"
                        } else if message.contains("idle timeout") {
                            "idle_timeout"
                        } else {
                            "stream_error"
                        }
                        .to_string()
                    })
                });
        }
        if trace.error.is_some() && trace.error_class.is_none() {
            trace.error_class = Some(
                classify_error(
                    trace.upstream_provider.as_deref().unwrap_or("unknown"),
                    trace.status.and_then(|value| u16::try_from(value).ok()),
                    trace.error_kind.as_deref(),
                )
                .as_str()
                .into(),
            );
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
struct ParsedError {
    kind: Option<String>,
    code: Option<String>,
    message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ErrorClass {
    Auth,
    Capacity,
    BadRequest,
    Server,
    ClientDisconnect,
    Network,
    Other,
}

impl ErrorClass {
    fn as_str(self) -> &'static str {
        match self {
            Self::Auth => "auth",
            Self::Capacity => "capacity",
            Self::BadRequest => "bad_request",
            Self::Server => "server",
            Self::ClientDisconnect => "client_disconnect",
            Self::Network => "network",
            Self::Other => "other",
        }
    }
}

/// The sole error taxonomy used by trace storage and later substitution work.
fn classify_error(_provider: &str, status: Option<u16>, error_kind: Option<&str>) -> ErrorClass {
    let kind = error_kind.unwrap_or("").to_ascii_lowercase();
    if kind == "client_disconnect" {
        return ErrorClass::ClientDisconnect;
    }
    if matches!(
        kind.as_str(),
        "stream_error" | "idle_timeout" | "upstream_unreachable"
    ) || kind.contains("timeout")
        || kind.contains("connect")
        || kind.contains("reset")
        || kind.contains("early-eof")
    {
        return ErrorClass::Network;
    }
    if matches!(status, Some(401 | 403))
        || matches!(
            kind.as_str(),
            "authentication_error"
                | "permission_error"
                | "invalid_api_key"
                | "token_refresh_failure"
                | "token-refresh-failure"
        )
    {
        return ErrorClass::Auth;
    }
    if status == Some(429)
        || matches!(
            kind.as_str(),
            "rate_limit_error" | "overloaded_error" | "insufficient_quota" | "quota_exceeded"
        )
        || kind.contains("at capacity")
    {
        return ErrorClass::Capacity;
    }
    if matches!(status, Some(400 | 404 | 422))
        || kind == "invalid_request_error"
        || kind.contains("model_not_found")
        || kind.contains("model-not-found")
    {
        return ErrorClass::BadRequest;
    }
    if status.is_some_and(|status| status >= 500)
        || matches!(kind.as_str(), "api_error" | "internal_server_error")
    {
        return ErrorClass::Server;
    }
    ErrorClass::Other
}

fn json_string(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(String::from)
        .or_else(|| value.as_i64().map(|n| n.to_string()))
}

/// Reads the native provider error envelope without translating it.  `format`
/// is the upstream wire format, including the two OpenAI variants.
fn parse_upstream_error(format: &str, _status: u16, body: &[u8]) -> Option<ParsedError> {
    let value: Value = serde_json::from_slice(body).ok()?;
    let error = match format {
        "anthropic" => value.get("error")?,
        "openai-chat" | "openai-responses" => value.get("error")?,
        "gemini" => value
            .get("error")
            .or_else(|| value.pointer("/response/error"))?,
        _ => value.get("error")?,
    };
    if !error.is_object() {
        return None;
    }
    Some(ParsedError {
        kind: json_string(&error["type"]).or_else(|| json_string(&error["status"])),
        code: json_string(&error["code"]),
        message: json_string(&error["message"]),
    })
}

fn capture_response_error(trace: &mut TraceRecord, body: &[u8]) {
    let Some(status) = trace.status.and_then(|status| u16::try_from(status).ok()) else {
        return;
    };
    if (200..300).contains(&status) {
        return;
    }
    let parsed = trace
        .upstream_format
        .as_deref()
        .and_then(|format| parse_upstream_error(format, status, body));
    let fallback_kind = format!("http_status_{status}");
    let kind = parsed
        .as_ref()
        .and_then(|error| error.kind.clone())
        .unwrap_or(fallback_kind);
    let code = parsed
        .as_ref()
        .and_then(|error| error.code.clone())
        .unwrap_or_else(|| status.to_string());
    if trace.error_kind.is_none() {
        trace.error_kind = Some(kind.clone());
    }
    if trace.error_code.is_none() {
        trace.error_code = Some(code);
    }
    if trace.error.is_none() {
        let message = parsed
            .and_then(|error| error.message)
            .unwrap_or_else(|| format!("upstream returned HTTP {status}"));
        trace.error = Some(format!("{kind}: {message}"));
    }
    if trace.error_class.is_none() {
        trace.error_class = Some(
            classify_error(
                trace.upstream_provider.as_deref().unwrap_or("unknown"),
                Some(status),
                trace.error_kind.as_deref(),
            )
            .as_str()
            .into(),
        );
    }
}

fn parse_simulated_error(value: &str) -> Result<(u16, String), String> {
    let (status, kind) = value.trim().split_once(':').unwrap_or((value.trim(), ""));
    let status: u16 = status
        .parse()
        .map_err(|_| "x-alexandria-simulate-error must be STATUS or STATUS:kind".to_string())?;
    if !(400..600).contains(&status) {
        return Err("x-alexandria-simulate-error status must be 400-599".into());
    }
    let kind = if kind.trim().is_empty() {
        format!("http_status_{status}")
    } else {
        kind.trim().to_string()
    };
    Ok((status, kind))
}

fn simulated_error_body(format: ClientFormat, status: u16, kind: &str, message: &str) -> Vec<u8> {
    let value = match format {
        ClientFormat::AnthropicMessages => json!({
            "type": "error", "error": {"type": kind, "message": message}
        }),
        ClientFormat::OpenaiChat | ClientFormat::OpenaiResponses => json!({
            "error": {"type": kind, "code": status.to_string(), "message": message}
        }),
        ClientFormat::GeminiGenerate => json!({
            "error": {"code": status, "status": kind, "message": message}
        }),
    };
    serde_json::to_vec(&value).unwrap_or_default()
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

    fn upstream_error(&self) -> Option<UpstreamSseError> {
        self.error.clone()
    }

    #[cfg(test)]
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
                self.error =
                    upstream_sse_error(self.upstream_format, self.event_name.as_deref(), &value);
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
    emit_reauth_notification(state, &trace);
    let store = &state.store;
    if let Some(resp) = resp_body {
        capture_response_error(&mut trace, resp);
    }
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
        let _ = state.plugins.invoke(
            "on_trace",
            serde_json::to_value(&trace).unwrap_or(Value::Null),
        );
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

/// This is deliberately attached to trace finalization, after every response
/// shape has been classified (regular JSON, translated responses, and SSE).
/// It also covers a forced OAuth refresh failure: the retained upstream 401 is
/// finalized as the existing `auth` class. The dispatcher only schedules work,
/// so notification delivery cannot delay this request or stream finalization.
fn emit_reauth_notification(state: &AppState, trace: &TraceRecord) {
    let Some(event) = reauth_notification_event(state, trace) else {
        return;
    };
    if let Ok(notifications) = state.notifications.read() {
        notifications.emit(event);
    }
}

/// Dario authenticates inside its Claude Code child rather than through the
/// proxy's OAuth path.  Its supervisor calls this when its readiness probe
/// receives a confirmed 401; the shared dispatcher supplies the cooldown.
pub fn emit_dario_reauth_notification(state: &AppState) {
    if let Ok(notifications) = state.notifications.read() {
        notifications.emit(notify::NotificationEvent {
            level: notify::NotificationLevel::Warn,
            category: "reauth".into(),
            title: "Dario needs re-authentication".into(),
            body: "Dario is down — your Claude Code login needs re-auth. Run `claude` login (or tap Reauth Dario).".into(),
            account: notify::NotificationAccount {
                provider: "anthropic".into(),
                label: Some("Dario".into()),
            },
            action_url: Some("claude login".into()),
            ts: now_ms(),
        });
    }
}

fn emit_reauth_notification_for_account(state: &AppState, account: &Account) {
    if account.kind != "oauth" {
        return;
    }
    if let Ok(notifications) = state.notifications.read() {
        notifications.emit(reauth_notification_event_for_account(account));
    }
}

fn reauth_notification_event(
    state: &AppState,
    trace: &TraceRecord,
) -> Option<notify::NotificationEvent> {
    if trace.error_class.as_deref() != Some("auth") {
        return None;
    }
    let Some(account_id) = trace.account_id.as_deref() else {
        return None;
    };
    let Some(account) = state
        .vault
        .list_cached()
        .into_iter()
        .find(|account| account.id == account_id && account.kind == "oauth")
    else {
        return None;
    };
    Some(reauth_notification_event_for_account(&account))
}

fn reauth_notification_event_for_account(account: &Account) -> notify::NotificationEvent {
    let provider = account.provider.as_str().to_string();
    let label = account
        .email()
        .or(account.label.clone())
        .or_else(|| (!account.name.trim().is_empty()).then(|| account.name.clone()));
    notify::NotificationEvent {
        level: notify::NotificationLevel::Warn,
        category: "reauth".into(),
        title: format!("{} needs re-authentication", provider),
        body: reauth_body_for(account.provider),
        account: notify::NotificationAccount {
            provider: provider.clone(),
            label,
        },
        action_url: Some(reauth_action_for(account.provider)),
        ts: now_ms(),
    }
}

/// Provider-specific, secret-free guidance for the re-auth notification body.
/// Anthropic keeps the Dario/Claude Code phrasing added in beta.5; the rest use
/// the generic managed-subscription message. The body never contains tokens.
fn reauth_body_for(provider: Provider) -> String {
    match provider {
        Provider::Anthropic => "Your Claude (Anthropic) login is logged out — its OAuth token expired and could not be refreshed. Re-authenticate with `alex auth login anthropic` (or `claude` login for Dario).".into(),
        _ => "This managed subscription is logged out — its OAuth token expired and could not be refreshed. Re-authenticate it before retrying.".into(),
    }
}

fn reauth_action_for(provider: Provider) -> String {
    format!("alex auth login {}", provider.as_str())
}

/// Grace period after `expires_at_ms` before the watchdog treats an idle token
/// as dead. Absorbs clock skew and the normal just-in-time refresh window so a
/// token that is merely about to expire is never mistaken for a logout.
const REAUTH_EXPIRY_GRACE_MS: i64 = 60_000;

/// Proactive logout watchdog. Runs on a lightweight daemon timer (independent
/// of the heartbeat/ping loop, so it fires even when heartbeats are disabled)
/// and closes the gap where an OAuth token expires while the proxy is idle: no
/// live request means the request-path re-auth notification never fires.
///
/// For each active managed OAuth account whose access token has expired past a
/// small grace window it distinguishes a *silently refreshable* token (which it
/// refreshes in place, no alert) from a genuinely *dead* one — no refresh token
/// at all, or a refresh the provider rejects with invalid_grant — and only for
/// the latter does it emit `emit_reauth_notification_for_account`. That reuses
/// the exact same dispatcher (and its cooldown) as the live 401 path, so a
/// persistently-dead account alerts about once per cooldown window rather than
/// every tick, and the live/proactive events coalesce. The per-account
/// needs-reauth flag is set on death and cleared on a successful refresh so the
/// admin UI reflects it and the next fresh logout alerts again.
pub async fn reauth_watch_once(state: &Arc<AppState>) {
    let now = now_ms();
    for account in state.vault.list().await {
        // Only managed OAuth logins expire and need refreshing. API-key
        // accounts (openrouter/amp/exo) never reach here; paused or non-active
        // accounts are intentionally left alone.
        if account.kind != "oauth" || account.paused || account.status != "active" {
            continue;
        }
        let Some(expires_at) = account.expires_at_ms else {
            // Unknown expiry: never claim a logout we cannot prove.
            continue;
        };
        if expires_at > now - REAUTH_EXPIRY_GRACE_MS {
            // Still valid (or only within the normal refresh margin). If it was
            // previously flagged as logged out, a fresh login/refresh has
            // recovered it: clear the flag so the next genuine logout alerts.
            if account.needs_reauth() {
                mark_account_needs_reauth(state, &account.id, false).await;
            }
            continue;
        }
        let has_refresh_token = account
            .refresh_token
            .as_deref()
            .map(|token| !token.trim().is_empty())
            .unwrap_or(false);
        if !has_refresh_token {
            // Expired with nothing to refresh from: a confirmed logout.
            mark_account_needs_reauth(state, &account.id, true).await;
            emit_reauth_notification_for_account(state, &account);
            continue;
        }
        // Has a refresh token: the only reliable way to tell a live login from a
        // revoked one is to try. Success silently recovers the account (no
        // alert); an invalid_grant is a confirmed logout; a transient failure is
        // ignored so a network blip never cries wolf.
        match state.vault.refresh(&account.id, true).await {
            Ok(_) => {
                // Silently recovered. Clear a prior flag (a refresh copies the
                // old account_meta forward, so an earlier logout mark can
                // survive the token swap and must be reset here).
                if account.needs_reauth() {
                    mark_account_needs_reauth(state, &account.id, false).await;
                }
            }
            Err(error) if alex_auth::refresh_error_needs_reauth(&error) => {
                mark_account_needs_reauth(state, &account.id, true).await;
                emit_reauth_notification_for_account(state, &account);
            }
            Err(error) => {
                tracing::warn!(
                    account = %account.id,
                    "proactive reauth refresh failed transiently; not alerting: {error}"
                );
            }
        }
    }
}

/// Persist (or clear) the display-only needs-reauth flag on an account so the
/// admin/accounts view and UI reflect the logout. Never stores credentials.
async fn mark_account_needs_reauth(state: &AppState, account_id: &str, needs: bool) {
    if let Err(error) = state
        .vault
        .set_account_meta(account_id, "needs_reauth", json!(needs))
        .await
    {
        tracing::warn!(account = %account_id, %error, "could not update needs_reauth flag");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn kimi_usage_payload_maps_windows_and_credits() {
        let payload = json!({
            "usage": {"limit": 1000, "used": 250, "name": "Weekly", "reset_at": "2026-07-20T00:00:00Z"},
            "limits": [
                {"detail": {"limit": 40, "remaining": 10, "title": "5h"}, "window": {}}
            ],
            "boosterWallet": {"balance": {"type": "BOOSTER", "amount": 5}, "currency": "USD"}
        });
        let snap = parse_kimi_usage_payload(&payload);
        assert_eq!(snap["provider"], json!("kimi"));
        let windows = snap["windows"].as_array().unwrap();
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0]["label"], json!("Weekly"));
        assert_eq!(windows[0]["used"], json!(250));
        assert_eq!(windows[0]["limit"], json!(1000));
        assert_eq!(windows[0]["used_pct"], json!(25.0));
        // Second window derives used from limit - remaining (40 - 10 = 30).
        assert_eq!(windows[1]["label"], json!("5h"));
        assert_eq!(windows[1]["used"], json!(30));
        assert!(!snap["credits"].is_null());
    }

    #[test]
    fn kimi_usage_payload_degrades_on_empty() {
        let snap = parse_kimi_usage_payload(&json!({}));
        assert_eq!(snap["windows"].as_array().unwrap().len(), 0);
        assert!(snap["credits"].is_null());
    }

    async fn collect_test_webhook(
        State(received): State<Arc<std::sync::Mutex<Vec<Value>>>>,
        axum::Json(payload): axum::Json<Value>,
    ) -> axum::Json<Value> {
        received.lock().unwrap().push(payload);
        axum::Json(json!({}))
    }

    async fn webhook_sink() -> (
        String,
        Arc<std::sync::Mutex<Vec<Value>>>,
        tokio::task::JoinHandle<()>,
    ) {
        let received = Arc::new(std::sync::Mutex::new(Vec::new()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new()
            .route("/", post(collect_test_webhook))
            .with_state(received.clone());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{address}/"), received, server)
    }

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

    fn test_state_with_substitution(name: &str, substitution: SubstitutionConfig) -> Arc<AppState> {
        let dir = tmpdir(name);
        let store = Arc::new(Store::open(dir.join("store")).unwrap());
        let vault = Arc::new(Vault::open(dir.join("vault")).unwrap());
        build_state_with_substitution(
            "alx-local".into(),
            vault,
            store,
            None,
            "http://127.0.0.1:4100".into(),
            Duration::from_secs(15 * 60),
            substitution,
        )
    }

    fn test_state_with_dario(name: &str, dario: Option<Arc<dyn DarioRouter>>) -> Arc<AppState> {
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

    #[derive(Default)]
    struct RecordingProtectionPolicyPersister {
        policies: std::sync::Mutex<Vec<ProtectionPolicy>>,
    }

    impl ProtectionPolicyPersister for RecordingProtectionPolicyPersister {
        fn persist(&self, policy: &ProtectionPolicy) -> std::result::Result<(), String> {
            self.policies.lock().unwrap().push(policy.clone());
            Ok(())
        }
    }

    #[derive(Default)]
    struct RecordingExoPersister {
        configs: std::sync::Mutex<Vec<ExoConfig>>,
    }

    impl ExoConfigPersister for RecordingExoPersister {
        fn persist(&self, config: &ExoConfig) -> std::result::Result<(), String> {
            self.configs.lock().unwrap().push(config.clone());
            Ok(())
        }
    }

    #[derive(Default)]
    struct RecordingNotificationPersister {
        settings: std::sync::Mutex<Vec<notify::NotificationSettings>>,
    }

    impl NotificationConfigPersister for RecordingNotificationPersister {
        fn persist(
            &self,
            settings: &notify::NotificationSettings,
        ) -> std::result::Result<(), String> {
            self.settings.lock().unwrap().push(settings.clone());
            Ok(())
        }
    }

    #[tokio::test]
    async fn exo_admin_put_then_get_round_trips_and_hot_applies_catalog() {
        let state = test_state("exo-admin-round-trip");
        let persister = Arc::new(RecordingExoPersister::default());
        set_exo_config_persister(&state, persister.clone());
        let (status, saved) = response_json(
            admin_exo_update(
                State(state.clone()),
                axum::Json(ExoConfig {
                    url: "http://127.0.0.1:52415/".into(),
                    enabled_models: vec!["mlx-community/llama".into()],
                }),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(saved["url"], "http://127.0.0.1:52415");
        assert_eq!(
            exo_catalog_models(&state),
            vec!["exo/mlx-community/llama", "alex/mlx-community/llama"]
        );
        let (status, fetched) = response_json(admin_exo(State(state.clone())).await).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(fetched, saved);
        assert_eq!(persister.configs.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn exo_prefix_and_enabled_alex_alias_target_exo_url() {
        use axum::routing::post;
        let received = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let captured = received.clone();
        let upstream = Router::new().route("/v1/chat/completions", post(move |axum::Json(body): axum::Json<Value>| {
            let captured = captured.clone();
            async move {
                captured.lock().await.push(body["model"].as_str().unwrap_or("").to_string());
                axum::Json(json!({"id":"exo-test","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}]}))
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, upstream).await.unwrap();
        });
        let state = test_state("exo-routing");
        set_exo_config(
            &state,
            ExoConfig {
                url: format!("http://{address}"),
                enabled_models: vec!["mlx-community/llama".into()],
            },
        );
        for model in ["exo/mlx-community/llama", "alex/mlx-community/llama"] {
            let mut headers = HeaderMap::new();
            headers.insert("x-api-key", HeaderValue::from_static("alx-local"));
            let response = proxy(state.clone(), ClientFormat::OpenaiChat, "/v1/chat/completions", headers,
                Bytes::from(serde_json::to_vec(&json!({"model": model, "stream": false, "messages": [{"role":"user","content":"hi"}]})).unwrap()), None).await;
            assert_eq!(response.status(), StatusCode::OK);
        }
        assert_eq!(
            *received.lock().await,
            vec!["mlx-community/llama", "mlx-community/llama"]
        );
        server.abort();
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

    #[tokio::test]
    async fn admin_traces_filters_by_scoped_run_id() {
        let state = test_state("admin-traces-run-id");
        for (id, run_id) in [("trace-in-run", "hreg-1"), ("trace-other", "hreg-2")] {
            state
                .store
                .insert_trace(&TraceRecord {
                    id: id.into(),
                    ts_request_ms: now_ms(),
                    run_id: Some(run_id.into()),
                    ..Default::default()
                })
                .unwrap();
        }
        let mut query = HashMap::new();
        query.insert("run_id".into(), "hreg-1".into());
        let (status, body) = response_json(admin_traces(State(state), Query(query)).await).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["traces"].as_array().unwrap().len(), 1);
        assert_eq!(body["traces"][0]["id"], "trace-in-run");
        assert_eq!(body["traces"][0]["run_id"], "hreg-1");
    }

    #[test]
    fn dario_health_state_follows_credentials_generation_and_probe() {
        assert_eq!(
            dario_health_state(false, true, true),
            DarioHealthState::NotApplicable
        );
        assert_eq!(
            dario_health_state(true, false, true),
            DarioHealthState::Down
        );
        assert_eq!(
            dario_health_state(true, true, false),
            DarioHealthState::Down
        );
        assert_eq!(
            dario_health_state(true, true, true),
            DarioHealthState::Healthy
        );
    }

    #[test]
    fn dario_direct_fallback_reason_is_retained_in_trace_tags() {
        let tags = merge_trace_note(None, "dario_fallback", "warm timed out").unwrap();
        let value: Value = serde_json::from_str(&tags).unwrap();
        assert_eq!(value["dario_fallback"], "warm timed out");
    }

    #[test]
    fn tool_payload_redacts_free_text_argv_and_environment_secrets() {
        let command = "curl -H 'Authorization: Bearer sk-LEAK' https://user:p4ss@h";
        let bytes = tool_event_body(Some(json!({
            "command": command,
            "argv": ["curl", "--token", "super-secret"],
            "inline_args": ["--apikey=SECRET", "keepme"],
            "equals_args": ["--token=abc", "keepme"],
            "env": {"API_KEY": "also-secret"}
        })))
        .unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["argv"][2], "<redacted>");
        assert_eq!(
            value["inline_args"],
            json!(["--apikey=<redacted>", "keepme"])
        );
        assert_eq!(
            value["equals_args"],
            json!(["--token=<redacted>", "keepme"])
        );
        assert_eq!(value["env"], "<redacted>");
        assert_eq!(
            value["command"],
            "curl -H 'Authorization: Bearer <redacted>' https://user:<redacted>@h"
        );
        let redacted = String::from_utf8(bytes).unwrap();
        assert!(!redacted.contains("sk-LEAK"));
        assert!(!redacted.contains("p4ss"));
    }

    #[test]
    fn scrub_secret_string_masks_known_inline_credential_forms() {
        let input = "Authorization: Basic basic-secret x-api-key: api-secret PGPASSWORD=pg-secret AWS_SECRET_ACCESS_KEY=aws-secret export TOKEN=token-secret --password password-secret sk_live_secret ghp_secret xoxb-secret AKIA1234567890ABCDEF";
        let redacted = scrub_secret_string(input).unwrap();
        for secret in [
            "basic-secret",
            "api-secret",
            "pg-secret",
            "aws-secret",
            "token-secret",
            "password-secret",
            "sk_live_secret",
            "ghp_secret",
            "xoxb-secret",
            "AKIA1234567890ABCDEF",
        ] {
            assert!(!redacted.contains(secret), "leaked {secret}: {redacted}");
        }
        assert!(redacted.contains("Authorization: Basic <redacted>"));
        assert!(redacted.contains("x-api-key: <redacted>"));
        assert!(redacted.contains("PGPASSWORD=<redacted>"));
    }

    #[test]
    fn normalizes_native_tool_hook_payloads() {
        let mut event = json!({
            "hook_event_name": "PreToolUse",
            "session_id": "session",
            "toolUseId": "call",
            "tool_input": {"command": "echo hi"},
        });
        normalize_tool_event(&mut event);
        assert_eq!(event["phase"], "start");
        assert_eq!(event["tool_call_id"], "call");
        assert_eq!(event["args"]["command"], "echo hi");

        let mut failed = json!({"hook_event_name": "PostToolUseFailure", "tool_useID": "call"});
        normalize_tool_event(&mut failed);
        assert_eq!(failed["phase"], "end");
        assert_eq!(failed["is_error"], true);
        assert_eq!(failed["tool_call_id"], "call");
    }

    #[tokio::test]
    async fn tool_ingest_requires_harness_key_and_persists_a_session_join() {
        let state = test_state("tool-ingest");
        let key = "tool-harness-key";
        state
            .store
            .insert_run_key(
                "rk-tool",
                &key_hash_hex(key),
                "harness",
                None,
                None,
                Some("pi"),
                now_ms(),
                None,
            )
            .unwrap();
        let event = json!({"phase":"end", "session_id":"session", "turn_id":"1", "tool_call_id":"call", "tool_name":"bash", "args":{"command":"echo hi"}, "result":{"content":"hi"}, "is_error":false, "exit_status":0, "timestamp_ms":100});
        let mut headers = HeaderMap::new();
        headers.insert("x-api-key", HeaderValue::from_static(key));
        let response = tool_event(State(state.clone()), headers, axum::Json(event)).await;
        assert_eq!(response.status(), StatusCode::OK);
        let rows = state.store.session_tool_calls("session").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["turn_id"], "1");
        let response = tool_event(State(state), HeaderMap::new(), axum::Json(json!({}))).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn tool_ingest_rejects_dotted_ids_before_they_can_collide_with_body_paths() {
        let state = test_state("tool-ingest-dot-id");
        let key = "tool-dot-id-harness-key";
        state
            .store
            .insert_run_key(
                "rk-tool-dot-id",
                &key_hash_hex(key),
                "harness",
                None,
                None,
                Some("pi"),
                now_ms(),
                None,
            )
            .unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("x-api-key", HeaderValue::from_static(key));

        let dotted = json!({
            "phase": "end",
            "session_id": "s.1",
            "tool_call_id": "call",
            "tool_name": "bash",
            "args": {"command": "echo dotted"},
            "timestamp_ms": 100,
        });
        assert_eq!(
            tool_event(State(state.clone()), headers.clone(), axum::Json(dotted))
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );

        let underscore = json!({
            "phase": "end",
            "session_id": "s_1",
            "tool_call_id": "call",
            "tool_name": "bash",
            "args": {"command": "echo underscore"},
            "timestamp_ms": 100,
        });
        assert_eq!(
            tool_event(State(state.clone()), headers, axum::Json(underscore))
                .await
                .status(),
            StatusCode::OK
        );
        let rows = state.store.session_tool_calls("s_1").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            read_gz_json(rows[0]["args_body_path"].as_str()).unwrap()["command"],
            "echo underscore"
        );
    }

    #[tokio::test]
    async fn native_tool_hooks_persist_bodies_complete_calls_and_mark_failures() {
        let state = test_state("native-tool-hooks");
        let key = "native-tool-harness-key";
        state
            .store
            .insert_run_key(
                "rk-native-tool",
                &key_hash_hex(key),
                "harness",
                None,
                None,
                Some("claude"),
                now_ms(),
                None,
            )
            .unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            HeaderValue::from_static("Bearer native-tool-harness-key"),
        );

        let start = json!({"hook_event_name":"PreToolUse", "session_id":"session", "tool_use_id":"call", "tool_name":"Bash", "tool_input":{"command":"echo hi"}, "timestamp_ms":100});
        assert_eq!(
            tool_event(State(state.clone()), headers.clone(), axum::Json(start))
                .await
                .status(),
            StatusCode::OK
        );
        let end = json!({"hook_event_name":"PostToolUse", "session_id":"session", "tool_use_id":"call", "tool_name":"Bash", "tool_response":{"content":"hi"}, "timestamp_ms":200});
        assert_eq!(
            tool_event(State(state.clone()), headers.clone(), axum::Json(end))
                .await
                .status(),
            StatusCode::OK
        );
        let rows = state.store.session_tool_calls("session").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["ts_end_ms"], 200);
        let args = read_gz_json(rows[0]["args_body_path"].as_str()).unwrap();
        let result = read_gz_json(rows[0]["result_body_path"].as_str()).unwrap();
        assert_eq!(args["command"], "echo hi");
        assert_eq!(result["content"], "hi");

        let failed = json!({"hook_event_name":"PostToolUseFailure", "session_id":"session", "tool_use_id":"failed", "tool_name":"Bash", "tool_response":{"error":"nope"}, "timestamp_ms":300});
        assert_eq!(
            tool_event(State(state.clone()), headers, axum::Json(failed))
                .await
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            state.store.session_tool_calls("session").unwrap()[1]["is_error"],
            true
        );
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
                Some((
                    Ok::<_, std::io::Error>(Bytes::from_static(b"token")),
                    index + 1,
                ))
            }
        })
        .boxed();
        let task = tokio::spawn(async move {
            forward_upstream_stream(&mut stream, &tx, Duration::from_secs(5), |_| {}).await
        });

        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(4)).await;
        tokio::task::yield_now().await;
        assert_eq!(
            rx.recv().await.unwrap().unwrap(),
            Bytes::from_static(b"token")
        );
        tokio::time::advance(Duration::from_secs(4)).await;
        tokio::task::yield_now().await;
        assert_eq!(
            rx.recv().await.unwrap().unwrap(),
            Bytes::from_static(b"token")
        );
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
        routes_requests: bool,
        status: Option<Value>,
    }

    impl DarioRouter for FakeDario {
        fn routes_requests(&self) -> bool {
            self.routes_requests
        }

        fn active(&self) -> Option<DarioActive> {
            self.active.clone()
        }

        fn begin(&self, _generation_id: &str) -> Option<Box<dyn std::any::Any + Send>> {
            self.begin_succeeds
                .then(|| Box::new(()) as Box<dyn std::any::Any + Send>)
        }

        fn status(&self) -> Value {
            self.status
                .clone()
                .unwrap_or_else(|| json!({"active": self.active.is_some()}))
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
            routes_requests: true,
            status: None,
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
                !is_genuine_claude_code_request(ClientFormat::AnthropicMessages, &missing, &body),
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
        explicit_other_harness.insert("x-alexandria-harness", HeaderValue::from_static("pi"));
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
        conflicting_harness.append("x-alexandria-harness", HeaderValue::from_static("codex"));
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
        assert_eq!(dario.account.id, "anthropic:test");
        assert_eq!(dario.connection_account.as_ref().unwrap().kind, "dario");
        assert!(dario.via_dario);
        assert_eq!(dario.dario_generation.as_deref(), Some("test-generation"));
        assert!(dario
            .extra_headers
            .contains(&("x-dario-capture-id".into(), "trace-dario".into())));
    }

    #[tokio::test]
    async fn ready_dario_can_remain_passive_while_direct_routing_is_selected() {
        let state = test_state_with_dario(
            "dario-passive",
            Some(Arc::new(FakeDario {
                active: Some(DarioActive {
                    generation_id: "warm-generation".into(),
                    base_url: "http://127.0.0.1:9191".into(),
                    api_key: "dario-key".into(),
                }),
                begin_succeeds: true,
                routes_requests: false,
                status: None,
            })),
        );
        state.vault.upsert(anthropic_account()).await.unwrap();
        let (_, mut request) = claude_code_request();
        let original = serde_json::to_vec(&request).unwrap();
        let plan = plan_upstream(
            &state,
            ClientFormat::AnthropicMessages,
            Provider::Anthropic,
            "claude-sonnet-4-5",
            &mut request,
            &original,
            "trace-passive",
            &HashSet::new(),
            None,
            &HeaderMap::new(),
        )
        .await
        .unwrap();
        assert_eq!(plan.url, "https://api.anthropic.com/v1/messages");
        assert_eq!(plan.account.id, "anthropic:test");
        assert!(plan.dario_guard.is_none());
    }

    #[tokio::test]
    async fn configured_dario_falls_back_direct_when_repair_is_unavailable() {
        let state = test_state_with_dario(
            "dario-unhealthy",
            Some(Arc::new(FakeDario {
                active: None,
                begin_succeeds: false,
                routes_requests: true,
                status: None,
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
        let plan = result.unwrap();
        assert_eq!(plan.url, "https://api.anthropic.com/v1/messages");
        assert!(!plan.via_dario);
        assert!(plan
            .dario_fallback_reason
            .unwrap()
            .contains("repair failed"));
    }

    #[tokio::test]
    async fn dario_401_probe_is_an_actionable_reauth_issue() {
        let active = DarioActive {
            generation_id: "logged-out-generation".into(),
            base_url: "http://127.0.0.1:9191".into(),
            api_key: "dario-key".into(),
        };
        let state = test_state_with_dario(
            "dario-401-admin-status",
            Some(Arc::new(FakeDario {
                active: Some(active),
                begin_succeeds: true,
                routes_requests: true,
                status: Some(json!({
                    "active_generation_id": "logged-out-generation",
                    "route_enabled": true,
                    "health": "down",
                    "health_reason": null,
                    "generations": [{
                        "id": "logged-out-generation",
                        "last_probe": {"ok": false, "status": 401}
                    }]
                })),
            })),
        );
        state.vault.upsert(anthropic_account()).await.unwrap();

        let (_, body) = response_json(admin_dario(State(state)).await).await;
        assert_eq!(body["should_be_healthy"], true);
        assert_eq!(body["generation_health"], "down");
        assert_eq!(
            body["issue"],
            json!({
                "code": "reauth",
                "message": "Claude Code login needs re-auth",
                "fixable": true,
            })
        );
    }

    #[test]
    fn anthropic_ping_is_annotated_when_dario_falls_back_direct() {
        assert_eq!(
            annotate_anthropic_ping_message(Provider::Anthropic, true, "creds ok".into()),
            "degraded — serving via direct fallback, Dario down"
        );
        assert_eq!(
            annotate_anthropic_ping_message(Provider::Openai, true, "creds ok".into()),
            "creds ok"
        );
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
                routes_requests: true,
                status: None,
            })),
        );
        state.vault.upsert(anthropic_account()).await.unwrap();
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
        let plan = result.unwrap();
        assert_eq!(plan.url, "https://api.anthropic.com/v1/messages");
        assert!(!plan.via_dario);
        assert!(plan
            .dario_fallback_reason
            .unwrap()
            .contains("became unavailable"));
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
        client_headers.insert("x-alexandria-harness", HeaderValue::from_static("claude"));
        client_headers.insert("x-dario-capture-id", HeaderValue::from_static("spoofed"));

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
        client_headers.insert(
            "x-api-key",
            HeaderValue::from_static("another-provider-key"),
        );
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

    #[tokio::test]
    async fn vault_export_requires_passphrase_and_is_decryptable() {
        let state = test_state("vault-export");
        state.vault.upsert(anthropic_account()).await.unwrap();
        let (status, _) = response_json(admin_vault_export(State(state.clone()), None).await).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, _) = response_json(
            admin_vault_export(
                State(state.clone()),
                Some(axum::Json(VaultExportRequest {
                    passphrase: Some(String::new()),
                    selection: BundleSelection::default(),
                })),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, blob) = response_json(
            admin_vault_export(
                State(state),
                Some(axum::Json(VaultExportRequest {
                    passphrase: Some("test123".into()),
                    selection: BundleSelection::default(),
                })),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let blob: alex_auth::vault_bundle::EncryptedVaultBlob =
            serde_json::from_value(blob).unwrap();
        assert_eq!(
            alex_auth::decrypt_bundle(&blob, "test123")
                .unwrap()
                .accounts
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn vault_export_route_requires_admin_key() {
        use tower::ServiceExt;
        let state = test_state("vault-export-auth");
        let request = axum::http::Request::post("/admin/vault/export")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"passphrase":"test123"}"#))
            .unwrap();
        assert_eq!(
            router(state.clone())
                .oneshot(request)
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        let request = axum::http::Request::post("/admin/vault/export")
            .header("content-type", "application/json")
            .header("x-api-key", "alx-local")
            .body(Body::from(r#"{}"#))
            .unwrap();
        assert_eq!(
            router(state).oneshot(request).await.unwrap().status(),
            StatusCode::BAD_REQUEST
        );
    }

    fn anthropic_account_full(id: &str, name: &str, email: &str, expires_at_ms: i64) -> Account {
        Account {
            id: id.into(),
            provider: Provider::Anthropic,
            kind: "oauth".into(),
            name: name.into(),
            description: Some(email.into()),
            paused: false,
            label: None,
            access_token: Some(format!("token-{id}")),
            refresh_token: Some(format!("refresh-{id}")),
            id_token: None,
            api_key: None,
            expires_at_ms: Some(expires_at_ms),
            last_refresh_ms: Some(expires_at_ms),
            account_meta: json!({"email": email}),
            cooldown_until_ms: None,
            status: "active".into(),
            path: None,
        }
    }

    fn account_trace(id: &str, account_id: &str) -> TraceRecord {
        TraceRecord {
            id: id.into(),
            ts_request_ms: 1_000,
            ts_response_ms: Some(1_250),
            status: Some(200),
            upstream_provider: Some("anthropic".into()),
            routed_model: Some("claude-haiku-4-5".into()),
            account_id: Some(account_id.into()),
            subscription_identity: Some("anthropic:email:me@madhavajay.com".into()),
            usage: alex_core::Usage {
                input_tokens: Some(10),
                output_tokens: Some(5),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn account_merge_endpoint_unifies_history_and_keeps_survivor() {
        let now = now_ms();
        let state = test_state("account-merge-endpoint");
        // Survivor's own login is stale; the re-authed duplicate is fresh.
        state
            .vault
            .upsert(anthropic_account_full(
                "anthropic-oauth",
                "default",
                "me@madhavajay.com",
                now - 3_600_000,
            ))
            .await
            .unwrap();
        state
            .vault
            .upsert(anthropic_account_full(
                "anthropic-oauth-reauth",
                "reauth",
                "me@madhavajay.com",
                now + 3_600_000,
            ))
            .await
            .unwrap();
        // History is split: two traces on the survivor, three on the dup.
        for id in ["s1", "s2"] {
            state
                .store
                .insert_trace(&account_trace(id, "anthropic-oauth"))
                .unwrap();
        }
        for id in ["d1", "d2", "d3"] {
            state
                .store
                .insert_trace(&account_trace(id, "anthropic-oauth-reauth"))
                .unwrap();
        }

        let (status, body) = response_json(
            admin_account_merge(
                State(state.clone()),
                axum::Json(AccountMergeRequest {
                    from: "anthropic-oauth-reauth".into(),
                    into: "anthropic-oauth".into(),
                    allow_mismatch: false,
                }),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["merged_into"], "anthropic-oauth");
        assert_eq!(body["removed"], "anthropic-oauth-reauth");
        assert_eq!(body["adopted_credentials_from"], "anthropic-oauth-reauth");
        assert_eq!(body["rows"]["traces_account_id"], 3);

        // Only the survivor remains, and it adopted the fresh login.
        let accounts = state.vault.list().await;
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].id, "anthropic-oauth");
        assert_eq!(
            accounts[0].access_token.as_deref(),
            Some("token-anthropic-oauth-reauth")
        );
        // Every request is now attributed to the one survivor.
        let analytics = state.store.account_analytics(0, 60_000).unwrap();
        let by_account = analytics["by_account"].as_array().unwrap();
        assert_eq!(by_account.len(), 1);
        assert_eq!(by_account[0]["account_id"], "anthropic-oauth");
        assert_eq!(by_account[0]["requests"], 5);
        assert!(state.store.orphaned_trace_groups().unwrap().is_empty());
    }

    #[tokio::test]
    async fn account_merge_endpoint_refuses_email_mismatch() {
        let now = now_ms();
        let state = test_state("account-merge-mismatch");
        state
            .vault
            .upsert(anthropic_account_full(
                "anthropic-oauth",
                "default",
                "me@madhavajay.com",
                now + 3_600_000,
            ))
            .await
            .unwrap();
        state
            .vault
            .upsert(anthropic_account_full(
                "anthropic-oauth-other",
                "other",
                "someone@else.com",
                now + 3_600_000,
            ))
            .await
            .unwrap();
        state
            .store
            .insert_trace(&account_trace("x", "anthropic-oauth-other"))
            .unwrap();

        let (status, _) = response_json(
            admin_account_merge(
                State(state.clone()),
                axum::Json(AccountMergeRequest {
                    from: "anthropic-oauth-other".into(),
                    into: "anthropic-oauth".into(),
                    allow_mismatch: false,
                }),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        // Refusal must not have mutated anything.
        assert_eq!(state.vault.list().await.len(), 2);
        let trace = state.store.get_trace("x").unwrap().unwrap();
        assert_eq!(trace["account_id"], "anthropic-oauth-other");

        // The override merges it.
        let (status, body) = response_json(
            admin_account_merge(
                State(state.clone()),
                axum::Json(AccountMergeRequest {
                    from: "anthropic-oauth-other".into(),
                    into: "anthropic-oauth".into(),
                    allow_mismatch: true,
                }),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["rows"]["traces_account_id"], 1);
        assert_eq!(state.vault.list().await.len(), 1);
    }

    #[tokio::test]
    async fn credentials_view_redacts_secrets_and_includes_run_key_metadata() {
        let state = test_state("credentials-view");
        state.vault.upsert(anthropic_account()).await.unwrap();
        state
            .store
            .insert_run_key(
                "rk-1",
                "not-a-secret-value",
                "run",
                Some("run-1"),
                Some(r#"{"team":"test"}"#),
                Some("test run"),
                now_ms(),
                None,
            )
            .unwrap();
        let (status, body) = response_json(admin_credentials(State(state)).await).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["outbound"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["provider"] == "anthropic" && row["present"] == true));
        assert_eq!(body["inbound"]["run_keys"][0]["tags"]["team"], "test");
        assert!(!body.to_string().contains("direct-token"));
        assert!(!body.to_string().contains("not-a-secret-value"));
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
        let observed_error = observer.upstream_error();
        trace.error = observed_error.as_ref().map(UpstreamSseError::trace_message);
        trace.error_kind = observed_error.as_ref().map(|error| error.kind.clone());
        trace.error_class = trace.error_kind.as_deref().map(|kind| {
            classify_error("test", Some(StatusCode::OK.as_u16()), Some(kind))
                .as_str()
                .to_string()
        });
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
        assert_eq!(trace["error_kind"], "overloaded_error");
        assert_eq!(trace["error_class"], "capacity");
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

    fn test_api_account(id: &str, provider: Provider) -> Account {
        Account {
            id: id.into(),
            provider,
            kind: "api_key".into(),
            name: id.into(),
            description: None,
            paused: false,
            label: None,
            access_token: None,
            refresh_token: None,
            id_token: None,
            api_key: Some(format!("secret-{id}")),
            expires_at_ms: None,
            last_refresh_ms: None,
            account_meta: Value::Null,
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
        assert_eq!(
            cache.preferred("session-a", 20).as_deref(),
            Some("account-a")
        );

        // session-a's lookup extends its expiry, so session-b is oldest.
        cache.bind("session-c", "account-c", 30);
        assert!(cache.preferred("session-b", 30).is_none());
        assert_eq!(
            cache.preferred("session-a", 30).as_deref(),
            Some("account-a")
        );
        assert_eq!(
            cache.preferred("session-c", 30).as_deref(),
            Some("account-c")
        );
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
            state.vault.policy(Provider::Openai).account_reserve_pct["work"],
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
        assert_eq!(
            state.vault.policy(Provider::Anthropic).account_reserve_pct["work"],
            3
        );
    }

    #[tokio::test]
    async fn protection_endpoint_round_trips_persists_and_applies_live_policy() {
        let state = test_state("admin-protection");
        let persister = Arc::new(RecordingProtectionPolicyPersister::default());
        set_protection_policy_persister(&state, persister.clone());

        let mut anthropic = test_openai_account("anthropic-original");
        anthropic.id = "anthropic-oauth-original".into();
        anthropic.provider = Provider::Anthropic;
        state.vault.upsert(anthropic).await.unwrap();
        state
            .vault
            .upsert(test_api_account("openai-fallback", Provider::Openai))
            .await
            .unwrap();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server_state = state.clone();
        let server = tokio::spawn(async move {
            axum::serve(listener, router(server_state)).await.unwrap();
        });
        let client = reqwest::Client::new();
        let endpoint = format!("http://{address}/admin/protection");

        let defaults: Value = client
            .get(&endpoint)
            .header("x-api-key", "alx-local")
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(
            defaults,
            json!({
                "enabled": false,
                "reroute_on_auth": false,
                "retries": 1,
                "auto_return": true,
                "equivalencies": {},
            })
        );

        let policy = json!({
            "enabled": true,
            "reroute_on_auth": true,
            "retries": 2,
            "auto_return": true,
            "equivalencies": {
                "claude-fable-5": {"openai": "gpt-5.6-sol"}
            }
        });
        let saved: Value = client
            .put(&endpoint)
            .header("x-api-key", "alx-local")
            .json(&policy)
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(saved, policy);
        let fetched: Value = client
            .get(&endpoint)
            .header("x-api-key", "alx-local")
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(fetched, policy);
        let persisted = persister.policies.lock().unwrap();
        assert_eq!(persisted.len(), 1);
        assert!(persisted[0].enabled);
        assert_eq!(persisted[0].retries, 2);
        assert_eq!(
            persisted[0].equivalencies["claude-fable-5"]["openai"],
            "gpt-5.6-sol"
        );

        let invalid = client
            .put(&endpoint)
            .header("x-api-key", "alx-local")
            .json(&json!({"retries": 99}))
            .send()
            .await
            .unwrap();
        assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);

        let mut headers = HeaderMap::new();
        headers.insert("x-api-key", HeaderValue::from_static("alx-local"));
        headers.insert(
            "x-alexandria-simulate-error",
            HeaderValue::from_static("401:authentication_error"),
        );
        let response = proxy(
            state.clone(),
            ClientFormat::AnthropicMessages,
            "/v1/messages",
            headers,
            Bytes::from_static(br#"{"model":"claude-fable-5","messages":[]}"#),
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let trace = state
            .store
            .search_traces(&TraceFilter::default())
            .unwrap()
            .remove(0);
        assert_eq!(trace["substituted"], true);
        assert_eq!(trace["served_model"], "gpt-5.6-sol");
        server.abort();
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
        state
            .vault
            .upsert(test_openai_account("default"))
            .await
            .unwrap();
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
            assert!(
                account.contains_key(field),
                "missing legacy account field {field}"
            );
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

    /// Mirrors the binary's `ConfigUpdateChannelController`: rejects unknown
    /// channels, persists the normalized value, and recomputes a status against
    /// it. `beta` advertises an available update and `stable` does not, so a
    /// test can prove the status is recomputed against the newly-set channel.
    struct FakeChannelController {
        channel: std::sync::Mutex<String>,
    }

    impl UpdateChannelController for FakeChannelController {
        fn current(&self) -> String {
            self.channel.lock().unwrap().clone()
        }

        fn set(&self, channel: String) -> UpdateChannelSetFuture {
            let normalized = match channel.trim().to_ascii_lowercase().as_str() {
                "" | "stable" => "stable".to_string(),
                "beta" => "beta".to_string(),
                other => {
                    let message =
                        format!("unknown update channel '{other}' (expected stable or beta)");
                    return Box::pin(async move { Err(UpdateChannelError::Invalid(message)) });
                }
            };
            *self.channel.lock().unwrap() = normalized.clone();
            let update_available = normalized == "beta";
            let status = json!({
                "current": "0.1.0",
                "latest": if update_available { "0.2.0-beta.1" } else { "0.1.0" },
                "update_available": update_available,
                "update_channel": normalized,
                "checked_at_ms": 1,
            });
            Box::pin(async move {
                Ok(SetChannelOutcome {
                    channel: normalized,
                    status: Some(status),
                })
            })
        }
    }

    #[tokio::test]
    async fn admin_update_channel_gets_sets_and_recomputes() {
        let state = test_state("admin-update-channel");
        set_update_channel_controller(
            &state,
            Arc::new(FakeChannelController {
                channel: std::sync::Mutex::new("stable".into()),
            }),
        );

        // GET reflects the initially persisted channel.
        let (status, body) = response_json(admin_update_channel(State(state.clone())).await).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["channel"], "stable");

        // POST beta persists it and returns the recomputed availability.
        let (status, body) = response_json(
            admin_update_channel_set(
                State(state.clone()),
                axum::Json(UpdateChannelRequest {
                    channel: "beta".into(),
                }),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["channel"], "beta");
        assert_eq!(body["update_channel"], "beta");
        assert_eq!(body["update_available"], true);

        // GET now returns the persisted beta channel.
        let (_status, body) = response_json(admin_update_channel(State(state.clone())).await).await;
        assert_eq!(body["channel"], "beta");

        // The hot-apply means /admin/update recomputed against beta.
        let (status, body) = response_json(admin_update(State(state.clone())).await).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["update_channel"], "beta");
        assert_eq!(body["update_available"], true);

        // An invalid channel is rejected with 400 and does not change state.
        let (status, body) = response_json(
            admin_update_channel_set(
                State(state.clone()),
                axum::Json(UpdateChannelRequest {
                    channel: "nightly".into(),
                }),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("unknown update channel"));

        let (_status, body) = response_json(admin_update_channel(State(state)).await).await;
        assert_eq!(body["channel"], "beta");
    }

    #[tokio::test]
    async fn admin_update_channel_unconfigured_is_unavailable() {
        let state = test_state("admin-update-channel-unset");
        let (status, _) = response_json(admin_update_channel(State(state.clone())).await).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        let (status, _) = response_json(
            admin_update_channel_set(
                State(state),
                axum::Json(UpdateChannelRequest {
                    channel: "beta".into(),
                }),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
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
    async fn claude_gateway_model_catalog_uses_discoverable_alex_aliases() {
        let state = test_state("claude-gateway-models");
        let mut headers = HeaderMap::new();
        headers.insert("x-alexandria-harness", HeaderValue::from_static("claude"));
        let (status, body) =
            response_json(models(State(state), headers).await.into_response()).await;
        assert_eq!(status, StatusCode::OK);
        let rows = body["data"].as_array().unwrap();
        assert!(!rows.is_empty());
        assert!(rows.iter().all(|row| row["id"]
            .as_str()
            .is_some_and(|id| id.starts_with("claude-alex/"))));
        assert!(rows.iter().all(|row| row["display_name"]
            .as_str()
            .is_some_and(|name| name.starts_with("alex/"))));
        assert!(rows.iter().any(|row| {
            row["id"]
                .as_str()
                .is_some_and(|id| id.contains("gpt-") || id.contains("grok-"))
        }));
    }

    #[test]
    fn harness_event_normalization_accepts_grok_camel_and_snake_case() {
        let mut event = json!({
            "hookEventName": "subagent_start",
            "sessionId": "root-session",
            "subagentId": "child-agent",
            "agentType": "explore",
            "turnId": "turn-1",
        });
        normalize_harness_event(&mut event);
        assert_eq!(event["hook_event_name"], "SubagentStart");
        assert_eq!(event["session_id"], "root-session");
        assert_eq!(event["agent_id"], "child-agent");
        assert_eq!(event["agent_type"], "explore");
        assert_eq!(event["turn_id"], "turn-1");

        let mut stop = json!({"hook_event_name": "SubagentEnd"});
        normalize_harness_event(&mut stop);
        assert_eq!(stop["hook_event_name"], "SubagentStop");
    }

    fn lineage_test_headers(state: &Arc<AppState>, harness: &str, key: &str) -> HeaderMap {
        state
            .store
            .insert_run_key(
                "rk-lineage",
                &key_hash_hex(key),
                "harness",
                None,
                None,
                Some(harness),
                now_ms(),
                None,
            )
            .unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("x-api-key", HeaderValue::from_str(key).unwrap());
        headers
    }

    fn insert_lineage_test_traces(state: &Arc<AppState>, harness: &str, parent: &str, child: &str) {
        for (id, session_id) in [("lineage-parent", parent), ("lineage-child", child)] {
            state
                .store
                .insert_trace(&TraceRecord {
                    id: format!("{harness}-{id}"),
                    ts_request_ms: now_ms(),
                    session_id: Some(session_id.into()),
                    harness: Some(harness.into()),
                    ..Default::default()
                })
                .unwrap();
        }
    }

    fn lineage_session(state: &Arc<AppState>, child: &str) -> Value {
        state
            .store
            .sessions(None, 0)
            .unwrap()
            .into_iter()
            .find(|row| row["session_id"] == child)
            .unwrap()
    }

    async fn post_lineage_event(
        state: Arc<AppState>,
        headers: HeaderMap,
        event: Value,
    ) -> (StatusCode, Value) {
        response_json(harness_event(State(state), headers, axum::Json(event)).await).await
    }

    #[tokio::test]
    async fn claude_hook_subagent_start_and_stop_persist_lineage_timestamps() {
        let state = test_state("claude-hook-lineage");
        let parent = "claude-parent";
        let child = "claude-child";
        insert_lineage_test_traces(&state, "claude", parent, child);
        let headers = lineage_test_headers(&state, "claude", "claude-lineage-key");
        for (hook_event_name, timestamp_ms) in [("SubagentStart", 1_001), ("SubagentStop", 2_002)] {
            let (status, body) = post_lineage_event(
                state.clone(),
                headers.clone(),
                json!({
                    "hook_event_name": hook_event_name,
                    "session_id": parent,
                    "agent_id": child,
                    "agent_type": "general-purpose",
                    "timestamp_ms": timestamp_ms,
                }),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(body["lineage_updated"], true);
        }
        let row = lineage_session(&state, child);
        assert_eq!(row["parent_session_id"], parent);
        assert_eq!(row["agent_type"], "general-purpose");
        assert_eq!(row["subagent_started_ms"], 1_001);
        assert_eq!(row["subagent_stopped_ms"], 2_002);
    }

    #[tokio::test]
    async fn codex_harness_event_records_authenticated_lineage() {
        let state = test_state("codex-harness-event");
        let parent = "codex-parent";
        let child = "codex-child";
        insert_lineage_test_traces(&state, "codex", parent, child);
        let headers = lineage_test_headers(&state, "codex", "codex-lineage-key");
        for (hook_event_name, timestamp_ms) in [("SubagentStart", 3_003), ("SubagentStop", 4_004)] {
            let (status, body) = post_lineage_event(
                state.clone(),
                headers.clone(),
                json!({
                    "hook_event_name": hook_event_name,
                    "session_id": parent,
                    "agent_id": child,
                    "agent_type": "default",
                    "timestamp_ms": timestamp_ms,
                }),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(body["harness"], "codex");
        }
        assert_eq!(
            state.store.session_lineage_root("codex", child).unwrap(),
            parent
        );
        let row = lineage_session(&state, child);
        assert_eq!(row["agent_type"], "default");
        assert_eq!(row["subagent_started_ms"], 3_003);
        assert_eq!(row["subagent_stopped_ms"], 4_004);
    }

    #[tokio::test]
    async fn pi_extension_announcement_with_x_api_key_persists_lineage() {
        let state = test_state("pi-extension-lineage");
        let parent = "pi-parent";
        let child = "pi-child";
        insert_lineage_test_traces(&state, "pi", parent, child);
        let headers = lineage_test_headers(&state, "pi", "pi-lineage-key");
        let (status, body) = post_lineage_event(
            state.clone(),
            headers,
            json!({
                "hook_event_name": "SubagentStart",
                "session_id": parent,
                "agent_id": child,
                "agent_type": "pi",
                "timestamp_ms": 5_005,
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["harness"], "pi");
        assert_eq!(body["lineage_updated"], true);
        let row = lineage_session(&state, child);
        assert_eq!(row["parent_session_id"], parent);
        assert_eq!(row["agent_type"], "pi");
        assert_eq!(row["subagent_started_ms"], 5_005);
        assert_eq!(row["subagent_stopped_ms"], Value::Null);
    }

    #[tokio::test]
    async fn amp_subagent_start_and_stop_persist_tool_agent_type_and_timestamps() {
        let state = test_state("amp-hook-lineage");
        let parent = "T-parent";
        let child = "T-child";
        insert_lineage_test_traces(&state, "amp", parent, child);
        let headers = lineage_test_headers(&state, "amp", "amp-lineage-key");
        for (hook_event_name, timestamp_ms) in [("SubagentStart", 6_006), ("SubagentStop", 7_007)] {
            let (status, _) = post_lineage_event(
                state.clone(),
                headers.clone(),
                json!({
                    "hook_event_name": hook_event_name,
                    "session_id": parent,
                    "agent_id": child,
                    "agent_type": "Task",
                    "timestamp_ms": timestamp_ms,
                }),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
        }
        let row = lineage_session(&state, child);
        assert_eq!(row["agent_type"], "Task");
        assert_eq!(row["subagent_started_ms"], 6_006);
        assert_eq!(row["subagent_stopped_ms"], 7_007);
    }

    #[tokio::test]
    async fn subagent_event_missing_agent_id_is_rejected_without_a_lineage_row() {
        let state = test_state("lineage-missing-agent-id");
        let parent = "missing-parent";
        let child = "missing-child";
        insert_lineage_test_traces(&state, "pi", parent, child);
        let headers = lineage_test_headers(&state, "pi", "missing-agent-id-key");
        let (status, body) = post_lineage_event(
            state.clone(),
            headers,
            json!({
                "hook_event_name": "SubagentStart",
                "session_id": parent,
                "timestamp_ms": 8_008,
            }),
        )
        .await;
        // The current endpoint contract rejects incomplete subagent events.
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("agent_id"));
        assert_eq!(
            state.store.session_lineage_root("pi", child).unwrap(),
            child
        );
        assert_eq!(
            lineage_session(&state, child)["parent_session_id"],
            Value::Null
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

    #[tokio::test]
    async fn simulate_error_short_circuits_upstream_and_records_classified_trace() {
        let state = test_state("simulate-error");
        let mut headers = HeaderMap::new();
        headers.insert("x-api-key", HeaderValue::from_static("alx-local"));
        headers.insert(
            "x-alexandria-simulate-error",
            HeaderValue::from_static("429:rate_limit_error"),
        );
        let response = proxy(
            state.clone(),
            ClientFormat::OpenaiChat,
            "/v1/chat/completions",
            headers,
            Bytes::from_static(br#"{"model":"gpt-5.5","messages":[]}"#),
            None,
        )
        .await;
        let (status, body) = response_json(response).await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(body["error"]["type"], "rate_limit_error");
        let rows = state.store.search_traces(&TraceFilter::default()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["status"], 429);
        assert_eq!(rows[0]["error_kind"], "rate_limit_error");
        assert_eq!(rows[0]["error_class"], "capacity");
        assert_eq!(rows[0]["tags_json"], r#"{"simulated":"true"}"#);
        let mut query = HashMap::new();
        query.insert("error_class".into(), "capacity".into());
        let (admin_status, admin_body) =
            response_json(admin_traces(State(state.clone()), Query(query)).await).await;
        assert_eq!(admin_status, StatusCode::OK);
        assert_eq!(admin_body["traces"][0]["status"], 429);
        assert_eq!(admin_body["traces"][0]["error_kind"], "rate_limit_error");
        assert_eq!(admin_body["traces"][0]["error_class"], "capacity");
        // This test state has no usable upstream account. A response proves
        // the simulation returned before account planning / HTTP dispatch.
        assert_eq!(
            state.in_flight.load(std::sync::atomic::Ordering::Relaxed),
            0
        );
    }

    #[tokio::test]
    async fn provider_pause_admin_reports_state_and_resume_clears_it() {
        let state = test_state("provider-pause-admin");
        let paused = response_json(
            admin_provider_pause(
                State(state.clone()),
                Path("openai".into()),
                axum::Json(ProviderPauseRequest {
                    mode: PauseMode::Down,
                }),
            )
            .await,
        )
        .await
        .1;
        assert_eq!(
            paused,
            json!({"provider": "openai", "paused": true, "mode": "down"})
        );

        let (_, listed) = response_json(admin_providers(State(state.clone())).await).await;
        assert_eq!(
            listed["providers"]
                .as_array()
                .unwrap()
                .iter()
                .find(|provider| provider["provider"] == "openai")
                .unwrap(),
            &paused
        );

        let resumed =
            response_json(admin_provider_resume(State(state.clone()), Path("openai".into())).await)
                .await
                .1;
        assert_eq!(resumed, json!({"provider": "openai", "paused": false}));
        assert_eq!(paused_provider_mode(&state, Provider::Openai), None);
    }

    #[tokio::test]
    async fn paused_logged_out_provider_emits_reauth_and_records_pause_marker() {
        let (url, received, sink) = webhook_sink().await;
        let state = test_state("provider-pause-logged-out");
        state
            .vault
            .upsert(test_openai_account("paused-reauth"))
            .await
            .unwrap();
        set_notifications(
            &state,
            notify::NotificationSettings {
                channels: vec![notify::NotificationChannelConfig {
                    url,
                    ..Default::default()
                }],
                ..Default::default()
            },
        );
        admin_provider_pause(
            State(state.clone()),
            Path("openai".into()),
            axum::Json(ProviderPauseRequest {
                mode: PauseMode::LoggedOut,
            }),
        )
        .await;
        let mut headers = HeaderMap::new();
        headers.insert("x-api-key", HeaderValue::from_static("alx-local"));
        let response = proxy(
            state.clone(),
            ClientFormat::OpenaiChat,
            "/v1/chat/completions",
            headers,
            Bytes::from_static(br#"{"model":"gpt-5.5","messages":[]}"#),
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            response.headers()["x-alexandria-paused"],
            "openai:logged_out"
        );
        let (_, body) = response_json(response).await;
        assert_eq!(body["error"]["type"], "provider_paused");
        let event = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Some(event) = received.lock().unwrap().first().cloned() {
                    return event;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("local webhook sink did not receive paused-provider reauth alert");
        assert_eq!(event["category"], "reauth");
        assert_eq!(event["account"]["provider"], "openai");
        let trace = state
            .store
            .search_traces(&TraceFilter::default())
            .unwrap()
            .remove(0);
        assert_eq!(trace["error_kind"], "provider_paused");
        assert_eq!(trace["error_class"], "auth");
        sink.abort();
    }

    async fn paused_down_request(state: Arc<AppState>, no_substitute: bool) -> Response {
        admin_provider_pause(
            State(state.clone()),
            Path("openai".into()),
            axum::Json(ProviderPauseRequest {
                mode: PauseMode::Down,
            }),
        )
        .await;
        let mut headers = HeaderMap::new();
        headers.insert("x-api-key", HeaderValue::from_static("alx-local"));
        if no_substitute {
            headers.insert("x-alexandria-no-substitute", HeaderValue::from_static("1"));
        }
        proxy(
            state,
            ClientFormat::OpenaiChat,
            "/v1/chat/completions",
            headers,
            Bytes::from_static(br#"{"model":"gpt-5.5","messages":[]}"#),
            None,
        )
        .await
    }

    #[tokio::test]
    async fn paused_down_provider_reroutes_unless_no_substitute_is_set() {
        let policy = ProtectionPolicy {
            enabled: true,
            reroute_on_auth: false,
            retries: 0,
            auto_return: true,
            equivalencies: BTreeMap::from([(
                "gpt-5.5".into(),
                BTreeMap::from([("anthropic".into(), "claude-sonnet-5".into())]),
            )]),
        };
        let state = test_state("provider-pause-down-reroute");
        set_protection_policy(&state, policy.clone());
        state
            .vault
            .upsert(test_api_account("openai-only", Provider::Openai))
            .await
            .unwrap();
        state
            .vault
            .upsert(test_api_account("anthropic-fallback", Provider::Anthropic))
            .await
            .unwrap();
        let response = paused_down_request(state.clone(), false).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(response.headers()["x-alexandria-paused"], "openai:down");
        let trace = state
            .store
            .search_traces(&TraceFilter::default())
            .unwrap()
            .remove(0);
        assert_eq!(trace["error_kind"], "provider_paused");
        assert_eq!(trace["error_class"], "server");
        assert_eq!(trace["substituted"], true);
        assert_eq!(trace["served_model"], "claude-sonnet-5");

        let no_substitute = test_state("provider-pause-down-no-substitute");
        set_protection_policy(&no_substitute, policy);
        no_substitute
            .vault
            .upsert(test_api_account("openai-only", Provider::Openai))
            .await
            .unwrap();
        no_substitute
            .vault
            .upsert(test_api_account("anthropic-fallback", Provider::Anthropic))
            .await
            .unwrap();
        assert_eq!(
            paused_down_request(no_substitute.clone(), true)
                .await
                .status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        let trace = no_substitute
            .store
            .search_traces(&TraceFilter::default())
            .unwrap()
            .remove(0);
        assert_eq!(trace["substituted"], false);
        assert!(trace["attempts"].is_null());
    }

    #[tokio::test]
    async fn fixture_round_trip_injects_into_session_and_records_trace_fields() {
        let state = test_state("fixture-session-injection");
        let root = tmpdir("fixtures");
        set_fixture_dir(&state, root.join("fixtures"));
        let saved = admin_fixture_save(
            State(state.clone()),
            axum::Json(json!({
                "name": "captured-auth", "provider": "anthropic", "status": 401,
                "error_kind": "authentication_error",
                "body": r#"{"type":"error","error":{"type":"authentication_error","message":"captured"}}"#
            })),
        ).await;
        assert_eq!(saved.status(), StatusCode::CREATED);
        let (_, fixture) = response_json(
            admin_fixture_get(State(state.clone()), Path("captured-auth".into())).await,
        )
        .await;
        assert_eq!(fixture["name"], "captured-auth");
        let injected = admin_session_inject(
            State(state.clone()),
            Path("session-lab".into()),
            axum::Json(json!({"fixture": "captured-auth"})),
        )
        .await;
        assert_eq!(injected.status(), StatusCode::CREATED);
        let mut headers = HeaderMap::new();
        headers.insert("x-api-key", HeaderValue::from_static("alx-local"));
        headers.insert("x-session-id", HeaderValue::from_static("session-lab"));
        let response = proxy(
            state.clone(),
            ClientFormat::OpenaiChat,
            "/v1/chat/completions",
            headers,
            Bytes::from_static(br#"{"model":"gpt-5.5","messages":[]}"#),
            None,
        )
        .await;
        let (status, body) = response_json(response).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"]["message"], "captured");
        let row = state
            .store
            .search_traces(&TraceFilter::default())
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(row["injected"], true);
        assert_eq!(row["fixture_name"], "captured-auth");
    }

    #[tokio::test]
    async fn auth_protection_is_opt_in_and_uses_the_cross_provider_equivalency() {
        let state = test_state("auth-protection-equivalency");
        set_protection_policy(
            &state,
            ProtectionPolicy {
                enabled: true,
                reroute_on_auth: true,
                retries: 1,
                auto_return: true,
                equivalencies: BTreeMap::from([(
                    "claude-fable-5".into(),
                    BTreeMap::from([("openai".into(), "gpt-5.6-sol".into())]),
                )]),
            },
        );
        let mut anthropic = test_openai_account("anthropic-original");
        anthropic.id = "anthropic-oauth-original".into();
        anthropic.provider = Provider::Anthropic;
        state.vault.upsert(anthropic).await.unwrap();
        state
            .vault
            .upsert(test_api_account("openai-fallback", Provider::Openai))
            .await
            .unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("x-api-key", HeaderValue::from_static("alx-local"));
        headers.insert(
            "x-alexandria-simulate-error",
            HeaderValue::from_static("401:authentication_error"),
        );
        let response = proxy(
            state.clone(),
            ClientFormat::AnthropicMessages,
            "/v1/messages",
            headers,
            Bytes::from_static(br#"{"model":"claude-fable-5","messages":[]}"#),
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let trace = state
            .store
            .search_traces(&TraceFilter::default())
            .unwrap()
            .remove(0);
        assert_eq!(trace["substituted"], true);
        assert_eq!(trace["substitution_reason"], "auth");
        assert_eq!(trace["original_model"], "claude-fable-5");
        assert_eq!(trace["served_model"], "gpt-5.6-sol");
    }

    #[tokio::test]
    async fn protection_equivalency_accepts_short_model_aliases_for_capacity_failover() {
        let state = test_state("protection-short-model-alias");
        set_protection_policy(
            &state,
            ProtectionPolicy {
                enabled: true,
                reroute_on_auth: false,
                retries: 1,
                auto_return: true,
                equivalencies: BTreeMap::from([(
                    "fable-5".into(),
                    BTreeMap::from([("openai".into(), "gpt-5.6-sol".into())]),
                )]),
            },
        );
        let mut anthropic = test_openai_account("anthropic-original");
        anthropic.id = "anthropic-oauth-original".into();
        anthropic.provider = Provider::Anthropic;
        state.vault.upsert(anthropic).await.unwrap();
        state
            .vault
            .upsert(test_api_account("openai-fallback", Provider::Openai))
            .await
            .unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("x-api-key", HeaderValue::from_static("alx-local"));
        headers.insert(
            "x-alexandria-simulate-error",
            HeaderValue::from_static("429:rate_limit_error"),
        );
        let response = proxy(
            state.clone(),
            ClientFormat::AnthropicMessages,
            "/v1/messages",
            headers,
            Bytes::from_static(br#"{"model":"claude-fable-5","messages":[]}"#),
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        let trace = state
            .store
            .search_traces(&TraceFilter::default())
            .unwrap()
            .remove(0);
        assert_eq!(trace["substituted"], true);
        assert_eq!(trace["substitution_reason"], "capacity");
        assert_eq!(trace["original_model"], "claude-fable-5");
        assert_eq!(trace["served_model"], "gpt-5.6-sol");
    }

    async fn simulated_capacity_request(state: Arc<AppState>, no_substitute: bool) -> Response {
        let mut headers = HeaderMap::new();
        headers.insert("x-api-key", HeaderValue::from_static("alx-local"));
        headers.insert(
            "x-alexandria-simulate-error",
            HeaderValue::from_static("429:rate_limit_error"),
        );
        if no_substitute {
            headers.insert("x-alexandria-no-substitute", HeaderValue::from_static("1"));
        }
        proxy(
            state,
            ClientFormat::OpenaiChat,
            "/v1/chat/completions",
            headers,
            Bytes::from_static(br#"{"model":"gpt-5.5","messages":[]}"#),
            None,
        )
        .await
    }

    #[tokio::test]
    async fn simulated_capacity_rotates_to_a_second_account_and_records_the_attempts() {
        let state = test_state("simulated-account-failover");
        state
            .vault
            .upsert(test_api_account("openai-a", Provider::Openai))
            .await
            .unwrap();
        state
            .vault
            .upsert(test_api_account("openai-b", Provider::Openai))
            .await
            .unwrap();

        assert_eq!(
            simulated_capacity_request(state.clone(), false)
                .await
                .status(),
            StatusCode::TOO_MANY_REQUESTS
        );
        let trace = state
            .store
            .search_traces(&TraceFilter::default())
            .unwrap()
            .remove(0);
        assert_eq!(trace["substituted"], true);
        assert_eq!(trace["original_model"], "gpt-5.5");
        assert_eq!(trace["served_model"], "gpt-5.5");
        assert_eq!(trace["original_account_id"], "openai-a");
        assert_eq!(trace["served_account_id"], "openai-b");
        assert_eq!(trace["substitution_reason"], "capacity");
        assert_eq!(trace["attempts"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn simulated_capacity_with_only_one_account_passes_through_when_cross_model_is_disabled()
    {
        let state = test_state("simulated-single-account");
        state
            .vault
            .upsert(test_api_account("openai-only", Provider::Openai))
            .await
            .unwrap();

        assert_eq!(
            simulated_capacity_request(state.clone(), false)
                .await
                .status(),
            StatusCode::TOO_MANY_REQUESTS
        );
        let trace = state
            .store
            .search_traces(&TraceFilter::default())
            .unwrap()
            .remove(0);
        assert_eq!(trace["substituted"], false);
        assert_eq!(trace["error_class"], "capacity");
    }

    #[tokio::test]
    async fn simulated_cross_model_fallback_requires_explicit_configuration() {
        let mut substitution = SubstitutionConfig {
            enabled: true,
            fallbacks: BTreeMap::new(),
        };
        substitution
            .fallbacks
            .insert("gpt-5.5".into(), vec!["claude-sonnet-5".into()]);
        let state = test_state_with_substitution("simulated-cross-model", substitution);
        state
            .vault
            .upsert(test_api_account("openai-only", Provider::Openai))
            .await
            .unwrap();
        state
            .vault
            .upsert(test_api_account("anthropic-fallback", Provider::Anthropic))
            .await
            .unwrap();

        assert_eq!(
            simulated_capacity_request(state.clone(), false)
                .await
                .status(),
            StatusCode::TOO_MANY_REQUESTS
        );
        let trace = state
            .store
            .search_traces(&TraceFilter::default())
            .unwrap()
            .remove(0);
        assert_eq!(trace["substituted"], true);
        assert_eq!(trace["original_model"], "gpt-5.5");
        assert_eq!(trace["served_model"], "claude-sonnet-5");
        assert_eq!(trace["served_account_id"], "anthropic-fallback");
    }

    #[tokio::test]
    async fn no_substitute_header_forces_the_simulated_error_to_pass_through() {
        let state = test_state("simulated-no-substitute");
        state
            .vault
            .upsert(test_api_account("openai-a", Provider::Openai))
            .await
            .unwrap();
        state
            .vault
            .upsert(test_api_account("openai-b", Provider::Openai))
            .await
            .unwrap();

        assert_eq!(
            simulated_capacity_request(state.clone(), true)
                .await
                .status(),
            StatusCode::TOO_MANY_REQUESTS
        );
        let trace = state
            .store
            .search_traces(&TraceFilter::default())
            .unwrap()
            .remove(0);
        assert_eq!(trace["substituted"], false);
        assert!(trace["attempts"].is_null());
    }

    #[tokio::test]
    async fn simulation_rejects_non_harness_run_keys() {
        let state = test_state("simulate-error-run-key-gate");
        let (_, created) = response_json(
            admin_run_keys_create(
                State(state.clone()),
                Some(axum::Json(json!({"kind": "run", "label": "ordinary run"}))),
            )
            .await,
        )
        .await;
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-api-key",
            HeaderValue::from_str(created["key"].as_str().unwrap()).unwrap(),
        );
        headers.insert(
            "x-alexandria-simulate-error",
            HeaderValue::from_static("429:rate_limit_error"),
        );
        let response = proxy(
            state.clone(),
            ClientFormat::OpenaiChat,
            "/v1/chat/completions",
            headers,
            Bytes::from_static(br#"{"model":"gpt-5.5","messages":[]}"#),
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(state
            .store
            .search_traces(&TraceFilter::default())
            .unwrap()
            .is_empty());
    }

    #[test]
    fn parses_native_provider_error_envelopes() {
        let cases = [
            ("anthropic", 429, br#"{"type":"error","error":{"type":"rate_limit_error","message":"slow down"}}"#.as_slice(), "rate_limit_error", None, "slow down"),
            ("openai-chat", 401, br#"{"error":{"type":"authentication_error","code":"invalid_api_key","message":"bad key"}}"#.as_slice(), "authentication_error", Some("invalid_api_key"), "bad key"),
            ("openai-responses", 529, br#"{"error":{"type":"overloaded_error","message":"busy"}}"#.as_slice(), "overloaded_error", None, "busy"),
            ("gemini", 429, br#"{"error":{"code":429,"status":"RESOURCE_EXHAUSTED","message":"quota"}}"#.as_slice(), "RESOURCE_EXHAUSTED", Some("429"), "quota"),
        ];
        for (format, status, body, kind, code, message) in cases {
            let parsed = parse_upstream_error(format, status, body).unwrap();
            assert_eq!(parsed.kind.as_deref(), Some(kind));
            assert_eq!(parsed.code.as_deref(), code);
            assert_eq!(parsed.message.as_deref(), Some(message));
        }
    }

    #[test]
    fn dario_brokered_http_error_is_classified_at_finalization() {
        let state = test_state("dario-classified-error");
        let trace = TraceRecord {
            id: "dario-429".into(),
            ts_request_ms: now_ms(),
            status: Some(429),
            upstream_provider: Some("anthropic".into()),
            upstream_format: Some("anthropic".into()),
            via_dario: true,
            dario_generation: Some("generation-test".into()),
            ..Default::default()
        };
        finalize_trace(
            &state,
            trace,
            b"{}",
            None,
            Some(br#"{"type":"error","error":{"type":"rate_limit_error","message":"too many requests"}}"#),
        );
        let trace = state.store.get_trace("dario-429").unwrap().unwrap();
        assert_eq!(trace["via_dario"], true);
        assert_eq!(trace["status"], 429);
        assert_eq!(trace["error_kind"], "rate_limit_error");
        assert_eq!(trace["error_class"], "capacity");
    }

    #[test]
    fn classifies_provider_and_transport_errors() {
        let cases = [
            (
                "anthropic",
                Some(401),
                Some("authentication_error"),
                ErrorClass::Auth,
            ),
            (
                "openai",
                Some(429),
                Some("rate_limit_error"),
                ErrorClass::Capacity,
            ),
            (
                "gemini",
                Some(400),
                Some("INVALID_ARGUMENT"),
                ErrorClass::BadRequest,
            ),
            ("openai", Some(503), Some("api_error"), ErrorClass::Server),
            (
                "anthropic",
                Some(200),
                Some("client_disconnect"),
                ErrorClass::ClientDisconnect,
            ),
            (
                "openai",
                Some(502),
                Some("upstream_unreachable"),
                ErrorClass::Network,
            ),
            ("gemini", Some(418), Some("teapot"), ErrorClass::Other),
        ];
        for (provider, status, kind, expected) in cases {
            assert_eq!(classify_error(provider, status, kind), expected);
        }
    }

    #[test]
    fn failover_statuses_are_limited_to_capacity_and_server_errors() {
        for status in [
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
            reqwest::StatusCode::UNAUTHORIZED,
            reqwest::StatusCode::FORBIDDEN,
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
        state.vault.upsert(test_openai_account("a")).await.unwrap();
        state.vault.upsert(test_openai_account("b")).await.unwrap();
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

    #[tokio::test]
    async fn admin_notification_test_posts_to_local_webhook_sink_and_redacts_url() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let sink = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let sink_address = sink.local_addr().unwrap();
        let received = tokio::spawn(async move {
            let (mut socket, _) = sink.accept().await.unwrap();
            let mut bytes = Vec::new();
            let mut buffer = [0u8; 1024];
            let (header_end, content_length) = loop {
                let count = socket.read(&mut buffer).await.unwrap();
                assert!(count > 0, "webhook closed before request completed");
                bytes.extend_from_slice(&buffer[..count]);
                let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") else {
                    continue;
                };
                let headers = String::from_utf8_lossy(&bytes[..end]);
                let length = headers
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length: "))
                    .and_then(|value| value.parse::<usize>().ok())
                    .unwrap_or(0);
                break (end + 4, length);
            };
            while bytes.len() < header_end + content_length {
                let count = socket.read(&mut buffer).await.unwrap();
                assert!(count > 0, "webhook body ended early");
                bytes.extend_from_slice(&buffer[..count]);
            }
            socket
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\n{}")
                .await
                .unwrap();
            serde_json::from_slice::<Value>(&bytes[header_end..header_end + content_length])
                .unwrap()
        });

        let state = test_state("notification-local-sink");
        let secret = "very-secret-webhook-token";
        set_notifications(
            &state,
            notify::NotificationSettings {
                channels: vec![notify::NotificationChannelConfig {
                    url: format!("http://{sink_address}/{secret}"),
                    ..Default::default()
                }],
                ..Default::default()
            },
        );
        let admin_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let admin_address = admin_listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let _ = axum::serve(admin_listener, router(state)).await;
        });
        let client = reqwest::Client::new();
        let response = client
            .post(format!("http://{admin_address}/admin/notifications/test"))
            .header("x-api-key", "alx-local")
            .json(&json!({}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let event = tokio::time::timeout(Duration::from_secs(2), received)
            .await
            .expect("local webhook sink did not receive the test event")
            .unwrap();
        assert_eq!(event["category"], "test");
        assert_eq!(event["account"]["provider"], "alexandria");

        let listing = client
            .get(format!("http://{admin_address}/admin/notifications"))
            .header("x-api-key", "alx-local")
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        server.abort();
        assert!(!listing.contains(secret));
        assert!(!listing.contains(&format!("http://{sink_address}")));
    }

    #[tokio::test]
    async fn notification_save_get_test_and_delete_hot_apply_without_secrets() {
        let (url, received, sink) = webhook_sink().await;
        let state = test_state("notification-runtime-save");
        let persister = Arc::new(RecordingNotificationPersister::default());
        set_notification_config_persister(&state, persister.clone());

        let saved = response_json(
            admin_notifications_save(
                State(state.clone()),
                axum::Json(NotificationChannelRequest {
                    id: None,
                    kind: None,
                    format: notify::WebhookFormat::Generic,
                    url: Some(format!("{url}?token=very-secret-webhook-token")),
                    token: None,
                    chat_id: None,
                    min_level: notify::NotificationLevel::Warn,
                    categories: vec!["reauth".into()],
                }),
            )
            .await,
        )
        .await
        .1;
        let id = saved["channel"]["id"].as_str().unwrap().to_owned();
        assert_eq!(saved["channel"]["format"], "generic");
        assert!(saved.to_string().contains(&id));
        assert!(!saved.to_string().contains("very-secret-webhook-token"));
        assert!(saved["channel"].get("url").is_none());

        let tested = response_json(
            admin_notifications_test(State(state.clone()), axum::Json(json!({}))).await,
        )
        .await
        .1;
        assert_eq!(tested["channels"][0]["id"], id);
        assert_eq!(tested["channels"][0]["ok"], true);
        assert_eq!(received.lock().unwrap()[0]["category"], "test");

        let listing = response_json(admin_notifications(State(state.clone())).await)
            .await
            .1;
        let listing_text = listing.to_string();
        assert_eq!(listing["channels"][0]["id"], id);
        assert!(!listing_text.contains("very-secret-webhook-token"));
        assert!(!listing_text.contains(&url));

        let deleted =
            response_json(admin_notifications_delete(State(state.clone()), Path(id.clone())).await)
                .await
                .1;
        assert_eq!(deleted["ok"], true);
        assert_eq!(
            response_json(admin_notifications_test(State(state), axum::Json(json!({}))).await)
                .await
                .1["channels"]
                .as_array()
                .unwrap()
                .len(),
            0
        );
        assert_eq!(persister.settings.lock().unwrap().len(), 2);
        sink.abort();
    }

    #[tokio::test]
    async fn inline_notification_test_delivers_without_persisting() {
        let (url, received, sink) = webhook_sink().await;
        let state = test_state("notification-inline-test");
        let response = response_json(
            admin_notifications_test(
                State(state.clone()),
                axum::Json(json!({"format": "generic", "url": url})),
            )
            .await,
        )
        .await
        .1;
        assert_eq!(response["channels"][0]["ok"], true);
        assert_eq!(received.lock().unwrap()[0]["category"], "test");
        assert!(state
            .notification_settings
            .read()
            .unwrap()
            .channels
            .is_empty());
        sink.abort();
    }

    #[tokio::test]
    async fn telegram_validate_and_discover_use_local_stub_and_hide_token_on_failure() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let stub = tokio::spawn(async move {
            for (expected_path, response) in [
                (
                    "/botgood-token/getMe",
                    b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 67\r\n\r\n{\"ok\":true,\"result\":{\"username\":\"YourBot\",\"first_name\":\"Your Bot\"}}".as_slice(),
                ),
                (
                    "/botbad-token/getMe",
                    b"HTTP/1.1 401 Unauthorized\r\ncontent-length: 0\r\n\r\n".as_slice(),
                ),
                (
                    "/botgood-token/getUpdates",
                    b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 67\r\n\r\n{\"ok\":true,\"result\":[{\"message\":{\"chat\":{\"id\":42,\"title\":\"Ops\"}}}]}".as_slice(),
                ),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = [0u8; 1024];
                let count = socket.read(&mut bytes).await.unwrap();
                let request = String::from_utf8_lossy(&bytes[..count]);
                assert!(request.starts_with(&format!("GET {expected_path} ")));
                socket.write_all(response).await.unwrap();
            }
        });
        let state = test_state("telegram-validation-stub");
        set_telegram_base(&state, format!("http://{address}"));

        let valid = response_json(
            admin_notifications_validate(
                State(state.clone()),
                axum::Json(TelegramTokenRequest {
                    format: notify::WebhookFormat::Telegram,
                    token: "good-token".into(),
                }),
            )
            .await,
        )
        .await
        .1;
        assert_eq!(valid["bot_username"], "YourBot", "{valid}");
        assert_eq!(valid["bot_name"], "Your Bot");
        let invalid = response_json(
            admin_notifications_validate(
                State(state.clone()),
                axum::Json(TelegramTokenRequest {
                    format: notify::WebhookFormat::Telegram,
                    token: "bad-token".into(),
                }),
            )
            .await,
        )
        .await
        .1;
        assert_eq!(invalid["ok"], false);
        assert!(!invalid.to_string().contains("bad-token"));
        let chats = response_json(
            admin_notifications_discover_chat(
                State(state),
                axum::Json(TelegramTokenRequest {
                    format: notify::WebhookFormat::Telegram,
                    token: "good-token".into(),
                }),
            )
            .await,
        )
        .await
        .1;
        assert_eq!(
            chats["chats"][0],
            json!({"chat_id": "42", "chat_name": "Ops"})
        );
        stub.await.unwrap();
    }

    #[tokio::test]
    async fn auth_class_for_managed_oauth_account_builds_reauth_event() {
        let state = test_state("auth-reauth-notification");
        state
            .vault
            .upsert(test_openai_account("reauth-account"))
            .await
            .unwrap();
        let trace = TraceRecord {
            account_id: Some("openai-oauth-reauth-account".into()),
            error_class: Some("auth".into()),
            ..Default::default()
        };
        let event = reauth_notification_event(&state, &trace).expect("managed auth error event");
        assert_eq!(event.category, "reauth");
        assert_eq!(event.account.provider, "openai");
        assert_eq!(event.action_url.as_deref(), Some("alex auth login openai"));

        let non_auth = TraceRecord {
            error_class: Some("network".into()),
            ..trace
        };
        assert!(reauth_notification_event(&state, &non_auth).is_none());
    }

    #[tokio::test]
    async fn dario_401_notification_uses_reauth_guidance_and_is_debounced() {
        let (url, received, server) = webhook_sink().await;
        let state = test_state("dario-reauth-notification");
        set_notifications(
            &state,
            notify::NotificationSettings {
                channels: vec![notify::NotificationChannelConfig {
                    url,
                    min_level: notify::NotificationLevel::Warn,
                    categories: vec!["reauth".into()],
                    ..Default::default()
                }],
                ..Default::default()
            },
        );

        emit_dario_reauth_notification(&state);
        emit_dario_reauth_notification(&state);
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if received.lock().unwrap().len() == 1 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("local webhook sink did not receive Dario reauth event");

        let event = received.lock().unwrap()[0].clone();
        server.abort();
        assert_eq!(event["category"], "reauth");
        assert_eq!(event["account"]["label"], "Dario");
        assert_eq!(
            event["body"],
            "Dario is down — your Claude Code login needs re-auth. Run `claude` login (or tap Reauth Dario)."
        );
        assert_ne!(event["action_url"], "alex auth login anthropic");
    }

    fn expired_oauth_account(
        provider: Provider,
        name: &str,
        refresh_token: Option<&str>,
        expires_at_ms: i64,
    ) -> Account {
        Account {
            id: format!("{}-oauth-{name}", provider.as_str()),
            provider,
            kind: "oauth".into(),
            name: name.into(),
            description: None,
            paused: false,
            label: Some(format!("{} (test)", provider.as_str())),
            access_token: Some(format!("token-{name}")),
            refresh_token: refresh_token.map(str::to_string),
            id_token: None,
            api_key: None,
            expires_at_ms: Some(expires_at_ms),
            last_refresh_ms: Some(expires_at_ms),
            account_meta: json!({}),
            cooldown_until_ms: None,
            status: "active".into(),
            path: None,
        }
    }

    fn reauth_channel(url: String) -> notify::NotificationSettings {
        notify::NotificationSettings {
            channels: vec![notify::NotificationChannelConfig {
                url,
                min_level: notify::NotificationLevel::Warn,
                categories: vec!["reauth".into()],
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    async fn first_event(received: &Arc<std::sync::Mutex<Vec<Value>>>, context: &str) -> Value {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Some(event) = received.lock().unwrap().first().cloned() {
                    return event;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("{context}"))
    }

    async fn needs_reauth_flag(state: &Arc<AppState>, id: &str) -> bool {
        state
            .vault
            .list()
            .await
            .into_iter()
            .find(|a| a.id == id)
            .expect("account present")
            .needs_reauth()
    }

    /// Local OAuth token endpoint that always rejects a refresh with a 400
    /// invalid_grant, so the proactive path sees a confirmed logout.
    async fn invalid_grant_token_server() -> (String, tokio::task::JoinHandle<()>) {
        async fn reject() -> Response {
            (
                StatusCode::BAD_REQUEST,
                axum::Json(json!({"error": "invalid_grant"})),
            )
                .into_response()
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new().route("/", post(reject));
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{address}/"), server)
    }

    #[tokio::test]
    async fn proactive_idle_expiry_without_refresh_token_emits_one_reauth_notification() {
        let (url, received, sink) = webhook_sink().await;
        let state = test_state("proactive-expiry-dead");
        state
            .vault
            .upsert(expired_oauth_account(
                Provider::Anthropic,
                "idle",
                None,
                now_ms() - 5 * 60_000,
            ))
            .await
            .unwrap();
        set_notifications(&state, reauth_channel(url));

        reauth_watch_once(&state).await;

        let event = first_event(
            &received,
            "idle-expired account with no refresh token did not alert",
        )
        .await;
        sink.abort();
        assert_eq!(event["category"], "reauth");
        assert_eq!(event["account"]["provider"], "anthropic");
        assert_eq!(event["action_url"], "alex auth login anthropic");
        // The delivered payload must never carry credential material (the
        // access token value for this account is "token-idle").
        let serialized = event.to_string();
        assert!(!serialized.contains("token-idle"));
        assert!(needs_reauth_flag(&state, "anthropic-oauth-idle").await);
    }

    #[tokio::test]
    async fn proactive_refresh_invalid_grant_emits_reauth_notification() {
        let (url, received, sink) = webhook_sink().await;
        let (token_url, token_server) = invalid_grant_token_server().await;
        let state = test_state("proactive-invalid-grant");
        state.vault.set_refresh_endpoint_override(Some(token_url));
        // Non-default name so a native re-import can never collide and mask the
        // rejected refresh (import writes the `<provider>-oauth` default id).
        state
            .vault
            .upsert(expired_oauth_account(
                Provider::Openai,
                "expired-invalid",
                Some("dead-refresh-token"),
                now_ms() - 5 * 60_000,
            ))
            .await
            .unwrap();
        set_notifications(&state, reauth_channel(url));

        reauth_watch_once(&state).await;

        let event = first_event(
            &received,
            "refresh rejected with invalid_grant did not alert",
        )
        .await;
        sink.abort();
        token_server.abort();
        assert_eq!(event["category"], "reauth");
        assert_eq!(event["account"]["provider"], "openai");
        assert!(!event.to_string().contains("dead-refresh-token"));
        assert!(needs_reauth_flag(&state, "openai-oauth-expired-invalid").await);
    }

    #[tokio::test]
    async fn proactive_expiry_debounces_within_cooldown_window() {
        let (url, received, sink) = webhook_sink().await;
        let state = test_state("proactive-debounce");
        state
            .vault
            .upsert(expired_oauth_account(
                Provider::Openai,
                "idle-dead",
                None,
                now_ms() - 5 * 60_000,
            ))
            .await
            .unwrap();
        // Default 30-minute cooldown: two back-to-back ticks must coalesce.
        set_notifications(&state, reauth_channel(url));

        reauth_watch_once(&state).await;
        reauth_watch_once(&state).await;

        // Let any deliveries land before counting.
        tokio::time::sleep(Duration::from_millis(300)).await;
        let count = received.lock().unwrap().len();
        sink.abort();
        assert_eq!(
            count, 1,
            "a persistently-expired account must alert once per cooldown window, not per tick"
        );
    }

    #[tokio::test]
    async fn valid_not_yet_expired_account_never_alerts() {
        let (url, received, sink) = webhook_sink().await;
        let state = test_state("proactive-still-valid");
        // Not yet expired and holding a refresh token: silently refreshable, so
        // it is not a logout and must never alert.
        state
            .vault
            .upsert(expired_oauth_account(
                Provider::Openai,
                "fresh",
                Some("good-refresh-token"),
                now_ms() + 60 * 60_000,
            ))
            .await
            .unwrap();
        set_notifications(&state, reauth_channel(url));

        reauth_watch_once(&state).await;

        tokio::time::sleep(Duration::from_millis(300)).await;
        let count = received.lock().unwrap().len();
        sink.abort();
        assert_eq!(count, 0, "a still-valid refreshable account must not alert");
        assert!(!needs_reauth_flag(&state, "openai-oauth-fresh").await);
    }

    #[tokio::test]
    async fn recovered_account_clears_needs_reauth_so_next_logout_alerts() {
        let (url, _received, sink) = webhook_sink().await;
        let state = test_state("proactive-recovery-clear");
        state
            .vault
            .upsert(expired_oauth_account(
                Provider::Openai,
                "recovered",
                Some("good-refresh-token"),
                now_ms() + 60 * 60_000,
            ))
            .await
            .unwrap();
        // Simulate a prior logout that flagged the account; the user has since
        // re-authenticated (token is now valid again).
        mark_account_needs_reauth(&state, "openai-oauth-recovered", true).await;
        assert!(needs_reauth_flag(&state, "openai-oauth-recovered").await);
        set_notifications(&state, reauth_channel(url));

        reauth_watch_once(&state).await;

        sink.abort();
        assert!(
            !needs_reauth_flag(&state, "openai-oauth-recovered").await,
            "a recovered account must have its needs-reauth flag cleared"
        );
    }

    // ---- probe-derived health + reauth-on-auth-failure --------------------

    /// A valid, active managed OAuth account for a provider. Probe health is
    /// independent of token expiry, so this uses a far-future expiry to make the
    /// intent ("live credential, but the ping decides health") explicit.
    fn active_oauth_account(provider: Provider, name: &str) -> Account {
        expired_oauth_account(
            provider,
            name,
            Some("refresh-token"),
            now_ms() + 60 * 60_000,
        )
    }

    fn probe_result(account_id: &str, ok: bool, status: Option<u16>) -> PingResult {
        PingResult {
            provider: "xai",
            account_id: Some(account_id.to_string()),
            ok,
            status,
            latency_ms: 12,
            message: "probe".into(),
        }
    }

    async fn account_health_of(state: &Arc<AppState>, id: &str) -> String {
        let account = state
            .vault
            .list()
            .await
            .into_iter()
            .find(|a| a.id == id)
            .expect("account present");
        account_health(&account)
    }

    async fn admin_accounts_health(state: &Arc<AppState>, id: &str) -> String {
        let (_, body) =
            response_json(admin_accounts(State(state.clone())).await.into_response()).await;
        body["accounts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["id"] == id)
            .unwrap_or_else(|| panic!("account {id} not in /admin/accounts"))["health"]
            .as_str()
            .unwrap()
            .to_string()
    }

    #[tokio::test]
    async fn probe_401_marks_auth_failed_and_fires_one_reauth() {
        let (url, received, sink) = webhook_sink().await;
        let state = test_state("probe-401-auth");
        state
            .vault
            .upsert(active_oauth_account(Provider::Xai, "grok"))
            .await
            .unwrap();
        set_notifications(&state, reauth_channel(url));

        // Two consecutive failing probes: the alert must debounce to one per
        // cooldown window, not fire per probe.
        record_probe_outcome(&state, &probe_result("xai-oauth-grok", false, Some(401))).await;
        record_probe_outcome(&state, &probe_result("xai-oauth-grok", false, Some(401))).await;

        let event = first_event(&received, "401 probe did not fire a reauth alert").await;
        tokio::time::sleep(Duration::from_millis(150)).await;
        let count = received.lock().unwrap().len();
        sink.abort();

        assert_eq!(event["category"], "reauth");
        assert_eq!(event["account"]["provider"], "xai");
        // The alert never carries credential material.
        assert!(!event.to_string().contains("token-grok"));
        assert_eq!(
            count, 1,
            "an auth-failed provider must alert once per window, not once per probe"
        );
        assert!(needs_reauth_flag(&state, "xai-oauth-grok").await);
        assert_eq!(
            account_health_of(&state, "xai-oauth-grok").await,
            "auth_failed"
        );
        // The probe-derived health, not the still-"active" credential status,
        // is what /admin/accounts reports.
        assert_eq!(
            admin_accounts_health(&state, "xai-oauth-grok").await,
            "auth_failed"
        );
    }

    #[tokio::test]
    async fn probe_503_is_unreachable_and_never_fires_reauth() {
        let (url, received, sink) = webhook_sink().await;
        let state = test_state("probe-503-down");
        state
            .vault
            .upsert(active_oauth_account(Provider::Xai, "grok"))
            .await
            .unwrap();
        set_notifications(&state, reauth_channel(url));

        record_probe_outcome(&state, &probe_result("xai-oauth-grok", false, Some(503))).await;

        tokio::time::sleep(Duration::from_millis(200)).await;
        let count = received.lock().unwrap().len();
        sink.abort();

        assert_eq!(
            count, 0,
            "a transient 5xx is a failover/down condition and must not fire a reauth alert"
        );
        assert!(!needs_reauth_flag(&state, "xai-oauth-grok").await);
        assert_eq!(
            account_health_of(&state, "xai-oauth-grok").await,
            "unreachable"
        );
        assert_eq!(
            admin_accounts_health(&state, "xai-oauth-grok").await,
            "unreachable"
        );
    }

    #[tokio::test]
    async fn probe_timeout_is_unreachable_and_never_fires_reauth() {
        let (url, received, sink) = webhook_sink().await;
        let state = test_state("probe-timeout-down");
        state
            .vault
            .upsert(active_oauth_account(Provider::Xai, "grok"))
            .await
            .unwrap();
        set_notifications(&state, reauth_channel(url));

        // A timed-out probe yields no HTTP status at all.
        record_probe_outcome(&state, &probe_result("xai-oauth-grok", false, None)).await;

        tokio::time::sleep(Duration::from_millis(200)).await;
        let count = received.lock().unwrap().len();
        sink.abort();

        assert_eq!(count, 0, "a timeout must not fire a reauth alert");
        assert!(!needs_reauth_flag(&state, "xai-oauth-grok").await);
        assert_eq!(
            account_health_of(&state, "xai-oauth-grok").await,
            "unreachable"
        );
    }

    #[tokio::test]
    async fn probe_recovery_clears_needs_reauth_and_reports_healthy() {
        let (url, _received, sink) = webhook_sink().await;
        let state = test_state("probe-recovery");
        state
            .vault
            .upsert(active_oauth_account(Provider::Xai, "grok"))
            .await
            .unwrap();
        set_notifications(&state, reauth_channel(url));

        // First a confirmed auth failure flags the account...
        record_probe_outcome(&state, &probe_result("xai-oauth-grok", false, Some(401))).await;
        assert!(needs_reauth_flag(&state, "xai-oauth-grok").await);
        assert_eq!(
            account_health_of(&state, "xai-oauth-grok").await,
            "auth_failed"
        );

        // ...then a healthy probe recovers it.
        record_probe_outcome(&state, &probe_result("xai-oauth-grok", true, Some(200))).await;
        sink.abort();

        assert!(
            !needs_reauth_flag(&state, "xai-oauth-grok").await,
            "a recovered probe must clear the needs-reauth flag"
        );
        assert_eq!(account_health_of(&state, "xai-oauth-grok").await, "healthy");
        assert_eq!(
            admin_accounts_health(&state, "xai-oauth-grok").await,
            "healthy"
        );
    }

    #[tokio::test]
    async fn never_probed_account_reports_unknown_not_healthy() {
        let state = test_state("probe-unknown");
        state
            .vault
            .upsert(active_oauth_account(Provider::Xai, "grok"))
            .await
            .unwrap();
        // No probe has run: the credential is present ("active") but we must not
        // claim green without evidence of reachability.
        assert_eq!(account_health_of(&state, "xai-oauth-grok").await, "unknown");
        assert_eq!(
            admin_accounts_health(&state, "xai-oauth-grok").await,
            "unknown"
        );
    }
}
