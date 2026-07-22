# LAR V1 public scale gate

The alex-lar-scale tool replaces the private-capture benchmark with a
reproducible public corpus. It generates deterministic JSON and authoritative
SQLite rows, then exercises the same offline migration and dual-read seam used
by Alex. No private prompts, credentials, or user traces are inputs.

## Profiles

| Profile | Traces | LAR body records | Logical raw body bytes | Purpose |
| --- | ---: | ---: | ---: | --- |
| ci | 64 | 128 | 8,000,000 | Deterministic pull-request regression |
| full | 55,000 | 110,000 | 9,400,000,000 | V1 scale and memory gate |

Each trace is a one-turn session with a request and response pointer. The two
pointers share one deterministic gzip source file, but migration writes and
indexes both immutable LAR records. Generation retains only one body in memory.
The full legacy input compresses heavily by design; the resulting LAR contains
the real 9.4 GB raw payload stream. Nothing large is checked into Git.

The verifier deliberately interrupts migration after 257 records (17 in CI),
asserts that no SQLite pointers changed, resumes the checkpoint, validates all
source/archive hashes, and then asserts that all pointers changed. Originals
remain available for rollback.

## Budgets

Full-profile budgets:

| Measurement | Budget |
| --- | ---: |
| Corpus generation | ≤ 15 minutes |
| Resume migration plus complete source/archive validation | ≤ 60 minutes |
| Independent full archive verification | ≤ 20 minutes |
| SQLite trace summary page p95 | ≤ 100 ms |
| SQLite session summary page p95 | ≤ 600 ms |
| SQLite filtered model search p95 | ≤ 100 ms |
| One SQLite trace lookup p95 | ≤ 25 ms |
| Indexed random LAR body read p95 | ≤ 25 ms |
| One-turn open, including two LAR bodies, p95 | ≤ 75 ms |
| Process peak RSS | ≤ 512 MiB |

The session-summary budget is higher because that operation deliberately
groups all 55,000 unique synthetic sessions and computes error/account
aggregates before applying its page limit. The ordinary trace summary and
filtered-search budgets remain 100 ms.

CI uses deliberately looser query budgets (250–500 ms), a two-minute migration
budget, and the same 512 MiB RSS ceiling to avoid treating noisy shared runners
as performance laboratories. The exact active budgets are embedded in every
result.

## Run it

Use a disposable location with at least 12 GB free for the full profile:

    cargo run --release --locked -p alex-lar-scale -- run \
      --profile full \
      --root /tmp/alex-lar-scale-full \
      --output docs/benchmarks/lar-v1-full-local.json

The command refuses a non-empty root. It writes the result before returning a
failure for any exceeded budget. The --no-enforce option is available for
diagnostic machines. Generated corpora are disposable and must not be
committed.

To split generation from verification:

    cargo run --release --locked -p alex-lar-scale -- generate \
      --profile full --root /tmp/alex-lar-scale-full
    cargo run --release --locked -p alex-lar-scale -- verify \
      --profile full --root /tmp/alex-lar-scale-full \
      --output /tmp/lar-scale-result.json

Result JSON includes OS, architecture, CPU model, logical CPU count, physical
memory, Rust version, Git commit, corpus/archive/SQLite sizes, migration-resume
evidence, every sample percentile and budget, and peak RSS.

## Sanitized Fable→Sol replay fixture

The fixture command validates the checked-in middleware vector and error body,
injects a synthetic bearer credential, exports through the fail-closed LAR
sanitizer, reopens the archive, and proves the secret is absent while the
Anthropic Fable failure and OpenAI Sol reroute decision remain replayable:

    cargo run --release --locked -p alex-lar-scale -- fixture-fable-sol \
      --vector crates/alex-proxy/tests/fixtures/middleware/fable-to-sol-vector.json \
      --failure crates/alex-proxy/tests/fixtures/middleware/anthropic-fable-refusal-200.json \
      --output /tmp/fable-to-sol.lar \
      --report /tmp/fable-to-sol.json

CI runs both the small scale gate and this fixture path and uploads their
machine-readable reports.

## Published V1 result

The V1 release-gate run is checked in as
[`benchmarks/lar-v1-full-macos-m2-max.json`](benchmarks/lar-v1-full-macos-m2-max.json).
It passed the full 55,000-trace, 110,000-record, 9.4-GB logical corpus on
macOS aarch64 with 65.1 MB peak RSS. Its p95 measurements were 14.747 ms for a
trace-summary page, 342.611 ms for the deliberately all-session aggregate,
1.943 ms for filtered search, 0.864 ms for an indexed body read, and 4.022 ms
to open one turn with both bodies.

## Interpretation limits

- Random body reads are warm-cache measurements. Portable cold-cache eviction
  requires privileged OS-specific controls and remains unverified.
- Synthetic repetitive JSON tests scale, indexing, migration, and bounded
  memory. It does not claim a representative compression ratio for real
  conversations.
- UI virtualization, HAR conversion, live LAR capture, tool-body archives,
  rotation, and compaction remain separate work.
