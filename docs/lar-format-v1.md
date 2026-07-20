# LAR v1 implemented container format

Status: implementation reference for `alex-lar` v0.1.25

This document describes the bytes accepted and emitted by the current
`alex-lar` crate. It is deliberately narrower than the design in
`lar-format.md`: undocumented proposed records are not part of this v1 wire
contract.

All integers are unsigned little-endian unless explicitly stated. Lengths count
bytes. Checksums are IEEE CRC-32 values stored as little-endian `u32` values.
Content identifiers and chunk digests are BLAKE3 digests over the canonical
bytes described below.

## Container

A file consists of one header followed by zero or more records. Active files
may end in an append-only checkpoint plus locator record. Sealed files end in a
checkpoint plus a fixed footer trailer:

```text
+-------------------------+
| file header             |
+-------------------------+
| record frame            |
+-------------------------+
| record frame            |
+-------------------------+
| ... canonical records   |
+-------------------------+
| optional index blocks   |
+-------------------------+
| optional checkpoint     |
+-------------------------+
| locator record OR       |
| fixed footer trailer    |
+-------------------------+
```

`ArchiveReader` first probes the fixed footer and fixed-size checkpoint locator.
A valid one loads bounded persisted indexes and seeks directly to referenced
metadata records; it does not scan chunk records. `ArchiveReader::open_path()`
reports `Footer`, `Checkpoint`, or `ForwardScan` for instrumentation. Missing
indexes fall back to a bounded forward scan. A truncated canonical record is a
recoverable truncated tail; checksum corruption is an error. A corrupt derived
index/footer falls back to canonical records but reports
`CorruptIndexFallback`, never `Clean`, and cannot be reopened for append until
repaired.

Checkpoints and index blocks are derived, optional records. They never own body,
header, stream, stage, exchange, or conversation-graph bytes. Active checkpoints are append-only:
later canonical records may follow an old locator, and a new checkpoint at EOF
supersedes it. `seal()` writes the footer trailer and permanently prevents
append. A crash after an index block/checkpoint but before its locator/footer is
reported as an incomplete derived-index tail.

### File header envelope

```text
offset  width  field
0       4      ASCII `LAR1`
4       2      container major
6       2      container minor
8       4      header payload length N
12      N      header payload
12+N    4      CRC-32(prefix || payload)
```

The v1 payload is:

```text
width       field
16          file UUID (opaque bytes)
1           file role
8           creation wall-clock time, nanoseconds
2 + N       writer byte string (`u16` length, then bytes)
8           required feature bits
8           optional feature bits
1           default hash algorithm
1           zstd level encoded as the two's-complement byte of an `i8`
2           dictionary descriptor count D
repeated D times:
  32        dictionary ID
  8         uncompressed dictionary length
  2 + N     dictionary name (`u16` length, then bytes)
remaining   minor-version extension bytes, ignored by this reader
```

File-role codes are `1` body pack, `2` event log, `3` standalone, `4` search
pack, and `5` dictionary. The only hash-algorithm code is `1` for BLAKE3.
Descriptors identify self-contained metadata-page dictionaries. The body chunk
codec remains independently zstd-compressed without a shared dictionary.

The current reader requires major version `1`, accepts any minor version, and
skips remaining bytes in the bounded header payload. It supports required bit
`0x1` for archive-set body-manifest references and required bit `0x2` for
conversation-entry, generation, and turn-view records. A writer must set bit
`0x2` before emitting any of those records. The reader rejects every unknown
required bit and ignores optional feature bits. A future minor writer must use
optional bits for behavior an old reader can safely ignore, and a required bit
for behavior needed to interpret existing data.

### Record envelope

```text
offset  width  field
0       4      ASCII `LREC`
4       2      record type
6       2      record schema version
8       4      flags
12      8      payload length N
20      N      record payload
20+N    4      CRC-32(prefix || payload)
```

Flag bit 0 (`0x00000001`) means required. Other flag bits are currently
preserved by the frame reader but have no semantics.

Implemented record types are:

| Code | Name | Implemented schema |
| ---: | --- | ---: |
| 1 | chunk | 1 |
| 2 | body manifest | 1 |
| 3 | header block | 1 |
| 4 | stream index | 1 |
| 5 | stage | 1 |
| 6 | exchange | 1 |
| 7 | index block (optional derived metadata) | 1 |
| 8 | checkpoint root (optional derived metadata) | 1 |
| 9 | checkpoint locator (optional derived metadata) | 1 |
| 10 | compression dictionary bytes | 1 |
| 11 | compressed canonical metadata page | 1 |
| 12 | conversation entry | 1 |
| 13 | conversation generation | 1 |
| 14 | turn view | 1 |
| 15 | exchange metadata (optional companion) | 1 |

Canonical record types 1–6 and 10–14 are written with the required flag.
Metadata pages currently batch body manifests, header blocks, stream indexes,
stages, exchanges, conversation entries, generations, and turn views; a reader
that does not understand them must reject rather than silently lose canonical
records. Type 15 is deliberately an optional outer frame and is never placed
inside a metadata page; its compatibility and adjacency rules are defined
below.
Dictionary records are required whenever later pages reference them. Derived
index types 7–9 are written without it, so v1 readers predating persisted
indexes can skip them and reconstruct the same capture. An unknown record type
is rejected when required and skipped when optional. A known or unknown record
with a schema other than 1 is rejected when required
and skipped when optional. An optional unknown record with schema 1 is counted
by `ArchiveReader::record_count`, but does not produce an exposed index entry;
an optional future-schema record is not counted. Callers must not treat
`record_count` as a count of logical bodies or exchanges.

## Metadata pages and dictionaries

Type 10 stores `LDI1`, its 32-byte BLAKE3 content ID, a `u32` byte length, and
the exact dictionary bytes. The file header carries the matching descriptor;
the reader validates descriptor length and identity before accepting a page
reference.

Type 11 stores `LMP1`, compression code (`1` plain zstd or `2` zstd with a
32-byte dictionary ID), reserved zero bytes, uncompressed/compressed `u64`
lengths, inner-record count, optional dictionary ID, and compressed bytes. The
uncompressed payload is a bounded sequence of ordinary `LREC` canonical body
manifest, header block, stream index, stage, exchange, conversation entry,
generation, and turn-view frames. Each page is independently decompressible.
Inner frame CRCs, required flags, schemas, counts, IDs, dependency references,
and page lengths are validated normally in stored order. A checkpoint may map
IDs of different metadata types to one page offset; the fast reader decompresses
that page once and resolves each ID. Generation ancestry is validated in the
inner frame order, even though all records in the page share one outer offset.

`ArchiveWriter::enable_metadata_pages()` opts a writer into mixed plain-zstd
pages. Live Alex body/event packs enable this mode after creation or recovery.
`create_with_metadata_dictionary()` writes a self-contained dictionary record
before any referencing page. Existing unpaged v1 files remain readable, and a
new reader may mix paged and direct metadata records while recovering an
append.

## Persisted indexes and footer

Type 7 payloads start with `LIDX`, schema `u16`, kind `u8`, reserved zero `u8`,
and entry count `u32`. Blocks are deterministically sorted and sharded below
the configured maximum frame payload. Kind codes are `1` chunk, `2` manifest,
`3` header block, `4` stream index, `5` stage, `6` exchange, `7` trace, `8`
session, `9` conversation entry, `10` generation, `11` turn view, and `12`
turn trace. Chunk entries contain their hash, frame offset, uncompressed length,
and compressed length. Manifest/header/stream/stage/exchange/conversation-entry/
generation/turn-view entries contain a 32-byte ID and frame offset. Trace and
turn-trace entries map bounded byte-string IDs to exchange and turn-view IDs,
respectively. Session entries map bounded byte-string IDs plus a contiguous
`u32` start index to ordered exchange IDs, allowing one large session to span
blocks.

Type 8 payloads start with `LCP1`, schema `u16`, zero flags `u16`, the end offset
and record count represented by the snapshot, then bounded block references.
Each reference carries kind, frame offset, frame length, and BLAKE3 payload
hash. The root therefore authenticates every block without duplicating any
canonical bytes.

Type 9 has a fixed 56-byte payload beginning `LCPT`; it contains schema, zero
flags, checkpoint frame offset/length, and checkpoint payload hash. Including
the record envelope it is exactly 80 bytes, so an active reader can probe EOF
without scanning. It remains a normal `LREC`, permitting future appends.

The sealed footer trailer is exactly 72 bytes:

```text
8   `LARFOOT1`
2   schema (1)
2   zero flags
8   checkpoint frame offset
8   checkpoint frame length
32  checkpoint payload BLAKE3
4   CRC-32 of the preceding 60 bytes
8   trailing `LAR1END!` magic
```

Index frame CRCs, block/root hashes, offsets, lengths, IDs, cross-references,
and configured count limits are validated before the fast path is exposed.
Unknown future optional index records remain safe to skip. Any future index
change required to interpret canonical data must instead use a required feature
bit or a new container major version; an index-only optimization uses an
optional record/schema and must retain forward-scan equivalence.

## Primitive encodings

```text
hash          = algorithm:u8 || digest:[u8;32]
bytes         = length:u32 || value:[u8;length]
optional      = 0:u8 | 1:u8 || bytes
uvarint       = canonical unsigned LEB128, at most 10 bytes
event_bytes   = length:uvarint || value:[u8;length]
event_optional= 0:u8 | 1:u8 || encoded value
```

Optional tags other than 0 and 1 are invalid. Record payload decoders reject
trailing bytes, unlike the file header where bounded trailing bytes are the
minor-version extension mechanism.

## Type 1: chunk, schema 1

```text
hash                 hash of the uncompressed chunk
uncompressed_length  u64
compression          u8; only 1 (zstd) is accepted
compressed_length    u64
compressed_bytes     [u8; compressed_length]
```

Decompression is bounded to `uncompressed_length + 1`. Reading a body verifies
the decompressed length and BLAKE3 hash before exposing those bytes.

## Type 2: body manifest, schema 1

```text
manifest_id          [u8;32]
total_length         u64
whole_body_hash      hash
media_type           optional bytes
content_encoding     optional bytes
chunk_count          u32
repeated chunk_count times:
  chunk_hash         hash
  chunk_offset       u64
  logical_offset     u64
  length             u64
```

The manifest ID is BLAKE3 over every field after `manifest_id`, using the exact
canonical encodings above. References must be contiguous and ordered from
logical offset zero, and their lengths must cover `total_length` exactly. When
reconstructing a body, the reader also verifies the complete body length and
`whole_body_hash`.

The ordinary byte-slice writer stores `media_type` and `content_encoding` as
absent; `append_body_with_metadata` retains those fields while still reusing
content-addressed chunks. Both order every newly written literal chunk before
its manifest. The predecessor-
aware path may use a nonzero `chunk_offset` to reference an exact byte range in
an earlier chunk. Those references remain direct: a manifest never names or
depends on another manifest. The writer validates every range against the
stored chunk length and the ordinary reader reconstructs and whole-body-hash
checks it without a special delta path.

## Type 3: header block, schema 1

```text
header_block_id      [u8;32]
fidelity             u8
atom_count           u32
repeated atom_count times:
  original_name      bytes
  value              bytes
  flags              u32
```

Fidelity codes are:

| Code | Meaning |
| ---: | --- |
| 0 | exact order and casing |
| 1 | legacy order unknown |
| 2 | legacy casing unknown |
| 3 | legacy order and casing unknown |

The header-block ID is BLAKE3 over `fidelity || atom_count || atoms`, using the
canonical encodings above. Atoms are an ordered list rather than a map, so
duplicate header names, original casing, and order survive when fidelity is
`exact`. Atom flag bit `0x1` means the value was redacted at capture time. Alex's
HTTP libraries expose duplicate value order but normalize field-name casing, so
live proxy blocks use fidelity code `2`. Headers reconstructed from legacy JSON
maps use code `3` and never claim exact order or casing.

## Type 4: stream index, schema 1

A stream index owns no body or frame bytes. It addresses byte ranges in one
existing raw body manifest:

```text
stream_index_id       [u8;32]
raw_body_manifest_id  [u8;32]
read_count            uvarint
repeated read_count times:
  offset_delta        uvarint; from the previous read's start (zero initially)
  byte_length         uvarint
  time_delta_ns       uvarint; from the previous read time (zero initially)
frame_count           uvarint
repeated frame_count times:
  offset_delta        uvarint; from the previous frame's start (zero initially)
  byte_length         uvarint
  time_delta_ns       uvarint; from the previous frame time (zero initially)
  parser              uvarint, constrained to u16
  frame_kind          uvarint, constrained to u16
```

