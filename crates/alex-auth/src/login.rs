use std::collections::HashMap;
use std::io::Write as _;

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use rand::RngCore;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::{
    fetch_provider_email, import_grok, jwt_exp_ms, named_account_id, normalize_email, now_ms,
    persist_account_email, Account, Vault, ANTHROPIC_CLIENT_ID, ANTHROPIC_TOKEN_URL,
    OPENAI_CLIENT_ID, OPENAI_TOKEN_URL, XAI_CLIENT_ID, XAI_TOKEN_URL,
};
use alex_core::Provider;

pub const PROVIDERS: &[&str] = &["claude", "codex", "grok", "gemini"];

pub const ANTHROPIC_AUTHORIZE_URL: &str = "https://claude.ai/oauth/authorize";
pub const ANTHROPIC_REDIRECT_URI: &str = "https://console.anthropic.com/oauth/code/callback";
pub const ANTHROPIC_SCOPES: &str = "org:create_api_key user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload";
pub const OPENAI_AUTHORIZE_URL: &str = "https://auth.openai.com/oauth/authorize";
pub const OPENAI_REDIRECT_URI: &str = "http://localhost:1455/auth/callback";
pub const OPENAI_SCOPES: &str = "openid profile email offline_access";
pub const OPENAI_CALLBACK_PATH: &str = "/auth/callback";
pub(crate) const OPENAI_CALLBACK_ADDR: &str = "127.0.0.1:1455";
const OPENAI_USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
pub const OPENAI_DEVICE_USER_CODE_URL: &str =
    "https://auth.openai.com/api/accounts/deviceauth/usercode";
pub const OPENAI_DEVICE_TOKEN_URL: &str =
    "https://auth.openai.com/api/accounts/deviceauth/token";
pub const OPENAI_DEVICE_VERIFICATION_URL: &str = "https://auth.openai.com/codex/device";
pub const OPENAI_DEVICE_REDIRECT_URI: &str = "https://auth.openai.com/deviceauth/callback";
const OPENAI_JWT_CLAIM: &str = "https://api.openai.com/auth";
pub const XAI_DEVICE_CODE_URL: &str = "https://auth.x.ai/oauth2/device/code";
pub const XAI_SCOPES: &str = "openid profile email offline_access grok-cli:access api:access conversations:read conversations:write";
pub const GEMINI_AUTHORIZE_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
pub const GEMINI_SCOPES: &str = "https://www.googleapis.com/auth/cloud-platform https://www.googleapis.com/auth/userinfo.email https://www.googleapis.com/auth/userinfo.profile openid";
pub const GEMINI_CALLBACK_PATH: &str = "/oauth2callback";

pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

fn base64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

pub fn pkce_challenge(verifier: &str) -> String {
    base64url(&Sha256::digest(verifier.as_bytes()))
}

pub fn generate_pkce() -> Pkce {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    let verifier = base64url(&bytes);
    let challenge = pkce_challenge(&verifier);
    Pkce {
        verifier,
        challenge,
    }
}

fn random_state() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn anthropic_authorize_url(challenge: &str, state: &str) -> String {
    let mut url = reqwest::Url::parse(ANTHROPIC_AUTHORIZE_URL).unwrap();
    url.query_pairs_mut()
        .append_pair("code", "true")
        .append_pair("client_id", ANTHROPIC_CLIENT_ID)
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", ANTHROPIC_REDIRECT_URI)
        .append_pair("scope", ANTHROPIC_SCOPES)
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", state);
    url.to_string()
}

pub fn openai_authorize_url(challenge: &str, state: &str) -> String {
    let mut url = reqwest::Url::parse(OPENAI_AUTHORIZE_URL).unwrap();
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", OPENAI_CLIENT_ID)
        .append_pair("redirect_uri", OPENAI_REDIRECT_URI)
        .append_pair("scope", OPENAI_SCOPES)
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", state)
        .append_pair("id_token_add_organizations", "true")
        .append_pair("codex_cli_simplified_flow", "true")
        .append_pair("originator", "pi");
    url.to_string()
}

pub fn parse_authorization_input(input: &str) -> (Option<String>, Option<String>) {
    let value = input.trim();
    if value.is_empty() {
        return (None, None);
    }
    if let Ok(url) = reqwest::Url::parse(value) {
        let find = |key: &str| {
            url.query_pairs()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.into_owned())
        };
        return (find("code"), find("state"));
    }
    if let Some((code, state)) = value.split_once('#') {
        return (Some(code.to_string()), Some(state.to_string()));
    }
    if value.contains("code=") {
        let mut code = None;
        let mut state = None;
        for pair in value.split('&') {
            match pair.split_once('=') {
                Some(("code", v)) => code = Some(v.to_string()),
                Some(("state", v)) => state = Some(v.to_string()),
                _ => {}
            }
        }
        return (code, state);
    }
    (Some(value.to_string()), None)
}

pub fn jwt_payload(token: &str) -> Option<serde_json::Value> {
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub fn chatgpt_account_id(token: &str) -> Option<String> {
    jwt_payload(token)?
        .get(OPENAI_JWT_CLAIM)?
        .get("chatgpt_account_id")?
        .as_str()
        .map(String::from)
}

fn jwt_email(token: &str) -> Option<String> {
    jwt_payload(token)
        .and_then(|payload| payload.get("email").and_then(Value::as_str).and_then(normalize_email))
}

fn token_email(id_token: Option<&str>, access_token: &str) -> Option<String> {
    id_token.and_then(jwt_email).or_else(|| jwt_email(access_token))
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    id_token: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
    #[serde(default)]
    scope: Option<String>,
}

async fn read_token_response(resp: reqwest::Response) -> Result<TokenResponse> {
    let status = resp.status();
    let text = resp.text().await?;
    if !status.is_success() {
        bail!("token exchange failed ({status}): {text}");
    }
    serde_json::from_str(&text).context("bad token exchange response")
}

pub fn browser_open_command(url: &str) -> Option<(&'static str, Vec<String>)> {
    if cfg!(target_os = "macos") {
        Some(("open", vec![url.to_string()]))
    } else if cfg!(target_os = "windows") {
        Some((
            "cmd",
            vec!["/C".into(), "start".into(), String::new(), url.to_string()],
        ))
    } else if cfg!(target_os = "linux") {
        Some(("xdg-open", vec![url.to_string()]))
    } else {
        None
    }
}

fn open_browser(url: &str) {
    if let Some((program, args)) = browser_open_command(url) {
        let _ = std::process::Command::new(program).args(args).spawn();
    }
}

async fn prompt_line(message: &str) -> Result<String> {
    print!("{message}");
    std::io::stdout().flush()?;
    let line = tokio::task::spawn_blocking(|| {
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).map(|_| line)
    })
    .await??;
    Ok(line.trim().to_string())
}

