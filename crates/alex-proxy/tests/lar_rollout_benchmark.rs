use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use alex_auth::Vault;
use alex_core::TraceRecord;
use alex_proxy::{build_state, router, set_exo_config, ExoConfig};
use alex_store::{
    LarArtifactLocation, LarBodyArtifact, LarBodyStoreConfig, LarBodyStoreMode, Store,
};
use axum::{routing::post, Json, Router};
use serde_json::{json, Value};

static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct BenchmarkDir(PathBuf);

impl BenchmarkDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "alex-proxy-rollout-benchmark-{name}-{}-{}",
            std::process::id(),
            TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for BenchmarkDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .map(|value| {
            value
                .parse::<usize>()
                .unwrap_or_else(|error| panic!("{name} must be an unsigned integer: {error}"))
        })
        .unwrap_or(default)
}

fn percentile_ms(sorted: &[Duration], percentile: usize) -> f64 {
    assert!(!sorted.is_empty());
    let index = (sorted.len().saturating_sub(1) * percentile) / 100;
    sorted[index].as_secs_f64() * 1_000.0
}

fn percentile_report(mut samples: Vec<Duration>) -> Value {
    samples.sort_unstable();
    json!({
        "samples": samples.len(),
        "p50_ms": percentile_ms(&samples, 50),
        "p95_ms": percentile_ms(&samples, 95),
        "p99_ms": percentile_ms(&samples, 99),
    })
}

#[derive(Clone, Copy)]
enum ThresholdDirection {
    Minimum,
    Maximum,
}

fn threshold(
    environment: &str,
    measured: f64,
    unit: &str,
    direction: ThresholdDirection,
) -> (Value, Option<String>) {
    let Ok(raw) = std::env::var(environment) else {
        return (
            json!({
                "environment": environment,
                "status": "unconfigured",
                "measured": measured,
                "unit": unit,
            }),
            None,
        );
    };
    let configured = raw
        .parse::<f64>()
        .unwrap_or_else(|error| panic!("{environment} must be a number: {error}"));
    assert!(
        configured.is_finite() && configured >= 0.0,
        "{environment} must be a finite non-negative number"
    );
    let passed = match direction {
        ThresholdDirection::Minimum => measured >= configured,
        ThresholdDirection::Maximum => measured <= configured,
    };
    let comparison = match direction {
        ThresholdDirection::Minimum => "minimum",
        ThresholdDirection::Maximum => "maximum",
    };
    let failure = (!passed).then(|| {
        format!(
            "{environment} {comparison} gate failed: measured {measured:.3} {unit}, configured {configured:.3} {unit}"
        )
    });
    (
        json!({
            "environment": environment,
            "status": if passed { "pass" } else { "fail" },
            "comparison": comparison,
            "configured": configured,
            "measured": measured,
            "unit": unit,
        }),
        failure,
    )
}

fn emit_report(benchmark: &str, metrics: Value, gates: Vec<(Value, Option<String>)>) {
    let (gates, failures): (Vec<_>, Vec<_>) = gates.into_iter().unzip();
    let failures = failures.into_iter().flatten().collect::<Vec<_>>();
    eprintln!(
        "ALEX_LAR_BENCHMARK {}",
        serde_json::to_string(&json!({
            "schema": "alex-lar-rollout-benchmark-v1",
            "benchmark": benchmark,
            "metrics": metrics,
            "gates": gates,
        }))
        .unwrap()
    );
    assert!(failures.is_empty(), "{}", failures.join("; "));
}

async fn start_upstream() -> (String, tokio::task::JoinHandle<()>) {
    let upstream = Router::new().route(
        "/v1/chat/completions",
        post(|Json(request): Json<Value>| async move {
            Json(json!({
                "id": "benchmark-completion",
                "object": "chat.completion",
                "model": request["model"],
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": "ok"},
                    "finish_reason": "stop"
                }],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
            }))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, upstream).await.unwrap();
    });
    (format!("http://{address}"), server)
}

fn request_body(worker: usize, turn: usize, prefix_bytes: usize) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "model": "exo/benchmark-model",
        "stream": false,
        "messages": [
            {"role": "system", "content": "s".repeat(prefix_bytes)},
            {"role": "user", "content": format!("worker {worker} turn {turn}")}
        ]
    }))
    .unwrap()
}

