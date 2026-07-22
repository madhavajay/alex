use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Capability, ErrorClass, HeaderPatch, HookPoint, ResponsePatch, API_VERSION_V1};

fn api_version_v1() -> u16 {
    API_VERSION_V1
}

fn default_enabled() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuleSetV1 {
    #[serde(default = "api_version_v1")]
    pub api_version: u16,
    #[serde(default)]
    pub rules: Vec<RuleSpecV1>,
}

impl Default for RuleSetV1 {
    fn default() -> Self {
        Self {
            api_version: API_VERSION_V1,
            rules: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuleSpecV1 {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Emit an opt-in match diagnostic for every response attempt. Debug rules
    /// may require response-prefix inspection even when the response succeeds.
    #[serde(default)]
    pub debug: bool,
    #[serde(default)]
    pub priority: i32,
    pub hook: HookPoint,
    #[serde(default)]
    pub capabilities: Vec<Capability>,
    #[serde(default)]
    pub when: MatchConditionsV1,
    #[serde(default)]
    pub expression: Option<MatchExpressionV1>,
    #[serde(rename = "then")]
    pub action: ActionSpecV1,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MatchConditionsV1 {
    pub harness_names: Vec<String>,
    pub harness_versions: Vec<String>,
    /// Full Rust regular expressions. Values inside each field are ORed.
    pub harness_name_regex: Vec<String>,
    pub harness_version_regex: Vec<String>,
    /// Matches either the originally requested or current selected model.
    pub models: Vec<String>,
    pub model_regex: Vec<String>,
    pub original_models: Vec<String>,
    pub current_models: Vec<String>,
    pub model_aliases: Vec<String>,
    pub equivalence_classes: Vec<String>,
    /// Optional reasoning effort/thinking level on the incoming request.
    pub efforts: Vec<String>,
    pub providers: Vec<String>,
    pub provider_regex: Vec<String>,
    pub exclude_providers: Vec<String>,
    pub status: Vec<StatusMatcherSpec>,
    pub status_regex: Vec<String>,
    pub response_header_regex: Vec<HeaderRegexMatcherV1>,
    pub error_classes: Vec<ErrorClass>,
    pub error_kinds: Vec<String>,
    pub error_codes: Vec<String>,
    pub error_messages: Vec<String>,
    /// Every phrase must occur.
    pub body_contains: Vec<String>,
    /// At least one phrase must occur.
    pub body_contains_any: Vec<String>,
    pub body_regex: Vec<String>,
    pub body_json_equals: Vec<JsonPointerEqualsV1>,
    pub require_complete_body: bool,
    pub content_types: Vec<String>,
    pub attempt_numbers: Vec<u32>,
    pub same_route_accounts_remaining: Option<bool>,
    pub session_present: Option<bool>,
    pub stable_session: Option<bool>,
}

impl MatchConditionsV1 {
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }

    pub fn needs_body(&self) -> bool {
        !self.body_contains.is_empty()
            || !self.body_contains_any.is_empty()
            || !self.body_regex.is_empty()
            || !self.body_json_equals.is_empty()
    }

    pub fn needs_json_body(&self) -> bool {
        !self.body_json_equals.is_empty()
    }
}

/// Nested composition used by advanced rules. A rule's `when` block and its
/// optional expression are ANDed. Each conditions leaf retains the usual rule:
/// fields are ANDed and values within a field are ORed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MatchExpressionV1 {
    All { all: Vec<MatchExpressionV1> },
    Any { any: Vec<MatchExpressionV1> },
    Not { not: Box<MatchExpressionV1> },
    Conditions { conditions: Box<MatchConditionsV1> },
}

impl MatchExpressionV1 {
    pub fn needs_body(&self) -> bool {
        match self {
            Self::All { all } => all.iter().any(Self::needs_body),
            Self::Any { any } => any.iter().any(Self::needs_body),
            Self::Not { not } => not.needs_body(),
            Self::Conditions { conditions } => conditions.needs_body(),
        }
    }

    pub fn needs_json_body(&self) -> bool {
        match self {
            Self::All { all } => all.iter().any(Self::needs_json_body),
            Self::Any { any } => any.iter().any(Self::needs_json_body),
            Self::Not { not } => not.needs_json_body(),
            Self::Conditions { conditions } => conditions.needs_json_body(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StatusMatcherSpec {
    Exact(u16),
    RangeOrClass(String),
}

impl From<u16> for StatusMatcherSpec {
    fn from(value: u16) -> Self {
        Self::Exact(value)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HeaderRegexMatcherV1 {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonPointerEqualsV1 {
    pub pointer: String,
    pub value: Value,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ActionSpecV1 {
    #[serde(rename = "continue")]
    pub continue_action: bool,
    pub return_original: bool,
    pub retry_same_route: Option<RetrySameRouteSpecV1>,
    pub reroute: Option<RerouteActionSpecV1>,
    pub request_headers: Vec<HeaderPatch>,
    pub response_patches: Vec<ResponsePatch>,
}

impl ActionSpecV1 {
    pub fn terminal_action_count(&self) -> usize {
        usize::from(self.return_original)
            + usize::from(self.retry_same_route.is_some())
            + usize::from(self.reroute.is_some())
    }

    pub fn has_any_action(&self) -> bool {
        self.continue_action
            || self.terminal_action_count() > 0
            || !self.request_headers.is_empty()
            || !self.response_patches.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RetrySameRouteSpecV1 {
    pub exclude_current_account: bool,
    pub reason: String,
    pub max_attempts: Option<u32>,
}

impl Default for RetrySameRouteSpecV1 {
    fn default() -> Self {
        Self {
            exclude_current_account: true,
            reason: "retry another eligible account".to_owned(),
            max_attempts: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderModeV1 {
    #[default]
    Any,
    Only,
    Prefer,
    Exclude,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteScopeKindV1 {
    #[default]
    Request,
    Session,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RerouteActionSpecV1 {
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub equivalent_class: Option<String>,
    #[serde(default)]
    pub providers: Vec<String>,
    #[serde(default)]
    pub provider_mode: ProviderModeV1,
    #[serde(default)]
    pub scope: RouteScopeKindV1,
    #[serde(default)]
    pub ttl_seconds: Option<u64>,
    #[serde(default)]
    pub notice: Option<String>,
    /// Optional reasoning effort/thinking level to apply to the replacement request.
    #[serde(default)]
    pub effort: Option<String>,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub max_attempts: Option<u32>,
    #[serde(default)]
    pub required_capabilities: ModelCapabilityRequirementsV1,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelCapabilityRequirementsV1 {
    pub tools: bool,
    pub vision: bool,
    pub reasoning: bool,
    pub portable_history: bool,
}