fn request_target(request: &str) -> Option<&str> {
    request.lines().next()?.split_whitespace().nth(1)
}

fn callback_path(target: &str) -> String {
    reqwest::Url::parse(&format!("http://localhost{target}"))
        .map(|u| u.path().to_string())
        .unwrap_or_default()
}

fn callback_query(target: &str) -> HashMap<String, String> {
    reqwest::Url::parse(&format!("http://localhost{target}"))
        .map(|url| {
            url.query_pairs()
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect()
        })
        .unwrap_or_default()
}

async fn respond(stream: &mut TcpStream, status: &str, body: &str) {
    let resp = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(resp.as_bytes()).await;
    let _ = stream.shutdown().await;
}

pub(crate) async fn wait_for_openai_callback(
    listener: &TcpListener,
    expected_state: &str,
) -> Result<String> {
    wait_for_loopback_callback(listener, expected_state, OPENAI_CALLBACK_PATH).await
}

pub(crate) async fn wait_for_loopback_callback(
    listener: &TcpListener,
    expected_state: &str,
    expected_path: &str,
) -> Result<String> {
    loop {
        let (mut stream, _) = listener.accept().await?;
        let mut buf = vec![0u8; 8192];
        let n = stream.read(&mut buf).await.unwrap_or(0);
        let request = String::from_utf8_lossy(&buf[..n]).into_owned();
        let Some(target) = request_target(&request) else {
            respond(
                &mut stream,
                "400 Bad Request",
                "<html><body>bad request</body></html>",
            )
            .await;
            continue;
        };
        if callback_path(target) != expected_path {
            respond(
                &mut stream,
                "404 Not Found",
                "<html><body>not found</body></html>",
            )
            .await;
            continue;
        }
        let params = callback_query(target);
        if let Some(err) = params.get("error") {
            respond(
                &mut stream,
                "400 Bad Request",
                &format!("<html><body>login failed: {err}</body></html>"),
            )
            .await;
            bail!("oauth provider returned error: {err}");
        }
        if params.get("state").map(String::as_str) != Some(expected_state) {
            respond(
                &mut stream,
                "400 Bad Request",
                "<html><body>state mismatch</body></html>",
            )
            .await;
            continue;
        }
        let Some(code) = params.get("code") else {
            respond(
                &mut stream,
                "400 Bad Request",
                "<html><body>missing code</body></html>",
            )
            .await;
            continue;
        };
        let code = code.clone();
        respond(
            &mut stream,
            "200 OK",
            "<html><body>Login complete. You can close this tab.</body></html>",
        )
        .await;
        return Ok(code);
    }
}

pub async fn login(vault: &Vault, provider: &str) -> Result<String> {
    login_named(vault, provider, "default", false).await
}

pub async fn login_named(vault: &Vault, provider: &str, name: &str, force: bool) -> Result<String> {
    let p = match provider {
        "claude" | "anthropic" => Provider::Anthropic,
        "codex" | "openai" | "chatgpt" => Provider::Openai,
        "grok" | "xai" => Provider::Xai,
        "gemini" | "google" => Provider::Gemini,
        other => bail!("unknown provider '{other}' (expected claude|codex|grok|gemini)"),
    };
    validate_account_name(name)?;
    if !force && vault.has_account_name(p, name).await { bail!("{} account '{name}' already exists (use --force to replace)", p.as_str()); }
    match provider {
        "claude" | "anthropic" => login_claude(vault).await,
        "codex" | "openai" | "chatgpt" => login_codex(vault, name).await,
        "grok" | "xai" => login_grok(vault).await,
        "gemini" | "google" => login_gemini(vault).await,
        _ => unreachable!(),
    }
}

fn validate_account_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 32
        || !name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
    {
        bail!("account name must match [a-z0-9_-]{{1,32}}");
    }
    Ok(())
}

async fn save_named_login_account(
    vault: &Vault,
    mut account: Account,
    account_name: &str,
) -> Result<String> {
    validate_account_name(account_name)?;
    account.name = account_name.to_string();
    account.id = named_account_id(account.provider, &account.kind, account_name);
    account.path = None;
    let id = account.id.clone();
    vault.upsert(account).await?;
    Ok(id)
}

