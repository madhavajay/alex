#![allow(dead_code)]

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, ensure, Context, Result};
use serde::Serialize;
use serde_json::{json, Value};
use tokio::process::{Child, Command};

const NPM_PACKAGE: &str = "@askalf/dario";
const REGISTRY_LATEST_URL: &str = "https://registry.npmjs.org/@askalf%2Fdario/latest";
const HEALTH_POLL_MS: u64 = 500;
const HEALTH_TIMEOUT_MS: u64 = 60_000;
const REAPER_INTERVAL_MS: u64 = 2_000;
const DRAIN_IDLE_MS: i64 = 10_000;
const DRAIN_MAX_MS: i64 = 480_000;
const UNHEALTHY_DRAIN_MAX_MS: i64 = 30_000;
const KILL_GRACE_MS: i64 = 10_000;
const HISTORY_CAP: usize = 10;
const PROBE_CONNECT_TIMEOUT_MS: u64 = 2_000;
const PROBE_TOTAL_TIMEOUT_MS: u64 = 30_000;
const PROBE_BODY_SNIPPET_MAX: usize = 256;
const SUSPECT_DEBOUNCE_MS: i64 = 5_000;
const RESPAWN_RETRY_MS: i64 = 10_000;
pub struct DarioSettings {
    pub install_root: PathBuf,
    pub log_root: PathBuf,
    pub capture_root: PathBuf,
    pub prompt_cache_root: PathBuf,
    pub api_key: String,
    pub update_check_minutes: u64,
    pub version_pin: Option<String>,
    pub probe_seconds: u64,
    pub probe_failures: u32,
    pub probe_model: String,
    pub validate_subscription: bool,
}

#[derive(Clone, Debug)]
struct JavascriptRuntime {
    bin: PathBuf,
    version: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PackageManagerKind {
    Npm,
    Pnpm,
    Bun,
}

impl PackageManagerKind {
    fn name(self) -> &'static str {
        match self {
            Self::Npm => "npm",
            Self::Pnpm => "pnpm",
            Self::Bun => "bun",
        }
    }
}

#[derive(Clone, Debug)]
struct PackageManager {
    kind: PackageManagerKind,
    bin: PathBuf,
}

#[derive(Clone, Debug, Serialize)]
pub struct BootstrapResult {
    pub version: String,
    pub runtime: String,
    pub runtime_version: String,
    pub package_manager: String,
    pub entrypoint: PathBuf,
    pub already_installed: bool,
}

struct RuntimeShims {
    claude_bin: PathBuf,
    fetch_capture_preload: PathBuf,
}

