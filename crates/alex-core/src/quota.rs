//! The single, normalized presentation state for a provider's binding quota.
//!
//! A request-rate window is not useful evidence that a credit-metered account
//! can serve traffic.  Keep that decision at the daemon boundary so every
//! client presents the same primary constraint.

use serde_json::{json, Value};

use crate::Provider;

fn boolish(value: &Value) -> Option<bool> {
    value.as_bool().or_else(|| {
        value
            .as_str()
            .and_then(|s| match s.trim().to_ascii_lowercase().as_str() {
                "true" => Some(true),
                "false" => Some(false),
                _ => None,
            })
    })
}

fn non_empty_display(value: &Value) -> Option<String> {
    if let Some(s) = value.as_str() {
        let s = s.trim();
        return (!s.is_empty()).then(|| s.to_string());
    }
    value.as_f64().map(|n| n.to_string())
}

fn top_up_url(provider: Provider) -> &'static str {
    match provider {
        Provider::Xai => "https://grok.com/settings/billing",
        Provider::Amp => "https://ampcode.com/settings",
        _ => "",
    }
}

/// Return the quota a user should treat as binding for this limits entry.
///
/// Only xAI/SuperGrok and Amp are credit-metered here.  OpenAI's captured
/// `credits` headers must not make its subscription 5h/7d windows disappear.
pub fn quota_state(provider: Provider, limits: &Value) -> Value {
    if !matches!(provider, Provider::Xai | Provider::Amp) {
        return json!({"kind": "rate_window", "label": "Rate window"});
    }

    let credits = &limits["credits"];
    if boolish(&credits["unlimited"]) == Some(true) {
        return json!({"kind": "unlimited", "label": "Unlimited credits"});
    }
    if boolish(&credits["has_credits"]) == Some(false) {
        return json!({
            "kind": "out_of_credits",
            "label": "Out of credits",
            "top_up_url": top_up_url(provider),
        });
    }
    if let Some(balance) = non_empty_display(&credits["balance"]) {
        return json!({"kind": "balance", "label": "Credit balance", "balance": balance});
    }

    // Grok's billing endpoint reports a credit percentage even when it cannot
    // report a currency balance.  It is still a credit quota, not a rate limit.
    if let Some(used_pct) = credits["used_pct"].as_f64() {
        return json!({
            "kind": "credit_window",
            "label": "Credit quota",
            "used_pct": used_pct,
            "remaining_pct": (100.0 - used_pct).clamp(0.0, 100.0),
        });
    }

    json!({"kind": "rate_window", "label": "Rate window"})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn out_of_credits_is_binding_even_when_a_window_looks_empty() {
        let state = quota_state(
            Provider::Xai,
            &json!({
                "credits": {"has_credits": false, "unlimited": false},
                "windows": [{"window": "7d", "used_pct": 1.0}],
            }),
        );
        assert_eq!(state["kind"], "out_of_credits");
    }

    #[test]
    fn unlimited_credits_win() {
        let state = quota_state(
            Provider::Xai,
            &json!({
                "credits": {"has_credits": false, "unlimited": true},
            }),
        );
        assert_eq!(state["kind"], "unlimited");
    }

    #[test]
    fn balance_is_presented_for_credit_providers() {
        let state = quota_state(
            Provider::Amp,
            &json!({
                "credits": {"has_credits": true, "balance": "$20.95"},
            }),
        );
        assert_eq!(state["kind"], "balance");
        assert_eq!(state["balance"], "$20.95");
    }

    #[test]
    fn missing_credit_evidence_falls_back_to_rate_window() {
        let state = quota_state(
            Provider::Amp,
            &json!({
                "windows": [{"window": "free", "used_pct": 20.0}],
            }),
        );
        assert_eq!(state["kind"], "rate_window");
    }

    #[test]
    fn subscription_providers_keep_their_rate_windows() {
        let state = quota_state(
            Provider::Openai,
            &json!({
                "credits": {"has_credits": false, "unlimited": false},
                "windows": [{"window": "7d", "used_pct": 1.0}],
            }),
        );
        assert_eq!(state["kind"], "rate_window");
    }

    #[test]
    fn credit_provider_quota_variants_are_normalized() {
        let cases = [
            (
                "string unlimited",
                Provider::Xai,
                json!({"credits": {"unlimited": " TRUE "}}),
                "unlimited",
                None,
            ),
            (
                "string out of credits",
                Provider::Amp,
                json!({"credits": {"has_credits": "false"}}),
                "out_of_credits",
                None,
            ),
            (
                "numeric balance",
                Provider::Amp,
                json!({"credits": {"has_credits": true, "balance": 12.5}}),
                "balance",
                Some("12.5"),
            ),
            (
                "credit utilization",
                Provider::Xai,
                json!({"credits": {"has_credits": true, "used_pct": 37.25}}),
                "credit_window",
                None,
            ),
            (
                "unknown boolean text",
                Provider::Xai,
                json!({"credits": {"has_credits": "unknown"}}),
                "rate_window",
                None,
            ),
        ];

        for (name, provider, limits, kind, balance) in cases {
            let state = quota_state(provider, &limits);
            assert_eq!(state["kind"], kind, "{name}");
            assert_eq!(state["balance"].as_str(), balance, "{name}");
            if name == "credit utilization" {
                assert_eq!(state["remaining_pct"], 62.75);
            }
        }
    }
}
