use std::cmp::Ordering;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use flate2::read::GzDecoder;
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::{alexandria_home, current_uid, detect_service_state, Config, ServiceState};

const DEFAULT_MANIFEST_URL: &str =
    "https://github.com/madhavajay/alex/releases/latest/download/manifest.json";
const DEFAULT_RELEASES_URL: &str =
    "https://api.github.com/repos/madhavajay/alex/releases?per_page=30";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Brew,
    Cargo,
    Standalone,
}

impl Channel {
    pub fn as_str(self) -> &'static str {
        match self {
            Channel::Brew => "brew",
            Channel::Cargo => "cargo",
            Channel::Standalone => "standalone",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UpdateChannel {
    #[default]
    Stable,
    Beta,
}

impl UpdateChannel {
    pub fn as_str(self) -> &'static str {
        match self {
            UpdateChannel::Stable => "stable",
            UpdateChannel::Beta => "beta",
        }
    }

    pub fn parse(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "" | "stable" => Ok(UpdateChannel::Stable),
            "beta" => Ok(UpdateChannel::Beta),
            other => anyhow::bail!("unknown update channel '{other}' (expected stable or beta)"),
        }
    }
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct Manifest {
    pub schema_version: u32,
    pub published_at: Option<String>,
    pub components: Components,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct Components {
    pub cli: Component,
    pub app: Option<AppComponent>,
}

#[derive(Debug, Deserialize)]
pub struct Component {
    pub version: String,
    pub notes_url: Option<String>,
    pub platforms: std::collections::HashMap<String, PlatformAsset>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct AppComponent {
    pub version: String,
    pub appcast: Option<String>,
    pub platforms: std::collections::HashMap<String, PlatformAsset>,
}

#[derive(Debug, Deserialize, Clone)]
#[allow(dead_code)]
pub struct PlatformAsset {
    pub url: String,
    pub sha256: String,
    pub size: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct UpdateCheck {
    pub current: String,
    pub latest: String,
    pub update_available: bool,
    pub notes_url: Option<String>,
    pub channel: Channel,
    pub update_channel: UpdateChannel,
    pub asset: PlatformAsset,
}

pub fn install_channel(exe: &Path, home: &Path) -> Channel {
    let s = exe.to_string_lossy();
    if s.contains("/Cellar/")
        || s.starts_with("/opt/homebrew/")
        || s.starts_with("/home/linuxbrew/")
    {
        return Channel::Brew;
    }
    if exe.starts_with(home.join(".cargo").join("bin")) {
        return Channel::Cargo;
    }
    Channel::Standalone
}

pub fn platform_key() -> Result<&'static str> {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Ok("aarch64-apple-darwin")
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        Ok("x86_64-apple-darwin")
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Ok("x86_64-unknown-linux-gnu")
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        Ok("aarch64-unknown-linux-gnu")
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        Ok("x86_64-pc-windows-msvc")
    } else {
        anyhow::bail!(
            "self-update is not available for this platform (os={}, arch={})",
            std::env::consts::OS,
            std::env::consts::ARCH
        );
    }
}

/// Parsed version ordered so that a stable release ranks above any of its
/// betas: `0.1.24-beta.2` < `0.1.24`, and `0.1.23` < `0.1.24-beta.1`.
/// The fourth component is `(1, 0)` for stable and `(0, n)` for `-beta.n`.
fn parse_version(version: &str) -> Option<(u64, u64, u64, (u8, u64))> {
    let trimmed = version.trim().strip_prefix('v').unwrap_or(version.trim());
    let (base, pre) = match trimmed.split_once('-') {
        Some((base, pre)) => (base, Some(pre)),
        None => (trimmed, None),
    };
    let mut parts = base.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    let rank = match pre {
        None => (1, 0),
        Some(pre) => {
            let n = pre.strip_prefix("beta.")?.parse().ok()?;
            (0, n)
        }
    };
    Some((major, minor, patch, rank))
}

fn compare_versions(current: &str, latest: &str) -> Option<Ordering> {
    Some(parse_version(latest)?.cmp(&parse_version(current)?))
}

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    assets: Vec<GithubReleaseAsset>,
}

#[derive(Debug, Deserialize)]
struct GithubReleaseAsset {
    name: String,
    browser_download_url: String,
}

/// Pick the newest release (stable or beta) that carries a manifest.json,
/// so a beta user is offered the final release once it ships.
fn select_release_manifest_url(releases: &[GithubRelease]) -> Option<(String, String)> {
    releases
        .iter()
        .filter(|r| !r.draft)
        .filter_map(|r| {
            let version = parse_version(&r.tag_name)?;
            let manifest = r.assets.iter().find(|a| a.name == "manifest.json")?;
            Some((version, r.tag_name.clone(), manifest.browser_download_url.clone()))
        })
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, tag, url)| (tag, url))
}

