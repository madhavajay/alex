use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use alexandria_auth::now_ms;
use alexandria_core::route_model;
use alexandria_store::Store;
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::ui;

const DEFAULT_PROMPT: &str = "Reply with the single line: alexandria-e2e-ok";
const DEFAULT_TIMEOUT_SECS: u64 = 300;
const DEFAULT_DOCKER_IMAGE: &str = "node:22-bookworm-slim";
const CATALOG_JSON: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../config/harnesses.json"
));

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HarnessKind {
    Claude,
    Codex,
    Grok,
}

#[derive(Debug, Clone, Copy)]
struct RunnableHarness {
    name: &'static str,
    kind: HarnessKind,
    default_package: Option<&'static str>,
    default_model: &'static str,
}

const RUNNABLE_HARNESSES: &[RunnableHarness] = &[
    RunnableHarness {
        name: "claude",
        kind: HarnessKind::Claude,
        default_package: Some("@anthropic-ai/claude-code"),
        default_model: "claude-haiku-4-5",
    },
    RunnableHarness {
        name: "codex",
        kind: HarnessKind::Codex,
        default_package: Some("@openai/codex"),
        default_model: "gpt-5.5",
    },
    RunnableHarness {
        name: "grok-build",
        kind: HarnessKind::Grok,
        default_package: None,
        default_model: "gpt-5.5",
    },
];

#[derive(Debug, Deserialize)]
struct HarnessCatalog {
    harnesses: BTreeMap<String, CatalogHarness>,
}

#[derive(Debug, Clone, Deserialize)]
struct CatalogHarness {
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default)]
    default_model: Option<String>,
    #[serde(default)]
    default_version: Option<String>,
    #[serde(default)]
    proxy_supported: Option<bool>,
    #[serde(default)]
    proxy_mode: Option<String>,
    #[serde(default)]
    notes: Option<String>,
}

#[derive(Debug)]
pub struct RunOptions {
    pub harness: String,
    pub model: Option<String>,
    pub prompt: Option<String>,
    pub package_tarball: Option<PathBuf>,
    pub docker_image: String,
    pub container_base_url: String,
    pub timeout_secs: u64,
    pub no_trace_check: bool,
    pub local_key: String,
    pub data_dir: PathBuf,
}

