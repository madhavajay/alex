# Traces, transcripts, and scoped capture

Every model request creates a metadata record and, where available, gzip body
artifacts. The same store accepts normalized reverse-wrap traces, harness
lifecycle events, and consented tool execution events.

The local TUI (`alex` or `alex tui`) is the terminal Trace Browser. The daemon
also exposes the JSON/NDJSON API used by UI clients and automation.

## Capture model

The SQLite `traces` row is the searchable index. Important field groups are:

| Group | Fields |
| --- | --- |
| Identity/time | `id`, request/response timestamps, `session_id`, `run_id`, harness, client IP, key fingerprint, tags |
| Request route | client/upstream format, requested/routed/original/served model, provider, account and durable subscription identity, method/path |
| Result | status, streamed, latency, error kind/code/class, injection/fixture markers |
| Usage | input, cached input, cache creation, output, reasoning tokens, computed cost, billing bucket |
| Retry/protection | substituted, original/served account, substitution reason, JSON attempt list |
| Dario | `via_dario`, generation ID, and `dario_fallback` in tags when a direct fallback occurred |
| Artifacts | client request, translated upstream request, response body paths, redacted request/response header JSON |

`finalize_trace` writes the client request unconditionally. It writes a separate
upstream request only when its bytes differ, and writes the response when one
exists. The row is inserted after artifact writes, so paths refer to completed
gzip files.

```text
<data_dir>/alexandria.sqlite3
<data_dir>/bodies/2026-07-19/
  <trace-id>.request.json.gz
  <trace-id>.upstream-request.json.gz
  <trace-id>.response.body.gz
  <tool-id>.tool-args.json.gz
  <tool-id>.tool-result.json.gz
```

Writes use a same-directory temporary file, gzip it, sync it, then rename it to
the final path. This prevents a trace row from pointing at a partially written
artifact after the normal write succeeds.

## Redaction boundary

Stored header JSON replaces values for `authorization`, `x-api-key`, `cookie`,
and `chatgpt-account-id` with `<redacted>`. Dario fetch capture also redacts
`set-cookie`. Tool event bodies recursively redact common secret field names,
auth/API-key header text, environment assignments, secret flags, URL passwords,
and standalone key patterns before writing.

Model request and response artifacts are otherwise the actual bodies. They can
contain prompts, source code, tool results, or user-supplied secrets. Treat
`bodies/` and NDJSON exports with inlined bodies as sensitive data.

## Session identity

The proxy discovers a session from explicit Alex/Claude session headers,
known request metadata, and format-specific fields such as Codex
`prompt_cache_key`. A harness tag comes from `x-alexandria-harness` (with a
user-agent fallback). Run keys can supply a fixed `run_id` and tags; request
headers can add trace tags and selected `x-alexandria-*` metadata.

Harness lifecycle hooks post to `/harness-events`. The accepted events are
`SessionStart`, `SubagentStart`, `SubagentStop`, and `Stop`. Parent/child edges
are stored separately from trace rows with harness, child/parent IDs, turn,
agent type, and start/stop times. Session summaries expose child count and
lineage fields without requiring clients to reconstruct the graph.

## Transcript reconstruction

`GET /traces/sessions/{session_id}/transcript` orders the session's trace rows
and produces one normalized turn per request:

```json
{
  "trace_id": "9b2f...",
  "user": "List the Rust crates.",
  "assistant": "I’ll inspect the workspace.",
  "tool_calls": [
    {"id": "call_1", "name": "list_files", "arguments": "{\"path\":\"crates\"}"}
  ],
  "executed_tools": [],
  "model": "claude-sonnet-5",
  "provider": "anthropic",
  "status": 200
}
```

Long sessions can be read without rebuilding the full transcript. `limit`
accepts 1–500 turns, `tail=true` returns the newest page, and stable cursor
pairs walk in either direction:

- `after_ms` + `after_id` returns the next chronological page;
- `before_ms` + `before_id` returns the preceding page;
- `since`/`since_ms` remain supported for older clients.

Paged responses add `total_turns`, `has_more_before`, `has_more_after`, and
oldest/newest timestamp-and-trace-ID cursor fields. Trace rows and the required
tool metadata are selected with session/timestamp indexes; compressed bodies
are opened only for the bounded page. Body parsing runs on a blocking worker so
a large transcript cannot starve daemon health and admin requests.

When a turn has LAR exchange metadata, the same page includes its ordered
`stages` (client request, router decision, every upstream attempt/failure,
client response, and optional stream-index reference). All stages for the page
come from one SQLite query; the Trace Browser does not open body packs merely
to draw the route. Its compact transport line compares content-addressed
header/body references, so it can accurately label client/upstream artifacts
as shared or changed and identify timed streams without loading their bytes.
Legacy turns omit the additive field and remain decodable by older clients.

