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
    pub email: Option<String>,
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
            let email = account.email();
            StatusAccount {
                id: account.id,
                provider: account.provider.as_str().to_string(),
                name: account.name,
                kind: account.kind,
                label: account.label,
                email,
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
        let daemon = if self.daemon_up {
            self.uptime_s
                .map(|seconds| format!("up {}", compact_duration(seconds * 1000)))
                .unwrap_or_else(|| "up".to_string())
        } else {
            "down".to_string()
        };
        let header = format!(
            "Alex v{} · Daemon {daemon} · Dario {}",
            self.version,
            if self.dario_ready { "ready" } else { "down" }
        );
        let account_blocks = self
            .accounts
            .iter()
            .map(|account| account_block(self, account, now))
            .collect();
        telegram_status_text(header, account_blocks)
    }
}

#[derive(Debug, Clone)]
struct LimitWindow {
    name: String,
    remaining_pct: f64,
    reset: String,
}

fn account_block(summary: &StatusSummary, account: &StatusAccount, now: i64) -> Vec<String> {
    let same_provider = summary
        .accounts
        .iter()
        .filter(|other| other.provider == account.provider)
        .count();
    let local_name = (same_provider > 1).then(|| {
        let name = strip_urls(&compact_whitespace(&account.name));
        format!(" ({})", if name.is_empty() { "account" } else { &name })
    });
    let display_name = format!(
        "{}{}",
        account.provider,
        local_name.as_deref().unwrap_or_default()
    );
    let ping_failed = account.last_heartbeat["ok"].as_bool() == Some(false);
    let failed = account.needs_reauth || ping_failed;
    let mut first = format!("{} {display_name}", if failed { "❌" } else { "✅" });

    let limit_entry = limit_entry(summary.limits.as_ref(), account);
    if failed {
        let reason = if account.needs_reauth {
            "needs reauth".to_string()
        } else {
            let message = account.last_heartbeat["message"]
                .as_str()
                .map(compact_whitespace)
                .map(|message| strip_urls(&message))
                .filter(|message| !message.is_empty())
                .map(|message| truncate_chars(&message, HEARTBEAT_ERROR_LIMIT));
            format!(
                "ping failed{}",
                message
                    .map(|message| format!(": {message}"))
                    .unwrap_or_default()
            )
        };
        first.push_str(&format!(" — {reason}"));
    } else {
        let plan = limit_entry
            .and_then(|entry| plain_plan(entry["plan"].as_str()))
            .or_else(|| plain_plan(account.label.as_deref()));
        let email = account
            .email
            .as_deref()
            .map(compact_whitespace)
            .filter(|email| !email.is_empty() && !contains_url(email));
        let details = plan.into_iter().chain(email).collect::<Vec<_>>();
        if !details.is_empty() {
            first.push_str(&format!(" — {}", details.join(" · ")));
        }
    }

    let mut lines = vec![first];
    if let Some(entry) = limit_entry {
        let windows = limit_windows(entry, now);
        if let Some((tightest_index, tightest)) = windows
            .iter()
            .enumerate()
            .min_by(|(_, left), (_, right)| left.remaining_pct.total_cmp(&right.remaining_pct))
        {
            lines.push(format!(
                "{} {:.0}% left · {} resets {}",
                remaining_bar(tightest.remaining_pct),
                tightest.remaining_pct,
                tightest.name,
                tightest.reset
            ));
            for (index, window) in windows.iter().enumerate() {
                if index != tightest_index && window.remaining_pct < 50.0 {
                    lines.push(format!(
                        "↓ {:.0}% left · {} resets {}",
                        window.remaining_pct, window.name, window.reset
                    ));
                }
            }
        }
        if let Some(balance) = credit_balance_line(entry) {
            lines.push(balance);
        }
    }
    if account.needs_reauth {
        lines.push(format!("→ /reauth {}", account.provider));
    }
    lines
}

fn limit_entry<'a>(limits: Option<&'a Value>, account: &StatusAccount) -> Option<&'a Value> {
    let entries = limits?["providers"].as_array()?;
    entries
        .iter()
        .find(|entry| {
            entry["provider"].as_str() == Some(account.provider.as_str())
                && entry["account_id"].as_str() == Some(account.id.as_str())
        })
        .or_else(|| {
            entries.iter().find(|entry| {
                entry["provider"].as_str() == Some(account.provider.as_str())
                    && entry["account_id"].as_str().is_none()
            })
        })
}

