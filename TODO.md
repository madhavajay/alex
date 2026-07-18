# Alexandria

A long-lived local **daemon** that holds your LLM provider credentials, auto-routes
LLM/agent-harness traffic through itself, and captures full request/response traces
into SQLite + files.

Think of it as: `dario` + `CLIProxyAPI` + a trace database, rebuilt in Rust as one
purpose-built binary. It replaces the tangle of cove's in-repo proxy + external Dario +
per-job spawns with a single durable service.

---

## 1. Goals & non-goals

### Goals
- **Credential vault** — hold OAuth subscription tokens (Claude Code, Codex/ChatGPT,
  Gemini CLI, Grok) and raw API keys; import them from each tool's native location;
  auto-refresh before expiry. Hand harnesses a **fake local key** so the real creds
  never leave the daemon.
- **Auto-routing** — one local endpoint that fronts many providers. Point any
  OpenAI/Anthropic/Gemini-compatible harness at it via base-URL env vars. Route by
  model prefix (`claude:`, `openai:`, `gemini:`, `grok:`).
- **Format translation** — accept requests in any wire format
  (OpenAI chat/responses, Anthropic messages, Gemini generateContent) and translate
  to whatever the chosen upstream speaks, including streaming SSE.
- **Trace capture** — every exchange → durable record: metadata + usage in SQLite,
  full request/response bodies (incl. SSE streams) spilled to files, cost computation from usage.
- **Daemon lifecycle** — start once, run forever (systemd/launchd), hot-reload config
  and credentials, health + control API, live trace view.

### Non-goals (for now)
- Not a benchmark runner / report generator (that stays in cove).
- No hosted/multi-tenant mode initially — single-user localhost daemon.

---

## 2. Architecture

```
                          ┌───────────────────────────────────────────┐
   harness (claude-code,  │              alexandria daemon             │
   codex, gemini-cli,     │                                            │
   pi, opencode, …)       │  ┌──────────┐   ┌───────────────────────┐  │
        │  base-URL env    │  │ ingress  │──▶│ router (model→upstream)│  │
        ├─────────────────▶│  │ HTTP(S)  │   └───────────┬───────────┘  │
        │  fake local key  │  │ server   │               ▼              │
        │                  │  └──────────┘   ┌───────────────────────┐  │
        │                  │        │        │ translator registry   │  │
  from→to req/stream    │  │
    HTTP_PROXY + CA)       │        │        └───────────┬───────────┘  │
        └─────────────────▶│  ┌──────────┐               ▼              │
                           │  │ capture  │◀──┐ ┌───────────────────────┐│
                           │  │ (tee)    │   │ │ credential vault +    ││
                           │  └────┬─────┘   │ │ OAuth refresh + pool  ││
                           │       ▼         │ └───────────┬───────────┘│
                           │  ┌──────────┐   │             ▼            │
                           │  │ SQLite   │   └──── upstream client ─────┼──▶ api.anthropic.com
                           │  │ + files  │      (inject real creds)     │    api.openai.com
                           │  └──────────┘                              │    generativelanguage…
                           │  control/admin API + live SSE + TUI        │    api.x.ai
                           └───────────────────────────────────────────┘
```

### Stack (recommended)
- **Runtime**: `tokio`.
- **HTTP server + client**: `hyper` (low-level control for streaming/SSE tee) or
  `axum` for ingress + `reqwest` for upstream. Recommend **axum ingress + reqwest
  upstream** to start; drop to raw hyper only if streaming control needs it.
- **TLS**: `rustls` + `rcgen` for on-the-fly cert generation.
- **DB**: `rusqlite` (or `sqlx` if we want async/compile-checked queries — recommend
  `sqlx` with the sqlite backend). WAL mode.
- **Serde** for all wire formats; keep translation as pure fns over `serde_json::Value`.
- **Config**: `figment` or plain `serde` + TOML at `~/.alexandria/config.toml`.
- **CLI**: `clap`. **TUI** (later): `ratatui`.

### Crate layout
```
alexandria/
├─ crates/
│  ├─ alexandria-core/      # pure logic: translation, routing, trace model, 
│  ├─ alexandria-auth/      # credential vault, OAuth flows, refresh, import
│  ├─ alexandria-proxy/     # ingress server, upstream client, capture
│  ├─ alexandria-store/     # SQLite + file body store, migrations
│  └─ alexandria-daemon/    # binary: wiring, config, control API, lifecycle
└─ xtask/ or scripts/
```
Keep `core` free of I/O so translation + routing are unit-testable in isolation
(this is where cove's converters were entangled with the server — don't repeat that).

---

## 3. Data model (SQLite)

Metadata + usage in rows; large bodies as files referenced by path. Borrowed from
cove's `events.ndjson` shape + CrabTrap's audit row + agent-super-spy's usage parse.