async fn fetch_text(url: &str) -> Result<String> {
    let path = PathBuf::from(url);
    if path.exists() {
        return tokio::fs::read_to_string(&path)
            .await
            .with_context(|| format!("reading {}", path.display()));
    }
    Ok(reqwest::Client::builder()
        .timeout(Duration::from_secs(600))
        .user_agent(concat!("alex-update/", env!("CARGO_PKG_VERSION")))
        .build()?
        .get(url)
        .send()
        .await
        .with_context(|| format!("fetching {url}"))?
        .error_for_status()
        .with_context(|| format!("fetching {url}"))?
        .text()
        .await?)
}

async fn manifest_source(update_channel: UpdateChannel) -> Result<String> {
    if let Ok(explicit) = std::env::var("ALEX_UPDATE_MANIFEST_URL") {
        return Ok(explicit);
    }
    match update_channel {
        UpdateChannel::Stable => Ok(DEFAULT_MANIFEST_URL.into()),
        UpdateChannel::Beta => {
            let releases_url = std::env::var("ALEX_UPDATE_RELEASES_URL")
                .unwrap_or_else(|_| DEFAULT_RELEASES_URL.into());
            let raw = fetch_text(&releases_url).await?;
            let releases: Vec<GithubRelease> =
                serde_json::from_str(&raw).context("parsing GitHub releases list")?;
            let (tag, url) = select_release_manifest_url(&releases).with_context(|| {
                format!("no release with a manifest.json found via {releases_url}")
            })?;
            tracing::debug!("beta channel resolved release {tag}");
            Ok(url)
        }
    }
}

async fn load_manifest(update_channel: UpdateChannel) -> Result<Manifest> {
    let source = manifest_source(update_channel).await?;
    let raw = fetch_text(&source)
        .await
        .context("loading update manifest")?;
    let manifest: Manifest = serde_json::from_str(&raw).context("parsing update manifest")?;
    if manifest.schema_version != 1 {
        anyhow::bail!(
            "unsupported update manifest schema_version {} (expected 1)",
            manifest.schema_version
        );
    }
    Ok(manifest)
}