pub async fn claude_exchange(vault: &Vault, verifier: &str, input: &str) -> Result<String> {
    let (code, state) = parse_authorization_input(input);
    let code = code.ok_or_else(|| anyhow!("no authorization code provided"))?;
    if let Some(s) = &state {
        if s != verifier {
            bail!("oauth state mismatch");
        }
    }
    let state = state.unwrap_or_else(|| verifier.to_string());
    let resp = reqwest::Client::new()
        .post(ANTHROPIC_TOKEN_URL)
        .json(&json!({
            "grant_type": "authorization_code",
            "client_id": ANTHROPIC_CLIENT_ID,
            "code": code,
            "state": state,
            "redirect_uri": ANTHROPIC_REDIRECT_URI,
            "code_verifier": verifier,
        }))
        .send()
        .await?;
    let tokens = read_token_response(resp).await?;
    let scopes: Vec<String> = tokens
        .scope
        .as_deref()
        .map(|s| s.split_whitespace().map(String::from).collect())
        .unwrap_or_default();
    let access_token = tokens.access_token;
    let email = match fetch_provider_email(
        &reqwest::Client::new(),
        Provider::Anthropic,
        &access_token,
    )
    .await
    {
        Some(email) => Some(email),
        None => token_email(tokens.id_token.as_deref(), &access_token),
    };
    let mut account = Account {
        id: named_account_id(Provider::Anthropic, "oauth", "default"),
        provider: Provider::Anthropic,
        kind: "oauth".into(),
        name: "default".into(),
        description: email.clone(),
        paused: false,
        label: Some(email.as_ref().map(|email| format!("claude ({email})")).unwrap_or_else(|| "claude (oauth login)".into())),
        access_token: Some(access_token),
        refresh_token: tokens.refresh_token,
        id_token: tokens.id_token,
        api_key: None,
        expires_at_ms: tokens.expires_in.map(|s| now_ms() + s * 1000),
        last_refresh_ms: Some(now_ms()),
        account_meta: json!({"scopes": scopes}),
        cooldown_until_ms: None,
        status: "active".into(),
        path: None,
    };
    if let Some(email) = email {
        persist_account_email(&mut account, &email);
    }
    vault.upsert(account).await?;
    Ok("anthropic-oauth".into())
}

async fn login_claude(vault: &Vault) -> Result<String> {
    let pkce = generate_pkce();
    let url = anthropic_authorize_url(&pkce.challenge, &pkce.verifier);
    println!("open this url to authorize:\n\n  {url}\n");
    open_browser(&url);
    let input = prompt_line("paste the authorization code (format: code#state): ").await?;
    claude_exchange(vault, &pkce.verifier, &input).await
}

pub async fn codex_exchange(vault: &Vault, verifier: &str, code: &str) -> Result<String> {
    codex_exchange_named(vault, verifier, code, "default").await
}

pub async fn codex_exchange_named(
    vault: &Vault,
    verifier: &str,
    code: &str,
    account_name: &str,
) -> Result<String> {
    let tokens = exchange_codex_tokens(verifier, code, OPENAI_REDIRECT_URI).await?;
    let account = codex_account_from_tokens(tokens).await;
    save_named_login_account(vault, account, account_name).await
}

pub async fn codex_exchange_auto(vault: &Vault, verifier: &str, code: &str) -> Result<String> {
    let tokens = exchange_codex_tokens(verifier, code, OPENAI_REDIRECT_URI).await?;
    let account = codex_account_from_tokens(tokens).await;
    save_auto_codex_account(vault, account).await
}

async fn exchange_codex_tokens(
    verifier: &str,
    code: &str,
    redirect_uri: &str,
) -> Result<TokenResponse> {
    let resp = reqwest::Client::new()
        .post(OPENAI_TOKEN_URL)
        .form(&[
            ("grant_type", "authorization_code"),
            ("client_id", OPENAI_CLIENT_ID),
            ("code", code),
            ("code_verifier", verifier),
            ("redirect_uri", redirect_uri),
        ])
        .send()
        .await?;
    read_token_response(resp).await
}

#[derive(Debug, Clone)]
pub struct CodexDeviceStart {
    pub device_auth_id: String,
    pub user_code: String,
    pub interval_s: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodexDevicePoll {
    Pending,
    Done {
        authorization_code: String,
        code_verifier: String,
    },
    Failed(String),
}

pub async fn codex_device_start(client: &reqwest::Client) -> Result<CodexDeviceStart> {
    let response = client
        .post(OPENAI_DEVICE_USER_CODE_URL)
        .json(&json!({"client_id": OPENAI_CLIENT_ID}))
        .send()
        .await?;
    let status = response.status();
    let raw: Value = response.json().await?;
    if !status.is_success() {
        bail!("Codex device login could not start ({status})");
    }
    let device_auth_id = raw
        .get("device_auth_id")
        .and_then(Value::as_str)
        .context("Codex device login response omitted device_auth_id")?
        .to_string();
    let user_code = raw
        .get("user_code")
        .and_then(Value::as_str)
        .context("Codex device login response omitted user_code")?
        .to_string();
    let interval_s = raw
        .get("interval")
        .and_then(|value| {
            value
                .as_u64()
                .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
        })
        .unwrap_or(5)
        .clamp(1, 30);
    Ok(CodexDeviceStart {
        device_auth_id,
        user_code,
        interval_s,
    })
}

pub async fn codex_device_poll_once(
    client: &reqwest::Client,
    start: &CodexDeviceStart,
) -> CodexDevicePoll {
    let response = match client
        .post(OPENAI_DEVICE_TOKEN_URL)
        .json(&json!({
            "device_auth_id": start.device_auth_id,
            "user_code": start.user_code,
        }))
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => return CodexDevicePoll::Failed(error.to_string()),
    };
    let status = response.status();
    let body = match response.text().await {
        Ok(body) => body,
        Err(error) => return CodexDevicePoll::Failed(error.to_string()),
    };
    parse_codex_device_poll(status.as_u16(), &body)
}

pub fn parse_codex_device_poll(status: u16, body: &str) -> CodexDevicePoll {
    if status == 403 || status == 404 {
        return CodexDevicePoll::Pending;
    }
    let raw: Value = match serde_json::from_str(body) {
        Ok(raw) => raw,
        Err(error) => return CodexDevicePoll::Failed(error.to_string()),
    };
    if !(200..300).contains(&status) {
        return CodexDevicePoll::Failed(format!("Codex device login failed ({status})"));
    }
    let Some(authorization_code) = raw.get("authorization_code").and_then(Value::as_str) else {
        return CodexDevicePoll::Failed("device login omitted authorization_code".into());
    };
    let Some(code_verifier) = raw.get("code_verifier").and_then(Value::as_str) else {
        return CodexDevicePoll::Failed("device login omitted code_verifier".into());
    };
    if let Some(expected_challenge) = raw.get("code_challenge").and_then(Value::as_str) {
        if pkce_challenge(code_verifier) != expected_challenge {
            return CodexDevicePoll::Failed("device login returned an invalid PKCE verifier".into());
        }
    }
    CodexDevicePoll::Done {
        authorization_code: authorization_code.to_string(),
        code_verifier: code_verifier.to_string(),
    }
}