#[derive(Clone)]
pub struct ActiveDario {
    pub generation_id: String,
    pub base_url: String,
    pub api_key: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum GenState {
    Warming,
    Active,
    Draining,
    Failed,
    Stopped,
}

impl GenState {
    fn as_str(&self) -> &'static str {
        match self {
            GenState::Warming => "warming",
            GenState::Active => "active",
            GenState::Draining => "draining",
            GenState::Failed => "failed",
            GenState::Stopped => "stopped",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ReapAction {
    Keep,
    Sigterm,
    Kill,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ProbeEscalation {
    Keep,
    Replace,
}

#[derive(Clone, PartialEq, Eq, Debug)]
enum ProbeOutcome {
    Ready,
    Unhealthy { status: u16, body_snippet: String },
    Wedged { error: String },
}

impl ProbeOutcome {
    fn is_ready(&self) -> bool {
        matches!(self, ProbeOutcome::Ready)
    }

    fn status(&self) -> Option<u16> {
        match self {
            ProbeOutcome::Unhealthy { status, .. } => Some(*status),
            _ => None,
        }
    }

    fn error(&self) -> Option<&str> {
        match self {
            ProbeOutcome::Wedged { error } => Some(error),
            _ => None,
        }
    }

    fn body_snippet(&self) -> Option<&str> {
        match self {
            ProbeOutcome::Unhealthy { body_snippet, .. } => Some(body_snippet),
            _ => None,
        }
    }

    fn summary(&self) -> String {
        match self {
            ProbeOutcome::Ready => "ready".to_string(),
            ProbeOutcome::Unhealthy {
                status,
                body_snippet,
            } => format!("unhealthy status={status} body={body_snippet}"),
            ProbeOutcome::Wedged { error } => format!("wedged: {error}"),
        }
    }
}

#[derive(Clone)]
struct ProbeRecord {
    at_ms: i64,
    latency_ms: i64,
    outcome: ProbeOutcome,
}

struct Generation {
    id: String,
    version: String,
    port: u16,
    pid: u32,
    child: Mutex<Option<Child>>,
    state: Mutex<GenState>,
    started_at: i64,
    promoted_at: AtomicI64,
    drain_started_at: AtomicI64,
    sigterm_at: AtomicI64,
    in_flight: AtomicI64,
    last_activity_ms: AtomicI64,
    unhealthy: AtomicBool,
    consecutive_failures: AtomicU32,
    probe_in_flight: AtomicBool,
    replacement_pending: AtomicBool,
    last_probe: Mutex<Option<ProbeRecord>>,
    stdout_log: PathBuf,
    stderr_log: PathBuf,
}

impl Generation {
    fn new(
        id: String,
        version: String,
        port: u16,
        pid: u32,
        stdout_log: PathBuf,
        stderr_log: PathBuf,
        now: i64,
    ) -> Self {
        Self {
            id,
            version,
            port,
            pid,
            child: Mutex::new(None),
            state: Mutex::new(GenState::Warming),
            started_at: now,
            promoted_at: AtomicI64::new(0),
            drain_started_at: AtomicI64::new(0),
            sigterm_at: AtomicI64::new(0),
            in_flight: AtomicI64::new(0),
            last_activity_ms: AtomicI64::new(now),
            unhealthy: AtomicBool::new(false),
            consecutive_failures: AtomicU32::new(0),
            probe_in_flight: AtomicBool::new(false),
            replacement_pending: AtomicBool::new(false),
            last_probe: Mutex::new(None),
            stdout_log,
            stderr_log,
        }
    }

    fn state(&self) -> GenState {
        *self.state.lock().unwrap()
    }

    fn set_state(&self, state: GenState) {
        *self.state.lock().unwrap() = state;
    }

    fn try_begin(self: &Arc<Self>) -> Option<InFlightGuard> {
        match self.state() {
            GenState::Active | GenState::Draining => {
                self.in_flight.fetch_add(1, Ordering::SeqCst);
                Some(InFlightGuard {
                    generation: self.clone(),
                })
            }
            _ => None,
        }
    }

    fn observe_exit(&self) {
        let mut guard = self.child.lock().unwrap();
        if let Some(child) = guard.as_mut() {
            if matches!(child.try_wait(), Ok(Some(_))) {
                *guard = None;
                let mut state = self.state.lock().unwrap();
                if *state != GenState::Failed {
                    *state = GenState::Stopped;
                }
            }
        }
    }

    fn kill_now(&self) {
        let mut guard = self.child.lock().unwrap();
        if let Some(child) = guard.as_mut() {
            let _ = child.start_kill();
        }
    }

    fn record_probe(&self, outcome: &ProbeOutcome, latency_ms: i64, now: i64) -> u32 {
        let failures = if outcome.is_ready() {
            self.consecutive_failures.store(0, Ordering::SeqCst);
            0
        } else {
            self.consecutive_failures.fetch_add(1, Ordering::SeqCst) + 1
        };
        *self.last_probe.lock().unwrap() = Some(ProbeRecord {
            at_ms: now,
            latency_ms,
            outcome: outcome.clone(),
        });
        failures
    }

    fn status_json(&self) -> Value {
        let last_probe = self.last_probe.lock().unwrap().clone();
        json!({
            "id": self.id,
            "version": self.version,
            "port": self.port,
            "pid": self.pid,
            "state": self.state().as_str(),
            "phase": phase_name(self.state(), self.unhealthy.load(Ordering::SeqCst)),
            "in_flight": self.in_flight.load(Ordering::SeqCst),
            "consecutive_failures": self.consecutive_failures.load(Ordering::SeqCst),
            "last_probe": last_probe
                .map(|p| json!({
                    "at_ms": p.at_ms,
                    "ok": p.outcome.is_ready(),
                    "status": p.outcome.status(),
                    "latency_ms": p.latency_ms,
                    "error": p.outcome.error(),
                }))
                .unwrap_or(Value::Null),
            "started_at": self.started_at,
            "promoted_at": ms_or_null(self.promoted_at.load(Ordering::SeqCst)),
            "drain_started_at": ms_or_null(self.drain_started_at.load(Ordering::SeqCst)),
            "last_activity_ms": ms_or_null(self.last_activity_ms.load(Ordering::SeqCst)),
            "stdout_log": self.stdout_log,
            "stderr_log": self.stderr_log,
        })
    }
}

pub struct InFlightGuard {
    generation: Arc<Generation>,
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        self.generation.in_flight.fetch_sub(1, Ordering::SeqCst);
        self.generation
            .last_activity_ms
            .store(now_ms(), Ordering::SeqCst);
    }
}

pub struct DarioSupervisor {
    settings: DarioSettings,
    runtime: JavascriptRuntime,
    active: RwLock<Option<Arc<Generation>>>,
    generations: Mutex<Vec<Arc<Generation>>>,
    http: reqwest::Client,
    roll_lock: tokio::sync::Mutex<()>,
    shutting_down: AtomicBool,
    weak_self: OnceLock<Weak<Self>>,
    last_version: Mutex<Option<String>>,
    respawn_in_flight: AtomicBool,
    respawn_next_at: AtomicI64,
}

pub async fn bootstrap(
    install_root: PathBuf,
    version_pin: Option<String>,
) -> Result<BootstrapResult> {
    let runtime = find_node_runtime().await?;
    tokio::fs::create_dir_all(&install_root)
        .await
        .with_context(|| format!("creating {install_root:?}"))?;
    let http = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .build()
        .context("building Dario bootstrap HTTP client")?;
    let version = if let Some(pin) = version_pin {
        pin
    } else {
        match registry_latest(&http).await {
            Ok(version) => version,
            Err(error) => latest_installed_version(&install_root).ok_or_else(|| {
                error.context("fetching the Dario version and no installed fallback was found")
            })?,
        }
    };
    let installed = install_version(&install_root, &version).await?;
    Ok(BootstrapResult {
        version,
        runtime: "node".into(),
        runtime_version: runtime.version,
        package_manager: installed.package_manager,
        entrypoint: installed.entrypoint,
        already_installed: installed.already_installed,
    })
}

struct InstallResult {
    package_manager: String,
    entrypoint: PathBuf,
    already_installed: bool,
}

async fn install_version(install_root: &Path, version: &str) -> Result<InstallResult> {
    let prefix = install_root.join(version);
    let entrypoint = dario_entrypoint(&prefix);
    if entrypoint.exists() {
        return Ok(InstallResult {
            package_manager: "existing".into(),
            entrypoint,
            already_installed: true,
        });
    }
    tokio::fs::create_dir_all(&prefix)
        .await
        .with_context(|| format!("creating Dario install prefix {prefix:?}"))?;
    let package_json = prefix.join("package.json");
    if !package_json.exists() {
        tokio::fs::write(&package_json, b"{\n  \"private\": true\n}\n")
            .await
            .with_context(|| format!("writing {package_json:?}"))?;
    }

    let managers = package_managers();
    ensure!(
        !managers.is_empty(),
        "Dario needs a JavaScript package manager; install npm, pnpm, or Bun"
    );
    let spec = format!("{NPM_PACKAGE}@{version}");
    let mut failures = Vec::new();
    for manager in managers {
        let mut command = Command::new(&manager.bin);
        command.args(package_manager_args(manager.kind, &prefix, &spec));
        let output = match command.stdin(Stdio::null()).output().await {
            Ok(output) => output,
            Err(error) => {
                failures.push(format!("{}: {error}", manager.kind.name()));
                continue;
            }
        };
        if output.status.success() && entrypoint.exists() {
            return Ok(InstallResult {
                package_manager: manager.kind.name().into(),
                entrypoint,
                already_installed: false,
            });
        }
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        failures.push(format!(
            "{}: {}",
            manager.kind.name(),
            if detail.is_empty() {
                format!("exited with {}", output.status)
            } else {
                detail
            }
        ));
    }
    Err(anyhow!(
        "failed to install {spec} with the available package managers: {}",
        failures.join("; ")
    ))
}

fn package_manager_args(kind: PackageManagerKind, prefix: &Path, spec: &str) -> Vec<OsString> {
    match kind {
        PackageManagerKind::Npm => vec![
            "install".into(),
            spec.into(),
            "--prefix".into(),
            prefix.as_os_str().into(),
            "--omit=dev".into(),
            "--no-audit".into(),
            "--no-fund".into(),
        ],
        PackageManagerKind::Pnpm => vec![
            "--dir".into(),
            prefix.as_os_str().into(),
            "add".into(),
            "--prod".into(),
            "--save-exact".into(),
            spec.into(),
        ],
        PackageManagerKind::Bun => vec![
            "add".into(),
            "--cwd".into(),
            prefix.as_os_str().into(),
            "--production".into(),
            "--exact".into(),
            spec.into(),
        ],
    }
}

fn dario_entrypoint(prefix: &Path) -> PathBuf {
    prefix
        .join("node_modules")
        .join("@askalf")
        .join("dario")
        .join("dist")
        .join("cli.js")
}

fn package_managers() -> Vec<PackageManager> {
    [
        (PackageManagerKind::Npm, "npm"),
        (PackageManagerKind::Pnpm, "pnpm"),
        (PackageManagerKind::Bun, "bun"),
    ]
    .into_iter()
    .filter_map(|(kind, name)| find_on_path(name).map(|bin| PackageManager { kind, bin }))
    .collect()
}

async fn find_node_runtime() -> Result<JavascriptRuntime> {
    let bin = find_on_path("node").ok_or_else(|| {
        anyhow!(
            "Dario requires Node.js 18 or newer at runtime; install Node.js (npm, pnpm, or Bun may be used to install the package)"
        )
    })?;
    let output = Command::new(&bin)
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        .await
        .with_context(|| format!("checking Node.js runtime {bin:?}"))?;
    ensure!(output.status.success(), "{bin:?} --version failed");
    let version = String::from_utf8_lossy(&output.stdout)
        .trim()
        .trim_start_matches('v')
        .to_string();
    let major = version
        .split('.')
        .next()
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| anyhow!("could not parse Node.js version {version:?} from {bin:?}"))?;
    ensure!(
        major >= 18,
        "Dario requires Node.js 18 or newer; found {version} at {bin:?}"
    );
    Ok(JavascriptRuntime { bin, version })
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

fn latest_installed_version(install_root: &Path) -> Option<String> {
    let mut versions = std::fs::read_dir(install_root)
        .ok()?
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir() && dario_entrypoint(&entry.path()).exists())
        .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
        .filter_map(|version| version_key(&version).map(|key| (key, version)))
        .collect::<Vec<_>>();
    versions.sort_by(|a, b| a.0.cmp(&b.0));
    versions.pop().map(|(_, version)| version)
}

fn version_key(version: &str) -> Option<Vec<u64>> {
    version
        .split('.')
        .map(str::parse::<u64>)
        .collect::<std::result::Result<Vec<_>, _>>()
        .ok()
}

async fn registry_latest(http: &reqwest::Client) -> Result<String> {
    let body: Value = http
        .get(REGISTRY_LATEST_URL)
        .send()
        .await
        .context("fetching npm registry latest")?
        .error_for_status()
        .context("npm registry latest status")?
        .json()
        .await
        .context("parsing npm registry response")?;
    body["version"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| anyhow!("npm registry response missing version"))
}

impl DarioSupervisor {
    pub async fn start(settings: DarioSettings) -> Result<Arc<Self>> {
        tokio::fs::create_dir_all(&settings.install_root)
            .await
            .with_context(|| format!("creating {:?}", settings.install_root))?;
        tokio::fs::create_dir_all(&settings.log_root)
            .await
            .with_context(|| format!("creating {:?}", settings.log_root))?;
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .build()
            .context("building http client")?;
        let runtime = find_node_runtime().await?;
        let supervisor = Arc::new(Self {
            settings,
            runtime,
            active: RwLock::new(None),
            generations: Mutex::new(Vec::new()),
            http,
            roll_lock: tokio::sync::Mutex::new(()),
            shutting_down: AtomicBool::new(false),
            weak_self: OnceLock::new(),
            last_version: Mutex::new(None),
            respawn_in_flight: AtomicBool::new(false),
            respawn_next_at: AtomicI64::new(0),
        });
        let _ = supervisor.weak_self.set(Arc::downgrade(&supervisor));
        supervisor.reap_orphans();
        let version = supervisor.resolve_version().await?;
        supervisor
            .roll(&version)
            .await
            .context("starting initial dario generation")?;
        supervisor.spawn_reaper();
        if supervisor.settings.update_check_minutes > 0 {
            supervisor.spawn_update_loop();
        }
        if supervisor.settings.validate_subscription && supervisor.settings.probe_seconds > 0 {
            supervisor.spawn_probe_loop();
        }
        Ok(supervisor)
    }

    pub fn active(&self) -> Option<ActiveDario> {
        let guard = self.active.read().unwrap();
        let gen = guard.as_ref()?;
        if gen.unhealthy.load(Ordering::SeqCst) || gen.state() != GenState::Active {
            return None;
        }
        Some(ActiveDario {
            generation_id: gen.id.clone(),
            base_url: format!("http://127.0.0.1:{}", gen.port),
            api_key: self.settings.api_key.clone(),
        })
    }

    pub fn begin_request(&self, generation_id: &str) -> Option<InFlightGuard> {
        let gen = {
            let gens = self.generations.lock().unwrap();
            gens.iter().find(|g| g.id == generation_id)?.clone()
        };
        gen.try_begin()
    }

    pub fn suspect(&self, generation_id: &str) {
        let gen = {
            let gens = self.generations.lock().unwrap();
            gens.iter().find(|g| g.id == generation_id).cloned()
        };
        let Some(gen) = gen else { return };
        let last_at = gen
            .last_probe
            .lock()
            .unwrap()
            .as_ref()
            .map(|p| p.at_ms)
            .unwrap_or(0);
        if !should_probe(
            now_ms(),
            last_at,
            gen.probe_in_flight.load(Ordering::SeqCst),
        ) {
            return;
        }
        let Some(sup) = self.weak_self.get().and_then(Weak::upgrade) else {
            return;
        };
        tokio::spawn(async move {
            sup.run_probe(&gen).await;
        });
    }

    pub fn status(&self) -> Value {
        let active_id = {
            let guard = self.active.read().unwrap();
            guard.as_ref().map(|g| g.id.clone())
        };
        let gens = self.generations.lock().unwrap().clone();
        json!({
            "available": active_id.is_some(),
            "runtime": "node",
            "runtime_version": self.runtime.version,
            "runtime_path": self.runtime.bin,
            "active_generation_id": active_id,
            "generations": gens.iter().map(|g| g.status_json()).collect::<Vec<_>>(),
        })
    }

