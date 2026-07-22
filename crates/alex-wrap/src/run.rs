//! Start reverse wrap + spawn harness binary; wait for exit.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use tokio::process::Command;

use crate::capture::{capture_dir_for, CaptureLog};
use crate::catalog::{expand_user_path, WrapHarness};
use crate::reverse::{ReverseOptions, ReverseWrap};
use crate::{load_catalog, plan_for};

const REMOTE_TRACE_ENV_VARS: &[&str] = &[
    "ALEX_TRACE_URL",
    "ALEX_TRACE_KEY",
    "ALEX_TRACE_KEY_FILE",
    "ALEX_TRACE_ALLOW_INSECURE_HTTP",
];

fn alex_wrap_reverse_opts(harness: &WrapHarness) -> ReverseOptions {
    ReverseOptions {
        inject: harness.reverse_inject.clone(),
    }
}

#[derive(Debug, Clone)]
pub struct RunOptions {
    pub harness: String,
    pub mode: Option<String>,
    /// e.g. `127.0.0.1:0` for ephemeral, or `127.0.0.1:4101`
    pub bind: String,
    pub upstream: Option<String>,
    pub capture_base: PathBuf,
    pub credential_override: Option<String>,
    pub ca_cert_path: Option<PathBuf>,
    /// If true, only run reverse wrap until Ctrl-C (no child).
    pub serve_only: bool,
    /// Args passed to the harness binary (after `--`).
    pub args: Vec<String>,
    /// Extra quiet: less stderr chatter
    pub quiet: bool,
}

#[derive(Debug)]
pub struct RunOutcome {
    pub exit_code: i32,
    pub wrap_base_url: String,
    pub capture_dir: PathBuf,
    pub flows_path: Option<PathBuf>,
    pub binary: PathBuf,
    pub mode_id: String,
}

pub async fn run_wrapped(opts: RunOptions) -> Result<RunOutcome> {
    let catalog = load_catalog()?;
    let (id, harness) = catalog
        .resolve(&opts.harness)
        .map(|(i, h)| (i.to_string(), h.clone()))
        .with_context(|| {
            format!(
                "unknown wrap harness '{}' (configured: {})",
                opts.harness,
                catalog.list_ids().join(", ")
            )
        })?;
    if !harness.enabled {
        bail!("wrap harness '{id}' is disabled in catalog");
    }

    let upstream = opts
        .upstream
        .clone()
        .or_else(|| harness.upstream.as_ref().and_then(|u| u.default.clone()))
        .unwrap_or_else(|| "https://ampcode.com".into());

    let capture_dir = capture_dir_for(&opts.capture_base, &id);
    std::fs::create_dir_all(&capture_dir)?;
    let flows_path = capture_dir.join(&harness.capture.jsonl_name);
    // Fresh per-run capture files. `flows.jsonl` is recreated by CaptureLog;
    // websocket and HTTP body sidecars are append-only, so clear stale data.
    let _ = std::fs::remove_file(capture_dir.join("ws.jsonl"));
    let _ = std::fs::remove_file(capture_dir.join("http-bodies.jsonl"));
    let log = CaptureLog::with_jsonl(&flows_path, harness.capture.clone())?;

    let bind: std::net::SocketAddr = opts
        .bind
        .parse()
        .with_context(|| format!("bad --bind {}", opts.bind))?;

    if !opts.quiet {
        eprintln!("alex wrap: reverse wrap → {upstream} (listening…)");
    }
    let reverse_opts = alex_wrap_reverse_opts(&harness);
    let wrap = ReverseWrap::start_to_url_with(bind, &upstream, log.clone(), reverse_opts).await?;
    let wrap_base_url = wrap.base_url();
    if !opts.quiet {
        eprintln!("alex wrap: listening on {wrap_base_url}");
        eprintln!("alex wrap: capture → {}", flows_path.display());
    }

    if opts.serve_only {
        if !opts.quiet {
            eprintln!("alex wrap: serve-only — Ctrl-C to stop");
        }
        tokio::signal::ctrl_c().await.ok();
        wrap.shutdown().await;
        return Ok(RunOutcome {
            exit_code: 0,
            wrap_base_url,
            capture_dir,
            flows_path: Some(flows_path),
            binary: PathBuf::from(&harness.binary),
            mode_id: opts
                .mode
                .clone()
                .unwrap_or_else(|| harness.default_mode.clone()),
        });
    }

    let (_hid, plan) = plan_for(
        &catalog,
        &id,
        opts.mode.as_deref(),
        &wrap_base_url,
        &capture_dir,
        opts.credential_override.clone(),
        opts.ca_cert_path.clone(),
    )?;

    if plan.env.values().any(|v| v.is_empty()) {
        // already filtered empty keys
    }
    // Require credential for amp-like modes when env expects it
    if harness.credentials.is_some()
        && !plan
            .env
            .keys()
            .any(|k| k.contains("KEY") || k.contains("TOKEN"))
    {
        if !opts.quiet {
            eprintln!(
                "alex wrap: warning: no credential resolved — run `alex auth import {id}` or set the harness API key env"
            );
        }
    }

    let binary = find_binary(&harness)?;
    let user_args: Vec<&str> = opts.args.iter().map(String::as_str).collect();
    let argv = plan.full_argv(&user_args);

    if !opts.quiet {
        eprintln!(
            "alex wrap: spawn {} {}  (mode={})",
            binary.display(),
            argv.join(" "),
            plan.mode_id
        );
    }

    let mut cmd = Command::new(&binary);
    cmd.args(&argv)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .kill_on_drop(true);
    for (k, v) in &plan.env {
        cmd.env(k, v);
    }
    strip_remote_trace_env(&mut cmd);
    // Ensure child cwd is user cwd
    let mut child = cmd
        .spawn()
        .with_context(|| format!("spawn {}", binary.display()))?;

    let status = tokio::select! {
        status = child.wait() => {
            status.context("wait for harness")?
        }
        _ = tokio::signal::ctrl_c() => {
            if !opts.quiet {
                eprintln!("alex wrap: interrupt — stopping harness");
            }
            let _ = child.start_kill();
            // brief grace
            let _ = tokio::time::timeout(Duration::from_secs(3), child.wait()).await;
            wrap.shutdown().await;
            return Ok(RunOutcome {
                exit_code: 130,
                wrap_base_url,
                capture_dir,
                flows_path: Some(flows_path),
                binary,
                mode_id: plan.mode_id,
            });
        }
    };

    wrap.shutdown().await;

    let code = status.code().unwrap_or(1);
    if !opts.quiet {
        eprintln!(
            "alex wrap: harness exited {code}; flows: {}",
            flows_path.display()
        );
    }
    Ok(RunOutcome {
        exit_code: code,
        wrap_base_url,
        capture_dir,
        flows_path: Some(flows_path),
        binary,
        mode_id: plan.mode_id,
    })
}

