use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use alex_auth::Vault;
use alex_proxy::{build_state, router, set_exo_config, ExoConfig};
use alex_store::{LarArtifactLocation, LarBodyStoreConfig, LarBodyStoreMode, Store};
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

fn emit_report(metrics: Value, gates: Vec<(Value, Option<String>)>) {
    let (gates, failures): (Vec<_>, Vec<_>) = gates.into_iter().unzip();
    let failures = failures.into_iter().flatten().collect::<Vec<_>>();
    eprintln!(
        "ALEX_LAR_BENCHMARK {}",
        serde_json::to_string(&json!({
            "schema": "alex-lar-rollout-benchmark-v1",
            "benchmark": "concurrent_proxy_requests",
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
