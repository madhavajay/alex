use std::path::PathBuf;

use anyhow::Result;
use serde::Serialize;
use serde_json::Value;

use crate::{
    detect_service_state, fetch_json, installed_binaries, now_ms, open_vault, service_managed,
    service_state_label, Config, ServiceState,
};

const TELEGRAM_MESSAGE_LIMIT: usize = 4096;
const HEARTBEAT_ERROR_LIMIT: usize = 120;

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
        format!("pong · Alex v{} · uptime {uptime}", self.version)
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
        let lines = vec![
            format!("Alex v{} · {update}", self.version),
            format!("Daemon: {daemon} · service {}", self.service),
            format!("Dario: {}", if self.dario_ready { "ready" } else { "down" }),
            String::new(),
            "Accounts:".to_string(),
        ];
        let account_lines = if self.accounts.is_empty() {
            vec!["• none configured".to_string()]
        } else {
            self.accounts
                .iter()
                .map(|account| {
                    let mut details = vec![account.status.clone(), account.health.clone()];
                    if account.needs_reauth {
                        details.push("reauth needed".to_string());
                    }
                    if let Some(usage) = account.usage_pct {
                        details.push(format!("{usage:.0}% used"));
                    }
                    details.push(expiry_text(account.expires_at_ms, now));
                    format!(
                        "• {}/{} — {}",
                        account.provider,
                        account.name,
                        details.join(" · ")
                    )
                })
                .collect()
        };
        telegram_status_text(
            lines,
            account_lines,
            limit_blocks(self.limits.as_ref(), now),
            heartbeat_lines(self.admin_health.as_ref(), now),
        )
    }
}

#[derive(Debug)]
struct HeartbeatLine {
    text: String,
    ok: bool,
}

fn limit_blocks(limits: Option<&Value>, now: i64) -> Vec<Vec<String>> {
    limits
        .and_then(|value| value["providers"].as_array())
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let provider = entry["provider"].as_str()?.trim();
            if provider.is_empty() {
                return None;
            }
            let windows = entry["windows"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|window| {
                    let usage = window["used_pct"].as_f64()?;
                    if !usage.is_finite() {
                        return None;
                    }
                    let name = window["window"]
                        .as_str()
                        .map(str::trim)
                        .filter(|name| !name.is_empty())
                        .unwrap_or("window");
                    let reset = reset_at_ms(window)
                        .map(|reset| reset_relative_text(reset, now))
                        .unwrap_or_else(|| "reset unknown".to_string());
                    Some(format!(
                        "  {name} — {:.0}% used · {reset}",
                        usage.clamp(0.0, 100.0)
                    ))
                })
                .collect::<Vec<_>>();
            if windows.is_empty() {
                return None;
            }
            let plan = entry["plan"]
                .as_str()
                .map(str::trim)
                .filter(|plan| !plan.is_empty());
            let mut block = vec![match plan {
                Some(plan) => format!("• {provider} — plan {plan}"),
                None => format!("• {provider}"),
            }];
            block.extend(windows);
            Some(block)
        })
        .collect()
}

fn reset_at_ms(window: &Value) -> Option<i64> {
    window["resets_at_s"]
        .as_i64()
        .map(|seconds| seconds.saturating_mul(1000))
        .or_else(|| {
            window["resets_at"]
                .as_str()
                .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
                .map(|value| value.timestamp_millis())
        })
}

fn reset_relative_text(reset: i64, now: i64) -> String {
    if reset >= now {
        format!("resets in {}", compact_duration(reset.saturating_sub(now)))
    } else {
        format!("reset {} ago", compact_duration(now.saturating_sub(reset)))
    }
}

