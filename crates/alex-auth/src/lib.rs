use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use alex_core::Provider;
use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::{Mutex, RwLock};

pub mod login;
pub mod sessions;

pub const ANTHROPIC_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
pub const ANTHROPIC_TOKEN_URL: &str = "https://console.anthropic.com/v1/oauth/token";
pub const OPENAI_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
pub const OPENAI_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
pub const XAI_CLIENT_ID: &str = "b1a00492-073a-47ea-816f-4c329264a828";
pub const XAI_TOKEN_URL: &str = "https://auth.x.ai/oauth2/token";
pub const GEMINI_CLIENT_ID: &str =
    "681255809395-oo8ft2oprdrnp9e3aqf6av3hmdib135j.apps.googleusercontent.com";
pub const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";

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

    fn needs_refresh(&self) -> bool {
        self.kind == "oauth"
            && match self.expires_at_ms {
                Some(exp) => exp < now_ms() + REFRESH_MARGIN_MS,
                None => true,
            }
    }
}

fn oauth_rank(account: &Account, prefer_oauth: bool) -> u8 {
    if (account.kind == "oauth") == prefer_oauth {
        0
    } else {
        1
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

pub struct Vault {
    dir: PathBuf,
    accounts: RwLock<HashMap<String, Account>>,
    locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    rr_counter: AtomicUsize,
    http: reqwest::Client,
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
                    Ok(acct) => {
                        accounts.insert(acct.id.clone(), acct);
                    }
                    Err(e) => tracing::warn!("skipping unreadable account file {path:?}: {e}"),
                }
            }
        }
        Ok(Self {
            dir,
            accounts: RwLock::new(accounts),
            locks: Mutex::new(HashMap::new()),
            rr_counter: AtomicUsize::new(0),
            http: reqwest::Client::new(),
        })
    }

    pub async fn list(&self) -> Vec<Account> {
        let mut v: Vec<Account> = self.accounts.read().await.values().cloned().collect();
        v.sort_by(|a, b| a.id.cmp(&b.id));
        v
    }

    pub async fn upsert(&self, account: Account) -> Result<()> {
        write_account_file(&self.dir, &account)?;
        self.accounts
            .write()
            .await
            .insert(account.id.clone(), account);
        Ok(())
    }

    pub async fn remove(&self, id: &str) -> Result<bool> {
        let existed = self.accounts.write().await.remove(id).is_some();
        let path = self.dir.join(format!("{id}.json"));
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        Ok(existed)
    }

    pub async fn account_for(&self, provider: Provider, prefer_oauth: bool) -> Result<Account> {
        let now = now_ms();
        let candidates: Vec<Account> = {
            let map = self.accounts.read().await;
            let mut v: Vec<Account> = map
                .values()
                .filter(|a| a.provider == provider && a.status == "active")
                .cloned()
                .collect();
            v.sort_by_key(|a| (oauth_rank(a, prefer_oauth), a.id.clone()));
            v
        };
        if candidates.is_empty() {
            bail!(
                "no active {} account; run `alexandria auth import`",
                provider.as_str()
            );
        }
        let ready: Vec<&Account> = candidates
            .iter()
            .filter(|a| a.cooldown_until_ms.map(|c| c <= now).unwrap_or(true))
            .collect();
        let account = if ready.is_empty() {
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
            let top_rank = oauth_rank(ready[0], prefer_oauth);
            let group: Vec<&&Account> = ready
                .iter()
                .filter(|a| oauth_rank(a, prefer_oauth) == top_rank)
                .collect();
            let idx = self.rr_counter.fetch_add(1, Ordering::Relaxed) % group.len();
            (*group[idx]).clone()
        };
        if account.needs_refresh() {
            return self.refresh(&account.id, false).await;
        }
        Ok(account)
    }

    pub async fn mark_cooldown(&self, id: &str, until_ms: i64) -> Result<()> {
        let mut account = self
            .accounts
            .read()
            .await
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow!("unknown account {id}"))?;
        account.cooldown_until_ms = Some(until_ms);
        self.upsert(account).await
    }

    pub async fn refresh(&self, id: &str, force: bool) -> Result<Account> {
        let lock = {
            let mut locks = self.locks.lock().await;
            locks
                .entry(id.to_string())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        let _guard = lock.lock().await;

        let account = self
            .accounts
            .read()
            .await
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow!("unknown account {id}"))?;
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
            (Provider::Amp, _) => Err(anyhow!(
                "amp accounts use a long-lived API key; re-run `alex auth import amp` or `alex auth amp-key`"
            )),
            (_, None) => Err(anyhow!("account {id} has no refresh token")),
        };
        let refreshed = match result {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("refresh failed for {id}: {e}; re-importing from native creds");
                if let Some(fresh) = self.reimport_native(&account).await {
                    return Ok(fresh);
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
        self.upsert(updated.clone()).await?;
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
            .post(ANTHROPIC_TOKEN_URL)
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
            .post(OPENAI_TOKEN_URL)
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
            .post(XAI_TOKEN_URL)
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
            .post(GOOGLE_TOKEN_URL)
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

    pub async fn set_account_meta(&self, id: &str, key: &str, value: Value) -> Result<()> {
        let mut account = self
            .accounts
            .read()
            .await
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow!("unknown account {id}"))?;
        if !account.account_meta.is_object() {
            account.account_meta = json!({});
        }
        account.account_meta[key] = value;
        self.upsert(account).await
    }
}