pub async fn check(channel: Channel, update_channel: UpdateChannel) -> Result<UpdateCheck> {
    let manifest = load_manifest(update_channel).await?;
    let key = platform_key()?;
    let asset = manifest
        .components
        .cli
        .platforms
        .get(key)
        .cloned()
        .with_context(|| {
            format!(
                "update manifest has no CLI asset for platform '{key}' (available: {})",
                manifest
                    .components
                    .cli
                    .platforms
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?;
    let current = env!("CARGO_PKG_VERSION").to_string();
    let latest = manifest.components.cli.version;
    let update_available = match compare_versions(&current, &latest) {
        Some(Ordering::Greater) => true,
        Some(_) => false,
        None => {
            eprintln!(
                "warning: could not parse update version(s): current={current}, latest={latest}; treating as up to date"
            );
            false
        }
    };
    Ok(UpdateCheck {
        current,
        latest,
        update_available,
        notes_url: manifest.components.cli.notes_url,
        channel,
        update_channel,
        asset,
    })
}

pub async fn daemon_update_status_value(update_channel: UpdateChannel) -> Result<serde_json::Value> {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    let channel = std::env::current_exe()
        .ok()
        .and_then(|p| p.canonicalize().ok())
        .map(|p| install_channel(&p, &home))
        .unwrap_or(Channel::Standalone);
    let update = check(channel, update_channel).await?;
    Ok(json!({
        "current": update.current,
        "latest": update.latest,
        "update_available": update.update_available,
        "notes_url": update.notes_url,
        "update_channel": update.update_channel.as_str(),
        "checked_at_ms": now_ms(),
    }))
}

pub(crate) enum DaemonUpdateApplyError {
    Conflict(serde_json::Value),
    Failed(anyhow::Error),
}

fn managed_reason(channel: Channel) -> Option<&'static str> {
    match channel {
        Channel::Brew => Some("alex is managed by Homebrew - run `brew upgrade alex`"),
        Channel::Cargo => Some("alex is managed by Cargo - run `cargo install alex --force`"),
        Channel::Standalone => None,
    }
}

fn update_body(update: &UpdateCheck, applying: bool) -> serde_json::Value {
    json!({
        "applying": applying,
        "current": update.current,
        "latest": update.latest,
        "update_available": update.update_available,
        "notes_url": update.notes_url,
        "update_channel": update.update_channel.as_str(),
    })
}

pub(crate) async fn daemon_apply_update(
    config: Config,
) -> std::result::Result<serde_json::Value, DaemonUpdateApplyError> {
    let exe = std::env::current_exe()
        .context("resolving current executable")
        .and_then(|p| {
            p.canonicalize()
                .context("canonicalizing current executable")
        })
        .map_err(DaemonUpdateApplyError::Failed)?;
    let home = dirs::home_dir()
        .context("no home directory")
        .map_err(DaemonUpdateApplyError::Failed)?;
    let channel = install_channel(&exe, &home);
    let update = check(channel, config.update_channel())
        .await
        .map_err(DaemonUpdateApplyError::Failed)?;

    if !update.update_available {
        return Ok(update_body(&update, false));
    }
    if let Some(reason) = managed_reason(channel) {
        let mut body = update_body(&update, false);
        if let Some(obj) = body.as_object_mut() {
            obj.insert("reason".into(), json!(reason));
        }
        return Err(DaemonUpdateApplyError::Conflict(body));
    }

    #[cfg(unix)]
    {
        let task_update = update.clone();
        let task_exe = exe.clone();
        tokio::spawn(async move {
            let result = async {
                install_unix(&task_exe, &task_update).await?;
                restart_daemon(&config, &task_exe).await
            }
            .await;
            if let Err(e) = result {
                tracing::error!("daemon self-update failed: {e:#}");
            }
        });
        Ok(update_body(&update, true))
    }

    #[cfg(not(unix))]
    {
        let mut body = update_body(&update, false);
        if let Some(obj) = body.as_object_mut() {
            obj.insert(
                "reason".into(),
                json!("self-update is not available for this platform"),
            );
        }
        Err(DaemonUpdateApplyError::Conflict(body))
    }
}

pub async fn run_update(
    config: &Config,
    check_only: bool,
    yes: bool,
    no_restart: bool,
    json_output: bool,
    force: bool,
    update_channel: UpdateChannel,
) -> Result<()> {
    let exe = std::env::current_exe()
        .context("resolving current executable")?
        .canonicalize()
        .context("canonicalizing current executable")?;
    let home = dirs::home_dir().context("no home directory")?;
    let channel = install_channel(&exe, &home);
    let update = check(channel, update_channel).await?;

    if check_only {
        print_check(&update, json_output)?;
        return Ok(());
    }

    if !update.update_available {
        println!("alex {} is up to date", update.current);
        return Ok(());
    }

    if !force {
        match channel {
            Channel::Brew => {
                println!("alex is managed by Homebrew — run `brew upgrade alex`");
                return Ok(());
            }
            Channel::Cargo => {
                println!("alex is managed by Cargo — run `cargo install alex --force`");
                return Ok(());
            }
            Channel::Standalone => {}
        }
    }

    #[cfg(windows)]
    {
        println!(
            "self-update not yet supported on Windows — download {}",
            update.asset.url
        );
        std::process::exit(1);
    }

    #[cfg(unix)]
    {
        if !yes && !confirm(&update)? {
            println!("cancelled");
            return Ok(());
        }
        install_unix(&exe, &update).await?;
        if no_restart {
            println!("daemon restart skipped (--no-restart)");
        } else {
            restart_daemon(config, &exe).await?;
        }
        Ok(())
    }
}

fn print_check(update: &UpdateCheck, json_output: bool) -> Result<()> {
    if json_output {
        println!(
            "{}",
            serde_json::to_string(&json!({
                "current": update.current,
                "latest": update.latest,
                "update_available": update.update_available,
                "notes_url": update.notes_url,
                "channel": update.channel.as_str(),
                "update_channel": update.update_channel.as_str(),
            }))?
        );
    } else {
        let suffix = match update.update_channel {
            UpdateChannel::Stable => "",
            UpdateChannel::Beta => " [beta channel]",
        };
        if update.update_available {
            if let Some(notes) = &update.notes_url {
                println!(
                    "alex {} → {} available{suffix} (notes: {notes})",
                    update.current, update.latest
                );
            } else {
                println!(
                    "alex {} → {} available{suffix}",
                    update.current, update.latest
                );
            }
        } else {
            println!("alex {} is up to date{suffix}", update.current);
        }
    }
    Ok(())
}

#[cfg(unix)]
fn confirm(update: &UpdateCheck) -> Result<bool> {
    print!("Update alex {} → {}? [y/N] ", update.current, update.latest);
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    Ok(matches!(answer.trim(), "y" | "Y" | "yes" | "YES" | "Yes"))
}

#[cfg(unix)]
async fn install_unix(exe: &Path, update: &UpdateCheck) -> Result<()> {
    let dir = exe
        .parent()
        .context("current executable has no parent directory")?;
    let pid = std::process::id();
    let archive_tmp = dir.join(format!(".alex-update.{pid}.tmp"));
    let extracted_tmp = dir.join(format!(".alex-update.{pid}.bin"));
    let _ = std::fs::remove_file(&archive_tmp);
    let _ = std::fs::remove_file(&extracted_tmp);

    println!(
        "downloading alex {} from {}",
        update.latest, update.asset.url
    );
    let digest = download_to(&update.asset.url, &archive_tmp)
        .await
        .with_context(|| {
            format!(
                "writing update into {}; if this directory is protected, run `sudo alex update` or use your package manager",
                dir.display()
            )
        })?;
    if !digest.eq_ignore_ascii_case(&update.asset.sha256) {
        let _ = std::fs::remove_file(&archive_tmp);
        anyhow::bail!(
            "download checksum mismatch: expected {}, got {}",
            update.asset.sha256,
            digest
        );
    }
    println!("verified sha256 {digest}");

    extract_alex(&archive_tmp, &extracted_tmp)?;
    chmod_755(&extracted_tmp)?;
    println!("replacing {}", exe.display());
    std::fs::rename(&extracted_tmp, exe).with_context(|| format!("replacing {}", exe.display()))?;

    let sibling = exe.with_file_name("alexandria");
    if sibling.exists() {
        let meta = std::fs::symlink_metadata(&sibling)?;
        if meta.file_type().is_symlink() {
            println!("leaving alexandria symlink unchanged");
        } else {
            let sibling_tmp = dir.join(format!(".alex-update.{pid}.alexandria"));
            let _ = std::fs::remove_file(&sibling_tmp);
            std::fs::copy(exe, &sibling_tmp)
                .with_context(|| format!("preparing {}", sibling.display()))?;
            chmod_755(&sibling_tmp)?;
            println!("replacing {}", sibling.display());
            std::fs::rename(&sibling_tmp, &sibling)
                .with_context(|| format!("replacing {}", sibling.display()))?;
        }
    }

    let _ = std::fs::remove_file(&archive_tmp);
    println!("alex updated to {}", update.latest);
    Ok(())
}

#[cfg(unix)]
async fn download_to(url: &str, dest: &Path) -> Result<String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(600))
        .build()?;
    let resp = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("downloading {url}"))?
        .error_for_status()
        .with_context(|| format!("downloading {url}"))?;
    let mut stream = resp.bytes_stream();
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dest)
        .await?;
    let mut hasher = Sha256::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        hasher.update(&chunk);
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await?;
    }
    tokio::io::AsyncWriteExt::flush(&mut file).await?;
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(unix)]
fn extract_alex(archive_path: &Path, out_path: &Path) -> Result<()> {
    let archive_file = std::fs::File::open(archive_path)?;
    let decoder = GzDecoder::new(archive_file);
    let mut archive = tar::Archive::new(decoder);
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?;
        if path.file_name().and_then(|n| n.to_str()) == Some("alex") {
            let mut out = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(out_path)?;
            io::copy(&mut entry, &mut out)?;
            return Ok(());
        }
    }
    anyhow::bail!("update archive did not contain an alex binary");
}

