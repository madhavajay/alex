//! Resolve harness credentials from catalog-declared sources.

use std::path::Path;

use anyhow::{Context, Result};
use serde_json::Value;

use crate::catalog::{expand_user_path, WrapCredentials};

/// Resolve an API key / token using the harness credentials block.
pub fn resolve_credential(creds: &WrapCredentials) -> Result<Option<String>> {
    match creds.kind.as_str() {
        "secrets_json" => resolve_secrets_json(creds),
        "env" => Ok(resolve_env_fallbacks(&creds.env_fallbacks)),
        "none" => Ok(None),
        other => anyhow::bail!("unknown credentials.kind '{other}'"),
    }
}

fn resolve_env_fallbacks(names: &[String]) -> Option<String> {
    for name in names {
        if let Ok(v) = std::env::var(name) {
            let t = v.trim();
            if !t.is_empty() {
                return Some(t.to_string());
            }
        }
    }
    None
}

fn resolve_secrets_json(creds: &WrapCredentials) -> Result<Option<String>> {
    // Prefer env fallbacks first when set (explicit override).
    if let Some(k) = resolve_env_fallbacks(&creds.env_fallbacks) {
        return Ok(Some(k));
    }
    let Some(path_tmpl) = creds.path.as_deref() else {
        return Ok(None);
    };
    let path = expand_user_path(path_tmpl);
    read_secrets_json_key(
        &path,
        creds.key_prefix.as_deref().unwrap_or("apiKey@"),
        creds.prefer_url_contains.as_deref(),
    )
}

/// Read `apiKey@…` style secrets file (Amp CLI format).
pub fn read_secrets_json_key(
    path: &Path,
    key_prefix: &str,
    prefer_url_contains: Option<&str>,
) -> Result<Option<String>> {
    if !path.exists() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("read secrets {}", path.display()))?;
    let v: Value = serde_json::from_str(&raw).context("parse secrets JSON")?;
    let Some(obj) = v.as_object() else {
        return Ok(None);
    };
    let mut preferred: Option<String> = None;
    let mut any: Option<String> = None;
    for (k, val) in obj {
        if !k.starts_with(key_prefix) {
            continue;
        }
        let Some(s) = val.as_str().filter(|s| !s.is_empty()) else {
            continue;
        };
        any = Some(s.to_string());
        if let Some(needle) = prefer_url_contains {
            if k.contains(needle) {
                preferred = Some(s.to_string());
                break;
            }
        }
    }
    Ok(preferred.or(any))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::WrapCredentials;

    #[test]
    fn secrets_json_prefers_ampcode() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secrets.json");
        std::fs::write(
            &path,
            r#"{
              "apiKey@https://other.example/":"other",
              "apiKey@https://ampcode.com/":"sgamp_ok"
            }"#,
        )
        .unwrap();
        let key = read_secrets_json_key(&path, "apiKey@", Some("ampcode.com"))
            .unwrap()
            .unwrap();
        assert_eq!(key, "sgamp_ok");
    }

    #[test]
    fn env_override_wins() {
        let creds = WrapCredentials {
            kind: "secrets_json".into(),
            path: Some("/nonexistent".into()),
            key_prefix: Some("apiKey@".into()),
            prefer_url_contains: None,
            env_fallbacks: vec!["ALEX_WRAP_TEST_KEY".into()],
            vault_provider: None,
            vault_account_id: None,
        };
        std::env::set_var("ALEX_WRAP_TEST_KEY", "from_env");
        let k = resolve_credential(&creds).unwrap().unwrap();
        assert_eq!(k, "from_env");
        std::env::remove_var("ALEX_WRAP_TEST_KEY");
    }
}
