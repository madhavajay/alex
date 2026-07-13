//! Declarative wrap harness catalog (JSON).
//!
//! Profiles live in `config/wrap-harnesses.json` so Amp (and future binaries)
//! can be updated when env knobs change without touching launch code.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const EMBEDDED: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/config/wrap-harnesses.json"
));

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WrapCatalog {
    pub version: u32,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub harnesses: BTreeMap<String, WrapHarness>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WrapHarness {
    #[serde(default)]
    pub aliases: Vec<String>,
    pub binary: String,
    #[serde(default)]
    pub binary_candidates: Vec<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_mode_name")]
    pub default_mode: String,
    #[serde(default)]
    pub upstream: Option<WrapUpstream>,
    /// Optional reverse-proxy request rewrites (e.g. inject Rivet public token).
    #[serde(default)]
    pub reverse_inject: Option<WrapReverseInject>,
    #[serde(default)]
    pub credentials: Option<WrapCredentials>,
    #[serde(default)]
    pub capture: WrapCapture,
    #[serde(default)]
    pub modes: BTreeMap<String, WrapModeSpec>,
}

/// Query/header injections applied by the reverse wrap when the client omits
/// production-only credentials (Amp treats localhost AMP_URL as token-less).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WrapReverseInject {
    #[serde(default)]
    pub notes: Option<String>,
    /// Query params to ensure are present on matching paths.
    #[serde(default)]
    pub query_params: BTreeMap<String, String>,
    /// Only apply when request path starts with one of these prefixes.
    #[serde(default)]
    pub path_prefixes: Vec<String>,
    /// If true (default), do not overwrite a param the client already sent.
    #[serde(default = "default_true")]
    pub only_if_missing: bool,
}

fn default_true() -> bool {
    true
}

