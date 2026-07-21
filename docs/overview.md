# Alex architecture

Alex (`alex` and `alexandria` are two names for the same binary) is a local
credential vault, HTTP model gateway, account router, and trace recorder. It
accepts several model API dialects, chooses an upstream provider and account,
injects the upstream credential, translates formats when necessary, and stores
the request outcome. It also has reverse wraps for clients such as Amp that do
not expose a normal model base URL.

This is an implementation reference. For commands see [CLI reference](cli.md),
for the HTTP boundary see [API and formats](api-and-formats.md), and for state
and defaults see [Configuration](configuration.md).

## Workspace crates

| Crate | Responsibility |
| --- | --- |
| `alex` | The CLI binaries, daemon assembly, service management, TUI, harness connection, Dario supervisor, update/reset/status commands, and configuration loading. |
| `alex-proxy` | The Axum ingress and admin router, request authentication, provider planning, credential/header injection, failover, Dario dispatch, trace finalization, and agent-facing trace/transcript handlers. |
| `alex-auth` | Vault account files, OAuth/device login and native credential import, refresh, multi-account selection policies, pauses/cooldowns, routing reserves, encrypted vault bundles, and removed-account tombstones. |
| `alex-core` | Provider/model-name routing, the four client formats, request/response/SSE translation, usage parsing, quota normalization, and price-based cost calculation. |
| `alex-store` | SQLite schema and queries for traces, accounts, pricing, heartbeats, run keys, lineage, and tool calls; live gzip body capture; migrated LAR body pointers and bounded reads; retention and analytics. |
| `alex-wrap` | Catalog-driven application-level reverse wraps, launch environment/settings generation, HTTP/WebSocket capture, secret redaction, and wrapped process execution. |

## Request data flow

```text
AI harness or API client
  |  model request + local/run/harness key
  |  optional harness/session/run metadata
  v
Alex listener (default 127.0.0.1:4100)
  |  authenticate; parse client dialect; start trace metadata
  |  strip alex/* namespace; infer/accept provider prefix
  v
Provider/account planner
  |  skip paused, disabled, cooling, or reserve-blocked accounts
  |  preserve eligible Codex thread affinity; choose retry account on failure
  |  fetch/refresh secret from the local account vault
  v
Format adapter (Anthropic Messages pivot)
  |  Anthropic / OpenAI Chat / OpenAI Responses / Gemini
  |  inject provider credential and provider-specific headers
  v
Provider upstream
  |  response JSON or SSE; retry eligible capacity/server failures
  v
Response adapter -> original client dialect
  |  parse usage; calculate catalog cost; classify error
  v
SQLite trace row + live gzip artifacts or migrated LAR body pointers
```

The client never supplies an upstream provider secret. It supplies an
Alex key. The proxy selects a vault account and builds upstream auth
headers from that account. Inbound `authorization`, `x-api-key`, cookies, and
`chatgpt-account-id` are redacted in stored header JSON.

### Routing boundary

Model prefixes are explicit routing controls. For example,
`alex/openai/gpt-5.6-sol` first loses the `alex/` harness namespace, then routes
to OpenAI because of `openai/`. Bare names beginning with `claude`, `gpt`,
`codex`, `chatgpt`, `o<digit>`, `gemini`, or `grok` are inferred. If no provider
can be inferred, the ingress dialect's default provider is used.

Same-provider account failover is always separate from opt-in cross-model
substitution and protection equivalencies. See
[Providers and routing](providers-and-routing.md).

### Dario path

All non-genuine-Claude-Code Anthropic traffic goes through a supervised local
Dario generation and fails closed if Dario is unavailable. Dario supplies the
Claude-subscription wire shape and Alex keeps the underlying Anthropic account
as the trace/billing identity. Genuine Claude Code requests are detected from
their complete request signature and remain direct. See [Dario](dario.md).

### Wrap path

`alex-wrap` is a separate route for harnesses whose inference traffic is not a
normal OpenAI/Anthropic HTTP API. A reverse wrap points the harness at a local
URL, forwards to the product service, records catalog-selected HTTP/WebSocket
traffic, and imports normalized events into the same local trace store. Amp is
wrap/billing-only; it is not a `/v1` model upstream. See
[Amp wrap](amp-wrap.md).

## Authentication scopes

| Credential | Intended capability |
| --- | --- |
| `local_key` | Model ingress plus all `/admin/*` and trace-browser endpoints. Treat it as the local admin credential. |
| `kind=run` key | Model ingress with a bounded lifetime (default 24 hours, maximum 7 days), optional default `run_id`, and bound tags. Request metadata can override/extend them as described in [Traces](traces.md). |
| `kind=harness` key | Long-lived model ingress and authenticated `/harness-events` and `/tool-events`; the required label is the harness namespace. |
| `kind=wrap` key | Only `GET/POST /traces/ingest`; it cannot invoke models or read/administer traces. |

Run and harness keys do not grant trace-browser reads. Remote workers can
produce attributed traces without receiving the local admin key; a trusted
operator reads those traces through the local-key-gated API.

## Local state

The state root defaults to `~/.alexandria`; `ALEXANDRIA_HOME` changes the config
root, and a custom `data_dir` can place runtime data elsewhere.

```text
~/.alexandria/
  config.toml                 daemon settings and local admin key (mode 0600)
  accounts/                   one JSON file per vault account
    removed-accounts/         non-secret account tombstones
    .routing-policies         persisted account selection/reserve policies
  alexandria.sqlite3[-wal|-shm]
  bodies/YYYY-MM-DD/          live gzipped trace and tool payloads
  lar/legacy-v1.lar           migrated immutable trace-body records
  lar/legacy-v1.import.json   resumable migration checkpoint (while incomplete)
  dario/                      installed generations, logs, runtime state
  dario-prompt-cache/         model-specific captured Claude prompt material
  fixtures/                   saved upstream error fixtures
  wrap/<harness>/             reverse-wrap settings and capture streams
  wrap-harnesses.json         optional replacement wrap catalog
  daemon.log                  `alex daemon --background` output
```

Connected harnesses also receive managed files in their own native config
directories. Those files and their restore behavior are described in
[Harness integration](harnesses.md). Encrypted `alex vault export` bundles are
written only to the explicit `--out` path.

## Operational properties

- The default bind is loopback. A non-loopback bind still causes locally
  generated harness URLs to use loopback; remote clients are configured with an
  explicit address.
- Upstream streams have an idle timeout, not a fixed request deadline. Every
  received chunk resets it.
- Trace body and row retention are independent. Bodies default to 30 days;
  rows default to unlimited retention.
- Live capture writes gzip. `alex traces migrate-lar` can add LAR pointers to
  existing trace bodies; readers prefer bounded LAR reads and fall back to the
  preserved gzip source if an archive is unavailable or corrupt.
- Full request/response bodies are sensitive. Header and tool-capture
  redaction does not turn the trace body store into a secret-free store.
- `alex reset` is a dry run until `--yes` is supplied.

Next: [CLI reference](cli.md) · [Configuration](configuration.md) ·
[Traces](traces.md)
