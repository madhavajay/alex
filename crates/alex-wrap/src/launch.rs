//! Build process env / settings / argv from a catalog mode profile.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::catalog::{TemplateCtx, WrapHarness};
use crate::credentials::resolve_credential;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WrapRole {
    ReverseHttp,
    HttpProxy,
}

impl WrapRole {
    pub fn from_str_loose(s: &str) -> Self {
        match s {
            "http_proxy" => Self::HttpProxy,
            _ => Self::ReverseHttp,
        }
    }
}

/// Fully resolved launch plan for one harness + mode.
#[derive(Debug, Clone, Serialize)]
pub struct LaunchPlan {
    pub harness_id: String,
    pub mode_id: String,
    pub binary: String,
    pub wrap_role: WrapRole,
    pub wrap_base_url: String,
    pub capture_dir: PathBuf,
    pub env: BTreeMap<String, String>,
    pub settings_path: Option<PathBuf>,
    pub settings_cli_flag: Option<String>,
    pub argv_prefix: Vec<String>,
    pub argv_suffix: Vec<String>,
    pub notes: Option<String>,
    pub upstream_default: Option<String>,
}

#[derive(Debug, Clone)]
pub struct LaunchRequest {
    pub harness_id: String,
    pub mode_id: Option<String>,
    pub wrap_base_url: String,
    pub capture_dir: PathBuf,
    /// Override credential (e.g. from vault); falls back to catalog resolution.
    pub credential_override: Option<String>,
    pub ca_cert_path: Option<PathBuf>,
}

impl LaunchPlan {
    pub fn resolve(harness: &WrapHarness, harness_id: &str, req: &LaunchRequest) -> Result<Self> {
        let mode_id = req
            .mode_id
            .clone()
            .unwrap_or_else(|| harness.default_mode.clone());
        let mode = harness.modes.get(&mode_id).with_context(|| {
            format!(
                "unknown mode '{mode_id}' for harness '{harness_id}' (available: {})",
                harness.modes.keys().cloned().collect::<Vec<_>>().join(", ")
            )
        })?;

        std::fs::create_dir_all(&req.capture_dir)
            .with_context(|| format!("create capture dir {}", req.capture_dir.display()))?;

        let credential = if let Some(c) = &req.credential_override {
            c.clone()
        } else if let Some(creds) = &harness.credentials {
            resolve_credential(creds)?.unwrap_or_default()
        } else {
            String::new()
        };

        // settings path first so templates can use it
        let settings_path = mode.settings_file.as_ref().map(|sf| {
            let tmp = TemplateCtx {
                wrap_base_url: req.wrap_base_url.clone(),
                credential: credential.clone(),
                capture_dir: req.capture_dir.display().to_string(),
                ca_cert_path: req
                    .ca_cert_path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default(),
                settings_path: String::new(),
            };
            PathBuf::from(tmp.apply(&sf.path))
        });

        let ctx = TemplateCtx {
            wrap_base_url: req.wrap_base_url.clone(),
            credential: credential.clone(),
            capture_dir: req.capture_dir.display().to_string(),
            ca_cert_path: req
                .ca_cert_path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            settings_path: settings_path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
        };

        let mut env = BTreeMap::new();
        for (k, v) in &mode.env {
            let rendered = ctx.apply(v);
            // Skip empty credential injection rather than export empty string.
            if k.contains("KEY") || k.contains("TOKEN") {
                if rendered.is_empty() || TemplateCtx::has_unresolved(&rendered) {
                    continue;
                }
            }
            env.insert(k.clone(), rendered);
        }
        for (k, v) in &mode.optional_env {
            let rendered = ctx.apply(v);
            if rendered.is_empty() || TemplateCtx::has_unresolved(&rendered) {
                continue;
            }
            env.insert(k.clone(), rendered);
        }

        if let Some(sf) = &mode.settings_file {
            let path = settings_path.clone().unwrap();
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut obj = serde_json::Map::new();
            for (k, v) in &sf.merge {
                obj.insert(k.clone(), render_json_value(v, &ctx));
            }
            let text = serde_json::to_string_pretty(&Value::Object(obj))?;
            std::fs::write(&path, text)
                .with_context(|| format!("write settings {}", path.display()))?;
        }

        let mut argv_prefix: Vec<String> =
            mode.cli_args_prefix.iter().map(|a| ctx.apply(a)).collect();
        let argv_suffix: Vec<String> = mode.cli_args_suffix.iter().map(|a| ctx.apply(a)).collect();

        // Settings flags go first so harness flags like `-x prompt` don't
        // swallow them as prompt text.
        let settings_cli_flag = mode.settings_file.as_ref().and_then(|s| s.cli_flag.clone());
        if let (Some(flag), Some(path)) = (&settings_cli_flag, &settings_path) {
            argv_prefix.push(flag.clone());
            argv_prefix.push(path.display().to_string());
        }

        Ok(Self {
            harness_id: harness_id.to_string(),
            mode_id,
            binary: harness.binary.clone(),
            wrap_role: WrapRole::from_str_loose(&mode.wrap_role),
            wrap_base_url: req.wrap_base_url.clone(),
            capture_dir: req.capture_dir.clone(),
            env,
            settings_path,
            settings_cli_flag,
            argv_prefix,
            argv_suffix,
            notes: mode.notes.clone(),
            upstream_default: harness.upstream.as_ref().and_then(|u| u.default.clone()),
        })
    }