```sql
-- one row per proxied exchange
CREATE TABLE traces (
  id                TEXT PRIMARY KEY,          -- trace_id (also correlation header)
  ts_request_ms     INTEGER NOT NULL,
  ts_response_ms    INTEGER,
  session_id        TEXT,                       -- harness session grouping
  harness           TEXT,                       -- claude-code, codex, …
  client_format     TEXT,                       -- openai-chat|openai-responses|anthropic|gemini
  upstream_provider TEXT,                       -- anthropic|openai|gemini|xai
  upstream_format   TEXT,
  requested_model   TEXT,                       -- as client asked
  routed_model      TEXT,                       -- after normalization
  method            TEXT,
  path              TEXT,
  status            INTEGER,
  streamed          INTEGER,                    -- bool
  -- usage (parsed from response, incl. SSE)
  input_tokens      INTEGER,
  cached_input_tokens INTEGER,
  cache_creation_tokens INTEGER,
  output_tokens     INTEGER,
  reasoning_tokens  INTEGER,
  cost_usd          REAL,                       -- computed from pricing table
  billing_bucket    TEXT,                       -- subscription|api (Claude split)
  -- body pointers (files on disk, gzipped)
  req_body_path     TEXT,
  upstream_req_body_path TEXT,
  resp_body_path    TEXT,
  req_headers_json  TEXT,                       --
  resp_headers_json TEXT,                       --
  error             TEXT,
  account_id        TEXT                        -- which pooled account served it
);
CREATE INDEX traces_session ON traces(session_id);
CREATE INDEX traces_ts ON traces(ts_request_ms);
CREATE INDEX traces_model ON traces(routed_model);

CREATE TABLE pricing (
  model TEXT PRIMARY KEY,
  input_per_m REAL, cached_input_per_m REAL,
  cache_creation_per_m REAL, output_per_m REAL
);

-- credentials live here OR as 0600 JSON files; see open questions
CREATE TABLE accounts (
  id TEXT PRIMARY KEY,
  provider TEXT,            -- anthropic|openai|gemini|xai
  kind TEXT,                -- oauth|api_key
  label TEXT,
  access_token TEXT, refresh_token TEXT, id_token TEXT,
  expires_at_ms INTEGER, last_refresh_ms INTEGER,
  account_meta_json TEXT,   -- account_id, email, scopes, etc.
  status TEXT,              -- active|cooldown|suspended
  cooldown_until_ms INTEGER
);
```

File body layout: `~/.alexandria/bodies/<yyyy-mm-dd>/<trace_id>.{request,upstream-request,response}.{json,body}.gz`

---

## 4. Milestones

### M0 — Skeleton & daemon shell
- [x] Cargo workspace with the 5 crates above.
- [x] `alexandria daemon` starts, binds a port, `/health` endpoint, graceful shutdown.
- [x] Config load from `~/.alexandria/config.toml` (bind host/port, data dir, local key).
- [x] SQLite store init + migrations; WAL; `pricing` seeded from a `models.json`.
- [x] launchd plist + systemd unit templates (`config/launchd/`, `config/systemd/`).
- [x] `./install.sh [--service|--upgrade|--prefix DIR]` — release build + system install;
      `--upgrade` is a zero-downtime blue-green deploy (SO_REUSEPORT shared bind, new
      daemon health-checked, old daemon SIGTERM → graceful drain of in-flight
      requests/SSE; verified 0 dropped requests during a live swap).

### M1 — Passthrough proxy + trace capture (single provider: Anthropic)
- [x] axum ingress; `POST /v1/messages` forwarded to `api.anthropic.com` unchanged.
- [x] Local-key check: harness sends fake `x-api-key`; reject others (not an open relay).
- [x] Streaming: `tee` the SSE body → client + background buffer.
- [x] Trace write: metadata row + gzipped bodies to files; auth/cookie headers redacted.
- [x] SSE usage parser (Anthropic `message_start`/`message_delta`) → token counts + cost.
      (Also OpenAI chat/responses shapes; sniffs SSE when upstream omits content-type.)
- [x] Point real `claude-code` at it via `ANTHROPIC_BASE_URL` + `ANTHROPIC_API_KEY`.
      **Acceptance:** run a claude-code task through it, see traces + correct token/cost.
      (Met: `./test.sh harness --only H1` runs claude-code in Docker → Alexandria →
      dario → Anthropic with complete trace capture.)

### M2 — Credential vault + OAuth (Anthropic first)
- [x] Import existing creds: read `~/.claude/.credentials.json` (+ macOS Keychain),
      `~/.codex/auth.json`, `~/.gemini/oauth_creds.json` → `accounts`. `alexandria auth import`.
