pub mod amp_usage;
pub mod grok_billing;
pub mod openrouter_catalog;
pub mod quota;
pub mod resume_context;
pub mod translate;

pub use amp_usage::{
    parse_usage_api_response, parse_usage_display_text, usage_to_limits_entry, AmpUsageSnapshot,
    AmpWorkspaceBalance,
};
pub use grok_billing::{
    parse_grpc_web_response, validate_grpc_status_headers, window_label, GrokWebBillingError,
    GrokWebBillingSnapshot, GROK_CREDITS_ENDPOINT, GROK_CREDITS_REQUEST_BODY,
};
pub use openrouter_catalog::parse_models_response as parse_openrouter_models_response;
pub use quota::quota_state;
pub use resume_context::{
    build_resume_context, build_resume_context_from_captures, request_entries, ResumeCapture,
    ResumeContext, ResumeEntry,
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Anthropic,
    Openai,
    Gemini,
    Xai,
    Openrouter,
    Exo,
    /// Amp subscription / credits (billing + wrap harness; not a /v1 upstream route yet).
    Amp,
    /// Kimi Code (Moonshot AI) subscription — OAuth device flow, OpenAI-compatible
    /// upstream at `https://api.kimi.com/coding/v1`.
    Kimi,
}

impl Provider {
    pub fn as_str(self) -> &'static str {
        match self {
            Provider::Anthropic => "anthropic",
            Provider::Openai => "openai",
            Provider::Gemini => "gemini",
            Provider::Xai => "xai",
            Provider::Openrouter => "openrouter",
            Provider::Exo => "exo",
            Provider::Amp => "amp",
            Provider::Kimi => "kimi",
        }
    }

    pub fn from_str_loose(s: &str) -> Option<Provider> {
        match s.to_lowercase().as_str() {
            "anthropic" | "claude" => Some(Provider::Anthropic),
            "openai" | "codex" | "chatgpt" => Some(Provider::Openai),
            "gemini" | "google" => Some(Provider::Gemini),
            "xai" | "grok" => Some(Provider::Xai),
            "openrouter" | "or" => Some(Provider::Openrouter),
            "exo" => Some(Provider::Exo),
            "amp" | "ampcode" => Some(Provider::Amp),
            "kimi" | "kimi-code" | "moonshot" => Some(Provider::Kimi),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientFormat {
    AnthropicMessages,
    OpenaiChat,
    OpenaiResponses,
    GeminiGenerate,
}

impl ClientFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            ClientFormat::AnthropicMessages => "anthropic",
            ClientFormat::OpenaiChat => "openai-chat",
            ClientFormat::OpenaiResponses => "openai-responses",
            ClientFormat::GeminiGenerate => "gemini",
        }
    }

    pub fn default_provider(self) -> Provider {
        match self {
            ClientFormat::AnthropicMessages => Provider::Anthropic,
            ClientFormat::OpenaiChat | ClientFormat::OpenaiResponses => Provider::Openai,
            ClientFormat::GeminiGenerate => Provider::Gemini,
        }
    }
}

const PREFIXES: &[(&str, Provider)] = &[
    ("claude:", Provider::Anthropic),
    ("anthropic:", Provider::Anthropic),
    ("openai:", Provider::Openai),
    ("codex:", Provider::Openai),
    ("gemini:", Provider::Gemini),
    ("grok:", Provider::Xai),
    ("xai:", Provider::Xai),
    ("openrouter:", Provider::Openrouter),
    ("exo:", Provider::Exo),
    ("kimi:", Provider::Kimi),
    ("claude/", Provider::Anthropic),
    ("anthropic/", Provider::Anthropic),
    ("openai/", Provider::Openai),
    ("codex/", Provider::Openai),
    ("chatgpt/", Provider::Openai),
    ("gemini/", Provider::Gemini),
    ("google/", Provider::Gemini),
    ("grok/", Provider::Xai),
    ("xai/", Provider::Xai),
    ("openrouter/", Provider::Openrouter),
    ("exo/", Provider::Exo),
    ("kimi/", Provider::Kimi),
];