    fn write_update_state(&self, latest: &str, active_version: Option<&str>) {
        let state = json!({
            "checked_at_ms": now_ms(),
            "latest": latest,
            "active_version": active_version,
            "pinned": self.settings.version_pin,
            "update_available": active_version.map(|v| v != latest).unwrap_or(false),
        });
        let path = self.settings.install_root.join("update-state.json");
        if let Err(e) = std::fs::create_dir_all(&self.settings.install_root)
            .and_then(|_| std::fs::write(&path, state.to_string()))
        {
            tracing::warn!("failed to write dario update state: {e}");
        }
    }

    pub async fn update_now(&self) -> Result<Value> {
        let latest = self.npm_latest().await?;
        let current = {
            let guard = self.active.read().unwrap();
            guard.clone()
        };
        self.write_update_state(&latest, current.as_ref().map(|g| g.version.as_str()));
        if let Some(pin) = &self.settings.version_pin {
            return Ok(json!({
                "outcome": "pinned",
                "version": pin,
                "latest": latest,
                "update_available": pin.as_str() != latest,
            }));
        }
        let Some(current) = current else {
            return Err(anyhow!("no active generation"));
        };
        if current.version == latest {
            return Ok(json!({"outcome": "up_to_date", "version": latest}));
        }
        let gen = self.roll(&latest).await?;
        self.write_update_state(&latest, Some(latest.as_str()));
        Ok(json!({
            "outcome": "updated",
            "from": current.version,
            "to": latest,
            "generation_id": gen.id,
        }))
    }

    pub async fn restart(&self) -> Result<Value> {
        let current = {
            let guard = self.active.read().unwrap();
            guard.clone()
        };
        let Some(current) = current else {
            return Err(anyhow!("no active generation"));
        };
        let gen = self.roll(&current.version).await?;
        Ok(json!({
            "outcome": "restarted",
            "version": gen.version,
            "generation_id": gen.id,
        }))
    }

    pub async fn shutdown(&self) {
        self.shutting_down.store(true, Ordering::SeqCst);
        let gens = self.generations.lock().unwrap().clone();
        for gen in &gens {
            if matches!(gen.state(), GenState::Stopped | GenState::Failed) {
                continue;
            }
            send_sigterm(gen.pid).await;
            gen.sigterm_at.store(now_ms(), Ordering::SeqCst);
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let mut alive = false;
            for gen in &gens {
                gen.observe_exit();
                if gen.child.lock().unwrap().is_some() {
                    alive = true;
                }
            }
            if !alive || tokio::time::Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        for gen in &gens {
            gen.kill_now();
            gen.observe_exit();
            if !matches!(gen.state(), GenState::Failed) {
                gen.set_state(GenState::Stopped);
            }
        }
        {
            let mut guard = self.active.write().unwrap();
            *guard = None;
        }
    }

    async fn roll(&self, version: &str) -> Result<Arc<Generation>> {
        let _guard = self.roll_lock.lock().await;
        let bin = self.ensure_installed(version).await?;
        let gen = self.spawn_generation(version, &bin)?;
        self.remember(gen.clone());
        if let Err(e) = self.wait_ready(&gen).await {
            gen.set_state(GenState::Failed);
            gen.kill_now();
            return Err(e);
        }
        self.promote(gen.clone());
        tracing::info!(generation = %gen.id, version = %gen.version, port = gen.port, "dario generation promoted");
        Ok(gen)
    }

    fn promote(&self, gen: Arc<Generation>) {
        let now = now_ms();
        let old = {
            let mut guard = self.active.write().unwrap();
            guard.replace(gen.clone())
        };
        *self.last_version.lock().unwrap() = Some(gen.version.clone());
        promote_bookkeeping(&gen, old.as_deref(), now);
    }

    fn remember(&self, gen: Arc<Generation>) {
        let mut gens = self.generations.lock().unwrap();
        gens.push(gen);
        while gens.len() > HISTORY_CAP {
            match gens
                .iter()
                .position(|g| matches!(g.state(), GenState::Stopped | GenState::Failed))
            {
                Some(idx) => {
                    gens.remove(idx);
                }
                None => break,
            }
        }
    }

    async fn ensure_installed(&self, version: &str) -> Result<PathBuf> {
        install_version(&self.settings.install_root, version)
            .await
            .map(|result| result.entrypoint)
    }

    async fn resolve_version(&self) -> Result<String> {
        if let Some(pin) = &self.settings.version_pin {
            return Ok(pin.clone());
        }
        match self.npm_latest().await {
            Ok(version) => Ok(version),
            Err(error) => latest_installed_version(&self.settings.install_root).ok_or_else(|| {
                error.context("fetching the Dario version and no installed fallback was found")
            }),
        }
    }

    async fn npm_latest(&self) -> Result<String> {
        registry_latest(&self.http).await
    }

    fn pids_path(&self) -> PathBuf {
        self.settings.install_root.join("child-pids.json")
    }

    fn record_child(&self, gen_id: &str, child_pid: u32, port: u16) {
        let path = self.pids_path();
        let mut entries: Vec<Value> = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        entries.retain(|e| e["child_pid"].as_u64() != Some(child_pid as u64));
        entries.push(json!({
            "daemon_pid": std::process::id(),
            "child_pid": child_pid,
            "port": port,
            "generation_id": gen_id,
            "started_ms": now_ms(),
        }));
        if let Ok(data) = serde_json::to_string(&entries) {
            let _ = std::fs::write(&path, data);
        }
    }

    fn reap_orphans(&self) {
        let path = self.pids_path();
        let Some(entries) = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<Vec<Value>>(&s).ok())
        else {
            return;
        };
        let self_pid = std::process::id() as u64;
        let mut kept = Vec::new();
        for e in entries {
            let daemon = e["daemon_pid"].as_u64().unwrap_or(0);
            let child = e["child_pid"].as_u64().unwrap_or(0);
            if child == 0 {
                continue;
            }
            let daemon_alive = daemon == self_pid || (daemon > 0 && process_alive(daemon as i32));
            let child_alive = process_alive(child as i32);
            if daemon_alive && child_alive {
                kept.push(e);
                continue;
            }
            if child_alive {
                tracing::info!(
                    child,
                    generation = e["generation_id"].as_str().unwrap_or("-"),
                    "reaping orphaned dario child from dead daemon"
                );
                terminate_pid(child as i32);
            }
        }
        if let Ok(data) = serde_json::to_string(&kept) {
            let _ = std::fs::write(&path, data);
        }
    }

    fn prepare_runtime_shims(&self) -> Result<RuntimeShims> {
        let dir = self.settings.install_root.join("shims");
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {dir:?}"))?;

        let claude_bin = dir.join(claude_shim_name());
        write_if_changed(&claude_bin, claude_capture_shim())?;
        make_executable(&claude_bin)?;

        let fetch_capture_preload_path = dir.join("dario-fetch-capture.cjs");
        write_if_changed(&fetch_capture_preload_path, fetch_capture_preload())?;

        Ok(RuntimeShims {
            claude_bin,
            fetch_capture_preload: fetch_capture_preload_path,
        })
    }