pub async fn codex_device_exchange_auto(
    vault: &Vault,
    authorization_code: &str,
    code_verifier: &str,
) -> Result<String> {
    let tokens = exchange_codex_tokens(
        code_verifier,
        authorization_code,
        OPENAI_DEVICE_REDIRECT_URI,
    )
    .await?;
    let account = codex_account_from_tokens(tokens).await;
    save_auto_codex_account(vault, account).await
}

async fn codex_account_from_tokens(tokens: TokenResponse) -> Account {
    let account_id = tokens
        .id_token
        .as_deref()
        .and_then(chatgpt_account_id)
        .or_else(|| chatgpt_account_id(&tokens.access_token));
    let identity_payload = tokens
        .id_token
        .as_deref()
        .and_then(jwt_payload)
        .or_else(|| jwt_payload(&tokens.access_token));
    let profile = identity_payload
        .as_ref()
        .and_then(|payload| payload.get("https://api.openai.com/profile"));
    let email = identity_payload
        .as_ref()
        .and_then(|payload| payload.get("email").and_then(Value::as_str))
        .or_else(|| profile.and_then(|value| value.get("email").and_then(Value::as_str)))
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty());
    let auth_claim = identity_payload
        .as_ref()
        .and_then(|payload| payload.get(OPENAI_JWT_CLAIM));
    let plan = auth_claim
        .and_then(|value| value.get("chatgpt_plan_type").and_then(Value::as_str))
        .or_else(|| {
            identity_payload
                .as_ref()
                .and_then(|payload| payload.get("chatgpt_plan_type").and_then(Value::as_str))
        })
        .map(String::from);
    let mut account_meta = json!({
        "account_id": account_id,
        "email": email,
        "plan": plan,
    });
    if let Ok(snapshot) = fetch_codex_usage(
        &tokens.access_token,
        account_meta.get("account_id").and_then(Value::as_str),
    )
    .await
    {
        account_meta["codex_limits"] = snapshot;
        account_meta["verified_at_ms"] = json!(now_ms());
    }
    Account {
        id: named_account_id(Provider::Openai, "oauth", "default"),
        provider: Provider::Openai,
        kind: "oauth".into(),
        name: "default".into(),
        description: None,
        paused: false,
        label: email
            .as_ref()
            .map(|value| format!("codex ({value})"))
            .or_else(|| Some("codex (chatgpt)".into())),
        access_token: Some(tokens.access_token.clone()),
        refresh_token: tokens.refresh_token,
        id_token: tokens.id_token,
        api_key: None,
        expires_at_ms: tokens
            .expires_in
            .map(|s| now_ms() + s * 1000)
            .or_else(|| jwt_exp_ms(&tokens.access_token)),
        last_refresh_ms: Some(now_ms()),
        account_meta,
        cooldown_until_ms: None,
        status: "active".into(),
        path: None,
    }
}

fn codex_usage_snapshot(raw: &Value) -> Option<Value> {
    let mut windows = Vec::new();
    for key in ["primary_window", "secondary_window"] {
        let Some(window) = raw.get("rate_limit").and_then(|limits| limits.get(key)) else {
            continue;
        };
        let Some(used_pct) = window.get("used_percent").and_then(Value::as_f64) else {
            continue;
        };
        let seconds = window
            .get("limit_window_seconds")
            .and_then(Value::as_i64);
        let label = match seconds {
            Some(18_000) => "5h".to_string(),
            Some(86_400) => "1d".to_string(),
            Some(604_800) => "7d".to_string(),
            Some(value) if value > 0 && value % 3_600 == 0 => format!("{}h", value / 3_600),
            Some(value) if value > 0 => format!("{}m", value / 60),
            _ => key.trim_end_matches("_window").to_string(),
        };
        windows.push(json!({
            "window": label,
            "used_pct": used_pct,
            "resets_at_s": window.get("reset_at").and_then(Value::as_i64),
        }));
    }
    if windows.is_empty() {
        return None;
    }
    Some(json!({
        "source": "Codex usage API",
        "observed_at_ms": now_ms(),
        "plan": raw.get("plan_type").cloned().unwrap_or(Value::Null),
        "windows": windows,
        "credits": raw.get("credits").cloned().unwrap_or(Value::Null),
    }))
}

async fn fetch_codex_usage(access_token: &str, account_id: Option<&str>) -> Result<Value> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?;
    let mut request = client
        .get(OPENAI_USAGE_URL)
        .bearer_auth(access_token)
        .header("accept", "application/json")
        .header("user-agent", "Alexandria");
    if let Some(account_id) = account_id.filter(|value| !value.is_empty()) {
        request = request.header("chatgpt-account-id", account_id);
    }
    let response = request.send().await?;
    let status = response.status();
    let raw: Value = response.json().await?;
    if !status.is_success() {
        bail!("Codex usage verification failed ({status})");
    }
    codex_usage_snapshot(&raw).context("Codex usage response did not contain rate-limit windows")
}

