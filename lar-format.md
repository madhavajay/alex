# LAR — LLM Archive Format and Implementation Plan

Status: DRAFT v0.2

Owner: storage lane

Extension: `.lar`

Working name: LAR (LLM Archive)

Positioning: a compact, loss-aware, indexed archive for LLM and agent traffic

## 1. Decision summary

LAR is a content-addressed byte store plus an ordered exchange log.

Its defining invariant is:

> Raw body bytes are stored once per unique content-defined chunk. Requests,
> responses, retries, routing stages, tool payloads, stream events, normalized
> conversations, exports, and replay views reference those chunks rather than
> storing duplicate bodies.

The live Alex store uses globally deduplicated rolling LAR segments. A
standalone `.lar` export contains all chunks needed to read it without the live
catalog. SQLite remains the live query/index layer and maps traces, sessions,
artifacts, chunks, and archive files.

LAR borrows proven ideas from MCAP and WARC:

- length-prefixed, skippable records;
- independently compressed chunks/pages and per-record integrity checks;
- checkpoint and footer indexes;
- durable record IDs, content digests, references, and continuation records;
- recovery by scanning valid records after an unclean shutdown.

OpenTelemetry GenAI and OpenInference inform the normalized semantic view, but
their evolving schemas do not define LAR's permanent disk representation.

## 2. Problem statement

Alex currently stores request, upstream-request, response, and tool bodies as
separate gzip files. Agent harnesses repeatedly resend the full conversation,
and passthrough routes frequently store identical client and upstream bodies.
Storage therefore grows with repeated prefixes rather than genuinely new data.

Measured example:

- 14 days, one user: approximately 9.4 GB body files and 240 MB metadata;
- 55,000 exchanges;
- representative day: 104 MB client requests, another 104 MB upstream-request
  copies, and 18 MB responses.

The target is for each turn to cost approximately its genuinely new bytes plus
small manifests and exchange metadata.

## 3. Goals

### 3.1 Storage

- Store each unique raw content-defined chunk once in the live store.
- Make unchanged client/upstream bodies share the same manifest.
- Deduplicate repeated conversation prefixes, system prompts, tool schemas,
  tool results, attachments, and other large repeated byte ranges.
- Compress unique chunks and repetitive metadata independently with zstd.
- Keep memory use bounded while ingesting and migrating large captures.
- Support rolling files without losing global deduplication.

### 3.2 Fidelity

- Reconstruct every captured body byte-for-byte at Alex's application boundary.
- Preserve every observed request/response header block in order, including
  duplicates, original casing where available, and trailers.
- Preserve the ordered sequence of client, router, retry, failover, upstream,
  tool, and downstream stages.
- Preserve raw streamed bytes and observed read timing without storing a second
  reassembled body.
- Record a fidelity level so imported legacy data never claims details that the
  old format did not capture.

### 3.3 Performance

- Keep up with the live proxy write path without buffering whole sessions.
- Locate a trace and begin reading a small body in under 10 ms from a local,
  sealed archive under benchmark conditions.
- Keep active/new logs immediately readable without waiting for file rotation.
- Avoid blocking async request handling with compression, parsing, migration,
  recovery, or archive reads.
- Support bounded paging for Trace Browser transcripts.

### 3.4 Operations

- Import the existing SQLite plus `bodies/` layout idempotently.
- Allow migration to run automatically after startup only when an operator has
  explicitly enabled it; the rollout default remains read-only/dry-run first.
- Keep the daemon healthy and old traces readable while migration runs.
- Resume safely after process interruption, power loss, or partial corruption.
- Verify imported bytes before changing any trace pointer.
- Defer deletion of legacy files to an explicit, separately reversible cleanup
  stage.
- Support retention, garbage collection, repair, export, and re-import.

### 3.5 Interoperability

- Export HAR, WARC, JSONL, OpenTelemetry/OpenInference, and raw artifacts.
- Import Alex's legacy body layout.
- Publish and version the format with a conformance corpus.
- Keep archives self-describing and readable without Alex-specific source code
  when a schema-aware LAR reader is available.

## 4. Non-goals for v1

- Reproducing TLS ciphertext, TCP packet boundaries, HPACK/QPACK encoder state,
  or kernel/network scheduling.
- Treating LAR as the primary query engine; SQLite and FTS remain the live index.
- In-place mutation of sealed archives.
- Lossless conversion of every LAR feature into HAR or OpenTelemetry.
- At-rest encryption in v1. The header must reserve a compatible feature path.
- Mathematical substring deduplication at every possible byte offset. The v1
  guarantee is one stored copy per unique content-defined chunk.

## 5. Fidelity boundary

LAR preserves the exact HTTP/body representation visible to Alex after
transport and TLS decoding, subject to capture-time redaction.

Header pairs preserve logical order, duplicates, and available casing. They do
not reproduce HTTP/2 HPACK bytes, HTTP/3 QPACK bytes, TLS records, or TCP packet
segmentation.

For streams, LAR distinguishes:

- raw byte sequences returned to Alex by the HTTP library;
- read boundaries and arrival deltas;
- parsed SSE/NDJSON/provider frame byte ranges;
- derived provider-neutral deltas.

The raw observed byte sequence is authoritative. Parsed and normalized views
are derived and independently versioned.

## 6. Logical architecture

```text
SQLite catalog and search indexes
  ├── trace/session/exchange metadata
  ├── artifact → manifest references
  ├── chunk hash → pack location
  ├── archive/checkpoint state
  ├── migration state
  └── FTS/reverse references

Rolling live LAR packs (v1 combines bytes and event records)
  ├── unique compressed chunks
  ├── exchange and stage/attempt records
  ├── header blocks and stream indexes
  ├── external body-manifest references
  ├── chunk integrity records
  ├── checkpoints
  └── chunk index/footer
```

All physical LAR files carry a role in their header:

- `body-pack`: globally unique chunks for the live store;
- `event-log`: exchanges, manifests, references, and indexes;
- `standalone`: a self-contained exported archive containing both;
- `search-pack`: optional portable search accelerators;
- `dictionary`: optional shared compression dictionary material.

The v1 live writer uses combined rolling `body-pack` files so one synced append
boundary covers both bytes and their exchange metadata. The `event-log` role is
reserved for a future split layout if measurement shows that separating hot
metadata from cold chunks improves operation; readers already understand the
role and stable IDs never depend on the physical split.

## 7. Core data model

### 7.1 Chunk

```text
Chunk {
  hash_algorithm
  hash
  uncompressed_length
  compression
  dictionary_id?
  compressed_length
  compressed_bytes
  checksum
}
```

Requirements:

- Hash uncompressed bytes.
- Use BLAKE3 initially, with an explicit algorithm identifier.
- Verify hash, length, and byte equality before accepting an existing chunk.
- Use content-defined chunking so insertions and moving JSON fields resynchronize
  rather than invalidating every later chunk boundary.
- Benchmark FastCDC and at least one simpler rolling-hash implementation.
- Benchmark chunk sizes around 8 KiB minimum, 32 KiB target, and 128 KiB maximum;
  do not freeze these values before corpus measurement.