Explicit conversation-generation metadata is separately paged at
`/traces/sessions/{session_id}/conversation-events`. Stable `after_ms` +
`after_id` cursors and a `limit` of 1–500 walk canonical turn/generation IDs,
parent/reason/evidence, `upto_index`, and (by default) the ordered semantic
entry refs with exact manifest byte ranges. Branch, compaction, mutation, and
import reasons are accepted only with capture/import evidence; the catalog
does not infer them from similar JSON bodies. Use `include_entries=false` for
the Trace Browser summary projection: it avoids expanding every entry in every
growing prefix while retaining the authoritative generation timeline. The
macOS timeline renders structural events plus a bounded recent append window.

Transcript pages also have a reconstructed-body budget. The default is 16 MiB;
local clients may request a smaller or larger cap with `body_byte_budget` (up
to 256 MiB). Responses report `body_byte_budget`, `body_bytes_loaded`,
`body_truncations`, and `body_errors`. Oversized or unreadable artifacts are
omitted explicitly on both the page and affected turn instead of silently
blanking the entire transcript page.

An offline or missing LAR file is a body-level condition, not a daemon health
failure. The transcript endpoint still returns `200` with all available turn
metadata. Its affected `body_errors` entry has `kind` and
`archive_availability` set to `archived_offline` or `archived_missing`, plus
the stable `archive_file_uuid` and last cataloged `archive_path`. The macOS
Trace Browser presents reattach guidance and a body reload action while
leaving daemon connectivity healthy. For compatibility with an older daemon,
the client also recognizes the archive state when only `kind` is present.

Live LAR trace persistence is also kept off Tokio executor threads. A bounded
worker gate is acquired asynchronously before large request/response buffers
are cloned; hashing, zstd, archive sync, and SQLite publication then run on a
blocking worker. `/health` and `/admin/storage` expose worker capacity,
availability, queue wait, work latency, completions, and join failures, while
storage status reports archive states, physical/catalog bytes, unique and
referenced bytes, compression/dedup ratios, checkpoints, and unreachable
chunks.

The user side comes from `last_user_text(client_format, request)`. The assistant
side and requested tool calls come from the upstream response format, including
SSE reassembly. The transcript preserves text/tool/text ordering in
`assistant_blocks`, provider call IDs, and bounded tool arguments. Tool-result
inputs appear on the user side as a bounded `[tool result] ...` summary.

OpenAI Responses clients can replay the entire user history on successive
requests. For Codex sessions the transcript compares a user-history signature
and suppresses an immediately repeated user half, avoiding duplicate display.

## Executed tool calls

Tool capture is opt-in per harness (`alex tool-capture <harness> on`). A labeled
harness key authenticates `/tool-events`; the server derives the harness from
the key, not from an untrusted payload. Normalized tool rows record:

- harness, session and optional turn/trace ID;
- tool-call ID and tool name;
- start/end timestamps, error and exit status;
- redacted gzip args/result paths.

Transcript assembly associates an executed tool by explicit `trace_id` when
provided. Otherwise it uses the session-local interval between the requesting
trace and the next model request. Tool activity is intentionally a separate
table because a turn can have zero or many executions and hook delivery is
asynchronous. In LAR mode, live ingest also appends immutable, self-describing
child Exchanges for the call and result. Those stages reuse the tool body
manifests rather than copying bytes. If arguments or a result arrive after a
body-less phase was published, separate `arguments`/`result` enrichment
children preserve the bytes without mutating history. The Trace Browser merges
only strictly validated tool supplements into the parent stage timeline;
ordinary subagent/model children that also use `parent_trace_id` remain
separate. The exact base-stage sequence is always shown first, followed by
supplements ordered by event time, capture sequence, and stable phase order, so
backdated or end-before-start hook delivery cannot reorder the original
exchange.

## Trace Browser and CLI

```bash
# Terminal browser (default when stdout is a terminal)
alex

# Fast offline recent list
alex traces --limit 20 --model claude-sonnet-5

# Daemon-backed filtered search
alex traces search --since 2h --provider anthropic --errors --limit 100

# One session as JSON
alex traces search --session ses_123 --json
```

Search filters include time bounds, run/session/model/provider, historical
account, path, harness, status/errors, key fingerprint, and limit. The HTTP
search API additionally accepts text (`text` or `q`), error class, multiple
account IDs, and reasoning effort. For LAR-backed traces, text search uses a
versioned SQLite FTS5 index of bounded provider-neutral user, assistant,
reasoning, and tool text. Repeated entries are stored once and reverse
references retain the trace, session, timestamp, stage, and manifest anchors.
The index is derived from authoritative LAR bytes and can be cleared and
rebuilt with explicit artifact/body/text limits.

Mixed stores remain searchable during migration. The HTTP path searches FTS
first, then checks only request/response gzip slots not already covered by the
normalized index. That compatibility pass is capped at 300 trace rows and 4
MiB of decompressed bytes per body, so a malformed or very large legacy body
cannot turn one search into an unbounded scan.