async fn save_auto_codex_account(vault: &Vault, mut account: Account) -> Result<String> {
    let provider_account_id = account
        .account_meta
        .get("account_id")
        .and_then(Value::as_str)
        .map(String::from);
    let email = account
        .account_meta
        .get("email")
        .and_then(Value::as_str)
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty());
    let identity = provider_account_id
        .as_deref()
        .or(email.as_deref())
        .context("Codex login succeeded but no account identity or email was returned")?;
    let existing = vault.list().await.into_iter().find(|candidate| {
        if candidate.provider != Provider::Openai || candidate.kind != "oauth" {
            return false;
        }
        let existing_account_id = candidate
            .account_meta
            .get("account_id")
            .and_then(Value::as_str);
        if let (Some(expected), Some(actual)) =
            (provider_account_id.as_deref(), existing_account_id)
        {
            return expected == actual;
        }
        provider_account_id.is_none()
            && email.as_deref()
                == candidate
                    .account_meta
                    .get("email")
                    .and_then(Value::as_str)
                    .map(str::trim)
    });
    if let Some(existing) = existing {
        if let (Some(old), Some(new)) = (
            existing.account_meta.as_object(),
            account.account_meta.as_object_mut(),
        ) {
            for (key, value) in old {
                new.entry(key.clone()).or_insert_with(|| value.clone());
            }
        }
        account.id = existing.id;
        account.name = existing.name;
        account.description = existing.description.or(email.clone());
        account.paused = existing.paused;
        account.status = existing.status;
        account.cooldown_until_ms = None;
        account.path = existing.path;
    } else {
        let digest = Sha256::digest(identity.as_bytes());
        let suffix: String = digest[..8].iter().map(|byte| format!("{byte:02x}")).collect();
        account.name = format!("acct-{suffix}");
        account.id = named_account_id(Provider::Openai, &account.kind, &account.name);
        account.description = email;
        account.path = None;
    }
    let id = account.id.clone();
    vault.upsert(account).await?;
    Ok(id)
}

async fn login_codex(vault: &Vault, account_name: &str) -> Result<String> {
    let listener = TcpListener::bind(OPENAI_CALLBACK_ADDR)
        .await
        .with_context(|| format!("binding {OPENAI_CALLBACK_ADDR} for the oauth callback"))?;
    let pkce = generate_pkce();
    let state = random_state();
    let url = openai_authorize_url(&pkce.challenge, &state);
    println!("open this url to authorize:\n\n  {url}\n");
    println!("waiting for the browser callback on {OPENAI_REDIRECT_URI} ...");
    open_browser(&url);
    let code = wait_for_openai_callback(&listener, &state).await?;
    codex_exchange_named(vault, &pkce.verifier, &code, account_name).await
}

pub fn gemini_authorize_url(challenge: &str, state: &str, redirect_uri: &str) -> String {
    let mut url = reqwest::Url::parse(GEMINI_AUTHORIZE_URL).unwrap();
    url.query_pairs_mut()
        .append_pair("client_id", crate::GEMINI_CLIENT_ID)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("response_type", "code")
        .append_pair("scope", GEMINI_SCOPES)
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", state)
        .append_pair("access_type", "offline")
        .append_pair("prompt", "consent");
    url.to_string()
}

pub async fn gemini_exchange(
    vault: &Vault,
    verifier: &str,
    redirect_uri: &str,
    code: &str,
) -> Result<String> {
    let resp = reqwest::Client::new()
        .post(crate::GOOGLE_TOKEN_URL)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("client_id", crate::GEMINI_CLIENT_ID),
            ("client_secret", &crate::gemini_client_secret()),
            ("redirect_uri", redirect_uri),
            ("code_verifier", verifier),
        ])
        .send()
        .await?;
    let tokens = read_token_response(resp).await?;
    let email = tokens
        .id_token
        .as_deref()
        .and_then(jwt_payload)
        .and_then(|p| p.get("email").and_then(|v| v.as_str().map(String::from)));
    let label = match &email {
        Some(e) => format!("gemini ({e})"),
        None => "gemini (oauth login)".into(),
    };
    let account = Account {
        id: named_account_id(Provider::Gemini, "oauth", "default"),
        provider: Provider::Gemini,
        kind: "oauth".into(),
        name: "default".into(),
        description: None,
        paused: false,
        label: Some(label),
        access_token: Some(tokens.access_token.clone()),
        refresh_token: tokens.refresh_token,
        id_token: tokens.id_token,
        api_key: None,
        expires_at_ms: tokens.expires_in.map(|s| now_ms() + s * 1000),
        last_refresh_ms: Some(now_ms()),
        account_meta: json!({"email": email}),
        cooldown_until_ms: None,
        status: "active".into(),
        path: None,
    };
    vault.upsert(account).await?;
    Ok("gemini-oauth".into())
}

pub async fn bind_loopback() -> Result<(TcpListener, u16)> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .context("binding a loopback port for the oauth callback")?;
    let port = listener.local_addr()?.port();
    Ok((listener, port))
}

async fn login_gemini(vault: &Vault) -> Result<String> {
    let (listener, port) = bind_loopback().await?;
    let redirect_uri = format!("http://localhost:{port}{GEMINI_CALLBACK_PATH}");
    let pkce = generate_pkce();
    let state = random_state();
    let url = gemini_authorize_url(&pkce.challenge, &state, &redirect_uri);
    println!("open this url and pick a Google account:\n\n  {url}\n");
    println!("waiting for the browser callback on {redirect_uri} ...");
    open_browser(&url);
    let code = wait_for_loopback_callback(&listener, &state, GEMINI_CALLBACK_PATH).await?;
    gemini_exchange(vault, &pkce.verifier, &redirect_uri, &code).await
}

#[derive(Debug, Clone, Deserialize)]
pub struct XaiDeviceStart {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    #[serde(default)]
    pub verification_uri_complete: Option<String>,
    pub expires_in: i64,
    #[serde(default = "default_device_interval")]
    pub interval: i64,
}

fn default_device_interval() -> i64 {
    5
}