- The first 77-turn synthetic-corpus gate also measures a 512-byte minimum,
  2-KiB target, and 8-KiB maximum profile. It removes substantially more
  repeated-prefix bytes and is the implementation default below 8 MiB. Larger
  bodies use a measured 2/8/32-KiB profile to avoid tiny-frame reconstruction
  overhead; see `docs/lar-benchmark.md`.
- Store chunks in independent frames/pages so reading one body does not require
  decompressing an entire daily file.

### 7.2 Body manifest

```text
BodyManifest {
  manifest_id
  total_length
  whole_body_hash
  media_type?
  content_encoding?
  chunks: [
    { chunk_hash, logical_offset, length }
  ]
}
```

Requirements:

- Reconstruct the exact captured bytes by concatenating referenced ranges.
- Content-address manifests so identical complete bodies with the same
  manifest metadata share one manifest. Media/encoding variants may keep tiny
  distinct manifests while their content-addressed chunks remain single-copy.
- Permit byte ranges into chunks for import, segmentation, and future packing.
- Permit a writer to segment a body against a known predecessor manifest and
  reference matching predecessor chunk ranges directly. Store only literal
  ranges that do not already exist, keep manifests non-recursive, and bound
  matcher memory and work. This is the measured fallback when CDC boundaries
  alone cannot find short growing-conversation prefixes.
- Never store a second reassembled stream body when it can be derived from the
  raw manifest and indexes.
- Support empty bodies without allocating a chunk.

### 7.3 Header atoms and blocks

```text
HeaderAtom {
  original_name_bytes
  value_bytes
  flags
}

HeaderBlock {
  block_id
  ordered_atoms: [HeaderAtomRef]
}
```

Requirements:

- Preserve order and duplicate fields.
- Preserve original name casing when exposed by the HTTP stack.
- Redact sensitive values before hashing and storage.
- Deduplicate complete identical blocks.
- Intern repeated names and exact name/value atoms.
- Batch header blocks with turn metadata into zstd pages; avoid one compression
  frame per tiny block.
- Preserve trailers as distinct blocks.
- Represent unavailable order/casing from legacy imports with a fidelity flag.

### 7.4 Exchange, stage, and attempt records

```text
Exchange {
  trace_id
  session_id?
  run_id?
  parent_trace_id?
  capture_sequence
  wall_time_ns
  monotonic_delta_ns?
  clock_id?
  stages: [StageRef]
  normalized_view_ref?
}

Stage {
  stage_id
  kind
  attempt_number?
  wall_time_ns
  monotonic_delta_ns?
  request_headers_ref?
  request_body_manifest_ref?
  response_headers_ref?
  response_body_manifest_ref?
  trailers_ref?
  stream_index_ref?
  provider/model/account/routing fields
  status/usage/cost/error fields
}
```

Stage kinds must cover at least:

- client request as received;
- normalized/router decision;
- substitution/failover/retry decision;
- each upstream request attempt;
- each upstream response or transport failure;
- client response headers/body/trailers;
- harness tool call/result;
- auth refresh and account routing events;
- Dario request/response capture;
- injected/simulated response;
- cancellation/client disconnect.

Unchanged bodies at different stages must reference the same manifest.

### 7.5 Stream index

```text
StreamRead {
  byte_offset
  byte_length
  delta_from_first_byte_ns
}

ParsedFrame {
  byte_offset
  byte_length
  parser
  frame_kind
  normalized_delta_ref?
}
```

Requirements:

- Store raw observed response bytes once.
- Record first-byte and last-byte timestamps on the stage.
- Record read/frame deltas compactly using delta-varints.
- Allow exact raw replay at the observed read boundaries.
- Allow parsed SSE/NDJSON replay from byte ranges.
- Fall back to opaque raw reads for unknown or malformed streams.

### 7.6 Normalized conversation graph

A session is not assumed to be a single linear sequence. Retries, subagents,
compaction, edits, branches, and concurrent requests require a persistent DAG.

```text
ConversationEntry {
  entry_id
  semantic_schema
  role/kind
  raw_byte_ranges: [ArtifactRangeRef]
  name?
  tool_call_id?
}

Generation {
  generation_id
  parent_generation_id?
  ordered_entry_refs
  reason: initial | append | compaction | branch | mutation | import
}

TurnView {
  turn_view_id
  trace_id
  generation_id
  upto_index // inclusive request-prefix cursor
  response_entry_refs
}
```

The implemented v1 core uses required feature bit `0x2` and canonical record
types 12 (conversation entry), 13 (generation), and 14 (turn view). All three
have stable BLAKE3 IDs, bounded canonical decoders, metadata-page support,
footer/checkpoint indexes, and forward recovery. The core deliberately does not
parse provider wire bodies: known-format adapters construct semantic-schema-1
entries, while unknown formats can use semantic schema 0 with opaque labels and
raw ranges only. Exact layouts are specified in `docs/lar-format-v1.md`.

Requirements:

- Raw body manifests remain the fidelity source of truth.
- The normalized graph references raw ranges wherever possible instead of
  duplicating prompt/result text.
- Compaction creates a generation referencing surviving entries plus the new
  summary entry.
- Unknown provider formats can retain raw-only entries and a valid exchange.
- Version normalized semantic schemas independently from container records.
- Provide adapters to the pinned OpenTelemetry GenAI and OpenInference versions.

Binary patch payloads remain deferred. Initial measurements show that CDC
boundaries alone are insufficient for the representative storage target, so
v1 uses predecessor-aware direct chunk-range manifests instead: copied ranges
still point at authoritative raw chunks, have no recursive replay chain, and
remain verifiable by the ordinary manifest reader.

## 8. Physical container

### 8.1 Header

```text
magic: LAR1
container_major
container_minor
file_uuid
file_role
created_at_ns
writer_id/version
required_feature_bits
optional_feature_bits
default_hash_algorithm
default_compression
dictionary_descriptors
```

### 8.2 Record frame

```text
record_type
record_schema_version
flags
payload_length
payload
checksum
```

Rules:

- Unknown optional record types can be skipped using `payload_length`.
- Unknown required features cause a clear reader error.
- Minor versions may add optional records or fields.
- Major versions may change required semantics.
- Sealed archives are immutable; upgrading rewrites to a new archive.
- Hashes identify uncompressed logical content and survive repacking.
- File offsets are never durable public identities.

### 8.3 Pages and compression

- Body chunks are independently decompressible or grouped only into bounded
  pages with an index that identifies the containing page.
- Metadata/header/turn records are batched into bounded zstd pages.
- Benchmark a static JSON/HTTP dictionary versus plain zstd.
- Per-file trained dictionaries are not required for live v1 because training
  conflicts with one-pass append.
- Every page records compressed/uncompressed lengths and a checksum.

### 8.4 Checkpoints and footer

- Active files emit periodic checkpoint records by byte count and time.
- After file sync, SQLite atomically stores the checkpoint sequence, frame
  offset/length, payload hash, and append position with the corresponding
  catalog publication. Startup validates or reconstructs a row lost after sync.
- On clean seal, write complete indexes, footer, and trailing magic.
- A sealed footer indexes trace IDs, sessions, manifests, chunks, header blocks,
  conversation entries, generations, turn views, turn trace IDs, and
  section/page offsets. A persisted time index remains future work.
