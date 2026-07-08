# Integrating cove with Alexandria

Alexandria replaces cove's in-repo capture proxy, per-job proxy spawns, and cove-managed
Dario processes with one durable localhost daemon. Cove becomes a pure client: discover,
inject env, correlate traces. This doc is the cove-side contract.

## What cove deletes

- `ensure_*_proxy` per-job proxy lifecycle (port scanning, start locks, `jobs/<job>/proxy`).
- Dario spawning/reuse at `127.0.0.1:3456`, Dario health checks, Dario preflight, and the
  "reuse if /health OK" logic. Alexandria owns Dario: generational supervisor, active
  `/v1/messages` readiness probes (never `/health` alone), kill/respawn self-healing,
  npm auto-update. Cove must not talk to Dario directly, ever.
- Per-provider upstream clients and format converters used for capture-proxy routing.

Cove keeps: benchmark semantics, run-log classification (see "Pre-proxy failures"),
report generation.

## Discovery and readiness

Alexandria is a user-level daemon (launchd/systemd), default `http://127.0.0.1:4100`.

```
liveness:   GET  {base}/health                     -> 200 {"status":"ok","service":"alexandria"}
readiness:  POST {base}/v1/messages                -> 2xx with a tiny real completion
```

Readiness probe body (proves the model path, not just the process — the same lesson as
the wedged-Dario incident):

```json
{"model": "claude-haiku-4-5", "max_tokens": 8, "stream": false,
 "messages": [{"role": "user", "content": "Reply with exactly: hello"}]}
```

with headers `x-api-key: <local_key>` and a short timeout (connect 2s, read 30s).
Alternatively run `alexandria ping anthropic` (exit code 0/1). Per-provider readiness
for a job that needs OpenAI or Grok: same pattern against `/v1/responses` /
`/v1/chat/completions`, or `alexandria ping openai|grok`.

