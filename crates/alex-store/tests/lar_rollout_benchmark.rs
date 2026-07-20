use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Barrier};
use std::time::{Duration, Instant};

use alex_core::TraceRecord;
use alex_store::{
    LarBodyArtifact, LarBodyStoreConfig, LarBodyStoreMode, LarLegacyImportBoundary,
    LarLegacyImportHook, LarLegacyImportOptions, LarLegacyResourceControls, LarRepackConfig, Store,
};
use flate2::write::GzEncoder;
use flate2::Compression;
use serde_json::{json, Value};

static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);
const MEMORY_CHILD_ENV: &str = "ALEX_LAR_BENCH_MEMORY_CHILD";

struct BenchmarkDir(PathBuf);

impl BenchmarkDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "alex-lar-rollout-benchmark-{name}-{}-{}",
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

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .map(|value| {
            value
                .parse::<u64>()
                .unwrap_or_else(|error| panic!("{name} must be an unsigned integer: {error}"))
        })
        .unwrap_or(default)
}

fn percentile_ms(sorted: &[Duration], percentile: usize) -> f64 {
    assert!(!sorted.is_empty());
    let index = (sorted.len().saturating_sub(1) * percentile) / 100;
    sorted[index].as_secs_f64() * 1_000.0
}

fn percentiles(mut samples: Vec<Duration>) -> Value {
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
    let result = json!({
        "environment": environment,
        "status": if passed { "pass" } else { "fail" },
        "comparison": comparison,
        "configured": configured,
        "measured": measured,
        "unit": unit,
    });
    let failure = (!passed).then(|| {
        format!(
            "{environment} {comparison} gate failed: measured {measured:.3} {unit}, configured {configured:.3} {unit}"
        )
    });
    (result, failure)
}

fn emit_report(name: &str, metrics: Value, gates: Vec<(Value, Option<String>)>) {
    let (gates, failures): (Vec<_>, Vec<_>) = gates.into_iter().unzip();
    let failures = failures.into_iter().flatten().collect::<Vec<_>>();
    eprintln!(
        "ALEX_LAR_BENCHMARK {}",
        serde_json::to_string(&json!({
            "schema": "alex-lar-rollout-benchmark-v1",
            "benchmark": name,
            "metrics": metrics,
            "gates": gates,
        }))
        .unwrap()
    );
    assert!(failures.is_empty(), "{}", failures.join("; "));
}

fn deterministic_bytes(mut state: u64, length: usize) -> Vec<u8> {
    (0..length)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        })
        .collect()
}

fn deterministic_ascii(state: u64, length: usize) -> Vec<u8> {
    deterministic_bytes(state, length)
        .into_iter()
        .map(|byte| b'a' + (byte % 26))
        .collect()
}

