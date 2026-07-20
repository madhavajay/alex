# LAR complete-transaction JSON sequence v1

`alex lar transaction` is a byte-authoritative interchange envelope for one
captured transaction. It is not a second body store and does not replace the
`.lar` container. A live export reads canonical manifests/chunks directly from
the catalog packs; a sealed export reads the archive directly. Each referenced
logical body is emitted once even when retries or several stages share it.

The media is an RFC 7464 JSON Text Sequence. Every record begins with byte
`0x1e` and ends with LF. The format marker is
`alex-lar-transaction-json-seq`, version `1`.

## Record order

1. `format` declares the version, trace, fidelity, decoded artifact-piece
   limit, byte encoding, and limitations.
2. `transaction_timeline` names the immutable base Exchange content ID but
   separately lists the projected ordered Stage content IDs. It never claims
   that the base Exchange hashes the flattened supplement list. Strict
   `alex.tool-supplement.v1` child exchanges are listed with their own Exchange
   and Stage content IDs; ordinary children/subagent lineage are not merged.
3. `stage` records appear in transaction order. `timeline_ordinal` is the
   flattened projection order. `exchange_content_id`, `exchange_ordinal`, and
   `ordinal_within_exchange` identify the actual immutable source Exchange.
   `occurrence_id` is the source-neutral `(exchange_content_id,
   ordinal_within_exchange)` identity, while `content_id` is the immutable
   Stage content ID. Repeated equal Stage records therefore share `content_id`
   but retain distinct occurrence identities without exposing a live
   catalog-only row ID.
   Records retain timing, attempts, routing, provider/model/account selection,
   status, usage, cost, errors, header/trailer references, body references, and
   stream-index references.
4. Deduplicated `header_block` records preserve ordered atoms, duplicate names,
   original retained name/value bytes, capture flags, and fidelity.
5. Deduplicated `stream_index` records preserve observed read ranges and
   independently parsed frame ranges with deltas from first byte.
6. Each body is emitted once as `artifact_start`, original manifest
   `artifact_range` descriptors, bounded `artifact_bytes`, and a verified
   `artifact_end`. Binary bytes use base64; textual metadata uses a UTF-8 string
   or an explicit base64/length object.
7. A single `end` record carries counts and `complete: true`.

The raw artifact bytes and their BLAKE3 identity are authoritative. Normalized
metadata is descriptive and must never override them. Consumers must reject a
missing `end`, records after `end`, overlapping artifacts, non-contiguous body
pieces, invalid base64, length/hash mismatches, or an unsupported format
version.

## Replay

`alex lar transaction-replay FILE` validates the complete sequence before
replay. Raw mode emits the exact recorded observed-read byte ranges; `--parsed`
emits the exact recorded SSE/NDJSON frame ranges. Ranges must be non-empty,
non-overlapping, in bounds, and have monotonic timing. Raw observed reads must
be contiguous and cover the selected artifact. Parsed ranges may skip comments,
separators, or malformed raw gaps. `--speed
instant|0.25x|0.5x|1x|2x|4x` controls delays and defaults to `instant`. No
HTTP/1 text, HTTP/2 frames, SSE delimiters, or provider framing is invented.

File replay uses a synced sibling temporary file and atomic publication, so a
validation/replay failure does not truncate the requested destination. Without
`--force`, an existing destination is never replaced. Standard-output replay
cannot roll back bytes after an output-device failure.

## Bounds

- Decoded `artifact_bytes` payload: at most 48 KiB per record.
- Consumer JSON record limit: 32 MiB.
- Export stage count: at most 100,000.
- Export distinct artifact count: at most 100,000.
- Canonical source memory: one configured LAR source chunk plus one 48 KiB
  output piece; never the complete logical body.
- Legacy source memory: gzip/LAR streaming buffers plus one 48 KiB output
  piece. Legacy bytes are read once to establish identity and once to emit and
  re-verify them; no temporary standalone LAR or whole-body `Vec` is built.

## Fidelity limits

The envelope replays the application bytes Alex captured. It cannot recover
TCP segmentation, TLS records, HTTP/2 or HTTP/3 frames, connection scheduling,
or secrets redacted at capture. It does not invent missing transport details.
Canonical headers/trailers retain their recorded fidelity; legacy-only rows are
explicitly labeled `synthesized_legacy`, use content IDs synthesized from the
retained bytes, and may lack original header order/casing, duplicate headers,
trailers, attempts, or stream timing.

The envelope contains the ordered metadata, identities, and exact bytes needed
by a byte-preserving importer or application-level replay engine. The current
CLI consumer implements stream replay; it does not open a network connection or
install the transaction into the live catalog.