fn default_mode_name() -> String {
    "base_url".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WrapUpstream {
    #[serde(default)]
    pub default: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WrapCredentials {
    /// secrets_json | env | none
    pub kind: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub key_prefix: Option<String>,
    #[serde(default)]
    pub prefer_url_contains: Option<String>,
    #[serde(default)]
    pub env_fallbacks: Vec<String>,
    #[serde(default)]
    pub vault_provider: Option<String>,
    #[serde(default)]
    pub vault_account_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WrapCapture {
    #[serde(default = "default_jsonl")]
    pub jsonl_name: String,
    #[serde(default = "default_log")]
    pub log_name: String,
    #[serde(default)]
    pub interesting_path_prefixes: Vec<String>,
    #[serde(default)]
    pub ignore_path_prefixes: Vec<String>,
    #[serde(default)]
    pub redact_query_keys: Vec<String>,
    #[serde(default)]
    pub redact_headers: Vec<String>,
    #[serde(default = "default_preview")]
    pub max_body_preview_bytes: usize,
    #[serde(default = "default_max_events")]
    pub max_events: usize,
}

fn default_jsonl() -> String {
    "flows.jsonl".into()
}
fn default_log() -> String {
    "harness.log".into()
}
fn default_preview() -> usize {
    8000
}
fn default_max_events() -> usize {
    10_000
}

impl Default for WrapCapture {
    fn default() -> Self {
        Self {
            jsonl_name: default_jsonl(),
            log_name: default_log(),
            interesting_path_prefixes: vec![],
            ignore_path_prefixes: vec![],
            redact_query_keys: vec![],
            redact_headers: vec![],
            max_body_preview_bytes: default_preview(),
            max_events: default_max_events(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WrapModeSpec {
    #[serde(default)]
    pub description: Option<String>,
    /// reverse_http | http_proxy
    pub wrap_role: String,
    #[serde(default)]
    pub preferred: bool,
    /// Required env: key -> template value
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Optional env (skipped when template vars unresolved, e.g. empty ca path)
    #[serde(default)]
    pub optional_env: BTreeMap<String, String>,
    #[serde(default)]
    pub settings_file: Option<WrapSettingsFile>,
    #[serde(default)]
    pub cli_args_prefix: Vec<String>,
    #[serde(default)]
    pub cli_args_suffix: Vec<String>,
    #[serde(default)]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WrapSettingsFile {
    pub path: String,
    #[serde(default)]
    pub cli_flag: Option<String>,
    #[serde(default)]
    pub merge: BTreeMap<String, Value>,
}

impl WrapCatalog {
    pub fn embedded() -> Result<Self> {
        Self::from_str(EMBEDDED)
    }

    pub fn load_path(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("read wrap catalog {}", path.display()))?;
        Self::from_str(&raw)
    }

    /// Prefer override file if present, else embedded defaults.
    pub fn load_with_optional_override(override_path: Option<&Path>) -> Result<Self> {
        if let Some(p) = override_path {
            if p.exists() {
                return Self::load_path(p);
            }
        }
        Self::embedded()
    }

    pub fn from_str(raw: &str) -> Result<Self> {
        let cat: Self = serde_json::from_str(raw).context("parse wrap catalog JSON")?;
        if cat.version == 0 {
            bail!("wrap catalog version must be >= 1");
        }
        Ok(cat)
    }

    pub fn resolve(&self, name: &str) -> Option<(&str, &WrapHarness)> {
        let lower = name.to_ascii_lowercase();
        if let Some((id, h)) = self.harnesses.get_key_value(&lower) {
            return Some((id.as_str(), h));
        }
        for (id, h) in &self.harnesses {
            if h.aliases.iter().any(|a| a.eq_ignore_ascii_case(name)) {
                return Some((id.as_str(), h));
            }
        }
        // binary name match
        for (id, h) in &self.harnesses {
            if h.binary.eq_ignore_ascii_case(name) {
                return Some((id.as_str(), h));
            }
        }
        None
    }

    pub fn list_ids(&self) -> Vec<&str> {
        self.harnesses.keys().map(|s| s.as_str()).collect()
    }
}

/// Expand `~` and simple env for paths in the catalog.
pub fn expand_user_path(raw: &str) -> PathBuf {
    if let Some(rest) = raw.strip_prefix("~/") {
        if let Some(home) = home_dir() {
            return home.join(rest);
        }
    }
    if raw == "~" {
        if let Some(home) = home_dir() {
            return home;
        }
    }
    PathBuf::from(raw)
}

pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// Template context for substituting `{var}` in catalog strings.
#[derive(Debug, Clone, Default)]
pub struct TemplateCtx {
    pub wrap_base_url: String,
    pub credential: String,
    pub capture_dir: String,
    pub ca_cert_path: String,
    pub settings_path: String,
}

impl TemplateCtx {
    pub fn apply(&self, template: &str) -> String {
        template
            .replace("{wrap_base_url}", &self.wrap_base_url)
            .replace("{credential}", &self.credential)
            .replace("{capture_dir}", &self.capture_dir)
            .replace("{ca_cert_path}", &self.ca_cert_path)
            .replace("{settings_path}", &self.settings_path)
    }

    /// True if result still contains unresolved `{var}` placeholders we care about.
    pub fn has_unresolved(s: &str) -> bool {
        s.contains("{wrap_base_url}")
            || s.contains("{credential}")
            || s.contains("{capture_dir}")
            || s.contains("{ca_cert_path}")
            || s.contains("{settings_path}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_catalog_loads_amp() {
        let cat = WrapCatalog::embedded().unwrap();
        assert_eq!(cat.version, 1);
        let (id, h) = cat.resolve("amp").unwrap();
        assert_eq!(id, "amp");
        assert_eq!(h.binary, "amp");
        assert!(h.modes.contains_key("base_url"));
        assert!(h.modes.contains_key("env_proxy"));
        let mode = h.modes.get("base_url").unwrap();
        assert_eq!(
            mode.env.get("AMP_URL").map(String::as_str),
            Some("{wrap_base_url}")
        );
        assert!(mode.settings_file.is_some());
    }

    #[test]
    fn aliases_resolve() {
        let cat = WrapCatalog::embedded().unwrap();
        assert_eq!(cat.resolve("amp-code").unwrap().0, "amp");
        assert_eq!(cat.resolve("ampcode").unwrap().0, "amp");
    }

    #[test]
    fn template_apply() {
        let ctx = TemplateCtx {
            wrap_base_url: "http://127.0.0.1:9".into(),
            credential: "k".into(),
            capture_dir: "/tmp/c".into(),
            ca_cert_path: "".into(),
            settings_path: "/tmp/c/s.json".into(),
        };
        assert_eq!(ctx.apply("{wrap_base_url}/x"), "http://127.0.0.1:9/x");
        assert!(TemplateCtx::has_unresolved("{ca_cert_path}"));
        assert!(!TemplateCtx::has_unresolved(
            ctx.apply("{wrap_base_url}").as_str()
        ));
    }
}