- [x] `alexandria auth login claude|codex` — full PKCE OAuth flows from the terminal
      (prints authorize URL + opens browser; claude = code-paste, codex = loopback
      listener on :1455). `auth login grok` delegates to the grok CLI login then
      auto-imports; gemini not yet supported.
- [x] Token refresh with **singleflight** per account (Anthropic + OpenAI/Codex),
      refresh-on-expiry before forward. 429 → `Retry-After`-aware account cooldown.
      TODO: proactive refresh loop.
- [x] Inject real token on forward; harness only ever sees the fake key.

### M-extra — shipped ahead of schedule
- [x] `alexandria ping [anthropic|openai|all]` — fire a tiny prompt through the proxy
      to verify credentials end-to-end (exit code 1 on failure).
- [x] Heartbeat loop: `heartbeat_minutes` in config (default 15, 0 = off); pings each
      provider, records to `heartbeats` table, surfaced at `GET /admin/health`.
- [x] `alexandria traces` query CLI + `GET /admin/traces` + `GET /admin/accounts`.
- [x] `alexandria env` — print base-URL/key exports for harnesses.
- [x] Codex/ChatGPT oauth upstream (`chatgpt.com/backend-api/codex/responses`) with
      `chatgpt-account-id`/`originator`/`OpenAI-Beta` headers; OpenAI api-key upstream.
- [x] `./alexandria` dev shim (cargo run wrapper).

### M3 — Multi-provider routing + translation (see §9 for cove-parity detail)
- [x] Router: model-prefix → provider (colon + slash prefixes, `cove/`/`alexandria/`
      passthrough, bare aliases like `opus-4.8`); `/v1/models` advertises models +
      aliases (`alexandria_core::model_aliases`).
- [x] Translation layer (`alexandria-core/src/translate.rs`, pure fns over `Value`,
      buffered v1 with synthesized SSE back to the client):
  - [x] OpenAI chat ↔ Anthropic messages (both directions, tools included)
  - [x] OpenAI responses ↔ Anthropic messages (both directions)
  - [x] Codex Responses backend specifics (`normalize_codex_request` set/strip lists;
        headers `chatgpt-account-id`, `originator`, `OpenAI-Beta` already in place)
  - [ ] Gemini generateContent ↔ OpenAI/Anthropic (blocked on §8.5 decision)
  - [ ] Incremental event-by-event SSE translation (buffered v1 shipped; do after correctness)
- [x] Upstream clients per provider incl. xai/grok CLI upstream
      (`cli-chat-proxy.grok.com`, `X-XAI-Token-Auth`/`x-grok-*` headers).
- [x] Dual-format SSE usage parser (Anthropic + OpenAI shapes).
- [x] Per-provider adapters — harness wiring via `alexandria env`, `harness run`, §5.

### M4 — Account pool + resilience
- [x] Multiple accounts per provider; selector with round-robin + cooldown on 429
      (`Retry-After`-aware, degraded soonest-expiry pick when all cooling).
- [ ] Conductor/scheduler pattern (crib CLIProxyAPI `sdk/cliproxy/auth/`).
- [ ] Native request queue + concurrency limit + upstream timeout + retry
      (dario carries this for Anthropic traffic in dario mode; native queue still open).
- [ ] Rate-limit / overage awareness surfaced in analytics.

### M5 — Observability & control
- [x] Control/admin API: `/admin/{traces,accounts,health,analytics,limits,dario}`.
- [x] `alexandria limits` — subscription plan + limit-window utilization + reset times
      per provider (Anthropic via the OAuth usage endpoint; Codex/xai from captured
      rate-limit response headers).
- [x] Rolling-window analytics (`GET /admin/analytics?since_minutes=` — per-model
      requests/tokens/cost/errors/latency + billing-bucket split).
- [x] `alexandria tui` live view (ratatui): sessions tab (grouped, ping-filtered),
      Enter → live transcript follow, limits gauges, accounts, dario tabs.
      (Polling-based; a push SSE trace stream remains a possible optimization.)
- [x] Shipped beyond plan: run keys (`/admin/run-keys`, `alex keys`) for
      harness-agnostic run attribution; metadata headers (harness/task/model/job/
      phase/kind) → tags; sessions + transcript API with tool_calls; body-text
      search; `/traces/{id}/body/{kind}`; AlexandriaBar macOS app (menu bar +
      Trace Browser + Dario window); `GET /admin/storage` retention (§ M8+).
- [x] Query CLI: `alexandria traces --session … --model …`.

### M6 — Dario runtime + generational supervisor (see §10)
- [x] Submodule `repos/dario`, npm runtime (`@askalf/dario`, per-version installs),
      update monitor, dario pwd (`dario_api_key` in config, auto-generated).
- [x] Generational supervisor: warm → health + preflight → promote → drain
      (streaming-aware in_flight) → SIGTERM/kill; rollback on failed preflight.
