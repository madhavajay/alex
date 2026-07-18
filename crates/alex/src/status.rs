use std::path::PathBuf;

use anyhow::Result;
use serde::Serialize;
use serde_json::Value;

use crate::{
    detect_service_state, fetch_json, installed_binaries, now_ms, open_vault, service_managed,
    service_state_label, Config, ServiceState,
};

#[derive(Debug, Clone, Serialize)]
pub(crate) struct BinaryStatus {
    pub path: PathBuf,
    pub this_binary: bool,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct StatusAccount {
    pub id: String,
    pub provider: String,
    pub name: String,
    pub kind: String,
    pub label: Option<String>,
    pub status: String,
    pub health: String,
    pub needs_reauth: bool,
    pub usage_pct: Option<f64>,
    pub expires_at_ms: Option<i64>,
    pub last_heartbeat: Value,
}

/// The one shared status aggregation boundary for CLI and inbound commands.
/// Raw endpoint payloads stay available to the rich CLI while typed fields
/// make compact control-channel rendering deterministic and testable.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct StatusSummary {
    pub version: String,
    pub update_available: bool,
    pub update_target: Option<String>,
    pub daemon_up: bool,
    pub uptime_s: Option<i64>,
    pub service: String,
    pub service_managed: bool,
    pub dario_ready: bool,
    pub accounts: Vec<StatusAccount>,
    pub binaries: Vec<BinaryStatus>,
    pub base_url: String,
    pub health: Option<Value>,
    pub limits: Option<Value>,
    pub dario: Option<Value>,
    pub dario_response: Option<(u16, Value)>,
    pub update: Option<Value>,
    pub admin_health: Option<Value>,
    #[serde(skip)]
    pub service_state: ServiceState,
}

pub(crate) async fn status_summary(config: &Config) -> Result<StatusSummary> {
    let base_url = config.base_url();
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()?;
    let key = config.local_key.as_str();
    let health = successful(fetch_json(&client, &format!("{base_url}/health"), key).await);
    let daemon_up = health.is_some();

    let (admin_health, limits, dario_response, update) = if daemon_up {
        let admin_health_url = format!("{base_url}/admin/health");
        let limits_url = format!("{base_url}/admin/limits");
        let dario_url = format!("{base_url}/admin/dario");
        let update_url = format!("{base_url}/admin/update");
        let (admin_health, limits, dario, update) = tokio::join!(
            fetch_json(&client, &admin_health_url, key),
            fetch_json(&client, &limits_url, key),
            fetch_json(&client, &dario_url, key),
            fetch_json(&client, &update_url, key),
        );
        (
            successful(admin_health),
            successful(limits),
            dario,
            successful(update),
        )
    } else {
        (None, None, None, None)
    };

    let vault = open_vault(config)?;
    let vault_accounts = vault.list().await;
    let accounts = vault_accounts
        .into_iter()
        .map(|account| {
            let last_heartbeat = heartbeat_for(admin_health.as_ref(), &account.id);
            let needs_reauth = account.needs_reauth();
            let usage_pct = usage_for(limits.as_ref(), account.provider.as_str());
            StatusAccount {
                id: account.id,
                provider: account.provider.as_str().to_string(),
                name: account.name,
                kind: account.kind,
                label: account.label,
                status: account.status,
                health: heartbeat_health(&last_heartbeat),
                needs_reauth,
                usage_pct,
                expires_at_ms: account.expires_at_ms,
                last_heartbeat,
            }
        })
        .collect();

    let current_exe = std::env::current_exe()
        .ok()
        .and_then(|path| path.canonicalize().ok());
    let binaries = installed_binaries()
        .into_iter()
        .map(|path| BinaryStatus {
            this_binary: current_exe.is_some() && path.canonicalize().ok() == current_exe,
            path,
        })
        .collect();
    let service_state = detect_service_state();
    let service = service_state_label(&service_state).to_string();
    let service_managed = service_managed(&service_state);
    let version = health
        .as_ref()
        .and_then(|value| value["version"].as_str())
        .unwrap_or(env!("CARGO_PKG_VERSION"))
        .to_string();
    let uptime_s = health.as_ref().and_then(|value| value["uptime_s"].as_i64());
    let update_available = update
        .as_ref()
        .and_then(|value| value["update_available"].as_bool())
        .unwrap_or(false);
    let update_target = update.as_ref().and_then(|value| {
        value["latest"]
            .as_str()
            .or_else(|| value["target"].as_str())
            .map(str::to_owned)
    });
    let dario = successful(dario_response.clone());
    let dario_ready = dario
        .as_ref()
        .map(|value| {
            value["available"].as_bool().unwrap_or(false)
                && value["active"].as_bool().unwrap_or(true)
        })
        .unwrap_or(false);

    Ok(StatusSummary {
        version,
        update_available,
        update_target,
        daemon_up,
        uptime_s,
        service,
        service_managed,
        dario_ready,
        accounts,
        binaries,
        base_url,
        health,
        limits,
        dario,
        dario_response,
        update,
        admin_health,
        service_state,
    })
}

fn successful(response: Option<(u16, Value)>) -> Option<Value> {
    response
        .filter(|(status, _)| (200..300).contains(status))
        .map(|(_, value)| value)
}