    fn spawn_generation(&self, version: &str, bin: &Path) -> Result<Arc<Generation>> {
        let port = alloc_port()?;
        let id = generation_id(version, port);
        let work_dir = self.settings.install_root.join("workspace");
        std::fs::create_dir_all(&work_dir)
            .with_context(|| format!("creating private Dario workspace {work_dir:?}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&work_dir, std::fs::Permissions::from_mode(0o700))
                .with_context(|| format!("securing private Dario workspace {work_dir:?}"))?;
        }
        let stdout_log = self.settings.log_root.join(format!("{id}.out.log"));
        let stderr_log = self.settings.log_root.join(format!("{id}.err.log"));
        let out = std::fs::File::create(&stdout_log)
            .with_context(|| format!("creating {stdout_log:?}"))?;
        let err = std::fs::File::create(&stderr_log)
            .with_context(|| format!("creating {stderr_log:?}"))?;
        quarantine_fable_live_cache();
        let shims = self.prepare_runtime_shims()?;
        let node_options = node_options_with_require(&shims.fetch_capture_preload);
        let mut child_cmd = Command::new(&self.runtime.bin);
        child_cmd
            .arg(bin)
            .arg("proxy")
            .arg("--host=127.0.0.1")
            .arg(format!("--port={port}"))
            .env("DARIO_API_KEY", &self.settings.api_key)
            .env("DARIO_CLAUDE_BIN", &shims.claude_bin)
            .env("ALEXANDRIA_DARIO_CAPTURE_DIR", &self.settings.capture_root)
            .env(
                "ALEXANDRIA_DARIO_PROMPT_CACHE_DIR",
                &self.settings.prompt_cache_root,
            )
            .env("ALEXANDRIA_DARIO_WORK_DIR", &work_dir)
            .env("NODE_OPTIONS", node_options)
            // The fetch-capture preload is a Node hook. Keeping supervised
            // Dario in Node avoids losing trace capture to Bun auto-relaunch.
            .env("DARIO_NO_BUN", "1")
            .current_dir(&work_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::from(out))
            .stderr(Stdio::from(err));
        if let Some(real) = std::env::var_os("DARIO_CLAUDE_BIN") {
            child_cmd.env("ALEXANDRIA_REAL_CLAUDE_BIN", real);
        }
        let child = child_cmd
            .spawn()
            .with_context(|| format!("spawning {bin:?}"))?;
        let pid = child.id().unwrap_or(0);
        if pid > 0 {
            self.record_child(&id, pid, port);
        }
        let gen = Arc::new(Generation::new(
            id,
            version.to_string(),
            port,
            pid,
            stdout_log,
            stderr_log,
            now_ms(),
        ));
        *gen.child.lock().unwrap() = Some(child);
        Ok(gen)
    }

    async fn wait_ready(&self, gen: &Generation) -> Result<()> {
        let health_url = format!("http://127.0.0.1:{}/health", gen.port);
        let deadline = tokio::time::Instant::now() + Duration::from_millis(HEALTH_TIMEOUT_MS);
        loop {
            let ok = matches!(
                self.http
                    .get(&health_url)
                    .timeout(Duration::from_secs(2))
                    .send()
                    .await,
                Ok(r) if r.status().is_success()
            );
            if ok {
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(anyhow!(
                    "{} not healthy within {HEALTH_TIMEOUT_MS}ms",
                    gen.id
                ));
            }
            tokio::time::sleep(Duration::from_millis(HEALTH_POLL_MS)).await;
        }
        if !self.settings.validate_subscription {
            return Ok(());
        }
        let base_url = format!("http://127.0.0.1:{}", gen.port);
        let (outcome, latency_ms) = probe_messages(
            &base_url,
            &self.settings.api_key,
            &self.settings.probe_model,
            Duration::from_millis(PROBE_CONNECT_TIMEOUT_MS),
            Duration::from_millis(PROBE_TOTAL_TIMEOUT_MS),
        )
        .await;
        gen.record_probe(&outcome, latency_ms, now_ms());
        if !outcome.is_ready() {
            self.persist_failed_probe(gen, &outcome).await;
            return Err(anyhow!(
                "{} readiness probe failed: {}",
                gen.id,
                outcome.summary()
            ));
        }
        Ok(())
    }

    async fn run_probe(&self, gen: &Arc<Generation>) {
        if gen.probe_in_flight.swap(true, Ordering::SeqCst) {
            return;
        }
        let base_url = format!("http://127.0.0.1:{}", gen.port);
        let (outcome, latency_ms) = probe_messages(
            &base_url,
            &self.settings.api_key,
            &self.settings.probe_model,
            Duration::from_millis(PROBE_CONNECT_TIMEOUT_MS),
            Duration::from_millis(PROBE_TOTAL_TIMEOUT_MS),
        )
        .await;
        gen.probe_in_flight.store(false, Ordering::SeqCst);
        let failures = gen.record_probe(&outcome, latency_ms, now_ms());
        if outcome.is_ready() {
            if gen.unhealthy.swap(false, Ordering::SeqCst) {
                tracing::info!(generation = %gen.id, "dario generation recovered");
            }
            return;
        }
        self.persist_failed_probe(gen, &outcome).await;
        tracing::warn!(
            generation = %gen.id,
            failures,
            outcome = %outcome.summary(),
            "dario readiness probe failed"
        );
        if probe_escalation(failures, self.settings.probe_failures) == ProbeEscalation::Replace
            && gen.state() == GenState::Active
        {
            self.replace_unhealthy(gen).await;
        }
    }

    async fn replace_unhealthy(&self, gen: &Arc<Generation>) {
        let is_active = {
            let guard = self.active.read().unwrap();
            guard.as_ref().map(|a| Arc::ptr_eq(a, gen)).unwrap_or(false)
        };
        if !is_active || self.shutting_down.load(Ordering::SeqCst) {
            return;
        }
        if !gen.unhealthy.swap(true, Ordering::SeqCst) {
            tracing::warn!(generation = %gen.id, "dario generation marked unhealthy");
        }
        if gen.replacement_pending.swap(true, Ordering::SeqCst) {
            return;
        }
        match self.roll(&gen.version).await {
            Ok(newer) => {
                tracing::info!(old = %gen.id, new = %newer.id, "replaced unhealthy dario generation")
            }
            Err(e) => {
                tracing::warn!(generation = %gen.id, error = %e, "failed to replace unhealthy dario generation");
                gen.replacement_pending.store(false, Ordering::SeqCst);
            }
        }
    }

    async fn persist_failed_probe(&self, gen: &Generation, outcome: &ProbeOutcome) {
        let path = self
            .settings
            .log_root
            .join(format!("{}.last-probe.json", gen.id));
        let payload = json!({
            "ts": now_ms(),
            "status": outcome.status(),
            "error": outcome.error(),
            "body": outcome.body_snippet(),
        });
        if let Err(e) = tokio::fs::write(&path, payload.to_string()).await {
            tracing::warn!(generation = %gen.id, error = %e, "writing last-probe file failed");
        }
    }

    fn spawn_reaper(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(REAPER_INTERVAL_MS));
            loop {
                interval.tick().await;
                let Some(sup) = weak.upgrade() else { break };
                if sup.shutting_down.load(Ordering::SeqCst) {
                    break;
                }
                sup.reaper_tick().await;
            }
        });
    }

    fn spawn_update_loop(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        let minutes = self.settings.update_check_minutes;
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(minutes * 60));
            interval.tick().await;
            loop {
                interval.tick().await;
                let Some(sup) = weak.upgrade() else { break };
                if sup.shutting_down.load(Ordering::SeqCst) {
                    break;
                }
                match sup.update_now().await {
                    Ok(outcome) => tracing::info!(%outcome, "dario update check"),
                    Err(e) => tracing::warn!(error = %e, "dario update check failed"),
                }
            }
        });
    }

    fn spawn_probe_loop(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        let seconds = self.settings.probe_seconds;
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(seconds));
            interval.tick().await;
            loop {
                interval.tick().await;
                let Some(sup) = weak.upgrade() else { break };
                if sup.shutting_down.load(Ordering::SeqCst) {
                    break;
                }
                sup.probe_tick().await;
            }
        });
    }

    async fn probe_tick(&self) {
        let gen = {
            let guard = self.active.read().unwrap();
            guard.clone()
        };
        let Some(gen) = gen else { return };
        if gen.probe_in_flight.load(Ordering::SeqCst) {
            return;
        }
        self.run_probe(&gen).await;
    }

    async fn reaper_tick(&self) {
        let gens = self.generations.lock().unwrap().clone();
        let now = now_ms();
        for gen in &gens {
            gen.observe_exit();
        }
        self.detect_active_death();
        self.maybe_respawn();
        for gen in gens {
            if gen.state() != GenState::Draining {
                continue;
            }
            let action = reap_action(
                now,
                gen.in_flight.load(Ordering::SeqCst),
                gen.last_activity_ms.load(Ordering::SeqCst),
                gen.drain_started_at.load(Ordering::SeqCst),
                gen.sigterm_at.load(Ordering::SeqCst),
                gen.unhealthy.load(Ordering::SeqCst),
            );
            match action {
                ReapAction::Keep => {}
                ReapAction::Sigterm => {
                    tracing::info!(generation = %gen.id, "draining dario generation: SIGTERM");
                    gen.sigterm_at.store(now, Ordering::SeqCst);
                    send_sigterm(gen.pid).await;
                }
                ReapAction::Kill => {
                    tracing::warn!(generation = %gen.id, "draining dario generation ignored SIGTERM: kill");
                    gen.kill_now();
                }
            }
        }
    }

    fn detect_active_death(&self) {
        let gen = {
            let guard = self.active.read().unwrap();
            guard.clone()
        };
        let Some(gen) = gen else { return };
        if !matches!(gen.state(), GenState::Stopped | GenState::Failed) {
            return;
        }
        tracing::warn!(generation = %gen.id, "active dario generation exited unexpectedly");
        let mut guard = self.active.write().unwrap();
        if guard
            .as_ref()
            .map(|g| Arc::ptr_eq(g, &gen))
            .unwrap_or(false)
        {
            *guard = None;
            self.respawn_next_at.store(0, Ordering::SeqCst);
        }
    }

    fn maybe_respawn(&self) {
        if self.shutting_down.load(Ordering::SeqCst) {
            return;
        }
        if self.active.read().unwrap().is_some() {
            return;
        }
        let version = { self.last_version.lock().unwrap().clone() };
        let Some(version) = version else { return };
        if now_ms() < self.respawn_next_at.load(Ordering::SeqCst) {
            return;
        }
        if self.respawn_in_flight.swap(true, Ordering::SeqCst) {
            return;
        }
        let Some(sup) = self.weak_self.get().and_then(Weak::upgrade) else {
            self.respawn_in_flight.store(false, Ordering::SeqCst);
            return;
        };
        tokio::spawn(async move {
            tracing::info!(version = %version, "respawning dario after unexpected exit");
            if let Err(e) = sup.roll(&version).await {
                tracing::warn!(version = %version, error = %e, "dario respawn failed; will retry");
                sup.respawn_next_at
                    .store(now_ms() + RESPAWN_RETRY_MS, Ordering::SeqCst);
            }
            sup.respawn_in_flight.store(false, Ordering::SeqCst);
        });
    }
}

async fn probe_messages(
    base_url: &str,
    api_key: &str,
    model: &str,
    connect_timeout: Duration,
    total_timeout: Duration,
) -> (ProbeOutcome, i64) {
    let started = std::time::Instant::now();
    let outcome =
        probe_messages_outcome(base_url, api_key, model, connect_timeout, total_timeout).await;
    (outcome, started.elapsed().as_millis() as i64)
}

