use std::collections::{HashMap, HashSet};
#[cfg(windows)]
use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::FromStr;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use anyhow::{bail, Context, Result};
use futures_util::future::join_all;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use toml_edit::{value, Array, DocumentMut, InlineTable, Item, Table};

use crate::{ui, Config, HarnessOverride};

const PROVIDER_NAME: &str = "alexandria";
const PI_SESSION_EXTENSION_FILE: &str = "alexandria-session.ts";
const CODEX_CONFIG_FILE: &str = "config.toml";
const CODEX_CATALOG_FILE: &str = "alexandria-models.json";
const CODEX_NATIVE_CATALOG_FILE: &str = "alexandria-openai-models.json";
const CODEX_KEY_FILE: &str = "alexandria-api-key";
const CODEX_STATE_FILE: &str = "alexandria-harness-state.json";
const CODEX_BACKUP_FILE: &str = "alexandria-original-config.toml";
const CODEX_OPENAI_PROFILE_FILE: &str = "openai.config.toml";
pub(crate) const CODEX_ALEX_PROFILE_FILE: &str = "alex.config.toml";
const CODEX_HOOK_FILE: &str = "alexandria-session-hook.sh";
const CODEX_HOOK_CURL_FILE: &str = "alexandria-hook-curl.conf";
const CODEX_EVENT_LOG_FILE: &str = "alexandria-session-events.jsonl";
const CODEX_TOOL_HOOK_FILE: &str = "alexandria-tool-hook.sh";
const CODEX_TOOL_HOOK_CURL_FILE: &str = "alexandria-tool-curl.conf";
const CODEX_TOOL_EVENT_LOG_FILE: &str = "alexandria-tool-events.jsonl";
const CLAUDE_SETTINGS_FILE: &str = "settings.json";
pub(crate) const CLAUDE_PROFILE_FILE: &str = "alexandria-settings.json";
const CLAUDE_CATALOG_FILE: &str = "alexandria-models.json";
const CLAUDE_KEY_FILE: &str = "alexandria-api-key";
const CLAUDE_STATE_FILE: &str = "alexandria-harness-state.json";
const CLAUDE_BACKUP_FILE: &str = "alexandria-original-settings.json";
const CLAUDE_HOOK_FILE: &str = "alexandria-session-hook.sh";
const CLAUDE_HOOK_CURL_FILE: &str = "alexandria-hook-curl.conf";
const CLAUDE_EVENT_LOG_FILE: &str = "alexandria-session-events.jsonl";
const CLAUDE_TOOL_HOOK_FILE: &str = "alexandria-tool-hook.sh";
const CLAUDE_TOOL_HOOK_CURL_FILE: &str = "alexandria-tool-curl.conf";
const CLAUDE_TOOL_EVENT_LOG_FILE: &str = "alexandria-tool-events.jsonl";
const GROK_CONFIG_FILE: &str = "config.toml";
const GROK_KEY_FILE: &str = "alexandria-api-key";
const GROK_STATE_FILE: &str = "alexandria-harness-state.json";
const GROK_BACKUP_FILE: &str = "alexandria-original-config.toml";
const GROK_HOOK_FILE: &str = "alexandria-session-hook.sh";
const GROK_HOOK_CONFIG_FILE: &str = "alexandria-hook-curl.conf";
const GROK_HOOK_REGISTRATION_FILE: &str = "alexandria.json";
const GROK_EVENT_LOG_FILE: &str = "alexandria-session-events.jsonl";
const AMP_PLUGIN_FILE: &str = "alexandria.ts";
const AMP_KEY_FILE: &str = "alexandria-api-key";
const AMP_STATE_FILE: &str = "alexandria-harness-state.json";
const AMP_EVENT_LOG_FILE: &str = "alexandria-session-events.jsonl";
const KIMI_CONFIG_FILE: &str = "config.toml";
const KIMI_STATE_FILE: &str = "alexandria-harness-state.json";
const KIMI_BACKUP_FILE: &str = "alexandria-original-config.toml";
const KIMI_PROVIDER_NAME: &str = "alexandria";
const KIMI_INSTALL_DESCRIPTION: &str = "Alex backs up your Kimi Code config.toml and adds an OpenAI-compatible provider named `alexandria` plus selectable models named alex/* that route through the local proxy. Those models use a local-only harness credential and static Alex headers; Kimi's own kimi/* models and subscription authentication are left untouched. `alex harness disconnect kimi` removes the added provider and models and restores the backup.";
const PI_INSTALL_DESCRIPTION: &str = "Alex adds models named alex/* to Pi and installs a small session hook. The hook sets a local session header that the Alex proxy detects for tracing and removes before forwarding traffic upstream.";
const CODEX_INSTALL_DESCRIPTION: &str = "Alex backs up your original Codex configuration and any existing openai/alex profile files, then creates two fixed profiles. `codex --profile openai` uses normal Codex authentication; `codex --profile alex` routes alex/* models through the local proxy. A lifecycle hook sends session and sub-agent events to Alex so related traces can be grouped; Alex-only headers and credentials are not forwarded upstream.";
const CLAUDE_INSTALL_DESCRIPTION: &str = "Alex leaves your normal Claude Code configuration untouched and backs it up for reference. It creates a separate Alex settings profile that routes gateway models displayed as alex/* through the local proxy. Start it with `claude --settings ~/.claude/alexandria-settings.json`; plain `claude` continues to use your normal Claude authentication. Claude's native session, agent, and parent-agent headers provide exact nested traces, while lifecycle hooks add sub-agent names and timing.";
const GROK_INSTALL_DESCRIPTION: &str = "Alex backs up your Grok configuration, preserves Grok's built-in models and current default, and adds selectable models named alex/*. Those custom models use Grok's OpenAI-compatible Chat Completions backend, a local-only credential in the 0600 config, and static Alex harness headers. A trusted global lifecycle hook reports sessions and sub-agents to the local proxy; plain built-in Grok models continue to use your normal Grok authentication.";
const AMP_INSTALL_DESCRIPTION: &str = "Alex installs a reversible system Amp plugin that records thread, turn, tool, and exact native T-* session identifiers without changing prompts, permissions, tools, models, or Amp authentication. The plugin reports lineage to the local daemon and can recover exact built-in sub-agent edges when Amp exposes child thread IDs. Start Amp with `alex wrap amp` to capture model traffic; the wrapper and plugin join on the same native Amp thread ID. Amp's public plugin API cannot add an Alex model provider, so model selection remains Amp-native.";
const PI_SESSION_EXTENSION: &str = r#"// Generated by Alex. Re-run `alex connect pi` to refresh.
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

export default function (pi: ExtensionAPI) {
  const captureEnabled = __CAPTURE_ENABLED__;
  const toolEventsUrl = __TOOL_EVENTS_URL__;
  const harnessEventsUrl = __HARNESS_EVENTS_URL__;
  const apiKey = __API_KEY__;
  // Pi spawns subagents as child `pi` processes, so the parent's session id
  // reaches them only through inherited environment. Capture it before this
  // session overwrites the variable with its own id for grandchildren.
  const inheritedParent = process.env.ALEXANDRIA_SESSION_ID;
  let announced = false;
  let turnId: string | undefined;
  const post = (event: Record<string, unknown>) => {
    if (!captureEnabled) return;
    // Never delay or modify Pi's tool execution for telemetry.
    void fetch(toolEventsUrl, { method: "POST", headers: { "content-type": "application/json", "x-api-key": apiKey }, body: JSON.stringify(event) }).catch(() => {});
  };
  // Lineage is independent of the tool-capture opt-in.
  const announce = (sessionId: string) => {
    if (announced || !sessionId) return;
    announced = true;
    process.env.ALEXANDRIA_SESSION_ID = sessionId;
    if (!inheritedParent || inheritedParent === sessionId) return;
    void fetch(harnessEventsUrl, { method: "POST", headers: { "content-type": "application/json", "x-api-key": apiKey }, body: JSON.stringify({ hook_event_name: "SubagentStart", session_id: inheritedParent, agent_id: sessionId, agent_type: "pi", timestamp_ms: Date.now() }) }).catch(() => {});
  };
  pi.on("before_provider_headers", (event, ctx) => {
    // This extension is global, but session attribution belongs only on
    // requests routed through Alex's provider.
    if (ctx.model.provider !== "alexandria") return;

    announce(ctx.sessionManager.getSessionId());
    event.headers["x-session-id"] = ctx.sessionManager.getSessionId();
    if (turnId) event.headers["x-pi-turn-id"] = turnId;
  });
  pi.on("turn_start", (event, ctx) => { turnId = String(event.turnIndex); announce(ctx.sessionManager.getSessionId()); post({ phase: "turn_start", session_id: ctx.sessionManager.getSessionId(), turn_id: turnId, timestamp_ms: event.timestamp }); });
  pi.on("agent_start", (_event, ctx) => { announce(ctx.sessionManager.getSessionId()); post({ phase: "agent_start", session_id: ctx.sessionManager.getSessionId(), timestamp_ms: Date.now() }); });
  pi.on("agent_end", (_event, ctx) => post({ phase: "agent_end", session_id: ctx.sessionManager.getSessionId(), timestamp_ms: Date.now() }));
  pi.on("tool_execution_start", (event, ctx) => post({ phase: "start", session_id: ctx.sessionManager.getSessionId(), turn_id: turnId, tool_call_id: event.toolCallId, tool_name: event.toolName, args: event.args, timestamp_ms: Date.now() }));
  pi.on("tool_execution_end", (event, ctx) => post({ phase: "end", session_id: ctx.sessionManager.getSessionId(), turn_id: turnId, tool_call_id: event.toolCallId, tool_name: event.toolName, result: event.result, is_error: event.isError, timestamp_ms: Date.now() }));
  pi.on("turn_end", () => { turnId = undefined; });
}
"#;
const PI_MIN_VERSION: Version = Version {
    major: 0,
    minor: 80,
    patch: 0,
};
// Only used when the daemon's real catalog (the `pricing` table, seeded from
// alex-store/src/models.json) is unreachable. Keep it in step with that seed:
// claude-fable-5 was missing here, so any fallback silently dropped Fable from
// every harness even though routing supported it.
const FALLBACK_MODELS: &[&str] = &[
    "claude-opus-4-8",
    "claude-fable-5",
    "claude-sonnet-5",
    "claude-haiku-4-5",
    "gpt-5.6-sol",
    "gpt-5.6-terra",
    "gpt-5.6-luna",
    "gpt-5.5",
    "grok-code-fast-1",
    "gemini-2.5-flash",
];

pub(crate) struct HarnessSpec {
    pub(crate) name: &'static str,
    pub(crate) binary: &'static str,
    pub(crate) config_dir: fn(&Path) -> PathBuf,
    pub(crate) version_args: &'static [&'static str],
    pub(crate) supports_connect: bool,
    /// How `alex up` can install this harness when it is absent. Keeping this
    /// in the catalog means adding another npm-backed harness is data-only.
    pub(crate) install: Option<HarnessInstall>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HarnessInstall {
    Npm { package: &'static str },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InstallCommand {
    pub(crate) program: &'static str,
    pub(crate) args: Vec<String>,
}

pub(crate) const HARNESSES: &[HarnessSpec] = &[
    HarnessSpec {
        name: "pi",
        binary: "pi",
        config_dir: pi_config_dir_for_home,
        version_args: &["--version"],
        supports_connect: true,
        install: Some(HarnessInstall::Npm {
            package: "@earendil-works/pi-coding-agent",
        }),
    },
    HarnessSpec {
        name: "claude",
        binary: "claude",
        config_dir: claude_config_dir_for_home,
        version_args: &["--version"],
        supports_connect: true,
        install: None,
    },
    HarnessSpec {
        name: "codex",
        binary: "codex",
        config_dir: codex_config_dir_for_home,
        version_args: &["--version"],
        supports_connect: true,
        install: Some(HarnessInstall::Npm {
            package: "@openai/codex",
        }),
    },
    HarnessSpec {
        name: "omp",
        binary: "omp",
        config_dir: omp_config_dir_for_home,
        version_args: &["--version"],
        supports_connect: false,
        install: None,
    },
    HarnessSpec {
        name: "opencode",
        binary: "opencode",
        config_dir: opencode_config_dir_for_home,
        version_args: &["--version"],
        supports_connect: false,
        install: None,
    },
    HarnessSpec {
        name: "mini-swe-agent",
        binary: "mini-swe-agent",
        config_dir: mini_swe_agent_config_dir_for_home,
        version_args: &["--version"],
        supports_connect: false,
        install: None,
    },
    HarnessSpec {
        name: "kimi",
        binary: "kimi",
        config_dir: kimi_config_dir_for_home,
        version_args: &["--version"],
        supports_connect: true,
        install: Some(HarnessInstall::Npm {
            package: "@moonshot-ai/kimi-code",
        }),
    },
    HarnessSpec {
        name: "gemini",
        binary: "gemini",
        config_dir: gemini_config_dir_for_home,
        version_args: &["--version"],
        supports_connect: false,
        install: None,
    },
    HarnessSpec {
        name: "qwen",
        binary: "qwen",
        config_dir: qwen_config_dir_for_home,
        version_args: &["--version"],
        supports_connect: false,
        install: None,
    },
    HarnessSpec {
        name: "goose",
        binary: "goose",
        config_dir: goose_config_dir_for_home,
        version_args: &["--version"],
        supports_connect: false,
        install: None,
    },
    HarnessSpec {
        name: "opensage",
        binary: "opensage",
        config_dir: opensage_config_dir_for_home,
        version_args: &["--version"],
        supports_connect: false,
        install: None,
    },
    HarnessSpec {
        name: "pydantic-ai",
        binary: "pydantic-ai",
        config_dir: pydantic_ai_config_dir_for_home,
        version_args: &["--version"],
        supports_connect: false,
        install: None,
    },
    HarnessSpec {
        name: "stirrup",
        binary: "stirrup",
        config_dir: stirrup_config_dir_for_home,
        version_args: &["--version"],
        supports_connect: false,
        install: None,
    },
    HarnessSpec {
        name: "jcode",
        binary: "jcode",
        config_dir: jcode_config_dir_for_home,
        version_args: &["--version"],
        supports_connect: false,
        install: None,
    },
    HarnessSpec {
        name: "cursor",
        binary: "cursor-agent",
        config_dir: cursor_config_dir_for_home,
        version_args: &["--version"],
        supports_connect: false,
        install: None,
    },
    HarnessSpec {
        name: "amp",
        binary: "amp",
        config_dir: amp_config_dir_for_home,
        version_args: &["--version"],
        supports_connect: true,
        install: None,
    },
    HarnessSpec {
        name: "droid",
        binary: "droid",
        config_dir: droid_config_dir_for_home,
        version_args: &["--version"],
        supports_connect: false,
        install: None,
    },
    HarnessSpec {
        name: "grok",
        binary: "grok",
        config_dir: grok_config_dir_for_home,
        version_args: &["--version"],
        supports_connect: true,
        install: None,
    },
    HarnessSpec {
        name: "hermes",
        binary: "hermes",
        config_dir: hermes_config_dir_for_home,
        version_args: &["--version"],
        supports_connect: false,
        install: None,
    },
];

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

#[derive(Debug, Clone, Serialize)]
pub(crate) struct HarnessStatus {
    pub(crate) name: &'static str,
    pub(crate) installed: bool,
    pub(crate) binary: Option<String>,
    pub(crate) version: Option<String>,
    pub(crate) version_warning: Option<String>,
    pub(crate) config_dir: String,
    pub(crate) config_dir_exists: bool,
    pub(crate) connected: bool,
    pub(crate) tool_capture_enabled: bool,
    pub(crate) supports_connect: bool,
    #[serde(rename = "override")]
    pub(crate) override_: HarnessOverrideJson,
    pub(crate) daemon_reachable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) default_route: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) backup_path: Option<String>,
}

#[derive(Debug)]
struct HarnessDetection {
    binary: Option<PathBuf>,
    version: Option<String>,
    version_warning: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct HarnessOverrideJson {
    pub(crate) binary: Option<String>,
    pub(crate) config_dir: Option<String>,
}

#[derive(Debug)]
pub(crate) struct HarnessConnectSummary {
    pub(crate) key_id: String,
    pub(crate) models: Vec<String>,
    pub(crate) config_path: PathBuf,
    pub(crate) extension_path: PathBuf,
    pub(crate) version: Option<String>,
    pub(crate) base_url: String,
    pub(crate) added: Vec<String>,
    pub(crate) removed: Vec<String>,
    pub(crate) unchanged: usize,
    pub(crate) description: &'static str,
}

#[derive(Debug, Clone)]
struct VersionOutput {
    version: Option<String>,
    warning: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct DetectionCacheKey {
    binary: PathBuf,
    modified: Option<SystemTime>,
    size: Option<u64>,
}

static VERSION_DETECTION_CACHE: OnceLock<Mutex<HashMap<DetectionCacheKey, VersionOutput>>> =
    OnceLock::new();
const VERSION_DETECTION_CACHE_MAX_ENTRIES: usize = 64;

#[derive(Debug)]
struct PiDetection {
    binary: Option<PathBuf>,
    version_raw: Option<String>,
    version_check: VersionCheck,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct CodexManagedState {
    previous_model: Option<String>,
    previous_model_provider: Option<String>,
    previous_model_catalog_json: Option<String>,
    previous_hooks_enabled: Option<bool>,
    manages_model: bool,
    profiles_backed_up: bool,
    previous_openai_profile: Option<String>,
    previous_alex_profile: Option<String>,
    native_model: Option<String>,
    alex_model: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct ClaudeManagedState {
    previous_profile: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct GrokManagedState {
    managed_models: Vec<String>,
    previous_hook_registration: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct AmpManagedState {
    previous_plugin: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct KimiManagedState {
    managed_models: Vec<String>,
    /// Whether Alexandria added the `[providers."alexandria"]` table (so
    /// disconnect only removes a provider it created, never a user's own).
    added_provider: bool,
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
        Some("claude") => connect_claude(config, config_dir, json).await,
        Some("codex") => connect_codex(config, config_dir, json).await,
        Some("grok") => connect_grok(config, config_dir, json).await,
        Some("amp") => connect_amp(config, config_dir, json).await,
        Some("kimi") => connect_kimi(config, config_dir, json).await,
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

/// Connect using a controller-minted harness key. This path deliberately has
/// no `Config` argument: it is used by container entrypoints that do not have
/// the host's Alexandria home directory or local admin key.
pub(crate) async fn connect_with_preminted_key(
    harness: &str,
    explicit_config_dir: Option<PathBuf>,
    base_url: &str,
    api_key: String,
    key_id: Option<String>,
    capture_enabled: bool,
    json_out: bool,
) -> Result<()> {
    let spec = spec_by_name(harness).with_context(|| format!("unknown harness '{harness}'"))?;
    if !spec.supports_connect {
        bail!("harness '{harness}' does not support connect");
    }

    let detection = detect_harness_without_config(spec).await;
    let binary = detection
        .binary
        .as_ref()
        .with_context(|| format!("{harness} is not installed or not on PATH"))?;
    let config_dir = explicit_config_dir.unwrap_or_else(|| {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        (spec.config_dir)(&home)
    });
    if !config_dir.is_dir() {
        std::fs::create_dir_all(&config_dir).with_context(|| {
            format!("create {harness} config directory {}", config_dir.display())
        })?;
    }

    let base_url = base_url.trim_end_matches('/').to_string();
    if base_url.is_empty() {
        bail!("--url / ALEXANDRIA_URL must not be empty");
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;
    let models = fetch_models_with_harness_key(&base_url, &client, &api_key).await?;
    let codex_catalog = if harness == "codex" {
        Some(codex_model_catalog(binary, &models).await?)
    } else {
        None
    };
    let summary = write_preminted_connection(
        harness,
        config_dir,
        base_url,
        key_id.unwrap_or_else(|| "rk-external".into()),
        api_key,
        models,
        codex_catalog,
        detection.version,
        capture_enabled,
    )?;

    if json_out {
        let mut body = config_write_json(&summary, "provided", None);
        body["harness"] = json!(harness);
        println!("{}", serde_json::to_string_pretty(&body)?);
    } else {
        println!("{}", ui::section(&format!("{harness} connected")));
        println!("key id: {}", ui::amber(&summary.key_id));
        println!("models: {}", summary.models.len());
        println!("config: {}", summary.config_path.display());
    }
    Ok(())
}

fn write_preminted_connection(
    harness: &str,
    config_dir: PathBuf,
    base_url: String,
    key_id: String,
    api_key: String,
    models: Vec<String>,
    codex_catalog: Option<Value>,
    version: Option<String>,
    capture_enabled: bool,
) -> Result<HarnessConnectSummary> {
    match harness {
        "pi" => write_pi_connection_with_capture(
            config_dir,
            base_url,
            key_id,
            api_key,
            models,
            version,
            capture_enabled,
        ),
        "claude" => write_claude_connection_with_capture(
            config_dir,
            base_url,
            key_id,
            api_key,
            models,
            version,
            capture_enabled,
        ),
        "codex" => write_codex_connection_with_capture(
            config_dir,
            base_url,
            key_id,
            api_key,
            codex_catalog.context("Codex model catalog was not prepared")?,
            version,
            capture_enabled,
        ),
        "grok" => write_grok_connection(config_dir, base_url, key_id, api_key, models, version),
        "kimi" => write_kimi_connection(config_dir, base_url, key_id, api_key, models, version),
        "amp" => write_amp_connection_with_capture(
            config_dir,
            base_url,
            key_id,
            api_key,
            version,
            capture_enabled,
        ),
        _ => bail!("harness '{harness}' does not support connect"),
    }
}

pub(crate) async fn tool_capture_cmd(
    config: &Config,
    harness: String,
    enabled: Option<bool>,
    json_out: bool,
) -> Result<()> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;
    let base_url = normalized_base_url(config);
    if let Some(enabled) = enabled {
        let response = client
            .put(format!("{base_url}/admin/harnesses/{harness}/tool-capture"))
            .header("x-api-key", &config.local_key)
            .json(&json!({"enabled": enabled}))
            .send()
            .await
            .with_context(|| format!("could not reach the Alex daemon at {base_url}"))?;
        let status = response.status();
        let body: Value = response.json().await.unwrap_or_default();
        if !status.is_success() {
            bail!(daemon_error_text(&body));
        }
        if json_out {
            println!("{}", serde_json::to_string_pretty(&body)?);
        } else {
            println!(
                "{harness} tool capture {}",
                if body["tool_capture_enabled"].as_bool().unwrap_or(enabled) {
                    "enabled"
                } else {
                    "disabled"
                }
            );
        }
        return Ok(());
    }

    let response = client
        .get(format!("{base_url}/admin/harnesses"))
        .header("x-api-key", &config.local_key)
        .send()
        .await
        .with_context(|| format!("could not reach the Alex daemon at {base_url}"))?;
    let status = response.status();
    let body: Value = response.json().await.unwrap_or_default();
    if !status.is_success() {
        bail!(daemon_error_text(&body));
    }
    let status = body["harnesses"]
        .as_array()
        .and_then(|harnesses| {
            harnesses
                .iter()
                .find(|candidate| candidate["name"].as_str() == Some(harness.as_str()))
        })
        .with_context(|| format!("unknown harness '{harness}'"))?;
    let capture_enabled = status["tool_capture_enabled"].as_bool().unwrap_or(false);
    if json_out {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "harness": harness,
                "tool_capture_enabled": capture_enabled,
            }))?
        );
    } else {
        println!(
            "{harness} tool capture {}",
            if capture_enabled {
                "enabled"
            } else {
                "disabled"
            }
        );
    }
    Ok(())
}

fn daemon_error_text(body: &Value) -> String {
    body["error"]
        .as_str()
        .map(String::from)
        .or_else(|| body["error"]["message"].as_str().map(String::from))
        .unwrap_or_else(|| ui::truncate(&body.to_string(), 300))
}

pub(crate) async fn disconnect_cmd(
    config: &Config,
    harness: String,
    config_dir: Option<PathBuf>,
) -> Result<()> {
    match harness.as_str() {
        "pi" => disconnect_pi(config, config_dir).await,
        "claude" => disconnect_claude(config, config_dir).await,
        "codex" => disconnect_codex(config, config_dir).await,
        "grok" => disconnect_grok(config, config_dir).await,
        "amp" => disconnect_amp(config, config_dir).await,
        "kimi" => disconnect_kimi(config, config_dir).await,
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

async fn connect_status(
    config: &Config,
    config_dir: Option<PathBuf>,
    json_out: bool,
) -> Result<()> {
    let statuses = harness_statuses(config, config_dir, daemon_health(config).await).await?;
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
        let config_exists = if status.config_dir_exists {
            "yes"
        } else {
            "no"
        };
        let connected = if status.connected { "yes" } else { "no" };
        let daemon = if status.daemon_reachable {
            "up"
        } else {
            "down"
        };
        println!(
            "{} {} {} {} {} {}",
            ui::pad_right(status.name, 10),
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

pub(crate) async fn harness_statuses(
    config: &Config,
    pi_config_dir: Option<PathBuf>,
    daemon_reachable: bool,
) -> Result<Vec<HarnessStatus>> {
    let statuses = HARNESSES.iter().map(|spec| {
        let explicit_config_dir = if spec.name == "pi" {
            pi_config_dir.clone()
        } else {
            None
        };
        harness_status(config, spec, explicit_config_dir, daemon_reachable)
    });
    join_all(statuses).await.into_iter().collect()
}

pub(crate) async fn harness_status(
    config: &Config,
    spec: &'static HarnessSpec,
    explicit_config_dir: Option<PathBuf>,
    daemon_reachable: bool,
) -> Result<HarnessStatus> {
    let detection = detect_harness(config, spec).await;
    let config_dir = resolve_config_dir(config, spec, explicit_config_dir);
    let config_dir_exists = config_dir.is_dir();
    let connected = match spec.name {
        "pi" => models_json_connected(&config_dir.join("models.json")).unwrap_or(false),
        "claude" => claude_config_connected(&config_dir).unwrap_or(false),
        "codex" => codex_config_connected(&config_dir).unwrap_or(false),
        "grok" => grok_config_connected(&config_dir).unwrap_or(false),
        "amp" => amp_config_connected(&config_dir).unwrap_or(false),
        "kimi" => kimi_config_connected(&config_dir).unwrap_or(false),
        _ => false,
    };
    let override_ = override_json(config.harness_overrides.get(spec.name));
    let default_route = (spec.name == "codex")
        .then(|| codex_default_route(&config_dir).ok().flatten())
        .flatten();
    let backup_file = match spec.name {
        "claude" => Some(CLAUDE_BACKUP_FILE),
        "codex" => Some(CODEX_BACKUP_FILE),
        "grok" => Some(GROK_BACKUP_FILE),
        "kimi" => Some(KIMI_BACKUP_FILE),
        _ => None,
    };
    let backup_path = backup_file
        .map(|file| config_dir.join(file))
        .filter(|path| path.exists())
        .map(|path| path.to_string_lossy().to_string());
    Ok(HarnessStatus {
        name: spec.name,
        installed: detection.binary.is_some(),
        binary: detection
            .binary
            .as_ref()
            .map(|p| p.to_string_lossy().to_string()),
        version: detection.version,
        version_warning: detection.version_warning,
        config_dir: config_dir.to_string_lossy().to_string(),
        config_dir_exists,
        connected,
        tool_capture_enabled: config
            .harness_tool_capture
            .get(spec.name)
            .copied()
            .unwrap_or(false),
        supports_connect: spec.supports_connect,
        override_,
        daemon_reachable,
        default_route,
        backup_path,
    })
}

async fn connect_pi(config: &Config, config_dir: Option<PathBuf>, json_out: bool) -> Result<()> {
    let detection = detect_pi(config).await;
    if detection.binary.is_none() {
        bail!("pi is not installed or not on PATH; install it with `npm install -g @earendil-works/pi-coding-agent`");
    }
    if let Some(warning) = &detection.version_check.warning {
        eprintln!("{}", ui::amber(warning));
    }

    let config_dir = resolve_config_dir(config, pi_spec(), config_dir);
    if !config_dir.is_dir() {
        bail!(
            "pi config dir does not exist at {}; run pi once first (it creates ~/.pi/agent), or pass --config-dir",
            config_dir.display()
        );
    }

    if !daemon_health(config).await {
        bail!(
            "could not reach the Alex daemon at {}; start it with `alex daemon --background`",
            normalized_base_url(config)
        );
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()?;
    revoke_stale_keys_best_effort(config, &client, "pi").await;
    let minted = mint_harness_key(config, &client, "pi").await?;
    let models = fetch_models(config, &client)
        .await
        .unwrap_or_else(|| FALLBACK_MODELS.iter().map(|m| (*m).to_string()).collect());
    let summary = write_pi_connection_with_capture(
        config_dir,
        normalized_base_url(config),
        minted.id,
        minted.key,
        models,
        detection.version_raw,
        config
            .harness_tool_capture
            .get("pi")
            .copied()
            .unwrap_or(false),
    )?;

    if json_out {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "harness": "pi",
                "version": summary.version,
                "config_path": summary.config_path,
                "extension_path": summary.extension_path,
                "models": summary.models,
                "key_id": summary.key_id,
            }))?
        );
    } else {
        println!("{}", ui::section("pi connected"));
        println!("key id: {}", ui::amber(&summary.key_id));
        println!("models: {}", summary.models.len());
        println!("session extension: {}", summary.extension_path.display());
        println!();
        println!("pi --model alex/claude-opus-4-8");
        println!("or pick via /model inside pi — changes hot-reload");
    }
    Ok(())
}

async fn disconnect_pi(config: &Config, config_dir: Option<PathBuf>) -> Result<()> {
    let config_dir = resolve_config_dir(config, pi_spec(), config_dir);
    let was_connected = disconnect_pi_config(&config_dir)?;
    if !was_connected {
        println!("pi not connected");
        return Ok(());
    }

    let revoked = revoke_disconnected_harness_keys(config, "pi").await?;
    println!("disconnected pi; revoked {revoked} harness key(s)");
    Ok(())
}

async fn revoke_disconnected_harness_keys(config: &Config, harness: &str) -> Result<usize> {
    if daemon_health(config).await {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()?;
        match revoke_harness_keys(config, &client, harness).await {
            Ok(count) => Ok(count),
            Err(e) => {
                // A reachable daemon may still belong to a different local
                // profile/key (notably reset's isolated config). The local
                // store is authoritative for the config we just removed.
                eprintln!(
                    "{}",
                    ui::amber(&format!(
                        "daemon key revocation failed ({e}); revoking local {harness} harness keys"
                    ))
                );
                revoke_local_harness_keys(config, harness)
            }
        }
    } else {
        revoke_local_harness_keys(config, harness)
    }
}

fn revoke_local_harness_keys(config: &Config, harness: &str) -> Result<usize> {
    let store = alex_store::Store::open(config.data_dir.clone())?;
    let rows = store.list_run_keys(true)?;
    let mut revoked = 0;
    for id in rows
        .iter()
        .filter(|row| {
            row["kind"].as_str() == Some("harness") && row["label"].as_str() == Some(harness)
        })
        .filter_map(|row| row["id"].as_str())
    {
        if store.revoke_run_key(id)? {
            revoked += 1;
        }
    }
    Ok(revoked)
}

async fn connect_claude(
    config: &Config,
    config_dir: Option<PathBuf>,
    json_out: bool,
) -> Result<()> {
    let detection = detect_harness(config, claude_spec()).await;
    detection
        .binary
        .as_ref()
        .context("claude is not installed or not on PATH")?;
    let config_dir = resolve_config_dir(config, claude_spec(), config_dir);
    if !config_dir.is_dir() {
        bail!(
            "claude config dir does not exist at {}; run claude once first, or pass --config-dir",
            config_dir.display()
        );
    }
    if !daemon_health(config).await {
        bail!(
            "could not reach the Alex daemon at {}; start it with `alex daemon --background`",
            normalized_base_url(config)
        );
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()?;
    revoke_stale_keys_best_effort(config, &client, "claude").await;
    let minted = mint_harness_key(config, &client, "claude").await?;
    let models = fetch_models(config, &client)
        .await
        .unwrap_or_else(|| FALLBACK_MODELS.iter().map(|m| (*m).to_string()).collect());
    let summary = write_claude_connection_with_capture(
        config_dir.clone(),
        normalized_base_url(config),
        minted.id,
        minted.key,
        models,
        detection.version,
        config
            .harness_tool_capture
            .get("claude")
            .copied()
            .unwrap_or(false),
    )?;

    if json_out {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "harness": "claude",
                "version": summary.version,
                "config_path": summary.config_path,
                "hook_path": summary.extension_path,
                "models": summary.models,
                "key_id": summary.key_id,
            }))?
        );
    } else {
        println!("{}", ui::section("claude connected"));
        println!("key id: {}", ui::amber(&summary.key_id));
        println!("models: {}", summary.models.len());
        println!("settings profile: {}", summary.config_path.display());
        println!();
        println!("claude --settings {}", summary.config_path.display());
        println!("plain `claude` still uses your normal Claude Code configuration");
    }
    Ok(())
}

async fn disconnect_claude(config: &Config, config_dir: Option<PathBuf>) -> Result<()> {
    let config_dir = resolve_config_dir(config, claude_spec(), config_dir);
    let was_connected = disconnect_claude_config(&config_dir)?;
    if !was_connected {
        println!("claude not connected");
        return Ok(());
    }

    let revoked = revoke_disconnected_harness_keys(config, "claude").await?;
    println!("disconnected claude; revoked {revoked} harness key(s)");
    Ok(())
}

async fn connect_codex(config: &Config, config_dir: Option<PathBuf>, json_out: bool) -> Result<()> {
    let detection = detect_harness(config, codex_spec()).await;
    let binary = detection
        .binary
        .context("codex is not installed or not on PATH")?;
    let config_dir = resolve_config_dir(config, codex_spec(), config_dir);
    if !config_dir.is_dir() {
        bail!(
            "codex config dir does not exist at {}; run codex once first, or pass --config-dir",
            config_dir.display()
        );
    }
    if !daemon_health(config).await {
        bail!(
            "could not reach the Alex daemon at {}; start it with `alex daemon --background`",
            normalized_base_url(config)
        );
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()?;
    revoke_stale_keys_best_effort(config, &client, "codex").await;
    let minted = mint_harness_key(config, &client, "codex").await?;
    let available = fetch_models(config, &client)
        .await
        .unwrap_or_else(|| FALLBACK_MODELS.iter().map(|m| (*m).to_string()).collect());
    let catalog = codex_model_catalog(&binary, &available).await?;
    let summary = write_codex_connection_with_capture(
        config_dir,
        normalized_base_url(config),
        minted.id,
        minted.key,
        catalog,
        detection.version,
        config
            .harness_tool_capture
            .get("codex")
            .copied()
            .unwrap_or(false),
    )?;

    if json_out {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "harness": "codex",
                "version": summary.version,
                "config_path": summary.config_path,
                "hook_path": summary.extension_path,
                "models": summary.models,
                "key_id": summary.key_id,
            }))?
        );
    } else {
        println!("{}", ui::section("codex connected"));
        println!("key id: {}", ui::amber(&summary.key_id));
        println!("models: {}", summary.models.len());
        println!("session hook: {}", summary.extension_path.display());
        println!();
        println!("Start a new Codex session to use Alex.");
        println!("Codex will ask you to review and trust the new lifecycle hooks.");
        println!(
            "Until you open `codex` interactively once and approve that review, headless `codex exec` silently skips untrusted hooks and no tool events are recorded."
        );
    }
    Ok(())
}

