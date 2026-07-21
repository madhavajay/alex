//! Loopback-only Alex + deterministic provider used by the real CLIProxyAPI
//! Docker compatibility fixture. It never reads a user's Alex configuration or
//! provider vault; every credential and data path must be supplied explicitly.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use alex_auth::{Account, Vault};
use alex_core::Provider;
use alex_store::Store;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

#[derive(Clone)]
struct ProviderState {
    expected_authorization: Arc<String>,
    calls: Arc<AtomicU64>,
}

fn required_env(name: &str) -> anyhow::Result<String> {
    std::env::var(name).map_err(|_| anyhow::anyhow!("missing required fixture variable {name}"))
}

fn env_port(name: &str) -> anyhow::Result<u16> {
    required_env(name)?
        .parse::<u16>()
        .map_err(|_| anyhow::anyhow!("{name} must be a valid TCP port"))
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn account(id: &str, provider: Provider, api_key: String, meta: Value) -> Account {
    Account {
        id: id.into(),
        provider,
        kind: "api_key".into(),
        name: id.into(),
        description: Some("CLIProxyAPI V1 Docker fixture".into()),
        paused: false,
        label: Some("fixture-only".into()),
        access_token: None,
        refresh_token: None,
        id_token: None,
        api_key: Some(api_key),
        expires_at_ms: None,
        last_refresh_ms: None,
        account_meta: meta,
        cooldown_until_ms: None,
        status: "active".into(),
        path: None,
    }
}

fn authorized(headers: &HeaderMap, state: &ProviderState) -> bool {
    headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == state.expected_authorization.as_str())
}

fn error_response(status: StatusCode, code: &str) -> Response {
    let mut builder = Response::builder()
        .status(status)
        .header("content-type", "application/json");
    if status == StatusCode::TOO_MANY_REQUESTS {
        builder = builder.header("retry-after", "29");
    }
    builder
        .body(Body::from(
            json!({
                "error": {
                    "type": if status == StatusCode::TOO_MANY_REQUESTS { "rate_limit_error" } else { "fixture_error" },
                    "code": code,
                    "message": format!("fixture {code}")
                }
            })
            .to_string(),
        ))
        .expect("fixture response")
}

fn model_error(model: &str) -> Option<(StatusCode, &'static str)> {
    if model.contains("auth") {
        Some((StatusCode::UNAUTHORIZED, "fixture_auth"))
    } else if model.contains("rate") {
        Some((StatusCode::TOO_MANY_REQUESTS, "fixture_rate"))
    } else if model.contains("server") {
        Some((StatusCode::SERVICE_UNAVAILABLE, "fixture_server"))
    } else {
        None
    }
}

fn chat_tool_stream(model: &str) -> Response {
    let first = json!({
        "id": "chatcmpl-fixture-tool",
        "object": "chat.completion.chunk",
        "created": 1,
        "model": model,
        "choices": [{
            "index": 0,
            "delta": {
                "role": "assistant",
                "tool_calls": [{
                    "index": 0,
                    "id": "call-fixture-shell",
                    "type": "function",
                    "function": {"name": "shell", "arguments": "{\"command\":\"pwd\"}"}
                }]
            },
            "finish_reason": null
        }]
    });
    let final_chunk = json!({
        "id": "chatcmpl-fixture-tool",
        "object": "chat.completion.chunk",
        "created": 1,
        "model": model,
        "choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}]
    });
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/event-stream")
        .header("cache-control", "no-cache")
        .body(Body::from(format!(
            "data: {first}\n\ndata: {final_chunk}\n\ndata: [DONE]\n\n"
        )))
        .expect("fixture stream")
}

async fn provider_chat(
    State(state): State<ProviderState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    state.calls.fetch_add(1, Ordering::SeqCst);
    if !authorized(&headers, &state) {
        return error_response(StatusCode::UNAUTHORIZED, "fixture_bad_provider_key");
    }
    let model = body["model"].as_str().unwrap_or("fixture-unknown");
    if let Some((status, code)) = model_error(model) {
        return error_response(status, code);
    }
    if model.contains("tool") && body["stream"].as_bool() == Some(true) {
        return chat_tool_stream(model);
    }
    let message = if model.contains("tool") {
        json!({
            "role": "assistant",
            "content": null,
            "tool_calls": [{
                "id": "call-fixture-shell",
                "type": "function",
                "function": {"name": "shell", "arguments": "{\"command\":\"pwd\"}"}
            }]
        })
    } else {
        json!({"role": "assistant", "content": "cliproxyapi-v1-ok"})
    };
    Json(json!({
        "id": "chatcmpl-fixture",
        "object": "chat.completion",
        "created": 1,
        "model": model,
        "choices": [{
            "index": 0,
            "message": message,
            "finish_reason": if model.contains("tool") { "tool_calls" } else { "stop" }
        }],
        "usage": {"prompt_tokens": 2, "completion_tokens": 3, "total_tokens": 5}
    }))
    .into_response()
}

