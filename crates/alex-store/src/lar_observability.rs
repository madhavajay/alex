//! Bounded in-process observability for the live LAR body path.
//!
//! These accumulators deliberately retain counts, totals, maxima, and a fixed
//! set of exponential histogram buckets only. They make the storage phases
//! visible without keeping per-request samples or allowing telemetry memory
//! use to grow with trace volume.

use std::io::{self, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, serde::Serialize, PartialEq)]
pub struct LarHistogramBucket {
    pub le_ns: u64,
    pub count: u64,
}

#[derive(Debug, Clone, serde::Serialize, PartialEq)]
pub struct LarLatencySnapshot {
    pub samples: u64,
    pub total_ns: u64,
    pub average_ns: u64,
    pub max_ns: u64,
    pub p50_upper_bound_ns: u64,
    pub p95_upper_bound_ns: u64,
    pub p99_upper_bound_ns: u64,
    pub histogram: Vec<LarHistogramBucket>,
}

#[derive(Debug, Clone, serde::Serialize, PartialEq)]
pub struct LarWriteMetricsSnapshot {
    pub operations: u64,
    pub successful_operations: u64,
    pub failures: u64,
    pub attempted_body_bytes: u64,
    pub committed_body_bytes: u64,
    pub whole_body_dedup_hits: u64,
    pub whole_body_deduplicated_bytes: u64,
    pub whole_body_dedup_ratio: f64,
    pub chunk_references: u64,
    pub candidate_chunk_bytes: u64,
    pub planned_new_chunks: u64,
    pub new_chunk_bytes: u64,
    pub new_compressed_bytes: u64,
    pub chunk_dedup_ratio: f64,
    pub chunker_latency: LarLatencySnapshot,
    pub hash_latency: LarLatencySnapshot,
    pub compression_latency: LarLatencySnapshot,
    pub append_latency: LarLatencySnapshot,
    pub flush_latency: LarLatencySnapshot,
    pub sqlite_commit_latency: LarLatencySnapshot,
}

#[derive(Debug, Clone, serde::Serialize, PartialEq)]
pub struct LarReadMetricsSnapshot {
    pub operations: u64,
    pub failures: u64,
    pub reconstructed_bytes: u64,
    pub time_to_first_byte: LarLatencySnapshot,
    pub reconstruction_latency: LarLatencySnapshot,
    pub reconstruction_bytes_per_second: f64,
}

#[derive(Debug, Clone, serde::Serialize, PartialEq)]
pub struct LarRuntimeMetricsSnapshot {
    pub since_ms: u64,
    pub writes: LarWriteMetricsSnapshot,
    pub reads: LarReadMetricsSnapshot,
}

#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct LarHealthMetricsSnapshot {
    pub state: &'static str,
    pub since_ms: u64,
    pub write_failures: u64,
    pub read_failures: u64,
    pub last_write_failure_ms: Option<u64>,
    pub last_read_failure_ms: Option<u64>,
}

struct LatencyMetric {
    samples: AtomicU64,
    total_ns: AtomicU64,
    max_ns: AtomicU64,
    histogram: [AtomicU64; HISTOGRAM_BUCKETS],
}

const HISTOGRAM_BUCKETS: usize = 32;

impl Default for LatencyMetric {
    fn default() -> Self {
        Self {
            samples: AtomicU64::new(0),
            total_ns: AtomicU64::new(0),
            max_ns: AtomicU64::new(0),
            histogram: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }
}

impl LatencyMetric {
    fn record(&self, elapsed: Duration) {
        let elapsed = duration_ns(elapsed);
        saturating_add(&self.samples, 1);
        saturating_add(&self.total_ns, elapsed);
        self.max_ns.fetch_max(elapsed, Ordering::Relaxed);
        saturating_add(&self.histogram[histogram_index(elapsed)], 1);
    }