async fn send_request(client: &reqwest::Client, proxy_url: &str, session_id: &str, body: Vec<u8>) {
    let response = client
        .post(format!("{proxy_url}/v1/chat/completions"))
        .header("x-api-key", "alx-local")
        .header("x-alexandria-harness", "benchmark")
        .header("x-session-id", session_id)
        .header("content-type", "application/json")
        .body(body)
        .send()
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.bytes().await.unwrap();
    assert_eq!(
        status,
        reqwest::StatusCode::OK,
        "proxy response: {}",
        String::from_utf8_lossy(&bytes)
    );
    assert!(!bytes.is_empty());
}

struct WorkloadResult {
    latencies: Vec<Duration>,
    elapsed: Duration,
    logical_request_bytes: u64,
}

async fn run_proxy_workload(
    mode: LarBodyStoreMode,
    label: &'static str,
    upstream_url: &str,
    workers: usize,
    turns_per_worker: usize,
    prefix_bytes: usize,
) -> WorkloadResult {
    let root = BenchmarkDir::new(label);
    let store = Arc::new(
        Store::open_with_lar_body_store(
            root.path().join("store"),
            LarBodyStoreConfig {
                mode,
                max_pack_bytes: 512 * 1024 * 1024,
                checkpoint_bytes: 8 * 1024 * 1024,
                checkpoint_interval: Duration::from_secs(30),
                writer_lock_timeout: Duration::from_secs(10),
                ..LarBodyStoreConfig::default()
            },
        )
        .unwrap(),
    );
    let vault = Arc::new(Vault::open(root.path().join("vault")).unwrap());
    let state = build_state(
        "alx-local".into(),
        vault,
        store.clone(),
        None,
        "http://127.0.0.1:0".into(),
        Duration::from_secs(60),
    );
    set_exo_config(
        &state,
        ExoConfig {
            url: upstream_url.into(),
            enabled_models: vec!["benchmark-model".into()],
        },
    );
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
    let proxy_url = format!("http://{address}");
    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(workers)
        .build()
        .unwrap();

    send_request(
        &client,
        &proxy_url,
        &format!("{label}-warmup"),
        request_body(usize::MAX, 0, prefix_bytes),
    )
    .await;

    let barrier = Arc::new(tokio::sync::Barrier::new(workers + 1));
    let handles = (0..workers)
        .map(|worker| {
            let client = client.clone();
            let proxy_url = proxy_url.clone();
            let barrier = barrier.clone();
            tokio::spawn(async move {
                let mut latencies = Vec::with_capacity(turns_per_worker);
                let mut logical_request_bytes = 0u64;
                barrier.wait().await;
                for turn in 0..turns_per_worker {
                    let body = request_body(worker, turn, prefix_bytes);
                    logical_request_bytes = logical_request_bytes.saturating_add(body.len() as u64);
                    let began = Instant::now();
                    send_request(&client, &proxy_url, &format!("{label}-{worker:02}"), body).await;
                    latencies.push(began.elapsed());
                }
                (latencies, logical_request_bytes)
            })
        })
        .collect::<Vec<_>>();
    let began = Instant::now();
    barrier.wait().await;
    let mut latencies = Vec::with_capacity(workers * turns_per_worker);
    let mut logical_request_bytes = 0u64;
    for handle in handles {
        let (mut samples, bytes) = handle.await.unwrap();
        latencies.append(&mut samples);
        logical_request_bytes = logical_request_bytes.saturating_add(bytes);
    }
    let elapsed = began.elapsed();

    let traces = store
        .list_traces(workers * turns_per_worker + 10, None, None)
        .unwrap();
    assert_eq!(traces.len(), workers * turns_per_worker + 1);
    if mode != LarBodyStoreMode::Legacy {
        for trace in traces {
            let trace_id = trace["id"].as_str().unwrap();
            let location = store
                .lar_artifact_location("trace", trace_id, "client_request", None)
                .unwrap();
            assert!(
                matches!(&location, Some(LarArtifactLocation::Lar { .. })),
                "trace {trace_id} client request location was {location:?}: {trace}"
            );
        }
    }
    server.abort();
    WorkloadResult {
        latencies,
        elapsed,
        logical_request_bytes,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "manual release gate for representative concurrent proxy traffic"]
async fn concurrent_proxy_throughput_and_added_request_latency() {
    let workers = env_usize("ALEX_LAR_BENCH_PROXY_WORKERS", 4);
    let turns_per_worker = env_usize("ALEX_LAR_BENCH_PROXY_TURNS_PER_WORKER", 24);
    let prefix_bytes = env_usize("ALEX_LAR_BENCH_PROXY_PREFIX_BYTES", 32 * 1024);
    assert!(workers > 0 && turns_per_worker > 0 && prefix_bytes > 0);
    let (upstream_url, upstream) = start_upstream().await;
    let legacy = run_proxy_workload(
        LarBodyStoreMode::Legacy,
        "legacy-proxy",
        &upstream_url,
        workers,
        turns_per_worker,
        prefix_bytes,
    )
    .await;
    let lar = run_proxy_workload(
        LarBodyStoreMode::LarWithFallback,
        "lar-proxy",
        &upstream_url,
        workers,
        turns_per_worker,
        prefix_bytes,
    )
    .await;
    upstream.abort();

    let mut legacy_sorted = legacy.latencies.clone();
    legacy_sorted.sort_unstable();
    let mut lar_sorted = lar.latencies.clone();
    lar_sorted.sort_unstable();
    let added_p50 = percentile_ms(&lar_sorted, 50) - percentile_ms(&legacy_sorted, 50);
    let added_p95 = percentile_ms(&lar_sorted, 95) - percentile_ms(&legacy_sorted, 95);
    let added_p99 = percentile_ms(&lar_sorted, 99) - percentile_ms(&legacy_sorted, 99);
    let operations = workers * turns_per_worker;
    let lar_ops_per_second = operations as f64 / lar.elapsed.as_secs_f64();
    let lar_mib_per_second =
        lar.logical_request_bytes as f64 / (1024.0 * 1024.0) / lar.elapsed.as_secs_f64();

    emit_report(
        "concurrent_proxy_requests",
        json!({
            "workload": {
                "workers": workers,
                "turns_per_worker": turns_per_worker,
                "operations": operations,
                "request_prefix_bytes": prefix_bytes,
                "transport": "loopback_http",
                "upstream": "deterministic_local_exo_openai_chat",
            },
            "legacy": {
                "elapsed_ms": legacy.elapsed.as_secs_f64() * 1_000.0,
                "ops_per_second": operations as f64 / legacy.elapsed.as_secs_f64(),
                "latency": percentile_report(legacy.latencies),
            },
            "lar_with_fallback": {
                "elapsed_ms": lar.elapsed.as_secs_f64() * 1_000.0,
                "ops_per_second": lar_ops_per_second,
                "logical_request_mib_per_second": lar_mib_per_second,
                "latency": percentile_report(lar.latencies),
            },
            "added_request_latency_ms": {
                "definition": "lar_with_fallback_end_to_end_percentile_minus_legacy_end_to_end_percentile",
                "p50": added_p50,
                "p95": added_p95,
                "p99": added_p99,
            },
        }),
        vec![
            threshold(
                "ALEX_LAR_BENCH_MIN_PROXY_OPS_PER_SECOND",
                lar_ops_per_second,
                "ops/s",
                ThresholdDirection::Minimum,
            ),
            threshold(
                "ALEX_LAR_BENCH_MAX_PROXY_ADDED_P50_MS",
                added_p50.max(0.0),
                "ms",
                ThresholdDirection::Maximum,
            ),
            threshold(
                "ALEX_LAR_BENCH_MAX_PROXY_ADDED_P95_MS",
                added_p95.max(0.0),
                "ms",
                ThresholdDirection::Maximum,
            ),
            threshold(
                "ALEX_LAR_BENCH_MAX_PROXY_ADDED_P99_MS",
                added_p99.max(0.0),
                "ms",
                ThresholdDirection::Maximum,
            ),
        ],
    );
}

const BROWSER_PAGE_SIZE: usize = 50;
const BROWSER_BODY_BYTE_BUDGET: u64 = 16 * 1024 * 1024;
const BROWSER_SEARCH_MARKER: &str = "tracebrowserbenchmark-7f4a9d3c";

fn browser_request_body(index: usize, prefix: &str, search_index: usize) -> Vec<u8> {
    let marker = if index == search_index {
        BROWSER_SEARCH_MARKER
    } else {
        "ordinary"
    };
    serde_json::to_vec(&json!({
        "model": "benchmark-model",
        "messages": [
            {"role": "system", "content": prefix},
            {
                "role": "user",
                "content": format!(
                    "long session turn {index:05} {marker} nonce-{index:016x}-{:016x}",
                    index.wrapping_mul(1_048_583)
                )
            }
        ]
    }))
    .unwrap()
}

fn browser_response_body(index: usize) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "id": format!("benchmark-completion-{index:05}"),
        "object": "chat.completion",
        "model": "benchmark-model",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": format!(
                    "synthetic answer {index:05} result-{:016x}",
                    index.wrapping_mul(7_919)
                )
            },
            "finish_reason": "stop"
        }]
    }))
    .unwrap()
}

