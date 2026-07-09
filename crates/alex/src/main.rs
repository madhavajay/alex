use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use alex_auth::{import_all, now_ms, Vault};
use alex_store::Store;
use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use rand::Rng;
use serde::{Deserialize, Serialize};

mod dario;
mod harness_connect;
mod harness_e2e;
mod light;
mod selfupdate;
mod tui;
mod ui;

#[derive(Parser)]
#[command(
    name = "alexandria",
    version,
    about = "LLM credential vault + routing proxy + trace capture"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the proxy daemon in the foreground
    Daemon {
        #[arg(long)]
        host: Option<String>,
        #[arg(long)]
        port: Option<u16>,
        /// Skip the first-run splash animation
        #[arg(long)]
        nosplash: bool,
        /// Detach and run in the background, logging to ~/.alexandria/daemon.log
        #[arg(long)]
        background: bool,
    },
    /// Credential vault operations
    Auth {
        #[command(subcommand)]
        command: AuthCommand,
    },
    /// Query captured traces
    Traces {
        #[command(subcommand)]
        command: Option<TracesCommand>,
        #[arg(long, default_value_t = 20)]
        limit: usize,
        #[arg(long)]
        session: Option<String>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Print env exports for pointing harnesses at the daemon
    Env,
    /// Connect an installed AI harness to this daemon (pi)
    Connect {
        /// Harness name; omit to show detection status
        harness: Option<String>,
        /// Override the harness config dir (default: ~/.pi/agent for pi)
        #[arg(long)]
        config_dir: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Remove a harness's alexandria config and revoke its keys
    Disconnect {
        harness: String,
        #[arg(long)]
        config_dir: Option<PathBuf>,
    },
    /// Fire a tiny test prompt through each provider to verify credentials
    Ping {
        #[arg(default_value = "all")]
        target: String,
    },
    /// Run frozen CLI harnesses in Docker against this proxy and verify traces
    Harness {
        #[command(subcommand)]
        command: HarnessCommand,
    },
    /// Inspect or control the dario generational proxy (requires a running daemon)
    Dario {
        #[command(subcommand)]
        command: DarioCommand,
    },
    /// Show subscription plans, limit-window utilization, and reset times
    Limits {
        #[arg(long)]
        json: bool,
    },
    /// Manage the OS user service (launchd on macOS, systemd on Linux)
    Service {
        #[command(subcommand)]
        command: ServiceCommand,
    },
    /// Check for and install a newer alex release
    Update {
        /// Only check and report; never install
        #[arg(long)]
        check: bool,
        /// Install without prompting
        #[arg(long, short = 'y')]
        yes: bool,
        /// Do not restart a running daemon after updating
        #[arg(long)]
        no_restart: bool,
        /// Machine-readable output for --check
        #[arg(long)]
        json: bool,
        /// Proceed even when the install looks brew- or cargo-managed
        #[arg(long)]
        force: bool,
    },
    /// Play the Pharos of Alexandria in your terminal (truecolor blocks)
    Light {
        #[arg(long, default_value_t = 2)]
        loops: u32,
        #[arg(long)]
        forever: bool,
        /// Tail this file and show its last lines under the animation
        #[arg(long)]
        follow: Option<PathBuf>,
    },
    /// Print client connection exports (fast; reads config only). Alias: creds
    #[command(alias = "creds")]
    Credentials {
        #[arg(long)]
        json: bool,
        /// Rewrite the host in emitted URLs (e.g. host.docker.internal)
        #[arg(long)]
        host: Option<String>,
    },
    /// One-shot overview: daemon, service, accounts, limits, dario
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Mint, list, and revoke ephemeral run keys (requires a running daemon)
    Keys {
        #[command(subcommand)]
        command: KeysCommand,
    },
    /// Live dashboard: traces, limits, accounts, dario generations
    Tui,
}

#[derive(Subcommand)]
enum KeysCommand {
    /// Mint a run key bound to run metadata; the key is printed exactly once
    Mint {
        #[arg(long)]
        run_id: Option<String>,
        /// Tag as k=v; repeatable
        #[arg(long = "tag")]
        tag: Vec<String>,
        /// Lifetime: seconds or relative (45s, 30m, 24h, 7d); capped at 7d
        #[arg(long, default_value = "24h")]
        ttl: String,
        #[arg(long)]
        label: Option<String>,
    },
    /// List run keys (active only by default)
    List {
        #[arg(long)]
        all: bool,
        #[arg(long)]
        json: bool,
    },
    /// Revoke a run key by id (or unique id prefix)
    Revoke { id: String },
}

#[derive(Subcommand)]
enum ServiceCommand {
    /// Install + start the OS user service pointing at this binary
    Install,
    /// Stop + remove the OS user service
    Uninstall,
    /// Show service state
    Status,
}

#[derive(Subcommand)]
enum DarioCommand {
    /// Show generations and their states
    Status,
    /// Roll to a fresh generation of the same version
    Restart,
    /// Check npm for a newer version and roll if found
    Update,
}

#[derive(Subcommand)]
enum AuthCommand {
    /// Import credentials from native tool locations (claude|codex|gemini|all)
    Import {
        #[arg(default_value = "all")]
        source: String,
    },
    /// Run an OAuth login flow from the terminal (claude|codex|grok|gemini); no arg opens a picker
    Login { provider: Option<String> },
    /// Register a Google AI Studio API key for Gemini (from aistudio.google.com/apikey)
    GeminiKey {
        /// The API key; omit to read from the GEMINI_API_KEY env var
        key: Option<String>,
    },
    /// List vault accounts
    List,
}

#[derive(Subcommand)]
enum HarnessCommand {
    /// List known harness smoke definitions
    List {
        #[arg(long)]
        json: bool,
    },
    /// Run one harness in a fresh Docker container
    Run {
        /// Harness name: claude|codex|grok
        harness: String,
        /// Model to request from the harness; prefixes route through Alexandria
        #[arg(long)]
        model: Option<String>,
        /// Prompt sent to the harness
        #[arg(long)]
        prompt: Option<String>,
        /// Frozen npm package tarball to install inside the container
        #[arg(long)]
        package_tarball: Option<PathBuf>,
        /// Docker image used for the smoke container
        #[arg(long)]
        docker_image: Option<String>,
        /// Base URL visible from inside Docker, without /v1
        #[arg(long)]
        container_base_url: Option<String>,
        /// Kill the container after this many seconds
        #[arg(long)]
        timeout_secs: Option<u64>,
        /// Skip SQLite/body capture verification
        #[arg(long)]
        no_trace_check: bool,
        #[arg(long)]
        json: bool,
    },
    /// Pack an npm CLI package into Alexandria's frozen harness cache
    Pack {
        /// Harness name or npm package name, for example claude or @anthropic-ai/claude-code
        target: String,
        /// npm package version; defaults to the harness catalog version, or latest for raw packages
        #[arg(long)]
        version: Option<String>,
        /// Recreate the tarball even if the cache file exists
        #[arg(long)]
        force: bool,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum TracesCommand {
    /// Search traces via the running daemon with time/run/model filters
    Search {
        #[command(flatten)]
        filter: TraceFilterArgs,
        #[arg(long)]
        json: bool,
    },
    /// Export traces as NDJSON (optionally with inlined base64 bodies)
    Export {
        #[command(flatten)]
        filter: TraceFilterArgs,
        #[arg(long)]
        bodies: bool,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Print data dir, sqlite path, and artifact paths for a run (offline)
    Path {
        #[arg(long)]
        run_id: String,
    },
    /// Delete old trace bodies + headers, and rows with --rows (offline)
    Prune {
        /// Cutoff: relative (45s, 30m, 24h, 30d) or RFC3339
        #[arg(long, default_value = "30d")]
        older_than: String,
        /// Only remove bodies/headers, keep rows (the default)
        #[arg(long, conflicts_with = "rows")]
        bodies_only: bool,
        /// Also delete trace rows older than the cutoff
        #[arg(long)]
        rows: bool,
        /// Report what would be removed without touching anything
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        json: bool,
    },
    /// Show sqlite + body file disk usage (offline)
    Du {
        #[arg(long)]
        json: bool,
    },
}

#[derive(clap::Args)]
struct TraceFilterArgs {
    /// RFC3339 timestamp or relative (30m, 2h, 7d, 45s)
    #[arg(long)]
    since: Option<String>,
    #[arg(long)]
    until: Option<String>,
    #[arg(long)]
    run_id: Option<String>,
    #[arg(long)]
    session: Option<String>,
    #[arg(long)]
    model: Option<String>,
    #[arg(long)]
    provider: Option<String>,
    #[arg(long)]
    path: Option<String>,
    #[arg(long)]
    harness: Option<String>,
    #[arg(long)]
    status: Option<i64>,
    #[arg(long)]
    errors: bool,
    #[arg(long)]
    key_fingerprint: Option<String>,
    #[arg(long)]
    limit: Option<usize>,
}

impl TraceFilterArgs {
    fn query_params(&self) -> Vec<(&'static str, String)> {
        let mut params: Vec<(&'static str, String)> = Vec::new();
        let opts = [
            ("since", &self.since),
            ("until", &self.until),
            ("run_id", &self.run_id),
            ("session", &self.session),
            ("model", &self.model),
            ("provider", &self.provider),
            ("path", &self.path),
            ("harness", &self.harness),
            ("key_fingerprint", &self.key_fingerprint),
        ];
        for (name, value) in opts {
            if let Some(v) = value {
                params.push((name, v.clone()));
            }
        }
        if let Some(s) = self.status {
            params.push(("status", s.to_string()));
        }
        if self.errors {
            params.push(("errors", "1".into()));
        }
        if let Some(l) = self.limit {
            params.push(("limit", l.to_string()));
        }
        params
    }
}

#[derive(Serialize, Deserialize, Clone)]
struct Config {
    host: String,
    port: u16,
    data_dir: PathBuf,
    local_key: String,
    #[serde(default = "default_heartbeat_minutes")]
    heartbeat_minutes: u64,
    #[serde(default = "default_ping_anthropic")]
    ping_anthropic_model: String,
    #[serde(default = "default_ping_openai")]
    ping_openai_model: String,
    #[serde(default = "default_ping_xai")]
    ping_xai_model: String,
    #[serde(default = "default_ping_gemini")]
    ping_gemini_model: String,
    #[serde(default)]
    gemini_project: String,
    #[serde(default = "default_anthropic_upstream")]
    anthropic_upstream: String,
    #[serde(default)]
    dario_api_key: String,
    #[serde(default = "default_dario_update_minutes")]
    dario_update_check_minutes: u64,
    #[serde(default)]
    dario_version: Option<String>,
    #[serde(default = "default_dario_probe_seconds")]
    dario_probe_seconds: u64,
    #[serde(default = "default_dario_probe_failures")]
    dario_probe_failures: u32,
    #[serde(default = "default_dario_probe_model")]
    dario_probe_model: String,
    #[serde(default = "default_trace_body_retention_days")]
    trace_body_retention_days: u64,
    #[serde(default)]
    trace_row_retention_days: u64,
    #[serde(default = "default_update_check_hours")]
    update_check_hours: u64,
    #[serde(default)]
    harness_overrides: BTreeMap<String, HarnessOverride>,
}

#[derive(Clone)]
struct SelfUpdateApplier {
    config: Config,
}

impl alex_proxy::DaemonUpdater for SelfUpdateApplier {
    fn apply(&self) -> alex_proxy::UpdateApplyFuture {
        let config = self.config.clone();
        Box::pin(async move {
            match selfupdate::daemon_apply_update(config).await {
                Ok(body) => Ok(body),
                Err(selfupdate::DaemonUpdateApplyError::Conflict(body)) => {
                    Err(alex_proxy::UpdateApplyError::Conflict(body))
                }
                Err(selfupdate::DaemonUpdateApplyError::Failed(e)) => {
                    Err(alex_proxy::UpdateApplyError::Failed(e.to_string()))
                }
            }
        })
    }
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub(crate) struct HarnessOverride {
    #[serde(default)]
    binary: Option<PathBuf>,
    #[serde(default)]
    config_dir: Option<PathBuf>,
}

fn default_heartbeat_minutes() -> u64 {
    15
}

fn default_trace_body_retention_days() -> u64 {
    30
}

fn default_update_check_hours() -> u64 {
    24
}

fn default_ping_anthropic() -> String {
    "claude-haiku-4-5".into()
}

fn default_ping_openai() -> String {
    "gpt-5.5".into()
}

fn default_ping_xai() -> String {
    "grok-code-fast-1".into()
}

fn default_ping_gemini() -> String {
    "gemini-2.5-flash".into()
}

fn default_anthropic_upstream() -> String {
    "direct".into()
}

fn default_dario_update_minutes() -> u64 {
    60
}

fn default_dario_probe_seconds() -> u64 {
    90
}

fn default_dario_probe_failures() -> u32 {
    2
}

fn default_dario_probe_model() -> String {
    "claude-haiku-4-5".into()
}

impl Config {
    fn ping_models(&self) -> alex_proxy::PingModels {
        alex_proxy::PingModels {
            anthropic: self.ping_anthropic_model.clone(),
            openai: self.ping_openai_model.clone(),
            xai: self.ping_xai_model.clone(),
            gemini: self.ping_gemini_model.clone(),
        }
    }

    fn dario_enabled(&self) -> bool {
        self.anthropic_upstream == "dario"
    }

    fn base_url(&self) -> String {
        format!("http://{}:{}", self.host, self.port)
    }
}

fn random_key(prefix: &str) -> String {
    let mut rng = rand::thread_rng();
    let bytes: [u8; 24] = rng.gen();
    format!(
        "{prefix}-{}",
        bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
    )
}

fn save_config(config: &Config) -> Result<()> {
    let path = alexandria_home().join("config.toml");
    std::fs::write(&path, toml::to_string_pretty(config)?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn alexandria_home() -> PathBuf {
    if let Some(path) = std::env::var_os("ALEXANDRIA_HOME") {
        return PathBuf::from(path);
    }
    dirs::home_dir().expect("no home dir").join(".alexandria")
}

fn load_or_create_config() -> Result<(Config, bool)> {
    let home = alexandria_home();
    std::fs::create_dir_all(&home)?;
    let path = home.join("config.toml");
    if path.exists() {
        let raw = std::fs::read_to_string(&path)?;
        let config: Config = toml::from_str(&raw).with_context(|| format!("parsing {path:?}"))?;
        let upgraded = toml::to_string_pretty(&config)?;
        if upgraded != raw {
            std::fs::write(&path, upgraded)?;
        }
        return Ok((config, false));
    }
    let config = Config {
        host: "127.0.0.1".into(),
        port: 4100,
        data_dir: home.clone(),
        local_key: random_key("alx"),
        ping_gemini_model: default_ping_gemini(),
        gemini_project: String::new(),
        heartbeat_minutes: default_heartbeat_minutes(),
        ping_anthropic_model: default_ping_anthropic(),
        ping_openai_model: default_ping_openai(),
        ping_xai_model: default_ping_xai(),
        anthropic_upstream: default_anthropic_upstream(),
        dario_api_key: String::new(),
        dario_update_check_minutes: default_dario_update_minutes(),
        dario_version: None,
        dario_probe_seconds: default_dario_probe_seconds(),
        dario_probe_failures: default_dario_probe_failures(),
        dario_probe_model: default_dario_probe_model(),
        trace_body_retention_days: default_trace_body_retention_days(),
        trace_row_retention_days: 0,
        update_check_hours: default_update_check_hours(),
        harness_overrides: BTreeMap::new(),
    };
    std::fs::write(&path, toml::to_string_pretty(&config)?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    eprintln!("created {path:?}");
    Ok((config, true))
}

fn open_vault(config: &Config) -> Result<Vault> {
    Vault::open(config.data_dir.join("accounts"))
}

struct DarioGlue(Arc<dario::DarioSupervisor>);

impl alex_proxy::DarioRouter for DarioGlue {
    fn active(&self) -> Option<alex_proxy::DarioActive> {
        self.0.active().map(|a| alex_proxy::DarioActive {
            generation_id: a.generation_id,
            base_url: a.base_url,
            api_key: a.api_key,
        })
    }

    fn begin(&self, generation_id: &str) -> Option<Box<dyn std::any::Any + Send>> {
        self.0
            .begin_request(generation_id)
            .map(|g| Box::new(g) as Box<dyn std::any::Any + Send>)
    }

    fn status(&self) -> serde_json::Value {
        self.0.status()
    }

    fn suspect(&self, generation_id: &str) {
        self.0.suspect(generation_id);
    }
}

fn dario_admin_router(sup: Arc<dario::DarioSupervisor>, local_key: String) -> axum::Router {
    use axum::extract::{Path as AxPath, Query, State};
    use axum::response::IntoResponse;
    use axum::routing::{get, post};

    async fn require_local_key(
        State(key): State<String>,
        req: axum::extract::Request,
        next: axum::middleware::Next,
    ) -> axum::response::Response {
        let presented = req
            .headers()
            .get("x-api-key")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
            .or_else(|| {
                req.headers()
                    .get("authorization")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.strip_prefix("Bearer "))
                    .map(str::to_string)
            });
        if presented.as_deref() != Some(key.as_str()) {
            return (
                axum::http::StatusCode::UNAUTHORIZED,
                axum::Json(
                    serde_json::json!({"error": "admin routes require x-api-key: <local_key>"}),
                ),
            )
                .into_response();
        }
        next.run(req).await
    }

    fn tail_lines(path: &str, lines: usize) -> String {
        std::fs::read_to_string(path)
            .map(|s| {
                let all: Vec<&str> = s.lines().collect();
                let start = all.len().saturating_sub(lines);
                all[start..].join("\n")
            })
            .unwrap_or_default()
    }

    async fn logs(
        State(sup): State<Arc<dario::DarioSupervisor>>,
        AxPath(gen_id): AxPath<String>,
        Query(q): Query<std::collections::HashMap<String, String>>,
    ) -> axum::response::Response {
        let lines = q
            .get("lines")
            .and_then(|s| s.parse().ok())
            .unwrap_or(200usize)
            .min(2000);
        let status = sup.status();
        let found = status["generations"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|g| g["id"].as_str() == Some(gen_id.as_str()))
            .cloned();
        match found {
            Some(g) => {
                let out = g["stdout_log"].as_str().map(|p| tail_lines(p, lines));
                let err = g["stderr_log"].as_str().map(|p| tail_lines(p, lines));
                axum::Json(serde_json::json!({
                    "generation_id": gen_id,
                    "stdout": out,
                    "stderr": err,
                    "lines": lines,
                }))
                .into_response()
            }
            None => (
                axum::http::StatusCode::NOT_FOUND,
                axum::Json(serde_json::json!({"error": {"message": "unknown generation"}})),
            )
                .into_response(),
        }
    }

    async fn restart(State(sup): State<Arc<dario::DarioSupervisor>>) -> axum::response::Response {
        match sup.restart().await {
            Ok(v) => axum::Json(v).into_response(),
            Err(e) => (
                axum::http::StatusCode::BAD_GATEWAY,
                axum::Json(serde_json::json!({"error": e.to_string()})),
            )
                .into_response(),
        }
    }

    async fn update(State(sup): State<Arc<dario::DarioSupervisor>>) -> axum::response::Response {
        match sup.update_now().await {
            Ok(v) => axum::Json(v).into_response(),
            Err(e) => (
                axum::http::StatusCode::BAD_GATEWAY,
                axum::Json(serde_json::json!({"error": e.to_string()})),
            )
                .into_response(),
        }
    }

    axum::Router::new()
        .route("/admin/dario/restart", post(restart))
        .route("/admin/dario/update", post(update))
        .route("/admin/dario/logs/{generation_id}", get(logs))
        .route_layer(axum::middleware::from_fn_with_state(
            local_key,
            require_local_key,
        ))
        .with_state(sup)
}

fn harness_admin_router(state: Arc<alex_proxy::AppState>, local_key: String) -> axum::Router {
    use axum::extract::{Path as AxPath, Query, State};
    use axum::response::IntoResponse;
    use axum::routing::{get, post, put};

    #[derive(Deserialize)]
    struct HarnessOverrideBody {
        binary: Option<PathBuf>,
        config_dir: Option<PathBuf>,
    }

    fn parse_dry_run(q: &std::collections::HashMap<String, String>) -> bool {
        q.get("dry_run")
            .map(|s| matches!(s.as_str(), "1" | "true" | "yes"))
            .unwrap_or(false)
    }

    fn list_active_pi_keys(state: &alex_proxy::AppState) -> Result<Vec<(String, String)>> {
        let rows = state.store.list_run_keys(true)?;
        Ok(rows
            .iter()
            .filter(|row| row["kind"].as_str() == Some("harness"))
            .filter(|row| row["label"].as_str() == Some("pi"))
            .filter(|row| !row["revoked"].as_bool().unwrap_or(false))
            .filter_map(|row| {
                let id = row["id"].as_str()?.to_string();
                let fp = row["key_fingerprint"]
                    .as_str()
                    .unwrap_or(id.as_str())
                    .to_string();
                Some((id, fp))
            })
            .collect())
    }

    async fn require_local_key(
        State(key): State<String>,
        req: axum::extract::Request,
        next: axum::middleware::Next,
    ) -> axum::response::Response {
        let presented = req
            .headers()
            .get("x-api-key")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
            .or_else(|| {
                req.headers()
                    .get("authorization")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.strip_prefix("Bearer "))
                    .map(str::to_string)
            });
        if presented.as_deref() != Some(key.as_str()) {
            return (
                axum::http::StatusCode::UNAUTHORIZED,
                axum::Json(
                    serde_json::json!({"error": "admin routes require x-api-key: <local_key>"}),
                ),
            )
                .into_response();
        }
        next.run(req).await
    }

    fn error(status: axum::http::StatusCode, message: impl Into<String>) -> axum::response::Response {
        (status, axum::Json(serde_json::json!({"error": message.into()}))).into_response()
    }

    fn key_hash_hex(key: &str) -> String {
        use sha2::{Digest, Sha256};
        let digest = Sha256::digest(key.as_bytes());
        digest.iter().map(|b| format!("{b:02x}")).collect()
    }

    fn revoke_pi_keys(state: &alex_proxy::AppState) -> Result<usize> {
        let rows = state.store.list_run_keys(true)?;
        let ids: Vec<String> = rows
            .iter()
            .filter(|row| row["kind"].as_str() == Some("harness"))
            .filter(|row| row["label"].as_str() == Some("pi"))
            .filter(|row| !row["revoked"].as_bool().unwrap_or(false))
            .filter_map(|row| row["id"].as_str().map(String::from))
            .collect();
        let mut revoked = 0usize;
        for id in ids {
            if state.store.revoke_run_key(&id)? {
                revoked += 1;
            }
        }
        if revoked > 0 {
            state.run_keys.write().unwrap().clear();
        }
        Ok(revoked)
    }

    fn mint_pi_key(state: &alex_proxy::AppState) -> Result<(String, String)> {
        let key = alex_proxy::generate_run_key();
        let key_hash = key_hash_hex(&key);
        let id = format!("rk-{}", &key_hash[..8]);
        let tags_json = serde_json::json!({"harness": "pi"}).to_string();
        state.store.insert_run_key(
            &id,
            &key_hash,
            "harness",
            None,
            Some(&tags_json),
            Some("pi"),
            now_ms(),
            None,
        )?;
        Ok((id, key))
    }

    fn state_models(state: &alex_proxy::AppState) -> Vec<String> {
        let mut ids = state.store.pricing_models();
        for (alias, _) in alex_core::model_aliases() {
            ids.push((*alias).to_string());
        }
        let filtered = harness_connect::filter_model_ids(ids);
        if filtered.is_empty() {
            vec![
                "claude-opus-4-8".into(),
                "claude-sonnet-5".into(),
                "claude-haiku-4-5".into(),
                "gpt-5.5".into(),
                "grok-code-fast-1".into(),
                "gemini-2.5-flash".into(),
            ]
        } else {
            filtered
        }
    }

    async fn list() -> axum::response::Response {
        match load_or_create_config() {
            Ok((config, _)) => match harness_connect::harness_statuses(&config, None, true).await {
                Ok(harnesses) => axum::Json(serde_json::json!({"harnesses": harnesses})).into_response(),
                Err(e) => error(axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
            },
            Err(e) => error(axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        }
    }

    async fn connect(
        State(state): State<Arc<alex_proxy::AppState>>,
        AxPath(name): AxPath<String>,
        Query(q): Query<std::collections::HashMap<String, String>>,
    ) -> axum::response::Response {
        if name != "pi" {
            return error(
                axum::http::StatusCode::BAD_REQUEST,
                format!("harness '{name}' does not support connect"),
            );
        }
        let dry_run = parse_dry_run(&q);
        let (config, _) = match load_or_create_config() {
            Ok(v) => v,
            Err(e) => return error(axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        };
        let spec = harness_connect::pi_spec();
        let status = match harness_connect::harness_status(&config, spec, None, true).await {
            Ok(status) => status,
            Err(e) => return error(axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        };
        if !status.installed {
            return error(axum::http::StatusCode::BAD_REQUEST, "pi is not installed");
        }
        let config_dir = harness_connect::resolve_config_dir(&config, spec, None);
        if !config_dir.is_dir() {
            return error(
                axum::http::StatusCode::BAD_REQUEST,
                format!("pi config dir does not exist at {}", config_dir.display()),
            );
        }
        let models = state_models(&state);
        if dry_run {
            let keys = match list_active_pi_keys(&state) {
                Ok(v) => v,
                Err(e) => {
                    return error(axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
                }
            };
            return axum::Json(harness_connect::plan_connect(
                &config_dir,
                models.len(),
                &keys,
            ))
            .into_response();
        }
        if let Err(e) = revoke_pi_keys(&state) {
            return error(axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
        }
        let (key_id, key) = match mint_pi_key(&state) {
            Ok(v) => v,
            Err(e) => return error(axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        };
        match harness_connect::write_pi_connection(
            config_dir,
            state.base_url.clone(),
            key_id,
            key,
            models,
            status.version,
        ) {
            Ok(summary) => {
                axum::Json(harness_connect::config_write_json(&summary, "minted", None))
                    .into_response()
            }
            Err(e) => error(axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        }
    }

    async fn disconnect(
        State(state): State<Arc<alex_proxy::AppState>>,
        AxPath(name): AxPath<String>,
        Query(q): Query<std::collections::HashMap<String, String>>,
    ) -> axum::response::Response {
        if name != "pi" {
            return error(
                axum::http::StatusCode::BAD_REQUEST,
                format!("harness '{name}' does not support disconnect"),
            );
        }
        let dry_run = parse_dry_run(&q);
        let (config, _) = match load_or_create_config() {
            Ok(v) => v,
            Err(e) => return error(axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        };
        let config_dir =
            harness_connect::resolve_config_dir(&config, harness_connect::pi_spec(), None);
        if dry_run {
            let keys = match list_active_pi_keys(&state) {
                Ok(v) => v,
                Err(e) => {
                    return error(axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
                }
            };
            return axum::Json(harness_connect::plan_disconnect(&config_dir, &keys)).into_response();
        }
        let models_path = config_dir.join("models.json");
        let previous_models = harness_connect::read_pi_model_ids(&config_dir);
        let was_connected = match harness_connect::disconnect_pi_config(&config_dir) {
            Ok(v) => v,
            Err(e) => return error(axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        };
        match revoke_pi_keys(&state) {
            Ok(revoked) => axum::Json(harness_connect::disconnect_summary_json(
                &models_path,
                if was_connected {
                    previous_models
                } else {
                    Vec::new()
                },
                &state.base_url,
                revoked,
                was_connected,
            ))
            .into_response(),
            Err(e) => error(axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        }
    }

    async fn refresh_config(
        State(state): State<Arc<alex_proxy::AppState>>,
        AxPath(name): AxPath<String>,
    ) -> axum::response::Response {
        let Some(spec) = harness_connect::spec_by_name(&name) else {
            return error(
                axum::http::StatusCode::NOT_FOUND,
                format!("unknown harness '{name}'"),
            );
        };
        if !spec.supports_connect || name != "pi" {
            return error(
                axum::http::StatusCode::BAD_REQUEST,
                format!("harness '{name}' does not support connect"),
            );
        }
        let (config, _) = match load_or_create_config() {
            Ok(v) => v,
            Err(e) => return error(axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        };
        let config_dir = harness_connect::resolve_config_dir(&config, spec, None);
        if !config_dir.is_dir() {
            return error(
                axum::http::StatusCode::BAD_REQUEST,
                format!("pi config dir does not exist at {}", config_dir.display()),
            );
        }
        let existing_key = harness_connect::read_pi_api_key(&config_dir);
        let (key_status, key_id, api_key) = if let Some(key) = existing_key {
            ("reused", String::new(), key)
        } else {
            match mint_pi_key(&state) {
                Ok((id, key)) => ("minted", id, key),
                Err(e) => {
                    return error(axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
                }
            }
        };
        let models = state_models(&state);
        match harness_connect::write_pi_connection(
            config_dir,
            state.base_url.clone(),
            key_id,
            api_key,
            models,
            None,
        ) {
            Ok(summary) => axum::Json(harness_connect::config_write_json(
                &summary,
                key_status,
                Some(true),
            ))
            .into_response(),
            Err(e) => error(axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        }
    }

    async fn put_override(
        AxPath(name): AxPath<String>,
        axum::Json(body): axum::Json<HarnessOverrideBody>,
    ) -> axum::response::Response {
        let Some(spec) = harness_connect::spec_by_name(&name) else {
            return error(
                axum::http::StatusCode::BAD_REQUEST,
                format!("unknown harness '{name}'"),
            );
        };
        let (mut config, _) = match load_or_create_config() {
            Ok(v) => v,
            Err(e) => return error(axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        };
        if body.binary.is_none() && body.config_dir.is_none() {
            config.harness_overrides.remove(&name);
        } else {
            config.harness_overrides.insert(
                name,
                HarnessOverride {
                    binary: body.binary,
                    config_dir: body.config_dir,
                },
            );
        }
        if let Err(e) = save_config(&config) {
            return error(axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
        }
        match harness_connect::harness_status(&config, spec, None, true).await {
            Ok(status) => axum::Json(status).into_response(),
            Err(e) => error(axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        }
    }

    axum::Router::new()
        .route("/admin/harnesses", get(list))
        .route("/admin/harnesses/{name}/connect", post(connect))
        .route("/admin/harnesses/{name}/disconnect", post(disconnect))
        .route("/admin/harnesses/{name}/refresh-config", post(refresh_config))
        .route("/admin/harnesses/{name}/override", put(put_override))
        .route_layer(axum::middleware::from_fn_with_state(
            local_key,
            require_local_key,
        ))
        .with_state(state)
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let command = match cli.command {
        Some(c) => c,
        None => {
            if !std::io::IsTerminal::is_terminal(&std::io::stdout()) {
                anyhow::bail!(
                    "no subcommand given and stdout is not a terminal — try `alexandria --help`"
                );
            }
            Command::Tui
        }
    };
    let default_filter = match &command {
        Command::Daemon { .. } => "info,alexandria=debug",
        _ => "warn",
    };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| default_filter.into()),
        )
        .init();
    let (config, fresh_install) = load_or_create_config()?;

    match command {
        Command::Daemon {
            host,
            port,
            background,
            nosplash,
        } => {
            let mut config = config.clone();
            let host_val = host.clone().unwrap_or_else(|| config.host.clone());
            let port_val = port.unwrap_or(config.port);
            if background {
                return daemon_background(&host_val, port_val, host, port).await;
            }
            let host = host_val;
            let port = port_val;
            if fresh_install
                && !nosplash
                && std::env::var("ALEXANDRIA_NO_LIGHT").is_err()
                && std::io::IsTerminal::is_terminal(&std::io::stdout())
            {
                let _ = light::run(1, false, None).await;
            }
            let store = Arc::new(Store::open(config.data_dir.clone())?);
            let vault = Arc::new(open_vault(&config)?);
            if vault.list().await.is_empty() {
                eprintln!("warning: no accounts in vault — run `alexandria auth import` first");
            }
            if !config.gemini_project.is_empty() {
                let _ = vault
                    .set_account_meta(
                        "gemini-oauth",
                        "project_id",
                        serde_json::json!(config.gemini_project),
                    )
                    .await;
            }
            let mut dario_router: Option<Arc<dyn alex_proxy::DarioRouter>> = None;
            let mut supervisor: Option<Arc<dario::DarioSupervisor>> = None;
            if config.dario_enabled() {
                if config.dario_api_key.is_empty() {
                    config.dario_api_key = random_key("dario");
                    save_config(&config)?;
                    eprintln!("generated dario_api_key and saved it to config.toml");
                }
                let settings = dario::DarioSettings {
                    install_root: config.data_dir.join("dario"),
                    log_root: config.data_dir.join("dario").join("logs"),
                    capture_root: config.data_dir.join("bodies"),
                    prompt_cache_root: config.data_dir.join("dario-prompt-cache"),
                    api_key: config.dario_api_key.clone(),
                    update_check_minutes: config.dario_update_check_minutes,
                    version_pin: config.dario_version.clone(),
                    probe_seconds: config.dario_probe_seconds,
                    probe_failures: config.dario_probe_failures,
                    probe_model: config.dario_probe_model.clone(),
                };
                match dario::DarioSupervisor::start(settings).await {
                    Ok(sup) => {
                        eprintln!(
                            "dario: active generation {}",
                            sup.active()
                                .map(|a| a.generation_id)
                                .unwrap_or_else(|| "-".into())
                        );
                        dario_router =
                            Some(Arc::new(DarioGlue(sup.clone()))
                                as Arc<dyn alex_proxy::DarioRouter>);
                        supervisor = Some(sup);
                    }
                    Err(e) => {
                        eprintln!("dario: failed to start ({e}); using direct anthropic upstream");
                    }
                }
            }
            let state = alex_proxy::build_state(
                config.local_key.clone(),
                vault,
                store,
                dario_router,
                format!("http://{host}:{port}"),
            );
            alex_proxy::set_daemon_updater(
                &state,
                Arc::new(SelfUpdateApplier {
                    config: config.clone(),
                }),
            );
            if config.update_check_hours > 0 {
                let update_status = state.update_status.clone();
                let hours = config.update_check_hours;
                tokio::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                    loop {
                        match selfupdate::daemon_update_status_value().await {
                            Ok(status) => {
                                *update_status.write().await = Some(status);
                            }
                            Err(e) => tracing::debug!("update check failed: {e}"),
                        }
                        tokio::time::sleep(std::time::Duration::from_secs(hours * 3600)).await;
                    }
                });
                eprintln!(
                    "update checks: every {}h (set update_check_hours = 0 to disable)",
                    config.update_check_hours
                );
            } else {
                eprintln!(
                    "update checks: disabled (set update_check_hours in config.toml to enable)"
                );
            }
            if config.heartbeat_minutes > 0 {
                let hb_state = state.clone();
                let models = config.ping_models();
                let minutes = config.heartbeat_minutes;
                tokio::spawn(async move {
                    let mut interval =
                        tokio::time::interval(std::time::Duration::from_secs(minutes * 60));
                    interval.tick().await;
                    loop {
                        interval.tick().await;
                        alex_proxy::heartbeat_once(&hb_state, &models).await;
                    }
                });
                eprintln!(
                    "heartbeat: every {}m (set heartbeat_minutes = 0 to disable)",
                    config.heartbeat_minutes
                );
            } else {
                eprintln!("heartbeat: disabled (set heartbeat_minutes in config.toml to enable)");
            }
            let body_days = config.trace_body_retention_days;
            let row_days = config.trace_row_retention_days;
            if body_days > 0 || row_days > 0 {
                let prune_state = state.clone();
                tokio::spawn(async move {
                    let mut interval =
                        tokio::time::interval(std::time::Duration::from_secs(24 * 3600));
                    interval.tick().await;
                    loop {
                        interval.tick().await;
                        let store = prune_state.store.clone();
                        let reports = tokio::task::spawn_blocking(move || {
                            let now = now_ms();
                            let day_ms = 86_400_000i64;
                            let mut out = Vec::new();
                            if body_days > 0 {
                                out.push((
                                    "bodies",
                                    store.prune(now - body_days as i64 * day_ms, true, false),
                                ));
                            }
                            if row_days > 0 {
                                out.push((
                                    "rows",
                                    store.prune(now - row_days as i64 * day_ms, false, false),
                                ));
                            }
                            out
                        })
                        .await
                        .unwrap_or_default();
                        for (scope, report) in reports {
                            match report {
                                Ok(r) => tracing::info!("retention prune ({scope}): {r:?}"),
                                Err(e) => {
                                    tracing::warn!("retention prune ({scope}) failed: {e}")
                                }
                            }
                        }
                    }
                });
                let describe = |days: u64| {
                    if days > 0 {
                        format!("{days}d")
                    } else {
                        "keep forever".into()
                    }
                };
                eprintln!(
                    "retention: bodies {} / rows {} (daily check)",
                    describe(body_days),
                    describe(row_days)
                );
            }
            let mut app = alex_proxy::router(state.clone());
            app = app.merge(harness_admin_router(state.clone(), config.local_key.clone()));
            if let Some(sup) = supervisor.clone() {
                app = app.merge(dario_admin_router(sup, config.local_key.clone()));
            }
            let addr: std::net::SocketAddr = format!("{host}:{port}")
                .parse()
                .with_context(|| format!("parsing bind address {host}:{port}"))?;
            let socket = if addr.is_ipv4() {
                tokio::net::TcpSocket::new_v4()?
            } else {
                tokio::net::TcpSocket::new_v6()?
            };
            socket.set_reuseaddr(true)?;
            #[cfg(unix)]
            socket.set_reuseport(true)?;
            socket
                .bind(addr)
                .with_context(|| format!("binding {addr}"))?;
            let listener = socket.listen(1024)?;
            print_banner(&host, port, &config.local_key);
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .with_graceful_shutdown(async {
                #[cfg(unix)]
                {
                    let mut term =
                        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                            .expect("sigterm handler");
                    tokio::select! {
                        _ = tokio::signal::ctrl_c() => {}
                        _ = term.recv() => {}
                    }
                }
                #[cfg(not(unix))]
                {
                    let _ = tokio::signal::ctrl_c().await;
                }
                eprintln!("\ndraining in-flight connections, then shutting down");
            })
            .await?;
            if let Some(sup) = supervisor {
                sup.shutdown().await;
            }
        }
        Command::Auth { command } => match command {
            AuthCommand::Import { source } => {
                let vault = open_vault(&config)?;
                for outcome in import_all(&vault, &source).await? {
                    if outcome.imported.is_empty() {
                        println!(
                            "{:<8} skipped ({})",
                            outcome.source,
                            outcome.note.unwrap_or_else(|| "nothing found".into())
                        );
                    } else {
                        println!(
                            "{:<8} imported: {}",
                            outcome.source,
                            outcome.imported.join(", ")
                        );
                    }
                }
            }
            AuthCommand::Login { provider } => {
                use std::io::IsTerminal;
                let vault = open_vault(&config)?;
                let provider = match provider {
                    Some(p) => p,
                    None if std::io::stdout().is_terminal() && std::io::stdin().is_terminal() => {
                        let accounts = vault.list().await;
                        match pick_provider(&accounts)? {
                            Some(p) => p,
                            None => return Ok(()),
                        }
                    }
                    None => anyhow::bail!(
                        "usage: alexandria auth login <provider> — providers: {}",
                        alex_auth::login::PROVIDERS.join(", ")
                    ),
                };
                let id = alex_auth::login::login(&vault, &provider).await?;
                println!("saved account: {id}");
            }
            AuthCommand::GeminiKey { key } => {
                let key = key
                    .or_else(|| std::env::var("GEMINI_API_KEY").ok())
                    .filter(|k| !k.trim().is_empty())
                    .context(
                        "provide the key: `alexandria auth gemini-key <KEY>` (get one at https://aistudio.google.com/apikey)",
                    )?;
                let vault = open_vault(&config)?;
                let account = alex_auth::Account {
                    id: "gemini-api-key".into(),
                    provider: alex_core::Provider::Gemini,
                    kind: "api_key".into(),
                    label: Some("gemini (AI Studio key)".into()),
                    access_token: None,
                    refresh_token: None,
                    id_token: None,
                    api_key: Some(key.trim().to_string()),
                    expires_at_ms: None,
                    last_refresh_ms: None,
                    account_meta: serde_json::Value::Null,
                    cooldown_until_ms: None,
                    status: "active".into(),
                };
                vault.upsert(account).await?;
                println!(
                    "{} saved gemini-api-key — gemini-* models now route to AI Studio",
                    ui::green(ui::dot())
                );
            }
            AuthCommand::List => {
                let vault = open_vault(&config)?;
                let accounts = vault.list().await;
                if accounts.is_empty() {
                    println!("no accounts — run `alexandria auth import`");
                }
                for a in accounts {
                    let (dot, expiry) = account_indicators(&a);
                    println!(
                        "{dot} {} {} {} {} {}  {}",
                        ui::pad_right(&a.id, 18),
                        ui::pad_right(&ui::amber(a.provider.as_str()), 10),
                        ui::pad_right(&a.kind, 8),
                        ui::pad_right(&a.status, 10),
                        ui::pad_right(&expiry, 20),
                        ui::sand(&a.label.unwrap_or_default())
                    );
                }
            }
        },
        Command::Traces {
            command,
            limit,
            session,
            model,
            json,
        } => match command {
            None => {
                let store = Store::open(config.data_dir.clone())?;
                let rows = store.list_traces(limit, session.as_deref(), model.as_deref())?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&rows)?);
                } else if rows.is_empty() {
                    println!("no traces yet");
                } else {
                    render_traces_table(&rows);
                }
            }
            Some(TracesCommand::Search { filter, json }) => {
                traces_search_cmd(&config, &filter, json).await?;
            }
            Some(TracesCommand::Export {
                filter,
                bodies,
                out,
            }) => {
                traces_export_cmd(&config, &filter, bodies, out).await?;
            }
            Some(TracesCommand::Path { run_id }) => {
                traces_path_cmd(&config, &run_id)?;
            }
            Some(TracesCommand::Prune {
                older_than,
                bodies_only: _,
                rows,
                dry_run,
                json,
            }) => {
                traces_prune_cmd(&config, &older_than, rows, dry_run, json)?;
            }
            Some(TracesCommand::Du { json }) => {
                traces_du_cmd(&config, json)?;
            }
        },
        Command::Env => {
            print_env(&config.host, config.port, &config.local_key);
        }
        Command::Connect {
            harness,
            config_dir,
            json,
        } => {
            harness_connect::connect_cmd(&config, harness, config_dir, json).await?;
        }
        Command::Disconnect {
            harness,
            config_dir,
        } => {
            harness_connect::disconnect_cmd(&config, harness, config_dir).await?;
        }
        Command::Ping { target } => {
            let store = Arc::new(Store::open(config.data_dir.clone())?);
            let vault = Arc::new(open_vault(&config)?);
            let state = alex_proxy::build_state(
                config.local_key.clone(),
                vault,
                store,
                None,
                config.base_url(),
            );
            let models = config.ping_models();
            let providers: Vec<alex_core::Provider> = if target == "all" {
                let mut seen = Vec::new();
                for a in state.vault.list().await {
                    if a.status == "active"
                        && matches!(
                            a.provider,
                            alex_core::Provider::Anthropic
                                | alex_core::Provider::Openai
                                | alex_core::Provider::Xai
                                | alex_core::Provider::Gemini
                        )
                        && !seen.contains(&a.provider)
                    {
                        seen.push(a.provider);
                    }
                }
                seen
            } else {
                vec![
                    alex_core::Provider::from_str_loose(&target).with_context(|| {
                        format!("unknown target '{target}' (anthropic|openai|grok|all)")
                    })?,
                ]
            };
            if providers.is_empty() {
                println!("no pingable accounts — run `alexandria auth import`");
                return Ok(());
            }
            let results = run_pings(&state, &models, &providers).await;
            let ok = results.iter().filter(|r| r.ok).count();
            let summary = format!("{ok}/{} providers healthy", results.len());
            if ok == results.len() {
                println!("{} {}", ui::gold(ui::ankh()), ui::bold(&summary));
            } else {
                println!("{} {}", ui::red("✗"), ui::red(&ui::bold(&summary)));
            }
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            if ok != results.len() {
                std::process::exit(1);
            }
        }
        Command::Harness { command } => match command {
            HarnessCommand::List { json } => {
                harness_e2e::print_harnesses(&config.data_dir, json)?;
            }
            HarnessCommand::Run {
                harness,
                model,
                prompt,
                package_tarball,
                docker_image,
                container_base_url,
                timeout_secs,
                no_trace_check,
                json,
            } => {
                let summary = harness_e2e::run_harness(harness_e2e::RunOptions {
                    harness,
                    model,
                    prompt,
                    package_tarball,
                    docker_image: docker_image
                        .unwrap_or_else(|| harness_e2e::default_docker_image().to_string()),
                    container_base_url: container_base_url.unwrap_or_else(|| {
                        harness_e2e::default_container_base_url(&config.host, config.port)
                    }),
                    timeout_secs: timeout_secs.unwrap_or_else(harness_e2e::default_timeout_secs),
                    no_trace_check,
                    local_key: config.local_key.clone(),
                    data_dir: config.data_dir.clone(),
                })?;
                harness_e2e::print_run_summary(&summary, json)?;
            }
            HarnessCommand::Pack {
                target,
                version,
                force,
                json,
            } => {
                let summary =
                    harness_e2e::pack_target(&config.data_dir, &target, version.as_deref(), force)?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&summary)?);
                } else {
                    println!(
                        "{}@{} -> {}{}",
                        summary.package,
                        summary.version,
                        summary.tarball,
                        if summary.reused { " (cached)" } else { "" }
                    );
                }
            }
        },
        Command::Limits { json } => {
            let store = Arc::new(Store::open(config.data_dir.clone())?);
            let vault = Arc::new(open_vault(&config)?);
            let state = alex_proxy::build_state(
                config.local_key.clone(),
                vault,
                store,
                None,
                config.base_url(),
            );
            let snap = alex_proxy::limits_snapshot(&state).await;
            if json {
                println!("{}", serde_json::to_string_pretty(&snap)?);
            } else {
                print_limits(&snap);
                print_dario_update_notice();
            }
        }
        Command::Status { json } => {
            run_status(&config, json).await?;
        }
        Command::Light {
            loops,
            forever,
            follow,
        } => {
            light::run(loops, forever, follow).await?;
        }
        Command::Credentials { json, host } => {
            let base = match host {
                Some(h) => format!("http://{h}:{}", config.port),
                None => config.base_url(),
            };
            let (payload, exports) = alex_proxy::connect_payload(&base, &config.local_key);
            if json {
                println!("{}", serde_json::to_string_pretty(&payload)?);
            } else {
                print!("{exports}");
            }
        }
        Command::Service { command } => match command {
            ServiceCommand::Install => service_install()?,
            ServiceCommand::Uninstall => service_uninstall()?,
            ServiceCommand::Status => {
                println!("{}", service_state_label(&detect_service_state()));
            }
        },
        Command::Update {
            check,
            yes,
            no_restart,
            json,
            force,
        } => {
            selfupdate::run_update(&config, check, yes, no_restart, json, force).await?;
        }
        Command::Dario { command } => {
            let http = reqwest::Client::new();
            let base = config.base_url();
            let is_status = matches!(command, DarioCommand::Status);
            let key = config.local_key.as_str();
            let result = match command {
                DarioCommand::Status => {
                    http.get(format!("{base}/admin/dario"))
                        .header("x-api-key", key)
                        .send()
                        .await
                }
                DarioCommand::Restart => {
                    http.post(format!("{base}/admin/dario/restart"))
                        .header("x-api-key", key)
                        .send()
                        .await
                }
                DarioCommand::Update => {
                    http.post(format!("{base}/admin/dario/update"))
                        .header("x-api-key", key)
                        .send()
                        .await
                }
            };
            let resp = result.with_context(|| {
                format!("could not reach the alexandria daemon at {base} — is it running?")
            })?;
            let status = resp.status();
            let body: serde_json::Value = resp.json().await.unwrap_or_default();
            if is_status && status.is_success() {
                print_dario_status(&body)?;
                print_dario_update_notice();
            } else {
                if let Some(outcome) = body["outcome"].as_str() {
                    println!("{}", ui::bold(&ui::gold(&format!("outcome: {outcome}"))));
                }
                println!("{}", serde_json::to_string_pretty(&body)?);
            }
            if !status.is_success() {
                std::process::exit(1);
            }
        }
        Command::Keys { command } => match command {
            KeysCommand::Mint {
                run_id,
                tag,
                ttl,
                label,
            } => {
                keys_mint_cmd(&config, run_id, &tag, &ttl, label).await?;
            }
            KeysCommand::List { all, json } => {
                keys_list_cmd(&config, all, json).await?;
            }
            KeysCommand::Revoke { id } => {
                keys_revoke_cmd(&config, &id).await?;
            }
        },
        Command::Tui => {
            tui::run(&config.base_url(), &config.local_key).await?;
        }
    }
    Ok(())
}

fn render_traces_table(rows: &[serde_json::Value]) {
    let show_run = rows.iter().any(|r| r["run_id"].is_string());
    println!("{}", ui::section("traces"));
    let run_header = if show_run {
        format!("{} ", ui::pad_right(&ui::column_header("run"), 16))
    } else {
        String::new()
    };
    println!(
        "{}   {} {} {} {} {} {}  {}{}",
        ui::pad_right(&ui::column_header("when"), 12),
        ui::pad_right(&ui::column_header("model"), 26),
        ui::pad_right(&ui::column_header("provider"), 9),
        ui::pad_left(&ui::column_header("st"), 4),
        ui::pad_left(&ui::column_header("in"), 8),
        ui::pad_left(&ui::column_header("out"), 8),
        ui::pad_left(&ui::column_header("cost"), 10),
        run_header,
        ui::column_header("id")
    );
    for r in rows {
        let ts = r["ts_request_ms"].as_i64().unwrap_or(0);
        let when = if ts > 0 {
            ui::human_ago(ts)
        } else {
            "-".into()
        };
        let streamed = r["streamed"]
            .as_bool()
            .or_else(|| r["streamed"].as_i64().map(|v| v != 0))
            .unwrap_or(false);
        let model = r["routed_model"].as_str().unwrap_or("-");
        let model = if streamed {
            format!("{} {}", ui::dim("≈"), ui::turquoise(model))
        } else {
            format!("  {}", ui::turquoise(model))
        };
        let cost = r["cost_usd"]
            .as_f64()
            .map(|c| format!("${c:.5}"))
            .unwrap_or_else(|| "-".into());
        let err = r["error"]
            .as_str()
            .map(|e| format!("  {}", ui::red(&format!("ERR: {e}"))))
            .unwrap_or_default();
        let id = r["id"].as_str().unwrap_or("-");
        let short_id = id.chars().take(8).collect::<String>();
        let run_cell = if show_run {
            let run = r["run_id"].as_str().unwrap_or("-");
            format!("{} ", ui::pad_right(&ui::amber(&ui::truncate(run, 16)), 16))
        } else {
            String::new()
        };
        println!(
            "{} {} {} {} {} {} {}  {}{}{}",
            ui::pad_right(&ui::sand(&when), 12),
            ui::pad_right(&model, 28),
            ui::pad_right(r["upstream_provider"].as_str().unwrap_or("-"), 9),
            ui::pad_left(&ui::status_color(r["status"].as_i64()), 4),
            ui::pad_left(
                &r["input_tokens"]
                    .as_i64()
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "-".into()),
                8
            ),
            ui::pad_left(
                &r["output_tokens"]
                    .as_i64()
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "-".into()),
                8
            ),
            ui::pad_left(&cost, 10),
            run_cell,
            ui::dim(&short_id),
            err
        );
    }
}

async fn daemon_get(
    config: &Config,
    path: &str,
    params: &[(&str, String)],
) -> Result<reqwest::Response> {
    let base = config.base_url();
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;
    let resp = client
        .get(format!("{base}{path}"))
        .header("x-api-key", &config.local_key)
        .query(params)
        .send()
        .await
        .with_context(|| {
            format!("could not reach the alexandria daemon at {base} — is it running?")
        })?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("daemon returned {status}: {}", ui::truncate(&body, 300));
    }
    Ok(resp)
}

async fn traces_search_cmd(config: &Config, filter: &TraceFilterArgs, json: bool) -> Result<()> {
    let resp = daemon_get(config, "/traces/search", &filter.query_params()).await?;
    let body: serde_json::Value = resp.json().await?;
    let rows = body["traces"].as_array().cloned().unwrap_or_default();
    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
    } else if rows.is_empty() {
        println!("no matching traces");
    } else {
        render_traces_table(&rows);
    }
    Ok(())
}

async fn traces_export_cmd(
    config: &Config,
    filter: &TraceFilterArgs,
    bodies: bool,
    out: Option<PathBuf>,
) -> Result<()> {
    let (path, mut params) = match &filter.run_id {
        Some(id) => {
            let mut params: Vec<(&str, String)> = Vec::new();
            if let Some(l) = filter.limit {
                params.push(("limit", l.to_string()));
            }
            (format!("/traces/runs/{id}/export.ndjson"), params)
        }
        None => ("/traces/export.ndjson".to_string(), filter.query_params()),
    };
    if bodies {
        params.push(("bodies", "1".into()));
    }
    let resp = daemon_get(config, &path, &params).await?;
    let text = resp.text().await?;
    let count = text.lines().filter(|l| !l.trim().is_empty()).count();
    match out {
        Some(dest) => {
            if let Some(parent) = dest.parent() {
                if !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(parent)?;
                }
            }
            std::fs::write(&dest, &text)?;
            eprintln!("wrote {count} traces to {}", dest.display());
        }
        None => {
            print!("{text}");
            eprintln!("wrote {count} traces to stdout");
        }
    }
    Ok(())
}

fn traces_path_cmd(config: &Config, run_id: &str) -> Result<()> {
    let store = Store::open(config.data_dir.clone())?;
    let summary = store.run_summary(run_id)?;
    if summary["trace_count"].as_i64().unwrap_or(0) == 0 {
        eprintln!("no traces for run '{run_id}'");
        std::process::exit(1);
    }
    println!("data_dir: {}", store.data_dir.display());
    println!(
        "sqlite: {}",
        store.data_dir.join("alexandria.sqlite3").display()
    );
    for artifact in store.run_artifacts(run_id)? {
        if let Some(p) = artifact["path"].as_str() {
            println!("{p}");
        }
    }
    Ok(())
}

fn traces_prune_cmd(
    config: &Config,
    older_than: &str,
    rows: bool,
    dry_run: bool,
    json: bool,
) -> Result<()> {
    let now = now_ms();
    let cutoff = alex_core::parse_since(older_than, now).with_context(|| {
        format!("invalid --older-than '{older_than}' (use 45s, 30m, 24h, 30d, or RFC3339)")
    })?;
    anyhow::ensure!(
        cutoff <= now,
        "--older-than '{older_than}' resolves to the future"
    );
    let store = Store::open(config.data_dir.clone())?;
    let report = store.prune(cutoff, !rows, dry_run)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    let title = if dry_run { "prune (dry run)" } else { "prune" };
    println!("{}", ui::section(title));
    let verb = if dry_run { "would delete" } else { "deleted" };
    println!(
        "{verb} {} body files ({})",
        ui::bold(&report.bodies_deleted.to_string()),
        ui::amber(&ui::human_bytes(report.bytes_freed))
    );
    println!(
        "{} {} rows of body paths + headers",
        if dry_run { "would strip" } else { "stripped" },
        ui::bold(&report.rows_affected.to_string())
    );
    if rows {
        println!(
            "{verb} {} trace rows",
            ui::bold(&report.rows_deleted.to_string())
        );
    }
    if report.dirs_removed > 0 {
        println!("removed {} empty date dirs", report.dirs_removed);
    }
    if dry_run {
        println!("{}", ui::dim("no changes made — rerun without --dry-run"));
    }
    Ok(())
}

fn traces_du_cmd(config: &Config, json: bool) -> Result<()> {
    let store = Store::open(config.data_dir.clone())?;
    let du = store.disk_usage()?;
    if json {
        println!("{}", serde_json::to_string_pretty(&du)?);
        return Ok(());
    }
    println!("{}", ui::section("storage"));
    let sqlite = du["sqlite_bytes"].as_u64().unwrap_or(0);
    let bodies = du["bodies_bytes"].as_u64().unwrap_or(0);
    println!(
        "sqlite {}  bodies {}  total {}",
        ui::amber(&ui::human_bytes(sqlite)),
        ui::amber(&ui::human_bytes(bodies)),
        ui::bold(&ui::human_bytes(sqlite + bodies))
    );
    let rows = du["trace_rows"].as_i64().unwrap_or(0);
    let span = match (du["oldest_ts_ms"].as_i64(), du["newest_ts_ms"].as_i64()) {
        (Some(oldest), Some(newest)) if rows > 0 => {
            format!(" ({} → {})", ui::human_ago(oldest), ui::human_ago(newest))
        }
        _ => String::new(),
    };
    println!("{} trace rows{span}", ui::bold(&rows.to_string()));
    let days = du["days"].as_array().cloned().unwrap_or_default();
    if days.is_empty() {
        println!("{}", ui::dim("no body files"));
        return Ok(());
    }
    println!(
        "{} {} {}",
        ui::pad_right(&ui::column_header("date"), 12),
        ui::pad_left(&ui::column_header("files"), 7),
        ui::pad_left(&ui::column_header("size"), 10)
    );
    for d in &days {
        println!(
            "{} {} {}",
            ui::pad_right(&ui::sand(d["date"].as_str().unwrap_or("-")), 12),
            ui::pad_left(&d["files"].as_u64().unwrap_or(0).to_string(), 7),
            ui::pad_left(&ui::human_bytes(d["bytes"].as_u64().unwrap_or(0)), 10)
        );
    }
    Ok(())
}

fn parse_ttl_seconds(s: &str) -> Option<i64> {
    let s = s.trim();
    if let Ok(n) = s.parse::<i64>() {
        return (n > 0).then_some(n);
    }
    let unit = s.chars().last()?;
    let n: i64 = s[..s.len() - unit.len_utf8()].parse().ok()?;
    if n <= 0 {
        return None;
    }
    match unit {
        's' => Some(n),
        'm' => Some(n * 60),
        'h' => Some(n * 3_600),
        'd' => Some(n * 86_400),
        _ => None,
    }
}

async fn daemon_send(
    config: &Config,
    method: reqwest::Method,
    path: &str,
    body: Option<serde_json::Value>,
) -> Result<(reqwest::StatusCode, serde_json::Value)> {
    let base = config.base_url();
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;
    let mut req = client.request(method, format!("{base}{path}"));
    req = req.header("x-api-key", &config.local_key);
    if let Some(b) = body {
        req = req.json(&b);
    }
    let resp = req.send().await.with_context(|| {
        format!("could not reach the alexandria daemon at {base} — is it running?")
    })?;
    let status = resp.status();
    let body: serde_json::Value = resp.json().await.unwrap_or_default();
    Ok((status, body))
}

async fn keys_mint_cmd(
    config: &Config,
    run_id: Option<String>,
    tags: &[String],
    ttl: &str,
    label: Option<String>,
) -> Result<()> {
    let ttl_seconds = parse_ttl_seconds(ttl)
        .with_context(|| format!("invalid --ttl '{ttl}' (use seconds or 45s, 30m, 24h, 7d)"))?;
    let tag_refs: Vec<&str> = tags.iter().map(String::as_str).collect();
    let tag_values = alex_core::parse_trace_tags(&tag_refs);
    let mut body = serde_json::json!({"ttl_seconds": ttl_seconds});
    if let Some(r) = &run_id {
        body["run_id"] = serde_json::json!(r);
    }
    if tag_values
        .as_object()
        .map(|o| !o.is_empty())
        .unwrap_or(false)
    {
        body["tags"] = tag_values;
    }
    if let Some(l) = &label {
        body["label"] = serde_json::json!(l);
    }
    let (status, resp) =
        daemon_send(config, reqwest::Method::POST, "/admin/run-keys", Some(body)).await?;
    if !status.is_success() {
        anyhow::bail!(
            "daemon returned {status}: {}",
            ui::truncate(&resp.to_string(), 300)
        );
    }
    let key = resp["key"].as_str().unwrap_or("-").to_string();
    println!("{}", ui::section("run key minted"));
    println!(
        "{} {}   {} {}   {} {}",
        ui::column_header("id"),
        ui::amber(resp["id"].as_str().unwrap_or("-")),
        ui::column_header("run"),
        resp["run_id"].as_str().unwrap_or("-"),
        ui::column_header("expires"),
        resp["expires_ms"]
            .as_i64()
            .map(|e| format!("in {}", ui::human_ms(e - now_ms())))
            .unwrap_or_else(|| "-".into())
    );
    println!();
    println!("{}", ui::bold(&ui::gold(&key)));
    println!();
    println!(
        "{}",
        ui::dim("shown once — inject into the harness env (any of):")
    );
    println!("export ANTHROPIC_API_KEY={key}");
    println!("export OPENAI_API_KEY={key}");
    println!("export XAI_API_KEY={key}");
    Ok(())
}

async fn keys_list_cmd(config: &Config, all: bool, json: bool) -> Result<()> {
    let params: Vec<(&str, String)> = if all {
        vec![("all", "1".into())]
    } else {
        vec![]
    };
    let resp = daemon_get(config, "/admin/run-keys", &params).await?;
    let body: serde_json::Value = resp.json().await?;
    let rows = body["run_keys"].as_array().cloned().unwrap_or_default();
    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    if rows.is_empty() {
        println!(
            "no run keys{} — mint one with `alexandria keys mint`",
            if all { "" } else { " (try --all)" }
        );
        return Ok(());
    }
    println!("{}", ui::section("run keys"));
    println!(
        "{} {} {} {} {} {}",
        ui::pad_right(&ui::column_header("id"), 12),
        ui::pad_right(&ui::column_header("run"), 18),
        ui::pad_right(&ui::column_header("tags"), 26),
        ui::pad_left(&ui::column_header("uses"), 5),
        ui::pad_right(&ui::column_header("expires"), 10),
        ui::column_header("label")
    );
    for r in &rows {
        let tags = r["tags"]
            .as_object()
            .map(|o| {
                o.iter()
                    .map(|(k, v)| format!("{k}={}", v.as_str().unwrap_or("?")))
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "-".into());
        let expires = if r["revoked"].as_bool().unwrap_or(false) {
            ui::red("revoked")
        } else {
            match r["expires_ms"].as_i64() {
                Some(e) if e <= now_ms() => ui::dim("expired"),
                Some(e) => format!("in {}", ui::human_ms(e - now_ms())),
                None => "never".into(),
            }
        };
        println!(
            "{} {} {} {} {} {}",
            ui::pad_right(&ui::amber(r["id"].as_str().unwrap_or("-")), 12),
            ui::pad_right(
                &ui::turquoise(&ui::truncate(r["run_id"].as_str().unwrap_or("-"), 18)),
                18
            ),
            ui::pad_right(&ui::sand(&ui::truncate(&tags, 26)), 26),
            ui::pad_left(&r["use_count"].as_i64().unwrap_or(0).to_string(), 5),
            ui::pad_right(&expires, 10),
            ui::dim(r["label"].as_str().unwrap_or(""))
        );
    }
    Ok(())
}

async fn keys_revoke_cmd(config: &Config, id: &str) -> Result<()> {
    let (status, resp) = daemon_send(
        config,
        reqwest::Method::DELETE,
        &format!("/admin/run-keys/{id}"),
        None,
    )
    .await?;
    if status == reqwest::StatusCode::NOT_FOUND {
        anyhow::bail!("unknown run key '{id}'");
    }
    if !status.is_success() {
        anyhow::bail!(
            "daemon returned {status}: {}",
            ui::truncate(&resp.to_string(), 300)
        );
    }
    println!("{} revoked {}", ui::gold(ui::ankh()), ui::amber(id));
    Ok(())
}

fn fmt_reset(v: &serde_json::Value) -> String {
    let dt = if let Some(s) = v.as_str() {
        chrono::DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|d| d.with_timezone(&chrono::Local))
    } else {
        v.as_i64()
            .and_then(|s| chrono::DateTime::from_timestamp(s, 0))
            .map(|d| d.with_timezone(&chrono::Local))
    };
    match dt {
        Some(d) => {
            let mins = (d.timestamp() * 1000 - now_ms()) / 60_000;
            let today = chrono::Local::now().date_naive() == d.date_naive();
            let stamp = if today {
                d.format("%H:%M").to_string()
            } else {
                d.format("%a %d %b %H:%M").to_string()
            };
            if mins < 0 {
                format!("{stamp} {}", ui::dim("(passed)"))
            } else {
                format!(
                    "{stamp} {}",
                    ui::dim(&format!("(in {})", ui::human_duration(mins)))
                )
            }
        }
        None => "-".into(),
    }
}

fn print_limits(snap: &serde_json::Value) {
    let providers = snap["providers"].as_array().cloned().unwrap_or_default();
    if providers.is_empty() {
        println!(
            "no limit data yet — send some traffic through the proxy (or run `alexandria ping`)"
        );
        return;
    }
    println!("{}", ui::section("subscription limits"));
    for p in &providers {
        println!();
        let name = p["provider"].as_str().unwrap_or("-");
        let plan = p["plan"]
            .as_str()
            .or(p["active_limit"].as_str())
            .unwrap_or("-");
        let marker = if p["error"].is_string() {
            ui::red(ui::diamond())
        } else {
            ui::gold(ui::diamond())
        };
        let head = format!("{marker} {}  {}", ui::amber(name), ui::bold(plan));
        let source = p["source"]
            .as_str()
            .map(|s| ui::sand(&format!("via {s}")))
            .unwrap_or_default();
        println!("{} {}", ui::pad_right(&head, 54), source);
        if let Some(err) = p["error"].as_str() {
            println!("   {}", ui::red(err));
            continue;
        }
        let windows = p["windows"].as_array().cloned().unwrap_or_default();
        let label_width = windows
            .iter()
            .filter_map(|w| w["window"].as_str())
            .map(str::len)
            .max()
            .unwrap_or(2)
            .max(2);
        let mut printed = false;
        for w in &windows {
            let pct = w["used_pct"].as_f64();
            let bar = pct
                .map(|u| ui::gauge(u, 24))
                .unwrap_or_else(|| ui::dim(&"░".repeat(24)));
            let used = pct
                .map(|u| format!("{u:>3.0}%"))
                .unwrap_or_else(|| "   -".into());
            let reset = if w["resets_at"].is_null() {
                fmt_reset(&w["resets_at_s"])
            } else {
                fmt_reset(&w["resets_at"])
            };
            let status = w["status"]
                .as_str()
                .filter(|s| *s != "allowed")
                .map(|s| format!("  {}", ui::red(&format!("[{s}]"))))
                .unwrap_or_default();
            println!(
                "   {}  {}  {}   resets {}{}",
                ui::pad_right(w["window"].as_str().unwrap_or("-"), label_width),
                bar,
                used,
                reset,
                status
            );
            printed = true;
        }
        if windows.is_empty() {
            for kind in ["requests", "tokens"] {
                let (limit, remaining) = (p[kind]["limit"].as_i64(), p[kind]["remaining"].as_i64());
                if let (Some(l), Some(r)) = (limit, remaining) {
                    let used = 100.0 * (l - r) as f64 / l.max(1) as f64;
                    println!(
                        "   {}  {}  {used:>3.0}%   {}",
                        ui::pad_right(kind, 8),
                        ui::gauge(used, 24),
                        ui::sand(&format!("{r} of {l} remaining"))
                    );
                    printed = true;
                }
            }
        }
        if !printed {
            println!("   {}", ui::dim("no window data captured yet"));
        }
        if let Some(ts) = p["observed_at_ms"].as_i64() {
            println!(
                "   {}",
                ui::dim(&format!(
                    "observed {} from proxied traffic",
                    ui::human_ago(ts)
                ))
            );
        }
    }
}

fn print_dario_status(body: &serde_json::Value) -> Result<()> {
    let Some(gens) = body["generations"].as_array() else {
        println!("{}", ui::dim("unexpected dario status payload — raw JSON:"));
        println!("{}", serde_json::to_string_pretty(body)?);
        return Ok(());
    };
    println!("{}", ui::section("dario generations"));
    let active = body["active_generation_id"].as_str().unwrap_or("-");
    println!("active: {}", ui::bold(&ui::turquoise(active)));
    if gens.is_empty() {
        println!("{}", ui::dim("no generations"));
        return Ok(());
    }
    println!();
    println!(
        "{} {} {} {} {} {} {}  {}",
        ui::pad_right(&ui::column_header("id"), 22),
        ui::pad_right(&ui::column_header("version"), 10),
        ui::pad_right(&ui::column_header("phase"), 10),
        ui::pad_left(&ui::column_header("pid"), 7),
        ui::pad_left(&ui::column_header("port"), 6),
        ui::pad_left(&ui::column_header("in-flight"), 9),
        ui::pad_right(&ui::column_header("last probe"), 24),
        ui::column_header("age")
    );
    for g in gens {
        let phase_raw = g["phase"].as_str().unwrap_or("-");
        let phase = match phase_raw {
            "ready" => ui::green(phase_raw),
            "starting" | "draining" => ui::yellow(phase_raw),
            "unhealthy" => ui::red(phase_raw),
            "dead" => ui::dim(phase_raw),
            _ => phase_raw.to_string(),
        };
        let probe = match &g["last_probe"] {
            serde_json::Value::Null => ui::dim("-"),
            p if p["ok"].as_bool() == Some(true) => {
                let latency = p["latency_ms"]
                    .as_i64()
                    .map(|l| format!(" {l}ms"))
                    .unwrap_or_default();
                format!("{}{}", ui::green("✓"), ui::dim(&latency))
            }
            p => {
                let detail = p["error"]
                    .as_str()
                    .map(String::from)
                    .or_else(|| p["status"].as_i64().map(|s| format!("status {s}")))
                    .unwrap_or_else(|| "failed".into());
                format!("{} {}", ui::red("✗"), ui::red(&ui::truncate(&detail, 40)))
            }
        };
        let age = g["started_at"]
            .as_i64()
            .map(ui::human_ago)
            .unwrap_or_else(|| "-".into());
        println!(
            "{} {} {} {} {} {} {}  {}",
            ui::pad_right(g["id"].as_str().unwrap_or("-"), 22),
            ui::pad_right(&ui::turquoise(g["version"].as_str().unwrap_or("-")), 10),
            ui::pad_right(&phase, 10),
            ui::pad_left(
                &g["pid"]
                    .as_i64()
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "-".into()),
                7
            ),
            ui::pad_left(
                &g["port"]
                    .as_i64()
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "-".into()),
                6
            ),
            ui::pad_left(
                &g["in_flight"]
                    .as_i64()
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "-".into()),
                9
            ),
            ui::pad_right(&probe, 24),
            ui::sand(&age)
        );
    }
    Ok(())
}

fn print_banner(host: &str, port: u16, local_key: &str) {
    eprintln!("{}", ui::divider("alexandria"));
    eprintln!(
        "daemon listening on {}",
        ui::bold(&ui::lapis(&format!("http://{host}:{port}")))
    );
    eprintln!(
        "  health:   {}",
        ui::lapis(&format!("http://{host}:{port}/health"))
    );
    eprintln!(
        "  traces:   {}",
        ui::lapis(&format!("http://{host}:{port}/admin/traces"))
    );
    eprintln!(
        "  accounts: {}",
        ui::lapis(&format!("http://{host}:{port}/admin/accounts"))
    );
    eprintln!();
    print_env(host, port, local_key);
}

fn print_env(host: &str, port: u16, local_key: &str) {
    eprintln!(
        "{}",
        ui::dim("# anthropic-format harnesses (claude-code, …)")
    );
    eprintln!("export ANTHROPIC_BASE_URL=http://{host}:{port}");
    eprintln!("export ANTHROPIC_API_KEY={local_key}");
    eprintln!("{}", ui::dim("# openai-format harnesses (codex, pi, …)"));
    eprintln!("export OPENAI_BASE_URL=http://{host}:{port}/v1");
    eprintln!("export OPENAI_API_KEY={local_key}");
    eprintln!("{}", ui::dim("# xai/grok harnesses"));
    eprintln!("export XAI_API_KEY={local_key}");
    eprintln!("export GROK_MODELS_BASE_URL=http://{host}:{port}/v1");
    eprintln!(
        "{}",
        ui::dim("# gemini-cli (needs security.auth.selectedType=gemini-api-key)")
    );
    eprintln!("export GOOGLE_GEMINI_BASE_URL=http://{host}:{port}");
    eprintln!("export GOOGLE_GENAI_API_VERSION=v1beta");
    eprintln!("export GEMINI_API_KEY={local_key}");
    eprintln!("export GEMINI_API_KEY_AUTH_MECHANISM=bearer");
    eprintln!("export GOOGLE_GENAI_USE_GCA=false");
}

fn account_indicators(a: &alex_auth::Account) -> (String, String) {
    let remaining_ms = a.expires_at_ms.map(|exp| exp - now_ms());
    let expired = remaining_ms.map(|r| r < 0).unwrap_or(false);
    let expiring = remaining_ms
        .map(|r| r >= 0 && r < 30 * 60_000)
        .unwrap_or(false);
    let cooldown = a.cooldown_until_ms.map(|c| c > now_ms()).unwrap_or(false);
    let active = a.status == "active" && !cooldown;
    let dot = if expired || !active {
        ui::red(ui::dot())
    } else if expiring {
        ui::yellow(ui::dot())
    } else {
        ui::green(ui::dot())
    };
    let expiry = match remaining_ms {
        Some(r) if r < 0 => ui::red(&format!("expired {} ago", ui::human_ms(-r))),
        Some(r) if r < 30 * 60_000 => ui::yellow(&format!("expires in {}", ui::human_ms(r))),
        Some(r) => format!("expires in {}", ui::human_ms(r)),
        None => ui::dim("no expiry"),
    };
    (dot, expiry)
}

#[derive(Debug, PartialEq)]
#[allow(dead_code)]
enum ServiceState {
    LaunchdLoaded {
        pid: Option<i64>,
    },
    LaunchdNotLoaded,
    LaunchdNotInstalled,
    Systemd {
        enabled: bool,
        active: bool,
        unit_present: bool,
    },
    SystemdMissing,
    Unsupported,
}

#[allow(dead_code)]
fn parse_launchctl_pid(output: &str) -> Option<i64> {
    output.lines().find_map(|l| {
        l.trim()
            .strip_prefix("pid = ")
            .and_then(|v| v.trim().parse().ok())
    })
}

fn ping_done_line(r: &alex_proxy::PingResult) -> String {
    let (mark, bar_color) = if r.ok {
        (ui::green("✓"), "42")
    } else {
        (ui::red("✗"), "196")
    };
    let flat = r.message.split_whitespace().collect::<Vec<_>>().join(" ");
    format!(
        "{} {} {}  {} {} {}",
        mark,
        ui::pad_right(&ui::amber(&ui::bold(r.provider)), 10),
        ui::progress_bar(100.0, 12, bar_color),
        ui::status_color(r.status.map(i64::from)),
        ui::pad_left(&ui::sand(&format!("{}ms", r.latency_ms)), 7),
        ui::dim(&flat)
    )
}

async fn run_pings(
    state: &Arc<alex_proxy::AppState>,
    models: &alex_proxy::PingModels,
    providers: &[alex_core::Provider],
) -> Vec<alex_proxy::PingResult> {
    use std::io::{IsTerminal, Write};
    let n = providers.len();
    let slots: Arc<std::sync::Mutex<Vec<Option<alex_proxy::PingResult>>>> =
        Arc::new(std::sync::Mutex::new(vec![None; n]));
    let mut handles = Vec::new();
    for (i, provider) in providers.iter().enumerate() {
        let state = state.clone();
        let models = models.clone();
        let slots = slots.clone();
        let provider = *provider;
        handles.push(tokio::spawn(async move {
            let r = alex_proxy::ping_provider(&state, provider, &models).await;
            let _ = state.store.insert_heartbeat(
                now_ms(),
                r.provider,
                r.account_id.as_deref(),
                r.ok,
                r.status.map(|s| s as i64),
                r.latency_ms,
                &r.message,
            );
            slots.lock().unwrap()[i] = Some(r);
        }));
    }
    let model_for = |p: alex_core::Provider| match p {
        alex_core::Provider::Anthropic => models.anthropic.clone(),
        alex_core::Provider::Openai => models.openai.clone(),
        alex_core::Provider::Xai => models.xai.clone(),
        alex_core::Provider::Gemini => models.gemini.clone(),
    };
    if std::io::stdout().is_terminal() {
        println!("{}", ui::section("provider health"));
        print!("\x1b[?25l");
        let start = std::time::Instant::now();
        let mut frame = 0usize;
        let mut printed = 0usize;
        loop {
            let snap = slots.lock().unwrap().clone();
            let width = ui::term_width().saturating_sub(1);
            if printed > 0 {
                print!("\x1b[{printed}A");
            }
            printed = n;
            for (i, provider) in providers.iter().enumerate() {
                let elapsed = start.elapsed().as_secs_f32();
                let line = match &snap[i] {
                    Some(r) => ping_done_line(r),
                    None => format!(
                        "{} {} {}  {}",
                        ui::gold(ui::spinner(frame)),
                        ui::pad_right(&ui::amber(&ui::bold(provider.as_str())), 10),
                        ui::progress_bar((elapsed as f64 / 6.0 * 100.0).min(94.0), 12, "178"),
                        ui::dim(&format!("{} · {elapsed:.1}s", model_for(*provider)))
                    ),
                };
                print!("\r\x1b[2K");
                println!("{}", ui::clip(&line, width));
            }
            let _ = std::io::stdout().flush();
            if snap.iter().all(Option::is_some) {
                break;
            }
            frame += 1;
            tokio::time::sleep(std::time::Duration::from_millis(120)).await;
        }
        print!("\x1b[?25h");
        let _ = std::io::stdout().flush();
    }
    for h in handles {
        let _ = h.await;
    }
    let results: Vec<alex_proxy::PingResult> =
        slots.lock().unwrap().iter().flatten().cloned().collect();
    if !std::io::stdout().is_terminal() {
        for r in &results {
            println!("{}", ping_done_line(r));
        }
    }
    results
}

const LAUNCHD_TEMPLATE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/config/launchd/com.alexandria.daemon.plist"
));
const SYSTEMD_TEMPLATE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/config/systemd/alexandria.service"
));

fn current_uid() -> String {
    std::process::Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

fn service_install() -> Result<()> {
    let exe = std::env::current_exe()?
        .canonicalize()
        .context("resolving current binary path")?;
    let exe_str = exe.to_string_lossy().to_string();
    if exe_str.contains("/target/") {
        eprintln!(
            "{}",
            ui::yellow(&format!(
                "warning: installing a development build path into the service ({exe_str})"
            ))
        );
    }
    if cfg!(target_os = "macos") {
        let dst = dirs::home_dir()
            .context("no home dir")?
            .join("Library/LaunchAgents/com.alexandria.daemon.plist");
        std::fs::create_dir_all(dst.parent().unwrap())?;
        std::fs::write(
            &dst,
            LAUNCHD_TEMPLATE.replace("/usr/local/bin/alexandria", &exe_str),
        )?;
        let uid = current_uid();
        let _ = std::process::Command::new("launchctl")
            .args(["bootout", &format!("gui/{uid}")])
            .arg(&dst)
            .output();
        let out = std::process::Command::new("launchctl")
            .args(["bootstrap", &format!("gui/{uid}")])
            .arg(&dst)
            .output()
            .context("running launchctl bootstrap")?;
        if !out.status.success() {
            anyhow::bail!(
                "launchctl bootstrap failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        println!(
            "{} {}",
            ui::gold(ui::ankh()),
            ui::bold("launchd service installed and started")
        );
        println!("  {}", ui::dim(&dst.to_string_lossy()));
    } else if cfg!(target_os = "linux") {
        let dst = dirs::home_dir()
            .context("no home dir")?
            .join(".config/systemd/user/alexandria.service");
        std::fs::create_dir_all(dst.parent().unwrap())?;
        let unit: String = SYSTEMD_TEMPLATE
            .lines()
            .map(|l| {
                if l.starts_with("ExecStart=") {
                    format!("ExecStart={exe_str} daemon")
                } else {
                    l.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(&dst, unit + "\n")?;
        for args in [
            vec!["--user", "daemon-reload"],
            vec!["--user", "enable", "--now", "alexandria"],
        ] {
            let out = std::process::Command::new("systemctl")
                .args(&args)
                .output()
                .context("running systemctl")?;
            if !out.status.success() {
                anyhow::bail!(
                    "systemctl {} failed: {}",
                    args.join(" "),
                    String::from_utf8_lossy(&out.stderr)
                );
            }
        }
        println!(
            "{} {}",
            ui::gold(ui::ankh()),
            ui::bold("systemd user service installed and started")
        );
        println!("  {}", ui::dim(&dst.to_string_lossy()));
        println!(
            "  {}",
            ui::dim("tip: loginctl enable-linger $USER keeps it running after logout")
        );
    } else {
        anyhow::bail!("service install supports macOS (launchd) and Linux (systemd) only");
    }
    println!("  {}", service_state_label(&detect_service_state()));
    Ok(())
}

fn service_uninstall() -> Result<()> {
    if cfg!(target_os = "macos") {
        let dst = dirs::home_dir()
            .context("no home dir")?
            .join("Library/LaunchAgents/com.alexandria.daemon.plist");
        let uid = current_uid();
        let _ = std::process::Command::new("launchctl")
            .args(["bootout", &format!("gui/{uid}")])
            .arg(&dst)
            .output();
        if dst.exists() {
            std::fs::remove_file(&dst)?;
        }
        println!("launchd service removed");
    } else if cfg!(target_os = "linux") {
        let _ = std::process::Command::new("systemctl")
            .args(["--user", "disable", "--now", "alexandria"])
            .output();
        let dst = dirs::home_dir()
            .context("no home dir")?
            .join(".config/systemd/user/alexandria.service");
        if dst.exists() {
            std::fs::remove_file(&dst)?;
        }
        let _ = std::process::Command::new("systemctl")
            .args(["--user", "daemon-reload"])
            .output();
        println!("systemd user service removed");
    } else {
        anyhow::bail!("service uninstall supports macOS and Linux only");
    }
    Ok(())
}

fn service_state_label(state: &ServiceState) -> String {
    match state {
        ServiceState::LaunchdLoaded { pid: Some(p) } => {
            format!("launchd: installed + loaded (pid {p})")
        }
        ServiceState::LaunchdLoaded { pid: None } => "launchd: installed + loaded".into(),
        ServiceState::LaunchdNotLoaded => {
            "launchd: installed but not loaded → alexandria service install".into()
        }
        ServiceState::LaunchdNotInstalled => {
            "launchd: not installed → alexandria service install".into()
        }
        ServiceState::Systemd {
            enabled: true,
            active: true,
            ..
        } => "systemd: enabled + active".into(),
        ServiceState::Systemd {
            enabled: true,
            active: false,
            ..
        } => "systemd: enabled but not active → systemctl --user start alexandria".into(),
        ServiceState::Systemd {
            enabled: false,
            active: true,
            ..
        } => "systemd: active but not enabled → systemctl --user enable alexandria".into(),
        ServiceState::Systemd {
            unit_present: true, ..
        } => "systemd: installed but disabled → systemctl --user enable --now alexandria".into(),
        ServiceState::Systemd { .. } => {
            "systemd: not installed → alexandria service install".into()
        }
        ServiceState::SystemdMissing => "systemd: systemctl not found".into(),
        ServiceState::Unsupported => "service management: unsupported OS".into(),
    }
}

fn service_managed(state: &ServiceState) -> bool {
    matches!(
        state,
        ServiceState::LaunchdLoaded { .. } | ServiceState::Systemd { active: true, .. }
    )
}

fn detect_service_state() -> ServiceState {
    #[cfg(target_os = "macos")]
    return detect_service_state_macos();
    #[cfg(target_os = "linux")]
    return detect_service_state_linux();
    #[allow(unreachable_code)]
    ServiceState::Unsupported
}

#[cfg(target_os = "macos")]
fn detect_service_state_macos() -> ServiceState {
    let uid = std::process::Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    if let Ok(o) = std::process::Command::new("launchctl")
        .args(["print", &format!("gui/{uid}/com.alexandria.daemon")])
        .output()
    {
        if o.status.success() {
            return ServiceState::LaunchdLoaded {
                pid: parse_launchctl_pid(&String::from_utf8_lossy(&o.stdout)),
            };
        }
    }
    let plist = dirs::home_dir()
        .map(|h| h.join("Library/LaunchAgents/com.alexandria.daemon.plist"))
        .filter(|p| p.exists());
    if plist.is_some() {
        ServiceState::LaunchdNotLoaded
    } else {
        ServiceState::LaunchdNotInstalled
    }
}

#[cfg(target_os = "linux")]
fn detect_service_state_linux() -> ServiceState {
    let run = |arg: &str| {
        std::process::Command::new("systemctl")
            .args(["--user", arg, "alexandria"])
            .output()
    };
    match run("is-enabled") {
        Err(_) => ServiceState::SystemdMissing,
        Ok(enabled_out) => {
            let enabled = enabled_out.status.success();
            let active = run("is-active")
                .map(|o| o.status.success())
                .unwrap_or(false);
            let unit_present = dirs::home_dir()
                .map(|h| h.join(".config/systemd/user/alexandria.service").exists())
                .unwrap_or(false);
            ServiceState::Systemd {
                enabled,
                active,
                unit_present,
            }
        }
    }
}

fn installed_binaries() -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(path) = std::env::var_os("PATH") {
        candidates.extend(std::env::split_paths(&path).map(|d| d.join("alexandria")));
    }
    candidates.push(PathBuf::from("/usr/local/bin/alexandria"));
    if let Some(h) = dirs::home_dir() {
        candidates.push(h.join(".local").join("bin").join("alexandria"));
    }
    let mut found: Vec<PathBuf> = Vec::new();
    for c in candidates {
        if c.is_file() && !found.contains(&c) {
            found.push(c);
        }
    }
    found
}

fn parse_dario_update_notice(raw: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(raw).ok()?;
    if v["update_available"].as_bool() != Some(true) {
        return None;
    }
    let latest = v["latest"].as_str()?;
    let running = v["active_version"].as_str().unwrap_or("unknown");
    Some(format!(
        "dario {latest} available (running {running}) — alexandria dario update"
    ))
}

fn print_dario_update_notice() {
    let path = alexandria_home().join("dario").join("update-state.json");
    let Ok(raw) = std::fs::read_to_string(path) else {
        return;
    };
    if let Some(notice) = parse_dario_update_notice(&raw) {
        println!(
            "{}",
            ui::dim(&ui::sand(&format!("{} {notice}", ui::ankh())))
        );
    }
}

async fn fetch_json(
    client: &reqwest::Client,
    url: &str,
    key: &str,
) -> Option<(u16, serde_json::Value)> {
    let resp = client.get(url).header("x-api-key", key).send().await.ok()?;
    let status = resp.status().as_u16();
    let value = resp.json().await.unwrap_or(serde_json::Value::Null);
    Some((status, value))
}

async fn daemon_background(
    host: &str,
    port: u16,
    host_arg: Option<String>,
    port_arg: Option<u16>,
) -> Result<()> {
    let base = format!("http://{host}:{port}");
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()?;
    let healthy = |client: reqwest::Client, base: String| async move {
        client
            .get(format!("{base}/health"))
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false)
    };
    if healthy(client.clone(), base.clone()).await {
        println!(
            "{}",
            ui::gold(&format!("{} daemon already running at {base}", ui::ankh()))
        );
        return Ok(());
    }
    let log_path = alexandria_home().join("daemon.log");
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;
    let mut cmd = std::process::Command::new(std::env::current_exe()?);
    cmd.arg("daemon");
    if let Some(h) = host_arg {
        cmd.args(["--host", &h]);
    }
    if let Some(p) = port_arg {
        cmd.args(["--port", &p.to_string()]);
    }
    cmd.stdin(std::process::Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let child = cmd.spawn()?;
    println!(
        "{}",
        ui::gold(&format!(
            "{} daemon started in the background (pid {}) — log: ~/.alexandria/daemon.log",
            ui::ankh(),
            child.id()
        ))
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        if healthy(client.clone(), base.clone()).await {
            println!(
                "{} daemon ready at {}",
                ui::green(ui::dot()),
                ui::bold(&ui::lapis(&base))
            );
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }
    println!(
        "{} daemon did not become ready within 15s — check: tail -n 50 ~/.alexandria/daemon.log",
        ui::red(ui::dot())
    );
    std::process::exit(1);
}

struct RawModeGuard;

impl RawModeGuard {
    fn new() -> Result<Self> {
        crossterm::terminal::enable_raw_mode()?;
        let _ = crossterm::execute!(std::io::stdout(), crossterm::cursor::Hide);
        Ok(Self)
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = crossterm::execute!(std::io::stdout(), crossterm::cursor::Show);
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

fn provider_menu_status(accounts: &[alex_auth::Account], provider: &str) -> String {
    let target = alex_core::Provider::from_str_loose(provider);
    let account = accounts.iter().find(|a| Some(a.provider) == target);
    match account {
        Some(a) => {
            let (dot, expiry) = account_indicators(a);
            let expired = a
                .expires_at_ms
                .map(|exp| exp - now_ms() < 0)
                .unwrap_or(false);
            if expired {
                format!("{dot} {expiry} {}", ui::red("— re-login recommended"))
            } else {
                let label = a.label.clone().unwrap_or_else(|| a.id.clone());
                format!("{dot} logged in ({label}, {expiry})")
            }
        }
        None if provider == "gemini" => format!(
            "{} {}",
            ui::dim(ui::circle()),
            ui::dim("import only (gemini CLI login)")
        ),
        None => format!("{} {}", ui::dim(ui::circle()), ui::dim("not logged in")),
    }
}

fn pick_provider(accounts: &[alex_auth::Account]) -> Result<Option<String>> {
    use crossterm::cursor::{MoveToColumn, MoveUp};
    use crossterm::event::{read, Event, KeyCode, KeyEventKind, KeyModifiers};
    use crossterm::terminal::{Clear, ClearType};
    use std::io::Write;

    let providers = alex_auth::login::PROVIDERS;
    let statuses: Vec<String> = providers
        .iter()
        .map(|p| provider_menu_status(accounts, p))
        .collect();
    let mut out = std::io::stdout();
    writeln!(
        out,
        "{} {}",
        ui::gold(ui::ankh()),
        ui::bold("choose a provider to authenticate")
    )?;
    let guard = RawModeGuard::new()?;
    let mut selected = 0usize;
    let mut drawn = false;
    let choice = loop {
        if drawn {
            crossterm::execute!(out, MoveUp(providers.len() as u16))?;
        }
        for (i, p) in providers.iter().enumerate() {
            crossterm::execute!(out, MoveToColumn(0), Clear(ClearType::CurrentLine))?;
            let marker = if i == selected {
                ui::gold(ui::selector())
            } else {
                " ".into()
            };
            let name = if i == selected {
                ui::bold(p)
            } else {
                (*p).to_string()
            };
            write!(
                out,
                " {marker} {} {}\r\n",
                ui::pad_right(&name, 8),
                statuses[i]
            )?;
        }
        out.flush()?;
        drawn = true;
        match read()? {
            Event::Key(k) if k.kind != KeyEventKind::Release => match k.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    selected = selected.checked_sub(1).unwrap_or(providers.len() - 1)
                }
                KeyCode::Down | KeyCode::Char('j') => selected = (selected + 1) % providers.len(),
                KeyCode::Enter => break Some(providers[selected].to_string()),
                KeyCode::Esc | KeyCode::Char('q') => break None,
                KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => break None,
                _ => {}
            },
            _ => {}
        }
    };
    drop(guard);
    if choice.is_none() {
        println!("{}", ui::dim("cancelled"));
    }
    Ok(choice)
}

async fn run_status(config: &Config, json: bool) -> Result<()> {
    if !json && ui::colors_enabled() {
        if let Some(banner) = light::logo_banner(ui::term_width()) {
            println!("{banner}");
        }
    }
    let base = config.base_url();
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()?;
    let key = config.local_key.as_str();
    let health = fetch_json(&client, &format!("{base}/health"), key)
        .await
        .filter(|(s, _)| (200..300).contains(s))
        .map(|(_, v)| v);
    let running = health.is_some();
    let service = detect_service_state();
    let binaries = installed_binaries();
    let current_exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.canonicalize().ok());
    let vault = open_vault(config)?;
    let accounts = vault.list().await;
    let admin_health = if running {
        fetch_json(&client, &format!("{base}/admin/health"), key)
            .await
            .filter(|(s, _)| (200..300).contains(s))
            .map(|(_, v)| v)
    } else {
        None
    };
    let limits = if running {
        fetch_json(&client, &format!("{base}/admin/limits"), key)
            .await
            .filter(|(s, _)| (200..300).contains(s))
            .map(|(_, v)| v)
    } else {
        None
    };
    let dario = if running {
        fetch_json(&client, &format!("{base}/admin/dario"), key).await
    } else {
        None
    };
    let heartbeat_for = |id: &str| -> serde_json::Value {
        admin_health
            .as_ref()
            .and_then(|v| v["accounts"].as_array())
            .and_then(|arr| arr.iter().find(|a| a["id"].as_str() == Some(id)))
            .map(|a| a["last_heartbeat"].clone())
            .unwrap_or(serde_json::Value::Null)
    };

    if json {
        let binaries_json: Vec<serde_json::Value> = binaries
            .iter()
            .map(|p| {
                serde_json::json!({
                    "path": p.to_string_lossy(),
                    "this_binary": current_exe.is_some() && p.canonicalize().ok() == current_exe,
                })
            })
            .collect();
        let accounts_json: Vec<serde_json::Value> = accounts
            .iter()
            .map(|a| {
                serde_json::json!({
                    "id": a.id,
                    "provider": a.provider.as_str(),
                    "kind": a.kind,
                    "label": a.label,
                    "status": a.status,
                    "expires_at_ms": a.expires_at_ms,
                    "last_heartbeat": heartbeat_for(&a.id),
                })
            })
            .collect();
        let combined = serde_json::json!({
            "daemon": {
                "version": env!("CARGO_PKG_VERSION"),
                "binaries": binaries_json,
                "service": {
                    "state": service_state_label(&service),
                    "managed": service_managed(&service),
                },
                "running": running,
                "health": health,
                "base_url": base,
                "openai_base_url": format!("{base}/v1"),
                "env": {
                    "ANTHROPIC_BASE_URL": base,
                    "ANTHROPIC_API_KEY": config.local_key,
                    "OPENAI_BASE_URL": format!("{base}/v1"),
                    "OPENAI_API_KEY": config.local_key,
                },
            },
            "accounts": accounts_json,
            "limits": limits,
            "dario": dario
                .as_ref()
                .filter(|(s, _)| (200..300).contains(s))
                .map(|(_, v)| v.clone()),
        });
        println!("{}", serde_json::to_string_pretty(&combined)?);
        return Ok(());
    }

    println!("{}", ui::section("daemon"));
    if binaries.is_empty() {
        println!(
            "  {} {}",
            ui::pad_right(&ui::sand("binary"), 10),
            ui::dim("alexandria not found on PATH")
        );
    } else {
        let mut first = true;
        for p in &binaries {
            let label = if first {
                ui::sand("binary")
            } else {
                String::new()
            };
            first = false;
            let this = current_exe.is_some() && p.canonicalize().ok() == current_exe;
            let suffix = if this {
                format!(" {}", ui::dim("(this binary)"))
            } else {
                String::new()
            };
            println!("  {} {}{suffix}", ui::pad_right(&label, 10), p.display());
        }
    }
    println!(
        "  {} {}",
        ui::pad_right(&ui::sand("version"), 10),
        env!("CARGO_PKG_VERSION")
    );
    let sdot = if service_managed(&service) {
        ui::green(ui::dot())
    } else {
        match service {
            ServiceState::LaunchdNotLoaded
            | ServiceState::Systemd {
                unit_present: true, ..
            } => ui::yellow(ui::dot()),
            _ => ui::dim(ui::circle()),
        }
    };
    println!(
        "  {} {sdot} {}",
        ui::pad_right(&ui::sand("service"), 10),
        service_state_label(&service)
    );
    if let ServiceState::Systemd { .. } = service {
        println!(
            "  {} {}",
            ui::pad_right("", 10),
            ui::dim("unit: ~/.config/systemd/user/alexandria.service")
        );
    }
    match &health {
        Some(h) => {
            let version = h["version"].as_str().unwrap_or("?");
            let mut details: Vec<String> = Vec::new();
            if let Some(s) = h["uptime_s"].as_i64() {
                details.push(format!("up {}", ui::human_ms(s * 1000)));
            }
            if let Some(n) = h["in_flight"].as_i64() {
                details.push(format!("{n} in flight"));
            }
            if let Some(d) = h["dario"].as_bool() {
                details.push(format!("dario {}", if d { "on" } else { "off" }));
            }
            let detail = if details.is_empty() {
                String::new()
            } else {
                format!(" — {}", details.join(", "))
            };
            println!(
                "  {} {} {}{}",
                ui::pad_right(&ui::sand("process"), 10),
                ui::green(ui::dot()),
                ui::green(&format!("running v{version}")),
                ui::dim(&detail)
            );
            if !service_managed(&service) {
                println!(
                    "  {} {}",
                    ui::pad_right("", 10),
                    ui::yellow("running ad-hoc (not service-managed)")
                );
            }
        }
        None => {
            println!(
                "  {} {} {}",
                ui::pad_right(&ui::sand("process"), 10),
                ui::red(ui::dot()),
                ui::red("not running")
            );
            println!(
                "  {} {}",
                ui::pad_right("", 10),
                ui::dim("start: alexandria daemon --background")
            );
        }
    }
    println!(
        "  {} {}  {}",
        ui::pad_right(&ui::sand("endpoint"), 10),
        ui::bold(&ui::lapis(&base)),
        ui::bold(&ui::lapis(&format!("{base}/v1")))
    );
    println!();
    println!(
        "{}",
        ui::dim("# anthropic-format harnesses (claude-code, …)")
    );
    println!("export ANTHROPIC_BASE_URL={base}");
    println!("export ANTHROPIC_API_KEY={}", config.local_key);
    println!("{}", ui::dim("# openai-format harnesses (codex, pi, …)"));
    println!("export OPENAI_BASE_URL={base}/v1");
    println!("export OPENAI_API_KEY={}", config.local_key);
    println!(
        "{}",
        ui::dim("# gemini-cli (needs security.auth.selectedType=gemini-api-key)")
    );
    println!("export GOOGLE_GEMINI_BASE_URL={base}");
    println!("export GEMINI_API_KEY={}", config.local_key);
    println!("export GEMINI_API_KEY_AUTH_MECHANISM=bearer");
    println!("export GOOGLE_GENAI_USE_GCA=false");

    println!();
    println!("{}", ui::section("accounts"));
    if accounts.is_empty() {
        println!("{}", ui::dim("no accounts — run `alexandria auth import`"));
    }
    for a in &accounts {
        let (dot, expiry) = account_indicators(a);
        let hb = heartbeat_for(&a.id);
        let hb_str = if hb.is_object() {
            let ok = hb["ok"].as_bool().unwrap_or(false);
            let age = hb["ts_ms"]
                .as_i64()
                .map(ui::human_ago)
                .unwrap_or_else(|| "-".into());
            if ok {
                format!("{} {}", ui::green("✓"), ui::dim(&age))
            } else {
                let msg = hb["message"]
                    .as_str()
                    .unwrap_or("failed")
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ");
                format!(
                    "{} {} {}",
                    ui::red("✗"),
                    ui::dim(&age),
                    ui::red(&ui::truncate(&msg, 48))
                )
            }
        } else {
            ui::dim("-")
        };
        println!(
            "{dot} {} {} {} {} {}  {hb_str}",
            ui::pad_right(&a.id, 18),
            ui::pad_right(&ui::amber(a.provider.as_str()), 10),
            ui::pad_right(&a.kind, 8),
            ui::pad_right(&expiry, 20),
            ui::pad_right(&ui::sand(a.label.as_deref().unwrap_or("")), 24)
        );
    }

    println!();
    match &limits {
        Some(snap) => print_limits(snap),
        None => println!("{}", ui::dim("limits: skipped (daemon not running)")),
    }

    match &dario {
        Some((s, body)) if (200..300).contains(s) => {
            println!();
            print_dario_status(body)?;
        }
        Some((404, _)) => {
            println!();
            println!("{}", ui::dim("dario mode disabled"));
        }
        Some((s, _)) => {
            println!();
            println!("{}", ui::red(&format!("dario status returned {s}")));
        }
        None => {}
    }
    print_dario_update_notice();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::{Method, StatusCode};

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn tmpdir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "alex-main-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn test_config(home: PathBuf) -> Config {
        Config {
            host: "127.0.0.1".into(),
            port: 4100,
            data_dir: home,
            local_key: "alx-local".into(),
            heartbeat_minutes: default_heartbeat_minutes(),
            ping_anthropic_model: default_ping_anthropic(),
            ping_openai_model: default_ping_openai(),
            ping_xai_model: default_ping_xai(),
            ping_gemini_model: default_ping_gemini(),
            gemini_project: String::new(),
            anthropic_upstream: "direct".into(),
            dario_api_key: String::new(),
            dario_update_check_minutes: 60,
            dario_version: None,
            dario_probe_seconds: 90,
            dario_probe_failures: 2,
            dario_probe_model: "claude-haiku-4-5".into(),
            trace_body_retention_days: default_trace_body_retention_days(),
            trace_row_retention_days: 0,
            update_check_hours: default_update_check_hours(),
            harness_overrides: BTreeMap::new(),
        }
    }

    #[cfg(unix)]
    fn fake_executable(dir: &PathBuf, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn test_state(name: &str) -> Arc<alex_proxy::AppState> {
        let dir = tmpdir(name);
        let store = Arc::new(Store::open(dir.join("store")).unwrap());
        let vault = Arc::new(Vault::open(dir.join("vault")).unwrap());
        alex_proxy::build_state(
            "alx-local".into(),
            vault,
            store,
            None,
            "http://127.0.0.1:4100".into(),
        )
    }

    async fn router_json(
        app: axum::Router,
        method: Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> (StatusCode, serde_json::Value) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        let client = reqwest::Client::new();
        let mut request = client
            .request(method, format!("http://{addr}{path}"))
            .header("x-api-key", "alx-local");
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.unwrap();
        let status = response.status();
        let value = response
            .json::<serde_json::Value>()
            .await
            .unwrap_or(serde_json::Value::Null);
        server.abort();
        (status, value)
    }

    #[test]
    fn launchctl_pid_parsing() {
        let out =
            "com.alexandria.daemon = {\n\tactive count = 1\n\tpid = 96513\n\tstate = running\n}";
        assert_eq!(parse_launchctl_pid(out), Some(96513));
        assert_eq!(parse_launchctl_pid("state = running"), None);
        assert_eq!(parse_launchctl_pid(""), None);
    }

    #[test]
    fn home_dir_is_platform_native() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::remove_var("ALEXANDRIA_HOME");
        let home = alexandria_home();
        assert!(home.ends_with(".alexandria"));
        assert!(home.is_absolute());
    }

    #[cfg(windows)]
    #[test]
    fn windows_reports_service_unsupported() {
        assert!(matches!(detect_service_state(), ServiceState::Unsupported));
        assert!(service_install().is_err());
        assert!(!service_managed(&ServiceState::Unsupported));
    }

    #[test]
    fn unsupported_service_state_has_readable_label() {
        let label = service_state_label(&ServiceState::Unsupported);
        assert!(label.contains("unsupported"), "got: {label}");
    }

    #[test]
    fn config_toml_roundtrip_with_native_paths() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::remove_var("ALEXANDRIA_HOME");
        let config = Config {
            host: "127.0.0.1".into(),
            port: 4100,
            data_dir: alexandria_home(),
            local_key: "alx-test".into(),
            heartbeat_minutes: default_heartbeat_minutes(),
            ping_anthropic_model: default_ping_anthropic(),
            ping_openai_model: default_ping_openai(),
            ping_xai_model: default_ping_xai(),
            ping_gemini_model: default_ping_gemini(),
            gemini_project: String::new(),
            anthropic_upstream: "direct".into(),
            dario_api_key: String::new(),
            dario_update_check_minutes: 60,
            dario_version: None,
            dario_probe_seconds: 90,
            dario_probe_failures: 2,
            dario_probe_model: "claude-haiku-4-5".into(),
            trace_body_retention_days: default_trace_body_retention_days(),
            trace_row_retention_days: 0,
            update_check_hours: default_update_check_hours(),
            harness_overrides: BTreeMap::new(),
        };
        let text = toml::to_string_pretty(&config).unwrap();
        let reloaded: Config = toml::from_str(&text).unwrap();
        assert_eq!(reloaded.port, config.port);
        assert_eq!(reloaded.local_key, config.local_key);
        assert_eq!(reloaded.data_dir, config.data_dir);
        assert!(reloaded.data_dir.is_absolute());
    }

    #[test]
    fn config_old_toml_defaults_harness_overrides() {
        let config = test_config(tmpdir("old-config"));
        let text = toml::to_string_pretty(&config).unwrap();
        let old_text = text
            .lines()
            .filter(|line| !line.starts_with("harness_overrides"))
            .collect::<Vec<_>>()
            .join("\n");
        let reloaded: Config = toml::from_str(&old_text).unwrap();
        assert!(reloaded.harness_overrides.is_empty());
    }

    #[test]
    fn save_load_preserves_harness_overrides() {
        let _guard = ENV_LOCK.lock().unwrap();
        let home = tmpdir("config-home");
        std::env::set_var("ALEXANDRIA_HOME", &home);
        let mut config = test_config(home.clone());
        config.harness_overrides.insert(
            "pi".into(),
            HarnessOverride {
                binary: Some(home.join("bin").join("pi")),
                config_dir: Some(home.join("pi-agent")),
            },
        );
        save_config(&config).unwrap();
        let (loaded, fresh) = load_or_create_config().unwrap();
        std::env::remove_var("ALEXANDRIA_HOME");
        assert!(!fresh);
        let override_ = loaded.harness_overrides.get("pi").unwrap();
        assert_eq!(override_.binary.as_ref().unwrap(), &home.join("bin").join("pi"));
        assert_eq!(override_.config_dir.as_ref().unwrap(), &home.join("pi-agent"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn harness_router_lists_and_rejects_non_pi_connect() {
        let _guard = ENV_LOCK.lock().unwrap();
        let home = tmpdir("router-list");
        std::env::set_var("ALEXANDRIA_HOME", &home);
        save_config(&test_config(home.clone())).unwrap();
        let app = harness_admin_router(test_state("router-list-state"), "alx-local".into());

        let (status, body) = router_json(app.clone(), Method::GET, "/admin/harnesses", None).await;
        assert_eq!(status, StatusCode::OK);
        let harnesses = body["harnesses"].as_array().unwrap();
        assert_eq!(harnesses.len(), 6);
        assert!(harnesses.iter().all(|h| h["daemon_reachable"] == true));
        assert!(harnesses.iter().all(|h| h.get("name").is_some()));
        assert!(harnesses.iter().all(|h| h.get("override").is_some()));

        let (status, body) = router_json(
            app,
            Method::POST,
            "/admin/harnesses/claude/connect",
            None,
        )
        .await;
        std::env::remove_var("ALEXANDRIA_HOME");
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body["error"].as_str().unwrap().contains("does not support connect"));
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn harness_router_put_override_persists_and_clears() {
        let _guard = ENV_LOCK.lock().unwrap();
        let home = tmpdir("router-override");
        let bin_dir = tmpdir("router-override-bin");
        let binary = fake_executable(&bin_dir, "claude", "echo claude 1.0.0");
        let config_dir = home.join("claude-config");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::env::set_var("ALEXANDRIA_HOME", &home);
        save_config(&test_config(home.clone())).unwrap();
        let app = harness_admin_router(test_state("router-override-state"), "alx-local".into());

        let (status, body) = router_json(
            app.clone(),
            Method::PUT,
            "/admin/harnesses/claude/override",
            Some(serde_json::json!({
                "binary": binary,
                "config_dir": config_dir,
            })),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["name"], "claude");
        assert_eq!(body["installed"], true);
        let (loaded, _) = load_or_create_config().unwrap();
        assert!(loaded.harness_overrides.contains_key("claude"));

        let (status, body) = router_json(
            app,
            Method::PUT,
            "/admin/harnesses/claude/override",
            Some(serde_json::json!({"binary": null, "config_dir": null})),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["override"]["binary"], serde_json::Value::Null);
        let (loaded, _) = load_or_create_config().unwrap();
        std::env::remove_var("ALEXANDRIA_HOME");
        assert!(!loaded.harness_overrides.contains_key("claude"));
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn harness_router_connect_pi_writes_models_json() {
        let _guard = ENV_LOCK.lock().unwrap();
        let home = tmpdir("router-connect");
        let bin_dir = tmpdir("router-connect-bin");
        let binary = fake_executable(&bin_dir, "pi", "echo pi 0.80.0");
        let config_dir = home.join("pi-agent");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::env::set_var("ALEXANDRIA_HOME", &home);
        let mut config = test_config(home.clone());
        config.harness_overrides.insert(
            "pi".into(),
            HarnessOverride {
                binary: Some(binary),
                config_dir: Some(config_dir.clone()),
            },
        );
        save_config(&config).unwrap();
        let state = test_state("router-connect-state");
        let app = harness_admin_router(state.clone(), "alx-local".into());

        let models_path = config_dir.join("models.json");
        assert!(!models_path.exists());

        let (status, body) = router_json(
            app.clone(),
            Method::POST,
            "/admin/harnesses/pi/connect?dry_run=true",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let plan = body["plan"].as_array().unwrap();
        assert!(!plan.is_empty());
        assert!(plan.iter().any(|s| s["detail"]
            .as_str()
            .unwrap_or("")
            .contains("add provider 'alexandria'")));
        assert!(plan
            .iter()
            .any(|s| s["detail"].as_str() == Some("mint harness key")));
        assert!(!models_path.exists());
        assert_eq!(state.store.list_run_keys(false).unwrap().len(), 0);

        let (status, body) =
            router_json(app.clone(), Method::POST, "/admin/harnesses/pi/connect", None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["key_id"].as_str().unwrap().starts_with("rk-"));
        assert_eq!(body["key"], "minted");
        assert!(body["models_total"].as_u64().unwrap() > 0);
        assert!(body["path"].as_str().unwrap().ends_with("models.json"));
        assert!(body["base_url"].as_str().is_some());
        assert!(body["added"].as_array().unwrap().len() > 0);
        assert_eq!(body["removed"].as_array().unwrap().len(), 0);
        let models: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&models_path).unwrap()).unwrap();
        assert!(models["providers"]["alexandria"].is_object());
        let written_ids: Vec<&str> = models["providers"]["alexandria"]["models"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|m| m["id"].as_str())
            .collect();
        assert!(written_ids.iter().all(|id| id.starts_with("alex/")));
        assert!(written_ids.iter().any(|id| *id == "alex/claude-fable-5" || id.ends_with("claude-opus-4-8")));
        let saved_key = models["providers"]["alexandria"]["apiKey"]
            .as_str()
            .unwrap()
            .to_string();
        let before_disconnect = std::fs::read_to_string(&models_path).unwrap();
        let keys = state.store.list_run_keys(false).unwrap();
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0]["kind"], "harness");
        assert_eq!(keys[0]["label"], "pi");

        let (status, body) = router_json(
            app.clone(),
            Method::POST,
            "/admin/harnesses/pi/disconnect?dry_run=true",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let dplan = body["plan"].as_array().unwrap();
        assert!(dplan.iter().any(|s| s["detail"].as_str() == Some("remove provider block")));
        assert!(dplan.iter().any(|s| s["detail"]
            .as_str()
            .unwrap_or("")
            .starts_with("revoke harness key")));
        assert_eq!(std::fs::read_to_string(&models_path).unwrap(), before_disconnect);
        assert_eq!(state.store.list_run_keys(false).unwrap().len(), 1);

        let (status, body) = router_json(
            app.clone(),
            Method::POST,
            "/admin/harnesses/pi/refresh-config",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["refreshed"], true);
        assert_eq!(body["key"], "reused");
        assert!(body["models_total"].as_u64().unwrap() > 0);
        assert_eq!(body["added"].as_array().unwrap().len(), 0);
        assert_eq!(body["removed"].as_array().unwrap().len(), 0);
        assert_eq!(
            body["unchanged"].as_u64().unwrap(),
            body["models_total"].as_u64().unwrap()
        );
        let refreshed: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&models_path).unwrap()).unwrap();
        assert_eq!(
            refreshed["providers"]["alexandria"]["apiKey"].as_str().unwrap(),
            saved_key
        );
        assert_eq!(state.store.list_run_keys(false).unwrap().len(), 1);

        let (status, body) = router_json(
            app.clone(),
            Method::POST,
            "/admin/harnesses/pi/disconnect",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["was_connected"], true);
        assert_eq!(body["revoked"], 1);
        assert!(body["path"].as_str().unwrap().ends_with("models.json"));
        assert!(body["removed"].as_array().unwrap().len() > 0);
        assert_eq!(body["key"], "revoked");
        assert_eq!(state.store.list_run_keys(false).unwrap().len(), 0);

        let (status, body) = router_json(
            app.clone(),
            Method::POST,
            "/admin/harnesses/nope/refresh-config",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(body["error"].as_str().unwrap().contains("unknown harness"));

        let (status, body) = router_json(
            app,
            Method::POST,
            "/admin/harnesses/claude/refresh-config",
            None,
        )
        .await;
        std::env::remove_var("ALEXANDRIA_HOME");
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body["error"].as_str().unwrap().contains("does not support connect"));
    }

    #[test]
    fn service_state_labels() {
        assert_eq!(
            service_state_label(&ServiceState::LaunchdLoaded { pid: Some(7) }),
            "launchd: installed + loaded (pid 7)"
        );
        assert_eq!(
            service_state_label(&ServiceState::LaunchdLoaded { pid: None }),
            "launchd: installed + loaded"
        );
        assert!(service_state_label(&ServiceState::LaunchdNotLoaded)
            .contains("alexandria service install"));
        assert!(service_state_label(&ServiceState::LaunchdNotInstalled)
            .contains("alexandria service install"));
        assert_eq!(
            service_state_label(&ServiceState::Systemd {
                enabled: true,
                active: true,
                unit_present: true
            }),
            "systemd: enabled + active"
        );
        assert!(service_state_label(&ServiceState::Systemd {
            enabled: true,
            active: false,
            unit_present: true
        })
        .contains("systemctl --user start"));
        assert!(service_state_label(&ServiceState::Systemd {
            enabled: false,
            active: true,
            unit_present: true
        })
        .contains("systemctl --user enable"));
        assert!(service_state_label(&ServiceState::Systemd {
            enabled: false,
            active: false,
            unit_present: true
        })
        .contains("enable --now"));
        assert!(service_state_label(&ServiceState::Systemd {
            enabled: false,
            active: false,
            unit_present: false
        })
        .contains("not installed"));
        assert!(service_state_label(&ServiceState::SystemdMissing).contains("not found"));
        assert!(service_state_label(&ServiceState::Unsupported).contains("unsupported OS"));
    }

    #[test]
    fn service_managed_matrix() {
        assert!(service_managed(&ServiceState::LaunchdLoaded { pid: None }));
        assert!(service_managed(&ServiceState::Systemd {
            enabled: false,
            active: true,
            unit_present: true
        }));
        assert!(!service_managed(&ServiceState::LaunchdNotLoaded));
        assert!(!service_managed(&ServiceState::LaunchdNotInstalled));
        assert!(!service_managed(&ServiceState::Systemd {
            enabled: true,
            active: false,
            unit_present: true
        }));
        assert!(!service_managed(&ServiceState::SystemdMissing));
        assert!(!service_managed(&ServiceState::Unsupported));
    }

    #[test]
    fn dario_update_notice_parsing() {
        let raw = r#"{"checked_at_ms":1,"latest":"4.8.140","active_version":"4.8.139","pinned":null,"update_available":true}"#;
        assert_eq!(
            parse_dario_update_notice(raw),
            Some("dario 4.8.140 available (running 4.8.139) — alexandria dario update".into())
        );
        assert_eq!(
            parse_dario_update_notice(
                r#"{"latest":"4.8.140","active_version":"4.8.140","update_available":false}"#
            ),
            None
        );
        assert_eq!(parse_dario_update_notice("not json"), None);
        assert_eq!(
            parse_dario_update_notice(r#"{"update_available":true}"#),
            None
        );
        assert_eq!(
            parse_dario_update_notice(r#"{"latest":"5","update_available":true}"#),
            Some("dario 5 available (running unknown) — alexandria dario update".into())
        );
    }
}