    fn snapshot(&self) -> LarLatencySnapshot {
        let samples = self.samples.load(Ordering::Relaxed);
        let total_ns = self.total_ns.load(Ordering::Relaxed);
        let histogram = self
            .histogram
            .iter()
            .enumerate()
            .map(|(index, count)| LarHistogramBucket {
                le_ns: histogram_upper_bound(index),
                count: count.load(Ordering::Relaxed),
            })
            .collect::<Vec<_>>();
        LarLatencySnapshot {
            samples,
            total_ns,
            average_ns: total_ns.checked_div(samples).unwrap_or(0),
            max_ns: self.max_ns.load(Ordering::Relaxed),
            p50_upper_bound_ns: histogram_quantile(&histogram, samples, 50),
            p95_upper_bound_ns: histogram_quantile(&histogram, samples, 95),
            p99_upper_bound_ns: histogram_quantile(&histogram, samples, 99),
            histogram,
        }
    }
}

pub(crate) struct LarRuntimeMetrics {
    since_ms: u64,
    write_operations: AtomicU64,
    successful_write_operations: AtomicU64,
    write_failures: AtomicU64,
    last_write_failure_ms: AtomicU64,
    attempted_body_bytes: AtomicU64,
    committed_body_bytes: AtomicU64,
    whole_body_dedup_hits: AtomicU64,
    whole_body_deduplicated_bytes: AtomicU64,
    chunk_references: AtomicU64,
    candidate_chunk_bytes: AtomicU64,
    planned_new_chunks: AtomicU64,
    new_chunk_bytes: AtomicU64,
    new_compressed_bytes: AtomicU64,
    chunker_latency: LatencyMetric,
    hash_latency: LatencyMetric,
    compression_latency: LatencyMetric,
    append_latency: LatencyMetric,
    flush_latency: LatencyMetric,
    sqlite_commit_latency: LatencyMetric,
    read_operations: AtomicU64,
    read_failures: AtomicU64,
    last_read_failure_ms: AtomicU64,
    reconstructed_bytes: AtomicU64,
    read_ttft: LatencyMetric,
    read_latency: LatencyMetric,
}

impl Default for LarRuntimeMetrics {
    fn default() -> Self {
        Self {
            since_ms: unix_time_ms(),
            write_operations: AtomicU64::new(0),
            successful_write_operations: AtomicU64::new(0),
            write_failures: AtomicU64::new(0),
            last_write_failure_ms: AtomicU64::new(0),
            attempted_body_bytes: AtomicU64::new(0),
            committed_body_bytes: AtomicU64::new(0),
            whole_body_dedup_hits: AtomicU64::new(0),
            whole_body_deduplicated_bytes: AtomicU64::new(0),
            chunk_references: AtomicU64::new(0),
            candidate_chunk_bytes: AtomicU64::new(0),
            planned_new_chunks: AtomicU64::new(0),
            new_chunk_bytes: AtomicU64::new(0),
            new_compressed_bytes: AtomicU64::new(0),
            chunker_latency: LatencyMetric::default(),
            hash_latency: LatencyMetric::default(),
            compression_latency: LatencyMetric::default(),
            append_latency: LatencyMetric::default(),
            flush_latency: LatencyMetric::default(),
            sqlite_commit_latency: LatencyMetric::default(),
            read_operations: AtomicU64::new(0),
            read_failures: AtomicU64::new(0),
            last_read_failure_ms: AtomicU64::new(0),
            reconstructed_bytes: AtomicU64::new(0),
            read_ttft: LatencyMetric::default(),
            read_latency: LatencyMetric::default(),
        }
    }
}

impl LarRuntimeMetrics {
    pub(crate) fn record_write(&self, bytes: u64, failed: bool) {
        saturating_add(&self.write_operations, 1);
        saturating_add(&self.attempted_body_bytes, bytes);
        if failed {
            saturating_add(&self.write_failures, 1);
            self.last_write_failure_ms
                .store(unix_time_ms(), Ordering::Relaxed);
        } else {
            saturating_add(&self.successful_write_operations, 1);
            saturating_add(&self.committed_body_bytes, bytes);
        }
    }

    pub(crate) fn record_whole_body_dedup_hit(&self, bytes: u64) {
        saturating_add(&self.whole_body_dedup_hits, 1);
        saturating_add(&self.whole_body_deduplicated_bytes, bytes);
    }

    pub(crate) fn record_chunk_candidates(&self, references: u64, bytes: u64) {
        saturating_add(&self.chunk_references, references);
        saturating_add(&self.candidate_chunk_bytes, bytes);
    }

    pub(crate) fn record_new_chunk_plan(&self, new_chunks: u64) {
        saturating_add(&self.planned_new_chunks, new_chunks);
    }

    pub(crate) fn record_new_chunk_bytes(&self, uncompressed: u64, compressed: u64) {
        saturating_add(&self.new_chunk_bytes, uncompressed);
        saturating_add(&self.new_compressed_bytes, compressed);
    }

    pub(crate) fn record_chunker(&self, elapsed: Duration) {
        self.chunker_latency.record(elapsed);
    }

    pub(crate) fn record_hash(&self, elapsed: Duration) {
        self.hash_latency.record(elapsed);
    }

    pub(crate) fn record_compression(&self, elapsed: Duration) {
        self.compression_latency.record(elapsed);
    }

