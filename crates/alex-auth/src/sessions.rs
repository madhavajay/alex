use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::login::{
    anthropic_authorize_url, claude_exchange, codex_exchange_named, generate_pkce,
    openai_authorize_url, wait_for_openai_callback, xai_device_poll_once, xai_device_start,
    xai_upsert_from_tokens, XaiDevicePoll, OPENAI_CALLBACK_ADDR, OPENAI_REDIRECT_URI,
};
use crate::{named_account_id, now_ms, Vault};

const SESSION_TTL_MS: i64 = 30 * 60 * 1000;

#[derive(Debug, Clone, PartialEq)]
pub enum LoginPhase {
    Pending,
    Done { account_id: String },
    Failed { error: String },
}

pub struct LoginSession {
    pub id: String,
    pub provider: String,
    pub mode: &'static str,
    pub authorize_url: Option<String>,
    pub user_code: Option<String>,
    pub verification_uri: Option<String>,
    pub verification_uri_complete: Option<String>,
    pub created_ms: i64,
    pub expires_at_ms: i64,
    account_name: String,
    verifier: Option<String>,
    pub phase: LoginPhase,
}

impl LoginSession {
    pub fn snapshot(&self) -> Value {
        let (state, account_id, error) = match &self.phase {
            LoginPhase::Pending => ("pending", None, None),
            LoginPhase::Done { account_id } => ("done", Some(account_id.clone()), None),
            LoginPhase::Failed { error } => ("failed", None, Some(error.clone())),
        };
        json!({
            "login_id": self.id,
            "provider": self.provider,
            "mode": self.mode,
            "state": state,
            "account_id": account_id,
            "error": error,
            "authorize_url": self.authorize_url,
            "user_code": self.user_code,
            "verification_uri": self.verification_uri,
            "verification_uri_complete": self.verification_uri_complete,
            "expires_at_ms": self.expires_at_ms,
        })
    }
}

type SharedSession = Arc<Mutex<LoginSession>>;

#[derive(Default)]
pub struct LoginManager {
    sessions: Mutex<HashMap<String, SharedSession>>,
}

fn random_id() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

impl LoginManager {
    pub async fn start(&self, vault: Arc<Vault>, provider: &str, account_name: &str) -> Result<Value> {
        self.prune().await;
        validate_account_name(account_name)?;
        let id = random_id();
        let shared = match provider {
            "claude" | "anthropic" => Arc::new(Mutex::new(self.start_claude(&id, account_name))),
            "codex" | "openai" | "chatgpt" => self.start_codex(&id, vault.clone(), account_name).await?,
            "grok" | "xai" => self.start_grok(&id, vault.clone(), account_name).await?,
            "gemini" | "google" => self.start_gemini(&id, vault.clone(), account_name).await?,
            other => bail!("unknown provider '{other}' (expected claude|codex|grok|gemini)"),
        };
        let snapshot = shared.lock().await.snapshot();
        self.sessions.lock().await.insert(id, shared);
        Ok(snapshot)
    }

    pub async fn status(&self, login_id: &str) -> Option<Value> {
        let session = self.sessions.lock().await.get(login_id).cloned()?;
        let snapshot = session.lock().await.snapshot();
        Some(snapshot)
    }

    pub async fn complete(&self, vault: Arc<Vault>, login_id: &str, input: &str) -> Result<Value> {
        let session = self
            .sessions
            .lock()
            .await
            .get(login_id)
            .cloned()
            .context("unknown or expired login session")?;
        let mut session = session.lock().await;
        if session.mode != "paste" {
            bail!(
                "login session '{}' does not take a pasted code (mode: {})",
                login_id,
                session.mode
            );
        }
        if session.phase != LoginPhase::Pending {
            return Ok(session.snapshot());
        }
        let verifier = session.verifier.clone().context("session has no verifier")?;
        match claude_exchange(&vault, &verifier, input).await {
            Ok(account_id) => match rename_login_account(&vault, &account_id, &session.account_name).await {
                Ok(account_id) => session.phase = LoginPhase::Done { account_id },
                Err(e) => session.phase = LoginPhase::Failed { error: e.to_string() },
            },
            Err(e) => session.phase = LoginPhase::Failed { error: e.to_string() },
        }
        Ok(session.snapshot())
    }

    fn start_claude(&self, id: &str, account_name: &str) -> LoginSession {
        let pkce = generate_pkce();
        let url = anthropic_authorize_url(&pkce.challenge, &pkce.verifier);
        LoginSession {
            id: id.to_string(),
            provider: "claude".into(),
            mode: "paste",
            authorize_url: Some(url),
            user_code: None,
            verification_uri: None,
            verification_uri_complete: None,
            created_ms: now_ms(),
            expires_at_ms: now_ms() + SESSION_TTL_MS,
            account_name: account_name.to_string(),
            verifier: Some(pkce.verifier),
            phase: LoginPhase::Pending,
        }
    }