- Standalone readers can recover a missing footer by forward-scanning valid
  frames and rebuilding indexes.
- Provide a `lar repair` command that writes a repaired file rather than
  mutating the damaged input.

### 8.5 Durable identity

SQLite and external references use IDs, not physical offsets:

```text
file_uuid + record_id
archive_set_uuid + trace_id
chunk_hash
manifest_id
```

Offsets are replaceable index/cache values because repacking, garbage
collection, and format upgrades change them.

## 9. Live write path

Required ordering:

1. Receive/redact captured bytes.
2. Stream through content-defined chunker on a blocking worker.
3. Look up candidate chunk hashes in the live catalog.
4. Verify and append missing chunks.
5. Append body manifest, header blocks, stages, and exchange records.
6. Flush the LAR records to the configured durability boundary.
7. Commit SQLite locations and trace/artifact references transactionally.
8. Publish the completed trace to readers/UI.

Rules:

- Never commit SQLite references to bytes that are not recoverably appended.
- LAR-first failures may leave unreachable orphan records; repair/GC removes
  them safely.
- Compression, hashing, body parsing, and disk reads must not run on Tokio's
  async executor threads.
- Bound queues and apply backpressure rather than growing memory without limit.
- The durability mode is configurable as `sync` (full `sync_all` per publication
  boundary, the default), `batch` (one `sync_data` per publication boundary),
  or `best-effort` (writer flush with no file-sync call). `best-effort` is valid
  only while legacy gzip remains authoritative (`legacy` or
  `dual-write-validated`) and is rejected for `lar-with-fallback`; `batch` is
  not a delayed multi-capture queue.
- Chunk and manifest insertion must be idempotent under retries.

## 10. SQLite integration

Existing trace/session metadata remains queryable. Add normalized tables or
equivalent structures for:

```text
lar_archive_sets
lar_files
lar_checkpoints
lar_chunks
lar_manifests
lar_manifest_chunks
lar_header_atoms
lar_header_blocks
lar_trace_artifacts
lar_stage_records
lar_migration_jobs
lar_migration_items
lar_gc_runs
```

Requirements:

- Existing `traces` rows gain LAR artifact references without immediately
  removing legacy path columns.
- A unified artifact reader prefers valid LAR refs and falls back to legacy
  body paths during migration/rollback.
- Index trace ID, session ID/time, manifest ID, chunk hash, archive UUID, and
  migration state.
- Keep a generation/revision counter for live transcript paging.
- SQLite remains rebuildable from sealed LAR indexes plus explicit catalog
  backup records where feasible.

## 11. Search and listing

### 11.1 Metadata listing

- List sessions, traces, stages, attempts, artifacts, and archive files from
  SQLite without opening bodies.
- A standalone `lar ls` reads sealed indexes without decompressing body pages.
- Active records are visible immediately through SQLite.

### 11.2 Semantic text search

- Use SQLite FTS5 initially for normalized user, assistant, reasoning, tool,
  error, and selected header fields.
- Index each unique normalized entry once.
- Maintain reverse references from entry/chunk to session/trace/ranges.
- Search results include the matching trace ID and timestamp so Trace Browser
  can page directly to the match.
- Treat the FTS index as disposable and rebuildable.
- Version both the FTS schema and provider-neutral extractor. A version change
  clears derived rows and marks the index as needing a bounded rebuild; raw LAR
  bytes remain authoritative.
- Track artifact-level index coverage, including artifacts with no semantic
  text, so mixed-mode search opens only genuinely legacy/unindexed gzip bodies.
- Bound live/rebuild extraction by body bytes, JSON depth/nodes, entry count,
  per-entry characters, and total characters. A partial rebuild remains marked
  `needs_rebuild` rather than presenting partial coverage as complete.
- The initial extractor indexes bodies up to 64 MiB and at most 1 MiB of unique
  normalized semantic text per artifact, split into 64 KiB FTS entries. Hitting
  any byte, parse, depth, node, entry, or character bound records
  `skipped_limit`; already-extracted text remains useful, but is never treated
  as complete coverage.
- Search only an explicit low-risk header allow-list (content negotiation and
  encoding, user agent, provider/API versions and beta flags, request IDs, and
  SDK runtime-identification fields). Authentication, cookies, API keys,
  credentials, secrets, tokens, and arbitrary extension headers never enter
  the semantic index or compatibility fallback, even if capture redaction was
  accidentally omitted.
- Index new trace request/response header metadata synchronously with its trace
  anchor. A bounded rebuild restores those derived rows from SQLite alongside
  normalized LAR bodies.
- Mixed-mode compatibility search uses stable timestamp/trace-ID cursor pages,
  rather than a newest-N window. One request scans at most 100,000 trace rows
  and 256 MiB of otherwise-unindexed bodies, with 64 MiB per read page. The API
  returns `coverage_complete`, `coverage_limit_reasons`, bytes read, and bounded
  body errors; reaching a limit is an explicit partial result, not a silent
  claim that older or larger traces did not match. A dropped HTTP request sets
  a cooperative cancellation flag checked between cursor pages.

### 11.3 Raw byte grep

- Provide `lar grep <literal>` for exact raw artifact search.
- Search logical manifest ranges with matcher state carried across range/chunk
  boundaries. Verify BLAKE3 after decompression and reuse each unique chunk per
  source instead of rescanning bytes for every referencing body.
- Treat `max_cached_chunk_bytes` as a RAM budget, not a corpus-size limit. The
  current scanner keeps a bounded LRU-style hot set (including per-entry
  overhead), spills verified decompressed chunks to an append-only temporary
  file, and reuses spilled bytes without decompressing the source again. The
  temporary file and its on-disk hash buckets are removed on normal scanner
  drop; temporary-disk exhaustion or spill corruption fails the exact search.
- Keep explicit hard limits for literal bytes, manifests, manifest ranges,
  logical bytes scanned, and returned reverse references. Exceeding a limit
  fails rather than returning an apparently complete partial result. Current
  defaults allow a 1 MiB literal, 1,000,000 manifests, 8,000,000 ranges,
  512 MiB of charged chunk-cache RAM, and 64 GiB of logical bytes per source.
- Add optional per-page trigram/Bloom filters to skip impossible compressed
  pages; always verify matches after decompression.
- Regex without an extractable literal may fall back to a full scan.
- Report every referencing trace/stage without searching duplicate chunks more
  than once.
- Clearly distinguish normalized text search from exact raw byte search.

## 12. Legacy conversion and startup migration

Migration is a permanent, versioned subsystem rather than a disposable script.

### 12.1 Inputs

Inventory and import every legacy artifact category, including:

- client request body;
- upstream request body;
- response body or captured stream;
- request/response header JSON;
- tool arguments and results;
- Dario upstream request/response captures;
- fixture/injected bodies;
- attempts and routing metadata;
- any backup/import body paths supported by the current store.

### 12.2 Fidelity of imported data

- Decompress legacy gzip files and store the exact resulting bytes.
- Preserve the original legacy path, size, timestamp, and import hash as
  provenance metadata.