- [x] `anthropic_upstream = "direct" | "dario"` routing; `GET /admin/dario`,
      `POST /admin/dario/{restart,update}`, `alexandria dario status|restart|update`.

### M7 — Claude subscription billing preservation
- [x] Primary path: CC wire-shape delegated to Dario via §10 (verified live:
      claude-code Docker harness → Alexandria → dario generation → Anthropic, W10/H1).
- [ ] Fallback (only if Dario path insufficient): rewrite outbound to interactive
      Claude-Code wire-shape natively (dario `cc-template.ts` reference).
      **Caution**: dario hit payload corruption from over-aggressive identifier
      scrubbing — keep rewriting minimal + well-tested; snapshot-test wire shape.

### M8 — Test suite (see §11)
- [x] `./test.sh` with unit/wire/harness/dario tiers, parallel matrix, trace-back
      verification, live per-provider ping preflight (`--strict` to override),
      SKIP-when-unconfigured.

---

## 5. Harness wiring reference

How each harness is pointed at the daemon (from the survey). All get a fake local key.

| Harness | How to route | Creds location (for import) |
|---|---|---|
| claude-code | `ANTHROPIC_BASE_URL` + `ANTHROPIC_API_KEY` | `~/.claude/.credentials.json` |
| codex | `~/.codex/config.toml` `[model_providers.x]` `base_url`+`env_key`+`wire_api`; `OPENAI_BASE_URL` | `~/.codex/auth.json` |
| gemini-cli | `GOOGLE_GEMINI_BASE_URL` / `CODE_ASSIST_ENDPOINT`; `GEMINI_API_KEY` | `~/.gemini/oauth_creds.json` |
| pi / oh-my-pi | provider `baseUrl` in models config; `openai/` prefix | `~/.pi/agent/auth.json` |
| opencode | provider `baseURL` in config | its own config |
| jcode (Rust) | `OPENAI_API_BASE` / provider `base_url` | `~/.jcode/auth.json` (+ reads others) |
| gemini/grok | `XAI_API_KEY`, `GROK_MODELS_BASE_URL` | `~/.../grok/auth.json` |
| mini-swe-agent / pydantic-ai / stirrup | standard `OPENAI_BASE_URL`/`ANTHROPIC_*` env | env / config |

Container reachability: bind `0.0.0.0`, advertise `host.docker.internal` (macOS) /
`172.17.0.1` (Linux) to containerized harnesses.

---

## 6. What to borrow (source map)

| Need | Borrow from | Where |
|---|---|---|
| Translation registry (`from→to`, req/stream/nonstream) | CLIProxyAPI | `internal/translator/translator/translator.go`, `sdk/translator/` |
| Concrete converter logic (OpenAI↔Anthropic↔Gemini) | cove | `cli/src/main.rs` L3184–5300 (convert_* fns) |
| Upstream clients + provider headers | cove | `cli/src/upstream.rs` |
| OAuth (loopback, constants) + refresh hardening | CLIProxyAPI | `internal/auth/claude/anthropic_auth.go`, `sdk/cliproxy/auth/{auto_refresh_loop,conductor,scheduler,selector}.go` |
| Account pool / cooldown / selection | CLIProxyAPI | `sdk/cliproxy/auth/` |
| Local-key→real-key swap + SIGHUP reload | agent-super-spy | `llm-proxy/app/lib/keys.ts` |
| SSE tee + dual-format usage parse | agent-super-spy | `llm-proxy/app/server.ts` `parseStreamForMetadata` |
| Trace/body layout + gzip | cove | `write_proxy_trace` (main.rs L5657) |
| In-process fetch shim fallback (Claude) | cove | `harbor_agents/cove_claude_fetch_logger.cjs` |
| Audit row schema + multi-codec decompress | CrabTrap | `internal/audit/logger.go`, `DESIGN.md §10` |
| Subscription billing-bucket preservation | dario | `src/cc-template.ts` (+ scrubbing cautionary tale) |
| Rolling-window analytics + TUI | dario | `src/analytics.ts`, `src/tui/` |
| Daemon packaging (systemd/launchd, file-watch reload) | cove | `systemd/*.service`, `REPORT-SERVICE.md` |
| Model-prefix routing, base-URL+key model | dario | `src/proxy.ts`, README |

---

## 7. Key decisions from cove (learnings)

- **Single long-lived daemon**, not per-job proxy spawns with port scanning + start-locks.
  Cove's `ensure_*_proxy` lifecycle was coupled to `jobs/<job>/` dirs — drop entirely.
- **Own the queue/concurrency/retry natively.** Cove delegated this to external Dario
  (Node); that's the biggest gap to close in Rust. No external process dependency.
