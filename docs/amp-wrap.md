# Amp wrap (`alex wrap amp`)

Self-contained harness capture for the Sourcegraph Amp CLI. Lives in `crates/alex-wrap` so the main alex proxy routing path stays clean.

## Goal

Point Amp at a local reverse wrap (`AMP_URL` / `amp.url`) so management REST **and** actor WebSocket traffic can be captured without Docker MITM and without polluting `alex-proxy`. Amp model rerouting is not supported.

## Quick use

```bash
# reverse wrap → https://ampcode.com + spawn amp
alex wrap amp

# pass args through to amp (use `--` before amp flags)
alex wrap amp -- -x 'hello'
alex wrap amp -- -x 'Reply with only the word PONG'

# reverse wrap only (no child)
alex wrap amp --serve-only --bind 127.0.0.1:4101

# catalog-driven env/settings plan only (no server)
alex wrap env amp --mode base_url
alex wrap status
```

Capture artifacts (default):

```text
~/.alex/wrap/amp/
  settings.json   # amp.url = wrap base URL
  amp.log         # AMP_LOG_FILE / --log-file
  flows.jsonl     # reverse wrap request/response events
```

### Send wrapped traces to another Alex machine

The reverse wrap and Amp still run on the machine where you launch them and connect directly to `ampcode.com`. Trace upload is a separate channel: normalized records are written to the local Alex spool first, then copied to a central Alex daemon.

Mint an ingest-only credential on the central machine:

```bash
alex keys mint --kind wrap --label remote-mac
```

On the machine running Amp:

```bash
export ALEX_TRACE_URL=https://alex.example.net
export ALEX_TRACE_KEY=alxk-...

alex wrap amp
```

For persistent setup, put only the key in a mode-`0600` file and use `--trace-key-file ~/.config/alex/wrap.key`. The equivalent flags are `--trace-url` and `--trace-key-file`; the equivalent environment variables are `ALEX_TRACE_URL`, `ALEX_TRACE_KEY`, and `ALEX_TRACE_KEY_FILE`.

If preflight or a later upload fails, the wrapper continues capturing locally. After connectivity is restored, replay the run printed by `alex wrap`:

```bash
alex traces push --run-id wrap-amp-<timestamp>-<suffix> \
  --trace-url https://alex.example.net \
  --trace-key-file ~/.config/alex/wrap.key
```

Plain `http://` is accepted automatically only for loopback. A trusted private-network HTTP endpoint requires `--allow-insecure-http` or `ALEX_TRACE_ALLOW_INSECURE_HTTP=1`; use HTTPS for internet-facing endpoints. A `kind=wrap` key can only access `GET/POST /traces/ingest`, cannot invoke models or browse/administer traces, and can be disabled centrally with `alex keys revoke <rk-id>`.

Requires:

1. Amp on `PATH` (or catalog `binary_candidates`)
2. Credential: `alex auth import amp` / vault `amp-api-key` / `AMP_API_KEY`
3. **Outbound allow for `alex` → `ampcode.com:443`** (see [Little Snitch](#little-snitch--local-firewall) below)

## Architecture

```text
                    AMP_URL / amp.url
  amp CLI  ──────────────────────────►  alex reverse wrap (http://127.0.0.1:PORT)
                                              │
                                              │ Host/Origin rewrite
                                              │ + inject rvt-token on /actors/*
                                              │ TLS (rustls) to edge
                                              ▼
                                         https://ampcode.com
                                              │
                              ┌───────────────┴───────────────┐
                              │ REST: /api/* (auth, plugins,  │
                              │       thread-actors, …)       │
                              │ WS:   /actors/gateway/…       │
                              └───────────────────────────────┘
```

### Why not `amp.proxy` / `HTTPS_PROXY` alone?

| Path | What it carries | `amp.proxy` | env `HTTPS_PROXY` | `AMP_URL` reverse |
|------|-----------------|-------------|-------------------|-------------------|
| Management REST (`/api/*`) | auth, plugins, thread create, billing | often yes | yes (with CA) | **yes** |
| Actor WebSocket (`/actors/gateway/...`) | inference / executor stream | **misses WS** | yes if TLS trusted | **yes** |

Native Amp inference is **actor WebSocket (Rivet)**, not OpenAI-style `/v1` through alex-proxy. Amp in Alex is wrap + billing only until actor protocol is mapped.

### Config-driven catalog

Profiles: `crates/alex-wrap/config/wrap-harnesses.json`  
Optional override: `~/.alex/wrap-harnesses.json`

Amp profile knobs (no code change for renames):

- `env` / `optional_env` templates (`AMP_URL`, `AMP_API_KEY`, log paths)
- `settings_file.merge` (`amp.url`, timeouts, updates)
- `credentials` (secrets.json + vault)
- `capture` filters / redaction
- **`reverse_inject`** — query params injected by the reverse wrap (see below)

Modes:

| Mode | Role | Preferred |
|------|------|-----------|
| `base_url` | reverse HTTP choke point (`AMP_URL`) | **yes** |
| `env_proxy` | `HTTPS_PROXY` + CA trust | diagnostic |
| `amp_proxy_setting` | `amp.proxy` only (misses actor WS) | diagnostic |

## Auth & billing

- Import CLI secrets: `alex auth import amp`
- Secrets path: `~/.local/share/amp/secrets.json` key `apiKey@https://ampcode.com/`
- Vault account: `amp` / `amp-api-key`
- Amp keys are **AMP_URL-scoped** — always pass `AMP_API_KEY` when pointing at localhost
- Limits / credits: `alex limits` (Amp balance via `userDisplayBalanceInfo`)
- Auth is for Amp CLI wrap + billing display — **not** bidirectional cross-harness routing yet

If a wrapped run reports `Unexpected server response: 401` only after
`assistant.start`, do not assume the local key is missing. A valid run can
still show `getUserInfo: 200`, actor creation, and a WebSocket `101`
before Amp's server-side inference authorization rejects the turn. First try
the same prompt with plain `amp`; if that also fails, refresh/check the Amp
account's service-side billing or subscription state rather than re-importing
the Alex vault key.

## The WebSocket bug (`guard.missing_header`)

### Symptom

REST through wrap succeeded (`getUserInfo` 200, `thread-actors` 201, WS upgrade **101**), then server closed:

```text
code: 1011
reason: guard.missing_header#<ray-id>
```

Amp reconnect-looped; executor handshake never completed.

### Cause

Amp builds the Rivet endpoint from `AMP_URL` roughly like:

```js
// simplified from amp binary
function rivetEndpoint(baseUrl) {
  if (isLocalhost(baseUrl)) {
    // pathname=/actors — NO public token
    return new URL(baseUrl) with path /actors
  }
  // production: username=default, password=pk_…
  // Rivet client turns password into query rvt-token=pk_…
  url.username = "default"
  url.password = "pk_9tm4qz3zrMerdZXTlBRLRsmJIzSQIPH24meKBqiL6vVpscTvc4w1YPiBgymXf9Az"
  return url  // → /actors/...?...&rvt-token=pk_...
}
```

When wrap binds on `127.0.0.1` / `localhost`, Amp **omits `rvt-token`**.  
Production edge still requires that **public gateway token** (`pk_…`).  
Auth JWT (`wsToken` from `POST /api/thread-actors`) rides in `Sec-WebSocket-Protocol` (`rivet_conn_params`) — necessary but **not sufficient** without `rvt-token`.

Evidence:

| Request | Result |
|---------|--------|
| WS without `rvt-token` | 101 then close `guard.missing_header` |
| WS with `rvt-token=pk_…` | 101 then live frames (`pong` / traffic) |
| WS with `rvt-token=<wsToken JWT>` | `acl.token_not_found` (wrong kind of token) |

Successful direct Amp (via mitmdump) always includes:

```text
GET /actors/gateway/threadActor/websocket/
  ?rvt-namespace=default
  &rvt-method=get
  &rvt-key=T-…
  &rvt-token=pk_9tm4qz3zrMerdZXTlBRLRsmJIzSQIPH24meKBqiL6vVpscTvc4w1YPiBgymXf9Az
  &rvt-skip-ready-wait=true
```

### Fix

Catalog `reverse_inject` + reverse rewrite:

```json
"reverse_inject": {
  "query_params": {
    "rvt-token": "pk_9tm4qz3zrMerdZXTlBRLRsmJIzSQIPH24meKBqiL6vVpscTvc4w1YPiBgymXf9Az"
  },
  "path_prefixes": ["/actors/"],
  "only_if_missing": true
}
```

Implemented in `crates/alex-wrap/src/reverse.rs` (`inject_query_params` / `rewrite_request_headers`).

Also:

- Host / Origin rewrite to upstream (fixes REST “Untrusted origin”)
- `Accept-Encoding: identity` (simple body framing)
- HTTP/1.1 keep-alive request cycles
- WebSocket: after 101, `copy_bidirectional`
- Dial: prefer IPv4, hard timeouts; surface errors to stderr

### Verification of inject (independent of Little Snitch)

Python reverse wrap with the same `rvt-token` inject:

```text
AMP_URL=http://127.0.0.1:4101 amp -x 'Reply with only the word PONG'
→ PONG
```

So the protocol fix is correct; remaining live-`alex` failures on this machine were **outbound firewall**, not inject.

## Reverse wrap implementation notes

| File | Role |
|------|------|
| `crates/alex-wrap/src/reverse.rs` | HTTP/WS reverse proxy, inject, rewrite, tunnel |
| `crates/alex-wrap/src/run.rs` | start wrap + plan + spawn harness |
| `crates/alex-wrap/src/launch.rs` | env/settings/argv from catalog |
| `crates/alex-wrap/src/catalog.rs` | JSON catalog types (`WrapReverseInject`, …) |
| `crates/alex-wrap/config/wrap-harnesses.json` | Amp profile |
| `crates/alex/src/main.rs` | CLI: `wrap amp`, `wrap run`, `wrap env`, `wrap status`, `wrap smoke` |

Important reverse behaviors:

1. Read **first client request** before dialing upstream (avoids CLOSE_WAIT when upstream is blocked).
2. Inject catalog query params on matching paths.
3. On upgrade: forward 101, then raw byte tunnel.
4. Log capture events to in-memory + optional JSONL.

Tests (`cargo test -p alex-wrap`): inject on/off, host rewrite, keep-alive mock, path capture — **15 passed** at last check.

## Little Snitch / local firewall

On this development Mac, **Little Snitch silently drops SYNs** from unsigned Rust binaries (`target/debug/alex`, `rustc`-built probes) to `ampcode.com` (`34.54.147.251:443`), while Python / curl / signed `amp` still connect.

Symptoms:

```text
alex wrap: client error: timeout connect 34.54.147.251:443
  (check local firewall / Little Snitch allow for alex → ampcode.com)
Error: The socket connection was closed unexpectedly. …
alex wrap: harness exited 1
```

**Allow rule needed:**

- Process: `alex` / path to your built binary  
- Remote: `ampcode.com` (and DNS if prompted)  
- Port: `443`  

Then re-run:

```bash
./target/debug/alex wrap amp -- -x 'Reply with only the word PONG'
```

Until that allow exists, **live Rust wrap E2E will fail** even though unit tests and Python-inject E2E pass.

## Status (as of dump)

| Item | Status |
|------|--------|
| `alex wrap amp` / `alex wrap amp -- -x …` CLI | done |
| Catalog-driven plan (env, settings, credentials) | done |
| Reverse wrap REST | done (when outbound allowed) |
| `rvt-token` inject for actor WS | done |
| Protocol E2E (inject via Python wrap → PONG) | **verified** |
| Live `alex` binary E2E on this host | **blocked by Little Snitch** |
| Bidirectional cross-harness routing via wrap | not done |
| Full body capture / pretty dump UI | minimal JSONL only |

## Related commands

```bash
alex auth import amp
alex auth amp-key            # or AMP_API_KEY env
alex limits                  # Amp remaining balance when vault has key
alex wrap status
alex wrap env amp --mode base_url
alex wrap smoke              # mock upstream smoke
```

## Security notes

- Do not log raw `Authorization`, cookies, or `rvt-token` / `wsToken` values.
- `pk_…` is a **public** gateway token baked into the Amp client (not a user secret).
- User JWT `wsToken` is short-lived and sensitive; redacted in capture policy (`redact_query_keys` includes `rvt-token`, `token`, …).

## Future

- Allow / document firewall onboarding for signed release binaries  
- Optional body capture + redacted WS frame log  
- Map actor protocol into multi-provider routing (real cross-harness)  
- Staging Rivet password (`bvUh…`) if wrap targets staging hosts  
- Rotate `pk_` in catalog when Amp ships a new public token  
