# LAR — LLM Archive format · design brief

Status: DRAFT v0.1 · owner: storage lane · file extension: `.lar`

A purpose-built capture/storage format for LLM traffic — "HAR for LLM agents."
Replaces Alex's current per-exchange gzipped body files with a session-oriented,
deduplicated, seekable archive.

## Why (measured on real data)

One 14-day single-user capture: **9.4 GB of body files vs 240 MB of metadata**
(55k exchanges). One representative day: request bodies 104 MB, upstream-request
copies another 104 MB, responses only 18 MB. The cause: agent harnesses re-send
the entire conversation prefix on every turn, so a large tool result is stored
once per remaining turn of its session; and client/upstream request bodies are
near-identical duplicates on passthrough routes.

Design target: **each turn costs ≈ its genuinely new bytes.** Expected ≥10x
reduction on agent traffic while *gaining* capabilities (stream replay, cheap
archival, conversation reconstruction).

## Core model

A session is stored as **one indexed transcript plus a tiny cursor per turn**.

### 1. Sequence store (per session)
The conversation as an append-only list of **entries** — system prompt, user
message, assistant message, tool result — each stored **once** as raw wire
bytes: `entry := { index, role/kind, byte-range → chunk, blake3 hash }`.
The response to turn N is simply appended as the next entry, so responses live
in the sequence and the next turn's prefix covers them. Final storage cost of a
session ≈ one copy of the final conversation. Entries are the dedupe unit
(message granularity, not fixed-size chunks) because prefixes repeat at message
boundaries.

### 2. Turn records (per exchange)
Small fixed records (CBOR): timestamps (request/first-byte/last-byte), model
(requested + routed), provider, upstream format, status, usage (in/out/cached/
reasoning tokens), cost, session/run/key/harness tags, error class — plus the
two numbers that make the format work:

    (sequence_generation, upto_index)   // "this request sent entries 0..k"

and a reference to the response entry, the header-set refs, and an optional
divergence patch ref.

### 3. Generations & divergence patches — prefixes are NOT always pure
Real harness traffic mutates history. The format must model this or corrupt:
- **Context compaction** (Claude Code does this live): early transcript is
  rewritten into a summary mid-session → writer bumps `sequence_generation`;
  the new generation lists its entries, referencing *surviving* entries by hash
  (so a compaction event costs ≈ the summary text only).
- **Small per-turn mutations** (moving `cache_control` markers, injected
  dynamic system blocks, retry branches): writer stores a **divergence patch**
  (binary diff against the recorded prefix) on the turn record.
Verification rule: the writer hash-checks the claimed prefix; on mismatch it
patches or bumps generation. Byte-exact replay of ANY turn must always be
reconstructible: `entries[0..k] + patch = exact request body`.

### 4. Header table
Full request/response headers kept per exchange (auth values redacted at
capture, as today). Header **sets** repeat almost verbatim across turns →
content-addressed header-block table: `hash → block`, stored once; turn records
hold refs. Full fidelity, near-zero marginal cost.

### 5. SSE timing index
Streamed responses store the reassembled body as the sequence entry **plus an
event index**: `[(byte_offset, ms_delta_from_first_byte), …]`. This preserves
stream fidelity and enables asciinema-style replay of the generation in a UI.
Raw SSE framing need not be duplicated if `entry bytes + index` can reproduce
it byte-exactly; if a provider's framing is irregular, fall back to storing the
raw stream as the entry with offsets into it.

### 6. Container layout (one `.lar` file per day or per session-group)
Parquet-style: magic `LAR1` + format version + zstd dictionary id → appended
sections as capture happens (chunk data, entries, turn records) → **footer**
with section offsets + indexes written on rotation/close:
- chunk index: hash → (offset, len)
- turn index: trace-id → record offset (Trace Browser opens one trace without
  scanning)
- session index: session-id → generations → entry lists
Live-appendable; crash recovery by forward scan when footer is missing.
Compression: zstd with a shared dictionary trained on LLM wire JSON; the
dictionary ships in the file (self-contained archives).

### 7. Fidelity & normalization
Chunks are **raw wire bytes** (billing proof, byte-exact replay). Turn records
additionally carry a thin normalized view (roles, tool-call ids, model) so
cross-provider tooling doesn't parse three wire formats. Raw is the source of
truth; the normalized view is derived and versioned.

## Interop & integration

- Converters both ways: `lar export --format har|jsonl`, and an importer for
  Alex's current `bodies/` directory layout (migration path).
- Alex integration: SQLite keeps one row per exchange (unchanged schema) with a
  `(lar_file, turn_offset)` pointer replacing the three body paths. Retention
  becomes file-granular: a day's `.lar` IS the cold-archive bundle (move to
  NAS, delete local; re-import on demand).
- The sequence store is the substrate for `alex resume <trace-id> <harness>`
  (reconstruct conversation → replay into another harness) and for compaction
  visualization in the Trace Browser (generation bumps are visible events).
- Spec published + versioned; MIT; positioned as "HAR for LLM agents".

## Non-goals (v1)

- Not a query engine — SQLite remains the index; LAR is the body store.
- No encryption at rest in v1 (directory perms as today); leave header room.
- No cross-file dedupe in v1 (per-file chunk namespace); design hashes so a
  later global store can adopt them unchanged.

## Success criteria

1. Re-pack a real captured day (240 MB gzip files) → ≥5x smaller; agent-heavy
   sessions ≥10x. Benchmark script in-repo.
2. Byte-exact reconstruction of every request/response body vs the original
   files (hash comparison across the full corpus).
3. Random access: open any single trace's bodies in <10 ms from a cold file.
4. Write path keeps up with live capture (streaming append, no buffering of
   whole sessions in memory).
5. HAR export of a session opens in standard HAR viewers.

## Open questions for the implementer

- CBOR vs flatbuffers for turn records (seekable mmap reads favor flatbuffers;
  CBOR is simpler — benchmark both on criterion 3).
- Per-session vs per-day file granularity default (retention favors per-day;
  resume/replay favors per-session; footer session-index may make per-day fine).
- Divergence patch algorithm: bsdiff-style vs simple structural JSON diff —
  measure on real cache_control-move turns from the corpus.
- Zstd dictionary training cadence (ship static v1 dict vs per-file trained).
