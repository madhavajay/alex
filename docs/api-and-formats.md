# HTTP API and format translation

The daemon exposes model ingress, a local control/trace API, scoped event and
trace ingest, and health/discovery endpoints on one Axum listener. The default
base URL is `http://127.0.0.1:4100`.

All model requests require `x-api-key`, `x-goog-api-key`, or
`Authorization: Bearer`. The value may be the configured `local_key` or a valid
run/harness key. Administrative and trace-browser endpoints require the local
key specifically.

## Client-facing ingress

| Method and path | Client dialect | Notes |
| --- | --- | --- |
| `GET /health` | JSON health | Ungated liveness/status response. |
| `GET /connect` | Connection metadata | Loopback clients only. Emits base URLs and the local key (JSON, or env text with `?format=env`); never expose it through an untrusted local process. |
| `GET /v1/models` | OpenAI-style model list | Catalog filtered by configured provider/account availability and exposed model settings. |
| `POST /v1/messages` | Anthropic Messages | Native pivot format; can route to any implemented provider path. |
| `POST /v1/chat/completions` | OpenAI Chat Completions | Canonical versioned chat path. |
| `POST /chat/completions` | OpenAI Chat Completions | Compatibility alias. |
| `POST /v1/responses` | OpenAI Responses | Canonical versioned Responses path. |
| `POST /responses` | OpenAI Responses | Compatibility alias. |
| `POST /v1beta/models/{model}:generateContent` | Gemini GenerateContent | The router receives `{model_action}` and rejects unsupported actions. |
| `POST /v1beta/models/{model}:streamGenerateContent` | Gemini streaming | Sets `stream=true`; upstream Gemini uses `alt=sse`. |

Example Anthropic request routed explicitly to OpenAI:

```bash
curl http://127.0.0.1:4100/v1/messages \
  -H 'x-api-key: <redacted-alex-key>' \
  -H 'content-type: application/json' \
  -d '{
    "model": "openai/gpt-5.6-sol",
    "max_tokens": 64,
    "messages": [{"role": "user", "content": "Reply with PONG"}]
  }'
```

The proxy records the requested model separately from the normalized routed
model. See [Providers and routing](providers-and-routing.md) for prefix and
account selection rules.

## Four dialects, one pivot

`ClientFormat::as_str()` uses these stable trace names:

| Dialect | Trace name | System/user request shape | Assistant tool call | Tool result |
| --- | --- | --- | --- | --- |
| Anthropic Messages | `anthropic` | top-level `system`; `messages[].content` string or typed blocks | `{type:"tool_use", id, name, input}` content block | user content block `{type:"tool_result", tool_use_id, content}` |
| OpenAI Chat | `openai-chat` | `messages[]` roles including `system`, `user`, `assistant`, `tool` | `assistant.tool_calls[].function.{name,arguments}` | `role:"tool"`, `tool_call_id` |
| OpenAI Responses | `openai-responses` | top-level `instructions`; string/array `input` items | `{type:"function_call", call_id, name, arguments}` | `{type:"function_call_output", call_id, output}` |
| Gemini | `gemini` | `systemInstruction`; `contents[].parts[]` | part `{functionCall:{name,args}}` | part `{functionResponse:{name,response}}` |

Anthropic Messages is the internal semantic pivot, not necessarily the network
upstream. Examples:

- OpenAI Chat to Anthropic: Chat messages/tools become Messages blocks, the
  upstream response becomes a Chat completion.
- Anthropic to Codex OAuth: Messages become OpenAI Responses; Codex is
  normalized to streaming with default `tool_choice=auto` and parallel tool
  calls, then translated back to Anthropic.
- OpenAI Chat to Gemini: Chat becomes Anthropic, then Gemini; the response
  follows the reverse path.
- Anthropic to Grok, Kimi, OpenRouter, or Exo: Messages become OpenAI Chat;
  those upstreams currently do not accept Responses or Gemini ingress directly.

Translation preserves text, declared tools, tool choice where the target has
an equivalent, tool-call IDs, tool results, token limits, temperature/top-p,
stop sequences where supported, and stream intent. It intentionally drops
fields with no target equivalent; for example Anthropic thinking budgets do
not map to OpenAI Chat. Anthropic `output_config.effort` does map to OpenAI
Responses `reasoning.effort`.

## Response and transcript extraction

The same translation module provides format-aware helpers used by trace
transcripts:

- `last_user_text(format, request)` finds the most recent user text. If the
  latest input is a tool result, it emits a bounded `[tool result] ...` summary.
- `assistant_reply_text(upstream_format, response)` extracts assistant text
  from JSON or reconstructed SSE.
- `assistant_tool_calls(upstream_format, response)` normalizes Anthropic,
  OpenAI Chat, and OpenAI Responses tool calls without discarding their IDs.
  Gemini `functionCall` parts are recovered by the transcript block reassembler.

