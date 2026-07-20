# LAR operator runbook

LAR rollout is deliberately reversible. Current releases always create the
legacy gzip capture first; LAR modes add validated archive data without
deleting that rollback copy. Do not run legacy cleanup until the verification
and downgrade decisions below are complete.

## Modes and safe progression

`lar_body_store_mode` accepts three values:

| Mode | New LAR bytes | Published read pointer | Legacy gzip retained |
| --- | --- | --- | --- |
| `legacy` | no | legacy | yes |
| `dual-write-validated` | yes, shadow copy | legacy | yes |
| `lar-with-fallback` | yes | validated LAR, then legacy fallback | yes |

The default remains `legacy`. A conservative rollout is:

1. Back up traces and run the importer dry-run.
2. Enable `dual-write-validated` for a controlled beta and compare hashes,
   latency, disk growth, and error counters.
3. Change to `lar-with-fallback` only after the beta archive verifies and an
   older binary can still read the retained gzip files.
4. Leave legacy cleanup disabled for the documented compatibility window.

Set the mode in `config.toml` or for one daemon process with
`ALEXANDRIA_LAR_BODY_STORE`. Invalid values fail startup rather than silently
selecting another mode.

## Durability and concurrent capture

`lar_durability` controls the boundary completed before a LAR location is
published in SQLite:

| Durability | Boundary | Allowed modes |
| --- | --- | --- |
| `sync` | Flush the complete body/exchange record group and call a full file sync before the SQLite commit. | all; default |
| `batch` | Flush the complete body/exchange record group and call one data-only sync before the SQLite commit. | all |
| `best-effort` | Flush userspace buffers without a disk sync. The synced gzip remains authoritative. | `legacy`, `dual-write-validated` only |

`best-effort` is rejected with `lar-with-fallback`: an authoritative SQLite
pointer is never published without a durable LAR boundary. `batch` means one
sync for all records produced by a single capture; it does not leave a queue of
published but unsynced captures. In implementation terms, `sync` calls
`sync_all`, `batch` calls `sync_data`, and `best-effort` performs no file-sync
call after the writer flush. Pack seal/rotation uses the same selected boundary;
explicit archive publication paths that require stronger atomicity still sync
the file and parent directory independently.

Set the value in `config.toml` or override one daemon process with
`ALEXANDRIA_LAR_DURABILITY`. Live body and exchange appends share one serialized
writer. Concurrent captures wait for it; crossing the contention-warning
threshold emits a warning but does not silently discard the LAR copy or its
exchange metadata.

## Before enabling writes

Stop the daemon for the offline backup command, then retain the resulting file
away from the Alex data directory:

```bash
alex traces export alex-traces-before-lar.tar.gz
cargo build -p alex
alex lar import-legacy --dry-run --verify --json
alex lar ls
```

Trace backup v2 contains its own sealed `capture.lar`, manifest length and
BLAKE3 digest. Restore validates and publishes the archive before importing
rows; repeating the restore is safe:

```bash
alex traces import alex-traces-before-lar.tar.gz
```

The importer dry-run does not write archive pointers or remove sources. Resolve
missing/corrupt gzip reports before rollout, or explicitly accept that those
items will continue to use their existing legacy behavior.

## Controlled beta

Start with:

```toml
lar_body_store_mode = "dual-write-validated"
```

In shadow mode, automatic startup import is disabled because it would publish
owner pointers; manual dry-run and import commands remain explicit. Monitor
`/health`, `/admin/storage`, and:

```bash
alex lar migration status --json
alex lar verify
alex lar grep 'known canary text' --limit 100 --json
```

The health/storage responses report body-store worker queue and work latency,
archive states, physical/catalog bytes, logical/reference/unique bytes,
compression and deduplication ratios, checkpoints, and unreachable chunks.
Compare proxy p50/p95/p99 request latency and error rate with the same workload
in `legacy`; a sequential storage microbenchmark is not sufficient rollout
evidence.

To test authoritative LAR reads while retaining rollback bytes:

```toml
lar_body_store_mode = "lar-with-fallback"
```

On startup the daemon becomes healthy before it schedules bounded background
migration. Configure worker, I/O, CPU, memory, pack, yield, and free-disk limits
under `[lar_migration_resources]`; see [Configuration](configuration.md).

## Migration control

```bash
alex lar migration status --json
alex lar migration pause --json
alex lar migration resume --json
alex lar migration verify --json
```

The JSON report uses schema `alex-lar-migration-verification-v1` and includes
`checksum_algorithm: "blake3"` plus `report_checksum`. The checksum covers the
compact JSON serialization, in declared field order, of `report_schema`,
`valid`, the four checked-item counters, `bytes_reconstructed`, and the ordered issue list,
prefixed by the domain bytes `alex-lar-migration-verification-v1\0`. The CLI's
outer `kind` field is not part of the checksum. This makes saved reports
tamper-detectable; it is intentionally not an identity/authenticity signature.

Jobs, item cursors, leases, counters, failures, and source fingerprints are
durable. Restarting retries only incomplete work. Pointer publication happens
after the appended body reconstructs to the legacy length and BLAKE3 hash. A
failed item remains readable from gzip and is reported rather than hidden.

If free space falls below `min_free_disk_bytes`, leave the job paused, add
space, confirm existing trace reads still work, then resume. Do not delete live
packs or SQLite catalog rows manually.

## Verification and legacy cleanup

Before cleanup, require all of the following:

- every migration job is complete with no pending or failed items;
- `alex lar migration verify --json` succeeds;
- `alex lar verify` succeeds for active and sealed catalog files;
- representative trace extraction matches retained source hashes;
- a trace backup exists and the intended downgrade binary was tested;
- the cleanup plan names only expected legacy files.

