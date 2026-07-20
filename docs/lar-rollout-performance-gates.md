# LAR rollout performance gates

This document covers the remaining local performance measurements in the LAR
rollout matrix. The harnesses are ignored integration tests, so normal test
runs do not spend minutes doing proxy traffic, synchronized writes, throttled
migration, or corpus packing. Run the real proxy gate with:

```sh
cargo test -p alex-proxy --test lar_rollout_benchmark --release \
  -- --ignored --nocapture --test-threads=1
```

Run the Store-level supporting gates with:

```sh
cargo test -p alex-store --test lar_rollout_benchmark --release \
  -- --ignored --nocapture --test-threads=1
```

Run on otherwise idle rollout hardware. `--release` and
`--test-threads=1` are part of the protocol, not optional tuning. Each result is
one line beginning with `ALEX_LAR_BENCHMARK` followed by JSON using schema
`alex-lar-rollout-benchmark-v1`.

The checked-in defaults are reproducible development workloads. They do not
encode rollout thresholds and an unset threshold is reported as
`"status":"unconfigured"`, never as a pass. Operators must set thresholds
agreed for the target Mac before using the command as a release gate.

## Concurrent proxy throughput and added request latency

The `alex-proxy` gate sends real loopback HTTP requests through Alexandria's
public OpenAI chat route to a deterministic local Exo-compatible upstream:

```sh
cargo test -p alex-proxy --test lar_rollout_benchmark --release \
  concurrent_proxy_throughput_and_added_request_latency -- \
  --ignored --nocapture --test-threads=1
```

Four clients each send 24 requests with a 32 KiB conversation prefix by
default. The timer covers client HTTP, Alexandria parsing/routing, an upstream
HTTP request and response, full response consumption, and the awaited trace
storage job. The harness first runs with the legacy body store and repeats the
same workload with `LarWithFallback`. That production mode retains the legacy
rollback gzip while making a validated LAR artifact the preferred read. After
the timer, it requires every request plus the warm-up to have a trace row and
requires every LAR-mode request trace to resolve its client body from LAR. A
silent legacy fallback
therefore fails the benchmark.

The deterministic loopback upstream removes internet/provider variance; it
does not imitate real model generation time. This isolates Alexandria's added
latency while retaining the real router, network stacks, trace finalization,
storage-worker semaphore, compression, sync, readback validation, and SQLite
publication paths.

Workload overrides:

- `ALEX_LAR_BENCH_PROXY_WORKERS`
- `ALEX_LAR_BENCH_PROXY_TURNS_PER_WORKER`
- `ALEX_LAR_BENCH_PROXY_PREFIX_BYTES`

Optional release gates:

- `ALEX_LAR_BENCH_MIN_PROXY_OPS_PER_SECOND`
- `ALEX_LAR_BENCH_MAX_PROXY_ADDED_P50_MS`
- `ALEX_LAR_BENCH_MAX_PROXY_ADDED_P95_MS`
- `ALEX_LAR_BENCH_MAX_PROXY_ADDED_P99_MS`

## Long-session Trace Browser backend

Run the production-route backend measurement with:

```sh
cargo test -p alex-proxy --test lar_rollout_benchmark --release \
  long_session_trace_browser_http_paging_and_cancellation -- \
  --ignored --nocapture --test-threads=1
```

The default fixture writes 1,500 provider-shaped turns with 32 KiB repeated
request prefixes through `LarWithFallback`, forces both sealed and active body
packs, verifies every request and response has a validated LAR pointer, and
then removes every legacy gzip fallback. Timed reads therefore fail rather
than silently leaving the production LAR path. Every transcript request uses
the public authenticated HTTP route and a 50-turn page.

Forty samples measure initial tail paging, an older cursor page, and the full
search-to-anchored-page navigation. Each sample also starts eight concurrent
long-session pages, cancels their client tasks, then measures navigation to a
different session and the public health route. Responses must retain the
requested session/cursor, stay within the transcript body-byte budget, contain
no body errors or truncations, and leave the daemon healthy after cancellation.
The report distinguishes tasks cancelled by the abort from responses that
completed before the abort, avoiding a timing-dependent failure on fast hosts.

Workload overrides:

- `ALEX_LAR_BENCH_BROWSER_TURNS`
- `ALEX_LAR_BENCH_BROWSER_SAMPLES`
- `ALEX_LAR_BENCH_BROWSER_PREFIX_BYTES`
- `ALEX_LAR_BENCH_BROWSER_CANCEL_REQUESTS`

Optional release gates:

- `ALEX_LAR_BENCH_MAX_BROWSER_TAIL_P99_MS`
- `ALEX_LAR_BENCH_MAX_BROWSER_OLDER_P99_MS`
- `ALEX_LAR_BENCH_MAX_BROWSER_SEARCH_ANCHOR_P99_MS`
- `ALEX_LAR_BENCH_MAX_BROWSER_POST_CANCEL_NAVIGATION_P99_MS`
- `ALEX_LAR_BENCH_MAX_BROWSER_POST_CANCEL_HEALTH_P99_MS`

This is deliberately a backend gate. It includes loopback HTTP, authentication,
SQLite cursor/search work, catalog resolution, LAR reads, decompression, JSON
translation, response transfer, and client JSON decoding. It does not measure
AppKit/SwiftUI rendering, main-actor scheduling, window navigation, or the
visual stability of loading and daemon indicators. Client cancellation may
leave already-started blocking reads to finish; the post-cancellation samples
measure the resulting service responsiveness. Consequently this benchmark
does not complete the end-to-end macOS Trace Browser checklist item.

Linux development-host release baseline (2026-07-20), with thresholds
unconfigured: 1,500 turns, 40 samples, 47 sealed packs plus one active pack,
and 320/320 cancellation tasks aborted. Tail-page p99 was 138.83 ms, older-page
p99 95.74 ms, search-plus-anchor p99 60.53 ms, post-cancellation navigation p99
97.62 ms, and health p99 0.22 ms. Keep the rollout gates unset until the full
workload has been repeated and accepted on the agreed Mac hardware; this Linux
measurement is reproducibility evidence only.

## Concurrent storage-path precursor

Run only this measurement with:

```sh
cargo test -p alex-store --test lar_rollout_benchmark --release \
  concurrent_storage_write_throughput_and_added_latency -- \
  --ignored --nocapture --test-threads=1
```

Four workers each submit 48 deterministic 68 KiB agent-like request bodies by
default. Every body has the same 64 KiB conversation prefix and a deterministic
4 KiB per-turn tail. Body construction is outside the timer. The harness first
measures the legacy gzip path, then repeats the same workload through
`DualWriteValidated`, which includes legacy rollback capture, LAR append and
sync, catalog publication, and direct readback validation. A LAR fallback makes
the sample fail rather than silently counting a legacy-only request.

`added_latency_ms` is the signed dual-write percentile minus the corresponding
legacy percentile. A negative value is retained in the report because it
describes measurement noise; maximum gates compare its non-negative storage
penalty. This is the persistence contribution to request latency at the Store
boundary, not end-to-end HTTP/router latency. It is a repeatable precursor for
the rollout experiment, but it does **not** satisfy the required “representative
concurrent proxy traffic” or end-to-end added-request-latency measurements.
`DualWriteValidated` is the beta overhead path: it writes and validates LAR
objects but intentionally does not publish the LAR-preferred trace-artifact
pointer. The real proxy gate above separately measures the LAR-preferred
`LarWithFallback` path and verifies those pointers.

Workload overrides:

- `ALEX_LAR_BENCH_WRITE_WORKERS`
- `ALEX_LAR_BENCH_WRITE_TURNS_PER_WORKER`
- `ALEX_LAR_BENCH_WRITE_PREFIX_BYTES`
- `ALEX_LAR_BENCH_WRITE_TAIL_BYTES`

Optional release gates:

- `ALEX_LAR_BENCH_MIN_WRITE_OPS_PER_SECOND`
- `ALEX_LAR_BENCH_MAX_ADDED_P50_MS`
- `ALEX_LAR_BENCH_MAX_ADDED_P95_MS`
- `ALEX_LAR_BENCH_MAX_ADDED_P99_MS`

For example, after the Mac hardware owner has approved values:

```sh
ALEX_LAR_BENCH_MIN_WRITE_OPS_PER_SECOND=AGREED_VALUE \
ALEX_LAR_BENCH_MAX_ADDED_P50_MS=AGREED_VALUE \
ALEX_LAR_BENCH_MAX_ADDED_P95_MS=AGREED_VALUE \
ALEX_LAR_BENCH_MAX_ADDED_P99_MS=AGREED_VALUE \
cargo test -p alex-store --test lar_rollout_benchmark --release \
  concurrent_storage_write_throughput_and_added_latency -- \
  --ignored --nocapture --test-threads=1
```

Replace every `AGREED_VALUE`; the literal placeholder is intentionally invalid.

## Throttled migration and concurrent interactive reads

Run only this measurement with:

```sh
cargo test -p alex-store --test lar_rollout_benchmark --release \
  throttled_migration_throughput_and_interactive_read_latency -- \
  --ignored --nocapture --test-threads=1
```

The default fixture contains 96 deterministic, incompressible 64 KiB legacy
gzip bodies. Incompressibility matters because the production I/O controller
throttles source-file reads: a repeated-text fixture would compress to almost
nothing and would not exercise the configured 4 MiB/s limit. Migration uses two
preparation workers, a 50% CPU budget, a 16 MiB memory budget, a yield after
every artifact, and the real mixed LAR/legacy reader concurrently samples one
body every 5 ms. The report includes wall-clock logical MiB/s, importer-reported
source throughput, throttle/yield counters, pack rotations, and read
p50/p95/p99. The test fails if any body disappears or migration finishes before
one concurrent read is observed.

Workload overrides:

- `ALEX_LAR_BENCH_MIGRATION_ARTIFACTS`
- `ALEX_LAR_BENCH_MIGRATION_BODY_BYTES`
- `ALEX_LAR_BENCH_MIGRATION_IO_BYTES_PER_SECOND`
- `ALEX_LAR_BENCH_MIGRATION_READ_INTERVAL_MS`

Optional release gates:

- `ALEX_LAR_BENCH_MIN_MIGRATION_MIB_PER_SECOND`
- `ALEX_LAR_BENCH_MAX_MIGRATION_READ_P50_MS`
- `ALEX_LAR_BENCH_MAX_MIGRATION_READ_P95_MS`
- `ALEX_LAR_BENCH_MAX_MIGRATION_READ_P99_MS`

## Synthetic 14-day corpus, rotation, and repack RSS

Run the two isolated memory scenarios with:

```sh
cargo test -p alex-store --test lar_rollout_benchmark --release \
  synthetic_14_day_corpus_and_rotation_repack_peak_rss -- \
  --ignored --nocapture --test-threads=1
```

The parent test starts a fresh child process for each scenario so one workload's
peak cannot contaminate the other. Peak RSS comes from
`getrusage(RUSAGE_SELF).ru_maxrss`; the harness normalizes macOS bytes and Linux
KiB to MiB. It reports the absolute child-process peak, including the Rust test
harness, SQLite, codecs, Store, and workload. That is more conservative and
more portable than subtracting two noisy point-in-time RSS samples.

The quick corpus profile creates 14 days with 100 traces/day (1,400 artifacts),
a stable 15.5 KiB prefix, and a deterministic 512-byte tail. It then imports
the entire corpus with an 8 MiB importer budget and 4 MiB pack rotation. This
profile is useful for regression work but is not the measured 55k-exchange
rollout corpus. Run the full-count shape explicitly:

```sh
ALEX_LAR_BENCH_TRACES_PER_DAY=3929 \
cargo test -p alex-store --test lar_rollout_benchmark --release \
  synthetic_14_day_corpus_and_rotation_repack_peak_rss -- \
  --ignored --nocapture --test-threads=1
```

That produces 55,006 artifacts. `ALEX_LAR_BENCH_CORPUS_BODY_BYTES` changes the
logical body size. It does not claim to reproduce the content distribution of
the anonymized real Mac corpus; that corpus remains required for a rollout
decision.

The rotation/repack child writes 160 unique 32 KiB bodies through the live
store with a 384 KiB pack ceiling, checks that rotation occurred, removes half
the trace roots, runs production GC accounting, and repacks every eligible
sealed pack. It reports file count after rotation, candidate/repack counts,
logical bytes reclaimed, and peak RSS. Overrides are:

- `ALEX_LAR_BENCH_REPACK_ARTIFACTS`
- `ALEX_LAR_BENCH_REPACK_BODY_BYTES`
- `ALEX_LAR_BENCH_REPACK_MAX_PACK_BYTES`