async fn probe_messages_outcome(
    base_url: &str,
    api_key: &str,
    model: &str,
    connect_timeout: Duration,
    total_timeout: Duration,
) -> ProbeOutcome {
    let client = match reqwest::Client::builder()
        .connect_timeout(connect_timeout)
        .timeout(total_timeout)
        .build()
    {
        Ok(client) => client,
        Err(e) => {
            return ProbeOutcome::Wedged {
                error: e.to_string(),
            }
        }
    };
    let body = json!({
        "model": model,
        "max_tokens": 8,
        "stream": false,
        "messages": [{"role": "user", "content": "Reply with exactly: hello"}],
    });
    match client
        .post(format!("{base_url}/v1/messages"))
        .header("x-api-key", api_key)
        .json(&body)
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => ProbeOutcome::Ready,
        Ok(resp) => {
            let status = resp.status().as_u16();
            let text = resp.text().await.unwrap_or_default();
            ProbeOutcome::Unhealthy {
                status,
                body_snippet: snippet(&text),
            }
        }
        Err(e) => ProbeOutcome::Wedged {
            error: e.to_string(),
        },
    }
}

fn snippet(text: &str) -> String {
    let mut end = text.len().min(PROBE_BODY_SNIPPET_MAX);
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

fn should_probe(now: i64, last_probe_at: i64, probe_in_flight: bool) -> bool {
    !probe_in_flight && now - last_probe_at >= SUSPECT_DEBOUNCE_MS
}

fn probe_escalation(consecutive_failures: u32, threshold: u32) -> ProbeEscalation {
    if threshold > 0 && consecutive_failures >= threshold {
        ProbeEscalation::Replace
    } else {
        ProbeEscalation::Keep
    }
}

fn phase_name(state: GenState, unhealthy: bool) -> &'static str {
    match state {
        GenState::Warming => "starting",
        GenState::Active => {
            if unhealthy {
                "unhealthy"
            } else {
                "ready"
            }
        }
        GenState::Draining => "draining",
        GenState::Failed | GenState::Stopped => "dead",
    }
}

fn promote_bookkeeping(new_gen: &Generation, old: Option<&Generation>, now: i64) {
    new_gen.set_state(GenState::Active);
    new_gen.promoted_at.store(now, Ordering::SeqCst);
    if let Some(old) = old {
        if old.id != new_gen.id {
            old.set_state(GenState::Draining);
            old.drain_started_at.store(now, Ordering::SeqCst);
            old.last_activity_ms.store(now, Ordering::SeqCst);
        }
    }
}

fn reap_action(
    now: i64,
    in_flight: i64,
    last_activity_ms: i64,
    drain_started_ms: i64,
    sigterm_at_ms: i64,
    unhealthy: bool,
) -> ReapAction {
    let drain_max_ms = if unhealthy {
        UNHEALTHY_DRAIN_MAX_MS
    } else {
        DRAIN_MAX_MS
    };
    if sigterm_at_ms > 0 {
        if now - sigterm_at_ms > KILL_GRACE_MS {
            ReapAction::Kill
        } else {
            ReapAction::Keep
        }
    } else if now - drain_started_ms > drain_max_ms {
        ReapAction::Sigterm
    } else if in_flight == 0 && now - last_activity_ms > DRAIN_IDLE_MS {
        ReapAction::Sigterm
    } else {
        ReapAction::Keep
    }
}

fn claude_shim_name() -> &'static str {
    if cfg!(windows) {
        "claude-alexandria-shim.cmd"
    } else {
        "claude-alexandria-shim"
    }
}

fn claude_capture_shim() -> String {
    if cfg!(windows) {
        r#"@echo off
setlocal
set "REAL=%ALEXANDRIA_REAL_CLAUDE_BIN%"
if "%REAL%"=="" set "REAL=claude"
set "MODEL=%ALEXANDRIA_DARIO_CAPTURE_MODEL%"
set "HAS_MODEL="
set "PASSTHROUGH="
for %%A in (%*) do (
  if "%%~A"=="--model" set "HAS_MODEL=1"
  echo %%~A | findstr /B /C:"--model=" >nul && set "HAS_MODEL=1"
  if "%%~A"=="--version" set "PASSTHROUGH=1"
)
set "NODE_OPTIONS="
if defined PASSTHROUGH "%REAL%" %* & exit /b %ERRORLEVEL%
if defined HAS_MODEL "%REAL%" %* & exit /b %ERRORLEVEL%
if defined MODEL "%REAL%" --model "%MODEL%" %* & exit /b %ERRORLEVEL%
"%REAL%" %*
"#
        .to_string()
    } else {
        r#"#!/bin/sh
set -eu
real="${ALEXANDRIA_REAL_CLAUDE_BIN:-}"
model="${ALEXANDRIA_DARIO_CAPTURE_MODEL:-}"
if [ -z "$real" ]; then
  real="$(command -v claude 2>/dev/null || true)"
fi
if [ -z "$real" ]; then
  echo "alexandria dario claude shim: claude not found" >&2
  exit 127
fi
has_model=0
pass_through=0
for arg in "$@"; do
  case "$arg" in
    --model|--model=*) has_model=1 ;;
    --version) pass_through=1 ;;
  esac
done
unset NODE_OPTIONS
if [ "$pass_through" = "1" ] || [ "$has_model" = "1" ]; then
  exec "$real" "$@"
fi
if [ -n "$model" ]; then
  exec "$real" --model "$model" "$@"
fi
exec "$real" "$@"
"#
        .to_string()
    }
}

fn fetch_capture_preload() -> &'static str {
    r#"'use strict';
const fs = require('node:fs');
const path = require('node:path');
const zlib = require('node:zlib');
const http = require('node:http');
const childProcess = require('node:child_process');
const crypto = require('node:crypto');
const { AsyncLocalStorage } = require('node:async_hooks');
const { syncBuiltinESMExports } = require('node:module');

const captureDir = process.env.ALEXANDRIA_DARIO_CAPTURE_DIR || '';
const promptCacheDir = process.env.ALEXANDRIA_DARIO_PROMPT_CACHE_DIR || '';
const idPattern = /^[A-Za-z0-9._-]{1,128}$/;
const modelPattern = /^[A-Za-z0-9._:[\]-]{1,160}$/;
const promptCacheTtlMs = 24 * 60 * 60 * 1000;
const clientSystemPreface = '\n\n---\n\nIMPORTANT: The operator of this session has supplied the following task-specific instructions.';
const als = new AsyncLocalStorage();
const promptCaptures = new Map();

function capturePath(id, kind) {
  const date = new Date().toISOString().slice(0, 10);
  return path.join(captureDir, date, `${id}.${kind}.json.gz`);
}

function captureHeaders(headers) {
  const out = {};
  try {
    const h = new Headers(headers || {});
    h.forEach((value, name) => {
      const key = String(name).toLowerCase();
      out[key] = ['authorization', 'x-api-key', 'cookie', 'set-cookie'].includes(key)
        ? '<redacted>'
        : String(value);
    });
  } catch {
    for (const [name, value] of Object.entries(headers || {})) {
      const key = String(name).toLowerCase();
      out[key] = ['authorization', 'x-api-key', 'cookie', 'set-cookie'].includes(key)
        ? '<redacted>'
        : String(value);
    }
  }
  return out;
}

function captureBody(body) {
  if (body == null) return null;
  let text;
  if (typeof body === 'string') {
    text = body;
  } else if (Buffer.isBuffer(body)) {
    text = body.toString('utf8');
  } else if (body instanceof Uint8Array) {
    text = Buffer.from(body).toString('utf8');
  } else {
    return `[${body.constructor?.name || 'body'}]`;
  }
  try {
    return JSON.parse(text);
  } catch {
    return text;
  }
}

function writeCapture(id, kind, payload) {
  if (!captureDir || !idPattern.test(id)) return;
  const file = capturePath(id, kind);
  try {
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(file, zlib.gzipSync(Buffer.from(JSON.stringify(payload, null, 2), 'utf8')));
  } catch (err) {
    try { process.stderr.write(`[alexandria] dario capture failed: ${err.message}\n`); } catch {}
  }
}

function firstHeader(req, name) {
  const raw = req?.headers?.[name];
  return Array.isArray(raw) ? raw[0] : raw;
}

function normalizeCaptureModel(model) {
  if (typeof model !== 'string') return null;
  const trimmed = model.trim();
  if (!trimmed || !modelPattern.test(trimmed)) return null;
  return trimmed.replace(/\[[^\]]+\]$/, '');
}

function requestCaptureContext(req) {
  const rawId = firstHeader(req, 'x-dario-capture-id');
  const id = typeof rawId === 'string' && idPattern.test(rawId) ? rawId : null;
  if (!id) return null;
  return {
    id,
    model: normalizeCaptureModel(firstHeader(req, 'x-dario-capture-model')),
    attempt: 0,
  };
}

function promptCacheKey(model) {
  const slug = model
    .toLowerCase()
    .replace(/[^a-z0-9._-]+/g, '-')
    .replace(/^-+|-+$/g, '')
    .slice(0, 80) || 'model';
  const hash = crypto.createHash('sha256').update(model).digest('hex').slice(0, 12);
  return `${slug}-${hash}`;
}

function promptCachePath(model) {
  return path.join(promptCacheDir, `${promptCacheKey(model)}.json`);
}

function readJsonFile(file) {
  try {
    return JSON.parse(fs.readFileSync(file, 'utf8'));
  } catch {
    return null;
  }
}

function writeJsonFile(file, data) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  const tmp = `${file}.${process.pid}.tmp`;
  fs.writeFileSync(tmp, JSON.stringify(data, null, 2));
  fs.renameSync(tmp, file);
}

function promptCacheMeta(entry, file, status, applied, error) {
  return {
    key: entry?.key,
    model: entry?.model,
    status,
    applied,
    path: file,
    captured_at: entry?.captured_at,
    last_used_at: entry?.last_used_at,
    system_prompt_chars: entry?.system_prompt_chars,
    agent_identity_chars: entry?.agent_identity_chars,
    claude_version: entry?.claude_version,
    error: error ? String(error) : undefined,
  };
}