/// A stable, highly repeated prefix plus a deterministic per-request tail.
/// Allocation happens before the measured storage call, as it does after an
/// HTTP stack has already collected the request bytes.
fn agent_request_body(
    worker: usize,
    turn: usize,
    prefix_bytes: usize,
    tail_bytes: usize,
) -> Vec<u8> {
    let mut body = Vec::with_capacity(prefix_bytes + tail_bytes + 128);
    body.extend_from_slice(br#"{"messages":[{"role":"system","content":""#);
    body.extend(std::iter::repeat_n(b's', prefix_bytes));
    body.extend_from_slice(br#""},{"role":"user","content":""#);
    body.extend_from_slice(&deterministic_ascii(
        0x9e37_79b9_u64 ^ ((worker as u64) << 32) ^ turn as u64,
        tail_bytes,
    ));
    body.extend_from_slice(format!(r#""}}],"worker":{worker},"turn":{turn}}}"#).as_bytes());
    body
}

struct WriteWorkloadResult {
    latencies: Vec<Duration>,
    elapsed: Duration,
    logical_bytes: u64,
}

fn run_concurrent_write_workload(
    root: PathBuf,
    mode: LarBodyStoreMode,
    label: &'static str,
    workers: usize,
    turns_per_worker: usize,
    prefix_bytes: usize,
    tail_bytes: usize,
) -> WriteWorkloadResult {
    let store = Arc::new(
        Store::open_with_lar_body_store(
            root,
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
    // Keep first-pack creation and directory initialization out of the sample.
    let warmup = agent_request_body(usize::MAX, 0, prefix_bytes, tail_bytes);
    serde_json::from_slice::<Value>(&warmup).expect("agent request fixture must be valid JSON");
    let warmup = store
        .write_body_artifact(
            &LarBodyArtifact::trace(format!("{label}-warmup"), "client_request"),
            "request.json",
            &warmup,
        )
        .unwrap();
    assert!(warmup.lar_error.is_none(), "{:?}", warmup.lar_error);

    let start = Arc::new(Barrier::new(workers + 1));
    let handles = (0..workers)
        .map(|worker| {
            let store = store.clone();
            let start = start.clone();
            std::thread::spawn(move || {
                let mut samples = Vec::with_capacity(turns_per_worker);
                let mut logical_bytes = 0u64;
                start.wait();
                for turn in 0..turns_per_worker {
                    let body = agent_request_body(worker, turn, prefix_bytes, tail_bytes);
                    logical_bytes = logical_bytes.saturating_add(body.len() as u64);
                    let began = Instant::now();
                    let result = store
                        .write_body_artifact(
                            &LarBodyArtifact::trace(
                                format!("{label}-{worker:02}-{turn:04}"),
                                "client_request",
                            ),
                            "request.json",
                            &body,
                        )
                        .unwrap();
                    samples.push(began.elapsed());
                    assert!(
                        result.lar_error.is_none(),
                        "concurrent LAR fallback would invalidate the sample: {:?}",
                        result.lar_error
                    );
                }
                (samples, logical_bytes)
            })
        })
        .collect::<Vec<_>>();
    let began = Instant::now();
    start.wait();
    let mut latencies = Vec::with_capacity(workers * turns_per_worker);
    let mut logical_bytes = 0u64;
    for handle in handles {
        let (mut worker_latencies, worker_bytes) = handle.join().unwrap();
        latencies.append(&mut worker_latencies);
        logical_bytes = logical_bytes.saturating_add(worker_bytes);
    }
    WriteWorkloadResult {
        latencies,
        elapsed: began.elapsed(),
        logical_bytes,
    }
}

#[test]
#[ignore = "manual release precursor for concurrent storage throughput and added latency"]
fn concurrent_storage_write_throughput_and_added_latency() {
    let workers = env_usize("ALEX_LAR_BENCH_WRITE_WORKERS", 4);
    let turns = env_usize("ALEX_LAR_BENCH_WRITE_TURNS_PER_WORKER", 48);
    let prefix_bytes = env_usize("ALEX_LAR_BENCH_WRITE_PREFIX_BYTES", 64 * 1024);
    let tail_bytes = env_usize("ALEX_LAR_BENCH_WRITE_TAIL_BYTES", 4 * 1024);
    assert!(workers > 0 && turns > 0 && prefix_bytes > 0 && tail_bytes > 0);

    let legacy_dir = BenchmarkDir::new("write-legacy");
    let legacy = run_concurrent_write_workload(
        legacy_dir.path().to_path_buf(),
        LarBodyStoreMode::Legacy,
        "legacy",
        workers,
        turns,
        prefix_bytes,
        tail_bytes,
    );
    let lar_dir = BenchmarkDir::new("write-lar");
    let lar = run_concurrent_write_workload(
        lar_dir.path().to_path_buf(),
        LarBodyStoreMode::DualWriteValidated,
        "dual",
        workers,
        turns,
        prefix_bytes,
        tail_bytes,
    );

    let mut legacy_sorted = legacy.latencies.clone();
    legacy_sorted.sort_unstable();
    let mut lar_sorted = lar.latencies.clone();
    lar_sorted.sort_unstable();
    let added_p50 = percentile_ms(&lar_sorted, 50) - percentile_ms(&legacy_sorted, 50);
    let added_p95 = percentile_ms(&lar_sorted, 95) - percentile_ms(&legacy_sorted, 95);
    let added_p99 = percentile_ms(&lar_sorted, 99) - percentile_ms(&legacy_sorted, 99);
    let operations = workers * turns;
    let lar_ops_per_second = operations as f64 / lar.elapsed.as_secs_f64();
    let lar_mib_per_second =
        lar.logical_bytes as f64 / (1024.0 * 1024.0) / lar.elapsed.as_secs_f64();

    emit_report(
        "concurrent_storage_write",
        json!({
            "workload": {
                "workers": workers,
                "turns_per_worker": turns,
                "operations": operations,
                "prefix_bytes": prefix_bytes,
                "tail_bytes": tail_bytes,
            },
            "legacy": {
                "elapsed_ms": legacy.elapsed.as_secs_f64() * 1_000.0,
                "ops_per_second": operations as f64 / legacy.elapsed.as_secs_f64(),
                "logical_mib_per_second": legacy.logical_bytes as f64 / (1024.0 * 1024.0) / legacy.elapsed.as_secs_f64(),
                "latency": percentiles(legacy.latencies),
            },
            "dual_write_validated": {
                "elapsed_ms": lar.elapsed.as_secs_f64() * 1_000.0,
                "ops_per_second": lar_ops_per_second,
                "logical_mib_per_second": lar_mib_per_second,
                "latency": percentiles(lar.latencies),
            },
            "added_latency_ms": {
                "definition": "dual_write_percentile_minus_legacy_percentile",
                "p50": added_p50,
                "p95": added_p95,
                "p99": added_p99,
            },
        }),
        vec![
            threshold(
                "ALEX_LAR_BENCH_MIN_WRITE_OPS_PER_SECOND",
                lar_ops_per_second,
                "ops/s",
                ThresholdDirection::Minimum,
            ),
            threshold(
                "ALEX_LAR_BENCH_MAX_ADDED_P50_MS",
                added_p50.max(0.0),
                "ms",
                ThresholdDirection::Maximum,
            ),
            threshold(
                "ALEX_LAR_BENCH_MAX_ADDED_P95_MS",
                added_p95.max(0.0),
                "ms",
                ThresholdDirection::Maximum,
            ),
            threshold(
                "ALEX_LAR_BENCH_MAX_ADDED_P99_MS",
                added_p99.max(0.0),
                "ms",
                ThresholdDirection::Maximum,
            ),
        ],
    );
}

fn write_gzip(path: &Path, bytes: &[u8]) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let file = std::fs::File::create(path).unwrap();
    let mut encoder = GzEncoder::new(file, Compression::default());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap().sync_all().unwrap();
}

fn insert_legacy_trace(
    store: &Store,
    root: &Path,
    id: &str,
    session_id: &str,
    timestamp_ms: i64,
    body: &[u8],
) {
    let day = 1 + ((timestamp_ms / 86_400_000).unsigned_abs() % 28);
    let path = root
        .join(format!("bodies/2026-07-{day:02}"))
        .join(format!("{id}.request.json.gz"));
    write_gzip(&path, body);
    store
        .insert_trace(&TraceRecord {
            id: id.into(),
            ts_request_ms: timestamp_ms,
            session_id: Some(session_id.into()),
            req_body_path: Some(path.to_string_lossy().into_owned()),
            ..TraceRecord::default()
        })
        .unwrap();
}

#[test]
#[ignore = "manual release gate for throttled migration throughput and concurrent reads"]
fn throttled_migration_throughput_and_interactive_read_latency() {
    let artifacts = env_usize("ALEX_LAR_BENCH_MIGRATION_ARTIFACTS", 96);
    let body_bytes = env_usize("ALEX_LAR_BENCH_MIGRATION_BODY_BYTES", 64 * 1024);
    let io_bytes_per_second = env_u64(
        "ALEX_LAR_BENCH_MIGRATION_IO_BYTES_PER_SECOND",
        4 * 1024 * 1024,
    );
    let read_interval_ms = env_u64("ALEX_LAR_BENCH_MIGRATION_READ_INTERVAL_MS", 5);
    assert!(artifacts > 0 && body_bytes > 0 && io_bytes_per_second > 0);

    let root = BenchmarkDir::new("migration");
    let store = Arc::new(
        Store::open_with_lar_body_store(
            root.path().to_path_buf(),
            LarBodyStoreConfig {
                mode: LarBodyStoreMode::LarWithFallback,
                writer_lock_timeout: Duration::from_secs(10),
                ..LarBodyStoreConfig::default()
            },
        )
        .unwrap(),
    );
    for index in 0..artifacts {
        let body = deterministic_bytes(0xd1b5_4a32_d192_ed03 ^ index as u64, body_bytes);
        insert_legacy_trace(
            &store,
            root.path(),
            &format!("migration-{index:05}"),
            "migration-benchmark-session",
            1_700_000_000_000 + index as i64,
            &body,
        );
    }

    let (started_tx, started_rx) = mpsc::channel();
    let hook = LarLegacyImportHook::new(move |boundary| {
        if boundary == LarLegacyImportBoundary::JobClaimed {
            let _ = started_tx.send(());
        }
        Ok(())
    });
    let worker_store = store.clone();
    let (finished_tx, finished_rx) = mpsc::channel();
    let migration_started = Instant::now();
    let worker = std::thread::spawn(move || {
        let report = worker_store.run_lar_legacy_import(&LarLegacyImportOptions {
            batch_size: 16,
            lease_owner: "rollout-benchmark-migrator".into(),
            resources: LarLegacyResourceControls {
                worker_count: 2,
                io_bytes_per_second: Some(io_bytes_per_second),
                cpu_budget_percent: 50,
                yield_every_artifacts: 1,
                max_memory_bytes: 16 * 1024 * 1024,
                max_pack_bytes: 64 * 1024 * 1024,
                max_pack_index_entries: 65_536,
                min_free_disk_bytes: None,
            },
            suffix_artifacts: Vec::new(),
            boundary_hook: Some(hook),
            ..LarLegacyImportOptions::default()
        });
        let _ = finished_tx.send(report);
    });
    started_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("migration did not reach the claimed boundary");

    let mut read_latencies = Vec::new();
    let mut read_index = 0usize;
    let report = loop {
        match finished_rx.try_recv() {
            Ok(report) => break report.unwrap(),
            Err(mpsc::TryRecvError::Disconnected) => panic!("migration worker disconnected"),
            Err(mpsc::TryRecvError::Empty) => {}
        }
        let id = format!("migration-{:05}", read_index % artifacts);
        let began = Instant::now();
        let body = store
            .read_lar_or_legacy_artifact("trace", &id, "client_request", None)
            .unwrap()
            .unwrap_or_else(|| panic!("interactive read lost {id}"));
        read_latencies.push(began.elapsed());
        assert_eq!(body.len(), body_bytes);
        read_index += 17;
        if read_interval_ms > 0 {
            std::thread::sleep(Duration::from_millis(read_interval_ms));
        }
    };
    worker.join().unwrap();
    let migration_elapsed = migration_started.elapsed();
    assert_eq!(report.migrated as usize, artifacts);
    assert_eq!(report.failed, 0);
    assert!(
        !read_latencies.is_empty(),
        "the migration completed before any concurrent read was sampled"
    );

    let mut sorted = read_latencies.clone();
    sorted.sort_unstable();
    let read_p50 = percentile_ms(&sorted, 50);
    let read_p95 = percentile_ms(&sorted, 95);
    let read_p99 = percentile_ms(&sorted, 99);
    let actual_mib_per_second =
        (artifacts * body_bytes) as f64 / (1024.0 * 1024.0) / migration_elapsed.as_secs_f64();

    emit_report(
        "throttled_migration",
        json!({
            "workload": {
                "artifacts": artifacts,
                "body_bytes": body_bytes,
                "configured_io_bytes_per_second": io_bytes_per_second,
                "configured_cpu_budget_percent": 50,
                "configured_workers": 2,
                "read_interval_ms": read_interval_ms,
            },
            "migration": {
                "elapsed_ms": migration_elapsed.as_secs_f64() * 1_000.0,
                "logical_mib_per_second": actual_mib_per_second,
                "reported_source_bytes_per_second": report.throughput_bytes_per_second,
                "reported_artifacts_per_second": report.throughput_artifacts_per_second,
                "throttled_ms": report.throttled_ms,
                "yield_count": report.yield_count,
                "packs_rotated": report.packs_rotated,
            },
            "interactive_read_latency": percentiles(read_latencies),
        }),
        vec![
            threshold(
                "ALEX_LAR_BENCH_MIN_MIGRATION_MIB_PER_SECOND",
                actual_mib_per_second,
                "MiB/s",
                ThresholdDirection::Minimum,
            ),
            threshold(
                "ALEX_LAR_BENCH_MAX_MIGRATION_READ_P50_MS",
                read_p50,
                "ms",
                ThresholdDirection::Maximum,
            ),
            threshold(
                "ALEX_LAR_BENCH_MAX_MIGRATION_READ_P95_MS",
                read_p95,
                "ms",
                ThresholdDirection::Maximum,
            ),
            threshold(
                "ALEX_LAR_BENCH_MAX_MIGRATION_READ_P99_MS",
                read_p99,
                "ms",
                ThresholdDirection::Maximum,
            ),
        ],
    );
}

#[cfg(unix)]
fn peak_rss_bytes() -> Option<u64> {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) } != 0 {
        return None;
    }
    let rss = unsafe { usage.assume_init() }.ru_maxrss;
    let rss = u64::try_from(rss).ok()?;
    #[cfg(target_os = "macos")]
    {
        Some(rss)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Some(rss.saturating_mul(1024))
    }
}

#[cfg(not(unix))]
fn peak_rss_bytes() -> Option<u64> {
    None
}

fn rss_mib() -> Option<f64> {
    peak_rss_bytes().map(|bytes| bytes as f64 / (1024.0 * 1024.0))
}

fn run_corpus_memory_child() {
    let days = 14usize;
    let traces_per_day = env_usize("ALEX_LAR_BENCH_TRACES_PER_DAY", 100);
    let body_bytes = env_usize("ALEX_LAR_BENCH_CORPUS_BODY_BYTES", 16 * 1024);
    let artifacts = days * traces_per_day;
    assert!(traces_per_day > 0 && body_bytes >= 1024);
    let baseline_rss_mib = rss_mib();
    let root = BenchmarkDir::new("memory-corpus");
    let store = Store::open_with_lar_body_store(
        root.path().to_path_buf(),
        LarBodyStoreConfig {
            mode: LarBodyStoreMode::LarWithFallback,
            ..LarBodyStoreConfig::default()
        },
    )
    .unwrap();
    let stable_prefix = vec![b'p'; body_bytes - 512];
    for day in 0..days {
        for turn in 0..traces_per_day {
            let index = day * traces_per_day + turn;
            let mut body = stable_prefix.clone();
            body.extend_from_slice(&deterministic_bytes(
                0x94d0_49bb_1331_11eb ^ index as u64,
                512,
            ));
            insert_legacy_trace(
                &store,
                root.path(),
                &format!("corpus-{day:02}-{turn:05}"),
                &format!("corpus-session-{day:02}"),
                1_700_000_000_000 + (day as i64 * 86_400_000) + turn as i64,
                &body,
            );
        }
    }
    let generation_rss_mib = rss_mib();
    let began = Instant::now();
    let report = store
        .run_lar_legacy_import(&LarLegacyImportOptions {
            batch_size: 32,
            lease_owner: "memory-corpus-migrator".into(),
            resources: LarLegacyResourceControls {
                worker_count: 2,
                io_bytes_per_second: None,
                cpu_budget_percent: 100,
                yield_every_artifacts: 32,
                max_memory_bytes: 8 * 1024 * 1024,
                max_pack_bytes: 4 * 1024 * 1024,
                max_pack_index_entries: 32_768,
                min_free_disk_bytes: None,
            },
            suffix_artifacts: Vec::new(),
            ..LarLegacyImportOptions::default()
        })
        .unwrap();
    let elapsed = began.elapsed();
    assert_eq!(report.migrated as usize, artifacts);
    assert_eq!(report.failed, 0);
    let peak_mib = rss_mib();
    let measured = peak_mib.unwrap_or(f64::NAN);
    let gates = if measured.is_finite() {
        vec![threshold(
            "ALEX_LAR_BENCH_MAX_CORPUS_PEAK_RSS_MIB",
            measured,
            "MiB",
            ThresholdDirection::Maximum,
        )]
    } else {
        Vec::new()
    };
    emit_report(
        "synthetic_14_day_corpus_memory",
        json!({
            "workload": {
                "days": days,
                "traces_per_day": traces_per_day,
                "artifacts": artifacts,
                "body_bytes": body_bytes,
                "logical_bytes": artifacts.saturating_mul(body_bytes),
                "configured_migration_memory_mib": 8,
            },
            "elapsed_ms": elapsed.as_secs_f64() * 1_000.0,
            "packs_rotated": report.packs_rotated,
            "rss": {
                "available": peak_mib.is_some(),
                "baseline_peak_mib": baseline_rss_mib,
                "after_generation_peak_mib": generation_rss_mib,
                "final_peak_mib": peak_mib,
                "source": "getrusage(RUSAGE_SELF).ru_maxrss",
            },
        }),
        gates,
    );
}

fn count_lar_files(path: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| {
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                count_lar_files(&entry.path())
            } else if entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "lar")
            {
                1
            } else {
                0
            }
        })
        .sum()
}

