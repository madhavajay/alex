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
- Selection-based replay-fixture export. JSON credential fields are redacted,
  the output is a normal independently verifiable LAR file, and its bodies can
  be replayed through the same random-read API.
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

## Existing-store migration seam

The existing `alex-store` SQLite schema still owns `req_body_path`,
`upstream_req_body_path`, and `resp_body_path`, and the proxy writes those gzip
files synchronously while finalizing a trace. Replacing those paths safely
requires a schema migration that atomically commits `(lar_file, body_kind)`
pointers with the trace row and keeps dual-read support during rollback.

This branch intentionally does **not** silently switch that production write
path. The safe next seam is:

1. Add nullable `lar_file`/LAR-version columns while retaining all legacy path
   columns.
2. Add a store body-reader abstraction that resolves LAR first and legacy gzip
   second, then move every proxy/resume/Trace Browser caller to it.
3. Run `alex-lar import-bodies`, validate every source/archive hash, and update
   SQLite pointers in one transaction. Do not remove legacy paths or files.
4. Enable LAR writes behind a configuration gate, with the archive record
   synced before committing its SQLite pointer.
5. Remove originals only in a later explicit retention operation after a
   second validation pass and rollback window.

Until that seam lands, `alex-lar import-bodies` is an opt-in compatibility and
fixture tool and cannot compromise current trace capture.

## Explicitly deferred

- Sequence generations, divergence patches, message-level deduplication, and
  shared dictionaries.
- SQLite pointer migration and live proxy capture.
- Trace-summary/search API pagination and web UI virtualization; LAR provides
  the lazy body primitive but does not by itself make those surfaces lazy.
- HAR conversion. V1 replay fixtures are LAR files; HAR remains a later
  interoperability feature.
- Corpus compression targets and cold-cache latency benchmarks against the
  private 9.4 GB capture.

Run `cargo test -p alex-lar` for interrupted-write, corruption, random-access,
resumable-import, sanitized-replay, and 55,000-record lazy-index coverage.
