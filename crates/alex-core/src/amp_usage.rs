//! Amp Free / credits usage parsing (CLI display text + API envelope).
//! Shape matches CodexBar's AmpUsageParser so menu limits stay consistent.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AmpWorkspaceBalance {
    pub name: String,
    pub remaining: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AmpUsageSnapshot {
    pub free_quota: Option<f64>,
    pub free_used: Option<f64>,
    pub free_remaining: Option<f64>,
    pub hourly_replenishment: Option<f64>,
    pub window_hours: Option<f64>,
    pub individual_credits: Option<f64>,
    pub workspace_balances: Vec<AmpWorkspaceBalance>,
    pub account_email: Option<String>,
    pub account_organization: Option<String>,
    pub display_text: String,
}

#[derive(Debug, Deserialize)]
struct ApiEnvelope {
    ok: bool,
    result: Option<ApiResult>,
    error: Option<ApiError>,
}

#[derive(Debug, Deserialize)]
struct ApiResult {
    #[serde(rename = "displayText")]
    display_text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ApiError {
    code: Option<String>,
    message: Option<String>,
}

/// Parse Amp API JSON: `{ "ok": true, "result": { "displayText": "..." } }`.
pub fn parse_usage_api_response(body: &str) -> Result<AmpUsageSnapshot, String> {
    let env: ApiEnvelope =
        serde_json::from_str(body).map_err(|e| format!("invalid Amp usage API JSON: {e}"))?;
    if !env.ok {
        if env.error.as_ref().and_then(|e| e.code.as_deref()) == Some("auth-required") {
            return Err("amp access token is invalid or expired".into());
        }
        return Err(env
            .error
            .and_then(|e| e.message)
            .unwrap_or_else(|| "Amp usage API returned an error".into()));
    }
    let text = env
        .result
        .and_then(|r| r.display_text)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| "missing Amp usage display text".to_string())?;
    parse_usage_display_text(&text)
}

/// Parse `amp usage` / API display text.
pub fn parse_usage_display_text(display_text: &str) -> Result<AmpUsageSnapshot, String> {
    let text = strip_ansi(display_text);
    let mut email = None;
    let mut org = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("Signed in as ") {
            if let Some((e, o)) = rest.split_once(" (") {
                email = Some(e.trim().to_string());
                org = Some(o.trim_end_matches(')').trim().to_string());
            } else {
                email = Some(rest.trim().to_string());
            }
            break;
        }
    }
    if email.is_none() && looks_signed_out(&text) {
        return Err("not logged in to Amp".into());
    }

    let free = parse_free_line(&text);
    let individual = parse_individual_credits(&text);
    let workspace_balances = parse_workspaces(&text);

    if free.is_none() && individual.is_none() && workspace_balances.is_empty() {
        return Err("missing Amp usage data".into());
    }

    let (free_quota, free_remaining, free_used, hourly, window_hours) = match free {
        Some((remaining, quota, hourly)) => {
            let used = (quota - remaining).max(0.0);
            let window = if hourly > 0.0 {
                Some((quota / hourly).max(1.0).round())
            } else {
                None
            };
            (
                Some(quota),
                Some(remaining),
                Some(used),
                Some(hourly),
                window,
            )
        }
        None => (None, None, None, None, None),
    };

    Ok(AmpUsageSnapshot {
        free_quota,
        free_used,
        free_remaining,
        hourly_replenishment: hourly,
        window_hours,
        individual_credits: individual,
        workspace_balances,
        account_email: email,
        account_organization: org,
        display_text: text.trim().to_string(),
    })
}

/// Convert a snapshot into the admin `/limits` provider entry shape.
pub fn usage_to_limits_entry(snap: &AmpUsageSnapshot, plan: Option<&str>) -> Value {
    let mut windows = Vec::new();
    if let (Some(quota), Some(used)) = (snap.free_quota, snap.free_used) {
        let used_pct = if quota > 0.0 {
            ((used / quota) * 100.0).min(100.0)
        } else {
            0.0
        };
        let mut w = serde_json::json!({
            "window": "free",
            "used_pct": used_pct,
            "quota_usd": quota,
            "used_usd": used,
            "remaining_usd": snap.free_remaining,
        });
        if let Some(h) = snap.hourly_replenishment {
            w["hourly_replenishment_usd"] = serde_json::json!(h);
            if h > 0.0 && used > 0.0 {
                let hours = used / h;
                let resets_at_s = (now_s() as f64 + hours * 3600.0) as i64;
                w["resets_at_s"] = serde_json::json!(resets_at_s);
            }
        }
        windows.push(w);
    }
    if let Some(credits) = snap.individual_credits {
        // Remaining paid balance — surface as a credits window for the menu bar.
        windows.push(serde_json::json!({
            "window": "credits",
            "used_pct": 0.0,
            "remaining_usd": credits,
        }));
    }
    for ws in &snap.workspace_balances {
        windows.push(serde_json::json!({
            "window": format!("ws:{}", ws.name),
            "used_pct": 0.0,
            "remaining_usd": ws.remaining,
        }));
    }

    // Any funded individual, workspace, or free balance can serve an Amp
    // request. Do not call an account exhausted merely because its individual
    // balance is zero while a workspace still has credit.
    let workspace_available = snap.workspace_balances.iter().any(|ws| ws.remaining > 0.0);
    let free_available = snap.free_remaining.is_some_and(|credits| credits > 0.0);
    let has_credit_evidence = snap.individual_credits.is_some()
        || !snap.workspace_balances.is_empty()
        || snap.free_remaining.is_some();
    let has_credits = has_credit_evidence.then_some(
        snap.individual_credits.is_some_and(|credits| credits > 0.0)
            || workspace_available
            || free_available,
    );
    let credit_balance = snap
        .individual_credits
        .filter(|credits| *credits > 0.0)
        .or_else(|| {
            snap.workspace_balances
                .iter()
                .map(|ws| ws.remaining)
                .find(|credits| *credits > 0.0)
        })
        .or_else(|| snap.free_remaining.filter(|credits| *credits > 0.0))
        .map(|credits| format!("${credits:.2}"));
    let mut entry = serde_json::json!({
        "provider": "amp",
        "source": "amp usage API",
        "windows": windows,
        "credits": {
            "balance": credit_balance,
            "has_credits": has_credits,
            "unlimited": false,
        },
        "individual_credits_usd": snap.individual_credits,
        "workspace_balances": snap.workspace_balances,
        "account_email": snap.account_email,
        "display_text": snap.display_text,
    });
    if let Some(p) = plan {
        entry["plan"] = serde_json::json!(p);
    } else if let Some(email) = &snap.account_email {
        entry["plan"] = serde_json::json!(email);
    }
    entry
}