fn seed_lar_only_browser_trace(
    store: &Store,
    session_id: &str,
    trace_id: &str,
    timestamp_ms: i64,
    request: &[u8],
    response: &[u8],
) -> u64 {
    let request_write = store
        .write_body_artifact(
            &LarBodyArtifact::trace(trace_id, "client_request"),
            "request.json",
            request,
        )
        .unwrap();
    let response_write = store
        .write_body_artifact(
            &LarBodyArtifact::trace(trace_id, "client_response"),
            "response.body",
            response,
        )
        .unwrap();
    assert!(
        request_write.lar_error.is_none() && request_write.manifest_id.is_some(),
        "request LAR write failed for {trace_id}: {:?}",
        request_write.lar_error
    );
    assert!(
        response_write.lar_error.is_none() && response_write.manifest_id.is_some(),
        "response LAR write failed for {trace_id}: {:?}",
        response_write.lar_error
    );

    store
        .insert_trace(&TraceRecord {
            id: trace_id.to_string(),
            ts_request_ms: timestamp_ms,
            ts_response_ms: Some(timestamp_ms + 800),
            session_id: Some(session_id.to_string()),
            harness: Some("benchmark".into()),
            client_format: Some("openai-chat".into()),
            upstream_provider: Some("benchmark".into()),
            upstream_format: Some("openai-chat".into()),
            requested_model: Some("benchmark-model".into()),
            routed_model: Some("benchmark-model".into()),
            status: Some(200),
            req_body_path: Some(request_write.legacy_path.clone()),
            resp_body_path: Some(response_write.legacy_path.clone()),
            ..TraceRecord::default()
        })
        .unwrap();

    for path in [&request_write.legacy_path, &response_write.legacy_path] {
        std::fs::remove_file(path)
            .unwrap_or_else(|error| panic!("removing benchmark fallback {path}: {error}"));
    }
    for artifact_kind in ["client_request", "client_response"] {
        let location = store
            .lar_artifact_location("trace", trace_id, artifact_kind, None)
            .unwrap();
        assert!(
            matches!(location, Some(LarArtifactLocation::Lar { .. })),
            "{trace_id} {artifact_kind} was not published to LAR: {location:?}"
        );
    }
    request.len() as u64 + response.len() as u64
}