fn heartbeat_lines(admin_health: Option<&Value>, now: i64) -> Vec<HeartbeatLine> {
    let mut latest: Vec<(String, &Value)> = Vec::new();
    for account in admin_health
        .and_then(|value| value["accounts"].as_array())
        .into_iter()
        .flatten()
    {
        let heartbeat = &account["last_heartbeat"];
        if heartbeat["ok"].as_bool().is_none() {
            continue;
        }
        let Some(provider) = account["provider"]
            .as_str()
            .or_else(|| heartbeat["provider"].as_str())
            .map(str::trim)
            .filter(|provider| !provider.is_empty())
        else {
            continue;
        };
        if let Some((_, current)) = latest.iter_mut().find(|(name, _)| name == provider) {
            if heartbeat["ts_ms"].as_i64().unwrap_or(i64::MIN)
                > current["ts_ms"].as_i64().unwrap_or(i64::MIN)
            {
                *current = heartbeat;
            }
        } else {
            latest.push((provider.to_string(), heartbeat));
        }
    }

    latest
        .into_iter()
        .map(|(provider, heartbeat)| {
            let ok = heartbeat["ok"].as_bool().unwrap_or(false);
            let age = heartbeat["ts_ms"]
                .as_i64()
                .map(|timestamp| {
                    format!(
                        "{} ago",
                        compact_duration(now.saturating_sub(timestamp).max(0))
                    )
                })
                .unwrap_or_else(|| "time unknown".to_string());
            let result = if ok { "✓ ok" } else { "✗ fail" };
            let error = (!ok)
                .then(|| heartbeat["message"].as_str())
                .flatten()
                .map(compact_whitespace)
                .filter(|message| !message.is_empty())
                .map(|message| truncate_chars(&message, HEARTBEAT_ERROR_LIMIT));
            HeartbeatLine {
                text: format!(
                    "• {provider} — {result} · {age}{}",
                    error
                        .map(|message| format!(" · {message}"))
                        .unwrap_or_default()
                ),
                ok,
            }
        })
        .collect()
}

fn compact_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn telegram_status_text(
    base_lines: Vec<String>,
    mut account_lines: Vec<String>,
    mut limit_blocks: Vec<Vec<String>>,
    mut heartbeat_lines: Vec<HeartbeatLine>,
) -> String {
    let mut truncated = false;
    loop {
        let text = assemble_telegram_text(
            &base_lines,
            &account_lines,
            &limit_blocks,
            &heartbeat_lines,
            truncated,
        );
        if telegram_len(&text) <= TELEGRAM_MESSAGE_LIMIT {
            return text;
        }
        truncated = true;
        if limit_blocks.pop().is_some() {
            continue;
        }
        if let Some(index) = heartbeat_lines.iter().rposition(|line| line.ok) {
            heartbeat_lines.remove(index);
            continue;
        }
        if heartbeat_lines.pop().is_some() {
            continue;
        }
        if account_lines.pop().is_some() {
            continue;
        }
        return truncate_chars(&text, TELEGRAM_MESSAGE_LIMIT);
    }
}

fn assemble_telegram_text(
    base_lines: &[String],
    account_lines: &[String],
    limit_blocks: &[Vec<String>],
    heartbeat_lines: &[HeartbeatLine],
    truncated: bool,
) -> String {
    let mut lines = base_lines.to_vec();
    lines.extend(account_lines.iter().cloned());
    if !limit_blocks.is_empty() {
        lines.push(String::new());
        lines.push("Subscriptions & limits:".to_string());
        lines.extend(limit_blocks.iter().flatten().cloned());
    }
    if !heartbeat_lines.is_empty() {
        lines.push(String::new());
        lines.push("Ping / health:".to_string());
        lines.extend(heartbeat_lines.iter().map(|line| line.text.clone()));
    }
    if truncated {
        lines.push("… additional status lines omitted".to_string());
    }
    lines.join("\n")
}

fn telegram_len(value: &str) -> usize {
    value.encode_utf16().count()
}