- Import headers exactly as represented by the old database.
- Mark headers reconstructed from JSON maps as `legacy_normalized` when order,
  casing, duplicates, or original wire representation were not retained.
- Store the supported legacy non-body trace fields in an optional
  exchange-metadata companion frame adjacent to its exchange. This preserves
  exact request and response millisecond timestamps, nullable token fields, f64
  cost bits, transport formats, routing/error fields, tags, and the other fields
  enumerated by the Type 15 schema; IDs, stages, body/header bytes, and catalog
  provenance remain in their ordinary records or SQLite projections.
- Do not infer missing stream timing or transport details.
- Preserve missing/corrupt state as an explicit artifact error.

### 12.3 Startup behavior

On a version introducing LAR, with startup migration explicitly enabled:

1. Apply additive SQLite schema migrations.
2. Open and recover active LAR files to the last valid record/checkpoint.
3. Start the daemon and health/admin endpoints.
4. Route all new writes to the configured LAR write path.
5. Start or resume the legacy migrator in the background. The
   `lar_startup_migration` setting defaults to `false`, and startup never runs
   migration in `dual_write_validated` mode.
6. Serve old and migrated traces through the same body/transcript APIs.

Migration must not delay normal daemon availability beyond the bounded catalog
and recovery checks required for safe operation.

### 12.4 Migration job state

Persist:

- migration format/source version;
- job ID and owner lease;
- start/update/completion timestamps;
- discovered, pending, migrated, skipped, and failed counts;
- bytes read, unique bytes written, and bytes deduplicated;
- last committed cursor/batch;
- per-artifact source fingerprint and destination manifest;
- validation status and error details;
- cleanup eligibility.

### 12.5 Idempotence and concurrency

- Only one migrator owns the migration lease.
- A stale lease can be safely recovered after timeout.
- Artifact identity uses trace/stage/kind plus a source fingerprint.
- Reprocessing a completed item verifies/reuses its manifest and does not write
  duplicate chunks.
- Metadata conversion uses a versioned job namespace independent of the older
  body-only job, and reuses validated body manifests even when their original
  gzip files have already been cleaned up.
- Inventory planning prefetches manifests, tool rows, and completed metadata
  fingerprints in bounded pages; startup must not issue per-trace planning
  queries across a large corpus.
- Commit each bounded batch transactionally.
- Migration may be interrupted at any record boundary and resumed.
- New live captures and legacy migration may write concurrently through the
  same single-writer/append coordinator.

### 12.6 Validation

For every imported body:

1. Hash decompressed legacy bytes.
2. Store/reuse LAR chunks and manifest.
3. Read the manifest through the normal LAR reader.
4. Hash reconstructed bytes.
5. Require length and hash equality.
6. Update the trace artifact pointer only after validation succeeds.

Corpus validation additionally compares all request/upstream/response/tool
artifacts and produces a signed/checksummed migration report.

### 12.7 Resource controls

- Configurable worker count, I/O rate, CPU budget, and batch size.
- Rotate importer packs using configurable byte and index-entry caps; derive
  effective inventory and index caps from the memory budget.
- Pause or slow migration when free disk falls below a safety threshold.
- Yield to live capture and interactive Trace Browser reads.
- Bound inventory, provenance batches, predecessor caches, validation state,
  error details, and each opened archive index independent of corpus size. A
  single streamed artifact is the only permitted one-artifact cap overshoot.
- Surface progress, throughput, dedup ratio, ETA, and last error.
- Allow pause/resume through CLI/admin UI without corrupting state.

### 12.8 Failure handling

- Missing legacy file: record failure and keep the legacy pointer.
- Corrupt/truncated gzip: record failure and keep the source untouched.
- LAR append failure: do not update the trace pointer.
- Validation mismatch: quarantine the new manifest and keep legacy data.
- SQLite failure after append: leave an orphan record for later GC/recovery.
- Process crash: resume from the last committed batch/checkpoint.
- Unsupported legacy record: preserve metadata and report it explicitly.

### 12.9 Rollback and cleanup

- Keep legacy path columns readable for at least two subsequent minor releases
  and at least 90 days after LAR writes become the default, whichever is
  longer. Announce removal at least one minor release in advance.
- Do not delete a legacy body during the startup migration.
- All supported write modes retain gzip; current mixed readers can reopen data
  in any supported mode. A pre-LAR downgrade remains supported only until
  cleanup moves those gzip files to quarantine.
- Provide `alex lar migration status|pause|resume|verify`.
- Provide `alex lar cleanup --dry-run` with counts and byte totals.
- Cleanup requires migration completion plus a full verification pass.
- Cleanup moves recoverable files to trash/quarantine where practical before
  permanent deletion.
- Record what was removed, when, by which version, and whether it is recoverable.
- Document downgrade behavior before enabling LAR-only writes by default.

### 12.10 Standalone conversion command

```text
alex lar import-legacy [--dry-run] [--verify] [--limit N]
alex lar migration status
alex lar migration pause
alex lar migration resume
alex lar migration verify
alex lar cleanup --dry-run
alex lar cleanup --apply
```

The standalone command and startup migrator must call the same library code.

## 13. Retention, garbage collection, and archive movement

Global deduplication makes retention reference-based rather than simple file
deletion.

- Deleting a trace removes artifact/manifest references, not chunks directly.
- Maintain reference counts as an optimization, not the sole source of truth.
- Periodic mark-and-sweep verifies reachable manifests/chunks.
- Repack body packs when garbage exceeds a configurable threshold.
- Repacking writes a new pack, verifies it, atomically switches catalog
  locations, then retires the old pack.
- Repacking persists and rechecks the source pack's whole-file identity at
  planning, copy, switch, and retirement boundaries. Selective graph rewrites
  reject unknown optional records, optional file features, header extensions,
  and unsupported record schemas rather than silently dropping them.
- Never rewrite a sealed pack in place.
- Archive movement uses an archive-set catalog and stable file UUIDs, not paths.
- Missing/offline packs produce an explicit `archived_offline` state and can be
  reattached later.

Standalone export:

- for every already-cataloged LAR trace, copies its authoritative ordered stage
  list, duplicate-preserving request/response/trailer header blocks, every
  referenced logical body into self-contained destination manifests/chunks,
  its stream read/frame index, an existing ExchangeMetadata companion, and the
  turn's complete conversation closure. When an older source has no companion,
  the exporter derives the supported metadata fields from the current trace row;
- includes every conversation entry raw-range manifest, response entry,
  generation, and ancestor generation needed to preserve compaction/branch
  history; IDs are recomputed only from equivalent rewritten references;
- writes one sealed `.lar` with no external archive-set body references and
  verifies its footer plus every reconstructed body before publication;
- preserves manifest media type and content encoding. Destination chunking may
  produce a different manifest ID/topology, so dependent content IDs are
  transitively recomputed while the reconstructed body bytes remain exact;
- may duplicate chunks across separate standalone exports by design;
- treats an offline or inconsistent cataloged LAR trace as an error rather than
  silently downgrading it. A genuinely legacy-only trace is exported through a
  separate, explicitly declared legacy-fidelity synthesis path;