Parser codes are `0` opaque, `1` SSE, and `2` NDJSON. Frame-kind codes are `0`
opaque, `1` SSE event, and `2` NDJSON record. Other `u16` values round-trip as
unknown values so a minor producer can add parse annotations without losing
the raw source.

The stream-index ID is BLAKE3 over every field following the ID. Reads must be
non-empty, contiguous from offset zero, cover the raw manifest exactly, and
have nondecreasing timing. Parsed frames must be non-empty, ordered,
non-overlapping, within the same manifest, and have nondecreasing timing.
Frames may leave gaps because delimiters or malformed regions still remain in
the raw manifest. This supplies observed read-boundary replay and parsed-frame
navigation while retaining exactly one body.

Alex's live producer records the boundaries yielded by the upstream HTTP
client, not TCP/TLS packet boundaries. The first observed read has time zero;
later reads use monotonic deltas from it. For final upstream responses labeled
`text/event-stream`, `application/x-ndjson`, or `application/ndjson`, the live
producer adds ranges for complete JSON/`[DONE]` SSE events or valid NDJSON
records. A range receives the delta of the observed read that completed it, so
events split across reads and multiple events joined in one read retain honest
timing. Comments, delimiters, malformed records, and other unparsed gaps remain
only in the authoritative raw replay; the index never copies their bytes or
invents semantic payloads.

Live timing and parsed-frame capture are each capped at 65,536 entries. Read
overflow omits the stream index because complete raw timing coverage is no
longer available. Parsed-frame overflow instead discards all derived frame
annotations while preserving the complete observed-read index and exact body.
Buffered translation does not create another upstream body copy: its timing
index references the upstream manifest, while any different bytes sent to the
client use their own manifest.

`ArchiveReader::read_stream_replay` reconstructs that manifest once and returns
a zero-copy schedule over either observed reads or parsed frames. Schedule
timing is instant, original, or a checked rational speed multiplier; the
blocking player flushes after every event so read boundaries remain externally
observable. `alex lar replay` exposes the same modes and deliberately defaults
to instant for long captures.

`ArchiveReader::read_body_range` and `read_body_ranges` are the bounded UI/API
primitives. They validate logical ranges against the manifest, verify and cache
only touched chunks, and do not read or decompress untouched chunks. Because a
partial range cannot prove the manifest's whole-body BLAKE3, callers requiring
that proof still use `read_body`; range reads retain manifest-structure and
per-chunk hash verification.

Alex's Trace Browser backend resolves a trace and stage through
`lar_stage_records`, opens its active or sealed archive, pages either observed
reads or parsed frames by event cursor, and reconstructs only that page's byte
ranges. It returns absolute observed deltas and performs no server-side sleep;
playback speed is a presentation concern. Catalog-only live manifests use the
same bounded algorithm across body packs, caching each touched chunk once per
page. Offline/missing archives remain typed retryable states.

## Type 5: stage, schema 1

A stage is an immutable event in capture order. All headers, trailers, request
bodies, response bodies, and streams are 32-byte content-ID references; the
stage payload has no field capable of embedding those bytes.

```text
stage_id                    [u8;32]
kind                        uvarint, constrained to u16
attempt_number              event_optional<uvarint u32>
wall_time_ns                uvarint
monotonic_delta_ns          event_optional<uvarint>
first_byte_delta_ns         event_optional<uvarint>
last_byte_delta_ns          event_optional<uvarint>
request_headers_ref         event_optional<[u8;32]>
request_body_manifest_ref   event_optional<[u8;32]>
response_headers_ref        event_optional<[u8;32]>
response_body_manifest_ref  event_optional<[u8;32]>
trailers_ref                event_optional<[u8;32]>
stream_index_ref            event_optional<[u8;32]>
provider                    event_optional<event_bytes>
requested_model             event_optional<event_bytes>
routed_model                event_optional<event_bytes>
account_id                  event_optional<event_bytes>
routing_reason              event_optional<event_bytes>
status_code                 event_optional<uvarint u16>
usage                       0:u8 | 1:u8 || four uvarints
cost_nanos                  event_optional<uvarint>
cost_currency               event_optional<event_bytes>
error_class                 event_optional<event_bytes>
error_message               event_optional<event_bytes>
```