    /// Shell export lines (safe for eval).
    pub fn export_lines(&self) -> Vec<String> {
        self.env
            .iter()
            .map(|(k, v)| {
                let escaped = v.replace('\'', r#"'"'"'"#);
                format!("export {k}='{escaped}'")
            })
            .collect()
    }

    /// Full argv: prefix (incl. settings) + user args + suffix (log file, etc.).
    pub fn full_argv(&self, user_args: &[&str]) -> Vec<String> {
        let mut out = self.argv_prefix.clone();
        out.extend(user_args.iter().map(|s| (*s).to_string()));
        out.extend(self.argv_suffix.iter().cloned());
        out
    }

    pub fn spawn(&self, program: &Path, user_args: &[&str]) -> Result<std::process::Child> {
        let args = self.full_argv(user_args);
        let mut cmd = Command::new(program);
        cmd.args(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());
        for (k, v) in &self.env {
            cmd.env(k, v);
        }
        cmd.spawn()
            .with_context(|| format!("spawn {}", program.display()))
    }

    pub fn summary_json(&self) -> Value {
        json!({
            "harness_id": self.harness_id,
            "mode_id": self.mode_id,
            "binary": self.binary,
            "wrap_role": match self.wrap_role {
                WrapRole::ReverseHttp => "reverse_http",
                WrapRole::HttpProxy => "http_proxy",
            },
            "wrap_base_url": self.wrap_base_url,
            "capture_dir": self.capture_dir,
            "env_keys": self.env.keys().cloned().collect::<Vec<_>>(),
            "settings_path": self.settings_path,
            "argv_suffix": self.argv_suffix,
            "notes": self.notes,
            "upstream_default": self.upstream_default,
        })
    }
}

fn render_json_value(v: &Value, ctx: &TemplateCtx) -> Value {
    match v {
        Value::String(s) => Value::String(ctx.apply(s)),
        Value::Array(a) => Value::Array(a.iter().map(|x| render_json_value(x, ctx)).collect()),
        Value::Object(o) => {
            let mut m = serde_json::Map::new();
            for (k, val) in o {
                m.insert(k.clone(), render_json_value(val, ctx));
            }
            Value::Object(m)
        }
        other => other.clone(),
    }
}

/// Pick a mode id: explicit, else preferred, else default_mode.
pub fn select_mode(harness: &WrapHarness, explicit: Option<&str>) -> Result<String> {
    if let Some(m) = explicit {
        if harness.modes.contains_key(m) {
            return Ok(m.to_string());
        }
        bail!(
            "unknown mode '{m}' (available: {})",
            harness.modes.keys().cloned().collect::<Vec<_>>().join(", ")
        );
    }
    if let Some((id, _)) = harness.modes.iter().find(|(_, m)| m.preferred) {
        return Ok(id.clone());
    }
    if harness.modes.contains_key(&harness.default_mode) {
        return Ok(harness.default_mode.clone());
    }
    harness
        .modes
        .keys()
        .next()
        .cloned()
        .context("harness has no modes configured")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::WrapCatalog;

    #[test]
    fn amp_base_url_plan_writes_settings_and_env() {
        let cat = WrapCatalog::embedded().unwrap();
        let (id, h) = cat.resolve("amp").unwrap();
        let dir = tempfile::tempdir().unwrap();
        let req = LaunchRequest {
            harness_id: id.to_string(),
            mode_id: Some("base_url".into()),
            wrap_base_url: "http://127.0.0.1:4101".into(),
            capture_dir: dir.path().to_path_buf(),
            credential_override: Some("sgamp_test".into()),
            ca_cert_path: None,
        };
        let plan = LaunchPlan::resolve(h, id, &req).unwrap();
        assert_eq!(plan.env.get("AMP_URL").unwrap(), "http://127.0.0.1:4101");
        assert_eq!(plan.env.get("AMP_API_KEY").unwrap(), "sgamp_test");
        let settings = plan.settings_path.as_ref().unwrap();
        assert!(settings.exists());
        let raw = std::fs::read_to_string(settings).unwrap();
        assert!(raw.contains("amp.url"));
        assert!(raw.contains("http://127.0.0.1:4101"));
        assert!(plan.argv_prefix.iter().any(|a| a == "--settings-file"));
    }

    #[test]
    fn empty_credential_skipped() {
        let cat = WrapCatalog::embedded().unwrap();
        let (id, h) = cat.resolve("amp").unwrap();
        let dir = tempfile::tempdir().unwrap();
        let req = LaunchRequest {
            harness_id: id.to_string(),
            mode_id: Some("base_url".into()),
            wrap_base_url: "http://127.0.0.1:1".into(),
            capture_dir: dir.path().to_path_buf(),
            credential_override: Some("".into()),
            ca_cert_path: None,
        };
        let plan = LaunchPlan::resolve(h, id, &req).unwrap();
        assert!(!plan.env.contains_key("AMP_API_KEY"));
    }
}