async fn disconnect_codex(config: &Config, config_dir: Option<PathBuf>) -> Result<()> {
    let config_dir = resolve_config_dir(config, codex_spec(), config_dir);
    let was_connected = disconnect_codex_config(&config_dir)?;
    if !was_connected {
        println!("codex not connected");
        return Ok(());
    }

    let revoked = revoke_disconnected_harness_keys(config, "codex").await?;
    println!("disconnected codex; revoked {revoked} harness key(s)");
    Ok(())
}

async fn connect_grok(config: &Config, config_dir: Option<PathBuf>, json_out: bool) -> Result<()> {
    let detection = detect_harness(config, grok_spec()).await;
    detection
        .binary
        .as_ref()
        .context("grok is not installed or not on PATH")?;
    let config_dir = resolve_config_dir(config, grok_spec(), config_dir);
    if !config_dir.is_dir() {
        bail!(
            "grok config dir does not exist at {}; run grok once first, or pass --config-dir",
            config_dir.display()
        );
    }
    if !daemon_health(config).await {
        bail!(
            "could not reach the Alex daemon at {}; start it with `alex daemon --background`",
            normalized_base_url(config)
        );
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()?;
    revoke_stale_keys_best_effort(config, &client, "grok").await;
    let minted = mint_harness_key(config, &client, "grok").await?;
    let models = fetch_models(config, &client)
        .await
        .unwrap_or_else(|| FALLBACK_MODELS.iter().map(|m| (*m).to_string()).collect());
    let summary = write_grok_connection(
        config_dir,
        normalized_base_url(config),
        minted.id,
        minted.key,
        models,
        detection.version,
    )?;

    if json_out {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "harness": "grok",
                "version": summary.version,
                "config_path": summary.config_path,
                "hook_path": summary.extension_path,
                "models": summary.models,
                "key_id": summary.key_id,
            }))?
        );
    } else {
        println!("{}", ui::section("grok connected"));
        println!("key id: {}", ui::amber(&summary.key_id));
        println!("models: {}", summary.models.len());
        println!("session hook: {}", summary.extension_path.display());
        println!();
        println!("grok --model {}", summary.models[0]);
        println!("or pick an alex/* model with /model; Grok's native models remain available");
    }
    Ok(())
}

async fn disconnect_grok(config: &Config, config_dir: Option<PathBuf>) -> Result<()> {
    let config_dir = resolve_config_dir(config, grok_spec(), config_dir);
    let was_connected = disconnect_grok_config(&config_dir)?;
    if !was_connected {
        println!("grok not connected");
        return Ok(());
    }

    let revoked = revoke_disconnected_harness_keys(config, "grok").await?;
    println!("disconnected grok; revoked {revoked} harness key(s)");
    Ok(())
}

async fn connect_kimi(config: &Config, config_dir: Option<PathBuf>, json_out: bool) -> Result<()> {
    let detection = detect_harness(config, kimi_spec()).await;
    detection.binary.as_ref().context(
        "kimi is not installed or not on PATH; install it with `npm install -g @moonshot-ai/kimi-code`",
    )?;
    let config_dir = resolve_config_dir(config, kimi_spec(), config_dir);
    if !config_dir.is_dir() {
        bail!(
            "kimi config dir does not exist at {}; run kimi once first, or pass --config-dir",
            config_dir.display()
        );
    }
    if !daemon_health(config).await {
        bail!(
            "could not reach the Alex daemon at {}; start it with `alex daemon --background`",
            normalized_base_url(config)
        );
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()?;
    revoke_stale_keys_best_effort(config, &client, "kimi").await;
    let minted = mint_harness_key(config, &client, "kimi").await?;
    let models = fetch_models(config, &client)
        .await
        .unwrap_or_else(|| FALLBACK_MODELS.iter().map(|m| (*m).to_string()).collect());
    let summary = write_kimi_connection(
        config_dir,
        normalized_base_url(config),
        minted.id,
        minted.key,
        models,
        detection.version,
    )?;

    if json_out {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "harness": "kimi",
                "version": summary.version,
                "config_path": summary.config_path,
                "models": summary.models,
                "key_id": summary.key_id,
            }))?
        );
    } else {
        println!("{}", ui::section("kimi connected"));
        println!("key id: {}", ui::amber(&summary.key_id));
        println!("models: {}", summary.models.len());
        println!("config: {}", summary.config_path.display());
        println!();
        println!("kimi --model {}", summary.models[0]);
        println!("or pick an alex/* model in Kimi's model picker; Kimi's own kimi/* models remain available");
    }
    Ok(())
}

async fn disconnect_kimi(config: &Config, config_dir: Option<PathBuf>) -> Result<()> {
    let config_dir = resolve_config_dir(config, kimi_spec(), config_dir);
    let was_connected = disconnect_kimi_config(&config_dir)?;
    if !was_connected {
        println!("kimi not connected");
        return Ok(());
    }

    let revoked = revoke_disconnected_harness_keys(config, "kimi").await?;
    println!("disconnected kimi; revoked {revoked} harness key(s)");
    Ok(())
}

async fn connect_amp(config: &Config, config_dir: Option<PathBuf>, json_out: bool) -> Result<()> {
    let detection = detect_harness(config, amp_spec()).await;
    detection
        .binary
        .as_ref()
        .context("amp is not installed or not on PATH")?;
    let config_dir = resolve_config_dir(config, amp_spec(), config_dir);
    if !config_dir.is_dir() {
        bail!(
            "amp config dir does not exist at {}; run amp once first, or pass --config-dir",
            config_dir.display()
        );
    }
    if !daemon_health(config).await {
        bail!(
            "could not reach the Alex daemon at {}; start it with `alex daemon --background`",
            normalized_base_url(config)
        );
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()?;
    revoke_stale_keys_best_effort(config, &client, "amp").await;
    let minted = mint_harness_key(config, &client, "amp").await?;
    let summary = write_amp_connection_with_capture(
        config_dir,
        normalized_base_url(config),
        minted.id,
        minted.key,
        detection.version,
        config
            .harness_tool_capture
            .get("amp")
            .copied()
            .unwrap_or(false),
    )?;

    if json_out {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "harness": "amp",
                "version": summary.version,
                "plugin_path": summary.config_path,
                "key_id": summary.key_id,
                "capture_command": "alex wrap amp",
            }))?
        );
    } else {
        println!("{}", ui::section("amp connected"));
        println!("key id: {}", ui::amber(&summary.key_id));
        println!("lifecycle plugin: {}", summary.config_path.display());
        println!();
        println!("alex wrap amp");
        println!("Amp keeps its native models and authentication; restart Amp to load the plugin");
    }
    Ok(())
}

async fn disconnect_amp(config: &Config, config_dir: Option<PathBuf>) -> Result<()> {
    let config_dir = resolve_config_dir(config, amp_spec(), config_dir);
    let was_connected = disconnect_amp_config(&config_dir)?;
    if !was_connected {
        println!("amp not connected");
        return Ok(());
    }

    let revoked = revoke_disconnected_harness_keys(config, "amp").await?;
    println!("disconnected amp; revoked {revoked} harness key(s)");
    Ok(())
}

pub(crate) async fn codex_model_catalog(binary: &Path, available: &[String]) -> Result<Value> {
    codex_model_catalog_with_timeout(binary, available, Duration::from_secs(5)).await
}