- writes directly to a uniquely named sibling temporary file, syncs it, and
  verifies it. Without `--force`, a sibling hard-link provides atomic
  no-clobber publication even if another process races to create the output.
  Forced replacement is atomic on Unix; platforms that cannot replace by
  rename may remove the old destination immediately before publication. It
  does not build the complete archive bytes or a complete logical body in a
  `Vec`; body copying uses fixed-size memory windows and a temporary on-disk
  spool while retaining transport metadata. Writer indexes/record metadata and
  native multi-trace selection metadata still scale with the selected archive,
  subject to format limits. Trace backup construction is a separate path and
  still materializes its embedded LAR before packaging.

Standalone import validates a regular, clean, sealed file with the standalone
role, rejects external archive-set body references, verifies the whole-file
identity, every chunk, and every reconstructed manifest, then rechecks the file
identity before one SQLite publication transaction. It attaches ordered stages,
headers, ExchangeMetadata, streams, conversation entries/ranges, generations,
turn response references, and each turn's ancestor-generation/session evidence.
Conflicting content-addressed IDs and generation cycles fail; exact re-imports
are idempotent and existing logical bodies may be reused. Manifest validation
streams verified chunks to a sink rather than allocating a complete body;
canonical record/index metadata still scales with the archive within explicit
format limits.

The v1 conversation record does not encode the parser's local `source_format`
label. Standalone import therefore leaves that derived catalog field empty
instead of inventing a value. Raw ranges, normalized schema version, role,
kind, name, and tool-call ID remain preserved. Selective exact export is
version-scoped to canonical record types understood by the current reader;
future optional record types require an ownership envelope before they can be
carried through unchanged.

### 13.1 Trace backup, restore, and reset

Trace backups use a versioned envelope. Version 2 retains the portable JSONL
rows for complete trace, tool-call, heartbeat, session, and run metadata, but
replaces the copied `bodies/` tree with one sealed `capture.lar`. The LAR holds
each exact request/response/tool body once at the content-addressed manifest
layer and carries headers/stages at their declared capture fidelity. Tool body
owner edges are recorded in the envelope by BLAKE3 plus length so restore can
resolve them even when the destination catalog deduplicates to a pre-existing
manifest ID.

Restore requirements:

- Continue accepting the version 1 JSONL plus `bodies/` tar layout.
- Reject duplicate/unsafe tar paths and unknown envelope versions.
- Verify the embedded LAR byte length and BLAKE3, then validate its sealed,
  self-contained closure before any backed-up SQLite row is inserted.
- Publish the immutable LAR under an Alex-owned content-addressed path before
  attaching catalog records; never leave catalog pointers to staging files.
- Import complete metadata only after LAR publication, then attach tool owner
  edges by validated content identity.
- Make every boundary idempotent. A retry after LAR publication, metadata
  insertion, or tool-edge attachment completes without duplicating rows or
  body bytes.

Reset requirements:

- Close the active writer before changing catalog ownership.
- Remove roots, derived indexes, migration/GC/repack state, and archive catalog
  records in the same SQLite transaction as trace/tool/heartbeat rows.
- Delete Alex-owned `bodies/` and `lar/` directories only after that commit, so
  an interrupted filesystem cleanup leaves unreferenced bytes rather than
  references to missing shared content.
- Never delete an attached standalone archive outside the configured data
  directory.

## 14. Replay and export

### 14.1 Raw artifact replay

- Reconstruct method/path, ordered headers, body, response headers/body/trailers,
  and observed stage ordering.
- Clearly mark redacted values and fidelity limitations.
- Never silently fabricate transport details.

### 14.2 Stream replay

- Replay raw observed reads with original or scaled timing.
- Replay parsed provider events independently.
- Permit instant mode for tests and UI inspection.
- Resolve replay by trace plus stage, since retries may have independent
  upstream streams.
- Page observed reads or parsed frames by bounded event cursor and byte budget;
  reconstruct only the selected manifest ranges and cache touched chunks.
- Return absolute observed deltas without server-side sleeping. UI speed
  scaling schedules those deltas locally.
- Return typed offline/missing archive details so clients can reattach and
  retry the same cursor.

### 14.3 Exports

- Modern interchange exports derive from the exact canonical exchange/stage
  timeline. Base stages retain capture order; late linked tool supplements are
  added in deterministic wall-time/capture-sequence/phase order even when their
  records live in another pack. Occurrence IDs remain distinct from immutable
  content IDs. Header/trailer atoms, retries, stream reads/frames, exchange
  metadata, and one descriptor per distinct logical body are retained.
- HAR: transport-oriented request/response/timing representation. Standard
  client fields are populated and the complete canonical graph plus non-client
  stage bodies remain in Alex extensions.
- WARC: durable linked request/response/metadata/resource records with
  interoperable SHA-256 block digests. HTTP framing is a declared synthesis;
  projected HTTP records do not mislabel a hash of their complete block as an
  entity payload digest.
- JSONL: legacy-only selections retain the import-compatible v1 shape.
  Canonical/mixed selections use v2 graph records plus bounded 48 KiB body-part
  records, so a physical line is independent of logical body size. The current
  v1 importer rejects v2 precisely instead of silently dropping canonical
  details; standalone LAR is the lossless re-import path for now.
- OpenTelemetry and OpenInference: distinct derived JSONL span adapters. OTel
  uses current `gen_ai.*` attributes; OpenInference uses
  `openinference.span.kind`, `llm.*`, and `input/output.*` attributes. Both keep
  the exact canonical graph in an Alex extension.
- Bulk interchange export freezes a SQLite high-water mark, pages trace rows by
  a stable cursor, streams bodies in fixed-size windows, and atomically publishes
  a sibling temporary file. A concurrent backdated insert is not pulled into an
  in-progress export; a deletion or selection mutation that changes the emitted
  count aborts publication rather than producing a contradictory summary.
- OTAP/Arrow: optional columnar analytics output.
- Raw: exact reconstructed artifact bytes plus metadata manifest.

Every lossy export must report unsupported/dropped fields.

## 15. Versioning and compatibility

- New readers must read every supported older major/minor version.
- Old readers may skip new optional records within the same major version.
- A reader must reject unknown required feature bits.
- Record payloads use schema-numbered canonical encodings; incompatible payload
  changes use a new schema rather than reinterpreting existing bytes.
- The implemented v1 control records use bounded fixed fields and canonical
  varints. Benchmark alternative encodings only behind a new schema/version.
- Pin and record semantic vocabulary versions.
- Before shipping the first post-v1 archive schema/version, publish upgrade
  tools that rewrite rather than mutate archives.
- Keep golden files for every released version.
- Fuzz record framing, lengths, decompression, indexes, manifests, and recovery.

## 16. Security and privacy

- Apply current header/body redaction before content hashing and persistence.
- Never put secrets in logs, migration errors, indexes, or CLI output.
- Preserve directory permission behavior from the existing store.
- Content hashes can reveal equality; document this property.
- FTS and Bloom/trigram indexes may reveal terms and must inherit archive
  permissions and reset/retention behavior.
- Reserve header fields and feature bits for future per-file or per-record
  encryption and key rotation.
- Standalone export must default to the same redaction guarantees as live data.

## 17. Observability

Expose metrics and status for:

- live bytes received versus unique bytes written;
- whole-body and chunk dedup ratios;
- chunker/hash/compression latency;
- append, flush, and SQLite commit latency;
- read time-to-first-byte and reconstruction throughput;
- active file/checkpoint sizes and ages;
- orphan/corrupt/unreachable object counts;
- migration progress, throughput, failures, and ETA;
- GC/repack reclaimed bytes;
- search indexing lag;
- archive files offline or awaiting repair.

LAR failures must not be reported as generic daemon-down state when metadata and
health endpoints remain available.

## 18. Benchmark and conformance corpus

Build an anonymized corpus derived from real shapes, including:

- the 1,277-exchange long Codex session;
- growing repeated conversation prefixes;
- identical client/upstream passthrough bodies;
- moved cache-control markers and dynamic system blocks;
- Claude Code compaction generations;
- retries, failovers, Dario attempts, and simulated responses;
- Pi tool calls/results and subagent branches;
- SSE with split/joined frames, comments, malformed frames, and disconnects;
- repeated and slightly changing ordered header blocks;
- duplicate millisecond timestamps;
- missing/corrupt/truncated legacy gzip files;
- sessions spanning multiple days and pack rotations;
- multimodal/binary and very large tool payloads.

For real-corpus validation, store only hashes, sizes, distributions, and
anonymized/generated replacements in the repository.

## 19. Success criteria

1. Repack the measured representative legacy day to at least 5x smaller.
2. Agent-heavy sessions achieve at least 10x reduction.
3. Storage growth for a repeated-prefix turn is approximately its new bytes
   plus bounded manifest/metadata overhead.
4. Every imported request, upstream request, response, and tool body reconstructs
   to the same length and cryptographic hash as the legacy decompressed bytes.
5. Identical client/upstream bodies resolve to one manifest and one chunk set.
6. Ordered header blocks and stage/attempt ordering round-trip exactly at the
   declared fidelity level.
7. A local sealed archive locates and begins reading a small trace body in under
   10 ms at the agreed percentile and hardware profile.
8. Full body reconstruction reaches a documented throughput target proportional
   to output size.
9. Live capture remains within its latency/error budget during background
   migration and GC.
10. Startup migration is idempotent, resumable, bounded, observable, and leaves
    legacy data untouched until verified cleanup.
11. Active files recover after truncation at every tested byte boundary.
12. Search finds active and sealed records, returns trace anchors, and does not
    decompress duplicate chunks more than once per search.
13. A standalone exported `.lar` is self-contained and passes conformance checks.

Current feature-branch evidence (2026-07-20): criteria 2–6, 8, and 10–13 pass
focused synthetic, production-path, migration, search, and conformance tests.
Criterion 1 passes the deterministic representative corpus at 5.50x
(tool-heavy: 17.20x) but still requires the anonymized real-corpus run.
Criteria 7 and 9 remain rollout gates: criterion 9 now has an ignored
production-path benchmark reporting live proxy error rate and p50/p95/p99 while
the throttled importer, physical GC, and copy-verify-switch-retire repack run concurrently, but its
target-Mac thresholds remain unset. The sealed filesystem open plus 1 KiB body
benchmark is now below 10 ms at 6.10 ms warm p99, but cold-cache evidence on the
agreed Mac hardware profile remains pending. Built-in Linux/macOS cache advice
now stays on the exact descriptor used by the reader but is explicitly not
called cold; the benchmark has an external cache-drop-and-verification helper
mode plus an explicitly configured p99 gate for the controlled hardware run.
Only an external-helper-attested run can report that gate as passing; advisory
cache-control results remain non-acceptance evidence. The documented 500 MiB/s warm
large-body reconstruction target passes at 724.9 MiB/s p95 on this Linux host
with the adaptive large-body profile. The
3.28/3.67/4.48 ms fresh-pack p50/p95/p99 measurement remains a narrower
sequential hot-path result. Exact measurements and the coexistence benchmark
protocol live in `docs/lar-benchmark.md` and
`docs/lar-rollout-performance-gates.md`.

## 20. Implementation tasks

Checkboxes track the current feature branch as of 2026-07-20. Checked means
the task has implementation plus focused tests in this worktree; it does not
mean the branch has shipped. Unchecked tasks remain rollout gates or incomplete
work, even when a narrower prototype exists.

### Phase 0 — decisions and measurements

- [x] Inventory every body/header/artifact write and read path in Alex.
- [ ] Measure the real corpus by artifact kind, whole-body duplication, prefix
      duplication, chunk-size distribution, and session duration.
      The read-only aggregate profiler and real-shape synthetic generator are
      implemented in `scripts/measure-lar-corpus.py` and
      `scripts/generate-lar-corpus.py`; the actual Mac corpus run remains the
      unchecked evidence gate.
- [x] Produce an anonymized synthetic corpus generator.
- [x] Define the exact application-level fidelity boundary.
- [x] Prototype FastCDC and alternative chunkers on the corpus.
- [x] Benchmark zstd settings and static dictionaries.
- [x] Implement the same minimal blob/manifest workload in custom framing and an
      MCAP profile; compare size, recovery, random access, and complexity.
- [x] Decide custom LAR versus MCAP-profile envelope at a recorded design gate.
- [x] Freeze v1 IDs, feature bits, record types, and compatibility rules.
- [x] Write an Architecture Decision Record for global cross-file deduplication.

### Phase 1 — `alex-lar` core crate

- [x] Create an isolated reader/writer crate with no proxy/UI dependencies.
- [x] Implement headers, record framing, checksums, and bounded record parsing.
- [x] Implement independent zstd pages/frames and dictionary descriptors.
- [x] Implement content-defined streaming chunker.
- [x] Implement chunk hashing, equality verification, and append coordinator.
- [x] Implement body manifests and exact reconstruction.
- [x] Implement ordered header atoms/blocks.
- [x] Implement exchange/stage/attempt records.
- [x] Implement stream read/frame indexes.
- [x] Implement normalized conversation entries, generation DAGs, and turn
      views as raw-manifest references without provider parsing in the core.
- [x] Implement checkpoints, sealing, footer indexes, and forward recovery.
- [x] Implement standalone indexes for trace/session/manifest/chunk and
      conversation graph lookup.
- [x] Add malformed input limits and fuzz targets.
- [x] Add golden files and cross-version reader tests.

### Phase 2 — SQLite catalog and live body store

- [x] Add additive LAR catalog schema and migrations.
- [x] Implement rolling combined body/event packs. Body manifests, exchange
      metadata, stages, headers, and stream indexes share the same append-only
      archive without duplicating body bytes into a second event log.
- [x] Implement LAR/SQLite durability ordering and orphan recovery.
      `sync` performs `sync_all`, `batch` performs `sync_data`, and
      shadow-only `best-effort` performs no file-sync call after flush;
      authoritative mode rejects `best-effort`.
- [x] Add unified artifact references and legacy fallback reads.
- [x] Route new request, upstream-request, response, tool, Dario, and fixture
      artifacts through manifests.