This means a session transcript can show both client-side user input and the
actual upstream assistant/tool response even when their dialects differ. See
[Traces](traces.md).

## Streaming and SSE

The proxy recognizes streaming from request `stream`, response content type,
and bodies beginning with `event:` or `data:`. It has parsers/reassemblers for
Anthropic SSE, OpenAI Chat chunks, OpenAI Responses events, and Gemini SSE.

When client and upstream formats differ, Alex may buffer/destream the
upstream final response, translate it, and synthesize the client's SSE event
sequence. Synthesizers emit the dialect's normal terminal signal: Anthropic
message events, OpenAI Chat `[DONE]`, OpenAI Responses output/completed events,
or Gemini SSE frames. Tool-call arguments are reassembled across deltas before
translation.

The configured `upstream_stream_idle_timeout_seconds` is a quiet-period limit.
It is not an overall response deadline: every upstream chunk resets it. The
upstream response-head timeout is 120 seconds.

## Usage, cost, and billing bucket

Usage is parsed from regular response JSON or merged across every SSE `data:`
frame. The normalizer recognizes provider variants for:

- input/prompt tokens;
- cached input/cache-read tokens;
- cache-creation tokens;
- output/completion tokens;
- reasoning/thought tokens.

If the routed model matches an embedded pricing row, Alex computes cost
from uncached input, cached input, cache creation, and output rates. For OpenAI
formats the reported input total is treated as including cached tokens, so the
cached portion is subtracted before applying the uncached input rate. Cost is
absent when no pricing or usable token counts exist.

Traces label OAuth/Dario attempts as billing bucket `subscription` and API-key
attempts as `api`. This is attribution, not a claim that every provider reports
identical quota semantics.

## Control and trace API

The following groups all require the configured local key. The table groups
related routes; it is not an alternate URL scheme.

| Area | Important routes |
| --- | --- |
| Trace search/browser | `GET /admin/traces`, `GET /traces/search`, `GET /traces/accounts`, `GET /traces/sessions`, `GET /traces/sessions/{session_id}/transcript` |
| Trace records/bodies | `GET/DELETE /traces/{id}`, `GET /traces/{id}/reply.md`, `GET /traces/{id}/body/{request|upstream-request|response|dario-upstream-request|dario-upstream-response}`, `GET /tools/{id}/body/{args|result}` |
| Export and runs | `GET /traces/export.ndjson`, `GET /traces/runs/{run_id}`, `/events`, `/export.ndjson`, and `/artifacts` |
| Accounts/providers | `/admin/accounts`, `/admin/accounts/analytics`, `/admin/accounts/merge`, `/admin/providers`, provider pause/resume, `/admin/routing/{provider}` |
| Credentials/auth | `/admin/auth/import`, `/admin/auth/login/*`, reauth endpoints, Gemini/OpenRouter key endpoints, `/admin/vault/export`, `/admin/credentials` |
| Resilience/testing | `/admin/protection`, `/admin/fixtures`, `/admin/sessions/{session_id}/inject`, `/injections` |
| Operations | `/admin/health`, `/admin/analytics`, `/admin/limits`, `/admin/storage`, `/admin/storage/prune`, `/admin/reset`, `/admin/update`, `/admin/update/channel` |
| Dario and local providers | `/admin/dario`, `/admin/dario/ping`, prompt-cache routes, `/admin/exo`, `/admin/exo/status`, `/admin/exo/models` |
| Catalog/notifications | `/admin/openrouter/catalog`, `/admin/openrouter/exposed`, `/admin/notifications` and validation/test/discovery routes |
| Key management | `GET/POST /admin/run-keys`, `DELETE /admin/run-keys/{id}` |

The `alex` binary merges additional local-key-gated Dario supervisor handlers:
`POST /admin/dario/restart`, `POST /admin/dario/update`,
`POST /admin/dario/repair`, and `GET /admin/dario/logs/{generation_id}`.

## Scoped write endpoints

| Route | Accepted credential | Purpose |
| --- | --- | --- |
| `GET/POST /traces/ingest` | `kind=wrap` key or local key | Preflight and upload normalized wrapped trace records with optional base64 bodies (16 MiB per decoded body). |
| `POST /harness-events` | labeled `kind=harness` key | Record `SessionStart`, `SubagentStart`, `SubagentStop`, or `Stop` lineage events. |
| `POST /tool-events` | labeled `kind=harness` key | Record consented tool start/end data and redacted args/results. |

Scoped model keys cannot call `/admin/*` or browse `/traces/*`. Wrap keys cannot
invoke models. This separation lets remote agents submit attributed work without
receiving the local administrative credential.

Next: [Traces](traces.md) · [Configuration](configuration.md) ·
[Providers and routing](providers-and-routing.md)