Usage fields, in order, are input, output, cached, and reasoning tokens. Cost
is an integer number of one-billionths of the named currency to avoid a
floating-point wire representation. The stage ID is BLAKE3 over every
following field.

Stage kind codes are:

| Code | Kind |
| ---: | --- |
| 1 | client request |
| 2 | normalized request |
| 3 | router decision |
| 4 | retry decision |
| 5 | failover decision |
| 6 | upstream request |
| 7 | upstream response |
| 8 | upstream failure |
| 9 | client response |
| 10 | client trailers |
| 11 | tool call |
| 12 | tool result |
| 13 | auth refresh |
| 14 | account routing |
| 15 | Dario request |
| 16 | Dario response |
| 17 | injected response |
| 18 | cancellation/client disconnect |

Other `u16` kind codes round-trip as unknown. Upstream request, response, and
failure stages require an attempt number. If both byte timings are present,
last byte must not precede first byte. Header and stream references resolve to
earlier records. Body-manifest references do too in standalone files; a body
pack with required feature bit `0x1` may resolve them through its archive-set
catalog. A stage with a stream index must use that index's raw manifest as its
response-body manifest, preventing a reassembled second stream body.

## Type 6: exchange, schema 1

```text
exchange_id          [u8;32]
trace_id             event_bytes
session_id           event_optional<event_bytes>
run_id               event_optional<event_bytes>
parent_trace_id      event_optional<event_bytes>
capture_sequence     uvarint
wall_time_ns         uvarint
monotonic_delta_ns   event_optional<uvarint>
clock_id             event_optional<event_bytes>
stage_count          uvarint
stages               repeated [u8;32] stage IDs
```

The exchange ID is BLAKE3 over every following field. Trace IDs are non-empty,
and one trace ID identifies exactly one exchange within an archive. Stage IDs
must resolve to earlier records and their encoded order is authoritative; the
reader never sorts them by timestamps. The reader obtains direct trace lookup
and append-ordered session-to-exchange lookup tables from a validated persisted
index, or rebuilds them during its bounded recovery scan.
Session, run, parent, and clock identifiers are optional byte strings rather
than assumed UUIDs so existing Alex identifiers survive migration exactly.

### Alex tool-supplement application profile

Late harness events do not mutate an already-published Exchange. Alex appends
one immutable child Exchange per published phase, containing exactly one
`tool call` or `tool result` Stage and reusing a previously published
body-manifest ID. The semantic phases are `start` and `end`. If an initially
body-less callback is enriched later, immutable `arguments` and `result`
phases add the newly available manifest instead of rewriting the earlier
records. The Stage `routing_reason` is UTF-8 JSON with schema
`alex.tool-supplement.v1`; it carries the phase, original tool row/call ID,
harness, name, source trace, and start/end/error/exit metadata required to
rebuild the SQLite projection. The child has the same non-empty session ID as
its canonical parent and a deterministic internal trace ID derived from
harness, session, tool-call ID, and phase.

`parent_trace_id` alone is general trace/subagent lineage and never proves that
a child is a supplement. Consumers must validate the versioned provenance,
deterministic ID, session, one-stage cardinality, and phase/stage-kind match.
The canonical view keeps the exact base Exchange first, then orders validated
supplements by stage wall time, capture sequence, the phase order `start`,
`arguments`, `end`, `result`, and internal trace ID. A result callback always
proves a preceding tool-call occurrence even when no argument body was
captured. The entire `lar-tool-` trace-ID prefix is reserved and malformed or
unknown-phase records fail closed. Unknown ordinary child exchanges remain
separate lineage and must not be folded into replay/export.

This profile adds no v1 record type: old readers retain the child Exchange and
Stage as ordinary canonical records. Alex's supplement/tool tables are derived
indexes and can be rebuilt from the archive. Explicit retention tombstones
prevent deleted immutable children from being resurrected by a later rescan.

## Type 15: exchange metadata companion, record schema 1

This optional extension carries transport, accounting, and Alex trace metadata
that is not part of the stable, content-addressed Type 6 exchange. When present,
it is the frame immediately following a direct (non-metadata-page) Exchange:

```text
exchange_id          [u8;32]
payload_magic        [u8;4] = ASCII `LEM1`
payload_schema       u16 = 1
attribute_count      u16, maximum 128
repeated attribute_count times:
  flags              u8; bit 0 means required
  reserved           u8 = 0
  key_length         u16
  value_length       u32
  key                 key_length bytes
  value               value_length bytes
```

Keys are non-empty, strictly byte-sorted, and unique. A key is at most the
smaller of 128 bytes and the configured identifier limit; a value is bounded by
the configured field-length limit. Known value encodings are:

| Encoding | Keys |
| --- | --- |
| little-endian two's-complement `i64` | `alex.ts_request_ms`, `alex.ts_response_ms`, `http.status`, `gen_ai.thinking_budget`, `gen_ai.input_tokens`, `gen_ai.cached_input_tokens`, `gen_ai.cache_creation_tokens`, `gen_ai.output_tokens`, `gen_ai.reasoning_tokens` |
| little-endian `u64` containing exact IEEE-754 bits | `alex.cost_usd_f64_bits` |
| one byte, `0` or `1` | `http.streamed`, `alex.substituted`, `alex.injected`, `alex.via_dario` |
| uninterpreted bytes | `alex.harness`, `alex.client_format`, `alex.upstream_format`, `http.method`, `http.path`, `alex.billing_bucket`, `alex.error.kind`, `alex.error.code`, `alex.original_model`, `alex.served_model`, `alex.substitution_reason`, `alex.fixture_name`, `alex.attempts_json`, `alex.original_account_id`, `alex.served_account_id`, `alex.subscription_identity`, `alex.dario_generation`, `alex.tags_json`, `network.client_ip`, `alex.key_fingerprint`, `gen_ai.reasoning_effort` |

All attributes written by this version are optional. Unknown optional
attributes must use canonical lowercase ASCII keys containing only letters,
digits, `.`, `_`, and `-`; their values are retained across decode/re-encode.
Unknown required attributes are rejected. Unknown extension keys containing a
`body`, `header`, `manifest`, `chunk`, `ref`, or `path` token (including the
listed plurals) are rejected: body bytes, ordered header atoms, manifests, and
their references remain solely on the ordinary stage/header/manifest graph.

The outer Type 15 frame is written with flags `0`, never `required`, so shipped
v1 readers that know only Types 1–14 skip it while retaining the complete older
exchange semantics. Current readers consume at most one immediate companion
and reject an orphan, duplicate, or exchange-ID mismatch. A future optional
outer schema or optional outer flags are skipped; setting the outer required
flag is rejected. Footer/checkpoint readers find the companion by seeking just
past the indexed direct Exchange, so no new required index kind is introduced.
The upgrade operation copies optional companion frames, including unknown
future forms, in canonical order rather than silently dropping them.

## Type 12: conversation entry, schema 1

A conversation entry is a normalized label plus one or more ranges into raw
body manifests. It cannot embed prompt, message, summary, or tool-result bytes:

```text
conversation_entry_id  [u8;32]
semantic_schema        uvarint, constrained to u16
role                   uvarint, constrained to u16
kind                   uvarint, constrained to u16
raw_range_count        uvarint
repeated raw_range_count times:
  manifest_id          [u8;32]
  byte_offset          uvarint
  byte_length          uvarint
name                    event_optional<event_bytes>
tool_call_id            event_optional<event_bytes>
```

The entry ID is BLAKE3 over every following field. Every entry has at least one
non-empty raw range, and every range must fit within its manifest. In an
archive-set file using required feature bit `0x1`, a range may instead resolve
through a caller-validated catalog manifest and length. Multiple entries and
generations may reference the same range without copying its bytes.

Semantic schema `0` means raw-only/unparsed and requires role and kind `0` with
no name or tool-call ID. Semantic schema `1` is the current minimal normalized
view. Role codes are `0` opaque, `1` system, `2` user, `3` assistant, and `4`
tool. Kind codes are `0` opaque, `1` message, `2` tool call, `3` tool result,
and `4` summary. Other `u16` semantic-schema, role, and kind values round-trip;
raw manifest ranges remain the fidelity source of truth rather than those
annotations.

## Type 13: conversation generation, schema 1

```text
generation_id          [u8;32]
parent_generation_id   0:u8 | 1:u8 || [u8;32]
reason                 uvarint, constrained to u16
entry_count            uvarint
entries                repeated [u8;32] conversation-entry IDs
```

