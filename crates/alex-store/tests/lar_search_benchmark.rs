//! Manual release-mode gate for normalized FTS and exact raw grep across a
//! mixed active/sealed live catalog.
//!
//! Run with:
//! `cargo test -p alex-store --test lar_search_benchmark --release -- --ignored --nocapture`

use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use alex_core::TraceRecord;
use alex_lar::RawSearchLimits;
use alex_store::{LarBodyArtifact, LarBodyStoreConfig, LarBodyStoreMode, Store, TraceFilter};
use rusqlite::Connection;

const TRACES: usize = 400;
const SAMPLES: usize = 40;
const NEEDLE: &str = "benchmarkneedle-7f4a9d3c";

fn tmpdir() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "alex-lar-search-benchmark-{}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn percentile(samples: &[Duration], percentile: usize) -> Duration {
    let mut samples = samples.to_vec();
    samples.sort_unstable();
    let index = ((samples.len() - 1) * percentile).div_ceil(100);
    samples[index]
}

fn unique_payload(index: usize) -> String {
    let mut output = String::with_capacity(1_600);
    for salt in 0..24 {
        output.push_str(
            &blake3::hash(format!("trace-{index}-payload-{salt}").as_bytes())
                .to_hex()
                .to_string(),
        );
    }
    output
}

#[test]
#[ignore = "manual release search benchmark"]
fn fts_and_raw_grep_latency_across_active_and_sealed_archives() {
    let root = tmpdir();
    let config = LarBodyStoreConfig {
        mode: LarBodyStoreMode::LarWithFallback,
        max_pack_bytes: 32 * 1024,
        ..Default::default()
    };
    let store = Store::open_with_lar_body_store(root.clone(), config).unwrap();
    let common = "shared system prompt and tool schema ".repeat(48);
    let mut logical_body_bytes = 0u64;
    for index in 0..TRACES {
        let marker = if index == TRACES - 1 {
            NEEDLE
        } else {
            "ordinary"
        };
        let body = serde_json::to_vec(&serde_json::json!({
            "model": "benchmark-model",
            "messages": [
                {"role": "system", "content": common},
                {"role": "user", "content": format!("turn {index} {marker} {}", unique_payload(index))}
            ]
        }))
        .unwrap();
        logical_body_bytes += body.len() as u64;
        let trace_id = format!("search-benchmark-{index:04}");
        let written = store
            .write_body_artifact(
                &LarBodyArtifact::trace(&trace_id, "client_request"),
                "request.json",
                &body,
            )
            .unwrap();
        assert!(written.lar_error.is_none());
        store
            .insert_trace(&TraceRecord {
                id: trace_id,
                session_id: Some("search-benchmark-session".into()),
                ts_request_ms: index as i64,
                req_body_path: Some(written.legacy_path.clone()),
                status: Some(200),
                ..Default::default()
            })
            .unwrap();
        std::fs::remove_file(written.legacy_path).unwrap();
    }

    let conn = Connection::open(root.join("alexandria.sqlite3")).unwrap();
    let (sealed, active): (u64, u64) = conn
        .query_row(
            "SELECT
               SUM(CASE WHEN state='sealed' THEN 1 ELSE 0 END),
               SUM(CASE WHEN state='active' THEN 1 ELSE 0 END)
             FROM lar_files",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert!(sealed > 0, "fixture did not rotate a sealed pack");
    assert!(active > 0, "fixture has no active pack");

    let filter = TraceFilter {
        text: Some(NEEDLE.into()),
        limit: 10,
        ..Default::default()
    };
    assert_eq!(store.search_traces(&filter).unwrap().len(), 1);
    assert_eq!(
        store
            .grep_lar_catalog_raw(NEEDLE.as_bytes(), 10, RawSearchLimits::default())
            .unwrap()
            .matches
            .len(),
        1
    );

    let mut fts_samples = Vec::with_capacity(SAMPLES);
    let mut raw_samples = Vec::with_capacity(SAMPLES);
    let mut raw_stats = None;
    for _ in 0..SAMPLES {
        let started = Instant::now();
        let rows = store.search_traces(&filter).unwrap();
        fts_samples.push(started.elapsed());
        assert_eq!(rows.len(), 1);

        let started = Instant::now();
        let report = store
            .grep_lar_catalog_raw(NEEDLE.as_bytes(), 10, RawSearchLimits::default())
            .unwrap();
        raw_samples.push(started.elapsed());
        assert_eq!(report.matches.len(), 1);
        raw_stats = Some(report.stats);
    }
    let stats = raw_stats.unwrap();
    println!(
        "LAR_SEARCH_BENCHMARK traces={TRACES} logical_body_bytes={logical_body_bytes} sealed={sealed} active={active} \
         fts_p50_ms={:.3} fts_p95_ms={:.3} fts_p99_ms={:.3} \
         raw_p50_ms={:.3} raw_p95_ms={:.3} raw_p99_ms={:.3} \
         raw_manifests={} raw_ranges={} raw_logical_bytes={} raw_unique_chunks={} raw_decompressed_bytes={}",
        percentile(&fts_samples, 50).as_secs_f64() * 1_000.0,
        percentile(&fts_samples, 95).as_secs_f64() * 1_000.0,
        percentile(&fts_samples, 99).as_secs_f64() * 1_000.0,
        percentile(&raw_samples, 50).as_secs_f64() * 1_000.0,
        percentile(&raw_samples, 95).as_secs_f64() * 1_000.0,
        percentile(&raw_samples, 99).as_secs_f64() * 1_000.0,
        stats.manifests_scanned,
        stats.manifest_ranges_scanned,
        stats.logical_bytes_scanned,
        stats.unique_chunks_read,
        stats.decompressed_chunk_bytes,
    );
}