- [x] Append live harness tool-call/tool-result events as immutable,
      self-describing child exchanges as well as the separate tool table.
      Call/result stages reuse the one body manifest; strict provenance keeps
      ordinary child-agent lineage out of the parent timeline. Live-only intent,
      startup recovery, reattach/rescan, standalone export/import, retention
      tombstones, and repack preserve occurrence identity without duplicating
      legacy-imported base stages. Body-less phases can be enriched later by
      immutable arguments/result records, and the exact base stage order always
      precedes even backdated supplements.
- [x] Persist ordered trailer blocks when a capture source supplies them; the
      current HTTP proxy stacks expose data frames but not HTTP trailer frames,
      so absent trailers remain explicitly absent rather than fabricated.
- [x] Populate bounded parsed SSE/NDJSON frame ranges on live captures from the
      final upstream content type, alongside the authoritative observed raw
      read index. Frames carry the completion time of their observed read;
      malformed gaps remain raw-only, and frame overflow drops all derived
      annotations without dropping raw reads or body bytes.
- [x] Reuse one manifest for identical stages.
- [x] Move hashing/compression/parsing to bounded blocking workers.
- [x] Add pack rotation and periodic checkpoints.
- [ ] Add the complete live health and storage metric set from section 17.
      The health surface currently reports bounded-worker queue wait/work and
      failure counters. Storage status reports logical/unique bytes, dedup
      ratios, checkpoint/file state, corruption/unreachable counts, migration
      progress, and offline archives. Runtime histograms for
      chunker/hash/compression, append/flush/SQLite commit, read TTFT and
      reconstruction, GC/repack reclamation, and search-index lag remain to be
      instrumented before this item is complete.
- [x] Add feature flags for read, write, dual-write, and fallback modes.

### Phase 3 — legacy importer and startup migration

- [x] Implement a complete legacy artifact inventory.
- [x] Implement shared import library used by CLI and startup migration.
- [x] Add persistent migration jobs, items, lease, cursor, and counters.
- [x] Implement decompression, provenance, fidelity labels, and source fingerprint.
- [x] Implement import → reconstruct → hash/length validation.
- [x] Implement transactional pointer switch only after validation.
- [x] Implement mixed-mode reads throughout migration.
- [x] Start the background migrator after daemon health is available when the
      explicit default-off `lar_startup_migration` setting is enabled.
- [x] Implement configurable worker, batch, I/O, CPU/yield, and disk-pressure
      controls.
- [x] Bound total importer memory independently of archive/corpus index size.
- [x] Surface migration progress, throughput, dedup ratio, ETA, and last error.
- [x] Implement pause/resume/status/verify admin and CLI commands.
- [x] Implement crash, restart, stale lease, and duplicate work recovery.
      A dedicated heartbeat renews an owned job independently of batch size,
      artifact size, codec speed, and I/O throttling, so a valid long-running
      import cannot lose its lease merely because one batch exceeds the lease
      duration.
- [x] Implement missing/corrupt file reporting without data loss.
- [x] Import legacy headers, routing attempts, exchange/stage ordering, linked
      tool stages, and explicitly represented unlinked tools into the same
      combined body/event packs without copying body bytes.
- [x] Give the metadata importer a distinct v2 job namespace and reuse
      validated body-only v1 manifests after legacy gzip cleanup.
- [x] Batch metadata planning and mixed-reader catalog lookups by bounded page,
      preserving request order and a global reconstructed-byte budget.
- [x] Add migration progress to the macOS app.
- [x] Implement dry-run and full-corpus verification reports.
- [x] Implement cleanup eligibility and a separate dry-run/apply workflow.
- [x] Test legacy, dual-write-validated, and LAR-with-fallback upgrade/downgrade
      reopen behavior and document the pre-LAR cleanup boundary.

### Phase 4 — Trace Browser and search

- [x] Read paged transcript bodies through LAR manifests.
- [x] Display stage/attempt timelines and body/header differences.
      The authenticated, bounded, cursor-paged inspector exposes each stage's
      actual ordered duplicate header bytes and raw body bytes through deduped
      content tables, compares adjacent/retry content using bytes as the source
      of truth, and reports truncation plus offline/missing archive states.
- [x] Display compaction/generation/branch events.
- [x] Add raw stream replay with speed controls.
- [x] Build normalized-entry FTS indexing and reverse references.
- [x] Preserve search trace/timestamp anchors.
- [x] Implement raw `lar grep` with unique-chunk scanning.
      The scanner has bounded charged RAM, spills verified decompressed chunks
      to an auto-cleaned temporary file for reuse, carries matches across
      manifest ranges, and fails explicitly on scan/result limits.
- [x] Add an explicit whole-record search mode covering body bytes, safe ordered
      headers/trailers, and canonical exchange/stage control metadata including
      models, providers, routing reasons, and errors. Sensitive header values
      are excluded even for foreign archives that lack Alex's redaction flag;
      structured coverage reports list every searched, excluded, missing, or
      offline category. Body grep remains the default deduplicated byte-search
      path.
- [x] Prototype page Bloom/trigram filters and measure value/size/privacy cost.
- [x] Add offline/archive-missing states without showing daemon down.
- [ ] Run an end-to-end macOS long-session Trace Browser benchmark covering
      initial load, paging, rendering, cancellation, navigation, and stable
      loading/daemon indicators.
      A deterministic 1,277-turn Swift core benchmark and a 1,500-turn
      production-LAR/public-HTTP backend benchmark now cover paging, bounded
      filtering/render preparation, search anchors, cancellation pressure,
      navigation, and health. Packaged-window automation additionally exercises
      body-text discovery, its search spinner, trace-anchored navigation, stale
      page suppression, and stable indicators. This item remains open until
      that packaged benchmark actually passes on the agreed Mac.

### Phase 5 — retention, GC, repair, and archive operations

- [x] Implement reference-aware trace deletion.
- [x] Implement verified mark-and-sweep.
- [x] Implement pack garbage accounting and repack thresholds.
- [x] Implement copy-verify-switch-retire compaction for sealed chunk-only and
      canonical combined packs, including manifests, external cross-pack
      manifest references, ordered headers, stream indexes, repeated/shared
      Stage occurrences, zero-stage exchanges, exact ExchangeMetadata
      companion presence, and the transitive conversation DAG. Planning stores
      the source whole-file identity; copy and switch verify every canonical
      value, perform a final catalog-ownership recheck, atomically move the
      complete graph, and quarantine the source recoverably.
- [x] Implement archive catalog relocation/offline/reattach behavior.
- [x] Implement `lar verify` and non-mutating `lar repair`.
- [x] Implement exact standalone archive packing/import for cataloged LAR
      traces, including ordered stages and headers, all manifest/stream refs,
      ExchangeMetadata, and the transitive conversation generation/entry/turn
      closure. Export uses a sibling temp file and atomic publication; body
      copy/validation uses fixed-size memory windows and a temporary disk spool.
      Legacy-only traces retain their explicitly declared synthesized-fidelity
      path.
- [x] Integrate reset, backup, and restore flows.
- [x] Add interrupted GC recovery tests.
- [x] Add interrupted repack recovery tests.
- [x] Add interrupted rotation recovery tests.

### Phase 6 — replay, converters, and public format