Then inspect, but do not yet apply, the plan:

```bash
alex lar cleanup --dry-run --json
```

`cleanup --apply` moves eligible gzip files into recoverable
`lar/quarantine/`; it does not delete LAR data. Applying it changes the
downgrade boundary: a pre-LAR binary needs those quarantined files restored to
read bodies. Keep compatibility reads and quarantine for the declared support
window. Permanent legacy-write removal is a later release decision, not part
of LAR v1 rollout.

## Rollback and downgrade

Before cleanup, rollback is configuration-only:

1. Pause migration.
2. Set `lar_body_store_mode = "legacy"`.
3. Restart and verify a recent request, response, tool body, and long-session
   transcript through the legacy paths.

Changing modes does not rewrite validated pointers or delete either copy. A
current mixed reader prefers a published valid LAR reference and otherwise
falls back to gzip. A pre-LAR binary ignores additive catalog tables and reads
the original body-path columns while the files remain present.

After cleanup, either restore quarantine/backup before downgrading or stay on a
LAR-aware release. Never point an old binary at a store whose only remaining
body copy is LAR and assume the trace bodies will be visible.

## GC, repack, and repair

GC is logical; repack is the separate operation that reclaims immutable pack
bytes. Plan first and retain every run ID printed by the command:

```bash
alex lar gc plan --json
alex lar gc apply --json
alex lar repack plan --min-garbage-ratio 0.25 --json
alex lar repack apply --min-garbage-ratio 0.25 --json
```

Interrupted runs are resumed with `gc resume RUN_ID` or
`repack resume RUN_ID`. Repack currently selects sealed chunk-only packs: it
copies reachable chunks, verifies the replacement, switches catalog locations
atomically, and only then moves the source to quarantine. Combined packs that
also contain manifests, headers, stream indexes, stages, exchanges, exchange
metadata, or conversation records are deliberately ineligible until repack can
rewrite and switch their complete canonical graph. It never edits a sealed
source in place.

Selection is intentionally conservative: a source must be a clean sealed body
pack under Alex's managed LAR directory, contain no non-chunk canonical records,
and have no catalog-owned manifests, header blocks, stages, or migration
destinations. The apply transaction rechecks those conditions before switching
chunk locations. Logical GC and physical reclamation are distinct; the old pack
is quarantined after the switch rather than permanently deleted.

## Standalone export and import

For an already-cataloged LAR trace, `alex lar export --format lar` copies the
complete record closure: authoritative ordered stages, duplicate-preserving
headers/trailers, referenced logical bodies in self-contained destination
manifests/chunks, stream timing/index data, existing exchange metadata, and the
turn's conversation entries, raw ranges, response entries, generation, and
ancestor generations. For an older LAR exchange without a companion, supported
metadata fields are derived from the current trace row. An offline or
inconsistent LAR source is an error. Only a truly legacy-only trace uses the
declared legacy-fidelity synthesis path.

The exporter writes to a unique sibling temporary file, syncs and seals it,
and reopens it to verify the clean footer and every body. Without `--force`, it
publishes through an atomic no-clobber sibling hard-link, so a racing creator is
not overwritten. Forced replacement uses atomic rename on Unix; platforms that
cannot replace by rename may remove the old destination first. It syncs the
parent directory on Unix and never holds the complete
output archive in memory. Current body copying reconstructs
one artifact before immediately chunking it into the destination, so peak body
memory is bounded by the largest single artifact rather than by the complete
session. Writer indexes/record metadata and selected-trace metadata still scale
with the selected archive, subject to format limits. The separate trace backup
builder still materializes its embedded LAR before packaging.

Import accepts only a regular, clean, sealed standalone file with no required
external body references. Before its single catalog transaction it verifies
the whole-file identity, chunks, all reconstructed manifests, stages, headers,
streams, ExchangeMetadata, and conversation graph, then confirms the source did
not change during validation. Publication preserves generation ancestry and
turn/session evidence, rejects conflicting IDs or cycles, and is idempotent.
Import body validation streams verified chunks to a sink. Parsed canonical
record/index state still scales with the archive within the configured limits.

For a damaged standalone/active file, inspect first:

```bash
alex lar verify suspect.lar
alex lar repair suspect.lar --output recovered.lar
alex lar verify recovered.lar
```

Repair is copy-only and never mutates the input. Checksum corruption is not
treated as a harmless truncated tail. Active startup recovery truncates only a
derived incomplete tail to a reader-proven valid boundary; sealed files remain
immutable.

## Offline archives

Moving a sealed archive is a catalog operation, not a daemon outage. Transcript
metadata remains available and affected bodies report `archived_offline` or
`archived_missing` with the stable file UUID and last path. Reattach only the
same clean sealed file: Alex verifies its role, UUID, length, whole-file digest,
footer, chunks, manifests, and bodies before switching the path. Do not replace
an expected archive with a different file at the same pathname.

## Incident checklist

For disk-full, crash-loop, corrupt-tail, or unexpectedly missing-body events:

1. Stop new cleanup/repack work; pause migration when the daemon is reachable.
2. Preserve SQLite, the affected `.lar`, and legacy/quarantine files.
3. Capture `/health`, `/admin/storage`, `alex lar migration status --json`, and
   `alex lar verify` output.
4. Confirm whether the file is active, sealed, retired, offline, or missing.
5. Use copy-only repair for a truncated active archive; never truncate a sealed
   file manually.
6. Restore or reattach only after identity/hash validation.
7. Re-run full migration/archive verification before resuming cleanup.

Format details and compatibility invariants are in
[LAR v1](lar-format-v1.md); benchmark commands and host-specific limitations
are in [LAR benchmark](lar-benchmark.md).
