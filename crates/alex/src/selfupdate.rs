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

use crate::{alex_home, detect_service_state, Config, ServiceState};

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

    /// The channel a build should follow when the user has not explicitly
    /// chosen one (B2). A build whose own version is a pre-release
    /// (`-beta`/`-rc`/`-alpha`) defaults to beta, so it actually checks the
    /// beta feed instead of comparing against the older latest *stable* and
    /// falsely reporting "up to date". A stable build defaults to stable.
    pub fn default_for_version(version: &str) -> Self {
        match parse_version(version) {
            Some(v) if !v.is_stable() => UpdateChannel::Beta,
            _ => UpdateChannel::Stable,
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
    /// The backstop verdict (B5). When `Unconfirmed` we offer the resolved
    /// latest but never claim the user is on it.
    pub(crate) decision: UpdateDecision,
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
    } else if cfg!(all(target_os = "windows", target_arch = "aarch64")) {
        Ok("aarch64-pc-windows-msvc")
    } else {
        anyhow::bail!(
            "self-update is not available for this platform (os={}, arch={})",
            std::env::consts::OS,
            std::env::consts::ARCH
        );
    }
}

/// Ordered pre-release stages. Unknown labels sort below the recognized ones,
/// and a stable release outranks every pre-release, so `0.1.24-beta.2` <
/// `0.1.24` and `0.1.24-alpha.9` < `0.1.24-beta.1` < `0.1.24-rc.1` < `0.1.24`.
const STAGE_UNKNOWN_PRE: u8 = 0;
const STAGE_ALPHA: u8 = 1;
const STAGE_BETA: u8 = 2;
const STAGE_RC: u8 = 3;
const STAGE_STABLE: u8 = 4;

/// A parsed, orderable version. `release` is the dotted numeric core with
/// trailing zeros trimmed (so `0.1.0` == `0.1`); `stage`/`pre_num` order the
/// pre-release suffix. Deriving `Ord` compares `release`, then `stage`, then
/// `pre_num` — exactly the precedence we want (base dominates; a final release
/// beats its betas; a higher beta number is newer).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ParsedVersion {
    release: Vec<u64>,
    stage: u8,
    pre_num: u64,
}

impl ParsedVersion {
    fn is_stable(&self) -> bool {
        self.stage == STAGE_STABLE
    }
}

/// Trim surrounding whitespace and a leading `v`/`V` so `v0.1.28` and
/// `0.1.28 ` are recognized as the same tag.
fn normalize_tag(version: &str) -> &str {
    let trimmed = version.trim();
    trimmed
        .strip_prefix('v')
        .or_else(|| trimmed.strip_prefix('V'))
        .unwrap_or(trimmed)
}

/// The first contiguous run of ASCII digits in `s`, or 0 when there is none.
/// Lets `beta.10`, `beta10`, and `beta.3-dirty` all yield their number.
fn first_number(s: &str) -> u64 {
    let digits: String = s
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().unwrap_or(0)
}

/// Robust version parser (B1). Tolerates a leading `v`, ignores `+build`
/// metadata, accepts `-alpha`/`-beta`/`-rc` with or without a number and with
/// trailing junk (`-beta.3-dirty`), and accepts extra dotted numeric
/// components (`0.1.2.3`). Returns `None` only when the numeric core is
/// genuinely absent or non-numeric (e.g. `garbage`, ``). A `None` here never
/// becomes a false "up to date": `decide_update` treats an unparseable side
/// whose tag differs as an unconfirmable difference, not as "latest".
fn parse_version(version: &str) -> Option<ParsedVersion> {
    let core = normalize_tag(version);
    // Drop build metadata: everything from the first '+'.
    let core = core.split('+').next().unwrap_or(core);
    let (base, pre) = match core.split_once('-') {
        Some((base, pre)) => (base, Some(pre)),
        None => (core, None),
    };
    if base.is_empty() {
        return None;
    }
    let mut release = Vec::new();
    for part in base.split('.') {
        release.push(part.parse::<u64>().ok()?);
    }
    // Trim trailing zeros so 0.1.0 == 0.1 and 0.1.24 == 0.1.24.0.
    while release.len() > 1 && *release.last().unwrap() == 0 {
        release.pop();
    }
    let (stage, pre_num) = match pre {
        None => (STAGE_STABLE, 0),
        Some(pre) => {
            let low = pre.to_ascii_lowercase();
            let stage = if low.starts_with("rc") {
                STAGE_RC
            } else if low.starts_with("beta") {
                STAGE_BETA
            } else if low.starts_with("alpha") {
                STAGE_ALPHA
            } else {
                STAGE_UNKNOWN_PRE
            };
            (stage, first_number(pre))
        }
    };
    Some(ParsedVersion {
        release,
        stage,
        pre_num,
    })
}