    async fn start_codex(&self, id: &str, vault: Arc<Vault>, account_name: &str) -> Result<SharedSession> {
        let listener = tokio::net::TcpListener::bind(OPENAI_CALLBACK_ADDR)
            .await
            .with_context(|| {
                format!("binding {OPENAI_CALLBACK_ADDR} for the oauth callback (is another login in progress?)")
            })?;
        let pkce = generate_pkce();
        let state = random_id();
        let url = openai_authorize_url(&pkce.challenge, &state);
        let session = LoginSession {
            id: id.to_string(),
            provider: "codex".into(),
            mode: "loopback",
            authorize_url: Some(url),
            user_code: None,
            verification_uri: Some(OPENAI_REDIRECT_URI.into()),
            verification_uri_complete: None,
            created_ms: now_ms(),
            expires_at_ms: now_ms() + SESSION_TTL_MS,
            account_name: account_name.to_string(),
            verifier: None,
            phase: LoginPhase::Pending,
        };
        let shared = Arc::new(Mutex::new(session));
        let worker = shared.clone();
        let verifier = pkce.verifier;
        let account_name = account_name.to_string();
        tokio::spawn(async move {
            let deadline = std::time::Duration::from_millis(SESSION_TTL_MS as u64);
            let phase = match tokio::time::timeout(
                deadline,
                wait_for_openai_callback(&listener, &state),
            )
            .await
            {
                Ok(Ok(code)) => match codex_exchange_named(&vault, &verifier, &code, &account_name).await {
                    Ok(account_id) => LoginPhase::Done { account_id },
                    Err(e) => LoginPhase::Failed { error: e.to_string() },
                },
                Ok(Err(e)) => LoginPhase::Failed { error: e.to_string() },
                Err(_) => LoginPhase::Failed {
                    error: "timed out waiting for the browser callback".into(),
                },
            };
            worker.lock().await.phase = phase;
        });
        Ok(shared)
    }

    async fn start_gemini(&self, id: &str, vault: Arc<Vault>, account_name: &str) -> Result<SharedSession> {
        let (listener, port) = crate::login::bind_loopback().await?;
        let redirect_uri = format!("http://localhost:{port}{}", crate::login::GEMINI_CALLBACK_PATH);
        let pkce = generate_pkce();
        let state = random_id();
        let url = crate::login::gemini_authorize_url(&pkce.challenge, &state, &redirect_uri);
        let session = LoginSession {
            id: id.to_string(),
            provider: "gemini".into(),
            mode: "loopback",
            authorize_url: Some(url),
            user_code: None,
            verification_uri: Some(redirect_uri.clone()),
            verification_uri_complete: None,
            created_ms: now_ms(),
            expires_at_ms: now_ms() + SESSION_TTL_MS,
            account_name: account_name.to_string(),
            verifier: None,
            phase: LoginPhase::Pending,
        };
        let shared = Arc::new(Mutex::new(session));
        let worker = shared.clone();
        let verifier = pkce.verifier;
        let account_name = account_name.to_string();
        tokio::spawn(async move {
            let deadline = std::time::Duration::from_millis(SESSION_TTL_MS as u64);
            let phase = match tokio::time::timeout(
                deadline,
                crate::login::wait_for_loopback_callback(
                    &listener,
                    &state,
                    crate::login::GEMINI_CALLBACK_PATH,
                ),
            )
            .await
            {
                Ok(Ok(code)) => {
                    match crate::login::gemini_exchange(&vault, &verifier, &redirect_uri, &code)
                        .await
                    {
                        Ok(account_id) => match rename_login_account(&vault, &account_id, &account_name).await {
                            Ok(account_id) => LoginPhase::Done { account_id },
                            Err(e) => LoginPhase::Failed { error: e.to_string() },
                        },
                        Err(e) => LoginPhase::Failed { error: e.to_string() },
                    }
                }
                Ok(Err(e)) => LoginPhase::Failed { error: e.to_string() },
                Err(_) => LoginPhase::Failed {
                    error: "timed out waiting for the browser callback".into(),
                },
            };
            worker.lock().await.phase = phase;
        });
        Ok(shared)
    }