    pub(crate) fn record_append(&self, elapsed: Duration) {
        self.append_latency.record(elapsed);
    }

    pub(crate) fn record_flush(&self, elapsed: Duration) {
        self.flush_latency.record(elapsed);
    }

    pub(crate) fn record_sqlite_commit(&self, elapsed: Duration) {
        self.sqlite_commit_latency.record(elapsed);
    }

    pub(crate) fn record_read(
        &self,
        bytes: u64,
        ttft: Option<Duration>,
        elapsed: Duration,
        failed: bool,
    ) {
        saturating_add(&self.read_operations, 1);
        saturating_add(&self.reconstructed_bytes, bytes);
        if failed {
            saturating_add(&self.read_failures, 1);
            self.last_read_failure_ms
                .store(unix_time_ms(), Ordering::Relaxed);
        }
        if let Some(ttft) = ttft {
            self.read_ttft.record(ttft);
        }
        self.read_latency.record(elapsed);
    }

    pub(crate) fn snapshot(&self) -> LarRuntimeMetricsSnapshot {
        let chunk_references = self.chunk_references.load(Ordering::Relaxed);
        let candidate_chunk_bytes = self.candidate_chunk_bytes.load(Ordering::Relaxed);
        let planned_new_chunks = self.planned_new_chunks.load(Ordering::Relaxed);
        let write_operations = self.write_operations.load(Ordering::Relaxed);
        let successful_write_operations = self.successful_write_operations.load(Ordering::Relaxed);
        let whole_body_dedup_hits = self.whole_body_dedup_hits.load(Ordering::Relaxed);
        let whole_body_deduplicated_bytes =
            self.whole_body_deduplicated_bytes.load(Ordering::Relaxed);
        let committed_body_bytes = self.committed_body_bytes.load(Ordering::Relaxed);
        let read_latency = self.read_latency.snapshot();
        let reconstructed_bytes = self.reconstructed_bytes.load(Ordering::Relaxed);
        let reconstruction_bytes_per_second = if read_latency.total_ns == 0 {
            0.0
        } else {
            reconstructed_bytes as f64 * 1_000_000_000.0 / read_latency.total_ns as f64
        };
        LarRuntimeMetricsSnapshot {
            since_ms: self.since_ms,
            writes: LarWriteMetricsSnapshot {
                operations: write_operations,
                successful_operations: successful_write_operations,
                failures: self.write_failures.load(Ordering::Relaxed),
                attempted_body_bytes: self.attempted_body_bytes.load(Ordering::Relaxed),
                committed_body_bytes,
                whole_body_dedup_hits,
                whole_body_deduplicated_bytes,
                whole_body_dedup_ratio: if committed_body_bytes == 0 {
                    0.0
                } else {
                    whole_body_deduplicated_bytes as f64 / committed_body_bytes as f64
                },
                chunk_references,
                candidate_chunk_bytes,
                planned_new_chunks,
                new_chunk_bytes: self.new_chunk_bytes.load(Ordering::Relaxed),
                new_compressed_bytes: self.new_compressed_bytes.load(Ordering::Relaxed),
                chunk_dedup_ratio: if candidate_chunk_bytes == 0 {
                    0.0
                } else {
                    candidate_chunk_bytes
                        .saturating_sub(self.new_chunk_bytes.load(Ordering::Relaxed))
                        as f64
                        / candidate_chunk_bytes as f64
                },
                chunker_latency: self.chunker_latency.snapshot(),
                hash_latency: self.hash_latency.snapshot(),
                compression_latency: self.compression_latency.snapshot(),
                append_latency: self.append_latency.snapshot(),
                flush_latency: self.flush_latency.snapshot(),
                sqlite_commit_latency: self.sqlite_commit_latency.snapshot(),
            },
            reads: LarReadMetricsSnapshot {
                operations: self.read_operations.load(Ordering::Relaxed),
                failures: self.read_failures.load(Ordering::Relaxed),
                reconstructed_bytes,
                time_to_first_byte: self.read_ttft.snapshot(),
                reconstruction_latency: read_latency,
                reconstruction_bytes_per_second,
            },
        }
    }

    pub(crate) fn health_snapshot(&self) -> LarHealthMetricsSnapshot {
        let write_failures = self.write_failures.load(Ordering::Relaxed);
        let read_failures = self.read_failures.load(Ordering::Relaxed);
        LarHealthMetricsSnapshot {
            state: if write_failures == 0 && read_failures == 0 {
                "ok"
            } else {
                "degraded"
            },
            since_ms: self.since_ms,
            write_failures,
            read_failures,
            last_write_failure_ms: nonzero(self.last_write_failure_ms.load(Ordering::Relaxed)),
            last_read_failure_ms: nonzero(self.last_read_failure_ms.load(Ordering::Relaxed)),
        }
    }
}

pub(crate) struct ObservedWriter<'a, W> {
    inner: &'a mut W,
    started: Instant,
    first_write: Option<Duration>,
    bytes: u64,
}