fn compare_versions(current: &str, latest: &str) -> Option<Ordering> {
    Some(parse_version(latest)?.cmp(&parse_version(current)?))
}

/// The conclusion of an update check. `Unconfirmed` is the user-trust backstop
/// (B5): whenever we cannot *prove* the running tag is the newest, we must not
/// claim "you're on the latest".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UpdateDecision {
    /// A strictly newer version is available.
    Available,
    /// Confirmed newest: identical tags, or the running build is strictly
    /// newer than the resolved latest (a local/dev build ahead of the
    /// channel — no newer tag exists).
    UpToDate,
    /// The tags differ but the comparator cannot order them (one side is
    /// unparseable). Never reported as "latest"; surfaced as an offer/flag.
    Unconfirmed,
}

impl UpdateDecision {
    /// Whether an update should be offered. Both a real newer version and an
    /// unconfirmable difference are offered — the alternative (staying silent)
    /// is the exact false "up to date" the user rejected.
    pub(crate) fn update_available(self) -> bool {
        matches!(
            self,
            UpdateDecision::Available | UpdateDecision::Unconfirmed
        )
    }
}

/// The hard backstop (B5). Decide whether `latest` supersedes `current`, never
/// returning `UpToDate` when the two tags differ and cannot be ordered.
pub(crate) fn decide_update(current: &str, latest: &str) -> UpdateDecision {
    if normalize_tag(current) == normalize_tag(latest) {
        return UpdateDecision::UpToDate;
    }
    match compare_versions(current, latest) {
        Some(Ordering::Greater) => UpdateDecision::Available,
        // Equal despite differing text (e.g. only `+build` metadata differs),
        // or the running build is strictly newer than the channel's latest.
        Some(Ordering::Equal) | Some(Ordering::Less) => UpdateDecision::UpToDate,
        // Tags differ and cannot be ordered: never claim "latest".
        None => UpdateDecision::Unconfirmed,
    }
}

/// True when a daemon reporting `reported` on /health is serving the intended
/// `target` build (post-update verification, B3). Uses the same comparator so
/// a `v`-prefix or `+build` metadata never causes a spurious mismatch.
fn version_matches(reported: &str, target: &str) -> bool {
    if normalize_tag(reported) == normalize_tag(target) {
        return true;
    }
    matches!(compare_versions(reported, target), Some(Ordering::Equal))
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

/// Pick the newest release carrying a manifest.json that `channel` is allowed
/// to install. Beta users also see the final stable (so they roll onto it once
/// it ships); a stable user is NEVER offered a pre-release, even when a beta is
/// the newest tag on GitHub. Draft releases and releases without a manifest are
/// skipped.
fn select_release_for_channel(
    releases: &[GithubRelease],
    channel: UpdateChannel,
) -> Option<(String, String)> {
    releases
        .iter()
        .filter(|r| !r.draft)
        .filter_map(|r| {
            let version = parse_version(&r.tag_name)?;
            if channel == UpdateChannel::Stable && !version.is_stable() {
                return None;
            }
            let manifest = r.assets.iter().find(|a| a.name == "manifest.json")?;
            Some((
                version,
                r.tag_name.clone(),
                manifest.browser_download_url.clone(),
            ))
        })
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, tag, url)| (tag, url))
}