async fn benchmark_json_get(request: reqwest::RequestBuilder, label: &str) -> (Duration, Value) {
    let started = Instant::now();
    let response = request
        .header("x-api-key", "alx-local")
        .send()
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.bytes().await.unwrap();
    assert_eq!(
        status,
        reqwest::StatusCode::OK,
        "{label} response: {}",
        String::from_utf8_lossy(&bytes)
    );
    let value = serde_json::from_slice(&bytes)
        .unwrap_or_else(|error| panic!("decoding {label} response: {error}"));
    (started.elapsed(), value)
}

fn assert_browser_page(value: &Value, session_id: &str, expected_turns: usize, total: usize) {
    assert_eq!(value["session_id"], session_id);
    assert_eq!(
        value["turns"].as_array().map(Vec::len),
        Some(expected_turns)
    );
    assert_eq!(value["total_turns"].as_u64(), Some(total as u64));
    assert_eq!(
        value["body_byte_budget"].as_u64(),
        Some(BROWSER_BODY_BYTE_BUDGET)
    );
    assert!(value["body_bytes_loaded"].as_u64().unwrap_or(u64::MAX) <= BROWSER_BODY_BYTE_BUDGET);
    assert_eq!(value["body_errors"].as_array().map(Vec::len), Some(0));
    assert_eq!(value["body_truncations"].as_array().map(Vec::len), Some(0));
}