- **SQLite is a real trace sink**, not a placeholder — cove's `runs.sqlite3` was 0 bytes;
  traces were only files. Keep files for bodies, but index everything in SQLite.
- **Keep translation pure and testable.** Cove's converters were buried in a 573 KB
  `main.rs`. Isolate in `alexandria-core` with snapshot tests.
- **Native token refresh.** Cove never refreshed — it read tokens the CLIs refreshed, and
  Dario owned Claude's lifecycle. Alexandria must refresh all providers itself.

---

## 8. Open questions

1. **Credential storage**: SQLite rows vs `0600` JSON files (CLIProxyAPI style)? JSON
   files are easier to inspect/import and match native tools; SQLite is queryable and
   atomic. Recommend: JSON files under `~/.alexandria/accounts/` as source of truth,
   mirrored read-only into an `accounts` view. Encrypt at rest? (macOS Keychain / age?)
3. **Billing-bucket preservation (M7)** — do we need Claude subscription-pool routing, or
   is API-key/normal-OAuth enough? This is the riskiest, highest-maintenance piece.
4. **axum+reqwest vs raw hyper** — start with axum+reqwest; revisit if SSE tee latency or
   header fidelity (billing bucket) demands byte-exact control.
5. **Gemini** — support as client format only (translate to OpenAI/Anthropic upstream) or
   as a real upstream via Gemini OAuth too?

---

## 9. Cove proxy replacement (parity notes)

Goal: Alexandria replaces cove's in-repo proxy while preserving the harness
compatibility cove accumulated. Alexandria is the better foundation (daemon, vault,
refresh, traces, health, admin); cove's proxy is ad hoc but speaks more wire formats.

### Gate first: prove the Dario replacement

Cove starts/reuses Dario as the Anthropic upstream for Claude subscription traffic
(port discovery, concurrency, timeouts, traffic logs). Alexandria talks to Anthropic
directly with imported Claude OAuth creds. Before investing in the conversion matrix,
run real claude-code end-to-end through the current passthrough (the unchecked M1
acceptance item) and confirm subscription traffic works. If plain OAuth forwarding
isn't equivalent (M7 billing-bucket risk), the replacement plan changes shape.

### Cove behaviors to cover

1. **Cross-format routing** — the biggest gap (== M3). Cove accepts OpenAI Responses
   (`/v1/responses`, `/responses`), OpenAI Chat (`/v1/chat/completions`), Anthropic
   Messages (`/v1/messages`, `/messages`), and Gemini
   `models/<model>:generateContent` / `:streamGenerateContent`, then routes by model
   name to Codex, Claude, or xAI. Alexandria currently 501s every cross-format combo
   (`plan_upstream` in `alexandria-proxy/src/lib.rs`):
   - OpenAI Responses/Chat + Claude model → Anthropic Messages, and the Anthropic
     stream converted back to OpenAI Responses/Chat.
   - Anthropic Messages + Codex model → OpenAI Responses, and back.
   - Gemini generateContent routes + conversions.

   Streaming note: buffered translation is an acceptable v1 (the Codex destream path
   already does buffer→final-response); incremental SSE event-by-event translation
   with interleaved tool calls is the hard part — land it after correctness.
2. **Model aliases** — `cove/` and slash prefixes, `opus-4.8`, Grok aliases;
   `/v1/models` should advertise them (cove-compatible `/health` + `/v1/models`
   responses for harness probes).
3. **Codex request normalization** (trivial — const lists in `plan_upstream`).
   Set/preserve: `store=false`, `stream=true`, `tool_choice=auto`,
   `parallel_tool_calls=true`, `reasoning.effort`, `text.verbosity`,
   `include=["reasoning.encrypted_content"]`, `prompt_cache_key`, `service_tier`.
   Strip fields ChatGPT Codex upstream rejects: `context_management`,
   `max_completion_tokens`, `max_output_tokens`, `max_tokens`,
   `prompt_cache_retention`, `safety_identifier`, `temperature`, `top_p`,
   `truncation`, `user`.
4. **xAI/Grok upstream** (small). Route `xai/`|`xai:`|`grok/`|`grok:` to Grok CLI
   auth with headers `X-XAI-Token-Auth`, `x-grok-client-version`,
   `x-grok-model-override`, `x-grok-conv-id`. Prefix parsing already exists in
   `alexandria-core`; the upstream arm is 501 + Grok auth import is missing.
5. **Gemini upstream** (largest). OAuth + code-assist endpoint + a third conversion
   axis. Decide open question §8.5 (client-format-only vs real upstream) first.
6. **Cove integration mode**. Cove discovers/starts Alexandria instead of spawning
   per-job proxies, injects Docker-safe `OPENAI_BASE_URL`/`ANTHROPIC_BASE_URL` +
   local key (`host.docker.internal` on macOS / `172.17.0.1` on Linux). The adapter
   lives in cove; Alexandria stays a static daemon.