async fn codex_model_catalog_with_timeout(
    binary: &Path,
    available: &[String],
    timeout: Duration,
) -> Result<Value> {
    let binary = binary.to_path_buf();
    let display = binary.display().to_string();
    let output = match tokio::time::timeout(
        timeout,
        tokio::task::spawn_blocking(move || {
            Command::new(binary)
                .args(["debug", "models", "--bundled"])
                .output()
        }),
    )
    .await
    {
        Err(_) => bail!("timed out reading bundled model catalog from {display}"),
        Ok(Err(e)) => return Err(e).context("Codex model catalog task failed"),
        Ok(Ok(Err(e))) => {
            return Err(e)
                .with_context(|| format!("could not read bundled model catalog from {display}"))
        }
        Ok(Ok(Ok(output))) => output,
    };
    if !output.status.success() {
        bail!(
            "`{} debug models --bundled` failed: {}",
            display,
            ui::truncate(&String::from_utf8_lossy(&output.stderr), 300)
        );
    }
    let mut catalog: Value = serde_json::from_slice(&output.stdout)
        .context("codex bundled model catalog was not valid JSON")?;
    let models = catalog["models"]
        .as_array()
        .context("codex bundled model catalog did not contain a models array")?;
    let native = models.clone();
    let template = native
        .iter()
        .find(|row| row["slug"].as_str() == Some("gpt-5.6-sol"))
        .or_else(|| native.first())
        .cloned()
        .context("codex bundled model catalog was empty")?;
    let mut by_slug = native
        .iter()
        .filter_map(|row| {
            row["slug"]
                .as_str()
                .map(|slug| (slug.to_string(), row.clone()))
        })
        .collect::<std::collections::HashMap<_, _>>();
    let mut seen = native
        .iter()
        .filter_map(|row| row["slug"].as_str().map(String::from))
        .collect::<HashSet<_>>();
    let mut merged = native;
    for available_id in available {
        let bare_id = available_id
            .strip_prefix("alex/")
            .or_else(|| available_id.strip_prefix("alexandria/"))
            .or_else(|| available_id.strip_prefix("cove/"))
            .unwrap_or(available_id);
        let alex_id = format!("alex/{bare_id}");
        if !seen.insert(alex_id.clone()) {
            continue;
        }
        let native_match = by_slug.remove(bare_id);
        let mut row = native_match.clone().unwrap_or_else(|| template.clone());
        row["slug"] = json!(alex_id);
        row["display_name"] = json!(format!("alex/{bare_id}"));
        if native_match.is_none() {
            row["description"] = json!(format!("Routed through Alex: {bare_id}"));
            if let Some(object) = row.as_object_mut() {
                object.remove("availability_nux");
                object.remove("upgrade");
            }
        }
        merged.push(row);
    }
    catalog["models"] = Value::Array(merged);
    Ok(catalog)
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn write_pi_connection(
    config_dir: PathBuf,
    base_url: String,
    key_id: String,
    api_key: String,
    models: Vec<String>,
    version: Option<String>,
) -> Result<HarnessConnectSummary> {
    write_pi_connection_with_capture(
        config_dir, base_url, key_id, api_key, models, version, false,
    )
}

pub(crate) fn write_pi_connection_with_capture(
    config_dir: PathBuf,
    base_url: String,
    key_id: String,
    api_key: String,
    models: Vec<String>,
    version: Option<String>,
    capture_enabled: bool,
) -> Result<HarnessConnectSummary> {
    let models = short_alex_model_ids(models);
    let models_path = config_dir.join("models.json");
    let before = read_pi_model_ids(&config_dir);
    upsert_pi_provider(&models_path, &base_url, &api_key, &models)?;
    let (added, removed, unchanged) = model_id_diff(&before, &models);
    let extension_path =
        install_pi_session_extension(&config_dir, &base_url, &api_key, capture_enabled)?;
    Ok(HarnessConnectSummary {
        key_id,
        models,
        config_path: models_path,
        extension_path,
        version,
        base_url,
        added,
        removed,
        unchanged,
        description: PI_INSTALL_DESCRIPTION,
    })
}

/// Set the model used by a connected harness's Alexandria profile. `alex up`
/// calls this after connection so its explicit default wins over a harness's
/// usual "first catalog entry" choice.
pub(crate) fn set_default_model(harness: &str, config_dir: &Path, model: &str) -> Result<()> {
    let model = short_alex_model_id(model);
    match harness {
        "pi" => {
            let path = config_dir.join("settings.json");
            let mut settings = if path.exists() {
                let raw = std::fs::read_to_string(&path)?;
                serde_json::from_str::<Value>(&raw)
                    .with_context(|| format!("could not parse {}", path.display()))?
            } else {
                json!({})
            };
            let object = settings
                .as_object_mut()
                .with_context(|| format!("{} must contain a JSON object", path.display()))?;
            object.insert("defaultProvider".into(), json!(PROVIDER_NAME));
            object.insert(
                "defaultModel".into(),
                json!(model.strip_prefix("alex/").unwrap_or(&model)),
            );
            atomic_write_json(&path, &settings)
        }
        "codex" => set_codex_default_model(config_dir, &model),
        _ => bail!("setting a default Alex model is not yet supported for {harness}"),
    }
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn write_claude_connection(
    config_dir: PathBuf,
    base_url: String,
    key_id: String,
    api_key: String,
    models: Vec<String>,
    version: Option<String>,
) -> Result<HarnessConnectSummary> {
    write_claude_connection_with_capture(
        config_dir, base_url, key_id, api_key, models, version, false,
    )
}

pub(crate) fn write_claude_connection_with_capture(
    config_dir: PathBuf,
    base_url: String,
    key_id: String,
    api_key: String,
    models: Vec<String>,
    version: Option<String>,
    capture_enabled: bool,
) -> Result<HarnessConnectSummary> {
    let settings_path = config_dir.join(CLAUDE_SETTINGS_FILE);
    let profile_path = config_dir.join(CLAUDE_PROFILE_FILE);
    let catalog_path = config_dir.join(CLAUDE_CATALOG_FILE);
    let key_path = config_dir.join(CLAUDE_KEY_FILE);
    let state_path = config_dir.join(CLAUDE_STATE_FILE);
    let backup_path = config_dir.join(CLAUDE_BACKUP_FILE);
    let hook_path = config_dir.join(CLAUDE_HOOK_FILE);
    let hook_curl_path = config_dir.join(CLAUDE_HOOK_CURL_FILE);
    let tool_hook_path = config_dir.join(CLAUDE_TOOL_HOOK_FILE);
    let tool_hook_curl_path = config_dir.join(CLAUDE_TOOL_HOOK_CURL_FILE);
    let before = read_claude_model_ids(&config_dir);
    let display_models = short_alex_model_ids(models);
    if display_models.is_empty() {
        bail!("Alex did not return any Claude-compatible gateway models");
    }
    let gateway_models = display_models
        .iter()
        .map(|model| {
            format!(
                "claude-alex/{}",
                model.strip_prefix("alex/").unwrap_or(model)
            )
        })
        .collect::<Vec<_>>();

    let managed_before = state_path.exists();
    let state = if managed_before {
        read_claude_state(&state_path)?
    } else {
        ClaudeManagedState {
            previous_profile: read_optional_text(&profile_path)?,
        }
    };

    // Validate the normal user settings before advertising a backup. The
    // Alexandria profile is additive and never changes this file.
    let original_settings = if settings_path.exists() {
        let raw = std::fs::read_to_string(&settings_path)?;
        let parsed: Value = serde_json::from_str(&raw).with_context(|| {
            format!(
                "could not parse {}; aborting without changes",
                settings_path.display()
            )
        })?;
        if !parsed.is_object() {
            bail!("{} must contain a JSON object", settings_path.display());
        }
        raw
    } else {
        "{}\n".to_string()
    };
    if !managed_before || !backup_path.exists() {
        atomic_write_text(&backup_path, &original_settings)?;
    }

    let selected_model = if managed_before {
        read_json_object(&profile_path)?["model"]
            .as_str()
            .filter(|model| gateway_models.iter().any(|candidate| candidate == model))
            .map(String::from)
            .unwrap_or_else(|| preferred_claude_model(&gateway_models).clone())
    } else {
        preferred_claude_model(&gateway_models).clone()
    };
    let hook_base_url = base_url.replace("://0.0.0.0", "://127.0.0.1");
    let hook_url = format!("{}/harness-events", hook_base_url.trim_end_matches('/'));
    let hook_command = claude_hook_command(&hook_path);
    let hook_handler = json!({
        "type": "command",
        "command": hook_command,
        "timeout": 5,
        "statusMessage": "Recording Alex session lineage",
    });
    let mut hooks = serde_json::Map::new();
    for event in ["SessionStart", "SubagentStart", "SubagentStop"] {
        hooks.insert(
            event.to_string(),
            json!([{ "hooks": [hook_handler.clone()] }]),
        );
    }
    if capture_enabled {
        let handler = json!({
            "type": "command",
            "command": claude_hook_command(&tool_hook_path),
            "timeout": 5,
            "statusMessage": "Recording Alex tool execution",
        });
        for event in ["PreToolUse", "PostToolUse", "PostToolUseFailure"] {
            hooks.insert(event.to_string(), json!([{ "hooks": [handler.clone()] }]));
        }
    }
    let profile = json!({
        "$schema": "https://json.schemastore.org/claude-code-settings.json",
        "model": selected_model,
        "apiKeyHelper": claude_api_key_helper(&key_path),
        "env": {
            "ANTHROPIC_BASE_URL": base_url,
            "ANTHROPIC_CUSTOM_HEADERS": format!(
                "x-alexandria-harness: claude\nx-alexandria-harness-version: {}",
                version.as_deref().unwrap_or("unknown")
            ),
            "CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY": "1",
            "CLAUDE_CODE_API_KEY_HELPER_TTL_MS": "0",
        },
        "hooks": Value::Object(hooks),
    });
    let catalog = json!({
        "models": gateway_models
            .iter()
            .zip(display_models.iter())
            .map(|(id, display_name)| json!({
                "id": id,
                "display_name": display_name,
            }))
            .collect::<Vec<_>>(),
    });

    atomic_write_json(&state_path, &serde_json::to_value(&state)?)?;
    atomic_write_json(&catalog_path, &catalog)?;
    atomic_write_text(&key_path, &format!("{api_key}\n"))?;
    install_harness_hook(
        &hook_path,
        &config_dir.join(CLAUDE_EVENT_LOG_FILE),
        &hook_curl_path,
        &hook_url,
        &key_path,
        &api_key,
        "Claude Code",
    )?;
    if capture_enabled {
        install_harness_hook(
            &tool_hook_path,
            &config_dir.join(CLAUDE_TOOL_EVENT_LOG_FILE),
            &tool_hook_curl_path,
            &format!("{}/tool-events", hook_base_url.trim_end_matches('/')),
            &key_path,
            &api_key,
            "Claude Code",
        )?;
    }
    atomic_write_json(&profile_path, &profile)?;

    let (added, removed, unchanged) = model_id_diff(&before, &display_models);
    Ok(HarnessConnectSummary {
        key_id,
        models: display_models,
        config_path: profile_path,
        extension_path: hook_path,
        version,
        base_url,
        added,
        removed,
        unchanged,
        description: CLAUDE_INSTALL_DESCRIPTION,
    })
}

pub(crate) fn read_claude_api_key(config_dir: &Path) -> Option<String> {
    std::fs::read_to_string(config_dir.join(CLAUDE_KEY_FILE))
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub(crate) fn read_claude_model_ids(config_dir: &Path) -> Vec<String> {
    let Ok(catalog) = read_json_object(&config_dir.join(CLAUDE_CATALOG_FILE)) else {
        return Vec::new();
    };
    catalog["models"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|row| row["display_name"].as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn read_claude_hook_events(config_dir: &Path) -> Vec<Value> {
    let Ok(raw) = std::fs::read_to_string(config_dir.join(CLAUDE_EVENT_LOG_FILE)) else {
        return Vec::new();
    };
    raw.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect()
}

pub(crate) fn claude_config_connected(config_dir: &Path) -> Result<bool> {
    let state_path = config_dir.join(CLAUDE_STATE_FILE);
    let profile_path = config_dir.join(CLAUDE_PROFILE_FILE);
    if !state_path.exists() || !profile_path.exists() {
        return Ok(false);
    }
    let profile = read_json_object(&profile_path)?;
    Ok(profile["env"]["ANTHROPIC_BASE_URL"].is_string()
        && profile["env"]["ANTHROPIC_CUSTOM_HEADERS"]
            .as_str()
            .is_some_and(|headers| headers.contains("x-alexandria-harness: claude"))
        && profile["apiKeyHelper"].is_string())
}

pub(crate) fn disconnect_claude_config(config_dir: &Path) -> Result<bool> {
    let state_path = config_dir.join(CLAUDE_STATE_FILE);
    let mut changed = clear_stale_claude_gateway_selection(config_dir)?;
    if !state_path.exists() {
        return Ok(changed);
    }
    let state = read_claude_state(&state_path)?;
    restore_managed_text_file(
        &config_dir.join(CLAUDE_PROFILE_FILE),
        state.previous_profile.as_deref(),
    )?;
    changed = true;
    for path in [
        config_dir.join(CLAUDE_CATALOG_FILE),
        config_dir.join(CLAUDE_KEY_FILE),
        config_dir.join(CLAUDE_HOOK_FILE),
        config_dir.join(CLAUDE_HOOK_CURL_FILE),
        config_dir.join(CLAUDE_TOOL_HOOK_FILE),
        config_dir.join(CLAUDE_TOOL_HOOK_CURL_FILE),
        state_path,
    ] {
        if path.exists() {
            std::fs::remove_file(&path)
                .with_context(|| format!("could not remove {}", path.display()))?;
            changed = true;
        }
    }
    Ok(changed)
}

pub(crate) fn set_claude_tool_capture(
    config_dir: &Path,
    base_url: &str,
    enabled: bool,
) -> Result<()> {
    if !claude_config_connected(config_dir)? {
        bail!("Claude is not connected to Alex; connect it before enabling tool capture");
    }
    let profile_path = config_dir.join(CLAUDE_PROFILE_FILE);
    let mut profile = read_json_object(&profile_path)?;
    let tool_hook_path = config_dir.join(CLAUDE_TOOL_HOOK_FILE);
    let tool_command = claude_hook_command(&tool_hook_path);
    remove_hook_handlers(&mut profile, &tool_command, &tool_hook_path)?;
    if enabled {
        let key = read_claude_api_key(config_dir)
            .context("Claude is not connected to Alex; missing harness key")?;
        let hook_base_url = base_url.replace("://0.0.0.0", "://127.0.0.1");
        install_harness_hook(
            &tool_hook_path,
            &config_dir.join(CLAUDE_TOOL_EVENT_LOG_FILE),
            &config_dir.join(CLAUDE_TOOL_HOOK_CURL_FILE),
            &format!("{}/tool-events", hook_base_url.trim_end_matches('/')),
            &config_dir.join(CLAUDE_KEY_FILE),
            &key,
            "Claude Code",
        )?;
        let hooks = profile["hooks"]
            .as_object_mut()
            .context("Claude Alex profile hooks must be an object")?;
        let handler = json!({ "type": "command", "command": tool_command, "timeout": 5, "statusMessage": "Recording Alex tool execution" });
        for event in ["PreToolUse", "PostToolUse", "PostToolUseFailure"] {
            hooks.insert(event.to_string(), json!([{ "hooks": [handler.clone()] }]));
        }
    }
    atomic_write_json(&profile_path, &profile)
}

fn remove_hook_handlers(value: &mut Value, command: &str, hook_path: &Path) -> Result<bool> {
    let Some(hooks) = value.get_mut("hooks") else {
        return Ok(false);
    };
    let hooks = hooks.as_object_mut().context("hooks must be an object")?;
    let path_text = hook_path.to_string_lossy();
    let mut changed = false;
    for groups in hooks.values_mut() {
        let Some(groups) = groups.as_array_mut() else {
            continue;
        };
        for group in groups.iter_mut() {
            let Some(handlers) = group.get_mut("hooks").and_then(Value::as_array_mut) else {
                continue;
            };
            let before = handlers.len();
            handlers.retain(|handler| {
                let configured = handler["command"].as_str().unwrap_or_default();
                configured != command && !configured.contains(path_text.as_ref())
            });
            changed |= handlers.len() != before;
        }
        groups.retain(|group| {
            group["hooks"]
                .as_array()
                .is_none_or(|handlers| !handlers.is_empty())
        });
    }
    hooks.retain(|_, groups| groups.as_array().is_none_or(|groups| !groups.is_empty()));
    Ok(changed)
}

/// Claude Code can persist a model selected from an additive `--settings`
/// profile into the user's ordinary settings file. Once Alexandria's profile
/// is removed that provider-prefixed model is no longer resolvable, so remove
/// only the stale Alexandria selection and its discovered gateway cache.
/// Native model choices and unrelated gateway caches are left untouched.
fn clear_stale_claude_gateway_selection(config_dir: &Path) -> Result<bool> {
    let mut changed = false;
    let settings_path = config_dir.join(CLAUDE_SETTINGS_FILE);
    if settings_path.exists() {
        let mut settings = read_json_object(&settings_path)?;
        let stale_model = settings["model"]
            .as_str()
            .is_some_and(|model| model.starts_with("claude-alex/"));
        if stale_model {
            settings
                .as_object_mut()
                .expect("read_json_object returns an object")
                .remove("model");
            atomic_write_json(&settings_path, &settings)?;
            changed = true;
        }
    }

    let cache_path = config_dir.join("cache").join("gateway-models.json");
    if cache_path.exists() {
        let cache = read_json_object(&cache_path)?;
        let is_alexandria_cache = cache["models"].as_array().is_some_and(|models| {
            models.iter().any(|model| {
                model["id"]
                    .as_str()
                    .is_some_and(|id| id.starts_with("claude-alex/"))
            })
        });
        if is_alexandria_cache {
            std::fs::remove_file(&cache_path)
                .with_context(|| format!("could not remove {}", cache_path.display()))?;
            changed = true;
        }
    }
    Ok(changed)
}

fn read_claude_state(path: &Path) -> Result<ClaudeManagedState> {
    let raw = std::fs::read_to_string(path)?;
    serde_json::from_str(&raw).with_context(|| {
        format!(
            "could not parse Alex Claude state {}; aborting without changes",
            path.display()
        )
    })
}

fn read_json_object(path: &Path) -> Result<Value> {
    let raw = if path.exists() {
        std::fs::read_to_string(path)?
    } else {
        "{}".to_string()
    };
    let value: Value = serde_json::from_str(&raw).with_context(|| {
        format!(
            "could not parse {}; aborting without changes",
            path.display()
        )
    })?;
    if !value.is_object() {
        bail!("{} must contain a JSON object", path.display());
    }
    Ok(value)
}

#[cfg(not(windows))]
fn claude_api_key_helper(path: &Path) -> String {
    format!(
        "/bin/cat {}",
        shell_single_quote(path.to_string_lossy().as_ref())
    )
}

#[cfg(windows)]
fn claude_api_key_helper(path: &Path) -> String {
    format!(
        "powershell -NoProfile -Command Get-Content -Raw '{}'",
        path.to_string_lossy().replace('\'', "''")
    )
}

fn claude_hook_command(path: &Path) -> String {
    codex_hook_command(path)
}

pub(crate) fn write_grok_connection(
    config_dir: PathBuf,
    base_url: String,
    key_id: String,
    api_key: String,
    models: Vec<String>,
    version: Option<String>,
) -> Result<HarnessConnectSummary> {
    let config_path = config_dir.join(GROK_CONFIG_FILE);
    let state_path = config_dir.join(GROK_STATE_FILE);
    let backup_path = config_dir.join(GROK_BACKUP_FILE);
    let key_path = config_dir.join(GROK_KEY_FILE);
    let hook_path = config_dir.join(GROK_HOOK_FILE);
    let hook_curl_path = config_dir.join(GROK_HOOK_CONFIG_FILE);
    let hook_registration_path = config_dir.join("hooks").join(GROK_HOOK_REGISTRATION_FILE);
    let display_models = short_alex_model_ids(models);
    if display_models.is_empty() {
        bail!("Alex did not return any Grok-compatible models");
    }
    let before = read_grok_model_ids(&config_dir);
    let managed_before = state_path.exists();
    let mut state = if managed_before {
        read_grok_state(&state_path)?
    } else {
        GrokManagedState {
            managed_models: Vec::new(),
            previous_hook_registration: read_optional_text(&hook_registration_path)?,
        }
    };
    let previous_managed = state.managed_models.iter().cloned().collect::<HashSet<_>>();
    let original_config = if config_path.exists() {
        std::fs::read_to_string(&config_path)?
    } else {
        String::new()
    };
    let mut config_doc = DocumentMut::from_str(&original_config).with_context(|| {
        format!(
            "could not parse {}; aborting without changes",
            config_path.display()
        )
    })?;
    if !managed_before || !backup_path.exists() {
        atomic_write_text(&backup_path, &original_config)?;
    }
    if config_doc.get("model").is_none() {
        config_doc["model"] = Item::Table(Table::new());
    }
    let model_configs = config_doc["model"]
        .as_table_mut()
        .with_context(|| "Grok config.toml model must be a table; aborting without changes")?;
    for model in &display_models {
        if model_configs.contains_key(model) && !previous_managed.contains(model) {
            bail!(
                "{} already defines model.{}; Alex will not replace an unmanaged model",
                config_path.display(),
                model
            );
        }
    }
    for previous in &state.managed_models {
        model_configs.remove(previous);
    }
    for model in &display_models {
        let mut entry = Table::new();
        entry["model"] = value(model);
        entry["name"] = value(model);
        entry["description"] = value(format!(
            "Routed through Alex: {}",
            model.strip_prefix("alex/").unwrap_or(model)
        ));
        entry["base_url"] = value(format!("{}/v1", base_url.trim_end_matches('/')));
        entry["api_key"] = value(&api_key);
        entry["api_backend"] = value("chat_completions");
        entry["context_window"] = value(200_000);
        let mut headers = InlineTable::new();
        headers.insert("x-alexandria-harness", "grok".into());
        headers.insert(
            "x-alexandria-harness-version",
            version.as_deref().unwrap_or("unknown").into(),
        );
        entry["extra_headers"] = value(headers);
        model_configs[model] = Item::Table(entry);
    }
    state.managed_models = display_models.clone();

    let hook_base_url = base_url.replace("://0.0.0.0", "://127.0.0.1");
    let hook_url = format!("{}/harness-events", hook_base_url.trim_end_matches('/'));
    let hook_command = codex_hook_command(&hook_path);
    let handler = json!({
        "type": "command",
        "command": hook_command,
        "timeout": 5,
    });
    let hook_registration = json!({
        "hooks": {
            "SessionStart": [{"hooks": [handler.clone()]}],
            "SubagentStart": [{"hooks": [handler.clone()]}],
            "SubagentStop": [{"hooks": [handler]}],
        }
    });
    std::fs::create_dir_all(
        hook_registration_path
            .parent()
            .expect("Grok hook registration path has a parent"),
    )?;
    atomic_write_json(&state_path, &serde_json::to_value(&state)?)?;
    atomic_write_text(&key_path, &format!("{api_key}\n"))?;
    install_harness_hook(
        &hook_path,
        &config_dir.join(GROK_EVENT_LOG_FILE),
        &hook_curl_path,
        &hook_url,
        &key_path,
        &api_key,
        "Grok",
    )?;
    atomic_write_json(&hook_registration_path, &hook_registration)?;
    atomic_write_text(&config_path, &config_doc.to_string())?;

    let (added, removed, unchanged) = model_id_diff(&before, &display_models);
    Ok(HarnessConnectSummary {
        key_id,
        models: display_models,
        config_path,
        extension_path: hook_path,
        version,
        base_url,
        added,
        removed,
        unchanged,
        description: GROK_INSTALL_DESCRIPTION,
    })
}

pub(crate) fn read_grok_api_key(config_dir: &Path) -> Option<String> {
    std::fs::read_to_string(config_dir.join(GROK_KEY_FILE))
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub(crate) fn read_grok_model_ids(config_dir: &Path) -> Vec<String> {
    read_grok_state(&config_dir.join(GROK_STATE_FILE))
        .map(|state| state.managed_models)
        .unwrap_or_default()
}

pub(crate) fn read_grok_hook_events(config_dir: &Path) -> Vec<Value> {
    let Ok(raw) = std::fs::read_to_string(config_dir.join(GROK_EVENT_LOG_FILE)) else {
        return Vec::new();
    };
    raw.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect()
}

pub(crate) fn grok_config_connected(config_dir: &Path) -> Result<bool> {
    let state_path = config_dir.join(GROK_STATE_FILE);
    if !state_path.exists() {
        return Ok(false);
    }
    let state = read_grok_state(&state_path)?;
    if state.managed_models.is_empty() {
        return Ok(false);
    }
    let config = read_grok_config(&config_dir.join(GROK_CONFIG_FILE))?;
    let Some(models) = config.get("model").and_then(Item::as_table_like) else {
        return Ok(false);
    };
    Ok(state.managed_models.iter().all(|model| {
        models
            .get(model)
            .and_then(Item::as_table_like)
            .and_then(|entry| entry.get("extra_headers"))
            .and_then(Item::as_inline_table)
            .and_then(|headers| headers.get("x-alexandria-harness"))
            .and_then(|value| value.as_str())
            == Some("grok")
    }))
}

pub(crate) fn disconnect_grok_config(config_dir: &Path) -> Result<bool> {
    let state_path = config_dir.join(GROK_STATE_FILE);
    if !state_path.exists() {
        return Ok(false);
    }
    let state = read_grok_state(&state_path)?;
    let config_path = config_dir.join(GROK_CONFIG_FILE);
    let mut config = read_grok_config(&config_path)?;
    if let Some(models) = config.get_mut("model").and_then(Item::as_table_mut) {
        for model in &state.managed_models {
            models.remove(model);
        }
        if models.is_empty() {
            config.as_table_mut().remove("model");
        }
    }
    atomic_write_text(&config_path, &config.to_string())?;
    restore_managed_text_file(
        &config_dir.join("hooks").join(GROK_HOOK_REGISTRATION_FILE),
        state.previous_hook_registration.as_deref(),
    )?;
    for path in [
        config_dir.join(GROK_KEY_FILE),
        config_dir.join(GROK_HOOK_FILE),
        config_dir.join(GROK_HOOK_CONFIG_FILE),
        state_path,
    ] {
        if path.exists() {
            std::fs::remove_file(&path)
                .with_context(|| format!("could not remove {}", path.display()))?;
        }
    }
    Ok(true)
}

fn read_grok_config(path: &Path) -> Result<DocumentMut> {
    let raw = if path.exists() {
        std::fs::read_to_string(path)?
    } else {
        String::new()
    };
    DocumentMut::from_str(&raw).with_context(|| {
        format!(
            "could not parse {}; aborting without changes",
            path.display()
        )
    })
}

fn read_grok_state(path: &Path) -> Result<GrokManagedState> {
    let raw = std::fs::read_to_string(path)?;
    serde_json::from_str(&raw).with_context(|| {
        format!(
            "could not parse Alex Grok state {}; aborting without changes",
            path.display()
        )
    })
}

// ---------------------------------------------------------------------------
// Kimi Code (Moonshot AI) bidirectional connect: add alex/* models to Kimi.
// ---------------------------------------------------------------------------

/// Rewrite `~/.kimi-code/config.toml` to add an OpenAI-compatible `alexandria`
/// provider pointing at the local proxy plus alex/* models, backing up the
/// original once. Reversible via [`disconnect_kimi_config`].
pub(crate) fn write_kimi_connection(
    config_dir: PathBuf,
    base_url: String,
    key_id: String,
    api_key: String,
    models: Vec<String>,
    version: Option<String>,
) -> Result<HarnessConnectSummary> {
    let config_path = config_dir.join(KIMI_CONFIG_FILE);
    let state_path = config_dir.join(KIMI_STATE_FILE);
    let backup_path = config_dir.join(KIMI_BACKUP_FILE);
    let display_models = short_alex_model_ids(models);
    if display_models.is_empty() {
        bail!("Alex did not return any Kimi-compatible models");
    }
    let before = read_kimi_model_ids(&config_dir);
    let managed_before = state_path.exists();
    let mut state = if managed_before {
        read_kimi_state(&state_path)?
    } else {
        KimiManagedState::default()
    };
    let previous_managed: HashSet<String> = state.managed_models.iter().cloned().collect();

    let original_config = if config_path.exists() {
        std::fs::read_to_string(&config_path)?
    } else {
        String::new()
    };
    let mut doc = DocumentMut::from_str(&original_config).with_context(|| {
        format!(
            "could not parse {}; aborting without changes",
            config_path.display()
        )
    })?;
    if !managed_before || !backup_path.exists() {
        atomic_write_text(&backup_path, &original_config)?;
    }
    let orphan_adoptable = kimi_provider_is_ours(&doc);

    if doc.get("providers").is_none() {
        doc["providers"] = Item::Table(Table::new());
    }
    let providers = doc["providers"].as_table_mut().with_context(|| {
        "Kimi config.toml `providers` must be a table; aborting without changes"
    })?;
    // An existing provider block without a state marker is either an orphan
    // from a partial disconnect (its api_key is one of our alxk- run keys —
    // adopt and replace it, since bailing used to strand Kimi on a revoked key
    // with no way to reconnect) or genuinely user-authored (leave it alone).
    if providers.contains_key(KIMI_PROVIDER_NAME) && !state.added_provider && !orphan_adoptable {
        bail!(
            "{} already defines providers.{}; Alex will not replace an unmanaged provider",
            config_path.display(),
            KIMI_PROVIDER_NAME
        );
    }
    let mut provider_entry = Table::new();
    provider_entry["type"] = value("openai");
    provider_entry["api_key"] = value(&api_key);
    provider_entry["base_url"] = value(format!("{}/v1", base_url.trim_end_matches('/')));
    providers[KIMI_PROVIDER_NAME] = Item::Table(provider_entry);
    state.added_provider = true;

    if doc.get("models").is_none() {
        doc["models"] = Item::Table(Table::new());
    }
    let model_configs = doc["models"]
        .as_table_mut()
        .with_context(|| "Kimi config.toml `models` must be a table; aborting without changes")?;
    for model in &display_models {
        if model_configs.contains_key(model) && !previous_managed.contains(model) {
            let ours = is_alex_kimi_model(model)
                || model_configs
                    .get(model)
                    .and_then(Item::as_table_like)
                    .and_then(|entry| entry.get("provider"))
                    .and_then(Item::as_str)
                    == Some(KIMI_PROVIDER_NAME);
            if !ours {
                bail!(
                    "{} already defines models.{}; Alex will not replace an unmanaged model",
                    config_path.display(),
                    model
                );
            }
        }
    }
    for previous in &state.managed_models {
        model_configs.remove(previous);
    }
    // Sweep orphaned alex/* entries a lost state file no longer tracks, so a
    // reconnect never leaves stale models pointing at a dead key.
    let orphaned: Vec<String> = model_configs
        .iter()
        .filter(|(model, entry)| {
            is_alex_kimi_model(model)
                || entry
                    .as_table_like()
                    .and_then(|entry| entry.get("provider"))
                    .and_then(Item::as_str)
                    == Some(KIMI_PROVIDER_NAME)
        })
        .map(|(model, _)| model.to_string())
        .collect();
    for model in orphaned {
        model_configs.remove(&model);
    }
    for model in &display_models {
        let mut entry = Table::new();
        entry["provider"] = value(KIMI_PROVIDER_NAME);
        entry["model"] = value(model);
        entry["max_context_size"] = value(200_000);
        entry["display_name"] = value(model);
        model_configs[model] = Item::Table(entry);
    }
    state.managed_models = display_models.clone();

    atomic_write_json(&state_path, &serde_json::to_value(&state)?)?;
    atomic_write_text(&config_path, &doc.to_string())?;

    let (added, removed, unchanged) = model_id_diff(&before, &display_models);
    Ok(HarnessConnectSummary {
        key_id,
        models: display_models,
        config_path: config_path.clone(),
        extension_path: config_path,
        version,
        base_url,
        added,
        removed,
        unchanged,
        description: KIMI_INSTALL_DESCRIPTION,
    })
}

fn read_kimi_state(path: &Path) -> Result<KimiManagedState> {
    let raw = std::fs::read_to_string(path)?;
    serde_json::from_str(&raw).with_context(|| {
        format!(
            "could not parse Alex Kimi state {}; aborting without changes",
            path.display()
        )
    })
}

pub(crate) fn read_kimi_model_ids(config_dir: &Path) -> Vec<String> {
    read_kimi_state(&config_dir.join(KIMI_STATE_FILE))
        .map(|state| state.managed_models)
        .unwrap_or_default()
}

/// The Alexandria harness key Kimi stores inside its config.toml
/// (`[providers."alexandria"].api_key`). Lets the daemon `refresh-config`
/// endpoint reuse the existing key instead of minting (and orphaning) a new one
/// on every refresh, matching how the other harnesses read their stored key.
pub(crate) fn read_kimi_api_key(config_dir: &Path) -> Option<String> {
    let doc = read_grok_config(&config_dir.join(KIMI_CONFIG_FILE)).ok()?;
    doc.get("providers")
        .and_then(Item::as_table_like)
        .and_then(|providers| providers.get(KIMI_PROVIDER_NAME))
        .and_then(Item::as_table_like)
        .and_then(|provider| provider.get("api_key"))
        .and_then(Item::as_str)
        .filter(|key| !key.is_empty())
        .map(String::from)
}

pub(crate) fn kimi_config_connected(config_dir: &Path) -> Result<bool> {
    let state_path = config_dir.join(KIMI_STATE_FILE);
    if !state_path.exists() {
        return Ok(false);
    }
    let state = read_kimi_state(&state_path)?;
    if state.managed_models.is_empty() {
        return Ok(false);
    }
    let doc = read_grok_config(&config_dir.join(KIMI_CONFIG_FILE))?;
    let Some(models) = doc.get("models").and_then(Item::as_table_like) else {
        return Ok(false);
    };
    Ok(state.managed_models.iter().all(|model| {
        models
            .get(model)
            .and_then(Item::as_table_like)
            .and_then(|entry| entry.get("provider"))
            .and_then(Item::as_str)
            == Some(KIMI_PROVIDER_NAME)
    }))
}

pub(crate) fn disconnect_kimi_config(config_dir: &Path) -> Result<bool> {
    let state_path = config_dir.join(KIMI_STATE_FILE);
    let config_path = config_dir.join(KIMI_CONFIG_FILE);
    if !state_path.exists() {
        if !config_path.exists() {
            return Ok(false);
        }
        // No managed-state marker, but Alexandria's entries are self-identifying
        // — remove them anyway so a lost state file can't leave Kimi wired to a
        // revoked key.
        let mut doc = read_grok_config(&config_path)?;
        let mut changed = remove_self_identifying_kimi_entries(&mut doc);
        changed |= clear_stale_kimi_default_selection(config_dir, &mut doc)?;
        if changed {
            atomic_write_text(&config_path, &doc.to_string())?;
        }
        return Ok(changed);
    }
    let state = read_kimi_state(&state_path)?;
    let mut doc = read_grok_config(&config_path)?;
    if let Some(models) = doc.get_mut("models").and_then(Item::as_table_mut) {
        for model in &state.managed_models {
            models.remove(model);
        }
        if models.is_empty() {
            doc.as_table_mut().remove("models");
        }
    }
    if state.added_provider {
        if let Some(providers) = doc.get_mut("providers").and_then(Item::as_table_mut) {
            providers.remove(KIMI_PROVIDER_NAME);
            if providers.is_empty() {
                doc.as_table_mut().remove("providers");
            }
        }
    }
    clear_stale_kimi_default_selection(config_dir, &mut doc)?;
    atomic_write_text(&config_path, &doc.to_string())?;
    if state_path.exists() {
        std::fs::remove_file(&state_path)
            .with_context(|| format!("could not remove {}", state_path.display()))?;
    }
    Ok(true)
}

/// Kimi persists a model chosen as its default in the same config file as the
/// additive Alexandria provider. Restore a native selection without replacing
/// unrelated configuration changes made since Alexandria connected.
fn clear_stale_kimi_default_selection(config_dir: &Path, doc: &mut DocumentMut) -> Result<bool> {
    let stale_model = doc
        .get("default_model")
        .and_then(Item::as_str)
        .is_some_and(is_alex_kimi_model);
    if !stale_model {
        return Ok(false);
    }

    let backup_path = config_dir.join(KIMI_BACKUP_FILE);
    let backup_default = if backup_path.exists() {
        read_grok_config(&backup_path)?
            .get("default_model")
            .and_then(Item::as_str)
            .filter(|model| !is_alex_kimi_model(model))
            .map(String::from)
    } else {
        None
    };
    let surviving_default = doc
        .get("models")
        .and_then(Item::as_table)
        .and_then(|models| {
            let native_models: Vec<&str> = models
                .iter()
                .map(|(model, _)| model)
                .filter(|model| !is_alex_kimi_model(model))
                .collect();
            native_models
                .iter()
                .find(|model| model.starts_with("kimi-code/"))
                .or_else(|| native_models.first())
                .map(|model| (*model).to_string())
        });

    if let Some(native_default) = backup_default.or(surviving_default) {
        doc["default_model"] = value(native_default);
    } else {
        doc.as_table_mut().remove("default_model");
    }
    Ok(true)
}

fn is_alex_kimi_model(model: &str) -> bool {
    model.starts_with("alex/") || model.starts_with("alexandria/")
}

/// True when the `alexandria` provider block in a Kimi config carries an Alex
/// run key (`alxk-` prefix) — the marker that it was written by `alex connect`
/// rather than authored by the user.
fn kimi_provider_is_ours(doc: &DocumentMut) -> bool {
    doc.get("providers")
        .and_then(Item::as_table_like)
        .and_then(|providers| providers.get(KIMI_PROVIDER_NAME))
        .and_then(Item::as_table_like)
        .and_then(|entry| entry.get("api_key"))
        .and_then(Item::as_str)
        .is_some_and(|key| key.starts_with("alxk-"))
}

/// Removes every Kimi config entry that is Alex's by construction — the
/// `alexandria` provider block (when its key proves it ours) and any model
/// named `alex/*` or routed through that provider. These entries are
/// self-identifying, so they can be cleaned up even when the managed-state
/// marker file has been lost (a partial disconnect or a crash between writes
/// previously stranded them, leaving Kimi pointed at a revoked key and
/// blocking reconnects). A user-authored provider that happens to share the
/// name is left alone.
fn remove_self_identifying_kimi_entries(doc: &mut DocumentMut) -> bool {
    let provider_is_ours = kimi_provider_is_ours(doc);
    let mut changed = false;
    if let Some(models) = doc.get_mut("models").and_then(Item::as_table_mut) {
        let orphaned: Vec<String> = models
            .iter()
            .filter(|(model, entry)| {
                is_alex_kimi_model(model)
                    || (provider_is_ours
                        && entry
                            .as_table_like()
                            .and_then(|entry| entry.get("provider"))
                            .and_then(Item::as_str)
                            == Some(KIMI_PROVIDER_NAME))
            })
            .map(|(model, _)| model.to_string())
            .collect();
        for model in orphaned {
            models.remove(&model);
            changed = true;
        }
        if models.is_empty() {
            doc.as_table_mut().remove("models");
        }
    }
    if provider_is_ours {
        if let Some(providers) = doc.get_mut("providers").and_then(Item::as_table_mut) {
            if providers.remove(KIMI_PROVIDER_NAME).is_some() {
                changed = true;
            }
            if providers.is_empty() {
                doc.as_table_mut().remove("providers");
            }
        }
    }
    changed
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn write_amp_connection(
    config_dir: PathBuf,
    base_url: String,
    key_id: String,
    api_key: String,
    version: Option<String>,
) -> Result<HarnessConnectSummary> {
    write_amp_connection_with_capture(config_dir, base_url, key_id, api_key, version, false)
}

pub(crate) fn write_amp_connection_with_capture(
    config_dir: PathBuf,
    base_url: String,
    key_id: String,
    api_key: String,
    version: Option<String>,
    capture_enabled: bool,
) -> Result<HarnessConnectSummary> {
    let plugin_path = amp_plugin_path(&config_dir);
    let key_path = config_dir.join(AMP_KEY_FILE);
    let state_path = config_dir.join(AMP_STATE_FILE);
    let event_log_path = config_dir.join(AMP_EVENT_LOG_FILE);
    let state = if state_path.exists() {
        read_amp_state(&state_path)?
    } else {
        AmpManagedState {
            previous_plugin: read_optional_text(&plugin_path)?,
        }
    };
    std::fs::create_dir_all(
        plugin_path
            .parent()
            .expect("Amp plugin path has a parent directory"),
    )?;
    let hook_base_url = base_url.replace("://0.0.0.0", "://127.0.0.1");
    let event_url = format!("{}/harness-events", hook_base_url.trim_end_matches('/'));
    let tool_event_url = format!("{}/tool-events", hook_base_url.trim_end_matches('/'));
    let source = amp_plugin_source(
        &hook_base_url,
        &event_url,
        &tool_event_url,
        &key_path,
        &event_log_path,
        capture_enabled,
    )?;

    atomic_write_json(&state_path, &serde_json::to_value(&state)?)?;
    atomic_write_text(&key_path, &format!("{api_key}\n"))?;
    atomic_write_text(&plugin_path, &source)?;

    Ok(HarnessConnectSummary {
        key_id,
        models: Vec::new(),
        config_path: plugin_path.clone(),
        extension_path: plugin_path,
        version,
        base_url,
        added: Vec::new(),
        removed: Vec::new(),
        unchanged: 0,
        description: AMP_INSTALL_DESCRIPTION,
    })
}

pub(crate) fn read_amp_api_key(config_dir: &Path) -> Option<String> {
    std::fs::read_to_string(config_dir.join(AMP_KEY_FILE))
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub(crate) fn read_amp_hook_events(config_dir: &Path) -> Vec<Value> {
    let Ok(raw) = std::fs::read_to_string(config_dir.join(AMP_EVENT_LOG_FILE)) else {
        return Vec::new();
    };
    raw.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect()
}

pub(crate) fn amp_config_connected(config_dir: &Path) -> Result<bool> {
    let state_path = config_dir.join(AMP_STATE_FILE);
    let plugin_path = amp_plugin_path(config_dir);
    if !state_path.exists() || !plugin_path.exists() || !config_dir.join(AMP_KEY_FILE).exists() {
        return Ok(false);
    }
    let source = std::fs::read_to_string(plugin_path)?;
    Ok(source.contains("Generated by Alex for Amp")
        || source.contains("Generated by Alexandria for Amp"))
}

pub(crate) fn set_amp_tool_capture(config_dir: &Path, base_url: &str, enabled: bool) -> Result<()> {
    if !amp_config_connected(config_dir)? {
        bail!("Amp is not connected to Alex; connect it before enabling tool capture");
    }
    let key_path = config_dir.join(AMP_KEY_FILE);
    let event_log_path = config_dir.join(AMP_EVENT_LOG_FILE);
    let hook_base_url = base_url.replace("://0.0.0.0", "://127.0.0.1");
    let source = amp_plugin_source(
        &hook_base_url,
        &format!("{}/harness-events", hook_base_url.trim_end_matches('/')),
        &format!("{}/tool-events", hook_base_url.trim_end_matches('/')),
        &key_path,
        &event_log_path,
        enabled,
    )?;
    atomic_write_text(&amp_plugin_path(config_dir), &source)
}

pub(crate) fn disconnect_amp_config(config_dir: &Path) -> Result<bool> {
    let state_path = config_dir.join(AMP_STATE_FILE);
    if !state_path.exists() {
        return Ok(false);
    }
    let state = read_amp_state(&state_path)?;
    restore_managed_text_file(
        &amp_plugin_path(config_dir),
        state.previous_plugin.as_deref(),
    )?;
    for path in [config_dir.join(AMP_KEY_FILE), state_path] {
        if path.exists() {
            std::fs::remove_file(&path)
                .with_context(|| format!("could not remove {}", path.display()))?;
        }
    }
    Ok(true)
}

fn amp_plugin_path(config_dir: &Path) -> PathBuf {
    config_dir.join("plugins").join(AMP_PLUGIN_FILE)
}

fn read_amp_state(path: &Path) -> Result<AmpManagedState> {
    let raw = std::fs::read_to_string(path)?;
    serde_json::from_str(&raw).with_context(|| {
        format!(
            "could not parse Alex Amp state {}; aborting without changes",
            path.display()
        )
    })
}

fn amp_plugin_source(
    base_url: &str,
    event_url: &str,
    tool_event_url: &str,
    key_path: &Path,
    event_log_path: &Path,
    capture_enabled: bool,
) -> Result<String> {
    let mut source = AMP_PLUGIN_SOURCE.to_string();
    for (token, value) in [
        ("__BASE_URL__", base_url.to_string()),
        ("__EVENT_URL__", event_url.to_string()),
        ("__TOOL_EVENT_URL__", tool_event_url.to_string()),
        ("__KEY_FILE__", key_path.to_string_lossy().to_string()),
        (
            "__EVENT_LOG__",
            event_log_path.to_string_lossy().to_string(),
        ),
    ] {
        source = source.replace(token, &serde_json::to_string(&value)?);
    }
    source = source.replace(
        "__CAPTURE_ENABLED__",
        if capture_enabled { "true" } else { "false" },
    );
    source = source.replace(
        "__TOOL_CALL_CAPTURE__",
        if capture_enabled {
            "postToolEvent({ phase: 'start', session_id: event.thread.id, turn_id: event.toolUseID, tool_call_id: event.toolUseID, tool_name: event.tool, args: event.input, timestamp_ms: Date.now() })"
        } else {
            ""
        },
    );
    source = source.replace(
        "__TOOL_RESULT_CAPTURE__",
        if capture_enabled {
            // Amp reports success as status 'done'; only explicit failure words
            // may set is_error or every successful call renders as failed.
            "const resultEvent = event as unknown as Record<string, unknown>\n    const resultBody = resultEvent.output ?? resultEvent.result ?? event\n    const exitCode = typeof resultBody === 'object' && resultBody !== null && typeof (resultBody as Record<string, unknown>).exitCode === 'number' ? (resultBody as Record<string, unknown>).exitCode as number : undefined\n    const failedStatuses = new Set(['error', 'failed', 'failure', 'cancelled', 'canceled', 'rejected', 'timeout', 'timed_out'])\n    postToolEvent({ phase: 'end', session_id: event.thread.id, turn_id: event.toolUseID, tool_call_id: event.toolUseID, tool_name: event.tool, result: resultBody, is_error: failedStatuses.has(String(event.status ?? '').toLowerCase()) || (exitCode !== undefined && exitCode !== 0), exit_status: exitCode, timestamp_ms: Date.now() })"
        } else {
            ""
        },
    );
    Ok(source)
}

const AMP_PLUGIN_SOURCE: &str = r#"// Generated by Alex for Amp. Reconnect Amp to refresh.
import type { PluginAPI } from '@ampcode/plugin'
import { appendFile, chmod } from 'node:fs/promises'

const BASE_URL = __BASE_URL__
const EVENT_URL = __EVENT_URL__
const TOOL_EVENT_URL = __TOOL_EVENT_URL__
const CAPTURE_ENABLED = __CAPTURE_ENABLED__
const KEY_FILE = __KEY_FILE__
const EVENT_LOG = __EVENT_LOG__
const SUBAGENT_TOOLS = new Set(['task', 'finder', 'librarian', 'oracle', 'painter'])

type PendingSubagent = {
  parent: string
  tool: string
  children: Set<string>
}

export default function (amp: PluginAPI) {
  let queue: Promise<void> = Promise.resolve()
  const pending = new Map<string, PendingSubagent>()

  function enqueue(record: Record<string, unknown>, deliver = false): Promise<void> {
    queue = queue.then(async () => {
      const line = JSON.stringify({ ts: new Date().toISOString(), ...record }) + '\n'
      await appendFile(EVENT_LOG, line, { encoding: 'utf8', mode: 0o600 })
      await chmod(EVENT_LOG, 0o600)
      if (!deliver) return
      const key = (await Bun.file(KEY_FILE).text()).trim()
      if (!key) return
      const controller = new AbortController()
      const timeout = setTimeout(() => controller.abort(), 1500)
      try {
        const response = await fetch(EVENT_URL, {
          method: 'POST',
          headers: {
            authorization: `Bearer ${key}`,
            'content-type': 'application/json',
            'x-alexandria-harness': 'amp',
          },
          body: JSON.stringify(record),
          signal: controller.signal,
        })
        if (!response.ok) amp.logger.log(`Alex lifecycle delivery returned ${response.status}`)
      } finally {
        clearTimeout(timeout)
      }
    }).catch((error) => {
      amp.logger.log('Alex lifecycle capture failed', String(error))
    })
    return queue
  }

  function postToolEvent(record: Record<string, unknown>) {
    if (!CAPTURE_ENABLED) return
    // Tool telemetry is deliberately independent from the local lineage queue.
    void (async () => {
      const key = (await Bun.file(KEY_FILE).text()).trim()
      if (!key) return
      const controller = new AbortController()
      const timeout = setTimeout(() => controller.abort(), 1500)
      try {
        await fetch(TOOL_EVENT_URL, {
          method: 'POST',
          headers: { authorization: `Bearer ${key}`, 'content-type': 'application/json', 'x-alexandria-harness': 'amp' },
          body: JSON.stringify(record),
          signal: controller.signal,
        })
      } catch {
        // Telemetry must never influence Amp tool execution.
      } finally {
        clearTimeout(timeout)
      }
    })()
  }

  function threadIDs(value: unknown, found = new Set<string>(), depth = 0, field = ''): Set<string> {
    if (depth > 8 || value == null) return found
    if (typeof value === 'string') {
      const structuredID = /(^id$|thread_?id$|child.*id$|agent_?id$|session_?id$)/i.test(field)
      const threadLink = /(?:^|\s)https?:\/\/\S*\/threads?\/T-[A-Za-z0-9][A-Za-z0-9_-]*/i.test(value)
      if (structuredID || threadLink) {
        for (const match of value.matchAll(/\bT-[A-Za-z0-9][A-Za-z0-9_-]*\b/g)) found.add(match[0])
      }
      return found
    }
    if (Array.isArray(value)) {
      for (const item of value) threadIDs(item, found, depth + 1, field)
      return found
    }
    if (typeof value === 'object') {
      for (const [key, item] of Object.entries(value as Record<string, unknown>)) {
        threadIDs(item, found, depth + 1, key)
      }
    }
    return found
  }

  amp.on('session.start', (event) => {
    void enqueue({
      event: 'session.start',
      hook_event_name: 'SessionStart',
      session_id: event.thread.id,
      thread_id: event.thread.id,
    }, true)
  })

  amp.on('agent.start', (event) => {
    void enqueue({
      event: 'agent.start',
      session_id: event.thread.id,
      thread_id: event.thread.id,
      turn_id: String(event.id),
    })
    return {}
  })

  amp.on('tool.call', (event) => {
    const tool = event.tool.toLowerCase()
    const row: PendingSubagent = {
      parent: event.thread.id,
      tool: event.tool,
      children: new Set(),
    }
    if (SUBAGENT_TOOLS.has(tool)) {
      for (const child of threadIDs(event.input)) {
        if (child === event.thread.id) continue
        row.children.add(child)
        void enqueue({
          event: 'subagent.start',
          hook_event_name: 'SubagentStart',
          session_id: event.thread.id,
          agent_id: child,
          turn_id: event.toolUseID,
          agent_type: event.tool,
        }, true)
      }
      pending.set(event.toolUseID, row)
    }
    void enqueue({
      event: 'tool.call',
      session_id: event.thread.id,
      thread_id: event.thread.id,
      turn_id: event.toolUseID,
      tool: event.tool,
    })
    __TOOL_CALL_CAPTURE__
    return { action: 'allow' }
  })

  amp.on('tool.result', (event) => {
    const row = pending.get(event.toolUseID)
    if (row) {
      for (const child of threadIDs(event)) {
        if (child === row.parent) continue
        if (!row.children.has(child)) {
          row.children.add(child)
          void enqueue({
            event: 'subagent.start',
            hook_event_name: 'SubagentStart',
            session_id: row.parent,
            agent_id: child,
            turn_id: event.toolUseID,
            agent_type: row.tool,
          }, true)
        }
      }
      for (const child of row.children) {
        void enqueue({
          event: 'subagent.stop',
          hook_event_name: 'SubagentStop',
          session_id: row.parent,
          agent_id: child,
          turn_id: event.toolUseID,
          agent_type: row.tool,
          status: event.status,
        }, true)
      }
      pending.delete(event.toolUseID)
    }
    void enqueue({
      event: 'tool.result',
      session_id: event.thread.id,
      thread_id: event.thread.id,
      turn_id: event.toolUseID,
      tool: event.tool,
      status: event.status,
    })
    __TOOL_RESULT_CAPTURE__
  })

  amp.on('agent.end', (event) => {
    void enqueue({
      event: 'agent.end',
      hook_event_name: 'Stop',
      session_id: event.thread.id,
      thread_id: event.thread.id,
      turn_id: String(event.id),
      status: event.status,
    }, true)
  })

  const statusCommand = async (ctx) => {
    const controller = new AbortController()
    const timeout = setTimeout(() => controller.abort(), 1000)
    try {
      const response = await fetch(`${BASE_URL}/health`, { signal: controller.signal })
      const thread = ctx.thread?.id ? ` Thread ${ctx.thread.id}.` : ''
      await ctx.ui.notify(response.ok
        ? `Alex lifecycle reporting is connected.${thread} Use alex wrap amp for traffic capture.`
        : `Alex returned HTTP ${response.status}.`)
    } catch {
      await ctx.ui.notify('Alex is not reachable. Start or restart the local daemon.')
    } finally {
      clearTimeout(timeout)
    }
  }
  const statusCommandOptions = {
    title: 'Status',
    category: 'Alex',
    description: 'Check lifecycle reporting and show the current Amp thread ID',
  }
  amp.registerCommand('alex-status', statusCommandOptions, statusCommand)
  // Keep the old command as a compatibility alias for existing Amp installs.
  amp.registerCommand('alexandria-status', {
    ...statusCommandOptions,
    title: 'Status (legacy alias)',
  }, statusCommand)
}
"#;

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn write_codex_connection(
    config_dir: PathBuf,
    base_url: String,
    key_id: String,
    api_key: String,
    catalog: Value,
    version: Option<String>,
) -> Result<HarnessConnectSummary> {
    write_codex_connection_with_capture(
        config_dir, base_url, key_id, api_key, catalog, version, false,
    )
}

pub(crate) fn write_codex_connection_with_capture(
    config_dir: PathBuf,
    base_url: String,
    key_id: String,
    api_key: String,
    catalog: Value,
    version: Option<String>,
    capture_enabled: bool,
) -> Result<HarnessConnectSummary> {
    let config_path = config_dir.join(CODEX_CONFIG_FILE);
    let catalog_path = config_dir.join(CODEX_CATALOG_FILE);
    let native_catalog_path = config_dir.join(CODEX_NATIVE_CATALOG_FILE);
    let key_path = config_dir.join(CODEX_KEY_FILE);
    let state_path = config_dir.join(CODEX_STATE_FILE);
    let backup_path = config_dir.join(CODEX_BACKUP_FILE);
    let openai_profile_path = config_dir.join(CODEX_OPENAI_PROFILE_FILE);
    let alex_profile_path = config_dir.join(CODEX_ALEX_PROFILE_FILE);
    let hook_path = config_dir.join(CODEX_HOOK_FILE);
    let hook_curl_path = config_dir.join(CODEX_HOOK_CURL_FILE);
    let tool_hook_path = config_dir.join(CODEX_TOOL_HOOK_FILE);
    let tool_hook_curl_path = config_dir.join(CODEX_TOOL_HOOK_CURL_FILE);
    let hooks_path = config_dir.join("hooks.json");
    let models = codex_catalog_model_ids(&catalog)?;
    let native_catalog = codex_native_catalog(&catalog)?;
    let native_models = codex_catalog_model_ids(&native_catalog)?;
    let before = read_codex_model_ids(&config_dir);

    let mut config_doc = read_codex_config(&config_path)?;
    let managed_before = state_path.exists();
    if !managed_before
        && config_doc
            .get("model_providers")
            .and_then(Item::as_table_like)
            .and_then(|providers| providers.get(PROVIDER_NAME))
            .is_some_and(Item::is_table_like)
    {
        bail!(
            "{} already defines model_providers.{PROVIDER_NAME}; Alex will not replace an unmanaged provider",
            config_path.display()
        );
    }
    let mut state = if managed_before {
        read_codex_state(&state_path)?
    } else {
        CodexManagedState {
            previous_model: config_doc
                .get("model")
                .and_then(Item::as_str)
                .map(String::from),
            previous_model_provider: config_doc
                .get("model_provider")
                .and_then(Item::as_str)
                .map(String::from),
            previous_model_catalog_json: config_doc
                .get("model_catalog_json")
                .and_then(Item::as_str)
                .map(String::from),
            previous_hooks_enabled: codex_hooks_enabled(&config_doc),
            manages_model: true,
            profiles_backed_up: true,
            previous_openai_profile: read_optional_text(&openai_profile_path)?,
            previous_alex_profile: read_optional_text(&alex_profile_path)?,
            native_model: None,
            alex_model: None,
        }
    };
    // Migrate connections created before Alexandria managed the namespaced
    // default model. At that point the current model was still the user's
    // pre-connect value, so it is safe to capture for disconnect restoration.
    if !state.manages_model {
        state.previous_model = config_doc
            .get("model")
            .and_then(Item::as_str)
            .map(String::from);
        state.manages_model = true;
    }
    if !state.profiles_backed_up {
        state.previous_openai_profile = read_optional_text(&openai_profile_path)?;
        state.previous_alex_profile = read_optional_text(&alex_profile_path)?;
        state.profiles_backed_up = true;
    }
    // A fresh connection cycle must replace any backup retained from an older
    // disconnected install. Refreshes of the active connection never replace
    // the backup, so it remains the exact pre-connect configuration.
    if !managed_before || !backup_path.exists() {
        let source = if managed_before {
            codex_original_config_source(&config_doc, &state)?
        } else {
            std::fs::read_to_string(&config_path).unwrap_or_default()
        };
        atomic_write_text(&backup_path, &source)?;
    }

    let selected_model = managed_codex_model(&config_doc, &models);
    let native_model = state
        .native_model
        .as_ref()
        .filter(|model| native_models.contains(model))
        .cloned()
        .or_else(|| {
            state
                .previous_model
                .as_ref()
                .filter(|model| native_models.contains(model))
                .cloned()
        })
        .or_else(|| {
            config_doc
                .get("model")
                .and_then(Item::as_str)
                .filter(|model| native_models.iter().any(|native| native == model))
                .map(String::from)
        })
        .unwrap_or_else(|| native_models[0].clone());
    let default_route = if managed_before
        && config_doc.get("model_provider").and_then(Item::as_str) == Some("openai")
    {
        "openai"
    } else {
        "alex"
    };
    state.native_model = Some(native_model.clone());
    state.alex_model = Some(selected_model.clone());
    let mut hooks = read_hooks_json(&hooks_path)?;
    let hook_command = codex_hook_command(&hook_path);
    let tool_command = codex_hook_command(&tool_hook_path);
    upsert_codex_hooks(
        &mut hooks,
        &hook_command,
        &hook_path,
        capture_enabled.then_some((tool_command.as_str(), tool_hook_path.as_path())),
        &tool_hook_path,
    )?;
    upsert_codex_config(
        &mut config_doc,
        &base_url,
        &catalog_path,
        &key_path,
        &selected_model,
        version.as_deref(),
    )?;
    if default_route == "openai" {
        apply_codex_route(
            &mut config_doc,
            "openai",
            &native_model,
            &native_catalog_path,
        );
    }

    atomic_write_json(&state_path, &serde_json::to_value(&state)?)?;
    atomic_write_json(&catalog_path, &catalog)?;
    atomic_write_json(&native_catalog_path, &native_catalog)?;
    atomic_write_text(
        &openai_profile_path,
        &codex_profile_source("openai", &native_model, &native_catalog_path),
    )?;
    atomic_write_text(
        &alex_profile_path,
        &codex_profile_source(PROVIDER_NAME, &selected_model, &catalog_path),
    )?;
    atomic_write_text(&key_path, &format!("{api_key}\n"))?;
    let hook_base_url = base_url.replace("://0.0.0.0", "://127.0.0.1");
    let hook_url = format!("{}/harness-events", hook_base_url.trim_end_matches('/'));
    install_harness_hook(
        &hook_path,
        &config_dir.join(CODEX_EVENT_LOG_FILE),
        &hook_curl_path,
        &hook_url,
        &key_path,
        &api_key,
        "Codex",
    )?;
    if capture_enabled {
        install_harness_hook(
            &tool_hook_path,
            &config_dir.join(CODEX_TOOL_EVENT_LOG_FILE),
            &tool_hook_curl_path,
            &format!("{}/tool-events", hook_base_url.trim_end_matches('/')),
            &key_path,
            &api_key,
            "Codex",
        )?;
    }
    atomic_write_json(&hooks_path, &hooks)?;
    atomic_write_text(&config_path, &config_doc.to_string())?;

    let (added, removed, unchanged) = model_id_diff(&before, &models);
    Ok(HarnessConnectSummary {
        key_id,
        models,
        config_path,
        extension_path: hook_path,
        version,
        base_url,
        added,
        removed,
        unchanged,
        description: CODEX_INSTALL_DESCRIPTION,
    })
}

pub(crate) fn read_codex_api_key(config_dir: &Path) -> Option<String> {
    std::fs::read_to_string(config_dir.join(CODEX_KEY_FILE))
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub(crate) fn read_codex_model_ids(config_dir: &Path) -> Vec<String> {
    let path = config_dir.join(CODEX_CATALOG_FILE);
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(catalog) = serde_json::from_str::<Value>(&raw) else {
        return Vec::new();
    };
    codex_catalog_model_ids(&catalog).unwrap_or_default()
}

pub(crate) fn read_codex_hook_events(config_dir: &Path) -> Vec<Value> {
    let Ok(raw) = std::fs::read_to_string(config_dir.join(CODEX_EVENT_LOG_FILE)) else {
        return Vec::new();
    };
    raw.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect()
}

fn codex_catalog_model_ids(catalog: &Value) -> Result<Vec<String>> {
    let rows = catalog["models"]
        .as_array()
        .context("Codex model catalog must contain a models array")?;
    let mut seen = HashSet::new();
    let models = rows
        .iter()
        .filter_map(|row| row["slug"].as_str())
        .filter(|slug| seen.insert((*slug).to_string()))
        .map(String::from)
        .collect::<Vec<_>>();
    if models.is_empty() {
        bail!("Codex model catalog did not contain any model slugs");
    }
    Ok(models)
}

fn codex_native_catalog(catalog: &Value) -> Result<Value> {
    let mut native = catalog.clone();
    let rows = native["models"]
        .as_array_mut()
        .context("Codex model catalog must contain a models array")?;
    rows.retain(|row| {
        row["slug"]
            .as_str()
            .is_some_and(|slug| !slug.starts_with("alex/"))
    });
    if rows.is_empty() {
        bail!("Codex bundled model catalog did not contain native models");
    }
    Ok(native)
}

fn read_optional_text(path: &Path) -> Result<Option<String>> {
    if path.exists() {
        Ok(Some(std::fs::read_to_string(path)?))
    } else {
        Ok(None)
    }
}

fn codex_profile_source(provider: &str, model: &str, catalog_path: &Path) -> String {
    let mut doc = DocumentMut::new();
    doc["model"] = value(model);
    doc["model_provider"] = value(provider);
    doc["model_catalog_json"] = value(catalog_path.to_string_lossy().as_ref());
    let description = if provider == "openai" {
        "# Generated by Alex. This profile bypasses the Alex proxy.\n# Run: codex --profile openai\n"
    } else {
        "# Generated by Alex. This profile routes alex/* models through the local proxy.\n# Run: codex --profile alex\n"
    };
    format!("{description}{doc}")
}

fn apply_codex_route(doc: &mut DocumentMut, provider: &str, model: &str, catalog_path: &Path) {
    doc["model"] = value(model);
    doc["model_provider"] = value(provider);
    doc["model_catalog_json"] = value(catalog_path.to_string_lossy().as_ref());
}

fn codex_original_config_source(
    current: &DocumentMut,
    state: &CodexManagedState,
) -> Result<String> {
    let mut original = DocumentMut::from_str(&current.to_string())?;
    if let Some(providers) = original
        .get_mut("model_providers")
        .and_then(Item::as_table_mut)
    {
        providers.remove(PROVIDER_NAME);
    }
    restore_string_key(
        &mut original,
        "model_provider",
        state.previous_model_provider.as_deref(),
    );
    restore_string_key(&mut original, "model", state.previous_model.as_deref());
    restore_string_key(
        &mut original,
        "model_catalog_json",
        state.previous_model_catalog_json.as_deref(),
    );
    if state.previous_hooks_enabled == Some(false) {
        original["features"]["hooks"] = value(false);
    }
    Ok(original.to_string())
}

pub(crate) fn codex_default_route(config_dir: &Path) -> Result<Option<String>> {
    if !config_dir.join(CODEX_STATE_FILE).exists() {
        return Ok(None);
    }
    let doc = read_codex_config(&config_dir.join(CODEX_CONFIG_FILE))?;
    Ok(Some(
        if doc.get("model_provider").and_then(Item::as_str) == Some(PROVIDER_NAME) {
            "alex"
        } else {
            "openai"
        }
        .to_string(),
    ))
}

pub(crate) fn set_codex_default_route(config_dir: &Path, route: &str) -> Result<String> {
    if !matches!(route, "openai" | "alex") {
        bail!("Codex default route must be 'openai' or 'alex'");
    }
    let state_path = config_dir.join(CODEX_STATE_FILE);
    if !state_path.exists() {
        bail!("Codex is not connected to Alex");
    }
    let state = read_codex_state(&state_path)?;
    let (provider, model, catalog_path) = if route == "alex" {
        (
            PROVIDER_NAME,
            state.alex_model.as_deref().context(
                "Alex profile is missing its selected model; update the Codex config",
            )?,
            config_dir.join(CODEX_CATALOG_FILE),
        )
    } else {
        (
            "openai",
            state
                .native_model
                .as_deref()
                .context("OpenAI profile is missing its selected model; update the Codex config")?,
            config_dir.join(CODEX_NATIVE_CATALOG_FILE),
        )
    };
    let config_path = config_dir.join(CODEX_CONFIG_FILE);
    let mut doc = read_codex_config(&config_path)?;
    apply_codex_route(&mut doc, provider, model, &catalog_path);
    atomic_write_text(&config_path, &doc.to_string())?;
    Ok(route.to_string())
}

fn set_codex_default_model(config_dir: &Path, model: &str) -> Result<()> {
    if !codex_config_connected(config_dir)? {
        bail!("Codex is not connected to Alex");
    }
    let catalog = read_json_object(&config_dir.join(CODEX_CATALOG_FILE))?;
    if !codex_catalog_model_ids(&catalog)?
        .iter()
        .any(|candidate| candidate == model)
    {
        bail!("{model} is not in Codex's Alex model catalog");
    }
    let state_path = config_dir.join(CODEX_STATE_FILE);
    let mut state = read_codex_state(&state_path)?;
    state.alex_model = Some(model.to_string());
    atomic_write_json(&state_path, &serde_json::to_value(&state)?)?;
    let profile_path = config_dir.join(CODEX_ALEX_PROFILE_FILE);
    atomic_write_text(
        &profile_path,
        &codex_profile_source(PROVIDER_NAME, model, &config_dir.join(CODEX_CATALOG_FILE)),
    )?;
    let config_path = config_dir.join(CODEX_CONFIG_FILE);
    let mut doc = read_codex_config(&config_path)?;
    if doc.get("model_provider").and_then(Item::as_str) == Some(PROVIDER_NAME) {
        apply_codex_route(
            &mut doc,
            PROVIDER_NAME,
            model,
            &config_dir.join(CODEX_CATALOG_FILE),
        );
        atomic_write_text(&config_path, &doc.to_string())?;
    }
    Ok(())
}

pub(crate) fn codex_config_connected(config_dir: &Path) -> Result<bool> {
    let path = config_dir.join(CODEX_CONFIG_FILE);
    if !path.exists() {
        return Ok(false);
    }
    let doc = read_codex_config(&path)?;
    Ok(config_dir.join(CODEX_STATE_FILE).exists()
        && doc
            .get("model_providers")
            .and_then(Item::as_table_like)
            .and_then(|providers| providers.get(PROVIDER_NAME))
            .is_some_and(Item::is_table_like))
}

pub(crate) fn disconnect_codex_config(config_dir: &Path) -> Result<bool> {
    let config_path = config_dir.join(CODEX_CONFIG_FILE);
    let state_path = config_dir.join(CODEX_STATE_FILE);
    if !state_path.exists() {
        return Ok(false);
    }
    let state = read_codex_state(&state_path)?;
    let mut doc = read_codex_config(&config_path)?;
    let catalog_path = config_dir.join(CODEX_CATALOG_FILE);
    let mut changed = false;

    if let Some(providers) = doc.get_mut("model_providers").and_then(Item::as_table_mut) {
        changed |= providers.remove(PROVIDER_NAME).is_some();
    }
    if state.manages_model {
        restore_string_key(
            &mut doc,
            "model_provider",
            state.previous_model_provider.as_deref(),
        );
        restore_string_key(&mut doc, "model", state.previous_model.as_deref());
        restore_string_key(
            &mut doc,
            "model_catalog_json",
            state.previous_model_catalog_json.as_deref(),
        );
        changed = true;
    }
    if codex_hooks_enabled(&doc) == Some(true) && state.previous_hooks_enabled == Some(false) {
        doc["features"]["hooks"] = value(false);
        changed = true;
    }
    if changed {
        atomic_write_text(&config_path, &doc.to_string())?;
    }

    let hooks_path = config_dir.join("hooks.json");
    if hooks_path.exists() {
        let mut hooks = read_hooks_json(&hooks_path)?;
        if remove_codex_hooks(
            &mut hooks,
            &codex_hook_command(&config_dir.join(CODEX_HOOK_FILE)),
            &config_dir.join(CODEX_HOOK_FILE),
            Some(&config_dir.join(CODEX_TOOL_HOOK_FILE)),
        )? {
            atomic_write_json(&hooks_path, &hooks)?;
            changed = true;
        }
    }

    restore_managed_text_file(
        &config_dir.join(CODEX_OPENAI_PROFILE_FILE),
        state.previous_openai_profile.as_deref(),
    )?;
    restore_managed_text_file(
        &config_dir.join(CODEX_ALEX_PROFILE_FILE),
        state.previous_alex_profile.as_deref(),
    )?;

    for path in [
        config_dir.join(CODEX_KEY_FILE),
        catalog_path,
        config_dir.join(CODEX_NATIVE_CATALOG_FILE),
        config_dir.join(CODEX_HOOK_FILE),
        config_dir.join(CODEX_HOOK_CURL_FILE),
        config_dir.join(CODEX_TOOL_HOOK_FILE),
        config_dir.join(CODEX_TOOL_HOOK_CURL_FILE),
        state_path,
    ] {
        if path.exists() {
            std::fs::remove_file(&path)
                .with_context(|| format!("could not remove {}", path.display()))?;
            changed = true;
        }
    }
    Ok(changed)
}

fn restore_managed_text_file(path: &Path, previous: Option<&str>) -> Result<()> {
    if let Some(previous) = previous {
        atomic_write_text(path, previous)
    } else if path.exists() {
        std::fs::remove_file(path).with_context(|| format!("could not remove {}", path.display()))
    } else {
        Ok(())
    }
}

fn read_codex_config(path: &Path) -> Result<DocumentMut> {
    let raw = if path.exists() {
        std::fs::read_to_string(path)?
    } else {
        String::new()
    };
    DocumentMut::from_str(&raw).with_context(|| {
        format!(
            "could not parse {}; aborting without changes",
            path.display()
        )
    })
}

fn read_codex_state(path: &Path) -> Result<CodexManagedState> {
    let raw = std::fs::read_to_string(path)?;
    serde_json::from_str(&raw).with_context(|| {
        format!(
            "could not parse Alex Codex state {}; aborting without changes",
            path.display()
        )
    })
}

fn restore_string_key(doc: &mut DocumentMut, key: &str, previous: Option<&str>) {
    if let Some(previous) = previous {
        doc[key] = value(previous);
    } else {
        doc.as_table_mut().remove(key);
    }
}

fn codex_hooks_enabled(doc: &DocumentMut) -> Option<bool> {
    doc.get("features")
        .and_then(Item::as_table_like)
        .and_then(|features| features.get("hooks"))
        .and_then(Item::as_bool)
}

fn upsert_codex_config(
    doc: &mut DocumentMut,
    base_url: &str,
    catalog_path: &Path,
    key_path: &Path,
    selected_model: &str,
    version: Option<&str>,
) -> Result<()> {
    doc["model"] = value(selected_model);
    doc["model_provider"] = value(PROVIDER_NAME);
    doc["model_catalog_json"] = value(catalog_path.to_string_lossy().as_ref());
    // Codex treats an omitted setting as disabled. Alexandria's lifecycle
    // hooks must therefore opt in both absent and explicitly-false configs,
    // while preserving an explicit user true.
    if codex_hooks_enabled(doc) != Some(true) {
        doc["features"]["hooks"] = value(true);
    }

    if doc.get("model_providers").is_none() {
        doc["model_providers"] = Item::Table(Table::new());
    }
    let providers = doc["model_providers"]
        .as_table_mut()
        .with_context(|| "config.toml model_providers must be a table; aborting without changes")?;
    let mut provider = Table::new();
    provider["name"] = value("Alex Proxy");
    provider["base_url"] = value(format!("{}/v1", base_url.trim_end_matches('/')));
    provider["wire_api"] = value("responses");
    provider["supports_websockets"] = value(false);
    let mut headers = InlineTable::new();
    headers.insert("x-alexandria-harness", "codex".into());
    headers.insert(
        "x-alexandria-harness-version",
        version.unwrap_or("unknown").into(),
    );
    provider["http_headers"] = value(headers);

    let mut auth = Table::new();
    #[cfg(not(windows))]
    {
        auth["command"] = value("/bin/cat");
        let mut args = Array::new();
        args.push(key_path.to_string_lossy().as_ref());
        auth["args"] = value(args);
    }
    #[cfg(windows)]
    {
        auth["command"] = value("powershell");
        let mut args = Array::new();
        args.push("-NoProfile");
        args.push("-Command");
        args.push(format!(
            "Get-Content -Raw '{}'",
            key_path.to_string_lossy().replace('\'', "''")
        ));
        auth["args"] = value(args);
    }
    auth["timeout_ms"] = value(5000);
    auth["refresh_interval_ms"] = value(0);
    provider["auth"] = Item::Table(auth);
    providers[PROVIDER_NAME] = Item::Table(provider);
    Ok(())
}

fn read_hooks_json(path: &Path) -> Result<Value> {
    if !path.exists() {
        return Ok(json!({}));
    }
    let raw = std::fs::read_to_string(path)?;
    let value: Value = serde_json::from_str(&raw).with_context(|| {
        format!(
            "could not parse {}; aborting without changes",
            path.display()
        )
    })?;
    if !value.is_object() {
        bail!("{} must contain a JSON object", path.display());
    }
    Ok(value)
}

fn upsert_codex_hooks(
    value: &mut Value,
    command: &str,
    hook_path: &Path,
    tool_hook: Option<(&str, &Path)>,
    tool_hook_path: &Path,
) -> Result<()> {
    remove_codex_hooks(value, command, hook_path, Some(tool_hook_path))?;
    if value.get("hooks").is_none() {
        value["hooks"] = json!({});
    }
    let hooks = value["hooks"]
        .as_object_mut()
        .context("hooks.json .hooks must be an object")?;
    for event in ["SessionStart", "SubagentStart", "SubagentStop"] {
        let groups = hooks.entry(event).or_insert_with(|| json!([]));
        let groups = groups
            .as_array_mut()
            .with_context(|| format!("hooks.json hooks.{event} must be an array"))?;
        groups.push(json!({
            "hooks": [{
                "type": "command",
                "command": command,
                "timeout": 5,
                "statusMessage": "Recording Alex session lineage"
            }]
        }));
    }
    if let Some((tool_command, _)) = tool_hook {
        // Codex 0.144 has no PostToolUseFailure event; unknown names are
        // silently dropped from hooks.json, so register only what fires.
        for event in ["PreToolUse", "PostToolUse"] {
            let groups = hooks.entry(event).or_insert_with(|| json!([]));
            let groups = groups
                .as_array_mut()
                .with_context(|| format!("hooks.json hooks.{event} must be an array"))?;
            groups.push(json!({
                "hooks": [{
                    "type": "command",
                    "command": tool_command,
                    "timeout": 5,
                    "statusMessage": "Recording Alex tool execution"
                }]
            }));
        }
    }
    Ok(())
}

fn remove_codex_hooks(
    value: &mut Value,
    command: &str,
    hook_path: &Path,
    tool_hook_path: Option<&Path>,
) -> Result<bool> {
    let Some(hooks) = value.get_mut("hooks") else {
        return Ok(false);
    };
    let hooks = hooks
        .as_object_mut()
        .context("hooks.json .hooks must be an object")?;
    let path_text = hook_path.to_string_lossy();
    let tool_path_text = tool_hook_path.map(|path| path.to_string_lossy().to_string());
    let mut changed = false;
    for groups in hooks.values_mut() {
        let Some(groups) = groups.as_array_mut() else {
            continue;
        };
        for group in groups.iter_mut() {
            let Some(handlers) = group.get_mut("hooks").and_then(Value::as_array_mut) else {
                continue;
            };
            let before = handlers.len();
            handlers.retain(|handler| {
                let configured = handler["command"].as_str().unwrap_or_default();
                configured != command
                    && !configured.contains(path_text.as_ref())
                    && tool_path_text
                        .as_deref()
                        .is_none_or(|path| !configured.contains(path))
            });
            changed |= handlers.len() != before;
        }
        groups.retain(|group| {
            group["hooks"]
                .as_array()
                .is_none_or(|handlers| !handlers.is_empty())
        });
    }
    hooks.retain(|_, groups| groups.as_array().is_none_or(|groups| !groups.is_empty()));
    if hooks.is_empty() {
        value
            .as_object_mut()
            .expect("hooks JSON object")
            .remove("hooks");
    }
    Ok(changed)
}

pub(crate) fn set_codex_tool_capture(
    config_dir: &Path,
    base_url: &str,
    enabled: bool,
) -> Result<()> {
    if !codex_config_connected(config_dir)? {
        bail!("Codex is not connected to Alex; connect it before enabling tool capture");
    }
    let hooks_path = config_dir.join("hooks.json");
    let mut hooks = read_hooks_json(&hooks_path)?;
    let hook_path = config_dir.join(CODEX_HOOK_FILE);
    let tool_hook_path = config_dir.join(CODEX_TOOL_HOOK_FILE);
    let command = codex_hook_command(&hook_path);
    let tool_command = codex_hook_command(&tool_hook_path);
    upsert_codex_hooks(
        &mut hooks,
        &command,
        &hook_path,
        enabled.then_some((tool_command.as_str(), tool_hook_path.as_path())),
        &tool_hook_path,
    )?;
    if enabled {
        let key = read_codex_api_key(config_dir)
            .context("Codex is not connected to Alex; missing harness key")?;
        let hook_base_url = base_url.replace("://0.0.0.0", "://127.0.0.1");
        install_harness_hook(
            &tool_hook_path,
            &config_dir.join(CODEX_TOOL_EVENT_LOG_FILE),
            &config_dir.join(CODEX_TOOL_HOOK_CURL_FILE),
            &format!("{}/tool-events", hook_base_url.trim_end_matches('/')),
            &config_dir.join(CODEX_KEY_FILE),
            &key,
            "Codex",
        )?;
    }
    atomic_write_json(&hooks_path, &hooks)
}

#[cfg(not(windows))]
fn codex_hook_command(path: &Path) -> String {
    shell_single_quote(path.to_string_lossy().as_ref())
}

#[cfg(windows)]
fn codex_hook_command(path: &Path) -> String {
    format!(
        "powershell -NoProfile -ExecutionPolicy Bypass -File \"{}\"",
        path.display()
    )
}

#[cfg(not(windows))]
fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn install_harness_hook(
    path: &Path,
    event_log: &Path,
    curl_config: &Path,
    event_url: &str,
    key_path: &Path,
    api_key: &str,
    harness_name: &str,
) -> Result<()> {
    #[cfg(not(windows))]
    let _ = key_path;
    #[cfg(windows)]
    let _ = (curl_config, api_key);
    #[cfg(not(windows))]
    {
        let curl_source = format!(
            "url = \"{}\"\nheader = \"Authorization: Bearer {}\"\nheader = \"Content-Type: application/json\"\nconnect-timeout = 1\nmax-time = 2\n",
            event_url.replace('\\', "\\\\").replace('"', "\\\""),
            api_key.replace('\\', "\\\\").replace('"', "\\\"")
        );
        atomic_write_text(curl_config, &curl_source)?;
    }
    #[cfg(not(windows))]
    let source = format!(
        "#!/bin/sh\n# Generated by Alex. Reconnect {harness_name} to refresh.\nset -eu\numask 077\nevent_log={}\ncurl_config={}\npayload=$(/bin/cat)\nif [ -n \"$payload\" ]; then\n  /usr/bin/printf '%s\\n' \"$payload\" >> \"$event_log\"\n  if [ -x /usr/bin/curl ]; then\n    /usr/bin/printf '%s' \"$payload\" | /usr/bin/curl --silent --show-error --fail --config \"$curl_config\" --data-binary @- >/dev/null 2>&1 || true\n  fi\nfi\n",
        shell_single_quote(event_log.to_string_lossy().as_ref()),
        shell_single_quote(curl_config.to_string_lossy().as_ref())
    );
    #[cfg(windows)]
    let source = format!(
        "$ErrorActionPreference = 'Stop'\n$payload = [Console]::In.ReadToEnd()\nif ($payload) {{\n  Add-Content -LiteralPath '{}' -Value $payload\n  try {{\n    $key = (Get-Content -Raw -LiteralPath '{}').Trim()\n    Invoke-RestMethod -Uri '{}' -Method Post -Headers @{{ Authorization = \"Bearer $key\" }} -ContentType 'application/json' -Body $payload -TimeoutSec 2 | Out-Null\n  }} catch {{ }}\n}}\n",
        event_log.to_string_lossy().replace('\'', "''"),
        key_path.to_string_lossy().replace('\'', "''"),
        event_url.replace('\'', "''")
    );
    atomic_write_text(path, &source)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn short_alex_model_id(id: &str) -> String {
    if id.starts_with("alex/") {
        id.to_string()
    } else if let Some(rest) = id.strip_prefix("alexandria/") {
        format!("alex/{rest}")
    } else if let Some(rest) = id.strip_prefix("cove/") {
        format!("alex/{rest}")
    } else {
        format!("alex/{id}")
    }
}

/// Prefix bare catalog ids as `alex/<model>` for proxy-backed harness catalogs.
/// Bare ids still route via PASSTHROUGH stripping; existing bare configs keep working.
pub(crate) fn short_alex_model_ids(models: Vec<String>) -> Vec<String> {
    models
        .into_iter()
        .map(|id| short_alex_model_id(&id))
        .collect()
}

fn preferred_claude_model(models: &[String]) -> &String {
    for family in ["sonnet-5", "sonnet-4", "haiku-4", "fable-5", "opus-4"] {
        if let Some(model) = models.iter().find(|model| model.contains(family)) {
            return model;
        }
    }
    models
        .first()
        .expect("Claude gateway model ids are validated as non-empty")
}

fn managed_codex_model(doc: &DocumentMut, models: &[String]) -> String {
    if let Some(current) = doc.get("model").and_then(Item::as_str) {
        let candidate = short_alex_model_id(current);
        if models.contains(&candidate) {
            return candidate;
        }
    }
    models
        .iter()
        .find(|model| model.starts_with("alex/"))
        .or_else(|| models.first())
        .cloned()
        .expect("Codex catalog model ids are validated as non-empty")
}

pub(crate) fn read_pi_api_key(config_dir: &Path) -> Option<String> {
    read_pi_provider(config_dir)?["apiKey"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(String::from)
}

/// Read only the credential Alexandria itself wrote. This is intentionally
/// limited to connected harnesses and is used by `alex up` solely to prove a
/// re-run can skip its connection step; it never prints the secret.
pub(crate) fn configured_api_key(harness: &str, config_dir: &Path) -> Option<String> {
    match harness {
        "pi" => read_pi_api_key(config_dir),
        "codex" => std::fs::read_to_string(config_dir.join(CODEX_KEY_FILE))
            .ok()
            .map(|key| key.trim().to_string())
            .filter(|key| !key.is_empty()),
        _ => None,
    }
}

pub(crate) fn read_pi_model_ids(config_dir: &Path) -> Vec<String> {
    let Some(provider) = read_pi_provider(config_dir) else {
        return Vec::new();
    };
    provider["models"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|row| row["id"].as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

fn read_pi_provider(config_dir: &Path) -> Option<Value> {
    let path = config_dir.join("models.json");
    if !path.exists() {
        return None;
    }
    let raw = std::fs::read_to_string(path).ok()?;
    let value: Value = serde_json::from_str(&raw).ok()?;
    value["providers"][PROVIDER_NAME]
        .as_object()
        .map(|o| Value::Object(o.clone()))
}

pub(crate) fn model_id_diff(
    before: &[String],
    after: &[String],
) -> (Vec<String>, Vec<String>, usize) {
    let before_set: HashSet<&str> = before.iter().map(String::as_str).collect();
    let after_set: HashSet<&str> = after.iter().map(String::as_str).collect();
    let mut added: Vec<String> = after_set
        .difference(&before_set)
        .map(|s| (*s).to_string())
        .collect();
    let mut removed: Vec<String> = before_set
        .difference(&after_set)
        .map(|s| (*s).to_string())
        .collect();
    added.sort();
    removed.sort();
    let unchanged = before_set.intersection(&after_set).count();
    (added, removed, unchanged)
}

pub(crate) fn config_write_json(
    summary: &HarnessConnectSummary,
    key_status: &str,
    refreshed: Option<bool>,
) -> Value {
    let mut body = json!({
        "path": summary.config_path.display().to_string(),
        "models_total": summary.models.len(),
        "added": summary.added,
        "removed": summary.removed,
        "unchanged": summary.unchanged,
        "key": key_status,
        "base_url": summary.base_url,
        "description": summary.description,
    });
    if let Some(refreshed) = refreshed {
        body["refreshed"] = json!(refreshed);
    }
    if !summary.key_id.is_empty() {
        body["key_id"] = json!(summary.key_id);
    }
    body
}

pub(crate) fn disconnect_summary_json(
    models_path: &Path,
    previous_models: Vec<String>,
    base_url: &str,
    revoked: usize,
    was_connected: bool,
) -> Value {
    json!({
        "path": models_path.display().to_string(),
        "models_total": 0,
        "added": [],
        "removed": previous_models,
        "unchanged": 0,
        "key": if revoked > 0 { "revoked" } else { "none" },
        "base_url": base_url,
        "revoked": revoked,
        "was_connected": was_connected,
    })
}

/// Plan step for dry-run connect/disconnect. `keys` is `(id, fingerprint)`.
pub(crate) fn plan_connect(
    config_dir: &Path,
    model_count: usize,
    keys: &[(String, String)],
) -> Value {
    let models_path = config_dir.join("models.json");
    let mut plan = vec![json!({
        "path": "",
        "action": "about",
        "detail": PI_INSTALL_DESCRIPTION,
    })];
    let connected = models_json_connected(&models_path).unwrap_or(false);
    let file_exists = models_path.exists();
    let action = if !file_exists { "create" } else { "modify" };
    let detail = if connected {
        format!("update provider 'alexandria' with {model_count} models")
    } else {
        format!("add provider 'alexandria' with {model_count} models")
    };
    plan.push(json!({
        "path": models_path.display().to_string(),
        "action": action,
        "detail": detail,
    }));
    for (id, fp) in keys {
        plan.push(json!({
            "path": id,
            "action": "delete",
            "detail": format!("revoke harness key {fp}"),
        }));
    }
    plan.push(json!({
        "path": "run-keys",
        "action": "create",
        "detail": "mint harness key",
    }));
    json!({"plan": plan})
}

pub(crate) fn plan_disconnect(config_dir: &Path, keys: &[(String, String)]) -> Value {
    let models_path = config_dir.join("models.json");
    let mut plan = Vec::new();
    if models_json_connected(&models_path).unwrap_or(false) {
        plan.push(json!({
            "path": models_path.display().to_string(),
            "action": "modify",
            "detail": "remove provider block",
        }));
    }
    for (id, fp) in keys {
        plan.push(json!({
            "path": id,
            "action": "delete",
            "detail": format!("revoke harness key {fp}"),
        }));
    }
    json!({"plan": plan})
}

pub(crate) fn plan_claude_connect(
    config_dir: &Path,
    model_count: usize,
    keys: &[(String, String)],
) -> Value {
    let profile_path = config_dir.join(CLAUDE_PROFILE_FILE);
    let mut plan = vec![
        json!({
            "path": "",
            "action": "about",
            "detail": CLAUDE_INSTALL_DESCRIPTION,
        }),
        json!({
            "path": profile_path.display().to_string(),
            "action": if profile_path.exists() { "modify" } else { "create" },
            "detail": format!("write an opt-in Alex gateway profile with {model_count} alex/* models"),
        }),
    ];
    for (path, detail) in [
        (
            config_dir.join(CLAUDE_CATALOG_FILE),
            "write the managed Claude gateway model catalog",
        ),
        (
            config_dir.join(CLAUDE_KEY_FILE),
            "write command-backed harness credential (0600)",
        ),
        (
            config_dir.join(CLAUDE_HOOK_FILE),
            "install SessionStart and sub-agent lineage hook",
        ),
        (
            config_dir.join(CLAUDE_HOOK_CURL_FILE),
            "write authenticated hook delivery config (0600)",
        ),
    ] {
        plan.push(json!({
            "path": path.display().to_string(),
            "action": if path.exists() { "modify" } else { "create" },
            "detail": detail,
        }));
    }
    let backup_path = config_dir.join(CLAUDE_BACKUP_FILE);
    plan.push(json!({
        "path": backup_path.display().to_string(),
        "action": if backup_path.exists() { "preserve" } else { "create" },
        "detail": "keep an exact copy of the normal Claude Code settings for reference",
    }));
    for (id, fp) in keys {
        plan.push(json!({
            "path": id,
            "action": "delete",
            "detail": format!("revoke harness key {fp}"),
        }));
    }
    plan.push(json!({
        "path": "run-keys",
        "action": "create",
        "detail": "mint harness key",
    }));
    json!({"plan": plan})
}

pub(crate) fn plan_claude_disconnect(config_dir: &Path, keys: &[(String, String)]) -> Value {
    let mut plan = Vec::new();
    if config_dir.join(CLAUDE_STATE_FILE).exists() {
        plan.push(json!({
            "path": config_dir.join(CLAUDE_PROFILE_FILE).display().to_string(),
            "action": "restore",
            "detail": "remove the Alex profile or restore the file that previously occupied its path",
        }));
        for (path, detail) in [
            (
                config_dir.join(CLAUDE_CATALOG_FILE),
                "remove managed gateway model catalog",
            ),
            (
                config_dir.join(CLAUDE_KEY_FILE),
                "remove harness credential",
            ),
            (
                config_dir.join(CLAUDE_HOOK_FILE),
                "remove session lineage hook",
            ),
            (
                config_dir.join(CLAUDE_HOOK_CURL_FILE),
                "remove hook delivery credential",
            ),
        ] {
            if path.exists() {
                plan.push(json!({
                    "path": path.display().to_string(),
                    "action": "delete",
                    "detail": detail,
                }));
            }
        }
        plan.push(json!({
            "path": config_dir.join(CLAUDE_BACKUP_FILE).display().to_string(),
            "action": "preserve",
            "detail": "keep the original normal Claude Code settings backup",
        }));
    }
    for (id, fp) in keys {
        plan.push(json!({
            "path": id,
            "action": "delete",
            "detail": format!("revoke harness key {fp}"),
        }));
    }
    json!({"plan": plan})
}

pub(crate) fn plan_grok_connect(
    config_dir: &Path,
    model_count: usize,
    keys: &[(String, String)],
) -> Value {
    let config_path = config_dir.join(GROK_CONFIG_FILE);
    let mut plan = vec![
        json!({
            "path": "",
            "action": "about",
            "detail": GROK_INSTALL_DESCRIPTION,
        }),
        json!({
            "path": config_path.display().to_string(),
            "action": if config_path.exists() { "modify" } else { "create" },
            "detail": format!("add {model_count} alex/* models while preserving Grok's native models and default"),
        }),
    ];
    for (path, detail) in [
        (
            config_dir.join(GROK_KEY_FILE),
            "write the local-only harness credential (0600)",
        ),
        (
            config_dir.join(GROK_HOOK_FILE),
            "install SessionStart and sub-agent lineage hook",
        ),
        (
            config_dir.join(GROK_HOOK_CONFIG_FILE),
            "write authenticated hook delivery config (0600)",
        ),
        (
            config_dir.join("hooks").join(GROK_HOOK_REGISTRATION_FILE),
            "register the trusted global lifecycle hook",
        ),
    ] {
        plan.push(json!({
            "path": path.display().to_string(),
            "action": if path.exists() { "modify" } else { "create" },
            "detail": detail,
        }));
    }
    let backup_path = config_dir.join(GROK_BACKUP_FILE);
    plan.push(json!({
        "path": backup_path.display().to_string(),
        "action": if backup_path.exists() { "preserve" } else { "create" },
        "detail": "keep the exact pre-connect Grok configuration available for recovery",
    }));
    for (id, fp) in keys {
        plan.push(json!({
            "path": id,
            "action": "delete",
            "detail": format!("revoke harness key {fp}"),
        }));
    }
    plan.push(json!({
        "path": "run-keys",
        "action": "create",
        "detail": "mint harness key",
    }));
    json!({"plan": plan})
}

pub(crate) fn plan_kimi_disconnect(config_dir: &Path, keys: &[(String, String)]) -> Value {
    let mut plan = Vec::new();
    if kimi_config_connected(config_dir).unwrap_or(false)
        || config_dir.join(KIMI_STATE_FILE).exists()
    {
        plan.push(json!({
            "path": config_dir.join(KIMI_CONFIG_FILE).display().to_string(),
            "action": "modify",
            "detail": "remove only the Alex provider and alex/* models; Kimi's own providers and models are preserved",
        }));
        plan.push(json!({
            "path": config_dir.join(KIMI_BACKUP_FILE).display().to_string(),
            "action": "preserve",
            "detail": "keep the original Kimi configuration backup",
        }));
    }
    for (id, fp) in keys {
        plan.push(json!({
            "path": id,
            "action": "delete",
            "detail": format!("revoke harness key {fp}"),
        }));
    }
    json!({"plan": plan})
}

pub(crate) fn plan_grok_disconnect(config_dir: &Path, keys: &[(String, String)]) -> Value {
    let mut plan = Vec::new();
    if config_dir.join(GROK_STATE_FILE).exists() {
        plan.push(json!({
            "path": config_dir.join(GROK_CONFIG_FILE).display().to_string(),
            "action": "modify",
            "detail": "remove only Alex-managed alex/* models and preserve native/user models",
        }));
        for (path, detail) in [
            (config_dir.join(GROK_KEY_FILE), "remove harness credential"),
            (
                config_dir.join(GROK_HOOK_FILE),
                "remove session lineage hook",
            ),
            (
                config_dir.join(GROK_HOOK_CONFIG_FILE),
                "remove hook delivery credential",
            ),
        ] {
            if path.exists() {
                plan.push(json!({
                    "path": path.display().to_string(),
                    "action": "delete",
                    "detail": detail,
                }));
            }
        }
        plan.push(json!({
            "path": config_dir.join(GROK_BACKUP_FILE).display().to_string(),
            "action": "preserve",
            "detail": "keep the original Grok configuration backup",
        }));
    }
    for (id, fp) in keys {
        plan.push(json!({
            "path": id,
            "action": "delete",
            "detail": format!("revoke harness key {fp}"),
        }));
    }
    json!({"plan": plan})
}

pub(crate) fn plan_amp_connect(config_dir: &Path, keys: &[(String, String)]) -> Value {
    let plugin_path = amp_plugin_path(config_dir);
    let mut plan = vec![
        json!({
            "path": "",
            "action": "about",
            "detail": AMP_INSTALL_DESCRIPTION,
        }),
        json!({
            "path": plugin_path.display().to_string(),
            "action": if plugin_path.exists() { "modify" } else { "create" },
            "detail": "install the observational system plugin for Amp thread, turn, tool, and sub-agent lifecycle events",
        }),
        json!({
            "path": config_dir.join(AMP_KEY_FILE).display().to_string(),
            "action": if config_dir.join(AMP_KEY_FILE).exists() { "modify" } else { "create" },
            "detail": "write the local-only lifecycle credential (0600)",
        }),
        json!({
            "path": config_dir.join(AMP_STATE_FILE).display().to_string(),
            "action": if config_dir.join(AMP_STATE_FILE).exists() { "preserve" } else { "create" },
            "detail": "remember any pre-existing plugin at Alex's managed path for exact restoration",
        }),
        json!({
            "path": config_dir.join(AMP_EVENT_LOG_FILE).display().to_string(),
            "action": "preserve",
            "detail": "append privacy-minimized lifecycle metadata locally; prompts, tool inputs, and outputs are not logged",
        }),
    ];
    for (id, fp) in keys {
        plan.push(json!({
            "path": id,
            "action": "delete",
            "detail": format!("revoke harness key {fp}"),
        }));
    }
    plan.push(json!({
        "path": "run-keys",
        "action": "create",
        "detail": "mint harness key",
    }));
    json!({"plan": plan})
}

pub(crate) fn plan_amp_disconnect(config_dir: &Path, keys: &[(String, String)]) -> Value {
    let mut plan = Vec::new();
    if config_dir.join(AMP_STATE_FILE).exists() {
        plan.push(json!({
            "path": amp_plugin_path(config_dir).display().to_string(),
            "action": "restore",
            "detail": "remove Alex's plugin or restore the file that previously occupied its managed path",
        }));
        if config_dir.join(AMP_KEY_FILE).exists() {
            plan.push(json!({
                "path": config_dir.join(AMP_KEY_FILE).display().to_string(),
                "action": "delete",
                "detail": "remove lifecycle credential",
            }));
        }
        plan.push(json!({
            "path": config_dir.join(AMP_EVENT_LOG_FILE).display().to_string(),
            "action": "preserve",
            "detail": "keep the local lifecycle event log for trace recovery and debugging",
        }));
    }
    for (id, fp) in keys {
        plan.push(json!({
            "path": id,
            "action": "delete",
            "detail": format!("revoke harness key {fp}"),
        }));
    }
    json!({"plan": plan})
}

pub(crate) fn plan_codex_connect(
    config_dir: &Path,
    model_count: usize,
    keys: &[(String, String)],
) -> Value {
    let config_path = config_dir.join(CODEX_CONFIG_FILE);
    let mut plan = vec![
        json!({
            "path": "",
            "action": "about",
            "detail": CODEX_INSTALL_DESCRIPTION,
        }),
        json!({
            "path": config_path.display().to_string(),
            "action": if config_path.exists() { "modify" } else { "create" },
            "detail": format!("activate provider 'alexandria' with {model_count} native and alex/* models"),
        }),
    ];
    for (path, detail) in [
        (
            config_dir.join(CODEX_CATALOG_FILE),
            "write merged native Codex and Alex model catalog",
        ),
        (
            config_dir.join(CODEX_NATIVE_CATALOG_FILE),
            "write bundled native Codex model catalog for --profile openai",
        ),
        (
            config_dir.join(CODEX_OPENAI_PROFILE_FILE),
            "write fixed profile using normal Codex authentication",
        ),
        (
            config_dir.join(CODEX_ALEX_PROFILE_FILE),
            "write fixed profile routing alex/* through Alex",
        ),
        (
            config_dir.join(CODEX_KEY_FILE),
            "write command-backed harness credential (0600)",
        ),
        (
            config_dir.join(CODEX_HOOK_FILE),
            "install SessionStart and sub-agent lineage hook",
        ),
        (
            config_dir.join(CODEX_HOOK_CURL_FILE),
            "write authenticated hook delivery config (0600)",
        ),
        (
            config_dir.join("hooks.json"),
            "register SessionStart, SubagentStart, and SubagentStop",
        ),
    ] {
        plan.push(json!({
            "path": path.display().to_string(),
            "action": if path.exists() { "modify" } else { "create" },
            "detail": detail,
        }));
    }
    plan.push(json!({
        "path": config_dir.join(CODEX_BACKUP_FILE).display().to_string(),
        "action": if config_dir.join(CODEX_BACKUP_FILE).exists() { "preserve" } else { "create" },
        "detail": "keep the original Codex configuration available for restoration",
    }));
    for (id, fp) in keys {
        plan.push(json!({
            "path": id,
            "action": "delete",
            "detail": format!("revoke harness key {fp}"),
        }));
    }
    plan.push(json!({
        "path": "run-keys",
        "action": "create",
        "detail": "mint harness key",
    }));
    json!({"plan": plan})
}

pub(crate) fn plan_codex_disconnect(config_dir: &Path, keys: &[(String, String)]) -> Value {
    let mut plan = Vec::new();
    if config_dir.join(CODEX_STATE_FILE).exists() {
        plan.push(json!({
            "path": config_dir.join(CODEX_CONFIG_FILE).display().to_string(),
            "action": "modify",
            "detail": "remove Alex provider and restore previous Codex defaults",
        }));
        for (path, detail) in [
            (
                config_dir.join(CODEX_CATALOG_FILE),
                "remove Alex model catalog",
            ),
            (config_dir.join(CODEX_KEY_FILE), "remove harness credential"),
            (
                config_dir.join(CODEX_HOOK_FILE),
                "remove session lineage hook",
            ),
            (
                config_dir.join(CODEX_HOOK_CURL_FILE),
                "remove hook delivery credential",
            ),
            (
                config_dir.join(CODEX_NATIVE_CATALOG_FILE),
                "remove native profile model catalog",
            ),
        ] {
            if path.exists() {
                plan.push(json!({
                    "path": path.display().to_string(),
                    "action": "delete",
                    "detail": detail,
                }));
            }
        }
        plan.push(json!({
            "path": config_dir.join(CODEX_BACKUP_FILE).display().to_string(),
            "action": "restore",
            "detail": "restore original defaults and any pre-existing openai/alex profiles",
        }));
    }
    for (id, fp) in keys {
        plan.push(json!({
            "path": id,
            "action": "delete",
            "detail": format!("revoke harness key {fp}"),
        }));
    }
    json!({"plan": plan})
}

pub(crate) fn disconnect_pi_config(config_dir: &Path) -> Result<bool> {
    let removed_provider = remove_pi_provider(&config_dir.join("models.json"))?;
    let extension_path = pi_session_extension_path(config_dir);
    let removed_extension = if extension_path.exists() {
        std::fs::remove_file(&extension_path).with_context(|| {
            format!(
                "could not remove Pi session extension {}",
                extension_path.display()
            )
        })?;
        true
    } else {
        false
    };
    Ok(removed_provider || removed_extension)
}

fn pi_session_extension_path(config_dir: &Path) -> PathBuf {
    config_dir
        .join("extensions")
        .join(PI_SESSION_EXTENSION_FILE)
}

fn install_pi_session_extension(
    config_dir: &Path,
    base_url: &str,
    api_key: &str,
    capture_enabled: bool,
) -> Result<PathBuf> {
    let path = pi_session_extension_path(config_dir);
    let parent = path.parent().expect("Pi extension path has a parent");
    std::fs::create_dir_all(parent).with_context(|| {
        format!(
            "could not create Pi extensions directory {}",
            parent.display()
        )
    })?;
    let source = PI_SESSION_EXTENSION
        .replace(
            "__CAPTURE_ENABLED__",
            if capture_enabled { "true" } else { "false" },
        )
        .replace(
            "__TOOL_EVENTS_URL__",
            &serde_json::to_string(&format!("{}/tool-events", base_url.trim_end_matches('/')))?,
        )
        .replace(
            "__HARNESS_EVENTS_URL__",
            &serde_json::to_string(&format!(
                "{}/harness-events",
                base_url.trim_end_matches('/')
            ))?,
        )
        .replace("__API_KEY__", &serde_json::to_string(api_key)?);
    atomic_write_text(&path, &source)?;
    Ok(path)
}

pub(crate) fn set_pi_tool_capture(config_dir: &Path, base_url: &str, enabled: bool) -> Result<()> {
    let models = read_models_json(&config_dir.join("models.json"))?;
    let key = models["providers"][PROVIDER_NAME]["apiKey"]
        .as_str()
        .filter(|key| !key.is_empty())
        .context("Pi is not connected to Alex; connect it before enabling tool capture")?;
    install_pi_session_extension(config_dir, base_url, key, enabled)?;
    Ok(())
}

#[derive(Debug)]
struct MintedKey {
    id: String,
    key: String,
}

async fn mint_harness_key(
    config: &Config,
    client: &reqwest::Client,
    harness: &str,
) -> Result<MintedKey> {
    let body = json!({
        "kind": "harness",
        "label": harness,
        "tags": {"harness": harness},
    });
    let (status, value) = admin_send(
        config,
        client,
        reqwest::Method::POST,
        "/admin/run-keys",
        Some(body),
    )
    .await?;
    if !status.is_success() {
        bail!(
            "daemon could not mint a {harness} harness key ({status}): {}",
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

/// Best-effort cleanup of a harness's previous keys before minting a fresh one.
/// A transient failure here (a momentary daemon blip between the health check
/// and this admin call) must never abort the connect and leave the local
/// harness config unwritten: the new key is minted immediately after, and any
/// key left behind is revoked on the next connect. This mirrors the non-fatal
/// treatment the disconnect paths already give the same call. A genuinely
/// unreachable daemon still surfaces a clear error from the required
/// `mint_harness_key` that follows.
async fn revoke_stale_keys_best_effort(config: &Config, client: &reqwest::Client, harness: &str) {
    if let Err(e) = revoke_harness_keys(config, client, harness).await {
        eprintln!(
            "{}",
            ui::amber(&format!(
                "could not revoke previous {harness} harness keys ({e}); continuing"
            ))
        );
    }
}

async fn revoke_harness_keys(
    config: &Config,
    client: &reqwest::Client,
    harness: &str,
) -> Result<usize> {
    let value = admin_get(config, client, "/admin/run-keys", &[("all", "1")]).await?;
    let rows = value["run_keys"].as_array().cloned().unwrap_or_default();
    let ids: Vec<String> = rows
        .iter()
        .filter(|row| {
            row["kind"].as_str() == Some("harness") && row["label"].as_str() == Some(harness)
        })
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
                "daemon could not revoke old {harness} harness key {id} ({status}): {}",
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

async fn fetch_models_with_harness_key(
    base_url: &str,
    client: &reqwest::Client,
    api_key: &str,
) -> Result<Vec<String>> {
    let response = client
        .get(format!("{base_url}/v1/models"))
        .header("x-api-key", api_key)
        .send()
        .await
        .with_context(|| format!("could not reach the Alex daemon at {base_url}"))?;
    let status = response.status();
    let value: Value = response.json().await.unwrap_or_default();
    if !status.is_success() {
        bail!(
            "daemon rejected the supplied harness key while fetching /v1/models ({status}): {}",
            ui::truncate(&value.to_string(), 300)
        );
    }
    let ids = value["data"]
        .as_array()
        .context("daemon /v1/models response did not contain a data array")?
        .iter()
        .filter_map(|row| row["id"].as_str().map(String::from))
        .collect();
    let models = filter_model_ids(ids);
    if models.is_empty() {
        bail!("daemon /v1/models did not return any usable models");
    }
    Ok(models)
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
        .with_context(|| {
            format!(
                "could not reach the Alex daemon at {}",
                normalized_base_url(config)
            )
        })?;
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
    let resp = req.send().await.with_context(|| {
        format!(
            "could not reach the Alex daemon at {}",
            normalized_base_url(config)
        )
    })?;
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
    config.base_url()
}

pub(crate) fn pi_spec() -> &'static HarnessSpec {
    HARNESSES
        .iter()
        .find(|spec| spec.name == "pi")
        .expect("pi harness spec")
}

pub(crate) fn claude_spec() -> &'static HarnessSpec {
    HARNESSES
        .iter()
        .find(|spec| spec.name == "claude")
        .expect("claude harness spec")
}

pub(crate) fn codex_spec() -> &'static HarnessSpec {
    HARNESSES
        .iter()
        .find(|spec| spec.name == "codex")
        .expect("codex harness spec")
}

pub(crate) fn grok_spec() -> &'static HarnessSpec {
    HARNESSES
        .iter()
        .find(|spec| spec.name == "grok")
        .expect("grok harness spec")
}

pub(crate) fn amp_spec() -> &'static HarnessSpec {
    HARNESSES
        .iter()
        .find(|spec| spec.name == "amp")
        .expect("amp harness spec")
}

pub(crate) fn kimi_spec() -> &'static HarnessSpec {
    HARNESSES
        .iter()
        .find(|spec| spec.name == "kimi")
        .expect("kimi harness spec")
}

pub(crate) fn spec_by_name(name: &str) -> Option<&'static HarnessSpec> {
    HARNESSES.iter().find(|spec| spec.name == name)
}

/// Resolve the exact installation command without executing it. This is kept
/// separate both for dry-run callers and so catalog changes are easy to test.
pub(crate) fn install_command(spec: &HarnessSpec, version: Option<&str>) -> Option<InstallCommand> {
    match spec.install? {
        HarnessInstall::Npm { package } => Some(InstallCommand {
            program: "npm",
            args: vec![
                "install".into(),
                "-g".into(),
                match version.filter(|v| !v.trim().is_empty()) {
                    Some(version) => format!("{package}@{}", version.trim()),
                    None => package.to_string(),
                },
            ],
        }),
    }
}

/// Install a catalog harness only when it is missing (or does not satisfy a
/// requested version pin). Returns true when an install was performed.
pub(crate) async fn ensure_installed(spec: &HarnessSpec, version: Option<&str>) -> Result<bool> {
    let detection = detect_harness_without_config(spec).await;
    let pinned_matches = version
        .filter(|v| !v.trim().is_empty())
        .is_none_or(|wanted| {
            detection
                .version
                .as_deref()
                .is_some_and(|found| version_matches(found, wanted))
        });
    if detection.binary.is_some() && pinned_matches {
        println!(
            "install: {} already present{}",
            spec.binary,
            version.map(|v| format!(" ({v})")).unwrap_or_default()
        );
        return Ok(false);
    }
    let command = install_command(spec, version).with_context(|| {
        format!(
            "{} is not installed and alex has no installer for this harness",
            spec.name
        )
    })?;
    if find_on_path(command.program).is_none() {
        bail!("{} is missing and requires npm to install it; install Node.js/npm, then rerun `alex up {}`", spec.name, spec.name);
    }
    println!("install: {} {}", command.program, command.args.join(" "));
    let status = tokio::task::spawn_blocking(move || {
        Command::new(command.program).args(&command.args).status()
    })
    .await
    .context("wait for harness installer")??;
    if !status.success() {
        bail!("npm failed while installing {} ({status})", spec.name);
    }
    if find_on_path(spec.binary).is_none() {
        bail!("installed {}, but '{}' is still not on PATH; restart the shell or fix npm's global bin directory", spec.name, spec.binary);
    }
    Ok(true)
}

fn version_matches(found: &str, wanted: &str) -> bool {
    let found = found.trim().trim_start_matches('v');
    let wanted = wanted.trim().trim_start_matches('v');
    found == wanted || found.starts_with(&format!("{wanted}+"))
}

pub(crate) fn resolve_config_dir(
    config: &Config,
    spec: &HarnessSpec,
    explicit: Option<PathBuf>,
) -> PathBuf {
    explicit
        .or_else(|| {
            config
                .harness_overrides
                .get(spec.name)
                .and_then(|override_| override_.config_dir.clone())
        })
        .unwrap_or_else(|| {
            let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
            (spec.config_dir)(&home)
        })
}

fn pi_config_dir_for_home(home: &Path) -> PathBuf {
    home.join(".pi").join("agent")
}

fn claude_config_dir_for_home(home: &Path) -> PathBuf {
    home.join(".claude")
}

fn codex_config_dir_for_home(home: &Path) -> PathBuf {
    home.join(".codex")
}

fn gemini_config_dir_for_home(home: &Path) -> PathBuf {
    home.join(".gemini")
}

fn grok_config_dir_for_home(home: &Path) -> PathBuf {
    home.join(".grok")
}

fn amp_config_dir_for_home(home: &Path) -> PathBuf {
    home.join(".config").join("amp")
}

fn opencode_config_dir_for_home(home: &Path) -> PathBuf {
    home.join(".config").join("opencode")
}

fn omp_config_dir_for_home(home: &Path) -> PathBuf {
    home.join(".omp").join("agent")
}
fn mini_swe_agent_config_dir_for_home(home: &Path) -> PathBuf {
    home.join(".mini-swe-agent")
}
fn kimi_config_dir_for_home(home: &Path) -> PathBuf {
    home.join(".kimi-code")
}
fn qwen_config_dir_for_home(home: &Path) -> PathBuf {
    home.join(".qwen")
}
fn goose_config_dir_for_home(home: &Path) -> PathBuf {
    home.join(".config").join("goose")
}
fn opensage_config_dir_for_home(home: &Path) -> PathBuf {
    home.join(".opensage")
}
fn pydantic_ai_config_dir_for_home(home: &Path) -> PathBuf {
    home.join(".pydantic-ai")
}
fn stirrup_config_dir_for_home(home: &Path) -> PathBuf {
    home.join(".stirrup")
}
fn jcode_config_dir_for_home(home: &Path) -> PathBuf {
    home.join(".jcode")
}
fn cursor_config_dir_for_home(home: &Path) -> PathBuf {
    home.join(".cursor")
}
fn droid_config_dir_for_home(home: &Path) -> PathBuf {
    home.join(".factory")
}
fn hermes_config_dir_for_home(home: &Path) -> PathBuf {
    home.join(".hermes")
}

fn override_json(override_: Option<&HarnessOverride>) -> HarnessOverrideJson {
    HarnessOverrideJson {
        binary: override_
            .and_then(|o| o.binary.as_ref())
            .map(|p| p.to_string_lossy().to_string()),
        config_dir: override_
            .and_then(|o| o.config_dir.as_ref())
            .map(|p| p.to_string_lossy().to_string()),
    }
}

async fn detect_pi(config: &Config) -> PiDetection {
    let detection = detect_harness(config, pi_spec()).await;
    let version_check = if detection.binary.is_some() {
        check_version(detection.version.as_deref())
    } else {
        VersionCheck {
            parsed: None,
            warning: None,
        }
    };
    PiDetection {
        binary: detection.binary,
        version_raw: detection.version,
        version_check,
    }
}

async fn detect_harness(config: &Config, spec: &HarnessSpec) -> HarnessDetection {
    detect_harness_with_timeout(config, spec, Duration::from_secs(5)).await
}

async fn detect_harness_without_config(spec: &HarnessSpec) -> HarnessDetection {
    let Some(binary) = find_on_path(spec.binary) else {
        return HarnessDetection {
            binary: None,
            version: None,
            version_warning: None,
        };
    };
    let version = command_version(&binary, spec.version_args, Duration::from_secs(5)).await;
    let mut version_warning = version.warning.clone();
    if spec.name == "pi" {
        version_warning = version_warning.or(check_version(version.version.as_deref()).warning);
    }
    HarnessDetection {
        binary: Some(binary),
        version: version.version,
        version_warning,
    }
}

async fn detect_harness_with_timeout(
    config: &Config,
    spec: &HarnessSpec,
    timeout: Duration,
) -> HarnessDetection {
    let binary = resolve_harness_binary(config, spec);
    let Some(binary_path) = binary.clone() else {
        return HarnessDetection {
            binary: None,
            version: None,
            version_warning: None,
        };
    };
    let version = command_version(&binary_path, spec.version_args, timeout).await;
    let mut version_warning = version.warning.clone();
    if spec.name == "pi" {
        let check = check_version(version.version.as_deref());
        version_warning = version_warning.or(check.warning);
    }
    HarnessDetection {
        binary: Some(binary_path),
        version: version.version,
        version_warning,
    }
}

/// Resolve a harness binary using the same override-or-PATH rules as connect.
pub(crate) fn resolve_harness_binary(config: &Config, spec: &HarnessSpec) -> Option<PathBuf> {
    match config
        .harness_overrides
        .get(spec.name)
        .and_then(|override_| override_.binary.clone())
    {
        Some(path) if is_executable_file(&path) => Some(path),
        Some(_) => None,
        None => find_on_path(spec.binary).or_else(|| harness_home_binary(spec)),
    }
}

/// Some harnesses (kimi) install their binary inside their own config dir
/// (`~/.kimi-code/bin/kimi`). Interactive shells add that dir to PATH via rc
/// files, but non-interactive `alex connect` runs never see it, so detection
/// failed and connect bailed before minting a key — leaving the harness config
/// pointing at a key the preceding disconnect had already revoked.
fn harness_home_binary(spec: &HarnessSpec) -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let candidate = (spec.config_dir)(&home).join("bin").join(spec.binary);
    is_executable_file(&candidate).then_some(candidate)
}

pub(crate) fn find_on_path(bin: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        if !probe_safe_dir(&dir) {
            continue;
        }
        for candidate in executable_candidates(&dir, bin) {
            if is_executable_file(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

/// Never probe network volumes or TCC-protected user folders for harness
/// binaries: on macOS each stat there makes the OS ask "alex wants to access
/// files on a network volume / Desktop / …" — for every fresh (unsigned local)
/// build — and no harness installs itself in those places.
pub(crate) fn probe_safe_dir(dir: &Path) -> bool {
    if dir.starts_with("/Volumes") {
        return false;
    }
    if let Some(home) = dirs::home_dir() {
        for protected in ["Desktop", "Documents", "Downloads"] {
            if dir.starts_with(home.join(protected)) {
                return false;
            }
        }
    }
    true
}

fn executable_candidates(dir: &Path, bin: &str) -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        let pathext =
            std::env::var_os("PATHEXT").unwrap_or_else(|| OsString::from(".EXE;.CMD;.BAT"));
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

fn is_executable_file(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        return std::fs::metadata(path)
            .map(|m| m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false);
    }
    #[cfg(not(unix))]
    {
        true
    }
}

async fn command_version(binary: &Path, args: &[&str], timeout: Duration) -> VersionOutput {
    let cache_key = binary_detection_cache_key(binary);
    if let Some(cached) = VERSION_DETECTION_CACHE
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .ok()
        .and_then(|cache| cache.get(&cache_key).cloned())
    {
        return cached;
    }
    let output = command_version_uncached(binary, args, timeout).await;
    // Cache only successful probes. A timeout under machine load is transient,
    // and memoizing it pinned "version check timed out" (the orange triangle)
    // to a harness row until the binary itself changed — surviving refreshes
    // and Update All.
    if output.version.is_none() {
        return output;
    }
    if let Ok(mut cache) = VERSION_DETECTION_CACHE
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
    {
        cache.retain(|key, _| key.binary != cache_key.binary || key == &cache_key);
        if cache.len() >= VERSION_DETECTION_CACHE_MAX_ENTRIES && !cache.contains_key(&cache_key) {
            if let Some(oldest_key) = cache.keys().next().cloned() {
                cache.remove(&oldest_key);
            }
        }
        cache.insert(cache_key, output.clone());
    }
    output
}

fn detection_cache_key(
    binary: PathBuf,
    modified: Option<SystemTime>,
    size: Option<u64>,
) -> DetectionCacheKey {
    DetectionCacheKey {
        binary,
        modified,
        size,
    }
}

fn binary_detection_cache_key(binary: &Path) -> DetectionCacheKey {
    let binary = std::fs::canonicalize(binary).unwrap_or_else(|_| binary.to_path_buf());
    let metadata = std::fs::metadata(&binary).ok();
    let modified = metadata
        .as_ref()
        .and_then(|metadata| metadata.modified().ok());
    let size = metadata.as_ref().map(std::fs::Metadata::len);
    detection_cache_key(binary, modified, size)
}

async fn command_version_uncached(
    binary: &Path,
    args: &[&str],
    timeout: Duration,
) -> VersionOutput {
    let binary = binary.to_path_buf();
    let args: Vec<String> = args.iter().map(|arg| (*arg).to_string()).collect();
    match tokio::time::timeout(
        timeout,
        tokio::task::spawn_blocking(move || {
            let out = Command::new(binary).args(args).output().ok()?;
            let raw = if out.stdout.is_empty() {
                String::from_utf8_lossy(&out.stderr).to_string()
            } else {
                String::from_utf8_lossy(&out.stdout).to_string()
            };
            version_token(&raw)
        }),
    )
    .await
    {
        Ok(Ok(version)) => VersionOutput {
            version,
            warning: None,
        },
        Ok(Err(e)) => VersionOutput {
            version: None,
            warning: Some(format!("version check failed: {e}")),
        },
        Err(_) => VersionOutput {
            version: None,
            warning: Some("version check timed out".into()),
        },
    }
}

fn version_token(raw: &str) -> Option<String> {
    raw.split_whitespace()
        .find(|part| part.chars().any(|c| c.is_ascii_digit()))
        .map(|part| part.trim().to_string())
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
        .find(|part| part.chars().any(|c| c.is_ascii_digit()))?;
    let start = token.find(|c: char| c.is_ascii_digit())?;
    let numeric: String = token[start..]
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let mut parts = numeric.split('.');
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
    let value: Value = serde_json::from_str(&raw).with_context(|| {
        format!(
            "could not parse {}; aborting without changes",
            path.display()
        )
    })?;
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
            "models": model_ids.iter().map(|id| pi_model_config(id)).collect::<Vec<_>>(),
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
    serde_json::from_str(&raw).with_context(|| {
        format!(
            "could not parse {}; aborting without changes",
            path.display()
        )
    })
}

fn ensure_providers_object<'a>(
    value: &'a mut Value,
    path: &Path,
) -> Result<&'a mut serde_json::Map<String, Value>> {
    if !value.is_object() {
        bail!(
            "{} must contain a JSON object; aborting without changes",
            path.display()
        );
    }
    if value.get("providers").is_none() {
        value["providers"] = json!({});
    }
    value["providers"].as_object_mut().with_context(|| {
        format!(
            "{}.providers must be an object; aborting without changes",
            path.display()
        )
    })
}

fn atomic_write_json(path: &Path, value: &Value) -> Result<()> {
    let data = serde_json::to_string_pretty(value)? + "\n";
    atomic_write_text(path, &data)
}

fn atomic_write_text(path: &Path, data: &str) -> Result<()> {
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
        if !allowed_model_id(&id) || !seen.insert(id.clone()) {
            continue;
        }
        out.push(id);
    }
    out
}

fn allowed_model_id(id: &str) -> bool {
    if allowed_model_prefix(id) && !id.contains('/') {
        return true;
    }
    // Provider-prefixed catalog ids the daemon advertises in /v1/models. Any new
    // provider whose ids carry a `provider/` prefix (e.g. Kimi's `kimi/k3`) must
    // be listed here, otherwise it is silently dropped from every connected
    // harness's model list. `alexandria/*` is deliberately absent: it is only a
    // duplicate alias of the bare/prefixed ids and would produce a second
    // `alex/...` entry after `short_alex_model_ids` normalization.
    let Some(model) = ["openrouter/", "exo/", "kimi/", "alex/"]
        .iter()
        .find_map(|prefix| id.strip_prefix(prefix))
    else {
        return false;
    };
    model.split('/').all(|segment| {
        !segment.is_empty()
            && segment != "."
            && segment != ".."
            && segment
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':' | '+'))
    })
}

fn allowed_model_prefix(id: &str) -> bool {
    ["claude-", "gpt-", "o3", "o4", "codex-", "grok-", "gemini-"]
        .iter()
        .any(|prefix| id.starts_with(prefix))
}

fn pi_model_config(id: &str) -> Value {
    match id.strip_prefix("alex/").unwrap_or(id) {
        "gpt-5.6-sol" => gpt_5_6_pi_model(id, "GPT-5.6 Sol", 5.0, 30.0, 0.5, 6.25),
        "gpt-5.6-terra" => gpt_5_6_pi_model(id, "GPT-5.6 Terra", 2.5, 15.0, 0.25, 3.125),
        "gpt-5.6-luna" => gpt_5_6_pi_model(id, "GPT-5.6 Luna", 1.0, 6.0, 0.1, 1.25),
        _ => json!({
            "id": id,
            "reasoning": reasoning_enabled(id),
            "input": ["text", "image"],
            "contextWindow": 200000,
            "maxTokens": 16384,
        }),
    }
}

fn gpt_5_6_pi_model(
    id: &str,
    name: &str,
    input_cost: f64,
    output_cost: f64,
    cache_read_cost: f64,
    cache_write_cost: f64,
) -> Value {
    json!({
        "id": id,
        "name": name,
        "reasoning": true,
        "thinkingLevelMap": {
            "off": null,
            "minimal": "low",
            "xhigh": "xhigh",
        },
        "input": ["text", "image"],
        "contextWindow": 372000,
        "maxTokens": 128000,
        "cost": {
            "input": input_cost,
            "output": output_cost,
            "cacheRead": cache_read_cost,
            "cacheWrite": cache_write_cost,
        },
        "compat": {
            "forceAdaptiveThinking": true,
        },
    })
}

pub(crate) fn reasoning_enabled(id: &str) -> bool {
    let id = id.to_ascii_lowercase();
    let bare = id
        .rsplit_once('/')
        .map(|(_, rest)| rest)
        .unwrap_or(id.as_str());
    bare.contains("opus")
        || bare.contains("sonnet")
        || bare.contains("fable")
        || bare.contains("gpt-5")
        || bare.starts_with("o3")
        || bare.starts_with("o4")
        || bare.contains("grok")
        || (bare.starts_with("gemini-") && bare.contains("pro"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[cfg(unix)]
    #[tokio::test]
    async fn version_probe_timeout_is_not_cached() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tmpdir("version-timeout-uncached");
        std::fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("slowver");
        std::fs::write(&bin, "#!/bin/sh\nsleep 0.3\necho 9.9.9\n").unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();

        let timed_out = command_version(&bin, &[], Duration::from_millis(30)).await;
        assert_eq!(timed_out.warning.as_deref(), Some("version check timed out"));
        // The failure must not be memoized: a retry with a workable timeout
        // succeeds instead of replaying the cached timeout.
        let retried = command_version(&bin, &[], Duration::from_secs(10)).await;
        assert_eq!(retried.version.as_deref(), Some("9.9.9"));
    }

    #[test]
    fn probe_skips_network_volumes_and_protected_folders() {
        assert!(!probe_safe_dir(Path::new("/Volumes/NAS/bin")));
        assert!(!probe_safe_dir(Path::new("/Volumes")));
        assert!(probe_safe_dir(Path::new("/usr/local/bin")));
        assert!(probe_safe_dir(Path::new("/opt/homebrew/bin")));
        if let Some(home) = dirs::home_dir() {
            assert!(!probe_safe_dir(&home.join("Desktop/tools")));
            assert!(!probe_safe_dir(&home.join("Documents")));
            assert!(!probe_safe_dir(&home.join("Downloads/node/bin")));
            assert!(probe_safe_dir(&home.join(".local/bin")));
            // Prefix lookalikes stay probeable (path components, not strings).
            assert!(probe_safe_dir(&home.join("DesktopNot")));
        }
    }

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
    fn short_alex_model_ids_prefixes_bare_and_normalizes_long_forms() {
        assert_eq!(
            short_alex_model_ids(vec![
                "claude-opus-4-8".into(),
                "alex/gpt-5.5".into(),
                "alexandria/grok-4.5".into(),
                "cove/claude-fable-5".into(),
            ]),
            vec![
                "alex/claude-opus-4-8",
                "alex/gpt-5.5",
                "alex/grok-4.5",
                "alex/claude-fable-5",
            ]
        );
    }

    #[test]
    fn model_id_diff_reports_added_removed_unchanged() {
        let before = vec!["alex/a".into(), "alex/b".into(), "alex/c".into()];
        let after = vec!["alex/b".into(), "alex/c".into(), "alex/d".into()];
        let (added, removed, unchanged) = model_id_diff(&before, &after);
        assert_eq!(added, vec!["alex/d"]);
        assert_eq!(removed, vec!["alex/a"]);
        assert_eq!(unchanged, 2);
    }

    #[test]
    fn codex_upsert_enables_absent_hooks_and_preserves_true() {
        let dir = tmpdir("codex-hooks-default");
        let catalog = dir.join(CODEX_CATALOG_FILE);
        let key = dir.join(CODEX_KEY_FILE);

        let mut absent = DocumentMut::new();
        upsert_codex_config(
            &mut absent,
            "http://127.0.0.1:4100",
            &catalog,
            &key,
            "alex/gpt-5.5",
            None,
        )
        .unwrap();
        assert_eq!(absent["features"]["hooks"].as_bool(), Some(true));

        let mut enabled = DocumentMut::from_str("[features]\nhooks = true\n").unwrap();
        upsert_codex_config(
            &mut enabled,
            "http://127.0.0.1:4100",
            &catalog,
            &key,
            "alex/gpt-5.5",
            None,
        )
        .unwrap();
        assert_eq!(enabled["features"]["hooks"].as_bool(), Some(true));
    }

    #[cfg(unix)]
    #[test]
    fn preminted_claude_writer_uses_provided_key_and_remote_hook_url() {
        let dir = tmpdir("preminted-claude");
        let summary = write_preminted_connection(
            "claude",
            dir.clone(),
            "http://host.docker.internal:4100".into(),
            "rk-cove-claude".into(),
            "alxk-cove-claude".into(),
            model_ids(),
            None,
            Some("1.2.3".into()),
            true,
        )
        .unwrap();

        assert_eq!(summary.key_id, "rk-cove-claude");
        assert_eq!(
            read_claude_api_key(&dir).as_deref(),
            Some("alxk-cove-claude")
        );
        assert!(std::fs::read_to_string(dir.join(CLAUDE_HOOK_CURL_FILE))
            .unwrap()
            .contains("http://host.docker.internal:4100/harness-events"));
        assert!(
            std::fs::read_to_string(dir.join(CLAUDE_TOOL_HOOK_CURL_FILE))
                .unwrap()
                .contains("http://host.docker.internal:4100/tool-events")
        );
    }

    #[cfg(unix)]
    #[test]
    fn preminted_codex_writer_uses_provided_key_and_remote_hook_url() {
        let dir = tmpdir("preminted-codex");
        let catalog = json!({"models": [
            {"slug": "gpt-5.6-luna", "display_name": "Luna"},
            {"slug": "alex/gpt-5.6-luna", "display_name": "alex/gpt-5.6-luna"}
        ]});
        let summary = write_preminted_connection(
            "codex",
            dir.clone(),
            "http://host.docker.internal:4100".into(),
            "rk-cove-codex".into(),
            "alxk-cove-codex".into(),
            model_ids(),
            Some(catalog),
            Some("0.144.3".into()),
            true,
        )
        .unwrap();

        assert_eq!(summary.key_id, "rk-cove-codex");
        assert_eq!(read_codex_api_key(&dir).as_deref(), Some("alxk-cove-codex"));
        assert!(std::fs::read_to_string(dir.join(CODEX_HOOK_CURL_FILE))
            .unwrap()
            .contains("http://host.docker.internal:4100/harness-events"));
        assert!(std::fs::read_to_string(dir.join(CODEX_TOOL_HOOK_CURL_FILE))
            .unwrap()
            .contains("http://host.docker.internal:4100/tool-events"));
    }

    fn test_config() -> Config {
        Config {
            host: "127.0.0.1".into(),
            port: 4100,
            data_dir: tmpdir("data"),
            local_key: "alx-test".into(),
            heartbeat_minutes: crate::default_heartbeat_minutes(),
            reauth_check_minutes: crate::default_reauth_check_minutes(),
            ping_anthropic_model: crate::default_ping_anthropic(),
            ping_openai_model: crate::default_ping_openai(),
            ping_xai_model: crate::default_ping_xai(),
            ping_gemini_model: crate::default_ping_gemini(),
            ping_openrouter_model: crate::default_ping_openrouter(),
            exo_url: crate::default_exo_url(),
            exo_enabled_models: Vec::new(),
            openrouter_exposed_models: alex_proxy::default_openrouter_exposed_models(),
            gemini_project: String::new(),
            anthropic_upstream: crate::default_anthropic_upstream(),
            dario_mode_migrated: true,
            dario_api_key: String::new(),
            dario_claude_bin: None,
            dario_node_path: None,
            dario_update_check_minutes: crate::default_dario_update_minutes(),
            dario_version: None,
            dario_probe_seconds: crate::default_dario_probe_seconds(),
            dario_probe_failures: crate::default_dario_probe_failures(),
            dario_probe_model: crate::default_dario_probe_model(),
            trace_body_retention_days: crate::default_trace_body_retention_days(),
            trace_row_retention_days: 0,
            update_check_hours: crate::default_update_check_hours(),
            update_channel: crate::default_update_channel(),
            upstream_stream_idle_timeout_seconds:
                crate::default_upstream_stream_idle_timeout_seconds(),
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

    #[cfg(unix)]
    fn fake_executable(dir: &Path, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn catalog_detection_uses_overrides() {
        let dir = tmpdir("override-detect");
        let config_dir = dir.join("claude-config");
        std::fs::create_dir_all(&config_dir).unwrap();
        let binary = fake_executable(&dir, "claude-fake", "echo claude-code 1.2.3");
        let mut config = test_config();
        config.harness_overrides.insert(
            "claude".into(),
            HarnessOverride {
                binary: Some(binary.clone()),
                config_dir: Some(config_dir.clone()),
            },
        );

        let status = harness_status(&config, spec_by_name("claude").unwrap(), None, true)
            .await
            .unwrap();
        assert_eq!(status.name, "claude");
        assert!(status.installed);
        assert_eq!(status.binary.as_deref(), Some(binary.to_str().unwrap()));
        assert_eq!(status.version.as_deref(), Some("1.2.3"));
        assert_eq!(status.config_dir, config_dir.to_string_lossy());
        assert!(status.config_dir_exists);
        assert!(!status.connected);
        assert!(status.supports_connect);
        assert_eq!(
            status.override_.config_dir.as_deref(),
            Some(config_dir.to_str().unwrap())
        );

        let statuses = harness_statuses(&config, None, true).await.unwrap();
        assert_eq!(statuses.len(), 19);
        assert!(statuses.iter().any(|s| s.name == "opencode"));
        assert!(statuses.iter().any(|s| s.name == "amp"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn version_detection_timeout_keeps_installed_true() {
        let dir = tmpdir("timeout");
        let binary = fake_executable(&dir, "codex-slow", "sleep 2\necho codex 9.9.9");
        let mut config = test_config();
        config.harness_overrides.insert(
            "codex".into(),
            HarnessOverride {
                binary: Some(binary),
                config_dir: None,
            },
        );
        let detection = detect_harness_with_timeout(
            &config,
            spec_by_name("codex").unwrap(),
            Duration::from_millis(100),
        )
        .await;
        assert!(detection.binary.is_some());
        assert!(detection.version.is_none());
        assert_eq!(
            detection.version_warning.as_deref(),
            Some("version check timed out")
        );
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
        assert_eq!(
            provider["headers"]["x-alexandria-harness-version"],
            "!pi --version"
        );
        assert_eq!(provider["models"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn tailscale_bind_writes_loopback_into_local_harness_config() {
        let dir = tmpdir("tailscale-local-base-url");
        let mut config = test_config();
        config.host = "100.101.102.103".into();
        let base_url = normalized_base_url(&config);
        write_pi_connection(
            dir.clone(),
            base_url,
            "rk-test".into(),
            "alxk-test".into(),
            model_ids(),
            None,
        )
        .unwrap();
        let models: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("models.json")).unwrap())
                .unwrap();
        assert_eq!(
            models["providers"]["alexandria"]["baseUrl"],
            "http://127.0.0.1:4100"
        );
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
        install_pi_session_extension(&dir, "http://127.0.0.1:4100", "test-key", false).unwrap();
        let extension_path = pi_session_extension_path(&dir);
        assert!(extension_path.exists());
        assert!(disconnect_pi_config(&dir).unwrap());
        let value: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(value["providers"]["alexandria"].is_null());
        assert_eq!(value["providers"]["foreign"]["api"], "x");
        assert!(!extension_path.exists());
        assert!(!disconnect_pi_config(&dir).unwrap());
    }

    #[test]
    fn write_connection_installs_scoped_pi_session_extension() {
        let dir = tmpdir("session-extension");
        let summary = write_pi_connection(
            dir.clone(),
            "http://127.0.0.1:4100".into(),
            "rk-test".into(),
            "alxk-test".into(),
            model_ids(),
            Some("0.80.6".into()),
        )
        .unwrap();

        assert_eq!(summary.extension_path, pi_session_extension_path(&dir));
        let source = std::fs::read_to_string(&summary.extension_path).unwrap();
        assert!(source.contains("ctx.model.provider !== \"alexandria\""));
        assert!(
            source.contains("event.headers[\"x-session-id\"] = ctx.sessionManager.getSessionId()")
        );
        // Subagent lineage: parent session id travels to child pi processes
        // via inherited env, and inherited ids announce to /harness-events.
        assert!(
            source.contains("const harnessEventsUrl = \"http://127.0.0.1:4100/harness-events\"")
        );
        assert!(source.contains("process.env.ALEXANDRIA_SESSION_ID"));
        assert!(source.contains("\"SubagentStart\""));
    }

    #[cfg(unix)]
    #[test]
    fn amp_connection_installs_privacy_minimized_plugin_and_restores_previous_file() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tmpdir("amp-round-trip");
        let plugin_path = amp_plugin_path(&dir);
        std::fs::create_dir_all(plugin_path.parent().unwrap()).unwrap();
        let previous = "export default function userPlugin() {}\n";
        std::fs::write(&plugin_path, previous).unwrap();

        let summary = write_amp_connection(
            dir.clone(),
            "http://127.0.0.1:4100".into(),
            "rk-amp".into(),
            "alxk-amp-secret".into(),
            Some("0.0.1784018462-g51e7e3".into()),
        )
        .unwrap();

        assert!(summary.models.is_empty());
        assert_eq!(summary.config_path, plugin_path);
        assert!(amp_config_connected(&dir).unwrap());
        assert_eq!(read_amp_api_key(&dir).as_deref(), Some("alxk-amp-secret"));
        assert_eq!(
            std::fs::metadata(dir.join(AMP_KEY_FILE))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let source = std::fs::read_to_string(&summary.config_path).unwrap();
        for event in [
            "session.start",
            "agent.start",
            "agent.end",
            "tool.call",
            "tool.result",
        ] {
            assert!(source.contains(&format!("amp.on('{event}'")));
        }
        assert!(source.contains("return { action: 'allow' }"));
        assert!(source.contains("alex-status"));
        assert!(source.contains("alexandria-status"));
        assert!(source.contains("Status (legacy alias)"));
        assert!(source.contains("hook_event_name: 'SubagentStart'"));
        assert!(source.contains("hook_event_name: 'SubagentStop'"));
        assert!(source.contains("http://127.0.0.1:4100/harness-events"));
        assert!(!source.contains("alxk-amp-secret"));
        assert!(!source.contains("event.input,"));
        assert!(!source.contains("event.output,"));

        std::fs::write(dir.join(AMP_EVENT_LOG_FILE), "kept\n").unwrap();
        assert!(disconnect_amp_config(&dir).unwrap());
        assert!(!amp_config_connected(&dir).unwrap());
        assert_eq!(std::fs::read_to_string(&plugin_path).unwrap(), previous);
        assert!(!dir.join(AMP_KEY_FILE).exists());
        assert_eq!(
            std::fs::read_to_string(dir.join(AMP_EVENT_LOG_FILE)).unwrap(),
            "kept\n"
        );
    }

    #[test]
    fn amp_disconnect_removes_new_managed_plugin_and_plan_explains_wrapper_boundary() {
        let dir = tmpdir("amp-new-plugin");
        let summary = write_amp_connection(
            dir.clone(),
            "http://127.0.0.1:4100".into(),
            "rk-amp".into(),
            "alxk-amp-secret".into(),
            None,
        )
        .unwrap();
        let plan = plan_amp_connect(&dir, &[]);
        let about = plan["plan"]
            .as_array()
            .unwrap()
            .iter()
            .find(|step| step["action"] == "about")
            .unwrap()["detail"]
            .as_str()
            .unwrap();
        assert!(about.contains("alex wrap amp"));
        assert!(about.contains("cannot add an Alex model provider"));
        assert!(disconnect_amp_config(&dir).unwrap());
        assert!(!summary.config_path.exists());
        assert!(!disconnect_amp_config(&dir).unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn claude_connection_uses_opt_in_profile_and_restores_existing_profile() {
        let dir = tmpdir("claude-round-trip");
        let settings_path = dir.join(CLAUDE_SETTINGS_FILE);
        let original_settings = r#"{
  "theme": "dark",
  "hooks": {"Stop": [{"hooks": [{"type": "command", "command": "/tmp/user-hook"}]}]}
}
"#;
        std::fs::write(&settings_path, original_settings).unwrap();
        let previous_profile = "{\"model\":\"user-model\",\"custom\":true}\n";
        std::fs::write(dir.join(CLAUDE_PROFILE_FILE), previous_profile).unwrap();

        let summary = write_claude_connection(
            dir.clone(),
            "http://127.0.0.1:4100".into(),
            "rk-claude".into(),
            "alxk-claude".into(),
            model_ids(),
            Some("2.1.202".into()),
        )
        .unwrap();

        assert_eq!(summary.models, vec!["alex/claude-opus-4-8", "alex/gpt-5.5"]);
        assert_eq!(
            std::fs::read_to_string(&settings_path).unwrap(),
            original_settings
        );
        assert_eq!(
            std::fs::read_to_string(dir.join(CLAUDE_BACKUP_FILE)).unwrap(),
            original_settings
        );
        assert_eq!(read_claude_api_key(&dir).as_deref(), Some("alxk-claude"));
        assert!(claude_config_connected(&dir).unwrap());
        let profile = read_json_object(&dir.join(CLAUDE_PROFILE_FILE)).unwrap();
        assert_eq!(profile["model"], "claude-alex/claude-opus-4-8");
        assert_eq!(
            profile["env"]["ANTHROPIC_BASE_URL"],
            "http://127.0.0.1:4100"
        );
        assert_eq!(
            profile["env"]["CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY"],
            "1"
        );
        assert!(profile["env"]["ANTHROPIC_CUSTOM_HEADERS"]
            .as_str()
            .unwrap()
            .contains("x-alexandria-harness: claude"));
        assert!(profile["apiKeyHelper"]
            .as_str()
            .unwrap()
            .contains(CLAUDE_KEY_FILE));
        for event in ["SessionStart", "SubagentStart", "SubagentStop"] {
            assert_eq!(profile["hooks"][event].as_array().unwrap().len(), 1);
        }
        let catalog = read_json_object(&dir.join(CLAUDE_CATALOG_FILE)).unwrap();
        assert_eq!(catalog["models"][1]["id"], "claude-alex/gpt-5.5");
        assert_eq!(catalog["models"][1]["display_name"], "alex/gpt-5.5");
        let hook = std::fs::read_to_string(&summary.extension_path).unwrap();
        assert!(hook.contains("Reconnect Claude Code to refresh"));
        assert!(hook.contains("--data-binary @-"));

        assert!(disconnect_claude_config(&dir).unwrap());
        assert!(!claude_config_connected(&dir).unwrap());
        assert_eq!(
            std::fs::read_to_string(dir.join(CLAUDE_PROFILE_FILE)).unwrap(),
            previous_profile
        );
        assert_eq!(
            std::fs::read_to_string(&settings_path).unwrap(),
            original_settings
        );
        assert!(dir.join(CLAUDE_BACKUP_FILE).exists());
        assert!(!dir.join(CLAUDE_KEY_FILE).exists());
        assert!(!dir.join(CLAUDE_CATALOG_FILE).exists());
        assert!(!summary.extension_path.exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn claude_disconnect_with_unreachable_daemon_revokes_local_harness_keys() {
        let dir = tmpdir("claude-disconnect-daemon-down");
        let mut config = test_config();
        config.port = 0;
        let store = alex_store::Store::open(config.data_dir.clone()).unwrap();
        store
            .insert_run_key(
                "rk-claude-local",
                "claude-local-key-hash",
                "harness",
                None,
                Some(r#"{"harness":"claude"}"#),
                Some("claude"),
                1,
                None,
            )
            .unwrap();
        write_claude_connection(
            dir.clone(),
            "http://127.0.0.1:4100".into(),
            "rk-claude-local".into(),
            "alxk-claude-local".into(),
            model_ids(),
            Some("2.1.202".into()),
        )
        .unwrap();

        disconnect_claude(&config, Some(dir)).await.unwrap();

        assert!(store.list_run_keys(false).unwrap().is_empty());
        assert_eq!(store.list_run_keys(true).unwrap()[0]["revoked"], true);
    }

    #[test]
    fn claude_disconnect_cleans_persisted_alexandria_model_after_state_is_gone() {
        let dir = tmpdir("claude-stale-selection");
        std::fs::create_dir_all(dir.join("cache")).unwrap();
        std::fs::write(
            dir.join(CLAUDE_SETTINGS_FILE),
            r#"{"model":"claude-alex/claude-fable-5","theme":"dark"}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("cache").join("gateway-models.json"),
            r#"{"baseUrl":"http://127.0.0.1:4100","models":[{"id":"claude-alex/claude-fable-5"}]}"#,
        )
        .unwrap();

        assert!(disconnect_claude_config(&dir).unwrap());
        let settings = read_json_object(&dir.join(CLAUDE_SETTINGS_FILE)).unwrap();
        assert!(settings.get("model").is_none());
        assert_eq!(settings["theme"], "dark");
        assert!(!dir.join("cache").join("gateway-models.json").exists());
        assert!(!disconnect_claude_config(&dir).unwrap());
    }

    #[test]
    fn claude_disconnect_preserves_native_model_and_unrelated_gateway_cache() {
        let dir = tmpdir("claude-native-selection");
        std::fs::create_dir_all(dir.join("cache")).unwrap();
        std::fs::write(
            dir.join(CLAUDE_SETTINGS_FILE),
            r#"{"model":"claude-fable-5","theme":"dark"}"#,
        )
        .unwrap();
        let cache_path = dir.join("cache").join("gateway-models.json");
        std::fs::write(
            &cache_path,
            r#"{"baseUrl":"https://gateway.example","models":[{"id":"claude-fable-5"}]}"#,
        )
        .unwrap();

        assert!(!disconnect_claude_config(&dir).unwrap());
        let settings = read_json_object(&dir.join(CLAUDE_SETTINGS_FILE)).unwrap();
        assert_eq!(settings["model"], "claude-fable-5");
        assert!(cache_path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn grok_connection_preserves_native_models_and_removes_only_managed_models() {
        let dir = tmpdir("grok-round-trip");
        let config_path = dir.join(GROK_CONFIG_FILE);
        let original_config = r#"[models]
default = "grok-build"

[model.user-local]
model = "local-model"
base_url = "http://127.0.0.1:11434/v1"

[ui]
yolo = false
"#;
        std::fs::write(&config_path, original_config).unwrap();
        std::fs::create_dir_all(dir.join("hooks")).unwrap();
        let prior_hook = "{\"hooks\":{\"Stop\":[]}}\n";
        std::fs::write(
            dir.join("hooks").join(GROK_HOOK_REGISTRATION_FILE),
            prior_hook,
        )
        .unwrap();

        let summary = write_grok_connection(
            dir.clone(),
            "http://127.0.0.1:4100".into(),
            "rk-grok".into(),
            "alxk-grok".into(),
            model_ids(),
            Some("0.2.93".into()),
        )
        .unwrap();

        assert_eq!(summary.models, vec!["alex/claude-opus-4-8", "alex/gpt-5.5"]);
        assert!(grok_config_connected(&dir).unwrap());
        assert_eq!(read_grok_api_key(&dir).as_deref(), Some("alxk-grok"));
        assert_eq!(
            std::fs::read_to_string(dir.join(GROK_BACKUP_FILE)).unwrap(),
            original_config
        );
        let config = read_grok_config(&config_path).unwrap();
        assert_eq!(config["models"]["default"].as_str(), Some("grok-build"));
        assert_eq!(
            config["model"]["user-local"]["model"].as_str(),
            Some("local-model")
        );
        for model in &summary.models {
            let entry = &config["model"][model];
            assert_eq!(entry["model"].as_str(), Some(model.as_str()));
            assert_eq!(entry["api_backend"].as_str(), Some("chat_completions"));
            assert_eq!(entry["api_key"].as_str(), Some("alxk-grok"));
            assert_eq!(
                entry["extra_headers"]
                    .as_inline_table()
                    .unwrap()
                    .get("x-alexandria-harness")
                    .and_then(|value| value.as_str()),
                Some("grok")
            );
        }
        let registration =
            read_json_object(&dir.join("hooks").join(GROK_HOOK_REGISTRATION_FILE)).unwrap();
        assert!(registration["hooks"]["SessionStart"].is_array());
        assert!(registration["hooks"]["SubagentStart"].is_array());
        assert!(registration["hooks"]["SubagentStop"].is_array());

        assert!(disconnect_grok_config(&dir).unwrap());
        assert!(!grok_config_connected(&dir).unwrap());
        let restored = read_grok_config(&config_path).unwrap();
        assert_eq!(restored["models"]["default"].as_str(), Some("grok-build"));
        assert_eq!(
            restored["model"]["user-local"]["model"].as_str(),
            Some("local-model")
        );
        assert!(restored["model"]
            .as_table()
            .unwrap()
            .iter()
            .all(|(model, _)| !model.starts_with("alex/")));
        assert_eq!(
            std::fs::read_to_string(dir.join("hooks").join(GROK_HOOK_REGISTRATION_FILE)).unwrap(),
            prior_hook
        );
        assert!(dir.join(GROK_BACKUP_FILE).exists());
        assert!(!dir.join(GROK_KEY_FILE).exists());
    }

    #[cfg(unix)]
    #[test]
    fn kimi_connection_adds_alex_provider_and_models_then_reverts() {
        let dir = tmpdir("kimi-round-trip");
        let config_path = dir.join(KIMI_CONFIG_FILE);
        // A realistic Kimi config with the native managed provider + model.
        let original_config = r#"default_model = "kimi-code/k3"

[providers."managed:kimi-code"]
type = "kimi"
api_key = ""
base_url = "https://api.kimi.com/coding/v1"

[models."kimi-code/k3"]
provider = "managed:kimi-code"
model = "k3"
max_context_size = 262144
display_name = "K3"
"#;
        std::fs::write(&config_path, original_config).unwrap();

        let summary = write_kimi_connection(
            dir.clone(),
            "http://127.0.0.1:4100".into(),
            "rk-kimi".into(),
            "alxk-kimi".into(),
            model_ids(),
            Some("0.27.0".into()),
        )
        .unwrap();

        assert_eq!(summary.models, vec!["alex/claude-opus-4-8", "alex/gpt-5.5"]);
        assert!(kimi_config_connected(&dir).unwrap());
        // Original config was backed up verbatim.
        assert_eq!(
            std::fs::read_to_string(dir.join(KIMI_BACKUP_FILE)).unwrap(),
            original_config
        );

        let config = read_grok_config(&config_path).unwrap();
        // Kimi's own provider + model are untouched.
        assert_eq!(
            config["providers"]["managed:kimi-code"]["type"].as_str(),
            Some("kimi")
        );
        assert_eq!(
            config["models"]["kimi-code/k3"]["model"].as_str(),
            Some("k3")
        );
        // The alexandria OpenAI-compatible provider points at the local proxy.
        let provider = &config["providers"][KIMI_PROVIDER_NAME];
        assert_eq!(provider["type"].as_str(), Some("openai"));
        assert_eq!(provider["api_key"].as_str(), Some("alxk-kimi"));
        assert_eq!(
            provider["base_url"].as_str(),
            Some("http://127.0.0.1:4100/v1")
        );
        // Each alex/* model routes through that provider.
        for model in &summary.models {
            let entry = &config["models"][model];
            assert_eq!(entry["provider"].as_str(), Some(KIMI_PROVIDER_NAME));
            assert_eq!(entry["model"].as_str(), Some(model.as_str()));
            assert_eq!(entry["display_name"].as_str(), Some(model.as_str()));
        }

        // Disconnect removes only what Alexandria added and reverts the config.
        assert!(disconnect_kimi_config(&dir).unwrap());
        assert!(!kimi_config_connected(&dir).unwrap());
        let restored = read_grok_config(&config_path).unwrap();
        assert_eq!(
            restored["providers"]["managed:kimi-code"]["type"].as_str(),
            Some("kimi")
        );
        assert_eq!(
            restored["models"]["kimi-code/k3"]["model"].as_str(),
            Some("k3")
        );
        assert!(restored["providers"].get(KIMI_PROVIDER_NAME).is_none());
        assert!(restored["models"]
            .as_table()
            .unwrap()
            .iter()
            .all(|(model, _)| !model.starts_with("alex/")));
        // Second disconnect is a no-op (state file already gone).
        assert!(!disconnect_kimi_config(&dir).unwrap());
        assert!(dir.join(KIMI_BACKUP_FILE).exists());
    }

    #[test]
    fn kimi_disconnect_without_state_removes_orphaned_alex_entries() {
        let dir = tmpdir("kimi-disconnect-orphaned");
        // Config left behind by a partial disconnect: Alex entries present but
        // the managed-state marker file is gone.
        std::fs::write(
            dir.join(KIMI_CONFIG_FILE),
            r#"default_model = "alex/gpt-5.5"

[providers."managed:kimi-code"]
type = "kimi"
api_key = ""
base_url = "https://api.kimi.com/coding/v1"

[providers.alexandria]
type = "openai"
api_key = "alxk-revoked"
base_url = "http://127.0.0.1:4100/v1"

[models."kimi-code/k3"]
provider = "managed:kimi-code"
model = "k3"

[models."alex/gpt-5.5"]
provider = "alexandria"
model = "alex/gpt-5.5"
"#,
        )
        .unwrap();

        assert!(disconnect_kimi_config(&dir).unwrap());
        let restored = read_grok_config(&dir.join(KIMI_CONFIG_FILE)).unwrap();
        assert!(restored["providers"].get(KIMI_PROVIDER_NAME).is_none());
        assert!(restored["models"].get("alex/gpt-5.5").is_none());
        assert_eq!(
            restored["models"]["kimi-code/k3"]["model"].as_str(),
            Some("k3")
        );
        // The stale alex default was replaced with a surviving native model.
        assert_eq!(restored["default_model"].as_str(), Some("kimi-code/k3"));
    }

    #[test]
    fn kimi_reconnect_adopts_orphaned_provider_without_state() {
        let dir = tmpdir("kimi-reconnect-orphaned");
        // A reconnect after the state file was lost must adopt and replace the
        // self-identifying Alex entries instead of bailing (which stranded the
        // revoked key in place).
        std::fs::write(
            dir.join(KIMI_CONFIG_FILE),
            r#"[providers.alexandria]
type = "openai"
api_key = "alxk-revoked"
base_url = "http://127.0.0.1:4100/v1"

[models."alex/legacy-model"]
provider = "alexandria"
model = "alex/legacy-model"
"#,
        )
        .unwrap();

        let summary = write_kimi_connection(
            dir.clone(),
            "http://127.0.0.1:4100".into(),
            "rk-kimi-2".into(),
            "alxk-fresh".into(),
            model_ids(),
            None,
        )
        .unwrap();

        assert_eq!(summary.models, vec!["alex/claude-opus-4-8", "alex/gpt-5.5"]);
        let config = read_grok_config(&dir.join(KIMI_CONFIG_FILE)).unwrap();
        assert_eq!(
            config["providers"][KIMI_PROVIDER_NAME]["api_key"].as_str(),
            Some("alxk-fresh")
        );
        // The orphaned model entry a lost state file no longer tracked is gone.
        assert!(config["models"].get("alex/legacy-model").is_none());
        assert!(kimi_config_connected(&dir).unwrap());
    }

    #[test]
    fn kimi_disconnect_restores_native_default_from_backup() {
        let dir = tmpdir("kimi-restore-backup-default");
        std::fs::write(
            dir.join(KIMI_BACKUP_FILE),
            r#"default_model = "kimi-code/k3"
"#,
        )
        .unwrap();
        std::fs::write(
            dir.join(KIMI_STATE_FILE),
            r#"{"managed_models":["alex/gpt-5.6-sol"],"added_provider":true}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join(KIMI_CONFIG_FILE),
            r#"default_model = "alex/gpt-5.6-sol"
theme = "dark"

[providers.alexandria]
type = "openai"
base_url = "http://127.0.0.1:4100/v1"

[models."kimi-code/k3"]
provider = "managed:kimi-code"
model = "k3"

[models."alex/gpt-5.6-sol"]
provider = "alexandria"
model = "alex/gpt-5.6-sol"
"#,
        )
        .unwrap();

        assert!(disconnect_kimi_config(&dir).unwrap());
        let config = read_grok_config(&dir.join(KIMI_CONFIG_FILE)).unwrap();
        let default_model = config["default_model"].as_str().unwrap();
        println!("final default_model: {default_model}");
        assert_eq!(default_model, "kimi-code/k3");
        assert_eq!(config["theme"].as_str(), Some("dark"));
        assert!(config["models"].get("alex/gpt-5.6-sol").is_none());
        assert!(config.get("providers").is_none());
    }

    #[test]
    fn kimi_disconnect_without_backup_prefers_surviving_kimi_model() {
        let dir = tmpdir("kimi-fallback-native-model");
        std::fs::write(
            dir.join(KIMI_STATE_FILE),
            r#"{"managed_models":["alex/gpt-5.6-sol"],"added_provider":false}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join(KIMI_CONFIG_FILE),
            r#"default_model = "alex/gpt-5.6-sol"

[models."custom/native"]
provider = "custom"
model = "native"

[models."kimi-code/k3"]
provider = "managed:kimi-code"
model = "k3"

[models."alex/gpt-5.6-sol"]
provider = "alexandria"
model = "alex/gpt-5.6-sol"
"#,
        )
        .unwrap();

        assert!(disconnect_kimi_config(&dir).unwrap());
        let config = read_grok_config(&dir.join(KIMI_CONFIG_FILE)).unwrap();
        assert_eq!(config["default_model"].as_str(), Some("kimi-code/k3"));
    }

    #[test]
    fn kimi_disconnect_restores_default_after_state_is_gone() {
        let dir = tmpdir("kimi-stale-selection-no-state");
        std::fs::write(
            dir.join(KIMI_BACKUP_FILE),
            r#"default_model = "kimi-code/k3"
"#,
        )
        .unwrap();
        std::fs::write(
            dir.join(KIMI_CONFIG_FILE),
            r#"default_model = "alexandria/gpt-5.6-sol"
theme = "dark"
"#,
        )
        .unwrap();

        assert!(disconnect_kimi_config(&dir).unwrap());
        let config = read_grok_config(&dir.join(KIMI_CONFIG_FILE)).unwrap();
        assert_eq!(config["default_model"].as_str(), Some("kimi-code/k3"));
        assert_eq!(config["theme"].as_str(), Some("dark"));
        assert!(!disconnect_kimi_config(&dir).unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn kimi_connection_refuses_to_clobber_a_user_provider() {
        let dir = tmpdir("kimi-guard");
        let config_path = dir.join(KIMI_CONFIG_FILE);
        // A user already has their own [providers.alexandria]; we must not touch it.
        std::fs::write(
            &config_path,
            "[providers.alexandria]\ntype = \"openai\"\napi_key = \"user-key\"\nbase_url = \"https://example.test\"\n",
        )
        .unwrap();
        let err = write_kimi_connection(
            dir.clone(),
            "http://127.0.0.1:4100".into(),
            "rk-kimi".into(),
            "alxk-kimi".into(),
            model_ids(),
            None,
        )
        .unwrap_err();
        assert!(err.to_string().contains("unmanaged provider"));
    }

    #[cfg(unix)]
    #[test]
    fn codex_connection_round_trip_preserves_user_config_and_hooks() {
        let dir = tmpdir("codex-round-trip");
        let config_path = dir.join(CODEX_CONFIG_FILE);
        std::fs::write(
            &config_path,
            r#"model = "gpt-5.4"
model_provider = "openai"

[features]
hooks = false

[projects."/tmp/example"]
trust_level = "trusted"
"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("hooks.json"),
            r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"/tmp/foreign-hook"}]}]}}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join(CODEX_OPENAI_PROFILE_FILE),
            "# user's existing openai profile\nmodel = \"gpt-5.4\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.join(CODEX_ALEX_PROFILE_FILE),
            "# user's existing alex profile\nmodel = \"custom\"\n",
        )
        .unwrap();
        let catalog = json!({"models": [
            {"slug": "gpt-5.6-luna", "display_name": "Luna"},
            {"slug": "gpt-5.5", "display_name": "GPT-5.5"},
            {"slug": "alex/gpt-5.6-luna", "display_name": "alex/gpt-5.6-luna"},
            {"slug": "alex/gpt-5.5", "display_name": "alex/gpt-5.5"}
        ]});

        let summary = write_codex_connection(
            dir.clone(),
            "http://127.0.0.1:4100".into(),
            "rk-codex".into(),
            "alxk-codex".into(),
            catalog,
            Some("0.144.3".into()),
        )
        .unwrap();

        assert_eq!(
            summary.models,
            vec![
                "gpt-5.6-luna",
                "gpt-5.5",
                "alex/gpt-5.6-luna",
                "alex/gpt-5.5"
            ]
        );
        assert_eq!(read_codex_api_key(&dir).as_deref(), Some("alxk-codex"));
        assert!(codex_config_connected(&dir).unwrap());
        let connected = std::fs::read_to_string(&config_path).unwrap();
        let doc = DocumentMut::from_str(&connected).unwrap();
        assert_eq!(doc["model"].as_str(), Some("alex/gpt-5.6-luna"));
        assert_eq!(doc["model_provider"].as_str(), Some("alexandria"));
        assert_eq!(doc["features"]["hooks"].as_bool(), Some(true));
        assert_eq!(
            doc["model_providers"]["alexandria"]["base_url"].as_str(),
            Some("http://127.0.0.1:4100/v1")
        );
        assert_eq!(
            doc["model_providers"]["alexandria"]["http_headers"]
                .as_inline_table()
                .and_then(|headers| headers.get("x-alexandria-harness"))
                .and_then(|value| value.as_str()),
            Some("codex")
        );
        assert_eq!(
            doc["model_providers"]["alexandria"]["auth"]["command"].as_str(),
            Some("/bin/cat")
        );
        assert_eq!(
            doc["projects"]["/tmp/example"]["trust_level"].as_str(),
            Some("trusted")
        );
        let hooks: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("hooks.json")).unwrap())
                .unwrap();
        assert_eq!(hooks["hooks"]["SessionStart"].as_array().unwrap().len(), 1);
        assert_eq!(hooks["hooks"]["SubagentStart"].as_array().unwrap().len(), 1);
        assert_eq!(hooks["hooks"]["SubagentStop"].as_array().unwrap().len(), 1);
        assert_eq!(hooks["hooks"]["Stop"].as_array().unwrap().len(), 1);
        assert!(summary.extension_path.exists());
        assert_eq!(codex_default_route(&dir).unwrap().as_deref(), Some("alex"));
        let backup = std::fs::read_to_string(dir.join(CODEX_BACKUP_FILE)).unwrap();
        assert!(backup.contains("model = \"gpt-5.4\""));
        assert!(backup.contains("model_provider = \"openai\""));
        assert!(!backup.contains("Alex Proxy"));
        let openai_profile = std::fs::read_to_string(dir.join(CODEX_OPENAI_PROFILE_FILE)).unwrap();
        assert!(openai_profile.contains("codex --profile openai"));
        assert!(openai_profile.contains("model_provider = \"openai\""));
        let alex_profile = std::fs::read_to_string(dir.join(CODEX_ALEX_PROFILE_FILE)).unwrap();
        assert!(alex_profile.contains("codex --profile alex"));
        assert!(alex_profile.contains("model_provider = \"alexandria\""));
        let native_catalog: Value = serde_json::from_str(
            &std::fs::read_to_string(dir.join(CODEX_NATIVE_CATALOG_FILE)).unwrap(),
        )
        .unwrap();
        assert!(native_catalog["models"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| !row["slug"].as_str().unwrap().starts_with("alex/")));

        assert_eq!(set_codex_default_route(&dir, "openai").unwrap(), "openai");
        assert!(codex_config_connected(&dir).unwrap());
        let openai_default = read_codex_config(&config_path).unwrap();
        assert_eq!(openai_default["model_provider"].as_str(), Some("openai"));
        assert_eq!(openai_default["model"].as_str(), Some("gpt-5.6-luna"));
        assert_eq!(
            openai_default["model_catalog_json"].as_str(),
            dir.join(CODEX_NATIVE_CATALOG_FILE).to_str()
        );
        assert_eq!(
            codex_default_route(&dir).unwrap().as_deref(),
            Some("openai")
        );
        assert_eq!(set_codex_default_route(&dir, "alex").unwrap(), "alex");
        let hook_source = std::fs::read_to_string(&summary.extension_path).unwrap();
        assert!(hook_source.contains("/usr/bin/curl"));
        assert!(hook_source.contains("--data-binary @-"));
        let curl_source = std::fs::read_to_string(dir.join(CODEX_HOOK_CURL_FILE)).unwrap();
        assert!(curl_source.contains("http://127.0.0.1:4100/harness-events"));
        assert!(curl_source.contains("Authorization: Bearer alxk-codex"));

        assert!(disconnect_codex_config(&dir).unwrap());
        assert!(!codex_config_connected(&dir).unwrap());
        assert!(read_codex_api_key(&dir).is_none());
        assert!(!dir.join(CODEX_CATALOG_FILE).exists());
        assert!(!dir.join(CODEX_NATIVE_CATALOG_FILE).exists());
        assert!(!summary.extension_path.exists());
        assert!(!dir.join(CODEX_HOOK_CURL_FILE).exists());
        assert_eq!(
            std::fs::read_to_string(dir.join(CODEX_OPENAI_PROFILE_FILE)).unwrap(),
            "# user's existing openai profile\nmodel = \"gpt-5.4\"\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join(CODEX_ALEX_PROFILE_FILE)).unwrap(),
            "# user's existing alex profile\nmodel = \"custom\"\n"
        );
        assert!(dir.join(CODEX_BACKUP_FILE).exists());
        let restored = std::fs::read_to_string(&config_path).unwrap();
        let doc = DocumentMut::from_str(&restored).unwrap();
        assert_eq!(doc["model_provider"].as_str(), Some("openai"));
        assert_eq!(doc["features"]["hooks"].as_bool(), Some(false));
        assert_eq!(doc["model"].as_str(), Some("gpt-5.4"));
        let hooks: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("hooks.json")).unwrap())
                .unwrap();
        assert!(hooks["hooks"]["SessionStart"].is_null());
        assert_eq!(hooks["hooks"]["Stop"].as_array().unwrap().len(), 1);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn codex_catalog_preserves_native_and_adds_all_alexandria_models() {
        let dir = tmpdir("codex-catalog");
        let binary = fake_executable(
            &dir,
            "codex",
            r#"if [ "$1" = "debug" ]; then
  printf '%s\n' '{"models":[{"slug":"gpt-5.6-luna"},{"slug":"gpt-5.4"}]}'
else
  echo codex-cli 0.144.3
fi"#,
        );
        let catalog =
            codex_model_catalog(&binary, &["gpt-5.6-luna".into(), "claude-opus-4-8".into()])
                .await
                .unwrap();
        let slugs: Vec<&str> = catalog["models"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|row| row["slug"].as_str())
            .collect();
        assert_eq!(
            slugs,
            vec![
                "gpt-5.6-luna",
                "gpt-5.4",
                "alex/gpt-5.6-luna",
                "alex/claude-opus-4-8"
            ]
        );
        assert_eq!(catalog["models"][2]["display_name"], "alex/gpt-5.6-luna");
        assert_eq!(
            catalog["models"][3]["description"],
            "Routed through Alex: claude-opus-4-8"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn codex_catalog_timeout_returns_an_error() {
        let dir = tmpdir("codex-catalog-timeout");
        let binary = fake_executable(&dir, "codex-slow-catalog", "sleep 2");
        let started = std::time::Instant::now();

        let err = codex_model_catalog_with_timeout(
            &binary,
            &["gpt-5.6-luna".into()],
            Duration::from_millis(100),
        )
        .await
        .unwrap_err();

        assert!(err
            .to_string()
            .contains("timed out reading bundled model catalog"));
        assert!(started.elapsed() < Duration::from_secs(1));
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
            "openrouter/anthropic/claude-opus-4.8".into(),
            "openrouter/meta-llama/llama-4:free".into(),
            "exo/mlx-community/Meta-Llama-3.1-8B-Instruct-4bit".into(),
            "alex/mlx-community/Meta-Llama-3.1-8B-Instruct-4bit".into(),
            "alexandria/openrouter/anthropic/claude-opus-4.8".into(),
            "openrouter/../secret".into(),
            "openrouter/provider/model with space".into(),
        ];
        assert_eq!(
            filter_model_ids(ids),
            vec![
                "claude-opus-4-8",
                "gpt-5.5",
                "o3-mini",
                "codex-mini",
                "gemini-2.5-flash",
                "openrouter/anthropic/claude-opus-4.8",
                "openrouter/meta-llama/llama-4:free",
                "exo/mlx-community/Meta-Llama-3.1-8B-Instruct-4bit",
                "alex/mlx-community/Meta-Llama-3.1-8B-Instruct-4bit"
            ]
        );
    }

    #[test]
    fn model_id_filter_keeps_kimi_provider_ids_and_drops_alexandria_alias() {
        // The daemon advertises Kimi Code models with a `kimi/` prefix (plus an
        // `alexandria/kimi/...` duplicate for non-Claude harnesses). The prefixed
        // ids must survive so a newly-added Kimi provider reaches connected
        // harnesses; the duplicate alias must not, or `short_alex_model_ids`
        // would emit two `alex/kimi/k3` entries. Path-traversal ids stay barred.
        let ids = vec![
            "claude-opus-4-8".into(),
            "kimi/k3".into(),
            "kimi/kimi-for-coding".into(),
            "kimi/kimi-for-coding-highspeed".into(),
            "alexandria/kimi/k3".into(),
            "kimi/../secret".into(),
        ];
        assert_eq!(
            filter_model_ids(ids),
            vec![
                "claude-opus-4-8",
                "kimi/k3",
                "kimi/kimi-for-coding",
                "kimi/kimi-for-coding-highspeed",
            ]
        );
        // Every harness writer normalizes them to the `alex/kimi/...` ids that
        // route back through the proxy to the Kimi provider.
        assert_eq!(
            short_alex_model_ids(vec!["kimi/k3".into()]),
            vec!["alex/kimi/k3"]
        );
    }

    #[cfg(unix)]
    #[test]
    fn pi_connection_carries_kimi_provider_models() {
        let dir = tmpdir("pi-kimi-models");
        // Post-filter model list as `fetch_models` hands it to the writer: bare
        // Alexandria ids plus Kimi's provider-prefixed ids.
        let summary = write_pi_connection(
            dir.clone(),
            "http://127.0.0.1:4100".into(),
            "rk-pi".into(),
            "alxk-pi".into(),
            vec![
                "claude-opus-4-8".into(),
                "kimi/k3".into(),
                "kimi/kimi-for-coding".into(),
            ],
            Some("0.80.0".into()),
        )
        .unwrap();
        assert!(summary.models.contains(&"alex/kimi/k3".to_string()));
        let written = read_pi_model_ids(&dir);
        assert!(written.contains(&"alex/kimi/k3".to_string()));
        assert!(written.contains(&"alex/kimi/kimi-for-coding".to_string()));
        assert!(written.contains(&"alex/claude-opus-4-8".to_string()));
    }

    #[cfg(unix)]
    #[test]
    fn grok_connection_carries_kimi_provider_models() {
        let dir = tmpdir("grok-kimi-models");
        std::fs::create_dir_all(dir.join("hooks")).unwrap();
        let summary = write_grok_connection(
            dir.clone(),
            "http://127.0.0.1:4100".into(),
            "rk-grok".into(),
            "alxk-grok".into(),
            vec!["claude-opus-4-8".into(), "kimi/k3".into()],
            Some("1.0.0".into()),
        )
        .unwrap();
        assert!(summary.models.contains(&"alex/kimi/k3".to_string()));
        assert!(read_grok_model_ids(&dir).contains(&"alex/kimi/k3".to_string()));
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
    fn detection_cache_key_includes_resolved_path_mtime_and_size() {
        let first_time = SystemTime::UNIX_EPOCH + Duration::from_secs(10);
        let second_time = SystemTime::UNIX_EPOCH + Duration::from_secs(11);
        let first =
            detection_cache_key(PathBuf::from("/tmp/bin/claude"), Some(first_time), Some(10));
        assert_eq!(
            first,
            detection_cache_key(PathBuf::from("/tmp/bin/claude"), Some(first_time), Some(10))
        );
        assert_ne!(
            first,
            detection_cache_key(
                PathBuf::from("/tmp/bin/claude"),
                Some(second_time),
                Some(10)
            )
        );
        assert_ne!(
            first,
            detection_cache_key(PathBuf::from("/tmp/bin/claude"), Some(first_time), Some(11))
        );
        assert_ne!(
            first,
            detection_cache_key(PathBuf::from("/tmp/bin/codex"), Some(first_time), Some(10))
        );
    }

    #[test]
    fn preferred_claude_default_selects_sonnet_over_live_haiku() {
        let models = vec![
            "claude-alex/claude-haiku-4-5".to_string(),
            "claude-alex/claude-opus-4-8".to_string(),
            "claude-alex/claude-sonnet-5".to_string(),
        ];
        assert_eq!(
            preferred_claude_model(&models),
            "claude-alex/claude-sonnet-5"
        );
    }

    #[test]
    fn gpt_5_6_pi_settings_match_codex_catalog() {
        let sol = pi_model_config("gpt-5.6-sol");
        assert_eq!(sol["name"], "GPT-5.6 Sol");
        assert_eq!(sol["contextWindow"], 372000);
        assert_eq!(sol["maxTokens"], 128000);
        assert_eq!(sol["thinkingLevelMap"]["off"], Value::Null);
        assert_eq!(sol["thinkingLevelMap"]["minimal"], "low");
        assert_eq!(sol["thinkingLevelMap"]["xhigh"], "xhigh");
        assert_eq!(sol["compat"]["forceAdaptiveThinking"], true);
        assert_eq!(sol["cost"]["input"], 5.0);
        assert_eq!(sol["cost"]["output"], 30.0);

        let terra = pi_model_config("gpt-5.6-terra");
        assert_eq!(terra["cost"]["input"], 2.5);
        assert_eq!(terra["cost"]["output"], 15.0);

        let luna = pi_model_config("gpt-5.6-luna");
        assert_eq!(luna["cost"]["input"], 1.0);
        assert_eq!(luna["cost"]["output"], 6.0);
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

    #[cfg(unix)]
    #[test]
    fn claude_tool_capture_profile_and_toggle_are_idempotent() {
        let dir = tmpdir("claude-tool-capture");
        write_claude_connection_with_capture(
            dir.clone(),
            "http://127.0.0.1:4100".into(),
            "rk".into(),
            "key".into(),
            model_ids(),
            None,
            true,
        )
        .unwrap();
        let profile_path = dir.join(CLAUDE_PROFILE_FILE);
        let profile = read_json_object(&profile_path).unwrap();
        for event in ["PreToolUse", "PostToolUse", "PostToolUseFailure"] {
            assert_eq!(profile["hooks"][event].as_array().unwrap().len(), 1);
        }
        set_claude_tool_capture(&dir, "http://127.0.0.1:4100", false).unwrap();
        let profile = read_json_object(&profile_path).unwrap();
        assert!(profile["hooks"]["PreToolUse"].is_null());
        set_claude_tool_capture(&dir, "http://127.0.0.1:4100", true).unwrap();
        set_claude_tool_capture(&dir, "http://127.0.0.1:4100", true).unwrap();
        let profile = read_json_object(&profile_path).unwrap();
        assert_eq!(profile["hooks"]["PreToolUse"].as_array().unwrap().len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn codex_tool_capture_hooks_toggle_and_disconnect_clean_tool_hooks() {
        let dir = tmpdir("codex-tool-capture");
        let catalog = json!({"models": [{"slug": "gpt-5.5", "display_name": "GPT-5.5"}]});
        write_codex_connection_with_capture(
            dir.clone(),
            "http://127.0.0.1:4100".into(),
            "rk".into(),
            "key".into(),
            catalog,
            None,
            true,
        )
        .unwrap();
        let hooks_path = dir.join("hooks.json");
        let hooks: Value =
            serde_json::from_str(&std::fs::read_to_string(&hooks_path).unwrap()).unwrap();
        assert!(hooks["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .contains(CODEX_TOOL_HOOK_FILE));
        set_codex_tool_capture(&dir, "http://127.0.0.1:4100", false).unwrap();
        let hooks: Value =
            serde_json::from_str(&std::fs::read_to_string(&hooks_path).unwrap()).unwrap();
        assert!(hooks["hooks"]["PreToolUse"].is_null());
        set_codex_tool_capture(&dir, "http://127.0.0.1:4100", true).unwrap();
        assert!(disconnect_codex_config(&dir).unwrap());
        let hooks: Value =
            serde_json::from_str(&std::fs::read_to_string(&hooks_path).unwrap()).unwrap();
        assert!(hooks["hooks"].is_null());
    }

    #[test]
    fn amp_tool_capture_source_has_independent_tool_event_posts() {
        let source = amp_plugin_source(
            "http://127.0.0.1:4100",
            "http://127.0.0.1:4100/harness-events",
            "http://127.0.0.1:4100/tool-events",
            Path::new("/tmp/key"),
            Path::new("/tmp/events"),
            true,
        )
        .unwrap();
        assert!(source.contains("const TOOL_EVENT_URL = \"http://127.0.0.1:4100/tool-events\""));
        assert!(source.contains("const CAPTURE_ENABLED = true"));
        assert!(source.contains("postToolEvent({ phase: 'start'"));
        assert!(source.contains("postToolEvent({ phase: 'end'"));
    }

    #[test]
    fn npm_install_commands_are_catalog_driven_and_pin_versions() {
        let pi = install_command(spec_by_name("pi").unwrap(), Some("0.80.3")).unwrap();
        assert_eq!(pi.program, "npm");
        assert_eq!(
            pi.args,
            ["install", "-g", "@earendil-works/pi-coding-agent@0.80.3"]
        );
        let codex = install_command(spec_by_name("codex").unwrap(), None).unwrap();
        assert_eq!(codex.args, ["install", "-g", "@openai/codex"]);
        assert!(install_command(spec_by_name("claude").unwrap(), None).is_none());
        assert!(version_matches("v0.80.3", "0.80.3"));
        assert!(!version_matches("0.80.2", "0.80.3"));
    }
}