fn sample_p99_ms(samples: &[Duration]) -> f64 {
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    percentile_ms(&sorted, 99)
}

/// Manual release-mode backend gate for the production Trace Browser path.
/// This deliberately stops at decoded HTTP responses; AppKit/SwiftUI rendering
/// and loading-indicator stability remain part of the separate macOS UI gate.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "manual release gate for long-session Trace Browser backend latency"]
async fn long_session_trace_browser_http_paging_and_cancellation() {
    let turns = env_usize("ALEX_LAR_BENCH_BROWSER_TURNS", 1_500);
    let samples = env_usize("ALEX_LAR_BENCH_BROWSER_SAMPLES", 40);
    let prefix_bytes = env_usize("ALEX_LAR_BENCH_BROWSER_PREFIX_BYTES", 32 * 1024);
    let cancellation_requests = env_usize("ALEX_LAR_BENCH_BROWSER_CANCEL_REQUESTS", 8);
    assert!(turns >= BROWSER_PAGE_SIZE * 3);
    assert!(samples > 0 && prefix_bytes > 0 && cancellation_requests > 0);

    let root = BenchmarkDir::new("trace-browser");
    let store = Arc::new(
        Store::open_with_lar_body_store(
            root.path().join("store"),
            LarBodyStoreConfig {
                mode: LarBodyStoreMode::LarWithFallback,
                // Keep both active and sealed packs in even the reduced local
                // smoke profile; production defaults remain exercised by the
                // writer/index code rather than by a huge benchmark fixture.
                max_pack_bytes: 16 * 1024,
                checkpoint_bytes: 8 * 1024,
                checkpoint_interval: Duration::from_secs(30),
                writer_lock_timeout: Duration::from_secs(10),
                ..LarBodyStoreConfig::default()
            },
        )
        .unwrap(),
    );
    let long_session = "benchmark-long-session";
    let navigation_session = "benchmark-navigation-session";
    let search_index = turns / 2;
    let prefix = "stable system prompt and tool schema "
        .repeat(prefix_bytes.div_ceil("stable system prompt and tool schema ".len()));
    let prefix = &prefix[..prefix_bytes];
    let base_timestamp_ms = 1_780_000_000_000i64;
    let setup_started = Instant::now();
    let mut logical_body_bytes = 0u64;
    for index in 0..turns {
        let trace_id = format!("browser-long-{index:05}");
        logical_body_bytes = logical_body_bytes.saturating_add(seed_lar_only_browser_trace(
            &store,
            long_session,
            &trace_id,
            base_timestamp_ms + index as i64 * 1_000,
            &browser_request_body(index, prefix, search_index),
            &browser_response_body(index),
        ));
    }
    for index in 0..3 {
        let trace_id = format!("browser-navigation-{index:02}");
        logical_body_bytes = logical_body_bytes.saturating_add(seed_lar_only_browser_trace(
            &store,
            navigation_session,
            &trace_id,
            base_timestamp_ms + turns as i64 * 1_000 + index as i64 * 1_000,
            &browser_request_body(turns + index, prefix, usize::MAX),
            &browser_response_body(turns + index),
        ));
    }
    let setup_elapsed = setup_started.elapsed();
    let archive_statuses = store.lar_archive_file_statuses().unwrap();
    let sealed_packs = archive_statuses
        .iter()
        .filter(|file| file.role == "body-pack" && file.catalog_state == "sealed")
        .count();
    let active_packs = archive_statuses
        .iter()
        .filter(|file| file.role == "body-pack" && file.catalog_state == "active")
        .count();
    assert!(sealed_packs > 0, "fixture did not rotate a sealed LAR pack");
    assert!(
        active_packs > 0,
        "fixture did not retain an active LAR pack"
    );

    let vault = Arc::new(Vault::open(root.path().join("vault")).unwrap());
    let state = build_state(
        "alx-local".into(),
        vault,
        store,
        None,
        "http://127.0.0.1:0".into(),
        Duration::from_secs(60),
    );
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
    let base_url = format!("http://{address}");
    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(cancellation_requests + 4)
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap();
    let transcript_url = format!("{base_url}/traces/sessions/{long_session}/transcript");

    let (_, tail) = benchmark_json_get(
        client
            .get(&transcript_url)
            .query(&[("limit", BROWSER_PAGE_SIZE), ("tail", 1usize)]),
        "warm tail transcript",
    )
    .await;
    assert_browser_page(&tail, long_session, BROWSER_PAGE_SIZE, turns);
    assert_eq!(tail["has_more_before"], true);
    assert_eq!(tail["has_more_after"], false);
    let oldest_ts_ms = tail["oldest_ts_ms"].as_i64().unwrap();
    let oldest_trace_id = tail["oldest_trace_id"].as_str().unwrap().to_string();

    let (_, older) = benchmark_json_get(
        client.get(&transcript_url).query(&[
            ("limit", BROWSER_PAGE_SIZE.to_string()),
            ("before_ms", oldest_ts_ms.to_string()),
            ("before_id", oldest_trace_id.clone()),
        ]),
        "warm older transcript",
    )
    .await;
    assert_browser_page(&older, long_session, BROWSER_PAGE_SIZE, turns);
    assert_eq!(older["has_more_after"], true);

    let mut tail_samples = Vec::with_capacity(samples);
    let mut older_samples = Vec::with_capacity(samples);
    let mut search_anchor_samples = Vec::with_capacity(samples);
    let mut navigation_samples = Vec::with_capacity(samples);
    let mut health_samples = Vec::with_capacity(samples);
    let mut cancelled_tasks = 0usize;
    let mut completed_before_abort = 0usize;
    for _ in 0..samples {
        let (elapsed, page) = benchmark_json_get(
            client
                .get(&transcript_url)
                .query(&[("limit", BROWSER_PAGE_SIZE), ("tail", 1usize)]),
            "tail transcript",
        )
        .await;
        assert_browser_page(&page, long_session, BROWSER_PAGE_SIZE, turns);
        tail_samples.push(elapsed);

        let (elapsed, page) = benchmark_json_get(
            client.get(&transcript_url).query(&[
                ("limit", BROWSER_PAGE_SIZE.to_string()),
                ("before_ms", oldest_ts_ms.to_string()),
                ("before_id", oldest_trace_id.clone()),
            ]),
            "older transcript",
        )
        .await;
        assert_browser_page(&page, long_session, BROWSER_PAGE_SIZE, turns);
        older_samples.push(elapsed);

        let search_started = Instant::now();
        let (_, search) = benchmark_json_get(
            client
                .get(format!("{base_url}/traces/search"))
                .query(&[("text", BROWSER_SEARCH_MARKER), ("limit", "1")]),
            "trace search",
        )
        .await;
        let match_row = &search["traces"].as_array().unwrap()[0];
        assert_eq!(match_row["id"], format!("browser-long-{search_index:05}"));
        assert_eq!(match_row["session_id"], long_session);
        let anchor_ts_ms = match_row["ts_request_ms"].as_i64().unwrap();
        let (_, page) = benchmark_json_get(
            client.get(&transcript_url).query(&[
                ("limit", BROWSER_PAGE_SIZE.to_string()),
                ("after_ms", anchor_ts_ms.saturating_sub(1).to_string()),
                ("after_id", "\u{10ffff}".to_string()),
            ]),
            "search-anchored transcript",
        )
        .await;
        assert_browser_page(&page, long_session, BROWSER_PAGE_SIZE, turns);
        assert_eq!(
            page["turns"][0]["trace_id"],
            format!("browser-long-{search_index:05}")
        );
        search_anchor_samples.push(search_started.elapsed());

        let barrier = Arc::new(tokio::sync::Barrier::new(cancellation_requests + 1));
        let mut requests = Vec::with_capacity(cancellation_requests);
        for _ in 0..cancellation_requests {
            let client = client.clone();
            let transcript_url = transcript_url.clone();
            let barrier = barrier.clone();
            requests.push(tokio::spawn(async move {
                barrier.wait().await;
                let response = client
                    .get(transcript_url)
                    .header("x-api-key", "alx-local")
                    .query(&[("limit", BROWSER_PAGE_SIZE), ("tail", 1usize)])
                    .send()
                    .await?;
                let status = response.status();
                response.bytes().await?;
                Ok::<_, reqwest::Error>(status)
            }));
        }
        barrier.wait().await;
        tokio::time::sleep(Duration::from_millis(1)).await;
        for request in &requests {
            request.abort();
        }
        for request in requests {
            match request.await {
                Err(error) if error.is_cancelled() => cancelled_tasks += 1,
                Ok(Ok(status)) => {
                    assert_eq!(status, reqwest::StatusCode::OK);
                    completed_before_abort += 1;
                }
                Ok(Err(error)) => panic!("cancellation request failed before abort: {error}"),
                Err(error) => panic!("cancellation request task failed: {error}"),
            }
        }

        let navigation_url = format!("{base_url}/traces/sessions/{navigation_session}/transcript");
        let (elapsed, page) = benchmark_json_get(
            client
                .get(navigation_url)
                .query(&[("limit", BROWSER_PAGE_SIZE), ("tail", 1usize)]),
            "post-cancellation navigation transcript",
        )
        .await;
        assert_browser_page(&page, navigation_session, 3, 3);
        navigation_samples.push(elapsed);

        let (elapsed, health) = benchmark_json_get(
            client.get(format!("{base_url}/health")),
            "post-cancellation health",
        )
        .await;
        assert_eq!(health["status"], "ok");
        health_samples.push(elapsed);
    }
    server.abort();

    let tail_p99 = sample_p99_ms(&tail_samples);
    let older_p99 = sample_p99_ms(&older_samples);
    let search_anchor_p99 = sample_p99_ms(&search_anchor_samples);
    let navigation_p99 = sample_p99_ms(&navigation_samples);
    let health_p99 = sample_p99_ms(&health_samples);
    emit_report(
        "long_session_trace_browser_backend",
        json!({
            "workload": {
                "turns": turns,
                "page_size": BROWSER_PAGE_SIZE,
                "samples": samples,
                "request_prefix_bytes": prefix_bytes,
                "logical_body_bytes": logical_body_bytes,
                "body_byte_budget": BROWSER_BODY_BYTE_BUDGET,
                "storage": "validated_lar_pointers_with_legacy_files_removed",
                "sealed_body_packs": sealed_packs,
                "active_body_packs": active_packs,
                "cancellation_requests_per_sample": cancellation_requests,
                "abort_signals_sent": samples.saturating_mul(cancellation_requests),
                "cancelled_client_tasks": cancelled_tasks,
                "completed_before_abort": completed_before_abort,
                "transport": "loopback_http_public_trace_routes",
            },
            "setup_elapsed_ms": setup_elapsed.as_secs_f64() * 1_000.0,
            "tail_page_latency": percentile_report(tail_samples),
            "older_page_latency": percentile_report(older_samples),
            "search_then_anchor_page_latency": percentile_report(search_anchor_samples),
            "post_cancellation_navigation_latency": percentile_report(navigation_samples),
            "post_cancellation_health_latency": percentile_report(health_samples),
            "scope": "backend_only_no_macos_rendering",
        }),
        vec![
            threshold(
                "ALEX_LAR_BENCH_MAX_BROWSER_TAIL_P99_MS",
                tail_p99,
                "ms",
                ThresholdDirection::Maximum,
            ),
            threshold(
                "ALEX_LAR_BENCH_MAX_BROWSER_OLDER_P99_MS",
                older_p99,
                "ms",
                ThresholdDirection::Maximum,
            ),
            threshold(
                "ALEX_LAR_BENCH_MAX_BROWSER_SEARCH_ANCHOR_P99_MS",
                search_anchor_p99,
                "ms",
                ThresholdDirection::Maximum,
            ),
            threshold(
                "ALEX_LAR_BENCH_MAX_BROWSER_POST_CANCEL_NAVIGATION_P99_MS",
                navigation_p99,
                "ms",
                ThresholdDirection::Maximum,
            ),
            threshold(
                "ALEX_LAR_BENCH_MAX_BROWSER_POST_CANCEL_HEALTH_P99_MS",
                health_p99,
                "ms",
                ThresholdDirection::Maximum,
            ),
        ],
    );
}