fn heartbeat_for(admin_health: Option<&Value>, account_id: &str) -> Value {
    admin_health
        .and_then(|value| value["accounts"].as_array())
        .and_then(|accounts| {
            accounts
                .iter()
                .find(|account| account["id"].as_str() == Some(account_id))
        })
        .map(|account| account["last_heartbeat"].clone())
        .unwrap_or(Value::Null)
}

fn heartbeat_health(heartbeat: &Value) -> String {
    match heartbeat["ok"].as_bool() {
        Some(true) => "healthy",
        Some(false) => "down",
        None => "unknown",
    }
    .to_string()
}

fn usage_for(limits: Option<&Value>, provider: &str) -> Option<f64> {
    let entry = limits?["providers"]
        .as_array()?
        .iter()
        .find(|entry| entry["provider"].as_str() == Some(provider))?;
    let mut values = Vec::new();
    if let Some(remaining) = entry["quota"]["remaining_pct"].as_f64() {
        values.push(100.0 - remaining);
    }
    values.extend(
        entry["windows"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|window| window["used_pct"].as_f64()),
    );
    for kind in ["requests", "tokens"] {
        if let (Some(limit), Some(remaining)) = (
            entry[kind]["limit"].as_f64(),
            entry[kind]["remaining"].as_f64(),
        ) {
            if limit > 0.0 {
                values.push(100.0 * (limit - remaining) / limit);
            }
        }
    }
    values
        .into_iter()
        .filter(|value| value.is_finite())
        .map(|value| value.clamp(0.0, 100.0))
        .max_by(f64::total_cmp)
}

impl StatusSummary {
    pub(crate) fn telegram_text(&self) -> String {
        self.telegram_text_at(now_ms())
    }

    pub(crate) fn ping_text(&self) -> String {
        let uptime = self
            .uptime_s
            .map(|seconds| compact_duration(seconds * 1000))
            .unwrap_or_else(|| "unknown".to_string());
        format!("pong · Alexandria v{} · uptime {uptime}", self.version)
    }

    fn telegram_text_at(&self, now: i64) -> String {
        let update = if self.update_available {
            format!(
                "update available → v{}",
                self.update_target.as_deref().unwrap_or("unknown")
            )
        } else {
            "up to date".to_string()
        };
        let daemon = if self.daemon_up {
            format!(
                "up{}",
                self.uptime_s
                    .map(|seconds| format!(" · uptime {}", compact_duration(seconds * 1000)))
                    .unwrap_or_default()
            )
        } else {
            "down".to_string()
        };
        let mut lines = vec![
            format!("Alexandria v{} · {update}", self.version),
            format!("Daemon: {daemon} · service {}", self.service),
            format!("Dario: {}", if self.dario_ready { "ready" } else { "down" }),
            String::new(),
            "Accounts:".to_string(),
        ];
        if self.accounts.is_empty() {
            lines.push("• none configured".to_string());
        }
        for account in &self.accounts {
            let mut details = vec![account.status.clone(), account.health.clone()];
            if account.needs_reauth {
                details.push("reauth needed".to_string());
            }
            if let Some(usage) = account.usage_pct {
                details.push(format!("{usage:.0}% used"));
            }
            details.push(expiry_text(account.expires_at_ms, now));
            lines.push(format!(
                "• {}/{} — {}",
                account.provider,
                account.name,
                details.join(" · ")
            ));
        }
        lines.join("\n")
    }
}

fn expiry_text(expires_at_ms: Option<i64>, now: i64) -> String {
    match expires_at_ms.map(|expiry| expiry - now) {
        Some(remaining) if remaining >= 0 => format!("{} left", compact_duration(remaining)),
        Some(remaining) => format!("expired {} ago", compact_duration(-remaining)),
        None => "no expiry".to_string(),
    }
}

fn compact_duration(ms: i64) -> String {
    let seconds = (ms.max(0) / 1000).max(1);
    if seconds >= 86_400 {
        format!("{}d", seconds / 86_400)
    } else if seconds >= 3_600 {
        format!("{}h", seconds / 3_600)
    } else if seconds >= 60 {
        format!("{}m", seconds / 60)
    } else {
        format!("{seconds}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_synthetic_status_for_telegram() {
        let now = 1_000_000;
        let summary = StatusSummary {
            version: "9.8.7".into(),
            update_available: true,
            update_target: Some("10.0.0".into()),
            daemon_up: true,
            uptime_s: Some(7_200),
            service: "active".into(),
            service_managed: true,
            dario_ready: true,
            accounts: vec![StatusAccount {
                id: "openai-oauth-work".into(),
                provider: "openai".into(),
                name: "work".into(),
                kind: "oauth".into(),
                label: None,
                status: "active".into(),
                health: "healthy".into(),
                needs_reauth: false,
                usage_pct: Some(42.4),
                expires_at_ms: Some(now + 6 * 3_600_000),
                last_heartbeat: serde_json::json!({"ok": true}),
            }],
            binaries: Vec::new(),
            base_url: "http://127.0.0.1:4100".into(),
            health: None,
            limits: None,
            dario: None,
            dario_response: None,
            update: None,
            admin_health: None,
            service_state: ServiceState::Unsupported,
        };

        let text = summary.telegram_text_at(now);
        assert!(text.contains("Alexandria v9.8.7 · update available → v10.0.0"));
        assert!(text.contains("Daemon: up · uptime 2h · service active"));
        assert!(text.contains("Dario: ready"));
        assert!(text.contains("openai/work — active · healthy · 42% used · 6h left"));
    }
}
