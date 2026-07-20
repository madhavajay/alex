use std::collections::BTreeMap;
use std::ffi::OsString;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use alex_auth::{
    decrypt_bundle, encrypt_bundle, export_bundle, import_all, import_bundle, named_account_id,
    now_ms, AccountPolicy, BundleSelection, Vault,
};
use alex_store::{KnownAccount, Store};
use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use rand::Rng;
use serde::{Deserialize, Serialize};
use serde_json::json;

mod commands;
mod dario;
mod harness_connect;
mod harness_e2e;
mod reset;
mod selfupdate;
mod status;
mod telegram;
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
        /// Detach and run in the background, logging to ~/.alexandria/daemon.log
        #[arg(long)]
        background: bool,
    },
    #[command(name = "__launchd-restart-helper", hide = true)]
    LaunchdRestartHelper,
    /// Credential vault operations
    Auth {
        #[command(subcommand)]
        command: AuthCommand,
    },
    /// Encrypted portable vault and harness credential bundles
    Vault {
        #[command(subcommand)]
        command: VaultCommand,
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
        /// Alex daemon base URL (env: ALEXANDRIA_URL)
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
    /// Inspect configured notification channels or send a synthetic test alert
    Notify {
        #[command(subcommand)]
        command: NotifyCommand,
    },
    /// Start, inspect, and complete daemon-managed provider re-authentication
    Reauth {
        #[command(subcommand)]
        command: ReauthCommand,
    },
    /// Manage captured upstream error fixtures
    Fixtures {
        #[command(subcommand)]
        command: FixturesCommand,
    },
    /// Inject a fixture into the next request for a live session
    Simulate {
        #[command(subcommand)]
        command: SimulateCommand,
    },
    /// Configure resilience protection presets
    Protection {
        #[command(subcommand)]
        command: ProtectionCommand,
    },
    /// Inspect, validate, install, and test request middleware
    Middleware {
        #[command(subcommand)]
        command: MiddlewareCommand,
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
    /// Deliberately pause a provider to test failover or re-auth alerts
    Provider {
        #[command(subcommand)]
        command: ProviderCommand,
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
    /// Install, connect, configure, and launch a harness through Alex
    Up {
        /// Harness to bootstrap (pi is the default)
        #[arg(default_value = "pi")]
        harness: String,
        /// Remote Alex base URL. Supplying this never starts a local daemon.
        #[arg(long)]
        url: Option<String>,
        /// A model-only scoped run key (never an Alex local/admin key)
        #[arg(long)]
        key: Option<String>,
        /// Alex model to make the harness default
        #[arg(long, default_value = "alex/gpt-5.6-sol")]
        model: String,
        /// npm package version to install when the harness is absent
        #[arg(long)]
        version: Option<String>,
        /// Configure only; do not exec the harness
        #[arg(long)]
        no_launch: bool,
        /// Reserved for non-interactive bootstrap callers
        #[arg(long, short = 'y')]
        yes: bool,
        /// Arguments passed to the harness after `--`
        #[arg(last = true, allow_hyphen_values = true)]
        args: Vec<String>,
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
    /// Selectively remove local Alex data (dry-run unless --yes is supplied)
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
    /// Gracefully drain and restart the loaded launchd service
    Restart {
        /// Use the legacy hard restart (routed requests may be interrupted)
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
    /// Route through Dario automatically when an active Claude subscription is available
    Auto,
    /// Show generations and their states
    Status,
    /// Roll to a fresh generation of the same version
    Restart,
    /// Check npm for a newer version and roll if found
    Update,
    /// Discover Node and Claude, save their paths, and start a fresh generation
    Fix,
}

#[derive(Subcommand)]
enum NotifyCommand {
    /// List configured channels (webhook URLs and tokens are redacted)
    #[command(alias = "list")]
    Channels,
    /// Enable or disable inbound commands for a saved Telegram channel
    Commands {
        /// Enable inbound commands and OAuth paste-back
        #[arg(long, conflicts_with = "disable", required_unless_present = "disable")]
        enable: bool,
        /// Disable inbound commands and OAuth paste-back
        #[arg(long, conflicts_with = "enable", required_unless_present = "enable")]
        disable: bool,
        /// Saved channel id; omit when exactly one channel exists
        #[arg(long)]
        channel: Option<String>,
    },
    /// Send a synthetic event through saved channels using their stored token
    Test {
        /// Saved channel id; omit to test all saved channels
        #[arg(long)]
        channel: Option<String>,
        /// Event category to exercise, such as reauth
        #[arg(long)]
        category: Option<String>,
    },
    /// Show recent redacted inbound commands and outbound notification messages
    Log {
        /// Maximum recent messages to return (up to the persisted 200-entry ring)
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
}

#[derive(Subcommand)]
enum ReauthCommand {
    /// Start or replace a pending re-authentication session
    Start {
        /// Provider name or alias, such as anthropic or claude
        provider: String,
        /// Also deliver the clickable authorization link to Telegram
        #[arg(long)]
        notify: bool,
        /// Replace a stuck pending session
        #[arg(long)]
        force: bool,
    },
    /// Submit the code#state for the pending paste-mode session
    Submit {
        /// Complete pasted OAuth value in code#state form
        input: String,
    },
    /// Show pending re-authentication sessions
    Status {
        /// Optional provider filter
        provider: Option<String>,
    },
}

#[derive(Subcommand)]
enum FixturesCommand {
    List,
    Show {
        name: String,
    },
    Save {
        #[arg(long)]
        name: String,
        #[arg(long)]
        provider: String,
        #[arg(long = "from-trace")]
        from_trace: Option<String>,
        #[arg(long)]
        status: Option<u16>,
        #[arg(long)]
        kind: Option<String>,
        #[arg(long)]
        body: Option<String>,
    },
    Rm {
        name: String,
    },
}

#[derive(Subcommand)]
enum SimulateCommand {
    Inject {
        session: String,
        fixture: Option<String>,
        #[arg(long, default_value_t = 1)]
        count: u32,
        #[arg(long = "inline-status")]
        inline_status: Option<u16>,
        #[arg(long = "inline-kind")]
        inline_kind: Option<String>,
        #[arg(long = "inline-body")]
        inline_body: Option<String>,
    },
    Pending {
        session: String,
    },
    Clear {
        session: String,
    },
}

#[derive(Subcommand)]
enum ProtectionCommand {
    Preset { name: String },
}

#[derive(Subcommand)]
enum MiddlewareCommand {
    /// Show middleware runtime settings, generation, and reload errors
    Status,
    /// List installed declarative middleware rules
    List,
    /// Show one installed declarative middleware rule
    Show { id: String },
    /// Validate one JSON or TOML rule file without installing it
    Validate { file: PathBuf },
    /// Validate and install one JSON or TOML rule file
    Install { file: PathBuf },
    /// Enable an installed middleware rule
    Enable { id: String },
    /// Disable an installed middleware rule
    Disable { id: String },
    /// Remove an installed middleware rule
    Rm { id: String },
    /// Atomically reload middleware from disk
    Reload,
    /// Dry-run a rule against a fixture, trace, or explicit context
    Test {
        id: String,
        #[arg(
            long,
            conflicts_with_all = ["trace", "context"],
            required_unless_present_any = ["trace", "context"]
        )]
        fixture: Option<String>,
        #[arg(long, conflicts_with_all = ["fixture", "context"])]
        trace: Option<String>,
        /// JSON or TOML AttemptResultContext file
        #[arg(long, conflicts_with_all = ["fixture", "trace"])]
        context: Option<PathBuf>,
    },
    /// List or clear active session route leases
    Leases {
        #[command(subcommand)]
        command: Option<MiddlewareLeasesCommand>,
    },
}

#[derive(Subcommand)]
enum MiddlewareLeasesCommand {
    /// Clear one active session route lease
    Clear { id: String },
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
    /// Launch connected Claude with its Alex settings: `alex wrap claude -p 'hi'`
    Claude {
        /// Args passed through verbatim to `claude`
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Launch connected Codex with the Alex profile: `alex wrap codex exec 'hi'`
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
    /// Central Alex base URL for trace upload (env: ALEXANDRIA_TRACE_URL)
    #[arg(long, alias = "alex-url")]
    trace_url: Option<String>,
    /// Correlate this wrap session under a caller-supplied run id instead of a
    /// generated one (env: ALEXANDRIA_RUN_ID). Lets an orchestrator pre-register
    /// the run and skip scraping the announced id.
    #[arg(long)]
    run_id: Option<String>,
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
    /// Merge a duplicate account into another, unifying split trace/usage history
    Merge {
        /// The duplicate account id to merge FROM (removed after the merge)
        from: String,
        /// The surviving account id to merge INTO (keeps its id and the valid login)
        into: String,
        /// Merge even if the two accounts differ in provider or email
        #[arg(long)]
        allow_mismatch: bool,
    },
}

#[derive(Subcommand)]
enum VaultCommand {
    /// Encrypt selected vault and installed harness credentials into a bundle
    Export {
        #[arg(long)]
        passphrase: String,
        #[arg(long, default_value = "all")]
        accounts: String,
        #[arg(long, default_value = "all")]
        harnesses: String,
        #[arg(long)]
        out: PathBuf,
    },
    /// Decrypt a bundle and merge its credentials into this machine
    Import {
        file: PathBuf,
        #[arg(long)]
        passphrase: String,
    },
    /// Fetch an encrypted bundle from another Alex daemon and import it
    Pull {
        #[arg(long = "from")]
        from: String,
        #[arg(long)]
        admin_key: String,
        #[arg(long)]
        passphrase: String,
        #[arg(long, default_value = "all")]
        accounts: String,
        #[arg(long, default_value = "all")]
        harnesses: String,
    },
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
enum ProviderCommand {
    /// Show each provider's transient pause state
    List,
    /// Pause all traffic to one provider
    Pause {
        /// Provider: claude|codex|grok|gemini|amp (aliases accepted)
        provider: String,
        /// Simulate provider down or a logged-out subscription
        #[arg(long, value_enum, default_value_t = ProviderPauseMode::Down)]
        mode: ProviderPauseMode,
    },
    /// Resume traffic to one provider
    Resume {
        /// Provider: claude|codex|grok|gemini|amp (aliases accepted)
        provider: String,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum ProviderPauseMode {
    Down,
    #[value(name = "logged_out")]
    LoggedOut,
}

impl ProviderPauseMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Down => "down",
            Self::LoggedOut => "logged_out",
        }
    }
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
        /// Model to request from the harness; prefixes route through Alex
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
    /// Pack an npm CLI package into Alex's frozen harness cache
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
    /// Back up trace history and captured bodies to a .tar.gz archive (offline)
    Export {
        path: PathBuf,
        /// Replace an existing archive
        #[arg(long)]
        force: bool,
    },
    /// Restore missing trace history and captured bodies from a backup (offline)
    Import {
        path: PathBuf,
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
    /// Push a locally spooled wrap run to a central Alex daemon
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
    /// How often the proactive logout watchdog checks for managed OAuth logins
    /// whose token expired while idle (and cannot silently refresh) so it can
    /// fire the re-auth notification without a live request. 0 disables it.
    #[serde(default = "default_reauth_check_minutes")]
    reauth_check_minutes: u64,
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
    #[serde(default = "default_exo_url")]
    exo_url: String,
    #[serde(default)]
    exo_enabled_models: Vec<String>,
    /// User-curated OpenRouter models advertised to `/v1/models` and injected
    /// into harnesses. Absent (a pre-curation install that implicitly exposed
    /// the entire catalog) defaults to a short list of examples; an explicit
    /// empty list exposes nothing. Never re-defaults over a user's own choice.
    #[serde(default = "alex_proxy::default_openrouter_exposed_models")]
    openrouter_exposed_models: Vec<String>,
    #[serde(default)]
    gemini_project: String,
    #[serde(default = "default_anthropic_upstream")]
    anthropic_upstream: String,
    /// Records the one-time conversion of the legacy `direct` default to `auto`.
    #[serde(default)]
    dario_mode_migrated: bool,
    #[serde(default)]
    dario_api_key: String,
    /// Explicit real Claude Code executable for Dario prompt capture.
    #[serde(default)]
    dario_claude_bin: Option<PathBuf>,
    /// Explicit Node runtime for Dario. Persisted by `alex dario fix` so a
    /// daemon started by launchd/systemd does not depend on its PATH.
    #[serde(default)]
    dario_node_path: Option<PathBuf>,
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
    /// Explicit cross-model fallback policy. Account failover does not read
    /// this: it is always enabled for reroutable capacity/server failures.
    #[serde(default)]
    substitution: alex_proxy::SubstitutionConfig,
    /// Opt-in retry/bond/cross-provider protection ladder.
    #[serde(default)]
    protection: alex_proxy::ProtectionPolicy,
    /// Notification channels. Telegram inbound control is separately opt-in
    /// per channel; URLs and tokens are never returned by the admin API.
    #[serde(default)]
    notifications: Vec<alex_proxy::notify::NotificationChannelConfig>,
    #[serde(default = "alex_proxy::notify::default_cooldown_seconds")]
    notification_cooldown_seconds: u64,
    #[serde(default = "alex_proxy::notify::default_timeout_seconds")]
    notification_timeout_seconds: u64,
}

struct ConfigProtectionPolicyPersister {
    config: Arc<std::sync::Mutex<Config>>,
}

struct ConfigExoPersister {
    config: Arc<std::sync::Mutex<Config>>,
}

impl alex_proxy::ExoConfigPersister for ConfigExoPersister {
    fn persist(&self, exo: &alex_proxy::ExoConfig) -> std::result::Result<(), String> {
        let exo = exo.clone();
        let fresh = update_config_on_disk(|config| {
            config.exo_url = exo.url.clone();
            config.exo_enabled_models = exo.enabled_models.clone();
        })
        .map_err(|error| error.to_string())?;
        sync_shared_config(&self.config, fresh);
        Ok(())
    }
}

struct ConfigOpenrouterExposedPersister {
    config: Arc<std::sync::Mutex<Config>>,
}

impl alex_proxy::OpenrouterExposedPersister for ConfigOpenrouterExposedPersister {
    fn persist(&self, exposed: &[String]) -> std::result::Result<(), String> {
        let exposed = exposed.to_vec();
        // Read-modify-write the whole config so persisting the curated list
        // never drops other sections (notifications, exo, protection, ...).
        let fresh = update_config_on_disk(|config| {
            config.openrouter_exposed_models = exposed.clone();
        })
        .map_err(|error| error.to_string())?;
        sync_shared_config(&self.config, fresh);
        Ok(())
    }
}

impl alex_proxy::ProtectionPolicyPersister for ConfigProtectionPolicyPersister {
    fn persist(&self, policy: &alex_proxy::ProtectionPolicy) -> std::result::Result<(), String> {
        let policy = policy.clone();
        let fresh = update_config_on_disk(|config| {
            config.protection = policy.clone();
        })
        .map_err(|error| error.to_string())?;
        sync_shared_config(&self.config, fresh);
        Ok(())
    }
}

struct ConfigNotificationPersister {
    config: Arc<std::sync::Mutex<Config>>,
}

impl alex_proxy::NotificationConfigPersister for ConfigNotificationPersister {
    fn persist(
        &self,
        settings: &alex_proxy::notify::NotificationSettings,
    ) -> std::result::Result<(), String> {
        let settings = settings.clone();
        let fresh = update_config_on_disk(|config| {
            config.notifications = settings.channels.clone();
            config.notification_cooldown_seconds = settings.cooldown_seconds;
            config.notification_timeout_seconds = settings.timeout_seconds;
        })
        .map_err(|error| error.to_string())?;
        sync_shared_config(&self.config, fresh);
        Ok(())
    }
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

/// Reads and persists the daemon's update channel (config.toml
/// `update_channel`) behind the `/admin/update/channel` endpoint, using the
/// same parse + persist as the CLI `--set-channel` so the two cannot diverge.
struct ConfigUpdateChannelController {
    config: Arc<std::sync::Mutex<Config>>,
}

impl alex_proxy::UpdateChannelController for ConfigUpdateChannelController {
    fn current(&self) -> String {
        self.config
            .lock()
            .map(|config| config.update_channel().as_str().to_string())
            .unwrap_or_else(|_| selfupdate::UpdateChannel::default().as_str().to_string())
    }

    fn set(&self, channel: String) -> alex_proxy::UpdateChannelSetFuture {
        let config = self.config.clone();
        Box::pin(async move {
            // Parse first so an unknown value is a client error (400), then
            // persist through the read-modify-write helper (a disk failure is a
            // 500). Reloading from disk before writing is what stops a channel
            // save from clobbering a freshly-added notification token.
            let parsed = selfupdate::UpdateChannel::parse(&channel)
                .map_err(|error| alex_proxy::UpdateChannelError::Invalid(error.to_string()))?;
            let fresh = update_config_on_disk(|config| {
                config.update_channel = parsed.as_str().to_string();
            })
            .map_err(|error| alex_proxy::UpdateChannelError::Failed(error.to_string()))?;
            sync_shared_config(&config, fresh);
            // Recompute the availability against the new channel so the next
            // `/admin/update` and the endpoint response reflect it. A failed
            // network check leaves the channel persisted and status unknown.
            let status = selfupdate::daemon_update_status_value(parsed).await.ok();
            Ok(alex_proxy::SetChannelOutcome {
                channel: parsed.as_str().to_string(),
                status,
            })
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

fn default_reauth_check_minutes() -> u64 {
    5
}

fn default_trace_body_retention_days() -> u64 {
    30
}

fn default_update_check_hours() -> u64 {
    24
}

fn default_update_channel() -> String {
    // B2: a pre-release daemon build (its own version carries `-beta`/`-rc`)
    // defaults to the beta channel, so it checks the beta feed instead of
    // comparing against the older latest *stable* and falsely reporting "up to
    // date". A stable build still defaults to stable.
    selfupdate::UpdateChannel::default_for_version(env!("CARGO_PKG_VERSION"))
        .as_str()
        .to_string()
}

fn default_upstream_stream_idle_timeout_seconds() -> u64 {
    // Long reasoning stretches are normal. Fifteen minutes only catches a
    // connection that has actually stopped producing output.
    15 * 60
}

fn default_ping_anthropic() -> String {
    "claude-sonnet-5".into()
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

fn default_exo_url() -> String {
    "http://localhost:52415".into()
}

fn default_anthropic_upstream() -> String {
    "auto".into()
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
        // Claude Code is commonly installed adjacent to a version-manager
        // Node installation, not just in ~/.local/bin.
        for root in [
            home.join(".nvm/versions/node"),
            home.join(".local/share/fnm"),
            home.join(".fnm"),
            home.join(".local/share/mise/installs/node"),
            home.join(".asdf/installs/nodejs"),
        ] {
            candidates.extend(dario::newest_versioned_bins_for_cli(&root, "claude"));
        }
        candidates.push(home.join(".volta/bin/claude"));
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
        // Haiku's subscription route remains healthy even when Anthropic
        // rate-limits the premium models used by non-Claude-Code clients.
        // Only replace this former default, never a user's chosen ping model.
        if self.ping_anthropic_model == "claude-haiku-4-5" {
            self.ping_anthropic_model = default_ping_anthropic();
        }
        // Before tri-state routing, `direct` was the implicit default. Convert
        // it exactly once, so a later explicit `dario disable` continues to win.
        if !self.dario_mode_migrated {
            if self.anthropic_upstream == "direct" {
                self.anthropic_upstream = default_anthropic_upstream();
            }
            self.dario_mode_migrated = true;
            tracing::info!("migrated legacy Dario routing default to auto");
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
            exo_url: default_exo_url(),
            exo_enabled_models: Vec::new(),
            openrouter_exposed_models: alex_proxy::default_openrouter_exposed_models(),
            gemini_project: String::new(),
            heartbeat_minutes: default_heartbeat_minutes(),
            reauth_check_minutes: default_reauth_check_minutes(),
            ping_anthropic_model: default_ping_anthropic(),
            ping_openai_model: default_ping_openai(),
            ping_xai_model: default_ping_xai(),
            anthropic_upstream: default_anthropic_upstream(),
            dario_mode_migrated: true,
            dario_api_key: String::new(),
            dario_claude_bin: None,
            dario_node_path: None,
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
            substitution: alex_proxy::SubstitutionConfig::default(),
            protection: alex_proxy::ProtectionPolicy::default(),
            notifications: Vec::new(),
            notification_cooldown_seconds: alex_proxy::notify::default_cooldown_seconds(),
            notification_timeout_seconds: alex_proxy::notify::default_timeout_seconds(),
        }
    }

    fn ping_models(&self) -> alex_proxy::PingModels {
        alex_proxy::PingModels {
            anthropic: self.ping_anthropic_model.clone(),
            openai: self.ping_openai_model.clone(),
            xai: self.ping_xai_model.clone(),
            gemini: self.ping_gemini_model.clone(),
            openrouter: self.ping_openrouter_model.clone(),
            kimi: "k3".to_string(),
        }
    }

    fn notification_settings(&self) -> alex_proxy::notify::NotificationSettings {
        alex_proxy::notify::NotificationSettings {
            channels: self.notifications.clone(),
            cooldown_seconds: self.notification_cooldown_seconds,
            timeout_seconds: self.notification_timeout_seconds,
        }
    }

    fn dario_route_should_enable(&self, has_active_anthropic_oauth: bool) -> bool {
        match self.anthropic_upstream.as_str() {
            "dario" => true,
            "direct" => false,
            "auto" => has_active_anthropic_oauth,
            _ => false,
        }
    }

    fn dario_routing_reason(&self, has_active_anthropic_oauth: bool) -> String {
        match self.anthropic_upstream.as_str() {
            "dario" => "forced on".into(),
            "direct" => "forced off".into(),
            "auto" if has_active_anthropic_oauth => {
                "auto: active Claude subscription detected".into()
            }
            "auto" => "auto: no Claude subscription".into(),
            other => format!("unrecognized mode {other:?}; routing direct"),
        }
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

/// Persist an already-validated update channel to config.toml. This is the one
/// writer shared by the CLI `alex update --set-channel` and the daemon
/// `/admin/update/channel` endpoint, so the app picker and the daemon can never
/// diverge on how the channel is normalized or stored. Callers parse the raw
/// value with `selfupdate::UpdateChannel::parse` first, keeping the parse and
/// the persist as the single source of truth for both paths.
fn persist_update_channel(config: &mut Config, channel: selfupdate::UpdateChannel) -> Result<()> {
    if config.update_channel != channel.as_str() {
        config.update_channel = channel.as_str().to_string();
        save_config(config)?;
    }
    Ok(())
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

/// Serializes every in-daemon config write so a read-modify-write can never be
/// interleaved by a second writer (which would let two writers each serialize a
/// stale whole-Config and drop each other's sections).
static CONFIG_WRITE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Parse the current config.toml from disk without the create-if-missing /
/// heal-rewrite side effects of `load_or_create_config`. Used by the
/// read-modify-write helper so a persist always starts from the authoritative
/// on-disk state rather than a possibly-stale in-memory snapshot.
fn read_config_from_disk() -> Result<Config> {
    let path = alexandria_home().join("config.toml");
    if !path.exists() {
        // No file yet (first run / tests that never saved): fall back to the
        // normal loader, which creates a default config.toml.
        return Ok(load_or_create_config()?.0);
    }
    let raw = std::fs::read_to_string(&path)?;
    let mut config: Config = toml::from_str(&raw).with_context(|| format!("parsing {path:?}"))?;
    config.heal();
    Ok(config)
}

/// The single safe primitive for persisting one section of config.toml from the
/// running daemon.
///
/// THE BUG THIS PREVENTS: `save_config` serializes the *whole* `Config`. Before
/// this helper, several writers (the notification persister, the update-channel
/// controller, the exo persister, the Dario repair path, the harness handlers)
/// each held their own in-memory `Config` and wrote it wholesale. Any writer
/// whose snapshot predated another section's save would clobber config.toml and
/// silently drop that section — e.g. saving the update channel or exo settings
/// wiped a freshly-added Telegram bot token, breaking every notification.
///
/// By reloading the latest config from disk, applying only the requested
/// mutation, and writing it back under a process-wide lock, no write can ever
/// lose a section it didn't touch. Returns the freshly-persisted `Config` so
/// callers can re-sync any in-memory handle they keep for runtime reads.
fn update_config_on_disk(mutate: impl FnOnce(&mut Config)) -> Result<Config> {
    let _guard = CONFIG_WRITE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut config = read_config_from_disk()?;
    mutate(&mut config);
    save_config(&config)?;
    Ok(config)
}

/// Refresh a daemon-held `Arc<Mutex<Config>>` with the just-persisted config so
/// any runtime reads through that handle (e.g. the update-channel loop) see the
/// latest full state rather than the stale snapshot captured at startup.
fn sync_shared_config(shared: &Arc<std::sync::Mutex<Config>>, fresh: Config) {
    if let Ok(mut guard) = shared.lock() {
        *guard = fresh;
    }
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
            "kimi" | "kimi-code" => alex_core::Provider::Kimi,
            _ => continue,
        };
        policies.push((p, v.clone()));
    }
    if !policies.is_empty() {
        vault.set_policies_blocking(policies);
    }
    Ok(vault)
}

fn bundle_selection(accounts: &str, harnesses: &str) -> BundleSelection {
    let parse = |value: &str| {
        value
            .split(',')
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string)
            .collect()
    };
    BundleSelection {
        accounts: Some(parse(accounts)),
        harnesses: Some(parse(harnesses)),
    }
}

fn print_bundle_import_summary(summary: &alex_auth::vault_bundle::ImportSummary) {
    println!(
        "imported {} vault account(s): {}",
        summary.accounts.len(),
        summary.accounts.join(", ")
    );
    println!(
        "imported {} harness credential file(s): {}",
        summary.harness_credentials.len(),
        summary.harness_credentials.join(", ")
    );
    for id in &summary.oauth_overwritten {
        eprintln!("warning: OAuth account {id} was overwritten; two always-on daemons sharing it will contend on refresh");
    }
}

async fn has_active_anthropic_oauth(vault: &Vault) -> bool {
    vault.list().await.into_iter().any(|account| {
        account.provider == alex_core::Provider::Anthropic
            && account.status == "active"
            && account.kind == "oauth"
    })
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
        "kimi" | "kimi-code" => alex_core::Provider::Kimi,
        other => anyhow::bail!("unknown provider '{other}'"),
    })
}

struct DarioGlue {
    supervisor: Arc<std::sync::RwLock<Option<Arc<dario::DarioSupervisor>>>>,
    settings: Arc<std::sync::RwLock<dario::DarioSettings>>,
    config: Arc<std::sync::Mutex<Config>>,
    route_enabled: bool,
    routing_mode: String,
    routing_reason: String,
    last_error: Arc<std::sync::Mutex<Option<String>>>,
    notification_state: Arc<std::sync::Mutex<Option<Arc<alex_proxy::AppState>>>>,
}

fn dario_issue(error: &str) -> serde_json::Value {
    let (code, message) = if dario_auth_error(error) {
        ("reauth", "Claude Code login needs re-auth")
    } else if error.contains("cannot find Node runtime") {
        (
            "node_not_found",
            "cannot find Node runtime — install Node.js or set dario_node_path",
        )
    } else if error.contains("claude binary not found") {
        (
            "claude_not_found",
            "cannot find Claude Code — install claude or set dario_claude_bin",
        )
    } else if error.contains("Anthropic") && error.contains("credential") {
        (
            "no_anthropic_creds",
            "no active Anthropic OAuth credentials",
        )
    } else {
        ("generation_failed", error)
    };
    serde_json::json!({"code": code, "message": message, "fixable": code != "no_anthropic_creds"})
}

fn dario_auth_error(error: &str) -> bool {
    error.contains("status=401") || error.contains("status 401")
}

fn dario_status_has_reauth_issue(status: &serde_json::Value) -> bool {
    let active_id = status["active_generation_id"].as_str();
    status["generations"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|generation| {
            generation["id"].as_str() == active_id
                && generation["last_probe"]["status"].as_u64() == Some(401)
        })
}

impl DarioGlue {
    fn clone_for_async(&self) -> Self {
        Self {
            supervisor: self.supervisor.clone(),
            settings: self.settings.clone(),
            config: self.config.clone(),
            route_enabled: self.route_enabled,
            routing_mode: self.routing_mode.clone(),
            routing_reason: self.routing_reason.clone(),
            last_error: self.last_error.clone(),
            notification_state: self.notification_state.clone(),
        }
    }

    fn supervisor(&self) -> Option<Arc<dario::DarioSupervisor>> {
        self.supervisor.read().unwrap().clone()
    }

    fn unavailable_status(&self) -> serde_json::Value {
        let settings = self.settings.read().unwrap();
        let error = self
            .last_error
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(|| "Dario generation has not started".into());
        serde_json::json!({
            "configured": true,
            "available": false,
            "runtime": serde_json::Value::Null,
            "runtime_version": serde_json::Value::Null,
            "runtime_path": serde_json::Value::Null,
            "resolved_node_bin": settings.node_bin,
            "resolved_claude_bin": settings.claude_bin,
            "claude_bin": settings.claude_bin,
            "route_enabled": self.route_enabled,
            "routing_mode": self.routing_mode,
            "routing_reason": self.routing_reason,
            "health": "down",
            "health_reason": error,
            "issue": dario_issue(&error),
            "active_generation_id": serde_json::Value::Null,
            "generations": [],
            "error": error,
        })
    }

    async fn retry_start(&self) -> Result<Arc<dario::DarioSupervisor>, String> {
        if let Some(sup) = self.supervisor() {
            return Ok(sup);
        }
        let mut settings = self.settings.read().unwrap().clone();
        // Re-resolve each retry: Node/Claude might have been installed after
        // the daemon began, and launchd/systemd PATH remains irrelevant.
        settings.node_bin =
            dario::resolve_dario_node_bin(self.config.lock().unwrap().dario_node_path.as_deref());
        settings.claude_bin =
            resolve_dario_claude_bin(self.config.lock().unwrap().dario_claude_bin.as_deref());
        match dario::DarioSupervisor::start(settings.clone()).await {
            Ok(sup) => {
                *self.settings.write().unwrap() = settings;
                *self.supervisor.write().unwrap() = Some(sup.clone());
                *self.last_error.lock().unwrap() = None;
                self.spawn_dario_auth_failure_listener(&sup);
                Ok(sup)
            }
            Err(error) => {
                let error = error.to_string();
                *self.settings.write().unwrap() = settings;
                *self.last_error.lock().unwrap() = Some(error.clone());
                self.emit_dario_reauth_if_needed(&error);
                Err(error)
            }
        }
    }

    fn set_notification_state(&self, state: Arc<alex_proxy::AppState>) {
        *self.notification_state.lock().unwrap() = Some(state.clone());
        if let Some(error) = self.last_error.lock().unwrap().as_deref() {
            if dario_auth_error(error) {
                alex_proxy::emit_dario_reauth_notification(&state);
            }
        }
        if let Some(sup) = self.supervisor() {
            self.spawn_dario_auth_failure_listener(&sup);
        }
    }

    fn emit_dario_reauth_if_needed(&self, error: &str) {
        if !dario_auth_error(error) {
            return;
        }
        if let Some(state) = self.notification_state.lock().unwrap().clone() {
            alex_proxy::emit_dario_reauth_notification(&state);
        }
    }

    fn spawn_dario_auth_failure_listener(&self, supervisor: &Arc<dario::DarioSupervisor>) {
        let Some(state) = self.notification_state.lock().unwrap().clone() else {
            return;
        };
        let mut failures = supervisor.subscribe_auth_failures();
        tokio::spawn(async move {
            loop {
                match failures.recv().await {
                    Ok(()) => alex_proxy::emit_dario_reauth_notification(&state),
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }

    fn spawn_initial_retry(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            let mut delay = std::time::Duration::from_secs(5);
            loop {
                tokio::time::sleep(delay).await;
                let Some(glue) = weak.upgrade() else { break };
                if glue.supervisor().is_some() {
                    break;
                }
                if let Err(error) = glue.retry_start().await {
                    tracing::warn!(%error, "Dario initial generation retry failed");
                    delay = (delay * 2).min(std::time::Duration::from_secs(60));
                } else {
                    tracing::info!("Dario initial generation retry succeeded");
                    break;
                }
            }
        });
    }

    async fn repair(&self) -> serde_json::Value {
        let node = dario::resolve_dario_node_bin(None);
        let claude = resolve_dario_claude_bin(None);
        let Some(node) = node else {
            return serde_json::json!({"fixed": false, "node_bin": null, "claude_bin": claude, "fixable": true, "message": "cannot find Node runtime — install Node.js or set dario_node_path"});
        };
        let Some(claude) = claude else {
            return serde_json::json!({"fixed": false, "node_bin": node, "claude_bin": null, "fixable": true, "message": "cannot find Claude Code — install claude or set dario_claude_bin"});
        };
        {
            let node = node.clone();
            let claude = claude.clone();
            match update_config_on_disk(|config| {
                config.dario_node_path = Some(node.clone());
                config.dario_claude_bin = Some(claude.clone());
            }) {
                Ok(fresh) => sync_shared_config(&self.config, fresh),
                Err(error) => {
                    return serde_json::json!({"fixed": false, "node_bin": node, "claude_bin": claude, "fixable": true, "message": format!("found Dario runtimes but could not save config: {error}")})
                }
            }
        }
        if let Some(sup) = self.supervisor() {
            if dario_status_has_reauth_issue(&sup.status()) {
                return match sup.restart().await {
                    Ok(result) => serde_json::json!({
                        "fixed": true,
                        "node_bin": node,
                        "claude_bin": claude,
                        "message": format!("Claude Code re-auth complete; started fresh generation {}", result["generation_id"].as_str().unwrap_or("")),
                    }),
                    Err(error) => serde_json::json!({
                        "fixed": false,
                        "node_bin": node,
                        "claude_bin": claude,
                        "fixable": true,
                        "message": error.to_string(),
                    }),
                };
            }
            match sup.ensure_active().await {
                Ok(active) => {
                    return serde_json::json!({"fixed": true, "node_bin": node, "claude_bin": claude, "message": format!("saved Dario runtime paths; generation {} is ready", active.generation_id)})
                }
                Err(error) => {
                    return serde_json::json!({"fixed": false, "node_bin": node, "claude_bin": claude, "fixable": true, "message": error})
                }
            }
        }
        match self.retry_start().await {
            Ok(sup) => {
                serde_json::json!({"fixed": sup.active().is_some(), "node_bin": node, "claude_bin": claude, "message": "saved Dario runtime paths and started a fresh generation"})
            }
            Err(error) => {
                serde_json::json!({"fixed": false, "node_bin": node, "claude_bin": claude, "fixable": true, "message": format!("saved Dario runtime paths but generation failed to start: {error}")})
            }
        }
    }
}

impl alex_proxy::DarioRouter for DarioGlue {
    fn routes_requests(&self) -> bool {
        self.route_enabled
    }

    fn active(&self) -> Option<alex_proxy::DarioActive> {
        self.supervisor()?
            .active()
            .map(|a| alex_proxy::DarioActive {
                generation_id: a.generation_id,
                base_url: a.base_url,
                api_key: a.api_key,
            })
    }

    fn ensure_active(&self) -> alex_proxy::DarioEnsureFuture {
        let glue = self.clone_for_async();
        Box::pin(async move {
            glue.retry_start()
                .await?
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
        self.supervisor()?
            .begin_request(generation_id)
            .map(|g| Box::new(g) as Box<dyn std::any::Any + Send>)
    }

    fn prepare_model(&self, model: &str) -> alex_proxy::DarioPrepareFuture {
        let supervisor = self.supervisor();
        let model = model.to_string();
        Box::pin(async move {
            match supervisor {
                Some(supervisor) => match supervisor.prepare_model(&model).await {
                    Some(reason) => alex_proxy::DarioPrepare::DirectFallback { reason },
                    None => alex_proxy::DarioPrepare::ServeThroughDario,
                },
                None => alex_proxy::DarioPrepare::Unavailable {
                    reason: "no healthy Dario generation".into(),
                },
            }
        })
    }

    fn probe(&self, model: &str) -> alex_proxy::DarioProbeFuture {
        let supervisor = self.supervisor();
        let model = model.to_string();
        Box::pin(async move {
            supervisor
                .ok_or_else(|| "no healthy Dario generation".to_string())?
                .through_dario_probe(&model)
                .await
        })
    }

    fn status(&self) -> serde_json::Value {
        let Some(sup) = self.supervisor() else {
            return self.unavailable_status();
        };
        let mut status = sup.status();
        status["route_enabled"] = serde_json::json!(self.route_enabled);
        status["routing_mode"] = serde_json::json!(self.routing_mode);
        status["routing_reason"] = serde_json::json!(self.routing_reason);
        status["resolved_node_bin"] = status["runtime_path"].clone();
        status["resolved_claude_bin"] = status["claude_bin"].clone();
        status["issue"] = if dario_status_has_reauth_issue(&status) {
            serde_json::json!({"code": "reauth", "message": "Claude Code login needs re-auth", "fixable": true})
        } else {
            status["health_reason"]
                .as_str()
                .map(dario_issue)
                .unwrap_or(serde_json::Value::Null)
        };
        status
    }

    fn suspect(&self, generation_id: &str) {
        if let Some(sup) = self.supervisor() {
            sup.suspect(generation_id);
        }
    }
}

fn dario_admin_router(
    glue: Arc<DarioGlue>,
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
        State(glue): State<Arc<DarioGlue>>,
        AxPath(gen_id): AxPath<String>,
        Query(q): Query<std::collections::HashMap<String, String>>,
    ) -> axum::response::Response {
        let lines = q
            .get("lines")
            .and_then(|s| s.parse().ok())
            .unwrap_or(200usize)
            .min(2000);
        let status = alex_proxy::DarioRouter::status(glue.as_ref());
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

    async fn restart(State(glue): State<Arc<DarioGlue>>) -> axum::response::Response {
        let Some(sup) = glue.supervisor() else {
            return (
                axum::http::StatusCode::BAD_GATEWAY,
                axum::Json(glue.unavailable_status()),
            )
                .into_response();
        };
        match sup.restart().await {
            Ok(v) => axum::Json(v).into_response(),
            Err(e) => (
                axum::http::StatusCode::BAD_GATEWAY,
                axum::Json(serde_json::json!({"error": e.to_string()})),
            )
                .into_response(),
        }
    }

    async fn update(State(glue): State<Arc<DarioGlue>>) -> axum::response::Response {
        let Some(sup) = glue.supervisor() else {
            return (
                axum::http::StatusCode::BAD_GATEWAY,
                axum::Json(glue.unavailable_status()),
            )
                .into_response();
        };
        match sup.update_now().await {
            Ok(v) => axum::Json(v).into_response(),
            Err(e) => (
                axum::http::StatusCode::BAD_GATEWAY,
                axum::Json(serde_json::json!({"error": e.to_string()})),
            )
                .into_response(),
        }
    }

    async fn repair(State(glue): State<Arc<DarioGlue>>) -> axum::response::Response {
        axum::Json(glue.repair().await).into_response()
    }

    axum::Router::new()
        .route("/admin/dario/restart", post(restart))
        .route("/admin/dario/update", post(update))
        .route("/admin/dario/repair", post(repair))
        .route("/admin/dario/logs/{generation_id}", get(logs))
        .route_layer(axum::middleware::from_fn_with_state(
            local_key,
            require_local_key,
        ))
        .with_state(glue)
}

const HARNESS_CACHE_FRESH_MS: i64 = 60_000;

#[derive(Default)]
struct HarnessListCacheState {
    scope: Option<PathBuf>,
    harnesses: Option<Vec<harness_connect::HarnessStatus>>,
    checked_ms: i64,
    refreshing: bool,
    last_refresh_succeeded: bool,
}

struct HarnessListCache {
    state: tokio::sync::Mutex<HarnessListCacheState>,
    refreshed: tokio::sync::watch::Sender<u64>,
}

static HARNESS_LIST_CACHE: std::sync::OnceLock<Arc<HarnessListCache>> = std::sync::OnceLock::new();

fn harness_list_cache() -> &'static Arc<HarnessListCache> {
    HARNESS_LIST_CACHE.get_or_init(|| {
        let (refreshed, _) = tokio::sync::watch::channel(0);
        Arc::new(HarnessListCache {
            state: tokio::sync::Mutex::new(HarnessListCacheState::default()),
            refreshed,
        })
    })
}

fn harness_cache_is_fresh(checked_ms: i64, now_ms: i64) -> bool {
    checked_ms > 0 && now_ms.saturating_sub(checked_ms) <= HARNESS_CACHE_FRESH_MS
}

async fn run_claimed_harness_refresh(
    config: Config,
) -> Result<(Vec<harness_connect::HarnessStatus>, i64)> {
    let result = harness_connect::harness_statuses(&config, None, true)
        .await
        .map(|harnesses| (harnesses, now_ms()));
    let cache = harness_list_cache();
    {
        let mut guard = cache.state.lock().await;
        guard.last_refresh_succeeded = result.is_ok();
        if let Ok((harnesses, checked_ms)) = &result {
            guard.scope = Some(config.data_dir.clone());
            guard.harnesses = Some(harnesses.clone());
            guard.checked_ms = *checked_ms;
        }
        guard.refreshing = false;
    }
    cache.refreshed.send_modify(|generation| *generation += 1);
    result
}

async fn refresh_harness_cache(
    config: Config,
) -> Result<(Vec<harness_connect::HarnessStatus>, i64)> {
    let scope = config.data_dir.clone();
    loop {
        let wait_for_refresh = {
            let cache = harness_list_cache();
            let mut guard = cache.state.lock().await;
            if guard.refreshing {
                Some(cache.refreshed.subscribe())
            } else {
                guard.refreshing = true;
                None
            }
        };
        if let Some(mut wait_for_refresh) = wait_for_refresh {
            let _ = wait_for_refresh.changed().await;
            let cache = harness_list_cache();
            let guard = cache.state.lock().await;
            if guard.last_refresh_succeeded && guard.scope.as_ref() == Some(&scope) {
                if let Some(harnesses) = guard.harnesses.clone() {
                    return Ok((harnesses, guard.checked_ms));
                }
            }
            continue;
        }
        return run_claimed_harness_refresh(config).await;
    }
}

fn harness_cache_response(
    result: Result<(Vec<harness_connect::HarnessStatus>, i64)>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    match result {
        Ok((harnesses, checked_ms)) => axum::Json(serde_json::json!({
            "harnesses": harnesses,
            "checked_ms": checked_ms,
        }))
        .into_response(),
        Err(error_value) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            axum::Json(serde_json::json!({"error": error_value.to_string()})),
        )
            .into_response(),
    }
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

    async fn state_models(state: &alex_proxy::AppState) -> Vec<String> {
        let mut ids = state.store.pricing_models();
        // Provider catalogs advertised in /v1/models must also reach the
        // app-driven connect / refresh-config path; otherwise a provider added
        // after a harness was connected (e.g. Kimi, or a newly-curated
        // OpenRouter model) never lands in that harness's model list on
        // reconnect. Mirror the daemon `/v1/models` handler: refresh the
        // OpenRouter catalog and fold in only the user-curated exposed subset.
        alex_proxy::refresh_openrouter_models(state).await;
        ids.extend(alex_proxy::openrouter_exposed_catalog(state));
        ids.extend(alex_proxy::exo_catalog_models(state));
        ids.extend(alex_proxy::kimi_catalog_models(state).await);
        for (alias, _) in alex_core::model_aliases() {
            ids.push((*alias).to_string());
        }
        let mut filtered = harness_connect::filter_model_ids(ids);
        // Advertise all providers' models alphabetically (case-insensitive) so
        // the harness picker matches the daemon `/v1/models` ordering.
        alex_proxy::sort_model_ids(&mut filtered);
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

    async fn list(
        Query(q): Query<std::collections::HashMap<String, String>>,
    ) -> axum::response::Response {
        let (config, _) = match load_or_create_config() {
            Ok(value) => value,
            Err(e) => return error(axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        };
        if q.get("refresh").is_some_and(|value| value == "1") {
            return harness_cache_response(refresh_harness_cache(config).await);
        }

        let scope = config.data_dir.clone();
        let now = now_ms();
        let (cached, start_refresh) = {
            let cache = harness_list_cache();
            let mut guard = cache.state.lock().await;
            let cached = (guard.scope.as_ref() == Some(&scope))
                .then(|| {
                    guard
                        .harnesses
                        .clone()
                        .map(|harnesses| (harnesses, guard.checked_ms))
                })
                .flatten();
            let stale = cached
                .as_ref()
                .is_some_and(|(_, checked_ms)| !harness_cache_is_fresh(*checked_ms, now));
            let start_refresh = stale && !guard.refreshing;
            if start_refresh {
                guard.refreshing = true;
            }
            (cached, start_refresh)
        };

        if let Some((harnesses, checked_ms)) = cached {
            if start_refresh {
                tokio::spawn(async move {
                    if let Err(error_value) = run_claimed_harness_refresh(config).await {
                        tracing::debug!(%error_value, "background harness status refresh failed");
                    }
                });
            }
            return harness_cache_response(Ok((harnesses, checked_ms)));
        }

        harness_cache_response(refresh_harness_cache(config).await)
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
        let models = state_models(&state).await;
        let codex_catalog = if name == "codex" {
            let Some(binary) = status.binary.as_deref() else {
                return error(
                    axum::http::StatusCode::BAD_REQUEST,
                    "codex is not installed",
                );
            };
            match harness_connect::codex_model_catalog(std::path::Path::new(binary), &models).await
            {
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
            "kimi" => harness_connect::write_kimi_connection(
                config_dir,
                state.base_url.clone(),
                key_id,
                key,
                models,
                status.version,
            ),
            // A connect-capable harness without a writer is a programming error,
            // but panicking here aborts the HTTP connection mid-request and the
            // caller (the app) surfaces it as a "network connection lost". Return
            // a clean 500 instead so the failure is legible and the connection
            // stays intact.
            other => {
                return error(
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("harness '{other}' has no connection writer"),
                )
            }
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
            } else if name == "kimi" {
                harness_connect::plan_kimi_disconnect(&config_dir, &keys)
            } else {
                harness_connect::plan_disconnect(&config_dir, &keys)
            };
            return axum::Json(plan).into_response();
        }
        let config_path = match name.as_str() {
            "claude" => config_dir.join("alexandria-settings.json"),
            "codex" => config_dir.join("config.toml"),
            "grok" => config_dir.join("config.toml"),
            "kimi" => config_dir.join("config.toml"),
            "amp" => config_dir.join("plugins").join("alexandria.ts"),
            _ => config_dir.join("models.json"),
        };
        let previous_models = match name.as_str() {
            "claude" => harness_connect::read_claude_model_ids(&config_dir),
            "codex" => harness_connect::read_codex_model_ids(&config_dir),
            "grok" => harness_connect::read_grok_model_ids(&config_dir),
            "kimi" => harness_connect::read_kimi_model_ids(&config_dir),
            "amp" => Vec::new(),
            _ => harness_connect::read_pi_model_ids(&config_dir),
        };
        let disconnected = match name.as_str() {
            "claude" => harness_connect::disconnect_claude_config(&config_dir),
            "codex" => harness_connect::disconnect_codex_config(&config_dir),
            "grok" => harness_connect::disconnect_grok_config(&config_dir),
            "kimi" => harness_connect::disconnect_kimi_config(&config_dir),
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
            "kimi" => harness_connect::read_kimi_api_key(&config_dir),
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
        let models = state_models(&state).await;
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
                match harness_connect::codex_model_catalog(std::path::Path::new(binary), &models)
                    .await
                {
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
        } else if name == "kimi" {
            harness_connect::write_kimi_connection(
                config_dir,
                state.base_url.clone(),
                key_id,
                api_key,
                models,
                status.version,
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
        let remove = body.binary.is_none() && body.config_dir.is_none();
        let HarnessOverrideBody { binary, config_dir } = body;
        // Read-modify-write so writing one harness override never clobbers other
        // config sections (e.g. the notifications a concurrent save just added).
        let config = match update_config_on_disk(move |config| {
            if remove {
                config.harness_overrides.remove(&name);
            } else {
                config
                    .harness_overrides
                    .insert(name, HarnessOverride { binary, config_dir });
            }
        }) {
            Ok(config) => config,
            Err(e) => return error(axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        };
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
        let (config, _) = match load_or_create_config() {
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
        let enabled = body.enabled;
        // Read-modify-write so this tool-capture flag write can't drop another
        // section (notifications, update channel, ...) from a stale snapshot.
        if let Err(e) = update_config_on_disk(move |config| {
            config.harness_tool_capture.insert(name, enabled);
        }) {
            return error(axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
        }
        axum::Json(serde_json::json!({"tool_capture_enabled": enabled})).into_response()
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
    bind_daemon_listener_named(host, port, "alexandria-primary").await
}

async fn bind_daemon_listener_named(
    host: &str,
    port: u16,
    socket_name: &str,
) -> Result<tokio::net::TcpListener> {
    if let Some(listener) = launchd_activated_listener(socket_name)? {
        return Ok(listener);
    }
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

/// A launchd socket is opt-in.  A daemon started directly (including Linux,
/// systemd, tests, and old launchd plists) always keeps the normal bind path.
#[cfg(target_os = "macos")]
fn launchd_socket_activation_requested() -> bool {
    cfg!(target_os = "macos")
        && std::env::var_os("ALEXANDRIA_LAUNCHD_SOCKET_ACTIVATION").as_deref()
            == Some(std::ffi::OsStr::new("1"))
}

#[cfg(target_os = "macos")]
fn launchd_activated_listener(socket_name: &str) -> Result<Option<tokio::net::TcpListener>> {
    use std::os::fd::FromRawFd;

    if !launchd_socket_activation_requested() {
        return Ok(None);
    }

    unsafe extern "C" {
        fn launch_activate_socket(
            name: *const std::ffi::c_char,
            fds: *mut *mut std::ffi::c_int,
            count: *mut usize,
        ) -> std::ffi::c_int;
        fn free(ptr: *mut std::ffi::c_void);
        fn close(fd: std::ffi::c_int) -> std::ffi::c_int;
    }

    let name = std::ffi::CString::new(socket_name).context("invalid launchd socket name")?;
    let mut fds: *mut std::ffi::c_int = std::ptr::null_mut();
    let mut count = 0usize;
    let status = unsafe { launch_activate_socket(name.as_ptr(), &mut fds, &mut count) };
    if status != 0 || fds.is_null() || count != 1 {
        if !fds.is_null() {
            for index in 0..count {
                unsafe { close(*fds.add(index)) };
            }
            unsafe { free(fds.cast()) };
        }
        anyhow::bail!(
            "launchd did not provide exactly one {socket_name} socket (status {status}, count {count})"
        );
    }
    let fd = unsafe { *fds };
    unsafe { free(fds.cast()) };
    let listener = unsafe { std::net::TcpListener::from_raw_fd(fd) };
    listener
        .set_nonblocking(true)
        .context("setting launchd listener nonblocking")?;
    let listener =
        tokio::net::TcpListener::from_std(listener).context("adopting launchd listener")?;
    Ok(Some(listener))
}

#[cfg(not(target_os = "macos"))]
fn launchd_activated_listener(_socket_name: &str) -> Result<Option<tokio::net::TcpListener>> {
    // This is intentionally a fallback, rather than an error: the same daemon
    // binary is used by standalone and systemd deployments.
    Ok(None)
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
                "a pre-minted harness key requires --url or ALEXANDRIA_URL; this avoids reading local Alex config",
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
    let (config, _) = load_or_create_config()?;

    match command {
        Command::Daemon {
            host,
            port,
            background,
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
            let has_active_anthropic_oauth = has_active_anthropic_oauth(&vault).await;
            let dario_route_enabled = config.dario_route_should_enable(has_active_anthropic_oauth);
            let dario_routing_reason = config.dario_routing_reason(has_active_anthropic_oauth);
            let dario_routing_mode = config.anthropic_upstream.clone();
            // Dario normally installs on demand when its supervisor creates a
            // generation. When routing has been selected, preflight that work so
            // the first premium Claude ping/request is less likely to fall back
            // direct while its generation warms. A failure remains non-fatal:
            // the supervisor's existing repair/direct-fallback path handles it.
            let _bootstrap_error = if dario_route_enabled {
                match dario::bootstrap(config.data_dir.join("dario"), config.dario_version.clone())
                    .await
                {
                    Ok(result) => {
                        tracing::info!(
                            version = %result.version,
                            already_installed = result.already_installed,
                            "Dario startup bootstrap complete"
                        );
                        None
                    }
                    Err(error) => {
                        let error = error.to_string();
                        tracing::warn!(%error, "Dario startup bootstrap failed; continuing with on-demand repair");
                        Some(error)
                    }
                }
            } else {
                None
            };
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
                node_bin: dario::resolve_dario_node_bin(config.dario_node_path.as_deref()),
            };
            let initial_start = dario::DarioSupervisor::start(settings.clone()).await;
            let initial_error = initial_start.as_ref().err().map(ToString::to_string);
            let initial_supervisor = initial_start.ok();
            let glue = Arc::new(DarioGlue {
                supervisor: Arc::new(std::sync::RwLock::new(initial_supervisor.clone())),
                settings: Arc::new(std::sync::RwLock::new(settings)),
                config: Arc::new(std::sync::Mutex::new(config.clone())),
                route_enabled: dario_route_enabled,
                routing_mode: dario_routing_mode.clone(),
                routing_reason: dario_routing_reason.clone(),
                last_error: Arc::new(std::sync::Mutex::new(initial_error.clone())),
                notification_state: Arc::new(std::sync::Mutex::new(None)),
            });
            let (dario_router, supervisor) = match initial_supervisor {
                Some(sup) => {
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
                        Some(glue.clone() as Arc<dyn alex_proxy::DarioRouter>),
                        Some(sup),
                    )
                }
                None => {
                    let e = initial_error.unwrap_or_else(|| "Dario failed to start".into());
                    if dario_route_enabled {
                        eprintln!(
                            "dario: failed to start ({e}); non-Claude-Code Anthropic traffic will fail closed"
                        );
                    } else {
                        eprintln!("dario: unavailable ({e}); Anthropic traffic remains direct");
                    }
                    (Some(glue.clone() as Arc<dyn alex_proxy::DarioRouter>), None)
                }
            };
            if supervisor.is_none() {
                glue.spawn_initial_retry();
            }
            let state = alex_proxy::build_state_with_substitution(
                config.local_key.clone(),
                vault,
                store,
                dario_router,
                daemon_connect_base_url(&host, port),
                config.upstream_stream_idle_timeout(),
                config.substitution.clone(),
            );
            alex_proxy::set_notifications(&state, config.notification_settings());
            glue.set_notification_state(state.clone());
            alex_proxy::set_protection_policy(&state, config.protection.clone());
            alex_proxy::set_exo_config(
                &state,
                alex_proxy::ExoConfig {
                    url: config.exo_url.clone(),
                    enabled_models: config.exo_enabled_models.clone(),
                },
            );
            alex_proxy::set_openrouter_exposed_models(
                &state,
                config.openrouter_exposed_models.clone(),
            );
            let daemon_config = Arc::new(std::sync::Mutex::new(config.clone()));
            alex_proxy::set_protection_policy_persister(
                &state,
                Arc::new(ConfigProtectionPolicyPersister {
                    config: daemon_config.clone(),
                }),
            );
            alex_proxy::set_notification_config_persister(
                &state,
                Arc::new(ConfigNotificationPersister {
                    config: daemon_config.clone(),
                }),
            );
            alex_proxy::set_update_channel_controller(
                &state,
                Arc::new(ConfigUpdateChannelController {
                    config: daemon_config.clone(),
                }),
            );
            alex_proxy::set_exo_config_persister(
                &state,
                Arc::new(ConfigExoPersister {
                    // Share the one daemon config handle so every persister
                    // mutates the same in-memory Config (no divergent clones).
                    config: daemon_config.clone(),
                }),
            );
            alex_proxy::set_openrouter_exposed_persister(
                &state,
                Arc::new(ConfigOpenrouterExposedPersister {
                    config: daemon_config.clone(),
                }),
            );
            alex_proxy::set_fixture_dir(&state, config.data_dir.join("fixtures"));
            alex_proxy::set_daemon_updater(
                &state,
                Arc::new(SelfUpdateApplier {
                    config: config.clone(),
                }),
            );
            alex_proxy::set_reset_handler(&state, Arc::new(reset::DaemonResetHandler));
            {
                let quota_vault = state.vault.clone();
                tokio::spawn(async move {
                    let mut interval =
                        tokio::time::interval(std::time::Duration::from_secs(5 * 60));
                    loop {
                        interval.tick().await;
                        for (account_id, result) in alex_auth::login::refresh_due_codex_usage(
                            &quota_vault,
                            alex_auth::login::CODEX_USAGE_REFRESH_MAX_AGE_MS,
                        )
                        .await
                        {
                            match result {
                                Ok(()) => tracing::debug!(
                                    account = %account_id,
                                    "refreshed Codex allowance snapshot"
                                ),
                                Err(error) => tracing::warn!(
                                    account = %account_id,
                                    %error,
                                    "Codex allowance refresh failed; retaining previous snapshot"
                                ),
                            }
                        }
                    }
                });
                eprintln!("codex quota refresh: every 5m (usage endpoint only)");
            }
            if config.update_check_hours > 0 {
                let update_status = state.update_status.clone();
                let hours = config.update_check_hours;
                // Read the channel from the shared config each tick so a channel
                // change via `/admin/update/channel` (or the CLI) steers the next
                // periodic check instead of a value captured once at startup.
                let channel_config = daemon_config.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                    loop {
                        let update_channel = channel_config
                            .lock()
                            .map(|config| config.update_channel())
                            .unwrap_or_default();
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
            if config.reauth_check_minutes > 0 {
                let watch_state = state.clone();
                let minutes = config.reauth_check_minutes;
                tokio::spawn(async move {
                    let mut interval =
                        tokio::time::interval(std::time::Duration::from_secs(minutes * 60));
                    interval.tick().await;
                    loop {
                        interval.tick().await;
                        alex_proxy::reauth_watch_once(&watch_state).await;
                    }
                });
                eprintln!(
                    "reauth watchdog: every {}m (set reauth_check_minutes = 0 to disable)",
                    config.reauth_check_minutes
                );
            } else {
                eprintln!(
                    "reauth watchdog: disabled (set reauth_check_minutes in config.toml to enable)"
                );
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
            if let Err(error_value) = refresh_harness_cache(config.clone()).await {
                tracing::warn!(%error_value, "could not warm harness status cache at startup");
            }
            let mut app = alex_proxy::router(state.clone());
            app = app.merge(harness_admin_router(state.clone()));
            app = app.merge(dario_admin_router(glue, state.local_key.clone()));
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
                eprintln!("\nWARNING: Alex could not bind its configured address {host}:{port}: {reason}");
                eprintln!("WARNING: Falling back to loopback (127.0.0.1) so the daemon remains available locally.");
                eprintln!("WARNING: The configured address was left unchanged; choose an available interface and restart to expose it again.\n");
            }
            let local_listener = if requires_explicit_loopback_listener(&bound_host) {
                Some(
                    bind_daemon_listener_named("127.0.0.1", port, "alexandria-local")
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
            let (telegram_command_supervisor, telegram_command_count) =
                telegram::spawn_command_poller_supervisor(
                    daemon_config.clone(),
                    state.notifications.clone(),
                );
            let telegram_command_tasks = vec![telegram_command_supervisor];
            if telegram_command_count > 0 {
                eprintln!(
                    "telegram commands: enabled for {} allowlisted channel(s)",
                    telegram_command_count
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
            for task in telegram_command_tasks {
                task.abort();
            }
            serve_result?;
            if let Some(sup) = supervisor {
                sup.shutdown().await;
            }
        }
        Command::LaunchdRestartHelper => {
            restart_launchd_daemon(&config, false).await?;
        }
        Command::Vault { command } => match command {
            VaultCommand::Export {
                passphrase,
                accounts,
                harnesses,
                out,
            } => {
                let vault = open_vault(&config)?;
                let selection = bundle_selection(&accounts, &harnesses);
                let bundle = export_bundle(&vault, selection).await?;
                let account_count = bundle.accounts.len();
                let harness_count = bundle.harness_credentials.len();
                let encrypted = encrypt_bundle(&bundle, &passphrase)?;
                std::fs::write(&out, serde_json::to_vec(&encrypted)?)?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&out, std::fs::Permissions::from_mode(0o600))?;
                }
                println!("exported {account_count} vault account(s) and {harness_count} harness credential file(s) to {}", out.display());
            }
            VaultCommand::Import { file, passphrase } => {
                let blob = serde_json::from_slice(&std::fs::read(&file)?)
                    .context("reading encrypted vault bundle")?;
                let vault = open_vault(&config)?;
                let summary = import_bundle(&vault, decrypt_bundle(&blob, &passphrase)?).await?;
                print_bundle_import_summary(&summary);
            }
            VaultCommand::Pull {
                from,
                admin_key,
                passphrase,
                accounts,
                harnesses,
            } => {
                let base = from.trim_end_matches('/');
                let response = reqwest::Client::new().post(format!("{base}/admin/vault/export"))
                    .header("x-api-key", admin_key)
                    .json(&serde_json::json!({"passphrase": passphrase, "selection": bundle_selection(&accounts, &harnesses)}))
                    .send().await?.error_for_status()?;
                let blob = response.json().await?;
                let vault = open_vault(&config)?;
                let summary = import_bundle(&vault, decrypt_bundle(&blob, &passphrase)?).await?;
                print_bundle_import_summary(&summary);
            }
        },
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
            AuthCommand::Merge {
                from,
                into,
                allow_mismatch,
            } => {
                // The merge rewrites both the trace database and the in-memory
                // vault, so it runs inside the daemon that owns both.
                let base = config.base_url();
                let key = config.local_key.as_str();
                let response = reqwest::Client::new()
                    .post(format!("{base}/admin/accounts/merge"))
                    .header("x-api-key", key)
                    .json(&json!({"from": from, "into": into, "allow_mismatch": allow_mismatch}))
                    .send()
                    .await
                    .with_context(|| {
                        format!("could not reach the alexandria daemon at {base} — is it running?")
                    })?;
                let status = response.status();
                let body: serde_json::Value = response.json().await.unwrap_or_default();
                if !status.is_success() {
                    anyhow::bail!(
                        "merge failed ({status}): {}",
                        body["error"].as_str().unwrap_or("unknown error")
                    );
                }
                let rows = &body["rows"];
                let moved = rows["traces_account_id"].as_u64().unwrap_or(0);
                println!(
                    "merged {from} into {} — {moved} traces re-keyed{}",
                    body["merged_into"].as_str().unwrap_or(&into),
                    body["adopted_credentials_from"]
                        .as_str()
                        .map(|src| format!(", adopted the login from {src}"))
                        .unwrap_or_default()
                );
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
            Some(TracesCommand::Export { path, force }) => {
                traces_backup_export_cmd(&config, &path, force)?;
            }
            Some(TracesCommand::Import { path }) => {
                traces_backup_import_cmd(&config, &path)?;
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
        Command::Up {
            harness,
            url,
            key,
            model,
            version,
            no_launch,
            yes: _,
            args,
        } => {
            up_cmd(
                &config,
                &harness,
                url.as_deref(),
                key.as_deref(),
                &model,
                version.as_deref(),
                no_launch,
                args,
            )
            .await?;
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
                                | alex_core::Provider::Kimi
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
                println!("{} {}", ui::gold(ui::diamond()), ui::bold(&summary));
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
                let vault = open_vault(&config)?;
                let dario_enabled =
                    config.dario_route_should_enable(has_active_anthropic_oauth(&vault).await);
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
                    dario_enabled,
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
        Command::Provider { command } => {
            let base = config.base_url();
            let key = config.local_key.as_str();
            let http = reqwest::Client::new();
            let response = match &command {
                ProviderCommand::List => {
                    http.get(format!("{base}/admin/providers"))
                        .header("x-api-key", key)
                        .send()
                        .await
                }
                ProviderCommand::Pause { provider, mode } => {
                    let provider = provider_from_cli(&provider)?;
                    http.post(format!(
                        "{base}/admin/providers/{}/pause",
                        provider.as_str()
                    ))
                    .header("x-api-key", key)
                    .json(&json!({"mode": mode.as_str()}))
                    .send()
                    .await
                }
                ProviderCommand::Resume { provider } => {
                    let provider = provider_from_cli(&provider)?;
                    http.post(format!(
                        "{base}/admin/providers/{}/resume",
                        provider.as_str()
                    ))
                    .header("x-api-key", key)
                    .send()
                    .await
                }
            }
            .with_context(|| {
                format!("could not reach the alexandria daemon at {base} — is it running?")
            })?;
            let status = response.status();
            let body: serde_json::Value = response.json().await.unwrap_or_default();
            if !status.is_success() {
                anyhow::bail!("provider command failed ({status}): {body}");
            }
            match &command {
                ProviderCommand::List => {
                    if let Some(providers) = body["providers"].as_array() {
                        for provider in providers {
                            let name = provider["provider"].as_str().unwrap_or("unknown");
                            match provider["mode"].as_str() {
                                Some(mode) => println!("{name}: paused ({mode})"),
                                None => println!("{name}: active"),
                            }
                        }
                    }
                }
                ProviderCommand::Pause { .. } => println!(
                    "{} paused ({})",
                    body["provider"].as_str().unwrap_or("provider"),
                    body["mode"].as_str().unwrap_or("down")
                ),
                ProviderCommand::Resume { .. } => println!(
                    "{} resumed",
                    body["provider"].as_str().unwrap_or("provider")
                ),
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
                    println!("restart the Alex daemon to apply the new listener");
                } else {
                    println!("daemon host is already {}", config.host);
                }
            }
        },
        Command::Service { command } => match command {
            ServiceCommand::Install => service_install(&config)?,
            ServiceCommand::Bind { target } => service_set_bind(&config, &target)?,
            ServiceCommand::Restart { force } => service_restart(&config, force).await?,
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
                persist_update_channel(&mut config, parsed)?;
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
                DarioCommand::Enable | DarioCommand::Disable | DarioCommand::Auto => {
                    let mut config = config;
                    let (mode, message) = match command {
                        DarioCommand::Enable => ("dario", "forced on"),
                        DarioCommand::Disable => ("direct", "forced off"),
                        DarioCommand::Auto => ("auto", "automatic"),
                        _ => unreachable!(),
                    };
                    config.anthropic_upstream = mode.into();
                    // Writing an explicit routing choice must never be treated as
                    // a legacy config by a later invocation.
                    config.dario_mode_migrated = true;
                    save_config(&config)?;
                    println!(
                        "Dario routing set to {message} ({mode}) in config.toml; restart Alex to apply"
                    );
                    return Ok(());
                }
                DarioCommand::Status
                | DarioCommand::Restart
                | DarioCommand::Update
                | DarioCommand::Fix => {}
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
                DarioCommand::Fix => {
                    http.post(format!("{base}/admin/dario/repair"))
                        .header("x-api-key", key)
                        .send()
                        .await
                }
                DarioCommand::Bootstrap { .. }
                | DarioCommand::Enable
                | DarioCommand::Disable
                | DarioCommand::Auto => {
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
        Command::Notify { command } => match command {
            NotifyCommand::Channels => {
                let response = daemon_get(&config, "/admin/notifications", &[]).await?;
                let body: serde_json::Value = response.json().await?;
                println!("{}", serde_json::to_string_pretty(&body)?);
            }
            NotifyCommand::Commands {
                enable,
                disable: _,
                channel,
            } => {
                let channel_id = resolve_notification_channel(&config, channel.as_deref()).await?;
                let (status, response) = daemon_send(
                    &config,
                    reqwest::Method::POST,
                    "/admin/notifications/commands",
                    Some(json!({"channel_id": channel_id, "allow_commands": enable})),
                )
                .await?;
                println!("{}", serde_json::to_string_pretty(&response)?);
                if !status.is_success() {
                    anyhow::bail!("notification commands update failed: {status}");
                }
            }
            NotifyCommand::Test { channel, category } => {
                let mut body = json!({});
                if let Some(channel) = channel {
                    body["channel_id"] = json!(channel);
                }
                if let Some(category) = category {
                    body["category"] = json!(category);
                }
                let (status, response) = daemon_send(
                    &config,
                    reqwest::Method::POST,
                    "/admin/notifications/test",
                    Some(body),
                )
                .await?;
                println!("{}", serde_json::to_string_pretty(&response)?);
                if !status.is_success() {
                    anyhow::bail!("notification test request failed: {status}");
                }
            }
            NotifyCommand::Log { limit } => {
                if limit == 0 {
                    anyhow::bail!("--limit must be positive");
                }
                let response = daemon_get(
                    &config,
                    "/admin/notifications/log",
                    &[("limit", limit.to_string())],
                )
                .await?;
                let body: serde_json::Value = response.json().await?;
                println!("{}", serde_json::to_string_pretty(&body)?);
            }
        },
        Command::Reauth { command } => match command {
            ReauthCommand::Start {
                provider,
                notify,
                force,
            } => {
                let (status, body) = daemon_send(
                    &config,
                    reqwest::Method::POST,
                    "/admin/auth/reauth/start",
                    Some(json!({"provider": provider, "notify": notify, "force": force})),
                )
                .await?;
                if !status.is_success() {
                    anyhow::bail!("could not start re-authentication: {body}");
                }
                let url = body["verification_uri_complete"]
                    .as_str()
                    .context("daemon response omitted authorization URL")?;
                println!("{url}");
                if notify {
                    println!(
                        "notification: {}",
                        if body["notification_sent"].as_bool() == Some(true) {
                            "sent"
                        } else {
                            "not sent"
                        }
                    );
                }
            }
            ReauthCommand::Submit { input } => {
                let (status, body) = daemon_send(
                    &config,
                    reqwest::Method::POST,
                    "/admin/auth/reauth/submit",
                    Some(json!({"input": input})),
                )
                .await?;
                println!("{}", serde_json::to_string_pretty(&body)?);
                if !status.is_success() || body["ok"].as_bool() != Some(true) {
                    anyhow::bail!("re-authentication was not completed");
                }
            }
            ReauthCommand::Status { provider } => {
                let params = provider
                    .map(|provider| vec![("provider", provider)])
                    .unwrap_or_default();
                let response = daemon_get(&config, "/admin/auth/reauth/status", &params).await?;
                let body: serde_json::Value = response.json().await?;
                println!("{}", serde_json::to_string_pretty(&body)?);
            }
        },
        Command::Fixtures { command } => match command {
            FixturesCommand::List => {
                let response = daemon_get(&config, "/admin/fixtures", &[]).await?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&response.json::<serde_json::Value>().await?)?
                );
            }
            FixturesCommand::Show { name } => {
                let response = daemon_get(&config, &format!("/admin/fixtures/{name}"), &[]).await?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&response.json::<serde_json::Value>().await?)?
                );
            }
            FixturesCommand::Rm { name } => {
                let (status, body) = daemon_send(
                    &config,
                    reqwest::Method::DELETE,
                    &format!("/admin/fixtures/{name}"),
                    None,
                )
                .await?;
                if !status.is_success() {
                    anyhow::bail!("fixture delete failed: {body}");
                }
            }
            FixturesCommand::Save {
                name,
                provider,
                from_trace,
                status,
                kind,
                body,
            } => {
                let payload = if let Some(trace_id) = from_trace {
                    json!({"name": name, "from_trace_id": trace_id, "kind": kind.unwrap_or_else(|| "resp".into())})
                } else {
                    let body =
                        body.context("--body @file is required unless --from-trace is used")?;
                    let body = if let Some(path) = body.strip_prefix('@') {
                        std::fs::read_to_string(path)
                            .with_context(|| format!("reading fixture body {path}"))?
                    } else {
                        body
                    };
                    json!({"name": name, "provider": provider, "status": status.context("--status is required")?, "error_kind": kind.context("--kind is required")?, "body": body})
                };
                let (status, response) = daemon_send(
                    &config,
                    reqwest::Method::POST,
                    "/admin/fixtures",
                    Some(payload),
                )
                .await?;
                println!("{}", serde_json::to_string_pretty(&response)?);
                if !status.is_success() {
                    anyhow::bail!("fixture save failed: {status}");
                }
            }
        },
        Command::Simulate { command } => match command {
            SimulateCommand::Pending { session } => {
                let response = daemon_get(
                    &config,
                    &format!("/admin/sessions/{session}/injections"),
                    &[],
                )
                .await?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&response.json::<serde_json::Value>().await?)?
                );
            }
            SimulateCommand::Clear { session } => {
                let (status, body) = daemon_send(
                    &config,
                    reqwest::Method::DELETE,
                    &format!("/admin/sessions/{session}/injections"),
                    None,
                )
                .await?;
                if !status.is_success() {
                    anyhow::bail!("clear failed: {body}");
                }
            }
            SimulateCommand::Inject {
                session,
                fixture,
                count,
                inline_status,
                inline_kind,
                inline_body,
            } => {
                let payload = if let Some(fixture) = fixture {
                    json!({"fixture": fixture, "count": count})
                } else {
                    let body = inline_body.context(
                        "fixture or --inline-status/--inline-kind/--inline-body is required",
                    )?;
                    json!({"count": count, "inline": {"status": inline_status.context("--inline-status is required")?, "error_kind": inline_kind.context("--inline-kind is required")?, "body": body}})
                };
                let (status, response) = daemon_send(
                    &config,
                    reqwest::Method::POST,
                    &format!("/admin/sessions/{session}/inject"),
                    Some(payload),
                )
                .await?;
                println!("{}", serde_json::to_string_pretty(&response)?);
                if !status.is_success() {
                    anyhow::bail!("injection failed: {status}");
                }
            }
        },
        Command::Protection { command } => match command {
            ProtectionCommand::Preset { name } => {
                if name != "anthropic-openai" {
                    anyhow::bail!("unknown protection preset '{name}'");
                }
                let mut config = config;
                config.protection.equivalencies.insert(
                    "claude-fable-5".into(),
                    BTreeMap::from([("openai".into(), "gpt-5.6-sol".into())]),
                );
                config.protection.equivalencies.insert(
                    "gpt-5.6-sol".into(),
                    BTreeMap::from([("anthropic".into(), "claude-fable-5".into())]),
                );
                save_config(&config)?;
                println!(
                    "wrote anthropic-openai equivalencies; protection.enabled remains {}",
                    config.protection.enabled
                );
            }
        },
        Command::Middleware { command } => {
            run_middleware_command(&config, command).await?;
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
            "{harness} is not connected to Alex; run `alex connect {harness}` first"
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
        anyhow::bail!("remote trace credential is not an Alex key (expected alxk-...)");
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
    let external_run_id = remote_trace
        .run_id
        .clone()
        .or_else(|| std::env::var("ALEXANDRIA_RUN_ID").ok())
        .map(|run_id| {
            if valid_wrap_run_id(&run_id) {
                Ok(run_id)
            } else {
                anyhow::bail!("invalid --run-id: use [A-Za-z0-9._-], max 128 chars")
            }
        })
        .transpose()?;
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
                external_run_id.clone(),
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
                external_run_id,
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

fn valid_wrap_run_id(run_id: &str) -> bool {
    !run_id.is_empty()
        && run_id.len() <= 128
        && !run_id.contains("..")
        && run_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn resolve_wrap_run_id(external_run_id: Option<String>, harness: &str) -> String {
    external_run_id.unwrap_or_else(|| match harness {
        "agent" => format!(
            "wrap-agent-{}-{:08x}",
            now_ms(),
            rand::thread_rng().gen::<u32>()
        ),
        _ => format!(
            "wrap-{harness}-{}-{:08x}",
            now_ms(),
            rand::thread_rng().gen::<u32>()
        ),
    })
}

async fn start_amp_trace_import(
    data_dir: PathBuf,
    harness: &str,
    remote: Option<RemoteTraceSender>,
    billing_account: Option<KnownAccount>,
    external_run_id: Option<String>,
) -> Result<AmpTraceImporter> {
    let run_id = resolve_wrap_run_id(external_run_id, harness);
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
            error_kind: None,
            error_code: None,
            error_class: None,
            substituted: false,
            original_model: None,
            served_model: None,
            substitution_reason: None,
            injected: false,
            fixture_name: None,
            attempts: None,
            original_account_id: None,
            served_account_id: None,
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
    external_run_id: Option<String>,
) -> Result<AgentTraceImporter> {
    let run_id = resolve_wrap_run_id(external_run_id, "agent");
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
        error_kind: None,
        error_code: None,
        error_class: None,
        substituted: false,
        original_model: None,
        served_model: None,
        substitution_reason: None,
        injected: false,
        fixture_name: None,
        attempts: None,
        original_account_id: None,
        served_account_id: None,
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
            ui::pad_right(&ui::purple(&when), 12),
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

fn middleware_path_id<'a>(id: &'a str, kind: &str) -> Result<&'a str> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
    {
        anyhow::bail!("invalid {kind} ID '{id}' (use letters, digits, '.', '-', or '_' only)");
    }
    Ok(id)
}

fn middleware_load_value(path: &Path) -> Result<serde_json::Value> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading middleware file {}", path.display()))?;
    match path.extension().and_then(|extension| extension.to_str()) {
        Some(extension) if extension.eq_ignore_ascii_case("json") => serde_json::from_str(&raw)
            .with_context(|| format!("parsing middleware JSON {}", path.display())),
        Some(extension) if extension.eq_ignore_ascii_case("toml") => {
            let value: toml::Value = toml::from_str(&raw)
                .with_context(|| format!("parsing middleware TOML {}", path.display()))?;
            serde_json::to_value(value)
                .with_context(|| format!("converting middleware TOML {}", path.display()))
        }
        _ => match serde_json::from_str(&raw) {
            Ok(value) => Ok(value),
            Err(json_error) => {
                let value: toml::Value = toml::from_str(&raw).with_context(|| {
                    format!(
                        "parsing middleware file {} as JSON ({json_error}) or TOML",
                        path.display()
                    )
                })?;
                serde_json::to_value(value)
                    .with_context(|| format!("converting middleware TOML {}", path.display()))
            }
        },
    }
}

/// Rule files may contain a bare RuleSpecV1, `{ rule = ... }`, or the
/// single-entry RuleSetV1 shape used by `middleware/rules.toml`.
fn middleware_rule_from_file(path: &Path) -> Result<serde_json::Value> {
    let value = middleware_load_value(path)?;
    let rule = if let Some(rule) = value.get("rule") {
        rule.clone()
    } else if let Some(rules) = value.get("rules") {
        let rules = rules
            .as_array()
            .context("middleware 'rules' field must be an array")?;
        match rules.as_slice() {
            [rule] => rule.clone(),
            [] => anyhow::bail!("middleware rule set contains no rules"),
            _ => anyhow::bail!(
                "middleware install/validate accepts one rule at a time; file contains {}",
                rules.len()
            ),
        }
    } else {
        value
    };
    if !rule.is_object() {
        anyhow::bail!("middleware rule must be a JSON/TOML object");
    }
    Ok(rule)
}

async fn middleware_status(config: &Config) -> Result<serde_json::Value> {
    let response = daemon_get(config, "/admin/middleware", &[]).await?;
    response
        .json()
        .await
        .context("decoding middleware status from daemon")
}

fn middleware_rule<'a>(status: &'a serde_json::Value, id: &str) -> Result<&'a serde_json::Value> {
    status["rules"]
        .as_array()
        .context("daemon middleware status omitted the rules array")?
        .iter()
        .find(|rule| rule["id"].as_str() == Some(id))
        .with_context(|| format!("unknown middleware rule '{id}'"))
}

async fn middleware_write(
    config: &Config,
    method: reqwest::Method,
    path: &str,
    body: Option<serde_json::Value>,
) -> Result<serde_json::Value> {
    let (status, response) = daemon_send(config, method, path, body).await?;
    if !status.is_success() {
        anyhow::bail!("middleware request failed ({status}): {response}");
    }
    Ok(response)
}

async fn middleware_set_rule_enabled(
    config: &Config,
    id: &str,
    enabled: bool,
) -> Result<serde_json::Value> {
    let id = middleware_path_id(id, "middleware rule")?;
    let status = middleware_status(config).await?;
    let mut rule = middleware_rule(&status, id)?.clone();
    rule["enabled"] = json!(enabled);
    middleware_write(
        config,
        reqwest::Method::PUT,
        &format!("/admin/middleware/rules/{id}"),
        Some(rule),
    )
    .await
}

async fn run_middleware_command(config: &Config, command: MiddlewareCommand) -> Result<()> {
    match command {
        MiddlewareCommand::Status => {
            println!(
                "{}",
                serde_json::to_string_pretty(&middleware_status(config).await?)?
            );
        }
        MiddlewareCommand::List => {
            let status = middleware_status(config).await?;
            let rules = status["rules"]
                .as_array()
                .context("daemon middleware status omitted the rules array")?;
            println!("{}", serde_json::to_string_pretty(rules)?);
        }
        MiddlewareCommand::Show { id } => {
            let id = middleware_path_id(&id, "middleware rule")?;
            let status = middleware_status(config).await?;
            println!(
                "{}",
                serde_json::to_string_pretty(middleware_rule(&status, id)?)?
            );
        }
        MiddlewareCommand::Validate { file } => {
            let rule = middleware_rule_from_file(&file)?;
            let response = middleware_write(
                config,
                reqwest::Method::POST,
                "/admin/middleware/validate",
                Some(json!({"rule": rule})),
            )
            .await?;
            println!("{}", serde_json::to_string_pretty(&response)?);
            if response["valid"].as_bool() == Some(false) {
                anyhow::bail!("middleware validation failed");
            }
        }
        MiddlewareCommand::Install { file } => {
            let rule = middleware_rule_from_file(&file)?;
            let response = middleware_write(
                config,
                reqwest::Method::POST,
                "/admin/middleware/rules",
                Some(rule),
            )
            .await?;
            println!("{}", serde_json::to_string_pretty(&response)?);
        }
        MiddlewareCommand::Enable { id } => {
            let response = middleware_set_rule_enabled(config, &id, true).await?;
            println!("{}", serde_json::to_string_pretty(&response)?);
        }
        MiddlewareCommand::Disable { id } => {
            let response = middleware_set_rule_enabled(config, &id, false).await?;
            println!("{}", serde_json::to_string_pretty(&response)?);
        }
        MiddlewareCommand::Rm { id } => {
            let id = middleware_path_id(&id, "middleware rule")?;
            let response = middleware_write(
                config,
                reqwest::Method::DELETE,
                &format!("/admin/middleware/rules/{id}"),
                None,
            )
            .await?;
            println!("{}", serde_json::to_string_pretty(&response)?);
        }
        MiddlewareCommand::Reload => {
            let response = middleware_write(
                config,
                reqwest::Method::POST,
                "/admin/middleware/reload",
                Some(json!({})),
            )
            .await?;
            println!("{}", serde_json::to_string_pretty(&response)?);
        }
        MiddlewareCommand::Test {
            id,
            fixture,
            trace,
            context,
        } => {
            let id = middleware_path_id(&id, "middleware rule")?;
            if fixture.is_none() && trace.is_none() && context.is_none() {
                anyhow::bail!("middleware test requires --fixture, --trace, or --context");
            }
            let mut payload = json!({"middleware_id": id});
            if let Some(fixture) = fixture {
                payload["fixture_name"] = json!(fixture);
            }
            if let Some(trace) = trace {
                payload["trace_id"] = json!(trace);
            }
            if let Some(context) = context {
                payload["context"] = middleware_load_value(&context)?;
            }
            let response = middleware_write(
                config,
                reqwest::Method::POST,
                "/admin/middleware/test",
                Some(payload),
            )
            .await?;
            println!("{}", serde_json::to_string_pretty(&response)?);
        }
        MiddlewareCommand::Leases { command: None } => {
            let response = daemon_get(config, "/admin/middleware/leases", &[]).await?;
            let body: serde_json::Value = response
                .json()
                .await
                .context("decoding middleware leases from daemon")?;
            println!("{}", serde_json::to_string_pretty(&body)?);
        }
        MiddlewareCommand::Leases {
            command: Some(MiddlewareLeasesCommand::Clear { id }),
        } => {
            let id = middleware_path_id(&id, "route lease")?;
            let response = middleware_write(
                config,
                reqwest::Method::DELETE,
                &format!("/admin/middleware/leases/{id}"),
                None,
            )
            .await?;
            println!("{}", serde_json::to_string_pretty(&response)?);
        }
    }
    Ok(())
}

async fn resolve_notification_channel(config: &Config, requested: Option<&str>) -> Result<String> {
    if let Some(requested) = requested.filter(|value| !value.trim().is_empty()) {
        return Ok(requested.trim().to_string());
    }
    let response = daemon_get(config, "/admin/notifications", &[]).await?;
    let body: serde_json::Value = response.json().await?;
    let channels = body["channels"].as_array().cloned().unwrap_or_default();
    match channels.as_slice() {
        [channel] => channel["id"]
            .as_str()
            .map(str::to_owned)
            .context("saved notification channel has no id"),
        [] => anyhow::bail!("no saved notification channels"),
        _ => anyhow::bail!("multiple notification channels are saved; use --channel <id>"),
    }
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

const TRACE_BACKUP_FORMAT: &str = "alex-trace-backup";
const TRACE_BACKUP_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TraceBackupManifest {
    format: String,
    version: u32,
}

fn trace_backup_temp_path(parent: &Path, label: &str) -> PathBuf {
    parent.join(format!(
        ".alex-trace-{label}-{}-{:08x}",
        std::process::id(),
        rand::thread_rng().gen::<u32>()
    ))
}

fn write_trace_jsonl(path: &Path, rows: &[serde_json::Value]) -> Result<()> {
    use std::io::Write;

    let file = std::fs::File::create(path)
        .with_context(|| format!("creating temporary JSONL file {}", path.display()))?;
    let mut writer = std::io::BufWriter::new(file);
    for row in rows {
        serde_json::to_writer(&mut writer, row)?;
        writer.write_all(b"\n")?;
    }
    writer.flush()?;
    Ok(())
}

fn collect_trace_body_files(
    root: &Path,
    directory: &Path,
    excluded: &[&Path],
    files: &mut Vec<(PathBuf, PathBuf)>,
) -> Result<()> {
    if !directory.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if excluded.iter().any(|excluded| path == **excluded) {
            continue;
        }
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_trace_body_files(root, &path, excluded, files)?;
        } else if file_type.is_file() {
            let relative = path
                .strip_prefix(root)
                .with_context(|| format!("body path escaped {}", root.display()))?;
            files.push((path.clone(), Path::new("bodies").join(relative)));
        } else {
            bail!("refusing to archive non-file body path {}", path.display());
        }
    }
    Ok(())
}

fn traces_backup_export_cmd(config: &Config, destination: &Path, force: bool) -> Result<()> {
    use flate2::write::GzEncoder;
    use flate2::Compression;

    if destination.exists() && !force {
        bail!(
            "{} already exists; pass --force to replace it",
            destination.display()
        );
    }
    let parent = destination.parent().filter(|path| !path.as_os_str().is_empty()).unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)?;

    let store = Store::open(config.data_dir.clone())?;
    let rows = store.export_trace_backup_rows()?;
    let staging = trace_backup_temp_path(&config.data_dir, "export-rows");
    let temporary_archive = trace_backup_temp_path(parent, "export.tar.gz.tmp");
    std::fs::create_dir_all(&staging)?;

    let result = (|| -> Result<(usize, u64)> {
        let traces_jsonl = staging.join("traces.jsonl");
        let tool_calls_jsonl = staging.join("tool_calls.jsonl");
        let heartbeats_jsonl = staging.join("heartbeats.jsonl");
        write_trace_jsonl(&traces_jsonl, &rows.traces)?;
        write_trace_jsonl(&tool_calls_jsonl, &rows.tool_calls)?;
        write_trace_jsonl(&heartbeats_jsonl, &rows.heartbeats)?;

        let mut body_files = Vec::new();
        collect_trace_body_files(
            &config.data_dir.join("bodies"),
            &config.data_dir.join("bodies"),
            &[destination, &temporary_archive],
            &mut body_files,
        )?;
        body_files.sort_by(|left, right| left.1.cmp(&right.1));

        let file = std::fs::File::create(&temporary_archive).with_context(|| {
            format!("creating temporary archive {}", temporary_archive.display())
        })?;
        let encoder = GzEncoder::new(file, Compression::default());
        let mut archive = tar::Builder::new(encoder);
        let manifest = serde_json::to_vec(&TraceBackupManifest {
            format: TRACE_BACKUP_FORMAT.into(),
            version: TRACE_BACKUP_VERSION,
        })?;
        let mut header = tar::Header::new_gnu();
        header.set_mode(0o644);
        header.set_size(manifest.len() as u64);
        header.set_cksum();
        archive.append_data(&mut header, "manifest.json", manifest.as_slice())?;
        archive.append_path_with_name(&traces_jsonl, "traces.jsonl")?;
        archive.append_path_with_name(&tool_calls_jsonl, "tool_calls.jsonl")?;
        archive.append_path_with_name(&heartbeats_jsonl, "heartbeats.jsonl")?;
        for (source, archive_path) in &body_files {
            archive.append_path_with_name(source, archive_path)?;
        }
        archive.finish()?;
        let encoder = archive.into_inner()?;
        let file = encoder.finish()?;
        file.sync_all()?;
        Ok((body_files.len(), std::fs::metadata(&temporary_archive)?.len()))
    })();

    let _ = std::fs::remove_dir_all(&staging);
    let (body_files, archive_bytes) = match result {
        Ok(result) => result,
        Err(error) => {
            let _ = std::fs::remove_file(&temporary_archive);
            return Err(error);
        }
    };
    std::fs::rename(&temporary_archive, destination).with_context(|| {
        format!("moving completed archive to {}", destination.display())
    })?;
    println!(
        "exported {} traces, {} tool calls, {} heartbeats, {} body files to {} ({})",
        rows.traces.len(),
        rows.tool_calls.len(),
        rows.heartbeats.len(),
        body_files,
        destination.display(),
        ui::human_bytes(archive_bytes)
    );
    Ok(())
}

fn safe_trace_archive_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path.components().all(|component| matches!(component, std::path::Component::Normal(_)))
}

fn read_trace_jsonl(path: &Path, label: &str) -> Result<Vec<serde_json::Value>> {
    use std::io::BufRead;

    let file = std::fs::File::open(path)?;
    let reader = std::io::BufReader::new(file);
    reader
        .lines()
        .enumerate()
        .map(|(index, line)| {
            let line = line?;
            if line.trim().is_empty() {
                bail!("{label} contains an empty JSONL row at line {}", index + 1);
            }
            serde_json::from_str(&line)
                .with_context(|| format!("invalid JSON in {label} at line {}", index + 1))
        })
        .collect()
}

fn traces_backup_import_cmd(config: &Config, source: &Path) -> Result<()> {
    use flate2::read::GzDecoder;
    use std::collections::HashSet;

    let store = Store::open(config.data_dir.clone())?;
    let staging = trace_backup_temp_path(&config.data_dir, "import");
    std::fs::create_dir_all(&staging)?;
    let result = (|| -> Result<(alex_store::TraceImportCounts, u64, u64)> {
        let file = std::fs::File::open(source)
            .with_context(|| format!("opening trace backup {}", source.display()))?;
        let decoder = GzDecoder::new(file);
        let mut archive = tar::Archive::new(decoder);
        let mut seen = HashSet::new();
        for entry in archive.entries().context("reading gzip tar archive")? {
            let mut entry = entry.context("reading tar entry")?;
            let path = entry.path().context("reading tar entry path")?.into_owned();
            if !safe_trace_archive_path(&path) || !seen.insert(path.clone()) {
                bail!("invalid or duplicate archive path {}", path.display());
            }
            let root = path.components().next().and_then(|component| match component {
                std::path::Component::Normal(value) => value.to_str(),
                _ => None,
            });
            let known_root_file = matches!(
                path.to_str(),
                Some("manifest.json" | "traces.jsonl" | "tool_calls.jsonl" | "heartbeats.jsonl")
            );
            let is_body = root == Some("bodies") && path.components().count() > 1;
            let entry_type = entry.header().entry_type();
            if entry_type.is_dir() {
                if root != Some("bodies") {
                    bail!("unexpected directory in trace backup: {}", path.display());
                }
                std::fs::create_dir_all(staging.join(&path))?;
                continue;
            }
            if !entry_type.is_file() || (!known_root_file && !is_body) {
                bail!("unexpected entry in trace backup: {}", path.display());
            }
            let destination = staging.join(&path);
            if let Some(parent) = destination.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut output = std::fs::File::create(&destination)?;
            std::io::copy(&mut entry, &mut output)?;
        }

        for required in ["manifest.json", "traces.jsonl", "tool_calls.jsonl", "heartbeats.jsonl"] {
            if !seen.contains(Path::new(required)) {
                bail!("trace backup is missing {required}");
            }
        }
        let manifest: TraceBackupManifest = serde_json::from_reader(std::fs::File::open(
            staging.join("manifest.json"),
        )?)
        .context("invalid trace backup manifest")?;
        if manifest.format != TRACE_BACKUP_FORMAT || manifest.version != TRACE_BACKUP_VERSION {
            bail!(
                "unsupported trace backup format/version: {}/{}",
                manifest.format,
                manifest.version
            );
        }
        let rows = alex_store::TraceBackupRows {
            traces: read_trace_jsonl(&staging.join("traces.jsonl"), "traces.jsonl")?,
            tool_calls: read_trace_jsonl(&staging.join("tool_calls.jsonl"), "tool_calls.jsonl")?,
            heartbeats: read_trace_jsonl(&staging.join("heartbeats.jsonl"), "heartbeats.jsonl")?,
        };
        let counts = store.import_trace_backup_rows(&rows)?;

        let mut bodies_imported = 0u64;
        let mut bodies_skipped = 0u64;
        let staged_bodies = staging.join("bodies");
        let mut body_files = Vec::new();
        collect_trace_body_files(&staged_bodies, &staged_bodies, &[], &mut body_files)?;
        for (staged, relative) in body_files {
            let destination = config.data_dir.join(&relative);
            if destination.exists() {
                bodies_skipped += 1;
                continue;
            }
            if let Some(parent) = destination.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(&staged, &destination).with_context(|| {
                format!("restoring body file {}", destination.display())
            })?;
            bodies_imported += 1;
        }
        Ok((counts, bodies_imported, bodies_skipped))
    })();
    let _ = std::fs::remove_dir_all(&staging);
    let (counts, bodies_imported, bodies_skipped) = result?;
    println!(
        "imported {} traces, {} tool calls, {} heartbeats, {} body files; skipped {} traces, {} tool calls, {} heartbeats, {} body files",
        counts.traces_imported,
        counts.tool_calls_imported,
        counts.heartbeats_imported,
        bodies_imported,
        counts.traces_skipped,
        counts.tool_calls_skipped,
        counts.heartbeats_skipped,
        bodies_skipped,
    );
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
            ui::pad_right(&ui::purple(d["date"].as_str().unwrap_or("-")), 12),
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
            ui::pad_right(&ui::purple(r["kind"].as_str().unwrap_or("run")), 8),
            ui::pad_right(
                &ui::turquoise(&ui::truncate(r["run_id"].as_str().unwrap_or("-"), 18)),
                18
            ),
            ui::pad_right(&ui::purple(&ui::truncate(&tags, 26)), 26),
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
    println!("{} revoked {}", ui::gold(ui::diamond()), ui::amber(id));
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
            .map(|s| ui::purple(&format!("via {s}")))
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
                    println!("   {}", ui::purple(&format!("top up: {url}")));
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
                        ui::purple(&format!("${usd:.2} remaining"))
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
                        ui::purple(&format!("{r} of {l} remaining"))
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
    println!(
        "available: {}",
        if available {
            ui::green("yes")
        } else {
            ui::red("no")
        }
    );
    println!("{}", dario_status_routing_text(body));
    if let (Some(runtime), Some(version)) =
        (body["runtime"].as_str(), body["runtime_version"].as_str())
    {
        println!("runtime: {runtime} {version}");
    }
    let node = body["resolved_node_bin"].as_str().unwrap_or("-");
    let claude = body["resolved_claude_bin"]
        .as_str()
        .or_else(|| body["claude_bin"].as_str())
        .unwrap_or("-");
    println!("node: {node}");
    println!("claude: {claude}");
    if let Some(issue) = body["issue"].as_object() {
        let code = issue
            .get("code")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let message = issue
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("Dario issue");
        println!("issue: {} — {}", ui::red(code), message);
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
            ui::purple(&age)
        );
    }
    Ok(())
}

fn dario_status_routing_text(body: &serde_json::Value) -> String {
    let routing_mode = body["routing_mode"].as_str().unwrap_or("unknown");
    let routing_reason = body["routing_reason"].as_str().unwrap_or("unknown");
    let route = if body["route_enabled"].as_bool().unwrap_or(false) {
        "dario"
    } else {
        "direct"
    };
    format!("mode: {routing_mode}\nrouting: {route} ({routing_reason})")
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

fn status_account_indicators(a: &status::StatusAccount) -> (String, String) {
    let remaining_ms = a.expires_at_ms.map(|expiry| expiry - now_ms());
    let expired = remaining_ms.is_some_and(|remaining| remaining < 0);
    let expiring = remaining_ms.is_some_and(|remaining| (0..30 * 60_000).contains(&remaining));
    let active = a.status == "active";
    let dot = if expired || !active {
        ui::red(ui::dot())
    } else if expiring {
        ui::yellow(ui::dot())
    } else {
        ui::green(ui::dot())
    };
    let expiry = match remaining_ms {
        Some(remaining) if remaining < 0 => {
            ui::red(&format!("expired {} ago", ui::human_ms(-remaining)))
        }
        Some(remaining) if remaining < 30 * 60_000 => {
            ui::yellow(&format!("expires in {}", ui::human_ms(remaining)))
        }
        Some(remaining) => format!("expires in {}", ui::human_ms(remaining)),
        None => ui::dim("no expiry"),
    };
    (dot, expiry)
}

#[derive(Debug, Clone, PartialEq)]
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
        ui::pad_left(&ui::purple(&format!("{}ms", r.latency_ms)), 7),
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
        alex_core::Provider::Exo => "exo-local".to_string(),
        alex_core::Provider::Amp => "amp".to_string(),
        alex_core::Provider::Kimi => models.kimi.clone(),
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

impl SystemLaunchctl {
    fn is_loaded(&mut self) -> Result<bool> {
        let output = std::process::Command::new("launchctl")
            .args(["print", &format!("{}/{LAUNCHD_LABEL}", self.domain)])
            .output()
            .context("running launchctl print")?;
        Ok(output.status.success())
    }

    fn bootstrap(&mut self, plist: &Path) -> Result<()> {
        self.run("bootstrap", plist)
    }
}

fn launchd_plist_path() -> Result<PathBuf> {
    Ok(dirs::home_dir()
        .context("no home dir")?
        .join("Library/LaunchAgents")
        .join(format!("{LAUNCHD_LABEL}.plist")))
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

fn render_launchd_sockets(config: &Config) -> Result<String> {
    let socket = |name: &str, host: &str| -> Result<String> {
        let addr = parse_bind_socket_addr(host, config.port)?;
        let family = if addr.is_ipv4() { "IPv4" } else { "IPv6" };
        Ok(format!(
            "    <key>{}</key>\n    <dict>\n      <key>SockFamily</key>\n      <string>{family}</string>\n      <key>SockType</key>\n      <string>stream</string>\n      <key>SockProtocol</key>\n      <string>TCP</string>\n      <key>SockNodeName</key>\n      <string>{}</string>\n      <key>SockServiceName</key>\n      <string>{}</string>\n    </dict>",
            xml_escape(name),
            xml_escape(host.trim_matches(['[', ']'])),
            config.port,
        ))
    };
    let mut entries = vec![socket("alexandria-primary", &config.host)?];
    if requires_explicit_loopback_listener(&config.host) {
        entries.push(socket("alexandria-local", "127.0.0.1")?);
    }
    Ok(format!(
        "  <key>Sockets</key>\n  <dict>\n{}\n  </dict>",
        entries.join("\n")
    ))
}

fn render_launchd_plist(
    exe: &Path,
    inherited_path: &str,
    known_dirs: &[PathBuf],
    config: &Config,
) -> Result<String> {
    Ok(LAUNCHD_TEMPLATE
        .replace(
            "/usr/local/bin/alexandria",
            &xml_escape(&exe.to_string_lossy()),
        )
        .replace(
            "__ALEX_LAUNCHD_PATH__",
            &xml_escape(&render_launchd_path(inherited_path, known_dirs)),
        )
        .replace("__ALEX_LAUNCHD_SOCKETS__", &render_launchd_sockets(config)?))
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
        let loaded = launchctl.is_loaded()?;
        let plist = render_launchd_plist(
            &exe,
            &std::env::var("PATH").unwrap_or_default(),
            &known_launchd_path_dirs(&exe),
            config,
        )?;
        std::fs::write(&dst, plist)?;
        if loaded {
            eprintln!("launchd plist updated; run `alex service restart` to load it.");
            anyhow::bail!("launchd service plist updated but not yet loaded (exit 1)");
        }
        launchctl.bootstrap(&dst)?;
        println!(
            "{} {}",
            ui::gold(ui::diamond()),
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
            ui::gold(ui::diamond()),
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

async fn service_restart(config: &Config, force: bool) -> Result<()> {
    if !cfg!(target_os = "macos") {
        anyhow::bail!("service restart supports macOS launchd only");
    }

    let mut launchctl = SystemLaunchctl::new();
    if !launchctl.is_loaded()? {
        anyhow::bail!("no loaded launchd daemon to restart; run alex service install first");
    }
    restart_launchd_daemon(config, force).await
}

fn launchd_hard_restart() -> Result<()> {
    let status = std::process::Command::new("launchctl")
        .args([
            "kickstart",
            "-k",
            &format!("gui/{}/{}", current_uid(), LAUNCHD_LABEL),
        ])
        .status()
        .context("running launchctl kickstart -k fallback")?;
    if status.success() {
        Ok(())
    } else {
        anyhow::bail!("launchctl kickstart -k fallback failed")
    }
}

/// Run the restart coordinator out-of-process for daemon self-updates.  The
/// coordinator must outlive the old daemon it signals; if it cannot be
/// spawned, the old daemon is deliberately left untouched.
pub(crate) fn spawn_launchd_restart_helper(exe: &Path) -> Result<()> {
    std::process::Command::new(exe)
        .arg("__launchd-restart-helper")
        .stdin(std::process::Stdio::null())
        // Inherit launchd's configured logs: a lifecycle fallback must be
        // diagnosable after the daemon that requested it has exited.
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .spawn()
        .with_context(|| format!("starting launchd restart helper from {}", exe.display()))?;
    Ok(())
}

async fn wait_for_local_health(config: &Config) -> Result<bool> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
        .context("building daemon health client")?;
    Ok(selfupdate::wait_for_daemon_health(
        &client,
        &format!("{}/health", config.base_url()),
        LAUNCHD_HEALTH_TIMEOUT,
    )
    .await)
}

/// B3 (launchd path): after a graceful drain, confirm the daemon now answering
/// /health is the build this helper was launched from (`CARGO_PKG_VERSION`).
/// A mismatch means launchd relaunched an old, pinned binary — surface it
/// loudly. Non-fatal: the graceful/hard fallback stays in control, and this is
/// only fully exercisable live on the Mac.
async fn warn_if_served_version_mismatch(config: &Config) {
    let target = env!("CARGO_PKG_VERSION");
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
    {
        Ok(client) => client,
        Err(_) => return,
    };
    let reported = match client
        .get(format!("{}/health", config.base_url()))
        .header("x-api-key", &config.local_key)
        .send()
        .await
        .ok()
    {
        Some(response) => response
            .json::<serde_json::Value>()
            .await
            .ok()
            .and_then(|v| v.get("version").and_then(|s| s.as_str()).map(String::from)),
        None => None,
    };
    match reported {
        Some(version) if version.trim_start_matches('v') == target.trim_start_matches('v') => {}
        Some(version) => eprintln!(
            "warning: after restart the daemon reports version {version} but this build is {target}; \
             launchd may be pinned to an old binary. Re-run `alex service restart` or reinstall."
        ),
        None => eprintln!(
            "warning: could not read the daemon version from /health after restart to confirm it is {target}."
        ),
    }
}

async fn launchd_socket_activation_is_live(config: &Config) -> Result<bool> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
        .context("building daemon health client")?;
    let response = client
        .get(format!("{}/health", config.base_url()))
        .header("x-api-key", &config.local_key)
        .send()
        .await
        .context("querying daemon /health")?;
    let health: serde_json::Value = response.json().await.context("parsing daemon /health")?;
    Ok(health
        .get("launchd_socket_activation")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false))
}

fn drain_timeout_from(value: Option<&str>) -> std::time::Duration {
    value
        .and_then(|value| value.parse::<u64>().ok())
        .map(std::time::Duration::from_secs)
        .unwrap_or_else(|| std::time::Duration::from_secs(30))
}

fn drain_timeout() -> std::time::Duration {
    drain_timeout_from(
        std::env::var("ALEXANDRIA_DRAIN_TIMEOUT_SECONDS")
            .ok()
            .as_deref(),
    )
}

fn graceful_or_hard_restart(
    graceful: Result<()>,
    hard_restart: impl FnOnce() -> Result<()>,
) -> Result<bool> {
    match graceful {
        Ok(()) => Ok(false),
        Err(_) => {
            hard_restart()?;
            Ok(true)
        }
    }
}

fn process_running(pid: i64) -> Result<bool> {
    #[cfg(unix)]
    {
        let Ok(pid) = libc::pid_t::try_from(pid) else {
            return Ok(false);
        };
        if pid <= 0 {
            return Ok(false);
        }

        if unsafe { libc::kill(pid, 0) } == 0 {
            return Ok(true);
        }

        // ESRCH (no such process) is the successful end state while waiting
        // for a graceful drain. EPERM still means the process exists.
        return match std::io::Error::last_os_error().raw_os_error() {
            Some(libc::EPERM) => Ok(true),
            Some(libc::ESRCH) => Ok(false),
            _ => Ok(false),
        };
    }

    #[cfg(not(unix))]
    {
        let _ = pid;
        Ok(false)
    }
}

async fn drain_launchd_daemon(pid: i64, timeout: std::time::Duration) -> Result<()> {
    if !process_running(pid)? {
        println!("old daemon already drained and exited");
        return Ok(());
    }
    let output = std::process::Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .output()
        .context("signalling launchd daemon to drain")?;
    if !output.status.success() {
        // The daemon may exit between the liveness check and SIGTERM. That is
        // still a completed graceful drain, not a reason to hard-restart.
        if !process_running(pid)? {
            println!("old daemon already drained and exited");
            return Ok(());
        }
        anyhow::bail!("could not signal launchd daemon to drain")
    }
    if !wait_for_process_exit(timeout, || process_running(pid)).await? {
        eprintln!(
            "launchd drain timed out after {}s; force-closing the old daemon",
            timeout.as_secs()
        );
        let _ = std::process::Command::new("kill")
            .args(["-KILL", &pid.to_string()])
            .output();
        anyhow::bail!("launchd daemon did not exit after the drain timeout")
    }
    println!("old daemon drained and exited");
    Ok(())
}

async fn wait_for_process_exit(
    timeout: std::time::Duration,
    mut is_running: impl FnMut() -> Result<bool>,
) -> Result<bool> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if !is_running()? {
            return Ok(true);
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(false);
        }
        tokio::time::sleep(LAUNCHD_HEALTH_POLL_INTERVAL).await;
    }
}

/// The only graceful launchd path.  The socket remains owned by launchd while
/// axum stops accepting and drains, so TCP handshakes queue instead of seeing
/// a refused connection.  Every error deliberately invokes the old hard
/// kickstart as the always-up fallback.
pub(crate) async fn restart_launchd_daemon(config: &Config, force: bool) -> Result<()> {
    let opted_out = std::env::var_os("ALEXANDRIA_GRACEFUL_RESTART").as_deref()
        == Some(std::ffi::OsStr::new("0"));
    if force || opted_out {
        eprintln!("using requested hard launchd restart");
        return launchd_hard_restart();
    }

    let graceful = async {
        if !launchd_socket_activation_is_live(config).await? {
            anyhow::bail!("loaded daemon does not have an activated launchd socket")
        }
        let pid = match detect_service_state() {
            ServiceState::LaunchdLoaded { pid: Some(pid) } => pid,
            _ => anyhow::bail!("launchd did not report a running daemon pid"),
        };
        eprintln!("launchd socket activation is live; draining daemon pid {pid}");
        drain_launchd_daemon(pid, drain_timeout()).await?;
        if !wait_for_local_health(config).await? {
            anyhow::bail!("replacement daemon did not become healthy before the timeout")
        }
        // B3: warn (non-fatally) if launchd relaunched an old, pinned binary.
        warn_if_served_version_mismatch(config).await;
        Ok(())
    }
    .await;

    if let Err(error) = &graceful {
        eprintln!("graceful launchd restart failed: {error:#}; falling back to hard restart");
    }
    if graceful_or_hard_restart(graceful, launchd_hard_restart)
        .context("graceful restart and hard fallback both failed")?
    {
        println!("daemon restarted via hard fallback");
    } else {
        println!("daemon restarted after graceful drain");
    }
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
            ui::dim(&ui::purple(&format!("{} {notice}", ui::diamond())))
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
            ui::gold(&format!(
                "{} daemon already running at {base}",
                ui::diamond()
            ))
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
            ui::diamond(),
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

const UP_STATE_FILE: &str = "alexandria-up-state.json";

#[derive(Debug, Clone, PartialEq, Eq)]
struct UpStepPlan {
    install: bool,
    connect: bool,
    launch: bool,
}

fn plan_up(installed: bool, connected: bool, configured: bool, no_launch: bool) -> UpStepPlan {
    UpStepPlan {
        install: !installed,
        connect: !connected || !configured,
        launch: !no_launch,
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct UpState {
    source: String,
    harness: String,
    base_url: String,
    model: String,
}

fn up_state_matches(config_dir: &Path, harness: &str, base_url: &str, model: &str) -> bool {
    let path = config_dir.join(UP_STATE_FILE);
    let Ok(raw) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(state) = serde_json::from_str::<UpState>(&raw) else {
        return false;
    };
    state.source == "alex-up"
        && state.harness == harness
        && state.base_url == base_url.trim_end_matches('/')
        && state.model == model
}

fn write_up_state(config_dir: &Path, harness: &str, base_url: &str, model: &str) -> Result<()> {
    let state = UpState {
        source: "alex-up".into(),
        harness: harness.into(),
        base_url: base_url.trim_end_matches('/').into(),
        model: model.into(),
    };
    std::fs::write(
        config_dir.join(UP_STATE_FILE),
        format!("{}\n", serde_json::to_string_pretty(&state)?),
    )?;
    Ok(())
}

async fn daemon_healthy(base_url: &str) -> bool {
    let Ok(client) = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
    else {
        return false;
    };
    client
        .get(format!("{}/health", base_url.trim_end_matches('/')))
        .send()
        .await
        .map(|response| response.status().is_success())
        .unwrap_or(false)
}

async fn mint_up_key(config: &Config, harness: &str) -> Result<(String, String)> {
    let body = json!({
        "kind": "run",
        "label": format!("alex-up:{harness}"),
        "tags": {"source": "alex-up", "harness": harness},
    });
    let (status, response) =
        daemon_send(config, reqwest::Method::POST, "/admin/run-keys", Some(body)).await?;
    if !status.is_success() {
        bail!(
            "could not mint the model-only alex up key: {status}: {}",
            ui::truncate(&response.to_string(), 300)
        );
    }
    let id = response["id"]
        .as_str()
        .context("daemon did not return a run-key id")?
        .to_string();
    let key = response["key"]
        .as_str()
        .context("daemon did not return a run key")?
        .to_string();
    println!("key: minted model-only run key {id} (tags: source=alex-up, harness={harness})");
    println!("key: {key}");
    Ok((id, key))
}

async fn up_cmd(
    config: &Config,
    harness: &str,
    url: Option<&str>,
    supplied_key: Option<&str>,
    model: &str,
    version: Option<&str>,
    no_launch: bool,
    args: Vec<String>,
) -> Result<()> {
    let spec = harness_connect::spec_by_name(harness)
        .with_context(|| format!("unknown harness '{harness}'"))?;
    if !spec.supports_connect {
        bail!("harness '{harness}' does not support Alex connection yet");
    }
    let remote = url.is_some();
    let base_url = if let Some(url) = url {
        let url = url.trim_end_matches('/');
        if url.is_empty() {
            bail!("--url must not be empty");
        }
        println!("target: remote Alex at {url} (will not start a local daemon)");
        url.to_string()
    } else {
        let base = config.base_url();
        if daemon_healthy(&base).await {
            println!("target: local daemon already running at {base}");
        } else {
            println!("target: starting local daemon at {base}");
            daemon_background(&config.host, config.port, None, None).await?;
        }
        base
    };
    let config_dir = harness_connect::resolve_config_dir(config, spec, None);
    let status = harness_connect::harness_status(config, spec, None, true).await?;
    let configured = status.connected && up_state_matches(&config_dir, harness, &base_url, model);
    let plan = plan_up(status.installed, status.connected, configured, no_launch);
    if !plan.connect {
        println!("connect: existing alex-up configuration is current");
    }
    harness_connect::ensure_installed(spec, version).await?;

    let (key_id, key) = match supplied_key {
        Some(key) => {
            if !key.starts_with("alxk-") {
                bail!("--key must be a model-only scoped run key (expected alxk-...), never the Alex local/admin key");
            }
            println!("key: using supplied scoped run key");
            ("rk-provided".to_string(), key.to_string())
        }
        None if remote => bail!("remote alex requires --key with a model-only scoped run key; alex up cannot mint on someone else's daemon"),
        None if configured => {
            let existing = harness_connect::configured_api_key(harness, &config_dir)
                .context("alex up state exists but the harness key is missing; rerun with --key or reconnect")?;
            println!("key: reusing the existing alex-up scoped key");
            ("rk-existing".into(), existing)
        }
        None => mint_up_key(config, harness).await?,
    };

    // A supplied key may be new even when a state marker exists, so only skip
    // configuration if its persisted key agrees with the requested one.
    let key_matches = harness_connect::configured_api_key(harness, &config_dir)
        .as_deref()
        .is_some_and(|current| current == key);
    if configured && key_matches {
        println!("connect: already configured for {base_url} with default model {model}");
    } else {
        println!("connect: configuring {harness} for {base_url}");
        harness_connect::connect_with_preminted_key(
            harness,
            Some(config_dir.clone()),
            &base_url,
            key,
            Some(key_id),
            false,
            false,
        )
        .await?;
        harness_connect::set_default_model(harness, &config_dir, model)?;
        write_up_state(&config_dir, harness, &base_url, model)?;
        println!("model: default set to {model}");
    }
    if no_launch {
        println!("launch: skipped (--no-launch)");
        return Ok(());
    }
    println!("launch: exec {}", spec.binary);
    let binary = harness_connect::resolve_harness_binary(config, spec)
        .with_context(|| format!("{harness} is not installed or not on PATH"))?;
    // Pi's non-interactive mode does not consistently honour its persisted
    // defaultProvider across releases, so pass the connected profile explicitly
    // while retaining every user argument after it (including an override).
    let mut launch_args = if harness == "pi" {
        vec![
            OsString::from("--provider"),
            OsString::from("alexandria"),
            OsString::from("--model"),
            OsString::from(model),
        ]
    } else {
        Vec::new()
    };
    launch_args.extend(args.into_iter().map(OsString::from));
    launch_harness(&binary, &launch_args)
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
        ui::gold(ui::diamond()),
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
    let summary = status::status_summary(config).await?;
    let base = &summary.base_url;
    let health = &summary.health;
    let running = summary.daemon_up;
    let service = &summary.service_state;
    let binaries = &summary.binaries;
    let accounts = &summary.accounts;
    let limits = &summary.limits;
    let dario = &summary.dario_response;

    if json {
        let binaries_json: Vec<serde_json::Value> = binaries
            .iter()
            .map(|binary| {
                serde_json::json!({
                    "path": binary.path.to_string_lossy(),
                    "this_binary": binary.this_binary,
                })
            })
            .collect();
        let accounts_json: Vec<serde_json::Value> = accounts
            .iter()
            .map(|a| {
                serde_json::json!({
                    "id": a.id,
                    "provider": a.provider,
                    "name": a.name,
                    "kind": a.kind,
                    "label": a.label,
                    "status": a.status,
                    "health": a.health,
                    "needs_reauth": a.needs_reauth,
                    "usage_pct": a.usage_pct,
                    "expires_at_ms": a.expires_at_ms,
                    "last_heartbeat": a.last_heartbeat,
                })
            })
            .collect();
        let combined = serde_json::json!({
            "daemon": {
                "version": summary.version,
                "binaries": binaries_json,
                "service": {
                    "state": summary.service,
                    "managed": summary.service_managed,
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
            "update": summary.update,
        });
        println!("{}", serde_json::to_string_pretty(&combined)?);
        return Ok(());
    }

    println!("{}", ui::section("daemon"));
    if binaries.is_empty() {
        println!(
            "  {} {}",
            ui::pad_right(&ui::purple("binary"), 10),
            ui::dim("alexandria not found on PATH")
        );
    } else {
        let mut first = true;
        for binary in binaries {
            let label = if first {
                ui::purple("binary")
            } else {
                String::new()
            };
            first = false;
            let suffix = if binary.this_binary {
                format!(" {}", ui::dim("(this binary)"))
            } else {
                String::new()
            };
            println!(
                "  {} {}{suffix}",
                ui::pad_right(&label, 10),
                binary.path.display()
            );
        }
    }
    println!(
        "  {} {}",
        ui::pad_right(&ui::purple("version"), 10),
        summary.version
    );
    let sdot = if service_managed(service) {
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
        ui::pad_right(&ui::purple("service"), 10),
        service_state_label(service)
    );
    if let ServiceState::Systemd { .. } = service {
        println!(
            "  {} {}",
            ui::pad_right("", 10),
            ui::dim("unit: ~/.config/systemd/user/alexandria.service")
        );
    }
    match health {
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
                ui::pad_right(&ui::purple("process"), 10),
                ui::green(ui::dot()),
                ui::green(&format!("running v{version}")),
                ui::dim(&detail)
            );
            if !service_managed(service) {
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
                ui::pad_right(&ui::purple("process"), 10),
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
        ui::pad_right(&ui::purple("endpoint"), 10),
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
    for a in accounts {
        let (dot, expiry) = status_account_indicators(a);
        let hb = &a.last_heartbeat;
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
            ui::pad_right(&ui::amber(&a.provider), 10),
            ui::pad_right(&a.kind, 8),
            ui::pad_right(&expiry, 20),
            ui::pad_right(&ui::purple(a.label.as_deref().unwrap_or("")), 24)
        );
    }

    println!();
    match limits {
        Some(snap) => print_limits(snap),
        None => println!("{}", ui::dim("limits: skipped (daemon not running)")),
    }

    match dario {
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
    fn notification_debug_cli_subcommands_parse_all_supported_steps() {
        assert!(matches!(
            Cli::try_parse_from(["alex", "notify", "channels"])
                .unwrap()
                .command,
            Some(Command::Notify {
                command: NotifyCommand::Channels
            })
        ));
        match Cli::try_parse_from([
            "alex",
            "notify",
            "commands",
            "--enable",
            "--channel",
            "control",
        ])
        .unwrap()
        .command
        .unwrap()
        {
            Command::Notify {
                command:
                    NotifyCommand::Commands {
                        enable: true,
                        disable: false,
                        channel: Some(channel),
                    },
            } => assert_eq!(channel, "control"),
            _ => panic!("unexpected notify commands parse"),
        }
        assert!(
            Cli::try_parse_from(["alex", "notify", "commands", "--enable", "--disable"]).is_err()
        );
        match Cli::try_parse_from([
            "alex",
            "notify",
            "test",
            "--channel",
            "saved",
            "--category",
            "reauth",
        ])
        .unwrap()
        .command
        .unwrap()
        {
            Command::Notify {
                command:
                    NotifyCommand::Test {
                        channel: Some(channel),
                        category: Some(category),
                    },
            } => {
                assert_eq!(channel, "saved");
                assert_eq!(category, "reauth");
            }
            _ => panic!("unexpected notify test parse"),
        }
        assert!(matches!(
            Cli::try_parse_from(["alex", "notify", "log", "--limit", "17"])
                .unwrap()
                .command,
            Some(Command::Notify {
                command: NotifyCommand::Log { limit: 17 }
            })
        ));
    }

    #[test]
    fn reauth_cli_subcommands_parse_start_submit_and_status() {
        match Cli::try_parse_from([
            "alex",
            "reauth",
            "start",
            "anthropic",
            "--notify",
            "--force",
        ])
        .unwrap()
        .command
        .unwrap()
        {
            Command::Reauth {
                command:
                    ReauthCommand::Start {
                        provider,
                        notify: true,
                        force: true,
                    },
            } => assert_eq!(provider, "anthropic"),
            _ => panic!("unexpected reauth start parse"),
        }
        match Cli::try_parse_from(["alex", "reauth", "submit", "code#state"])
            .unwrap()
            .command
            .unwrap()
        {
            Command::Reauth {
                command: ReauthCommand::Submit { input },
            } => assert_eq!(input, "code#state"),
            _ => panic!("unexpected reauth submit parse"),
        }
        match Cli::try_parse_from(["alex", "reauth", "status", "anthropic"])
            .unwrap()
            .command
            .unwrap()
        {
            Command::Reauth {
                command: ReauthCommand::Status { provider },
            } => assert_eq!(provider.as_deref(), Some("anthropic")),
            _ => panic!("unexpected reauth status parse"),
        }
    }

    #[test]
    fn middleware_cli_parses_rule_lifecycle_commands() {
        assert!(matches!(
            Cli::try_parse_from(["alex", "middleware", "status"])
                .unwrap()
                .command,
            Some(Command::Middleware {
                command: MiddlewareCommand::Status
            })
        ));
        assert!(matches!(
            Cli::try_parse_from(["alex", "middleware", "list"])
                .unwrap()
                .command,
            Some(Command::Middleware {
                command: MiddlewareCommand::List
            })
        ));
        match Cli::try_parse_from(["alex", "middleware", "show", "fable-to-sol"])
            .unwrap()
            .command
            .unwrap()
        {
            Command::Middleware {
                command: MiddlewareCommand::Show { id },
            } => assert_eq!(id, "fable-to-sol"),
            _ => panic!("unexpected middleware show parse"),
        }
        match Cli::try_parse_from(["alex", "middleware", "validate", "rule.toml"])
            .unwrap()
            .command
            .unwrap()
        {
            Command::Middleware {
                command: MiddlewareCommand::Validate { file },
            } => assert_eq!(file, PathBuf::from("rule.toml")),
            _ => panic!("unexpected middleware validate parse"),
        }
        match Cli::try_parse_from(["alex", "middleware", "install", "rule.json"])
            .unwrap()
            .command
            .unwrap()
        {
            Command::Middleware {
                command: MiddlewareCommand::Install { file },
            } => assert_eq!(file, PathBuf::from("rule.json")),
            _ => panic!("unexpected middleware install parse"),
        }
        for verb in ["enable", "disable", "rm"] {
            assert!(Cli::try_parse_from(["alex", "middleware", verb, "fable-to-sol"]).is_ok());
        }
        assert!(matches!(
            Cli::try_parse_from(["alex", "middleware", "reload"])
                .unwrap()
                .command,
            Some(Command::Middleware {
                command: MiddlewareCommand::Reload
            })
        ));
    }

    #[test]
    fn middleware_cli_parses_dry_run_sources_and_lease_commands() {
        match Cli::try_parse_from([
            "alex",
            "middleware",
            "test",
            "fable-to-sol",
            "--fixture",
            "fable-error",
        ])
        .unwrap()
        .command
        .unwrap()
        {
            Command::Middleware {
                command:
                    MiddlewareCommand::Test {
                        id,
                        fixture: Some(fixture),
                        trace: None,
                        context: None,
                    },
            } => {
                assert_eq!(id, "fable-to-sol");
                assert_eq!(fixture, "fable-error");
            }
            _ => panic!("unexpected middleware test parse"),
        }
        assert!(Cli::try_parse_from([
            "alex",
            "middleware",
            "test",
            "fable-to-sol",
            "--fixture",
            "fable-error",
            "--trace",
            "trace-id",
        ])
        .is_err());
        assert!(Cli::try_parse_from([
            "alex",
            "middleware",
            "test",
            "fable-to-sol",
        ])
        .is_err());
        assert!(matches!(
            Cli::try_parse_from(["alex", "middleware", "leases"])
                .unwrap()
                .command,
            Some(Command::Middleware {
                command: MiddlewareCommand::Leases { command: None }
            })
        ));
        match Cli::try_parse_from(["alex", "middleware", "leases", "clear", "lease-123"])
            .unwrap()
            .command
            .unwrap()
        {
            Command::Middleware {
                command:
                    MiddlewareCommand::Leases {
                        command: Some(MiddlewareLeasesCommand::Clear { id }),
                    },
            } => assert_eq!(id, "lease-123"),
            _ => panic!("unexpected middleware lease clear parse"),
        }
    }

    #[test]
    fn middleware_rule_files_accept_bare_json_and_single_rule_toml() {
        let dir = tmpdir("middleware-rule-files");
        let json_path = dir.join("bare.json");
        std::fs::write(
            &json_path,
            r#"{"id":"bare","name":"Bare","hook":"attempt_result","when":{"models":["fable-*"]},"then":{"continue":true}}"#,
        )
        .unwrap();
        assert_eq!(middleware_rule_from_file(&json_path).unwrap()["id"], "bare");

        let toml_path = dir.join("wrapped.toml");
        std::fs::write(
            &toml_path,
            r#"
api_version = 1

[[rules]]
id = "wrapped"
name = "Wrapped"
hook = "attempt_result"

[rules.when]
models = ["fable-*"]

[rules.then]
continue = true
"#,
        )
        .unwrap();
        let rule = middleware_rule_from_file(&toml_path).unwrap();
        assert_eq!(rule["id"], "wrapped");
        assert_eq!(rule["then"]["continue"], true);
    }

    #[test]
    fn middleware_rule_files_reject_multi_rule_install_and_unsafe_path_ids() {
        let dir = tmpdir("middleware-multi-rule-file");
        let path = dir.join("rules.json");
        std::fs::write(
            &path,
            r#"{"api_version":1,"rules":[{"id":"a"},{"id":"b"}]}"#,
        )
        .unwrap();
        assert!(middleware_rule_from_file(&path)
            .unwrap_err()
            .to_string()
            .contains("one rule at a time"));
        assert_eq!(
            middleware_path_id("alex.rule-1", "middleware rule").unwrap(),
            "alex.rule-1"
        );
        assert!(middleware_path_id("../rules/a", "middleware rule").is_err());
    }

    #[test]
    fn valid_wrap_run_id_allows_safe_ids_only() {
        assert!(valid_wrap_run_id("cove-run-123"));
        assert!(valid_wrap_run_id("run.2026_07_15"));
        assert!(!valid_wrap_run_id(""));
        assert!(!valid_wrap_run_id("../etc"));
        assert!(!valid_wrap_run_id("a b"));
        assert!(!valid_wrap_run_id(&"a".repeat(200)));
        assert!(!valid_wrap_run_id("foo/bar"));
    }

    #[test]
    fn resolve_wrap_run_id_uses_external_value_or_legacy_prefix() {
        assert_eq!(
            resolve_wrap_run_id(Some("cove-run-123".to_string()), "amp"),
            "cove-run-123"
        );

        let amp_run_id = resolve_wrap_run_id(None, "amp");
        assert!(amp_run_id.starts_with("wrap-amp-"));

        let agent_run_id = resolve_wrap_run_id(None, "agent");
        assert!(agent_run_id.starts_with("wrap-agent-"));
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
            run_id: None,
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
            reauth_check_minutes: default_reauth_check_minutes(),
            ping_anthropic_model: default_ping_anthropic(),
            ping_openai_model: default_ping_openai(),
            ping_xai_model: default_ping_xai(),
            ping_gemini_model: default_ping_gemini(),
            ping_openrouter_model: default_ping_openrouter(),
            exo_url: default_exo_url(),
            exo_enabled_models: Vec::new(),
            openrouter_exposed_models: alex_proxy::default_openrouter_exposed_models(),
            gemini_project: String::new(),
            anthropic_upstream: "direct".into(),
            dario_mode_migrated: true,
            dario_api_key: String::new(),
            dario_claude_bin: None,
            dario_node_path: None,
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
            substitution: alex_proxy::SubstitutionConfig::default(),
            protection: alex_proxy::ProtectionPolicy::default(),
            notifications: Vec::new(),
            notification_cooldown_seconds: alex_proxy::notify::default_cooldown_seconds(),
            notification_timeout_seconds: alex_proxy::notify::default_timeout_seconds(),
        }
    }

    #[test]
    fn trace_backup_archive_round_trip_restores_rows_and_body_files() {
        let data_dir = tmpdir("trace-backup-archive-round-trip");
        let archive = data_dir.parent().unwrap().join(format!(
            "alex-trace-backup-round-trip-{}.tar.gz",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&archive);
        let config = test_config(data_dir.clone());
        let store = Store::open(data_dir.clone()).unwrap();
        for (id, ts, body) in [
            ("archive-trace-1", 1_000, b"request one".as_slice()),
            ("archive-trace-2", 2_000, b"request two".as_slice()),
        ] {
            let mut trace = alex_core::TraceRecord {
                id: id.into(),
                ts_request_ms: ts,
                ..Default::default()
            };
            trace.req_body_path = Some(store.write_body(id, "request.json", body).unwrap());
            store.insert_trace(&trace).unwrap();
        }
        let tool_body = store
            .write_body("archive-tool", "tool-result.json", b"tool result")
            .unwrap();
        store
            .upsert_tool_call(&alex_store::ToolCallRecord {
                id: "archive-tool".into(),
                harness: "pi".into(),
                session_id: "archive-session".into(),
                turn_id: None,
                tool_call_id: "call-1".into(),
                trace_id: Some("archive-trace-1".into()),
                tool_name: "bash".into(),
                ts_start_ms: 1_100,
                ts_end_ms: Some(1_200),
                is_error: Some(false),
                exit_status: Some(0),
                args_body_path: None,
                result_body_path: Some(tool_body),
            })
            .unwrap();
        store
            .insert_heartbeat(900, "anthropic", None, true, Some(200), 10, "ok")
            .unwrap();

        traces_backup_export_cmd(&config, &archive, false).unwrap();
        assert!(traces_backup_export_cmd(&config, &archive, false).is_err());
        traces_backup_export_cmd(&config, &archive, true).unwrap();
        store.clear_traces_and_bodies().unwrap();
        assert_eq!(store.reset_counts().unwrap().body_files, 0);

        traces_backup_import_cmd(&config, &archive).unwrap();
        let counts = store.reset_counts().unwrap();
        assert_eq!((counts.traces, counts.heartbeats, counts.body_files), (2, 1, 3));
        assert_eq!(store.session_tool_calls("archive-session").unwrap().len(), 1);
        for id in ["archive-trace-1", "archive-trace-2"] {
            let path = store.get_trace(id).unwrap().unwrap()["req_body_path"]
                .as_str()
                .unwrap()
                .to_string();
            assert!(Path::new(&path).exists(), "restored body missing: {path}");
            assert!(Path::new(&path).starts_with(data_dir.join("bodies")));
        }
        let tool_path = store.get_tool_call("archive-tool").unwrap().unwrap()["result_body_path"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(Path::new(&tool_path).exists());

        traces_backup_import_cmd(&config, &archive).unwrap();
        let repeated = store.reset_counts().unwrap();
        assert_eq!((repeated.traces, repeated.heartbeats, repeated.body_files), (2, 1, 3));
        let _ = std::fs::remove_file(&archive);
    }

    #[test]
    fn trace_backup_archive_import_keeps_populated_store_rows() {
        let source_dir = tmpdir("trace-backup-populated-source");
        let destination_dir = tmpdir("trace-backup-populated-destination");
        let archive = source_dir.parent().unwrap().join(format!(
            "alex-trace-backup-populated-{}.tar.gz",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&archive);
        let source_config = test_config(source_dir.clone());
        let source = Store::open(source_dir).unwrap();
        source
            .insert_trace(&alex_core::TraceRecord {
                id: "shared".into(),
                ts_request_ms: 1_000,
                routed_model: Some("old-model".into()),
                ..Default::default()
            })
            .unwrap();
        source
            .insert_trace(&alex_core::TraceRecord {
                id: "backup-only".into(),
                ts_request_ms: 2_000,
                ..Default::default()
            })
            .unwrap();
        traces_backup_export_cmd(&source_config, &archive, false).unwrap();

        let destination_config = test_config(destination_dir.clone());
        let destination = Store::open(destination_dir).unwrap();
        destination
            .insert_trace(&alex_core::TraceRecord {
                id: "shared".into(),
                ts_request_ms: 9_000,
                routed_model: Some("new-model".into()),
                ..Default::default()
            })
            .unwrap();
        destination
            .insert_trace(&alex_core::TraceRecord {
                id: "newer-only".into(),
                ts_request_ms: 10_000,
                ..Default::default()
            })
            .unwrap();

        traces_backup_import_cmd(&destination_config, &archive).unwrap();
        assert_eq!(destination.reset_counts().unwrap().traces, 3);
        assert_eq!(
            destination.get_trace("shared").unwrap().unwrap()["routed_model"],
            "new-model"
        );
        assert!(destination.get_trace("backup-only").unwrap().is_some());
        assert!(destination.get_trace("newer-only").unwrap().is_some());
        let _ = std::fs::remove_file(&archive);
    }

    #[test]
    fn trace_backup_cli_surface_and_junk_archive_validation() {
        let export = Cli::try_parse_from([
            "alex",
            "traces",
            "export",
            "backup.tar.gz",
            "--force",
        ])
        .unwrap();
        match export.command {
            Some(Command::Traces {
                command: Some(TracesCommand::Export { path, force }),
                ..
            }) => {
                assert_eq!(path, PathBuf::from("backup.tar.gz"));
                assert!(force);
            }
            _ => panic!("expected traces export"),
        }
        let import = Cli::try_parse_from(["alex", "traces", "import", "backup.tar.gz"]).unwrap();
        assert!(matches!(
            import.command,
            Some(Command::Traces {
                command: Some(TracesCommand::Import { .. }),
                ..
            })
        ));

        let data_dir = tmpdir("trace-backup-junk");
        let junk = data_dir.join("junk.tar.gz");
        std::fs::write(&junk, b"this is not a gzip tar archive").unwrap();
        let error = traces_backup_import_cmd(&test_config(data_dir.clone()), &junk).unwrap_err();
        let error_chain = format!("{error:#}");
        assert!(
            error_chain.contains("archive") || error_chain.contains("gzip"),
            "unexpected error: {error:#}"
        );
        assert_eq!(Store::open(data_dir).unwrap().reset_counts().unwrap().traces, 0);
    }

    #[cfg(unix)]
    fn fake_runtime_executable(path: &Path, body: &str) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn dario_runtime_resolvers_cover_overrides_and_version_manager_paths() {
        let _guard = ENV_LOCK.lock().unwrap();
        let home = tmpdir("dario-runtime-resolver");
        let old_home = std::env::var_os("HOME");
        let old_path = std::env::var_os("PATH");
        let old_node = std::env::var_os("ALEXANDRIA_NODE_BIN");
        let old_claude = std::env::var_os("ALEXANDRIA_REAL_CLAUDE_BIN");
        std::env::set_var("HOME", &home);
        std::env::set_var("PATH", "/definitely/no/node");
        std::env::remove_var("ALEXANDRIA_NODE_BIN");
        std::env::remove_var("ALEXANDRIA_REAL_CLAUDE_BIN");

        let mise = home.join(".local/share/mise/installs/node");
        let node20 = mise.join("20.1.0/bin/node");
        let node22 = mise.join("22.3.0/bin/node");
        let unsupported_node = mise.join("30.0.0/bin/node");
        fake_runtime_executable(&node20, "#!/bin/sh\necho v20.1.0\n");
        fake_runtime_executable(&node22, "#!/bin/sh\necho v22.3.0\n");
        fake_runtime_executable(&unsupported_node, "#!/bin/sh\necho v17.9.0\n");
        assert_eq!(dario::resolve_dario_node_bin(None), Some(node22.clone()));

        let override_node = home.join("override-node");
        fake_runtime_executable(&override_node, "#!/bin/sh\necho v23.0.0\n");
        assert_eq!(
            dario::resolve_dario_node_bin(Some(&override_node)),
            Some(override_node)
        );
        assert_eq!(
            dario::resolve_dario_node_bin(Some(&home.join("missing-node"))),
            None
        );

        let claude20 = mise.join("20.1.0/bin/claude");
        let claude22 = mise.join("22.3.0/bin/claude");
        fake_runtime_executable(&claude20, "#!/bin/sh\nexit 0\n");
        fake_runtime_executable(&claude22, "#!/bin/sh\nexit 0\n");
        assert_eq!(resolve_dario_claude_bin(None), Some(claude22.clone()));
        let override_claude = home.join("override-claude");
        fake_runtime_executable(&override_claude, "#!/bin/sh\nexit 0\n");
        assert_eq!(
            resolve_dario_claude_bin(Some(&override_claude)),
            Some(override_claude)
        );

        match old_home {
            Some(value) => std::env::set_var("HOME", value),
            None => std::env::remove_var("HOME"),
        }
        match old_path {
            Some(value) => std::env::set_var("PATH", value),
            None => std::env::remove_var("PATH"),
        }
        match old_node {
            Some(value) => std::env::set_var("ALEXANDRIA_NODE_BIN", value),
            None => std::env::remove_var("ALEXANDRIA_NODE_BIN"),
        }
        match old_claude {
            Some(value) => std::env::set_var("ALEXANDRIA_REAL_CLAUDE_BIN", value),
            None => std::env::remove_var("ALEXANDRIA_REAL_CLAUDE_BIN"),
        }
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn dario_route_should_enable_resolves_tri_state() {
        let mut config = test_config(tmpdir("dario-route-resolver"));

        config.anthropic_upstream = "auto".into();
        assert!(config.dario_route_should_enable(true));
        assert!(
            !config.dario_route_should_enable(false),
            "no OAuth subscription (including API-key-only) must stay direct"
        );

        config.anthropic_upstream = "dario".into();
        assert!(config.dario_route_should_enable(true));
        assert!(config.dario_route_should_enable(false));

        config.anthropic_upstream = "direct".into();
        assert!(!config.dario_route_should_enable(true));
        assert!(!config.dario_route_should_enable(false));
    }

    #[test]
    fn config_heal_migrates_legacy_dario_default_once() {
        let mut config = test_config(tmpdir("dario-heal-legacy"));
        config.anthropic_upstream = "direct".into();
        config.dario_mode_migrated = false;

        config.heal();
        assert_eq!(config.anthropic_upstream, "auto");
        assert!(config.dario_mode_migrated);
        config.heal();
        assert_eq!(config.anthropic_upstream, "auto");

        let mut disabled = test_config(tmpdir("dario-heal-explicit-disable"));
        disabled.anthropic_upstream = "direct".into();
        disabled.dario_mode_migrated = true;
        disabled.heal();
        assert_eq!(disabled.anthropic_upstream, "direct");
    }

    #[test]
    fn config_heal_migrates_only_the_old_anthropic_ping_default() {
        let mut config = test_config(tmpdir("anthropic-ping-heal"));
        config.ping_anthropic_model = "claude-haiku-4-5".into();
        config.heal();
        assert_eq!(config.ping_anthropic_model, "claude-sonnet-5");

        config.ping_anthropic_model = "claude-opus-4-8".into();
        config.heal();
        assert_eq!(config.ping_anthropic_model, "claude-opus-4-8");
    }

    #[test]
    fn dario_and_anthropic_ping_defaults_target_auto_and_sonnet() {
        assert_eq!(default_anthropic_upstream(), "auto");
        assert_eq!(default_ping_anthropic(), "claude-sonnet-5");
    }

    #[test]
    fn dario_401_is_classified_as_reauth() {
        assert_eq!(
            dario_issue(
                "generation-a readiness probe failed: unhealthy status=401 body=unauthorized"
            ),
            serde_json::json!({
                "code": "reauth",
                "message": "Claude Code login needs re-auth",
                "fixable": true,
            })
        );
        assert!(dario_status_has_reauth_issue(&serde_json::json!({
            "active_generation_id": "generation-a",
            "generations": [{"id": "generation-a", "last_probe": {"status": 401}}],
        })));
    }

    #[test]
    fn dario_status_shows_mode_effective_route_and_reason() {
        let auto_with_subscription = serde_json::json!({
            "routing_mode": "auto",
            "route_enabled": true,
            "routing_reason": "auto: active Claude subscription detected",
        });
        assert_eq!(
            dario_status_routing_text(&auto_with_subscription),
            "mode: auto\nrouting: dario (auto: active Claude subscription detected)"
        );

        let auto_without_subscription = serde_json::json!({
            "routing_mode": "auto",
            "route_enabled": false,
            "routing_reason": "auto: no Claude subscription",
        });
        assert_eq!(
            dario_status_routing_text(&auto_without_subscription),
            "mode: auto\nrouting: direct (auto: no Claude subscription)"
        );

        let forced_disabled = serde_json::json!({
            "routing_mode": "direct",
            "route_enabled": false,
            "routing_reason": "forced off",
        });
        assert_eq!(
            dario_status_routing_text(&forced_disabled),
            "mode: direct\nrouting: direct (forced off)"
        );
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

    #[test]
    fn launchd_plist_renders_configured_socket_and_path() {
        let mut config = test_config(tmpdir("launchd-plist"));
        config.host = "127.0.0.1".into();
        config.port = 4321;
        let plist = render_launchd_plist(
            Path::new("/Users/alex/bin/alex&ria"),
            "/usr/bin:/custom/bin",
            &[
                PathBuf::from("/opt/homebrew/bin"),
                PathBuf::from("/custom/bin"),
            ],
            &config,
        )
        .unwrap();
        assert!(plist.contains("/Users/alex/bin/alex&amp;ria"));
        assert!(plist.contains("alexandria-primary"));
        assert!(plist.contains("<string>127.0.0.1</string>"));
        assert!(plist.contains("<string>4321</string>"));
    }

    #[tokio::test]
    async fn standalone_listener_binds_without_a_launchd_socket() {
        let listener = bind_daemon_listener("127.0.0.1", 0).await.unwrap();
        assert!(listener.local_addr().unwrap().ip().is_loopback());
    }

    #[test]
    fn graceful_failure_always_runs_the_hard_restart_fallback() {
        let called = std::cell::Cell::new(false);
        let used_fallback = graceful_or_hard_restart(
            Err(anyhow::anyhow!("new daemon never became healthy")),
            || {
                called.set(true);
                Ok(())
            },
        )
        .unwrap();
        assert!(used_fallback);
        assert!(called.get());
    }

    #[test]
    fn drain_timeout_defaults_and_accepts_an_override() {
        assert_eq!(drain_timeout_from(None).as_secs(), 30);
        assert_eq!(drain_timeout_from(Some("7")).as_secs(), 7);
        assert_eq!(drain_timeout_from(Some("invalid")).as_secs(), 30);
    }

    #[tokio::test]
    async fn drain_wait_has_a_bounded_timeout_for_a_stuck_process() {
        assert!(
            !wait_for_process_exit(std::time::Duration::ZERO, || Ok(true))
                .await
                .unwrap()
        );
        assert!(
            wait_for_process_exit(std::time::Duration::ZERO, || Ok(false))
                .await
                .unwrap()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn drain_treats_an_already_gone_pid_as_success() {
        drain_launchd_daemon(i64::MAX, std::time::Duration::ZERO)
            .await
            .unwrap();
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
            reauth_check_minutes: default_reauth_check_minutes(),
            ping_anthropic_model: default_ping_anthropic(),
            ping_openai_model: default_ping_openai(),
            ping_xai_model: default_ping_xai(),
            ping_gemini_model: default_ping_gemini(),
            ping_openrouter_model: default_ping_openrouter(),
            exo_url: default_exo_url(),
            exo_enabled_models: Vec::new(),
            openrouter_exposed_models: alex_proxy::default_openrouter_exposed_models(),
            gemini_project: String::new(),
            anthropic_upstream: "direct".into(),
            dario_mode_migrated: true,
            dario_api_key: String::new(),
            dario_claude_bin: None,
            dario_node_path: None,
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
            substitution: alex_proxy::SubstitutionConfig::default(),
            protection: alex_proxy::ProtectionPolicy::default(),
            notifications: Vec::new(),
            notification_cooldown_seconds: alex_proxy::notify::default_cooldown_seconds(),
            notification_timeout_seconds: alex_proxy::notify::default_timeout_seconds(),
        };
        let text = toml::to_string_pretty(&config).unwrap();
        let reloaded: Config = toml::from_str(&text).unwrap();
        assert_eq!(reloaded.port, config.port);
        assert_eq!(reloaded.local_key, config.local_key);
        assert_eq!(reloaded.data_dir, config.data_dir);
        assert!(reloaded.data_dir.is_absolute());
    }

    #[test]
    fn config_old_toml_defaults_new_collection_and_notification_fields() {
        let config = test_config(tmpdir("old-config"));
        let text = toml::to_string_pretty(&config).unwrap();
        let old_text = text
            .lines()
            .filter(|line| {
                !line.starts_with("harness_overrides")
                    && !line.starts_with("notifications")
                    && !line.starts_with("notification_cooldown_seconds")
                    && !line.starts_with("notification_timeout_seconds")
            })
            .collect::<Vec<_>>()
            .join("\n");
        let reloaded: Config = toml::from_str(&old_text).unwrap();
        assert!(reloaded.harness_overrides.is_empty());
        assert!(reloaded.notifications.is_empty());
        assert_eq!(
            reloaded.notification_cooldown_seconds,
            alex_proxy::notify::default_cooldown_seconds()
        );
        assert_eq!(
            reloaded.notification_timeout_seconds,
            alex_proxy::notify::default_timeout_seconds()
        );
    }

    #[test]
    fn config_without_substitution_section_keeps_cross_model_fallbacks_disabled() {
        let config: Config = toml::from_str(
            r#"
host = "127.0.0.1"
port = 4100
data_dir = "/tmp/alex-config-test"
local_key = "alx-test"
"#,
        )
        .unwrap();
        assert!(!config.substitution.enabled);
        assert!(config.substitution.fallbacks.is_empty());
        assert!(!config.protection.enabled);
        assert!(!config.protection.reroute_on_auth);
        assert_eq!(config.protection.retries, 1);
        assert!(config.protection.auto_return);
        assert!(config.protection.equivalencies.is_empty());
    }

    #[tokio::test]
    async fn legacy_config_protection_defaults_are_served_by_admin_api() {
        let config: Config = toml::from_str(
            r#"
host = "127.0.0.1"
port = 4100
data_dir = "/tmp/alex-config-test"
local_key = "alx-test"
"#,
        )
        .unwrap();
        let dir = tmpdir("legacy-protection-api");
        let state = alex_proxy::build_state(
            config.local_key.clone(),
            Arc::new(Vault::open(dir.join("vault")).unwrap()),
            Arc::new(Store::open(dir.join("store")).unwrap()),
            None,
            "http://127.0.0.1:4100".into(),
            config.upstream_stream_idle_timeout(),
        );
        alex_proxy::set_protection_policy(&state, config.protection);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, alex_proxy::router(state))
                .await
                .unwrap();
        });
        let policy: serde_json::Value = reqwest::Client::new()
            .get(format!("http://{address}/admin/protection"))
            .header("x-api-key", "alx-test")
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        server.abort();
        assert_eq!(
            policy,
            json!({
                "enabled": false,
                "reroute_on_auth": false,
                "retries": 1,
                "auto_return": true,
                "equivalencies": {},
            })
        );
    }

    #[test]
    fn protection_policy_persister_writes_config_toml() {
        let _guard = ENV_LOCK.lock().unwrap();
        let home = tmpdir("protection-policy-persist");
        std::env::set_var("ALEXANDRIA_HOME", &home);
        let config = Arc::new(std::sync::Mutex::new(test_config(home.clone())));
        save_config(&config.lock().unwrap()).unwrap();
        let persister = ConfigProtectionPolicyPersister {
            config: config.clone(),
        };
        let policy = alex_proxy::ProtectionPolicy {
            enabled: true,
            reroute_on_auth: true,
            retries: 2,
            auto_return: false,
            equivalencies: BTreeMap::from([(
                "claude-fable-5".into(),
                BTreeMap::from([("openai".into(), "gpt-5.6-sol".into())]),
            )]),
        };
        alex_proxy::ProtectionPolicyPersister::persist(&persister, &policy).unwrap();
        let (saved, fresh) = load_or_create_config().unwrap();
        std::env::remove_var("ALEXANDRIA_HOME");
        assert!(!fresh);
        assert!(saved.protection.enabled);
        assert_eq!(saved.protection.retries, 2);
        assert_eq!(
            saved.protection.equivalencies["claude-fable-5"]["openai"],
            "gpt-5.6-sol"
        );
    }

    #[test]
    fn notification_config_persister_writes_channels_and_old_toml_without_token_parses() {
        let _guard = ENV_LOCK.lock().unwrap();
        let home = tmpdir("notification-config-persist");
        std::env::set_var("ALEXANDRIA_HOME", &home);
        let config = Arc::new(std::sync::Mutex::new(test_config(home.clone())));
        save_config(&config.lock().unwrap()).unwrap();
        let persister = ConfigNotificationPersister {
            config: config.clone(),
        };
        let settings = alex_proxy::notify::NotificationSettings {
            channels: vec![alex_proxy::notify::NotificationChannelConfig {
                id: Some("telegram-1".into()),
                format: alex_proxy::notify::WebhookFormat::Telegram,
                token: Some("123:secret".into()),
                chat_id: Some("42".into()),
                ..Default::default()
            }],
            ..Default::default()
        };
        alex_proxy::NotificationConfigPersister::persist(&persister, &settings).unwrap();
        let (saved, fresh) = load_or_create_config().unwrap();
        assert!(!fresh);
        assert_eq!(saved.notifications[0].token.as_deref(), Some("123:secret"));

        let mut legacy = test_config(home.clone());
        legacy
            .notifications
            .push(alex_proxy::notify::NotificationChannelConfig {
                format: alex_proxy::notify::WebhookFormat::Telegram,
                url: "https://api.telegram.org/botlegacy/sendMessage".into(),
                chat_id: Some("42".into()),
                ..Default::default()
            });
        let raw = toml::to_string_pretty(&legacy).unwrap();
        assert!(!raw.contains("token"));
        let parsed: Config = toml::from_str(&raw).unwrap();
        assert!(parsed.notifications[0].token.is_none());
        std::env::remove_var("ALEXANDRIA_HOME");
    }

    fn telegram_channel(id: &str, token: &str) -> alex_proxy::notify::NotificationSettings {
        alex_proxy::notify::NotificationSettings {
            channels: vec![alex_proxy::notify::NotificationChannelConfig {
                id: Some(id.into()),
                format: alex_proxy::notify::WebhookFormat::Telegram,
                token: Some(token.into()),
                chat_id: Some("42".into()),
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    // The exact beta.2 regression: a user adds a Telegram bot token, then sets
    // the update channel via the unified picker, and the token vanishes. The
    // update-channel controller must persist the channel WITHOUT dropping the
    // notifications section.
    #[test]
    fn setting_update_channel_preserves_telegram_token() {
        let _guard = ENV_LOCK.lock().unwrap();
        let home = tmpdir("channel-vs-notification");
        std::env::set_var("ALEXANDRIA_HOME", &home);
        // Make the beta status probe fail fast (dead local port) so the
        // controller's cosmetic update check never touches the real network.
        std::env::set_var("ALEX_UPDATE_RELEASES_URL", "http://127.0.0.1:1/none");

        // The daemon's single shared config handle.
        let shared = Arc::new(std::sync::Mutex::new(test_config(home.clone())));
        save_config(&shared.lock().unwrap()).unwrap();

        // 1) Save a Telegram channel with a token.
        let notifications = ConfigNotificationPersister {
            config: shared.clone(),
        };
        alex_proxy::NotificationConfigPersister::persist(
            &notifications,
            &telegram_channel("telegram-1", "123:secret"),
        )
        .unwrap();

        // 2) Set the update channel to beta through the real controller.
        let controller = ConfigUpdateChannelController {
            config: shared.clone(),
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let outcome = runtime.block_on(alex_proxy::UpdateChannelController::set(
            &controller,
            "beta".into(),
        ));
        assert!(outcome.is_ok(), "channel set failed: {outcome:?}");

        // 3) Reload config.toml from disk: BOTH sections must be present.
        let (reloaded, _) = load_or_create_config().unwrap();
        std::env::remove_var("ALEX_UPDATE_RELEASES_URL");
        std::env::remove_var("ALEXANDRIA_HOME");
        assert_eq!(reloaded.update_channel, "beta");
        assert_eq!(
            reloaded.notifications.len(),
            1,
            "update channel write dropped the notifications section"
        );
        assert_eq!(
            reloaded.notifications[0].token.as_deref(),
            Some("123:secret"),
            "the Telegram bot token vanished after setting the update channel"
        );
    }

    // Reproduces the underlying cause: two config persisters that hold SEPARATE
    // `Arc<Mutex<Config>>` handles (as the exo persister did in beta.2). Saving
    // one section from a handle that never saw the other section must not wipe
    // it. Pre-fix this dropped the token; the read-modify-write persist keeps it.
    #[test]
    fn writer_with_separate_config_handle_does_not_drop_notifications() {
        let _guard = ENV_LOCK.lock().unwrap();
        let home = tmpdir("separate-handles");
        std::env::set_var("ALEXANDRIA_HOME", &home);
        save_config(&test_config(home.clone())).unwrap();

        // Two independent handles — exactly the divergence that caused the bug.
        let notif_config = Arc::new(std::sync::Mutex::new(test_config(home.clone())));
        let exo_config = Arc::new(std::sync::Mutex::new(test_config(home.clone())));

        let notifications = ConfigNotificationPersister {
            config: notif_config,
        };
        alex_proxy::NotificationConfigPersister::persist(
            &notifications,
            &telegram_channel("telegram-1", "123:secret"),
        )
        .unwrap();

        // The exo persister's handle never saw the token, yet its save must not
        // clobber it, because the write reloads the latest config from disk.
        let exo = ConfigExoPersister { config: exo_config };
        alex_proxy::ExoConfigPersister::persist(
            &exo,
            &alex_proxy::ExoConfig {
                url: "http://exo.local".into(),
                enabled_models: vec!["m".into()],
            },
        )
        .unwrap();

        let (reloaded, _) = load_or_create_config().unwrap();
        std::env::remove_var("ALEXANDRIA_HOME");
        assert_eq!(reloaded.exo_url, "http://exo.local");
        assert_eq!(
            reloaded.notifications.len(),
            1,
            "exo write from a separate handle dropped the notifications section"
        );
        assert_eq!(
            reloaded.notifications[0].token.as_deref(),
            Some("123:secret")
        );
    }

    // The curated OpenRouter exposure list persists through the shared
    // read-modify-write path and must survive an unrelated section write from a
    // separate config handle (same regression class as the exo/notification
    // persisters).
    #[test]
    fn openrouter_exposed_survives_unrelated_config_write() {
        let _guard = ENV_LOCK.lock().unwrap();
        let home = tmpdir("openrouter-exposed-persist");
        std::env::set_var("ALEXANDRIA_HOME", &home);
        save_config(&test_config(home.clone())).unwrap();

        // Persist a curated exposure list.
        let exposed_config = Arc::new(std::sync::Mutex::new(test_config(home.clone())));
        let exposed = ConfigOpenrouterExposedPersister {
            config: exposed_config,
        };
        alex_proxy::OpenrouterExposedPersister::persist(
            &exposed,
            &[
                "z-ai/glm-5.2".to_string(),
                "openai/gpt-5.6-terra".to_string(),
            ],
        )
        .unwrap();

        // An unrelated write from a SEPARATE handle that never saw the exposure
        // list must not clobber it, because every persist reloads from disk.
        let notif_config = Arc::new(std::sync::Mutex::new(test_config(home.clone())));
        let notifications = ConfigNotificationPersister {
            config: notif_config,
        };
        alex_proxy::NotificationConfigPersister::persist(
            &notifications,
            &telegram_channel("telegram-1", "123:secret"),
        )
        .unwrap();

        let (reloaded, _) = load_or_create_config().unwrap();
        std::env::remove_var("ALEXANDRIA_HOME");
        assert_eq!(
            reloaded.openrouter_exposed_models,
            vec![
                "z-ai/glm-5.2".to_string(),
                "openai/gpt-5.6-terra".to_string()
            ],
            "an unrelated notifications write dropped the OpenRouter exposure list"
        );
        assert_eq!(
            reloaded.notifications.len(),
            1,
            "the exposure write dropped the notifications section"
        );
    }

    // Migration: a pre-curation install has no `openrouter_exposed_models` key,
    // so it must default to the shipped examples (implicitly exposing "all" was
    // never intended); an explicit empty list is a real user choice and is kept.
    #[test]
    fn openrouter_exposed_defaults_when_unset_and_keeps_explicit_empty() {
        // Absent key -> shipped example list, including z-ai/glm-5.2.
        let unset: Config = toml::from_str(
            r#"
                host = "127.0.0.1"
                port = 4100
                data_dir = "/tmp/x"
                local_key = "alx-local"
            "#,
        )
        .unwrap();
        assert_eq!(
            unset.openrouter_exposed_models,
            alex_proxy::default_openrouter_exposed_models()
        );
        assert!(unset
            .openrouter_exposed_models
            .iter()
            .any(|id| id == "z-ai/glm-5.2"));
        // Curated starter set: OpenRouter's top-ranked models — small enough to
        // read as an example, not an endorsement of everything.
        assert!(unset.openrouter_exposed_models.len() <= 8);

        // Explicit empty list -> exposes nothing; never re-defaulted.
        let explicit_empty: Config = toml::from_str(
            r#"
                host = "127.0.0.1"
                port = 4100
                data_dir = "/tmp/x"
                local_key = "alx-local"
                openrouter_exposed_models = []
            "#,
        )
        .unwrap();
        assert!(explicit_empty.openrouter_exposed_models.is_empty());
    }

    // Fix #4: a token persisted to config.toml must be reloaded into the runtime
    // dispatcher on (re)start, so notifications keep working after a restart.
    #[test]
    fn restart_reloads_persisted_notifications_into_dispatcher() {
        let _guard = ENV_LOCK.lock().unwrap();
        let home = tmpdir("restart-reload-notifications");
        std::env::set_var("ALEXANDRIA_HOME", &home);

        // Persist a channel to disk (as a prior session would have).
        let shared = Arc::new(std::sync::Mutex::new(test_config(home.clone())));
        save_config(&shared.lock().unwrap()).unwrap();
        let notifications = ConfigNotificationPersister {
            config: shared.clone(),
        };
        alex_proxy::NotificationConfigPersister::persist(
            &notifications,
            &telegram_channel("telegram-1", "123:secret"),
        )
        .unwrap();

        // Simulate a restart: load config fresh and feed it to a new dispatcher
        // exactly as the daemon startup does.
        let (reloaded, _) = load_or_create_config().unwrap();
        std::env::remove_var("ALEXANDRIA_HOME");
        let state = test_state("restart-reload-notifications-state");
        alex_proxy::set_notifications(&state, reloaded.notification_settings());

        let view = state.notifications.read().unwrap().admin_view();
        let channels = view["channels"].as_array().unwrap();
        assert_eq!(
            channels.len(),
            1,
            "restart did not reload the persisted Telegram channel"
        );
        assert_eq!(channels[0]["id"], "telegram-1");
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
        assert!(body["checked_ms"].as_i64().unwrap() > 0);
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

    #[test]
    fn harness_cache_freshness_expires_after_one_minute() {
        let checked_ms = 1_000_000;
        assert!(harness_cache_is_fresh(checked_ms, checked_ms));
        assert!(harness_cache_is_fresh(
            checked_ms,
            checked_ms + HARNESS_CACHE_FRESH_MS
        ));
        assert!(!harness_cache_is_fresh(
            checked_ms,
            checked_ms + HARNESS_CACHE_FRESH_MS + 1
        ));
        assert!(!harness_cache_is_fresh(0, checked_ms));
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
    async fn harness_router_connect_kimi_writes_config_and_carries_kimi_models() {
        let _guard = ENV_LOCK.lock().unwrap();
        let home = tmpdir("router-connect-kimi");
        let bin_dir = tmpdir("router-connect-kimi-bin");
        let binary = fake_executable(&bin_dir, "kimi", "echo kimi 0.27.0");
        let config_dir = home.join("kimi-code");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::env::set_var("ALEXANDRIA_HOME", &home);
        let mut config = test_config(home.clone());
        config.harness_overrides.insert(
            "kimi".into(),
            HarnessOverride {
                binary: Some(binary),
                config_dir: Some(config_dir.clone()),
            },
        );
        save_config(&config).unwrap();

        let state = test_state("router-connect-kimi-state");
        // An active Kimi account is what makes the daemon advertise kimi/*
        // models; state_models must fold them in so they reach the harness.
        state
            .vault
            .upsert(alex_auth::Account {
                id: "kimi-router-test".into(),
                provider: alex_core::Provider::Kimi,
                kind: "oauth".into(),
                name: "Kimi router test".into(),
                description: None,
                paused: false,
                label: Some("Kimi".into()),
                access_token: Some("test-token".into()),
                refresh_token: None,
                id_token: None,
                api_key: None,
                expires_at_ms: None,
                last_refresh_ms: None,
                account_meta: serde_json::Value::Null,
                cooldown_until_ms: None,
                status: "active".into(),
                path: None,
            })
            .await
            .unwrap();
        let app = harness_admin_router(state.clone());

        // Regression for the "network connection lost" panic: kimi is
        // connect-capable but had no writer arm, so this endpoint hit
        // `unreachable!()` and dropped the connection mid-request.
        let (status, body) = router_json(
            app.clone(),
            Method::POST,
            "/admin/harnesses/kimi/connect",
            None,
        )
        .await;
        std::env::remove_var("ALEXANDRIA_HOME");
        assert_eq!(status, StatusCode::OK);
        assert!(body["key_id"].as_str().unwrap().starts_with("rk-"));
        assert!(body["models_total"].as_u64().unwrap() > 0);
        assert!(body["path"].as_str().unwrap().ends_with("config.toml"));

        // The freshly-added Kimi provider's models reached the connected harness.
        let added: Vec<&str> = body["added"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|m| m.as_str())
            .collect();
        assert!(added.contains(&"alex/kimi/k3"), "added models: {added:?}");
        let written = std::fs::read_to_string(config_dir.join("config.toml")).unwrap();
        assert!(written.contains("alexandria"));
        assert!(written.contains("alex/kimi/k3"));
        assert!(state
            .store
            .list_run_keys(false)
            .unwrap()
            .iter()
            .any(|k| k["label"] == "kimi"));
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
        assert!(plugin.contains("Generated by Alex for Amp"));
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

    #[test]
    fn up_step_planner_skips_satisfied_work() {
        assert_eq!(
            plan_up(true, true, true, false),
            UpStepPlan {
                install: false,
                connect: false,
                launch: true
            }
        );
        assert_eq!(
            plan_up(false, false, false, true),
            UpStepPlan {
                install: true,
                connect: true,
                launch: false
            }
        );
        assert_eq!(
            plan_up(true, true, false, true),
            UpStepPlan {
                install: false,
                connect: true,
                launch: false
            }
        );
    }
}
