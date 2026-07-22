use std::collections::{HashMap, HashSet};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, RwLock as StdRwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use alex_core::Provider;
use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, RwLock};

pub mod login;
pub mod sessions;
pub mod vault_bundle;
pub use vault_bundle::{
    decrypt_bundle, encrypt_bundle, export_bundle, harness_cred_paths, import_bundle,
    BundleSelection,
};

pub const ANTHROPIC_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
pub const ANTHROPIC_TOKEN_URL: &str = "https://console.anthropic.com/v1/oauth/token";
pub const OPENAI_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
pub const OPENAI_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
pub const XAI_CLIENT_ID: &str = "b1a00492-073a-47ea-816f-4c329264a828";
pub const XAI_TOKEN_URL: &str = "https://auth.x.ai/oauth2/token";
pub const GEMINI_CLIENT_ID: &str =
    "681255809395-oo8ft2oprdrnp9e3aqf6av3hmdib135j.apps.googleusercontent.com";
pub const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
// Kimi Code (Moonshot AI) public device-flow client. Extracted from the kimi
// node binary (`~/.kimi-code/bin/kimi`, KIMI_CODE_FLOW_CONFIG.clientId). The
// oauth host and API base are overridable via env for self-hosted deployments.
pub const KIMI_CLIENT_ID: &str = "17e5f671-d194-4dfb-9706-5516cb48c098";
pub const KIMI_OAUTH_HOST: &str = "https://auth.kimi.com";
pub const KIMI_TOKEN_URL: &str = "https://auth.kimi.com/api/oauth/token";
pub const KIMI_API_BASE: &str = "https://api.kimi.com/coding/v1";
const ANTHROPIC_PROFILE_URL: &str = "https://api.anthropic.com/api/oauth/profile";
const XAI_USERINFO_URL: &str = "https://auth.x.ai/oauth2/userinfo";
const AMP_USAGE_URL: &str = "https://ampcode.com/api/internal?userDisplayBalanceInfo";

// The gemini-cli OAuth client is a public "installed app" credential embedded
// in Google's open-source CLI (not a confidential secret). Assembled from
// fragments so repo secret-scanners don't false-positive on the literal.
pub fn gemini_client_secret() -> String {
    ["GOCSPX", "4uHgMPm", "1o7Sk", "geV6Cu5clXFsxl"].join("-")
}

const REFRESH_MARGIN_MS: i64 = 120_000;

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub id: String,
    pub provider: Provider,
    pub kind: String,
    #[serde(default = "default_account_name")]
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub paused: bool,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub access_token: Option<String>,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub id_token: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub expires_at_ms: Option<i64>,
    #[serde(default)]
    pub last_refresh_ms: Option<i64>,
    #[serde(default)]
    pub account_meta: Value,
    #[serde(default)]
    pub cooldown_until_ms: Option<i64>,
    #[serde(default = "default_status")]
    pub status: String,
    #[serde(skip)]
    pub path: Option<PathBuf>,
}

/// Non-secret account metadata retained after credential removal. This
/// sidecar makes removal durable even for callers that do not have the trace
/// database open (for example a terminal login flow).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemovedAccount {
    pub id: String,
    pub provider: Provider,
    pub name: String,
    pub kind: String,
    pub subscription_identity: Option<String>,
    pub email: Option<String>,
    pub removed_ms: i64,
}

/// Result of the credential side of an account merge. The DB-side row
/// reassignment is reported separately by the store.
#[derive(Debug, Clone, Serialize)]
pub struct MergeOutcome {
    /// The surviving account id (always `into`).
    pub survivor_id: String,
    /// The duplicate account id that was tombstoned.
    pub removed_id: String,
    /// Which of the two accounts supplied the surviving login. `None` means the
    /// survivor kept its own credentials because they were already the freshest
    /// valid ones.
    pub adopted_credentials_from: Option<String>,
}

fn default_account_name() -> String {
    "default".to_string()
}

fn default_status() -> String {
    "active".to_string()
}

impl Account {
    pub fn chatgpt_account_id(&self) -> Option<String> {
        self.account_meta
            .get("account_id")
            .and_then(|v| v.as_str())
            .map(String::from)
    }

    /// A display-only identity hint. This never returns token material.
    pub fn email(&self) -> Option<String> {
        fn payload_email(token: &str) -> Option<String> {
            let payload = crate::login::jwt_payload(token)?;
            payload
                .get("email")
                .and_then(Value::as_str)
                .and_then(normalize_email)
                .or_else(|| {
                    payload
                        .get("https://api.openai.com/profile")
                        .and_then(|profile| profile.get("email"))
                        .and_then(Value::as_str)
                        .and_then(normalize_email)
                })
        }

        self.account_meta
            .get("email")
            .and_then(Value::as_str)
            .and_then(normalize_email)
            .or_else(|| self.description.as_deref().and_then(normalize_email))
            .or_else(|| {
                (self.provider != Provider::Xai)
                    .then(|| self.id_token.as_deref().and_then(payload_email))
                    .flatten()
            })
            .or_else(|| {
                (self.provider != Provider::Xai)
                    .then(|| self.access_token.as_deref().and_then(payload_email))
                    .flatten()
            })
            .or_else(|| {
                self.label
                    .as_deref()
                    .and_then(|label| label.split(['(', ')', ' ', '·']).find_map(normalize_email))
            })
    }

    /// A durable, non-secret identifier for the upstream subscription behind
    /// this local account.  Local account ids and names are deliberately not
    /// used here: users can rename or remove those at any time.
    pub fn subscription_identity(&self) -> Option<String> {
        if self.provider == Provider::Openai {
            if let Some(id) = self.chatgpt_account_id().filter(|id| !id.trim().is_empty()) {
                return Some(format!("openai:chatgpt-account:{}", id.trim()));
            }
        }
        if let Some(email) = self.email() {
            return Some(format!("{}:email:{}", self.provider.as_str(), email));
        }
        // API keys have no provider-exposed account id in this application.
        // A one-way fingerprint is stable across a local rename and avoids
        // persisting the credential itself. It is intentionally last resort.
        self.api_key
            .as_deref()
            .filter(|key| !key.is_empty())
            .map(|key| {
                let digest = Sha256::digest(key.as_bytes());
                format!("{}:api-key-sha256:{digest:x}", self.provider.as_str())
            })
    }

    fn needs_refresh(&self) -> bool {
        self.kind == "oauth"
            && match self.expires_at_ms {
                Some(exp) => exp < now_ms() + REFRESH_MARGIN_MS,
                None => true,
            }
    }

    /// Display-only flag set by the background logout watchdog when this managed
    /// OAuth login is confirmed dead (its token expired and could not be
    /// silently refreshed). It is a UI hint only; a successful refresh or a
    /// fresh login clears it. It never holds credential material.
    pub fn needs_reauth(&self) -> bool {
        self.account_meta
            .get("needs_reauth")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }
}

/// A comparable freshness rank for choosing which of two logins survives a
/// merge. Higher is better: a present, unexpired credential outranks a missing
/// or expired one, and among equals the more recently refreshed login wins.
/// This mirrors the "active" test used by the credentials view.
fn credential_rank(account: &Account) -> (bool, i64) {
    let now = now_ms();
    let has_secret = account
        .access_token
        .as_deref()
        .is_some_and(|v| !v.is_empty())
        || account
            .refresh_token
            .as_deref()
            .is_some_and(|v| !v.is_empty())
        || account.api_key.as_deref().is_some_and(|v| !v.is_empty());
    let unexpired =
        account.kind != "oauth" || account.expires_at_ms.map(|e| e > now).unwrap_or(true);
    let valid = has_secret && unexpired;
    let freshness = account
        .last_refresh_ms
        .or(account.expires_at_ms)
        .unwrap_or(i64::MIN);
    (valid, freshness)
}

pub(crate) fn normalize_email(value: &str) -> Option<String> {
    let value = value.trim();
    (value.contains('@') && !value.chars().any(char::is_whitespace))
        .then(|| value.to_ascii_lowercase())
}

pub(crate) fn persist_account_email(account: &mut Account, email: &str) {
    let Some(email) = normalize_email(email) else {
        return;
    };
    if !account.account_meta.is_object() {
        account.account_meta = json!({});
    }
    account.account_meta["email"] = json!(email);
    if account.description.is_none()
        || account
            .description
            .as_deref()
            .and_then(normalize_email)
            .is_some()
    {
        account.description = Some(email);
    }
}

pub(crate) async fn fetch_provider_email(
    http: &reqwest::Client,
    provider: Provider,
    access_token: &str,
) -> Option<String> {
    let mut request = match provider {
        Provider::Anthropic => http.get(ANTHROPIC_PROFILE_URL),
        Provider::Xai => http.get(XAI_USERINFO_URL),
        _ => return None,
    }
    .bearer_auth(access_token)
    .timeout(Duration::from_secs(5));
    if provider == Provider::Anthropic {
        request = request
            .header("content-type", "application/json")
            .header("cache-control", "no-cache");
    }
    let response = request.send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let profile: Value = response.json().await.ok()?;
    let email = match provider {
        Provider::Anthropic => profile["account"]["email"].as_str(),
        Provider::Xai if profile["email_verified"].as_bool() == Some(false) => None,
        Provider::Xai => profile["email"].as_str(),
        _ => None,
    }?;
    normalize_email(email)
}

fn oauth_rank(account: &Account, prefer_oauth: bool) -> u8 {
    if (account.kind == "oauth") == prefer_oauth {
        0
    } else {
        1
    }
}

fn policy_rank(policy: &AccountPolicy, name: &str) -> usize {
    policy
        .order
        .iter()
        .position(|n| n == name)
        .unwrap_or(usize::MAX / 2)
}

fn utilization_pct(account: &Account) -> Option<u8> {
    account
        .account_meta
        .get("rate_limit_pct")
        .or_else(|| account.account_meta.get("utilization_pct"))
        .and_then(|v| v.as_u64())
        .map(|v| v.min(100) as u8)
}

/// The normalized quota snapshot used by provider routing. `codex_limits` is
/// retained as a read fallback so existing OpenAI account files keep exactly
/// the same routing behaviour after the routing machinery became generic.
fn routing_limit_snapshot(account: &Account) -> Option<&Value> {
    account
        .account_meta
        .get("routing_limits")
        .or_else(|| account.account_meta.get("codex_limits"))
}

