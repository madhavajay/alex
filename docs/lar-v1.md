# LAR V1 implementation

`alex-lar` implements the reliability slice from `docs/lar-format.md` and the
V1 roadmap. It is deliberately a raw-body archive first, not the final
conversation-deduplication format.

## What is implemented

- Append-only, checksummed `LAR1` records containing raw body bytes, body kind,
  trace ID, payload hash, optional SSE timing, and sanitized-fixture state.
- A sorted fixed-width footer index keyed by `(trace ID, body kind)`. Normal
  readers binary-search this index in place and retain zero index bytes in
  memory. Body reads are individually size-bounded and streamed.
- Footerless crash recovery by a forward record scan. A complete corrupt record
  is rejected; only an incomplete trailing record is discarded when the next
  writer opens the archive.
- Resumable import of Alex's existing `bodies/**/*.gz` files. Checkpoints are
  atomically replaced only after the corresponding archive record is synced.
  Repeated or resumed imports compare hashes and do not duplicate records.
  Source gzip files are opened read-only and validation completes before the
  checkpoint is marked complete. The importer never deletes originals.
- Selection-based replay-fixture export. JSON and strictly framed JSON SSE
  credential fields are redacted, common credential patterns are checked after
  redaction, and opaque/raw bodies are rejected rather than copied under a
  misleading sanitized label. The output is a normal independently verifiable
  LAR file and its bodies can be replayed through the same random-read API.
- `alex-lar inspect`, `read`, `verify`, `import-bodies`, and `export-fixture`
  commands.

The fixed-width on-disk index is capped at one million body records per file.
Recovery memory is therefore bounded even for an unclosed file; normal reads
are lazy. Individual body allocation/streaming is also guarded by an explicit
maximum.

## Format 1 wire layout

All integers are little-endian. The file is self-contained and has four
regions:

| Region | Shape | Recovery role |
| --- | --- | --- |
| File header | 16 bytes: `LAR1`, version, header length, reserved | Rejects non-LAR and future incompatible versions |
| Body records | 32-byte `LREC` header, bounded JSON metadata, raw payload | Header, metadata, and payload have independent checksums; metadata also records payload SHA-256 |
| Footer index | 24-byte `LIDX` marker followed by sorted 48-byte entries | The marker lets recovery distinguish a partially written derived index from record corruption |
| Footer | 48-byte `LFTR` structure | Records index offset/count plus checksums for the footer and complete index |

An index entry stores `SHA-256(trace_id + NUL + body_kind)`, record offset, and
record length. A lookup hashes the requested pair and binary-searches entries
directly from disk. The selected record metadata is then checked before its
payload is streamed. A missing footer triggers a bounded forward scan of
`LREC` headers; a trailing incomplete `LREC` or `LIDX` is safely truncated by
the next writer, while checksum failure in a complete record is corruption.

## Production store seam

`alex-store` adds nullable `req_body_lar`, `upstream_req_body_lar`, and
`resp_body_lar` columns alongside the legacy paths. Existing databases migrate
additively; trace refreshes preserve validated pointers. Store body resolution
prefers LAR, enforces an explicit byte limit, and falls back to the preserved
gzip when an archive is unavailable or corrupt. The body, transcript, text
search, fixture-capture, and body-inclusive export handlers use that resolver.

`alex traces migrate-lar [--max-entries N]` is an offline, resumable migration:

1. Candidates come from authoritative SQLite trace columns, never filename or
   directory discovery. Unreferenced files are ignored.
2. Each gzip is streamed into the archive and the archive is synced before an
   atomic checkpoint advances. `--max-entries` permits controlled batches.
3. No SQLite pointer changes during a partial run.
4. After all selected bodies compare byte-for-byte with their originals, every
   selected pointer changes in one SQLite transaction. If any row changed
   concurrently, the transaction is rolled back.
5. Original paths and files remain in place for fallback and rollback.

Trace summary queries are capped at 5,000 rows and support `offset`, while body
responses are capped at 64 MiB. The archive reader retains no footer index in
memory. Footerless recovery and the append writer may retain fixed-width index
entries, but both reject more than one million entries per archive; daily
rotation must remain below that explicit bound.

Live proxy capture still writes gzip. It is intentionally not dual-written in
this slice: a long-lived archive writer would expose a footerless file to
readers, while reopening/closing for every body rewrites an O(n) index, and
remote trace ingestion can replace the same `(trace, body kind)` with new
content. Enabling live append before defining rotation, lock ownership, and
replacement semantics would allow a SQLite pointer to reference stale bytes.
The offline path proves sync-before-pointer ordering without taking that risk.

## Explicitly deferred

- Sequence generations, divergence patches, message-level deduplication, and
  shared dictionaries.
- Live proxy capture, tool-call body pointers, and archive file compaction or
  per-day retention deletion.
- Web UI virtualization. Summary APIs are bounded and offsettable, but the
  clients still need to request and render pages.
- HAR conversion. V1 replay fixtures are LAR files; HAR remains a later
  interoperability feature.
- Corpus compression targets and cold-cache latency benchmarks against the
  private 9.4 GB capture.

Run `cargo test -p alex-lar` for interrupted-write, corruption, random-access,
resumable-import, sanitized-replay, and 55,000-record lazy-index coverage.