- [x] Implement raw artifact extraction.
- [x] Implement timed and instant stream replay.
- [x] Implement legacy-compatible HAR export with an explicit loss report.
- [x] Implement legacy-compatible WARC export with record IDs, digests,
      references, and an explicit loss report.
- [x] Implement explicitly lossy, legacy-compatible JSONL export/import.
- [x] Implement distinct legacy-compatible OpenTelemetry GenAI and
      OpenInference export adapters with explicit loss reports.
- [x] Derive HAR, WARC, JSONL, OpenTelemetry, and OpenInference export from the
      canonical LAR exchange/stage graph when it exists, including every
      upstream attempt, ordered header/trailer block, stream index, and linked
      tool event instead of reducing modern captures to three legacy bodies.
- [x] Add a raw transaction replay/export that reconstructs method/path,
      ordered stage sequence, headers/trailers, raw artifact bytes, and stream
      timing from one canonical exchange plus strictly validated late tool
      supplements. The RFC 7464 envelope streams canonical live/sealed sources
      into one source-neutral timeline, retains each stage's actual Exchange
      content ID, emits shared bodies once in bounded pieces, explicitly labels
      direct legacy synthesis, validates corruption/truncation and stream
      ranges before replay, and atomically publishes file outputs.
- [x] Stream bulk non-LAR conversion with memory bounded independently of the
      total selected logical body size.
- [x] Evaluate OTAP/Arrow analytics export after the semantic schema stabilizes.
- [x] Publish format specification, MIME proposal, examples, and CLI guide.
- [x] Publish conformance corpus and reader/writer test vectors.
- [x] Publish a non-mutating `lar upgrade INPUT --output OUTPUT` rewrite tool
      before the first post-v1 archive schema/version ships; it must preserve
      supported optional records and verify the rewritten archive.

### Phase 7 — rollout

- [ ] Ship read-only LAR tooling and importer dry-run first.
- [ ] Ship LAR writes behind an opt-in beta flag.
- [ ] Run controlled dual-write and byte/hash comparison during beta.
- [ ] Measure latency, dedup ratio, disk growth, recovery, and migration impact.
- [ ] Enable LAR reads with legacy fallback by default.
- [ ] Enable LAR writes by default only after rollback/downgrade policy is proven.
- [ ] Run automatic background migration with conservative throttles.
- [x] Require full verification before offering legacy cleanup.
- [ ] Keep compatibility reads for the documented support window.
- [ ] Remove legacy write paths only in a later major/minor release with explicit
      migration and downgrade documentation.

## 21. Required test matrix

### Unit/property tests

- [x] Chunk boundaries are deterministic across buffer sizes.
- [x] Manifest reconstruction equals source for arbitrary byte sequences.
- [x] Duplicate chunks are stored once under concurrency.
- [x] Hash collision/equality verification rejects mismatched bytes.
- [x] Ordered duplicate headers round-trip.
- [x] Unknown optional records are skipped.
- [x] Unknown required features are rejected.
- [x] Every prefix truncation recovers only complete valid records.
- [x] Conversation append, branch, mutation, and compaction share existing raw
      entry bytes across footer, checkpoint, and forward recovery paths.
- [x] Unknown provider data round-trips as bounded raw-only conversation
      entries without core parsing.
- [x] Eligible chunk-only and combined canonical repacking preserves chunk and
      logical record IDs, exact ExchangeMetadata presence/data, global
      single-copy cross-pack chunks, and reconstructed body hashes.

### Integration tests

- [x] Live capture and Trace Browser reads during migration.
- [x] Restart during each migration transaction boundary.
- [x] Restart during chunk append, manifest append, seal, and repack.
- [x] Restart after checkpoint sync but before SQLite checkpoint publication.
- [x] SQLite commit failure after successful LAR append.
- [x] Disk-full behavior before and during append.
- [x] Sessions crossing event-log and body-pack rotations.
- [x] Retention with chunks shared by old and retained traces.
- [x] Bodies-only retention clears manifest, header/trailer, and stream-index
      projections from replay/export while preserving conversation entry bytes
      still referenced by a newer retained turn.
- [x] Offline archive detach and reattach.
- [x] Standalone and JSONL export/import into an empty store.
- [x] Legacy fallback after an intentionally failed migration item.
- [x] Legacy metadata/header/attempt/tool import is idempotent, shares existing
      manifests, and survives upgrade from a completed v1 job after gzip
      cleanup.

### Performance tests

- [ ] Write throughput under representative concurrent proxy traffic.
- [ ] Added request latency at p50/p95/p99.
- [x] Unique-byte growth for long repeated-prefix sessions.
- [x] Random trace/body time-to-first-byte with a warm filesystem cache.
- [ ] Random trace/body time-to-first-byte after a controlled cold-cache drop
      on the agreed Mac hardware profile.
- [x] Large body reconstruction throughput.
- [x] Startup recovery with and without a valid footer.
- [ ] Migration throughput and interactive latency under throttle.
- [x] FTS and raw grep latency across active plus sealed archives.
- [ ] Memory use for a 14-day corpus and for pack rotation/repack.

## 22. Definition of done

LAR v1 is complete when:

- the published spec and conformance files match the implementation;
- new live captures use globally deduplicated body chunks;
- all exchange stages and available ordered headers remain inspectable;
- byte-exact artifact reconstruction passes across the validation corpus;
- startup migration safely converts, validates, resumes, and reports legacy
  traces without delaying daemon availability;
- mixed legacy/LAR reads work throughout the compatibility window;
- storage, latency, random-access, recovery, and search success criteria pass;
- retention and cleanup cannot remove chunks still referenced by any trace;
- standalone archives verify and can be re-imported;
- Trace Browser remains responsive on the long-session benchmark;
- rollback, downgrade, repair, and operator documentation are complete.

## 23. Resolved decisions and remaining design gate

Phase 0 resolved the v1 choices with reproducible benchmarks and ADRs:

- native LAR framing, not an MCAP envelope;
- adaptive Gear CDC (512/2 KiB/8 KiB for ordinary bodies and wider boundaries
  for bodies at least 8 MiB);
- independent zstd frames/pages at level 3, with self-contained optional
  dictionaries but no mandatory static body dictionary;
- schema-numbered canonical binary control records rather than CBOR or
  FlatBuffers for v1;
- 512 MiB/one-hour body-pack rotation, 8 MiB/30-second checkpoints, and `sync`
  durability by default;
- SQLite FTS plus exact unique-chunk raw grep for v1. Portable Bloom search
  pages remain optional until a real multi-file corpus justifies their privacy
  and roughly 3% storage cost.

Portable search indexes are explicitly not included by default in standalone
v1 exports. Reconsider them only in a post-v1 optional feature after the real
multi-file corpus demonstrates enough avoided I/O to justify their privacy and
storage cost; this is not a v1 container-design gate. The compatibility window
is resolved operationally in `docs/lar-operations.md`: at least two subsequent
minor releases and 90 days after default LAR writes, whichever is longer;
pre-LAR downgrade requires retained gzip files or restoration from
quarantine/backup.

These choices do not change the central invariants: raw bytes live in the
content-addressed chunk store, exchanges preserve ordered stage/header history,
and legacy migration must validate before switching pointers or deleting data.