/// The beta channel's release selection: newest of stable-or-beta, so a beta
/// user is offered the final release once it ships.
fn select_release_manifest_url(releases: &[GithubRelease]) -> Option<(String, String)> {
    select_release_for_channel(releases, UpdateChannel::Beta)
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
    let decision = decide_update(&current, &latest);
    if decision == UpdateDecision::Unconfirmed {
        // B5: we could not order the tags, but they differ. Never claim the
        // user is on the latest — offer/flag GitHub's newest for the channel.
        eprintln!(
            "warning: could not confirm alex {current} is the latest; GitHub's newest for this channel is {latest}. Offering it rather than claiming you are up to date."
        );
    }
    let update_available = decision.update_available();
    Ok(UpdateCheck {
        current,
        latest,
        update_available,
        decision,
        notes_url: manifest.components.cli.notes_url,
        channel,
        update_channel,
        asset,
    })
}

pub async fn daemon_update_status_value(
    update_channel: UpdateChannel,
) -> Result<serde_json::Value> {
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
        // B5: false when the tags differ but could not be ordered, so a UI can
        // avoid a confident "you're on the latest".
        "confirmed": update.decision != UpdateDecision::Unconfirmed,
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
        "confirmed": update.decision != UpdateDecision::Unconfirmed,
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
                restart_daemon(&config, &task_exe, &task_update.latest).await
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
            restart_daemon(config, &exe, &update.latest).await?;
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
                "confirmed": update.decision != UpdateDecision::Unconfirmed,
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
        if update.decision == UpdateDecision::Unconfirmed {
            // B5: we cannot prove the current build is newest, and the tags
            // differ — do NOT print "up to date".
            println!(
                "alex {}: could not confirm you are on the latest{suffix}; GitHub's newest for this channel is {}",
                update.current, update.latest
            );
        } else if update.update_available {
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

    let sibling = exe.with_file_name("alex");
    if sibling.exists() {
        let meta = std::fs::symlink_metadata(&sibling)?;
        if meta.file_type().is_symlink() {
            println!("leaving alex symlink unchanged");
        } else {
            let sibling_tmp = dir.join(format!(".alex-update.{pid}.alex"));
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

/// Poll a daemon health endpoint until it is ready.  Kept separate from a
/// particular supervisor so both the standalone blue/green path and launchd
/// can use the same readiness rule.
pub(crate) async fn wait_for_daemon_health(
    client: &reqwest::Client,
    health_url: &str,
    timeout: Duration,
) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if let Ok(response) = client.get(health_url).send().await {
            if response.status().is_success() {
                return true;
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

async fn restart_daemon(config: &Config, exe: &Path, target_version: &str) -> Result<()> {
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
            println!("daemon is launchd-managed; starting graceful restart helper");
            // This task runs inside the old daemon.  It cannot wait for its
            // own graceful exit, so the helper owns the post-drain health
            // check and hard-restart fallback. The helper runs from the new
            // binary and verifies the served version there (B3).
            crate::spawn_launchd_restart_helper(exe)
        }
        ServiceState::Systemd { active: true, .. } => {
            println!("daemon is systemd-managed; restarting with systemctl");
            let status = Command::new("systemctl")
                .args(["--user", "restart", "alex"])
                .status()
                .context("running systemctl --user restart alex")?;
            if !status.success() {
                anyhow::bail!("systemctl --user restart alex failed");
            }
            // B3: don't trust the exit code alone — confirm the daemon now
            // answering /health is actually the new build.
            if wait_for_daemon_health(&client, &health_url, Duration::from_secs(30)).await {
                verify_served_version(&client, &health_url, target_version).await?;
                println!("daemon restarted; verified it is now serving {target_version}");
            } else {
                anyhow::bail!("systemd restarted alex but it did not become healthy in time");
            }
            Ok(())
        }
        _ => blue_green_restart(config, exe, target_version).await,
    }
}

/// Poll `/health` and confirm the daemon is serving `target_version` (B3).
/// The new daemon is already healthy by the time this is called, so a short
/// retry only absorbs a stray old daemon still answering during the drain
/// window; a persistent mismatch is a real failure (a stray daemon owns the
/// port), surfaced with a clear, actionable error.
async fn verify_served_version(
    client: &reqwest::Client,
    health_url: &str,
    target_version: &str,
) -> Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut last_reported = String::new();
    loop {
        match fetch_health_version(client, health_url).await {
            Ok(reported) => {
                if version_matches(&reported, target_version) {
                    return Ok(());
                }
                last_reported = reported;
            }
            Err(error) => {
                if tokio::time::Instant::now() >= deadline {
                    return Err(error);
                }
            }
        }
        if tokio::time::Instant::now() >= deadline {
            anyhow::bail!(
                "post-update check: daemon is serving {last_reported}, expected {target_version}; \
                 a stray daemon may still own the port — stop it and restart with `alex daemon --background`"
            );
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

async fn fetch_health_version(client: &reqwest::Client, health_url: &str) -> Result<String> {
    let health: serde_json::Value = client
        .get(health_url)
        .send()
        .await
        .context("querying /health for post-update version check")?
        .error_for_status()
        .context("querying /health for post-update version check")?
        .json()
        .await
        .context("parsing /health for post-update version check")?;
    Ok(health
        .get("version")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string())
}

#[cfg(unix)]
async fn blue_green_restart(config: &Config, exe: &Path, target_version: &str) -> Result<()> {
    let old_pids = listener_pids(config.port)?;
    if old_pids.is_empty() {
        println!("no running daemon found; starting fresh");
    } else {
        println!("old daemon pid(s): {}", old_pids.join(" "));
    }

    let log_path = alex_home().join("daemon.log");
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
        if wait_for_daemon_health(&client, &health_url, Duration::ZERO).await {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    if old_pids.is_empty() {
        println!("daemon healthy");
    } else {
        println!(
            "new daemon healthy; draining old daemon(s): {}",
            old_pids.join(" ")
        );
    }

    // Reclaim the port (B6). TERM every daemon still listening that is NOT the
    // new one — including stray, manually-started daemons that co-bound via
    // SO_REUSEPORT and were never in `old_pids` — then KILL any straggler that
    // ignores TERM. If one survived it would keep serving old code and the
    // post-update check below would (correctly) fail.
    reclaim_port(config.port, &new_pid.to_string()).await;

    // Post-update verification (B3): confirm the port is now owned by the new
    // build, not a survivor.
    verify_served_version(&client, &health_url, target_version).await?;
    println!("daemon restarted; verified it is now serving {target_version}");
    Ok(())
}

/// TERM, then KILL, every listener on `port` except `keep_pid` (B6).
#[cfg(unix)]
async fn reclaim_port(port: u16, keep_pid: &str) {
    let listeners = listener_pids(port).unwrap_or_default();
    for pid in pids_to_reclaim(&listeners, keep_pid) {
        let _ = Command::new("kill").args(["-TERM", &pid]).status();
    }
    // Give graceful shutdown a moment, then force-close anything still bound.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let stragglers = pids_to_reclaim(&listener_pids(port).unwrap_or_default(), keep_pid);
    for pid in &stragglers {
        let _ = Command::new("kill").args(["-KILL", pid]).status();
    }
}

/// The listeners that must be reclaimed: everything except the daemon we just
/// started. Pure so the "never kill the new daemon" rule is unit-tested.
#[cfg(unix)]
fn pids_to_reclaim(listeners: &[String], keep_pid: &str) -> Vec<String> {
    listeners
        .iter()
        .filter(|pid| pid.as_str() != keep_pid)
        .cloned()
        .collect()
}

#[cfg(not(unix))]
async fn blue_green_restart(_config: &Config, _exe: &Path, _target_version: &str) -> Result<()> {
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

    #[tokio::test]
    async fn health_poll_times_out_when_a_new_daemon_never_becomes_ready() {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(10))
            .build()
            .unwrap();
        assert!(
            !wait_for_daemon_health(&client, "http://127.0.0.1:9/health", Duration::ZERO).await
        );
    }

    #[tokio::test]
    async fn health_poll_waits_until_the_new_daemon_is_ready() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 512];
            let _ = stream.read(&mut request).await.unwrap();
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                .await
                .unwrap();
        });
        let client = reqwest::Client::new();
        assert!(
            wait_for_daemon_health(
                &client,
                &format!("http://{address}/health"),
                Duration::from_secs(1),
            )
            .await
        );
    }

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
        // B1 fix: these two previously asserted `None` (the parser bug). `rc.1`
        // and a number-less `-beta` are now parsed, and both are a NEWER base
        // (0.1.25 > 0.1.24), so they must be offered — never a false "up to
        // date". The genuinely-unparseable case (`garbage`) still returns None.
        assert_eq!(
            compare_versions("0.1.24", "0.1.25-rc.1"),
            Some(Ordering::Greater)
        );
        assert_eq!(
            compare_versions("0.1.24", "0.1.25-beta"),
            Some(Ordering::Greater)
        );
    }

    #[test]
    fn update_channel_parse_cases() {
        assert_eq!(
            UpdateChannel::parse("stable").unwrap(),
            UpdateChannel::Stable
        );
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

        // Two-digit prerelease vs single-digit: beta.10 must beat beta.9. A string
        // compare would pick beta.9, and GitHub does not return the list newest-first,
        // so order cannot be trusted -- this is exactly what bit the installer.
        let double_digit: Vec<GithubRelease> = serde_json::from_str(
            r#"[
              {"tag_name": "v0.1.26-beta.10", "draft": false, "prerelease": true,
               "assets": [{"name": "manifest.json", "browser_download_url": "https://example.test/b10/manifest.json"}]},
              {"tag_name": "v0.1.26-beta.9", "draft": false, "prerelease": true,
               "assets": [{"name": "manifest.json", "browser_download_url": "https://example.test/b9/manifest.json"}]}
            ]"#,
        )
        .unwrap();
        let (tag, _) = select_release_manifest_url(&double_digit).unwrap();
        assert_eq!(tag, "v0.1.26-beta.10");
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
            "aarch64-pc-windows-msvc",
        ]
        .contains(&key));
    }

    // ----------------------------------------------------------------------
    // B1 — robust parser: none of these malformed-but-real tags may be lost,
    // and none may collapse to a false "up to date".
    // ----------------------------------------------------------------------

    #[test]
    fn parse_version_handles_malformed_but_real_tags() {
        // rc / alpha / beta suffixes all parse and order below stable.
        assert!(parse_version("0.1.25-rc.1").is_some());
        assert!(parse_version("0.1.25-alpha.2").is_some());
        // `-beta` with no number parses (number defaults to 0).
        let beta_none = parse_version("0.1.25-beta").unwrap();
        assert_eq!(beta_none.stage, STAGE_BETA);
        assert_eq!(beta_none.pre_num, 0);
        assert_eq!(parse_version("0.1.25-beta.0"), Some(beta_none));

        // `+build` metadata is ignored: a build-stamped stable equals the plain
        // stable, and stays a stable release.
        let plain = parse_version("0.1.28").unwrap();
        assert_eq!(parse_version("0.1.28+build.9"), Some(plain.clone()));
        assert!(parse_version("0.1.28+build.9").unwrap().is_stable());

        // Trailing junk after the pre-release number is tolerated.
        assert_eq!(
            parse_version("0.1.24-beta.3-dirty"),
            parse_version("0.1.24-beta.3")
        );

        // Extra dotted numeric components parse and order after the shorter tag.
        assert!(parse_version("0.1.2.3").is_some());
        assert_eq!(
            compare_versions("0.1.2", "0.1.2.3"),
            Some(Ordering::Greater)
        );
        // Trailing zeros are equivalent (0.1.0 == 0.1 == 0.1.0.0).
        assert_eq!(parse_version("0.1.0"), parse_version("0.1"));
        assert_eq!(parse_version("0.1.24.0"), parse_version("0.1.24"));

        // Leading v/V tolerated.
        assert_eq!(parse_version("v0.1.28"), parse_version("0.1.28"));
        assert_eq!(parse_version("V0.1.28"), parse_version("0.1.28"));

        // Genuinely unparseable → None (the decision layer, not the parser,
        // turns this into a safe "unable to confirm").
        assert!(parse_version("garbage").is_none());
        assert!(parse_version("").is_none());
        assert!(parse_version("x.y.z").is_none());
        assert!(parse_version("0..1").is_none());
    }

    #[test]
    fn pre_release_stage_ordering_alpha_beta_rc_stable() {
        // alpha < beta < rc < stable, all on the same base.
        assert_eq!(
            compare_versions("0.1.24-alpha.9", "0.1.24-beta.1"),
            Some(Ordering::Greater)
        );
        assert_eq!(
            compare_versions("0.1.24-beta.9", "0.1.24-rc.1"),
            Some(Ordering::Greater)
        );
        assert_eq!(
            compare_versions("0.1.24-rc.9", "0.1.24"),
            Some(Ordering::Greater)
        );
        // A higher base always wins regardless of pre-release stage.
        assert_eq!(
            compare_versions("0.1.24", "0.1.25-alpha.1"),
            Some(Ordering::Greater)
        );
    }

    // ----------------------------------------------------------------------
    // B5 — the hard backstop: differing tags that cannot be ordered must never
    // be reported as "up to date".
    // ----------------------------------------------------------------------

    #[test]
    fn decide_update_backstop_never_false_latest() {
        // Identical tag (incl. v-prefix / build metadata) → confirmed latest.
        assert_eq!(decide_update("0.1.28", "0.1.28"), UpdateDecision::UpToDate);
        assert_eq!(decide_update("v0.1.28", "0.1.28"), UpdateDecision::UpToDate);
        assert_eq!(
            decide_update("0.1.28", "0.1.28+build.5"),
            UpdateDecision::UpToDate
        );

        // A strictly newer latest → available.
        assert_eq!(decide_update("0.1.28", "0.1.29"), UpdateDecision::Available);
        // Row-8 shape: an rc tag is the newest → offered, not a false latest.
        assert_eq!(
            decide_update("0.1.28", "0.1.29-rc.1"),
            UpdateDecision::Available
        );

        // Running build strictly newer than the channel head (dev/ahead) →
        // up to date (no newer tag exists), which does not violate the rule.
        assert_eq!(decide_update("0.1.29", "0.1.28"), UpdateDecision::UpToDate);

        // THE user-trust cases: tags differ but one side is unparseable. Must
        // NOT be UpToDate — offered/flagged instead.
        assert_eq!(
            decide_update("0.1.28", "garbage-2026"),
            UpdateDecision::Unconfirmed
        );
        assert_eq!(
            decide_update("garbage", "0.1.28"),
            UpdateDecision::Unconfirmed
        );
        assert!(decide_update("0.1.28", "garbage-2026").update_available());
        // And crucially: never claims the user is on the latest.
        assert_ne!(
            decide_update("0.1.28", "garbage-2026"),
            UpdateDecision::UpToDate
        );
    }

    // ----------------------------------------------------------------------
    // Channel-semantics matrix (spec rows 1-8), encoded end-to-end:
    // resolve the channel's latest, then decide.
    // ----------------------------------------------------------------------

    /// Mirror of select_release_for_channel over two published heads: stable
    /// sees only stable; beta sees the newer of stable/beta (rolls onto final).
    fn resolve_latest(channel: UpdateChannel, latest_stable: &str, latest_beta: &str) -> String {
        match channel {
            UpdateChannel::Stable => latest_stable.to_string(),
            UpdateChannel::Beta => match compare_versions(latest_stable, latest_beta) {
                Some(Ordering::Greater) => latest_beta.to_string(),
                _ => latest_stable.to_string(),
            },
        }
    }

    #[test]
    fn channel_semantics_matrix() {
        struct Row {
            current: &'static str,
            channel: UpdateChannel,
            latest_stable: &'static str,
            latest_beta: &'static str,
            expect_available: bool,
            expect_latest: &'static str,
        }

        // Row 4's channel is DERIVED from the (beta) build version per B2 — a
        // beta build must not default to the stable channel.
        let row4_channel = UpdateChannel::default_for_version("0.1.28-beta.2");
        assert_eq!(
            row4_channel,
            UpdateChannel::Beta,
            "B2: beta build → beta channel"
        );

        let rows = [
            // 1: stable build, stable channel → stable update, NOT the beta.
            Row {
                current: "0.1.27",
                channel: UpdateChannel::Stable,
                latest_stable: "0.1.28",
                latest_beta: "0.1.29-beta.1",
                expect_available: true,
                expect_latest: "0.1.28",
            },
            // 2: stable build, beta channel → the newer beta.
            Row {
                current: "0.1.27",
                channel: UpdateChannel::Beta,
                latest_stable: "0.1.28",
                latest_beta: "0.1.29-beta.1",
                expect_available: true,
                expect_latest: "0.1.29-beta.1",
            },
            // 3: beta build, beta channel → newer beta.
            Row {
                current: "0.1.28-beta.2",
                channel: UpdateChannel::Beta,
                latest_stable: "0.1.27",
                latest_beta: "0.1.28-beta.3",
                expect_available: true,
                expect_latest: "0.1.28-beta.3",
            },
            // 4: beta build, channel defaulted (→ beta per B2) → newer beta, NOT "up to date".
            Row {
                current: "0.1.28-beta.2",
                channel: row4_channel,
                latest_stable: "0.1.27",
                latest_beta: "0.1.28-beta.3",
                expect_available: true,
                expect_latest: "0.1.28-beta.3",
            },
            // 5: beta build, beta channel, final shipped → roll onto the final (final > its betas).
            Row {
                current: "0.1.28-beta.3",
                channel: UpdateChannel::Beta,
                latest_stable: "0.1.28",
                latest_beta: "0.1.28-beta.3",
                expect_available: true,
                expect_latest: "0.1.28",
            },
            // 6: stable build, stable channel, same version → up to date.
            Row {
                current: "0.1.28",
                channel: UpdateChannel::Stable,
                latest_stable: "0.1.28",
                latest_beta: "0.1.28-beta.3",
                expect_available: false,
                expect_latest: "0.1.28",
            },
            // 7: beta build, beta channel, same beta → up to date.
            Row {
                current: "0.1.28-beta.3",
                channel: UpdateChannel::Beta,
                latest_stable: "0.1.27",
                latest_beta: "0.1.28-beta.3",
                expect_available: false,
                expect_latest: "0.1.28-beta.3",
            },
        ];

        for (i, row) in rows.iter().enumerate() {
            let n = i + 1;
            let latest = resolve_latest(row.channel, row.latest_stable, row.latest_beta);
            assert_eq!(
                latest, row.expect_latest,
                "row {n}: resolved latest for {:?}",
                row.channel
            );
            let decision = decide_update(row.current, &latest);
            assert_eq!(
                decision.update_available(),
                row.expect_available,
                "row {n}: update_available for current={} latest={latest} decision={decision:?}",
                row.current
            );
            // No matrix row may ever land on Unconfirmed — they are all
            // well-formed tags — so any "available" here is a real offer.
            assert_ne!(decision, UpdateDecision::Unconfirmed, "row {n}");
        }

        // Row 8: the newest tag on GitHub is an rc. Whatever the current build,
        // it must be offered/flagged, never a false "up to date".
        let decision = decide_update("0.1.28", "0.1.29-rc.1");
        assert!(decision.update_available(), "row 8: rc must be offered");
        assert_ne!(decision, UpdateDecision::UpToDate, "row 8");
    }

    #[test]
    fn stable_channel_is_never_offered_a_beta() {
        // Even when a beta is by far the newest head, a stable-channel resolve
        // returns the stable head, so the decision is "up to date".
        let latest = resolve_latest(UpdateChannel::Stable, "0.1.28", "0.1.29-beta.5");
        assert_eq!(latest, "0.1.28");
        assert_eq!(decide_update("0.1.28", &latest), UpdateDecision::UpToDate);

        // select_release_for_channel filters the same way: stable skips every
        // pre-release even when it is the newest tag present.
        let releases: Vec<GithubRelease> = serde_json::from_str(
            r#"[
              {"tag_name": "v0.1.28", "draft": false,
               "assets": [{"name": "manifest.json", "browser_download_url": "https://example.test/stable/manifest.json"}]},
              {"tag_name": "v0.1.29-beta.5", "draft": false,
               "assets": [{"name": "manifest.json", "browser_download_url": "https://example.test/beta5/manifest.json"}]}
            ]"#,
        )
        .unwrap();
        let (stable_tag, _) = select_release_for_channel(&releases, UpdateChannel::Stable).unwrap();
        assert_eq!(stable_tag, "v0.1.28");
        // Beta channel, in contrast, sees the newer beta.
        let (beta_tag, _) = select_release_for_channel(&releases, UpdateChannel::Beta).unwrap();
        assert_eq!(beta_tag, "v0.1.29-beta.5");
    }

    // ----------------------------------------------------------------------
    // B2 — a pre-release build defaults to the beta channel (daemon side).
    // ----------------------------------------------------------------------

    #[test]
    fn default_channel_derived_from_build_version() {
        assert_eq!(
            UpdateChannel::default_for_version("0.1.28"),
            UpdateChannel::Stable
        );
        assert_eq!(
            UpdateChannel::default_for_version("0.1.28-beta.2"),
            UpdateChannel::Beta
        );
        assert_eq!(
            UpdateChannel::default_for_version("0.1.28-rc.1"),
            UpdateChannel::Beta
        );
        assert_eq!(
            UpdateChannel::default_for_version("0.1.28-alpha.1"),
            UpdateChannel::Beta
        );
        // A garbage/dev version is not a recognized pre-release → stay stable.
        assert_eq!(
            UpdateChannel::default_for_version("garbage"),
            UpdateChannel::Stable
        );
    }

    // ----------------------------------------------------------------------
    // B3 — post-update verification: served version must equal the target.
    // ----------------------------------------------------------------------

    #[test]
    fn version_matches_ignores_v_prefix_and_build_metadata() {
        assert!(version_matches("0.1.28", "0.1.28"));
        assert!(version_matches("v0.1.28", "0.1.28"));
        assert!(version_matches("0.1.28+build.9", "0.1.28"));
        assert!(version_matches("0.1.28-beta.3", "0.1.28-beta.3"));
        // A stray OLD daemon must be detected as a mismatch.
        assert!(!version_matches("0.1.27", "0.1.28"));
        assert!(!version_matches("0.1.28-beta.2", "0.1.28-beta.3"));
        assert!(!version_matches("", "0.1.28"));
    }

    // ----------------------------------------------------------------------
    // B6 — port reclaim: kill every listener except the new daemon.
    // ----------------------------------------------------------------------

    #[cfg(unix)]
    #[test]
    fn pids_to_reclaim_keeps_only_the_new_daemon() {
        let listeners = vec!["1001".to_string(), "1002".to_string(), "2000".to_string()];
        // The new daemon (2000) is spared; the two strays are reclaimed.
        assert_eq!(
            pids_to_reclaim(&listeners, "2000"),
            vec!["1001".to_string(), "1002".to_string()]
        );
        // If the new daemon is the only listener, nothing is killed.
        assert!(pids_to_reclaim(&["2000".to_string()], "2000").is_empty());
        // Nothing listening → nothing to reclaim.
        assert!(pids_to_reclaim(&[], "2000").is_empty());
    }
}
