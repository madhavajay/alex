# LAR storage integration inventory

This inventory records the legacy paths that must be handled before LAR can
replace per-artifact gzip files. It is based on the repository at commit
`1e47fa4` and is intended to be updated when a path is migrated.

## Canonical legacy write seam

`alex_store::Store::write_body` writes compressed files below
`<data-dir>/bodies/YYYY-MM-DD/`. It is called for:

| Artifact | Producer | Legacy kind/path field |
| --- | --- | --- |
| Client request | proxy trace finalization and remote ingest | `request.json` / `traces.req_body_path` |
| Changed upstream request | proxy trace finalization and remote ingest | `upstream-request.json` / `traces.upstream_req_body_path` |
| Response or captured stream | proxy trace finalization and remote ingest | `response.body` / `traces.resp_body_path` |
| Tool arguments | tool-event ingest | `tool-args.json` / `tool_calls.args_body_path` |
| Tool result | tool-event ingest | `tool-result.json` / `tool_calls.result_body_path` |
| Imported harness transcript request/response | CLI transcript import | request/response trace fields above |
| Test/conformance fixtures persisted as traces | proxy test helpers | request/response trace fields above |

`finalize_trace` already avoids writing an upstream body when it is byte-equal
to the client body. LAR still has to deduplicate all identical bodies arriving
through other calls and the shared prefixes within changed bodies.

### Live-write caller audit (2026-07-20)

Every Rust call to `Store::write_body` now reaches the configured body-store
coordinator before any LAR work. The coordinator recognizes the complete set of
normal capture names used by the proxy, CLI imports, reset/backup tests, and
remote ingest:

- `request` / `request.json` -> trace `client_request`;
- `upstream-request` / `upstream-request.json` -> trace `upstream_request`;
- `response` / `response.body` -> trace `client_response`;
- `tool-args.json` / `args.json` -> tool call `tool_arguments`;
- `tool-result.json` / `result.json` -> tool call `tool_result`.

Unrecognized names deliberately remain legacy-only instead of guessing an
owner or publishing a wrong pointer. Dario's Node preload writes into a private
spool; trace finalization and trace-detail reads ingest completed gzip records
through `write_body_artifact`, retaining incomplete files for retry.

The default mode is still `legacy`. `dual-write-validated` and
`lar-with-fallback` both retain a gzip rollback copy for now. They append into
size/time-rotated body packs, serialize the local writer with a bounded lock,
resolve chunk hashes globally through SQLite, sync LAR bytes before catalog
publication, reconstruct through the cross-pack reader, then atomically publish
the manifest. Shadow-only `dual-write-validated` stops there;
`lar-with-fallback` also publishes the owner pointer so the mixed reader prefers
LAR. A failed/busy LAR writer never hides the legacy body.

Live-pack reconciliation and orphan-chunk recovery run once when a non-legacy
store opens, or when explicitly requested; they are not part of each body
write. A write resolves all chunk locations first, opens each reused source
pack once, flushes and syncs newly appended bytes, validates readback, and only
then opens the short SQLite publication transaction.

The live writer now flushes and syncs every publication boundary but writes a
complete derived checkpoint index only after 8 MiB or 30 seconds of new data.
After the checkpoint locator is synced, its monotonic sequence, frame
offset/length, BLAKE3 payload hash, and append boundary are inserted into
`lar_checkpoints` in the same SQLite transaction as the body/exchange catalog
publication. Startup validates the latest row against the immutable frame and
locator; if a crash synced the checkpoint but lost that transaction, live-pack
reconciliation recreates the row before append resumes.
Catalog reads use the synced chunk frame offset and verify the record directly,
so a just-written body is immediately readable without a full-pack scan. This
avoids quadratic index rewriting as a fresh pack grows.

Automatic startup import runs in `legacy` and `lar-with-fallback` modes. It is
disabled in shadow-only `dual-write-validated`, because the importer publishes
validated owner pointers and would otherwise violate the mode's no-publication
guarantee. An operator can still invoke the shared importer explicitly.

The legacy importer publishes each synced archive chunk and every
manifest-to-chunk edge into the same global catalog. Completed jobs backfill
missing rows idempotently one bounded pack at a time on a newer startup, so a
live pack can reference an imported chunk by hash rather than writing another
physical copy. A catalog repair that would require opening a pre-rotation pack
larger than the configured pack/index limits stops with an explicit error
instead of defeating the migration memory ceiling.

The v2 importer appends legacy exchange records, ordered stage timelines, and
header blocks to those same rolling body packs. Stages reference the already
validated manifests, including one shared manifest when the client and
upstream request bytes are identical; there is no second body copy or separate
event log. An adjacent optional exchange-metadata frame preserves the complete
non-body SQLite trace record, including exact nullable token values and f64
cost bits; older readers skip that frame safely. Catalog publication occurs
only after the combined records are
synced and reopened successfully, and the exchange receipt plus header/stage
catalog rows commit in one SQLite transaction. Linked tool rows are merged by
their captured timestamps. Tools with no surviving trace receive an explicit
`legacy-tool:<tool-id>` synthetic exchange containing only ToolCall/ToolResult
stages, so they remain visible rather than disappearing. ToolCall provenance
retains the harness, turn ID, original legacy trace ID (including a dangling
ID), tool-call ID, and tool name as bounded, explicitly legacy JSON metadata;
exit status remains tool metadata and is never fabricated as an HTTP status.