// Claude Code gateway discovery only accepts model ids beginning with
// `claude` or `anthropic`. Alexandria publishes `claude-alex/<model>` aliases
// to that client and removes the compatibility prefix before normal routing.
const PASSTHROUGH: &[&str] = &["claude-alex/", "cove/", "alexandria/", "alex/"];

const ALIASES: &[(&str, &str)] = &[
    ("opus-4.8", "claude-opus-4-8"),
    ("opus-4.5", "claude-opus-4-5"),
    ("sonnet-5", "claude-sonnet-5"),
    ("sonnet-4.5", "claude-sonnet-4-5"),
    ("haiku-4.5", "claude-haiku-4-5"),
];

pub fn model_aliases() -> &'static [(&'static str, &'static str)] {
    ALIASES
}

fn hs<'a>(h: &'a Value, key: &str) -> Option<&'a str> {
    h.get(key).and_then(|v| v.as_str())
}

fn hf(h: &Value, key: &str) -> Option<f64> {
    hs(h, key).and_then(|s| s.parse().ok())
}

fn hi(h: &Value, key: &str) -> Option<i64> {
    hs(h, key).and_then(|s| s.parse().ok())
}

pub fn parse_limit_headers(provider: Provider, h: &Value) -> Value {
    match provider {
        Provider::Anthropic => {
            let mut windows = Vec::new();
            for (name, prefix) in [
                ("5h", "anthropic-ratelimit-unified-5h"),
                ("7d", "anthropic-ratelimit-unified-7d"),
            ] {
                if let Some(util) = hf(h, &format!("{prefix}-utilization")) {
                    windows.push(serde_json::json!({
                        "window": name,
                        "used_pct": util * 100.0,
                        "status": hs(h, &format!("{prefix}-status")),
                        "resets_at_s": hi(h, &format!("{prefix}-reset")),
                    }));
                }
            }
            serde_json::json!({
                "windows": windows,
                "representative_window": hs(h, "anthropic-ratelimit-unified-representative-claim"),
                "overage": {
                    "status": hs(h, "anthropic-ratelimit-unified-overage-status"),
                    "reason": hs(h, "anthropic-ratelimit-unified-overage-disabled-reason"),
                },
            })
        }
        Provider::Openai => {
            let mut windows = Vec::new();
            for prefix in ["x-codex-primary", "x-codex-secondary"] {
                if let Some(used) = hf(h, &format!("{prefix}-used-percent")) {
                    let minutes = hi(h, &format!("{prefix}-window-minutes"));
                    let name = match minutes {
                        Some(300) => "5h".to_string(),
                        Some(10080) => "7d".to_string(),
                        Some(m) => format!("{m}m"),
                        None => "unknown".to_string(),
                    };
                    windows.push(serde_json::json!({
                        "window": name,
                        "used_pct": used,
                        "resets_at_s": hi(h, &format!("{prefix}-reset-at")),
                    }));
                }
            }
            serde_json::json!({
                "plan": hs(h, "x-codex-plan-type"),
                "active_limit": hs(h, "x-codex-active-limit"),
                "windows": windows,
                "credits": {
                    "balance": hs(h, "x-codex-credits-balance"),
                    "has_credits": hs(h, "x-codex-credits-has-credits"),
                    "unlimited": hs(h, "x-codex-credits-unlimited"),
                },
            })
        }
        // xAI's response carries only per-minute API rate-limit headers
        // (`x-ratelimit-*`), not subscription usage. Surfacing those as a usage
        // card is misleading: "120/120 requests" and "5m tokens" reset every
        // minute and say nothing about SuperGrok consumption, and — having no
        // `windows` — they render as bare text instead of a progress bar. Real
        // Grok usage comes from the grok.com billing endpoint (see
        // `xai_usage_entry`), which produces proper credit windows that draw as
        // a bar. Emit an empty window set so the header fallback keeps the
        // provider row (and its health dot) present without inventing numbers.
        Provider::Xai => serde_json::json!({ "windows": [] }),
        Provider::Gemini
        | Provider::Openrouter
        | Provider::Exo
        | Provider::Amp
        | Provider::Kimi => Value::Null,
    }
}