#[derive(Debug, Serialize)]
pub struct HarnessListRow {
    pub name: String,
    pub aliases: Vec<String>,
    pub default_model: Option<String>,
    pub proxy_supported: bool,
    pub proxy_mode: Option<String>,
    pub runner: bool,
    pub default_package: Option<String>,
    pub default_version: Option<String>,
    pub cached_tarball: Option<String>,
    pub cached_tarball_exists: bool,
    pub notes: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CaptureCheck {
    pub trace_count: usize,
    pub checked_trace_id: Option<String>,
    pub complete: bool,
    pub missing: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct PackSummary {
    pub package: String,
    pub version: String,
    pub tarball: String,
    pub reused: bool,
}

#[derive(Debug, Serialize)]
pub struct RunSummary {
    pub harness: String,
    pub model: String,
    pub routed_model: String,
    pub docker_image: String,
    pub container_base_url: String,
    pub session_dir: String,
    pub command: Vec<String>,
    pub status: Option<i32>,
    pub stdout_path: String,
    pub stderr_path: String,
    pub capture: CaptureCheck,
}

pub fn default_timeout_secs() -> u64 {
    DEFAULT_TIMEOUT_SECS
}

pub fn default_docker_image() -> &'static str {
    DEFAULT_DOCKER_IMAGE
}

pub fn default_container_base_url(host: &str, port: u16) -> String {
    let host = if matches!(host, "127.0.0.1" | "localhost" | "0.0.0.0" | "::1") {
        "host.docker.internal"
    } else {
        host
    };
    format!("http://{host}:{port}")
}

pub fn list_harnesses(data_dir: &Path) -> Result<Vec<HarnessListRow>> {
    let catalog = catalog()?;
    Ok(catalog
        .harnesses
        .iter()
        .map(|(name, h)| {
            let runner = runnable_by_name(name).is_some();
            let default_package = runnable_by_name(name)
                .and_then(|r| r.default_package)
                .map(String::from);
            let default_version = h
                .default_version
                .clone()
                .or_else(|| default_package.as_ref().map(|_| "latest".to_string()));
            let cached_tarball = default_package
                .as_deref()
                .zip(default_version.as_deref())
                .map(|(package, version)| {
                    package_cache_path(data_dir, package, version)
                        .to_string_lossy()
                        .to_string()
                });
            let cached_tarball_exists = cached_tarball
                .as_deref()
                .map(Path::new)
                .map(Path::exists)
                .unwrap_or(false);
            HarnessListRow {
                name: name.clone(),
                aliases: h.aliases.clone(),
                default_model: h.default_model.clone(),
                proxy_supported: h.proxy_supported.unwrap_or(false),
                proxy_mode: h.proxy_mode.clone(),
                runner,
                default_package,
                default_version,
                cached_tarball,
                cached_tarball_exists,
                notes: h.notes.clone(),
            }
        })
        .collect())
}

pub fn pack_target(
    data_dir: &Path,
    target: &str,
    version: Option<&str>,
    force: bool,
) -> Result<PackSummary> {
    let (package, version) = package_and_version_for_target(target, version)?;
    pack_npm_package(data_dir, &package, &version, force)
}

fn pack_npm_package(
    data_dir: &Path,
    package: &str,
    version: &str,
    force: bool,
) -> Result<PackSummary> {
    let package = package.trim();
    if package.is_empty() {
        bail!("package is required");
    }
    let version = version.trim();
    if version.is_empty() {
        bail!("version is required");
    }
    let out_dir = data_dir.join("harness-packages");
    std::fs::create_dir_all(&out_dir)?;
    let target = package_cache_path(data_dir, package, version);
    if target.is_file() && !force {
        return Ok(PackSummary {
            package: package.to_string(),
            version: version.to_string(),
            tarball: target.to_string_lossy().to_string(),
            reused: true,
        });
    }
    if target.exists() {
        std::fs::remove_file(&target)?;
    }

    let package_ref = if version == "latest" {
        format!("{package}@latest")
    } else {
        format!("{package}@{version}")
    };
    let output = Command::new("npm")
        .arg("pack")
        .arg(&package_ref)
        .arg("--pack-destination")
        .arg(&out_dir)
        .output()
        .with_context(|| format!("running npm pack {package_ref}"))?;
    if !output.status.success() {
        bail!(
            "npm pack {package_ref} failed with {}: {}",
            status_label(output.status),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let filename = String::from_utf8_lossy(&output.stdout)
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .ok_or_else(|| anyhow!("npm pack did not report an artifact filename"))?
        .to_string();
    let artifact = out_dir.join(filename);
    if !artifact.is_file() {
        bail!(
            "npm pack finished but artifact was not created at {}",
            artifact.display()
        );
    }
    if artifact != target {
        std::fs::copy(&artifact, &target)?;
    }
    Ok(PackSummary {
        package: package.to_string(),
        version: version.to_string(),
        tarball: target.to_string_lossy().to_string(),
        reused: false,
    })
}

pub fn run_harness(opts: RunOptions) -> Result<RunSummary> {
    let (canonical_name, catalog_harness) = resolve_harness(&opts.harness)?;
    let spec = runnable_by_name(&canonical_name).ok_or_else(|| {
        anyhow!("{canonical_name} is in the harness catalog but has no Docker smoke runner yet")
    })?;
    let model = opts
        .model
        .clone()
        .or_else(|| catalog_harness.default_model.clone())
        .unwrap_or_else(|| spec.default_model.to_string());
    let (_, routed_model) = route_model(&model);
    let prompt = opts.prompt.as_deref().unwrap_or(DEFAULT_PROMPT);
    let started_ms = now_ms();
    let nonce: u32 = rand::Rng::gen(&mut rand::thread_rng());
    let session_id = format!("alexandria-e2e-{}-{started_ms}-{nonce:08x}", canonical_name);
    let session_dir = opts.data_dir.join("harness-e2e").join(&session_id);
    let work_dir = session_dir.join("work");
    std::fs::create_dir_all(&work_dir)?;

    let tarball: Option<PathBuf> = if let Some(path) = opts.package_tarball.clone() {
        Some(path)
    } else if let Some(package) = spec.default_package {
        let version = catalog_harness
            .default_version
            .as_deref()
            .unwrap_or("latest");
        let packed = pack_npm_package(&opts.data_dir, package, version, false)?;
        Some(PathBuf::from(packed.tarball))
    } else if spec.kind == HarnessKind::Grok {
        None
    } else {
        bail!("{canonical_name} has no default npm package; pass --package-tarball");
    };
    if let Some(t) = &tarball {
        if !t.is_file() {
            bail!("package tarball does not exist: {}", t.display());
        }
    }

    let script = docker_script(spec.kind, &model)?;
    let script_path = session_dir.join("run.sh");
    std::fs::write(&script_path, script)?;

    let mut command = vec![
        "docker".to_string(),
        "run".to_string(),
        "--rm".to_string(),
        "--add-host".to_string(),
        "host.docker.internal:host-gateway".to_string(),
        "-v".to_string(),
        format!("{}:/out", session_dir.display()),
        "-w".to_string(),
        "/workspace".to_string(),
        "-e".to_string(),
        format!("ALEXANDRIA_E2E_PROMPT={prompt}"),
        "-e".to_string(),
        format!("ALEXANDRIA_E2E_MODEL={model}"),
        "-e".to_string(),
        format!("ALEXANDRIA_E2E_SESSION={session_id}"),
    ];
    if let Some(t) = &tarball {
        let parent = t
            .parent()
            .ok_or_else(|| anyhow!("tarball has no parent: {}", t.display()))?;
        let name = t
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| anyhow!("tarball filename is not utf8: {}", t.display()))?;
        command.push("-v".to_string());
        command.push(format!("{}:/pkg:ro", parent.display()));
        command.push("-e".to_string());
        command.push(format!("ALEXANDRIA_E2E_TARBALL=/pkg/{name}"));
    }
    command.extend(docker_env(
        spec.kind,
        &opts.container_base_url,
        &opts.local_key,
        &model,
    ));
    command.push(opts.docker_image.clone());
    command.push("bash".to_string());
    command.push("/out/run.sh".to_string());

    let stdout_path = session_dir.join("docker.stdout.log");
    let stderr_path = session_dir.join("docker.stderr.log");
    let outcome = run_with_timeout(&command, Duration::from_secs(opts.timeout_secs))?;
    std::fs::write(&stdout_path, &outcome.stdout)?;
    std::fs::write(&stderr_path, &outcome.stderr)?;
    if outcome.timed_out {
        bail!(
            "{} docker run timed out after {}s; stdout={} stderr={}",
            canonical_name,
            opts.timeout_secs,
            stdout_path.display(),
            stderr_path.display()
        );
    }
    if !outcome.status.success() {
        bail!(
            "{} docker run exited with {}; stdout={} stderr={}",
            canonical_name,
            status_label(outcome.status),
            stdout_path.display(),
            stderr_path.display()
        );
    }

    let capture = if opts.no_trace_check {
        CaptureCheck {
            trace_count: 0,
            checked_trace_id: None,
            complete: false,
            missing: vec![],
        }
    } else {
        verify_capture(&opts.data_dir, started_ms, &routed_model)?
    };
    if !opts.no_trace_check && !capture.complete {
        bail!(
            "{} completed but Alexandria capture is incomplete for model '{}': missing {}; stdout={} stderr={}",
            canonical_name,
            routed_model,
            capture.missing.join(", "),
            stdout_path.display(),
            stderr_path.display()
        );
    }

    Ok(RunSummary {
        harness: canonical_name,
        model,
        routed_model,
        docker_image: opts.docker_image,
        container_base_url: opts.container_base_url,
        session_dir: session_dir.to_string_lossy().to_string(),
        command: redact_command(&command),
        status: outcome.status.code(),
        stdout_path: stdout_path.to_string_lossy().to_string(),
        stderr_path: stderr_path.to_string_lossy().to_string(),
        capture,
    })
}

fn catalog() -> Result<HarnessCatalog> {
    serde_json::from_str(CATALOG_JSON).context("parsing config/harnesses.json")
}

fn resolve_harness(name: &str) -> Result<(String, CatalogHarness)> {
    let catalog = catalog()?;
    catalog
        .harnesses
        .into_iter()
        .find(|(key, h)| key == name || h.aliases.iter().any(|alias| alias == name))
        .ok_or_else(|| anyhow!("unknown harness '{name}'"))
}

fn runnable_by_name(name: &str) -> Option<&'static RunnableHarness> {
    RUNNABLE_HARNESSES.iter().find(|h| h.name == name)
}

fn package_and_version_for_target(target: &str, version: Option<&str>) -> Result<(String, String)> {
    let (canonical, h) = match resolve_harness(target) {
        Ok(found) => found,
        Err(_) => {
            return Ok((
                target.to_string(),
                version
                    .filter(|v| !v.trim().is_empty())
                    .unwrap_or("latest")
                    .to_string(),
            ))
        }
    };
    let runnable = runnable_by_name(&canonical)
        .ok_or_else(|| anyhow!("{canonical} has no default npm package mapping"))?;
    let package = runnable
        .default_package
        .ok_or_else(|| anyhow!("{canonical} has no default npm package mapping"))?;
    let version = version
        .filter(|v| !v.trim().is_empty())
        .map(String::from)
        .or(h.default_version)
        .unwrap_or_else(|| "latest".to_string());
    Ok((package.to_string(), version))
}

fn package_cache_path(data_dir: &Path, package: &str, version: &str) -> PathBuf {
    data_dir
        .join("harness-packages")
        .join(package_cache_filename(package, version))
}

fn package_cache_filename(package: &str, version: &str) -> String {
    let name = package
        .strip_prefix('@')
        .unwrap_or(package)
        .replace('/', "-");
    format!(
        "{}-{}.tgz",
        sanitize_cache_part(&name),
        sanitize_cache_part(version)
    )
}

fn sanitize_cache_part(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '-') {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

fn docker_env(kind: HarnessKind, base_url: &str, local_key: &str, model: &str) -> Vec<String> {
    let openai_base = format!("{}/v1", base_url.trim_end_matches('/'));
    let mut env = Vec::new();
    match kind {
        HarnessKind::Claude => {
            for (key, value) in [
                ("ANTHROPIC_BASE_URL", base_url.to_string()),
                ("ANTHROPIC_API_KEY", local_key.to_string()),
                ("ANTHROPIC_MODEL", model.to_string()),
                ("ANTHROPIC_DEFAULT_SONNET_MODEL", model.to_string()),
                ("ANTHROPIC_DEFAULT_OPUS_MODEL", model.to_string()),
                ("ANTHROPIC_DEFAULT_HAIKU_MODEL", model.to_string()),
                ("CLAUDE_CODE_SUBAGENT_MODEL", model.to_string()),
                ("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1".to_string()),
                ("IS_SANDBOX", "1".to_string()),
            ] {
                env.push("-e".to_string());
                env.push(format!("{key}={value}"));
            }
        }
        HarnessKind::Codex => {
            for (key, value) in [
                ("OPENAI_BASE_URL", openai_base.clone()),
                ("OPENAI_API_KEY", local_key.to_string()),
                ("CODEX_HOME", "/tmp/alexandria-codex-home".to_string()),
            ] {
                env.push("-e".to_string());
                env.push(format!("{key}={value}"));
            }
        }
        HarnessKind::Grok => {
            for (key, value) in [
                ("OPENAI_BASE_URL", openai_base.clone()),
                ("OPENAI_API_KEY", local_key.to_string()),
                ("XAI_API_KEY", local_key.to_string()),
                ("GROK_MODELS_BASE_URL", openai_base.clone()),
                ("GROK_MODELS_LIST_URL", format!("{openai_base}/models")),
            ] {
                env.push("-e".to_string());
                env.push(format!("{key}={value}"));
            }
        }
    }
    env
}

fn docker_script(kind: HarnessKind, model: &str) -> Result<String> {
    let escaped_model = shell_quote(model);
    let script = match kind {
        HarnessKind::Claude => format!(
            r#"set -euo pipefail
mkdir -p /workspace /out/logs /tmp/alexandria-claude-config
npm install -g "$ALEXANDRIA_E2E_TARBALL" > /out/logs/npm-install.log 2>&1
claude --version > /out/logs/version.txt 2>&1
export CLAUDE_CONFIG_DIR=/tmp/alexandria-claude-config
claude --verbose --output-format=stream-json --print -- "$ALEXANDRIA_E2E_PROMPT" > /out/harness.stdout.log 2> /out/harness.stderr.log
"#
        ),
        HarnessKind::Codex => format!(
            r#"set -euo pipefail
mkdir -p /workspace /out/logs "$CODEX_HOME"
npm install -g "$ALEXANDRIA_E2E_TARBALL" > /out/logs/npm-install.log 2>&1
codex --version > /out/logs/version.txt 2>&1 || true
cat > "$CODEX_HOME/auth.json" <<EOF
{{"OPENAI_API_KEY":"$OPENAI_API_KEY"}}
EOF
cat > "$CODEX_HOME/config.toml" <<EOF
model_provider = "alexandria"

[model_providers.alexandria]
name = "Alexandria Proxy"
base_url = "$OPENAI_BASE_URL"
env_key = "OPENAI_API_KEY"
wire_api = "responses"
requires_openai_auth = false
supports_websockets = false
EOF
codex exec --dangerously-bypass-approvals-and-sandbox --skip-git-repo-check --model {escaped_model} --json -- "$ALEXANDRIA_E2E_PROMPT" > /out/harness.stdout.log 2> /out/harness.stderr.log
"#
        ),
        HarnessKind::Grok => format!(
            r#"set -euo pipefail
mkdir -p /workspace /out/logs
if [ -n "${{ALEXANDRIA_E2E_TARBALL:-}}" ]; then
  npm install -g "$ALEXANDRIA_E2E_TARBALL" > /out/logs/npm-install.log 2>&1
else
  apt-get update -qq > /out/logs/apt.log 2>&1
  apt-get install -y -qq curl ca-certificates >> /out/logs/apt.log 2>&1
  curl -fsSL https://x.ai/cli/install.sh | bash > /out/logs/grok-install.log 2>&1
fi
export PATH="$HOME/.grok/bin:$HOME/.local/bin:/usr/local/bin:$PATH"
GROK_CLI="$(command -v agent || command -v grok || true)"
test -n "$GROK_CLI"
"$GROK_CLI" --version > /out/logs/version.txt 2>&1 || true
"$GROK_CLI" -p "$ALEXANDRIA_E2E_PROMPT" --model {escaped_model} --output-format streaming-json --permission-mode bypassPermissions > /out/harness.stdout.log 2> /out/harness.stderr.log
"#
        ),
    };
    Ok(script)
}

struct CommandOutcome {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    timed_out: bool,
}

fn run_with_timeout(argv: &[String], timeout: Duration) -> Result<CommandOutcome> {
    let Some((program, args)) = argv.split_first() else {
        bail!("empty docker command");
    };
    let mut child = Command::new(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("spawning {}", shell_display(argv)))?;
    let start = Instant::now();
    loop {
        if child.try_wait()?.is_some() {
            let output = child.wait_with_output()?;
            return Ok(CommandOutcome {
                status: output.status,
                stdout: output.stdout,
                stderr: output.stderr,
                timed_out: false,
            });
        }
        if start.elapsed() > timeout {
            child.kill().ok();
            let output = child.wait_with_output()?;
            return Ok(CommandOutcome {
                status: output.status,
                stdout: output.stdout,
                stderr: output.stderr,
                timed_out: true,
            });
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn verify_capture(data_dir: &Path, started_ms: i64, routed_model: &str) -> Result<CaptureCheck> {
    let store = Store::open(data_dir.to_path_buf())?;
    let rows = store.list_traces(100, None, Some(routed_model))?;
    let mut matches = rows
        .into_iter()
        .filter(|r| r["ts_request_ms"].as_i64().unwrap_or(0) >= started_ms)
        .collect::<Vec<_>>();
    matches.sort_by_key(|r| r["ts_request_ms"].as_i64().unwrap_or(0));
    let Some(trace) = matches.last() else {
        return Ok(CaptureCheck {
            trace_count: 0,
            checked_trace_id: None,
            complete: false,
            missing: vec!["trace row".to_string()],
        });
    };

    let mut missing = Vec::new();
    for key in [
        "status",
        "harness",
        "client_format",
        "upstream_provider",
        "upstream_format",
        "requested_model",
        "routed_model",
        "req_headers_json",
        "resp_headers_json",
        "req_body_path",
        "resp_body_path",
    ] {
        if trace.get(key).is_none_or(|v| v.is_null()) {
            missing.push(key.to_string());
        }
    }
    for key in ["req_body_path", "resp_body_path"] {
        if let Some(path) = trace[key].as_str() {
            if !Path::new(path).is_file() {
                missing.push(format!("{key} file"));
            }
        }
    }
    let usage_present =
        trace["input_tokens"].as_i64().is_some() || trace["output_tokens"].as_i64().is_some();
    if !usage_present {
        missing.push("usage tokens".to_string());
    }

    Ok(CaptureCheck {
        trace_count: matches.len(),
        checked_trace_id: trace["id"].as_str().map(String::from),
        complete: missing.is_empty(),
        missing,
    })
}

fn status_label(status: ExitStatus) -> String {
    status
        .code()
        .map(|c| format!("exit code {c}"))
        .unwrap_or_else(|| "signal".to_string())
}

fn shell_display(argv: &[String]) -> String {
    argv.iter()
        .map(|arg| {
            if arg
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_./:=+".contains(c))
            {
                arg.clone()
            } else {
                shell_quote(arg)
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn redact_command(argv: &[String]) -> Vec<String> {
    argv.iter()
        .map(|arg| {
            if arg.starts_with("ANTHROPIC_API_KEY=")
                || arg.starts_with("OPENAI_API_KEY=")
                || arg.starts_with("XAI_API_KEY=")
            {
                let (key, _) = arg.split_once('=').unwrap_or((arg, ""));
                format!("{key}=<redacted>")
            } else {
                arg.clone()
            }
        })
        .collect()
}

pub fn print_run_summary(summary: &RunSummary, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(summary)?);
        return Ok(());
    }
    println!("{}", ui::section("harness run"));
    println!("harness:      {}", ui::bold(&summary.harness));
    println!("model:        {}", ui::turquoise(&summary.model));
    println!("routed model: {}", ui::turquoise(&summary.routed_model));
    let status = match summary.status {
        Some(0) => ui::green("0"),
        Some(s) => ui::red(&s.to_string()),
        None => ui::red("-"),
    };
    println!("status:       {status}");
    println!("traces:       {}", summary.capture.trace_count);
    println!(
        "capture:      {}",
        if summary.capture.complete {
            ui::green("complete")
        } else {
            ui::red("incomplete")
        }
    );
    if !summary.capture.missing.is_empty() {
        println!(
            "missing:      {}",
            ui::red(&summary.capture.missing.join(", "))
        );
    }
    println!("session dir:  {}", ui::sand(&summary.session_dir));
    println!("stdout:       {}", ui::sand(&summary.stdout_path));
    println!("stderr:       {}", ui::sand(&summary.stderr_path));
    println!(
        "command:      {}",
        ui::dim(&shell_display(&summary.command))
    );
    Ok(())
}

pub fn print_harnesses(data_dir: &Path, json: bool) -> Result<()> {
    let rows = list_harnesses(data_dir)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    println!("{}", ui::section("harnesses"));
    println!(
        "{} {} {} {} {} {}",
        ui::pad_right(&ui::column_header("harness"), 20),
        ui::pad_right(&ui::column_header("default model"), 24),
        ui::pad_right(&ui::column_header("runner"), 7),
        ui::pad_right(&ui::column_header("tarball"), 9),
        ui::pad_right(&ui::column_header("package"), 24),
        ui::column_header("notes")
    );
    for row in rows {
        let tarball = match (row.cached_tarball.as_deref(), row.cached_tarball_exists) {
            (Some(_), true) => ui::green("cached"),
            (Some(_), false) => ui::yellow("missing"),
            (None, _) => "-".to_string(),
        };
        let runner = if row.runner {
            ui::green("yes")
        } else {
            ui::dim("no")
        };
        let package = row.default_package.as_deref().unwrap_or("-");
        let version = row.default_version.as_deref().unwrap_or("latest");
        let package_label = if package == "-" {
            "-".to_string()
        } else {
            format!("{package}@{version}")
        };
        println!(
            "{} {} {} {} {} {}",
            ui::pad_right(&ui::bold(&row.name), 20),
            ui::pad_right(
                &ui::turquoise(row.default_model.as_deref().unwrap_or("-")),
                24
            ),
            ui::pad_right(&runner, 7),
            ui::pad_right(&tarball, 9),
            ui::pad_right(&ui::sand(&package_label), 24),
            ui::dim(row.notes.as_deref().unwrap_or(""))
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_aliases() {
        assert_eq!(resolve_harness("claude-code").unwrap().0, "claude");
        assert_eq!(resolve_harness("grok").unwrap().0, "grok-build");
        assert!(resolve_harness("missing").is_err());
    }

    #[test]
    fn container_base_url_uses_host_gateway_for_loopback() {
        assert_eq!(
            default_container_base_url("127.0.0.1", 4100),
            "http://host.docker.internal:4100"
        );
        assert_eq!(
            default_container_base_url("10.0.0.2", 4100),
            "http://10.0.0.2:4100"
        );
    }

    #[test]
    fn codex_script_contains_responses_provider() {
        let script = docker_script(HarnessKind::Codex, "gpt-5.5").unwrap();
        assert!(script.contains("wire_api = \"responses\""));
        assert!(script.contains("codex exec"));
    }

    #[test]
    fn package_cache_filename_is_stable() {
        assert_eq!(
            package_cache_filename("@anthropic-ai/claude-code", "2.1.202"),
            "anthropic-ai-claude-code-2.1.202.tgz"
        );
    }

    #[test]
    fn redacts_proxy_keys_from_summary_command() {
        let redacted = redact_command(&[
            "-e".to_string(),
            "OPENAI_API_KEY=secret".to_string(),
            "ANTHROPIC_API_KEY=secret".to_string(),
        ]);
        assert_eq!(redacted[1], "OPENAI_API_KEY=<redacted>");
        assert_eq!(redacted[2], "ANTHROPIC_API_KEY=<redacted>");
    }
}
