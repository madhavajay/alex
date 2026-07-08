use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(windows)]
use std::ffi::OsString;

use anyhow::{bail, Context, Result};
use serde::Serialize;
use serde_json::{json, Value};

use crate::{ui, Config};

const PROVIDER_NAME: &str = "alexandria";
const PI_MIN_VERSION: Version = Version {
    major: 0,
    minor: 80,
    patch: 0,
};
const FALLBACK_MODELS: &[&str] = &[
    "claude-opus-4-8",
    "claude-sonnet-5",
    "claude-haiku-4-5",
    "gpt-5.5",
    "grok-code-fast-1",
    "gemini-2.5-flash",
];

struct HarnessDef {
    name: &'static str,
}

const HARNESSES: &[HarnessDef] = &[HarnessDef { name: "pi" }];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Version {
    major: u64,
    minor: u64,
    patch: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VersionCheck {
    pub parsed: Option<Version>,
    pub warning: Option<String>,
}

#[derive(Debug, Serialize)]
struct HarnessStatus {
    harness: &'static str,
    installed: bool,
    binary: Option<String>,
    version: Option<String>,
    version_warning: Option<String>,
    config_dir: String,
    config_dir_exists: bool,
    connected: bool,
    daemon_reachable: bool,
}

#[derive(Debug)]
struct PiDetection {
    binary: Option<PathBuf>,
    version_raw: Option<String>,
    version: VersionCheck,
}

pub(crate) async fn connect_cmd(
    config: &Config,
    harness: Option<String>,
    config_dir: Option<PathBuf>,
    json: bool,
) -> Result<()> {
    match harness.as_deref() {
        None => connect_status(config, config_dir, json).await,
        Some("pi") => connect_pi(config, config_dir, json).await,
        Some(name) => bail!(
            "unknown harness '{name}' (supported: {})",
            HARNESSES
                .iter()
                .map(|h| h.name)
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

pub(crate) async fn disconnect_cmd(
    config: &Config,
    harness: String,
    config_dir: Option<PathBuf>,
) -> Result<()> {
    match harness.as_str() {
        "pi" => disconnect_pi(config, config_dir).await,
        name => bail!(
            "unknown harness '{name}' (supported: {})",
            HARNESSES
                .iter()
                .map(|h| h.name)
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

async fn connect_status(config: &Config, config_dir: Option<PathBuf>, json_out: bool) -> Result<()> {
    let statuses = vec![pi_status(config, config_dir).await?];
    if json_out {
        println!("{}", serde_json::to_string_pretty(&statuses)?);
        return Ok(());
    }
    println!("{}", ui::section("harness connections"));
    println!(
        "{} {} {} {} {} {}",
        ui::pad_right(&ui::column_header("harness"), 10),
        ui::pad_right(&ui::column_header("installed"), 10),
        ui::pad_right(&ui::column_header("version"), 14),
        ui::pad_right(&ui::column_header("config"), 8),
        ui::pad_right(&ui::column_header("connected"), 10),
        ui::column_header("daemon")
    );
    for status in statuses {
        let installed = if status.installed { "yes" } else { "no" };
        let config_exists = if status.config_dir_exists { "yes" } else { "no" };
        let connected = if status.connected { "yes" } else { "no" };
        let daemon = if status.daemon_reachable { "up" } else { "down" };
        println!(
            "{} {} {} {} {} {}",
            ui::pad_right(status.harness, 10),
            ui::pad_right(installed, 10),
            ui::pad_right(status.version.as_deref().unwrap_or("-"), 14),
            ui::pad_right(config_exists, 8),
            ui::pad_right(connected, 10),
            daemon
        );
        if let Some(warning) = status.version_warning {
            println!("  {}", ui::amber(&warning));
        }
        if !status.config_dir_exists {
            println!("  {}", ui::dim(&format!("config: {}", status.config_dir)));
        }
    }
    Ok(())
}

async fn pi_status(config: &Config, config_dir: Option<PathBuf>) -> Result<HarnessStatus> {
    let detection = detect_pi();
    let config_dir = config_dir.unwrap_or_else(default_pi_config_dir);
    let config_dir_exists = config_dir.is_dir();
    let connected = models_json_connected(&config_dir.join("models.json")).unwrap_or(false);
    let daemon_reachable = daemon_health(config).await;
    Ok(HarnessStatus {
        harness: "pi",
        installed: detection.binary.is_some(),
        binary: detection
            .binary
            .as_ref()
            .map(|p| p.to_string_lossy().to_string()),
        version: detection.version_raw,
        version_warning: detection.version.warning,
        config_dir: config_dir.to_string_lossy().to_string(),
        config_dir_exists,
        connected,
        daemon_reachable,
    })
}

async fn connect_pi(config: &Config, config_dir: Option<PathBuf>, json_out: bool) -> Result<()> {
    let detection = detect_pi();
    if detection.binary.is_none() {
        bail!("pi is not installed or not on PATH; install it with `npm install -g @earendil-works/pi-coding-agent`");
    }
    if let Some(warning) = &detection.version.warning {
        eprintln!("{}", ui::amber(warning));
    }

    let config_dir = config_dir.unwrap_or_else(default_pi_config_dir);
    if !config_dir.is_dir() {
        bail!(
            "pi config dir does not exist at {}; run pi once first (it creates ~/.pi/agent), or pass --config-dir",
            config_dir.display()
        );
    }

    if !daemon_health(config).await {
        bail!(
            "could not reach the alexandria daemon at {}; start it with `alex daemon --background`",
            normalized_base_url(config)
        );
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()?;
    revoke_pi_harness_keys(config, &client).await?;
    let minted = mint_pi_key(config, &client).await?;
    let models = fetch_models(config, &client)
        .await
        .unwrap_or_else(|| FALLBACK_MODELS.iter().map(|m| (*m).to_string()).collect());
    let models_path = config_dir.join("models.json");
    upsert_pi_provider(&models_path, &normalized_base_url(config), &minted.key, &models)?;

    if json_out {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "harness": "pi",
                "version": detection.version_raw,
                "config_path": models_path,
                "models": models,
                "key_id": minted.id,
            }))?
        );
    } else {
        println!("{}", ui::section("pi connected"));
        println!("key id: {}", ui::amber(&minted.id));
        println!("models: {}", models.len());
        println!();
        println!("pi --provider alexandria --model claude-opus-4-8");
        println!("or pick via /model inside pi — changes hot-reload");
    }
    Ok(())
}

async fn disconnect_pi(config: &Config, config_dir: Option<PathBuf>) -> Result<()> {
    let config_dir = config_dir.unwrap_or_else(default_pi_config_dir);
    let models_path = config_dir.join("models.json");
    if !remove_pi_provider(&models_path)? {
        println!("pi not connected");
        return Ok(());
    }

    let mut revoked = 0usize;
    if daemon_health(config).await {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()?;
        match revoke_pi_harness_keys(config, &client).await {
            Ok(count) => revoked = count,
            Err(e) => eprintln!(
                "{}",
                ui::amber(&format!(
                    "removed local pi config, but could not revoke daemon keys: {e}"
                ))
            ),
        }
    } else {
        eprintln!(
            "{}",
            ui::amber("removed local pi config, but the daemon is unreachable; harness keys remain, rerun disconnect with the daemon up")
        );
    }
    println!("disconnected pi; revoked {revoked} harness key(s)");
    Ok(())
}

#[derive(Debug)]
struct MintedKey {
    id: String,
    key: String,
}

async fn mint_pi_key(config: &Config, client: &reqwest::Client) -> Result<MintedKey> {
    let body = json!({
        "kind": "harness",
        "label": "pi",
        "tags": {"harness": "pi"},
    });
    let (status, value) = admin_send(config, client, reqwest::Method::POST, "/admin/run-keys", Some(body)).await?;
    if !status.is_success() {
        bail!(
            "daemon could not mint a pi harness key ({status}): {}",
            ui::truncate(&value.to_string(), 300)
        );
    }
    let id = value["id"].as_str().unwrap_or("-").to_string();
    if value["kind"].as_str() != Some("harness") || !value["expires_ms"].is_null() {
        if id != "-" {
            let _ = admin_send(
                config,
                client,
                reqwest::Method::DELETE,
                &format!("/admin/run-keys/{id}"),
                None,
            )
            .await;
        }
        bail!("the running daemon does not support harness run keys; update alex and restart the daemon");
    }
    let key = value["key"]
        .as_str()
        .context("daemon response did not include the one-time run key")?
        .to_string();
    Ok(MintedKey { id, key })
}

async fn revoke_pi_harness_keys(config: &Config, client: &reqwest::Client) -> Result<usize> {
    let value = admin_get(config, client, "/admin/run-keys", &[("all", "1")]).await?;
    let rows = value["run_keys"].as_array().cloned().unwrap_or_default();
    let ids: Vec<String> = rows
        .iter()
        .filter(|row| row["kind"].as_str() == Some("harness") && row["label"].as_str() == Some("pi"))
        .filter_map(|row| row["id"].as_str().map(String::from))
        .collect();
    for id in &ids {
        let (status, value) = admin_send(
            config,
            client,
            reqwest::Method::DELETE,
            &format!("/admin/run-keys/{id}"),
            None,
        )
        .await?;
        if !status.is_success() {
            bail!(
                "daemon could not revoke old pi harness key {id} ({status}): {}",
                ui::truncate(&value.to_string(), 300)
            );
        }
    }
    Ok(ids.len())
}

async fn fetch_models(config: &Config, client: &reqwest::Client) -> Option<Vec<String>> {
    let url = format!("{}/v1/models", normalized_base_url(config));
    let resp = client
        .get(url)
        .header("x-api-key", &config.local_key)
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let value: Value = resp.json().await.ok()?;
    let ids = value["data"]
        .as_array()?
        .iter()
        .filter_map(|row| row["id"].as_str().map(String::from))
        .collect();
    let filtered = filter_model_ids(ids);
    (!filtered.is_empty()).then_some(filtered)
}

async fn admin_get(
    config: &Config,
    client: &reqwest::Client,
    path: &str,
    params: &[(&str, &str)],
) -> Result<Value> {
    let resp = client
        .get(format!("{}{}", normalized_base_url(config), path))
        .header("x-api-key", &config.local_key)
        .query(params)
        .send()
        .await
        .with_context(|| format!("could not reach the alexandria daemon at {}", normalized_base_url(config)))?;
    let status = resp.status();
    let value: Value = resp.json().await.unwrap_or_default();
    if !status.is_success() {
        bail!(
            "daemon returned {status}: {}",
            ui::truncate(&value.to_string(), 300)
        );
    }
    Ok(value)
}

async fn admin_send(
    config: &Config,
    client: &reqwest::Client,
    method: reqwest::Method,
    path: &str,
    body: Option<Value>,
) -> Result<(reqwest::StatusCode, Value)> {
    let mut req = client
        .request(method, format!("{}{}", normalized_base_url(config), path))
        .header("x-api-key", &config.local_key);
    if let Some(body) = body {
        req = req.json(&body);
    }
    let resp = req
        .send()
        .await
        .with_context(|| format!("could not reach the alexandria daemon at {}", normalized_base_url(config)))?;
    let status = resp.status();
    let value: Value = resp.json().await.unwrap_or_default();
    Ok((status, value))
}

async fn daemon_health(config: &Config) -> bool {
    let Ok(client) = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
    else {
        return false;
    };
    client
        .get(format!("{}/health", normalized_base_url(config)))
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

fn normalized_base_url(config: &Config) -> String {
    let host = if config.host == "0.0.0.0" {
        "127.0.0.1"
    } else {
        config.host.as_str()
    };
    format!("http://{host}:{}", config.port)
}

fn default_pi_config_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".pi")
        .join("agent")
}

fn detect_pi() -> PiDetection {
    let binary = find_on_path("pi");
    let version_raw = binary.as_ref().and_then(|path| pi_version(path));
    let version = if binary.is_some() {
        check_version(version_raw.as_deref())
    } else {
        VersionCheck {
            parsed: None,
            warning: None,
        }
    };
    PiDetection {
        binary,
        version_raw,
        version,
    }
}

fn find_on_path(bin: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for candidate in executable_candidates(&dir, bin) {
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn executable_candidates(dir: &Path, bin: &str) -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        let pathext = std::env::var_os("PATHEXT").unwrap_or_else(|| OsString::from(".EXE;.CMD;.BAT"));
        let mut out = vec![dir.join(bin)];
        for ext in pathext.to_string_lossy().split(';') {
            out.push(dir.join(format!("{bin}{ext}")));
        }
        out
    }
    #[cfg(not(windows))]
    {
        vec![dir.join(bin)]
    }
}

fn pi_version(binary: &Path) -> Option<String> {
    let out = Command::new(binary).arg("--version").output().ok()?;
    let raw = if out.stdout.is_empty() {
        String::from_utf8_lossy(&out.stderr).to_string()
    } else {
        String::from_utf8_lossy(&out.stdout).to_string()
    };
    let raw = raw.trim().to_string();
    (!raw.is_empty()).then_some(raw)
}

pub(crate) fn check_version(raw: Option<&str>) -> VersionCheck {
    match raw.and_then(parse_version) {
        Some(version) if version >= PI_MIN_VERSION => VersionCheck {
            parsed: Some(version),
            warning: None,
        },
        Some(version) => VersionCheck {
            parsed: Some(version),
            warning: Some(format!(
                "pi version {version} is older than 0.80.0; continuing, but upgrade pi if connection fails"
            )),
        },
        None => VersionCheck {
            parsed: None,
            warning: Some("could not parse `pi --version`; continuing".into()),
        },
    }
}

fn parse_version(raw: &str) -> Option<Version> {
    let token = raw
        .split_whitespace()
        .find(|part| part.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false))?;
    let mut parts = token.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts
        .next()
        .and_then(|p| {
            let digits: String = p.chars().take_while(|c| c.is_ascii_digit()).collect();
            digits.parse().ok()
        })
        .unwrap_or(0);
    Some(Version {
        major,
        minor,
        patch,
    })
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

fn models_json_connected(path: &Path) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let raw = std::fs::read_to_string(path)?;
    let value: Value = serde_json::from_str(&raw)
        .with_context(|| format!("could not parse {}; aborting without changes", path.display()))?;
    Ok(value["providers"][PROVIDER_NAME].is_object())
}

pub(crate) fn upsert_pi_provider(
    path: &Path,
    base_url: &str,
    api_key: &str,
    model_ids: &[String],
) -> Result<()> {
    let mut value = read_models_json(path)?;
    let providers = ensure_providers_object(&mut value, path)?;
    providers.insert(
        PROVIDER_NAME.to_string(),
        json!({
            "baseUrl": base_url,
            "api": "anthropic-messages",
            "apiKey": api_key,
            "headers": {
                "x-alexandria-harness": "pi",
                "x-alexandria-harness-version": "!pi --version",
            },
            "models": model_ids.iter().map(|id| {
                json!({
                    "id": id,
                    "reasoning": reasoning_enabled(id),
                    "input": ["text", "image"],
                    "contextWindow": 200000,
                    "maxTokens": 16384,
                })
            }).collect::<Vec<_>>(),
        }),
    );
    atomic_write_json(path, &value)
}

pub(crate) fn remove_pi_provider(path: &Path) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let mut value = read_models_json(path)?;
    let providers = ensure_providers_object(&mut value, path)?;
    if providers.remove(PROVIDER_NAME).is_none() {
        return Ok(false);
    }
    atomic_write_json(path, &value)?;
    Ok(true)
}

fn read_models_json(path: &Path) -> Result<Value> {
    if !path.exists() {
        return Ok(json!({"providers": {}}));
    }
    let raw = std::fs::read_to_string(path)?;
    serde_json::from_str(&raw)
        .with_context(|| format!("could not parse {}; aborting without changes", path.display()))
}

fn ensure_providers_object<'a>(
    value: &'a mut Value,
    path: &Path,
) -> Result<&'a mut serde_json::Map<String, Value>> {
    if !value.is_object() {
        bail!("{} must contain a JSON object; aborting without changes", path.display());
    }
    if value.get("providers").is_none() {
        value["providers"] = json!({});
    }
    value["providers"]
        .as_object_mut()
        .with_context(|| format!("{}.providers must be an object; aborting without changes", path.display()))
}

