# LAR benchmark

The checked-in corpus generator models the shapes that matter for LAR without
including captured user data: a 77-turn growing conversation, exact
client/upstream copies, compaction, retries, SSE, repeated headers, tool
arguments/results, and optional missing/corrupt gzip sources. Tool output uses
deterministic high-entropy paths and hashes; trivially repeated text would make
each legacy gzip file unrealistically small and hide cross-turn duplication.

Build the real CLI once, then run both profiles:

```sh
cargo build -p alex
python3 scripts/benchmark-lar-corpus.py --profile standard
python3 scripts/benchmark-lar-corpus.py --profile tool-heavy --require-ratio 10
```

The script retains its temporary directory and reports it as `output`. It runs
the shared production importer and normal archive verifier, counts only body
files referenced by SQLite, and compares their complete gzip size with all LAR
body packs. `--output` selects an inspectable directory and `--json-out` saves
the report.

### Privacy-safe real-corpus measurement

`measure-lar-corpus.py` opens SQLite and every referenced body read-only. Its
report contains aggregate counts, sizes, distributions, and ratios only: it
does not emit prompts, tool output, header names or values, filesystem paths,
trace/session IDs, or body digests. The tool measures artifact kinds, exact
whole-body duplication, exact client/upstream matches, consecutive
same-session prefix duplication, the production adaptive Gear chunk-size
distribution, header block/atom repetition, session turn counts, and session
durations.

Whole-body hashing and consecutive-prefix measurement cover every valid
artifact. To keep multi-week runs practical, CDC distribution defaults to 64
evenly spread artifacts and 32 MiB of decompressed input per artifact kind;
the report records exact coverage. Pass both
`--chunk-sample-artifacts-per-kind 0 --chunk-sample-bytes-per-kind 0` for an
unbounded full CDC distribution pass.

On the Mac that owns the capture, measure the known 15-hour session with:

```sh
python3 scripts/measure-lar-corpus.py \
  ~/.alexandria/alexandria.sqlite3 \
  --session-id 019f6872-a3ee-7431-b4bb-2bafbabb7235 \
  --json-out /tmp/lar-session-measurement.json \
  --shape-out /tmp/lar-session-shape.json
```

If `data_dir` points elsewhere, pass its `alexandria.sqlite3`; relative body
paths resolve from that directory unless `--body-root` overrides it. Omit
`--session-id` for the full corpus. Progress goes to stderr, and JSON goes to
stdout plus the optional output file.

The shape file is also aggregate-only. It can produce a deterministic fake
session using the measured p50, p95, p99, or maximum turn count, duration,
tool-result size, and response-size pressure:

```sh
python3 scripts/generate-lar-corpus.py /tmp/lar-shaped-corpus \
  --shape-profile /tmp/lar-session-shape.json \
  --shape-quantile max

python3 scripts/benchmark-lar-corpus.py \
  --shape-profile /tmp/lar-session-shape.json \
  --shape-quantile max
```

The benchmark command generates the shaped corpus, imports it through the
production migration path, verifies every resulting archive, and reports its
actual legacy-gzip/LAR ratio. Explicit `--turns` still overrides the aggregate
turn count when a controlled comparison is needed. The real-corpus rollout
gate stays unchecked until the aggregate report has actually been captured and
reviewed on the Mac; the existence of this read-only profiler is not treated as
measurement evidence.

### Chunker and compression design gate

The corpus directory can also be measured without writing an archive:

```sh
cargo run -p alex-lar --release --example design_gate -- \
  --corpus /path/from/benchmark-output \
  --dictionary-corpus /path/from/a-different-seed-or-period --json
```

This reads and length-checks every `valid` gzip artifact named by the generated
or anonymized `corpus-manifest.json`, then compares five deterministic
strategies over the identical body sequence:

- fixed 2 KiB chunks (control);
- the fine 0.5/2/8 KiB streaming Gear CDC;
- the large-body 2/8/32 KiB streaming Gear CDC;
- a normalized two-mask FastCDC-style prototype;
- a 64-byte rolling Buzhash prototype.

For each strategy it reports logical bytes, references, unique chunks and
bytes, chunking time, independently compressed unique-chunk bytes at zstd
levels 1/3/7, and level 3 with a corpus-trained 32 KiB dictionary (including
the dictionary itself). Dictionary results are omitted unless a distinct
training corpus is supplied; this prevents a same-corpus dictionary from
memorizing the bytes under test. Compression results deliberately exclude LAR framing
and manifest metadata, so the normal importer benchmark remains authoritative
for final on-disk size. Run the release command repeatedly on the same idle
machine and retain its JSON output before changing the stable chunking
algorithm or dictionary policy.