The TUI has Sessions, Limits, Accounts, and Dario tabs. Session rows include
stable display fields (short ID, duration, provider summary, tag summary, and
running/done/error status), and transcript tab counts are computed server-side.

## Agent-facing trace API

All routes below require the local admin key. A run/harness key can create model
traces but cannot read them.

| Method/path | Result |
| --- | --- |
| `GET /traces/search` | Filtered trace rows; supports optional body text search. |
| `GET /traces/accounts` | Active and removed historical account choices for filters. |
| `GET /traces/sessions` | Aggregated sessions with lineage/display fields. |
| `GET /traces/sessions/{session_id}/transcript` | Both conversation sides, assistant tool calls/blocks, and executed tools. |
| `GET /traces/{id}` | One complete metadata row. |
| `GET /traces/{id}/body/{kind}` | Decompressed `request`, `upstream-request`, `response`, `dario-upstream-request`, or `dario-upstream-response`. |
| `GET /traces/{id}/stages/{stage_id}/replay` | Cursor-paged stream reads or parsed frames with bytes and observed timing. |
| `GET /traces/{id}/reply.md` | Assistant reply rendered as Markdown text. |
| `DELETE /traces/{id}` | Delete one trace and its referenced body artifacts. |
| `GET /tools/{id}/body/{args|result}` | Decompressed redacted tool payload. |
| `GET /traces/export.ndjson` | Filtered NDJSON; `bodies=1` adds base64-decoded artifact bytes as base64 fields. |
| `GET /traces/runs/{run_id}` | Run summary, plus `/events`, `/artifacts`, and `/export.ndjson` child resources. |

### Paged stream replay

Replay is stage-specific because retries can produce multiple upstream response
streams for one trace. Request
`/traces/{id}/stages/{stage_id}/replay?source=observed_reads&cursor=0&limit=100`.
Use `source=parsed_frames` to navigate stored SSE/NDJSON frame annotations;
malformed or unparsed streams return an empty parsed page while their observed
raw reads remain available.

Each event includes `bytes_b64`, its raw manifest offset/length, and the
absolute `observed_delta_ns` from first byte. `next_cursor` is null at the end.
The daemon never sleeps: UI speed controls schedule events from those absolute
deltas. Limits are 500 events and 16 MiB per request, with defaults of 100
events and 4 MiB. One event larger than the requested byte cap returns a typed
`replay_event_too_large` error rather than allocating it. Offline and missing
archives return `archived_offline`/`archived_missing` with file UUID and path so
clients can offer reattach and retry.

Example:

```bash
curl -H 'x-api-key: <redacted-local-key>' \
  'http://127.0.0.1:4100/traces/search?since=2h&errors=1&limit=50'
```

## Scoped keys for remote work

| Key kind | Lifetime | Trace capability |
| --- | --- | --- |
| `run` | User TTL, default 24h, capped at 7d | Invoke models; trace rows default to the key's `run_id` and tags. An `x-alexandria-run-id` header takes precedence, and request trace tags extend/override same-named key tags. |
| `harness` | No expiry until revoked | Invoke models and post lifecycle/tool events under its required harness label. |
| `wrap` | No expiry until revoked | Preflight/post only `/traces/ingest`; cannot invoke models or browse traces. |

```bash
alex keys mint --kind run --run-id job-42 --tag team=infra --ttl 2h
alex keys mint --kind harness --label codex
alex keys mint --kind wrap --label remote-mac
```

Only a hash is stored; the raw key is printed once. The trace records a short
fingerprint for correlation. Revocation accepts a full key ID or unique prefix.

Wrap ingest accepts a normalized `TraceRecord` plus optional base64 request,
translated request, and response bodies. The server validates ownership
metadata from the wrap key, enforces safe trace IDs and per-body size limits,
merges key tags, and writes bodies into its own store. Local wrap capture is a
spool, so `alex traces push --run-id ...` can replay it after connectivity
returns.

## NDJSON export and retention

```bash
alex traces export --since 7d --harness codex --out codex.ndjson
alex traces export --run-id job-42 --bodies --out job-42-full.ndjson
alex traces du --json
alex traces prune --older-than 30d --dry-run
```

Export rows are ordered by request time. `--bodies` inlines all available gzip
artifacts as base64 and can produce sensitive, large files.

Body retention defaults to 30 days. Row retention defaults to `0` (unlimited).
`alex traces prune` removes old bodies/headers by default; `--rows` also deletes
rows. The daemon's storage-prune path applies configured retention. `alex reset
--traces --yes` removes trace/tool rows, heartbeats, and captured bodies while
retaining known-account attribution.

Next: [API and formats](api-and-formats.md) ·
[Configuration](configuration.md) · [Harness integration](harnesses.md)