#[derive(Debug, Clone, PartialEq)]
pub enum XaiDevicePoll {
    Pending,
    SlowDown,
    Done(Box<XaiTokens>),
    Failed(String),
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct XaiTokens {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub id_token: Option<String>,
    #[serde(default)]
    pub expires_in: Option<i64>,
}

pub async fn xai_device_start(http: &reqwest::Client) -> Result<XaiDeviceStart> {
    let resp = http
        .post(XAI_DEVICE_CODE_URL)
        .form(&[("client_id", XAI_CLIENT_ID), ("scope", XAI_SCOPES)])
        .send()
        .await?;
    let status = resp.status();
    let text = resp.text().await?;
    if !status.is_success() {
        bail!("xai device code request failed ({status}): {text}");
    }
    serde_json::from_str(&text).context("bad xai device code response")
}

pub fn parse_xai_device_poll(status: u16, body: &str) -> XaiDevicePoll {
    if (200..300).contains(&status) {
        return match serde_json::from_str::<XaiTokens>(body) {
            Ok(tokens) => XaiDevicePoll::Done(Box::new(tokens)),
            Err(e) => XaiDevicePoll::Failed(format!("bad xai token response: {e}")),
        };
    }
    let err = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v["error"].as_str().map(String::from))
        .unwrap_or_default();
    match err.as_str() {
        "authorization_pending" => XaiDevicePoll::Pending,
        "slow_down" => XaiDevicePoll::SlowDown,
        "access_denied" => XaiDevicePoll::Failed("authorization denied".into()),
        "expired_token" => XaiDevicePoll::Failed("device code expired".into()),
        other => XaiDevicePoll::Failed(format!(
            "xai token exchange failed ({status}): {}",
            if other.is_empty() { body } else { other }
        )),
    }
}

pub async fn xai_device_poll_once(http: &reqwest::Client, device_code: &str) -> XaiDevicePoll {
    let resp = http
        .post(XAI_TOKEN_URL)
        .form(&[
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ("device_code", device_code),
            ("client_id", XAI_CLIENT_ID),
        ])
        .send()
        .await;
    match resp {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            parse_xai_device_poll(status, &body)
        }
        Err(e) => XaiDevicePoll::Failed(format!("xai token endpoint unreachable: {e}")),
    }
}

pub async fn xai_upsert_from_tokens(vault: &Vault, tokens: &XaiTokens) -> Result<String> {
    // Prefer xAI's HTTPS OIDC userinfo response over locally decoded JWT
    // claims. The latter are not signature-verified here.
    let email = fetch_provider_email(
        &reqwest::Client::new(),
        Provider::Xai,
        &tokens.access_token,
    )
    .await;
    let label = match &email {
        Some(e) => format!("grok ({e})"),
        None => "grok (device login)".into(),
    };
    let mut account = Account {
        id: named_account_id(Provider::Xai, "oauth", "default"),
        provider: Provider::Xai,
        kind: "oauth".into(),
        name: "default".into(),
        description: email.clone(),
        paused: false,
        label: Some(label),
        access_token: Some(tokens.access_token.clone()),
        refresh_token: tokens.refresh_token.clone(),
        id_token: tokens.id_token.clone(),
        api_key: None,
        expires_at_ms: tokens
            .expires_in
            .map(|s| now_ms() + s * 1000)
            .or_else(|| jwt_exp_ms(&tokens.access_token)),
        last_refresh_ms: Some(now_ms()),
        account_meta: json!({
            "oidc_issuer": "https://auth.x.ai",
            "oidc_client_id": XAI_CLIENT_ID,
            "source": "device login",
        }),
        cooldown_until_ms: None,
        status: "active".into(),
        path: None,
    };
    if let Some(email) = email {
        persist_account_email(&mut account, &email);
    }
    vault.upsert(account).await?;
    Ok("xai-oauth".into())
}

