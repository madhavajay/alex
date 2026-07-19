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
asynchronous.

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
account IDs, and reasoning effort. Text search scans request/response bodies and
caps the candidate set at 300 rows.

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
| `GET /traces/{id}/reply.md` | Assistant reply rendered as Markdown text. |
| `DELETE /traces/{id}` | Delete one trace and its referenced body artifacts. |
| `GET /tools/{id}/body/{args|result}` | Decompressed redacted tool payload. |
| `GET /traces/export.ndjson` | Filtered NDJSON; `bodies=1` adds base64-decoded artifact bytes as base64 fields. |
| `GET /traces/runs/{run_id}` | Run summary, plus `/events`, `/artifacts`, and `/export.ndjson` child resources. |

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