If liveness fails, cove should NOT auto-start Alexandria silently; surface
"alexandria daemon is not running — start with `launchctl load …` or `alexandria daemon`"
and fail the job fast. (Auto-start is possible — `alexandria daemon` refuses to
double-bind, so it's safe to attempt — but a supervised daemon is the intended mode.)

## Credentials / the local key

The real provider credentials live only in Alexandria's vault. Harnesses get one fake
key. Read it from `~/.alexandria/config.toml`:

```toml
local_key = "alx-…"
port = 4100
host = "127.0.0.1"
```

(or parse the `export` lines from `alexandria env`). Never forward real provider keys
into job containers.

## Env injection per harness

Base URL from inside a container:
- macOS (Docker Desktop): `http://host.docker.internal:4100` — pass
  `--add-host host.docker.internal:host-gateway` for safety.
- Linux: `http://172.17.0.1:4100` (or host-gateway) and set `host = "0.0.0.0"` in
  Alexandria's config so it's reachable from the bridge network.

Anthropic-format harnesses (claude-code):

```
ANTHROPIC_BASE_URL=<base>          # no /v1 suffix
ANTHROPIC_API_KEY=<local_key>
ANTHROPIC_MODEL=<model>            # plus ANTHROPIC_DEFAULT_{SONNET,OPUS,HAIKU}_MODEL,
                                   # CLAUDE_CODE_SUBAGENT_MODEL for full pinning
```

OpenAI-format harnesses (codex, pi, opencode, mini-swe-agent, grok CLI in
openai-compat mode, anything LiteLLM-ish):

```
OPENAI_BASE_URL=<base>/v1
OPENAI_API_KEY=<local_key>
```

codex additionally wants a provider block (cove already generates `config.toml`):

```toml
model_provider = "alexandria"
[model_providers.alexandria]
name = "Alexandria Proxy"
base_url = "<base>/v1"
env_key = "OPENAI_API_KEY"
wire_api = "responses"
requires_openai_auth = false
```

grok CLI native-model mode:

```
XAI_API_KEY=<local_key>
GROK_MODELS_BASE_URL=<base>/v1
GROK_MODELS_LIST_URL=<base>/v1/models
```

Working references: `crates/alexandria-daemon/src/harness_e2e.rs` (`docker_env`,
`docker_script`) runs claude/codex/grok containers against the proxy today.

## Model routing semantics

Any model on any endpoint — Alexandria translates formats both ways (buffered v1 with
synthesized SSE back to the client):

- `claude-*` models on `/v1/chat/completions` or `/v1/responses` → Anthropic upstream.
- `gpt-*` / `o*` models on `/v1/messages` → ChatGPT-Codex (subscription) or OpenAI API.
- `grok-*` on `/v1/chat/completions` → xAI.

Prefixes `claude:`/`claude/`, `openai:`, `grok:`, … force a provider; `cove/…` and
`alexandria/…` prefixes are stripped and re-resolved (existing cove model names keep
working); bare aliases (`opus-4.8` → `claude-opus-4-8`) resolve too. `/v1/models`
advertises everything. Claude-subscription traffic is routed through the supervised
Dario generation automatically when `anthropic_upstream = "dario"`; if Dario is
unhealthy Alexandria falls back to the direct Anthropic upstream — cove sees nothing.

## Trace correlation (the piece cove actually integrates)

Send a unique header per request source:

```
x-session-id: cove-<job_id>
```

(Anthropic-format bodies may instead carry `metadata.user_id`; the header wins.) Then:

```
GET {base}/admin/traces?session=cove-<job_id>&limit=100
```

Each row: `status, harness (user-agent), client_format, upstream_provider,
upstream_format, requested_model, routed_model, input/output/cached tokens, cost_usd,
billing_bucket (subscription|api), error, account_id (dario:<generation> when routed
via Dario), req/resp body paths (gzipped on disk)`. Poll for ~15s after a streaming
request ends — the trace row is written when the stream closes.

Verification contract for a benchmark run: `traces > 0`, all `status` 2xx, tokens
present, `billing_bucket` as expected. This is exactly what `./test.sh` asserts;
`scripts/test-assert.py` is reusable.

## Pre-proxy failure classification (stays in cove)

A run with zero proxy traces AND zero token usage means the harness never reached
Alexandria — env injection or native login problem, not an upstream failure. Label it
`pre_proxy_auth_failure` / `client_not_logged_in` and mark the run invalid. Patterns
(config-driven, CLI strings drift):

- claude-code: `apiKeySource:"none"`, `authentication_failed`, `Please run /login`,
  `total_cost_usd:0`.
- codex: zero-token exit with no `/v1/responses` trace, missing `CODEX_HOME` auth.
- gemini/qwen/grok-style: transport/auth errors before any model request.

The correlation query above ("did session X produce traces?") is Alexandria's stable
surface for this.

## Admin API quick reference

```
GET  /health                       liveness
GET  /v1/models                    advertised models + aliases
GET  /admin/traces?session=&model=&limit=
GET  /admin/accounts               vault accounts + expiry
GET  /admin/health                 accounts + heartbeats + token expiry
GET  /admin/analytics?since_minutes=60    per-model burn, bucket split
GET  /admin/limits                 plan + window utilization + resets
GET  /admin/dario                  generations, phases, probe results
POST /admin/dario/restart|update
```

CLI equivalents: `alexandria ping|traces|limits|dario|auth|harness|env`.

## Migration checklist

1. Discover Alexandria (`/health`), read `local_key` + port from config.
2. Per-provider readiness probe for the providers the job needs; fail fast with a
   clear message otherwise.
3. Inject env per harness family (tables above) with Docker-safe base URL.
4. Tag every job with `x-session-id: cove-<job_id>`.
5. Post-run: correlate traces, compute cost from trace rows (cost_usd is precomputed),
   classify zero-trace runs as pre-proxy failures.
6. Delete: per-job proxy spawn, Dario management, port scanning, in-repo converters.
7. Regression gate: `./test.sh all` in the alexandria repo (wire matrix W1–W12,
   harness matrix H1–H6 incl. claude×gpt-5.5 and codex×opus-4.8, dario chaos tier).
