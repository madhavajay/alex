use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::login::{
    anthropic_authorize_url, claude_exchange, codex_device_exchange_auto,
    codex_device_exchange_named, codex_device_poll_once, codex_device_start, codex_exchange_named,
    generate_pkce, kimi_device_poll_once_at,
    kimi_device_start_at, kimi_oauth_host, kimi_upsert_from_tokens, kimi_verification_url,
    openai_authorize_url, poll_device_flow, validate_account_name, wait_for_openai_callback,
    xai_device_poll_once, xai_device_start, xai_upsert_from_tokens, DeviceFlowError,
    OPENAI_CALLBACK_ADDR, OPENAI_DEVICE_VERIFICATION_URL, OPENAI_REDIRECT_URI,
};
use crate::{now_ms, Vault};

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
    /// Optional OAuth-host override for the Kimi device flow. `None` (the
    /// default, used in production) resolves the host via
    /// `KIMI_*_OAUTH_HOST`/default at start time; tests set this to point the
    /// flow at a local mock without racing process-wide env vars.
    kimi_oauth_host: Option<String>,
}

fn random_id() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

impl LoginManager {
    /// Test-only: build a manager whose Kimi device flow targets `host`
    /// (a local mock) instead of the real Moonshot OAuth host.
    #[cfg(test)]
    fn with_kimi_oauth_host(host: impl Into<String>) -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            kimi_oauth_host: Some(host.into()),
        }
    }

    pub async fn start(
        &self,
        vault: Arc<Vault>,
        provider: &str,
        account_name: &str,
    ) -> Result<Value> {
        self.prune().await;
        validate_account_name(account_name)?;
        let id = random_id();
        let shared = match provider {
            "claude" | "anthropic" => Arc::new(Mutex::new(self.start_claude(&id, account_name))),
            "codex" | "openai" | "chatgpt" => {
                self.start_codex(&id, vault.clone(), account_name).await?
            }
            "grok" | "xai" => self.start_grok(&id, vault.clone(), account_name).await?,
            "kimi" | "kimi-code" => self.start_kimi(&id, vault.clone(), account_name).await?,
            "gemini" | "google" => self.start_gemini(&id, vault.clone(), account_name).await?,
            "amp" | "ampcode" => {
                // Amp uses CLI secrets / API key, not OAuth paste. Import now.
                let imported = crate::import_amp(&vault).await;
                let session = LoginSession {
                    id: id.clone(),
                    provider: "amp".into(),
                    mode: "import",
                    authorize_url: Some("https://ampcode.com/settings".into()),
                    user_code: None,
                    verification_uri: None,
                    verification_uri_complete: None,
                    created_ms: now_ms(),
                    expires_at_ms: now_ms() + SESSION_TTL_MS,
                    account_name: account_name.to_string(),
                    verifier: None,
                    phase: if let Some(aid) = imported.imported.first() {
                        LoginPhase::Done {
                            account_id: aid.clone(),
                        }
                    } else {
                        LoginPhase::Failed {
                            error: imported.note.unwrap_or_else(|| {
                                "run `amp login` then retry, or `alex auth amp-key <KEY>`".into()
                            }),
                        }
                    },
                };
                Arc::new(Mutex::new(session))
            }
            other => {
                bail!("unknown provider '{other}' (expected claude|codex|grok|gemini|amp|kimi)")
            }
        };
        let snapshot = shared.lock().await.snapshot();
        self.sessions.lock().await.insert(id, shared);
        Ok(snapshot)
    }

    pub async fn start_auto(&self, vault: Arc<Vault>, provider: &str) -> Result<Value> {
        self.prune().await;
        if !matches!(provider, "codex" | "openai" | "chatgpt") {
            bail!("automatic identity login is currently supported only for Codex");
        }
        let id = random_id();
        let shared = self.start_codex_device(&id, vault, None).await?;
        let snapshot = shared.lock().await.snapshot();
        self.sessions.lock().await.insert(id, shared);
        Ok(snapshot)
    }

    /// Start the most hands-off login variant available for a re-auth alert.
    /// Codex uses its polling device flow while the other providers retain
    /// their existing LoginManager flow. Every returned snapshot exposes the
    /// actionable browser URL as `verification_uri_complete`, allowing a
    /// notification caller to treat the session shapes uniformly.
    pub async fn start_reauth(
        &self,
        vault: Arc<Vault>,
        provider: &str,
        account_name: &str,
    ) -> Result<Value> {
        self.prune().await;
        validate_account_name(account_name)?;
        let id = random_id();
        let shared = match provider {
            "codex" | "openai" | "chatgpt" => {
                self.start_codex_device(&id, vault, Some(account_name.to_string()))
                    .await?
            }
            "claude" | "anthropic" => Arc::new(Mutex::new(self.start_claude(&id, account_name))),
            "grok" | "xai" => self.start_grok(&id, vault, account_name).await?,
            "kimi" | "kimi-code" => self.start_kimi(&id, vault, account_name).await?,
            "gemini" | "google" => self.start_gemini(&id, vault, account_name).await?,
            other => bail!("provider '{other}' does not support managed re-authentication"),
        };
        {
            let mut session = shared.lock().await;
            if session.verification_uri_complete.is_none() {
                session.verification_uri_complete = session.authorize_url.clone();
            }
        }
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
        let verifier = session
            .verifier
            .clone()
            .context("session has no verifier")?;
        match claude_exchange(&vault, &verifier, input, &session.account_name).await {
            Ok(account_id) => session.phase = LoginPhase::Done { account_id },
            Err(e) => {
                session.phase = LoginPhase::Failed {
                    error: e.to_string(),
                }
            }
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

    async fn start_codex(
        &self,
        id: &str,
        vault: Arc<Vault>,
        account_name: &str,
    ) -> Result<SharedSession> {
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
            let phase =
                match tokio::time::timeout(deadline, wait_for_openai_callback(&listener, &state))
                    .await
                {
                    Ok(Ok(code)) => {
                        match codex_exchange_named(&vault, &verifier, &code, &account_name).await {
                            Ok(account_id) => LoginPhase::Done { account_id },
                            Err(e) => LoginPhase::Failed {
                                error: e.to_string(),
                            },
                        }
                    }
                    Ok(Err(e)) => LoginPhase::Failed {
                        error: e.to_string(),
                    },
                    Err(_) => LoginPhase::Failed {
                        error: "timed out waiting for the browser callback".into(),
                    },
                };
            worker.lock().await.phase = phase;
        });
        Ok(shared)
    }

    async fn start_codex_device(
        &self,
        id: &str,
        vault: Arc<Vault>,
        account_name: Option<String>,
    ) -> Result<SharedSession> {
        const DEVICE_TTL_MS: i64 = 15 * 60 * 1000;
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()?;
        let start = codex_device_start(&http).await?;
        let session = LoginSession {
            id: id.to_string(),
            provider: "codex".into(),
            mode: "device",
            authorize_url: Some(OPENAI_DEVICE_VERIFICATION_URL.into()),
            user_code: Some(start.user_code.clone()),
            verification_uri: Some(OPENAI_DEVICE_VERIFICATION_URL.into()),
            verification_uri_complete: Some(format!(
                "{OPENAI_DEVICE_VERIFICATION_URL}?user_code={}",
                start.user_code
            )),
            created_ms: now_ms(),
            expires_at_ms: now_ms() + DEVICE_TTL_MS,
            account_name: account_name.clone().unwrap_or_default(),
            verifier: None,
            phase: LoginPhase::Pending,
        };
        let shared = Arc::new(Mutex::new(session));
        let worker = shared.clone();
        let requested_account_name = account_name;
        tokio::spawn(async move {
            let phase = match poll_device_flow(now_ms() + DEVICE_TTL_MS, start.interval_s, || {
                codex_device_poll_once(&http, &start)
            })
            .await
            {
                Ok((authorization_code, code_verifier)) => {
                    let exchanged = if let Some(account_name) = requested_account_name.as_deref() {
                        codex_device_exchange_named(
                            &vault,
                            &authorization_code,
                            &code_verifier,
                            account_name,
                        )
                        .await
                    } else {
                        codex_device_exchange_auto(&vault, &authorization_code, &code_verifier)
                            .await
                    };
                    match exchanged {
                        Ok(account_id) => LoginPhase::Done { account_id },
                        Err(error) => LoginPhase::Failed {
                            error: error.to_string(),
                        },
                    }
                }
                Err(DeviceFlowError::Expired) => LoginPhase::Failed {
                    error: "Codex device code expired before authorization completed".into(),
                },
                Err(DeviceFlowError::Failed(error)) => LoginPhase::Failed { error },
            };
            worker.lock().await.phase = phase;
        });
        Ok(shared)
    }

    async fn start_gemini(
        &self,
        id: &str,
        vault: Arc<Vault>,
        account_name: &str,
    ) -> Result<SharedSession> {
        let (listener, port) = crate::login::bind_loopback().await?;
        let redirect_uri = format!(
            "http://localhost:{port}{}",
            crate::login::GEMINI_CALLBACK_PATH
        );
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
                    match crate::login::gemini_exchange(
                        &vault,
                        &verifier,
                        &redirect_uri,
                        &code,
                        &account_name,
                    )
                    .await
                    {
                        Ok(account_id) => LoginPhase::Done { account_id },
                        Err(e) => LoginPhase::Failed {
                            error: e.to_string(),
                        },
                    }
                }
                Ok(Err(e)) => LoginPhase::Failed {
                    error: e.to_string(),
                },
                Err(_) => LoginPhase::Failed {
                    error: "timed out waiting for the browser callback".into(),
                },
            };
            worker.lock().await.phase = phase;
        });
        Ok(shared)
    }

    async fn start_grok(
        &self,
        id: &str,
        vault: Arc<Vault>,
        account_name: &str,
    ) -> Result<SharedSession> {
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
            verification_uri_complete: Some(
                start
                    .verification_uri_complete
                    .clone()
                    .unwrap_or_else(|| format!("{}?user_code={}", start.verification_uri, start.user_code)),
            ),
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
            let phase = match poll_device_flow(
                now_ms() + start.expires_in * 1000,
                start.interval.max(1) as u64,
                || xai_device_poll_once(&http, &start.device_code),
            )
            .await
            {
                Ok(tokens) => match xai_upsert_from_tokens(&vault, &tokens, &account_name).await {
                    Ok(account_id) => LoginPhase::Done { account_id },
                    Err(error) => LoginPhase::Failed {
                        error: error.to_string(),
                    },
                },
                Err(DeviceFlowError::Expired) => LoginPhase::Failed {
                    error: "device code expired before authorization completed".into(),
                },
                Err(DeviceFlowError::Failed(error)) => LoginPhase::Failed { error },
            };
            worker.lock().await.phase = phase;
        });
        Ok(shared)
    }

    /// Kimi Code (Moonshot AI) RFC 8628 device authorization grant. Mirrors
    /// `start_grok`: kick off the device flow, expose the `user_code` +
    /// `authorize_device` URL in the session snapshot, then poll to completion
    /// in the background and store the account. Never logs token material.
    async fn start_kimi(
        &self,
        id: &str,
        vault: Arc<Vault>,
        account_name: &str,
    ) -> Result<SharedSession> {
        let oauth_host = self.kimi_oauth_host.clone().unwrap_or_else(kimi_oauth_host);
        let http = reqwest::Client::new();
        let start = kimi_device_start_at(&http, &oauth_host).await?;
        let session = LoginSession {
            id: id.to_string(),
            provider: "kimi".into(),
            mode: "device",
            // The verification URL the user opens (server-provided complete URL,
            // else the canonical Kimi one with the user_code appended).
            authorize_url: Some(kimi_verification_url(&start)),
            user_code: Some(start.user_code.clone()),
            verification_uri: start.verification_uri.clone(),
            verification_uri_complete: Some(kimi_verification_url(&start)),
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
            let phase = match poll_device_flow(
                now_ms() + start.expires_in * 1000,
                start.interval.max(1) as u64,
                || kimi_device_poll_once_at(&http, &oauth_host, &start.device_code),
            )
            .await
            {
                Ok(tokens) => match kimi_upsert_from_tokens(&vault, &tokens, &account_name).await {
                    Ok(account_id) => LoginPhase::Done { account_id },
                    Err(error) => LoginPhase::Failed {
                        error: error.to_string(),
                    },
                },
                Err(DeviceFlowError::Expired) => LoginPhase::Failed {
                    error: "device code expired before authorization completed".into(),
                },
                Err(DeviceFlowError::Failed(error)) => LoginPhase::Failed { error },
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
        let bad = mgr
            .complete(vault.clone(), id, "code#wrong-state")
            .await
            .unwrap();
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
    async fn reauth_snapshot_normalizes_actionable_url() {
        let (dir, vault) = temp_vault("reauth-url");
        let mgr = LoginManager::default();
        let snap = mgr
            .start_reauth(vault, "anthropic", "default")
            .await
            .unwrap();
        assert_eq!(snap["state"], "pending");
        assert_eq!(
            snap["verification_uri_complete"],
            snap["authorize_url"],
            "notification callers always receive one actionable URL field"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Minimal RFC 8628 mock: `/api/oauth/device_authorization` hands out a
    /// device+user code, `/api/oauth/token` returns tokens on the first poll.
    /// Returns the `http://127.0.0.1:PORT` base to hand `with_kimi_oauth_host`.
    async fn spawn_kimi_mock() -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                let mut buf = vec![0u8; 8192];
                let n = stream.read(&mut buf).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..n]).into_owned();
                let target = request
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or("");
                let body = if target.contains("device_authorization") {
                    r#"{"device_code":"dev-code-xyz","user_code":"WXYZ-1234","verification_uri":"https://www.kimi.com/code/authorize_device","expires_in":900,"interval":1}"#
                } else if target.contains("token") {
                    r#"{"access_token":"kimi-access-token","refresh_token":"kimi-refresh-token","expires_in":900,"scope":"kimi-code"}"#
                } else {
                    "{}"
                };
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes()).await;
                let _ = stream.shutdown().await;
            }
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn kimi_device_session_starts_and_authorizes() {
        let (dir, vault) = temp_vault("kimi");
        vault
            .upsert(crate::kimi_account_from_credentials(
                "existing-default-token".into(),
                Some("existing-default-refresh".into()),
                None,
                Some(900),
                Some("kimi-code".into()),
            ))
            .await
            .unwrap();
        let default_path = dir.join("kimi-oauth.json");
        let default_before = std::fs::read(&default_path).unwrap();
        let host = spawn_kimi_mock().await;
        let mgr = LoginManager::with_kimi_oauth_host(host);

        // start → a device session exposing the user_code and authorize_device URL.
        let snap = mgr.start(vault.clone(), "kimi", "work").await.unwrap();
        assert_eq!(snap["mode"], "device");
        assert_eq!(snap["state"], "pending");
        assert_eq!(snap["provider"], "kimi");
        assert_eq!(snap["user_code"], "WXYZ-1234");
        let authorize_url = snap["authorize_url"].as_str().unwrap();
        assert!(
            authorize_url.contains("authorize_device"),
            "authorize_url should point at the Kimi device page: {authorize_url}"
        );
        assert!(
            authorize_url.contains("user_code=WXYZ-1234"),
            "authorize_url should carry the user_code: {authorize_url}"
        );
        assert_eq!(snap["verification_uri_complete"], authorize_url);
        // The token must never leak into the session snapshot the UI reads.
        assert!(!snap.to_string().contains("kimi-access-token"));

        // poll → the background worker authorizes and stores the account.
        let id = snap["login_id"].as_str().unwrap().to_string();
        let mut done: Option<Value> = None;
        for _ in 0..50 {
            let status = mgr.status(&id).await.unwrap();
            if status["state"] == "done" {
                done = Some(status);
                break;
            }
            assert_ne!(
                status["state"], "failed",
                "device login unexpectedly failed: {status}"
            );
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
        let done = done.expect("device login did not reach 'done' in time");
        let account_id = done["account_id"].as_str().unwrap();
        assert_eq!(account_id, "kimi-oauth-work");
        assert!(!done.to_string().contains("kimi-access-token"));

        // The named Kimi account is stored without changing or tombstoning the default.
        assert_eq!(std::fs::read(&default_path).unwrap(), default_before);
        assert!(dir.join("kimi-oauth-work.json").exists());
        assert!(!dir.join("removed-accounts").exists());
        let accounts = vault.list().await;
        let account = accounts
            .iter()
            .find(|a| a.id == "kimi-oauth-work")
            .expect("kimi account should be stored");
        assert_eq!(account.provider.as_str(), "kimi");
        assert_eq!(account.access_token.as_deref(), Some("kimi-access-token"));
        let default = accounts
            .iter()
            .find(|a| a.id == "kimi-oauth")
            .expect("default kimi account should remain stored");
        assert_eq!(
            default.access_token.as_deref(),
            Some("existing-default-token")
        );
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