function freshPromptCache(model) {
  if (!promptCacheDir || !model) return null;
  const file = promptCachePath(model);
  const entry = readJsonFile(file);
  if (!entry || typeof entry.system_prompt !== 'string') return null;
  const capturedMs = Date.parse(entry.captured_at || '');
  if (Number.isFinite(capturedMs) && Date.now() - capturedMs > promptCacheTtlMs) return null;
  return { entry, file };
}

function recordPromptCacheUse(model, traceId, status, error) {
  if (!promptCacheDir || !model) return null;
  const file = promptCachePath(model);
  const entry = readJsonFile(file);
  if (!entry) return null;
  const run = {
    trace_id: traceId,
    used_at: new Date().toISOString(),
    status,
    error: error ? String(error) : undefined,
  };
  entry.last_used_at = run.used_at;
  entry.runs = Array.isArray(entry.runs) ? entry.runs : [];
  entry.runs.push(run);
  entry.runs = entry.runs.slice(-50);
  try {
    writeJsonFile(file, entry);
  } catch (err) {
    try { process.stderr.write(`[alexandria] dario prompt-cache use failed: ${err.message}\n`); } catch {}
  }
  return { entry, file };
}

function findRealClaude() {
  if (process.env.ALEXANDRIA_REAL_CLAUDE_BIN) return process.env.ALEXANDRIA_REAL_CLAUDE_BIN;
  const pathEnv = process.env.PATH || '';
  const sep = process.platform === 'win32' ? ';' : ':';
  const names = process.platform === 'win32' ? ['claude.exe', 'claude.cmd', 'claude'] : ['claude'];
  for (const dir of pathEnv.split(sep).filter(Boolean)) {
    for (const name of names) {
      const candidate = path.join(dir, name);
      try {
        if (fs.existsSync(candidate)) return candidate;
      } catch {}
    }
  }
  return 'claude';
}

function pickTextBlock(block) {
  if (!block) return '';
  if (typeof block.text === 'string') return block.text;
  if (Array.isArray(block.content)) {
    return block.content
      .filter((part) => part && part.type === 'text' && typeof part.text === 'string')
      .map((part) => part.text)
      .join('\n');
  }
  return '';
}

function extractClaudeVersion(headers) {
  const billing = headers?.['x-anthropic-billing-header'] || '';
  const userAgent = headers?.['user-agent'] || '';
  const fromBilling = /cc_version=([^;]+)/.exec(billing);
  if (fromBilling) return fromBilling[1];
  const fromUa = /claude-cli\/([^\s]+)/.exec(userAgent);
  return fromUa ? fromUa[1] : undefined;
}

async function capturePromptForModel(model, traceId) {
  const key = promptCacheKey(model);
  const file = promptCachePath(model);
  const claudeBin = findRealClaude();
  const captured = await new Promise((resolve) => {
    let child;
    let done = false;
    const settle = (value) => {
      if (done) return;
      done = true;
      try { server.close(); } catch {}
      try { child?.kill('SIGTERM'); } catch {}
      resolve(value);
    };
    const server = http.createServer((req, res) => {
      if (!req.url?.includes('/v1/messages')) {
        res.writeHead(404, { 'content-type': 'application/json' });
        res.end('{"type":"error","error":{"type":"not_found_error","message":"not found"}}');
        return;
      }
      const chunks = [];
      req.on('data', (chunk) => chunks.push(chunk));
      req.on('end', () => {
        let body = null;
        try {
          body = JSON.parse(Buffer.concat(chunks).toString('utf8'));
        } catch {}
        const headers = {};
        for (const [name, value] of Object.entries(req.headers || {})) {
          headers[String(name).toLowerCase()] = Array.isArray(value) ? value.join(',') : String(value);
        }
        res.writeHead(200, {
          'content-type': 'text/event-stream',
          'cache-control': 'no-cache',
          connection: 'keep-alive',
          'anthropic-ratelimit-unified-status': 'allowed',
        });
        res.end([
          `event: message_start\ndata: ${JSON.stringify({ type: 'message_start', message: { id: 'msg_alexandria_capture', type: 'message', role: 'assistant', model, content: [], stop_reason: null, stop_sequence: null, usage: { input_tokens: 1, output_tokens: 1 } } })}\n\n`,
          `event: content_block_start\ndata: ${JSON.stringify({ type: 'content_block_start', index: 0, content_block: { type: 'text', text: '' } })}\n\n`,
          `event: content_block_delta\ndata: ${JSON.stringify({ type: 'content_block_delta', index: 0, delta: { type: 'text_delta', text: 'ok' } })}\n\n`,
          `event: content_block_stop\ndata: ${JSON.stringify({ type: 'content_block_stop', index: 0 })}\n\n`,
          `event: message_delta\ndata: ${JSON.stringify({ type: 'message_delta', delta: { stop_reason: 'end_turn', stop_sequence: null }, usage: { output_tokens: 1 } })}\n\n`,
          `event: message_stop\ndata: ${JSON.stringify({ type: 'message_stop' })}\n\n`,
        ].join(''));
        setTimeout(() => settle({ body, headers }), 100);
      });
    });
    server.on('error', () => settle(null));
    server.listen(0, '127.0.0.1', () => {
      const addr = server.address();
      if (!addr || typeof addr === 'string') return settle(null);
      const env = {
        ...process.env,
        ANTHROPIC_BASE_URL: `http://127.0.0.1:${addr.port}`,
        ANTHROPIC_API_KEY: process.env.ANTHROPIC_API_KEY || 'sk-dario-fingerprint-capture',
        CLAUDE_NONINTERACTIVE: '1',
        ALEXANDRIA_DARIO_CAPTURE_MODEL: model,
      };
      delete env.NODE_OPTIONS;
      delete env.DARIO_CLAUDE_BIN;
      const useShell = process.platform === 'win32' && /\.(cmd|bat)$/i.test(claudeBin);
      try {
        child = childProcess.spawn(claudeBin, ['--model', model, '--print', '-p', 'hi'], {
          env,
          cwd: process.env.ALEXANDRIA_DARIO_WORK_DIR || process.cwd(),
          stdio: ['ignore', 'ignore', 'ignore'],
          windowsHide: true,
          shell: useShell,
        });
        child.on('error', () => settle(null));
        child.on('exit', () => setTimeout(() => settle(null), 200));
      } catch {
        settle(null);
      }
    });
    setTimeout(() => settle(null), 15_000);
  });

  const system = captured?.body?.system;
  const systemPrompt = Array.isArray(system) ? pickTextBlock(system[2]) : '';
  const agentIdentity = Array.isArray(system) ? pickTextBlock(system[1]) : '';
  if (!systemPrompt) throw new Error(`no system prompt captured for ${model}`);
  const now = new Date().toISOString();
  const entry = {
    key,
    model,
    source: 'alexandria-claude-live-capture',
    captured_at: now,
    last_used_at: now,
    trace_id: traceId,
    claude_bin: claudeBin,
    claude_version: extractClaudeVersion(captured.headers),
    system_prompt_chars: systemPrompt.length,
    agent_identity_chars: agentIdentity.length || undefined,
    system_prompt: systemPrompt,
    agent_identity: agentIdentity || undefined,
    runs: [{ trace_id: traceId, used_at: now, status: 'refreshed' }],
  };
  writeJsonFile(file, entry);
  return { entry, file };
}

async function ensurePromptCache(model, traceId) {
  if (!promptCacheDir || !model) {
    return { systemPrompt: null, agentIdentity: null, meta: promptCacheMeta(null, null, 'disabled', false) };
  }
  const cached = freshPromptCache(model);
  if (cached) {
    const used = recordPromptCacheUse(model, traceId, 'hit') || cached;
    return {
      systemPrompt: used.entry.system_prompt,
      agentIdentity: used.entry.agent_identity,
      meta: promptCacheMeta(used.entry, used.file, 'hit', false),
    };
  }

  if (!promptCaptures.has(model)) {
    promptCaptures.set(model, capturePromptForModel(model, traceId)
      .finally(() => promptCaptures.delete(model)));
  }
  try {
    const refreshed = await promptCaptures.get(model);
    return {
      systemPrompt: refreshed.entry.system_prompt,
      agentIdentity: refreshed.entry.agent_identity,
      meta: promptCacheMeta(refreshed.entry, refreshed.file, 'refreshed', false),
    };
  } catch (err) {
    return {
      systemPrompt: null,
      agentIdentity: null,
      meta: promptCacheMeta({ key: promptCacheKey(model), model }, promptCachePath(model), 'failed', false, err.message),
    };
  }
}

function applyPromptCache(body, cache) {
  if (!cache?.systemPrompt || !body || typeof body !== 'object') return false;
  if (!Array.isArray(body.system) || body.system.length < 3) return false;
  const promptBlock = body.system[2];
  if (!promptBlock || typeof promptBlock.text !== 'string') return false;
  const original = promptBlock.text;
  const prefaceIndex = original.indexOf(clientSystemPreface);
  const suffix = prefaceIndex >= 0 ? original.slice(prefaceIndex) : '';
  promptBlock.text = `${cache.systemPrompt}${suffix}`;
  if (cache.agentIdentity && body.system[1] && typeof body.system[1].text === 'string') {
    body.system[1].text = cache.agentIdentity;
  }
  return true;
}

const originalCreateServer = http.createServer;
http.createServer = function patchedCreateServer(...args) {
  const idx = args.findIndex((arg) => typeof arg === 'function');
  if (idx >= 0) {
    const listener = args[idx];
    args[idx] = function wrappedRequestListener(req, res) {
      const ctx = requestCaptureContext(req);
      if (!ctx) return listener.call(this, req, res);
      return als.run(ctx, () => listener.call(this, req, res));
    };
  }
  return originalCreateServer.apply(this, args);
};
try { syncBuiltinESMExports(); } catch {}

