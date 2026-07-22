use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::SafeHeaders;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookPoint {
    RequestReceived,
    RoutePlanned,
    AttemptResult,
    ResponseReady,
    TraceFinalized,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HookEnvelopeV1<T> {
    pub api_version: u16,
    pub hook: HookPoint,
    pub middleware_run_id: String,
    pub context: T,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessView {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionIdSource {
    NativeHeader,
    Hook,
    RequestBody,
    PromptCacheKey,
    DerivedConversationRoot,
    #[default]
    Unknown,
}

impl SessionIdSource {
    pub fn is_stable(self) -> bool {
        matches!(self, Self::NativeHeader | Self::Hook | Self::RequestBody)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionView {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub run_id: Option<String>,
    #[serde(default)]
    pub source: SessionIdSource,
    #[serde(default)]
    pub active_route_lease: Option<RouteLeaseView>,
}

impl SessionView {
    pub fn has_stable_id(&self) -> bool {
        self.id.is_some() && self.source.is_stable()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteLeaseView {
    pub id: String,
    pub original_model: String,
    pub target: RouteTarget,
    pub expires_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientFormat {
    AnthropicMessages,
    OpenaiChat,
    OpenaiResponses,
    GeminiGenerate,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct JsonBodyView {
    #[serde(default)]
    pub json: Option<Value>,
    #[serde(default)]
    pub size_bytes: Option<u64>,
    #[serde(default)]
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClientRequestView {
    pub trace_id: String,
    pub method: String,
    pub path: String,
    pub client_format: ClientFormat,
    pub original_model: String,
    pub current_model: String,
    pub streaming: bool,
    #[serde(default)]
    pub headers: SafeHeaders,
    #[serde(default)]
    pub body: JsonBodyView,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelCapabilities {
    #[serde(default)]
    pub tools: bool,
    #[serde(default)]
    pub vision: bool,
    #[serde(default)]
    pub reasoning: bool,
    #[serde(default)]
    pub portable_history: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelRef {
    pub provider: String,
    pub id: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub equivalence_classes: Vec<String>,
    #[serde(default)]
    pub capabilities: ModelCapabilities,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderView {
    pub id: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub paused: bool,
    #[serde(default)]
    pub healthy: bool,
    #[serde(default)]
    pub supported_formats: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteView {
    pub requested: ModelRef,
    pub selected: ModelRef,
    pub provider: ProviderView,
    pub upstream_format: String,
    pub attempt_number: u32,
    #[serde(default)]
    pub same_route_accounts_remaining: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BodyView {
    #[serde(default)]
    pub content_type: Option<String>,
    #[serde(default)]
    pub size_bytes: Option<u64>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub json: Option<Value>,
    #[serde(default)]
    pub truncated: bool,
    #[serde(default)]
    pub inspected_bytes: usize,
}

impl BodyView {
    /// True when the proxy has supplied body bytes (including an empty complete body).
    pub fn inspected(&self) -> bool {
        self.inspected_bytes > 0 || self.size_bytes == Some(0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorClass {
    Auth,
    Capacity,
    BadRequest,
    Server,
    ClientDisconnect,
    Network,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorInfo {
    pub class: ErrorClass,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttemptTiming {
    #[serde(default)]
    pub started_ms: Option<i64>,
    #[serde(default)]
    pub ended_ms: Option<i64>,
    #[serde(default)]
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttemptOutcome {
    pub status: u16,
    #[serde(default)]
    pub headers: SafeHeaders,
    #[serde(default)]
    pub body: BodyView,
    #[serde(default)]
    pub error: Option<ErrorInfo>,
    #[serde(default)]
    pub timing: AttemptTiming,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RequestReceivedContext {
    pub request: ClientRequestView,
    pub harness: HarnessView,
    pub session: SessionView,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoutePlannedContext {
    pub request: ClientRequestView,
    pub harness: HarnessView,
    pub session: SessionView,
    pub route: RouteView,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttemptResultContext {
    pub request: ClientRequestView,
    pub harness: HarnessView,
    pub session: SessionView,
    pub route: RouteView,
    pub outcome: AttemptOutcome,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HttpResponseView {
    pub status: u16,
    #[serde(default)]
    pub headers: SafeHeaders,
    #[serde(default)]
    pub body: BodyView,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResponseReadyContext {
    pub request: ClientRequestView,
    pub harness: HarnessView,
    pub session: SessionView,
    pub route: RouteView,
    pub response: HttpResponseView,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TraceFinalizedContext {
    pub trace_id: String,
    #[serde(default)]
    pub requested_model: Option<String>,
    #[serde(default)]
    pub served_model: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub status: Option<u16>,
    #[serde(default)]
    pub attributes: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderConstraint {
    #[default]
    Any,
    Only(Vec<String>),
    Prefer(Vec<String>),
    Exclude(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RouteTarget {
    Exact {
        model: String,
        #[serde(default)]
        providers: ProviderConstraint,
    },
    Equivalent {
        class: String,
        #[serde(default)]
        providers: ProviderConstraint,
    },
}

impl RouteTarget {
    pub fn cycle_key(&self) -> String {
        match self {
            Self::Exact { model, providers } => format!("exact:{model}:{providers:?}"),
            Self::Equivalent { class, providers } => {
                format!("equivalent:{class}:{providers:?}")
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "scope", rename_all = "snake_case")]
pub enum RouteScope {
    Request,
    Session { ttl_seconds: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResponseNotice {
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum AttemptDecision {
    Continue,
    ReturnOriginal {
        reason: String,
    },
    RetrySameRoute {
        #[serde(default = "default_true")]
        exclude_current_account: bool,
        reason: String,
    },
    Reroute {
        target: RouteTarget,
        scope: RouteScope,
        #[serde(default)]
        notice: Option<ResponseNotice>,
        reason: String,
    },
}

impl AttemptDecision {
    pub fn is_terminal(&self) -> bool {
        !matches!(self, Self::Continue)
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum HeaderPatch {
    Set { name: String, value: String },
    Append { name: String, value: String },
    Remove { name: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonPatchOperation {
    pub op: String,
    pub path: String,
    #[serde(default)]
    pub from: Option<String>,
    #[serde(default)]
    pub value: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", content = "value", rename_all = "snake_case")]
pub enum ResponsePatch {
    PrependAssistantText(String),
    AppendAssistantText(String),
    JsonPatch(Vec<JsonPatchOperation>),
    Headers(Vec<HeaderPatch>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Capability {
    #[serde(rename = "request.read")]
    RequestRead,
    #[serde(rename = "request.patch")]
    RequestPatch,
    #[serde(rename = "route.override")]
    RouteOverride,
    #[serde(rename = "attempt.read_error_body")]
    AttemptReadErrorBody,
    #[serde(rename = "response.patch")]
    ResponsePatch,
    #[serde(rename = "response.prepend_text")]
    ResponsePrependText,
    #[serde(rename = "session.pin")]
    SessionPin,
    #[serde(rename = "trace.observe")]
    TraceObserve,
}