fn resolve_alias(model: &str) -> String {
    for (alias, full) in ALIASES {
        if model == *alias {
            return full.to_string();
        }
    }
    model.to_string()
}

pub fn route_model(model: &str) -> (Option<Provider>, String) {
    for prefix in PASSTHROUGH {
        if let Some(rest) = model.strip_prefix(prefix) {
            return route_model(rest);
        }
    }
    for (prefix, provider) in PREFIXES {
        if let Some(rest) = model.strip_prefix(prefix) {
            return (Some(*provider), resolve_alias(rest));
        }
    }
    let model = resolve_alias(model);
    let lower = model.to_lowercase();
    let inferred = if lower.starts_with("claude") {
        Some(Provider::Anthropic)
    } else if lower.starts_with("gpt")
        || lower.starts_with("codex")
        || lower.starts_with("chatgpt")
        || is_o_series(&lower)
    {
        Some(Provider::Openai)
    } else if lower.starts_with("gemini") {
        Some(Provider::Gemini)
    } else if lower.starts_with("grok") {
        Some(Provider::Xai)
    } else if lower.starts_with("kimi") || lower == "k3" {
        Some(Provider::Kimi)
    } else {
        None
    };
    (inferred, model)
}

fn is_o_series(lower: &str) -> bool {
    let mut chars = lower.chars();
    chars.next() == Some('o') && chars.next().map(|c| c.is_ascii_digit()).unwrap_or(false)
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: Option<i64>,
    pub cached_input_tokens: Option<i64>,
    pub cache_creation_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub reasoning_tokens: Option<i64>,
}

impl Usage {
    pub fn merge(&mut self, other: Usage) {
        if other.input_tokens.is_some() {
            self.input_tokens = other.input_tokens;
        }
        if other.cached_input_tokens.is_some() {
            self.cached_input_tokens = other.cached_input_tokens;
        }
        if other.cache_creation_tokens.is_some() {
            self.cache_creation_tokens = other.cache_creation_tokens;
        }
        if other.output_tokens.is_some() {
            self.output_tokens = other.output_tokens;
        }
        if other.reasoning_tokens.is_some() {
            self.reasoning_tokens = other.reasoning_tokens;
        }
    }

    pub fn is_empty(&self) -> bool {
        self.input_tokens.is_none() && self.output_tokens.is_none()
    }
}

fn path_i64(v: &Value, path: &[&str]) -> Option<i64> {
    let mut cur = v;
    for p in path {
        cur = cur.get(p)?;
    }
    cur.as_i64()
}

pub fn usage_from_obj(o: &Value) -> Usage {
    Usage {
        input_tokens: path_i64(o, &["input_tokens"])
            .or_else(|| path_i64(o, &["prompt_tokens"]))
            .or_else(|| path_i64(o, &["promptTokenCount"])),
        cached_input_tokens: path_i64(o, &["cache_read_input_tokens"])
            .or_else(|| path_i64(o, &["prompt_tokens_details", "cached_tokens"]))
            .or_else(|| path_i64(o, &["input_tokens_details", "cached_tokens"]))
            .or_else(|| path_i64(o, &["cachedContentTokenCount"])),
        cache_creation_tokens: path_i64(o, &["cache_creation_input_tokens"]),
        output_tokens: path_i64(o, &["output_tokens"])
            .or_else(|| path_i64(o, &["completion_tokens"]))
            .or_else(|| path_i64(o, &["candidatesTokenCount"])),
        reasoning_tokens: path_i64(o, &["completion_tokens_details", "reasoning_tokens"])
            .or_else(|| path_i64(o, &["output_tokens_details", "reasoning_tokens"]))
            .or_else(|| path_i64(o, &["thoughtsTokenCount"])),
    }
}

pub fn usage_from_json(v: &Value) -> Usage {
    let mut usage = Usage::default();
    for loc in [
        &v["usage"],
        &v["message"]["usage"],
        &v["response"]["usage"],
        &v["usageMetadata"],
        &v["response"]["usageMetadata"],
    ] {
        if loc.is_object() {
            usage.merge(usage_from_obj(loc));
        }
    }
    usage
}

