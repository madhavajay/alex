use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use alex_auth::{Account, Vault};
use alex_core::Provider;
use alex_fakeprov::{Config as FakeProvConfig, FakeProv, RequestRecord};
use alex_proxy::{apply_upstream_env_overrides, build_state, router};
use alex_store::{Store, TraceFilter};
use axum::http::StatusCode;
use serde_json::{json, Value};

const LOCAL_KEY: &str = "alx-fakeprov-router";

fn temp_root() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "alex-proxy-fakeprov-router-{}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn api_account(id: &str, provider: Provider, key: &str) -> Account {
    Account {
        id: id.into(),
        provider,
        kind: "api_key".into(),
        name: "mock".into(),
        description: None,
        paused: false,
        label: Some(format!("{} mock", provider.as_str())),
        access_token: None,
        refresh_token: None,
        id_token: None,
        api_key: Some(key.into()),
        expires_at_ms: None,
        last_refresh_ms: None,
        account_meta: Value::Null,
        cooldown_until_ms: None,
        status: "active".into(),
        path: None,
    }
}

fn apply_all_provider_overrides(state: &Arc<alex_proxy::AppState>, base_url: &str) {
    let names = [
        "ALEX_UPSTREAM_ANTHROPIC_URL",
        "ALEX_UPSTREAM_OPENAI_URL",
        "ALEX_UPSTREAM_CODEX_URL",
        "ALEX_UPSTREAM_XAI_URL",
        "ALEX_UPSTREAM_GEMINI_URL",
        "ALEX_UPSTREAM_GEMINI_CODE_ASSIST_URL",
        "ALEX_UPSTREAM_OPENROUTER_URL",
        "ALEX_UPSTREAM_KIMI_URL",
        "ALEX_UPSTREAM_AMP_URL",
    ];
    for name in names {
        std::env::set_var(name, base_url);
    }
    apply_upstream_env_overrides(state);
    for name in names {
        std::env::remove_var(name);
    }
}