7. **Auth-error visibility parity**. Cove surfaces `token_expired`, status, message,
   `cf-ray`, and upstream body per request. Preserve that in Alexandria traces +
   admin output (structured traces already exist; make sure these fields survive).

### Pre-proxy agent auth failures (run diagnostics)

Some failures never reach the proxy. Example claude-code output: `apiKeySource:"none"`,
`model:"<synthetic>"`, `error:"authentication_failed"`,
`result:"Not logged in · Please run /login"`, zero tokens, no upstream request.
Not an upstream 401 and not a model failure — the CLI never picked up usable
creds/proxy env before making any request. The proxy alone cannot see this.

Split ownership so Alexandria stays inside its non-goals (§1 — benchmark semantics
stay in cove):

- **Alexandria**: generic run-event ingestion (`POST /admin/runs/<id>/events` or a
  local JSONL watcher) + a correlation query — "did session/run/job X produce any
  traces?" (match on `session_id`, `x-session-id`, run id). Stable, harness-agnostic
  surface.
- **Cove adapter**: owns log-pattern classification — config-driven, not hardcoded,
  since CLI error strings change across versions — and remediation text:
  - claude-code: `apiKeySource:"none"`, `authentication_failed`, `Please run /login`,
    `total_cost_usd:0` → check `ANTHROPIC_BASE_URL` + `ANTHROPIC_API_KEY` injection
    (or native login / cred import).
  - codex: zero-token exit with no `/v1/responses` trace, missing/invalid
    `CODEX_HOME` auth, login-required text → check `OPENAI_BASE_URL` +
    `OPENAI_API_KEY` injection.
  - gemini/qwen/grok-style clients: transport/auth errors before any successful
    model request.
- Classification labels: `pre_proxy_auth_failure` / `client_not_logged_in`. Zero
  token usage + zero proxy traces ⇒ mark the run invalid benchmark data.

### Already better in Alexandria (keep, don't regress)

Persistent credential vault; OAuth refresh for OpenAI + Anthropic; forced refresh on
upstream 401 with re-import fallback; structured traces + cost + usage + admin
endpoints; real async streaming proxy.

### Sequencing

1. Gate: real claude-code end-to-end through current passthrough (M1 acceptance).
2. OpenAI↔Anthropic conversions incl. stream-back (M3) — unblocks most harnesses.
3. Codex normalization/strip lists — trivial, do alongside #2.
4. Aliases + `/health` + `/v1/models` parity.
5. xAI/Grok upstream (small).
6. Cove integration mode + run-event ingestion/correlation.
7. Gemini routes + upstream (largest; decide §8.5 first).
8. Auth-error trace parity verified throughout.

---

## 10. Dario integration (generational supervisor)

Dario handles Claude-subscription wire-shape (the M7 billing-bucket problem) — delegate
to it instead of reimplementing `cc-template`. Harnesses **never** talk to Dario
directly: they talk to Alexandria, and Alexandria routes to the currently active Dario
generation.

### Source + runtime
- [x] Add submodule `repos/dario` ← `git@github.com:askalf/dario.git` (reference code,
      read-only; used for reading auth/config semantics).
- [x] Runtime via npm package `@askalf/dario`, installed per-version under
      `~/.alexandria/dario/<version>/` (npm install --prefix), never global.
- [x] Update monitor: poll npm registry for `@askalf/dario` latest (and optionally the
      git remote main SHA) every `dario_update_check_minutes`; on new version → install
      → spawn new generation → promote → drain old (below).
- [x] Dario credentials/password (`dario pwd`): store in `~/.alexandria/config.toml`
      (or vault) and hand to the spawned Dario process (env/flag — confirm exact
      mechanism from `repos/dario` once the submodule lands).

### Generational supervisor
- [x] `DarioGeneration { id, port, base_url, child_pid, state, started_at, promoted_at,
      drain_started_at, in_flight, log_dir }`,
      `state: warming | active | draining | stopped | failed`.
- [x] Start: allocate free port → launch `dario proxy --host 127.0.0.1 --port <port>` →
      wait `/health` (liveness only) → **readiness probe**: tiny `POST /v1/messages`
      completion (2s connect / 30s read). `/health` is never trusted as readiness —
      observed failure mode: /health 200 while /v1/messages wedges.
- [x] Periodic readiness probes of the active generation (`dario_probe_seconds`, default
      90s; `dario_probe_failures` consecutive failures → mark unhealthy → roll a
      same-version replacement; `dario_probe_model` config). Failed probes persisted to
      `<log_root>/<gen>.last-probe.json` (distinguish timeout/auth/rate-limit/wire).