#[cfg(unix)]
fn chmod_755(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    Ok(())
}

fn check_host(config: &Config) -> &str {
    if config.host == "0.0.0.0" {
        "127.0.0.1"
    } else {
        config.host.as_str()
    }
}

async fn restart_daemon(config: &Config, exe: &Path) -> Result<()> {
    let health_url = format!("http://{}:{}/health", check_host(config), config.port);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()?;
    if client.get(&health_url).send().await.is_err() {
        println!("daemon not running; start it with `alex daemon --background`");
        return Ok(());
    }

    let state = detect_service_state();
    match state {
        ServiceState::LaunchdLoaded { .. } => {
            println!("daemon is launchd-managed; restarting with launchctl");
            let status = Command::new("launchctl")
                .args([
                    "kickstart",
                    "-k",
                    &format!("gui/{}/com.alexandria.daemon", current_uid()),
                ])
                .status()
                .context("running launchctl kickstart")?;
            if !status.success() {
                anyhow::bail!("launchctl kickstart failed");
            }
            println!("daemon restarted");
            Ok(())
        }
        ServiceState::Systemd { active: true, .. } => {
            println!("daemon is systemd-managed; restarting with systemctl");
            let status = Command::new("systemctl")
                .args(["--user", "restart", "alexandria"])
                .status()
                .context("running systemctl --user restart alexandria")?;
            if !status.success() {
                anyhow::bail!("systemctl --user restart alexandria failed");
            }
            println!("daemon restarted");
            Ok(())
        }
        _ => blue_green_restart(config, exe).await,
    }
}