Optional memory gates are:

- `ALEX_LAR_BENCH_MAX_CORPUS_PEAK_RSS_MIB`
- `ALEX_LAR_BENCH_MAX_ROTATION_REPACK_PEAK_RSS_MIB`

## Interpretation and limitations

- Percentiles use the deterministic floor index `(n - 1) * p / 100`, matching
  the existing LAR benchmarks. Report the sample count with every result.
- The write A/B runs are consecutive, not randomized, and include filesystem
  cache and scheduler noise. Repeat them on an idle machine before adopting a
  threshold.
- The proxy gate uses two loopback HTTP hops and a deterministic immediate
  upstream response. It measures Alexandria overhead under concurrency, not
  internet latency or token-generation throughput.
- The migration read loop is intentionally rate-limited instead of turning the
  benchmark into a read-saturation test. It measures interactive coexistence,
  not maximum read QPS.
- `ru_maxrss` is a process-lifetime high-water mark. Child isolation makes the
  scenarios comparable, but allocator behavior and the Rust test harness are
  still part of the number.
- Synthetic inputs prove reproducibility and expose regressions; they cannot
  replace the anonymized real 14-day corpus or the agreed Mac hardware profile.
- These gates report measurements and enforce only operator-provided values.
  They do not, by themselves, complete any `lar-format.md` rollout checklist
  item.

## Recorded development run

No rollout threshold is inferred from a development host. The following run is
a harness validation and baseline observation, not rollout evidence.

- Time: 2026-07-20 19:23 AEST
- OS: Linux 7.0.10-arch1-1, x86-64
- CPU: AMD Ryzen 7 8745HS, 8 cores/8 logical CPUs exposed
- Filesystem: btrfs, 84% used during the run
- Toolchain: rustc/cargo 1.95.0
- Git base: `1e47fa4`, with the shared uncommitted LAR implementation worktree
- Build: release, one test thread
- Threshold environment variables: all unset (`unconfigured`)

The three focused commands above produced:

| Measurement | Development result |
| --- | ---: |
| concurrent `LarWithFallback` proxy throughput | 96.84 ops/s; 3.039 logical request MiB/s |
| legacy end-to-end proxy latency p50/p95/p99 | 41.839 / 42.916 / 43.059 ms |
| `LarWithFallback` proxy latency p50/p95/p99 | 41.015 / 42.991 / 44.194 ms |
| signed added proxy latency p50/p95/p99 | -0.825 / 0.075 / 1.134 ms |
| concurrent storage dual-write throughput | 1,304.87 ops/s; 86.77 logical MiB/s |
| legacy request latency p50/p95/p99 | 0.254 / 0.372 / 0.650 ms |
| dual-write request latency p50/p95/p99 | 2.058 / 7.029 / 13.982 ms |
| added request latency p50/p95/p99 | 1.805 / 6.657 / 13.332 ms |
| throttled migration wall throughput | 1.947 logical MiB/s |
| concurrent migration-read latency p50/p95/p99 | 0.193 / 8.622 / 9.581 ms (425 samples) |
| importer throttle/yield accounting | 2,050 ms throttled; 96 yields |
| quick 14-day corpus peak RSS | 15.086 MiB (1,400 artifacts; 21.875 logical MiB) |
| full 14-day corpus peak RSS | 28.730 MiB (55,006 artifacts; 859.469 logical MiB) |
| full 14-day corpus import elapsed/rotations | 2,450.698 s (40m50.7s); 9 rotations; 0 failures |
| rotation/repack peak RSS | 12.113 MiB (15 packs; 14 candidates repacked) |

The quick corpus run did not cross its 4 MiB compressed pack ceiling because
the stable-prefix fixture deduplicated/compressed below it. Rotation was
measured separately: the second child forced 15 live packs, found 1,057
unreachable chunks after retention, repacked all 14 eligible sealed packs, and
reported 2,748,004 logical bytes reclaimed.

The full synthetic 55,006-artifact profile completed, but its stable-prefix
content distribution is not the anonymized real corpus, and the agreed Mac
hardware profile was not run. No rollout threshold was configured. Consequently
the memory and latency values above must not be used to check the performance
items in `lar-format.md`.
