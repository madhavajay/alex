use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use alex_store::{LarBodyArtifact, LarBodyStoreConfig, LarBodyStoreMode, Store};

static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn tmpdir() -> PathBuf {
    let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "alex-live-lar-benchmark-{}-{sequence}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn tree_bytes(path: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| {
            entry
                .file_type()
                .ok()
                .map(|kind| {
                    if kind.is_dir() {
                        tree_bytes(&entry.path())
                    } else if kind.is_file() {
                        entry.metadata().map(|metadata| metadata.len()).unwrap_or(0)
                    } else {
                        0
                    }
                })
                .unwrap_or(0)
        })
        .sum()
}

fn percentile(sorted: &[Duration], percentile: usize) -> Duration {
    let index = (sorted.len().saturating_sub(1) * percentile) / 100;
    sorted[index]
}

/// Manual write-path benchmark for a fresh active pack. It intentionally uses
/// the compatibility mode that also writes gzip, so reported latency includes
/// today's rollback cost rather than an unrealistically isolated codec loop.
#[test]
#[ignore = "manual live LAR latency/storage benchmark"]
fn growing_prefix_live_write_latency_and_pack_growth() {
    const TURNS: usize = 500;
    let root = tmpdir();
    let store = Store::open_with_lar_body_store(
        root.clone(),
        LarBodyStoreConfig {
            mode: LarBodyStoreMode::DualWriteValidated,
            max_pack_bytes: 512 * 1024 * 1024,
            checkpoint_bytes: 8 * 1024 * 1024,
            checkpoint_interval: Duration::from_secs(30),
            writer_lock_timeout: Duration::from_secs(2),
            ..Default::default()
        },
    )
    .unwrap();

    let mut body = vec![b's'; 64 * 1024];
    let mut logical_bytes = 0u64;
    let mut latencies = Vec::with_capacity(TURNS);
    for turn in 0..TURNS {
        body.extend_from_slice(format!("\nturn-{turn:04}: genuinely-new-bytes").as_bytes());
        logical_bytes += body.len() as u64;
        let started = Instant::now();
        let result = store
            .write_body_artifact(
                &LarBodyArtifact::trace(format!("benchmark-{turn:04}"), "client_request"),
                "request.json",
                &body,
            )
            .unwrap();
        assert!(result.lar_error.is_none(), "{:?}", result.lar_error);
        latencies.push(started.elapsed());
    }
    latencies.sort_unstable();
    drop(store);
    let lar_bytes = tree_bytes(&root.join("lar"));
    let legacy_bytes = tree_bytes(&root.join("bodies"));
    let sqlite_bytes = [
        "alexandria.sqlite3",
        "alexandria.sqlite3-wal",
        "alexandria.sqlite3-shm",
    ]
    .iter()
    .map(|name| {
        std::fs::metadata(root.join(name))
            .map(|metadata| metadata.len())
            .unwrap_or(0)
    })
    .sum::<u64>();
    eprintln!(
        "turns={TURNS} logical_bytes={logical_bytes} lar_bytes={lar_bytes} legacy_gzip_bytes={legacy_bytes} sqlite_bytes={sqlite_bytes} lar_ratio={:.2}x p50_ms={:.3} p95_ms={:.3} p99_ms={:.3}",
        logical_bytes as f64 / lar_bytes.max(1) as f64,
        percentile(&latencies, 50).as_secs_f64() * 1000.0,
        percentile(&latencies, 95).as_secs_f64() * 1000.0,
        percentile(&latencies, 99).as_secs_f64() * 1000.0,
    );
    std::fs::remove_dir_all(root).unwrap();
}