- [x] Phases starting|ready|unhealthy|draining|dead in `/admin/dario`; route only to
      ready — `active()` returns None while unhealthy, so Alexandria falls back to the
      direct Anthropic upstream until a replacement is ready.
- [x] Process-death self-heal: reaper detects an exited active child (kill -9, crash)
      and respawns the same version with 10s retry backoff — no Alexandria restart.
      Proxy `suspect()` hook fires an immediate debounced probe when a dario-routed
      request errors mid-flight.
- [x] Chaos verified: `./test.sh dario` → DARIO-PROBE (phase=ready + passing probe) and
      DARIO-RECOVER (kill -9 the active generation; new generation ready and serving a
      real completion in ~4s).
- [x] Promote only after healthy: atomic swap `active_dario = generation_id`; all new
      requests go to the new generation.
- [x] Rollback: if new generation fails health/preflight → do **not** promote; keep old
      active, kill the failed one.
- [x] Drain old: mark draining, stop routing new requests, per-generation `in_flight`
      counter — increment before proxying, decrement **only when the full response
      body/stream has finished or errored** (SSE/chunked: not at upstream headers;
      at stream close / client-disconnect completion).
- [x] Kill when safe: `in_flight == 0` + idle 5–15 s → SIGTERM; still draining after
      max grace (8 min healthy / 30 s unhealthy short-leash) → force kill. Logs per
      generation.
- [ ] Optional session pinning: `session_id → generation_id` map with idle expiry, only
      if harnesses prove sensitive to mid-session template/account changes.
- [x] Routing hook: config `anthropic_upstream = "direct" | "dario"`; when `dario`,
      Anthropic-bound requests go to the active generation's `base_url`; trace records
      the generation id. Keep `direct` as fallback.
- [x] Admin surface: `GET /admin/dario` (generations, states, in_flight, versions),
      `alexandria dario status|restart|update`.

---

## 11. Test suite — `./test.sh`

One script, tiered + highly parallel. Every test is a single tiny prompt completion
through the proxy, then asserts **the correct path was taken** by reading the trace
back from `/admin/traces` (unique `x-session-id` per cell; harness cells fall back to
time-window + model match).

### Tiers / args
```
./test.sh                # unit + wire (quick, default)
./test.sh unit           # cargo test --workspace
./test.sh wire           # curl-level matrix, all cells parallel (<~30s)
./test.sh harness        # Docker harness matrix, parallel --jobs N
./test.sh dario          # dario supervisor cells (spawn/promote/drain)
./test.sh all
# filters: --only W3,H2  --provider openai  --harness claude  --jobs 8  --json
```
Cells auto-SKIP (not FAIL) when the needed account is missing from the vault
(e.g. no xai account yet) — "assuming it's set up".

### Assertions per cell
status 200 · `upstream_provider` · `upstream_format` · `billing_bucket`
(subscription vs api) · `routed_model` · usage tokens present · cost computed ·
body files exist; for cross-format cells `client_format != upstream_format` proves the
translation path ran; for dario cells the trace carries the generation id.

### Wire matrix (curl-level, no Docker — every cell parallel)

| id | client format (endpoint) | model | expected upstream | bucket | needs |
|---|---|---|---|---|---|
| W1 | anthropic `/v1/messages` | claude-opus-4-8 | anthropic (native) | subscription | ✅ now |
| W2 | anthropic `/v1/messages` stream | claude-haiku-4-5 | anthropic (native) | subscription | ✅ now |
| W3 | openai-responses `/v1/responses` | gpt-5.5 | codex backend (native) | subscription | ✅ now |
| W4 | openai-responses `/v1/responses` non-stream | gpt-5.5 | codex (destream) | subscription | ✅ now |
| W5 | openai-chat `/v1/chat/completions` | gpt-5.5 | codex via chat→responses bridge | subscription | M3 |
| W6 | anthropic `/v1/messages` | gpt-5.5 | openai (anthropic→responses + back) | subscription | M3 |
| W7 | openai-chat `/v1/chat/completions` | claude-opus-4-8 | anthropic (chat→messages + back) | subscription | M3 |
| W8 | openai-responses `/v1/responses` | claude-opus-4-8 | anthropic (responses→messages + back) | subscription | M3 |
| W9 | openai-chat `/v1/chat/completions` | grok-code-fast-1 | xai (native) | subscription (grok CLI oauth) | ✅ live |
| W10 | anthropic `/v1/messages` | claude-opus-4-8 | **dario** active generation | subscription | §10 |
| W11 | model aliases (`cove/`, `alexandria/`, `opus-4.8`, slash prefixes) | various | correct provider | — | M3 |
| W12 | gemini `generateContent` | gemini-* | decide §8.5 first | — | open |

### Harness matrix (Docker, parallel `--jobs`)

