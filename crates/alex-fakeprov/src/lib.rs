use std::collections::{BTreeMap, HashMap, VecDeque};
use std::convert::Infallible;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::stream;
use rand::distributions::{Alphanumeric, DistString};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

pub const CONTROL_KEY_HEADER: &str = "x-control-key";

#[derive(Clone, Debug)]
pub struct Config {
    pub bind: IpAddr,
    pub port: u16,
    pub scenario: String,
    pub fixtures_dir: Option<PathBuf>,
    pub scenarios_dir: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bind: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: 0,
            scenario: "ok".into(),
            fixtures_dir: None,
            scenarios_dir: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct RequestRecord {
    pub method: String,
    pub path: String,
    pub headers: BTreeMap<String, String>,
    pub body: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ResponseSpec {
    #[serde(default)]
    pub fixture: Option<String>,
    #[serde(default)]
    pub failure: Option<String>,
    #[serde(default)]
    pub status: Option<u16>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub body: Option<Value>,
    #[serde(default)]
    pub raw_body: Option<String>,
    #[serde(default)]
    pub content_type: Option<String>,
    #[serde(default)]
    pub latency_ms: Option<u64>,
    #[serde(default)]
    pub chunk_delay_ms: Option<u64>,
    #[serde(default)]
    pub stall_after_chunks: Option<usize>,
    #[serde(default)]
    pub repeat: bool,
    #[serde(default)]
    pub use_default: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct QueueRequest {
    pub endpoint: String,
    pub response: ResponseSpec,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ControlQueueRequest {
    Nested(QueueRequest),
    Flat(FlatQueueRequest),
}

#[derive(Deserialize)]
struct FlatQueueRequest {
    endpoint: String,
    #[serde(flatten)]
    response: ResponseSpec,
}

impl ControlQueueRequest {
    fn into_parts(self) -> (String, ResponseSpec) {
        match self {
            Self::Nested(request) => (request.endpoint, request.response),
            Self::Flat(request) => (request.endpoint, request.response),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
struct FixtureMeta {
    #[serde(default = "ok_status")]
    status: u16,
    #[serde(default)]
    headers: BTreeMap<String, String>,
    #[serde(default)]
    latency_ms: u64,
    #[serde(default)]
    chunk_delay_ms: u64,
}

impl Default for FixtureMeta {
    fn default() -> Self {
        Self {
            status: 200,
            headers: BTreeMap::new(),
            latency_ms: 0,
            chunk_delay_ms: 0,
        }
    }
}

fn ok_status() -> u16 {
    200
}

#[derive(Clone, Debug, Deserialize)]
struct ScenarioFile {
    #[serde(default)]
    endpoints: HashMap<String, Vec<ResponseSpec>>,
}

struct ScenarioEngine {
    initial: HashMap<String, Vec<ResponseSpec>>,
    active: HashMap<String, VecDeque<ResponseSpec>>,
    queued: HashMap<String, VecDeque<ResponseSpec>>,
}

impl ScenarioEngine {
    fn new(endpoints: HashMap<String, Vec<ResponseSpec>>) -> Self {
        let active = endpoints
            .iter()
            .map(|(key, values)| (key.clone(), values.clone().into()))
            .collect();
        Self {
            initial: endpoints,
            active,
            queued: HashMap::new(),
        }
    }

    fn install(&mut self, endpoints: HashMap<String, Vec<ResponseSpec>>) {
        *self = Self::new(endpoints);
    }

    fn reset(&mut self) {
        self.active = self
            .initial
            .iter()
            .map(|(key, values)| (key.clone(), values.clone().into()))
            .collect();
        self.queued.clear();
    }

    fn queue(&mut self, endpoint: String, response: ResponseSpec) {
        self.queued.entry(endpoint).or_default().push_back(response);
    }

    fn take(&mut self, endpoint: &str, path: &str) -> Option<ResponseSpec> {
        for key in [endpoint, path, "*"] {
            if let Some(queue) = self.queued.get_mut(key) {
                if let Some(response) = queue.pop_front() {
                    return Some(response);
                }
            }
        }
        for key in [endpoint, path, "*"] {
            if let Some(queue) = self.active.get_mut(key) {
                if let Some(response) = queue.front().cloned() {
                    if !response.repeat {
                        queue.pop_front();
                    }
                    return Some(response);
                }
            }
        }
        None
    }
}

struct AppState {
    fixtures_dir: PathBuf,
    scenarios_dir: PathBuf,
    control_key: String,
    engine: Mutex<ScenarioEngine>,
    requests: Mutex<Vec<RequestRecord>>,
}

pub struct FakeProv {
    address: SocketAddr,
    base_url: String,
    control_key: String,
    state: Arc<AppState>,
    server: Option<JoinHandle<()>>,
}

impl FakeProv {
    pub async fn spawn(config: Config) -> Result<Self> {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let fixtures_dir = config
            .fixtures_dir
            .unwrap_or_else(|| manifest_dir.join("fixtures"));
        let scenarios_dir = config
            .scenarios_dir
            .unwrap_or_else(|| manifest_dir.join("scenarios"));
        if !fixtures_dir.is_dir() {
            bail!(
                "fixture directory does not exist: {}",
                fixtures_dir.display()
            );
        }
        if !scenarios_dir.is_dir() {
            bail!(
                "scenario directory does not exist: {}",
                scenarios_dir.display()
            );
        }
        let scenario = read_scenario(&scenarios_dir, &config.scenario)?;
        let control_key = Alphanumeric.sample_string(&mut rand::thread_rng(), 32);
        let state = Arc::new(AppState {
            fixtures_dir,
            scenarios_dir,
            control_key: control_key.clone(),
            engine: Mutex::new(ScenarioEngine::new(scenario.endpoints)),
            requests: Mutex::new(Vec::new()),
        });
        let listener = tokio::net::TcpListener::bind(SocketAddr::new(config.bind, config.port))
            .await
            .context("binding fake provider listener")?;
        let address = listener.local_addr()?;
        let app = router(state.clone());
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Ok(Self {
            address,
            base_url: format!("http://{address}"),
            control_key,
            state,
            server: Some(server),
        })
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn control_key(&self) -> &str {
        &self.control_key
    }

    pub fn port(&self) -> u16 {
        self.address.port()
    }

    pub async fn requests(&self) -> Vec<RequestRecord> {
        self.state.requests.lock().await.clone()
    }

    pub async fn set_scenario(&self, name: impl AsRef<str>) -> Result<()> {
        set_scenario(&self.state, name.as_ref()).await
    }

    pub async fn queue(&self, endpoint: impl Into<String>, response: ResponseSpec) {
        self.state
            .engine
            .lock()
            .await
            .queue(endpoint.into(), response);
    }

    pub async fn reset(&self) {
        self.state.requests.lock().await.clear();
        self.state.engine.lock().await.reset();
    }

    pub async fn shutdown(mut self) {
        if let Some(server) = self.server.take() {
            server.abort();
            let _ = server.await;
        }
    }
}

impl Drop for FakeProv {
    fn drop(&mut self) {
        if let Some(server) = self.server.take() {
            server.abort();
        }
    }
}

fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/v1/messages", post(anthropic_messages))
        .route("/api/oauth/profile", get(anthropic_profile))
        .route("/api/oauth/usage", get(anthropic_usage))
        .route("/v1/oauth/token", post(anthropic_token))
        .route("/v1/chat/completions", post(openai_chat))
        .route("/v1/responses", post(openai_responses))
        .route("/v1/models", get(openai_models))
        .route("/backend-api/codex/responses", post(codex_responses))
        .route("/backend-api/wham/usage", get(codex_usage))
        .route("/oauth/token", post(openai_token))
        .route("/_control/reset", post(control_reset))
        .route("/_control/scenario", post(control_scenario))
        .route("/_control/queue", post(control_queue))
        .route("/_control/requests", get(control_requests))
        .with_state(state)
}

async fn anthropic_messages(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    handle_model(state, Method::POST, uri, headers, body).await
}

async fn openai_chat(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    handle_model(state, Method::POST, uri, headers, body).await
}

async fn openai_responses(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    handle_model(state, Method::POST, uri, headers, body).await
}

async fn codex_responses(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    handle_model(state, Method::POST, uri, headers, body).await
}

async fn anthropic_profile(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    handle_fixed(
        state,
        Method::GET,
        uri,
        headers,
        Bytes::new(),
        "anthropic/profile.json",
    )
    .await
}

async fn anthropic_usage(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    handle_fixed(
        state,
        Method::GET,
        uri,
        headers,
        Bytes::new(),
        "anthropic/usage.json",
    )
    .await
}

async fn anthropic_token(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    handle_fixed(
        state,
        Method::POST,
        uri,
        headers,
        body,
        "anthropic/token.json",
    )
    .await
}

async fn openai_models(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    handle_fixed(
        state,
        Method::GET,
        uri,
        headers,
        Bytes::new(),
        "openai/models.json",
    )
    .await
}

async fn codex_usage(State(state): State<Arc<AppState>>, headers: HeaderMap, uri: Uri) -> Response {
    handle_fixed(
        state,
        Method::GET,
        uri,
        headers,
        Bytes::new(),
        "openai/codex-usage.json",
    )
    .await
}

async fn openai_token(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    handle_fixed(state, Method::POST, uri, headers, body, "openai/token.json").await
}

async fn handle_model(
    state: Arc<AppState>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let path = uri.path().to_string();
    record_request(&state, &method, &path, &headers, &body).await;
    let endpoint = format!("{method} {path}");
    let stream_requested = path == "/backend-api/codex/responses"
        || serde_json::from_slice::<Value>(&body)
            .ok()
            .and_then(|value| value.get("stream").and_then(Value::as_bool))
            .unwrap_or(false);
    let header_failure = headers
        .get("x-mock-fail")
        .and_then(|value| value.to_str().ok())
        .map(|value| ResponseSpec {
            failure: Some(value.to_ascii_lowercase()),
            ..ResponseSpec::default()
        });
    let response = match header_failure {
        Some(response) => response,
        None => state
            .engine
            .lock()
            .await
            .take(&endpoint, &path)
            .unwrap_or_default(),
    };
    if response.failure.as_deref() == Some("timeout") {
        return std::future::pending::<Response>().await;
    }
    let default_fixture = default_model_fixture(&path, stream_requested);
    match render_response(&state, &path, stream_requested, response, default_fixture).await {
        Ok(response) => response,
        Err(error) => internal_error(error),
    }
}

async fn handle_fixed(
    state: Arc<AppState>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
    fixture: &'static str,
) -> Response {
    let path = uri.path().to_string();
    record_request(&state, &method, &path, &headers, &body).await;
    let endpoint = format!("{method} {path}");
    let response = state
        .engine
        .lock()
        .await
        .take(&endpoint, &path)
        .unwrap_or_default();
    match render_response(&state, &path, false, response, fixture).await {
        Ok(response) => response,
        Err(error) => internal_error(error),
    }
}

fn default_model_fixture(path: &str, stream: bool) -> &'static str {
    match (path, stream) {
        ("/v1/messages", false) => "anthropic/default-message.json",
        ("/v1/messages", true) => "anthropic/default-message-stream.sse",
        ("/v1/chat/completions", false) => "openai/default-chat.json",
        ("/v1/chat/completions", true) => "openai/default-chat-stream.sse",
        ("/v1/responses", false) => "openai/default-responses.json",
        ("/v1/responses", true) => "openai/default-responses-stream.sse",
        ("/backend-api/codex/responses", _) => "openai/default-codex-responses.sse",
        _ => "openai/default-chat.json",
    }
}

async fn render_response(
    state: &AppState,
    path: &str,
    stream_requested: bool,
    response: ResponseSpec,
    default_fixture: &str,
) -> Result<Response> {
    if let Some(failure) = response.failure.as_deref() {
        return render_failure(state, path, stream_requested, failure).await;
    }
    let mut loaded = if let Some(fixture) = response.fixture.as_deref() {
        load_fixture(&state.fixtures_dir, fixture)?
    } else if let Some(raw_body) = response.raw_body.clone() {
        LoadedResponse {
            body: raw_body.into_bytes(),
            status: 200,
            headers: BTreeMap::new(),
            latency_ms: 0,
            chunk_delay_ms: 0,
        }
    } else if let Some(body) = response.body.as_ref() {
        LoadedResponse {
            body: serde_json::to_vec(body)?,
            status: 200,
            headers: BTreeMap::from([("content-type".into(), "application/json".into())]),
            latency_ms: 0,
            chunk_delay_ms: 0,
        }
    } else {
        load_fixture(&state.fixtures_dir, default_fixture)?
    };
    if let Some(status) = response.status {
        loaded.status = status;
    }
    loaded.headers.extend(response.headers);
    if let Some(content_type) = response.content_type {
        loaded.headers.insert("content-type".into(), content_type);
    }
    if let Some(latency_ms) = response.latency_ms {
        loaded.latency_ms = latency_ms;
    }
    if let Some(chunk_delay_ms) = response.chunk_delay_ms {
        loaded.chunk_delay_ms = chunk_delay_ms;
    }
    build_response(loaded, response.stall_after_chunks).await
}

async fn render_failure(
    state: &AppState,
    path: &str,
    stream_requested: bool,
    failure: &str,
) -> Result<Response> {
    match failure {
        "429" => Ok(provider_error(
            path,
            429,
            "rate_limit_error",
            "rate limit exceeded",
        )),
        "500" => Ok(provider_error(
            path,
            500,
            "api_error",
            "internal server error",
        )),
        "529" => Ok(provider_error(
            path,
            529,
            "overloaded_error",
            "provider is overloaded",
        )),
        "truncated-sse" => {
            let raw = match path {
                "/v1/messages" => "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_fakeprov_truncated\"}}\n\nevent: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":",
                "/v1/responses" | "/backend-api/codex/responses" => "event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_fakeprov_truncated\",\"status\":\"in_progress\"}}\n\nevent: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":",
                _ => "data: {\"id\":\"fakeprov-truncated\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\ndata: {\"truncated\":",
            };
            build_response(
                LoadedResponse::raw(200, "text/event-stream", raw.as_bytes().to_vec()),
                None,
            )
            .await
        }
        "refusal" if path == "/v1/messages" && stream_requested => {
            let loaded = load_fixture(
                &state.fixtures_dir,
                "anthropic/anthropic-fable-refusal-200.sse",
            )?;
            build_response(loaded, None).await
        }
        "refusal" if stream_requested => {
            let raw = refusal_sse(path);
            build_response(
                LoadedResponse::raw(200, "text/event-stream", raw.into_bytes()),
                None,
            )
            .await
        }
        "refusal" => {
            let value = refusal_body(path);
            build_response(
                LoadedResponse::raw(200, "application/json", serde_json::to_vec(&value)?),
                None,
            )
            .await
        }
        "malformed" => {
            build_response(
                LoadedResponse::raw(200, "application/json", b"{\"malformed\":".to_vec()),
                None,
            )
            .await
        }
        other => Ok(provider_error(
            path,
            400,
            "invalid_mock_failure",
            &format!("unsupported x-mock-fail value: {other}"),
        )),
    }
}

fn refusal_sse(path: &str) -> String {
    if matches!(path, "/v1/responses" | "/backend-api/codex/responses") {
        let response = refusal_body(path);
        return format!(
            "event: response.created\ndata: {}\n\nevent: response.completed\ndata: {}\n\n",
            json!({"type": "response.created", "response": {"id": "resp_fakeprov_refusal", "status": "in_progress"}}),
            json!({"type": "response.completed", "response": response})
        );
    }
    concat!(
        "data: {\"id\":\"chatcmpl_fakeprov_refusal\",\"object\":\"chat.completion.chunk\",\"created\":1700000000,\"model\":\"gpt-4.1\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"I cannot help with that request.\",\"refusal\":\"policy\"},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"chatcmpl_fakeprov_refusal\",\"object\":\"chat.completion.chunk\",\"created\":1700000000,\"model\":\"gpt-4.1\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n"
    )
    .into()
}

fn refusal_body(path: &str) -> Value {
    match path {
        "/v1/messages" => json!({
            "id": "msg_fakeprov_refusal",
            "type": "message",
            "role": "assistant",
            "model": "claude-fable-5",
            "content": [],
            "stop_reason": "refusal",
            "stop_sequence": null,
            "usage": {"input_tokens": 8, "output_tokens": 0}
        }),
        "/v1/responses" | "/backend-api/codex/responses" => json!({
            "id": "resp_fakeprov_refusal",
            "object": "response",
            "status": "completed",
            "model": "gpt-5.5",
            "output": [{
                "id": "msg_fakeprov_refusal",
                "type": "message",
                "status": "completed",
                "role": "assistant",
                "content": [{"type": "refusal", "refusal": "I cannot help with that request."}]
            }],
            "usage": {"input_tokens": 8, "output_tokens": 4, "total_tokens": 12}
        }),
        _ => json!({
            "id": "chatcmpl_fakeprov_refusal",
            "object": "chat.completion",
            "created": 1700000000,
            "model": "gpt-4.1",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "I cannot help with that request.", "refusal": "policy"},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 8, "completion_tokens": 4, "total_tokens": 12}
        }),
    }
}

fn provider_error(path: &str, status: u16, error_type: &str, message: &str) -> Response {
    let body = if path == "/v1/messages" {
        json!({"type": "error", "error": {"type": error_type, "message": message}})
    } else {
        json!({"error": {"message": message, "type": error_type, "param": null, "code": error_type}})
    };
    json_response(status, body)
}

fn json_response(status: u16, body: Value) -> Response {
    let status = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, Json(body)).into_response()
}

fn internal_error(error: anyhow::Error) -> Response {
    json_response(
        500,
        json!({"error": {"type": "fakeprov_error", "message": error.to_string()}}),
    )
}

struct LoadedResponse {
    body: Vec<u8>,
    status: u16,
    headers: BTreeMap<String, String>,
    latency_ms: u64,
    chunk_delay_ms: u64,
}

impl LoadedResponse {
    fn raw(status: u16, content_type: &str, body: Vec<u8>) -> Self {
        Self {
            body,
            status,
            headers: BTreeMap::from([("content-type".into(), content_type.into())]),
            latency_ms: 0,
            chunk_delay_ms: 0,
        }
    }
}

fn load_fixture(root: &Path, name: &str) -> Result<LoadedResponse> {
    let relative = Path::new(name);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        bail!("fixture path must be relative and cannot contain traversal");
    }
    let path = root.join(relative);
    let body =
        std::fs::read(&path).with_context(|| format!("reading fixture {}", path.display()))?;
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .context("fixture has no UTF-8 file stem")?;
    let meta_path = path.with_file_name(format!("{stem}.meta.json"));
    let mut meta = if meta_path.is_file() {
        serde_json::from_slice::<FixtureMeta>(&std::fs::read(&meta_path)?)
            .with_context(|| format!("parsing fixture metadata {}", meta_path.display()))?
    } else {
        FixtureMeta::default()
    };
    if !meta.headers.contains_key("content-type") {
        let content_type = match path.extension().and_then(|value| value.to_str()) {
            Some("sse") => "text/event-stream",
            _ => "application/json",
        };
        meta.headers
            .insert("content-type".into(), content_type.into());
    }
    Ok(LoadedResponse {
        body,
        status: meta.status,
        headers: meta.headers,
        latency_ms: meta.latency_ms,
        chunk_delay_ms: meta.chunk_delay_ms,
    })
}

async fn build_response(
    loaded: LoadedResponse,
    stall_after_chunks: Option<usize>,
) -> Result<Response> {
    if loaded.latency_ms > 0 {
        tokio::time::sleep(Duration::from_millis(loaded.latency_ms)).await;
    }
    let status = StatusCode::from_u16(loaded.status).context("invalid fixture status")?;
    let is_sse = loaded
        .headers
        .get("content-type")
        .is_some_and(|value| value.starts_with("text/event-stream"));
    let body = if is_sse {
        let frames = split_sse_frames(&loaded.body);
        let delay = loaded.chunk_delay_ms;
        let stream = stream::unfold(
            (frames, 0_usize, delay, stall_after_chunks),
            |(frames, index, delay, stall)| async move {
                let stall_at = stall.map(|value| value.min(frames.len()));
                if stall_at.is_some_and(|value| index >= value) {
                    return std::future::pending::<
                        Option<(
                            Result<Bytes, Infallible>,
                            (Vec<Vec<u8>>, usize, u64, Option<usize>),
                        )>,
                    >()
                    .await;
                }
                if index >= frames.len() {
                    return None;
                }
                if index > 0 && delay > 0 {
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                }
                let frame = Bytes::copy_from_slice(&frames[index]);
                Some((Ok(frame), (frames, index + 1, delay, stall)))
            },
        );
        Body::from_stream(stream)
    } else {
        Body::from(loaded.body)
    };
    let mut response = Response::builder().status(status);
    for (name, value) in loaded.headers {
        response = response.header(name, value);
    }
    Ok(response.body(body)?)
}

fn split_sse_frames(body: &[u8]) -> Vec<Vec<u8>> {
    let mut frames = Vec::new();
    let mut start = 0;
    let mut index = 0;
    while index + 1 < body.len() {
        let boundary = if body[index] == b'\n' && body[index + 1] == b'\n' {
            Some(index + 2)
        } else if index + 3 < body.len()
            && body[index] == b'\r'
            && body[index + 1] == b'\n'
            && body[index + 2] == b'\r'
            && body[index + 3] == b'\n'
        {
            Some(index + 4)
        } else {
            None
        };
        if let Some(end) = boundary {
            frames.push(body[start..end].to_vec());
            start = end;
            index = end;
        } else {
            index += 1;
        }
    }
    if start < body.len() {
        frames.push(body[start..].to_vec());
    }
    frames
}

async fn record_request(
    state: &AppState,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    body: &Bytes,
) {
    let mut recorded_headers = BTreeMap::<String, String>::new();
    for (name, value) in headers {
        let value = value.to_str().unwrap_or_default();
        recorded_headers
            .entry(name.as_str().to_string())
            .and_modify(|existing| {
                existing.push_str(", ");
                existing.push_str(value);
            })
            .or_insert_with(|| value.to_string());
    }
    state.requests.lock().await.push(RequestRecord {
        method: method.to_string(),
        path: path.to_string(),
        headers: recorded_headers,
        body: String::from_utf8_lossy(body).into_owned(),
    });
}

fn read_scenario(root: &Path, name: &str) -> Result<ScenarioFile> {
    if name.is_empty()
        || !name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        bail!("scenario name contains unsupported characters");
    }
    let path = root.join(format!("{name}.json"));
    serde_json::from_slice(
        &std::fs::read(&path).with_context(|| format!("reading scenario {}", path.display()))?,
    )
    .with_context(|| format!("parsing scenario {}", path.display()))
}

async fn set_scenario(state: &AppState, name: &str) -> Result<()> {
    let scenario = read_scenario(&state.scenarios_dir, name)?;
    state.engine.lock().await.install(scenario.endpoints);
    Ok(())
}

fn control_authorized(state: &AppState, headers: &HeaderMap) -> bool {
    let direct = headers
        .get(CONTROL_KEY_HEADER)
        .and_then(|value| value.to_str().ok());
    let bearer = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    direct.or(bearer) == Some(state.control_key.as_str())
}

fn unauthorized() -> Response {
    json_response(
        401,
        json!({"error": {"type": "unauthorized", "message": "valid control key required"}}),
    )
}

async fn control_reset(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if !control_authorized(&state, &headers) {
        return unauthorized();
    }
    state.requests.lock().await.clear();
    state.engine.lock().await.reset();
    Json(json!({"ok": true})).into_response()
}

#[derive(Deserialize)]
struct ScenarioSelection {
    #[serde(alias = "scenario")]
    name: String,
}

async fn control_scenario(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(selection): Json<ScenarioSelection>,
) -> Response {
    if !control_authorized(&state, &headers) {
        return unauthorized();
    }
    match set_scenario(&state, &selection.name).await {
        Ok(()) => Json(json!({"ok": true, "scenario": selection.name})).into_response(),
        Err(error) => json_response(
            400,
            json!({"error": {"type": "invalid_scenario", "message": error.to_string()}}),
        ),
    }
}

async fn control_queue(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<ControlQueueRequest>,
) -> Response {
    if !control_authorized(&state, &headers) {
        return unauthorized();
    }
    let (endpoint, response) = request.into_parts();
    state.engine.lock().await.queue(endpoint, response);
    Json(json!({"ok": true})).into_response()
}

async fn control_requests(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if !control_authorized(&state, &headers) {
        return unauthorized();
    }
    Json(state.requests.lock().await.clone()).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;

    async fn server() -> FakeProv {
        FakeProv::spawn(Config::default()).await.unwrap()
    }

    #[tokio::test]
    async fn json_completion_roundtrip_for_anthropic_and_openai() {
        let server = server().await;
        let client = reqwest::Client::new();
        let anthropic: Value = client
            .post(format!("{}/v1/messages", server.base_url()))
            .json(&json!({"model": "claude-sonnet-4-5", "messages": [{"role": "user", "content": "hi"}]}))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        let openai: Value = client
            .post(format!("{}/v1/chat/completions", server.base_url()))
            .json(&json!({"model": "gpt-4.1", "messages": [{"role": "user", "content": "hi"}]}))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(anthropic["type"], "message");
        assert_eq!(anthropic["usage"]["input_tokens"], 8);
        assert_eq!(openai["object"], "chat.completion");
        assert_eq!(openai["usage"]["total_tokens"], 12);
    }

    #[tokio::test]
    async fn sse_replay_preserves_frames_and_terminals() {
        let server = server().await;
        let client = reqwest::Client::new();
        let response = client
            .post(format!("{}/v1/chat/completions", server.base_url()))
            .json(&json!({"model": "gpt-4.1", "stream": true, "messages": []}))
            .send()
            .await
            .unwrap();
        let chunks = response
            .bytes_stream()
            .map(|chunk| chunk.unwrap().to_vec())
            .collect::<Vec<_>>()
            .await;
        assert!(chunks.len() >= 3);
        assert!(chunks.iter().all(|chunk| chunk.ends_with(b"\n\n")));
        assert_eq!(chunks.last().unwrap(), b"data: [DONE]\n\n");

        let anthropic = client
            .post(format!("{}/v1/messages", server.base_url()))
            .json(&json!({"model": "claude-sonnet-4-5", "stream": true, "messages": []}))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert!(anthropic.ends_with("event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"));
    }

    #[tokio::test]
    async fn mock_failure_header_injects_provider_error() {
        let server = server().await;
        let response = reqwest::Client::new()
            .post(format!("{}/v1/messages", server.base_url()))
            .header("x-mock-fail", "529")
            .json(&json!({"model": "claude-sonnet-4-5"}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 529);
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["error"]["type"], "overloaded_error");
    }

    #[tokio::test]
    async fn rate_limit_scenario_advances_to_success() {
        let server = FakeProv::spawn(Config {
            scenario: "rate-limit-then-ok".into(),
            ..Config::default()
        })
        .await
        .unwrap();
        let client = reqwest::Client::new();
        let url = format!("{}/v1/chat/completions", server.base_url());
        let first = client.post(&url).json(&json!({})).send().await.unwrap();
        let second = client.post(&url).json(&json!({})).send().await.unwrap();
        assert_eq!(first.status(), 429);
        assert_eq!(second.status(), 200);
    }

    #[tokio::test]
    async fn control_request_log_captures_authorization() {
        let server = server().await;
        let client = reqwest::Client::new();
        client
            .post(format!("{}/v1/responses", server.base_url()))
            .bearer_auth("fake-secret-token")
            .json(&json!({"model": "gpt-5.5", "input": "hello"}))
            .send()
            .await
            .unwrap();
        let records: Vec<RequestRecord> = client
            .get(format!("{}/_control/requests", server.base_url()))
            .header(CONTROL_KEY_HEADER, server.control_key())
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(
            records[0].headers.get("authorization").map(String::as_str),
            Some("Bearer fake-secret-token")
        );
        assert!(records[0].body.contains("gpt-5.5"));
    }

    #[tokio::test]
    async fn usage_endpoints_have_parser_compatible_shapes() {
        let server = server().await;
        let client = reqwest::Client::new();
        let anthropic: Value = client
            .get(format!("{}/api/oauth/usage", server.base_url()))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(anthropic["five_hour"]["utilization"].is_number());
        assert!(anthropic["seven_day"]["resets_at"].is_string());

        let codex: Value = client
            .get(format!("{}/backend-api/wham/usage", server.base_url()))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(codex["plan_type"], "plus");
        assert_eq!(
            codex["rate_limit"]["primary_window"]["limit_window_seconds"],
            18_000
        );
        assert!(codex["credits"]["has_credits"].is_boolean());
    }

    #[tokio::test]
    async fn remaining_endpoints_return_deterministic_success_shapes() {
        let server = server().await;
        let client = reqwest::Client::new();
        let profile: Value = client
            .get(format!("{}/api/oauth/profile", server.base_url()))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(profile["account"]["email"], "fakeprov@example.test");

        let anthropic_token: Value = client
            .post(format!("{}/v1/oauth/token", server.base_url()))
            .form(&[("grant_type", "refresh_token")])
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(
            anthropic_token["access_token"],
            "anthropic_access_fakeprov_0001"
        );

        let models: Value = client
            .get(format!("{}/v1/models", server.base_url()))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(models["object"], "list");
        assert_eq!(models["data"].as_array().unwrap().len(), 3);

        let responses: Value = client
            .post(format!("{}/v1/responses", server.base_url()))
            .json(&json!({"model": "gpt-5.5", "input": "hi"}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(responses["object"], "response");

        let codex = client
            .post(format!("{}/backend-api/codex/responses", server.base_url()))
            .json(&json!({"model": "gpt-5.5-codex", "input": "hi"}))
            .send()
            .await
            .unwrap();
        assert_eq!(codex.headers()["x-codex-plan-type"], "plus");
        assert!(codex
            .text()
            .await
            .unwrap()
            .contains("event: response.completed"));

        let openai_token: Value = client
            .post(format!("{}/oauth/token", server.base_url()))
            .form(&[("grant_type", "refresh_token")])
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(openai_token["token_type"], "Bearer");
        assert!(openai_token["id_token"]
            .as_str()
            .unwrap()
            .starts_with("eyJ"));
    }

    #[tokio::test]
    async fn fable_refusal_replays_raw_sse() {
        let server = server().await;
        let response = reqwest::Client::new()
            .post(format!("{}/v1/messages", server.base_url()))
            .header("x-mock-fail", "refusal")
            .json(&json!({"model": "claude-fable-5", "stream": true}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.headers()["content-type"], "text/event-stream");
        let body = response.text().await.unwrap();
        assert!(body.contains("\"stop_reason\":\"refusal\""));
        assert!(body.ends_with("event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"));
    }

    #[tokio::test]
    async fn control_queue_and_reset_are_key_gated() {
        let server = server().await;
        let client = reqwest::Client::new();
        let queue = client
            .post(format!("{}/_control/queue", server.base_url()))
            .header(CONTROL_KEY_HEADER, server.control_key())
            .json(&QueueRequest {
                endpoint: "POST /v1/responses".into(),
                response: ResponseSpec {
                    failure: Some("500".into()),
                    ..ResponseSpec::default()
                },
            })
            .send()
            .await
            .unwrap();
        assert_eq!(queue.status(), 200);
        let injected = client
            .post(format!("{}/v1/responses", server.base_url()))
            .json(&json!({}))
            .send()
            .await
            .unwrap();
        assert_eq!(injected.status(), 500);

        let unauthorized = client
            .get(format!("{}/_control/requests", server.base_url()))
            .send()
            .await
            .unwrap();
        assert_eq!(unauthorized.status(), 401);
        client
            .post(format!("{}/_control/reset", server.base_url()))
            .bearer_auth(server.control_key())
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
        assert!(server.requests().await.is_empty());
    }

    #[tokio::test]
    async fn stream_stall_holds_connection_after_configured_frames() {
        let server = FakeProv::spawn(Config {
            scenario: "stream-stall".into(),
            ..Config::default()
        })
        .await
        .unwrap();
        let response = reqwest::Client::new()
            .post(format!("{}/v1/chat/completions", server.base_url()))
            .json(&json!({"stream": true}))
            .send()
            .await
            .unwrap();
        let mut stream = response.bytes_stream();
        assert!(stream.next().await.unwrap().is_ok());
        assert!(stream.next().await.unwrap().is_ok());
        assert!(
            tokio::time::timeout(Duration::from_millis(50), stream.next())
                .await
                .is_err()
        );
    }
}