fn limit_windows(entry: &Value, now: i64) -> Vec<LimitWindow> {
    entry["windows"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|window| {
            let used = window["used_pct"].as_f64()?;
            if !used.is_finite() {
                return None;
            }
            let name = window["window"]
                .as_str()
                .map(compact_whitespace)
                .map(|name| strip_urls(&name))
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| "window".to_string());
            let reset = reset_at_ms(window)
                .map(|reset| reset_relative_value(reset, now))
                .unwrap_or_else(|| "unknown".to_string());
            Some(LimitWindow {
                name,
                remaining_pct: (100.0 - used).clamp(0.0, 100.0),
                reset,
            })
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

fn reset_relative_value(reset: i64, now: i64) -> String {
    if reset >= now {
        compact_duration(reset.saturating_sub(now))
    } else {
        format!("{} ago", compact_duration(now.saturating_sub(reset)))
    }
}

fn remaining_bar(remaining_pct: f64) -> String {
    let percent = remaining_pct.round().clamp(0.0, 100.0) as usize;
    let full = percent / 10;
    let partial = percent % 10;
    let mut blocks = vec!["🟩"; full];
    if partial > 0 && blocks.len() < 10 {
        blocks.push(if percent < 20 { "🟥" } else { "🟨" });
    }
    blocks.resize(10, "⬜");
    blocks.join("")
}

fn credit_balance_line(entry: &Value) -> Option<String> {
    if let Some(usd) = entry["individual_credits_usd"].as_f64() {
        if usd != 0.0 || credit_metered_without_windows(entry) {
            return Some(format!("💰 ${usd:.2} credits"));
        }
        return None;
    }
    let balance_value = entry["credits"]
        .get("balance")
        .or_else(|| entry["quota"].get("balance"))?;
    let balance = match balance_value {
        Value::String(value) => value.trim().to_string(),
        Value::Number(value) => value.to_string(),
        _ => return None,
    };
    if balance.is_empty() {
        return None;
    }
    let has_credits = entry["credits"]["has_credits"]
        .as_bool()
        .or_else(|| {
            entry["credits"]["has_credits"]
                .as_str()
                .map(|value| value.eq_ignore_ascii_case("true"))
        })
        .unwrap_or(false);
    let numeric_balance = balance
        .trim_start_matches('$')
        .replace(',', "")
        .parse::<f64>()
        .ok();
    if has_credits
        || numeric_balance.is_some_and(|value| value != 0.0)
        || credit_metered_without_windows(entry)
    {
        Some(format!("💰 {balance} credits"))
    } else {
        None
    }
}

fn credit_metered_without_windows(entry: &Value) -> bool {
    matches!(entry["provider"].as_str(), Some("amp" | "openrouter"))
        && entry["windows"]
            .as_array()
            .is_none_or(|windows| windows.is_empty())
}

fn compact_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn contains_url(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.contains("://") || lower.contains("www.")
}

fn strip_urls(value: &str) -> String {
    value
        .split_whitespace()
        .filter(|word| !contains_url(word))
        .collect::<Vec<_>>()
        .join(" ")
}

fn plain_plan(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    if value.is_empty() || contains_url(value) {
        return None;
    }
    value
        .split_whitespace()
        .next()
        .map(|word| {
            word.trim_matches(|character: char| {
                !character.is_alphanumeric() && !matches!(character, '+' | '-' | '_')
            })
        })
        .filter(|word| !word.is_empty())
        .map(str::to_string)
}

fn telegram_status_text(header: String, mut account_blocks: Vec<Vec<String>>) -> String {
    let mut truncated = false;
    loop {
        let text = assemble_telegram_text(&header, &account_blocks, truncated);
        if telegram_len(&text) <= TELEGRAM_MESSAGE_LIMIT {
            return text;
        }
        truncated = true;
        if account_blocks.pop().is_some() {
            continue;
        }
        return truncate_chars(&text, TELEGRAM_MESSAGE_LIMIT);
    }
}

fn assemble_telegram_text(header: &str, account_blocks: &[Vec<String>], truncated: bool) -> String {
    let mut lines = vec![header.to_string()];
    for block in account_blocks {
        lines.push(String::new());
        lines.extend(block.iter().cloned());
    }
    if account_blocks.is_empty() && !truncated {
        lines.push(String::new());
        lines.push("No accounts configured.".to_string());
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
                label: Some("codex (oauth)".into()),
                email: Some("work@example.com".into()),
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

    fn set_openai_limits(summary: &mut StatusSummary, now: i64) {
        summary.limits = Some(serde_json::json!({
            "providers": [{
                "provider": "openai",
                "plan": "plus",
                "windows": [{
                    "window": "7d",
                    "used_pct": 61.0,
                    "resets_at_s": (now + 12 * 3_600_000) / 1000
                }]
            }]
        }));
    }

    #[test]
    fn amp_credit_balance_without_windows_is_unchanged() {
        let now = 1_000_000;
        let mut summary = synthetic_summary(now);
        summary.accounts[0].provider = "amp".into();
        summary.limits = Some(serde_json::json!({
            "providers": [{
                "provider": "amp",
                "plan": "amp",
                "individual_credits_usd": 30.16
            }]
        }));
        let text = summary.telegram_text_at(now);
        assert!(text.contains("💰 $30.16 credits"), "{text}");
    }

    #[test]
    fn codex_windows_with_disabled_zero_credits_show_bars_without_credits() {
        let now = 1_000_000;
        let mut summary = synthetic_summary(now);
        set_openai_limits(&mut summary, now);
        summary.limits.as_mut().unwrap()["providers"][0]["credits"] = serde_json::json!({
            "has_credits": false,
            "unlimited": false,
            "balance": 0
        });

        let text = summary.telegram_text_at(now);
        println!("{text}");
        assert!(text.contains("🟩🟩🟩🟨⬜⬜⬜⬜⬜⬜ 39% left · 7d resets 12h"));
        assert!(!text.contains("credits"), "{text}");
    }

    #[test]
    fn codex_windows_with_enabled_credits_show_bars_and_credits() {
        let now = 1_000_000;
        let mut summary = synthetic_summary(now);
        set_openai_limits(&mut summary, now);
        summary.limits.as_mut().unwrap()["providers"][0]["credits"] = serde_json::json!({
            "has_credits": true,
            "unlimited": false,
            "balance": 12.34
        });

        let text = summary.telegram_text_at(now);
        println!("{text}");
        assert!(text.contains("🟩🟩🟩🟨⬜⬜⬜⬜⬜⬜ 39% left · 7d resets 12h"));
        assert!(text.contains("💰 12.34 credits"), "{text}");
    }

    #[test]
    fn openrouter_account_shows_dollar_balance() {
        let now = 1_000_000;
        let mut summary = synthetic_summary(now);
        summary.accounts[0].provider = "openrouter".into();
        summary.accounts[0].id = "openrouter-api-key".into();
        summary.limits = Some(serde_json::json!({
            "providers": [{
                "provider": "openrouter",
                "account_id": "openrouter-api-key",
                "individual_credits_usd": 30.25
            }]
        }));

        let text = summary.telegram_text_at(now);
        println!("{text}");
        assert!(text.contains("💰 $30.25 credits"), "{text}");
    }

    #[test]
    fn formats_healthy_account_as_one_entry() {
        let now = 1_000_000;
        let mut summary = synthetic_summary(now);
        set_openai_limits(&mut summary, now);

        let text = summary.telegram_text_at(now);
        assert!(text.starts_with("Alex v9.8.7 · Daemon up 2h · Dario ready"));
        assert!(text.contains("✅ openai — plus · work@example.com"));
        assert!(text.contains("🟩🟩🟩🟨⬜⬜⬜⬜⬜⬜ 39% left · 7d resets 12h"));
        assert!(!text.contains("Accounts:"));
        assert!(!text.contains("Subscriptions & limits:"));
        assert!(!text.contains("Ping / health:"));
    }

    #[test]
    fn failing_ping_keeps_usage_and_shows_inline_reason() {
        let now = 1_000_000;
        let mut summary = synthetic_summary(now);
        set_openai_limits(&mut summary, now);
        summary.accounts[0].last_heartbeat = serde_json::json!({
            "ok": false,
            "message": "connection   timeout"
        });

        let text = summary.telegram_text_at(now);
        assert!(text.contains("❌ openai — ping failed: connection timeout"));
        assert!(text.contains("39% left · 7d resets 12h"));
        assert!(!text.contains("/reauth openai"));
    }

    #[test]
    fn needs_reauth_has_hint_and_usage() {
        let now = 1_000_000;
        let mut summary = synthetic_summary(now);
        set_openai_limits(&mut summary, now);
        summary.accounts[0].needs_reauth = true;

        let text = summary.telegram_text_at(now);
        assert!(text.contains("❌ openai — needs reauth"));
        assert!(text.contains("→ /reauth openai"));
        assert!(text.contains("39% left"));
    }

    #[test]
    fn tightest_window_drives_bar_and_only_other_low_windows_are_listed() {
        let now = 1_000_000;
        let mut summary = synthetic_summary(now);
        summary.limits = Some(serde_json::json!({
            "providers": [{
                "provider": "openai",
                "plan": "plus",
                "windows": [
                    {"window": "5h", "used_pct": 30.0},
                    {"window": "7d", "used_pct": 61.0},
                    {"window": "30d", "used_pct": 55.0}
                ]
            }]
        }));

        let text = summary.telegram_text_at(now);
        assert!(text.contains("🟩🟩🟩🟨⬜⬜⬜⬜⬜⬜ 39% left · 7d resets unknown"));
        assert!(text.contains("↓ 45% left · 30d resets unknown"));
        assert!(!text.contains("70% left · 5h"));
    }

    #[test]
    fn output_never_contains_urls_and_multi_account_names_are_local() {
        let now = 1_000_000;
        let mut summary = synthetic_summary(now);
        let mut personal = summary.accounts[0].clone();
        personal.id = "openai-oauth-personal".into();
        personal.name = "personal".into();
        personal.email = Some("personal@example.com".into());
        summary.accounts.push(personal);
        summary.limits = Some(serde_json::json!({
            "providers": [{
                "provider": "openai",
                "plan": "https://plans.example/plus",
                "windows": [{"window": "https://limits.example/7d", "used_pct": 10.0}]
            }]
        }));

        let text = summary.telegram_text_at(now);
        assert!(text.contains("✅ openai (work) — codex · work@example.com"));
        assert!(text.contains("✅ openai (personal) — codex · personal@example.com"));
        assert!(!text.contains("http://"));
        assert!(!text.contains("https://"));
        assert!(!text.contains("plans.example"));
        assert!(!text.contains("limits.example"));
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