The generation ID is BLAKE3 over every following field. Entry order is
authoritative. Reason codes are `1` initial, `2` append, `3` compaction, `4`
branch, `5` mutation, and `6` import; other `u16` codes round-trip. An initial
generation cannot have a parent. Append, compaction, branch, and mutation
generations require one. Import generations may have a parent or stand alone.
Parents and entries must resolve to earlier canonical records. Compaction and
branching therefore share surviving entry IDs and add only genuinely new raw
manifest ranges.

## Type 14: turn view, schema 1

```text
turn_view_id           [u8;32]
trace_id               event_bytes
generation_id          [u8;32]
upto_index             uvarint
response_entry_count   uvarint
response_entry_refs    repeated [u8;32] conversation-entry IDs
```

The turn-view ID is BLAKE3 over every following field. Its non-empty trace ID
must resolve to exactly one exchange, and only one turn view may use a trace ID.
The generation and response entries must already exist. `upto_index` is the
inclusive index of the final generation entry sent in that request and must be
within the generation. Response references identify the entries produced by
the turn without copying their raw response bytes.

## Record dependency order and deduplication

Writers emit dependencies before references:

```text
chunks -> body manifest -> conversation entry -> generation
body manifest -> stream index
(body manifest + header block + stream index) -> stage -> exchange
(generation + exchange + response entries) -> turn view
```

Body manifests, header blocks, stream indexes, stages, and exchanges are
content addressed, as are conversation entries, generations, and turn views.
Re-appending an identical logical record reuses its ID and does not emit another
frame. Distinct client/upstream/downstream stages and normalized entries can all
reference the same body manifest. Standalone archives remain self-contained.
Live body packs advertise required feature bit `0x1`; their stage body IDs and
conversation ranges may resolve through the validated SQLite archive-set
catalog, which enables cross-pack global chunk/body reuse without copying body
bytes into event or graph data.

## Fidelity boundary

LAR preserves exactly the bytes supplied to `ArchiveWriter::append_body`; it
does not claim those bytes are the original socket bytes if an upstream layer
already decompressed, decoded, normalized, or reassembled them. Body manifests
and chunks preserve one content-addressed copy of those supplied bytes and
verify reconstruction.

Header blocks preserve ordered name/value byte strings and duplicates. They do
not preserve HTTP delimiter whitespace, header-line folding, HPACK/QPACK bytes,
TCP/TLS framing, or bytes lost before capture. A legacy importer must select a
non-exact fidelity code whenever source order or casing is unavailable. Stages
preserve the known ordering and relationships among captured artifacts, but
they cannot restore fidelity already lost before capture. Conversation entries
add a versioned semantic view over raw manifest ranges; that interpretation is
never a replacement for the referenced bytes.

## Limits and allocation safety

`Limits::default()` currently applies:

| Limit | Default |
| --- | ---: |
| file header payload | 1 MiB |
| record payload | 2 MiB |
| uncompressed metadata page | 2 MiB |
| uncompressed chunk | 128 KiB |
| reconstructed body | 16 GiB |
| chunks per manifest | 2,000,000 |
| atoms per header block | 65,536 |
| individual field | 1 MiB |
| dictionary descriptors | 256 |
| identifier bytes | 4 KiB |
| reads per stream index | 4,000,000 |
| parsed frames per stream index | 4,000,000 |
| stages per exchange | 65,536 |
| indexed stream records | 4,000,000 |
| indexed stage records | 16,000,000 |
| indexed exchange records | 4,000,000 |
| exchanges per session | 1,000,000 |
| raw ranges per conversation entry | 65,536 |
| entries per generation | 1,000,000 |
| response entries per turn view | 65,536 |
| indexed conversation entries | 16,000,000 |
| indexed generations | 4,000,000 |
| indexed turn views | 4,000,000 |

Envelope lengths are checked before allocating their declared buffers. Nested
lengths and counts are checked before their corresponding vectors or byte
strings are allocated. Applications accepting untrusted files should choose
limits appropriate to their memory and workload rather than automatically
increasing them to accommodate a file.

## Corruption and interrupted writes

A complete envelope with an incorrect CRC is corruption and causes an error.
Bad magic, unsupported required data, invalid payloads, duplicate content IDs,
hash mismatches, unresolved or backward references, and invalid ranges also
cause errors. They are not silently treated as interrupted appends.