pub fn parse_sse_usage(body: &str) -> Usage {
    let mut usage = Usage::default();
    for line in body.lines() {
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(data) else {
            continue;
        };
        usage.merge(usage_from_json(&v));
    }
    usage
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pricing {
    pub input_per_m: f64,
    pub cached_input_per_m: f64,
    pub cache_creation_per_m: f64,
    pub output_per_m: f64,
}

pub fn compute_cost(usage: &Usage, pricing: &Pricing, input_includes_cached: bool) -> f64 {
    let input = usage.input_tokens.unwrap_or(0) as f64;
    let cached = usage.cached_input_tokens.unwrap_or(0) as f64;
    let creation = usage.cache_creation_tokens.unwrap_or(0) as f64;
    let output = usage.output_tokens.unwrap_or(0) as f64;
    let uncached_input = if input_includes_cached {
        (input - cached).max(0.0)
    } else {
        input
    };
    (uncached_input * pricing.input_per_m
        + cached * pricing.cached_input_per_m
        + creation * pricing.cache_creation_per_m
        + output * pricing.output_per_m)
        / 1_000_000.0
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct TraceRecord {
    pub id: String,
    pub ts_request_ms: i64,
    pub ts_response_ms: Option<i64>,
    pub session_id: Option<String>,
    pub harness: Option<String>,
    pub client_format: Option<String>,
    pub upstream_provider: Option<String>,
    pub upstream_format: Option<String>,
    pub requested_model: Option<String>,
    pub routed_model: Option<String>,
    pub method: Option<String>,
    pub path: Option<String>,
    pub status: Option<i64>,
    pub streamed: Option<bool>,
    pub usage: Usage,
    pub cost_usd: Option<f64>,
    pub billing_bucket: Option<String>,
    pub req_body_path: Option<String>,
    pub upstream_req_body_path: Option<String>,
    pub resp_body_path: Option<String>,
    pub req_headers_json: Option<String>,
    pub resp_headers_json: Option<String>,
    pub error: Option<String>,
    /// Provider supplied error type (or a synthetic transport failure tag).
    #[serde(default)]
    pub error_kind: Option<String>,
    /// Provider supplied error code; falls back to the HTTP status for HTTP errors.
    #[serde(default)]
    pub error_code: Option<String>,
    /// Alexandria's stable, coarse error taxonomy.
    #[serde(default)]
    pub error_class: Option<String>,
    /// True when Alexandria retried this request through another account or model.
    #[serde(default)]
    pub substituted: bool,
    /// Model the client asked for before any configured fallback was considered.
    #[serde(default)]
    pub original_model: Option<String>,
    /// Model selected for the final upstream attempt.
    #[serde(default)]
    pub served_model: Option<String>,
    /// Reroutable error class that caused the most recent reroute.
    #[serde(default)]
    pub substitution_reason: Option<String>,
    /// True when the response was produced by an operator session injection.
    #[serde(default)]
    pub injected: bool,
    /// Name of the fixture used for an injected response, when applicable.
    #[serde(default)]
    pub fixture_name: Option<String>,
    /// JSON array of account/model attempts, kept as text for SQLite portability.
    #[serde(default)]
    pub attempts: Option<String>,
    #[serde(default)]
    pub original_account_id: Option<String>,
    #[serde(default)]
    pub served_account_id: Option<String>,
    pub account_id: Option<String>,
    /// Durable upstream subscription identity. Unlike `account_id`, this must
    /// not be derived from the user-editable local account nickname.
    #[serde(default)]
    pub subscription_identity: Option<String>,
    /// True when the request's connection was brokered by a Dario generation.
    /// This is operational metadata only; account attribution remains the
    /// underlying provider account.
    #[serde(default)]
    pub via_dario: bool,
    /// The Dario generation that brokered this request, if any.
    #[serde(default)]
    pub dario_generation: Option<String>,
    pub run_id: Option<String>,
    pub tags: Option<String>,
    pub client_ip: Option<String>,
    pub key_fingerprint: Option<String>,
    pub reasoning_effort: Option<String>,
    pub thinking_budget: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceIngestPayload {
    pub trace: TraceRecord,
    pub request_body_b64: Option<String>,
    pub upstream_request_body_b64: Option<String>,
    pub response_body_b64: Option<String>,
}

pub fn parse_trace_tags(values: &[&str]) -> Value {
    let mut map = serde_json::Map::new();
    for v in values {
        for piece in v.split(',') {
            let Some((k, val)) = piece.split_once('=') else {
                continue;
            };
            let k = k.trim();
            if k.is_empty() {
                continue;
            }
            map.insert(k.to_string(), Value::String(val.trim().to_string()));
        }
    }
    Value::Object(map)
}

pub fn conversation_root(format: ClientFormat, body: &Value) -> Option<String> {
    let (system, user) = match format {
        ClientFormat::AnthropicMessages => {
            let system = translate::txt(&body["system"]);
            let user = body["messages"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|m| m["role"] == "user")
                .map(|m| translate::txt(&m["content"]))
                .unwrap_or_default();
            (system, user)
        }
        ClientFormat::OpenaiChat => {
            let msgs = body["messages"].as_array();
            let find = |roles: &[&str]| {
                msgs.into_iter()
                    .flatten()
                    .find(|m| roles.contains(&m["role"].as_str().unwrap_or("")))
                    .map(|m| translate::txt(&m["content"]))
                    .unwrap_or_default()
            };
            (find(&["system", "developer"]), find(&["user"]))
        }
        ClientFormat::OpenaiResponses => {
            let system = body["instructions"].as_str().unwrap_or("").to_string();
            let user = match &body["input"] {
                Value::String(s) => s.clone(),
                Value::Array(items) => items
                    .iter()
                    .find(|it| {
                        it["role"] == "user"
                            && it["type"].as_str().unwrap_or("message") == "message"
                    })
                    .map(|it| translate::txt(&it["content"]))
                    .unwrap_or_default(),
                _ => String::new(),
            };
            (system, user)
        }
        ClientFormat::GeminiGenerate => {
            let system = translate::gemini_parts_text(&body["systemInstruction"]["parts"]);
            let user = body["contents"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|c| c["role"].as_str().unwrap_or("user") == "user")
                .map(|c| translate::gemini_parts_text(&c["parts"]))
                .unwrap_or_default();
            (system, user)
        }
    };
    let system = system.trim();
    let user = user.trim();
    if system.is_empty() && user.is_empty() {
        None
    } else {
        Some(format!("{system}\n{user}"))
    }
}

pub fn parse_since(s: &str, now_ms: i64) -> Option<i64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(dt.timestamp_millis());
    }
    let unit = s.chars().last()?;
    let num = &s[..s.len() - unit.len_utf8()];
    let n: i64 = num.parse().ok()?;
    if n < 0 {
        return None;
    }
    let ms = match unit {
        's' => n.checked_mul(1_000)?,
        'm' => n.checked_mul(60_000)?,
        'h' => n.checked_mul(3_600_000)?,
        'd' => n.checked_mul(86_400_000)?,
        _ => return None,
    };
    Some(now_ms - ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_prefixes() {
        assert_eq!(
            route_model("claude:claude-sonnet-4-5").0,
            Some(Provider::Anthropic)
        );
        assert_eq!(route_model("openai:gpt-5.1").1, "gpt-5.1");
        assert_eq!(route_model("gpt-5-codex").0, Some(Provider::Openai));
        assert_eq!(route_model("o3-mini").0, Some(Provider::Openai));
        assert_eq!(
            route_model("kimi-for-coding"),
            (
                Some(Provider::Kimi),
                "kimi-for-coding".to_string()
            )
        );
        assert_eq!(route_model("mystery-model").0, None);
    }

    #[test]
    fn routes_slash_prefixes() {
        assert_eq!(
            route_model("claude/claude-sonnet-4-5"),
            (Some(Provider::Anthropic), "claude-sonnet-4-5".to_string())
        );
        assert_eq!(route_model("anthropic/x").0, Some(Provider::Anthropic));
        assert_eq!(route_model("openai/gpt-5.5").1, "gpt-5.5");
        assert_eq!(route_model("codex/gpt-5-codex").0, Some(Provider::Openai));
        assert_eq!(route_model("chatgpt/gpt-5.1").0, Some(Provider::Openai));
        assert_eq!(route_model("gemini/gemini-3-pro").0, Some(Provider::Gemini));
        assert_eq!(route_model("google/gemini-3-pro").0, Some(Provider::Gemini));
        assert_eq!(route_model("grok/grok-4").0, Some(Provider::Xai));
        assert_eq!(route_model("xai/grok-4").0, Some(Provider::Xai));
        assert_eq!(
            route_model("exo/mlx-community/Meta-Llama-3.1-8B-Instruct-4bit"),
            (
                Some(Provider::Exo),
                "mlx-community/Meta-Llama-3.1-8B-Instruct-4bit".to_string()
            )
        );
        assert_eq!(
            route_model("alexandria/openrouter/anthropic/claude-3.5-sonnet"),
            (
                Some(Provider::Openrouter),
                "anthropic/claude-3.5-sonnet".to_string()
            )
        );
        assert_eq!(
            route_model("kimi/k3"),
            (Some(Provider::Kimi), "k3".to_string())
        );
        assert_eq!(
            route_model("kimi:kimi-for-coding"),
            (Some(Provider::Kimi), "kimi-for-coding".to_string())
        );
        assert_eq!(
            route_model("alex/kimi/k3"),
            (Some(Provider::Kimi), "k3".to_string())
        );
        assert_eq!(Provider::from_str_loose("kimi"), Some(Provider::Kimi));
        assert_eq!(Provider::Kimi.as_str(), "kimi");
    }

    #[test]
    fn routes_passthrough_prefixes() {
        assert_eq!(
            route_model("claude-alex/gpt-5.5"),
            (Some(Provider::Openai), "gpt-5.5".to_string())
        );
        assert_eq!(
            route_model("claude-alex/grok-4.5"),
            (Some(Provider::Xai), "grok-4.5".to_string())
        );
        assert_eq!(
            route_model("alexandria/gpt-5.5"),
            (Some(Provider::Openai), "gpt-5.5".to_string())
        );
        assert_eq!(
            route_model("alex/gpt-5.5"),
            (Some(Provider::Openai), "gpt-5.5".to_string())
        );
        assert_eq!(
            route_model("alex/claude-fable-5"),
            (Some(Provider::Anthropic), "claude-fable-5".to_string())
        );
        assert_eq!(
            route_model("alex/grok-4.5"),
            (Some(Provider::Xai), "grok-4.5".to_string())
        );
        assert_eq!(
            route_model("cove/claude-opus-4-8"),
            (Some(Provider::Anthropic), "claude-opus-4-8".to_string())
        );
        assert_eq!(
            route_model("cove/openai:gpt-5.1"),
            (Some(Provider::Openai), "gpt-5.1".to_string())
        );
    }

    #[test]
    fn xai_rate_headers_do_not_masquerade_as_usage() {
        // api.x.ai returns per-minute rate-limit ceilings. These are not
        // subscription usage and must not be surfaced as a "120/120 requests /
        // 5m tokens" pseudo-usage card. Real Grok usage flows through the
        // grok.com billing path instead.
        let headers = serde_json::json!({
            "x-ratelimit-limit-requests": "120",
            "x-ratelimit-remaining-requests": "120",
            "x-ratelimit-limit-tokens": "5000000",
            "x-ratelimit-remaining-tokens": "5000000",
        });
        let parsed = parse_limit_headers(Provider::Xai, &headers);
        assert!(parsed.get("requests").is_none(), "must not emit request counts");
        assert!(parsed.get("tokens").is_none(), "must not emit token counts");
        assert_eq!(
            parsed["windows"].as_array().map(Vec::len),
            Some(0),
            "no bogus windows from rate-limit headers"
        );
    }

    #[test]
    fn openai_headers_still_produce_a_windowed_usage_bar() {
        // Guard the contract the menu relies on: providers with real usage
        // windows render as bars. Only xai's meaningless rate headers were
        // dropped above.
        let headers = serde_json::json!({
            "x-codex-plan-type": "plus",
            "x-codex-primary-used-percent": "42",
            "x-codex-primary-window-minutes": "300",
        });
        let parsed = parse_limit_headers(Provider::Openai, &headers);
        assert_eq!(parsed["plan"], "plus");
        assert_eq!(parsed["windows"][0]["window"], "5h");
        assert_eq!(parsed["windows"][0]["used_pct"], 42.0);
    }

    #[test]
    fn routes_aliases() {
        assert_eq!(
            route_model("k3"),
            (Some(Provider::Kimi), "k3".to_string())
        );
        assert_eq!(
            route_model("opus-4.8"),
            (Some(Provider::Anthropic), "claude-opus-4-8".to_string())
        );
        assert_eq!(
            route_model("opus-4.5"),
            (Some(Provider::Anthropic), "claude-opus-4-5".to_string())
        );
        assert_eq!(
            route_model("sonnet-5"),
            (Some(Provider::Anthropic), "claude-sonnet-5".to_string())
        );
        assert_eq!(
            route_model("sonnet-4.5"),
            (Some(Provider::Anthropic), "claude-sonnet-4-5".to_string())
        );
        assert_eq!(
            route_model("haiku-4.5"),
            (Some(Provider::Anthropic), "claude-haiku-4-5".to_string())
        );
        assert_eq!(
            route_model("claude/opus-4.8"),
            (Some(Provider::Anthropic), "claude-opus-4-8".to_string())
        );
        assert_eq!(
            route_model("alexandria/sonnet-5"),
            (Some(Provider::Anthropic), "claude-sonnet-5".to_string())
        );
        assert_eq!(model_aliases().len(), 5);
    }

    #[test]
    fn kimi_bare_routing_does_not_change_existing_families() {
        assert_eq!(
            route_model("claude-sonnet-4-5"),
            (Some(Provider::Anthropic), "claude-sonnet-4-5".to_string())
        );
        assert_eq!(
            route_model("gpt-5.1"),
            (Some(Provider::Openai), "gpt-5.1".to_string())
        );
        assert_eq!(
            route_model("gemini-2.5-pro"),
            (Some(Provider::Gemini), "gemini-2.5-pro".to_string())
        );
        assert_eq!(
            route_model("grok-4"),
            (Some(Provider::Xai), "grok-4".to_string())
        );
        assert_eq!(
            route_model("mystery-model"),
            (None, "mystery-model".to_string())
        );
    }

    #[test]
    fn parses_anthropic_sse() {
        let sse = "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":10,\"cache_read_input_tokens\":4,\"output_tokens\":1}}}\n\nevent: message_delta\ndata: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":25}}\n\n";
        let u = parse_sse_usage(sse);
        assert_eq!(u.input_tokens, Some(10));
        assert_eq!(u.cached_input_tokens, Some(4));
        assert_eq!(u.output_tokens, Some(25));
    }

    #[test]
    fn parses_trace_tags() {
        let v = parse_trace_tags(&["suite=swebench", "case=astropy-123", "malformed", "=nokey"]);
        assert_eq!(v["suite"], "swebench");
        assert_eq!(v["case"], "astropy-123");
        assert_eq!(v.as_object().unwrap().len(), 2);
        assert_eq!(parse_trace_tags(&[]), serde_json::json!({}));
        let padded = parse_trace_tags(&[" k = v "]);
        assert_eq!(padded["k"], "v");
    }

    #[test]
    fn parses_coalesced_trace_tags() {
        let v = parse_trace_tags(&["harness=codex,task=smoke,model=gpt-5.5"]);
        assert_eq!(v["harness"], "codex");
        assert_eq!(v["task"], "smoke");
        assert_eq!(v["model"], "gpt-5.5");
        assert_eq!(v.as_object().unwrap().len(), 3);
        let mixed = parse_trace_tags(&["a=1, b = 2 ,junk,=x", "c=3"]);
        assert_eq!(mixed["a"], "1");
        assert_eq!(mixed["b"], "2");
        assert_eq!(mixed["c"], "3");
        assert_eq!(mixed.as_object().unwrap().len(), 3);
    }

    #[test]
    fn conversation_root_anthropic() {
        let body = serde_json::json!({
            "system": [{"type": "text", "text": "sys"}],
            "messages": [
                {"role": "assistant", "content": "prior"},
                {"role": "user", "content": [{"type": "text", "text": "hi"}]},
            ],
        });
        assert_eq!(
            conversation_root(ClientFormat::AnthropicMessages, &body),
            Some("sys\nhi".to_string())
        );
        let plain = serde_json::json!({
            "system": "s",
            "messages": [{"role": "user", "content": "u"}],
        });
        assert_eq!(
            conversation_root(ClientFormat::AnthropicMessages, &plain),
            Some("s\nu".to_string())
        );
        assert_eq!(
            conversation_root(ClientFormat::AnthropicMessages, &serde_json::json!({})),
            None
        );
    }

    #[test]
    fn conversation_root_openai_chat() {
        let body = serde_json::json!({
            "messages": [
                {"role": "developer", "content": "dev"},
                {"role": "user", "content": [{"type": "text", "text": "q"}]},
            ],
        });
        assert_eq!(
            conversation_root(ClientFormat::OpenaiChat, &body),
            Some("dev\nq".to_string())
        );
        let user_only = serde_json::json!({"messages": [{"role": "user", "content": "solo"}]});
        assert_eq!(
            conversation_root(ClientFormat::OpenaiChat, &user_only),
            Some("\nsolo".to_string())
        );
        assert_eq!(
            conversation_root(
                ClientFormat::OpenaiChat,
                &serde_json::json!({"messages": []})
            ),
            None
        );
    }

    #[test]
    fn conversation_root_openai_responses() {
        let body = serde_json::json!({
            "instructions": "inst",
            "input": [
                {"type": "message", "role": "user",
                 "content": [{"type": "input_text", "text": "first"}]},
            ],
        });
        assert_eq!(
            conversation_root(ClientFormat::OpenaiResponses, &body),
            Some("inst\nfirst".to_string())
        );
        let string_input = serde_json::json!({"input": "plain"});
        assert_eq!(
            conversation_root(ClientFormat::OpenaiResponses, &string_input),
            Some("\nplain".to_string())
        );
        assert_eq!(
            conversation_root(
                ClientFormat::OpenaiResponses,
                &serde_json::json!({"input": []})
            ),
            None
        );
    }

    #[test]
    fn parses_since_relative() {
        let now = 1_000_000_000_000;
        assert_eq!(parse_since("45s", now), Some(now - 45_000));
        assert_eq!(parse_since("30m", now), Some(now - 1_800_000));
        assert_eq!(parse_since("2h", now), Some(now - 7_200_000));
        assert_eq!(parse_since("7d", now), Some(now - 604_800_000));
    }

    #[test]
    fn parses_since_rfc3339() {
        assert_eq!(
            parse_since("2024-01-01T00:00:00Z", 0),
            Some(1_704_067_200_000)
        );
        assert_eq!(
            parse_since("2024-01-01T02:00:00+02:00", 0),
            Some(1_704_067_200_000)
        );
    }

    #[test]
    fn rejects_garbage_since() {
        assert_eq!(parse_since("", 0), None);
        assert_eq!(parse_since("yesterday", 0), None);
        assert_eq!(parse_since("30x", 0), None);
        assert_eq!(parse_since("m", 0), None);
        assert_eq!(parse_since("-5m", 0), None);
        assert_eq!(parse_since("3é", 0), None);
    }

    #[test]
    fn parses_openai_responses_sse() {
        let sse = "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":100,\"input_tokens_details\":{\"cached_tokens\":20},\"output_tokens\":30,\"output_tokens_details\":{\"reasoning_tokens\":5}}}}\n";
        let u = parse_sse_usage(sse);
        assert_eq!(u.input_tokens, Some(100));
        assert_eq!(u.cached_input_tokens, Some(20));
        assert_eq!(u.reasoning_tokens, Some(5));
    }
}
