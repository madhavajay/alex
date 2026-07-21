//! Harness wrap layer — self-contained Rust capture for special binaries (Amp, …).
//!
//! ## Design
//! - **Config-driven**: each harness profile lives in `config/wrap-harnesses.json`
//!   (env vars, settings files, capture filters, credential paths). Update the
//!   JSON when a harness renames knobs — no code change required.
//! - **Application-level only**: reverse base-URL wrap or process env proxy.
//!   Not a system-wide intercept.
//! - **Isolated from alex-proxy**: keep main routing path clean.
//!
//! ## Typical Amp flow
//! ```text
//! alex wrap env amp --mode base_url
//! # → export AMP_URL=… AMP_API_KEY=… ; writes settings.json
//! # run reverse wrap listening on that URL, then amp --settings-file …
//! ```

mod capture;
mod catalog;
mod credentials;
mod launch;
mod reverse;
mod run;

pub use capture::{capture_dir_for, CaptureEvent, CaptureLog};
pub use catalog::{
    expand_user_path, home_dir, TemplateCtx, WrapCapture, WrapCatalog, WrapCredentials,
    WrapHarness, WrapModeSpec, WrapReverseInject, WrapSettingsFile, WrapUpstream,
};
pub use credentials::{read_secrets_json_key, resolve_credential};
pub use launch::{select_mode, LaunchPlan, LaunchRequest, WrapRole};
pub use reverse::{ReverseOptions, ReverseWrap};
pub use run::{run_wrapped, RunOptions, RunOutcome};

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

/// Default override path: `~/.alex/wrap-harnesses.json` (optional).
pub fn user_catalog_override_path() -> Option<PathBuf> {
    home_dir().map(|h| h.join(".alex/wrap-harnesses.json"))
}

/// Load catalog: user override if present, else embedded defaults.
pub fn load_catalog() -> Result<WrapCatalog> {
    WrapCatalog::load_with_optional_override(user_catalog_override_path().as_deref())
}

/// Resolve harness by id/alias/binary and build a launch plan.
pub fn plan_for(
    catalog: &WrapCatalog,
    harness_name: &str,
    mode: Option<&str>,
    wrap_base_url: &str,
    capture_dir: &Path,
    credential_override: Option<String>,
    ca_cert_path: Option<PathBuf>,
) -> Result<(String, LaunchPlan)> {
    let Some((id, harness)) = catalog.resolve(harness_name) else {
        bail!(
            "unknown wrap harness '{harness_name}' (configured: {})",
            catalog.list_ids().join(", ")
        );
    };
    if !harness.enabled {
        bail!("wrap harness '{id}' is disabled in catalog");
    }
    let mode_id = select_mode(harness, mode)?;
    let req = LaunchRequest {
        harness_id: id.to_string(),
        mode_id: Some(mode_id),
        wrap_base_url: wrap_base_url.to_string(),
        capture_dir: capture_dir.to_path_buf(),
        credential_override,
        ca_cert_path,
    };
    let plan = LaunchPlan::resolve(harness, id, &req)
        .with_context(|| format!("resolve launch plan for {id}"))?;
    Ok((id.to_string(), plan))
}

/// Status snapshot for CLI / JSON.
pub fn status_json(catalog: &WrapCatalog) -> serde_json::Value {
    let mut harnesses = serde_json::Map::new();
    for (id, h) in &catalog.harnesses {
        let modes: Vec<_> = h
            .modes
            .iter()
            .map(|(mid, m)| {
                serde_json::json!({
                    "id": mid,
                    "wrap_role": m.wrap_role,
                    "preferred": m.preferred,
                    "description": m.description,
                    "env_keys": m.env.keys().cloned().collect::<Vec<_>>(),
                })
            })
            .collect();
        let secrets_path = h
            .credentials
            .as_ref()
            .and_then(|c| c.path.as_ref())
            .map(|p| expand_user_path(p));
        let secrets_present = secrets_path.as_ref().map(|p| p.exists()).unwrap_or(false);
        let cred = h
            .credentials
            .as_ref()
            .and_then(|c| resolve_credential(c).ok().flatten());
        harnesses.insert(
            id.clone(),
            serde_json::json!({
                "binary": h.binary,
                "aliases": h.aliases,
                "enabled": h.enabled,
                "default_mode": h.default_mode,
                "description": h.description,
                "upstream": h.upstream,
                "modes": modes,
                "credentials": {
                    "kind": h.credentials.as_ref().map(|c| &c.kind),
                    "path": secrets_path.as_ref().map(|p| p.display().to_string()),
                    "path_exists": secrets_present,
                    "resolved": cred.is_some(),
                    "vault_provider": h.credentials.as_ref().and_then(|c| c.vault_provider.clone()),
                },
                "capture": {
                    "interesting_path_prefixes": h.capture.interesting_path_prefixes,
                    "redact_query_keys": h.capture.redact_query_keys,
                },
            }),
        );
    }
    serde_json::json!({
        "version": catalog.version,
        "description": catalog.description,
        "catalog_source": if user_catalog_override_path().map(|p| p.exists()).unwrap_or(false) {
            "user_override"
        } else {
            "embedded"
        },
        "harnesses": harnesses,
    })
}

// Back-compat helpers used by older call sites / tests.
pub fn amp_secrets_path() -> Option<PathBuf> {
    home_dir().map(|h| h.join(".local/share/amp/secrets.json"))
}

pub fn read_amp_api_key_from_secrets(path: &Path) -> Result<Option<String>> {
    read_secrets_json_key(path, "apiKey@", Some("ampcode.com"))
}