async fn login_grok(vault: &Vault) -> Result<String> {
    let http = reqwest::Client::new();
    let start = match xai_device_start(&http).await {
        Ok(s) => s,
        Err(e) => {
            println!("xai device flow unavailable ({e}); falling back to grok CLI import:");
            println!("  1. run `grok` in another terminal and complete its login");
            println!("  2. come back here to import the credentials");
            prompt_line("press Enter once grok login is done: ").await?;
            let outcome = import_grok(vault).await;
            if outcome.imported.is_empty() {
                bail!(
                    "grok import found nothing ({})",
                    outcome
                        .note
                        .unwrap_or_else(|| "no ~/.grok/auth.json".into())
                );
            }
            return Ok(outcome.imported.join(", "));
        }
    };
    let url = start
        .verification_uri_complete
        .clone()
        .unwrap_or_else(|| start.verification_uri.clone());
    println!("open this url on any device to authorize:\n\n  {url}\n");
    println!("enter this code when asked: {}", start.user_code);
    open_browser(&url);
    let deadline = now_ms() + start.expires_in * 1000;
    let mut interval = start.interval.max(1) as u64;
    loop {
        if now_ms() > deadline {
            bail!("device code expired before authorization completed");
        }
        tokio::time::sleep(std::time::Duration::from_secs(interval)).await;
        match xai_device_poll_once(&http, &start.device_code).await {
            XaiDevicePoll::Pending => continue,
            XaiDevicePoll::SlowDown => {
                interval += 5;
                continue;
            }
            XaiDevicePoll::Done(tokens) => return xai_upsert_from_tokens(vault, &tokens).await,
            XaiDevicePoll::Failed(e) => bail!("xai device login failed: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_codex_account(token: &str) -> Account {
        Account {
            id: named_account_id(Provider::Openai, "oauth", "default"),
            provider: Provider::Openai,
            kind: "oauth".into(),
            name: "default".into(),
            description: None,
            paused: false,
            label: Some("codex (test)".into()),
            access_token: Some(token.into()),
            refresh_token: Some(format!("refresh-{token}")),
            id_token: None,
            api_key: None,
            expires_at_ms: Some(now_ms() + 60_000),
            last_refresh_ms: Some(now_ms()),
            account_meta: json!({"account_id": format!("account-{token}")}),
            cooldown_until_ms: None,
            status: "active".into(),
            path: None,
        }
    }

    #[tokio::test]
    async fn named_codex_save_preserves_existing_default() {
        let dir = std::env::temp_dir().join(format!(
            "alexandria-named-codex-{}-{}",
            std::process::id(),
            now_ms()
        ));
        let vault = Vault::open(dir.clone()).unwrap();
        vault.upsert(test_codex_account("default-token")).await.unwrap();

        let named_id = save_named_login_account(
            &vault,
            test_codex_account("second-token"),
            "work",
        )
        .await
        .unwrap();

        assert_eq!(named_id, "openai-oauth-work");
        let accounts = vault.list().await;
        assert_eq!(accounts.len(), 2);
        let default = accounts.iter().find(|account| account.name == "default").unwrap();
        let work = accounts.iter().find(|account| account.name == "work").unwrap();
        assert_eq!(default.access_token.as_deref(), Some("default-token"));
        assert_eq!(work.access_token.as_deref(), Some("second-token"));
        assert!(dir.join("openai-oauth.json").exists());
        assert!(dir.join("openai-oauth-work.json").exists());
        std::fs::remove_dir_all(dir).ok();
    }

    fn identified_codex_account(token: &str, account_id: &str, email: &str) -> Account {
        let mut account = test_codex_account(token);
        account.account_meta = json!({
            "account_id": account_id,
            "email": email,
            "codex_limits": {"windows": []},
        });
        account.label = Some(format!("codex ({email})"));
        account
    }

    #[tokio::test]
    async fn automatic_codex_identity_adds_reauths_and_preserves_workspaces() {
        let dir = std::env::temp_dir().join(format!(
            "alexandria-auto-codex-{}-{}",
            std::process::id(),
            now_ms()
        ));
        let vault = Vault::open(dir.clone()).unwrap();
        vault
            .upsert(identified_codex_account(
                "default-token",
                "workspace-default",
                "person@example.com",
            ))
            .await
            .unwrap();

        let default_id = save_auto_codex_account(
            &vault,
            identified_codex_account(
                "default-reauthed",
                "workspace-default",
                "person@example.com",
            ),
        )
        .await
        .unwrap();
        assert_eq!(default_id, "openai-oauth");
        assert_eq!(vault.list().await.len(), 1);

        let second_id = save_auto_codex_account(
            &vault,
            identified_codex_account(
                "second-token",
                "workspace-second",
                "second@example.com",
            ),
        )
        .await
        .unwrap();
        assert!(second_id.starts_with("openai-oauth-acct-"));
        assert_eq!(vault.list().await.len(), 2);

        let repeated_id = save_auto_codex_account(
            &vault,
            identified_codex_account(
                "replacement-token",
                "workspace-second",
                "second@example.com",
            ),
        )
        .await
        .unwrap();
        assert_eq!(repeated_id, second_id);
        assert_eq!(vault.list().await.len(), 2);
        assert_eq!(
            vault
                .list()
                .await
                .into_iter()
                .find(|account| account.id == second_id)
                .unwrap()
                .access_token
                .as_deref(),
            Some("replacement-token")
        );

        let third_id = save_auto_codex_account(
            &vault,
            identified_codex_account(
                "third-token",
                "workspace-third",
                "second@example.com",
            ),
        )
        .await
        .unwrap();
        assert_ne!(third_id, second_id);
        assert_eq!(vault.list().await.len(), 3);
        assert_eq!(
            vault
                .list()
                .await
                .into_iter()
                .find(|account| account.id == second_id)
                .unwrap()
                .description
                .as_deref(),
            Some("second@example.com")
        );
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn automatic_codex_identity_rejects_unidentified_account_without_mutation() {
        let dir = std::env::temp_dir().join(format!(
            "alexandria-auto-codex-missing-{}-{}",
            std::process::id(),
            now_ms()
        ));
        let vault = Vault::open(dir.clone()).unwrap();
        let mut account = test_codex_account("unknown-token");
        account.account_meta = json!({});
        assert!(save_auto_codex_account(&vault, account).await.is_err());
        assert!(vault.list().await.is_empty());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn codex_usage_snapshot_maps_windows_without_requiring_secondary() {
        let snapshot = codex_usage_snapshot(&json!({
            "plan_type": "plus",
            "rate_limit": {
                "primary_window": {
                    "used_percent": 23,
                    "reset_at": 1_800_000_000,
                    "limit_window_seconds": 18_000
                }
            },
            "credits": {"has_credits": false, "unlimited": false, "balance": 0}
        }))
        .unwrap();
        assert_eq!(snapshot["plan"], "plus");
        assert_eq!(snapshot["windows"][0]["window"], "5h");
        assert_eq!(snapshot["windows"][0]["used_pct"], 23.0);
        assert_eq!(snapshot["windows"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn codex_device_poll_handles_pending_and_validates_pkce() {
        assert_eq!(parse_codex_device_poll(403, "{}"), CodexDevicePoll::Pending);
        assert_eq!(parse_codex_device_poll(404, "{}"), CodexDevicePoll::Pending);
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let body = json!({
            "authorization_code": "auth-code",
            "code_verifier": verifier,
            "code_challenge": pkce_challenge(verifier),
        })
        .to_string();
        assert_eq!(
            parse_codex_device_poll(200, &body),
            CodexDevicePoll::Done {
                authorization_code: "auth-code".into(),
                code_verifier: verifier.into(),
            }
        );
        let bad = json!({
            "authorization_code": "auth-code",
            "code_verifier": verifier,
            "code_challenge": "wrong",
        })
        .to_string();
        assert!(matches!(
            parse_codex_device_poll(200, &bad),
            CodexDevicePoll::Failed(_)
        ));
    }

    #[test]
    fn pkce_shape() {
        let pkce = generate_pkce();
        assert_eq!(pkce.verifier.len(), 43);
        assert_eq!(pkce.challenge.len(), 43);
        for c in pkce.verifier.chars().chain(pkce.challenge.chars()) {
            assert!(c.is_ascii_alphanumeric() || c == '-' || c == '_');
        }
        assert_eq!(pkce.challenge, pkce_challenge(&pkce.verifier));
        assert_ne!(generate_pkce().verifier, pkce.verifier);
    }

    #[test]
    fn pkce_rfc7636_vector() {
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn anthropic_url_params() {
        let url = anthropic_authorize_url("chal", "stat");
        assert!(url.starts_with(ANTHROPIC_AUTHORIZE_URL));
        let parsed = reqwest::Url::parse(&url).unwrap();
        let q: HashMap<String, String> = parsed
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        assert_eq!(q["code"], "true");
        assert_eq!(q["client_id"], ANTHROPIC_CLIENT_ID);
        assert_eq!(q["response_type"], "code");
        assert_eq!(q["redirect_uri"], ANTHROPIC_REDIRECT_URI);
        assert_eq!(q["scope"], ANTHROPIC_SCOPES);
        assert_eq!(q["code_challenge"], "chal");
        assert_eq!(q["code_challenge_method"], "S256");
        assert_eq!(q["state"], "stat");
    }

    #[test]
    fn openai_url_params() {
        let url = openai_authorize_url("chal", "stat");
        assert!(url.starts_with(OPENAI_AUTHORIZE_URL));
        let parsed = reqwest::Url::parse(&url).unwrap();
        let q: HashMap<String, String> = parsed
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        assert_eq!(q["response_type"], "code");
        assert_eq!(q["client_id"], OPENAI_CLIENT_ID);
        assert_eq!(q["redirect_uri"], OPENAI_REDIRECT_URI);
        assert_eq!(q["scope"], OPENAI_SCOPES);
        assert_eq!(q["code_challenge"], "chal");
        assert_eq!(q["code_challenge_method"], "S256");
        assert_eq!(q["state"], "stat");
        assert_eq!(q["id_token_add_organizations"], "true");
        assert_eq!(q["codex_cli_simplified_flow"], "true");
        assert_eq!(q["originator"], "pi");
    }

    #[test]
    fn authorization_input_parsing() {
        assert_eq!(
            parse_authorization_input("abc#xyz"),
            (Some("abc".into()), Some("xyz".into()))
        );
        assert_eq!(
            parse_authorization_input(" plaincode \n"),
            (Some("plaincode".into()), None)
        );
        assert_eq!(
            parse_authorization_input("code=abc&state=xyz"),
            (Some("abc".into()), Some("xyz".into()))
        );
        assert_eq!(
            parse_authorization_input("http://localhost:1455/auth/callback?code=abc&state=xyz"),
            (Some("abc".into()), Some("xyz".into()))
        );
        assert_eq!(parse_authorization_input(""), (None, None));
        assert_eq!(parse_authorization_input("   "), (None, None));
    }

    #[test]
    fn jwt_account_id_extraction() {
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&json!({
                "https://api.openai.com/auth": {"chatgpt_account_id": "acct-123"}
            }))
            .unwrap(),
        );
        let token = format!("eyJhbGciOiJub25lIn0.{payload}.sig");
        assert_eq!(chatgpt_account_id(&token), Some("acct-123".into()));
        assert_eq!(chatgpt_account_id("not-a-jwt"), None);
        let empty = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&json!({})).unwrap());
        assert_eq!(chatgpt_account_id(&format!("h.{empty}.s")), None);
    }

    #[test]
    fn browser_command_per_platform() {
        let cmd = browser_open_command("https://example.com/auth?x=1");
        if cfg!(any(target_os = "macos", target_os = "linux", target_os = "windows")) {
            let (program, args) = cmd.expect("major platforms have a browser opener");
            let expected = if cfg!(target_os = "macos") {
                "open"
            } else if cfg!(target_os = "windows") {
                "cmd"
            } else {
                "xdg-open"
            };
            assert_eq!(program, expected);
            assert_eq!(
                args.last().map(String::as_str),
                Some("https://example.com/auth?x=1")
            );
            if cfg!(target_os = "windows") {
                assert_eq!(args[..3], ["/C", "start", ""]);
            }
        } else {
            assert!(cmd.is_none());
        }
    }

    #[test]
    fn xai_device_poll_parsing() {
        assert_eq!(
            parse_xai_device_poll(400, r#"{"error":"authorization_pending"}"#),
            XaiDevicePoll::Pending
        );
        assert_eq!(
            parse_xai_device_poll(429, r#"{"error":"slow_down"}"#),
            XaiDevicePoll::SlowDown
        );
        assert!(matches!(
            parse_xai_device_poll(400, r#"{"error":"access_denied"}"#),
            XaiDevicePoll::Failed(e) if e.contains("denied")
        ));
        assert!(matches!(
            parse_xai_device_poll(400, r#"{"error":"expired_token"}"#),
            XaiDevicePoll::Failed(e) if e.contains("expired")
        ));
        let done = parse_xai_device_poll(
            200,
            r#"{"access_token":"tok","refresh_token":"ref","expires_in":3600}"#,
        );
        match done {
            XaiDevicePoll::Done(t) => {
                assert_eq!(t.access_token, "tok");
                assert_eq!(t.refresh_token.as_deref(), Some("ref"));
                assert_eq!(t.expires_in, Some(3600));
            }
            other => panic!("expected Done, got {other:?}"),
        }
        assert!(matches!(
            parse_xai_device_poll(200, "not json"),
            XaiDevicePoll::Failed(_)
        ));
        assert!(matches!(
            parse_xai_device_poll(500, "boom"),
            XaiDevicePoll::Failed(e) if e.contains("500")
        ));
    }

    #[test]
    fn callback_request_parsing() {
        let req = "GET /auth/callback?code=abc&state=xyz HTTP/1.1\r\nHost: localhost\r\n\r\n";
        let target = request_target(req).unwrap();
        assert_eq!(callback_path(target), OPENAI_CALLBACK_PATH);
        let q = callback_query(target);
        assert_eq!(q["code"], "abc");
        assert_eq!(q["state"], "xyz");
        assert_eq!(callback_path("/favicon.ico"), "/favicon.ico");
        assert!(callback_query("/favicon.ico").is_empty());
        assert!(request_target("").is_none());
    }
}