## 2026-07-20 design-gate measurements

Linux development build, 77 turns, zstd level 3, Gear CDC:

| Profile / CDC bounds | Source bytes | Unique bytes | Legacy gzip | LAR | gzip/LAR |
| --- | ---: | ---: | ---: | ---: | ---: |
| Repetitive placeholder tool text, 8/32/128 KiB | 1,873,391 | 1,101,465 | 101,911 | 95,742 | 1.06x |
| Realistic 256-line tools, 8/32/128 KiB | 9,255,407 | 2,773,040 | 1,648,683 | 791,988 | 2.08x |
| Realistic 256-line tools, 0.5/2/8 KiB | 9,255,407 | 653,572 | 1,648,683 | 464,701 | 3.55x |
| Tool-heavy 1,024-line tools, 0.5/2/8 KiB | 35,494,895 | 2,014,829 | 16,493,781 | 1,464,307 | 11.26x |
| Realistic 256-line tools, 0.5/2/8 KiB + predecessor ranges | 9,255,407 | 503,344 | 1,648,683 | 486,975 | 3.39x |
| Tool-heavy 1,024-line tools, 0.5/2/8 KiB + predecessor ranges | 35,494,895 | 1,812,101 | 16,493,781 | 1,483,245 | 11.12x |
| Realistic 256-line tools, fine CDC + ranges + zstd manifest pages | 9,255,407 | 503,344 | 1,648,683 | 299,776 | **5.50x** |
| Tool-heavy 1,024-line tools, fine CDC + ranges + zstd manifest pages | 35,494,895 | 1,812,101 | 16,493,781 | 958,890 | **17.20x** |

This proves two different things. Fine-grained CDC removes 92.9–94.3% of
uncompressed repeated-prefix bytes and crosses the 10x target for the
tool-heavy agent case. It does not yet cross the 5x representative-corpus
target: many small independently compressed chunk frames and manifest records
lose compression context, while each legacy gzip body compresses its own
internal repetition well.

The initial design gate therefore remained open for metadata/page batching, a
static JSON/HTTP zstd dictionary, and compact predecessor-range references.
The subsequent page-compressed rows use the same production importer and
corpus with independently decompressible zstd manifest pages. Page compression
alone takes the representative profile past 5x and the tool-heavy profile well
past 10x; no dictionary was required for those two gates. A static dictionary
is still an optional measured optimization, not a prerequisite or hidden
external dependency. Live LAR writes must not become the default based only on
this synthetic benchmark. All future measurements must report both logical
unique bytes and actual bytes on disk.

The predecessor-range rows use the production legacy importer and normal
`ArchiveReader` verification. Requests only reuse a prior manifest from the
same session and artifact kind; an upstream request may additionally reuse the
client request from that exact exchange. The matcher stores only literal
ranges and direct slices of already stored chunks, so manifests never form a
recursive dependency chain. Exact client/upstream copies collapse to one body
identity (231 artifacts produced 169 manifests).

Range matching reduced unique uncompressed bytes by another 150,228 bytes on
the standard corpus and 202,728 bytes on the tool-heavy corpus. It did not
reduce the file size: the growing direct-reference lists add enough per-record
metadata to offset those literal savings in the unpacked-manifest measurement.
This closes the CDC-only correctness gap for shifted prefixes and confirms why
page-compressed manifest references are necessary: with pages enabled, the
same logical references cross the 5x standard on-disk target. The range
implementation is bounded to one 64 MiB predecessor
and current body, 262,144 sampled candidates, four byte-verified candidates per
hash, 262,144 segments, and a 256 MiB deterministic work budget; oversized
inputs fall back to streaming CDC.

The page-compressed validation runs completed on 2026-07-20 with zero import
failures and normal-reader reconstruction of all 169 unique manifests. Standard
import/verify took 1.31s/0.14s; tool-heavy took 3.53s/0.26s in an unoptimized
Linux development build. The next mandatory measurement is the anonymized real
Mac corpus and the 15-hour-session shape; synthetic ratios are not treated as a
production rollout decision by themselves.

### Chunker/zstd holdout result

The design-gate example was run in release mode on the 77-turn standard corpus,
with seed `20260721` used only to train the dictionary and the default seed used
as the 9,255,407-byte holdout. Times are one local run and are directional, not
a latency gate:

| Strategy | Unique bytes | Removed | Chunk time | zstd 1 | zstd 3 | zstd 7 | zstd 3 + held-out 32 KiB dictionary |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Fixed 2 KiB | 417,486 | 95.49% | 4.53 ms | 189,729 | 188,139 | 187,672 | 185,376 |
| Gear 0.5/2/8 KiB | 653,572 | 92.94% | 17.93 ms | 290,861 | 293,236 | 291,569 | 281,172 |
| Gear 2/8/32 KiB | 1,144,482 | 87.63% | 16.74 ms | 472,420 | 491,368 | 497,207 | 473,541 |
| normalized FastCDC prototype | 672,747 | 92.73% | 18.79 ms | 299,185 | 301,297 | 299,742 | 296,399 |
| rolling Buzhash prototype | 486,211 | 94.75% | 20.97 ms | 210,239 | 214,694 | 213,429 | 214,662 |

Fixed-size chunks win this deterministic generator because its JSON prefixes
mostly remain byte-aligned; that control cannot tolerate arbitrary insertions
as CDC does. Buzhash improves unique bytes but was slower here. The normalized
FastCDC prototype did not improve size or time over the current Gear chunker.
The held-out dictionary saved only 1.5–4.1% for fixed/Gear/FastCDC after paying
for its bytes and did not help Buzhash; zstd levels above 1 also had negligible
payload benefit. LAR v1 therefore retains fine Gear boundaries below 8 MiB,
zstd level 3 for conservative interoperability, and no mandatory static body
dictionary. The wider Gear row removes less repetition, so it is selected only
for large bodies where reconstruction measurements justify it. This is a
reversible implementation choice—not a container-format constraint—and must be
revisited with the anonymized real Mac corpus before default rollout.

## Large-body reconstruction throughput

The ignored release benchmark can be rerun with:

```sh
cargo test -p alex-lar --test reconstruction_benchmark --release -- --ignored --nocapture
```

The documented local warm-cache target is 500 MiB/s for a 64 MiB
incompressible body, including independent zstd decompression, manifest-range
assembly, whole-body BLAKE3 verification, and writes to a sink. The original
0.5/2/8 KiB profile produced 303.5 MiB/s p50 and 297.8 MiB/s p95 because it
required thousands of tiny frames. The measured 2/8/32 KiB profile produced
727.4 MiB/s p50 and 724.9 MiB/s p95; its archive was 68,331,878 bytes.

Production therefore selects 2/8/32 KiB only when a known body is at least
8 MiB and the caller retained the default profile. Smaller bodies keep
0.5/2/8 KiB boundaries, explicit configurations are never rewritten, and the
reader limit is validated against the largest profile at writer creation.
Streaming sources without a known final length conservatively retain the fine
profile. This passes the local throughput gate without paying the wider
profile's 75% unique-byte increase on the 77-turn holdout for ordinary bodies.

## Fresh active-pack write path

The ignored integration benchmark can be rerun with:

```sh
cargo test -p alex-store --test live_lar_benchmark -- --ignored --nocapture
```

In an unoptimized Linux build, 500 sequential growing-prefix writes processed
36,650,750 logical body bytes into a 107,092-byte active LAR pack. The complete
compatibility path—including the legacy gzip rollback write, sync, direct
catalog-offset readback verification, and SQLite publication—measured 3.28 ms
p50, 3.67 ms p95, and 4.48 ms p99. The retained rollback gzip files occupied
382,069 bytes and the closed SQLite catalog 6,344,704 bytes.

This benchmark is evidence for the fresh-log hot path, not the rollout latency
gate: it is sequential, uses synthetic repeated bytes, and does not include the
full proxy under concurrent traffic. Its important regression property is that
each publication is flushed and synced while complete checkpoint indexes are
emitted only at the 8 MiB/30-second cadence. Newly written chunks are read and
verified directly from their cataloged frame offsets, so immediate reads do not
require a checkpoint or a full active-pack scan.

## Sealed random-access path

Run the ignored release benchmark with:

```sh
cargo test -p alex-lar --test index_benchmark --release -- --ignored --nocapture
```

On this Linux workspace, a sealed archive containing 2,000 manifests opened a
fresh file descriptor through its footer and produced the first byte of a
selected 1 KiB body in 6.53 ms. Across 100 deterministic random-manifest
filesystem opens, open+lookup+first decompressed byte measured 6.42 ms p50,
6.65 ms p95, and 7.78 ms p99. Twenty random samples after Linux
`posix_fadvise(POSIX_FADV_DONTNEED)` measured 6.16 ms p50 and 8.42 ms p95/p99.
The equivalent first forward-scan TTFT took 12.03 ms. The ignored benchmark
asserts the sealed warm p99 remains below 10 ms.