impl<'a, W> ObservedWriter<'a, W> {
    pub(crate) fn new(inner: &'a mut W) -> Self {
        Self {
            inner,
            started: Instant::now(),
            first_write: None,
            bytes: 0,
        }
    }

    pub(crate) fn observation(&self) -> (u64, Option<Duration>, Duration) {
        (self.bytes, self.first_write, self.started.elapsed())
    }
}

impl<W: Write> Write for ObservedWriter<'_, W> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let written = self.inner.write(buffer)?;
        if written > 0 {
            if self.first_write.is_none() {
                self.first_write = Some(self.started.elapsed());
            }
            self.bytes = self.bytes.saturating_add(written as u64);
        }
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

fn duration_ns(duration: Duration) -> u64 {
    duration.as_nanos().try_into().unwrap_or(u64::MAX)
}

fn unix_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn nonzero(value: u64) -> Option<u64> {
    (value != 0).then_some(value)
}

fn histogram_index(elapsed_ns: u64) -> usize {
    let microseconds = elapsed_ns.saturating_add(999) / 1_000;
    let index = if microseconds <= 1 {
        0
    } else {
        (u64::BITS - (microseconds - 1).leading_zeros()) as usize
    };
    index.min(HISTOGRAM_BUCKETS - 1)
}

fn histogram_upper_bound(index: usize) -> u64 {
    if index == HISTOGRAM_BUCKETS - 1 {
        u64::MAX
    } else {
        1_000_u64.checked_shl(index as u32).unwrap_or(u64::MAX)
    }
}

fn histogram_quantile(histogram: &[LarHistogramBucket], samples: u64, percentile: u64) -> u64 {
    if samples == 0 {
        return 0;
    }
    let target = samples.saturating_mul(percentile).saturating_add(99) / 100;
    let mut cumulative = 0_u64;
    for bucket in histogram {
        cumulative = cumulative.saturating_add(bucket.count);
        if cumulative >= target {
            return bucket.le_ns;
        }
    }
    u64::MAX
}

fn saturating_add(value: &AtomicU64, addition: u64) {
    let _ = value.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
        Some(current.saturating_add(addition))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_histogram_reports_bounded_quantiles() {
        let metric = LatencyMetric::default();
        for elapsed in [1_000, 2_000, 10_000] {
            metric.record(Duration::from_nanos(elapsed));
        }
        let snapshot = metric.snapshot();
        assert_eq!(snapshot.samples, 3);
        assert_eq!(snapshot.total_ns, 13_000);
        assert_eq!(snapshot.average_ns, 4_333);
        assert_eq!(snapshot.max_ns, 10_000);
        assert_eq!(snapshot.p50_upper_bound_ns, 2_000);
        assert_eq!(snapshot.p95_upper_bound_ns, 16_000);
        assert_eq!(snapshot.p99_upper_bound_ns, 16_000);
        assert_eq!(snapshot.histogram.len(), HISTOGRAM_BUCKETS);
        assert_eq!(
            snapshot
                .histogram
                .iter()
                .map(|bucket| bucket.count)
                .sum::<u64>(),
            3
        );
    }

    #[test]
    fn byte_weighted_dedup_and_attempted_writes_are_distinct() {
        let metrics = LarRuntimeMetrics::default();
        metrics.record_write(1_000, false);
        metrics.record_write(500, true);
        metrics.record_whole_body_dedup_hit(1_000);
        metrics.record_chunk_candidates(4, 2_000);
        metrics.record_new_chunk_plan(1);
        metrics.record_new_chunk_bytes(500, 125);
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.writes.operations, 2);
        assert_eq!(snapshot.writes.successful_operations, 1);
        assert_eq!(snapshot.writes.failures, 1);
        assert_eq!(snapshot.writes.attempted_body_bytes, 1_500);
        assert_eq!(snapshot.writes.committed_body_bytes, 1_000);
        assert_eq!(snapshot.writes.whole_body_deduplicated_bytes, 1_000);
        assert_eq!(snapshot.writes.whole_body_dedup_ratio, 1.0);
        assert_eq!(snapshot.writes.chunk_dedup_ratio, 0.75);
    }
}