EOF at a record boundary is clean. EOF after at least one byte of a record but
before its checksum is complete produces `RecoveryStatus::TruncatedTail` with:

- `last_valid_offset`: the start of the incomplete frame, which is also the end
  of the last complete frame; and
- `tail_bytes`: bytes from that offset to EOF.

Only complete records before that offset are indexed. A partial file header is
not recoverable because the reader cannot establish the container contract.
Reopening for append rejects a truncated tail; safe repair must first copy or
truncate to the reported `last_valid_offset` using a file-aware operation.

The integration recovery tests exercise every possible byte prefix of a
multi-record archive and verify that no partial canonical record becomes
visible. Conversation-DAG tests additionally verify footer, active-checkpoint,
and forward-scan reconstruction of shared entries, branches, mutations, and
compaction generations.

## Evolution rules

Within container major 1:

1. Add ignorable header data by increasing the minor version, appending it
   inside the bounded header payload, and setting only optional feature bits.
2. Set a required feature bit whenever ignoring the new behavior could change
   interpretation or fidelity. Current readers will reject such a file.
3. Add records with a new type code. Mark them optional only if removing them
   leaves all older semantics and fidelity claims valid.
4. Change a record payload by using a new schema version. Do not reinterpret a
   schema-1 payload in place.
5. Change the container major for an incompatible envelope, checksum, ordering,
   or required interpretation change.

The frozen envelope fixture at
`crates/alex-lar/testdata/v1-envelope.hex` guards byte order, field widths, CRC
coverage, and deterministic public encoding. This is a compatibility sentinel,
not evidence that the broader LAR design is finished.

The versioned `v1.0-full.lar` fixture freezes the pre-conversation-DAG v1.0
surface: a complete sealed archive with chunk data, a mixed metadata page,
stream/stage/exchange records, indexes, and footer. It remains readable because
new graph records use new required type codes and feature bit `0x2` rather than
reinterpreting those records. `v1.future-minor-optional.lar` proves that the v1
reader accepts a newer minor and skips unknown optional record types and schemas. Run
`cargo run -p alex-lar --example generate_golden` to verify deterministic bytes;
overwriting fixtures requires the explicit trailing `-- --write` flag.
`v1.conversation-dag.lar` separately freezes required feature bit `0x2`, raw
range entries, generation and turn-view records, and their persisted indexes.
`v1.exchange-metadata.lar` freezes the optional Type-15 ExchangeMetadata
companion, including its current field encodings and unknown optional attribute
preservation.
`conformance-v1.json` publishes byte lengths, SHA-256 transport digests, and
semantic expectations for all public fixtures; run
`cargo run -p alex-lar --example verify_conformance` to validate it.

Malformed-input fuzz targets live under `crates/alex-lar/fuzz` for framing and
checksums, zstd decompression, metadata/index/footer parsing, and manifest
reconstruction/recovery. Each caps a fuzz input at 1 MiB and applies tighter
record/count limits than production defaults; the checked-in seed for each
target is the 1,968-byte v1.0 golden archive.

## Not implemented

The following design items are not part of the current on-disk implementation:

- a persisted time index (chunk, body, header, stream, stage, exchange, trace,
  session, conversation-entry, generation, turn-view, and turn-trace indexes
  are implemented);
- separate attempt records (v1 represents attempts on ordered stages);
- provider adapters that populate normalized conversation entries from known
  wire formats; the core accepts canonical entries or raw-only entries without
  parsing provider bodies;
- normalized stream deltas beyond parser/kind/range annotations;
- a self-contained cross-file archive-set container (the core supports
  caller-validated external manifest references; SQLite supplies the live
  cross-pack catalog and garbage collection);
- a special standalone closure record (standalone exporters instead copy the
  ordinary transitive chunk/manifest/event closure);
- encryption, signatures, or authenticated checksums; and
- a generic provenance record beyond the optional ExchangeMetadata fields and
  the migration/catalog provenance maintained by Alex.

Until the remaining records and integrations exist, the implemented container
must not be described as a complete LLM traffic archive or byte-exact HTTP
replay system. It can faithfully relate only the bytes and metadata supplied
to the current writer APIs.