fn strip_remote_trace_env(cmd: &mut Command) {
    for name in REMOTE_TRACE_ENV_VARS {
        cmd.env_remove(name);
    }
}

fn find_binary(harness: &WrapHarness) -> Result<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    candidates.push(PathBuf::from(&harness.binary));
    for c in &harness.binary_candidates {
        candidates.push(expand_user_path(c));
    }
    // PATH lookup
    if let Ok(path) = which(&harness.binary) {
        return Ok(path);
    }
    for c in candidates {
        if c.is_file() {
            return Ok(c);
        }
    }
    bail!(
        "could not find binary '{}' (tried PATH and catalog binary_candidates)",
        harness.binary
    )
}

fn which(name: &str) -> Result<PathBuf> {
    // PATHEXT-aware so npm-installed harness shims (claude.cmd, codex.cmd)
    // resolve on Windows.
    alex_core::exec::find_on_path_filtered(name, |_| true)
        .ok_or_else(|| anyhow::anyhow!("`{name}` not found on PATH"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::WrapCatalog;

    #[test]
    fn wrapped_child_does_not_inherit_remote_trace_config() {
        let mut cmd = Command::new("unused-test-program");
        for name in REMOTE_TRACE_ENV_VARS {
            cmd.env(name, "sensitive-parent-value");
        }
        cmd.env("AMP_API_KEY", "harness-credential");

        strip_remote_trace_env(&mut cmd);

        let configured: std::collections::BTreeMap<_, _> = cmd
            .as_std()
            .get_envs()
            .map(|(name, value)| {
                (
                    name.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect();
        for name in REMOTE_TRACE_ENV_VARS {
            assert_eq!(configured.get(*name), Some(&None), "{name} was not removed");
        }
        assert_eq!(
            configured.get("AMP_API_KEY"),
            Some(&Some("harness-credential".into()))
        );
    }

    #[test]
    fn find_binary_amp_candidates_shape() {
        let cat = WrapCatalog::embedded().unwrap();
        let (_, h) = cat.resolve("amp").unwrap();
        assert!(!h.binary_candidates.is_empty());
        // find_binary may or may not succeed depending on machine
        let _ = find_binary(h);
    }
}