fn truncate_chars(value: &str, max: usize) -> String {
    if telegram_len(value) <= max {
        return value.to_string();
    }
    let mut used = 0;
    let mut output = String::new();
    for character in value.chars() {
        let width = character.len_utf16();
        if used + width >= max {
            break;
        }
        output.push(character);
        used += width;
    }
    output.push('…');
    output
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

    fn synthetic_summary(now: i64) -> StatusSummary {
        StatusSummary {
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
        }
    }

    #[test]
    fn formats_synthetic_status_for_telegram() {
        let now = 1_000_000;
        let summary = synthetic_summary(now);

        let text = summary.telegram_text_at(now);
        assert!(text.contains("Alex v9.8.7 · update available → v10.0.0"));
        assert!(text.contains("Daemon: up · uptime 2h · service active"));
        assert!(text.contains("Dario: ready"));
        assert!(text.contains("openai/work — active · healthy · 42% used · 6h left"));
    }

    #[test]
    fn formats_limits_and_healthy_and_failing_pings() {
        let now = 1_000_000;
        let mut summary = synthetic_summary(now);
        summary.limits = Some(serde_json::json!({
            "providers": [
                {
                    "provider": "openai",
                    "plan": "plus",
                    "windows": [
                        {
                            "window": "5h",
                            "used_pct": 42.4,
                            "resets_at_s": (now + 6 * 3_600_000) / 1000
                        },
                        {
                            "window": "7d",
                            "used_pct": 75.0
                        }
                    ]
                },
                {
                    "provider": "anthropic",
                    "plan": "max",
                    "windows": []
                }
            ]
        }));
        summary.admin_health = Some(serde_json::json!({
            "accounts": [
                {
                    "provider": "openai",
                    "last_heartbeat": {
                        "provider": "openai",
                        "ok": true,
                        "ts_ms": now - 2 * 60_000,
                        "message": "creds ok"
                    }
                },
                {
                    "provider": "anthropic",
                    "last_heartbeat": {
                        "provider": "anthropic",
                        "ok": false,
                        "ts_ms": now - 5 * 60_000,
                        "message": "invalid token\n".to_string() + &"refused ".repeat(30)
                    }
                }
            ]
        }));

        let text = summary.telegram_text_at(now);
        assert!(text.contains("Subscriptions & limits:"));
        assert!(text.contains("• openai — plan plus"));
        assert!(text.contains("  5h — 42% used · resets in 6h"));
        assert!(text.contains("  7d — 75% used · reset unknown"));
        assert!(!text.contains("• anthropic — plan max"));
        assert!(text.contains("Ping / health:"));
        assert!(text.contains("• openai — ✓ ok · 2m ago"));
        assert!(text.contains("• anthropic — ✗ fail · 5m ago · invalid token refused"));
        assert!(text.contains('…'));
        assert!(!text.contains("invalid token\n"));
    }

    #[test]
    fn omits_sections_without_limit_or_heartbeat_data() {
        let now = 1_000_000;
        let mut summary = synthetic_summary(now);
        summary.limits = Some(serde_json::json!({
            "providers": [{"provider": "openai", "plan": "plus", "windows": []}]
        }));
        summary.admin_health = Some(serde_json::json!({
            "accounts": [{"provider": "openai", "last_heartbeat": null}]
        }));

        let text = summary.telegram_text_at(now);
        assert!(!text.contains("Subscriptions & limits:"));
        assert!(!text.contains("Ping / health:"));
        assert!(text.contains("Accounts:"));
    }

    #[test]
    fn telegram_status_stays_within_message_limit() {
        let now = 1_000_000;
        let mut summary = synthetic_summary(now);
        let account = summary.accounts[0].clone();
        summary.accounts = (0..100)
            .map(|index| StatusAccount {
                id: format!("account-{index}"),
                name: format!("{index}-{}", "very-long-name".repeat(40)),
                ..account.clone()
            })
            .collect();

        let text = summary.telegram_text_at(now);
        assert!(telegram_len(&text) <= TELEGRAM_MESSAGE_LIMIT);
        assert!(text.contains("Alex v9.8.7"));
        assert!(text.contains("… additional status lines omitted"));
    }
}