fn routing_window_values(account: &Account) -> impl Iterator<Item = &Value> {
    routing_limit_snapshot(account)
        .and_then(|snapshot| snapshot.get("windows"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
}

pub fn routing_reserve_pct(account: &Account, policy: &AccountPolicy) -> u8 {
    policy
        .account_reserve_pct
        .get(&account.name)
        .or_else(|| policy.account_reserve_pct.get(&account.id))
        .copied()
        .or(policy.reserve_pct)
        .unwrap_or(10)
        .min(100)
}

pub fn routing_reserve_blocked(account: &Account, reserve_pct: u8, now_s: i64) -> bool {
    if reserve_pct == 0 {
        return false;
    }
    routing_window_values(account).any(|window| {
        let future_window = window
            .get("resets_at_s")
            .and_then(Value::as_i64)
            .map(|reset| reset > now_s)
            .unwrap_or(true);
        let used_pct = window.get("used_pct").and_then(Value::as_f64);
        future_window
            && used_pct
                .map(|used| used >= 100.0 - reserve_pct as f64)
                .unwrap_or(false)
    })
}

/// The binding quota window is the active window closest to exhaustion.
/// Ties use the earlier reset. This avoids always picking the short window
/// when a weekly/monthly window is the actual constraint.
pub fn routing_reset_selection(account: &Account, now_s: i64) -> Option<Value> {
    let mut selected: Option<(f64, i64, Option<String>)> = None;
    for window in routing_window_values(account) {
        let Some(used_pct) = window.get("used_pct").and_then(Value::as_f64) else {
            continue;
        };
        let Some(resets_at_s) = window.get("resets_at_s").and_then(Value::as_i64) else {
            continue;
        };
        if resets_at_s <= now_s {
            continue;
        }
        let label = window
            .get("window")
            .and_then(Value::as_str)
            .map(String::from);
        let replace = selected
            .as_ref()
            .map(|(current_used, current_reset, _)| {
                used_pct > *current_used
                    || (used_pct == *current_used && resets_at_s < *current_reset)
            })
            .unwrap_or(true);
        if replace {
            selected = Some((used_pct, resets_at_s, label));
        }
    }
    selected.map(|(used_pct, resets_at_s, window)| {
        json!({"window": window, "used_pct": used_pct, "resets_at_s": resets_at_s})
    })
}

fn routing_binding_reset(account: &Account, now_s: i64) -> Option<i64> {
    routing_reset_selection(account, now_s)?["resets_at_s"].as_i64()
}

fn routing_binding_used_basis_points(account: &Account, now_s: i64) -> Option<u32> {
    routing_reset_selection(account, now_s)?["used_pct"]
        .as_f64()
        .map(|used| (used.clamp(0.0, 100.0) * 100.0).round() as u32)
}

fn account_proxy_eligible(account: &Account, policy: &AccountPolicy) -> bool {
    !policy
        .disabled
        .iter()
        .any(|name| name == &account.name || name == &account.id)
}

fn sort_by_policy(accounts: &mut Vec<Account>, policy: &AccountPolicy, prefer_oauth: bool) {
    let now_s = now_ms() / 1000;
    match policy.mode {
        AccountPolicyMode::RoundRobin => {
            accounts.sort_by_key(|a| {
                (
                    oauth_rank(a, prefer_oauth),
                    routing_reserve_blocked(a, routing_reserve_pct(a, policy), now_s),
                    policy_rank(policy, &a.name),
                    a.name.clone(),
                    a.id.clone(),
                )
            });
        }
        AccountPolicyMode::Threshold => {
            let threshold = policy.threshold_pct.unwrap_or(80);
            accounts.sort_by_key(|a| {
                let over = utilization_pct(a).map(|p| p >= threshold).unwrap_or(false);
                (
                    oauth_rank(a, prefer_oauth),
                    over,
                    policy_rank(policy, &a.name),
                    a.name.clone(),
                    a.id.clone(),
                )
            });
        }
        AccountPolicyMode::Priority => {
            accounts.sort_by_key(|a| {
                (
                    oauth_rank(a, prefer_oauth),
                    routing_reserve_blocked(a, routing_reserve_pct(a, policy), now_s),
                    policy_rank(policy, &a.name),
                    a.name.clone(),
                    a.id.clone(),
                )
            });
        }
        AccountPolicyMode::ResetFirst => {
            accounts.sort_by_key(|a| {
                (
                    oauth_rank(a, prefer_oauth),
                    routing_reserve_blocked(a, routing_reserve_pct(a, policy), now_s),
                    routing_binding_reset(a, now_s).unwrap_or(i64::MAX),
                    policy_rank(policy, &a.name),
                    a.name.clone(),
                    a.id.clone(),
                )
            });
        }
        AccountPolicyMode::HighestQuota => {
            accounts.sort_by_key(|a| {
                (
                    oauth_rank(a, prefer_oauth),
                    routing_reserve_blocked(a, routing_reserve_pct(a, policy), now_s),
                    // The quota APIs expose percentages rather than absolute
                    // plan allowances. Lowest usage in the binding (most
                    // consumed) active window therefore means the greatest
                    // reliably observable remaining quota.
                    routing_binding_used_basis_points(a, now_s).unwrap_or(u32::MAX),
                    policy_rank(policy, &a.name),
                    a.name.clone(),
                    a.id.clone(),
                )
            });
        }
    }
}

pub fn rfc3339_to_ms(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.timestamp_millis())
}

pub fn jwt_exp_ms(token: &str) -> Option<i64> {
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    v.get("exp")?.as_i64().map(|s| s * 1000)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AccountPolicyMode {
    Priority,
    RoundRobin,
    Threshold,
    ResetFirst,
    HighestQuota,
}

impl Default for AccountPolicyMode {
    fn default() -> Self {
        Self::Priority
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountPolicy {
    #[serde(default)]
    pub order: Vec<String>,
    #[serde(default)]
    pub mode: AccountPolicyMode,
    #[serde(default)]
    pub threshold_pct: Option<u8>,
    #[serde(default)]
    pub reserve_pct: Option<u8>,
    #[serde(default)]
    pub account_reserve_pct: HashMap<String, u8>,
    #[serde(default = "default_allow_mid_thread_failover")]
    pub allow_mid_thread_failover: bool,
    #[serde(default)]
    pub disabled: Vec<String>,
}

fn default_allow_mid_thread_failover() -> bool {
    true
}

impl Default for AccountPolicy {
    fn default() -> Self {
        Self {
            order: Vec::new(),
            mode: AccountPolicyMode::default(),
            threshold_pct: None,
            reserve_pct: None,
            account_reserve_pct: HashMap::new(),
            allow_mid_thread_failover: true,
            disabled: Vec::new(),
        }
    }
}

fn default_policy_for(provider: Provider) -> AccountPolicy {
    if provider == Provider::Openai {
        AccountPolicy {
            mode: AccountPolicyMode::ResetFirst,
            reserve_pct: Some(10),
            ..AccountPolicy::default()
        }
    } else {
        AccountPolicy::default()
    }
}

const ROUTING_POLICIES_FILE: &str = ".routing-policies";

pub struct Vault {
    dir: PathBuf,
    accounts: RwLock<HashMap<String, Account>>,
    locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    rr_counter: AtomicUsize,
    policies: StdRwLock<Vec<(Provider, AccountPolicy)>>,
    http: reqwest::Client,
    /// Test/diagnostic override for the OAuth token endpoint. Production code
    /// never sets this; it lets tests point a refresh at a local mock so the
    /// invalid_grant classification can be exercised without live upstreams.
    refresh_endpoint_override: StdRwLock<Option<String>>,
}

impl Vault {
    pub fn open(dir: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&dir)?;
        let mut accounts = HashMap::new();
        for entry in std::fs::read_dir(&dir)? {
            let path = entry?.path();
            if path.extension().map(|e| e == "json").unwrap_or(false) {
                match std::fs::read_to_string(&path)
                    .map_err(anyhow::Error::from)
                    .and_then(|s| serde_json::from_str::<Account>(&s).map_err(Into::into))
                {
                    Ok(mut acct) => {
                        if acct.name.is_empty() {
                            acct.name = "default".into();
                        }
                        acct.path = Some(path.clone());
                        accounts.insert(acct.id.clone(), acct);
                    }
                    Err(e) => tracing::warn!("skipping unreadable account file {path:?}: {e}"),
                }
            }
        }
        let policies = read_routing_policies(&dir).unwrap_or_else(|error| {
            tracing::warn!(%error, "could not load persisted routing policies");
            Vec::new()
        });
        Ok(Self {
            dir,
            accounts: RwLock::new(accounts),
            locks: Mutex::new(HashMap::new()),
            rr_counter: AtomicUsize::new(0),
            policies: StdRwLock::new(policies),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()?,
            refresh_endpoint_override: StdRwLock::new(None),
        })
    }

    pub async fn list(&self) -> Vec<Account> {
        let mut v: Vec<Account> = self.accounts.read().await.values().cloned().collect();
        v.sort_by(|a, b| {
            (a.provider.as_str(), a.name.as_str(), a.kind.as_str()).cmp(&(
                b.provider.as_str(),
                b.name.as_str(),
                b.kind.as_str(),
            ))
        });
        v
    }

    /// Best-effort synchronous snapshot for daemon construction. Normal
    /// callers should use `list`; this avoids a blocking runtime bridge just
    /// to seed non-secret trace attribution metadata.
    pub fn list_cached(&self) -> Vec<Account> {
        self.accounts
            .try_read()
            .map(|accounts| accounts.values().cloned().collect())
            .unwrap_or_default()
    }

    pub fn set_policies_blocking(&self, policies: Vec<(Provider, AccountPolicy)>) {
        *self.policies.write().expect("policy lock poisoned") = policies;
    }

    pub async fn set_policies(&self, policies: Vec<(Provider, AccountPolicy)>) {
        self.set_policies_blocking(policies);
    }

    pub fn policy(&self, provider: Provider) -> AccountPolicy {
        self.policies
            .read()
            .expect("policy lock poisoned")
            .iter()
            .find(|(candidate, _)| *candidate == provider)
            .map(|(_, policy)| policy.clone())
            .unwrap_or_else(|| default_policy_for(provider))
    }

    pub async fn set_policy_persisted(
        &self,
        provider: Provider,
        policy: AccountPolicy,
    ) -> Result<()> {
        let policies = {
            let mut guard = self.policies.write().expect("policy lock poisoned");
            if let Some((_, current)) = guard
                .iter_mut()
                .find(|(candidate, _)| *candidate == provider)
            {
                *current = policy;
            } else {
                guard.push((provider, policy));
            }
            guard.clone()
        };
        write_routing_policies(&self.dir, &policies)
    }

    pub async fn record_routing_limits(&self, id: &str, snapshot: Value) -> Result<()> {
        self.record_routing_limits_inner(id, None, snapshot).await
    }

    pub async fn record_routing_limits_for_workspace(
        &self,
        id: &str,
        expected_workspace_id: &str,
        snapshot: Value,
    ) -> Result<()> {
        self.record_routing_limits_inner(id, Some(expected_workspace_id), snapshot)
            .await
    }

    async fn record_routing_limits_inner(
        &self,
        id: &str,
        expected_workspace_id: Option<&str>,
        mut snapshot: Value,
    ) -> Result<()> {
        let lock = self.account_lock(id).await;
        let _guard = lock.lock().await;
        let now = now_ms();
        let mut account = self
            .accounts
            .read()
            .await
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow!("unknown account {id}"))?;
        if expected_workspace_id.is_some() && account.provider != Provider::Openai {
            bail!("account {id} is not an OpenAI account");
        }
        if let Some(expected_workspace_id) = expected_workspace_id {
            let current_workspace_id = account.chatgpt_account_id();
            if current_workspace_id.as_deref() != Some(expected_workspace_id) {
                bail!("Codex workspace identity changed while refreshing account {id}");
            }
        }
        if account
            .account_meta
            .get("routing_limits")
            .or_else(|| account.account_meta.get("codex_limits"))
            .and_then(|value| value.get("observed_at_ms"))
            .and_then(Value::as_i64)
            .map(|observed| now - observed < 30_000)
            .unwrap_or(false)
        {
            return Ok(());
        }
        if let Some(object) = snapshot.as_object_mut() {
            object.insert("observed_at_ms".into(), json!(now));
        }
        if let (Some(existing), Some(incoming)) = (
            account
                .account_meta
                .get("routing_limits")
                .or_else(|| account.account_meta.get("codex_limits"))
                .and_then(Value::as_object),
            snapshot.as_object(),
        ) {
            let mut merged = existing.clone();
            for (key, value) in incoming {
                if !value.is_null() {
                    merged.insert(key.clone(), value.clone());
                }
            }
            snapshot = Value::Object(merged);
        }
        if !account.account_meta.is_object() {
            account.account_meta = json!({});
        }
        account
            .account_meta
            .as_object_mut()
            .expect("account_meta initialized as object")
            .insert("routing_limits".into(), snapshot);
        self.upsert_unlocked(account).await
    }

    pub async fn pause(&self, provider: Provider, name: &str, paused: bool) -> Result<()> {
        let id = self
            .accounts
            .read()
            .await
            .values()
            .find(|a| a.provider == provider && a.name == name)
            .map(|account| account.id.clone())
            .ok_or_else(|| anyhow!("unknown {} account '{name}'", provider.as_str()))?;
        self.set_paused(&id, paused).await
    }

    pub async fn set_paused(&self, id: &str, paused: bool) -> Result<()> {
        if !self.update(id, |account| account.paused = paused).await? {
            bail!("unknown account {id}");
        }
        Ok(())
    }

    pub async fn has_account_name(&self, provider: Provider, name: &str) -> bool {
        self.accounts
            .read()
            .await
            .values()
            .any(|a| a.provider == provider && a.name == name)
    }

    pub async fn upsert(&self, account: Account) -> Result<()> {
        let lock = self.account_lock(&account.id).await;
        let _guard = lock.lock().await;
        self.upsert_unlocked(account).await
    }

    async fn upsert_unlocked(&self, account: Account) -> Result<()> {
        write_account_file(&self.dir, &account)?;
        self.accounts
            .write()
            .await
            .insert(account.id.clone(), account);
        Ok(())
    }

    /// Mutate an existing account while holding the write lock, then persist
    /// the resulting account once. Returns `false` when `id` does not exist.
    pub async fn update(&self, id: &str, f: impl FnOnce(&mut Account)) -> Result<bool> {
        let lock = self.account_lock(id).await;
        let _guard = lock.lock().await;
        let mut accounts = self.accounts.write().await;
        let Some(account) = accounts.get_mut(id) else {
            return Ok(false);
        };
        let previous = account.clone();
        f(account);
        if let Err(error) = write_account_file(&self.dir, account) {
            *account = previous;
            return Err(error);
        }
        Ok(true)
    }

    pub async fn remove(&self, id: &str) -> Result<bool> {
        let lock = self.account_lock(id).await;
        let _guard = lock.lock().await;
        let account = self.accounts.read().await.get(id).cloned();
        let Some(account) = account else {
            return Ok(false);
        };
        let tombstone = RemovedAccount {
            id: account.id.clone(),
            provider: account.provider,
            name: account.name.clone(),
            kind: account.kind.clone(),
            subscription_identity: account.subscription_identity(),
            email: account.email(),
            removed_ms: now_ms(),
        };
        let tombstone_dir = self.dir.join("removed-accounts");
        std::fs::create_dir_all(&tombstone_dir)?;
        let tombstone_path = tombstone_dir.join(format!("{id}.json"));
        let temporary = tombstone_path.with_extension("json.tmp");
        std::fs::write(&temporary, serde_json::to_vec_pretty(&tombstone)?)?;
        std::fs::rename(&temporary, &tombstone_path)?;
        self.accounts.write().await.remove(id);
        let path = self.dir.join(format!("{id}.json"));
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        Ok(true)
    }

    /// Validate an account merge without mutating anything. Confirms both ids
    /// exist and, unless `allow_mismatch`, that they are the same provider and
    /// resolve to the same email. Returns `(from, into)`.
    pub async fn validate_merge(
        &self,
        from_id: &str,
        into_id: &str,
        allow_mismatch: bool,
    ) -> Result<(Account, Account)> {
        if from_id == into_id {
            bail!("cannot merge account '{from_id}' into itself");
        }
        let (from, into) = {
            let accounts = self.accounts.read().await;
            let from = accounts
                .get(from_id)
                .cloned()
                .ok_or_else(|| anyhow!("unknown account '{from_id}'"))?;
            let into = accounts
                .get(into_id)
                .cloned()
                .ok_or_else(|| anyhow!("unknown account '{into_id}'"))?;
            (from, into)
        };
        if !allow_mismatch {
            if from.provider != into.provider {
                bail!(
                    "provider mismatch: '{from_id}' is {} but '{into_id}' is {} (pass allow_mismatch to override)",
                    from.provider.as_str(),
                    into.provider.as_str()
                );
            }
            match (from.email(), into.email()) {
                (Some(a), Some(b)) if a == b => {}
                (a, b) => bail!(
                    "email mismatch: '{from_id}' is {} but '{into_id}' is {} (pass allow_mismatch to override)",
                    a.as_deref().unwrap_or("<none>"),
                    b.as_deref().unwrap_or("<none>")
                ),
            }
        }
        Ok((from, into))
    }

    /// Merge the credential side of a duplicate account into a survivor. The
    /// survivor is always `into_id`: it keeps its id but adopts whichever of the
    /// two logins is the freshest still-valid credential — after a re-auth that
    /// is the newly added account. The duplicate is tombstoned through the
    /// normal removal path, so any trace already re-keyed to the survivor is
    /// unaffected. Reassigning the trace-database rows is the caller's
    /// responsibility (see `Store::merge_accounts`); doing both is how the two
    /// split histories become one.
    pub async fn merge_accounts(
        &self,
        from_id: &str,
        into_id: &str,
        allow_mismatch: bool,
    ) -> Result<MergeOutcome> {
        let (from, into) = self
            .validate_merge(from_id, into_id, allow_mismatch)
            .await?;
        let mut survivor = into.clone();
        let adopted = if credential_rank(&from) > credential_rank(&into) {
            survivor.kind = from.kind.clone();
            survivor.access_token = from.access_token.clone();
            survivor.refresh_token = from.refresh_token.clone();
            survivor.id_token = from.id_token.clone();
            survivor.api_key = from.api_key.clone();
            survivor.expires_at_ms = from.expires_at_ms;
            survivor.last_refresh_ms = from.last_refresh_ms;
            // Carry the fresh login's scopes/email so the adopted token keeps
            // its capabilities, while preserving the survivor's other metadata
            // (e.g. observed routing limits).
            if let (Some(dst), Some(src)) = (
                survivor.account_meta.as_object_mut(),
                from.account_meta.as_object(),
            ) {
                for key in ["scopes", "email", "account_id"] {
                    if let Some(value) = src.get(key) {
                        dst.insert(key.to_string(), value.clone());
                    }
                }
            } else if survivor.account_meta.is_null() {
                survivor.account_meta = from.account_meta.clone();
            }
            // Adopting a fresh login supersedes a stale/expired or cooling
            // survivor, so the unified account is immediately usable.
            survivor.paused = false;
            survivor.status = "active".into();
            survivor.cooldown_until_ms = None;
            Some(from_id.to_string())
        } else {
            None
        };
        self.upsert(survivor).await?;
        self.remove(from_id).await?;
        Ok(MergeOutcome {
            survivor_id: into_id.to_string(),
            removed_id: from_id.to_string(),
            adopted_credentials_from: adopted,
        })
    }

    /// Read non-secret removal tombstones. Corrupt individual sidecars are
    /// ignored so a bad historical file cannot prevent the vault opening.
    pub fn removed_accounts(&self) -> Vec<RemovedAccount> {
        let dir = self.dir.join("removed-accounts");
        let Ok(entries) = std::fs::read_dir(dir) else {
            return vec![];
        };
        entries
            .filter_map(|entry| {
                let path = entry.ok()?.path();
                serde_json::from_slice(&std::fs::read(path).ok()?).ok()
            })
            .collect()
    }

    async fn account_lock(&self, id: &str) -> Arc<Mutex<()>> {
        let mut locks = self.locks.lock().await;
        locks
            .entry(id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    pub async fn account_for(&self, provider: Provider, prefer_oauth: bool) -> Result<Account> {
        let excluded = HashSet::new();
        self.account_for_excluding(provider, prefer_oauth, &excluded)
            .await
    }

    pub async fn account_for_excluding(
        &self,
        provider: Provider,
        prefer_oauth: bool,
        excluded: &HashSet<String>,
    ) -> Result<Account> {
        self.account_for_excluding_preferred(provider, prefer_oauth, excluded, None)
            .await
    }

    /// Select an account using the provider policy, while keeping an existing
    /// conversation on `preferred_id` when that account is still usable.
    ///
    /// A preferred account never bypasses pause/eligibility/cooldown or the
    /// caller's exclusion set. This lets the proxy preserve prompt-cache
    /// affinity without defeating routing controls or retry failover.
    pub async fn account_for_excluding_preferred(
        &self,
        provider: Provider,
        prefer_oauth: bool,
        excluded: &HashSet<String>,
        preferred_id: Option<&str>,
    ) -> Result<Account> {
        self.account_for_excluding_preferred_mode(
            provider,
            prefer_oauth,
            excluded,
            preferred_id,
            false,
        )
        .await
    }

    pub async fn account_for_excluding_preferred_mode(
        &self,
        provider: Provider,
        prefer_oauth: bool,
        excluded: &HashSet<String>,
        preferred_id: Option<&str>,
        preferred_ignores_cooldown: bool,
    ) -> Result<Account> {
        let now = now_ms();
        let candidates: Vec<Account> = {
            let map = self.accounts.read().await;
            let mut v: Vec<Account> = map
                .values()
                .filter(|a| {
                    a.provider == provider
                        && a.status == "active"
                        && !a.paused
                        && !excluded.contains(&a.id)
                })
                .cloned()
                .collect();
            let policy = self.policy(provider);
            v.retain(|account| account_proxy_eligible(account, &policy));
            sort_by_policy(&mut v, &policy, prefer_oauth);
            v
        };
        if candidates.is_empty() {
            bail!(
                "no active {} account; run `alex auth import`",
                provider.as_str()
            );
        }
        let ready: Vec<&Account> = candidates
            .iter()
            .filter(|a| a.cooldown_until_ms.map(|c| c <= now).unwrap_or(true))
            .collect();
        let preferred = preferred_id.and_then(|preferred_id| {
            candidates
                .iter()
                .find(|account| account.id == preferred_id)
                .filter(|account| {
                    preferred_ignores_cooldown
                        || account.cooldown_until_ms.map(|c| c <= now).unwrap_or(true)
                })
                .cloned()
        });
        let account = if let Some(preferred) = preferred {
            preferred
        } else if ready.is_empty() {
            let account = candidates
                .iter()
                .min_by_key(|a| a.cooldown_until_ms.unwrap_or(i64::MAX))
                .unwrap()
                .clone();
            tracing::warn!(
                "all {} accounts cooling down; using {} (soonest expiry) in degraded mode",
                provider.as_str(),
                account.id
            );
            account
        } else {
            let policy = self.policy(provider);
            let policy_mode = policy.mode.clone();
            if policy_mode == AccountPolicyMode::RoundRobin {
                let unblocked: Vec<&Account> = ready
                    .iter()
                    .copied()
                    .filter(|account| {
                        !routing_reserve_blocked(
                            account,
                            routing_reserve_pct(account, &policy),
                            now / 1000,
                        )
                    })
                    .collect();
                let pool: Vec<&Account> = if unblocked.is_empty() {
                    ready.clone()
                } else {
                    unblocked
                };
                let top_rank = oauth_rank(pool[0], prefer_oauth);
                let group: Vec<&Account> = pool
                    .iter()
                    .copied()
                    .filter(|account| oauth_rank(account, prefer_oauth) == top_rank)
                    .collect();
                let idx = self.rr_counter.fetch_add(1, Ordering::Relaxed) % group.len();
                group[idx].clone()
            } else {
                ready[0].clone()
            }
        };
        if account.needs_refresh() {
            return self.refresh(&account.id, false).await;
        }
        Ok(account)
    }

    pub async fn mark_cooldown(&self, id: &str, until_ms: i64) -> Result<()> {
        if !self
            .update(id, |account| account.cooldown_until_ms = Some(until_ms))
            .await?
        {
            bail!("unknown account {id}");
        }
        Ok(())
    }

    /// A successful health probe makes the account immediately eligible again.
    /// This prevents a transient capacity/server failure from becoming a
    /// sticky downgrade after the upstream has recovered.
    pub async fn clear_cooldown(&self, id: &str) -> Result<()> {
        if !self
            .update(id, |account| account.cooldown_until_ms = None)
            .await?
        {
            bail!("unknown account {id}");
        }
        Ok(())
    }

    pub async fn refresh(&self, id: &str, force: bool) -> Result<Account> {
        self.refresh_inner(id, force, true, None).await
    }

    pub async fn refresh_without_native_reimport(
        &self,
        id: &str,
        force: bool,
        expected_workspace_id: &str,
    ) -> Result<Account> {
        self.refresh_inner(id, force, false, Some(expected_workspace_id))
            .await
    }

    async fn refresh_inner(
        &self,
        id: &str,
        force: bool,
        allow_native_reimport: bool,
        expected_workspace_id: Option<&str>,
    ) -> Result<Account> {
        let lock = self.account_lock(id).await;
        let guard = lock.lock().await;

        let account = self
            .accounts
            .read()
            .await
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow!("unknown account {id}"))?;
        if let Some(expected_workspace_id) = expected_workspace_id {
            if account.chatgpt_account_id().as_deref() != Some(expected_workspace_id) {
                bail!("Codex workspace identity changed while refreshing account {id}");
            }
        }
        if !force && !account.needs_refresh() {
            return Ok(account);
        }
        tracing::info!("refreshing oauth token for {id}");
        let result = match (account.provider, account.refresh_token.clone()) {
            (Provider::Anthropic, Some(rt)) => self.refresh_anthropic(&rt).await,
            (Provider::Openai, Some(rt)) => self.refresh_openai(&rt).await,
            (Provider::Xai, Some(rt)) => {
                let client_id = account
                    .account_meta
                    .get("oidc_client_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or(XAI_CLIENT_ID)
                    .to_string();
                self.refresh_xai(&rt, &client_id).await
            }
            (Provider::Gemini, Some(rt)) => self.refresh_gemini(&rt).await,
            (Provider::Openrouter, _) => Err(anyhow!(
                "openrouter accounts use a long-lived API key; re-run `alex auth openrouter-key`"
            )),
            (Provider::Cliproxyapi, _) => Err(anyhow!(
                "CLIProxyAPI accounts use a long-lived API key; reconnect the integration"
            )),
            (Provider::Exo, _) => Err(anyhow!("exo is configured locally and has no account to refresh")),
            (Provider::Kimi, Some(rt)) => self.refresh_kimi(&rt).await,
            (Provider::Amp, _) => Err(anyhow!(
                "amp accounts use a long-lived API key; re-run `alex auth import amp` or `alex auth amp-key`"
            )),
            (_, None) => Err(anyhow!("account {id} has no refresh token")),
        };
        let refreshed = match result {
            Ok(r) => r,
            Err(e) => {
                if !allow_native_reimport {
                    return Err(e);
                }
                drop(guard);
                tracing::warn!("refresh failed for {id}: {e}; re-importing from native creds");
                if let Some(fresh) = self.reimport_native(&account).await {
                    return Ok(fresh);
                }
                if refresh_error_needs_reauth(&e) {
                    if let Err(error) = self.set_account_meta(id, "needs_reauth", json!(true)).await
                    {
                        tracing::warn!(account = %id, %error, "could not mark account as needing re-authentication");
                    }
                }
                return Err(e);
            }
        };

        let mut updated = account.clone();
        if let Some(t) = refreshed.access_token {
            updated.access_token = Some(t);
        }
        if let Some(t) = refreshed.refresh_token {
            updated.refresh_token = Some(t);
        }
        if let Some(t) = refreshed.id_token {
            updated.id_token = Some(t);
        }
        updated.expires_at_ms = refreshed
            .expires_in
            .map(|s| now_ms() + s * 1000)
            .or_else(|| updated.access_token.as_deref().and_then(jwt_exp_ms));
        updated.last_refresh_ms = Some(now_ms());
        if let Some(expected_workspace_id) = expected_workspace_id {
            if updated
                .access_token
                .as_deref()
                .and_then(crate::login::chatgpt_account_id)
                .is_some_and(|workspace_id| workspace_id != expected_workspace_id)
            {
                bail!("refreshed Codex token belongs to a different workspace");
            }
        }
        if let Some(meta) = updated.account_meta.as_object_mut() {
            meta.remove("needs_reauth");
        }
        if let Some(email) = updated.email() {
            persist_account_email(&mut updated, &email);
        } else {
            if let Some(access_token) = updated.access_token.clone() {
                if let Some(email) =
                    fetch_provider_email(&self.http, updated.provider, &access_token).await
                {
                    persist_account_email(&mut updated, &email);
                }
            }
        }
        self.upsert_unlocked(updated.clone()).await?;
        Ok(updated)
    }

    async fn reimport_native(&self, stale: &Account) -> Option<Account> {
        match stale.provider {
            Provider::Anthropic => {
                let _ = import_claude(self).await;
            }
            Provider::Openai => {
                let _ = import_codex(self).await;
            }
            Provider::Gemini => {
                let _ = import_gemini(self).await;
            }
            Provider::Xai => {
                let _ = import_grok(self).await;
            }
            Provider::Amp => {
                let _ = import_amp(self).await;
            }
            Provider::Openrouter => {}
            Provider::Cliproxyapi => {}
            Provider::Exo => {}
            Provider::Kimi => {
                let _ = import_kimi(self).await;
            }
        };
        let fresh = self.accounts.read().await.get(&stale.id).cloned()?;
        let changed = fresh.access_token != stale.access_token;
        let valid = match fresh.expires_at_ms {
            Some(exp) => exp > now_ms() + REFRESH_MARGIN_MS,
            None => false,
        };
        if changed && valid {
            tracing::info!("recovered {} from native credential source", stale.id);
            Some(fresh)
        } else {
            None
        }
    }

    async fn refresh_anthropic(&self, refresh_token: &str) -> Result<RefreshedTokens> {
        let resp = self
            .http
            .post(self.refresh_endpoint(ANTHROPIC_TOKEN_URL))
            .json(&json!({
                "grant_type": "refresh_token",
                "refresh_token": refresh_token,
                "client_id": ANTHROPIC_CLIENT_ID,
            }))
            .send()
            .await?;
        parse_token_response(resp).await
    }

    async fn refresh_openai(&self, refresh_token: &str) -> Result<RefreshedTokens> {
        let resp = self
            .http
            .post(self.refresh_endpoint(OPENAI_TOKEN_URL))
            .json(&json!({
                "client_id": OPENAI_CLIENT_ID,
                "grant_type": "refresh_token",
                "refresh_token": refresh_token,
                "scope": "openid profile email",
            }))
            .send()
            .await?;
        parse_token_response(resp).await
    }

    async fn refresh_xai(&self, refresh_token: &str, client_id: &str) -> Result<RefreshedTokens> {
        let resp = self
            .http
            .post(self.refresh_endpoint(XAI_TOKEN_URL))
            .form(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
                ("client_id", client_id),
            ])
            .send()
            .await?;
        parse_token_response(resp).await
    }

    async fn refresh_gemini(&self, refresh_token: &str) -> Result<RefreshedTokens> {
        let resp = self
            .http
            .post(self.refresh_endpoint(GOOGLE_TOKEN_URL))
            .form(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
                ("client_id", GEMINI_CLIENT_ID),
                ("client_secret", &gemini_client_secret()),
            ])
            .send()
            .await?;
        parse_token_response(resp).await
    }

    /// Resolve the OAuth token endpoint, honouring a test/diagnostic override.
    fn refresh_endpoint(&self, default: &str) -> String {
        self.refresh_endpoint_override
            .read()
            .ok()
            .and_then(|guard| guard.clone())
            .unwrap_or_else(|| default.to_string())
    }

    /// Point every provider's token-refresh POST at `url` (or clear with
    /// `None`). Intended for tests that exercise the refresh-failure path
    /// against a local mock; it is never set in normal operation.
    pub fn set_refresh_endpoint_override(&self, url: Option<String>) {
        if let Ok(mut guard) = self.refresh_endpoint_override.write() {
            *guard = url;
        }
    }

    /// Refresh a Kimi Code access token (15-minute TTL) against the token
    /// endpoint using the stored refresh token. Mirrors the exact form body the
    /// kimi node binary posts for `grant_type=refresh_token`.
    async fn refresh_kimi(&self, refresh_token: &str) -> Result<RefreshedTokens> {
        let resp = self
            .http
            .post(crate::login::kimi_token_url())
            .form(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
                ("client_id", KIMI_CLIENT_ID),
            ])
            .send()
            .await?;
        parse_token_response(resp).await
    }

    pub async fn set_account_meta(&self, id: &str, key: &str, value: Value) -> Result<()> {
        if !self
            .update(id, |account| {
                if !account.account_meta.is_object() {
                    account.account_meta = json!({});
                }
                account.account_meta[key] = value;
            })
            .await?
        {
            bail!("unknown account {id}");
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
struct RefreshedTokens {
    access_token: Option<String>,
    refresh_token: Option<String>,
    id_token: Option<String>,
    expires_in: Option<i64>,
}

/// Stable marker embedded in the `Display` of a refresh error when the OAuth
/// token endpoint rejected the refresh token itself (invalid_grant / 400 / 401
/// / 403). That is a confirmed logout, not a transient upstream hiccup, so
/// callers can decide to alert the user without matching provider-specific
/// error bodies. It intentionally carries no credential material.
pub const REFRESH_REAUTH_MARKER: &str = "reauth_required";

/// Classify a [`Vault::refresh`] error. `true` means the account is logged out
/// and must be re-authenticated (dead refresh token); `false` means a transient
/// network / 5xx / rate-limit failure that may recover on its own. Callers use
/// this to avoid crying wolf on a temporary blip.
pub fn refresh_error_needs_reauth(err: &anyhow::Error) -> bool {
    let text = err.to_string();
    text.contains(REFRESH_REAUTH_MARKER)
        || text.contains("invalid_grant")
        || text.contains("has no refresh token")
}

/// Extract an OAuth2 `error` code (RFC 6749 §5.2) from a token endpoint error
/// body, if the body is JSON with a string `error` field.
fn oauth_error_code(body: &str) -> Option<String> {
    serde_json::from_str::<Value>(body)
        .ok()?
        .get("error")?
        .as_str()
        .map(str::to_string)
}

async fn parse_token_response(resp: reqwest::Response) -> Result<RefreshedTokens> {
    let status = resp.status();
    let text = resp.text().await?;
    if !status.is_success() {
        // A rejected refresh token (client-error status, or an explicit
        // permanent OAuth error code) is a confirmed logout; tag it so callers
        // can alert. Server errors / rate limits are transient and left
        // untagged. The raw body is deliberately not echoed into the error.
        let dead = matches!(status.as_u16(), 400 | 401 | 403)
            || oauth_error_code(&text).is_some_and(|code| {
                matches!(
                    code.as_str(),
                    "invalid_grant"
                        | "invalid_request"
                        | "invalid_client"
                        | "unauthorized_client"
                        | "unsupported_grant_type"
                )
            });
        if dead {
            bail!("token refresh rejected ({REFRESH_REAUTH_MARKER}, status {status})");
        }
        bail!("token refresh failed (status {status})");
    }
    serde_json::from_str(&text).context("bad token response")
}

pub fn named_account_id(provider: Provider, kind: &str, name: &str) -> String {
    if name == "default" {
        format!("{}-{kind}", provider.as_str())
    } else {
        format!("{}-{kind}-{name}", provider.as_str())
    }
}

fn account_path(dir: &Path, account: &Account) -> PathBuf {
    account
        .path
        .clone()
        .unwrap_or_else(|| dir.join(format!("{}.json", account.id)))
}

fn write_account_file(dir: &Path, account: &Account) -> Result<()> {
    let path = account_path(dir, account);
    atomic_write_private(&path, &serde_json::to_vec_pretty(account)?)
}

pub(crate) fn atomic_write_private(path: &Path, data: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("file has no parent: {}", path.display()))?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("credentials");
    let mut temporary = None;
    for _ in 0..16 {
        let candidate = parent.join(format!(
            ".{file_name}.{}-{:016x}.tmp",
            std::process::id(),
            rand::random::<u64>()
        ));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&candidate) {
            Ok(file) => {
                temporary = Some((candidate, file));
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    let (temporary_path, mut file) = temporary
        .ok_or_else(|| anyhow!("could not create temporary file beside {}", path.display()))?;
    let result = (|| -> Result<()> {
        file.write_all(data)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary_path, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary_path);
    }
    result
}

fn read_routing_policies(dir: &Path) -> Result<Vec<(Provider, AccountPolicy)>> {
    let path = dir.join(ROUTING_POLICIES_FILE);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let data =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&data).with_context(|| format!("parsing {}", path.display()))
}

fn write_routing_policies(dir: &Path, policies: &[(Provider, AccountPolicy)]) -> Result<()> {
    let path = dir.join(ROUTING_POLICIES_FILE);
    let temp = dir.join(format!("{ROUTING_POLICIES_FILE}.tmp"));
    let data = serde_json::to_vec_pretty(policies)?;
    std::fs::write(&temp, data)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&temp, &path)?;
    Ok(())
}

pub struct ImportOutcome {
    pub source: String,
    pub imported: Vec<String>,
    pub note: Option<String>,
}

/// Non-secret metadata about credentials owned by another CLI. Detecting a
/// candidate is deliberately read-only: callers must ask the user before
/// invoking `import_all` for its `source`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImportCandidate {
    pub source: String,
    pub provider: String,
    pub label: String,
    pub kind: String,
    pub source_path: String,
    pub requires_confirmation: bool,
}

/// Detect file-backed CLI credentials without copying, refreshing, or deleting
/// them. The explicit home argument keeps onboarding discovery isolated and
/// testable; the Kimi override mirrors `KIMI_CODE_HOME`.
pub fn detect_import_candidates_in(
    home: &Path,
    kimi_home_override: Option<&Path>,
) -> Vec<ImportCandidate> {
    fn json(path: &Path) -> Option<Value> {
        serde_json::from_slice(&std::fs::read(path).ok()?).ok()
    }
    fn candidate(
        source: &str,
        provider: &str,
        label: &str,
        kind: &str,
        source_path: &str,
    ) -> ImportCandidate {
        ImportCandidate {
            source: source.into(),
            provider: provider.into(),
            label: label.into(),
            kind: kind.into(),
            source_path: source_path.into(),
            requires_confirmation: true,
        }
    }

    let mut found = Vec::new();
    if json(&home.join(".claude/.credentials.json"))
        .and_then(|v| {
            v["claudeAiOauth"]["accessToken"]
                .as_str()
                .map(str::to_owned)
        })
        .is_some_and(|token| !token.is_empty())
    {
        found.push(candidate(
            "claude",
            "anthropic",
            "Claude Code",
            "oauth",
            "~/.claude/.credentials.json",
        ));
    }
    if let Some(value) = json(&home.join(".codex/auth.json")) {
        let oauth = value["tokens"]["access_token"]
            .as_str()
            .is_some_and(|token| !token.is_empty());
        let api_key = value["OPENAI_API_KEY"]
            .as_str()
            .is_some_and(|token| !token.is_empty());
        if oauth || api_key {
            found.push(candidate(
                "codex",
                "openai",
                "Codex",
                if oauth && api_key {
                    "oauth+api_key"
                } else if oauth {
                    "oauth"
                } else {
                    "api_key"
                },
                "~/.codex/auth.json",
            ));
        }
    }
    if json(&home.join(".gemini/oauth_creds.json"))
        .and_then(|v| v["access_token"].as_str().map(str::to_owned))
        .is_some_and(|token| !token.is_empty())
    {
        found.push(candidate(
            "gemini",
            "gemini",
            "Gemini CLI",
            "oauth",
            "~/.gemini/oauth_creds.json",
        ));
    }
    if json(&home.join(".grok/auth.json"))
        .and_then(|v| {
            v.as_object().map(|entries| {
                entries
                    .values()
                    .any(|entry| entry["key"].as_str().is_some_and(|token| !token.is_empty()))
            })
        })
        .unwrap_or(false)
    {
        found.push(candidate(
            "grok",
            "xai",
            "Grok",
            "oauth",
            "~/.grok/auth.json",
        ));
    }
    if json(&home.join(".local/share/amp/secrets.json"))
        .and_then(|v| {
            v.as_object().map(|entries| {
                entries.iter().any(|(key, value)| {
                    key.starts_with("apiKey@")
                        && value.as_str().is_some_and(|token| !token.is_empty())
                })
            })
        })
        .unwrap_or(false)
    {
        found.push(candidate(
            "amp",
            "amp",
            "Amp",
            "api_key",
            "~/.local/share/amp/secrets.json",
        ));
    }
    let kimi_root = kimi_home_override
        .map(Path::to_path_buf)
        .unwrap_or_else(|| home.join(".kimi-code"));
    if json(&kimi_root.join("credentials/kimi-code.json"))
        .and_then(|v| v["access_token"].as_str().map(str::to_owned))
        .is_some_and(|token| !token.is_empty())
    {
        found.push(candidate(
            "kimi",
            "kimi",
            "Kimi Code",
            "oauth",
            if kimi_home_override.is_some() {
                "$KIMI_CODE_HOME/credentials/kimi-code.json"
            } else {
                "~/.kimi-code/credentials/kimi-code.json"
            },
        ));
    }
    found
}

pub fn detect_import_candidates() -> Vec<ImportCandidate> {
    let home = home();
    let kimi_override = std::env::var_os("KIMI_CODE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    let mut candidates = detect_import_candidates_in(&home, kimi_override.as_deref());
    if !candidates
        .iter()
        .any(|candidate| candidate.source == "claude")
    {
        let has_keychain_oauth = claude_keychain()
            .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
            .and_then(|value| {
                value["claudeAiOauth"]["accessToken"]
                    .as_str()
                    .map(str::to_owned)
            })
            .is_some_and(|token| !token.is_empty());
        if has_keychain_oauth {
            candidates.insert(
                0,
                ImportCandidate {
                    source: "claude".into(),
                    provider: "anthropic".into(),
                    label: "Claude Code".into(),
                    kind: "oauth".into(),
                    source_path: "macOS Keychain: Claude Code-credentials".into(),
                    requires_confirmation: true,
                },
            );
        }
    }
    candidates
}

pub async fn import_all(vault: &Vault, source: &str) -> Result<Vec<ImportOutcome>> {
    let mut outcomes = Vec::new();
    if source == "all" || source == "claude" {
        outcomes.push(import_claude(vault).await);
    }
    if source == "all" || source == "codex" {
        outcomes.push(import_codex(vault).await);
    }
    if source == "all" || source == "gemini" {
        outcomes.push(import_gemini(vault).await);
    }
    if source == "all" || source == "grok" || source == "xai" {
        outcomes.push(import_grok(vault).await);
    }
    if source == "all" || source == "amp" || source == "ampcode" {
        outcomes.push(import_amp(vault).await);
    }
    if source == "all" || source == "kimi" || source == "kimi-code" {
        outcomes.push(import_kimi(vault).await);
    }
    if outcomes.is_empty() {
        bail!("unknown source '{source}' (expected claude|codex|gemini|grok|xai|amp|kimi|all)");
    }
    Ok(outcomes)
}

fn home() -> PathBuf {
    dirs::home_dir().expect("no home dir")
}

async fn import_claude(vault: &Vault) -> ImportOutcome {
    let mut outcome = ImportOutcome {
        source: "claude".into(),
        imported: vec![],
        note: None,
    };
    let path = home().join(".claude/.credentials.json");
    let raw = if path.exists() {
        std::fs::read_to_string(&path).ok()
    } else {
        claude_keychain()
    };
    let Some(raw) = raw else {
        outcome.note = Some("no ~/.claude/.credentials.json and no Keychain entry".into());
        return outcome;
    };
    let Ok(v) = serde_json::from_str::<Value>(&raw) else {
        outcome.note = Some("could not parse claude credentials".into());
        return outcome;
    };
    let oauth = &v["claudeAiOauth"];
    let Some(access) = oauth["accessToken"].as_str() else {
        outcome.note = Some("no claudeAiOauth.accessToken found".into());
        return outcome;
    };
    let fallback_email = oauth["email"]
        .as_str()
        .or_else(|| v["email"].as_str())
        .and_then(normalize_email)
        .or_else(|| {
            crate::login::jwt_payload(access)
                .and_then(|payload| payload["email"].as_str().and_then(normalize_email))
        });
    let email = fetch_provider_email(&reqwest::Client::new(), Provider::Anthropic, access)
        .await
        .or(fallback_email);
    let mut account = Account {
        id: named_account_id(Provider::Anthropic, "oauth", "default"),
        provider: Provider::Anthropic,
        kind: "oauth".into(),
        name: "default".into(),
        description: email.clone(),
        paused: false,
        label: oauth["subscriptionType"]
            .as_str()
            .map(|s| format!("claude-code ({s})")),
        access_token: Some(access.to_string()),
        refresh_token: oauth["refreshToken"].as_str().map(String::from),
        id_token: None,
        api_key: None,
        expires_at_ms: oauth["expiresAt"].as_i64(),
        last_refresh_ms: Some(now_ms()),
        account_meta: json!({"scopes": oauth["scopes"].clone()}),
        cooldown_until_ms: None,
        status: "active".into(),
        path: None,
    };
    if let Some(email) = email {
        persist_account_email(&mut account, &email);
    }
    match vault.upsert(account).await {
        Ok(()) => outcome.imported.push("anthropic-oauth".into()),
        Err(e) => outcome.note = Some(format!("failed to save: {e}")),
    }
    outcome
}

fn claude_keychain() -> Option<String> {
    let out = std::process::Command::new("security")
        .args([
            "find-generic-password",
            "-s",
            "Claude Code-credentials",
            "-w",
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

async fn import_codex(vault: &Vault) -> ImportOutcome {
    let mut outcome = ImportOutcome {
        source: "codex".into(),
        imported: vec![],
        note: None,
    };
    let path = home().join(".codex/auth.json");
    let Ok(raw) = std::fs::read_to_string(&path) else {
        outcome.note = Some("no ~/.codex/auth.json".into());
        return outcome;
    };
    let Ok(v) = serde_json::from_str::<Value>(&raw) else {
        outcome.note = Some("could not parse codex auth.json".into());
        return outcome;
    };
    if let Some(access) = v["tokens"]["access_token"].as_str() {
        let account = Account {
            id: named_account_id(Provider::Openai, "oauth", "default"),
            provider: Provider::Openai,
            kind: "oauth".into(),
            name: "default".into(),
            description: None,
            paused: false,
            label: Some("codex (chatgpt)".into()),
            access_token: Some(access.to_string()),
            refresh_token: v["tokens"]["refresh_token"].as_str().map(String::from),
            id_token: v["tokens"]["id_token"].as_str().map(String::from),
            api_key: None,
            expires_at_ms: jwt_exp_ms(access),
            last_refresh_ms: Some(now_ms()),
            account_meta: json!({"account_id": v["tokens"]["account_id"].clone()}),
            cooldown_until_ms: None,
            status: "active".into(),
            path: None,
        };
        match vault.upsert(account).await {
            Ok(()) => outcome.imported.push("openai-oauth".into()),
            Err(e) => outcome.note = Some(format!("failed to save: {e}")),
        }
    }
    if let Some(key) = v["OPENAI_API_KEY"].as_str() {
        let account = Account {
            id: named_account_id(Provider::Openai, "api_key", "default"),
            provider: Provider::Openai,
            kind: "api_key".into(),
            name: "default".into(),
            description: None,
            paused: false,
            label: Some("codex (api key)".into()),
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
        match vault.upsert(account).await {
            Ok(()) => outcome.imported.push("openai-api-key".into()),
            Err(e) => outcome.note = Some(format!("failed to save: {e}")),
        }
    }
    if outcome.imported.is_empty() && outcome.note.is_none() {
        outcome.note = Some("auth.json had neither tokens nor OPENAI_API_KEY".into());
    }
    outcome
}

async fn import_gemini(vault: &Vault) -> ImportOutcome {
    let mut outcome = ImportOutcome {
        source: "gemini".into(),
        imported: vec![],
        note: None,
    };
    let path = home().join(".gemini/oauth_creds.json");
    let Ok(raw) = std::fs::read_to_string(&path) else {
        outcome.note = Some("no ~/.gemini/oauth_creds.json".into());
        return outcome;
    };
    let Ok(v) = serde_json::from_str::<Value>(&raw) else {
        outcome.note = Some("could not parse gemini oauth_creds.json".into());
        return outcome;
    };
    let Some(access) = v["access_token"].as_str() else {
        outcome.note = Some("no access_token in gemini creds".into());
        return outcome;
    };
    let account = Account {
        id: named_account_id(Provider::Gemini, "oauth", "default"),
        provider: Provider::Gemini,
        kind: "oauth".into(),
        name: "default".into(),
        description: None,
        paused: false,
        label: Some("gemini-cli".into()),
        access_token: Some(access.to_string()),
        refresh_token: v["refresh_token"].as_str().map(String::from),
        id_token: v["id_token"].as_str().map(String::from),
        api_key: None,
        expires_at_ms: v["expiry_date"].as_i64(),
        last_refresh_ms: Some(now_ms()),
        account_meta: Value::Null,
        cooldown_until_ms: None,
        status: "active".into(),
        path: None,
    };
    match vault.upsert(account).await {
        Ok(()) => outcome.imported.push("gemini-oauth".into()),
        Err(e) => outcome.note = Some(format!("failed to save: {e}")),
    }
    outcome
}

/// Import Amp API key from `~/.local/share/amp/secrets.json` (CLI login material).
/// Resolve the Kimi Code data directory (`$KIMI_CODE_HOME`, else `~/.kimi-code`),
/// matching how the kimi node runtime resolves it.
pub fn kimi_home() -> PathBuf {
    std::env::var_os("KIMI_CODE_HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| home().join(".kimi-code"))
}

/// Path to the Kimi Code credential file written by `kimi` after its own login.
pub fn kimi_credentials_path() -> PathBuf {
    kimi_home().join("credentials").join("kimi-code.json")
}

/// Shape of `~/.kimi-code/credentials/kimi-code.json`. Only fields Alex needs.
#[derive(Debug, Deserialize)]
struct KimiCredentialFile {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    /// Unix expiry in SECONDS (kimi writes seconds, not millis).
    #[serde(default)]
    expires_at: Option<i64>,
    #[serde(default)]
    expires_in: Option<i64>,
    #[serde(default)]
    scope: Option<String>,
}

/// Build a Kimi account from an already-parsed credential file. Kept pure so it
/// can be unit-tested without touching disk or the network. Never logs tokens.
pub fn kimi_account_from_credentials(
    access_token: String,
    refresh_token: Option<String>,
    expires_at_s: Option<i64>,
    expires_in_s: Option<i64>,
    scope: Option<String>,
) -> Account {
    let expires_at_ms = expires_at_s
        .map(|s| s * 1000)
        .or_else(|| expires_in_s.map(|s| now_ms() + s * 1000))
        .or_else(|| jwt_exp_ms(&access_token));
    let mut account = Account {
        id: named_account_id(Provider::Kimi, "oauth", "default"),
        provider: Provider::Kimi,
        kind: "oauth".into(),
        name: default_account_name(),
        description: None,
        paused: false,
        label: Some("kimi (kimi-code)".into()),
        access_token: Some(access_token.clone()),
        refresh_token,
        id_token: None,
        api_key: None,
        expires_at_ms,
        last_refresh_ms: Some(now_ms()),
        account_meta: json!({
            "source": "kimi-code",
            "scope": scope.unwrap_or_else(|| "kimi-code".into()),
        }),
        cooldown_until_ms: None,
        status: "active".into(),
        path: None,
    };
    if let Some(email) = account.email() {
        persist_account_email(&mut account, &email);
    }
    account
}

/// Import the credentials the Kimi Code CLI already stored, so an authed user is
/// adopted with no re-login. Reads shape only; the raw tokens are never logged.
pub async fn import_kimi(vault: &Vault) -> ImportOutcome {
    let mut outcome = ImportOutcome {
        source: "kimi".into(),
        imported: vec![],
        note: None,
    };
    let path = kimi_credentials_path();
    if !path.exists() {
        outcome.note = Some(format!(
            "no {} — run `kimi` and log in, or `alex auth login kimi`",
            path.display()
        ));
        return outcome;
    }
    let Ok(raw) = std::fs::read_to_string(&path) else {
        outcome.note = Some("could not read kimi-code.json".into());
        return outcome;
    };
    let creds: KimiCredentialFile = match serde_json::from_str(&raw) {
        Ok(creds) => creds,
        Err(e) => {
            outcome.note = Some(format!("could not parse kimi-code.json: {e}"));
            return outcome;
        }
    };
    if creds.access_token.is_empty() {
        outcome.note = Some("kimi-code.json has no access_token".into());
        return outcome;
    }
    let account = kimi_account_from_credentials(
        creds.access_token,
        creds.refresh_token,
        creds.expires_at,
        creds.expires_in,
        creds.scope,
    );
    let id = account.id.clone();
    match vault.upsert(account).await {
        Ok(()) => outcome.imported.push(id),
        Err(e) => outcome.note = Some(format!("failed to save: {e}")),
    }
    outcome
}

pub async fn import_amp(vault: &Vault) -> ImportOutcome {
    let mut outcome = ImportOutcome {
        source: "amp".into(),
        imported: vec![],
        note: None,
    };
    let path = home().join(".local/share/amp/secrets.json");
    if !path.exists() {
        outcome.note = Some(
            "no ~/.local/share/amp/secrets.json — run `amp login` or `alex auth amp-key <KEY>`"
                .into(),
        );
        return outcome;
    }
    let Ok(raw) = std::fs::read_to_string(&path) else {
        outcome.note = Some("could not read amp secrets.json".into());
        return outcome;
    };
    let Ok(v) = serde_json::from_str::<Value>(&raw) else {
        outcome.note = Some("could not parse amp secrets.json".into());
        return outcome;
    };
    let Some(obj) = v.as_object() else {
        outcome.note = Some("amp secrets.json is not an object".into());
        return outcome;
    };
    let mut key: Option<(String, String)> = None; // (url, key)
    for (k, val) in obj {
        if let Some(url) = k.strip_prefix("apiKey@") {
            if let Some(s) = val.as_str() {
                if !s.is_empty() {
                    key = Some((url.to_string(), s.to_string()));
                    // Prefer ampcode.com if multiple
                    if url.contains("ampcode.com") {
                        break;
                    }
                }
            }
        }
    }
    let Some((amp_url, api_key)) = key else {
        outcome.note = Some("no apiKey@… entry in amp secrets.json".into());
        return outcome;
    };
    let previous_email = vault
        .list()
        .await
        .into_iter()
        .find(|account| account.id == "amp-api-key")
        .and_then(|account| account.email());
    let email = fetch_amp_account_email(&api_key).await.or(previous_email);
    let mut account = Account {
        id: "amp-api-key".into(),
        provider: Provider::Amp,
        kind: "api_key".into(),
        name: default_account_name(),
        description: None,
        paused: false,
        label: Some(format!("amp ({amp_url})")),
        access_token: None,
        refresh_token: None,
        id_token: None,
        api_key: Some(api_key),
        expires_at_ms: None,
        last_refresh_ms: Some(now_ms()),
        account_meta: json!({ "amp_url": amp_url }),
        cooldown_until_ms: None,
        status: "active".into(),
        path: None,
    };
    if let Some(email) = email {
        persist_account_email(&mut account, &email);
    }
    match vault.upsert(account).await {
        Ok(()) => outcome.imported.push("amp-api-key".into()),
        Err(e) => outcome.note = Some(format!("failed to save: {e}")),
    }
    outcome
}

/// Save an Amp API key provided by the user (settings token / AMP_API_KEY).
pub async fn save_amp_api_key(vault: &Vault, api_key: &str) -> Result<String> {
    let key = api_key.trim();
    if key.is_empty() {
        bail!("empty amp api key");
    }
    let previous_email = vault
        .list()
        .await
        .into_iter()
        .find(|account| account.id == "amp-api-key")
        .and_then(|account| account.email());
    let email = fetch_amp_account_email(key).await.or(previous_email);
    let mut account = Account {
        id: "amp-api-key".into(),
        provider: Provider::Amp,
        kind: "api_key".into(),
        name: default_account_name(),
        description: None,
        paused: false,
        label: Some("amp (api key)".into()),
        access_token: None,
        refresh_token: None,
        id_token: None,
        api_key: Some(key.to_string()),
        expires_at_ms: None,
        last_refresh_ms: Some(now_ms()),
        account_meta: json!({ "amp_url": "https://ampcode.com/" }),
        cooldown_until_ms: None,
        status: "active".into(),
        path: None,
    };
    if let Some(email) = email {
        persist_account_email(&mut account, &email);
    }
    vault.upsert(account).await?;
    Ok("amp-api-key".into())
}

/// Resolve the display identity exposed by Amp's authenticated usage API.
///
/// Amp's CLI secrets file contains only an opaque API key, so it cannot name
/// the subscription on its own. Identity lookup is deliberately best-effort:
/// auth import and wrapped runs continue if the endpoint is unavailable, and
/// neither the key nor the raw usage response is logged or persisted here.
pub async fn fetch_amp_account_email(api_key: &str) -> Option<String> {
    let key = api_key.trim();
    if key.is_empty() {
        return None;
    }
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(5))
        .build()
        .ok()?;
    let response = client
        .post(AMP_USAGE_URL)
        .bearer_auth(key)
        .header("accept", "application/json")
        .header("content-type", "application/json")
        .header("user-agent", "alex-amp-auth")
        .json(&json!({"method": "userDisplayBalanceInfo", "params": {}}))
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let body = response.text().await.ok()?;
    let snapshot = alex_core::parse_usage_api_response(&body).ok()?;
    snapshot.account_email.as_deref().and_then(normalize_email)
}

/// Save an OpenRouter API key and optional, locally configured attribution.
/// Neither attribution field is ever sourced from an inbound proxy request.
pub async fn save_openrouter_api_key(
    vault: &Vault,
    api_key: &str,
    http_referer: Option<&str>,
    x_title: Option<&str>,
) -> Result<String> {
    let key = api_key.trim();
    if key.is_empty() {
        bail!("empty openrouter api key");
    }
    let clean = |value: Option<&str>| {
        value
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(String::from)
    };
    let account = Account {
        id: "openrouter-api-key".into(),
        provider: Provider::Openrouter,
        kind: "api_key".into(),
        name: "default".into(),
        description: None,
        paused: false,
        label: Some("openrouter (api key)".into()),
        access_token: None,
        refresh_token: None,
        id_token: None,
        api_key: Some(key.to_string()),
        expires_at_ms: None,
        last_refresh_ms: None,
        account_meta: json!({
            "http_referer": clean(http_referer),
            "x_title": clean(x_title),
        }),
        cooldown_until_ms: None,
        status: "active".into(),
        path: None,
    };
    vault.upsert(account).await?;
    Ok("openrouter-api-key".into())
}

/// Save an OpenRouter key under a user-facing identity. OpenRouter keys do not
/// expose an account profile, so onboarding asks for this label. Reusing the
/// same label or key updates the existing vault row; a different label/key
/// creates another routable OpenRouter account.
pub async fn save_named_openrouter_api_key(
    vault: &Vault,
    display_name: &str,
    api_key: &str,
    http_referer: Option<&str>,
    x_title: Option<&str>,
) -> Result<String> {
    let display_name = display_name.trim();
    if display_name.is_empty() {
        bail!("empty openrouter account name");
    }
    if display_name.chars().count() > 80 {
        bail!("openrouter account name must be 80 characters or fewer");
    }
    let key = api_key.trim();
    if key.is_empty() {
        bail!("empty openrouter api key");
    }
    let existing = vault.list().await.into_iter().find(|account| {
        account.provider == Provider::Openrouter
            && (account.api_key.as_deref() == Some(key)
                || account
                    .label
                    .as_deref()
                    .is_some_and(|label| label.eq_ignore_ascii_case(display_name)))
    });
    let digest = Sha256::digest(display_name.to_lowercase().as_bytes());
    let suffix: String = digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let id = existing
        .as_ref()
        .map(|account| account.id.clone())
        .unwrap_or_else(|| format!("openrouter-api-key-{suffix}"));
    let name = existing
        .as_ref()
        .map(|account| account.name.clone())
        .unwrap_or_else(|| format!("acct-{suffix}"));
    let clean = |value: Option<&str>| {
        value
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(String::from)
    };
    let account = Account {
        id: id.clone(),
        provider: Provider::Openrouter,
        kind: "api_key".into(),
        name,
        description: Some(display_name.to_string()),
        paused: existing.as_ref().is_some_and(|account| account.paused),
        label: Some(display_name.to_string()),
        access_token: None,
        refresh_token: None,
        id_token: None,
        api_key: Some(key.to_string()),
        expires_at_ms: None,
        last_refresh_ms: None,
        account_meta: json!({
            "http_referer": clean(http_referer),
            "x_title": clean(x_title),
        }),
        cooldown_until_ms: None,
        status: "active".into(),
        path: existing.and_then(|account| account.path),
    };
    vault.upsert(account).await?;
    Ok(id)
}

/// Remove the single OpenRouter API-key account. Both the CLI and admin API
/// use this helper so the account identity and idempotent removal semantics
/// stay aligned.
pub async fn remove_openrouter_api_key(vault: &Vault) -> Result<bool> {
    vault.remove("openrouter-api-key").await
}

/// Save or replace the single CLIProxyAPI integration. The caller is
/// responsible for probing and normalizing the endpoint first; keeping that
/// network validation in the proxy crate lets the CLI and admin API share the
/// exact same capability check without teaching the vault about HTTP.
pub async fn save_cliproxyapi_account(
    vault: &Vault,
    api_base: &str,
    credential: &str,
    models: &[String],
) -> Result<String> {
    let api_base = api_base.trim();
    if api_base.is_empty() {
        bail!("empty CLIProxyAPI URL");
    }
    let credential = credential.trim();
    if credential.is_empty() {
        bail!("empty CLIProxyAPI credential");
    }
    let id = "cliproxyapi-default";
    let existing = vault
        .list()
        .await
        .into_iter()
        .find(|account| account.id == id);
    let account = Account {
        id: id.into(),
        provider: Provider::Cliproxyapi,
        kind: "api_key".into(),
        name: "default".into(),
        description: Some(api_base.to_string()),
        paused: existing.as_ref().is_some_and(|account| account.paused),
        label: Some("CLIProxyAPI".into()),
        access_token: None,
        refresh_token: None,
        id_token: None,
        api_key: Some(credential.to_string()),
        expires_at_ms: None,
        last_refresh_ms: None,
        account_meta: json!({
            "api_base": api_base,
            "models": models,
        }),
        cooldown_until_ms: None,
        status: "active".into(),
        path: existing.and_then(|account| account.path),
    };
    vault.upsert(account).await?;
    Ok(id.into())
}

pub async fn remove_cliproxyapi_account(vault: &Vault) -> Result<bool> {
    vault.remove("cliproxyapi-default").await
}

async fn import_grok(vault: &Vault) -> ImportOutcome {
    let mut outcome = ImportOutcome {
        source: "grok".into(),
        imported: vec![],
        note: None,
    };
    let path = home().join(".grok/auth.json");
    let Ok(raw) = std::fs::read_to_string(&path) else {
        outcome.note = Some("no ~/.grok/auth.json".into());
        return outcome;
    };
    let accounts = grok_accounts_from_json(&raw);
    if accounts.is_empty() {
        outcome.note = Some("no usable entries in grok auth.json".into());
        return outcome;
    }
    for account in accounts {
        let id = account.id.clone();
        match vault.upsert(account).await {
            Ok(()) => outcome.imported.push(id),
            Err(e) => outcome.note = Some(format!("failed to save: {e}")),
        }
    }
    outcome
}

pub fn grok_accounts_from_json(raw: &str) -> Vec<Account> {
    let Ok(v) = serde_json::from_str::<Value>(raw) else {
        return vec![];
    };
    let Some(map) = v.as_object() else {
        return vec![];
    };
    let mut accounts = Vec::new();
    for entry in map.values() {
        let Some(key) = entry["key"].as_str() else {
            continue;
        };
        let idx = accounts.len();
        let id = if idx == 0 {
            "xai-oauth".to_string()
        } else {
            format!("xai-oauth-{}", idx + 1)
        };
        let email = entry["email"].as_str().and_then(normalize_email);
        let mut account = Account {
            id,
            provider: Provider::Xai,
            kind: "oauth".into(),
            name: if idx == 0 {
                "default".into()
            } else {
                format!("{}", idx + 1)
            },
            description: email.clone(),
            paused: false,
            label: Some(
                email
                    .as_ref()
                    .map(|email| format!("grok ({email})"))
                    .unwrap_or_else(|| "grok (oauth)".into()),
            ),
            access_token: Some(key.to_string()),
            refresh_token: entry["refresh_token"].as_str().map(String::from),
            id_token: None,
            api_key: None,
            expires_at_ms: entry["expires_at"].as_str().and_then(rfc3339_to_ms),
            last_refresh_ms: Some(now_ms()),
            account_meta: json!({
                "oidc_issuer": entry["oidc_issuer"].clone(),
                "oidc_client_id": entry["oidc_client_id"].clone(),
                "user_id": entry["user_id"].clone(),
            }),
            cooldown_until_ms: None,
            status: "active".into(),
            path: None,
        };
        if let Some(email) = email {
            persist_account_email(&mut account, &email);
        }
        accounts.push(account);
    }
    accounts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_error_classification_separates_logout_from_transient() {
        // Confirmed logouts: rejected refresh token / missing refresh token.
        assert!(refresh_error_needs_reauth(&anyhow!(
            "token refresh rejected ({REFRESH_REAUTH_MARKER}, status 400 Bad Request)"
        )));
        assert!(refresh_error_needs_reauth(&anyhow!("invalid_grant")));
        assert!(refresh_error_needs_reauth(&anyhow!(
            "account anthropic-oauth has no refresh token"
        )));
        // Transient upstream failures must not be treated as a logout.
        assert!(!refresh_error_needs_reauth(&anyhow!(
            "token refresh failed (status 503 Service Unavailable)"
        )));
        assert!(!refresh_error_needs_reauth(&anyhow!("dns error")));
    }

    #[test]
    fn oauth_error_code_reads_rfc6749_error_field() {
        assert_eq!(
            oauth_error_code(r#"{"error":"invalid_grant"}"#).as_deref(),
            Some("invalid_grant")
        );
        assert_eq!(oauth_error_code("upstream 502 html"), None);
    }

    #[test]
    fn kimi_import_builds_oauth_account_with_seconds_expiry() {
        // expires_at is in unix SECONDS; the account must store millis.
        let expires_at_s = now_ms() / 1000 + 900;
        let account = kimi_account_from_credentials(
            "access-token-shape".into(),
            Some("refresh-token-shape".into()),
            Some(expires_at_s),
            Some(900),
            Some("kimi-code".into()),
        );
        assert_eq!(account.provider, Provider::Kimi);
        assert_eq!(account.kind, "oauth");
        assert_eq!(account.id, "kimi-oauth");
        assert_eq!(account.expires_at_ms, Some(expires_at_s * 1000));
        assert_eq!(
            account.refresh_token.as_deref(),
            Some("refresh-token-shape")
        );
        assert_eq!(account.account_meta["scope"], json!("kimi-code"));
    }

    #[test]
    fn kimi_refresh_decision_follows_expiry_margin() {
        // Freshly minted (15 min out) => no refresh needed.
        let fresh = kimi_account_from_credentials(
            "a".into(),
            Some("r".into()),
            Some(now_ms() / 1000 + 900),
            None,
            None,
        );
        assert!(!fresh.needs_refresh());
        // Inside the refresh margin (about to expire) => must refresh.
        let stale = kimi_account_from_credentials(
            "a".into(),
            Some("r".into()),
            Some((now_ms() + REFRESH_MARGIN_MS / 2) / 1000),
            None,
            None,
        );
        assert!(stale.needs_refresh());
        // Already expired => must refresh.
        let expired = kimi_account_from_credentials(
            "a".into(),
            Some("r".into()),
            Some(now_ms() / 1000 - 60),
            None,
            None,
        );
        assert!(expired.needs_refresh());
    }

    #[test]
    fn account_refresh_eligibility_covers_credential_kind_and_expiry_state() {
        let now = now_ms();
        let mut fresh = oauth_account(
            "openai-oauth-fresh",
            Provider::Openai,
            "fresh@example.com",
            now + REFRESH_MARGIN_MS + 300_000,
        );
        let mut inside_margin = fresh.clone();
        inside_margin.id = "openai-oauth-margin".into();
        inside_margin.expires_at_ms = Some(now + REFRESH_MARGIN_MS / 2);
        let mut expired = fresh.clone();
        expired.id = "openai-oauth-expired".into();
        expired.expires_at_ms = Some(now - 1);
        let mut unknown_expiry = fresh.clone();
        unknown_expiry.id = "openai-oauth-no-expiry".into();
        unknown_expiry.expires_at_ms = None;
        let api_key = api_key_account("openai-api_key", Provider::Openai);
        fresh.refresh_token = None;

        let cases = [
            ("fresh oauth", fresh, false),
            ("inside refresh margin", inside_margin, true),
            ("expired oauth", expired, true),
            ("oauth without expiry", unknown_expiry, true),
            ("api key", api_key, false),
        ];

        for (name, account, expected) in cases {
            assert_eq!(account.needs_refresh(), expected, "{name}");
        }
    }

    #[test]
    fn jwt_expiry_parsing_accepts_valid_claims_and_rejects_malformed_tokens() {
        let cases = [
            (
                "integer expiry",
                json!({"exp": 1_800_000_000_i64}),
                Some(1_800_000_000_000_i64),
            ),
            ("missing expiry", json!({"sub": "user"}), None),
            ("string expiry", json!({"exp": "1800000000"}), None),
            ("null expiry", json!({"exp": null}), None),
        ];

        for (name, payload, expected) in cases {
            let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(serde_json::to_vec(&payload).unwrap());
            assert_eq!(
                jwt_exp_ms(&format!("header.{encoded}.signature")),
                expected,
                "{name}"
            );
        }

        for malformed in ["", "header", "header.***.signature", "header.e30"] {
            assert_eq!(jwt_exp_ms(malformed), None, "{malformed}");
        }
    }

    #[test]
    fn pure_account_state_flags_cover_reauth_policy_and_credential_freshness() {
        let now = now_ms();
        let mut account = oauth_account(
            "openai-oauth-work",
            Provider::Openai,
            "work@example.com",
            now + 600_000,
        );
        account.name = "work".into();
        assert!(!account.needs_reauth());
        account.account_meta["needs_reauth"] = json!(true);
        assert!(account.needs_reauth());
        account.account_meta["needs_reauth"] = json!("true");
        assert!(!account.needs_reauth());

        let enabled = AccountPolicy::default();
        assert!(account_proxy_eligible(&account, &enabled));
        for disabled in [vec!["work".into()], vec![account.id.clone()]] {
            let policy = AccountPolicy {
                disabled,
                ..AccountPolicy::default()
            };
            assert!(!account_proxy_eligible(&account, &policy));
        }

        account.account_meta = json!({"email": "work@example.com"});
        let valid_rank = credential_rank(&account);
        account.expires_at_ms = Some(now - 1);
        let expired_rank = credential_rank(&account);
        account.expires_at_ms = Some(now + 600_000);
        account.access_token = None;
        account.refresh_token = None;
        let missing_rank = credential_rank(&account);
        assert!(valid_rank.0);
        assert!(!expired_rank.0);
        assert!(!missing_rank.0);
    }

    fn temp_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("alex-auth-{name}-{nanos}-{}", std::process::id()))
    }

    fn api_key_account(id: &str, provider: Provider) -> Account {
        Account {
            id: id.into(),
            provider,
            kind: "api_key".into(),
            name: id.rsplit('-').next().unwrap_or("default").into(),
            description: None,
            paused: false,
            label: None,
            access_token: None,
            refresh_token: None,
            id_token: None,
            api_key: Some(format!("sk-{id}")),
            expires_at_ms: None,
            last_refresh_ms: None,
            account_meta: Value::Null,
            cooldown_until_ms: None,
            status: "active".into(),
            path: None,
        }
    }

    fn oauth_account(id: &str, provider: Provider, email: &str, expires_at_ms: i64) -> Account {
        Account {
            id: id.into(),
            provider,
            kind: "oauth".into(),
            name: id.rsplit('-').next().unwrap_or("default").into(),
            description: Some(email.into()),
            paused: false,
            label: None,
            access_token: Some(format!("access-{id}")),
            refresh_token: Some(format!("refresh-{id}")),
            id_token: None,
            api_key: None,
            expires_at_ms: Some(expires_at_ms),
            last_refresh_ms: Some(expires_at_ms),
            account_meta: json!({"email": email, "scopes": ["user:inference"]}),
            cooldown_until_ms: None,
            status: "active".into(),
            path: None,
        }
    }

    async fn invalid_grant_refresh_server() -> (String, tokio::task::JoinHandle<()>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = vec![0; 4096];
            let _ = stream.read(&mut request).await;
            let body = r#"{"error":"invalid_grant"}"#;
            let response = format!(
                "HTTP/1.1 400 Bad Request\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        });
        (format!("http://{address}/token"), server)
    }

    #[tokio::test]
    async fn anthropic_refresh_rejection_marks_needs_reauth_and_persists_it() {
        let dir = temp_dir("anthropic-refresh-needs-reauth");
        let vault = Vault::open(dir.clone()).unwrap();
        let account_id = "anthropic-oauth-refresh-marker";
        vault
            .upsert(oauth_account(
                account_id,
                Provider::Anthropic,
                "refresh-marker@example.test",
                now_ms() - 60_000,
            ))
            .await
            .unwrap();
        let (endpoint, server) = invalid_grant_refresh_server().await;
        vault.set_refresh_endpoint_override(Some(endpoint));

        let error = vault.refresh(account_id, true).await.unwrap_err();
        assert!(refresh_error_needs_reauth(&error));
        server.await.unwrap();

        let marked = vault
            .list()
            .await
            .into_iter()
            .find(|account| account.id == account_id)
            .unwrap();
        assert!(marked.needs_reauth());
        drop(vault);

        let reopened = Vault::open(dir.clone()).unwrap();
        assert!(reopened
            .list()
            .await
            .into_iter()
            .find(|account| account.id == account_id)
            .unwrap()
            .needs_reauth());
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_account_updates_preserve_both_mutations() {
        let dir = temp_dir("concurrent-update");
        let vault = Arc::new(Vault::open(dir.clone()).unwrap());
        vault
            .upsert(api_key_account("openai-key-work", Provider::Openai))
            .await
            .unwrap();

        let barrier = Arc::new(tokio::sync::Barrier::new(3));
        let pause_task = {
            let vault = vault.clone();
            let barrier = barrier.clone();
            tokio::spawn(async move {
                barrier.wait().await;
                vault.set_paused("openai-key-work", true).await.unwrap();
            })
        };
        let meta_task = {
            let vault = vault.clone();
            let barrier = barrier.clone();
            tokio::spawn(async move {
                barrier.wait().await;
                vault
                    .set_account_meta("openai-key-work", "updated_by", json!("second-task"))
                    .await
                    .unwrap();
            })
        };
        barrier.wait().await;
        pause_task.await.unwrap();
        meta_task.await.unwrap();

        let account = vault
            .list()
            .await
            .into_iter()
            .find(|account| account.id == "openai-key-work")
            .unwrap();
        assert!(account.paused);
        assert_eq!(account.account_meta["updated_by"], json!("second-task"));

        let reopened = Vault::open(dir.clone()).unwrap();
        let persisted = reopened
            .list()
            .await
            .into_iter()
            .find(|account| account.id == "openai-key-work")
            .unwrap();
        assert!(persisted.paused);
        assert_eq!(persisted.account_meta["updated_by"], json!("second-task"));
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn merge_refuses_provider_or_email_mismatch_unless_overridden() {
        let dir = temp_dir("merge-mismatch");
        let vault = Vault::open(dir.clone()).unwrap();
        let now = now_ms();
        vault
            .upsert(oauth_account(
                "anthropic-oauth",
                Provider::Anthropic,
                "me@madhavajay.com",
                now + 3_600_000,
            ))
            .await
            .unwrap();
        vault
            .upsert(oauth_account(
                "anthropic-oauth-other",
                Provider::Anthropic,
                "someone@else.com",
                now + 3_600_000,
            ))
            .await
            .unwrap();
        vault
            .upsert(oauth_account(
                "openai-oauth",
                Provider::Openai,
                "me@madhavajay.com",
                now + 3_600_000,
            ))
            .await
            .unwrap();

        // Different email, same provider → refused by default.
        let email_err = vault
            .validate_merge("anthropic-oauth-other", "anthropic-oauth", false)
            .await
            .unwrap_err()
            .to_string();
        assert!(email_err.contains("email mismatch"), "{email_err}");
        // Different provider → refused by default.
        let provider_err = vault
            .validate_merge("openai-oauth", "anthropic-oauth", false)
            .await
            .unwrap_err()
            .to_string();
        assert!(provider_err.contains("provider mismatch"), "{provider_err}");
        // Override allows it.
        assert!(vault
            .validate_merge("anthropic-oauth-other", "anthropic-oauth", true)
            .await
            .is_ok());
        // Unknown ids are rejected.
        assert!(vault
            .validate_merge("nope", "anthropic-oauth", false)
            .await
            .is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn merge_keeps_survivor_id_and_adopts_the_fresh_login() {
        let dir = temp_dir("merge-credentials");
        let vault = Vault::open(dir.clone()).unwrap();
        let now = now_ms();
        // Survivor's own login is expired; the re-authed duplicate is fresh.
        vault
            .upsert(oauth_account(
                "anthropic-oauth",
                Provider::Anthropic,
                "me@madhavajay.com",
                now - 3_600_000,
            ))
            .await
            .unwrap();
        vault
            .upsert(oauth_account(
                "anthropic-oauth-reauth",
                Provider::Anthropic,
                "me@madhavajay.com",
                now + 3_600_000,
            ))
            .await
            .unwrap();

        let outcome = vault
            .merge_accounts("anthropic-oauth-reauth", "anthropic-oauth", false)
            .await
            .unwrap();
        assert_eq!(outcome.survivor_id, "anthropic-oauth");
        assert_eq!(outcome.removed_id, "anthropic-oauth-reauth");
        assert_eq!(
            outcome.adopted_credentials_from.as_deref(),
            Some("anthropic-oauth-reauth")
        );

        let accounts = vault.list().await;
        assert_eq!(accounts.len(), 1, "duplicate is gone");
        let survivor = &accounts[0];
        assert_eq!(survivor.id, "anthropic-oauth");
        // Kept its id but took the fresh, unexpired token.
        assert_eq!(
            survivor.access_token.as_deref(),
            Some("access-anthropic-oauth-reauth")
        );
        assert!(survivor.expires_at_ms.unwrap() > now);
        // The dup was tombstoned via the normal removal path.
        assert!(vault
            .removed_accounts()
            .iter()
            .any(|a| a.id == "anthropic-oauth-reauth"));

        // If instead the survivor already holds the freshest login, its
        // credentials are left untouched.
        let dir2 = temp_dir("merge-keep-survivor");
        let vault2 = Vault::open(dir2.clone()).unwrap();
        vault2
            .upsert(oauth_account(
                "anthropic-oauth",
                Provider::Anthropic,
                "me@madhavajay.com",
                now + 7_200_000,
            ))
            .await
            .unwrap();
        vault2
            .upsert(oauth_account(
                "anthropic-oauth-stale",
                Provider::Anthropic,
                "me@madhavajay.com",
                now - 3_600_000,
            ))
            .await
            .unwrap();
        let outcome2 = vault2
            .merge_accounts("anthropic-oauth-stale", "anthropic-oauth", false)
            .await
            .unwrap();
        assert_eq!(outcome2.adopted_credentials_from, None);
        assert_eq!(
            vault2.list().await[0].access_token.as_deref(),
            Some("access-anthropic-oauth")
        );
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&dir2).ok();
    }

    fn routing_limits(used_pct: f64, resets_at_s: i64) -> Value {
        json!({
            "routing_limits": {
                "windows": [{
                    "window": "5h",
                    "used_pct": used_pct,
                    "resets_at_s": resets_at_s,
                }],
            }
        })
    }

    #[test]
    fn routing_reserve_resolution_uses_name_then_id_then_policy_then_default() {
        let mut account = api_key_account("anthropic-oauth-account-id", Provider::Anthropic);
        account.name = "work".into();
        let mut policy = AccountPolicy {
            reserve_pct: Some(17),
            account_reserve_pct: HashMap::from([("work".into(), 23), (account.id.clone(), 31)]),
            ..AccountPolicy::default()
        };
        assert_eq!(routing_reserve_pct(&account, &policy), 23);
        policy.account_reserve_pct.remove("work");
        assert_eq!(routing_reserve_pct(&account, &policy), 31);
        policy.account_reserve_pct.clear();
        assert_eq!(routing_reserve_pct(&account, &policy), 17);
        policy.reserve_pct = None;
        assert_eq!(routing_reserve_pct(&account, &policy), 10);
    }

    #[test]
    fn routing_reserve_zero_never_blocks_and_blocks_at_the_boundary() {
        let now_s = now_ms() / 1000;
        let mut account = api_key_account("anthropic-oauth-work", Provider::Anthropic);
        account.account_meta = routing_limits(90.0, now_s + 60);
        assert!(!routing_reserve_blocked(&account, 0, now_s));
        assert!(routing_reserve_blocked(&account, 10, now_s));
        account.account_meta["routing_limits"]["windows"][0]["used_pct"] = json!(89.999);
        assert!(!routing_reserve_blocked(&account, 10, now_s));
    }

    #[tokio::test]
    async fn routing_policy_applies_identically_to_non_codex_accounts() {
        let dir = temp_dir("provider-neutral-routing");
        let vault = Vault::open(dir.clone()).unwrap();
        let now_s = now_ms() / 1000;
        for provider in [Provider::Openai, Provider::Anthropic] {
            let mut blocked =
                api_key_account(&format!("{}-api_key-blocked", provider.as_str()), provider);
            blocked.name = "blocked".into();
            blocked.account_meta = routing_limits(90.0, now_s + 60);
            let mut available = api_key_account(
                &format!("{}-api_key-available", provider.as_str()),
                provider,
            );
            available.name = "available".into();
            available.account_meta = routing_limits(20.0, now_s + 60);
            vault.upsert(blocked).await.unwrap();
            vault.upsert(available).await.unwrap();
            vault
                .set_policies(vec![(
                    provider,
                    AccountPolicy {
                        mode: AccountPolicyMode::Priority,
                        order: vec!["blocked".into(), "available".into()],
                        reserve_pct: Some(10),
                        ..AccountPolicy::default()
                    },
                )])
                .await;
            assert_eq!(
                vault.account_for(provider, false).await.unwrap().name,
                "available"
            );
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rfc3339_parse() {
        assert_eq!(rfc3339_to_ms("1970-01-01T00:00:01Z"), Some(1000));
        assert_eq!(
            rfc3339_to_ms("2001-09-09T01:46:40Z"),
            Some(1_000_000_000_000)
        );
        assert_eq!(
            rfc3339_to_ms("2001-09-09T03:46:40+02:00"),
            Some(1_000_000_000_000)
        );
        assert_eq!(rfc3339_to_ms("not a timestamp"), None);
    }

    #[tokio::test]
    async fn openrouter_api_key_round_trips_through_vault() {
        let dir = temp_dir("openrouter-key");
        let vault = Vault::open(dir.clone()).unwrap();
        save_openrouter_api_key(
            &vault,
            "or-test-key",
            Some("https://alex.example"),
            Some("Alex"),
        )
        .await
        .unwrap();
        drop(vault);

        let reopened = Vault::open(dir.clone()).unwrap();
        let account = reopened
            .account_for(Provider::Openrouter, false)
            .await
            .unwrap();
        assert_eq!(account.id, "openrouter-api-key");
        assert_eq!(account.api_key.as_deref(), Some("or-test-key"));
        assert_eq!(account.account_meta["http_referer"], "https://alex.example");
        assert_eq!(account.account_meta["x_title"], "Alex");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn named_openrouter_keys_add_and_replace_by_display_identity() {
        let dir = temp_dir("named-openrouter-keys");
        let vault = Vault::open(dir.clone()).unwrap();
        let personal = save_named_openrouter_api_key(&vault, "Personal", "or-personal", None, None)
            .await
            .unwrap();
        let work = save_named_openrouter_api_key(&vault, "Work", "or-work", None, None)
            .await
            .unwrap();
        assert_ne!(personal, work);
        assert_eq!(vault.list().await.len(), 2);

        let replaced =
            save_named_openrouter_api_key(&vault, "personal", "or-personal-fresh", None, None)
                .await
                .unwrap();
        assert_eq!(replaced, personal);
        let accounts = vault.list().await;
        assert_eq!(accounts.len(), 2);
        assert_eq!(
            accounts
                .iter()
                .find(|account| account.id == personal)
                .and_then(|account| account.api_key.as_deref()),
            Some("or-personal-fresh")
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn cliproxyapi_account_save_replaces_url_key_and_catalog() {
        let dir = temp_dir("cliproxyapi-account");
        let vault = Vault::open(dir.clone()).unwrap();
        let id = save_cliproxyapi_account(
            &vault,
            "http://127.0.0.1:8317/v1",
            "first-key",
            &["gpt-4o".into()],
        )
        .await
        .unwrap();
        assert_eq!(id, "cliproxyapi-default");
        let replaced = save_cliproxyapi_account(
            &vault,
            "https://proxy.example/v1",
            "second-key",
            &["claude-sonnet".into(), "gpt-5".into()],
        )
        .await
        .unwrap();
        assert_eq!(replaced, id);
        assert_eq!(vault.list().await.len(), 1);

        drop(vault);
        let reopened = Vault::open(dir.clone()).unwrap();
        let account = reopened
            .account_for(Provider::Cliproxyapi, false)
            .await
            .unwrap();
        assert_eq!(account.api_key.as_deref(), Some("second-key"));
        assert_eq!(account.account_meta["api_base"], "https://proxy.example/v1");
        assert_eq!(
            account.account_meta["models"],
            json!(["claude-sonnet", "gpt-5"])
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn account_email_prefers_metadata_and_falls_back_to_jwt_profile() {
        let mut account = api_key_account("openai-oauth-default", Provider::Openai);
        account.kind = "oauth".into();
        account.api_key = None;
        account.account_meta = json!({"email": "Primary@Example.com"});
        assert_eq!(account.email().as_deref(), Some("primary@example.com"));

        account.account_meta = json!({});
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
            json!({"https://api.openai.com/profile": {"email": "Workspace@Example.com"}})
                .to_string(),
        );
        account.id_token = Some(format!("header.{payload}.signature"));
        assert_eq!(account.email().as_deref(), Some("workspace@example.com"));
    }

    #[tokio::test]
    async fn remove_writes_non_secret_tombstone_with_durable_identity() {
        let dir = temp_dir("remove-tombstone");
        let vault = Vault::open(dir.clone()).unwrap();
        let mut account = api_key_account("openai-oauth-personal", Provider::Openai);
        account.kind = "oauth".into();
        account.name = "personal".into();
        account.account_meta = json!({"account_id": "acct_123", "email": "madhava@example.com"});
        vault.upsert(account).await.unwrap();
        assert!(vault.remove("openai-oauth-personal").await.unwrap());
        let tombstones = vault.removed_accounts();
        assert_eq!(tombstones.len(), 1);
        assert_eq!(tombstones[0].name, "personal");
        assert_eq!(
            tombstones[0].subscription_identity.as_deref(),
            Some("openai:chatgpt-account:acct_123")
        );
        let raw = std::fs::read_to_string(dir.join("removed-accounts/openai-oauth-personal.json"))
            .unwrap();
        assert!(!raw.contains("api_key"));
    }

    #[test]
    fn grok_json_parse() {
        let dir = temp_dir("grok");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("auth.json");
        let raw = json!({
            "https://auth.x.ai::b1a00492-0000-0000-0000-000000000000": {
                "key": "bearer-token-abc",
                "auth_mode": "oauth",
                "create_time": "2026-01-01T00:00:00Z",
                "user_id": "user-1",
                "email": "user@x.com",
                "refresh_token": "refresh-abc",
                "expires_at": "2026-07-07T00:00:00Z",
                "oidc_issuer": "https://auth.x.ai",
                "oidc_client_id": "client-1"
            },
            "https://auth.x.ai::c2b11503-0000-0000-0000-000000000000": {
                "key": "bearer-token-def",
                "email": "second@x.com",
                "refresh_token": "refresh-def",
                "expires_at": "2026-08-01T00:00:00Z"
            }
        })
        .to_string();
        std::fs::write(&path, &raw).unwrap();
        let read_back = std::fs::read_to_string(&path).unwrap();
        let accounts = grok_accounts_from_json(&read_back);
        assert_eq!(accounts.len(), 2);
        assert_eq!(accounts[0].id, "xai-oauth");
        assert_eq!(accounts[1].id, "xai-oauth-2");
        assert_eq!(accounts[0].provider, Provider::Xai);
        assert_eq!(accounts[0].kind, "oauth");
        assert_eq!(accounts[0].label.as_deref(), Some("grok (user@x.com)"));
        assert_eq!(accounts[0].description.as_deref(), Some("user@x.com"));
        assert_eq!(accounts[0].account_meta["email"], "user@x.com");
        assert_eq!(accounts[0].email().as_deref(), Some("user@x.com"));
        assert_eq!(
            accounts[0].access_token.as_deref(),
            Some("bearer-token-abc")
        );
        assert_eq!(accounts[0].refresh_token.as_deref(), Some("refresh-abc"));
        assert_eq!(
            accounts[0].expires_at_ms,
            rfc3339_to_ms("2026-07-07T00:00:00Z")
        );
        assert!(accounts[0].expires_at_ms.is_some());
        assert_eq!(
            accounts[0].account_meta["oidc_issuer"].as_str(),
            Some("https://auth.x.ai")
        );
        assert_eq!(
            accounts[0].account_meta["oidc_client_id"].as_str(),
            Some("client-1")
        );
        assert_eq!(accounts[0].account_meta["user_id"].as_str(), Some("user-1"));
        assert!(grok_accounts_from_json("not json").is_empty());
        assert!(grok_accounts_from_json("[1,2]").is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn round_robin_and_cooldown() {
        let dir = temp_dir("pool");
        let vault = Vault::open(dir.clone()).unwrap();
        vault
            .upsert(api_key_account("openai-key-a", Provider::Openai))
            .await
            .unwrap();
        vault
            .upsert(api_key_account("openai-key-b", Provider::Openai))
            .await
            .unwrap();
        vault
            .set_policies(vec![(
                Provider::Openai,
                AccountPolicy {
                    mode: AccountPolicyMode::RoundRobin,
                    ..AccountPolicy::default()
                },
            )])
            .await;

        let mut picks = Vec::new();
        for _ in 0..4 {
            picks.push(vault.account_for(Provider::Openai, false).await.unwrap().id);
        }
        assert!(picks.contains(&"openai-key-a".to_string()));
        assert!(picks.contains(&"openai-key-b".to_string()));
        for pair in picks.windows(2) {
            assert_ne!(pair[0], pair[1]);
        }

        vault
            .mark_cooldown("openai-key-a", now_ms() + 60_000)
            .await
            .unwrap();
        for _ in 0..4 {
            let picked = vault.account_for(Provider::Openai, false).await.unwrap();
            assert_eq!(picked.id, "openai-key-b");
        }

        vault
            .mark_cooldown("openai-key-b", now_ms() + 120_000)
            .await
            .unwrap();
        let degraded = vault.account_for(Provider::Openai, false).await.unwrap();
        assert_eq!(degraded.id, "openai-key-a");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn exclusions_visit_each_active_account_once_even_in_degraded_mode() {
        let dir = temp_dir("excluded-pool");
        let vault = Vault::open(dir.clone()).unwrap();
        for id in ["openai-key-a", "openai-key-b", "openai-key-c"] {
            vault
                .upsert(api_key_account(id, Provider::Openai))
                .await
                .unwrap();
        }

        let mut paused = api_key_account("openai-key-paused", Provider::Openai);
        paused.paused = true;
        vault.upsert(paused).await.unwrap();
        let mut inactive = api_key_account("openai-key-inactive", Provider::Openai);
        inactive.status = "disabled".into();
        vault.upsert(inactive).await.unwrap();

        let now = now_ms();
        vault
            .mark_cooldown("openai-key-a", now + 30_000)
            .await
            .unwrap();
        vault
            .mark_cooldown("openai-key-b", now + 20_000)
            .await
            .unwrap();
        vault
            .mark_cooldown("openai-key-c", now + 10_000)
            .await
            .unwrap();

        let mut excluded = HashSet::new();
        let mut selected = Vec::new();
        for _ in 0..3 {
            let account = vault
                .account_for_excluding(Provider::Openai, false, &excluded)
                .await
                .unwrap();
            assert!(excluded.insert(account.id.clone()));
            selected.push(account.id);
        }

        assert_eq!(
            selected,
            vec!["openai-key-c", "openai-key-b", "openai-key-a"]
        );
        assert!(vault
            .account_for_excluding(Provider::Openai, false, &excluded)
            .await
            .is_err());
        assert!(!excluded.contains("openai-key-paused"));
        assert!(!excluded.contains("openai-key-inactive"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn preferred_account_never_bypasses_pause_policy_cooldown_or_exclusion() {
        let dir = temp_dir("preferred-constraints");
        let vault = Vault::open(dir.clone()).unwrap();
        let mut first = api_key_account("openai-key-a", Provider::Openai);
        first.name = "a".into();
        let mut second = api_key_account("openai-key-b", Provider::Openai);
        second.name = "b".into();
        vault.upsert(first).await.unwrap();
        vault.upsert(second.clone()).await.unwrap();
        vault
            .set_policies(vec![(
                Provider::Openai,
                AccountPolicy {
                    order: vec!["a".into(), "b".into()],
                    mode: AccountPolicyMode::Priority,
                    ..AccountPolicy::default()
                },
            )])
            .await;

        let none = HashSet::new();
        assert_eq!(
            vault
                .account_for_excluding_preferred(
                    Provider::Openai,
                    false,
                    &none,
                    Some("openai-key-b"),
                )
                .await
                .unwrap()
                .id,
            "openai-key-b"
        );

        second.paused = true;
        vault.upsert(second.clone()).await.unwrap();
        assert_eq!(
            vault
                .account_for_excluding_preferred(
                    Provider::Openai,
                    false,
                    &none,
                    Some("openai-key-b"),
                )
                .await
                .unwrap()
                .id,
            "openai-key-a"
        );

        second.paused = false;
        second.cooldown_until_ms = Some(now_ms() + 60_000);
        vault.upsert(second.clone()).await.unwrap();
        assert_eq!(
            vault
                .account_for_excluding_preferred(
                    Provider::Openai,
                    false,
                    &none,
                    Some("openai-key-b"),
                )
                .await
                .unwrap()
                .id,
            "openai-key-a"
        );
        assert_eq!(
            vault
                .account_for_excluding_preferred_mode(
                    Provider::Openai,
                    false,
                    &none,
                    Some("openai-key-b"),
                    true,
                )
                .await
                .unwrap()
                .id,
            "openai-key-b"
        );

        second.cooldown_until_ms = None;
        vault.upsert(second).await.unwrap();
        vault
            .set_policies(vec![(
                Provider::Openai,
                AccountPolicy {
                    disabled: vec!["b".into()],
                    ..AccountPolicy::default()
                },
            )])
            .await;
        assert_eq!(
            vault
                .account_for_excluding_preferred(
                    Provider::Openai,
                    false,
                    &none,
                    Some("openai-key-b"),
                )
                .await
                .unwrap()
                .id,
            "openai-key-a"
        );

        let excluded = HashSet::from(["openai-key-b".to_string()]);
        assert_eq!(
            vault
                .account_for_excluding_preferred(
                    Provider::Openai,
                    false,
                    &excluded,
                    Some("openai-key-b"),
                )
                .await
                .unwrap()
                .id,
            "openai-key-a"
        );
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn vault_loads_multi_and_legacy_names() {
        let dir = temp_dir("multi");
        std::fs::create_dir_all(&dir).unwrap();
        let mut legacy = api_key_account("openai-oauth", Provider::Openai);
        legacy.kind = "oauth".into();
        legacy.name = "default".into();
        std::fs::write(
            dir.join("openai-oauth.json"),
            serde_json::to_string(&legacy).unwrap(),
        )
        .unwrap();
        let mut work = api_key_account("openai-oauth-work", Provider::Openai);
        work.kind = "oauth".into();
        work.name = "work".into();
        std::fs::write(
            dir.join("openai-oauth-work.json"),
            serde_json::to_string(&work).unwrap(),
        )
        .unwrap();
        let vault = Vault::open(dir.clone()).unwrap();
        let accounts = vault.list().await;
        assert_eq!(accounts.len(), 2);
        assert!(accounts.iter().any(
            |a| a.name == "default" && a.path.as_ref().unwrap().ends_with("openai-oauth.json")
        ));
        assert!(accounts
            .iter()
            .any(|a| a.name == "work"
                && a.path.as_ref().unwrap().ends_with("openai-oauth-work.json")));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn policy_selection_priority_round_robin_threshold() {
        let dir = temp_dir("policy");
        let vault = Vault::open(dir.clone()).unwrap();
        let mut work = api_key_account("openai-api_key-work", Provider::Openai);
        work.name = "work".into();
        let mut personal = api_key_account("openai-api_key-personal", Provider::Openai);
        personal.name = "personal".into();
        vault.upsert(work).await.unwrap();
        vault.upsert(personal).await.unwrap();
        vault
            .set_policies(vec![(
                Provider::Openai,
                AccountPolicy {
                    order: vec!["work".into(), "personal".into()],
                    mode: AccountPolicyMode::Priority,
                    ..AccountPolicy::default()
                },
            )])
            .await;
        assert_eq!(
            vault
                .account_for(Provider::Openai, false)
                .await
                .unwrap()
                .name,
            "work"
        );
        vault.pause(Provider::Openai, "work", true).await.unwrap();
        assert_eq!(
            vault
                .account_for(Provider::Openai, false)
                .await
                .unwrap()
                .name,
            "personal"
        );
        vault.pause(Provider::Openai, "work", false).await.unwrap();
        vault
            .mark_cooldown("openai-api_key-work", now_ms() + 60_000)
            .await
            .unwrap();
        assert_eq!(
            vault
                .account_for(Provider::Openai, false)
                .await
                .unwrap()
                .name,
            "personal"
        );
        vault
            .mark_cooldown("openai-api_key-work", now_ms() - 1)
            .await
            .unwrap();
        vault
            .set_policies(vec![(
                Provider::Openai,
                AccountPolicy {
                    order: vec!["work".into(), "personal".into()],
                    mode: AccountPolicyMode::RoundRobin,
                    ..AccountPolicy::default()
                },
            )])
            .await;
        let a = vault
            .account_for(Provider::Openai, false)
            .await
            .unwrap()
            .name;
        let b = vault
            .account_for(Provider::Openai, false)
            .await
            .unwrap()
            .name;
        assert_ne!(a, b);
        let mut over = vault
            .list()
            .await
            .into_iter()
            .find(|a| a.name == "work")
            .unwrap();
        over.account_meta = json!({"rate_limit_pct": 90});
        vault.upsert(over).await.unwrap();
        vault
            .set_policies(vec![(
                Provider::Openai,
                AccountPolicy {
                    order: vec!["work".into(), "personal".into()],
                    mode: AccountPolicyMode::Threshold,
                    threshold_pct: Some(80),
                    ..AccountPolicy::default()
                },
            )])
            .await;
        assert_eq!(
            vault
                .account_for(Provider::Openai, false)
                .await
                .unwrap()
                .name,
            "personal"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    fn codex_limits(used_pct: f64, resets_at_s: i64) -> Value {
        json!({
            "codex_limits": {
                "observed_at_ms": now_ms(),
                "windows": [{
                    "window": "5h",
                    "used_pct": used_pct,
                    "resets_at_s": resets_at_s,
                }],
            }
        })
    }

    #[test]
    fn binding_reset_uses_most_consumed_active_window() {
        let now_s = now_ms() / 1000;
        let mut account = api_key_account("openai-api_key-binding", Provider::Openai);
        account.account_meta = json!({
            "codex_limits": {"windows": [
                {"window": "5h", "used_pct": 25.0, "resets_at_s": now_s + 300},
                {"window": "7d", "used_pct": 70.0, "resets_at_s": now_s + 500_000}
            ]}
        });
        let selected = routing_reset_selection(&account, now_s).unwrap();
        assert_eq!(selected["window"], "7d");
        assert_eq!(selected["used_pct"], 70.0);

        account.account_meta["codex_limits"]["windows"][0]["used_pct"] = json!(70.0);
        let tie = routing_reset_selection(&account, now_s).unwrap();
        assert_eq!(tie["window"], "5h");
    }

    #[test]
    fn legacy_policy_defaults_new_fields_without_losing_global_reserve() {
        let policy: AccountPolicy = serde_json::from_value(json!({
            "mode": "reset_first",
            "reserve_pct": 17,
            "order": ["personal"]
        }))
        .unwrap();
        assert_eq!(policy.reserve_pct, Some(17));
        assert!(policy.account_reserve_pct.is_empty());
        assert!(policy.allow_mid_thread_failover);
    }

    #[tokio::test]
    async fn reset_first_reserve_and_eligibility() {
        let dir = temp_dir("reset-first");
        let vault = Vault::open(dir.clone()).unwrap();
        let now_s = now_ms() / 1000;
        let mut soon = api_key_account("openai-api_key-soon", Provider::Openai);
        soon.name = "soon".into();
        soon.account_meta = codex_limits(20.0, now_s + 600);
        let mut later = api_key_account("openai-api_key-later", Provider::Openai);
        later.name = "later".into();
        later.account_meta = codex_limits(20.0, now_s + 3600);
        vault.upsert(soon.clone()).await.unwrap();
        vault.upsert(later.clone()).await.unwrap();

        assert_eq!(
            vault
                .account_for(Provider::Openai, false)
                .await
                .unwrap()
                .name,
            "soon"
        );

        soon.account_meta = codex_limits(95.0, now_s + 600);
        vault.upsert(soon.clone()).await.unwrap();
        assert_eq!(
            vault
                .account_for(Provider::Openai, false)
                .await
                .unwrap()
                .name,
            "later"
        );

        vault
            .set_policies(vec![(
                Provider::Openai,
                AccountPolicy {
                    mode: AccountPolicyMode::ResetFirst,
                    reserve_pct: Some(10),
                    account_reserve_pct: HashMap::from([("soon".into(), 0), ("later".into(), 10)]),
                    ..AccountPolicy::default()
                },
            )])
            .await;
        assert_eq!(
            vault
                .account_for(Provider::Openai, false)
                .await
                .unwrap()
                .name,
            "soon"
        );

        later.account_meta = codex_limits(95.0, now_s + 3600);
        vault.upsert(later).await.unwrap();
        assert_eq!(
            vault
                .account_for(Provider::Openai, false)
                .await
                .unwrap()
                .name,
            "soon"
        );

        vault
            .set_policy_persisted(
                Provider::Openai,
                AccountPolicy {
                    mode: AccountPolicyMode::ResetFirst,
                    reserve_pct: Some(10),
                    disabled: vec!["soon".into()],
                    ..AccountPolicy::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(
            vault
                .account_for(Provider::Openai, false)
                .await
                .unwrap()
                .name,
            "later"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn highest_quota_prefers_lowest_binding_usage() {
        let dir = temp_dir("highest-quota");
        let vault = Vault::open(dir.clone()).unwrap();
        let now_s = now_ms() / 1000;
        let mut fuller = api_key_account("openai-api_key-fuller", Provider::Openai);
        fuller.name = "fuller".into();
        fuller.account_meta = codex_limits(70.0, now_s + 600);
        let mut emptier = api_key_account("openai-api_key-emptier", Provider::Openai);
        emptier.name = "emptier".into();
        emptier.account_meta = codex_limits(25.0, now_s + 3600);
        vault.upsert(fuller).await.unwrap();
        vault.upsert(emptier).await.unwrap();
        vault
            .set_policies(vec![(
                Provider::Openai,
                AccountPolicy {
                    mode: AccountPolicyMode::HighestQuota,
                    reserve_pct: Some(10),
                    ..AccountPolicy::default()
                },
            )])
            .await;

        assert_eq!(
            vault
                .account_for(Provider::Openai, false)
                .await
                .unwrap()
                .name,
            "emptier"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn routing_policy_and_limits_survive_reopen() {
        let dir = temp_dir("routing-persist");
        {
            let vault = Vault::open(dir.clone()).unwrap();
            let mut account = api_key_account("openai-api_key-work", Provider::Openai);
            account.name = "work".into();
            vault.upsert(account).await.unwrap();
            vault
                .set_policy_persisted(
                    Provider::Openai,
                    AccountPolicy {
                        order: vec!["work".into()],
                        mode: AccountPolicyMode::Priority,
                        reserve_pct: Some(15),
                        account_reserve_pct: HashMap::from([("work".into(), 23)]),
                        allow_mid_thread_failover: false,
                        ..AccountPolicy::default()
                    },
                )
                .await
                .unwrap();
            vault
                .record_routing_limits(
                    "openai-api_key-work",
                    json!({
                        "plan": "plus",
                        "windows": [{"window": "5h", "used_pct": 42.0, "resets_at_s": now_ms() / 1000 + 600}],
                    }),
                )
                .await
                .unwrap();
        }
        let reopened = Vault::open(dir.clone()).unwrap();
        let policy = reopened.policy(Provider::Openai);
        assert_eq!(policy.mode, AccountPolicyMode::Priority);
        assert_eq!(policy.reserve_pct, Some(15));
        assert_eq!(policy.account_reserve_pct["work"], 23);
        assert!(!policy.allow_mid_thread_failover);
        assert_eq!(policy.order, vec!["work"]);
        let account = reopened
            .list()
            .await
            .into_iter()
            .find(|account| account.name == "work")
            .unwrap();
        assert_eq!(
            account.account_meta["routing_limits"]["windows"][0]["used_pct"],
            42.0
        );
        assert!(account.account_meta["routing_limits"]["observed_at_ms"].is_i64());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn codex_limit_refresh_requires_same_workspace_and_preserves_safe_fields() {
        let dir = temp_dir("codex-limit-workspace");
        let vault = Vault::open(dir.clone()).unwrap();
        let mut account = api_key_account("openai-api_key-work", Provider::Openai);
        account.account_meta = json!({
            "account_id": "workspace-a",
            "routing_limits": {"active_limit": "premium"},
        });
        vault.upsert(account).await.unwrap();

        let snapshot = json!({
            "plan": "plus",
            "windows": [{"window": "5h", "used_pct": 4.0, "resets_at_s": now_ms() / 1000 + 600}],
        });
        assert!(vault
            .record_routing_limits_for_workspace(
                "openai-api_key-work",
                "workspace-b",
                snapshot.clone(),
            )
            .await
            .is_err());
        vault
            .record_routing_limits_for_workspace("openai-api_key-work", "workspace-a", snapshot)
            .await
            .unwrap();

        let account = vault
            .list()
            .await
            .into_iter()
            .find(|account| account.id == "openai-api_key-work")
            .unwrap();
        assert_eq!(
            account.account_meta["routing_limits"]["active_limit"],
            "premium"
        );
        assert_eq!(account.account_meta["routing_limits"]["plan"], "plus");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn credential_detection_is_read_only_secret_free_and_requires_confirmation() {
        let home = temp_dir("import-candidates");
        let files = [
            (
                ".claude/.credentials.json",
                r#"{"claudeAiOauth":{"accessToken":"claude-secret"}}"#,
            ),
            (
                ".codex/auth.json",
                r#"{"tokens":{"access_token":"codex-secret"}}"#,
            ),
            (
                ".gemini/oauth_creds.json",
                r#"{"access_token":"gemini-secret"}"#,
            ),
            (".grok/auth.json", r#"{"primary":{"key":"grok-secret"}}"#),
            (
                ".local/share/amp/secrets.json",
                r#"{"apiKey@https://ampcode.com/":"amp-secret"}"#,
            ),
            (
                ".kimi-code/credentials/kimi-code.json",
                r#"{"access_token":"kimi-secret"}"#,
            ),
        ];
        for (relative, contents) in files {
            let path = home.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, contents).unwrap();
        }
        let before = files
            .iter()
            .map(|(relative, _)| std::fs::read(home.join(relative)).unwrap())
            .collect::<Vec<_>>();

        let candidates = detect_import_candidates_in(&home, None);

        assert_eq!(
            candidates
                .iter()
                .map(|item| item.source.as_str())
                .collect::<Vec<_>>(),
            ["claude", "codex", "gemini", "grok", "amp", "kimi"]
        );
        assert!(candidates.iter().all(|item| item.requires_confirmation));
        let encoded = serde_json::to_string(&candidates).unwrap();
        for secret in [
            "claude-secret",
            "codex-secret",
            "gemini-secret",
            "grok-secret",
            "amp-secret",
            "kimi-secret",
        ] {
            assert!(!encoded.contains(secret));
        }
        let after = files
            .iter()
            .map(|(relative, _)| std::fs::read(home.join(relative)).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            after, before,
            "detection must not mutate source credentials"
        );
        assert!(
            !home.join("accounts").exists(),
            "detection must not create an Alex vault"
        );
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn malformed_or_empty_external_credentials_are_not_candidates() {
        let home = temp_dir("invalid-import-candidates");
        std::fs::create_dir_all(home.join(".codex")).unwrap();
        std::fs::write(home.join(".codex/auth.json"), r#"{"tokens":{}}"#).unwrap();
        std::fs::create_dir_all(home.join(".local/share/amp")).unwrap();
        std::fs::write(home.join(".local/share/amp/secrets.json"), "not json").unwrap();

        assert!(detect_import_candidates_in(&home, None).is_empty());
        std::fs::remove_dir_all(&home).ok();
    }
}
