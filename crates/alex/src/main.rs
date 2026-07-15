use std::collections::BTreeMap;
use std::ffi::OsString;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use alex_auth::{import_all, named_account_id, now_ms, AccountPolicy, Vault};
use alex_store::{KnownAccount, Store};
use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use rand::Rng;
use serde::{Deserialize, Serialize};

mod dario;
mod harness_connect;
mod harness_e2e;
mod light;
mod reset;
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
    /// Connect an installed AI harness to this daemon
    Connect {
        /// Harness name; omit to show detection status
        harness: Option<String>,
        /// Override the harness config dir
        #[arg(long)]
        config_dir: Option<PathBuf>,
        /// Alexandria daemon base URL (env: ALEXANDRIA_URL)
        #[arg(long)]
        url: Option<String>,
        /// Pre-minted harness key (env: ALEXANDRIA_HARNESS_KEY)
        #[arg(long)]
        key: Option<String>,
        /// Cosmetic ID for a pre-minted key
        #[arg(long)]
        key_id: Option<String>,
        /// Install tool-execution hooks in this connection
        #[arg(long)]
        tool_capture: bool,
        #[arg(long)]
        json: bool,
    },
    /// Show or change harness tool-capture status
    ToolCapture {
        harness: String,
        /// Set capture on or off; omit to show the current status
        state: Option<ToolCaptureState>,
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
    /// Install, configure, inspect, or control the Dario generational proxy
    Dario {
        #[command(subcommand)]
        command: DarioCommand,
    },
    /// Show subscription plans, limit-window utilization, and reset times
    Limits {
        #[arg(long)]
        json: bool,
    },
    /// Read or update persistent daemon settings
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Read or set provider routing reserve percentages
    Routing {
        #[command(subcommand)]
        command: RoutingCommand,
    },
    /// Manage the OS user service (launchd on macOS, systemd on Linux)
    Service {
        #[command(subcommand)]
        command: ServiceCommand,
    },
    /// Launch connected Claude/Codex or wrap Amp/Cursor Agent for capture
    Wrap {
        #[command(subcommand)]
        command: WrapCommand,
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
        /// Use this release channel for this run only (stable or beta)
        #[arg(long)]
        channel: Option<String>,
        /// Persist the release channel in config.toml, then use it (stable or beta)
        #[arg(long, value_name = "CHANNEL")]
        set_channel: Option<String>,
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
    /// Selectively remove local Alexandria data (dry-run unless --yes is supplied)
    Reset {
        /// Remove vault account JSON and revoke run keys; retain account tombstones
        #[arg(long)]
        credentials: bool,
        /// Restore config.toml defaults while preserving update_channel
        #[arg(long)]
        settings: bool,
        /// Remove trace rows, heartbeats, and captured request/response bodies
        #[arg(long)]
        traces: bool,
        /// Disconnect every connected harness using the normal disconnect path
        #[arg(long)]
        harnesses: bool,
        /// Remove derived pricing and other cached data
        #[arg(long)]
        cache: bool,
        /// Select every reset category
        #[arg(long)]
        all: bool,
        /// Apply the selected deletions. Without this flag, print the plan only.
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// Live dashboard: traces, limits, accounts, dario generations
    Tui,
}

#[derive(Clone, Copy, ValueEnum)]
enum ToolCaptureState {
    On,
    Off,
}

impl ToolCaptureState {
    fn enabled(self) -> bool {
        matches!(self, Self::On)
    }
}

#[derive(Subcommand)]
enum KeysCommand {
    /// Mint a run key bound to run metadata; the key is printed exactly once
    Mint {
        /// Credential capability: run, harness, or wrap
        #[arg(long, default_value = "run")]
        kind: String,
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
        #[arg(long)]
        json: bool,
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
    /// Install the OS user service pointing at this binary
    Install,
    /// Persist the daemon network exposure (loopback, all, or an interface IP)
    Bind {
        /// loopback (127.0.0.1), all (0.0.0.0), or a detected interface address
        target: String,
    },
    /// Safely replace the loaded launchd service when it has no routed requests in flight
    Restart {
        /// Replace even when routed requests are in flight (those requests will be interrupted)
        #[arg(long)]
        force: bool,
    },
    /// Stop + remove the OS user service
    Uninstall,
    /// Show service state
    Status,
}

#[derive(Subcommand)]
enum ConfigCommand {
    /// Set the daemon bind IP (`127.0.0.1` for local only, `0.0.0.0` for LAN + local)
    Host { address: String },
}

#[derive(Subcommand, Clone, Copy)]
enum DarioCommand {
    /// Install Dario using npm, pnpm, or Bun (Node.js 18+ is required at runtime)
    Bootstrap {
        #[arg(long)]
        json: bool,
    },
    /// Route non-Claude-Code Anthropic requests through Dario
    Enable,
    /// Keep Dario ready but route Anthropic requests directly
    Disable,
    /// Show generations and their states
    Status,
    /// Roll to a fresh generation of the same version
    Restart,
    /// Check npm for a newer version and roll if found
    Update,
}

#[derive(Subcommand)]
enum WrapCommand {
    /// List configured wrap harnesses (from embedded catalog / ~/.alexandria/wrap-harnesses.json)
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Print shell exports + write settings for a harness mode (config-driven)
    Env {
        /// Harness id (e.g. amp)
        #[arg(default_value = "amp")]
        harness: String,
        /// Mode from catalog (default: preferred / default_mode)
        #[arg(long)]
        mode: Option<String>,
        /// Base URL of the local wrap (reverse or HTTP proxy)
        #[arg(long, default_value = "http://127.0.0.1:4101")]
        wrap_url: String,
        /// Optional TLS CA PEM path (env_proxy mode)
        #[arg(long)]
        ca_cert: Option<PathBuf>,
        /// Machine-readable plan (no shell exports)
        #[arg(long)]
        json: bool,
    },
    /// Launch connected Claude with its Alexandria settings: `alex wrap claude -p 'hi'`
    Claude {
        /// Args passed through verbatim to `claude`
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Launch connected Codex with the Alexandria profile: `alex wrap codex exec 'hi'`
    Codex {
        /// Args passed through verbatim to `codex`
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Start reverse wrap + run Amp: `alex wrap amp` or `alex wrap amp -- -x 'hi'`
    Amp {
        #[command(flatten)]
        remote_trace: RemoteTraceArgs,
        /// Mode from catalog (default: base_url)
        #[arg(long)]
        mode: Option<String>,
        /// Bind address for reverse wrap (`127.0.0.1:0` = ephemeral port)
        #[arg(long, default_value = "127.0.0.1:0")]
        bind: String,
        /// Upstream to reverse to (default: catalog amp upstream)
        #[arg(long)]
        upstream: Option<String>,
        /// Only run the reverse wrap until Ctrl-C (do not spawn amp)
        #[arg(long)]
        serve_only: bool,
        /// Less stderr chatter
        #[arg(long, short = 'q')]
        quiet: bool,
        /// Args passed through to `amp` (use `--` before flags like `-x`)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Start reverse wrap + run Cursor Agent: `alex wrap agent -- --print --trust 'hi'`
    Agent {
        #[command(flatten)]
        remote_trace: RemoteTraceArgs,
        /// Mode from catalog (default: base_url)
        #[arg(long)]
        mode: Option<String>,
        /// Bind address for reverse wrap (`127.0.0.1:0` = ephemeral port)
        #[arg(long, default_value = "127.0.0.1:0")]
        bind: String,
        /// Upstream to reverse to (default: catalog agent upstream)
        #[arg(long)]
        upstream: Option<String>,
        /// Only run the reverse wrap until Ctrl-C (do not spawn agent)
        #[arg(long)]
        serve_only: bool,
        /// Less stderr chatter
        #[arg(long, short = 'q')]
        quiet: bool,
        /// Args passed through to `agent` (use `--` before flags like `--print`)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Start reverse wrap + run any catalog harness: `alex wrap run amp -- -x hi`
    Run {
        /// Harness id from catalog
        harness: String,
        #[command(flatten)]
        remote_trace: RemoteTraceArgs,
        #[arg(long)]
        mode: Option<String>,
        #[arg(long, default_value = "127.0.0.1:0")]
        bind: String,
        #[arg(long)]
        upstream: Option<String>,
        #[arg(long)]
        serve_only: bool,
        #[arg(long, short = 'q')]
        quiet: bool,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Run reverse-wrap smoke (mock upstream + catalog capture policy)
    Smoke {
        #[arg(long)]
        json: bool,
        /// Harness whose capture policy to apply (default: amp)
        #[arg(long, default_value = "amp")]
        harness: String,
    },
}

#[derive(clap::Args, Clone, Default)]
struct RemoteTraceArgs {
    /// Central Alexandria base URL for trace upload (env: ALEXANDRIA_TRACE_URL)
    #[arg(long, alias = "alex-url")]
    trace_url: Option<String>,
    /// File containing a wrap key (env alternatives: ALEXANDRIA_TRACE_KEY[_FILE])
    #[arg(long)]
    trace_key_file: Option<PathBuf>,
    /// Permit plaintext HTTP to a non-loopback trace destination
    #[arg(long)]
    allow_insecure_http: bool,
}

#[derive(Subcommand)]
enum AuthCommand {
    /// Import credentials from native tool locations (claude|codex|gemini|grok|amp|all)
    Import {
        #[arg(default_value = "all")]
        source: String,
        #[arg(long, default_value = "default")]
        name: String,
        #[arg(long)]
        force: bool,
    },
    /// Run an OAuth login flow from the terminal (claude|codex|grok|gemini|amp); no arg opens a picker
    Login {
        provider: Option<String>,
        #[arg(long, default_value = "default")]
        name: String,
        #[arg(long)]
        force: bool,
    },
    /// Pause an account so selection skips it
    Pause { provider: String, name: String },
    /// Resume a paused account
    Resume { provider: String, name: String },
    /// Register a Google AI Studio API key for Gemini (from aistudio.google.com/apikey)
    GeminiKey {
        /// The API key; omit to read from the GEMINI_API_KEY env var
        key: Option<String>,
    },
    /// Register an Amp access token (from ampcode.com/settings or AMP_API_KEY)
    AmpKey {
        /// The Amp API key; omit to read from the AMP_API_KEY env var
        key: Option<String>,
    },
    /// Register or remove an OpenRouter API key (OPENROUTER_API_KEY)
    OpenrouterKey {
        /// The API key; omit to read from the OPENROUTER_API_KEY env var
        key: Option<String>,
        /// Optional OpenRouter HTTP-Referer attribution, stored in the vault
        #[arg(long)]
        referer: Option<String>,
        /// Optional OpenRouter X-Title attribution, stored in the vault
        #[arg(long)]
        title: Option<String>,
        /// Remove the stored OpenRouter API key
        #[arg(long)]
        remove: bool,
    },
    /// List vault accounts
    List,
}

#[derive(Subcommand)]
enum RoutingCommand {
    /// Show a provider's effective reserve policy and per-account overrides
    Get {
        /// Provider: claude|codex|grok|gemini|amp (aliases accepted)
        provider: String,
        #[arg(long)]
        json: bool,
    },
    /// Set a provider-wide reserve or one account's override
    Set {
        /// Provider: claude|codex|grok|gemini|amp (aliases accepted)
        provider: String,
        /// Reserve percentage, from 0 (never block) through 100
        #[arg(long)]
        reserve_pct: u8,
        /// Account name or id; omit to set the provider-wide reserve
        #[arg(long)]
        account: Option<String>,
    },
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
        /// Read a scoped run or harness key from this file instead of giving
        /// the container the daemon's local admin key.
        #[arg(long)]
        run_key_file: Option<PathBuf>,
        /// Expected run id for a scoped run key (reported in the JSON summary).
        #[arg(long)]
        run_id: Option<String>,
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
    /// List legacy orphan groups, or attach one group after explicit confirmation (offline)
    Reattach {
        /// The old, now-unresolvable account id shown by the listing
        #[arg(long)]
        orphan_account_id: Option<String>,
        /// Existing account id to adopt the orphaned history
        #[arg(long)]
        to_account_id: Option<String>,
        /// Apply the displayed plan. Without this flag this command is always a no-op.
        #[arg(long)]
        yes: bool,
        #[arg(long)]
        json: bool,
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
    /// Reconcile one wrapped Cursor Agent transcript from Cursor's local JSONL
    RepairAgent {
        #[arg(long)]
        transcript_id: String,
        /// Report changes without rewriting trace bodies
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        json: bool,
    },
    /// Re-import the latest wrapped Amp websocket capture (including error-only turns)
    RepairAmp {
        #[arg(long)]
        run_id: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Push a locally spooled wrap run to a central Alexandria daemon
    Push {
        #[arg(long)]
        run_id: String,
        #[command(flatten)]
        remote_trace: RemoteTraceArgs,
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
    /// Account id from `GET /traces/accounts`; includes removed-account history
    #[arg(long)]
    account_id: Option<String>,
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
            ("account_id", &self.account_id),
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
    #[serde(default = "default_ping_openrouter")]
    ping_openrouter_model: String,
    #[serde(default)]
    gemini_project: String,
    #[serde(default = "default_anthropic_upstream")]
    anthropic_upstream: String,
    #[serde(default)]
    dario_api_key: String,
    /// Explicit real Claude Code executable for Dario prompt capture.
    #[serde(default)]
    dario_claude_bin: Option<PathBuf>,
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
    #[serde(default = "default_update_channel")]
    update_channel: String,
    /// Maximum quiet period between upstream streaming chunks. This is not a
    /// request deadline; any received chunk resets the timer.
    #[serde(default = "default_upstream_stream_idle_timeout_seconds")]
    upstream_stream_idle_timeout_seconds: u64,
    #[serde(default)]
    harness_overrides: BTreeMap<String, HarnessOverride>,
    /// Tool capture is an explicit per-harness consent setting. It defaults
    /// off because command arguments and outputs can be sensitive.
    #[serde(default)]
    harness_tool_capture: BTreeMap<String, bool>,
    #[serde(default)]
    account_policy: BTreeMap<String, AccountPolicy>,
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

fn default_update_channel() -> String {
    "stable".into()
}

fn default_upstream_stream_idle_timeout_seconds() -> u64 {
    // Long reasoning stretches are normal. Fifteen minutes only catches a
    // connection that has actually stopped producing output.
    15 * 60
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

fn default_ping_openrouter() -> String {
    // A health check should be near-free. This is one of OpenRouter's `:free`
    // models (verified live 2026-07-14: returns 200) rather than the previous
    // paid anthropic/claude-3.5-sonnet, which billed real credits on every ping.
    // Bare id: the ping path has already resolved the provider to OpenRouter.
    "google/gemma-4-26b-a4b-it:free".into()
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

/// Resolve Claude before entering the service-manager environment inherited by
/// the Dario child. User-local installs are intentionally checked even when a
/// systemd/launchd PATH omits them.
fn resolve_dario_claude_bin(override_bin: Option<&Path>) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = override_bin {
        // An explicit override is authoritative: falling through to another
        // binary conceals a typo and makes the configured deployment
        // irreproducible. `None` is surfaced as a prompt-cache health issue.
        return path.is_file().then(|| path.to_path_buf());
    }
    if let Some(path) = std::env::var_os("ALEXANDRIA_REAL_CLAUDE_BIN") {
        candidates.push(PathBuf::from(path));
    }
    if let Some(path) = std::env::var_os("PATH") {
        candidates.extend(std::env::split_paths(&path).map(|dir| dir.join("claude")));
    }
    if let Some(home) = dirs::home_dir() {
        candidates.push(home.join(".local/bin/claude"));
    }
    candidates.extend([
        PathBuf::from("/opt/homebrew/bin/claude"),
        PathBuf::from("/usr/local/bin/claude"),
    ]);
    candidates.into_iter().find(|path| path.is_file())
}

impl Config {
    /// Repair values that a stale config on disk would otherwise keep forever,
    /// since serde defaults only fill MISSING keys. Changing a default does not
    /// help an existing user whose config already has the old value written.
    fn heal(&mut self) {
        // OpenRouter removed anthropic/claude-3.5-sonnet, so the old ping default
        // now 404s "No endpoints found" on every health check. Move to the free
        // model. Only rewrite the known-dead value -- never override a user's own
        // choice of a working model.
        if self.ping_openrouter_model == "anthropic/claude-3.5-sonnet" {
            self.ping_openrouter_model = default_ping_openrouter();
        }
    }

    fn defaults_for(data_dir: PathBuf) -> Self {
        Self {
            host: "127.0.0.1".into(),
            port: 4100,
            data_dir,
            local_key: random_key("alx"),
            ping_gemini_model: default_ping_gemini(),
            ping_openrouter_model: default_ping_openrouter(),
            gemini_project: String::new(),
            heartbeat_minutes: default_heartbeat_minutes(),
            ping_anthropic_model: default_ping_anthropic(),
            ping_openai_model: default_ping_openai(),
            ping_xai_model: default_ping_xai(),
            anthropic_upstream: default_anthropic_upstream(),
            dario_api_key: String::new(),
            dario_claude_bin: None,
            dario_update_check_minutes: default_dario_update_minutes(),
            dario_version: None,
            dario_probe_seconds: default_dario_probe_seconds(),
            dario_probe_failures: default_dario_probe_failures(),
            dario_probe_model: default_dario_probe_model(),
            trace_body_retention_days: default_trace_body_retention_days(),
            trace_row_retention_days: 0,
            update_check_hours: default_update_check_hours(),
            update_channel: default_update_channel(),
            upstream_stream_idle_timeout_seconds: default_upstream_stream_idle_timeout_seconds(),
            harness_overrides: BTreeMap::new(),
            harness_tool_capture: BTreeMap::new(),
            account_policy: BTreeMap::new(),
        }
    }

    fn ping_models(&self) -> alex_proxy::PingModels {
        alex_proxy::PingModels {
            anthropic: self.ping_anthropic_model.clone(),
            openai: self.ping_openai_model.clone(),
            xai: self.ping_xai_model.clone(),
            gemini: self.ping_gemini_model.clone(),
            openrouter: self.ping_openrouter_model.clone(),
        }
    }

    fn dario_enabled(&self) -> bool {
        self.anthropic_upstream == "dario"
    }

    /// The URL a *client* should connect to.
    ///
    /// `host` is a BIND address: `0.0.0.0` (or `::`) means "listen on every
    /// interface". It is not a connect address. Handing it to a harness produced
    /// `http://0.0.0.0:4100/v1`, which macOS usually tolerates by routing to
    /// loopback -- and sometimes does not, giving intermittent
    /// "stream disconnected before completion" errors. Normalise it here, at the
    /// single source, rather than patching it per call site (which is how the hook
    /// URL got fixed while the provider URL the harness actually calls did not).
    /// The URL a *local* client (a harness on this machine) should connect to.
    ///
    /// `host` is a BIND address and must never be handed to a client as-is. It is
    /// not just `0.0.0.0` ("listen everywhere", which macOS sometimes refuses to
    /// route and which produced intermittent "stream disconnected" errors): binding
    /// to a specific non-loopback address -- a LAN or Tailscale IP, so another
    /// machine can reach the daemon -- would otherwise tell the LOCAL harnesses to
    /// talk to that address too. Local traffic would then leave the loopback
    /// interface, and would break outright whenever that network is down.
    ///
    /// A daemon bound to any non-loopback address is also listening on loopback
    /// (0.0.0.0/:: bind all; a specific IP still leaves 127.0.0.1 serving), so local
    /// clients can always use loopback. Remote clients are given an explicit address
    /// by whoever configures them -- never by this function.
    fn base_url(&self) -> String {
        daemon_connect_base_url(&self.host, self.port)
    }

    fn update_channel(&self) -> selfupdate::UpdateChannel {
        selfupdate::UpdateChannel::parse(&self.update_channel).unwrap_or_else(|e| {
            eprintln!("warning: {e}; using stable");
            selfupdate::UpdateChannel::Stable
        })
    }

    fn upstream_stream_idle_timeout(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.upstream_stream_idle_timeout_seconds.max(1))
    }
}

/// The intentionally small set of network-exposure choices presented by the
/// CLI and app. `host` remains the persisted config field for compatibility.
#[derive(Debug, Clone, PartialEq, Eq)]
enum BindTarget {
    Loopback,
    All,
    Interface(IpAddr),
}

impl BindTarget {
    fn parse(value: &str) -> Result<Self> {
        let value = value.trim();
        match value.to_ascii_lowercase().as_str() {
            "loopback" | "127.0.0.1" => Ok(Self::Loopback),
            "all" | "0.0.0.0" => Ok(Self::All),
            _ => {
                let ip = value
                    .trim_matches(['[', ']'])
                    .parse::<IpAddr>()
                    .with_context(|| {
                        format!(
                            "invalid bind target {value:?}; use loopback, all, or an interface IP address"
                        )
                    })?;
                if ip.is_loopback() {
                    Ok(Self::Loopback)
                } else if ip.is_unspecified() {
                    Ok(Self::All)
                } else {
                    Ok(Self::Interface(ip))
                }
            }
        }
    }

    fn host(&self) -> String {
        match self {
            Self::Loopback => "127.0.0.1".into(),
            Self::All => "0.0.0.0".into(),
            Self::Interface(ip) => ip.to_string(),
        }
    }

    fn description(&self) -> String {
        match self {
            Self::Loopback => "loopback only (127.0.0.1)".into(),
            Self::All => "all interfaces (0.0.0.0)".into(),
            Self::Interface(ip) => format!("specific interface ({ip})"),
        }
    }
}

fn is_loopback_bind_host(host: &str) -> bool {
    host.trim_matches(['[', ']'])
        .parse::<IpAddr>()
        .map(|ip| ip.is_loopback())
        .unwrap_or(matches!(host, "localhost"))
}

/// Binding a concrete LAN/VPN address does not also bind 127.0.0.1. Keep the
/// local listener explicit so harness config can always remain loopback-only.
fn requires_explicit_loopback_listener(host: &str) -> bool {
    host.trim_matches(['[', ']'])
        .parse::<IpAddr>()
        .map(|ip| !ip.is_loopback() && !ip.is_unspecified())
        .unwrap_or(false)
}

/// The host a *local* client (a harness on this machine) should connect to.
///
/// `bind_host` is a BIND address and must never reach a client as-is. Handling only
/// the wildcards is not enough: binding to a SPECIFIC non-loopback address -- a LAN
/// or Tailscale IP, so another machine can reach the daemon -- would otherwise write
/// that address into the LOCAL harness configs too. Local traffic would then leave
/// the loopback interface and break outright whenever that network was down or the
/// DHCP lease changed.
///
/// A daemon bound to any non-loopback address still serves loopback, so local clients
/// can always use it. Remote clients are configured explicitly by whoever configures
/// them -- never derived from the bind address.
fn daemon_connect_host(bind_host: &str) -> &str {
    match bind_host {
        "localhost" | "127.0.0.1" | "::1" | "[::1]" => bind_host,
        _ => "127.0.0.1",
    }
}

fn daemon_connect_base_url(bind_host: &str, port: u16) -> String {
    let host = daemon_connect_host(bind_host);
    if host.contains(':') {
        format!("http://[{host}]:{port}")
    } else {
        format!("http://{host}:{port}")
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
    save_config_at(config, &path)
}

fn save_config_at(config: &Config, path: &Path) -> Result<()> {
    std::fs::write(&path, toml::to_string_pretty(config)?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn set_daemon_host(config: &mut Config, address: &str) -> Result<bool> {
    let address = address.trim();
    address
        .parse::<std::net::IpAddr>()
        .with_context(|| format!("daemon host must be an IPv4 or IPv6 bind address: {address}"))?;
    if config.host == address {
        return Ok(false);
    }
    config.host = address.to_string();
    save_config(config)?;
    Ok(true)
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
        let mut config: Config =
            toml::from_str(&raw).with_context(|| format!("parsing {path:?}"))?;
        config.heal();
        let upgraded = toml::to_string_pretty(&config)?;
        if upgraded != raw {
            std::fs::write(&path, upgraded)?;
        }
        return Ok((config, false));
    }
    let config = Config::defaults_for(home.clone());
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
    let vault = Vault::open(config.data_dir.join("accounts"))?;
    let mut policies = Vec::new();
    for (k, v) in &config.account_policy {
        let p = match k.as_str() {
            "claude" | "anthropic" => alex_core::Provider::Anthropic,
            "codex" | "openai" | "chatgpt" => alex_core::Provider::Openai,
            "grok" | "xai" => alex_core::Provider::Xai,
            "gemini" | "google" => alex_core::Provider::Gemini,
            "amp" | "ampcode" => alex_core::Provider::Amp,
            "openrouter" | "or" => alex_core::Provider::Openrouter,
            _ => continue,
        };
        policies.push((p, v.clone()));
    }
    if !policies.is_empty() {
        vault.set_policies_blocking(policies);
    }
    Ok(vault)
}

fn validate_account_name(name: &str) -> Result<()> {
    if name.len() > 32
        || name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
    {
        anyhow::bail!("account name must match [a-z0-9_-]{{1,32}}");
    }
    Ok(())
}

fn provider_from_cli(s: &str) -> Result<alex_core::Provider> {
    Ok(match s {
        "claude" | "anthropic" => alex_core::Provider::Anthropic,
        "codex" | "openai" | "chatgpt" => alex_core::Provider::Openai,
        "grok" | "xai" => alex_core::Provider::Xai,
        "gemini" | "google" => alex_core::Provider::Gemini,
        "amp" | "ampcode" => alex_core::Provider::Amp,
        "openrouter" | "or" => alex_core::Provider::Openrouter,
        other => anyhow::bail!("unknown provider '{other}'"),
    })
}

struct DarioGlue {
    supervisor: Arc<dario::DarioSupervisor>,
    route_enabled: bool,
}

impl alex_proxy::DarioRouter for DarioGlue {
    fn routes_requests(&self) -> bool {
        self.route_enabled
    }

    fn active(&self) -> Option<alex_proxy::DarioActive> {
        self.supervisor.active().map(|a| alex_proxy::DarioActive {
            generation_id: a.generation_id,
            base_url: a.base_url,
            api_key: a.api_key,
        })
    }

    fn ensure_active(&self) -> alex_proxy::DarioEnsureFuture {
        let supervisor = self.supervisor.clone();
        Box::pin(async move {
            supervisor
                .ensure_active()
                .await
                .map(|a| alex_proxy::DarioActive {
                    generation_id: a.generation_id,
                    base_url: a.base_url,
                    api_key: a.api_key,
                })
        })
    }

    fn begin(&self, generation_id: &str) -> Option<Box<dyn std::any::Any + Send>> {
        self.supervisor
            .begin_request(generation_id)
            .map(|g| Box::new(g) as Box<dyn std::any::Any + Send>)
    }

    fn prepare_model(&self, model: &str) -> alex_proxy::DarioPrepareFuture {
        let supervisor = self.supervisor.clone();
        let model = model.to_string();
        Box::pin(async move {
            match supervisor.prepare_model(&model).await {
                Some(reason) => alex_proxy::DarioPrepare::DirectFallback { reason },
                None => alex_proxy::DarioPrepare::ServeThroughDario,
            }
        })
    }

    fn probe(&self, model: &str) -> alex_proxy::DarioProbeFuture {
        let supervisor = self.supervisor.clone();
        let model = model.to_string();
        Box::pin(async move { supervisor.through_dario_probe(&model).await })
    }

    fn status(&self) -> serde_json::Value {
        let mut status = self.supervisor.status();
        status["route_enabled"] = serde_json::json!(self.route_enabled);
        status
    }

    fn suspect(&self, generation_id: &str) {
        self.supervisor.suspect(generation_id);
    }
}

struct DarioUnavailable {
    error: String,
    route_enabled: bool,
}

impl alex_proxy::DarioRouter for DarioUnavailable {
    fn routes_requests(&self) -> bool {
        self.route_enabled
    }

    fn active(&self) -> Option<alex_proxy::DarioActive> {
        None
    }

    fn ensure_active(&self) -> alex_proxy::DarioEnsureFuture {
        let error = self.error.clone();
        Box::pin(async move { Err(error) })
    }

    fn begin(&self, _generation_id: &str) -> Option<Box<dyn std::any::Any + Send>> {
        None
    }

    fn prepare_model(&self, _model: &str) -> alex_proxy::DarioPrepareFuture {
        let error = self.error.clone();
        Box::pin(async move { alex_proxy::DarioPrepare::Unavailable { reason: error } })
    }

    fn probe(&self, _model: &str) -> alex_proxy::DarioProbeFuture {
        let error = self.error.clone();
        Box::pin(async move { Err(error) })
    }

    fn status(&self) -> serde_json::Value {
        serde_json::json!({
            "configured": true,
            "available": false,
            "route_enabled": self.route_enabled,
            "active_generation_id": null,
            "generations": [],
            "error": self.error,
        })
    }

    fn suspect(&self, _generation_id: &str) {}
}

fn dario_admin_router(
    sup: Arc<dario::DarioSupervisor>,
    local_key: Arc<std::sync::RwLock<String>>,
) -> axum::Router {
    use axum::extract::{Path as AxPath, Query, State};
    use axum::response::IntoResponse;
    use axum::routing::{get, post};

    async fn require_local_key(
        State(key): State<Arc<std::sync::RwLock<String>>>,
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
        if presented.as_deref() != key.read().ok().as_deref().map(String::as_str) {
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

fn harness_admin_router(state: Arc<alex_proxy::AppState>) -> axum::Router {
    use axum::extract::{Path as AxPath, Query, State};
    use axum::response::IntoResponse;
    use axum::routing::{get, post, put};

    #[derive(Deserialize)]
    struct HarnessOverrideBody {
        binary: Option<PathBuf>,
        config_dir: Option<PathBuf>,
    }

    #[derive(Deserialize)]
    struct CodexDefaultRouteBody {
        route: String,
    }

    #[derive(Deserialize)]
    struct ToolCaptureBody {
        enabled: bool,
    }

    fn parse_dry_run(q: &std::collections::HashMap<String, String>) -> bool {
        q.get("dry_run")
            .map(|s| matches!(s.as_str(), "1" | "true" | "yes"))
            .unwrap_or(false)
    }

    fn list_active_harness_keys(
        state: &alex_proxy::AppState,
        harness: &str,
    ) -> Result<Vec<(String, String)>> {
        let rows = state.store.list_run_keys(true)?;
        Ok(rows
            .iter()
            .filter(|row| row["kind"].as_str() == Some("harness"))
            .filter(|row| row["label"].as_str() == Some(harness))
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
        State(state): State<Arc<alex_proxy::AppState>>,
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
        if presented.as_deref() != state.local_key.read().ok().as_deref().map(String::as_str) {
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

    fn error(
        status: axum::http::StatusCode,
        message: impl Into<String>,
    ) -> axum::response::Response {
        (
            status,
            axum::Json(serde_json::json!({"error": message.into()})),
        )
            .into_response()
    }

    fn key_hash_hex(key: &str) -> String {
        use sha2::{Digest, Sha256};
        let digest = Sha256::digest(key.as_bytes());
        digest.iter().map(|b| format!("{b:02x}")).collect()
    }

    fn revoke_harness_keys(state: &alex_proxy::AppState, harness: &str) -> Result<usize> {
        let rows = state.store.list_run_keys(true)?;
        let ids: Vec<String> = rows
            .iter()
            .filter(|row| row["kind"].as_str() == Some("harness"))
            .filter(|row| row["label"].as_str() == Some(harness))
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

    fn mint_harness_key(state: &alex_proxy::AppState, harness: &str) -> Result<(String, String)> {
        let key = alex_proxy::generate_run_key();
        let key_hash = key_hash_hex(&key);
        let id = format!("rk-{}", &key_hash[..8]);
        let tags_json = serde_json::json!({"harness": harness}).to_string();
        state.store.insert_run_key(
            &id,
            &key_hash,
            "harness",
            None,
            Some(&tags_json),
            Some(harness),
            now_ms(),
            None,
        )?;
        Ok((id, key))
    }

    fn backfill_harness_lineage(
        state: &alex_proxy::AppState,
        harness: &str,
        config_dir: &std::path::Path,
    ) {
        let received_ms = now_ms();
        let events = match harness {
            "claude" => harness_connect::read_claude_hook_events(config_dir),
            "codex" => harness_connect::read_codex_hook_events(config_dir),
            "grok" => harness_connect::read_grok_hook_events(config_dir),
            "amp" => harness_connect::read_amp_hook_events(config_dir),
            _ => Vec::new(),
        };
        for (index, event) in events.iter().enumerate() {
            if let Err(error) = state.store.record_harness_event(
                harness,
                event,
                received_ms.saturating_add(index as i64),
            ) {
                tracing::warn!(%error, %harness, "could not backfill a harness lineage hook event");
            }
        }
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
                "gpt-5.6-sol".into(),
                "gpt-5.6-terra".into(),
                "gpt-5.6-luna".into(),
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
                Ok(harnesses) => {
                    axum::Json(serde_json::json!({"harnesses": harnesses})).into_response()
                }
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
        let Some(spec) = harness_connect::spec_by_name(&name) else {
            return error(
                axum::http::StatusCode::NOT_FOUND,
                format!("unknown harness '{name}'"),
            );
        };
        if !spec.supports_connect {
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
        let status = match harness_connect::harness_status(&config, spec, None, true).await {
            Ok(status) => status,
            Err(e) => return error(axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        };
        if !status.installed {
            return error(
                axum::http::StatusCode::BAD_REQUEST,
                format!("{name} is not installed"),
            );
        }
        let config_dir = harness_connect::resolve_config_dir(&config, spec, None);
        if !config_dir.is_dir() {
            return error(
                axum::http::StatusCode::BAD_REQUEST,
                format!(
                    "{name} config dir does not exist at {}",
                    config_dir.display()
                ),
            );
        }
        let models = state_models(&state);
        let codex_catalog = if name == "codex" {
            let Some(binary) = status.binary.as_deref() else {
                return error(
                    axum::http::StatusCode::BAD_REQUEST,
                    "codex is not installed",
                );
            };
            match harness_connect::codex_model_catalog(std::path::Path::new(binary), &models) {
                Ok(catalog) => Some(catalog),
                Err(e) => {
                    return error(axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
                }
            }
        } else {
            None
        };
        if dry_run {
            let keys = match list_active_harness_keys(&state, &name) {
                Ok(v) => v,
                Err(e) => {
                    return error(axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
                }
            };
            let plan = if name == "codex" {
                let model_count = codex_catalog
                    .as_ref()
                    .and_then(|catalog| catalog["models"].as_array())
                    .map(Vec::len)
                    .unwrap_or(models.len());
                harness_connect::plan_codex_connect(&config_dir, model_count, &keys)
            } else if name == "claude" {
                harness_connect::plan_claude_connect(&config_dir, models.len(), &keys)
            } else if name == "grok" {
                harness_connect::plan_grok_connect(&config_dir, models.len(), &keys)
            } else if name == "amp" {
                harness_connect::plan_amp_connect(&config_dir, &keys)
            } else {
                harness_connect::plan_connect(&config_dir, models.len(), &keys)
            };
            return axum::Json(plan).into_response();
        }
        if let Err(e) = revoke_harness_keys(&state, &name) {
            return error(axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
        }
        let (key_id, key) = match mint_harness_key(&state, &name) {
            Ok(v) => v,
            Err(e) => return error(axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        };
        let lineage_config_dir = matches!(name.as_str(), "claude" | "codex" | "grok" | "amp")
            .then(|| config_dir.clone());
        let result = match name.as_str() {
            "pi" => harness_connect::write_pi_connection_with_capture(
                config_dir,
                state.base_url.clone(),
                key_id,
                key,
                models,
                status.version,
                config
                    .harness_tool_capture
                    .get("pi")
                    .copied()
                    .unwrap_or(false),
            ),
            "codex" => harness_connect::write_codex_connection_with_capture(
                config_dir,
                state.base_url.clone(),
                key_id,
                key,
                codex_catalog.expect("Codex catalog prepared"),
                status.version,
                config
                    .harness_tool_capture
                    .get("codex")
                    .copied()
                    .unwrap_or(false),
            ),
            "claude" => harness_connect::write_claude_connection_with_capture(
                config_dir,
                state.base_url.clone(),
                key_id,
                key,
                models,
                status.version,
                config
                    .harness_tool_capture
                    .get("claude")
                    .copied()
                    .unwrap_or(false),
            ),
            "grok" => harness_connect::write_grok_connection(
                config_dir,
                state.base_url.clone(),
                key_id,
                key,
                models,
                status.version,
            ),
            "amp" => harness_connect::write_amp_connection_with_capture(
                config_dir,
                state.base_url.clone(),
                key_id,
                key,
                status.version,
                config
                    .harness_tool_capture
                    .get("amp")
                    .copied()
                    .unwrap_or(false),
            ),
            _ => unreachable!("connect-capable harness must have a writer"),
        };
        match result {
            Ok(summary) => {
                if let Some(config_dir) = lineage_config_dir.as_deref() {
                    backfill_harness_lineage(&state, &name, config_dir);
                }
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
        let Some(spec) = harness_connect::spec_by_name(&name) else {
            return error(
                axum::http::StatusCode::NOT_FOUND,
                format!("unknown harness '{name}'"),
            );
        };
        if !spec.supports_connect {
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
        let config_dir = harness_connect::resolve_config_dir(&config, spec, None);
        if dry_run {
            let keys = match list_active_harness_keys(&state, &name) {
                Ok(v) => v,
                Err(e) => {
                    return error(axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
                }
            };
            let plan = if name == "codex" {
                harness_connect::plan_codex_disconnect(&config_dir, &keys)
            } else if name == "claude" {
                harness_connect::plan_claude_disconnect(&config_dir, &keys)
            } else if name == "grok" {
                harness_connect::plan_grok_disconnect(&config_dir, &keys)
            } else if name == "amp" {
                harness_connect::plan_amp_disconnect(&config_dir, &keys)
            } else {
                harness_connect::plan_disconnect(&config_dir, &keys)
            };
            return axum::Json(plan).into_response();
        }
        let config_path = match name.as_str() {
            "claude" => config_dir.join("alexandria-settings.json"),
            "codex" => config_dir.join("config.toml"),
            "grok" => config_dir.join("config.toml"),
            "amp" => config_dir.join("plugins").join("alexandria.ts"),
            _ => config_dir.join("models.json"),
        };
        let previous_models = match name.as_str() {
            "claude" => harness_connect::read_claude_model_ids(&config_dir),
            "codex" => harness_connect::read_codex_model_ids(&config_dir),
            "grok" => harness_connect::read_grok_model_ids(&config_dir),
            "amp" => Vec::new(),
            _ => harness_connect::read_pi_model_ids(&config_dir),
        };
        let disconnected = match name.as_str() {
            "claude" => harness_connect::disconnect_claude_config(&config_dir),
            "codex" => harness_connect::disconnect_codex_config(&config_dir),
            "grok" => harness_connect::disconnect_grok_config(&config_dir),
            "amp" => harness_connect::disconnect_amp_config(&config_dir),
            _ => harness_connect::disconnect_pi_config(&config_dir),
        };
        let was_connected = match disconnected {
            Ok(v) => v,
            Err(e) => return error(axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        };
        match revoke_harness_keys(&state, &name) {
            Ok(revoked) => axum::Json(harness_connect::disconnect_summary_json(
                &config_path,
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
        if !spec.supports_connect {
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
                format!(
                    "{name} config dir does not exist at {}",
                    config_dir.display()
                ),
            );
        }
        let existing_key = match name.as_str() {
            "claude" => harness_connect::read_claude_api_key(&config_dir),
            "codex" => harness_connect::read_codex_api_key(&config_dir),
            "grok" => harness_connect::read_grok_api_key(&config_dir),
            "amp" => harness_connect::read_amp_api_key(&config_dir),
            _ => harness_connect::read_pi_api_key(&config_dir),
        };
        let (key_status, key_id, api_key) = if let Some(key) = existing_key {
            ("reused", String::new(), key)
        } else {
            match mint_harness_key(&state, &name) {
                Ok((id, key)) => ("minted", id, key),
                Err(e) => {
                    return error(axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
                }
            }
        };
        let models = state_models(&state);
        let status = match harness_connect::harness_status(&config, spec, None, true).await {
            Ok(status) => status,
            Err(e) => return error(axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        };
        let lineage_config_dir = matches!(name.as_str(), "claude" | "codex" | "grok" | "amp")
            .then(|| config_dir.clone());
        let result = if name == "codex" {
            let Some(binary) = status.binary.as_deref() else {
                return error(
                    axum::http::StatusCode::BAD_REQUEST,
                    "codex is not installed",
                );
            };
            let catalog =
                match harness_connect::codex_model_catalog(std::path::Path::new(binary), &models) {
                    Ok(catalog) => catalog,
                    Err(e) => {
                        return error(axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
                    }
                };
            harness_connect::write_codex_connection_with_capture(
                config_dir,
                state.base_url.clone(),
                key_id,
                api_key,
                catalog,
                status.version,
                config
                    .harness_tool_capture
                    .get("codex")
                    .copied()
                    .unwrap_or(false),
            )
        } else if name == "claude" {
            harness_connect::write_claude_connection_with_capture(
                config_dir,
                state.base_url.clone(),
                key_id,
                api_key,
                models,
                status.version,
                config
                    .harness_tool_capture
                    .get("claude")
                    .copied()
                    .unwrap_or(false),
            )
        } else if name == "grok" {
            harness_connect::write_grok_connection(
                config_dir,
                state.base_url.clone(),
                key_id,
                api_key,
                models,
                status.version,
            )
        } else if name == "amp" {
            harness_connect::write_amp_connection_with_capture(
                config_dir,
                state.base_url.clone(),
                key_id,
                api_key,
                status.version,
                config
                    .harness_tool_capture
                    .get("amp")
                    .copied()
                    .unwrap_or(false),
            )
        } else {
            harness_connect::write_pi_connection_with_capture(
                config_dir,
                state.base_url.clone(),
                key_id,
                api_key,
                models,
                status.version,
                config
                    .harness_tool_capture
                    .get("pi")
                    .copied()
                    .unwrap_or(false),
            )
        };
        match result {
            Ok(summary) => {
                if let Some(config_dir) = lineage_config_dir.as_deref() {
                    backfill_harness_lineage(&state, &name, config_dir);
                }
                axum::Json(harness_connect::config_write_json(
                    &summary,
                    key_status,
                    Some(true),
                ))
                .into_response()
            }
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

    async fn put_codex_default_route(
        State(_state): State<Arc<alex_proxy::AppState>>,
        axum::Json(body): axum::Json<CodexDefaultRouteBody>,
    ) -> axum::response::Response {
        let Some(spec) = harness_connect::spec_by_name("codex") else {
            return error(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "Codex harness definition is unavailable",
            );
        };
        let (config, _) = match load_or_create_config() {
            Ok(value) => value,
            Err(error_value) => {
                return error(
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    error_value.to_string(),
                )
            }
        };
        let config_dir = harness_connect::resolve_config_dir(&config, spec, None);
        match harness_connect::set_codex_default_route(&config_dir, &body.route) {
            Ok(route) => axum::Json(serde_json::json!({
                "default_route": route,
                "restart_required": true,
            }))
            .into_response(),
            Err(error_value) => error(axum::http::StatusCode::BAD_REQUEST, error_value.to_string()),
        }
    }

    async fn put_tool_capture(
        State(state): State<Arc<alex_proxy::AppState>>,
        AxPath(name): AxPath<String>,
        axum::Json(body): axum::Json<ToolCaptureBody>,
    ) -> axum::response::Response {
        let Some(spec) = harness_connect::spec_by_name(&name) else {
            return error(axum::http::StatusCode::NOT_FOUND, "unknown harness");
        };
        let (mut config, _) = match load_or_create_config() {
            Ok(value) => value,
            Err(e) => return error(axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        };
        let config_dir = harness_connect::resolve_config_dir(&config, spec, None);
        let result = match name.as_str() {
            "pi" => {
                harness_connect::set_pi_tool_capture(&config_dir, &state.base_url, body.enabled)
            }
            "claude" => {
                harness_connect::set_claude_tool_capture(&config_dir, &state.base_url, body.enabled)
            }
            "codex" => {
                harness_connect::set_codex_tool_capture(&config_dir, &state.base_url, body.enabled)
            }
            "amp" => {
                harness_connect::set_amp_tool_capture(&config_dir, &state.base_url, body.enabled)
            }
            _ => {
                return error(
                    axum::http::StatusCode::BAD_REQUEST,
                    format!("tool capture is not yet supported for {name}"),
                )
            }
        };
        if let Err(e) = result {
            return error(axum::http::StatusCode::BAD_REQUEST, e.to_string());
        }
        config.harness_tool_capture.insert(name, body.enabled);
        if let Err(e) = save_config(&config) {
            return error(axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
        }
        axum::Json(serde_json::json!({"tool_capture_enabled": body.enabled})).into_response()
    }

    axum::Router::new()
        .route("/admin/harnesses", get(list))
        .route("/admin/harnesses/{name}/connect", post(connect))
        .route("/admin/harnesses/{name}/disconnect", post(disconnect))
        .route(
            "/admin/harnesses/{name}/refresh-config",
            post(refresh_config),
        )
        .route("/admin/harnesses/{name}/override", put(put_override))
        .route(
            "/admin/harnesses/{name}/tool-capture",
            put(put_tool_capture),
        )
        .route(
            "/admin/harnesses/codex/default-route",
            put(put_codex_default_route),
        )
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            require_local_key,
        ))
        .with_state(state)
}

fn parse_bind_socket_addr(host: &str, port: u16) -> Result<SocketAddr> {
    if let Ok(ip) = host.trim_matches(['[', ']']).parse::<IpAddr>() {
        return Ok(SocketAddr::new(ip, port));
    }
    format!("{host}:{port}")
        .parse()
        .with_context(|| format!("parsing bind address {host}:{port}"))
}

async fn bind_daemon_listener(host: &str, port: u16) -> Result<tokio::net::TcpListener> {
    let addr = parse_bind_socket_addr(host, port)?;
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
    socket
        .listen(1024)
        .context("listening for daemon connections")
}

/// Bind the configured listener, preserving a local daemon if a selected
/// DHCP/VPN interface disappeared since it was saved. The saved setting is not
/// changed: it remains visible for the user to correct, while this process is
/// safely reachable through loopback.
async fn bind_daemon_listener_with_fallback(
    host: &str,
    port: u16,
    allow_loopback_fallback: bool,
) -> Result<(tokio::net::TcpListener, String, Option<String>)> {
    match bind_daemon_listener(host, port).await {
        Ok(listener) => Ok((listener, host.to_string(), None)),
        Err(error) if allow_loopback_fallback => {
            let listener = bind_daemon_listener("127.0.0.1", port)
                .await
                .context("configured bind failed and loopback fallback also failed")?;
            Ok((listener, "127.0.0.1".into(), Some(format!("{error:#}"))))
        }
        Err(error) => Err(error),
    }
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

    // Cove runs this command in a container with only a scoped harness key.
    // Handle the fully remote form before loading config so it neither needs
    // nor creates ~/.alexandria/config.toml in that container.
    if let Command::Connect {
        harness,
        config_dir,
        url,
        key,
        key_id,
        tool_capture,
        json,
    } = &command
    {
        let supplied_key = key
            .clone()
            .or_else(|| std::env::var("ALEXANDRIA_HARNESS_KEY").ok());
        let remote_url = url.clone().or_else(|| std::env::var("ALEXANDRIA_URL").ok());
        if let Some(key) = supplied_key.as_deref() {
            let harness = harness
                .as_deref()
                .context("a pre-minted harness key requires a harness name")?;
            let url = remote_url.as_deref().context(
                "a pre-minted harness key requires --url or ALEXANDRIA_URL; this avoids reading local Alexandria config",
            )?;
            harness_connect::connect_with_preminted_key(
                harness,
                config_dir.clone(),
                url,
                key.to_string(),
                key_id.clone(),
                *tool_capture,
                *json,
            )
            .await?;
            return Ok(());
        }
    }
    let (config, fresh_install) = load_or_create_config()?;

    match command {
        Command::Daemon {
            host,
            port,
            background,
            nosplash,
        } => {
            let mut config = config.clone();
            let host_was_overridden = host.is_some();
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
            if config.dario_api_key.is_empty() {
                config.dario_api_key = random_key("dario");
                save_config(&config)?;
                eprintln!("generated dario_api_key and saved it to config.toml");
            }
            let dario_route_enabled = config.dario_enabled();
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
                validate_subscription: dario_route_enabled,
                claude_bin: resolve_dario_claude_bin(config.dario_claude_bin.as_deref()),
            };
            let (dario_router, supervisor) = match dario::DarioSupervisor::start(settings).await {
                Ok(sup) => {
                    eprintln!(
                        "dario: ready generation {} (routing {})",
                        sup.active()
                            .map(|a| a.generation_id)
                            .unwrap_or_else(|| "-".into()),
                        if dario_route_enabled {
                            "enabled"
                        } else {
                            "direct"
                        }
                    );
                    (
                        Some(Arc::new(DarioGlue {
                            supervisor: sup.clone(),
                            route_enabled: dario_route_enabled,
                        })
                            as Arc<dyn alex_proxy::DarioRouter>),
                        Some(sup),
                    )
                }
                Err(e) => {
                    if dario_route_enabled {
                        eprintln!(
                            "dario: failed to start ({e}); non-Claude-Code Anthropic traffic will fail closed"
                        );
                    } else {
                        eprintln!("dario: unavailable ({e}); Anthropic traffic remains direct");
                    }
                    (
                        Some(Arc::new(DarioUnavailable {
                            error: e.to_string(),
                            route_enabled: dario_route_enabled,
                        })
                            as Arc<dyn alex_proxy::DarioRouter>),
                        None,
                    )
                }
            };
            let state = alex_proxy::build_state(
                config.local_key.clone(),
                vault,
                store,
                dario_router,
                daemon_connect_base_url(&host, port),
                config.upstream_stream_idle_timeout(),
            );
            alex_proxy::set_daemon_updater(
                &state,
                Arc::new(SelfUpdateApplier {
                    config: config.clone(),
                }),
            );
            alex_proxy::set_reset_handler(&state, Arc::new(reset::DaemonResetHandler));
            if config.update_check_hours > 0 {
                let update_status = state.update_status.clone();
                let hours = config.update_check_hours;
                let update_channel = config.update_channel();
                tokio::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                    loop {
                        match selfupdate::daemon_update_status_value(update_channel).await {
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
            app = app.merge(harness_admin_router(state.clone()));
            if let Some(sup) = supervisor.clone() {
                app = app.merge(dario_admin_router(sup, state.local_key.clone()));
            }
            let (listener, bound_host, fallback_reason) = bind_daemon_listener_with_fallback(
                &host,
                port,
                // An explicit `daemon --host` is a one-shot override and should
                // report its error. Only a persisted interface setting gets the
                // availability fallback promised by the settings UI.
                !host_was_overridden && !is_loopback_bind_host(&host),
            )
            .await?;
            if let Some(reason) = fallback_reason {
                eprintln!("\nWARNING: Alexandria could not bind its configured address {host}:{port}: {reason}");
                eprintln!("WARNING: Falling back to loopback (127.0.0.1) so the daemon remains available locally.");
                eprintln!("WARNING: The configured address was left unchanged; choose an available interface and restart to expose it again.\n");
            }
            let local_listener = if requires_explicit_loopback_listener(&bound_host) {
                Some(
                    bind_daemon_listener("127.0.0.1", port)
                        .await
                        .with_context(|| {
                            format!(
                        "binding loopback alongside the selected interface {bound_host}:{port}"
                    )
                        })?,
                )
            } else {
                None
            };
            print_banner(&bound_host, port, &config.local_key);
            if local_listener.is_some() {
                eprintln!(
                    "local clients and harnesses remain available at http://127.0.0.1:{port}"
                );
            }
            let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
            let shutdown_task = tokio::spawn(async move {
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
                let _ = shutdown_tx.send(true);
            });
            let shutdown = || {
                let mut receiver = shutdown_rx.clone();
                async move {
                    let _ = receiver.changed().await;
                }
            };
            let primary = axum::serve(
                listener,
                app.clone()
                    .into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .with_graceful_shutdown(shutdown());
            let serve_result = if let Some(local_listener) = local_listener {
                let local = axum::serve(
                    local_listener,
                    app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
                )
                .with_graceful_shutdown(shutdown());
                tokio::try_join!(primary, local).map(|_| ())
            } else {
                primary.await
            };
            shutdown_task.abort();
            serve_result?;
            if let Some(sup) = supervisor {
                sup.shutdown().await;
            }
        }
        Command::Auth { command } => match command {
            AuthCommand::Import {
                source,
                name,
                force,
            } => {
                validate_account_name(&name)?;
                let vault = open_vault(&config)?;
                let provider = if source != "all" {
                    Some(provider_from_cli(&source)?)
                } else {
                    None
                };
                if !force {
                    if let Some(p) = provider {
                        if vault.has_account_name(p, &name).await {
                            anyhow::bail!(
                                "{} account '{name}' already exists (use --force to replace)",
                                p.as_str()
                            );
                        }
                    } else if name != "default" {
                        anyhow::bail!(
                            "--name with source=all is ambiguous; import one provider at a time"
                        );
                    }
                }
                let pre_default = if name != "default" {
                    if let Some(p) = provider {
                        vault
                            .list()
                            .await
                            .into_iter()
                            .find(|a| a.id == named_account_id(p, "oauth", "default"))
                    } else {
                        None
                    }
                } else {
                    None
                };
                let mut outcomes = import_all(&vault, &source).await?;
                if name != "default" {
                    let imported: Vec<String> =
                        outcomes.iter().flat_map(|o| o.imported.clone()).collect();
                    for id in imported {
                        if let Some(mut a) = vault.list().await.into_iter().find(|a| a.id == id) {
                            a.name = name.clone();
                            a.id = named_account_id(a.provider, &a.kind, &name);
                            a.path = None;
                            vault.upsert(a).await?;
                        }
                    }
                    for o in &mut outcomes {
                        for id in &mut o.imported {
                            if let Some(a) = vault.list().await.into_iter().find(|a| {
                                a.name == name
                                    && a.provider.as_str()
                                        == provider.map(|p| p.as_str()).unwrap_or("")
                            }) {
                                *id = a.id;
                            }
                        }
                    }
                    if let Some(a) = pre_default {
                        vault.upsert(a).await?;
                    } else if let Some(p) = provider {
                        let _ = vault
                            .remove(&named_account_id(p, "oauth", "default"))
                            .await?;
                    }
                }
                for outcome in outcomes {
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
            AuthCommand::Login {
                provider,
                name,
                force,
            } => {
                use std::io::IsTerminal;
                validate_account_name(&name)?;
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
                let p = provider_from_cli(&provider)?;
                if !force && vault.has_account_name(p, &name).await {
                    anyhow::bail!(
                        "{} account '{name}' already exists (use --force to replace)",
                        p.as_str()
                    );
                }
                if p == alex_core::Provider::Openai {
                    let id = alex_auth::login::login_named(&vault, &provider, &name, force).await?;
                    println!("saved account: {id}");
                } else {
                    let default_id = named_account_id(p, "oauth", "default");
                    let pre_default = if name != "default" {
                        vault.list().await.into_iter().find(|a| a.id == default_id)
                    } else {
                        None
                    };
                    let id = alex_auth::login::login(&vault, &provider).await?;
                    if name != "default" {
                        if let Some(mut a) = vault.list().await.into_iter().find(|a| a.id == id) {
                            a.name = name.clone();
                            a.id = named_account_id(a.provider, &a.kind, &name);
                            a.path = None;
                            vault.upsert(a).await?;
                        }
                        if let Some(a) = pre_default {
                            vault.upsert(a).await?;
                        } else {
                            let _ = vault.remove(&default_id).await?;
                        }
                    }
                    println!(
                        "saved account: {}",
                        if name == "default" {
                            id
                        } else {
                            named_account_id(p, "oauth", &name)
                        }
                    );
                }
            }
            AuthCommand::Pause { provider, name } => {
                validate_account_name(&name)?;
                let vault = open_vault(&config)?;
                vault
                    .pause(provider_from_cli(&provider)?, &name, true)
                    .await?;
                println!("paused {provider}/{name}");
            }
            AuthCommand::Resume { provider, name } => {
                validate_account_name(&name)?;
                let vault = open_vault(&config)?;
                vault
                    .pause(provider_from_cli(&provider)?, &name, false)
                    .await?;
                println!("resumed {provider}/{name}");
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
                    id: named_account_id(alex_core::Provider::Gemini, "api_key", "default"),
                    provider: alex_core::Provider::Gemini,
                    kind: "api_key".into(),
                    name: "default".into(),
                    description: None,
                    paused: false,
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
                    path: None,
                };
                vault.upsert(account).await?;
                println!(
                    "{} saved gemini-api-key — gemini-* models now route to AI Studio",
                    ui::green(ui::dot())
                );
            }
            AuthCommand::AmpKey { key } => {
                let key = key
                    .or_else(|| std::env::var("AMP_API_KEY").ok())
                    .filter(|k| !k.trim().is_empty())
                    .context(
                        "provide the key: `alexandria auth amp-key <KEY>` (create one at https://ampcode.com/settings) or set AMP_API_KEY",
                    )?;
                let vault = open_vault(&config)?;
                let id = alex_auth::save_amp_api_key(&vault, &key).await?;
                println!(
                    "{} saved {id} — amp credits show in `alex limits` / menu bar",
                    ui::green(ui::dot())
                );
            }
            AuthCommand::OpenrouterKey {
                key,
                referer,
                title,
                remove,
            } => {
                let vault = open_vault(&config)?;
                if remove {
                    if key.is_some() || referer.is_some() || title.is_some() {
                        anyhow::bail!(
                            "--remove cannot be combined with a key, --referer, or --title"
                        );
                    }
                    if alex_auth::remove_openrouter_api_key(&vault).await? {
                        println!("{} removed openrouter-api-key", ui::green(ui::dot()));
                    } else {
                        println!("no OpenRouter API key was stored");
                    }
                } else {
                    let key = key
                        .or_else(|| std::env::var("OPENROUTER_API_KEY").ok())
                        .filter(|k| !k.trim().is_empty())
                        .context(
                            "provide the key: `alexandria auth openrouter-key <KEY>` or set OPENROUTER_API_KEY",
                        )?;
                    let id = alex_auth::save_openrouter_api_key(
                        &vault,
                        &key,
                        referer.as_deref(),
                        title.as_deref(),
                    )
                    .await?;
                    println!(
                        "{} saved {id} — use models such as openrouter/anthropic/claude-3.5-sonnet",
                        ui::green(ui::dot())
                    );
                }
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
                        "{dot} {} {} {} {} {} {}",
                        ui::pad_right(&ui::amber(a.provider.as_str()), 10),
                        ui::pad_right(&a.name, 12),
                        ui::pad_right(&a.kind, 8),
                        ui::pad_right(if a.paused { "paused" } else { &a.status }, 10),
                        ui::pad_right(&expiry, 20),
                        a.path
                            .as_ref()
                            .map(|p| p.display().to_string())
                            .unwrap_or_default()
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
            Some(TracesCommand::Reattach {
                orphan_account_id,
                to_account_id,
                yes,
                json,
            }) => {
                traces_reattach_cmd(
                    &config,
                    orphan_account_id.as_deref(),
                    to_account_id.as_deref(),
                    yes,
                    json,
                )
                .await?;
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
            Some(TracesCommand::RepairAgent {
                transcript_id,
                dry_run,
                json,
            }) => {
                traces_repair_agent_cmd(&config, &transcript_id, dry_run, json)?;
            }
            Some(TracesCommand::RepairAmp { run_id, json }) => {
                traces_repair_amp_cmd(&config, run_id.as_deref(), json).await?;
            }
            Some(TracesCommand::Push {
                run_id,
                remote_trace,
            }) => {
                traces_push_cmd(&config, &run_id, &remote_trace).await?;
            }
        },
        Command::Env => {
            print_env(&config.host, config.port, &config.local_key);
        }
        Command::Connect {
            harness,
            config_dir,
            url: _,
            key: _,
            key_id: _,
            tool_capture: _,
            json,
        } => {
            harness_connect::connect_cmd(&config, harness, config_dir, json).await?;
        }
        Command::ToolCapture {
            harness,
            state,
            json,
        } => {
            harness_connect::tool_capture_cmd(
                &config,
                harness,
                state.map(ToolCaptureState::enabled),
                json,
            )
            .await?;
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
                config.upstream_stream_idle_timeout(),
            );
            let models = config.ping_models();
            let wants_dario = target == "all" || target == "dario";
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
                                | alex_core::Provider::Openrouter
                        )
                        && !seen.contains(&a.provider)
                    {
                        seen.push(a.provider);
                    }
                }
                seen
            } else if target == "dario" {
                vec![]
            } else {
                vec![
                    alex_core::Provider::from_str_loose(&target).with_context(|| {
                        format!("unknown target '{target}' (anthropic|openai|grok|openrouter|dario|all)")
                    })?,
                ]
            };
            if providers.is_empty() && !wants_dario {
                println!("no pingable accounts — run `alexandria auth import`");
                return Ok(());
            }
            let mut results = if providers.is_empty() {
                vec![]
            } else {
                run_pings(&state, &models, &providers).await
            };
            if wants_dario {
                let dario = ping_dario_daemon(&config).await;
                println!("{}", ping_done_line(&dario));
                results.push(dario);
            }
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
                run_key_file,
                run_id,
                json,
            } => {
                let run_key = run_key_file
                    .as_deref()
                    .map(std::fs::read_to_string)
                    .transpose()?
                    .map(|key| key.trim().to_string())
                    .filter(|key| !key.is_empty());
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
                    dario_enabled: config.dario_enabled(),
                    local_key: run_key.unwrap_or_else(|| config.local_key.clone()),
                    run_id,
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
                config.upstream_stream_idle_timeout(),
            );
            let snap = alex_proxy::limits_snapshot(&state).await;
            if json {
                println!("{}", serde_json::to_string_pretty(&snap)?);
            } else {
                print_limits(&snap);
                print_dario_update_notice();
            }
        }
        Command::Routing { command } => {
            let vault = open_vault(&config)?;
            match command {
                RoutingCommand::Get { provider, json } => {
                    let provider = provider_from_cli(&provider)?;
                    let policy = vault.policy(provider);
                    let body = serde_json::json!({
                        "provider": provider.as_str(),
                        "reserve_pct": policy.reserve_pct.unwrap_or(10).min(100),
                        "account_reserve_pct": policy.account_reserve_pct,
                    });
                    if json {
                        println!("{}", serde_json::to_string_pretty(&body)?);
                    } else {
                        println!("{} reserve: {}%", provider.as_str(), body["reserve_pct"]);
                        let overrides = body["account_reserve_pct"].as_object().unwrap();
                        if overrides.is_empty() {
                            println!("per-account overrides: none");
                        } else {
                            for (account, reserve_pct) in overrides {
                                println!("{account}: {reserve_pct}%");
                            }
                        }
                    }
                }
                RoutingCommand::Set {
                    provider,
                    reserve_pct,
                    account,
                } => {
                    if reserve_pct > 100 {
                        anyhow::bail!("reserve_pct must be between 0 and 100");
                    }
                    let provider = provider_from_cli(&provider)?;
                    let mut policy = vault.policy(provider);
                    if let Some(account) = account {
                        let account = vault
                            .list()
                            .await
                            .into_iter()
                            .find(|candidate| {
                                candidate.provider == provider
                                    && (candidate.name == account || candidate.id == account)
                            })
                            .with_context(|| {
                                format!("unknown {} account '{account}'", provider.as_str())
                            })?;
                        policy
                            .account_reserve_pct
                            .insert(account.name.clone(), reserve_pct);
                        vault.set_policy_persisted(provider, policy).await?;
                        println!(
                            "{} / {} reserve: {}%",
                            provider.as_str(),
                            account.name,
                            reserve_pct
                        );
                    } else {
                        policy.reserve_pct = Some(reserve_pct);
                        vault.set_policy_persisted(provider, policy).await?;
                        println!("{} reserve: {}%", provider.as_str(), reserve_pct);
                    }
                }
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
        Command::Config { command } => match command {
            ConfigCommand::Host { address } => {
                let mut config = config;
                let changed = set_daemon_host(&mut config, &address)?;
                if changed {
                    println!("daemon host saved as {}", config.host);
                    println!("restart the Alexandria daemon to apply the new listener");
                } else {
                    println!("daemon host is already {}", config.host);
                }
            }
        },
        Command::Service { command } => match command {
            ServiceCommand::Install => service_install(&config)?,
            ServiceCommand::Bind { target } => service_set_bind(&config, &target)?,
            ServiceCommand::Restart { force } => service_restart(&config, force)?,
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
            channel,
            set_channel,
        } => {
            let mut config = config;
            let update_channel = if let Some(value) = set_channel {
                let parsed = selfupdate::UpdateChannel::parse(&value)?;
                if config.update_channel != parsed.as_str() {
                    config.update_channel = parsed.as_str().to_string();
                    save_config(&config)?;
                }
                println!("update channel set to {} in config.toml", parsed.as_str());
                parsed
            } else if let Some(value) = channel {
                selfupdate::UpdateChannel::parse(&value)?
            } else {
                config.update_channel()
            };
            selfupdate::run_update(&config, check, yes, no_restart, json, force, update_channel)
                .await?;
        }
        Command::Dario { command } => {
            match command {
                DarioCommand::Bootstrap { json } => {
                    let result = dario::bootstrap(
                        config.data_dir.join("dario"),
                        config.dario_version.clone(),
                    )
                    .await?;
                    if json {
                        println!("{}", serde_json::to_string_pretty(&result)?);
                    } else if result.already_installed {
                        println!(
                            "Dario {} is ready (Node {}, existing install)",
                            result.version, result.runtime_version
                        );
                    } else {
                        println!(
                            "installed Dario {} with {} (Node {})",
                            result.version, result.package_manager, result.runtime_version
                        );
                    }
                    return Ok(());
                }
                DarioCommand::Enable | DarioCommand::Disable => {
                    let enabled = matches!(command, DarioCommand::Enable);
                    let mut config = config;
                    config.anthropic_upstream = if enabled { "dario" } else { "direct" }.into();
                    save_config(&config)?;
                    println!(
                        "Dario routing {} in config.toml; restart Alexandria to apply",
                        if enabled { "enabled" } else { "disabled" }
                    );
                    return Ok(());
                }
                DarioCommand::Status | DarioCommand::Restart | DarioCommand::Update => {}
            }
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
                DarioCommand::Bootstrap { .. } | DarioCommand::Enable | DarioCommand::Disable => {
                    unreachable!()
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
                kind,
                run_id,
                tag,
                ttl,
                label,
                json,
            } => {
                keys_mint_cmd(&config, &kind, run_id, &tag, &ttl, label, json).await?;
            }
            KeysCommand::List { all, json } => {
                keys_list_cmd(&config, all, json).await?;
            }
            KeysCommand::Revoke { id } => {
                keys_revoke_cmd(&config, &id).await?;
            }
        },
        Command::Reset {
            credentials,
            settings,
            traces,
            harnesses,
            cache,
            all,
            yes,
        } => {
            let selection = reset::Selection {
                credentials: credentials || all,
                settings: settings || all,
                traces: traces || all,
                harnesses: harnesses || all,
                cache: cache || all,
            };
            let vault = open_vault(&config)?;
            let store = Store::open(config.data_dir.clone())?;
            let plan = reset::execute(
                &config,
                &alexandria_home().join("config.toml"),
                &vault,
                &store,
                selection,
                !yes,
            )
            .await?;
            println!("{}", serde_json::to_string_pretty(&plan)?);
        }
        Command::Tui => {
            tui::run(&config.base_url(), &config.local_key).await?;
        }
        Command::Wrap { command } => match command {
            WrapCommand::Status { json } => {
                wrap_status_cmd(&config, json).await?;
            }
            WrapCommand::Env {
                harness,
                mode,
                wrap_url,
                ca_cert,
                json,
            } => {
                wrap_env_cmd(&config, &harness, mode.as_deref(), &wrap_url, ca_cert, json).await?;
            }
            WrapCommand::Claude { args } => {
                wrap_launcher_cmd(&config, "claude", args)?;
            }
            WrapCommand::Codex { args } => {
                wrap_launcher_cmd(&config, "codex", args)?;
            }
            WrapCommand::Amp {
                remote_trace,
                mode,
                bind,
                upstream,
                serve_only,
                quiet,
                args,
            } => {
                wrap_run_cmd(
                    &config,
                    "amp",
                    remote_trace,
                    mode,
                    bind,
                    upstream,
                    serve_only,
                    quiet,
                    args,
                )
                .await?;
            }
            WrapCommand::Agent {
                remote_trace,
                mode,
                bind,
                upstream,
                serve_only,
                quiet,
                args,
            } => {
                wrap_run_cmd(
                    &config,
                    "agent",
                    remote_trace,
                    mode,
                    bind,
                    upstream,
                    serve_only,
                    quiet,
                    args,
                )
                .await?;
            }
            WrapCommand::Run {
                harness,
                remote_trace,
                mode,
                bind,
                upstream,
                serve_only,
                quiet,
                args,
            } => {
                wrap_run_cmd(
                    &config,
                    &harness,
                    remote_trace,
                    mode,
                    bind,
                    upstream,
                    serve_only,
                    quiet,
                    args,
                )
                .await?;
            }
            WrapCommand::Smoke { json, harness } => {
                wrap_smoke_cmd(&harness, json).await?;
            }
        },
    }
    Ok(())
}

#[derive(Clone)]
struct RemoteTraceConfig {
    base_url: String,
    key: String,
}

fn claude_launcher_args(config_dir: &Path, args: &[String]) -> Vec<OsString> {
    let mut launch_args = vec![
        OsString::from("--settings"),
        config_dir
            .join(harness_connect::CLAUDE_PROFILE_FILE)
            .into_os_string(),
    ];
    launch_args.extend(args.iter().map(OsString::from));
    launch_args
}

fn codex_launcher_args(args: &[String]) -> Vec<OsString> {
    let mut launch_args = vec![OsString::from("--profile"), OsString::from("alex")];
    launch_args.extend(args.iter().map(OsString::from));
    launch_args
}

fn ensure_wrap_launcher_connected(harness: &str, config_dir: &Path) -> Result<()> {
    let connected = match harness {
        "claude" => harness_connect::claude_config_connected(config_dir)?,
        "codex" => {
            harness_connect::codex_config_connected(config_dir)?
                && config_dir
                    .join(harness_connect::CODEX_ALEX_PROFILE_FILE)
                    .is_file()
        }
        _ => unreachable!("wrap launchers are only defined for Claude and Codex"),
    };
    if !connected {
        anyhow::bail!(
            "{harness} is not connected to Alexandria; run `alex connect {harness}` first"
        );
    }
    Ok(())
}

fn wrap_launcher_cmd(config: &Config, harness: &str, args: Vec<String>) -> Result<()> {
    let spec = match harness {
        "claude" => harness_connect::claude_spec(),
        "codex" => harness_connect::codex_spec(),
        _ => unreachable!("wrap launchers are only defined for Claude and Codex"),
    };
    let config_dir = harness_connect::resolve_config_dir(config, spec, None);
    ensure_wrap_launcher_connected(harness, &config_dir)?;
    let binary = harness_connect::resolve_harness_binary(config, spec)
        .with_context(|| format!("{harness} is not installed or not on PATH"))?;
    let launch_args = match harness {
        "claude" => claude_launcher_args(&config_dir, &args),
        "codex" => codex_launcher_args(&args),
        _ => unreachable!("wrap launchers are only defined for Claude and Codex"),
    };
    launch_harness(&binary, &launch_args)
}

#[cfg(unix)]
fn launch_harness(binary: &Path, args: &[OsString]) -> Result<()> {
    use std::os::unix::process::CommandExt;

    Err(std::process::Command::new(binary).args(args).exec())
        .with_context(|| format!("could not launch {}", binary.display()))
}

#[cfg(windows)]
fn launch_harness(binary: &Path, args: &[OsString]) -> Result<()> {
    let status = std::process::Command::new(binary)
        .args(args)
        .status()
        .with_context(|| format!("could not launch {}", binary.display()))?;
    std::process::exit(status.code().unwrap_or(1));
}

#[cfg(not(any(unix, windows)))]
fn launch_harness(binary: &Path, args: &[OsString]) -> Result<()> {
    let status = std::process::Command::new(binary)
        .args(args)
        .status()
        .with_context(|| format!("could not launch {}", binary.display()))?;
    if status.success() {
        Ok(())
    } else {
        anyhow::bail!("{} exited with {status}", binary.display())
    }
}

fn truthy_env(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .map(|value| {
            matches!(
                value.to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

fn resolve_remote_trace_config(args: &RemoteTraceArgs) -> Result<Option<RemoteTraceConfig>> {
    let base_url = args
        .trace_url
        .clone()
        .or_else(|| std::env::var("ALEXANDRIA_TRACE_URL").ok())
        .map(|value| value.trim_end_matches('/').to_string());
    let key = if let Some(path) = &args.trace_key_file {
        Some(
            std::fs::read_to_string(path)
                .with_context(|| format!("read trace key file {}", path.display()))?
                .trim()
                .to_string(),
        )
    } else if let Ok(value) = std::env::var("ALEXANDRIA_TRACE_KEY") {
        Some(value.trim().to_string())
    } else if let Ok(path) = std::env::var("ALEXANDRIA_TRACE_KEY_FILE") {
        Some(
            std::fs::read_to_string(&path)
                .with_context(|| format!("read trace key file {path}"))?
                .trim()
                .to_string(),
        )
    } else {
        None
    };
    let Some(base_url) = base_url else {
        if key
            .as_deref()
            .map(|value| !value.is_empty())
            .unwrap_or(false)
        {
            anyhow::bail!(
                "ALEXANDRIA_TRACE_KEY was set without --trace-url / ALEXANDRIA_TRACE_URL"
            );
        }
        return Ok(None);
    };
    let key = key.filter(|value| !value.is_empty()).with_context(|| {
        "remote trace upload requires --trace-key-file, ALEXANDRIA_TRACE_KEY, or ALEXANDRIA_TRACE_KEY_FILE"
    })?;
    if !key.starts_with("alxk-") {
        anyhow::bail!("remote trace credential is not an Alexandria key (expected alxk-...)");
    }
    let url = reqwest::Url::parse(&base_url)
        .with_context(|| format!("invalid trace destination URL '{base_url}'"))?;
    if !matches!(url.scheme(), "http" | "https") {
        anyhow::bail!("trace destination must use http:// or https://");
    }
    let loopback = url.host_str().is_some_and(|host| {
        host.eq_ignore_ascii_case("localhost")
            || host
                .parse::<std::net::IpAddr>()
                .map(|ip| ip.is_loopback())
                .unwrap_or(false)
    });
    let allow_insecure =
        args.allow_insecure_http || truthy_env("ALEXANDRIA_TRACE_ALLOW_INSECURE_HTTP");
    if url.scheme() == "http" && !loopback && !allow_insecure {
        anyhow::bail!(
            "refusing plaintext remote trace upload to {base_url}; use HTTPS or --allow-insecure-http on a trusted private network"
        );
    }
    Ok(Some(RemoteTraceConfig { base_url, key }))
}

async fn preflight_remote_trace(config: &RemoteTraceConfig) -> Result<()> {
    let url = format!("{}/traces/ingest", config.base_url);
    let response = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .build()?
        .get(&url)
        .header("x-api-key", &config.key)
        .send()
        .await
        .with_context(|| format!("could not reach remote trace ingest at {url}"))?;
    if !response.status().is_success() {
        let status = response.status();
        let detail = response.text().await.unwrap_or_default();
        anyhow::bail!(
            "remote trace ingest preflight returned {status}: {}",
            ui::truncate(&detail, 300)
        );
    }
    Ok(())
}

#[derive(Clone)]
struct RemoteTraceSender {
    tx: std::sync::mpsc::SyncSender<alex_core::TraceIngestPayload>,
}

impl RemoteTraceSender {
    fn send(&self, payload: alex_core::TraceIngestPayload) -> Result<()> {
        self.tx
            .send(payload)
            .context("remote trace uploader stopped unexpectedly")
    }
}

struct RemoteTraceWorker {
    sender: RemoteTraceSender,
    join: std::thread::JoinHandle<Result<RemoteTraceUploadReport>>,
}

#[derive(Debug, Default)]
struct RemoteTraceUploadReport {
    uploaded: usize,
    failed: usize,
    failures: Vec<String>,
}

impl RemoteTraceWorker {
    fn start(config: RemoteTraceConfig) -> Self {
        let (tx, rx) = std::sync::mpsc::sync_channel::<alex_core::TraceIngestPayload>(256);
        let join = std::thread::spawn(move || {
            let client = reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .redirect(reqwest::redirect::Policy::none())
                .build()?;
            let url = format!("{}/traces/ingest", config.base_url);
            let mut report = RemoteTraceUploadReport::default();
            for payload in rx {
                let trace_id = payload.trace.id.clone();
                let mut delivered = false;
                let mut last_error = String::new();
                for (attempt, delay_ms) in [0u64, 250, 1_000, 2_000, 4_000].into_iter().enumerate()
                {
                    if delay_ms > 0 {
                        std::thread::sleep(std::time::Duration::from_millis(delay_ms));
                    }
                    match client
                        .post(&url)
                        .header("x-api-key", &config.key)
                        .json(&payload)
                        .send()
                    {
                        Ok(response) if response.status().is_success() => {
                            delivered = true;
                            report.uploaded += 1;
                            break;
                        }
                        Ok(response)
                            if response.status().is_server_error()
                                || matches!(
                                    response.status(),
                                    reqwest::StatusCode::REQUEST_TIMEOUT
                                        | reqwest::StatusCode::TOO_MANY_REQUESTS
                                ) =>
                        {
                            last_error = format!("HTTP {}", response.status());
                        }
                        Ok(response) => {
                            let status = response.status();
                            let detail = response.text().unwrap_or_default();
                            last_error =
                                format!("rejected with {status}: {}", ui::truncate(&detail, 300));
                            break;
                        }
                        Err(error) => last_error = error.to_string(),
                    }
                    if attempt == 4 {
                        break;
                    }
                }
                if !delivered {
                    report.failed += 1;
                    report.failures.push(format!("{trace_id}: {last_error}"));
                }
            }
            Ok(report)
        });
        Self {
            sender: RemoteTraceSender { tx },
            join,
        }
    }

    fn sender(&self) -> RemoteTraceSender {
        self.sender.clone()
    }

    async fn stop(self) -> Result<RemoteTraceUploadReport> {
        drop(self.sender);
        tokio::task::spawn_blocking(move || {
            self.join
                .join()
                .map_err(|_| anyhow::anyhow!("remote trace uploader panicked"))?
        })
        .await
        .context("join remote trace uploader task")?
    }
}

async fn wrap_run_cmd(
    config: &Config,
    harness: &str,
    remote_trace: RemoteTraceArgs,
    mode: Option<String>,
    bind: String,
    upstream: Option<String>,
    serve_only: bool,
    quiet: bool,
    args: Vec<String>,
) -> Result<()> {
    let remote_config = match resolve_remote_trace_config(&remote_trace)? {
        Some(remote) => match preflight_remote_trace(&remote).await {
            Ok(()) => {
                eprintln!(
                    "alex wrap: normalized traces → {}/traces/ingest",
                    remote.base_url
                );
                Some(remote)
            }
            Err(error) => {
                eprintln!("alex wrap: remote trace upload unavailable: {error:#}");
                eprintln!(
                    "alex wrap: continuing with the local spool; replay with `alex traces push --run-id <run-id>`"
                );
                None
            }
        },
        None => None,
    };
    let remote_worker = remote_config.map(RemoteTraceWorker::start);
    let remote_sender = remote_worker.as_ref().map(RemoteTraceWorker::sender);
    let vault = open_vault(config)?;
    if harness == "amp" {
        // Refresh the native token and its provider-reported email before the
        // trace importer snapshots billing attribution for this run.
        let _ = alex_auth::import_amp(&vault).await;
    }
    let catalog = alex_wrap::load_catalog()?;
    let provider = catalog
        .resolve(harness)
        .and_then(|(_, h)| {
            h.credentials
                .as_ref()
                .and_then(|c| c.vault_provider.clone())
        })
        .unwrap_or_else(|| harness.to_string());
    // Native harness credentials are the freshest source (Amp can rotate its
    // secrets.json token independently). Use the vault only as a fallback.
    let native_key = match catalog
        .resolve(harness)
        .and_then(|(_, profile)| profile.credentials.as_ref())
    {
        Some(credentials) => alex_wrap::resolve_credential(credentials)?,
        None => None,
    };
    let credential_override = if let Some(key) = native_key {
        Some(key)
    } else {
        vault
            .list()
            .await
            .into_iter()
            .find(|a| a.provider.as_str() == provider && a.status == "active")
            .and_then(|a| a.api_key.or(a.access_token))
    };
    let amp_billing_account = if harness == "amp" {
        vault
            .list()
            .await
            .into_iter()
            .find(|account| {
                account.provider == alex_core::Provider::Amp
                    && account.status == "active"
                    && account
                        .api_key
                        .as_deref()
                        .or(account.access_token.as_deref())
                        == credential_override.as_deref()
            })
            .map(|account| known_account(&account))
    } else {
        None
    };
    let cursor_metadata = (harness == "agent").then(|| cursor_trace_metadata(&args));

    let amp_trace = if harness == "amp" {
        Some(
            start_amp_trace_import(
                config.data_dir.clone(),
                harness,
                remote_sender.clone(),
                amp_billing_account,
            )
            .await?,
        )
    } else {
        None
    };
    let agent_trace = if harness == "agent" {
        Some(
            start_agent_trace_import(
                config.data_dir.clone(),
                remote_sender,
                cursor_metadata.unwrap_or_default(),
            )
            .await?,
        )
    } else {
        None
    };

    let outcome = alex_wrap::run_wrapped(alex_wrap::RunOptions {
        harness: harness.to_string(),
        mode,
        bind,
        upstream,
        capture_base: config.data_dir.clone(),
        credential_override,
        ca_cert_path: None,
        serve_only,
        args,
        quiet,
    })
    .await;

    let amp_result = if let Some(importer) = amp_trace {
        importer.stop().await
    } else {
        Ok(())
    };
    let agent_result = if let Some(importer) = agent_trace {
        importer.stop().await
    } else {
        Ok(())
    };
    if let Some(worker) = remote_worker {
        match worker.stop().await {
            Ok(report) if report.failed == 0 => {
                eprintln!("alex wrap: uploaded {} trace update(s)", report.uploaded)
            }
            Ok(report) => {
                eprintln!(
                    "alex wrap: uploaded {} trace update(s); {} remain in the local spool",
                    report.uploaded, report.failed
                );
                if let Some(error) = report.failures.first() {
                    eprintln!("alex wrap: first upload failure: {error}");
                }
                eprintln!(
                    "alex wrap: replay with `alex traces push --run-id <run-id>` after fixing the destination"
                );
            }
            Err(error) => {
                eprintln!("alex wrap: remote trace upload stopped: {error:#}");
                eprintln!(
                    "alex wrap: traces remain in the local spool; replay with `alex traces push --run-id <run-id>`"
                );
            }
        }
    }

    amp_result?;
    agent_result?;

    let outcome = outcome?;

    if outcome.exit_code != 0 {
        std::process::exit(outcome.exit_code);
    }
    Ok(())
}

struct AmpTraceImporter {
    stop: tokio::sync::oneshot::Sender<()>,
    join: tokio::task::JoinHandle<Result<()>>,
    run_id: String,
}

impl AmpTraceImporter {
    async fn stop(self) -> Result<()> {
        let _ = self.stop.send(());
        self.join.await.context("join Amp trace importer")??;
        eprintln!("alex wrap: amp traces imported with run_id={}", self.run_id);
        Ok(())
    }
}

async fn start_amp_trace_import(
    data_dir: PathBuf,
    harness: &str,
    remote: Option<RemoteTraceSender>,
    billing_account: Option<KnownAccount>,
) -> Result<AmpTraceImporter> {
    let run_id = format!(
        "wrap-{harness}-{}-{:08x}",
        now_ms(),
        rand::thread_rng().gen::<u32>()
    );
    let tags = serde_json::json!({
        "harness": harness,
        "wrap": "amp",
        "source": "alex-wrap-ws",
        "stream": "dialogue",
    })
    .to_string();
    let run_key = alex_proxy::generate_run_key();
    let run_key_hash = amp_key_hash_hex(&run_key);
    let run_key_id = format!("rk-{}", &run_key_hash[..8]);
    Store::open(data_dir.clone())?.insert_run_key(
        &run_key_id,
        &run_key_hash,
        "harness",
        Some(&run_id),
        Some(&tags),
        Some("amp-wrap"),
        now_ms(),
        None,
    )?;
    let capture_dir = alex_wrap::capture_dir_for(&data_dir, harness);
    std::fs::create_dir_all(&capture_dir)?;
    let ws_path = capture_dir.join("ws.jsonl");
    let (tx, mut rx) = tokio::sync::oneshot::channel::<()>();
    let run_id_c = run_id.clone();
    let ws_path_c = ws_path.clone();
    let join = tokio::spawn(async move {
        let mut state = AmpWsTraceState::new(
            data_dir,
            run_id_c,
            tags,
            run_key_hash[..16].to_string(),
            remote,
        );
        state.billing_account = billing_account;
        let mut offset = 0u64;
        loop {
            tokio::select! {
                _ = &mut rx => {
                    break state.ingest_new(&ws_path_c, &mut offset);
                }
                _ = tokio::time::sleep(std::time::Duration::from_millis(250)) => {
                    if let Err(error) = state.ingest_new(&ws_path_c, &mut offset) {
                        eprintln!("alex wrap: amp trace import failed: {error:#}");
                    }
                }
            }
        }
    });
    eprintln!("alex wrap: amp trace run_id={run_id} key={run_key_id}");
    eprintln!("alex wrap: amp ws → traces from {}", ws_path.display());
    Ok(AmpTraceImporter {
        stop: tx,
        join,
        run_id,
    })
}

struct AmpWsTraceState {
    data_dir: PathBuf,
    run_id: String,
    tags: String,
    key_fingerprint: String,
    remote: Option<RemoteTraceSender>,
    user_by_thread: BTreeMap<String, AmpUserMessage>,
    inserted: std::collections::BTreeSet<String>,
    active_error: Option<String>,
    billing_account: Option<KnownAccount>,
}

#[derive(Clone)]
struct AmpUserMessage {
    id: String,
    text: String,
    ts_ms: i64,
}

impl AmpWsTraceState {
    fn new(
        data_dir: PathBuf,
        run_id: String,
        tags: String,
        key_fingerprint: String,
        remote: Option<RemoteTraceSender>,
    ) -> Self {
        Self {
            data_dir,
            run_id,
            tags,
            key_fingerprint,
            remote,
            user_by_thread: BTreeMap::new(),
            inserted: std::collections::BTreeSet::new(),
            active_error: None,
            billing_account: None,
        }
    }

    fn ingest_new(&mut self, path: &std::path::Path, offset: &mut u64) -> Result<()> {
        use std::io::{Read, Seek, SeekFrom};
        let Ok(mut f) = std::fs::File::open(path) else {
            return Ok(());
        };
        let len = f.metadata()?.len();
        if len < *offset {
            *offset = 0;
        }
        f.seek(SeekFrom::Start(*offset))?;
        let mut s = String::new();
        f.read_to_string(&mut s)?;
        // The websocket capture is appended concurrently. Only advance over
        // newline-terminated records so an in-progress JSONL write is retried
        // intact on the next poll instead of being permanently skipped.
        let complete_len = s.rfind('\n').map(|offset| offset + 1).unwrap_or(0);
        let complete = &s[..complete_len];
        *offset += complete_len as u64;
        for line in complete.lines() {
            if let Err(e) = self.ingest_line(line) {
                tracing::debug!("amp ws trace import skipped line: {e:#}");
            }
        }
        Ok(())
    }

    fn ingest_line(&mut self, line: &str) -> Result<()> {
        let outer: serde_json::Value = serde_json::from_str(line)?;
        if outer["direction"].as_str() != Some("upstream_to_client") {
            return Ok(());
        }
        let Some(text) = outer["text"].as_str() else {
            return Ok(());
        };
        if text == "pong" || text == "ping" {
            return Ok(());
        }
        let msg: serde_json::Value = serde_json::from_str(text)?;
        let method = msg["method"].as_str().unwrap_or_default();
        match method {
            "message_added" => self.ingest_message_added(&msg, outer["ts"].as_str()),
            "plugin_message" => self.ingest_plugin_message(&msg, outer["ts"].as_str()),
            "error_set" => {
                self.active_error = msg["params"]["error"]["message"].as_str().map(String::from);
                Ok(())
            }
            "error_cleared" => {
                self.active_error = None;
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn ingest_message_added(&mut self, msg: &serde_json::Value, ts: Option<&str>) -> Result<()> {
        let m = &msg["params"]["message"];
        let role = m["role"].as_str().unwrap_or_default();
        let thread_id = m["threadId"].as_str().unwrap_or_default();
        let message_id = m["messageId"].as_str().unwrap_or_default();
        let text = amp_content_text(&m["content"]);
        if thread_id.is_empty() || message_id.is_empty() {
            return Ok(());
        }
        if role == "user" && !text.is_empty() {
            // Amp also represents tool results as role=user messages. Those
            // blocks have no direct human text and must not replace the
            // question that originated the current assistant turn.
            self.user_by_thread.insert(
                thread_id.to_string(),
                AmpUserMessage {
                    id: message_id.to_string(),
                    text,
                    ts_ms: parse_ts_ms(ts).unwrap_or_else(now_ms),
                },
            );
        } else if role == "assistant" && !text.is_empty() {
            self.insert_amp_trace(thread_id, message_id, &text, &m["usage"], ts, None)?;
        }
        Ok(())
    }

    fn ingest_plugin_message(&mut self, msg: &serde_json::Value, ts: Option<&str>) -> Result<()> {
        let pm = &msg["params"]["message"];
        if pm["method"].as_str() != Some("agent.end") {
            return Ok(());
        }
        let event = &pm["params"]["event"];
        let thread_id = event["thread"]["id"].as_str().unwrap_or_default();
        let messages = event["messages"].as_array().cloned().unwrap_or_default();
        let mut user = None;
        let mut assistant = None;
        for m in messages {
            match m["role"].as_str() {
                Some("user") => {
                    let text = amp_content_text(&m["content"]);
                    if !text.is_empty() {
                        user = Some(AmpUserMessage {
                            id: m["id"].as_str().unwrap_or_default().to_string(),
                            text,
                            ts_ms: parse_ts_ms(ts).unwrap_or_else(now_ms),
                        });
                    }
                }
                Some("assistant") => {
                    assistant = Some((
                        m["id"].as_str().unwrap_or_default().to_string(),
                        amp_content_text(&m["content"]),
                    ));
                }
                _ => {}
            }
        }
        if let Some(u) = user {
            self.user_by_thread.insert(thread_id.to_string(), u);
        }
        if let Some((id, text)) = assistant {
            if !id.is_empty() && !text.is_empty() {
                self.insert_amp_trace(thread_id, &id, &text, &serde_json::Value::Null, ts, None)?;
            }
        } else if event["status"].as_str() == Some("error") {
            let user_id = self
                .user_by_thread
                .get(thread_id)
                .map(|user| user.id.as_str())
                .or_else(|| event["id"].as_str())
                .unwrap_or("unknown");
            let message_id = format!("error-{user_id}");
            let error = self
                .active_error
                .clone()
                .unwrap_or_else(|| "Amp turn ended with an error".into());
            self.insert_amp_trace(
                thread_id,
                &message_id,
                "",
                &serde_json::Value::Null,
                ts,
                Some(error),
            )?;
        }
        Ok(())
    }

    fn insert_amp_trace(
        &mut self,
        thread_id: &str,
        message_id: &str,
        assistant_text: &str,
        usage_v: &serde_json::Value,
        ts: Option<&str>,
        error: Option<String>,
    ) -> Result<()> {
        if self.inserted.contains(message_id) {
            return Ok(());
        }
        let store = Store::open(self.data_dir.clone())?;
        let user = self.user_by_thread.get(thread_id).cloned();
        let ts_resp = parse_ts_ms(ts).unwrap_or_else(now_ms);
        let ts_req = user.as_ref().map(|u| u.ts_ms).unwrap_or(ts_resp);
        let model = usage_v["model"].as_str().unwrap_or("amp-actor").to_string();
        let usage = alex_core::Usage {
            input_tokens: usage_v["inputTokens"].as_i64(),
            cached_input_tokens: usage_v["cacheReadInputTokens"].as_i64(),
            cache_creation_tokens: usage_v["cacheCreationInputTokens"].as_i64(),
            output_tokens: usage_v["outputTokens"].as_i64(),
            reasoning_tokens: None,
        };
        let cost_usd = store
            .pricing_for(&model)
            .map(|p| alex_core::compute_cost(&usage, &p, false));
        let trace_id = format!("amp-{message_id}");
        // Store normalized OpenAI-chat-shaped bodies so the existing trace
        // browser/transcript extractors can render Amp dialogue without
        // learning every noisy actor event shape.
        let req = serde_json::json!({
            "model": model,
            "messages": [{
                "role": "user",
                "content": user.as_ref().map(|u| u.text.as_str()).unwrap_or(""),
            }],
            "amp": {
                "thread_id": thread_id,
                "message_id": user.as_ref().map(|u| u.id.as_str()),
            }
        });
        let resp = serde_json::json!({
            "id": message_id,
            "model": model,
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": if assistant_text.is_empty() {
                    serde_json::Value::Null
                } else {
                    serde_json::Value::String(assistant_text.to_string())
                }},
                "finish_reason": if error.is_some() {
                    serde_json::Value::Null
                } else {
                    serde_json::Value::String("stop".into())
                }
            }],
            "error": error.as_ref().map(|message| serde_json::json!({"message": message})),
            "usage": {
                "prompt_tokens": usage.input_tokens,
                "completion_tokens": usage.output_tokens,
                "cache_creation_input_tokens": usage.cache_creation_tokens,
                "cache_read_input_tokens": usage.cached_input_tokens
            },
            "amp": {
                "thread_id": thread_id,
                "message_id": message_id,
                "raw_usage": usage_v
            }
        });
        let req_path = Some(store.write_body(
            &trace_id,
            "request.json",
            serde_json::to_string_pretty(&req)?.as_bytes(),
        )?);
        let resp_path = Some(store.write_body(
            &trace_id,
            "response.body",
            serde_json::to_string_pretty(&resp)?.as_bytes(),
        )?);
        if let Some(account) = &self.billing_account {
            store.upsert_known_account(account)?;
        }
        let rec = alex_core::TraceRecord {
            id: trace_id,
            ts_request_ms: ts_req,
            ts_response_ms: Some(ts_resp),
            session_id: Some(thread_id.to_string()),
            harness: Some("amp".into()),
            client_format: Some("openai-chat".into()),
            upstream_provider: Some("amp".into()),
            upstream_format: Some("openai-chat".into()),
            requested_model: Some(model.clone()),
            routed_model: Some(model),
            method: Some("WEBSOCKET".into()),
            path: Some("/actors/gateway/threadActor/websocket/".into()),
            status: Some(if error.as_deref().is_some_and(|message| message.contains("401")) {
                401
            } else if error.is_some() {
                500
            } else {
                200
            }),
            streamed: Some(true),
            usage,
            cost_usd,
            billing_bucket: Some("amp".into()),
            req_body_path: req_path,
            upstream_req_body_path: None,
            resp_body_path: resp_path,
            req_headers_json: Some(serde_json::json!({
                "x-alexandria-wrap": "amp",
                "x-alexandria-run-id": self.run_id,
                "x-alexandria-trace-tag": "harness=amp,wrap=amp,source=alex-wrap-ws,stream=dialogue"
            }).to_string()),
            resp_headers_json: Some(serde_json::json!({
                "x-alexandria-wrap": "amp",
                "x-alexandria-source": "websocket",
                "content-type": "application/json"
            }).to_string()),
            error,
            account_id: self
                .billing_account
                .as_ref()
                .map(|account| account.account_id.clone()),
            subscription_identity: self
                .billing_account
                .as_ref()
                .and_then(|account| account.subscription_identity.clone()),
            via_dario: false,
            dario_generation: None,
            run_id: Some(self.run_id.clone()),
            tags: Some(self.tags.clone()),
            client_ip: None,
            key_fingerprint: Some(self.key_fingerprint.clone()),
            reasoning_effort: None,
            thinking_budget: None,
        };
        store.insert_trace(&rec)?;
        self.inserted.insert(message_id.to_string());
        if let Some(remote) = &self.remote {
            queue_remote_trace_update(remote, &store, &rec.id);
        }
        tracing::info!(trace_id = %rec.id, "amp wrap trace recorded");
        Ok(())
    }
}

fn amp_content_text(content: &serde_json::Value) -> String {
    content
        .as_array()
        .map(|blocks| {
            blocks
                .iter()
                .filter_map(|b| b["text"].as_str())
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default()
}

fn parse_ts_ms(ts: Option<&str>) -> Option<i64> {
    let ts = ts?;
    chrono::DateTime::parse_from_rfc3339(ts)
        .ok()
        .map(|d| d.timestamp_millis())
}

fn amp_key_hash_hex(key: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(key.as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

struct AgentTraceImporter {
    stop: tokio::sync::oneshot::Sender<()>,
    join: tokio::task::JoinHandle<Result<()>>,
    run_id: String,
}

#[derive(Clone, Debug)]
struct CursorTraceMetadata {
    model: String,
    billing_account: Option<KnownAccount>,
}

impl Default for CursorTraceMetadata {
    fn default() -> Self {
        Self {
            model: "cursor-agent".into(),
            billing_account: None,
        }
    }
}

fn cursor_trace_metadata(args: &[String]) -> CursorTraceMetadata {
    let config = dirs::home_dir()
        .map(|home| home.join(".cursor/cli-config.json"))
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .unwrap_or(serde_json::Value::Null);
    cursor_trace_metadata_from_config(&config, args)
}

fn cursor_trace_metadata_from_config(
    config: &serde_json::Value,
    args: &[String],
) -> CursorTraceMetadata {
    let argument_model = args.iter().enumerate().find_map(|(offset, argument)| {
        argument
            .strip_prefix("--model=")
            .filter(|model| !model.trim().is_empty())
            .map(String::from)
            .or_else(|| {
                (argument == "--model")
                    .then(|| args.get(offset + 1))
                    .flatten()
                    .filter(|model| !model.trim().is_empty())
                    .cloned()
            })
    });
    let configured_model = config["selectedModel"]["modelId"]
        .as_str()
        .or_else(|| config["model"]["modelId"].as_str())
        .map(String::from);
    let model = argument_model
        .or(configured_model)
        .unwrap_or_else(|| "cursor-agent".into());

    let auth = &config["authInfo"];
    let email = auth["email"]
        .as_str()
        .map(str::trim)
        .filter(|email| email.contains('@') && !email.chars().any(char::is_whitespace))
        .map(str::to_ascii_lowercase);
    let user_id = auth["userId"]
        .as_u64()
        .map(|id| id.to_string())
        .or_else(|| auth["userId"].as_str().map(String::from));
    let auth_id = auth["authId"]
        .as_str()
        .filter(|id| !id.trim().is_empty())
        .map(String::from);
    let identity = user_id
        .as_deref()
        .map(|id| format!("cursor:user:{id}"))
        .or_else(|| auth_id.as_deref().map(|id| format!("cursor:auth:{id}")))
        .or_else(|| {
            email
                .as_deref()
                .map(|email| format!("cursor:email:{email}"))
        });
    let billing_account = identity.map(|identity| {
        let digest = amp_key_hash_hex(&identity);
        KnownAccount::new(
            format!("cursor-subscription-{}", &digest[..16]),
            "cursor",
            email.clone().unwrap_or_else(|| "Cursor".into()),
            "subscription",
            Some(identity),
            email,
        )
    });
    CursorTraceMetadata {
        model,
        billing_account,
    }
}

impl AgentTraceImporter {
    async fn stop(self) -> Result<()> {
        let _ = self.stop.send(());
        self.join
            .await
            .context("join Cursor Agent trace importer")??;
        eprintln!(
            "alex wrap: agent trace importer stopped with run_id={}",
            self.run_id
        );
        Ok(())
    }
}

async fn start_agent_trace_import(
    data_dir: PathBuf,
    remote: Option<RemoteTraceSender>,
    metadata: CursorTraceMetadata,
) -> Result<AgentTraceImporter> {
    let run_id = format!(
        "wrap-agent-{}-{:08x}",
        now_ms(),
        rand::thread_rng().gen::<u32>()
    );
    let transcript_root = cursor_agent_transcript_root()?;
    std::fs::create_dir_all(&transcript_root).ok();
    let started_ms = now_ms();
    let (tx, mut rx) = tokio::sync::oneshot::channel::<()>();
    let run_id_c = run_id.clone();
    let transcript_root_c = transcript_root.clone();
    let join = tokio::spawn(async move {
        let mut seen = BTreeMap::new();
        loop {
            tokio::select! {
                _ = &mut rx => {
                    break import_agent_transcripts(&data_dir, &transcript_root_c, &run_id_c, started_ms, &mut seen, remote.as_ref(), &metadata);
                }
                _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => {
                    if let Err(error) = import_agent_transcripts(&data_dir, &transcript_root_c, &run_id_c, started_ms, &mut seen, remote.as_ref(), &metadata) {
                        eprintln!("alex wrap: agent trace import failed: {error:#}");
                    }
                }
            }
        }
    });
    eprintln!("alex wrap: agent trace run_id={run_id}");
    eprintln!(
        "alex wrap: agent transcripts → traces from {}",
        transcript_root.display()
    );
    Ok(AgentTraceImporter {
        stop: tx,
        join,
        run_id,
    })
}

fn cursor_agent_transcript_root() -> Result<PathBuf> {
    let cwd = std::env::current_dir()?;
    let key = cwd
        .to_string_lossy()
        .trim_start_matches('/')
        .replace('/', "-");
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("home dir not found"))?;
    Ok(home
        .join(".cursor/projects")
        .join(key)
        .join("agent-transcripts"))
}

fn import_agent_transcripts(
    data_dir: &std::path::Path,
    root: &std::path::Path,
    run_id: &str,
    started_ms: i64,
    seen: &mut BTreeMap<String, AgentTranscriptTurn>,
    remote: Option<&RemoteTraceSender>,
    metadata: &CursorTraceMetadata,
) -> Result<()> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Ok(());
    };
    let store = Store::open(data_dir.to_path_buf())?;
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let Some(id) = dir.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        let path = dir.join(format!("{id}.jsonl"));
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        let modified = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        if modified + 2_000 < started_ms {
            continue;
        }
        let raw = std::fs::read_to_string(&path).unwrap_or_default();
        let turns = parse_agent_transcript_turns(&raw);
        for (offset, turn) in turns.into_iter().enumerate() {
            let idx = offset + 1;
            let key = format!("{id}-{idx}");
            if seen.get(&key) == Some(&turn) {
                continue;
            }
            let outcome =
                reconcile_agent_turn(&store, run_id, id, idx, modified, &turn, false, metadata)?;
            if outcome != AgentReconcileOutcome::Unchanged {
                if let Some(remote) = remote {
                    let trace_id = format!("agent-{id}-{idx}");
                    queue_remote_trace_update(remote, &store, &trace_id);
                }
            }
            seen.insert(key, turn);
        }
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq)]
struct AgentToolCall {
    name: String,
    input: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq)]
struct AgentTranscriptTurn {
    user: String,
    assistant: String,
    tool_calls: Vec<AgentToolCall>,
    assistant_blocks: Vec<serde_json::Value>,
    complete: bool,
    end_status: Option<String>,
}

#[derive(Default)]
struct AgentTurnBuilder {
    user: String,
    assistant_chunks: Vec<String>,
    tool_calls: Vec<AgentToolCall>,
    assistant_blocks: Vec<serde_json::Value>,
    saw_assistant: bool,
}

impl AgentTurnBuilder {
    fn finish(self, complete: bool, end_status: Option<String>) -> AgentTranscriptTurn {
        AgentTranscriptTurn {
            user: self.user,
            assistant: self.assistant_chunks.join("\n\n"),
            tool_calls: self.tool_calls,
            assistant_blocks: self.assistant_blocks,
            complete,
            end_status,
        }
    }
}

fn push_agent_turn(
    out: &mut Vec<AgentTranscriptTurn>,
    current: &mut Option<AgentTurnBuilder>,
    complete: bool,
    end_status: Option<String>,
) {
    let Some(turn) = current.take() else {
        return;
    };
    if turn.saw_assistant || complete {
        out.push(turn.finish(complete, end_status));
    }
}

fn parse_agent_transcript_turns(raw: &str) -> Vec<AgentTranscriptTurn> {
    let mut out = Vec::new();
    let mut current: Option<AgentTurnBuilder> = None;
    for line in raw.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        match v["role"].as_str() {
            Some("user") => {
                push_agent_turn(&mut out, &mut current, true, None);
                let text = agent_content_text(&v["message"]["content"]);
                current = Some(AgentTurnBuilder {
                    user: extract_user_query(&text),
                    ..AgentTurnBuilder::default()
                });
            }
            Some("assistant") => {
                let Some(turn) = current.as_mut() else {
                    continue;
                };
                turn.saw_assistant = true;
                for block in v["message"]["content"].as_array().into_iter().flatten() {
                    match block["type"].as_str() {
                        Some("text") => {
                            if let Some(text) = clean_agent_assistant_text(
                                block["text"].as_str().unwrap_or_default(),
                            ) {
                                turn.assistant_chunks.push(text.clone());
                                turn.assistant_blocks.push(serde_json::json!({
                                    "type": "text",
                                    "text": text,
                                }));
                            }
                        }
                        Some("tool_use") => {
                            if let Some(name) = block["name"].as_str() {
                                let input = block.get("input").cloned().unwrap_or_default();
                                turn.tool_calls.push(AgentToolCall {
                                    name: name.to_string(),
                                    input: input.clone(),
                                });
                                turn.assistant_blocks.push(serde_json::json!({
                                    "type": "tool_call",
                                    "name": name,
                                    "arguments": serde_json::to_string(&input).unwrap_or_default(),
                                }));
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        if v["type"] == "turn_ended" {
            push_agent_turn(
                &mut out,
                &mut current,
                true,
                v["status"].as_str().map(String::from),
            );
        }
    }
    push_agent_turn(&mut out, &mut current, false, None);
    out
}

fn parse_agent_transcript_turns_strict(raw: &str) -> Result<Vec<AgentTranscriptTurn>> {
    if raw.trim().is_empty() {
        anyhow::bail!("Cursor Agent transcript is empty");
    }
    for (offset, line) in raw.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        serde_json::from_str::<serde_json::Value>(line).with_context(|| {
            format!(
                "malformed Cursor Agent transcript JSON on line {}",
                offset + 1
            )
        })?;
    }
    let turns = parse_agent_transcript_turns(raw);
    if turns.is_empty() {
        anyhow::bail!("Cursor Agent transcript contains no conversation turns");
    }
    Ok(turns)
}

fn agent_content_text(content: &serde_json::Value) -> String {
    content
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|b| b["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

fn clean_agent_assistant_text(text: &str) -> Option<String> {
    let trimmed_end = text.trim_end_matches([' ', '\t', '\r', '\n']);
    let cleaned = if trimmed_end.trim() == "[REDACTED]" {
        ""
    } else if let Some(prefix) = trimmed_end.strip_suffix("[REDACTED]") {
        if prefix.ends_with('\n') {
            prefix.trim_end_matches(['\r', '\n'])
        } else {
            text
        }
    } else {
        text
    };
    (!cleaned.trim().is_empty()).then(|| cleaned.to_string())
}

fn extract_user_query(s: &str) -> String {
    if let Some(start) = s.find("<user_query>") {
        let rest = &s[start + "<user_query>".len()..];
        if let Some(end) = rest.find("</user_query>") {
            return rest[..end].trim().to_string();
        }
    }
    s.trim().to_string()
}

fn agent_trace_bodies(
    transcript_id: &str,
    turn_index: usize,
    turn: &AgentTranscriptTurn,
) -> (serde_json::Value, serde_json::Value) {
    agent_trace_bodies_for_model(transcript_id, turn_index, turn, "cursor-agent")
}

fn agent_trace_bodies_for_model(
    transcript_id: &str,
    turn_index: usize,
    turn: &AgentTranscriptTurn,
    model: &str,
) -> (serde_json::Value, serde_json::Value) {
    let req = serde_json::json!({
        "model": model,
        "messages": [{"role": "user", "content": turn.user}],
        "cursor": {"transcript_id": transcript_id}
    });
    let mut message = serde_json::json!({
        "role": "assistant",
        "content": if turn.assistant.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::Value::String(turn.assistant.clone())
        }
    });
    if !turn.tool_calls.is_empty() {
        message["tool_calls"] = serde_json::Value::Array(
            turn.tool_calls
                .iter()
                .enumerate()
                .map(|(offset, call)| {
                    serde_json::json!({
                        "id": format!("call_cursor_{turn_index}_{}", offset + 1),
                        "type": "function",
                        "function": {
                            "name": call.name,
                            "arguments": serde_json::to_string(&call.input).unwrap_or_default()
                        }
                    })
                })
                .collect(),
        );
    }
    let resp = serde_json::json!({
        "id": transcript_id,
        "model": model,
        "_alexandria": {"assistant_blocks": turn.assistant_blocks},
        "choices": [{
            "index": 0,
            "message": message,
            "finish_reason": if turn.complete {
                serde_json::Value::String("stop".into())
            } else {
                serde_json::Value::Null
            }
        }]
    });
    (req, resp)
}

fn read_gzip_json(path: &std::path::Path) -> Result<serde_json::Value> {
    use std::io::Read;

    let file = std::fs::File::open(path)
        .with_context(|| format!("open compressed trace body {}", path.display()))?;
    let mut decoder = flate2::read::GzDecoder::new(file);
    let mut text = String::new();
    decoder
        .read_to_string(&mut text)
        .with_context(|| format!("read compressed trace body {}", path.display()))?;
    serde_json::from_str(&text)
        .with_context(|| format!("parse compressed trace body {}", path.display()))
}

fn read_gzip_bytes(path: &std::path::Path) -> Result<Vec<u8>> {
    use std::io::Read;

    let file = std::fs::File::open(path)
        .with_context(|| format!("open compressed trace body {}", path.display()))?;
    let mut decoder = flate2::read::GzDecoder::new(file);
    let mut bytes = Vec::new();
    decoder
        .read_to_end(&mut bytes)
        .with_context(|| format!("read compressed trace body {}", path.display()))?;
    Ok(bytes)
}

fn trace_ingest_payload_from_store(
    store: &Store,
    trace_id: &str,
) -> Result<alex_core::TraceIngestPayload> {
    use base64::Engine;

    let row = store
        .get_trace(trace_id)?
        .with_context(|| format!("trace disappeared before upload: {trace_id}"))?;
    let string = |key: &str| row[key].as_str().map(String::from);
    let body = |key: &str| -> Result<Option<String>> {
        let Some(path) = row[key].as_str() else {
            return Ok(None);
        };
        Ok(Some(
            base64::engine::general_purpose::STANDARD
                .encode(read_gzip_bytes(std::path::Path::new(path))?),
        ))
    };
    let trace = alex_core::TraceRecord {
        id: trace_id.to_string(),
        ts_request_ms: row["ts_request_ms"].as_i64().unwrap_or_else(now_ms),
        ts_response_ms: row["ts_response_ms"].as_i64(),
        session_id: string("session_id"),
        harness: string("harness"),
        client_format: string("client_format"),
        upstream_provider: string("upstream_provider"),
        upstream_format: string("upstream_format"),
        requested_model: string("requested_model"),
        routed_model: string("routed_model"),
        method: string("method"),
        path: string("path"),
        status: row["status"].as_i64(),
        streamed: row["streamed"].as_i64().map(|value| value != 0),
        usage: alex_core::Usage {
            input_tokens: row["input_tokens"].as_i64(),
            cached_input_tokens: row["cached_input_tokens"].as_i64(),
            cache_creation_tokens: row["cache_creation_tokens"].as_i64(),
            output_tokens: row["output_tokens"].as_i64(),
            reasoning_tokens: row["reasoning_tokens"].as_i64(),
        },
        cost_usd: row["cost_usd"].as_f64(),
        billing_bucket: string("billing_bucket"),
        req_headers_json: string("req_headers_json"),
        resp_headers_json: string("resp_headers_json"),
        error: string("error"),
        account_id: string("account_id"),
        subscription_identity: string("subscription_identity"),
        run_id: string("run_id"),
        tags: string("tags_json"),
        client_ip: string("client_ip"),
        key_fingerprint: string("key_fingerprint"),
        reasoning_effort: string("reasoning_effort"),
        thinking_budget: row["thinking_budget"].as_i64(),
        ..Default::default()
    };
    Ok(alex_core::TraceIngestPayload {
        trace,
        request_body_b64: body("req_body_path")?,
        upstream_request_body_b64: body("upstream_req_body_path")?,
        response_body_b64: body("resp_body_path")?,
    })
}

fn queue_remote_trace_update(remote: &RemoteTraceSender, store: &Store, trace_id: &str) {
    let result =
        trace_ingest_payload_from_store(store, trace_id).and_then(|payload| remote.send(payload));
    if let Err(error) = result {
        eprintln!(
            "alex wrap: could not queue remote trace {trace_id}: {error:#}; it remains in the local spool"
        );
    }
}

fn write_gzip_json_atomic(path: &std::path::Path, value: &serde_json::Value) -> Result<()> {
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::io::Write;

    let parent = path
        .parent()
        .with_context(|| format!("trace body has no parent: {}", path.display()))?;
    std::fs::create_dir_all(parent)?;
    let file_name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("body.gz");
    let tmp = parent.join(format!(
        ".{file_name}.{}-{:08x}.tmp",
        std::process::id(),
        rand::thread_rng().gen::<u32>()
    ));
    let result = (|| -> Result<()> {
        let file = std::fs::File::create(&tmp)
            .with_context(|| format!("create temporary trace body {}", tmp.display()))?;
        let mut encoder = GzEncoder::new(file, Compression::default());
        encoder.write_all(serde_json::to_string_pretty(value)?.as_bytes())?;
        let file = encoder.finish()?;
        file.sync_all()?;
        std::fs::rename(&tmp, path).with_context(|| {
            format!(
                "replace compressed trace body {} with {}",
                path.display(),
                tmp.display()
            )
        })?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AgentReconcileOutcome {
    Inserted,
    Updated,
    Unchanged,
}

fn reconcile_agent_turn(
    store: &Store,
    run_id: &str,
    transcript_id: &str,
    turn_index: usize,
    ts_ms: i64,
    turn: &AgentTranscriptTurn,
    dry_run: bool,
    metadata: &CursorTraceMetadata,
) -> Result<AgentReconcileOutcome> {
    let trace_id = format!("agent-{transcript_id}-{turn_index}");
    let (req, resp) =
        agent_trace_bodies_for_model(transcript_id, turn_index, turn, &metadata.model);
    if let Some(existing) = store.get_trace(&trace_id)? {
        let expected_account_id = metadata
            .billing_account
            .as_ref()
            .map(|account| account.account_id.as_str());
        let expected_subscription_identity = metadata
            .billing_account
            .as_ref()
            .and_then(|account| account.subscription_identity.as_deref());
        let metadata_matches = existing["requested_model"].as_str() == Some(&metadata.model)
            && existing["routed_model"].as_str() == Some(&metadata.model)
            && existing["billing_bucket"].as_str() == Some("cursor")
            && expected_account_id
                .is_none_or(|expected| existing["account_id"].as_str() == Some(expected))
            && expected_subscription_identity.is_none_or(|expected| {
                existing["subscription_identity"].as_str() == Some(expected)
            });
        let req_path = existing["req_body_path"]
            .as_str()
            .map(std::path::PathBuf::from);
        let resp_path = existing["resp_body_path"]
            .as_str()
            .map(std::path::PathBuf::from);
        let req_matches = req_path
            .as_deref()
            .and_then(|path| read_gzip_json(path).ok())
            .as_ref()
            == Some(&req);
        let resp_matches = resp_path
            .as_deref()
            .and_then(|path| read_gzip_json(path).ok())
            .as_ref()
            == Some(&resp);
        if req_matches && resp_matches && metadata_matches {
            return Ok(AgentReconcileOutcome::Unchanged);
        }
        if !dry_run {
            if !metadata_matches {
                if let Some(account) = &metadata.billing_account {
                    store.upsert_known_account(account)?;
                }
                store.update_trace_billing_metadata(
                    &trace_id,
                    &metadata.model,
                    &metadata.model,
                    "cursor",
                    expected_account_id,
                    expected_subscription_identity,
                )?;
            }
            if !req_matches {
                let path = req_path.with_context(|| {
                    format!("existing Agent trace {trace_id} has no request body path")
                })?;
                write_gzip_json_atomic(&path, &req)?;
            }
            if !resp_matches {
                let path = resp_path.with_context(|| {
                    format!("existing Agent trace {trace_id} has no response body path")
                })?;
                write_gzip_json_atomic(&path, &resp)?;
            }
        }
        return Ok(AgentReconcileOutcome::Updated);
    }

    if dry_run {
        return Ok(AgentReconcileOutcome::Inserted);
    }
    let req_path = Some(store.write_body(
        &trace_id,
        "request.json",
        serde_json::to_string_pretty(&req)?.as_bytes(),
    )?);
    let resp_path = Some(store.write_body(
        &trace_id,
        "response.body",
        serde_json::to_string_pretty(&resp)?.as_bytes(),
    )?);
    if let Some(account) = &metadata.billing_account {
        store.upsert_known_account(account)?;
    }
    let tags = serde_json::json!({"harness":"agent","wrap":"agent","source":"cursor-agent-transcript","stream":"dialogue"}).to_string();
    let rec = alex_core::TraceRecord {
        id: trace_id,
        ts_request_ms: ts_ms + turn_index as i64,
        ts_response_ms: Some(ts_ms + turn_index as i64),
        session_id: Some(transcript_id.to_string()),
        harness: Some("agent".into()),
        client_format: Some("openai-chat".into()),
        upstream_provider: Some("cursor".into()),
        upstream_format: Some("openai-chat".into()),
        requested_model: Some(metadata.model.clone()),
        routed_model: Some(metadata.model.clone()),
        method: Some("TRANSCRIPT".into()),
        path: Some("cursor-agent-transcript".into()),
        status: Some(200),
        streamed: Some(true),
        usage: alex_core::Usage::default(),
        cost_usd: None,
        billing_bucket: Some("cursor".into()),
        req_body_path: req_path,
        upstream_req_body_path: None,
        resp_body_path: resp_path,
        req_headers_json: Some(serde_json::json!({"x-alexandria-wrap":"agent","x-alexandria-run-id":run_id}).to_string()),
        resp_headers_json: Some(serde_json::json!({"x-alexandria-source":"cursor-agent-transcript","content-type":"application/json"}).to_string()),
        error: None,
        account_id: metadata
            .billing_account
            .as_ref()
            .map(|account| account.account_id.clone()),
        subscription_identity: metadata
            .billing_account
            .as_ref()
            .and_then(|account| account.subscription_identity.clone()),
        via_dario: false,
        dario_generation: None,
        run_id: Some(run_id.to_string()),
        tags: Some(tags),
        client_ip: None,
        key_fingerprint: None,
        reasoning_effort: None,
        thinking_budget: None,
    };
    store.insert_trace(&rec)?;
    Ok(AgentReconcileOutcome::Inserted)
}

#[derive(Debug, Serialize)]
struct AgentRepairReport {
    transcript_id: String,
    turns: usize,
    inserted: usize,
    updated: usize,
    unchanged: usize,
    dry_run: bool,
}

fn validate_agent_transcript_id(id: &str) -> Result<()> {
    if id.is_empty()
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        anyhow::bail!("invalid Cursor Agent transcript id: {id}");
    }
    Ok(())
}

async fn traces_repair_amp_cmd(
    config: &Config,
    run_id: Option<&str>,
    json_out: bool,
) -> Result<()> {
    let capture_path = alex_wrap::capture_dir_for(&config.data_dir, "amp").join("ws.jsonl");
    if !capture_path.exists() {
        anyhow::bail!(
            "Amp websocket capture not found: {}",
            capture_path.display()
        );
    }
    let run_id = run_id
        .map(String::from)
        .unwrap_or_else(|| format!("repair-amp-{}", now_ms()));
    let tags = serde_json::json!({
        "harness": "amp",
        "wrap": "amp",
        "source": "alex-wrap-ws-repair",
        "stream": "dialogue",
    })
    .to_string();
    let vault = open_vault(config)?;
    let _ = alex_auth::import_amp(&vault).await;
    let billing_account = vault
        .list()
        .await
        .into_iter()
        .find(|account| account.provider == alex_core::Provider::Amp && account.status == "active")
        .map(|account| known_account(&account));
    let mut state = AmpWsTraceState::new(
        config.data_dir.clone(),
        run_id.clone(),
        tags,
        "amp-repair".into(),
        None,
    );
    state.billing_account = billing_account;
    let mut offset = 0;
    state.ingest_new(&capture_path, &mut offset)?;
    let result = serde_json::json!({
        "run_id": run_id,
        "capture_path": capture_path,
        "records_imported": state.inserted.len(),
        "bytes_read": offset,
    });
    if json_out {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        println!(
            "{} imported {} Amp trace record(s) from {}",
            ui::green(ui::dot()),
            state.inserted.len(),
            capture_path.display()
        );
    }
    Ok(())
}

fn repair_agent_transcript(
    data_dir: &std::path::Path,
    transcript_root: &std::path::Path,
    transcript_id: &str,
    dry_run: bool,
) -> Result<AgentRepairReport> {
    validate_agent_transcript_id(transcript_id)?;
    let path = transcript_root
        .join(transcript_id)
        .join(format!("{transcript_id}.jsonl"));
    let meta = std::fs::metadata(&path)
        .with_context(|| format!("Cursor Agent transcript not found: {}", path.display()))?;
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or_else(now_ms);
    let turns = parse_agent_transcript_turns_strict(&std::fs::read_to_string(&path)?)?;
    let store = Store::open(data_dir.to_path_buf())?;
    let existing = store.session_traces(transcript_id, None)?;
    let run_id = existing
        .iter()
        .find_map(|row| row["run_id"].as_str())
        .map(String::from)
        .unwrap_or_else(|| format!("repair-agent-{}", now_ms()));
    let mut report = AgentRepairReport {
        transcript_id: transcript_id.to_string(),
        turns: turns.len(),
        inserted: 0,
        updated: 0,
        unchanged: 0,
        dry_run,
    };
    let metadata = cursor_trace_metadata(&[]);
    for (offset, turn) in turns.iter().enumerate() {
        match reconcile_agent_turn(
            &store,
            &run_id,
            transcript_id,
            offset + 1,
            modified,
            turn,
            dry_run,
            &metadata,
        )? {
            AgentReconcileOutcome::Inserted => report.inserted += 1,
            AgentReconcileOutcome::Updated => report.updated += 1,
            AgentReconcileOutcome::Unchanged => report.unchanged += 1,
        }
    }
    Ok(report)
}

fn traces_repair_agent_cmd(
    config: &Config,
    transcript_id: &str,
    dry_run: bool,
    json_out: bool,
) -> Result<()> {
    let root = cursor_agent_transcript_root()?;
    let report = repair_agent_transcript(&config.data_dir, &root, transcript_id, dry_run)?;
    if json_out {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "{} Agent transcript {}: {} turn(s), {} inserted, {} updated, {} unchanged{}",
            ui::green(ui::dot()),
            ui::bold(&report.transcript_id),
            report.turns,
            report.inserted,
            report.updated,
            report.unchanged,
            if report.dry_run { " (dry run)" } else { "" }
        );
    }
    Ok(())
}

async fn traces_push_cmd(
    config: &Config,
    run_id: &str,
    remote_args: &RemoteTraceArgs,
) -> Result<()> {
    let remote = resolve_remote_trace_config(remote_args)?
        .context("traces push requires --trace-url / ALEXANDRIA_TRACE_URL")?;
    preflight_remote_trace(&remote).await?;
    let store = Store::open(config.data_dir.clone())?;
    let trace_ids = store.run_trace_ids(run_id)?;
    if trace_ids.is_empty() {
        anyhow::bail!("no local traces found for run '{run_id}'");
    }
    let worker = RemoteTraceWorker::start(remote);
    let sender = worker.sender();
    for trace_id in &trace_ids {
        sender.send(trace_ingest_payload_from_store(&store, trace_id)?)?;
    }
    drop(sender);
    let report = worker.stop().await?;
    if report.failed > 0 {
        anyhow::bail!(
            "pushed {} trace(s), but {} failed and remain in the local spool: {}",
            report.uploaded,
            report.failed,
            report.failures.join("; ")
        );
    }
    println!(
        "{} pushed {} trace(s) from run {}",
        ui::green(ui::dot()),
        report.uploaded,
        ui::bold(run_id)
    );
    Ok(())
}

async fn wrap_status_cmd(config: &Config, json_out: bool) -> Result<()> {
    let catalog = alex_wrap::load_catalog()?;
    let mut body = alex_wrap::status_json(&catalog);
    // Enrich with vault account presence per harness.
    let vault = open_vault(config)?;
    let accounts = vault.list().await;
    if let Some(obj) = body.get_mut("harnesses").and_then(|h| h.as_object_mut()) {
        for (id, entry) in obj.iter_mut() {
            let provider = entry
                .pointer("/credentials/vault_provider")
                .and_then(|v| v.as_str())
                .unwrap_or(id);
            let vault_id = accounts
                .iter()
                .find(|a| a.status == "active" && a.provider.as_str() == provider)
                .map(|a| a.id.clone());
            if let Some(map) = entry.as_object_mut() {
                map.insert("vault_account".into(), serde_json::json!(vault_id));
            }
        }
    }
    if json_out {
        println!("{}", serde_json::to_string_pretty(&body)?);
        return Ok(());
    }
    println!("{}", ui::section("wrap status"));
    println!(
        "  catalog: {}",
        body["catalog_source"].as_str().unwrap_or("?")
    );
    let Some(harnesses) = body["harnesses"].as_object() else {
        return Ok(());
    };
    for (id, h) in harnesses {
        let enabled = h["enabled"].as_bool().unwrap_or(true);
        let modes: Vec<&str> = h["modes"]
            .as_array()
            .map(|a| a.iter().filter_map(|m| m["id"].as_str()).collect())
            .unwrap_or_default();
        let vault = h["vault_account"].as_str().unwrap_or("(none)");
        let cred_ok = h
            .pointer("/credentials/resolved")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        println!(
            "  {} {}  modes=[{}]  vault={}  secrets={}",
            if enabled {
                ui::green(ui::dot())
            } else {
                ui::dim(ui::circle())
            },
            ui::bold(id),
            modes.join(", "),
            vault,
            if cred_ok { "ok" } else { "missing" }
        );
        if let Some(desc) = h["description"].as_str() {
            println!("      {}", ui::dim(desc));
        }
    }
    println!(
        "  try:  alex wrap env amp [--mode base_url|env_proxy]\n        alex wrap smoke --json"
    );
    println!(
        "  override catalog: {}",
        alex_wrap::user_catalog_override_path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(none)".into())
    );
    Ok(())
}

async fn wrap_env_cmd(
    config: &Config,
    harness: &str,
    mode: Option<&str>,
    wrap_url: &str,
    ca_cert: Option<PathBuf>,
    json_out: bool,
) -> Result<()> {
    let catalog = alex_wrap::load_catalog()?;
    let vault = open_vault(config)?;
    // Prefer vault key when catalog names a vault provider.
    let vault_key = {
        let resolved = catalog.resolve(harness);
        if let Some((_, h)) = resolved {
            if let Some(creds) = &h.credentials {
                let provider = creds.vault_provider.as_deref().unwrap_or(harness);
                vault
                    .list()
                    .await
                    .into_iter()
                    .find(|a| a.provider.as_str() == provider && a.status == "active")
                    .and_then(|a| a.api_key.or(a.access_token))
            } else {
                None
            }
        } else {
            None
        }
    };
    let capture = alex_wrap::capture_dir_for(&config.data_dir, harness);
    let (_id, plan) = alex_wrap::plan_for(
        &catalog, harness, mode, wrap_url, &capture, vault_key, ca_cert,
    )?;
    if json_out {
        println!("{}", serde_json::to_string_pretty(&plan.summary_json())?);
        return Ok(());
    }
    for line in plan.export_lines() {
        println!("{line}");
    }
    if let Some(path) = &plan.settings_path {
        println!("# settings written: {}", path.display());
    }
    println!("# mode={} role={:?}", plan.mode_id, plan.wrap_role);
    if let Some(notes) = &plan.notes {
        println!("# note: {notes}");
    }
    println!(
        "# then: {} {}",
        plan.binary,
        plan.argv_suffix
            .iter()
            .map(|s| shell_quote(s))
            .collect::<Vec<_>>()
            .join(" ")
    );
    println!("# wrap must listen at {}", plan.wrap_base_url);
    Ok(())
}

fn shell_quote(s: &str) -> String {
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || "-_./:@".contains(c))
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r#"'"'"'"#))
    }
}

async fn wrap_smoke_cmd(harness: &str, json_out: bool) -> Result<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let catalog = alex_wrap::load_catalog()?;
    let (_, h) = catalog
        .resolve(harness)
        .with_context(|| format!("unknown harness '{harness}' for smoke"))?;
    let policy = h.capture.clone();

    let upstream = TcpListener::bind("127.0.0.1:0").await?;
    let up_addr = upstream.local_addr()?;
    let up = tokio::spawn(async move {
        let (mut sock, _) = upstream.accept().await?;
        let mut buf = vec![0u8; 4096];
        let n = sock.read(&mut buf).await?;
        let req = String::from_utf8_lossy(&buf[..n]);
        anyhow::ensure!(
            req.contains("GET /api/internal?getUserInfo"),
            "unexpected request: {req}"
        );
        let body = br#"{"ok":true,"result":{"user":"smoke"}}"#;
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        sock.write_all(resp.as_bytes()).await?;
        sock.write_all(body).await?;
        Ok::<(), anyhow::Error>(())
    });

    let log = alex_wrap::CaptureLog::with_policy(policy);
    let wrap = alex_wrap::ReverseWrap::start_http_to_http(
        "127.0.0.1:0".parse().unwrap(),
        up_addr,
        log.clone(),
    )
    .await?;

    let mut client = tokio::net::TcpStream::connect(wrap.listen_addr).await?;
    client
        .write_all(
            b"GET /api/internal?getUserInfo HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .await?;
    let mut resp = Vec::new();
    client.read_to_end(&mut resp).await?;
    let resp_s = String::from_utf8_lossy(&resp);
    anyhow::ensure!(resp_s.contains("200 OK"), "bad response: {resp_s}");
    anyhow::ensure!(
        log.paths()
            .iter()
            .any(|p| p.contains("/api/internal?getUserInfo")),
        "path not captured: {:?}",
        log.paths()
    );

    // Plan resolution smoke (no network): ensure catalog templates expand.
    let dir = std::env::temp_dir().join(format!(
        "alex-wrap-smoke-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir)?;
    let (_id, plan) = alex_wrap::plan_for(
        &catalog,
        harness,
        Some("base_url"),
        &wrap.base_url(),
        &dir,
        Some("sgamp_smoke".into()),
        None,
    )?;
    anyhow::ensure!(plan.env.contains_key("AMP_URL") || harness != "amp");
    let _ = std::fs::remove_dir_all(&dir);

    wrap.shutdown().await;
    up.await??;

    let summary = serde_json::json!({
        "ok": true,
        "harness": harness,
        "captured_paths": log.paths(),
        "events": log.events().len(),
        "plan": plan.summary_json(),
    });
    if json_out {
        println!("{}", serde_json::to_string_pretty(&summary)?);
    } else {
        println!(
            "{} wrap smoke ok — harness={} captured {} event(s) mode={}",
            ui::green(ui::dot()),
            harness,
            log.events().len(),
            plan.mode_id
        );
        for p in log.paths() {
            println!("  {p}");
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

fn known_account(account: &alex_auth::Account) -> KnownAccount {
    KnownAccount::new(
        account.id.clone(),
        account.provider.as_str(),
        account.name.clone(),
        account.kind.clone(),
        account.subscription_identity(),
        account.email(),
    )
}

async fn traces_reattach_cmd(
    config: &Config,
    orphan_account_id: Option<&str>,
    to_account_id: Option<&str>,
    confirmed: bool,
    json: bool,
) -> Result<()> {
    let store = Store::open(config.data_dir.clone())?;
    // The command is offline, so make current vault accounts visible to the
    // catalogue before resolving the requested target.
    let vault = open_vault(config)?;
    let accounts = vault.list().await;
    for account in &accounts {
        store.upsert_known_account(&known_account(account))?;
    }
    let groups = store.orphaned_trace_groups()?;
    let display_time = |value: &serde_json::Value| {
        value
            .as_i64()
            .and_then(chrono::DateTime::<chrono::Utc>::from_timestamp_millis)
            .map(|time| time.to_rfc3339())
            .unwrap_or_else(|| "unknown".into())
    };
    match (orphan_account_id, to_account_id) {
        (None, None) => {
            if json {
                println!("{}", serde_json::to_string_pretty(&groups)?);
            } else if groups.is_empty() {
                println!("no orphaned legacy trace groups");
            } else {
                for group in groups {
                    println!(
                        "{}  provider={}  models={}  {} traces  {}..{}",
                        group["account_id"].as_str().unwrap_or("unknown"),
                        group["provider"].as_str().unwrap_or("unknown"),
                        group["models"].as_str().unwrap_or("unknown"),
                        group["count"],
                        display_time(&group["first_ts_ms"]),
                        display_time(&group["last_ts_ms"])
                    );
                }
            }
            Ok(())
        }
        (Some(orphan), Some(target_id)) => {
            let group = groups.iter().find(|g| g["account_id"].as_str() == Some(orphan))
                .context("orphan group not found (only untagged, unresolved legacy traces can be reattached)")?;
            let target = accounts
                .into_iter()
                .find(|a| a.id == target_id)
                .context("target must be an existing vault account")?;
            let target = known_account(&target);
            let identity = target
                .subscription_identity
                .clone()
                .context("target account has no durable subscription identity")?;
            let plan = serde_json::json!({"orphan_account_id": orphan, "trace_count": group["count"],
                "to_account_id": target.account_id, "subscription_identity": identity, "confirmed": confirmed});
            // Always render the plan before mutation. This also makes --yes
            // auditable in shell logs.
            if json {
                println!("{}", serde_json::to_string_pretty(&plan)?);
            } else {
                println!(
                    "would attach {} traces from {} to {} ({})",
                    group["count"], orphan, target.account_id, identity
                );
            }
            if !confirmed {
                if !json {
                    println!("no changes made; rerun with --yes to apply");
                }
                return Ok(());
            }
            let changed = store.reattach_orphaned_traces(orphan, &target, true)?;
            if !json {
                println!("reattached {changed} traces");
            }
            Ok(())
        }
        _ => anyhow::bail!("use both --orphan-account-id and --to-account-id, or neither to list"),
    }
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
    kind: &str,
    run_id: Option<String>,
    tags: &[String],
    ttl: &str,
    label: Option<String>,
    json_out: bool,
) -> Result<()> {
    if !matches!(kind, "run" | "harness" | "wrap") {
        anyhow::bail!("invalid --kind '{kind}' (use run, harness, or wrap)");
    }
    let ttl_seconds = parse_ttl_seconds(ttl)
        .with_context(|| format!("invalid --ttl '{ttl}' (use seconds or 45s, 30m, 24h, 7d)"))?;
    let tag_refs: Vec<&str> = tags.iter().map(String::as_str).collect();
    let tag_values = alex_core::parse_trace_tags(&tag_refs);
    let mut body = serde_json::json!({"kind": kind, "ttl_seconds": ttl_seconds});
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
    if json_out {
        println!("{}", serde_json::to_string_pretty(&resp)?);
        return Ok(());
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
    if kind == "wrap" {
        println!("{}", ui::dim("shown once — configure the remote wrapper:"));
        println!("export ALEXANDRIA_TRACE_URL={}", config.base_url());
        println!("export ALEXANDRIA_TRACE_KEY={key}");
    } else {
        println!(
            "{}",
            ui::dim("shown once — inject into the harness env (any of):")
        );
        println!("export ANTHROPIC_API_KEY={key}");
        println!("export OPENAI_API_KEY={key}");
        println!("export XAI_API_KEY={key}");
    }
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
        "{} {} {} {} {} {} {}",
        ui::pad_right(&ui::column_header("id"), 12),
        ui::pad_right(&ui::column_header("kind"), 8),
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
            "{} {} {} {} {} {} {}",
            ui::pad_right(&ui::amber(r["id"].as_str().unwrap_or("-")), 12),
            ui::pad_right(&ui::sand(r["kind"].as_str().unwrap_or("run")), 8),
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
        let quota = &p["quota"];
        let quota_kind = quota["kind"].as_str().unwrap_or("rate_window");
        let credit_primary = quota_kind != "rate_window";
        match quota_kind {
            "out_of_credits" => {
                println!("   {}", ui::red("OUT OF CREDITS"));
                if let Some(url) = quota["top_up_url"].as_str().filter(|url| !url.is_empty()) {
                    println!("   {}", ui::sand(&format!("top up: {url}")));
                }
            }
            "unlimited" => println!("   {}", ui::green("Unlimited credits")),
            "balance" => {
                let balance = quota["balance"].as_str().unwrap_or("-");
                println!("   {}", ui::green(&format!("Credit balance: {balance}")));
            }
            "credit_window" => {
                let remaining = quota["remaining_pct"].as_f64().unwrap_or(0.0);
                println!(
                    "   {}  {}  {:>3.0}% remaining",
                    quota["label"].as_str().unwrap_or("Credit quota"),
                    ui::gauge(100.0 - remaining, 24),
                    remaining,
                );
            }
            _ => {}
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
            let raw_label = w["window"].as_str().unwrap_or("-");
            // The Grok billing window is already printed as the primary credit
            // quota. Amp's paid balances are likewise represented above.
            if quota_kind == "credit_window" || (credit_primary && raw_label == "credits") {
                continue;
            }
            // Amp paid balances: show dollars remaining instead of an empty % bar.
            if let Some(usd) = w["remaining_usd"].as_f64() {
                let label = raw_label;
                if label == "credits" || label.starts_with("ws:") {
                    println!(
                        "   {}  {}",
                        ui::pad_right(label, label_width),
                        ui::sand(&format!("${usd:.2} remaining"))
                    );
                    printed = true;
                    continue;
                }
            }
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
                ui::pad_right(
                    &if credit_primary && !raw_label.starts_with("ws:") {
                        format!("rate {raw_label}")
                    } else {
                        raw_label.into()
                    },
                    label_width + usize::from(credit_primary && !raw_label.starts_with("ws:")) * 5,
                ),
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
    let available = body["available"].as_bool().unwrap_or(false);
    let route_enabled = body["route_enabled"].as_bool().unwrap_or(false);
    println!(
        "available: {}  routing: {}",
        if available {
            ui::green("yes")
        } else {
            ui::red("no")
        },
        if route_enabled {
            ui::green("dario")
        } else {
            ui::dim("direct")
        }
    );
    if let (Some(runtime), Some(version)) =
        (body["runtime"].as_str(), body["runtime_version"].as_str())
    {
        println!("runtime: {runtime} {version}");
    }
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
    let base = daemon_connect_base_url(host, port);
    eprintln!("{}", ui::divider("alexandria"));
    eprintln!(
        "daemon listening on {}",
        ui::bold(&ui::lapis(&format!("http://{host}:{port}")))
    );
    eprintln!("  health:   {}", ui::lapis(&format!("{base}/health")));
    eprintln!("  traces:   {}", ui::lapis(&format!("{base}/admin/traces")));
    eprintln!(
        "  accounts: {}",
        ui::lapis(&format!("{base}/admin/accounts"))
    );
    eprintln!();
    print_env(host, port, local_key);
}

fn print_env(host: &str, port: u16, local_key: &str) {
    let base = daemon_connect_base_url(host, port);
    eprintln!(
        "{}",
        ui::dim("# anthropic-format harnesses (claude-code, …)")
    );
    eprintln!("export ANTHROPIC_BASE_URL={base}");
    eprintln!("export ANTHROPIC_API_KEY={local_key}");
    eprintln!("{}", ui::dim("# openai-format harnesses (codex, pi, …)"));
    eprintln!("export OPENAI_BASE_URL={base}/v1");
    eprintln!("export OPENAI_API_KEY={local_key}");
    eprintln!("{}", ui::dim("# xai/grok harnesses"));
    eprintln!("export XAI_API_KEY={local_key}");
    eprintln!("export GROK_MODELS_BASE_URL={base}/v1");
    eprintln!(
        "{}",
        ui::dim("# gemini-cli (needs security.auth.selectedType=gemini-api-key)")
    );
    eprintln!("export GOOGLE_GEMINI_BASE_URL={base}");
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

async fn ping_dario_daemon(config: &Config) -> alex_proxy::PingResult {
    let started = now_ms();
    let endpoint = format!(
        "{}/admin/dario/ping",
        config.base_url().trim_end_matches('/')
    );
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(35))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            return alex_proxy::PingResult {
                provider: "dario",
                account_id: None,
                ok: false,
                status: None,
                latency_ms: now_ms() - started,
                message: error.to_string(),
            }
        }
    };
    match client
        .post(endpoint)
        .header("x-api-key", &config.local_key)
        .send()
        .await
    {
        Ok(response) => {
            let status = response.status().as_u16();
            let body = response.bytes().await.unwrap_or_default();
            serde_json::from_slice::<serde_json::Value>(&body)
                .ok()
                .and_then(|value| value.get("through_dario").cloned())
                .and_then(|value| {
                    Some(alex_proxy::PingResult {
                        provider: "dario",
                        account_id: value["account_id"].as_str().map(String::from),
                        ok: value["ok"].as_bool()?,
                        status: value["status"].as_u64().map(|status| status as u16),
                        latency_ms: value["latency_ms"].as_i64()?,
                        message: value["message"].as_str()?.to_string(),
                    })
                })
                .unwrap_or_else(|| alex_proxy::PingResult {
                    provider: "dario",
                    account_id: None,
                    ok: false,
                    status: Some(status),
                    latency_ms: now_ms() - started,
                    message: String::from_utf8_lossy(&body).chars().take(200).collect(),
                })
        }
        Err(error) => alex_proxy::PingResult {
            provider: "dario",
            account_id: None,
            ok: false,
            status: None,
            latency_ms: now_ms() - started,
            message: format!("dario ping could not reach daemon: {error}"),
        },
    }
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
        alex_core::Provider::Openrouter => models.openrouter.clone(),
        alex_core::Provider::Amp => "amp".to_string(),
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

const LAUNCHD_LABEL: &str = "com.alexandria.daemon";
const LAUNCHD_HEALTH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
const LAUNCHD_HEALTH_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(300);

#[derive(Debug, PartialEq)]
enum LaunchdInstallMode {
    Bootstrap,
    UpdatedRestartRequired,
}

fn launchd_install_mode(loaded: bool) -> LaunchdInstallMode {
    if loaded {
        LaunchdInstallMode::UpdatedRestartRequired
    } else {
        LaunchdInstallMode::Bootstrap
    }
}

#[derive(Debug, PartialEq)]
enum LaunchdRestartOutcome {
    Replaced,
    RefusedInFlight { status: InFlightStatus },
    RolledBack { cause: String },
    Failed { cause: String },
}

trait LaunchctlControl {
    fn is_loaded(&mut self) -> Result<bool>;
    fn bootout(&mut self, plist: &Path) -> Result<()>;
    fn bootstrap(&mut self, plist: &Path) -> Result<()>;
}

trait LaunchdHealthProbe {
    fn in_flight(&mut self) -> Result<InFlightStatus>;
    fn wait_until_healthy(&mut self) -> bool;
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct InFlightRequestSummary {
    age_s: i64,
    model: String,
    session_id: Option<String>,
    harness: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InFlightStatus {
    count: i64,
    requests: Vec<InFlightRequestSummary>,
}

trait LaunchdPlistRollback {
    fn restore_previous(&mut self) -> Result<()>;
}

struct SystemLaunchctl {
    domain: String,
}

impl SystemLaunchctl {
    fn new() -> Self {
        Self {
            domain: format!("gui/{}", current_uid()),
        }
    }

    fn run(&self, action: &str, plist: &Path) -> Result<()> {
        let output = std::process::Command::new("launchctl")
            .args([action, &self.domain])
            .arg(plist)
            .output()
            .with_context(|| format!("running launchctl {action}"))?;
        if output.status.success() {
            Ok(())
        } else {
            anyhow::bail!(
                "launchctl {action} failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
    }
}

impl LaunchctlControl for SystemLaunchctl {
    fn is_loaded(&mut self) -> Result<bool> {
        let output = std::process::Command::new("launchctl")
            .args(["print", &format!("{}/{LAUNCHD_LABEL}", self.domain)])
            .output()
            .context("running launchctl print")?;
        Ok(output.status.success())
    }

    fn bootout(&mut self, plist: &Path) -> Result<()> {
        self.run("bootout", plist)
    }

    fn bootstrap(&mut self, plist: &Path) -> Result<()> {
        self.run("bootstrap", plist)
    }
}

struct LocalLaunchdHealthProbe {
    client: reqwest::blocking::Client,
    base_url: String,
    local_key: String,
}

impl LocalLaunchdHealthProbe {
    fn new(config: &Config) -> Result<Self> {
        Ok(Self {
            client: reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(2))
                .build()
                .context("building daemon health client")?,
            base_url: config.base_url(),
            local_key: config.local_key.clone(),
        })
    }

    fn health(&self) -> Result<serde_json::Value> {
        let response = self
            .client
            .get(format!("{}/health", self.base_url))
            .header("x-api-key", &self.local_key)
            .send()
            .context("querying daemon /health")?;
        if !response.status().is_success() {
            anyhow::bail!("daemon /health returned {}", response.status());
        }
        response.json().context("parsing daemon /health response")
    }
}

impl LaunchdHealthProbe for LocalLaunchdHealthProbe {
    fn in_flight(&mut self) -> Result<InFlightStatus> {
        let health = self.health()?;
        let count = health
            .get("in_flight")
            .and_then(serde_json::Value::as_i64)
            .context("daemon /health did not include an in_flight count")?;
        if count < 0 {
            anyhow::bail!("daemon /health returned an invalid in_flight count ({count})");
        }
        let requests = serde_json::from_value(
            health
                .get("in_flight_requests")
                .cloned()
                .unwrap_or_else(|| serde_json::Value::Array(Vec::new())),
        )
        .context("daemon /health returned invalid in-flight request details")?;
        Ok(InFlightStatus { count, requests })
    }

    fn wait_until_healthy(&mut self) -> bool {
        let deadline = std::time::Instant::now() + LAUNCHD_HEALTH_TIMEOUT;
        loop {
            if self
                .health()
                .map(|health| {
                    health.get("status").and_then(serde_json::Value::as_str) == Some("ok")
                        && health.get("service").and_then(serde_json::Value::as_str)
                            == Some("alexandria")
                })
                .unwrap_or(false)
            {
                return true;
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(LAUNCHD_HEALTH_POLL_INTERVAL);
        }
    }
}

struct FilesystemLaunchdRollback<'a> {
    destination: &'a Path,
    previous: &'a Path,
}

impl LaunchdPlistRollback for FilesystemLaunchdRollback<'_> {
    fn restore_previous(&mut self) -> Result<()> {
        let plist = std::fs::read(self.previous)
            .with_context(|| format!("reading saved previous plist {}", self.previous.display()))?;
        std::fs::write(self.destination, plist)
            .with_context(|| format!("restoring previous plist {}", self.destination.display()))
    }
}

/// Replace a loaded job only after the daemon reports that it is idle.  The
/// launchctl and health operations are deliberately traits so this sequence is
/// testable on platforms that do not have launchd.
fn replace_loaded_launchd_service<C, H, F>(
    launchctl: &mut C,
    health: &mut H,
    rollback: &mut F,
    plist: &Path,
    force: bool,
    mut force_warning: Option<&mut dyn FnMut(&InFlightStatus)>,
) -> LaunchdRestartOutcome
where
    C: LaunchctlControl,
    H: LaunchdHealthProbe,
    F: LaunchdPlistRollback,
{
    let in_flight = match health.in_flight() {
        Ok(status) => status,
        Err(error) => {
            return LaunchdRestartOutcome::Failed {
                cause: format!("could not determine in-flight routed requests: {error:#}"),
            }
        }
    };
    if in_flight.count > 0 && !force {
        return LaunchdRestartOutcome::RefusedInFlight { status: in_flight };
    }
    if in_flight.count > 0 {
        if let Some(warn) = force_warning.as_mut() {
            warn(&in_flight);
        }
    }

    if let Err(error) = launchctl.bootout(plist) {
        return LaunchdRestartOutcome::Failed {
            cause: format!("could not boot out the loaded daemon: {error:#}"),
        };
    }

    let failure = match launchctl.bootstrap(plist) {
        Ok(()) if health.wait_until_healthy() => return LaunchdRestartOutcome::Replaced,
        Ok(()) => "the new daemon did not become healthy before the timeout".to_string(),
        Err(error) => format!("bootstrapping the new daemon failed: {error:#}"),
    };

    // A successful bootstrap can still leave a broken job loaded.  Try to
    // remove it before re-bootstrapping the previous plist.  A failed cleanup
    // is not fatal by itself: the old bootstrap and health probe are the
    // observable evidence that recovery succeeded.
    let cleanup_error = launchctl.bootout(plist).err();
    if let Err(error) = rollback.restore_previous() {
        return LaunchdRestartOutcome::Failed {
            cause: format!("{failure}; could not restore the previous plist: {error:#}"),
        };
    }
    if let Err(error) = launchctl.bootstrap(plist) {
        let cleanup = cleanup_error
            .map(|error| format!("; cleanup also failed: {error:#}"))
            .unwrap_or_default();
        return LaunchdRestartOutcome::Failed {
            cause: format!("{failure}; rollback bootstrap failed: {error:#}{cleanup}"),
        };
    }
    if !health.wait_until_healthy() {
        return LaunchdRestartOutcome::Failed {
            cause: format!("{failure}; rollback daemon did not become healthy before the timeout"),
        };
    }
    LaunchdRestartOutcome::RolledBack { cause: failure }
}

fn launchd_plist_path() -> Result<PathBuf> {
    Ok(dirs::home_dir()
        .context("no home dir")?
        .join("Library/LaunchAgents")
        .join(format!("{LAUNCHD_LABEL}.plist")))
}

fn launchd_previous_plist_path(plist: &Path) -> PathBuf {
    plist.with_extension("plist.previous")
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn render_launchd_path(inherited_path: &str, known_dirs: &[PathBuf]) -> String {
    let mut dirs: Vec<String> = inherited_path
        .split(':')
        .filter(|dir| !dir.is_empty())
        .map(str::to_owned)
        .collect();
    for dir in known_dirs {
        let dir = dir.to_string_lossy().to_string();
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    dirs.join(":")
}

fn known_launchd_path_dirs(exe: &Path) -> Vec<PathBuf> {
    let mut dirs = exe
        .parent()
        .map(Path::to_path_buf)
        .into_iter()
        .collect::<Vec<_>>();
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    dirs.extend([
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/opt/homebrew/bin"),
    ]);
    if let Some(home) = dirs::home_dir() {
        dirs.extend([
            home.join(".local/bin"),
            home.join(".cargo/bin"),
            home.join(".bun/bin"),
            home.join("Library/pnpm"),
            home.join(".nvm/current/bin"),
        ]);
    }
    dirs.retain(|dir| dir.is_dir());
    dirs.sort();
    dirs.dedup();
    dirs
}

fn render_launchd_plist(exe: &Path, inherited_path: &str, known_dirs: &[PathBuf]) -> String {
    LAUNCHD_TEMPLATE
        .replace(
            "/usr/local/bin/alexandria",
            &xml_escape(&exe.to_string_lossy()),
        )
        .replace(
            "__ALEX_LAUNCHD_PATH__",
            &xml_escape(&render_launchd_path(inherited_path, known_dirs)),
        )
}

fn save_previous_launchd_plist_if_needed(destination: &Path, loaded: bool) -> Result<()> {
    if !loaded {
        return Ok(());
    }
    let previous = launchd_previous_plist_path(destination);
    if previous.exists() || !destination.exists() {
        return Ok(());
    }
    std::fs::copy(destination, &previous).with_context(|| {
        format!(
            "saving previous launchd plist {}",
            previous.to_string_lossy()
        )
    })?;
    Ok(())
}

fn current_uid() -> String {
    std::process::Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

fn service_install(config: &Config) -> Result<()> {
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
        let dst = launchd_plist_path()?;
        std::fs::create_dir_all(dst.parent().unwrap())?;
        let mut launchctl = SystemLaunchctl::new();
        let mode = launchd_install_mode(launchctl.is_loaded()?);
        save_previous_launchd_plist_if_needed(
            &dst,
            mode == LaunchdInstallMode::UpdatedRestartRequired,
        )?;
        let plist = render_launchd_plist(
            &exe,
            &std::env::var("PATH").unwrap_or_default(),
            &known_launchd_path_dirs(&exe),
        );
        std::fs::write(&dst, plist)?;
        match mode {
            LaunchdInstallMode::UpdatedRestartRequired => {
                eprintln!("launchd plist updated, but the loaded daemon was left running.");
                eprintln!("  The on-disk plist is newer than the loaded service.");
                eprintln!(
                    "  Apply it when there are no in-flight routed requests: alex service restart"
                );
                anyhow::bail!("launchd service was not replaced (exit 1)");
            }
            LaunchdInstallMode::Bootstrap => {
                launchctl.bootstrap(&dst)?;
                let mut health = LocalLaunchdHealthProbe::new(config)?;
                if !health.wait_until_healthy() {
                    anyhow::bail!(
                        "launchd bootstrap completed, but the daemon did not become healthy within {}s",
                        LAUNCHD_HEALTH_TIMEOUT.as_secs()
                    );
                }
                println!(
                    "{} {}",
                    ui::gold(ui::ankh()),
                    ui::bold("launchd service installed and serving")
                );
                println!("  {}", ui::dim(&dst.to_string_lossy()));
            }
        }
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

fn service_set_bind(config: &Config, target: &str) -> Result<()> {
    let target = BindTarget::parse(target)?;
    let mut updated = config.clone();
    let previous_host = updated.host.clone();
    updated.host = target.host();
    save_config(&updated)?;

    println!(
        "Daemon network exposure saved: {} (host = {}).",
        target.description(),
        updated.host
    );
    // This changes only `host`: local_key and all harness configuration are
    // deliberately untouched. Local harnesses continue to use loopback via
    // Config::base_url().
    if service_managed(&detect_service_state()) {
        eprintln!(
            "The loaded daemon is still bound to {previous_host}; restart required to apply this setting."
        );
        eprintln!("  Run: alex service restart");
    } else {
        eprintln!("The setting will apply the next time the daemon starts.");
    }
    Ok(())
}

fn service_restart(config: &Config, force: bool) -> Result<()> {
    if !cfg!(target_os = "macos") {
        anyhow::bail!("service restart supports macOS launchd only");
    }

    let plist = launchd_plist_path()?;
    let previous = launchd_previous_plist_path(&plist);
    let mut launchctl = SystemLaunchctl::new();
    if !launchctl.is_loaded()? {
        anyhow::bail!("no loaded launchd daemon to restart; run alex service install first");
    }
    if !previous.exists() {
        anyhow::bail!(
            "no saved previous plist is available for rollback; run alex service install first"
        );
    }

    let mut health = LocalLaunchdHealthProbe::new(config)?;
    let mut rollback = FilesystemLaunchdRollback {
        destination: &plist,
        previous: &previous,
    };
    let mut force_warning = |status: &InFlightStatus| {
        eprintln!(
            "WARNING: forcing restart will interrupt {} routed request(s).",
            status.count
        );
        print_in_flight_requests(status);
    };
    let force_warning = force.then_some(&mut force_warning as &mut dyn FnMut(&InFlightStatus));
    match replace_loaded_launchd_service(
        &mut launchctl,
        &mut health,
        &mut rollback,
        &plist,
        force,
        force_warning,
    ) {
        LaunchdRestartOutcome::Replaced => {
            std::fs::remove_file(&previous).with_context(|| {
                format!(
                    "removing saved previous plist {}",
                    previous.to_string_lossy()
                )
            })?;
            println!(
                "{} {}",
                ui::gold(ui::ankh()),
                ui::bold("launchd service replaced and serving")
            );
            Ok(())
        }
        LaunchdRestartOutcome::RefusedInFlight { status } => {
            eprintln!(
                "launchd service was not replaced: {} routed request(s) are still in flight.",
                status.count
            );
            print_in_flight_requests(&status);
            eprintln!("  Wait for them to finish, then run: alex service restart");
            eprintln!("  To interrupt them and restart anyway: alex service restart --force");
            anyhow::bail!("refused to interrupt in-flight routed requests (exit 1)");
        }
        LaunchdRestartOutcome::RolledBack { cause } => {
            eprintln!(
                "launchd replacement failed; the previous daemon was restored and is serving."
            );
            eprintln!("  Cause: {cause}");
            anyhow::bail!("launchd service rollback completed (exit 1)");
        }
        LaunchdRestartOutcome::Failed { cause } => {
            eprintln!("launchd replacement failed; rollback did not restore a healthy daemon.");
            eprintln!("  Cause: {cause}");
            anyhow::bail!("launchd service restart failed (exit 1)");
        }
    }
}

fn print_in_flight_requests(status: &InFlightStatus) {
    for request in &status.requests {
        let session = request.session_id.as_deref().unwrap_or("unknown");
        let harness = request.harness.as_deref().unwrap_or("unknown");
        eprintln!(
            "  - age {}s · model {} · session {} · harness {}",
            request.age_s, request.model, session, harness
        );
    }
    if status.count > status.requests.len() as i64 {
        eprintln!(
            "  - {} request detail(s) unavailable from this daemon",
            status.count - status.requests.len() as i64
        );
    }
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
    let base = daemon_connect_base_url(host, port);
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

    #[test]
    fn tool_capture_cli_parses_harness_state_and_json() {
        let cli = Cli::try_parse_from(["alex", "tool-capture", "codex", "on", "--json"]).unwrap();
        match cli.command.unwrap() {
            Command::ToolCapture {
                harness,
                state: Some(ToolCaptureState::On),
                json: true,
            } => assert_eq!(harness, "codex"),
            _ => panic!("unexpected tool-capture command"),
        }
    }

    #[test]
    fn wrap_claude_forwards_hyphenated_args_verbatim() {
        let cli = Cli::try_parse_from([
            "alex",
            "wrap",
            "claude",
            "-p",
            "hi",
            "--allowedTools",
            "Bash",
        ])
        .unwrap();
        match cli.command.unwrap() {
            Command::Wrap {
                command: WrapCommand::Claude { args },
            } => assert_eq!(args, ["-p", "hi", "--allowedTools", "Bash"]),
            _ => panic!("unexpected wrap command"),
        }
    }

    #[test]
    fn wrap_launcher_builds_expected_arguments() {
        let dir = PathBuf::from("/tmp/alex-claude");
        let claude = claude_launcher_args(&dir, &["-p".into(), "hi".into()]);
        assert_eq!(
            claude,
            vec![
                OsString::from("--settings"),
                dir.join(harness_connect::CLAUDE_PROFILE_FILE)
                    .into_os_string(),
                OsString::from("-p"),
                OsString::from("hi"),
            ]
        );
        assert_eq!(
            codex_launcher_args(&["exec".into(), "hi".into()]),
            vec![
                OsString::from("--profile"),
                OsString::from("alex"),
                OsString::from("exec"),
                OsString::from("hi"),
            ]
        );
    }

    #[test]
    fn wrap_launcher_requires_connected_harness_state() {
        let dir = tmpdir("wrap-launcher-preflight");
        let claude = ensure_wrap_launcher_connected("claude", &dir).unwrap_err();
        assert!(claude.to_string().contains("alex connect claude"));
        let codex = ensure_wrap_launcher_connected("codex", &dir).unwrap_err();
        assert!(codex.to_string().contains("alex connect codex"));
    }

    fn agent_jsonl(lines: Vec<serde_json::Value>) -> String {
        lines
            .into_iter()
            .map(|line| serde_json::to_string(&line).unwrap())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn cursor_trace_metadata_uses_cli_identity_and_explicit_model() {
        let config = serde_json::json!({
            "authInfo": {
                "email": "Person@Example.com",
                "userId": 42,
                "authId": "github|secret-looking-but-not-used-as-the-local-id"
            },
            "selectedModel": {"modelId": "composer-2.5"}
        });
        let metadata =
            cursor_trace_metadata_from_config(&config, &["--model".into(), "gpt-5.6-sol".into()]);
        assert_eq!(metadata.model, "gpt-5.6-sol");
        let account = metadata.billing_account.unwrap();
        assert_eq!(account.provider, "cursor");
        assert_eq!(account.kind, "subscription");
        assert_eq!(account.email.as_deref(), Some("person@example.com"));
        assert_eq!(
            account.subscription_identity.as_deref(),
            Some("cursor:user:42")
        );
        assert!(account.account_id.starts_with("cursor-subscription-"));
        assert!(!account.account_id.contains("secret-looking"));
    }

    #[test]
    fn cursor_reconcile_records_billing_account_without_inventing_cost() {
        let store = Store::open(tmpdir("cursor-billing-attribution")).unwrap();
        let account = KnownAccount::new(
            "cursor-subscription-test",
            "cursor",
            "person@example.com",
            "subscription",
            Some("cursor:user:42".into()),
            Some("person@example.com".into()),
        );
        let metadata = CursorTraceMetadata {
            model: "composer-2.5".into(),
            billing_account: Some(account.clone()),
        };
        let turn = AgentTranscriptTurn {
            user: "hello".into(),
            assistant: "hi".into(),
            tool_calls: vec![],
            assistant_blocks: vec![],
            complete: true,
            end_status: Some("success".into()),
        };
        assert_eq!(
            reconcile_agent_turn(
                &store,
                "wrap-agent-test",
                "transcript-test",
                1,
                100,
                &turn,
                false,
                &metadata,
            )
            .unwrap(),
            AgentReconcileOutcome::Inserted
        );
        let row = store.get_trace("agent-transcript-test-1").unwrap().unwrap();
        assert_eq!(row["requested_model"], "composer-2.5");
        assert_eq!(row["account_id"], account.account_id);
        assert_eq!(row["subscription_identity"], "cursor:user:42");
        assert!(row["cost_usd"].is_null());
        let known = store.list_known_accounts().unwrap();
        assert_eq!(known[0]["email"], "person@example.com");
    }

    #[tokio::test]
    async fn traces_reattach_command_is_a_noop_without_yes() {
        let dir = tmpdir("traces-reattach-no-yes");
        let config: Config = serde_json::from_value(serde_json::json!({
            "host": "127.0.0.1", "port": 0, "data_dir": dir, "local_key": "test-key"
        }))
        .unwrap();
        let store = Store::open(config.data_dir.clone()).unwrap();
        store
            .insert_trace(&alex_core::TraceRecord {
                id: "legacy-orphan".into(),
                ts_request_ms: 100,
                account_id: Some("openai-oauth-old".into()),
                upstream_provider: Some("openai".into()),
                routed_model: Some("gpt-5".into()),
                ..Default::default()
            })
            .unwrap();
        let vault = open_vault(&config).unwrap();
        vault.upsert(alex_auth::Account {
            id: "openai-oauth-new".into(), provider: alex_core::Provider::Openai,
            kind: "oauth".into(), name: "new".into(), description: None, paused: false,
            label: None, access_token: None, refresh_token: None, id_token: None, api_key: None,
            expires_at_ms: None, last_refresh_ms: None,
            account_meta: serde_json::json!({"account_id": "acct_456", "email": "new@example.com"}),
            cooldown_until_ms: None, status: "active".into(), path: None,
        }).await.unwrap();
        traces_reattach_cmd(
            &config,
            Some("openai-oauth-old"),
            Some("openai-oauth-new"),
            false,
            true,
        )
        .await
        .unwrap();
        assert!(store
            .search_traces(&alex_store::TraceFilter::default())
            .unwrap()[0]["subscription_identity"]
            .is_null());
    }

    #[test]
    fn agent_transcript_aggregates_assistant_records_and_tool_calls() {
        let raw = agent_jsonl(vec![
            serde_json::json!({
                "role": "user",
                "message": {"content": [{"type": "text", "text": "<user_query>\nhi whats going on\n</user_query>"}]}
            }),
            serde_json::json!({
                "role": "assistant",
                "message": {"content": [
                    {"type": "text", "text": "Checking the workspace.\n\n[REDACTED]"},
                    {"type": "tool_use", "name": "Shell", "input": {"command": "git status"}},
                    {"type": "tool_use", "name": "Glob", "input": {"glob_pattern": "README*"}},
                    {"type": "tool_use", "name": "Glob", "input": {"glob_pattern": "*"}}
                ]}
            }),
            serde_json::json!({
                "role": "assistant",
                "message": {"content": [
                    {"type": "text", "text": "[REDACTED]"},
                    {"type": "tool_use", "name": "Read", "input": {"path": "README.md"}},
                    {"type": "tool_use", "name": "Shell", "input": {"command": "git diff"}},
                    {"type": "tool_use", "name": "Shell", "input": {"command": "git log"}}
                ]}
            }),
            serde_json::json!({
                "role": "assistant",
                "message": {"content": [{"type": "text", "text": "Here is the final answer."}]}
            }),
            serde_json::json!({
                "role": "user",
                "message": {"content": [{"type": "text", "text": "<user_query>what is your name?</user_query>"}]}
            }),
            serde_json::json!({
                "role": "assistant",
                "message": {"content": [{"type": "text", "text": "I'm Auto."}]}
            }),
            serde_json::json!({"type": "turn_ended", "status": "success"}),
        ]);

        let turns = parse_agent_transcript_turns(&raw);
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].user, "hi whats going on");
        assert_eq!(
            turns[0].assistant,
            "Checking the workspace.\n\nHere is the final answer."
        );
        assert_eq!(
            turns[0]
                .tool_calls
                .iter()
                .map(|call| call.name.as_str())
                .collect::<Vec<_>>(),
            ["Shell", "Glob", "Glob", "Read", "Shell", "Shell"]
        );
        assert_eq!(
            turns[0]
                .assistant_blocks
                .iter()
                .map(|block| block["type"].as_str().unwrap())
                .collect::<Vec<_>>(),
            [
                "text",
                "tool_call",
                "tool_call",
                "tool_call",
                "tool_call",
                "tool_call",
                "tool_call",
                "text",
            ]
        );
        assert!(turns[0].complete);
        assert_eq!(turns[1].assistant, "I'm Auto.");
        assert!(turns[1].complete);
        assert_eq!(turns[1].end_status.as_deref(), Some("success"));

        let (_, response) = agent_trace_bodies("transcript-1", 1, &turns[0]);
        let tool_calls = response["choices"][0]["message"]["tool_calls"]
            .as_array()
            .unwrap();
        assert_eq!(tool_calls.len(), 6);
        assert_eq!(tool_calls[0]["function"]["name"], "Shell");
        assert_eq!(
            response["_alexandria"]["assistant_blocks"][0]["text"],
            "Checking the workspace."
        );
        assert_eq!(
            response["_alexandria"]["assistant_blocks"][1]["name"],
            "Shell"
        );
        assert_eq!(
            response["_alexandria"]["assistant_blocks"][7]["text"],
            "Here is the final answer."
        );
        let arguments: serde_json::Value =
            serde_json::from_str(tool_calls[0]["function"]["arguments"].as_str().unwrap()).unwrap();
        assert_eq!(arguments["command"], "git status");
    }

    #[test]
    fn agent_text_cleanup_only_removes_cursor_marker_lines() {
        assert_eq!(
            clean_agent_assistant_text("A literal [REDACTED] value stays."),
            Some("A literal [REDACTED] value stays.".into())
        );
        assert_eq!(
            clean_agent_assistant_text("Final answer.\n\n[REDACTED]\n"),
            Some("Final answer.".into())
        );
        assert_eq!(clean_agent_assistant_text("[REDACTED]"), None);
    }

    #[test]
    fn agent_turn_ended_changes_completion_without_changing_content() {
        let user = serde_json::json!({
            "role": "user",
            "message": {"content": [{"type": "text", "text": "<user_query>hi</user_query>"}]}
        });
        let assistant = serde_json::json!({
            "role": "assistant",
            "message": {"content": [{"type": "text", "text": "Done."}]}
        });
        let partial =
            parse_agent_transcript_turns(&agent_jsonl(vec![user.clone(), assistant.clone()]));
        let complete = parse_agent_transcript_turns(&agent_jsonl(vec![
            user,
            assistant,
            serde_json::json!({"type": "turn_ended", "status": "success"}),
        ]));

        assert_eq!(partial[0].assistant, complete[0].assistant);
        assert_eq!(partial[0].assistant_blocks, complete[0].assistant_blocks);
        assert!(!partial[0].complete);
        assert!(complete[0].complete);
        assert_ne!(partial[0], complete[0]);
        let (_, partial_body) = agent_trace_bodies("completion-only", 1, &partial[0]);
        let (_, complete_body) = agent_trace_bodies("completion-only", 1, &complete[0]);
        assert!(partial_body["choices"][0]["finish_reason"].is_null());
        assert_eq!(complete_body["choices"][0]["finish_reason"], "stop");
    }

    #[test]
    fn agent_transcript_import_refreshes_a_growing_turn_in_place() {
        let data_dir = tmpdir("agent-growing-data");
        let transcript_root = tmpdir("agent-growing-source");
        let transcript_id = "d22baa9e-0303-4678-87f1-9d8b46376eb4";
        let transcript_dir = transcript_root.join(transcript_id);
        std::fs::create_dir_all(&transcript_dir).unwrap();
        let transcript_path = transcript_dir.join(format!("{transcript_id}.jsonl"));
        let user = serde_json::json!({
            "role": "user",
            "message": {"content": [{"type": "text", "text": "<user_query>hi</user_query>"}]}
        });
        let progress = serde_json::json!({
            "role": "assistant",
            "message": {"content": [
                {"type": "text", "text": "Checking."},
                {"type": "tool_use", "name": "Shell", "input": {"command": "git status"}}
            ]}
        });
        std::fs::write(
            &transcript_path,
            agent_jsonl(vec![user.clone(), progress.clone()]),
        )
        .unwrap();

        let mut seen = BTreeMap::new();
        let metadata = CursorTraceMetadata::default();
        import_agent_transcripts(
            &data_dir,
            &transcript_root,
            "wrap-agent-test",
            0,
            &mut seen,
            None,
            &metadata,
        )
        .unwrap();
        let store = Store::open(data_dir.clone()).unwrap();
        let trace_id = format!("agent-{transcript_id}-1");
        let first = store.get_trace(&trace_id).unwrap().unwrap();
        let first_ts = first["ts_request_ms"].as_i64().unwrap();
        let first_run = first["run_id"].as_str().unwrap().to_string();
        let first_path = first["resp_body_path"].as_str().unwrap().to_string();
        let first_response = read_gzip_json(std::path::Path::new(&first_path)).unwrap();
        assert_eq!(
            first_response["choices"][0]["message"]["content"],
            "Checking."
        );
        assert_eq!(
            first_response["_alexandria"]["assistant_blocks"]
                .as_array()
                .unwrap()
                .iter()
                .map(|block| block["type"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["text", "tool_call"]
        );
        assert!(first_response["choices"][0]["finish_reason"].is_null());

        let final_answer = serde_json::json!({
            "role": "assistant",
            "message": {"content": [{"type": "text", "text": "Final answer."}]}
        });
        std::fs::write(
            &transcript_path,
            agent_jsonl(vec![
                user,
                progress,
                final_answer,
                serde_json::json!({"type": "turn_ended", "status": "success"}),
            ]),
        )
        .unwrap();
        import_agent_transcripts(
            &data_dir,
            &transcript_root,
            "wrap-agent-test",
            0,
            &mut seen,
            None,
            &metadata,
        )
        .unwrap();

        let rows = store.session_traces(transcript_id, None).unwrap();
        assert_eq!(rows.len(), 1);
        let updated = store.get_trace(&trace_id).unwrap().unwrap();
        assert_eq!(updated["ts_request_ms"].as_i64(), Some(first_ts));
        assert_eq!(updated["run_id"].as_str(), Some(first_run.as_str()));
        assert_eq!(
            updated["resp_body_path"].as_str(),
            Some(first_path.as_str())
        );
        let response = read_gzip_json(std::path::Path::new(&first_path)).unwrap();
        assert_eq!(
            response["choices"][0]["message"]["content"],
            "Checking.\n\nFinal answer."
        );
        assert_eq!(response["choices"][0]["finish_reason"], "stop");
        assert_eq!(
            response["_alexandria"]["assistant_blocks"]
                .as_array()
                .unwrap()
                .iter()
                .map(|block| block["type"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["text", "tool_call", "text"]
        );
        assert_eq!(
            response["choices"][0]["message"]["tool_calls"][0]["function"]["name"],
            "Shell"
        );
    }

    #[test]
    fn agent_transcript_repair_rejects_malformed_or_empty_jsonl() {
        let data_dir = tmpdir("agent-repair-invalid-data");
        let transcript_root = tmpdir("agent-repair-invalid-source");
        let transcript_id = "bad-transcript";
        let transcript_dir = transcript_root.join(transcript_id);
        std::fs::create_dir_all(&transcript_dir).unwrap();
        let transcript_path = transcript_dir.join(format!("{transcript_id}.jsonl"));
        std::fs::write(
            &transcript_path,
            "{\"role\":\"user\",\"message\":{\"content\":[]}}\n{broken",
        )
        .unwrap();

        let error =
            repair_agent_transcript(&data_dir, &transcript_root, transcript_id, true).unwrap_err();
        assert!(error.to_string().contains("line 2"));

        std::fs::write(&transcript_path, "\n").unwrap();
        let error =
            repair_agent_transcript(&data_dir, &transcript_root, transcript_id, true).unwrap_err();
        assert!(error.to_string().contains("transcript is empty"));
    }

    #[test]
    fn remote_trace_config_supports_env_key_files_and_https_guard() {
        let _guard = ENV_LOCK.lock().unwrap();
        for name in [
            "ALEXANDRIA_TRACE_URL",
            "ALEXANDRIA_TRACE_KEY",
            "ALEXANDRIA_TRACE_KEY_FILE",
            "ALEXANDRIA_TRACE_ALLOW_INSECURE_HTTP",
        ] {
            std::env::remove_var(name);
        }
        assert!(resolve_remote_trace_config(&RemoteTraceArgs::default())
            .unwrap()
            .is_none());

        std::env::set_var("ALEXANDRIA_TRACE_URL", "http://10.0.0.8:4100/");
        std::env::set_var("ALEXANDRIA_TRACE_KEY", "alxk-env-key");
        let error = match resolve_remote_trace_config(&RemoteTraceArgs::default()) {
            Err(error) => error,
            Ok(_) => panic!("plaintext remote URL should be rejected"),
        };
        assert!(error.to_string().contains("refusing plaintext"));

        std::env::set_var("ALEXANDRIA_TRACE_ALLOW_INSECURE_HTTP", "true");
        let config = resolve_remote_trace_config(&RemoteTraceArgs::default())
            .unwrap()
            .unwrap();
        assert_eq!(config.base_url, "http://10.0.0.8:4100");
        assert_eq!(config.key, "alxk-env-key");

        let key_file = tmpdir("remote-trace-key").join("key");
        std::fs::write(&key_file, "  alxk-file-key\n").unwrap();
        let config = resolve_remote_trace_config(&RemoteTraceArgs {
            trace_url: Some("https://alex.example.test/".into()),
            trace_key_file: Some(key_file),
            allow_insecure_http: false,
        })
        .unwrap()
        .unwrap();
        assert_eq!(config.base_url, "https://alex.example.test");
        assert_eq!(config.key, "alxk-file-key");

        for name in [
            "ALEXANDRIA_TRACE_URL",
            "ALEXANDRIA_TRACE_KEY",
            "ALEXANDRIA_TRACE_KEY_FILE",
            "ALEXANDRIA_TRACE_ALLOW_INSECURE_HTTP",
        ] {
            std::env::remove_var(name);
        }
    }

    #[test]
    fn remote_trace_payload_preserves_metadata_and_reads_local_bodies() {
        use base64::Engine;

        let store = Store::open(tmpdir("remote-trace-payload")).unwrap();
        let request_path = store
            .write_body("remote-payload-1", "request.json", br#"{"prompt":"hi"}"#)
            .unwrap();
        let response_path = store
            .write_body(
                "remote-payload-1",
                "response.body",
                br#"{"answer":"hello"}"#,
            )
            .unwrap();
        store
            .insert_trace(&alex_core::TraceRecord {
                id: "remote-payload-1".into(),
                ts_request_ms: 100,
                ts_response_ms: Some(200),
                session_id: Some("session-1".into()),
                harness: Some("agent".into()),
                upstream_provider: Some("cursor".into()),
                requested_model: Some("cursor-agent".into()),
                routed_model: Some("cursor-agent".into()),
                method: Some("TRANSCRIPT".into()),
                path: Some("cursor-agent-transcript".into()),
                status: Some(200),
                run_id: Some("wrap-agent-1".into()),
                req_body_path: Some(request_path),
                resp_body_path: Some(response_path),
                ..Default::default()
            })
            .unwrap();

        let payload = trace_ingest_payload_from_store(&store, "remote-payload-1").unwrap();
        assert_eq!(payload.trace.method.as_deref(), Some("TRANSCRIPT"));
        assert_eq!(
            payload.trace.path.as_deref(),
            Some("cursor-agent-transcript")
        );
        assert_eq!(payload.trace.run_id.as_deref(), Some("wrap-agent-1"));
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(payload.request_body_b64.unwrap())
                .unwrap(),
            br#"{"prompt":"hi"}"#
        );
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(payload.response_body_b64.unwrap())
                .unwrap(),
            br#"{"answer":"hello"}"#
        );
    }

    #[test]
    fn amp_import_retries_a_partial_jsonl_tail() {
        let data_dir = tmpdir("amp-partial-tail-data");
        let capture_dir = tmpdir("amp-partial-tail-capture");
        let path = capture_dir.join("ws.jsonl");
        let message = serde_json::json!({
            "method": "message_added",
            "params": {"message": {
                "role": "user",
                "threadId": "thread-1",
                "messageId": "user-1",
                "content": [{"text": "hello"}],
            }},
        });
        let line = serde_json::json!({
            "direction": "upstream_to_client",
            "ts": "2026-07-10T00:00:00Z",
            "text": message.to_string(),
        })
        .to_string();
        std::fs::write(&path, &line).unwrap();

        let mut state = AmpWsTraceState::new(
            data_dir,
            "wrap-amp-test".into(),
            "{}".into(),
            "test-key".into(),
            None,
        );
        let mut offset = 0;
        state.ingest_new(&path, &mut offset).unwrap();
        assert_eq!(offset, 0);
        assert!(state.user_by_thread.is_empty());

        std::fs::write(&path, format!("{line}\n")).unwrap();
        state.ingest_new(&path, &mut offset).unwrap();
        assert_eq!(offset as usize, line.len() + 1);
        assert_eq!(
            state.user_by_thread["thread-1"].text, "hello",
            "the completed record is parsed on the next poll"
        );
    }

    #[test]
    fn amp_import_preserves_human_questions_across_tool_using_turns() {
        let data_dir = tmpdir("amp-multi-turn");
        let mut state = AmpWsTraceState::new(
            data_dir.clone(),
            "wrap-amp-multi".into(),
            r#"{"harness":"amp"}"#.into(),
            "test-key".into(),
            None,
        );
        state.billing_account = Some(KnownAccount::new(
            "amp-api-key",
            "amp",
            "person@example.com",
            "api_key",
            Some("amp:email:person@example.com".into()),
            Some("person@example.com".into()),
        ));
        let outer = |ts: &str, message: serde_json::Value| {
            serde_json::json!({
                "direction": "upstream_to_client",
                "ts": ts,
                "text": message.to_string(),
            })
            .to_string()
        };
        let message_added =
            |role: &str, id: &str, content: serde_json::Value, usage: serde_json::Value| {
                serde_json::json!({
                    "method": "message_added",
                    "params": {"message": {
                        "role": role,
                        "threadId": "thread-multi",
                        "messageId": id,
                        "content": content,
                        "usage": usage,
                    }},
                })
            };

        state
            .ingest_line(&outer(
                "2026-07-10T00:00:00Z",
                message_added(
                    "user",
                    "user-1",
                    serde_json::json!([{"type": "text", "text": "what files are in my dir"}]),
                    serde_json::Value::Null,
                ),
            ))
            .unwrap();
        state
            .ingest_line(&outer(
                "2026-07-10T00:00:01Z",
                message_added(
                    "assistant",
                    "assistant-tool-1",
                    serde_json::json!([{
                        "type": "tool_use",
                        "id": "tool-1",
                        "name": "shell_command",
                        "input": {"command": "ls -la"}
                    }]),
                    serde_json::Value::Null,
                ),
            ))
            .unwrap();
        state
            .ingest_line(&outer(
                "2026-07-10T00:00:01.500Z",
                message_added(
                    "user",
                    "tool-result-1",
                    serde_json::json!([{
                        "type": "tool_result",
                        "tool_use_id": "tool-1",
                        "content": "Cargo.toml\nREADME.md"
                    }]),
                    serde_json::Value::Null,
                ),
            ))
            .unwrap();
        state
            .ingest_line(&outer(
                "2026-07-10T00:00:02Z",
                message_added(
                    "assistant",
                    "assistant-1",
                    serde_json::json!([{"type": "text", "text": "Cargo.toml and README.md"}]),
                    serde_json::json!({"model": "amp-test", "inputTokens": 10, "outputTokens": 4}),
                ),
            ))
            .unwrap();

        state
            .ingest_line(&outer(
                "2026-07-10T00:01:00Z",
                message_added(
                    "user",
                    "user-2",
                    serde_json::json!([{"type": "text", "text": "what is the weather like in Spain"}]),
                    serde_json::Value::Null,
                ),
            ))
            .unwrap();
        state
            .ingest_line(&outer(
                "2026-07-10T00:01:01Z",
                message_added(
                    "user",
                    "tool-result-2",
                    serde_json::json!([{
                        "type": "tool_result",
                        "tool_use_id": "tool-2",
                        "content": [{"type": "text", "text": "Sunny and hot"}]
                    }]),
                    serde_json::Value::Null,
                ),
            ))
            .unwrap();
        state
            .ingest_line(&outer(
                "2026-07-10T00:01:02Z",
                message_added(
                    "assistant",
                    "assistant-2",
                    serde_json::json!([{"type": "text", "text": "Spain is generally hot in July."}]),
                    serde_json::json!({"model": "amp-test", "inputTokens": 12, "outputTokens": 7}),
                ),
            ))
            .unwrap();

        let rows = Store::open(data_dir)
            .unwrap()
            .session_traces("thread-multi", None)
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|row| row["account_id"] == "amp-api-key"));
        assert!(rows
            .iter()
            .all(|row| { row["subscription_identity"] == "amp:email:person@example.com" }));
        let questions = rows
            .iter()
            .map(|row| {
                let path = std::path::Path::new(row["req_body_path"].as_str().unwrap());
                read_gzip_json(path).unwrap()["messages"][0]["content"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect::<Vec<_>>();
        assert_eq!(
            questions,
            [
                "what files are in my dir",
                "what is the weather like in Spain"
            ]
        );
    }

    #[test]
    fn amp_import_records_error_only_turns() {
        let data_dir = tmpdir("amp-error-turn");
        let mut state = AmpWsTraceState::new(
            data_dir.clone(),
            "wrap-amp-error".into(),
            r#"{"harness":"amp"}"#.into(),
            "test-key".into(),
            None,
        );
        let outer = |message: serde_json::Value| {
            serde_json::json!({
                "direction": "upstream_to_client",
                "ts": "2026-07-10T00:00:00Z",
                "text": message.to_string(),
            })
            .to_string()
        };
        state
            .ingest_line(&outer(serde_json::json!({
                "method": "message_added",
                "params": {"message": {
                    "role": "user",
                    "threadId": "thread-error",
                    "messageId": "user-error",
                    "content": [{"text": "hi"}],
                }},
            })))
            .unwrap();
        state
            .ingest_line(&outer(serde_json::json!({
                "method": "error_set",
                "params": {"error": {"message": "Unexpected server response: 401"}},
            })))
            .unwrap();
        state
            .ingest_line(&outer(serde_json::json!({
                "method": "plugin_message",
                "params": {"message": {
                    "method": "agent.end",
                    "params": {"event": {
                        "thread": {"id": "thread-error"},
                        "id": "user-error",
                        "status": "error",
                        "messages": [{
                            "role": "user",
                            "id": "user-error",
                            "content": [{"text": "hi"}],
                        }],
                    }},
                }},
            })))
            .unwrap();

        let rows = Store::open(data_dir)
            .unwrap()
            .session_traces("thread-error", None)
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["status"], 401);
        assert_eq!(rows[0]["error"], "Unexpected server response: 401");
        assert_eq!(rows[0]["upstream_provider"], "amp");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn remote_trace_worker_preflights_and_uploads() {
        use axum::http::{HeaderMap, StatusCode};
        use axum::routing::get;
        use axum::{Json, Router};
        use std::sync::atomic::{AtomicUsize, Ordering};

        let uploads = Arc::new(AtomicUsize::new(0));
        let expected_key = "alxk-worker-key";
        let app = Router::new().route(
            "/traces/ingest",
            get({
                move |headers: HeaderMap| async move {
                    if headers.get("x-api-key").and_then(|v| v.to_str().ok()) == Some(expected_key)
                    {
                        (StatusCode::OK, Json(serde_json::json!({"ok": true})))
                    } else {
                        (
                            StatusCode::UNAUTHORIZED,
                            Json(serde_json::json!({"ok": false})),
                        )
                    }
                }
            })
            .post({
                let uploads = uploads.clone();
                move |headers: HeaderMap, Json(body): Json<serde_json::Value>| {
                    let uploads = uploads.clone();
                    async move {
                        if headers.get("x-api-key").and_then(|v| v.to_str().ok())
                            != Some(expected_key)
                        {
                            return (
                                StatusCode::UNAUTHORIZED,
                                Json(serde_json::json!({"ok": false})),
                            );
                        }
                        if body["trace"]["id"] == "remote-worker-rejected" {
                            return (
                                StatusCode::BAD_REQUEST,
                                Json(serde_json::json!({"error": "bad trace"})),
                            );
                        }
                        assert_eq!(body["trace"]["id"], "remote-worker-1");
                        uploads.fetch_add(1, Ordering::SeqCst);
                        (
                            StatusCode::CREATED,
                            Json(serde_json::json!({"outcome": "inserted"})),
                        )
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let config = RemoteTraceConfig {
            base_url: format!("http://{address}"),
            key: expected_key.into(),
        };
        preflight_remote_trace(&config).await.unwrap();
        let worker = RemoteTraceWorker::start(config);
        worker
            .sender()
            .send(alex_core::TraceIngestPayload {
                trace: alex_core::TraceRecord {
                    id: "remote-worker-rejected".into(),
                    ts_request_ms: 1,
                    ..Default::default()
                },
                request_body_b64: None,
                upstream_request_body_b64: None,
                response_body_b64: None,
            })
            .unwrap();
        worker
            .sender()
            .send(alex_core::TraceIngestPayload {
                trace: alex_core::TraceRecord {
                    id: "remote-worker-1".into(),
                    ts_request_ms: 1,
                    ..Default::default()
                },
                request_body_b64: None,
                upstream_request_body_b64: None,
                response_body_b64: None,
            })
            .unwrap();
        let report = worker.stop().await.unwrap();
        assert_eq!(report.uploaded, 1);
        assert_eq!(report.failed, 1);
        assert!(report.failures[0].contains("remote-worker-rejected"));
        assert_eq!(uploads.load(Ordering::SeqCst), 1);
        server.abort();
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
            ping_openrouter_model: default_ping_openrouter(),
            gemini_project: String::new(),
            anthropic_upstream: "direct".into(),
            dario_api_key: String::new(),
            dario_claude_bin: None,
            dario_update_check_minutes: 60,
            dario_version: None,
            dario_probe_seconds: 90,
            dario_probe_failures: 2,
            dario_probe_model: "claude-haiku-4-5".into(),
            trace_body_retention_days: default_trace_body_retention_days(),
            trace_row_retention_days: 0,
            update_check_hours: default_update_check_hours(),
            update_channel: default_update_channel(),
            upstream_stream_idle_timeout_seconds: default_upstream_stream_idle_timeout_seconds(),
            harness_overrides: BTreeMap::new(),
            harness_tool_capture: BTreeMap::new(),
            account_policy: BTreeMap::new(),
        }
    }

    #[test]
    fn bind_target_presets_map_to_persisted_host() {
        assert_eq!(BindTarget::parse("loopback").unwrap().host(), "127.0.0.1");
        assert_eq!(BindTarget::parse("all").unwrap().host(), "0.0.0.0");
        assert_eq!(
            BindTarget::parse("100.101.102.103").unwrap().host(),
            "100.101.102.103"
        );
        assert!(BindTarget::parse("not-an-address").is_err());
    }

    #[test]
    fn tailscale_bind_keeps_local_harness_base_url_on_loopback() {
        let mut config = test_config(tmpdir("tailscale-local-base-url"));
        config.host = "100.101.102.103".into();
        assert_eq!(config.base_url(), "http://127.0.0.1:4100");
    }

    #[tokio::test]
    async fn unavailable_configured_bind_falls_back_to_loopback() {
        // RFC 5737 TEST-NET-1 is never a locally assigned interface address.
        let (listener, host, reason) = bind_daemon_listener_with_fallback("192.0.2.1", 0, true)
            .await
            .unwrap();
        assert_eq!(host, "127.0.0.1");
        assert!(reason.is_some());
        assert!(listener.local_addr().unwrap().ip().is_loopback());
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
            std::time::Duration::from_secs(15 * 60),
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

    #[tokio::test(flavor = "current_thread")]
    async fn admin_reset_is_authenticated_and_returns_the_shared_dry_run_plan() {
        let _guard = ENV_LOCK.lock().unwrap();
        let home = tmpdir("admin-reset");
        std::env::set_var("ALEXANDRIA_HOME", &home);
        let mut config = test_config(home.clone());
        config.data_dir = home.clone();
        save_config(&config).unwrap();
        let state = alex_proxy::build_state(
            config.local_key.clone(),
            Arc::new(Vault::open(home.join("accounts")).unwrap()),
            Arc::new(Store::open(home.clone()).unwrap()),
            None,
            config.base_url(),
            config.upstream_stream_idle_timeout(),
        );
        alex_proxy::set_reset_handler(&state, Arc::new(reset::DaemonResetHandler));
        let app = alex_proxy::router(state);
        let (status, body) = router_json(
            app,
            Method::POST,
            "/admin/reset",
            Some(serde_json::json!({"traces": true, "dry_run": true})),
        )
        .await;
        std::env::remove_var("ALEXANDRIA_HOME");
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["dry_run"], true);
        assert_eq!(body["selected"], serde_json::json!(["traces"]));
        assert!(body["counts"]["bodies"]["bytes"].is_u64());
    }

    #[test]
    fn launchctl_pid_parsing() {
        let out =
            "com.alexandria.daemon = {\n\tactive count = 1\n\tpid = 96513\n\tstate = running\n}";
        assert_eq!(parse_launchctl_pid(out), Some(96513));
        assert_eq!(parse_launchctl_pid("state = running"), None);
        assert_eq!(parse_launchctl_pid(""), None);
    }

    struct FakeLaunchctl {
        calls: Vec<&'static str>,
        bootout_results: std::collections::VecDeque<bool>,
        bootstrap_results: std::collections::VecDeque<bool>,
    }

    impl FakeLaunchctl {
        fn succeeds() -> Self {
            Self {
                calls: Vec::new(),
                bootout_results: [true].into(),
                bootstrap_results: [true].into(),
            }
        }
    }

    impl LaunchctlControl for FakeLaunchctl {
        fn is_loaded(&mut self) -> Result<bool> {
            Ok(true)
        }

        fn bootout(&mut self, _: &Path) -> Result<()> {
            self.calls.push("bootout");
            if self.bootout_results.pop_front().unwrap_or(true) {
                Ok(())
            } else {
                anyhow::bail!("fake bootout failure")
            }
        }

        fn bootstrap(&mut self, _: &Path) -> Result<()> {
            self.calls.push("bootstrap");
            if self.bootstrap_results.pop_front().unwrap_or(true) {
                Ok(())
            } else {
                anyhow::bail!("fake bootstrap failure")
            }
        }
    }

    struct FakeHealth {
        in_flight: i64,
        healthy_results: std::collections::VecDeque<bool>,
    }

    impl LaunchdHealthProbe for FakeHealth {
        fn in_flight(&mut self) -> Result<InFlightStatus> {
            Ok(InFlightStatus {
                count: self.in_flight,
                requests: Vec::new(),
            })
        }

        fn wait_until_healthy(&mut self) -> bool {
            self.healthy_results.pop_front().unwrap_or(false)
        }
    }

    #[derive(Default)]
    struct FakeRollback {
        restored: bool,
    }

    impl LaunchdPlistRollback for FakeRollback {
        fn restore_previous(&mut self) -> Result<()> {
            self.restored = true;
            Ok(())
        }
    }

    #[test]
    fn launchd_plist_rendering_preserves_path_and_escapes_values() {
        let plist = render_launchd_plist(
            Path::new("/Users/alex/bin/alex&ria"),
            "/usr/bin:/custom/bin",
            &[
                PathBuf::from("/opt/homebrew/bin"),
                PathBuf::from("/custom/bin"),
            ],
        );
        assert!(plist.contains("/Users/alex/bin/alex&amp;ria"));
        assert!(plist.contains(
            "<key>PATH</key>\n    <string>/usr/bin:/custom/bin:/opt/homebrew/bin</string>"
        ));
        assert_eq!(launchd_install_mode(false), LaunchdInstallMode::Bootstrap);
        assert_eq!(
            launchd_install_mode(true),
            LaunchdInstallMode::UpdatedRestartRequired
        );
    }

    #[test]
    fn launchd_restart_refuses_while_routed_requests_are_in_flight() {
        let mut launchctl = FakeLaunchctl::succeeds();
        let mut health = FakeHealth {
            in_flight: 2,
            healthy_results: [].into(),
        };
        let mut rollback = FakeRollback::default();
        assert_eq!(
            replace_loaded_launchd_service(
                &mut launchctl,
                &mut health,
                &mut rollback,
                Path::new("daemon.plist"),
                false,
                None,
            ),
            LaunchdRestartOutcome::RefusedInFlight {
                status: InFlightStatus {
                    count: 2,
                    requests: Vec::new(),
                },
            }
        );
        assert!(launchctl.calls.is_empty());
        assert!(!rollback.restored);
    }

    #[test]
    fn launchd_restart_force_replaces_while_routed_requests_are_in_flight() {
        let mut launchctl = FakeLaunchctl::succeeds();
        let mut health = FakeHealth {
            in_flight: 2,
            healthy_results: [true].into(),
        };
        let mut rollback = FakeRollback::default();
        let mut interrupted = None;
        let mut warning = |status: &InFlightStatus| interrupted = Some(status.count);
        assert_eq!(
            replace_loaded_launchd_service(
                &mut launchctl,
                &mut health,
                &mut rollback,
                Path::new("daemon.plist"),
                true,
                Some(&mut warning),
            ),
            LaunchdRestartOutcome::Replaced
        );
        assert_eq!(interrupted, Some(2));
        assert_eq!(launchctl.calls, ["bootout", "bootstrap"]);
        assert!(!rollback.restored);
    }

    #[test]
    fn launchd_restart_boots_out_bootstraps_and_verifies_health() {
        let mut launchctl = FakeLaunchctl::succeeds();
        let mut health = FakeHealth {
            in_flight: 0,
            healthy_results: [true].into(),
        };
        let mut rollback = FakeRollback::default();
        assert_eq!(
            replace_loaded_launchd_service(
                &mut launchctl,
                &mut health,
                &mut rollback,
                Path::new("daemon.plist"),
                false,
                None,
            ),
            LaunchdRestartOutcome::Replaced
        );
        assert_eq!(launchctl.calls, ["bootout", "bootstrap"]);
        assert!(!rollback.restored);
    }

    #[test]
    fn launchd_restart_rolls_back_when_new_bootstrap_fails() {
        let mut launchctl = FakeLaunchctl {
            calls: Vec::new(),
            bootout_results: [true, true].into(),
            bootstrap_results: [false, true].into(),
        };
        let mut health = FakeHealth {
            in_flight: 0,
            healthy_results: [true].into(),
        };
        let mut rollback = FakeRollback::default();
        let outcome = replace_loaded_launchd_service(
            &mut launchctl,
            &mut health,
            &mut rollback,
            Path::new("daemon.plist"),
            false,
            None,
        );
        assert!(matches!(outcome, LaunchdRestartOutcome::RolledBack { .. }));
        assert_eq!(
            launchctl.calls,
            ["bootout", "bootstrap", "bootout", "bootstrap"]
        );
        assert!(rollback.restored);
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
        assert!(service_install(&test_config(tmpdir("windows-service"))).is_err());
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
            ping_openrouter_model: default_ping_openrouter(),
            gemini_project: String::new(),
            anthropic_upstream: "direct".into(),
            dario_api_key: String::new(),
            dario_claude_bin: None,
            dario_update_check_minutes: 60,
            dario_version: None,
            dario_probe_seconds: 90,
            dario_probe_failures: 2,
            dario_probe_model: "claude-haiku-4-5".into(),
            trace_body_retention_days: default_trace_body_retention_days(),
            trace_row_retention_days: 0,
            update_check_hours: default_update_check_hours(),
            update_channel: default_update_channel(),
            upstream_stream_idle_timeout_seconds: default_upstream_stream_idle_timeout_seconds(),
            harness_overrides: BTreeMap::new(),
            harness_tool_capture: BTreeMap::new(),
            account_policy: BTreeMap::new(),
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
        assert_eq!(
            override_.binary.as_ref().unwrap(),
            &home.join("bin").join("pi")
        );
        assert_eq!(
            override_.config_dir.as_ref().unwrap(),
            &home.join("pi-agent")
        );
    }

    #[test]
    fn daemon_host_setting_persists_wildcard_and_rejects_non_ip_values() {
        let _guard = ENV_LOCK.lock().unwrap();
        let home = tmpdir("config-host");
        std::env::set_var("ALEXANDRIA_HOME", &home);
        let mut config = test_config(home.clone());
        save_config(&config).unwrap();

        assert!(set_daemon_host(&mut config, "0.0.0.0").unwrap());
        assert_eq!(config.host, "0.0.0.0");
        assert_eq!(config.base_url(), "http://127.0.0.1:4100");
        assert!(!set_daemon_host(&mut config, "0.0.0.0").unwrap());
        assert!(set_daemon_host(&mut config, "192.168.1.20").unwrap());
        let (loaded, _) = load_or_create_config().unwrap();
        assert_eq!(loaded.host, "192.168.1.20");
        assert!(set_daemon_host(&mut config, "example.com")
            .unwrap_err()
            .to_string()
            .contains("IPv4 or IPv6"));
        std::env::remove_var("ALEXANDRIA_HOME");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn harness_router_lists_and_rejects_unsupported_connect() {
        let _guard = ENV_LOCK.lock().unwrap();
        let home = tmpdir("router-list");
        std::env::set_var("ALEXANDRIA_HOME", &home);
        save_config(&test_config(home.clone())).unwrap();
        let app = harness_admin_router(test_state("router-list-state"));

        let (status, body) = router_json(app.clone(), Method::GET, "/admin/harnesses", None).await;
        assert_eq!(status, StatusCode::OK);
        let harnesses = body["harnesses"].as_array().unwrap();
        assert_eq!(harnesses.len(), 19);
        assert!(harnesses.iter().all(|h| h["daemon_reachable"] == true));
        assert!(harnesses.iter().all(|h| h.get("name").is_some()));
        assert!(harnesses.iter().all(|h| h.get("override").is_some()));

        let (status, body) = router_json(
            app.clone(),
            Method::PUT,
            "/admin/harnesses/gemini/tool-capture",
            Some(serde_json::json!({"enabled": true})),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(
            body["error"].as_str(),
            Some("tool capture is not yet supported for gemini")
        );

        let (status, body) =
            router_json(app, Method::POST, "/admin/harnesses/gemini/connect", None).await;
        std::env::remove_var("ALEXANDRIA_HOME");
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body["error"]
            .as_str()
            .unwrap()
            .contains("does not support connect"));
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
        let app = harness_admin_router(test_state("router-override-state"));

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
        let app = harness_admin_router(state.clone());

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

        let (status, body) = router_json(
            app.clone(),
            Method::POST,
            "/admin/harnesses/pi/connect",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["key_id"].as_str().unwrap().starts_with("rk-"));
        assert_eq!(body["key"], "minted");
        assert!(body["models_total"].as_u64().unwrap() > 0);
        assert!(body["path"].as_str().unwrap().ends_with("models.json"));
        assert!(body["base_url"].as_str().is_some());
        assert!(body["added"].as_array().unwrap().len() > 0);
        assert_eq!(body["removed"].as_array().unwrap().len(), 0);
        let extension_path = config_dir.join("extensions/alexandria-session.ts");
        assert!(extension_path.exists());
        let extension = std::fs::read_to_string(extension_path).unwrap();
        assert!(extension.contains("ctx.model.provider !== \"alexandria\""));
        assert!(extension.contains("ctx.sessionManager.getSessionId()"));
        let models: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&models_path).unwrap()).unwrap();
        assert!(models["providers"]["alexandria"].is_object());
        let generated = models["providers"]["alexandria"]["models"]
            .as_array()
            .unwrap();
        let written_ids: Vec<&str> = generated.iter().filter_map(|m| m["id"].as_str()).collect();
        assert!(written_ids.iter().all(|id| id.starts_with("alex/")));
        assert!(written_ids
            .iter()
            .any(|id| *id == "alex/claude-fable-5" || id.ends_with("claude-opus-4-8")));
        for id in [
            "alex/gpt-5.6-sol",
            "alex/gpt-5.6-terra",
            "alex/gpt-5.6-luna",
        ] {
            assert!(generated.iter().any(|model| model["id"] == id));
        }
        let sol = generated
            .iter()
            .find(|model| model["id"] == "alex/gpt-5.6-sol")
            .unwrap();
        assert_eq!(sol["contextWindow"], 372000);
        assert_eq!(sol["maxTokens"], 128000);
        assert_eq!(sol["thinkingLevelMap"]["minimal"], "low");
        assert_eq!(sol["compat"]["forceAdaptiveThinking"], true);
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
        assert!(dplan
            .iter()
            .any(|s| s["detail"].as_str() == Some("remove provider block")));
        assert!(dplan.iter().any(|s| s["detail"]
            .as_str()
            .unwrap_or("")
            .starts_with("revoke harness key")));
        assert_eq!(
            std::fs::read_to_string(&models_path).unwrap(),
            before_disconnect
        );
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
            refreshed["providers"]["alexandria"]["apiKey"]
                .as_str()
                .unwrap(),
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
            "/admin/harnesses/gemini/refresh-config",
            None,
        )
        .await;
        std::env::remove_var("ALEXANDRIA_HOME");
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body["error"]
            .as_str()
            .unwrap()
            .contains("does not support connect"));
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn harness_router_connect_claude_writes_opt_in_profile() {
        let _guard = ENV_LOCK.lock().unwrap();
        let home = tmpdir("router-connect-claude");
        let bin_dir = tmpdir("router-connect-claude-bin");
        let binary = fake_executable(&bin_dir, "claude", "echo claude-code 2.1.202");
        let config_dir = home.join("claude-home");
        std::fs::create_dir_all(&config_dir).unwrap();
        let normal_settings = "{\"theme\":\"dark\"}\n";
        std::fs::write(config_dir.join("settings.json"), normal_settings).unwrap();
        std::env::set_var("ALEXANDRIA_HOME", &home);
        let mut config = test_config(home.clone());
        config.harness_overrides.insert(
            "claude".into(),
            HarnessOverride {
                binary: Some(binary),
                config_dir: Some(config_dir.clone()),
            },
        );
        save_config(&config).unwrap();
        let state = test_state("router-connect-claude-state");
        let app = harness_admin_router(state.clone());

        let (status, body) = router_json(
            app.clone(),
            Method::POST,
            "/admin/harnesses/claude/connect?dry_run=true",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["plan"]
            .as_array()
            .unwrap()
            .iter()
            .any(|step| step["detail"]
                .as_str()
                .unwrap_or_default()
                .contains("plain `claude`")));

        let (status, body) = router_json(
            app.clone(),
            Method::POST,
            "/admin/harnesses/claude/connect",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["key"], "minted");
        assert!(body["path"]
            .as_str()
            .unwrap()
            .ends_with("alexandria-settings.json"));
        assert!(body["description"]
            .as_str()
            .unwrap()
            .contains("plain `claude`"));
        assert_eq!(
            std::fs::read_to_string(config_dir.join("settings.json")).unwrap(),
            normal_settings
        );
        let profile: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(config_dir.join("alexandria-settings.json")).unwrap(),
        )
        .unwrap();
        assert!(profile["model"]
            .as_str()
            .unwrap()
            .starts_with("claude-alex/"));
        assert_eq!(
            profile["env"]["CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY"],
            "1"
        );
        assert!(config_dir
            .join("alexandria-original-settings.json")
            .exists());
        assert_eq!(state.store.list_run_keys(false).unwrap().len(), 1);

        let (status, body) = router_json(
            app,
            Method::POST,
            "/admin/harnesses/claude/disconnect",
            None,
        )
        .await;
        std::env::remove_var("ALEXANDRIA_HOME");
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["was_connected"], true);
        assert!(!config_dir.join("alexandria-settings.json").exists());
        assert_eq!(
            std::fs::read_to_string(config_dir.join("settings.json")).unwrap(),
            normal_settings
        );
        assert!(config_dir
            .join("alexandria-original-settings.json")
            .exists());
        assert_eq!(state.store.list_run_keys(false).unwrap().len(), 0);
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn harness_router_connect_amp_installs_refreshes_and_removes_plugin() {
        let _guard = ENV_LOCK.lock().unwrap();
        let home = tmpdir("router-connect-amp");
        let bin_dir = tmpdir("router-connect-amp-bin");
        let binary = fake_executable(&bin_dir, "amp", "echo 0.0.1784018462-g51e7e3");
        let config_dir = home.join("amp-home");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::env::set_var("ALEXANDRIA_HOME", &home);
        let mut config = test_config(home.clone());
        config.harness_overrides.insert(
            "amp".into(),
            HarnessOverride {
                binary: Some(binary),
                config_dir: Some(config_dir.clone()),
            },
        );
        save_config(&config).unwrap();
        let state = test_state("router-connect-amp-state");
        let app = harness_admin_router(state.clone());

        let (status, body) = router_json(
            app.clone(),
            Method::POST,
            "/admin/harnesses/amp/connect?dry_run=true",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["plan"].as_array().unwrap().iter().any(|step| {
            step["action"] == "about"
                && step["detail"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("alex wrap amp")
        }));

        let (status, body) = router_json(
            app.clone(),
            Method::POST,
            "/admin/harnesses/amp/connect",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["key"], "minted");
        assert_eq!(body["models_total"], 0);
        assert!(body["path"]
            .as_str()
            .unwrap()
            .ends_with("plugins/alexandria.ts"));
        let plugin_path = config_dir.join("plugins/alexandria.ts");
        let plugin = std::fs::read_to_string(&plugin_path).unwrap();
        assert!(plugin.contains("Generated by Alexandria for Amp"));
        assert!(plugin.contains("amp.on('tool.call'"));
        assert!(harness_connect::amp_config_connected(&config_dir).unwrap());
        assert_eq!(state.store.list_run_keys(false).unwrap().len(), 1);
        assert_eq!(state.store.list_run_keys(false).unwrap()[0]["label"], "amp");

        let saved_key = harness_connect::read_amp_api_key(&config_dir).unwrap();
        let (status, body) = router_json(
            app.clone(),
            Method::POST,
            "/admin/harnesses/amp/refresh-config",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["key"], "reused");
        assert_eq!(body["models_total"], 0);
        assert_eq!(
            harness_connect::read_amp_api_key(&config_dir).as_deref(),
            Some(saved_key.as_str())
        );

        let (status, body) =
            router_json(app, Method::POST, "/admin/harnesses/amp/disconnect", None).await;
        std::env::remove_var("ALEXANDRIA_HOME");
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["was_connected"], true);
        assert_eq!(body["revoked"], 1);
        assert!(!plugin_path.exists());
        assert_eq!(state.store.list_run_keys(false).unwrap().len(), 0);
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn harness_router_connect_codex_writes_reversible_config() {
        let _guard = ENV_LOCK.lock().unwrap();
        let home = tmpdir("router-connect-codex");
        let bin_dir = tmpdir("router-connect-codex-bin");
        let binary = fake_executable(
            &bin_dir,
            "codex",
            r#"if [ "$1" = "debug" ]; then
  printf '%s\n' '{"models":[{"slug":"gpt-5.6-luna","display_name":"Luna"},{"slug":"gpt-5.5","display_name":"GPT-5.5"}]}'
else
  echo codex-cli 0.144.3
fi"#,
        );
        let config_dir = home.join("codex-home");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(
            config_dir.join("config.toml"),
            "model = \"gpt-5.5\"\nmodel_provider = \"openai\"\n",
        )
        .unwrap();
        std::env::set_var("ALEXANDRIA_HOME", &home);
        let mut config = test_config(home.clone());
        config.harness_overrides.insert(
            "codex".into(),
            HarnessOverride {
                binary: Some(binary),
                config_dir: Some(config_dir.clone()),
            },
        );
        save_config(&config).unwrap();
        let state = test_state("router-connect-codex-state");
        let app = harness_admin_router(state.clone());

        let (status, body) = router_json(
            app.clone(),
            Method::POST,
            "/admin/harnesses/codex/connect?dry_run=true",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["plan"]
            .as_array()
            .unwrap()
            .iter()
            .any(|step| step["detail"]
                .as_str()
                .unwrap_or_default()
                .contains("sub-agent lineage")));
        assert!(body["plan"].as_array().unwrap().iter().any(|step| {
            step["action"] == "about"
                && step["detail"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("--profile openai")
        }));

        let (status, body) = router_json(
            app.clone(),
            Method::POST,
            "/admin/harnesses/codex/connect",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["key"], "minted");
        assert!(body["path"].as_str().unwrap().ends_with("config.toml"));
        assert!(body["models_total"].as_u64().unwrap() > 2);
        assert!(harness_connect::codex_config_connected(&config_dir).unwrap());
        assert!(config_dir.join("alexandria-models.json").exists());
        assert!(config_dir.join("alexandria-openai-models.json").exists());
        assert!(config_dir.join("openai.config.toml").exists());
        assert!(config_dir.join("alex.config.toml").exists());
        assert!(config_dir.join("alexandria-original-config.toml").exists());
        assert!(config_dir.join("alexandria-session-hook.sh").exists());
        let connected = std::fs::read_to_string(config_dir.join("config.toml")).unwrap();
        assert!(connected.contains("model = \"alex/gpt-5.5\""));
        let catalog: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(config_dir.join("alexandria-models.json")).unwrap(),
        )
        .unwrap();
        let slugs = catalog["models"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|model| model["slug"].as_str())
            .collect::<Vec<_>>();
        assert!(slugs.contains(&"gpt-5.6-luna"));
        assert!(slugs.contains(&"gpt-5.5"));
        assert!(slugs.contains(&"alex/gpt-5.5"));
        assert!(slugs.iter().any(|slug| slug.starts_with("alex/claude-")));
        let keys = state.store.list_run_keys(false).unwrap();
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0]["label"], "codex");

        let (status, body) = router_json(
            app.clone(),
            Method::PUT,
            "/admin/harnesses/codex/default-route",
            Some(serde_json::json!({"route": "openai"})),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["default_route"], "openai");
        assert_eq!(body["restart_required"], true);
        let native_default = std::fs::read_to_string(config_dir.join("config.toml")).unwrap();
        assert!(native_default.contains("model_provider = \"openai\""));

        let (status, body) = router_json(
            app.clone(),
            Method::POST,
            "/admin/harnesses/codex/refresh-config",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["refreshed"], true);
        assert_eq!(body["key"], "reused");
        assert_eq!(
            harness_connect::codex_default_route(&config_dir)
                .unwrap()
                .as_deref(),
            Some("openai")
        );

        let (status, body) =
            router_json(app, Method::POST, "/admin/harnesses/codex/disconnect", None).await;
        std::env::remove_var("ALEXANDRIA_HOME");
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["was_connected"], true);
        assert_eq!(body["revoked"], 1);
        let restored = std::fs::read_to_string(config_dir.join("config.toml")).unwrap();
        assert!(restored.contains("model_provider = \"openai\""));
        assert!(!restored.contains("[model_providers.alexandria]"));
        assert!(!config_dir.join("openai.config.toml").exists());
        assert!(!config_dir.join("alex.config.toml").exists());
        assert!(config_dir.join("alexandria-original-config.toml").exists());
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