fn now_s() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn parse_free_line(text: &str) -> Option<(f64, f64, f64)> {
    // Amp Free: $8.00 / $10.00 remaining (replenishes +$0.42 / hour)
    for line in text.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("Amp Free:") else {
            continue;
        };
        let rest = rest.trim();
        let (left, right) = rest.split_once('/')?;
        let remaining = parse_money(left)?;
        let right = right.trim();
        let (quota_part, after) = right
            .split_once(" remaining")
            .map(|(a, b)| (a.trim(), b))
            .unwrap_or((right, ""));
        let quota = parse_money(quota_part)?;
        let mut hourly = 0.0;
        if let Some(idx) = after.to_lowercase().find("replenishes") {
            let slice = &after[idx..];
            let num: String = slice
                .chars()
                .skip_while(|c| !c.is_ascii_digit() && *c != '.')
                .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == ',')
                .collect();
            hourly = parse_money(&num).unwrap_or(0.0);
        }
        return Some((remaining, quota, hourly));
    }
    None
}

fn parse_individual_credits(text: &str) -> Option<f64> {
    for line in text.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("Individual credits:") else {
            continue;
        };
        let rest = rest.trim();
        let (amt, _) = rest.split_once(" remaining")?;
        return parse_money(amt);
    }
    None
}

fn parse_workspaces(text: &str) -> Vec<AmpWorkspaceBalance> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("Workspace ") else {
            continue;
        };
        let Some((name, tail)) = rest.split_once(':') else {
            continue;
        };
        let tail = tail.trim();
        let Some((amt, _)) = tail.split_once(" remaining") else {
            continue;
        };
        if let Some(remaining) = parse_money(amt) {
            let name = name.trim().to_string();
            if !name.is_empty() {
                out.push(AmpWorkspaceBalance { name, remaining });
            }
        }
    }
    out
}

fn parse_money(s: &str) -> Option<f64> {
    let cleaned: String = s
        .trim()
        .trim_start_matches('$')
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
        .collect();
    if cleaned.is_empty() {
        return None;
    }
    cleaned.parse().ok()
}

fn looks_signed_out(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.contains("not logged in")
        || lower.contains("please log in")
        || lower.contains("ampcode.com/login")
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for d in chars.by_ref() {
                    if ('@'..='~').contains(&d) {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_credits_only_display() {
        let text = "Signed in as me@example.com\nIndividual credits: $20.95 remaining (set up automatic top-up) - https://ampcode.com/settings\n";
        let snap = parse_usage_display_text(text).unwrap();
        assert_eq!(snap.account_email.as_deref(), Some("me@example.com"));
        assert_eq!(snap.individual_credits, Some(20.95));
        assert!(snap.free_quota.is_none());
    }

    #[test]
    fn parse_free_and_workspace() {
        let text = "\
Signed in as dev@amp.dev (Acme)
Amp Free: $8.00 / $10.00 remaining (replenishes +$0.42 / hour)
Individual credits: $1.50 remaining
Workspace Team A: $100.00 remaining
";
        let snap = parse_usage_display_text(text).unwrap();
        assert_eq!(snap.account_email.as_deref(), Some("dev@amp.dev"));
        assert_eq!(snap.account_organization.as_deref(), Some("Acme"));
        assert_eq!(snap.free_remaining, Some(8.0));
        assert_eq!(snap.free_quota, Some(10.0));
        assert_eq!(snap.free_used, Some(2.0));
        assert_eq!(snap.hourly_replenishment, Some(0.42));
        assert_eq!(snap.individual_credits, Some(1.5));
        assert_eq!(snap.workspace_balances.len(), 1);
        assert_eq!(snap.workspace_balances[0].name, "Team A");
        assert_eq!(snap.workspace_balances[0].remaining, 100.0);
    }

    #[test]
    fn parse_api_envelope() {
        let body = r#"{"ok":true,"result":{"displayText":"Signed in as a@b.com\nIndividual credits: $5.00 remaining"}}"#;
        let snap = parse_usage_api_response(body).unwrap();
        assert_eq!(snap.individual_credits, Some(5.0));
        let entry = usage_to_limits_entry(&snap, None);
        assert_eq!(entry["provider"], "amp");
        assert!(entry["windows"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["window"] == "credits"));
    }

    #[test]
    fn free_window_used_pct() {
        let text = "Signed in as a@b.com\nAmp Free: $2.00 / $10.00 remaining\n";
        let snap = parse_usage_display_text(text).unwrap();
        let entry = usage_to_limits_entry(&snap, None);
        let free = entry["windows"].as_array().unwrap()[0].clone();
        assert_eq!(free["window"], "free");
        assert!((free["used_pct"].as_f64().unwrap() - 80.0).abs() < 0.01);
    }
}