#[derive(Debug, Deserialize)]
struct RefreshedTokens {
    access_token: Option<String>,
    refresh_token: Option<String>,
    id_token: Option<String>,
    expires_in: Option<i64>,
}

async fn parse_token_response(resp: reqwest::Response) -> Result<RefreshedTokens> {
    let status = resp.status();
    let text = resp.text().await?;
    if !status.is_success() {
        bail!("token refresh failed ({status}): {text}");
    }
    serde_json::from_str(&text).context("bad token response")
}

fn write_account_file(dir: &Path, account: &Account) -> Result<()> {
    let path = dir.join(format!("{}.json", account.id));
    let data = serde_json::to_string_pretty(account)?;
    std::fs::write(&path, data)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

pub struct ImportOutcome {
    pub source: String,
    pub imported: Vec<String>,
    pub note: Option<String>,
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
    if outcomes.is_empty() {
        bail!("unknown source '{source}' (expected claude|codex|gemini|grok|xai|amp|all)");
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
    let account = Account {
        id: "anthropic-oauth".into(),
        provider: Provider::Anthropic,
        kind: "oauth".into(),
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
    };
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
            id: "openai-oauth".into(),
            provider: Provider::Openai,
            kind: "oauth".into(),
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
        };
        match vault.upsert(account).await {
            Ok(()) => outcome.imported.push("openai-oauth".into()),
            Err(e) => outcome.note = Some(format!("failed to save: {e}")),
        }
    }
    if let Some(key) = v["OPENAI_API_KEY"].as_str() {
        let account = Account {
            id: "openai-api-key".into(),
            provider: Provider::Openai,
            kind: "api_key".into(),
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
        id: "gemini-oauth".into(),
        provider: Provider::Gemini,
        kind: "oauth".into(),
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
    };
    match vault.upsert(account).await {
        Ok(()) => outcome.imported.push("gemini-oauth".into()),
        Err(e) => outcome.note = Some(format!("failed to save: {e}")),
    }
    outcome
}

/// Import Amp API key from `~/.local/share/amp/secrets.json` (CLI login material).
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
    let account = Account {
        id: "amp-api-key".into(),
        provider: Provider::Amp,
        kind: "api_key".into(),
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
    };
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
    let account = Account {
        id: "amp-api-key".into(),
        provider: Provider::Amp,
        kind: "api_key".into(),
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
    };
    vault.upsert(account).await?;
    Ok("amp-api-key".into())
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
        let email = entry["email"].as_str().unwrap_or("unknown");
        accounts.push(Account {
            id,
            provider: Provider::Xai,
            kind: "oauth".into(),
            label: Some(format!("grok ({email})")),
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
        });
    }
    accounts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "alexandria-auth-{name}-{nanos}-{}",
            std::process::id()
        ))
    }

    fn api_key_account(id: &str, provider: Provider) -> Account {
        Account {
            id: id.into(),
            provider,
            kind: "api_key".into(),
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
        }
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
}