| id | harness | model | path exercised | needs |
|---|---|---|---|---|
| H1 | claude | claude-opus-4-8 | anthropic native (sub) | ✅ live |
| H2 | claude | gpt-5.5 | **cross**: claude harness → codex sub | ✅ live |
| H3 | codex | gpt-5.5 | openai-responses native (sub) | ✅ live |
| H4 | codex | claude-opus-4-8 | **cross**: codex → anthropic sub | ✅ live |
| H5 | grok-build | gpt-5.5 | openai-compatible → codex sub | ✅ live |
| H6 | grok-build | grok-code-fast-1 | xai native (curl installer) | ✅ live |
| H7+ | pi / opencode / mini-swe-agent / … | openai/gpt-5.5 | openai-compatible fan-out | add runners incrementally |

Speed: wire cells are all concurrent single completions; harness containers use
pre-packed tarballs from `~/.alexandria/harness-packages/` (no live npm) and run
under `--jobs` (default = CPU count). Target: `wire` < 30 s, `harness` < 3 min warm.

### Suite plumbing
- [x] `./test.sh` entrypoint (bash) + cell runner with per-cell timeout, PASS/FAIL/SKIP
      table, exit 1 on any FAIL, `--json` report.
- [ ] Ephemeral daemon mode for tests: scratch data-dir + port, real vault (read-only)
      so traces from test runs don't pollute the main DB.
- [x] Grok auth import (`~/.grok/auth.json` → vault `xai-*`) — prerequisite for W9/H6.
- [x] Wire W1–W12 (W12 gemini stubs SKIP); alias cells W11a/W11b; W9 with xai; W10 with §10; keep the matrix
      table as the source of truth for `--only` ids.

---

## 12. Testing — harness e2e groundwork (done)

### Harness e2e — Alexandria-owned registry/cache (done)

- [x] Alexandria-owned harness catalog at `config/harnesses.json` — supported harness
      list copied in, external project/path references scrubbed, proxy/model aliases
      reworded around Alexandria.
- [x] Removed hardcoded external Claude tarball path from
      `crates/alexandria-daemon/src/harness_e2e.rs`; `harness run claude` now resolves
      `@anthropic-ai/claude-code@2.1.202` from Alexandria's own cache at
      `~/.alexandria/harness-packages/`.
- [x] Package preparation owned by Alexandria: `alexandria harness pack claude`,
      `alexandria harness pack @openai/codex` (uses
      `npm pack … --pack-destination ~/.alexandria/harness-packages`).
- [x] `alexandria harness list` reads the catalog and shows all harnesses, plus
      Docker smoke-runner availability, default package/version, and whether the
      cached tarball is present.
- [x] Docker smoke runners: claude, codex, grok-build / grok.
- [x] `docs/harness-e2e.md` updated to describe Alexandria's own registry/cache
      pattern (Dario notes kept as reference context).
- Verified: `cargo test` passes; `alexandria harness list` and
  `alexandria harness pack --help` work.
- [ ] Live `npm pack` + Docker harness run not yet exercised (needs network + live
      proxy credentials).

## 13. Backlog (harvested from stale root handoffs — verify still-open before starting)

Salvaged from `fri.md` (2026-07-13 session handoff) and `CREDENTIALS.md` before those
files were removed. OpenRouter shipped since, so it is intentionally omitted. Confirm each
is still unimplemented against current `main` before picking it up.

### Trace Browser / model UI
- [ ] Trace Browser **Copy Path** button (copy a trace's stored body path). *(confirmed missing)*
- [ ] Mark in-stream SSE `overloaded_error` events inside HTTP 200 streams as trace errors
      (a 200 that streams an error should not read as success). *(verify — error classifier
      already knows `overloaded_error`, but the in-200-stream case may be unhandled)*
- [ ] Transcript **model-switch divider** (show where a session changed model mid-conversation).
- [ ] Per-model harness show/hide.
- [ ] Hide/alias unverified `gpt-5.5-codex` model IDs from the exposed catalog.

### CLI
- [ ] `alex doctor` — environment/health self-check command. *(confirmed missing)*

### Credentials vault — remaining from the credentials plan (now `docs/credentials-plan.md`)
- [ ] Token/dollar **budgets** per credential (per-day/week/month/lifetime) with auto-pause +
      failover at threshold, and menu-bar budget alerts.
- [ ] Per-credential **model allow-list** enforcement in the proxy.
- [ ] Credentials tab full CRUD: edit name/description, **copy-secret / reveal**, per-provider
      "copy env exports".
- [ ] **Audit log** (minted / revoked / paused / budget-hit / refresh-failed, timestamped).
- [ ] **Encrypted vault export/import** — groundwork for peer credential sync.
- [ ] Per-account scheduled **heartbeat/health** coverage (last result per account, not per provider).