#[cfg(unix)]
async fn blue_green_restart(config: &Config, exe: &Path) -> Result<()> {
    let old_pids = listener_pids(config.port)?;
    if old_pids.is_empty() {
        println!("no running daemon found; starting fresh");
    } else {
        println!("old daemon pid(s): {}", old_pids.join(" "));
    }

    let log_path = alexandria_home().join("daemon.log");
    let log_out = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("opening {}", log_path.display()))?;
    let log_err = log_out.try_clone()?;
    let mut child = Command::new(exe)
        .arg("daemon")
        .stdout(Stdio::from(log_out))
        .stderr(Stdio::from(log_err))
        .stdin(Stdio::null())
        .spawn()
        .with_context(|| format!("starting {} daemon", exe.display()))?;
    let new_pid = child.id();
    println!("started new daemon pid {new_pid}; waiting for /health");

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()?;
    let health_url = format!("http://{}:{}/health", check_host(config), config.port);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        if tokio::time::Instant::now() >= deadline {
            let _ = child.kill();
            anyhow::bail!(
                "new daemon did not become healthy within 60s; old daemon left running; see {}",
                log_path.display()
            );
        }
        if let Ok(Some(status)) = child.try_wait() {
            anyhow::bail!(
                "new daemon exited during startup with status {status}; old daemon left running; see {}",
                log_path.display()
            );
        }
        if let Ok(resp) = client.get(&health_url).send().await {
            if resp.status().is_success() {
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    if old_pids.is_empty() {
        println!("daemon healthy");
        return Ok(());
    }
    println!(
        "new daemon healthy; draining old daemon(s): {}",
        old_pids.join(" ")
    );
    for pid in old_pids {
        if pid == new_pid.to_string() {
            continue;
        }
        let _ = Command::new("kill").args(["-TERM", &pid]).status();
    }
    println!("daemon restarted; old instance drains in-flight requests then exits");
    Ok(())
}

#[cfg(not(unix))]
async fn blue_green_restart(_config: &Config, _exe: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn listener_pids(port: u16) -> Result<Vec<String>> {
    let out = match Command::new("lsof")
        .args(["-ti", &format!("tcp:{port}"), "-sTCP:LISTEN"])
        .output()
    {
        Ok(out) => out,
        Err(_) => {
            println!(
                "lsof not found — cannot discover the old daemon; restart it manually with `alex daemon --background`"
            );
            return Ok(Vec::new());
        }
    };
    if !out.status.success() && out.stdout.is_empty() {
        return Ok(Vec::new());
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_parse_tolerates_unknown_and_missing_app() {
        let raw = r#"{
          "schema_version": 1,
          "published_at": "2026-07-09T00:00:00Z",
          "ignored": true,
          "components": {
            "cli": {
              "version": "0.1.17",
              "notes_url": "https://example.test/notes",
              "extra": "ok",
              "platforms": {
                "x86_64-unknown-linux-gnu": {"url": "https://example.test/a.tgz", "sha256": "abc", "size": 123, "extra": 1}
              }
            }
          }
        }"#;
        let manifest: Manifest = serde_json::from_str(raw).unwrap();
        assert_eq!(manifest.schema_version, 1);
        assert_eq!(manifest.components.cli.version, "0.1.17");
        assert!(manifest.components.app.is_none());
        assert!(manifest
            .components
            .cli
            .platforms
            .contains_key("x86_64-unknown-linux-gnu"));
    }

    #[test]
    fn version_compare_cases() {
        assert_eq!(compare_versions("0.1.15", "0.1.15"), Some(Ordering::Equal));
        assert_eq!(compare_versions("0.1.15", "0.1.14"), Some(Ordering::Less));
        assert_eq!(
            compare_versions("0.1.15", "0.1.16"),
            Some(Ordering::Greater)
        );
        assert_eq!(
            compare_versions("0.1.15", "v0.1.16"),
            Some(Ordering::Greater)
        );
        assert_eq!(compare_versions("0.1.15", "garbage"), None);
    }

    #[test]
    fn version_compare_beta_cases() {
        // beta of the next version is newer than the current stable
        assert_eq!(
            compare_versions("0.1.23", "0.1.24-beta.1"),
            Some(Ordering::Greater)
        );
        // final release is newer than any of its betas
        assert_eq!(
            compare_versions("0.1.24-beta.2", "0.1.24"),
            Some(Ordering::Greater)
        );
        assert_eq!(
            compare_versions("0.1.24", "0.1.24-beta.2"),
            Some(Ordering::Less)
        );
        // later beta wins
        assert_eq!(
            compare_versions("0.1.24-beta.1", "0.1.24-beta.2"),
            Some(Ordering::Greater)
        );
        assert_eq!(
            compare_versions("0.1.24-beta.2", "v0.1.24-beta.2"),
            Some(Ordering::Equal)
        );
        // unknown prerelease labels are rejected, treated as unparseable
        assert_eq!(compare_versions("0.1.24", "0.1.25-rc.1"), None);
        assert_eq!(compare_versions("0.1.24", "0.1.25-beta"), None);
    }

    #[test]
    fn update_channel_parse_cases() {
        assert_eq!(UpdateChannel::parse("stable").unwrap(), UpdateChannel::Stable);
        assert_eq!(UpdateChannel::parse("").unwrap(), UpdateChannel::Stable);
        assert_eq!(UpdateChannel::parse("Beta").unwrap(), UpdateChannel::Beta);
        assert!(UpdateChannel::parse("nightly").is_err());
    }

    #[test]
    fn beta_release_selection_prefers_newest_including_stable() {
        let releases: Vec<GithubRelease> = serde_json::from_str(
            r#"[
              {"tag_name": "v0.1.24-beta.1", "draft": false, "prerelease": true,
               "assets": [{"name": "manifest.json", "browser_download_url": "https://example.test/beta1/manifest.json"}]},
              {"tag_name": "v0.1.24-beta.2", "draft": false, "prerelease": true,
               "assets": [{"name": "manifest.json", "browser_download_url": "https://example.test/beta2/manifest.json"}]},
              {"tag_name": "v0.1.23", "draft": false, "prerelease": false,
               "assets": [{"name": "manifest.json", "browser_download_url": "https://example.test/stable/manifest.json"}]},
              {"tag_name": "v0.1.25-beta.1", "draft": true, "prerelease": true,
               "assets": [{"name": "manifest.json", "browser_download_url": "https://example.test/draft/manifest.json"}]},
              {"tag_name": "v0.1.26-beta.1", "draft": false, "prerelease": true, "assets": []}
            ]"#,
        )
        .unwrap();
        let (tag, url) = select_release_manifest_url(&releases).unwrap();
        // beta.2 wins: drafts are skipped, releases without manifest.json are skipped
        assert_eq!(tag, "v0.1.24-beta.2");
        assert_eq!(url, "https://example.test/beta2/manifest.json");

        let with_final: Vec<GithubRelease> = serde_json::from_str(
            r#"[
              {"tag_name": "v0.1.24-beta.2", "draft": false, "prerelease": true,
               "assets": [{"name": "manifest.json", "browser_download_url": "https://example.test/beta2/manifest.json"}]},
              {"tag_name": "v0.1.24", "draft": false, "prerelease": false,
               "assets": [{"name": "manifest.json", "browser_download_url": "https://example.test/final/manifest.json"}]}
            ]"#,
        )
        .unwrap();
        let (tag, url) = select_release_manifest_url(&with_final).unwrap();
        assert_eq!(tag, "v0.1.24");
        assert_eq!(url, "https://example.test/final/manifest.json");
    }

    #[test]
    fn install_channel_cases() {
        let home = Path::new("/Users/tester");
        assert_eq!(
            install_channel(Path::new("/opt/homebrew/bin/alex"), home),
            Channel::Brew
        );
        assert_eq!(
            install_channel(Path::new("/usr/local/Cellar/alex/0.1.15/bin/alex"), home),
            Channel::Brew
        );
        assert_eq!(
            install_channel(Path::new("/Users/tester/.cargo/bin/alex"), home),
            Channel::Cargo
        );
        assert_eq!(
            install_channel(Path::new("/usr/local/bin/alex"), home),
            Channel::Standalone
        );
    }

    #[test]
    fn platform_key_is_known() {
        let key = platform_key().unwrap();
        assert!(!key.is_empty());
        assert!([
            "aarch64-apple-darwin",
            "x86_64-apple-darwin",
            "x86_64-unknown-linux-gnu",
            "aarch64-unknown-linux-gnu",
            "x86_64-pc-windows-msvc",
        ]
        .contains(&key));
    }
}