The metadata inventory prefetches manifests, tools, and migration state once
per bounded page instead of querying per trace. Mixed body pages likewise
batch both artifact resolution and manifest-to-file lookup, group reads by
archive, preserve request order, and enforce one reconstructed-byte budget.
The v2 job namespace is distinct from the old body-only v1 job. During upgrade
it reuses validated v1 manifest pointers even when cleanup already removed the
gzip source; a present but changed gzip is re-imported. An active v1 pack that
cannot express external body references is sealed before v2 metadata is
appended to a new deterministic pack.

A completed import is not treated as a permanent end marker while legacy mode
can still create files. On the next run, the importer stats the inventoried
paths and starts one deterministic migration generation only when it finds a
new or changed source. Prior validated fingerprints are skipped, all
generations continue through the same deterministic sequence of body packs,
and a run with no new source reuses the latest completed job without creating
generation churn. Each pack has a soft byte cap and a memory-derived
chunk/manifest index cap. Crossing either after an artifact seals the pack;
the next artifact resumes in the next deterministic pack. Inventory rows,
suffix-file discovery, provenance work, validation batches, predecessor caches,
and returned error details are also bounded independently of corpus size.

## Upgrade and downgrade contract

All three supported write modes create the legacy gzip file first. `legacy`
stops there. `dual-write-validated` additionally writes and validates LAR data
but deliberately leaves the owner on its legacy path. `lar-with-fallback`
publishes the validated LAR pointer while retaining that same legacy path and
gzip file. Changing modes never deletes either representation and never
rewrites an existing validated pointer merely because the process reopened in
a different mode.

The current mixed reader can reopen one database in any of the three modes:
validated LAR wins when published, and an unconverted, failed, or shadow-only
artifact reads from gzip. An older pre-LAR Alex binary ignores the additive LAR
tables and continues to use the original trace/tool body-path columns. That is
the supported downgrade path only while the legacy files remain present.
`cleanup --apply` moves those files into quarantine after full verification;
after cleanup, downgrade body reads require restoring the quarantined files or
using a LAR-aware version. There is no LAR-only write mode in the supported
matrix, and LAR writes must not become the default until that rollback boundary
is explicitly changed and tested.

## Immutable archive movement and availability

Sealed LAR files now have additive whole-file identity rows: stable file UUID,
BLAKE3 digest, byte length, provenance, and validation time. Standalone imports,
live-pack sealing/reconciliation, and repack publication populate that identity.
An older sealed row may be upgraded by validating and hashing only the file at
its current online catalog path; a replacement path is never trusted to define
the expected digest.

`Store::detach_lar_archive` is idempotent and catalog-only. It accepts immutable
sealed files, rejects active writers/repairing/retired files, retains every
manifest/chunk/header/stage reference, and never moves or deletes archive bytes.
File status distinguishes `online`, `archived_offline`, `archived_missing`, and
`retired`; mixed body reads return the file-level `archived_offline` or
`archived_missing` error rather than implying that the daemon is unavailable.

`Store::reattach_lar_archive` accepts body-pack, event-log, standalone,
search-pack, and dictionary roles once sealed identity is known. It validates a
clean sealed footer, UUID, role, format/features, all local chunks and bodies,
whole-file length, and whole-file digest before one transaction changes the
catalog path/state. Relative paths are resolved below the Alex data directory;
external paths remain absolute. Repeating detach or reattach is safe.

## Writes outside the canonical seam

These are not all ordinary trace artifacts and require an explicit policy:

- Dario's Node preload spool is outside the canonical seam by necessity, but
  completed request/response records are moved through the typed store before
  proxy readers expose them. The spool is not a durable body store.
- Dario prompt-cache files are separate operational cache data. They should be
  inventoried for backup/reset behavior but are not LAR trace bodies unless a
  capture explicitly references them.
- Error fixtures are JSON files below the configured fixture directory. The
  injected response is captured by normal trace finalization; the reusable
  fixture definition remains configuration data.
- Trace backup/export creates gzip-compressed tar archives containing SQLite
  rows and body files. LAR-aware backup must include the selected transitive
  manifest/chunk closure instead of copying arbitrary live pack fragments.
- Cursor/Agent trace reconciliation now compares through the mixed reader and
  writes content-addressed, immutable gzip fallbacks plus immutable replacement
  manifests. One transaction switches the trace path and authoritative LAR
  pointer; a failed LAR append clears the stale pointer and keeps the new gzip
  readable. Sealed LAR bytes and prior gzip fallbacks are never updated in
  place.

## Legacy read paths

