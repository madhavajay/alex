#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, ensure, Context, Result};
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
    pub api_key: String,
    pub update_check_minutes: u64,
    pub version_pin: Option<String>,
    pub probe_seconds: u64,
    pub probe_failures: u32,
    pub probe_model: String,
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
        let supervisor = Arc::new(Self {
            settings,
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
        let version = match &supervisor.settings.version_pin {
            Some(pin) => pin.clone(),
            None => supervisor.npm_latest().await?,
        };
        supervisor
            .roll(&version)
            .await
            .context("starting initial dario generation")?;
        supervisor.spawn_reaper();
        if supervisor.settings.update_check_minutes > 0 {
            supervisor.spawn_update_loop();
        }
        if supervisor.settings.probe_seconds > 0 {
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
        let prefix = self.settings.install_root.join(version);
        let bin = prefix.join("node_modules").join(".bin").join("dario");
        if bin.exists() {
            return Ok(bin);
        }
        tokio::fs::create_dir_all(&prefix).await?;
        let output = Command::new("npm")
            .arg("install")
            .arg(format!("{NPM_PACKAGE}@{version}"))
            .arg("--prefix")
            .arg(&prefix)
            .stdin(Stdio::null())
            .output()
            .await
            .context("running npm install")?;
        ensure!(
            output.status.success(),
            "npm install {NPM_PACKAGE}@{version} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        ensure!(bin.exists(), "npm install succeeded but {bin:?} is missing");
        Ok(bin)
    }

    async fn npm_latest(&self) -> Result<String> {
        let body: Value = self
            .http
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

    fn spawn_generation(&self, version: &str, bin: &Path) -> Result<Arc<Generation>> {
        let port = alloc_port()?;
        let id = generation_id(version, port);
        let stdout_log = self.settings.log_root.join(format!("{id}.out.log"));
        let stderr_log = self.settings.log_root.join(format!("{id}.err.log"));
        let out = std::fs::File::create(&stdout_log)
            .with_context(|| format!("creating {stdout_log:?}"))?;
        let err = std::fs::File::create(&stderr_log)
            .with_context(|| format!("creating {stderr_log:?}"))?;
        let child = Command::new(bin)
            .arg("proxy")
            .arg("--host=127.0.0.1")
            .arg(format!("--port={port}"))
            .env("DARIO_API_KEY", &self.settings.api_key)
            .stdin(Stdio::null())
            .stdout(Stdio::from(out))
            .stderr(Stdio::from(err))
            .spawn()
            .with_context(|| format!("spawning {bin:?}"))?;
        let pid = child.id().unwrap_or(0);
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

fn generation_id(version: &str, port: u16) -> String {
    format!("gen-{version}-{port}")
}

fn alloc_port() -> Result<u16> {
    let listener =
        std::net::TcpListener::bind("127.0.0.1:0").context("allocating ephemeral port")?;
    Ok(listener.local_addr()?.port())
}

async fn send_sigterm(pid: u32) {
    let _ = Command::new("kill")
        .arg("-TERM")
        .arg(pid.to_string())
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