function requestUrl(input) {
  if (typeof input === 'string') return input;
  if (input && typeof input.href === 'string') return input.href;
  if (input && typeof input.url === 'string') return input.url;
  return '';
}

const originalFetch = globalThis.fetch;
if (typeof originalFetch === 'function') {
  globalThis.fetch = async function patchedFetch(input, init = undefined) {
    const store = als.getStore();
    const url = requestUrl(input);
    if (!store || !captureDir || !url.startsWith('https://api.anthropic.com/')) {
      return originalFetch.call(this, input, init);
    }

    const attempt = ++store.attempt;
    let fetchInit = init;
    let requestBody = captureBody(init?.body);
    const model = normalizeCaptureModel(store.model || requestBody?.model);
    let promptCache = await ensurePromptCache(model, store.id);
    if (requestBody && typeof requestBody === 'object') {
      const applied = applyPromptCache(requestBody, promptCache);
      promptCache.meta.applied = applied;
      if (applied) {
        const headers = new Headers(init?.headers ?? input?.headers ?? {});
        headers.delete('content-length');
        fetchInit = { ...(init || {}), headers, body: JSON.stringify(requestBody) };
      }
    }

    const headers = fetchInit?.headers ?? input?.headers ?? {};
    writeCapture(store.id, 'dario-upstream-request', {
      trace_id: store.id,
      captured_at: new Date().toISOString(),
      direction: 'dario->anthropic',
      attempt,
      method: fetchInit?.method ?? input?.method ?? 'GET',
      url,
      headers: captureHeaders(headers),
      prompt_cache: promptCache.meta,
      body: requestBody,
    });

    const response = await originalFetch.call(this, input, fetchInit);
    try {
      const clone = response.clone();
      clone.text().then((text) => {
        writeCapture(store.id, 'dario-upstream-response', {
          trace_id: store.id,
          captured_at: new Date().toISOString(),
          direction: 'anthropic->dario',
          attempt,
          status: response.status,
          headers: captureHeaders(response.headers),
          body: captureBody(text),
        });
      }).catch(() => {});
    } catch {}
    return response;
  };
}
"#
}

fn write_if_changed(path: &Path, contents: impl AsRef<[u8]>) -> Result<()> {
    let contents = contents.as_ref();
    if std::fs::read(path)
        .map(|current| current == contents)
        .unwrap_or(false)
    {
        return Ok(());
    }
    std::fs::write(path, contents).with_context(|| format!("writing {path:?}"))
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms)?;
    Ok(())
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<()> {
    Ok(())
}

fn node_options_with_require(preload: &Path) -> String {
    let require = format!("--require={}", preload.to_string_lossy());
    match std::env::var("NODE_OPTIONS") {
        Ok(existing) if !existing.trim().is_empty() => format!("{existing} {require}"),
        _ => require,
    }
}

fn quarantine_fable_live_cache() {
    let Some(home) = dirs::home_dir() else {
        return;
    };
    let path = home.join(".dario").join("cc-template.live.json");
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return;
    };
    let Ok(value) = serde_json::from_str::<Value>(&raw) else {
        return;
    };
    if value["system_prompt_fable"]
        .as_str()
        .map(|s| !s.is_empty())
        .unwrap_or(false)
    {
        return;
    }
    let prompt = value["system_prompt"].as_str().unwrap_or("");
    let contaminated = prompt.contains("This iteration of Claude is Claude Fable 5")
        || prompt.contains("exact model ID is claude-fable-5");
    if !contaminated {
        return;
    }
    let quarantine = path.with_file_name(format!(
        "cc-template.live.json.alexandria-quarantine-{}",
        now_ms()
    ));
    match std::fs::rename(&path, &quarantine) {
        Ok(()) => tracing::warn!(
            path = %path.display(),
            quarantine = %quarantine.display(),
            "quarantined Dario live template captured from Fable default model"
        ),
        Err(e) => tracing::warn!(
            path = %path.display(),
            error = %e,
            "could not quarantine Fable-contaminated Dario live template"
        ),
    }
}

fn generation_id(version: &str, port: u16) -> String {
    format!("gen-{version}-{port}")
}

#[cfg(unix)]
fn process_alive(pid: i32) -> bool {
    unsafe { libc_kill(pid, 0) == 0 }
}

#[cfg(unix)]
extern "C" {
    #[link_name = "kill"]
    fn libc_kill(pid: i32, sig: i32) -> i32;
}

#[cfg(not(unix))]
fn process_alive(pid: i32) -> bool {
    std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains(&format!("\"{pid}\"")))
        .unwrap_or(false)
}

fn terminate_pid(pid: i32) {
    #[cfg(unix)]
    let _ = std::process::Command::new("kill")
        .arg("-TERM")
        .arg(pid.to_string())
        .output();
    #[cfg(not(unix))]
    let _ = std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string()])
        .output();
}

fn alloc_port() -> Result<u16> {
    let listener =
        std::net::TcpListener::bind("127.0.0.1:0").context("allocating ephemeral port")?;
    Ok(listener.local_addr()?.port())
}