The custom sink timestamps the first non-empty write, so the numbers include
opening and validating the footer/index, random manifest lookup, locating the
chunk, and decompression to first output rather than only index parsing. The
"cold" samples are an OS cache-drop advisory, not proof that hardware caches
were empty; the agreed Mac hardware profile therefore remains rollout
evidence.

## Native framing versus an MCAP profile

The checked-in comparison maps the same content-addressed workload into native
LAR and a valid MCAP file:

```sh
cargo run -p alex-lar --release --example container_gate -- --turns 77
```

The MCAP profile stores each independently zstd-compressed unique chunk once as
an indexed attachment and each compact binary manifest as a timestamped,
indexed message. New attachments precede the first manifest that references
them. Both readers reconstruct and whole-body-hash-check the selected body.

One release run over 18,938,807 logical body bytes and 284 unique chunks
measured native LAR at 379,886 bytes versus 723,145 bytes for MCAP. Median
in-memory indexed reconstruction of the final body was 2.13 ms for LAR and
1.66 ms for MCAP. At an identical 80% byte truncation, LAR exposed 61 complete
manifests and MCAP exposed 69 manifest messages plus 249 attachments. Recovery
counts are structural smoke evidence; LAR's exhaustive prefix-truncation suite
remains the conformance proof.

The MCAP result is respectable, but its attachment names, manifest schema,
content validation, reverse references, and hash index are all LAR-specific;
standard MCAP indexes are time/channel-oriented. Native LAR is 47% smaller in
this workload and directly indexes the identities Alex reads. The v1 decision
and trade-offs are recorded in
[ADR 0002](adr/0002-lar-native-container.md).

## Portable raw-search filter prototype

The page-filter gate can be reproduced with:

```sh
cargo run -p alex-lar --release --example search_filter_gate -- --corpus CORPUS
```

It chunks each valid corpus artifact through the production Gear chunker,
deduplicates the chunks, groups unique bytes into 256 KiB search pages, and
compares an exact trigram set with a 10-bit-per-distinct-trigram Bloom filter
using seven hashes. Every candidate is verified against raw bytes, so neither
filter is authoritative and both are tested for false negatives.

On the 77-turn deterministic corpus, 653,572 unique chunk bytes formed three
pages. Across 128 present 12-byte literals, both filters selected exactly the
375 pages containing a match. Across 128 generated absent literals, neither
selected a page. The Bloom payload occupied 20,486 bytes (3.13% of unique
chunk bytes); a packed exact-trigram bitmap has a 61,677-byte (9.44%) lower
bound before keys and container overhead. Three pages are too few to claim a
production false-positive rate, but the result establishes the size envelope,
no-false-negative invariant, and measurement harness.

The v1 decision is to keep portable Bloom filters optional. SQLite FTS and
unique-chunk raw grep remain the required search paths; a sealed search-pack
may add Bloom pages once a real multi-file corpus demonstrates enough avoided
decompression to justify roughly 3% extra bytes. Exact trigram postings are
not selected because their lower-bound size is triple the Bloom payload and
they expose exact page membership. Bloom filters still permit probabilistic
term/equality tests, inherit archive permissions and retention, never index
pre-capture redacted values, and do not accelerate literals shorter than three
bytes.

## Active plus sealed search latency

The ignored release benchmark can be rerun with:

```sh
cargo test -p alex-store --test lar_search_benchmark --release -- --ignored --nocapture
```

It writes 400 provider-shaped JSON request artifacts through the production
live store, removes every legacy gzip fallback, and requires both sealed and
active packs before measuring a one-result normalized FTS query and the same
literal through exact unique-chunk raw grep. Forty warm samples on this Linux
workspace produced 14 sealed packs plus one active pack over 1,371,906 logical
body bytes:

| Path | p50 | p95 | p99 |
| --- | ---: | ---: | ---: |
| normalized SQLite FTS | 0.112 ms | 0.151 ms | 0.161 ms |
| exact raw catalog grep | 16.304 ms | 16.663 ms | 17.182 ms |

The raw pass scanned 400 manifests/648 ranges and decompressed 648 unique
chunks (1,371,165 bytes) once per query. This proves the active+sealed search
path and supplies a repeatable latency regression fixture. It is not a 14-day
capacity result: the anonymized real corpus is still required to measure cache
pressure, portable-filter selectivity, and multi-gigabyte scaling.
