<p align="center">
  <img src="https://raw.githubusercontent.com/madhavajay/alex/main/macos/Resources/icon.png" width="360" alt="Alexandria — the lighthouse of your LLM subscriptions" />
</p>

<h1 align="center">alex — the Library of Alexandria for your LLM subscriptions</h1>

<p align="center">
  <a href="https://crates.io/crates/alex"><img src="https://img.shields.io/crates/v/alex.svg" alt="crates.io" /></a>
  <a href="https://github.com/madhavajay/alex/actions/workflows/ci.yml"><img src="https://github.com/madhavajay/alex/actions/workflows/ci.yml/badge.svg" alt="CI" /></a>
</p>

Alexandria is a local daemon that turns your AI subscriptions (Claude Max, ChatGPT/Codex, SuperGrok, Gemini) into one OpenAI/Anthropic-compatible endpoint on `127.0.0.1:4100` — with a credential vault, automatic token refresh, model routing, format translation, full trace capture, and usage/limit tracking.

Point any coding harness (Claude Code, Codex CLI, grok, opencode, …) at it and every request is authenticated with the right subscription, billed to the right bucket, and captured to SQLite for inspection.

## Features

- **Credential vault** — imports OAuth tokens from Claude Code, Codex, grok, and gemini CLIs; stores them in `~/.alexandria/accounts/` (0600); refreshes them itself (Anthropic, OpenAI, xAI)
- **One endpoint, every provider** — `/v1/messages`, `/v1/chat/completions`, `/v1/responses` with cross-format translation and model routing (`claude-*` → Anthropic, `gpt-*` → Codex, `grok-*` → xAI)
- **Login flows built in** — `alex auth login claude|codex|grok` (PKCE paste, loopback, and xAI device-code flows); also exposed over HTTP so GUIs can drive re-auth
- **Trace capture & sessions** — every request/response stored with tokens, cost, latency; group runs with `x-session-id`, tag with `x-alexandria-*` headers, search body text, stitch transcripts
- **Trace Browser & TUI** — a two-pane live trace browser in the menu bar app, `alex tui` in the terminal, `alex traces --json` for scripts
- **Limits & health** — subscription plan windows (5h/7d) with utilization and reset times, per-provider heartbeats, `alex ping`, `alex status`
- **Cost analytics** — per-model requests/tokens/cost with subscription-vs-API billing buckets (`/admin/analytics`)
- **Dario mode** — optional generational supervisor for the `@askalf/dario` Anthropic upstream with health probes, npm auto-update, and rolling restarts
- **macOS menu bar app** — live gauges, re-auth windows, ping checks, window-reset alerts in `macos/` (AlexandriaBar)
- **Harness smoke tests** — `alex harness run` executes frozen CLI harnesses (claude, codex, grok, …) in Docker against the proxy and verifies traces land
- **Zero-downtime upgrades** — `./install.sh --upgrade` blue-greens the daemon on a shared port (SO_REUSEPORT)
- **Cross-platform CLI** — Linux, macOS, and Windows binaries on every release (`cargo install alex`)

## Install

```bash
cargo install alex        # installs the `alex` and `alexandria` binaries
# or from a checkout:
./install.sh              # release build → /usr/local/bin/alex (+ alexandria symlink)
./install.sh --service    # also run at login (launchd/systemd)
```

## Quick start

```bash
alex auth import          # pull credentials from your existing CLI logins
alex daemon --background  # start the daemon on 127.0.0.1:4100
alex status               # accounts, limits, health at a glance
eval "$(alex env)"        # point ANTHROPIC_/OPENAI_/XAI_ env at the proxy
```

Re-auth a subscription any time with `alex auth login claude|codex|grok`, watch live traffic with `alex tui`, and check window utilization with `alex limits`.

## Sessions & trace tagging

Generate client credentials for the proxy, then tag requests so every trace from a run lands in one named session:

