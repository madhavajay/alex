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
`repack resume RUN_ID`. Repack copies reachable chunks, verifies the
replacement, switches catalog locations atomically, and only then moves the
source to quarantine. It never edits a sealed source in place.

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