async fn wait_for_trace(store: &Store, trace_id: &str) -> Value {
    for _ in 0..100 {
        if let Some(trace) = store.get_trace(trace_id).unwrap() {
            return trace;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("trace {trace_id} was not persisted");
}

async fn post(
    client: &reqwest::Client,
    base: &str,
    path: &str,
    body: Value,
    failure: bool,
) -> (reqwest::Response, String) {
    let mut request = client
        .post(format!("{base}{path}"))
        .header("x-api-key", LOCAL_KEY)
        .header("x-alex-harness", "fakeprov-router")
        .header("x-alex-trace-tag", "tier=mock")
        .json(&body);
    if failure {
        request = request.header("x-mock-fail", "429");
    }
    let response = request.send().await.unwrap();
    let trace_id = response
        .headers()
        .get("x-alex-trace-id")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    (response, trace_id)
}

fn assert_trace(
    trace: &Value,
    provider: &str,
    model: &str,
    status: u16,
    usage: Option<(i64, i64)>,
) {
    assert_eq!(trace["upstream_provider"], provider);
    assert_eq!(trace["routed_model"], model);
    assert_eq!(trace["status"], status);
    match usage {
        Some((input, output)) => {
            assert_eq!(trace["input_tokens"], input);
            assert_eq!(trace["output_tokens"], output);
            assert!(trace["error"].is_null());
        }
        None => {
            assert_eq!(trace["error_kind"], "rate_limit_error");
            assert_eq!(trace["error_class"], "capacity");
        }
    }
}

fn assert_upstream_headers(records: &[RequestRecord]) {
    for record in records {
        assert!(record
            .headers
            .keys()
            .all(|name| !name.starts_with("x-alex-")));
        match record.path.as_str() {
            "/v1/messages" => {
                assert_eq!(
                    record.headers.get("x-api-key").map(String::as_str),
                    Some("anthropic-test-key")
                );
                assert!(record.headers.get("authorization").is_none());
            }
            "/v1/chat/completions" | "/v1/responses" => {
                assert_eq!(
                    record.headers.get("authorization").map(String::as_str),
                    Some("Bearer openai-test-key")
                );
                assert!(record.headers.get("x-api-key").is_none());
            }
            "/openrouter/api/v1/credits" => {
                assert_eq!(
                    record.headers.get("authorization").map(String::as_str),
                    Some("Bearer openrouter-test-key")
                );
            }
            path => panic!("unexpected FakeProv request path {path}"),
        }
    }
    assert_eq!(
        records
            .iter()
            .filter(|record| record.headers.get("x-mock-fail").is_some())
            .count(),
        2
    );
}

#[tokio::test]
async fn fakeprov_drives_real_router_and_openrouter_limits_paths() {
    let fakeprov = FakeProv::spawn(FakeProvConfig::default()).await.unwrap();
    let root = temp_root();
    let vault = Arc::new(Vault::open(root.join("accounts")).unwrap());
    vault
        .upsert(api_account(
            "anthropic-mock",
            Provider::Anthropic,
            "anthropic-test-key",
        ))
        .await
        .unwrap();
    vault
        .upsert(api_account(
            "openai-mock",
            Provider::Openai,
            "openai-test-key",
        ))
        .await
        .unwrap();
    vault
        .upsert(api_account(
            "openrouter-mock",
            Provider::Openrouter,
            "openrouter-test-key",
        ))
        .await
        .unwrap();
    let store = Arc::new(Store::open(root.join("store")).unwrap());
    let state = build_state(
        LOCAL_KEY.into(),
        vault,
        store.clone(),
        None,
        "http://127.0.0.1:0".into(),
        Duration::from_secs(30),
    );
    apply_all_provider_overrides(&state, fakeprov.base_url());
    std::env::set_var(
        "ALEX_UPSTREAM_OPENROUTER_URL",
        format!("{}/openrouter", fakeprov.base_url()),
    );
    apply_upstream_env_overrides(&state);
    std::env::remove_var("ALEX_UPSTREAM_OPENROUTER_URL");

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            router(state).into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    let base = format!("http://{address}");
    let client = reqwest::Client::new();

    let limits: Value = client
        .get(format!("{base}/admin/limits"))
        .header("x-api-key", LOCAL_KEY)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let openrouter = limits["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["provider"] == "openrouter")
        .unwrap_or_else(|| panic!("missing openrouter limits entry: {limits}"));
    assert_eq!(openrouter["account_id"], "openrouter-mock");
    assert_eq!(openrouter["individual_credits_usd"], 30.25);

    let (anthropic_response, anthropic_trace_id) = post(
        &client,
        &base,
        "/v1/messages",
        json!({
            "model": "claude-sonnet-4-5",
            "max_tokens": 64,
            "messages": [{"role": "user", "content": "hello"}]
        }),
        false,
    )
    .await;
    assert_eq!(anthropic_response.status(), StatusCode::OK);
    let anthropic_body: Value = anthropic_response.json().await.unwrap();
    assert_eq!(
        anthropic_body["content"][0]["text"],
        "Fake Anthropic response."
    );
    assert_trace(
        &wait_for_trace(&store, &anthropic_trace_id).await,
        "anthropic",
        "claude-sonnet-4-5",
        200,
        Some((8, 4)),
    );

    let (chat_response, chat_trace_id) = post(
        &client,
        &base,
        "/v1/chat/completions",
        json!({
            "model": "gpt-4.1",
            "messages": [{"role": "user", "content": "hello"}]
        }),
        false,
    )
    .await;
    assert_eq!(chat_response.status(), StatusCode::OK);
    let chat_body: Value = chat_response.json().await.unwrap();
    assert_eq!(
        chat_body["choices"][0]["message"]["content"],
        "Fake OpenAI chat response."
    );
    assert_trace(
        &wait_for_trace(&store, &chat_trace_id).await,
        "openai",
        "gpt-4.1",
        200,
        Some((8, 4)),
    );

    let (responses_response, responses_trace_id) = post(
        &client,
        &base,
        "/v1/responses",
        json!({
            "model": "gpt-5.5",
            "stream": false,
            "input": "hello"
        }),
        false,
    )
    .await;
    assert_eq!(responses_response.status(), StatusCode::OK);
    let responses_body: Value = responses_response.json().await.unwrap();
    assert_eq!(
        responses_body["output"][0]["content"][0]["text"],
        "Fake OpenAI Responses response."
    );
    assert_trace(
        &wait_for_trace(&store, &responses_trace_id).await,
        "openai",
        "gpt-5.5",
        200,
        Some((8, 4)),
    );

    let (anthropic_failure, anthropic_failure_trace_id) = post(
        &client,
        &base,
        "/v1/messages",
        json!({
            "model": "claude-sonnet-4-5",
            "max_tokens": 64,
            "messages": [{"role": "user", "content": "fail"}]
        }),
        true,
    )
    .await;
    assert_eq!(anthropic_failure.status(), StatusCode::TOO_MANY_REQUESTS);
    let anthropic_error: Value = anthropic_failure.json().await.unwrap();
    assert_eq!(anthropic_error["error"]["type"], "rate_limit_error");
    assert_trace(
        &wait_for_trace(&store, &anthropic_failure_trace_id).await,
        "anthropic",
        "claude-sonnet-4-5",
        429,
        None,
    );

    let (openai_failure, openai_failure_trace_id) = post(
        &client,
        &base,
        "/v1/responses",
        json!({
            "model": "gpt-5.5",
            "stream": false,
            "input": "fail"
        }),
        true,
    )
    .await;
    assert_eq!(openai_failure.status(), StatusCode::TOO_MANY_REQUESTS);
    let openai_error: Value = openai_failure.json().await.unwrap();
    assert_eq!(openai_error["error"]["type"], "rate_limit_error");
    assert_trace(
        &wait_for_trace(&store, &openai_failure_trace_id).await,
        "openai",
        "gpt-5.5",
        429,
        None,
    );

    let records = fakeprov.requests().await;
    assert_eq!(records.len(), 6);
    assert_upstream_headers(&records);
    assert_eq!(
        store.search_traces(&TraceFilter::default()).unwrap().len(),
        5
    );

    server.abort();
    fakeprov.shutdown().await;
}