```bash
eval "$(alex env)"        # exports ANTHROPIC_/OPENAI_/XAI_ vars pointing at the proxy
alex credentials --json   # same thing as JSON (alias: creds); --host host.docker.internal for containers

SESSION="experiment-42"
curl -s "$OPENAI_BASE_URL/chat/completions" \
  -H "authorization: Bearer $OPENAI_API_KEY" \
  -H "x-session-id: $SESSION" \
  -H "x-alexandria-task: sparql-university" \
  -H "x-alexandria-job: cove-sparql-1" \
  -H "x-alexandria-trace-tag: attempt=1" \
  -d '{"model":"gpt-5.5","messages":[{"role":"user","content":"hi"}]}'
```

Every response carries an `x-alexandria-trace-id`. Typed headers (`x-alexandria-harness|task|model|job`) and free-form `x-alexandria-trace-tag: key=value` tags are captured with each trace. Collect a session back out:

```bash
alex traces --session "$SESSION" --json                 # CLI
curl -H "x-api-key: <local_key>" \
  "http://127.0.0.1:4100/admin/traces?session=$SESSION" # HTTP
curl -H "x-api-key: <local_key>" \
  "http://127.0.0.1:4100/traces/sessions/$SESSION/transcript"  # stitched transcript
curl -H "x-api-key: <local_key>" \
  "http://127.0.0.1:4100/traces/search?text=professor&session=$SESSION"  # body-text search
```

## Trace Browser

<p align="center">
  <img src="https://raw.githubusercontent.com/madhavajay/alex/main/docs/images/browser.png" width="720" alt="Trace Browser: session list with tags and cost, live transcript pane, omni search with typed filters" />
</p>

The menu bar app's Trace Browser gives the same data a UI: two-pane sessions + live transcript, an omni bar combining free text with `model:`, `harness:`, `task:`, `job:`, `tag:key=value`, `status:`, `run:` and `session:` filters, live/pin modes, and per-turn token/cost breakdowns.

## macOS menu bar app

<p align="center">
  <img src="https://raw.githubusercontent.com/madhavajay/alex/main/docs/images/mac-menu.png" width="440" alt="AlexandriaBar menu: subscription limit gauges, account status, dario, and actions" />
</p>

`macos/` contains **AlexandriaBar**, a Swift menu bar app that shows daemon health, subscription limit gauges, and account status — with in-app re-auth (device codes, paste flows), ping checks, and notifications when a subscription needs attention.

```bash
cd macos && ./Scripts/run.sh   # build + launch dist/AlexandriaBar.app
```

### Re-auth helpers

<p align="center">
  <img src="https://raw.githubusercontent.com/madhavajay/alex/main/docs/images/reauth.png" width="420" alt="Re-authenticate Codex: open the authorization page, approve in the browser, finishes automatically" />
</p>

When a subscription expires you get a notification and a one-click fix. Each provider gets the flow that suits it: **Codex** opens the browser and finishes automatically via the localhost callback (above), **Grok** shows an xAI device code you can enter from any device, and **Claude** takes the pasted `code#state`. The same flows are served by the daemon (`POST /admin/auth/login/start`, poll `GET /admin/auth/login/<id>`), so any UI can drive them — the terminal equivalent is `alex auth login <provider>`.

## Crates

| Crate | What it is |
|---|---|
| [`alex`](https://crates.io/crates/alex) | The daemon + CLI (binaries: `alex`, `alexandria`) |
| [`alex-core`](https://crates.io/crates/alex-core) | Routing, translation, usage & pricing logic (pure, no I/O) |
| [`alex-auth`](https://crates.io/crates/alex-auth) | Credential vault, OAuth/device login flows |
| [`alex-store`](https://crates.io/crates/alex-store) | SQLite trace store & analytics |
| [`alex-proxy`](https://crates.io/crates/alex-proxy) | axum ingress, admin API, upstream clients |

## Development

```bash
./alex daemon             # dev shim (cargo run)
./test.sh                 # tiered tests: unit | wire | harness | dario
cd macos && swift test    # menu bar app tests
```

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
