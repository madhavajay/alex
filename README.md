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
- **One endpoint, every provider** — `/v1/messages`, `/v1/chat/completions`, `/v1/responses`, `/v1beta/models/{model}:generateContent` with cross-format translation and model routing (`claude-*` → Anthropic, `gpt-*` → Codex, `grok-*` → xAI, `gemini-*` → Gemini)
- **Login flows built in** — `alex auth login claude|codex|grok` (PKCE paste, loopback, and xAI device-code flows); also exposed over HTTP so GUIs can drive re-auth
- **Trace capture & sessions** — every request/response stored with tokens, cost, latency; group runs with `x-session-id`, tag with `x-alexandria-*` headers, search body text, stitch transcripts
- **Trace Browser & TUI** — a two-pane live trace browser in the menu bar app, `alex tui` in the terminal, `alex traces --json` for scripts
- **Limits & health** — subscription plan windows (5h/7d) with utilization and reset times, per-provider heartbeats, `alex ping`, `alex status`
- **Cost analytics** — per-model requests/tokens/cost with subscription-vs-API billing buckets (`/admin/analytics`)
- **Dario mode** — an always-prepared generational supervisor for the `@askalf/dario` Anthropic upstream with health probes, automatic updates, and rolling restarts; routing remains an explicit toggle
- **macOS menu bar app** — live gauges, re-auth windows, ping checks, window-reset alerts in `macos/` (AlexandriaBar)
- **Harness smoke tests** — `alex harness run` executes frozen CLI harnesses (claude, codex, grok, …) in Docker against the proxy and verifies traces land
- **Harness regression lane** — `scripts/harness-regression.sh` uses per-cell scoped keys and verifies persisted trace API data, rather than trusting a harness exit code
- **Self-updating** — `alex update` fetches the release manifest, sha256-verifies the binary, and re-points the daemon to the new build. Today the swap **gracefully drains in-flight requests** before the restart, so nothing in flight is dropped (brand-new connections see a brief blip during the handover); **fully zero-downtime blue-green handover — new daemon takes over the socket before the old one exits — is coming soon.** The menu bar app keeps itself current via Sparkle and offers a one-click "Update daemon…" menu item
- **Cross-platform CLI** — Linux, macOS, and Windows binaries on every release (`cargo install alex`)

## One-liner: a coding agent on your subscriptions in seconds

From a bare machine to a running coding agent wired to your alex proxy, in one command.
It installs the harness if missing, points it at alex with a scoped key, and launches it —
skipping any step that's already done:

```bash
curl -fsSL https://raw.githubusercontent.com/madhavajay/alex/beta/v0.1.26/up.sh \
  | sh -s -- --harness pi --url https://<your-alex-host>:4100 --key <scoped-key> --model alex/gpt-5.6-sol
```

If `alex` is already installed, `up.sh` just calls the built-in orchestrator directly:

```bash
alex up pi                                   # install Pi if needed, connect to the local daemon, launch
alex up pi --model alex/fable-5              # pick the model
alex up codex --url https://<host>:4100 --key <scoped-key>   # point a worker at a remote alex
```

The `--key` is a **model-only scoped key** (mint one with `POST /admin/run-keys`) — safe to paste
onto another machine, since it can make model calls but nothing else. Over Tailscale the key rides
the encrypted tailnet. Supported today: **Pi** and **Codex** (more to come).

## What it can do

> Screenshots below are placeholders — drop real captures at the referenced `docs/img/…` paths.

### Use fable-5 (or any premium Anthropic model) inside Pi