fn run_rotation_repack_memory_child() {
    let artifacts = env_usize("ALEX_LAR_BENCH_REPACK_ARTIFACTS", 160);
    let body_bytes = env_usize("ALEX_LAR_BENCH_REPACK_BODY_BYTES", 32 * 1024);
    let max_pack_bytes = env_u64("ALEX_LAR_BENCH_REPACK_MAX_PACK_BYTES", 384 * 1024);
    assert!(artifacts >= 8 && body_bytes >= 1024 && max_pack_bytes > 0);
    let baseline_rss_mib = rss_mib();
    let root = BenchmarkDir::new("memory-rotation-repack");
    let store = Store::open_with_lar_body_store(
        root.path().to_path_buf(),
        LarBodyStoreConfig {
            mode: LarBodyStoreMode::LarWithFallback,
            max_pack_bytes,
            checkpoint_bytes: 128 * 1024,
            checkpoint_interval: Duration::from_secs(30),
            writer_lock_timeout: Duration::from_secs(10),
            ..LarBodyStoreConfig::default()
        },
    )
    .unwrap();
    let began = Instant::now();
    for index in 0..artifacts {
        let body = deterministic_bytes(0xa076_1d64_78bd_642f ^ index as u64, body_bytes);
        let id = format!("repack-{index:05}");
        let written = store
            .write_body_artifact(
                &LarBodyArtifact::trace(&id, "client_request"),
                "request.json",
                &body,
            )
            .unwrap();
        assert!(written.lar_error.is_none(), "{:?}", written.lar_error);
        store
            .insert_trace(&TraceRecord {
                id,
                ts_request_ms: index as i64,
                session_id: Some(format!("repack-session-{}", index % 8)),
                req_body_path: Some(written.legacy_path),
                ..TraceRecord::default()
            })
            .unwrap();
    }
    let packs_after_rotation = count_lar_files(&root.path().join("lar"));
    assert!(
        packs_after_rotation > 1,
        "the configured workload did not force pack rotation"
    );
    for index in (0..artifacts).step_by(2) {
        store.delete_trace(&format!("repack-{index:05}")).unwrap();
    }
    let gc = store.run_lar_gc(1_800_000_000_000).unwrap();
    assert!(gc.unreachable_chunks > 0);
    let config = LarRepackConfig {
        min_garbage_bytes: 1,
        min_garbage_ratio: 0.01,
    };
    let candidates = store.plan_lar_repacks(&config).unwrap();
    assert!(
        !candidates.is_empty(),
        "rotation produced no repack candidate"
    );
    let candidate_count = candidates.len();
    let mut repacked = 0usize;
    let mut logical_bytes_reclaimed = 0u64;
    while let Some(report) = store
        .run_lar_repack(&config, 1_800_000_000_001 + repacked as i64)
        .unwrap()
    {
        assert_eq!(report.state, "complete");
        repacked += 1;
        logical_bytes_reclaimed =
            logical_bytes_reclaimed.saturating_add(report.logical_bytes_reclaimed);
    }
    assert_eq!(repacked, candidate_count);
    let elapsed = began.elapsed();
    let peak_mib = rss_mib();
    let measured = peak_mib.unwrap_or(f64::NAN);
    let gates = if measured.is_finite() {
        vec![threshold(
            "ALEX_LAR_BENCH_MAX_ROTATION_REPACK_PEAK_RSS_MIB",
            measured,
            "MiB",
            ThresholdDirection::Maximum,
        )]
    } else {
        Vec::new()
    };
    emit_report(
        "pack_rotation_repack_memory",
        json!({
            "workload": {
                "artifacts": artifacts,
                "body_bytes": body_bytes,
                "max_pack_bytes": max_pack_bytes,
            },
            "elapsed_ms": elapsed.as_secs_f64() * 1_000.0,
            "packs_after_rotation": packs_after_rotation,
            "repack_candidates": candidate_count,
            "repacked": repacked,
            "logical_bytes_reclaimed": logical_bytes_reclaimed,
            "gc_unreachable_chunks": gc.unreachable_chunks,
            "rss": {
                "available": peak_mib.is_some(),
                "baseline_peak_mib": baseline_rss_mib,
                "final_peak_mib": peak_mib,
                "source": "getrusage(RUSAGE_SELF).ru_maxrss",
            },
        }),
        gates,
    );
}

fn run_memory_child(mode: &str) {
    match mode {
        "corpus" => run_corpus_memory_child(),
        "rotation-repack" => run_rotation_repack_memory_child(),
        other => panic!("unknown memory child mode: {other}"),
    }
}

fn spawn_memory_child(mode: &str) {
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "synthetic_14_day_corpus_and_rotation_repack_peak_rss",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(MEMORY_CHILD_ENV, mode)
        .output()
        .unwrap_or_else(|error| panic!("could not start {mode} memory child: {error}"));
    eprint!("{}", String::from_utf8_lossy(&output.stdout));
    eprint!("{}", String::from_utf8_lossy(&output.stderr));
    assert!(
        output.status.success(),
        "{mode} memory child failed with {}",
        output.status
    );
}

#[test]
#[ignore = "manual release gate for isolated synthetic-corpus, rotation, and repack RSS"]
fn synthetic_14_day_corpus_and_rotation_repack_peak_rss() {
    if let Ok(mode) = std::env::var(MEMORY_CHILD_ENV) {
        run_memory_child(&mode);
        return;
    }
    spawn_memory_child("corpus");
    spawn_memory_child("rotation-repack");
}