fn atomic_write_json(path: &Path, value: &Value) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let tmp = parent.join(format!(
        ".{}.tmp.{}",
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("models.json"),
        std::process::id()
    ));
    let data = serde_json::to_string_pretty(value)? + "\n";
    {
        let mut file = std::fs::File::create(&tmp)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        file.write_all(data.as_bytes())?;
        file.sync_all()?;
    }
    #[cfg(windows)]
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    std::fs::rename(&tmp, path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

pub(crate) fn filter_model_ids(ids: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for id in ids {
        if id.contains('/') || !allowed_model_prefix(&id) || !seen.insert(id.clone()) {
            continue;
        }
        out.push(id);
    }
    out
}

fn allowed_model_prefix(id: &str) -> bool {
    ["claude-", "gpt-", "o3", "o4", "codex-", "grok-", "gemini-"]
        .iter()
        .any(|prefix| id.starts_with(prefix))
}

pub(crate) fn reasoning_enabled(id: &str) -> bool {
    let id = id.to_ascii_lowercase();
    id.contains("opus")
        || id.contains("sonnet")
        || id.contains("gpt-5")
        || id.starts_with("o3")
        || id.starts_with("o4")
        || id.contains("grok")
        || (id.starts_with("gemini-") && id.contains("pro"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "alex-harness-connect-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn model_ids() -> Vec<String> {
        vec!["claude-opus-4-8".into(), "gpt-5.5".into()]
    }

    #[test]
    fn upsert_models_json_missing_file() {
        let dir = tmpdir("missing");
        let path = dir.join("models.json");
        upsert_pi_provider(&path, "http://127.0.0.1:4100", "alxk-test", &model_ids()).unwrap();
        let value: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let provider = &value["providers"]["alexandria"];
        assert_eq!(provider["baseUrl"], "http://127.0.0.1:4100");
        assert_eq!(provider["api"], "anthropic-messages");
        assert_eq!(provider["apiKey"], "alxk-test");
        assert_eq!(provider["headers"]["x-alexandria-harness"], "pi");
        assert_eq!(provider["headers"]["x-alexandria-harness-version"], "!pi --version");
        assert_eq!(provider["models"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn upsert_preserves_other_providers_and_top_level_keys() {
        let dir = tmpdir("preserve");
        let path = dir.join("models.json");
        std::fs::write(
            &path,
            r#"{"top":true,"providers":{"other":{"api":"openai","models":[{"id":"x"}]}}}"#,
        )
        .unwrap();
        upsert_pi_provider(&path, "http://127.0.0.1:4100", "alxk-test", &model_ids()).unwrap();
        let value: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(value["top"], true);
        assert_eq!(value["providers"]["other"]["api"], "openai");
        assert!(value["providers"]["alexandria"].is_object());
    }

    #[test]
    fn corrupt_json_errors_without_clobbering() {
        let dir = tmpdir("corrupt");
        let path = dir.join("models.json");
        std::fs::write(&path, "{not json").unwrap();
        let err = upsert_pi_provider(&path, "http://127.0.0.1:4100", "alxk-test", &model_ids())
            .unwrap_err();
        assert!(err.to_string().contains("could not parse"));
        assert_eq!(std::fs::read_to_string(path).unwrap(), "{not json");
    }

    #[test]
    fn disconnect_removal_round_trip_keeps_foreign_provider() {
        let dir = tmpdir("disconnect");
        let path = dir.join("models.json");
        std::fs::write(
            &path,
            r#"{"providers":{"alexandria":{"api":"anthropic-messages"},"foreign":{"api":"x"}}}"#,
        )
        .unwrap();
        assert!(remove_pi_provider(&path).unwrap());
        let value: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(value["providers"]["alexandria"].is_null());
        assert_eq!(value["providers"]["foreign"]["api"], "x");
        assert!(!remove_pi_provider(&path).unwrap());
    }

    #[test]
    fn model_id_filter_dedupes_and_rejects_wrapped_ids() {
        let ids = vec![
            "claude-opus-4-8".into(),
            "alexandria/claude-opus-4-8".into(),
            "gpt-5.5".into(),
            "gpt-5.5".into(),
            "random".into(),
            "o3-mini".into(),
            "codex-mini".into(),
            "gemini-2.5-flash".into(),
        ];
        assert_eq!(
            filter_model_ids(ids),
            vec![
                "claude-opus-4-8",
                "gpt-5.5",
                "o3-mini",
                "codex-mini",
                "gemini-2.5-flash"
            ]
        );
    }

    #[test]
    fn version_parse_and_warning() {
        let ok = check_version(Some("0.80.3"));
        assert_eq!(ok.parsed.unwrap().to_string(), "0.80.3");
        assert!(ok.warning.is_none());
        let old = check_version(Some("0.79.0"));
        assert_eq!(old.parsed.unwrap().to_string(), "0.79.0");
        assert!(old.warning.unwrap().contains("older than"));
        let garbage = check_version(Some("garbage"));
        assert!(garbage.parsed.is_none());
        assert!(garbage.warning.unwrap().contains("could not parse"));
    }

    #[test]
    fn reasoning_flag_mapping() {
        assert!(reasoning_enabled("claude-opus-4-8"));
        assert!(reasoning_enabled("claude-sonnet-5"));
        assert!(reasoning_enabled("gpt-5.5"));
        assert!(reasoning_enabled("o3-mini"));
        assert!(reasoning_enabled("o4-mini"));
        assert!(reasoning_enabled("grok-code-fast-1"));
        assert!(reasoning_enabled("gemini-2.5-pro"));
        assert!(!reasoning_enabled("claude-haiku-4-5"));
        assert!(!reasoning_enabled("gemini-2.5-flash"));
    }
}