Premium Claude models work in a non-Claude-Code harness, brokered through your Claude
subscription via [Dario](#dario-mode):

```bash
npm install -g @earendil-works/pi-coding-agent   # install the Pi harness
alex harness connect pi                           # adds alex/* models + a tracing hook
pi --model alex/fable-5                            # fable-5, served from your Claude sub
```

### Use an OpenAI model inside Claude Code

Cross-format translation lets Claude Code call a GPT model as if it were Claude:

```bash
alex harness connect claude                       # writes an Alexandria settings profile
claude --model alex/gpt-5.6-sol                    # GPT-5.6 Sol, translated for Claude Code
```

### Bond two Codex subscriptions in round-robin

```bash
alex auth login codex                             # add the first OpenAI/Codex subscription
alex auth login codex                             # add a second (different account)
# alternate across both subs, with automatic failover on 429 / rate limits:
curl -X PUT -H "x-api-key: <local_key>" -H "content-type: application/json" \
  -d '{"strategy":"round_robin"}' \
  http://127.0.0.1:4100/admin/routing/openai
```

Strategies: `round_robin`, `priority`, `reset_first` — also settable in the app's Providers
settings. (`<local_key>` is your daemon key from `~/.alexandria/config.toml`.)

### See subagent traces and costs

Every request is captured with tokens, cost, and latency; sub-agent calls nest under their
parent session so you can see the full tree and what each branch spent.

![Subagent traces and costs in the Trace Browser](docs/img/subagent-traces.png)

### Capture traces from Amp, Cursor, and other wrapped harnesses

Harnesses that don't take a custom endpoint are captured with a reverse wrap — full
conversation traces, no config changes to the tool:

```bash
alex wrap amp   -- -x 'refactor the auth module'      # Amp, fully traced
alex wrap agent -- --print 'summarize this repo'      # Cursor Agent, fully traced
```

![Amp conversation captured through alex wrap](docs/img/wrap-amp-trace.png)

### Know when a subscription needs re-auth

When a subscription's token is revoked or expires, alex classifies the failure as an `auth`
error and surfaces it in the Trace Browser and the menu bar so you can re-authenticate.
_(Push notifications — Telegram / webhook — are on the roadmap.)_

![Re-auth prompt in the menu bar](docs/img/reauth-alert.png)

## Runs everywhere

The daemon and CLI ship as prebuilt binaries for every common platform. The Linux builds are
**fully static musl** (no glibc, no OpenSSL) — they run in any container image, down to
`scratch`/`distroless`/Alpine.

| OS | Arch | Artifact | Notes |
| --- | --- | --- | --- |
| macOS | Apple Silicon (aarch64) | signed `.app` + CLI | native menu-bar app |
| macOS | Intel (x86_64) | CLI | |
| Linux | x86_64 | static-musl binary + gnu tarball | musl runs in any container |
| Linux | aarch64 | static-musl binary | arm64 servers / CI runners |
| Windows | x86_64 | zip | CLI |

Each release also publishes `checksums.txt` (sha256) so the static Linux binaries can be
pulled and verified by URL per version.

## Install

Install the precompiled CLI/daemon and, on macOS, the menu-bar app:

```bash
curl -fsSL https://raw.githubusercontent.com/madhavajay/alex/main/install-release.sh | sh
```

The macOS path uses Homebrew: the formula installs the `alex` CLI/daemon, the
cask installs `AlexandriaBar.app`, and the bootstrap registers the daemon with
launchd. The DMG by itself contains only the menu-bar app; it does not install
the daemon. Linux downloads and SHA-256 verifies the precompiled x86_64 release
binary, so neither platform needs a Rust compiler.

Equivalent manual commands:

```bash
brew install madhavajay/alex/alex               # CLI + daemon
brew install --cask madhavajay/alex/alexandria  # macOS menu bar app
# or:
cargo install alex        # installs the `alex` and `alexandria` binaries
# or from a checkout:
./install.sh              # release build → /usr/local/bin/alex (+ alexandria symlink)
./install.sh --service    # also run at login (launchd/systemd)
```

Alexandria prepares Dario during installation and again when the daemon starts.
Dario requires Node.js 18 or newer; the package install automatically tries npm,
pnpm, then Bun. `alex dario enable` routes non-Claude-Code Anthropic traffic
through the warm Dario generation after Alexandria is restarted, while
`alex dario disable` keeps it ready but returns that traffic to direct routing.

### Beta channel

Betas ship ahead of stable so new features can be tested on real builds. The catch
is that the channel setting lives *inside* the beta build, so a stable install has
no way to ask for one. This installer is the way in:

```bash
curl -fsSL https://raw.githubusercontent.com/madhavajay/alex/main/install-beta.sh -o /tmp/install-beta.sh
sh /tmp/install-beta.sh
```

Download it, then run it — don't pipe it straight to `sh`. Piped, the script *is*
the shell's stdin, so the shell and any child process it spawns are both reading
the same pipe; the shell can stop reading before `curl` has finished writing, and
a slow enough connection would leave it executing a truncated script. Running it
from a file is deterministic.

It resolves the newest **prerelease** (GitHub's `releases/latest` never points at
one, which is why the normal installer can't reach a beta), SHA-256 verifies and
installs the CLI, replaces a running daemon via `service restart` (which waits for
in-flight requests rather than cutting a session off), then quits AlexandriaBar,
installs the signed app bundle into `/Applications`, and relaunches it. It sets
your update channel to `beta`. It does not use Homebrew — there is no beta cask.

Both halves must land for the menu bar to show the new version: the menu's
`Alexandria app v… · daemon v…` line reports the **app bundle** and the **running
daemon** separately, and they update independently.

That bootstrap is a one-time cost. From then on betas arrive as ordinary updates:

```bash
alex update                      # follows the configured channel
alex update --set-channel beta   # or stable, to go back
```

In the app: **Preferences → Updates → Release channel**. Switching back to stable
always works — the baked-in Sparkle feed URL never changes, only the channel does.
Stable users are never offered a prerelease.

To pin one exact build: `ALEX_BETA_TAG=v0.1.26-beta.1 sh install-beta.sh`.

## Quick start

```bash
alex auth import          # pull credentials from your existing CLI logins
alex daemon --background  # start the daemon on 127.0.0.1:4100
alex status               # accounts, limits, health at a glance
eval "$(alex env)"        # point ANTHROPIC_/OPENAI_/XAI_ env at the proxy
```

The default listener is loopback-only. To make the authenticated proxy
reachable on both loopback and the local network, persist a wildcard bind and
restart the daemon:

```bash
alex config host 0.0.0.0
# restart the Alexandria daemon or menu-bar app service
```

The menu-bar app exposes the same opt-in under Settings → General → Daemon.
LAN mode exposes port 4100 to other devices on the network; keep the generated
credentials private and use host firewall rules on untrusted networks. Restore
local-only access with `alex config host 127.0.0.1` and another restart.

Re-auth a subscription any time with `alex auth login claude|codex|grok`, watch live traffic with `alex tui`, and check window utilization with `alex limits`.

## Format translation

Alexandria speaks four API dialects on the way **in** and routes to whichever provider owns the **model** you name — translating the request, the response, and the streaming events in between. Point any client's SDK at the proxy and use any model; the wire format is converted for you (all conversions pivot through the Anthropic Messages shape internally).

| Client sends (ingress) | Endpoint | → can drive any upstream |
|---|---|---|
| Anthropic Messages | `POST /v1/messages` | Anthropic · Codex · Gemini |
| OpenAI Chat Completions | `POST /v1/chat/completions` | Anthropic · Codex · Gemini · xAI |
| OpenAI Responses | `POST /v1/responses` | Anthropic · Codex · Gemini |
| Gemini generateContent | `POST /v1beta/models/{model}:generateContent` | Anthropic · Codex · Gemini |

The upstream is chosen purely by the model name, so e.g. an **OpenAI Chat** request naming `claude-opus-4-8` is translated to Anthropic and back; a **Gemini** request naming `gpt-5.5` is translated to Codex and returned as Gemini `candidates`. Streaming works across the matrix — SSE events are re-synthesized in the client's dialect.

### GPT-5.6 through Pi

Alexandria advertises the Codex subscription models `gpt-5.6-sol`, `gpt-5.6-terra`, and `gpt-5.6-luna`. Running `alex harness connect pi` writes them into Pi's `alexandria` provider with their 372K context, 128K maximum output, image input, pricing metadata, and adaptive reasoning configuration. It also installs a small Pi extension that sends Pi's real session ID only on requests using the `alexandria` provider, keeping identical prompts in separate trace-browser sessions. Pi's `minimal` level maps to Codex `low`; `low`, `medium`, `high`, and `xhigh` pass through unchanged.

The live Codex catalog also supports `max` on all three models and `ultra` on Sol and Terra. Pi's standard thinking-level UI currently ends at `xhigh`, so `max` and `ultra` are available to clients that can send raw Responses API reasoning effort, but are not relabeled or silently substituted for Pi's `xhigh` option. `ultra` additionally enables Codex's automatic task delegation.

### Serving the Gemini CLI with a different model

Point the Gemini CLI at the proxy and let it run on, say, `gpt-5.5` — requests arrive as Gemini, get rewritten to OpenAI/Codex upstream, and responses convert back to Gemini shape:

```bash
eval "$(alex env)"   # sets GOOGLE_GEMINI_BASE_URL, GEMINI_API_KEY, bearer mechanism, GOOGLE_GENAI_USE_GCA=false
gemini --model gpt-5.5 --prompt 'Reply with only PONG'
```

One-time: the Gemini CLI must use API-key auth, not its Google login. Set it in `~/.gemini/settings.json`:

```json
{ "security": { "auth": { "selectedType": "gemini-api-key" } } }
```

(or pick "Use API key" once in the CLI's `/auth` menu). Then `alex env` points it at the proxy — `GOOGLE_GEMINI_BASE_URL` has no `/v1`, since the SDK appends `/v1beta` itself, and `GEMINI_API_KEY_AUTH_MECHANISM=bearer` sends the key as `Authorization: Bearer`. Temp tokens from `POST /admin/run-keys` carry the same env block in their `exports` field, so a tagged run can drive the Gemini CLI too.

Direct curl (non-streaming and streaming):

```bash
curl -H 'Authorization: Bearer <local_key>' -H 'Content-Type: application/json' \
  -X POST 'http://127.0.0.1:4100/v1beta/models/gpt-5.5:generateContent' \
  -d '{"contents":[{"role":"user","parts":[{"text":"Reply with only PONG"}]}]}'

curl -N -H 'Authorization: Bearer <local_key>' -H 'Content-Type: application/json' \
  -X POST 'http://127.0.0.1:4100/v1beta/models/gpt-5.5:streamGenerateContent?alt=sse' \
  -d '{"contents":[{"role":"user","parts":[{"text":"Reply with only PONG"}]}]}'
```

To use your **Gemini subscription** as an upstream instead (name a `gemini-*` model), alexandria authenticates via the gemini-cli OAuth token it imported and Google's Code Assist API. Individual accounts are onboarded automatically; **Workspace/Enterprise accounts must supply a GCP project** with the Code Assist API enabled — set `gemini_project = "your-gcp-project"` in `~/.alexandria/config.toml` (or `GOOGLE_CLOUD_PROJECT`).

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

For deterministic local harness testing, authenticated local-key or scoped **harness-key** requests may add `x-alexandria-simulate-error: STATUS[:kind]` (for example `429:rate_limit_error`, `401:authentication_error`, `529:overloaded_error`, or `503`). Alexandria returns the matching provider error envelope and records a normal trace tagged `simulated=true`; it never contacts an upstream or affects account cooldowns.

```bash
alex traces --session "$SESSION" --json                 # CLI
curl -H "x-api-key: <local_key>" \
  "http://127.0.0.1:4100/admin/traces?session=$SESSION" # HTTP
curl -H "x-api-key: <local_key>" \
  "http://127.0.0.1:4100/traces/sessions/$SESSION/transcript"  # stitched transcript
curl -H "x-api-key: <local_key>" \
  "http://127.0.0.1:4100/traces/search?text=professor&session=$SESSION"  # body-text search
```

## Tool capture

Model traces show the tool calls a model *requested*; tool capture additionally records what the harness *actually executed* — tool name, arguments, result, and error status — and joins it to the same session and turn. Execution events arrive on a separate ingest endpoint (`POST /tool-events`, authenticated with the harness key), never ride on model traffic, and are best-effort: a failed telemetry post never blocks or alters a tool run.

Each harness reports through the integration surface it supports:

| Harness | Mechanism |
| --- | --- |
| Pi | in-process TypeScript extension (`tool_execution_start`/`tool_execution_end`) |
| Claude Code | `PreToolUse`/`PostToolUse` lifecycle hooks in the Alexandria settings profile |
| Codex | `PreToolUse`/`PostToolUse` hooks in `hooks.json` (requires `features.hooks`) |
| Amp | system plugin `tool.call`/`tool.result` handlers |
| Cursor | requested tool calls imported from transcripts (no execution results) |

Hook payloads are sent as-is; the daemon normalizes native shapes (`hook_event_name`, `tool_use_id`, `tool_input`, `tool_response`) into one record, strips secrets from arguments and results, and stores rows in the `tool_calls` table with bodies on disk.

Capture is off by default and toggled per harness (pi, claude, codex, amp):

```bash
curl -X PUT -H "x-api-key: <local_key>" -H "content-type: application/json" \
  -d '{"enabled":true}' \
  "http://127.0.0.1:4100/admin/harnesses/claude/tool-capture"
```

Toggling rewrites the harness's hook/extension/plugin files in place; restart the harness to pick them up. Executed tools come back on the stitched transcript as `turns[].executed_tools`, with argument and result bodies at `GET /tools/{id}/body/{args|result}`. The Trace Browser pairs requested and executed calls per turn (requested → running → executed/failed). Other harnesses currently show requested tool calls inferred from model traffic only; the support matrix lives in `crates/alex/tests/harness_matrix.rs`.

## Remote wrap capture

`alex wrap agent` and `alex wrap amp` can run on a different machine while sending their normalized conversation traces to a central Alexandria daemon. The remote reverse wrap still connects directly to Cursor or Amp; only the captured trace records and bodies are uploaded to Alexandria.

On the central Alexandria machine, expose the daemon through HTTPS (or a trusted encrypted private network), then mint an ingest-only credential. The daemon defaults to loopback; if your TLS gateway is on another host, bind an appropriate private interface with `alex daemon --host <private-ip>` and restrict that listener with the host firewall.

```bash
alex keys mint --kind wrap --label remote-mac --json
```

The secret is shown once. On the remote machine, configure the central trace destination and run either wrapper:

```bash
export ALEXANDRIA_TRACE_URL=https://alex.example.net
export ALEXANDRIA_TRACE_KEY=alxk-...     # the minted kind=wrap key

./alex wrap agent
./alex wrap amp
```

For a persistent machine, prefer a mode-`0600` key file instead of a shell environment variable:

```bash
./alex wrap agent \
  --trace-url https://alex.example.net \
  --trace-key-file ~/.config/alexandria/wrap.key
```

Environment equivalents are `ALEXANDRIA_TRACE_URL`, `ALEXANDRIA_TRACE_KEY`, and `ALEXANDRIA_TRACE_KEY_FILE`. Plain `http://` is accepted automatically only for loopback; a trusted private-network endpoint requires `--allow-insecure-http` (or `ALEXANDRIA_TRACE_ALLOW_INSECURE_HTTP=1`). Internet-facing deployments should use HTTPS.

Remote traces are always spooled into the remote machine's local `~/.alexandria` first. A failed preflight or later upload does not stop Cursor Agent or Amp; the wrapper prints a warning and keeps the local capture. Replay the run after connectivity is restored:

```bash
alex traces push --run-id wrap-agent-1783641674567-46419171 \
  --trace-url https://alex.example.net \
  --trace-key-file ~/.config/alexandria/wrap.key
```

The central daemon accepts `kind=wrap` credentials only on `GET/POST /traces/ingest`; they cannot invoke models or read/administer traces. Revoke a remote machine with `alex keys revoke <rk-id>`.

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
./scripts/harness-regression.sh  # real Docker harnesses + host-side trace assertions
cd macos && swift test    # menu bar app tests
```

Build Linux container binaries with `./scripts/build-linux.sh`; it builds only the release `alex` binary for x86_64 and aarch64 musl, preferring `cargo zigbuild`, then Docker-backed `cross`, and finally a locally installed Rust musl toolchain. The script prints the two output paths when it finishes.

## Got a bug or feature request?

Open one — bugs, ideas, and feature requests all go to
[GitHub Issues](https://github.com/madhavajay/alex/issues/new). The macOS app has the same link
under **Report a Bug or Request a Feature…** in the menu bar and in Settings → General.

Built by [madhavajay](https://github.com/madhavajay/) — message me on X:
[@madhavajay](https://x.com/madhavajay).

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