    async fn start_grok(&self, id: &str, vault: Arc<Vault>, account_name: &str) -> Result<SharedSession> {
        let http = reqwest::Client::new();
        let start = xai_device_start(&http).await?;
        let session = LoginSession {
            id: id.to_string(),
            provider: "grok".into(),
            mode: "device",
            authorize_url: start
                .verification_uri_complete
                .clone()
                .or_else(|| Some(start.verification_uri.clone())),
            user_code: Some(start.user_code.clone()),
            verification_uri: Some(start.verification_uri.clone()),
            verification_uri_complete: start.verification_uri_complete.clone(),
            created_ms: now_ms(),
            expires_at_ms: now_ms() + start.expires_in * 1000,
            account_name: account_name.to_string(),
            verifier: None,
            phase: LoginPhase::Pending,
        };
        let shared = Arc::new(Mutex::new(session));
        let worker = shared.clone();
        let account_name = account_name.to_string();
        tokio::spawn(async move {
            let deadline = now_ms() + start.expires_in * 1000;
            let mut interval = start.interval.max(1) as u64;
            let phase = loop {
                if now_ms() > deadline {
                    break LoginPhase::Failed {
                        error: "device code expired before authorization completed".into(),
                    };
                }
                tokio::time::sleep(std::time::Duration::from_secs(interval)).await;
                match xai_device_poll_once(&http, &start.device_code).await {
                    XaiDevicePoll::Pending => continue,
                    XaiDevicePoll::SlowDown => {
                        interval += 5;
                        continue;
                    }
                    XaiDevicePoll::Done(tokens) => {
                        break match xai_upsert_from_tokens(&vault, &tokens).await {
                            Ok(account_id) => match rename_login_account(&vault, &account_id, &account_name).await {
                                Ok(account_id) => LoginPhase::Done { account_id },
                                Err(e) => LoginPhase::Failed { error: e.to_string() },
                            },
                            Err(e) => LoginPhase::Failed { error: e.to_string() },
                        };
                    }
                    XaiDevicePoll::Failed(e) => break LoginPhase::Failed { error: e },
                }
            };
            worker.lock().await.phase = phase;
        });
        Ok(shared)
    }

    async fn prune(&self) {
        let now = now_ms();
        self.sessions
            .lock()
            .await
            .retain(|_, s| match s.try_lock() {
                Ok(s) => s.expires_at_ms > now - SESSION_TTL_MS,
                Err(_) => true,
            });
    }
}

fn validate_account_name(name: &str) -> Result<()> {
    if name.is_empty() || name.len() > 32 || !name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-') {
        bail!("account name must match [a-z0-9_-]{{1,32}}");
    }
    Ok(())
}

async fn rename_login_account(vault: &Vault, account_id: &str, account_name: &str) -> Result<String> {
    if account_name == "default" {
        return Ok(account_id.to_string());
    }
    let mut account = vault
        .list()
        .await
        .into_iter()
        .find(|account| account.id == account_id)
        .context("login completed but the saved account could not be found")?;
    if vault.has_account_name(account.provider, account_name).await {
        bail!("{} account '{account_name}' already exists", account.provider.as_str());
    }
    let default_id = named_account_id(account.provider, &account.kind, "default");
    let previous_default = vault
        .list()
        .await
        .into_iter()
        .find(|candidate| candidate.id == default_id && candidate.id != account.id);
    account.name = account_name.to_string();
    account.id = named_account_id(account.provider, &account.kind, account_name);
    account.path = None;
    let renamed_id = account.id.clone();
    vault.upsert(account).await?;
    if let Some(previous_default) = previous_default {
        vault.upsert(previous_default).await?;
    } else {
        let _ = vault.remove(&default_id).await?;
    }
    Ok(renamed_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_vault(name: &str) -> (PathBuf, Arc<Vault>) {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "alexandria-sessions-{name}-{nanos}-{}",
            std::process::id()
        ));
        let vault = Arc::new(Vault::open(dir.clone()).unwrap());
        (dir, vault)
    }

    #[tokio::test]
    async fn claude_session_lifecycle() {
        let (dir, vault) = temp_vault("claude");
        let mgr = LoginManager::default();
        let snap = mgr.start(vault.clone(), "claude", "default").await.unwrap();
        assert_eq!(snap["mode"], "paste");
        assert_eq!(snap["state"], "pending");
        let url = snap["authorize_url"].as_str().unwrap();
        assert!(url.starts_with("https://claude.ai/oauth/authorize"));
        let id = snap["login_id"].as_str().unwrap();
        let status = mgr.status(id).await.unwrap();
        assert_eq!(status["state"], "pending");
        let bad = mgr.complete(vault.clone(), id, "code#wrong-state").await.unwrap();
        assert_eq!(bad["state"], "failed");
        assert!(bad["error"].as_str().unwrap().contains("state mismatch"));
        assert!(mgr.status("nope").await.is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn unknown_provider_rejected() {
        let (dir, vault) = temp_vault("unknown");
        let mgr = LoginManager::default();
        assert!(mgr.start(vault, "hal9000", "default").await.is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn gemini_starts_loopback_oauth() {
        let (dir, vault) = temp_vault("gemini");
        let mgr = LoginManager::default();
        let snap = mgr.start(vault, "gemini", "default").await.unwrap();
        assert_eq!(snap["mode"], "loopback");
        assert_eq!(snap["state"], "pending");
        let url = snap["authorize_url"].as_str().unwrap();
        assert!(url.starts_with("https://accounts.google.com/o/oauth2/v2/auth"));
        assert!(url.contains("code_challenge"));
        assert!(url.contains("access_type=offline"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