The following behavior must use a unified artifact reader that prefers a
validated LAR reference and falls back to the legacy path:

- `/admin/traces` and NDJSON body inlining;
- Trace Browser request/upstream-request/response detail endpoints;
- normalized transcript and reply extraction;
- text search over captured bodies;
- error-fixture creation from a captured response;
- session fork/resume/history-signature derivation;
- tool argument/result detail endpoints;
- Dario capture summaries and raw capture detail;
- remote export/import and trace backup/restore;
- reset, retention, disk-usage, and prune accounting;
- tests and harness regression helpers that currently open gzip paths.

No caller should need to know whether an artifact is legacy gzip, a live LAR
manifest, a sealed standalone archive, corrupt, or temporarily offline.

### Proxy read audit (2026-07-20)

The proxy no longer reopens trace body-path columns for NDJSON export or the
compatibility text scan. Trace and run exports collect one bounded mixed-store
batch for the page (256 MiB reconstructed-byte ceiling), reuse archive readers
inside that batch, and perform body I/O, exact binary base64 encoding, and
NDJSON serialization on a blocking worker. A requested path that is missing,
truncated, corrupt, or in an offline/missing archive is represented explicitly
in that trace row's `body_errors`; it is not silently omitted. `/admin/traces`
is metadata-only, but its bounded list/query work now also runs off the async
request executor.

Fallback text search requests only artifacts not already covered by the
normalized index, reads them in one mixed-store batch under a 4 MiB page
budget, and returns per-artifact `body_errors`. This retains compatibility with
new/unindexed legacy and LAR bodies without opening their gzip paths in the
handler.

Trace details, reply extraction, tool body details, and error-fixture capture
read through the same LAR-first API on blocking workers. Trace metadata detail
remains a successful response when a cold body archive is detached and embeds
the typed archive UUID/path in `extras.body_errors`; body-specific endpoints
return that condition as a typed `503`. Dario summary and raw detail first
ingest completed preload records, then use the bounded mixed reader. The sole
production gzip read left in this proxy region is a 64 MiB-bounded compatibility
fallback for a conventionally named Dario record, and it runs only after the
catalog reports that artifact as missing; corrupt or oversized fallback data is
reported explicitly.

## Header fidelity

Current trace headers live in `req_headers_json` and `resp_headers_json`.
Capture redacts sensitive values before persistence. A legacy ordered pair list
retains its represented order and duplicate values, but cannot prove original
HTTP casing (`legacy_casing_unknown`). A JSON object is deterministically
key-sorted and labeled `legacy_order_and_casing_unknown`; scalar arrays retain
their duplicate values, but source field order remains unknown. Invalid or
non-scalar shapes are recorded as explicit unsupported metadata. New capture
converts the observed header sequence directly into ordered LAR atoms and
blocks before reducing it to a map-shaped API view.

## Required migration order

1. Add LAR catalog tables and artifact references without dropping any legacy
   columns.
2. Recover/open the LAR append files, start the daemon, and route new body
   writes to LAR.
3. Start the shared importer in a bounded background worker.
4. For each source: decompress, fingerprint, append/reuse chunks and manifest,
   reconstruct through the normal reader, and compare length plus digest.
5. Atomically publish the validated artifact reference. Keep the legacy path.
6. Serve mixed records through the unified reader throughout migration.
7. Offer legacy cleanup only after a complete verification report; cleanup is
   never part of startup migration.

## Integration completion checklist

- [x] Every recognized `Store::write_body` caller has an intentional, typed LAR
      routing seam (production remains legacy by default).
- [x] Completed Dario request/response spool files route through typed,
      cataloged artifact kinds; incomplete files remain retryable.
- [x] Cursor/Agent body changes create immutable, content-addressed fallback
      files and replacement manifests without rewriting prior bytes.
- [x] Remote trace upload and Cursor/Agent reconciliation read request,
      upstream-request, and response bodies through the unified mixed reader.
- [x] Proxy trace/run NDJSON body inlining, fallback text search, trace/reply
      detail, tool detail, and captured-response fixtures use bounded unified
      reads with explicit per-artifact or typed archive errors.
- [x] Dario summaries/details prefer cataloged artifacts after spool ingestion;
      only an uncataloged convention capture may use the bounded compatibility
      gzip fallback.
- [ ] Every gzip read listed above goes through the unified reader.
- [x] Headers from new captures use ordered blocks; imported headers are
      fidelity-labeled.
- [x] Backup, restore, reset, prune, and disk usage account for shared chunks.
- [x] Feature modes (`legacy`, `dual-write-validated`, `lar-with-fallback`)
      retain explicit gzip downgrade copies and reject unknown values.
- [x] Reopening data written by each supported mode in every supported mode
      preserves exact body reads and the original gzip downgrade copy.
- [x] The legacy importer inventories missing and corrupt paths without
      deleting or hiding them.
- [x] Import inventory, error reporting, active archive indexes, and catalog
      backfill remain bounded by batch/memory/pack caps across continuation.