async fn send_sigterm(pid: u32) {
    #[cfg(unix)]
    let _ = Command::new("kill")
        .arg("-TERM")
        .arg(pid.to_string())
        .status()
        .await;
    #[cfg(not(unix))]
    let _ = Command::new("taskkill")
        .args(["/PID", &pid.to_string()])
        .status()
        .await;
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn ms_or_null(v: i64) -> Value {
    if v > 0 {
        json!(v)
    } else {
        Value::Null
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    const FAST: Duration = Duration::from_millis(1_000);

    fn test_gen(id: &str) -> Arc<Generation> {
        Arc::new(Generation::new(
            id.to_string(),
            "1.0.0".to_string(),
            1234,
            42,
            PathBuf::from("/tmp/out.log"),
            PathBuf::from("/tmp/err.log"),
            1_000,
        ))
    }

    fn request_complete(buf: &[u8]) -> bool {
        let text = String::from_utf8_lossy(buf);
        let Some(idx) = text.find("\r\n\r\n") else {
            return false;
        };
        let body_len = text[..idx]
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.trim()
                    .eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())?
            })
            .unwrap_or(0);
        buf.len() >= idx + 4 + body_len
    }

    async fn read_request(sock: &mut TcpStream) -> String {
        let mut buf = Vec::new();
        let mut tmp = [0u8; 1024];
        loop {
            match sock.read(&mut tmp).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    buf.extend_from_slice(&tmp[..n]);
                    if request_complete(&buf) {
                        break;
                    }
                }
            }
        }
        String::from_utf8_lossy(&buf).to_string()
    }

    async fn write_response(sock: &mut TcpStream, status: u16, body: &str) {
        let resp = format!(
            "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = sock.write_all(resp.as_bytes()).await;
        let _ = sock.shutdown().await;
    }

    async fn one_shot_server(status: u16, body: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let _ = read_request(&mut sock).await;
                write_response(&mut sock, status, body).await;
            }
        });
        format!("http://{addr}")
    }

    #[test]
    fn allocates_bindable_port() {
        let port = alloc_port().unwrap();
        assert!(port > 0);
        std::net::TcpListener::bind(("127.0.0.1", port)).unwrap();
    }

    #[test]
    fn generation_id_format() {
        assert_eq!(generation_id("4.8.139", 5555), "gen-4.8.139-5555");
    }

    #[test]
    fn version_keys_are_numeric_and_reject_tags() {
        assert_eq!(version_key("4.8.139"), Some(vec![4, 8, 139]));
        assert_eq!(version_key("4.10.2"), Some(vec![4, 10, 2]));
        assert_eq!(version_key("latest"), None);
        assert_eq!(version_key("4.8.139-beta.1"), None);
    }

    #[test]
    fn dario_entrypoint_is_package_manager_independent() {
        assert_eq!(
            dario_entrypoint(Path::new("/tmp/dario/4.8.139")),
            PathBuf::from("/tmp/dario/4.8.139/node_modules/@askalf/dario/dist/cli.js")
        );
    }

    #[test]
    fn package_manager_commands_target_the_private_prefix() {
        let prefix = Path::new("/tmp/alex dario/4.8.139");
        let spec = "@askalf/dario@4.8.139";
        let strings = |kind| {
            package_manager_args(kind, prefix, spec)
                .into_iter()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            strings(PackageManagerKind::Npm),
            vec![
                "install",
                spec,
                "--prefix",
                "/tmp/alex dario/4.8.139",
                "--omit=dev",
                "--no-audit",
                "--no-fund"
            ]
        );
        assert_eq!(
            strings(PackageManagerKind::Pnpm),
            vec![
                "--dir",
                "/tmp/alex dario/4.8.139",
                "add",
                "--prod",
                "--save-exact",
                spec
            ]
        );
        assert_eq!(
            strings(PackageManagerKind::Bun),
            vec![
                "add",
                "--cwd",
                "/tmp/alex dario/4.8.139",
                "--production",
                "--exact",
                spec
            ]
        );
    }

    #[test]
    fn claude_prompt_capture_uses_private_working_directory() {
        let preload = fetch_capture_preload();
        assert!(preload.contains("cwd: process.env.ALEXANDRIA_DARIO_WORK_DIR"));
    }

    #[test]
    fn promote_sets_active_and_drains_old() {
        let old = test_gen("gen-1.0.0-1111");
        let newer = test_gen("gen-1.0.1-2222");
        promote_bookkeeping(&old, None, 10);
        assert_eq!(old.state(), GenState::Active);
        assert_eq!(old.promoted_at.load(Ordering::SeqCst), 10);
        assert_eq!(old.drain_started_at.load(Ordering::SeqCst), 0);
        promote_bookkeeping(&newer, Some(&old), 20);
        assert_eq!(newer.state(), GenState::Active);
        assert_eq!(newer.promoted_at.load(Ordering::SeqCst), 20);
        assert_eq!(old.state(), GenState::Draining);
        assert_eq!(old.drain_started_at.load(Ordering::SeqCst), 20);
        assert_eq!(old.last_activity_ms.load(Ordering::SeqCst), 20);
    }

    #[test]
    fn promote_same_generation_does_not_self_drain() {
        let gen = test_gen("gen-1.0.0-1111");
        promote_bookkeeping(&gen, None, 10);
        promote_bookkeeping(&gen, Some(&gen), 20);
        assert_eq!(gen.state(), GenState::Active);
    }

    #[test]
    fn in_flight_guard_increments_and_decrements() {
        let gen = test_gen("gen-1.0.0-1111");
        gen.set_state(GenState::Active);
        let g1 = gen.try_begin().unwrap();
        let g2 = gen.try_begin().unwrap();
        assert_eq!(gen.in_flight.load(Ordering::SeqCst), 2);
        drop(g1);
        assert_eq!(gen.in_flight.load(Ordering::SeqCst), 1);
        assert!(gen.last_activity_ms.load(Ordering::SeqCst) > 1_000);
        drop(g2);
        assert_eq!(gen.in_flight.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn in_flight_guard_allowed_only_when_active_or_draining() {
        let gen = test_gen("gen-1.0.0-1111");
        assert!(gen.try_begin().is_none());
        gen.set_state(GenState::Active);
        assert!(gen.try_begin().is_some());
        gen.set_state(GenState::Draining);
        assert!(gen.try_begin().is_some());
        gen.set_state(GenState::Failed);
        assert!(gen.try_begin().is_none());
        gen.set_state(GenState::Stopped);
        assert!(gen.try_begin().is_none());
        assert_eq!(gen.in_flight.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn reaper_keeps_busy_generation() {
        assert_eq!(
            reap_action(100_000, 3, 99_000, 50_000, 0, false),
            ReapAction::Keep
        );
    }

    #[test]
    fn reaper_keeps_recently_idle_generation() {
        assert_eq!(
            reap_action(100_000, 0, 95_000, 50_000, 0, false),
            ReapAction::Keep
        );
    }

    #[test]
    fn reaper_sigterms_idle_generation() {
        assert_eq!(
            reap_action(100_000, 0, 80_000, 50_000, 0, false),
            ReapAction::Sigterm
        );
    }

    #[test]
    fn reaper_sigterms_overlong_drain_despite_in_flight() {
        let now = 1_000_000;
        assert_eq!(
            reap_action(now, 5, now - 1_000, now - DRAIN_MAX_MS - 1, 0, false),
            ReapAction::Sigterm
        );
    }

    #[test]
    fn reaper_waits_then_kills_after_sigterm() {
        let now = 1_000_000;
        assert_eq!(
            reap_action(now, 0, 0, now - 60_000, now - 5_000, false),
            ReapAction::Keep
        );
        assert_eq!(
            reap_action(now, 0, 0, now - 60_000, now - KILL_GRACE_MS - 1, false),
            ReapAction::Kill
        );
    }

    #[test]
    fn reaper_short_leash_for_unhealthy_drain() {
        let now = 1_000_000;
        assert_eq!(
            reap_action(
                now,
                3,
                now - 1_000,
                now - UNHEALTHY_DRAIN_MAX_MS - 1,
                0,
                true
            ),
            ReapAction::Sigterm
        );
        assert_eq!(
            reap_action(
                now,
                3,
                now - 1_000,
                now - UNHEALTHY_DRAIN_MAX_MS + 5_000,
                0,
                true
            ),
            ReapAction::Keep
        );
        assert_eq!(
            reap_action(
                now,
                3,
                now - 1_000,
                now - UNHEALTHY_DRAIN_MAX_MS - 1,
                0,
                false
            ),
            ReapAction::Keep
        );
        assert_eq!(
            reap_action(
                now,
                3,
                now - 1_000,
                now - 60_000,
                now - KILL_GRACE_MS - 1,
                true
            ),
            ReapAction::Kill
        );
    }

    #[test]
    fn escalation_boundaries() {
        assert_eq!(probe_escalation(0, 2), ProbeEscalation::Keep);
        assert_eq!(probe_escalation(1, 2), ProbeEscalation::Keep);
        assert_eq!(probe_escalation(2, 2), ProbeEscalation::Replace);
        assert_eq!(probe_escalation(3, 2), ProbeEscalation::Replace);
        assert_eq!(probe_escalation(1, 1), ProbeEscalation::Replace);
        assert_eq!(probe_escalation(5, 0), ProbeEscalation::Keep);
    }

    #[test]
    fn phase_mapping() {
        assert_eq!(phase_name(GenState::Warming, false), "starting");
        assert_eq!(phase_name(GenState::Warming, true), "starting");
        assert_eq!(phase_name(GenState::Active, false), "ready");
        assert_eq!(phase_name(GenState::Active, true), "unhealthy");
        assert_eq!(phase_name(GenState::Draining, false), "draining");
        assert_eq!(phase_name(GenState::Draining, true), "draining");
        assert_eq!(phase_name(GenState::Failed, false), "dead");
        assert_eq!(phase_name(GenState::Stopped, true), "dead");
    }

    #[test]
    fn suspect_debounce_boundaries() {
        assert!(should_probe(10_000, 0, false));
        assert!(should_probe(10_000, 5_000, false));
        assert!(!should_probe(10_000, 6_000, false));
        assert!(!should_probe(10_000, 0, true));
    }

    #[test]
    fn record_probe_tracks_consecutive_failures() {
        let gen = test_gen("gen-1.0.0-1111");
        let wedged = ProbeOutcome::Wedged {
            error: "timeout".to_string(),
        };
        assert_eq!(gen.record_probe(&wedged, 5, 100), 1);
        assert_eq!(gen.record_probe(&wedged, 5, 200), 2);
        assert_eq!(gen.record_probe(&ProbeOutcome::Ready, 5, 300), 0);
        assert_eq!(gen.record_probe(&wedged, 5, 400), 1);
        let last = gen.last_probe.lock().unwrap().clone().unwrap();
        assert_eq!(last.at_ms, 400);
        assert_eq!(last.outcome, wedged);
    }

    #[test]
    fn status_json_includes_probe_fields() {
        let gen = test_gen("gen-1.0.0-1111");
        gen.set_state(GenState::Active);
        gen.record_probe(
            &ProbeOutcome::Unhealthy {
                status: 429,
                body_snippet: "rate limited".to_string(),
            },
            12,
            500,
        );
        let v = gen.status_json();
        assert_eq!(v["phase"], "ready");
        assert_eq!(v["consecutive_failures"], 1);
        assert_eq!(v["last_probe"]["ok"], false);
        assert_eq!(v["last_probe"]["status"], 429);
        assert_eq!(v["last_probe"]["at_ms"], 500);
        assert_eq!(v["last_probe"]["latency_ms"], 12);
        assert_eq!(v["state"], "active");
        gen.unhealthy.store(true, Ordering::SeqCst);
        assert_eq!(gen.status_json()["phase"], "unhealthy");
    }

    #[tokio::test]
    async fn probe_ready_on_2xx() {
        let base = one_shot_server(200, "{\"id\":\"probe\"}").await;
        let (outcome, latency_ms) = probe_messages(&base, "key", "model", FAST, FAST).await;
        assert_eq!(outcome, ProbeOutcome::Ready);
        assert!(latency_ms >= 0);
    }

    #[tokio::test]
    async fn probe_unhealthy_on_401() {
        let base = one_shot_server(401, "{\"error\":\"unauthorized\"}").await;
        let (outcome, _) = probe_messages(&base, "key", "model", FAST, FAST).await;
        match outcome {
            ProbeOutcome::Unhealthy {
                status,
                body_snippet,
            } => {
                assert_eq!(status, 401);
                assert!(body_snippet.contains("unauthorized"));
            }
            other => panic!("expected Unhealthy, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn probe_wedged_on_hung_connection() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((sock, _)) = listener.accept().await {
                held.push(sock);
            }
        });
        let (outcome, _) = probe_messages(
            &format!("http://{addr}"),
            "key",
            "model",
            Duration::from_millis(500),
            FAST,
        )
        .await;
        assert!(matches!(outcome, ProbeOutcome::Wedged { .. }));
    }

    #[tokio::test]
    async fn probe_wedged_on_connection_refused() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let (outcome, _) =
            probe_messages(&format!("http://{addr}"), "key", "model", FAST, FAST).await;
        assert!(matches!(outcome, ProbeOutcome::Wedged { .. }));
    }

    #[tokio::test]
    async fn health_lies_liveness_passes_readiness_fails() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let req = read_request(&mut sock).await;
                    if req.starts_with("GET /health") {
                        write_response(&mut sock, 200, "ok").await;
                    } else {
                        tokio::time::sleep(Duration::from_secs(60)).await;
                    }
                });
            }
        });
        let base = format!("http://{addr}");
        let client = reqwest::Client::new();
        let resp = client
            .get(format!("{base}/health"))
            .timeout(Duration::from_secs(2))
            .send()
            .await
            .unwrap();
        assert!(resp.status().is_success());
        let (outcome, _) =
            probe_messages(&base, "key", "model", Duration::from_millis(500), FAST).await;
        assert!(matches!(outcome, ProbeOutcome::Wedged { .. }));
    }
}
