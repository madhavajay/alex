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
- **Login flows built in** — `alex auth login claude|codex|grok` (PKCE paste, loopback, and xAI device-code flows); also exposed over HTTP for GUIs
- **Trace capture** — every request/response stored with tokens, cost, latency, and session correlation; browse via `alex traces`, `alex tui`, or the trace API
- **Limits & health** — subscription plan windows (5h/7d) with utilization and reset times, per-provider heartbeats, `alex ping`, `alex status`
- **Dario mode** — optional generational supervisor for the `@askalf/dario` Anthropic upstream with zero-downtime rolling restarts
- **macOS menu bar app** — live gauges, re-auth windows, ping checks, and alerts in `macos/` (AlexandriaBar)
- **Zero-downtime upgrades** — `./install.sh --upgrade` blue-greens the daemon on a shared port (SO_REUSEPORT)

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

## macOS menu bar app

<p align="center">
  <img src="https://raw.githubusercontent.com/madhavajay/alex/main/docs/images/mac-menu.png" width="440" alt="AlexandriaBar menu: subscription limit gauges, account status, dario, and actions" />
</p>

`macos/` contains **AlexandriaBar**, a Swift menu bar app that shows daemon health, subscription limit gauges, and account status — with in-app re-auth (device codes, paste flows), ping checks, and notifications when a subscription needs attention.

```bash
cd macos && ./Scripts/run.sh   # build + launch dist/AlexandriaBar.app
```

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