async fn provider_responses(
    State(state): State<ProviderState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    state.calls.fetch_add(1, Ordering::SeqCst);
    if !authorized(&headers, &state) {
        return error_response(StatusCode::UNAUTHORIZED, "fixture_bad_provider_key");
    }
    let model = body["model"].as_str().unwrap_or("fixture-unknown");
    if let Some((status, code)) = model_error(model) {
        return error_response(status, code);
    }
    Json(json!({
        "id": "resp-fixture",
        "object": "response",
        "created_at": 1,
        "status": "completed",
        "model": model,
        "output": [{
            "id": "msg-fixture",
            "type": "message",
            "role": "assistant",
            "status": "completed",
            "content": [{"type": "output_text", "text": "cliproxyapi-v1-ok", "annotations": []}]
        }],
        "usage": {"input_tokens": 2, "output_tokens": 3, "total_tokens": 5}
    }))
    .into_response()
}

async fn provider_stats(State(state): State<ProviderState>) -> Json<Value> {
    Json(json!({"calls": state.calls.load(Ordering::SeqCst)}))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let alex_port = env_port("ALEX_CPA_FIXTURE_ALEX_PORT")?;
    let provider_port = env_port("ALEX_CPA_FIXTURE_PROVIDER_PORT")?;
    let data_dir = PathBuf::from(required_env("ALEX_CPA_FIXTURE_DATA_DIR")?);
    let local_key = required_env("ALEX_CPA_FIXTURE_LOCAL_KEY")?;
    let harness_key = required_env("ALEX_CPA_FIXTURE_HARNESS_KEY")?;
    let cpa_url = required_env("ALEX_CPA_FIXTURE_CPA_URL")?;
    let cpa_key = required_env("ALEX_CPA_FIXTURE_CPA_KEY")?;
    let provider_key = required_env("ALEX_CPA_FIXTURE_PROVIDER_KEY")?;

    std::fs::create_dir_all(&data_dir)?;
    let store = Arc::new(Store::open(data_dir.join("store"))?);
    let vault = Arc::new(Vault::open(data_dir.join("vault"))?);
    vault
        .upsert(account(
            "fixture-openai",
            Provider::Openai,
            provider_key.clone(),
            Value::Null,
        ))
        .await?;
    vault
        .upsert(account(
            "fixture-cliproxyapi",
            Provider::Cliproxyapi,
            cpa_key,
            json!({
                "api_base": format!("{}/v1", cpa_url.trim_end_matches('/')),
                "models": ["cpa/echo", "cpa/tool", "cpa/auth", "cpa/rate", "cpa/server"]
            }),
        ))
        .await?;

    let harness_hash = format!("{:x}", Sha256::digest(harness_key.as_bytes()));
    store.insert_run_key(
        "rk-cliproxyapi-v1-fixture",
        &harness_hash,
        "harness",
        None,
        Some(r#"{"suite":"cliproxyapi-v1-docker"}"#),
        Some("cliproxyapi-fixture"),
        now_ms(),
        None,
    )?;

    let alex_base = format!("http://127.0.0.1:{alex_port}");
    let state = alex_proxy::build_state(
        local_key,
        vault,
        store,
        None,
        alex_base,
        Duration::from_secs(30),
    );
    alex_proxy::set_upstream_base_override(
        &state,
        Provider::Openai,
        format!("http://127.0.0.1:{provider_port}"),
    );

    let provider_state = ProviderState {
        expected_authorization: Arc::new(format!("Bearer {provider_key}")),
        calls: Arc::new(AtomicU64::new(0)),
    };
    let provider = Router::new()
        .route("/v1/chat/completions", post(provider_chat))
        .route("/v1/responses", post(provider_responses))
        .route("/fixture/stats", get(provider_stats))
        .with_state(provider_state);

    let alex_listener = tokio::net::TcpListener::bind(("127.0.0.1", alex_port)).await?;
    let provider_listener = tokio::net::TcpListener::bind(("127.0.0.1", provider_port)).await?;
    println!("READY alex_port={alex_port} provider_port={provider_port}");

    let alex_server = axum::serve(
        alex_listener,
        alex_proxy::router(state).into_make_service_with_connect_info::<std::net::SocketAddr>(),
    );
    let provider_server = axum::serve(provider_listener, provider);
    tokio::select! {
        result = alex_server => result?,
        result = provider_server => result?,
        result = tokio::signal::ctrl_c() => result?,
    }
    Ok(())
}
