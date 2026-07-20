# Changelog

All notable changes to Alex are documented here. Format loosely follows
[Keep a Changelog](https://keepachangelog.com/); versions follow the `vX.Y.Z`
git tags (stable) and `vX.Y.Z-beta.N` (beta channel). Releases before 0.1.27
predate this file — see the git history and GitHub releases.

## [Unreleased]

## [0.1.29-beta.6] - 2026-07-20

### Fixed
- **Anthropic requests no longer escape Dario after a reset.** Every eligible
  non-Claude-Code Anthropic request is routed through Dario and fails closed if
  its generation or prompt cache is unavailable; genuine Claude Code remains
  the only direct Anthropic path.
- **Fresh and reset installs reliably enter onboarding.** A missing daemon
  config is bootstrapped automatically, resetting Alex reopens onboarding, and
  a zero-provider menu now exposes **Start Onboarding** at the top.
- **Onboarding is resumable and testable.** Connected provider accounts can be
  selected without repeating OAuth, changing provider clears stale test state,
  and **Check for Request** manually checks for the copied harness command.
- **Fresh provider state is no longer misleading.** Newly authenticated
  accounts show Active before their first health probe, while Dario/Claude stay
  hidden until an Anthropic account exists.
- **Old clients can be recovered without accepting unknown secrets.** Requests
  rejected by Alex now appear in an **Alex Error** Trace Browser section. A
  known revoked or expired client can be right-clicked and **Approve**d; unknown
  credentials remain visible but cannot be authorized.
- **OpenAI-compatible model discovery uses the public namespace.** `/v1/models`
  now advertises proxy aliases as `alex/*`, not the legacy `alexandria/*` form.

## [0.1.28] - 2026-07-19

First stable release of the 0.1.28 line — everything from the 0.1.28 betas plus
the fixes below.

### Fixed
- **Kimi uninstall/reinstall no longer bricks the harness.** The app's
  disconnect API had no kimi case and fell through to the Pi handler: it revoked
  the run keys but edited the wrong file, leaving Kimi wired to a dead key with
  no way to reconnect ("401 run key expired or revoked"). Disconnect now edits
  `~/.kimi-code/config.toml` properly, reconnect adopts an orphaned Alex
  provider (identified by its `alxk-` key) instead of refusing, and cleanup
  works even when the managed-state marker file is lost. A user-authored
  provider that shares the name is still left alone.
- **Harness binaries are found even when they're not on the daemon's PATH.**
  Detection now also probes `<config dir>/bin/<binary>` (kimi installs itself at
  `~/.kimi-code/bin/kimi`, visible only to interactive shells), so
  connect no longer fails after a disconnect already revoked the old key.
- **Non-streaming failures are no longer masked as successes.** Buffered/
  destreamed responses (OpenAI Responses non-stream, chat bridge, Gemini
  generateContent) that carried an upstream `response.failed` error were
  synthesized into HTTP 200 responses with no usage recorded — hiding provider
  errors and undercounting cost. They now surface as classified 502s, and
  successful buffered responses record usage before the trace is finalized.
- **Provider health goes green after a successful re-auth.** Completing a
  Telegram re-auth now stamps the account's probe health (and Dario-routed
  traffic attributes health to the bonded account), so the provider no longer
  sits on a purple "unknown" dot after reporting success. The Providers pane
  also keeps its content visible while refreshing instead of blanking.
- **Exo model list is usable.** Model toggle rows showed bare switches with no
  label text; names/details render again, and running/enabled models sort to
  the top of the catalog.
- **Notifications pane polish.** "Send test message" is always pressable (with
  a clear error when there's nothing to send with); the Bot token row no longer
  wraps the Replace button or squeezes the connected-bot caption.
- **No more repeated "Alex wants to access network volumes" prompts.** Both the
  app's node runtime probe and the daemon's harness binary detection skip
  network volumes and TCC-protected folders (Desktop/Documents/Downloads)
  instead of stat-ing every PATH entry.

### Added
- **Detailed Telegram `/status`.** Subscriptions & limit windows (plan,
  utilization %, reset time) and per-provider ping health (✓/✗, age, error)
  alongside the existing daemon/account sections, truncation-safe for
  Telegram's message limit.
- **Run-key management.** Sortable columns in the Credentials key table,
  **Revoke all** and **Clear revoked** (new admin endpoints), and a per-key
  **Traces** button that opens the Trace Browser pre-filtered by that key's
  fingerprint (new `key:` omni-query token, server-side `key_fingerprint`
  filter).
- **Telegram channel controls.** Allow-commands toggle per channel, a recent
  messages in/out activity panel, and Test works for saved channels using the
  daemon-stored token.
- **Kimi harness smoke runner + H7 test cell.** `./test.sh harness` can now run
  the kimi CLI in Docker against an Anthropic-subscription Claude model via
  Dario, driving a real tool call.
- **Instant Harnesses page.** `/admin/harnesses` serves a warm cache with
  background refresh (version probes memoized by binary mtime), so the pane
  renders immediately instead of blocking ~5s on version subprocesses.

### Changed
- **OpenRouter settings moved into Providers → OpenRouter** (API key +
  curated model exposure in one place); the standalone sidebar entry is gone.
- **README**: per-harness tracing table (traces/subagents), providers &
  subscriptions table with coming-soon list, provider docs say "Alex".
- **CI**: the pre-release Windows job no longer fails the workflow (tracked
  separately until Windows ships).
- **User-facing product name is Alex everywhere** (217 strings across app,
  CLI, daemon messages, and docs); functional identifiers, paths, and config
  keys are unchanged.

## [0.1.28-beta.10] - 2026-07-19

### Fixed
- **Daemon liveness check no longer needs the `kill` binary.** `process_running`
  now uses a direct `libc::kill(pid, 0)` syscall instead of spawning `kill`,
  which is absent on some minimal hosts. (beta.9's release build failed on this;
  beta.10 supersedes it — no beta.9 was published.)
- **Anthropic re-auth actually works now.** A permanent token-refresh failure
  (`invalid_grant` / `reauth_required` / no refresh token) now persists a
  `needs_reauth` marker, so the account stops reporting **Active** when it is in
  fact broken, and the watchdog sends the paste-mode Telegram re-auth link end to
  end. This is the fix if Anthropic re-auth "still wasn't working" on beta.8.
- **Dario status line is back under the Anthropic provider.** Dario is only used
  for Anthropic subscription traffic, so its menu-bar line is nested under the
  Anthropic provider card again instead of a standalone section — while still
  always showing when Dario is enabled (red when down), even if the Anthropic
  account itself is unreachable.
- **Trace Browser shows OpenRouter / Kimi / xAI replies.** Streamed `openai-chat`
  passthrough responses previously rendered a blank assistant turn; the transcript
  now reassembles the streamed SSE body (text + tool calls).
- **Daemon update from the UI** no longer prints `kill: No such process` when the
  old daemon has already exited — a graceful drain treats that as success.
- **crates.io images render** — the README now uses absolute image URLs.

### Added
- **Provider detail health + re-auth controls (macOS).** A live status pill that
  never shows Active when the account is broken, a **Ping** button that runs a
  real round-trip and refreshes the pill, and a pending re-auth panel: view the
  authorize URL / user code, paste `code#state` by hand, **Reset/cancel**, or
  **Overwrite** a stuck pending session.
- **Force-overwrite + cancel a pending re-auth** — `reauth-notify` accepts
  `force:true` and a new local-key-gated `POST /admin/auth/reauth-cancel` backs
  the app's Overwrite/Reset.
- **Update the daemon from the menu banner / Preferences.**

### Internal
- Harness test suite now proves tool calls and both-sides trace capture/display
  across every API dialect (anthropic, `openai-chat` incl. streamed SSE,
  `openai-responses`, gemini), with gated live cells for real harness runs.
- New structured reference docs under `docs/` (overview, CLI, providers/routing,
  API/formats, Dario, traces, configuration), linked from the README.

## [0.1.28-beta.8] - 2026-07-18

### Added
- **Text the daemon from Telegram — inbound commands.** Opt in per channel with
  `allow_commands = true`, and only your configured `chat_id` is honored. Commands:
  - `/status` — full status in chat: version + update-available, daemon/service
    uptime, Dario health, and per-account status, %usage, and expiry.
  - `/ping`, `/help`.
  - `/reauth <provider>` — start a re-auth and get the link; `/reauth` alone lists
    accounts that need it.
- **Complete a paste-code re-auth from Telegram.** Device-flow providers (xAI,
  Kimi, OpenAI) already finished on their own after you tapped the link. Anthropic
  needs a `code#state` pasted back — now you can paste it straight into the chat
  (or `/code <value>`) and the daemon completes the login. This closes the gap
  where an Anthropic re-auth link could be sent but never finished. The command
  bus is transport-independent with a reserved hook for a future natural-language
  handler.

### Security
- Inbound commands are off by default (`allow_commands`), accepted only from the
  allowlisted `chat_id`, and code submission additionally requires the channel to
  be command-enabled; pasted codes are never logged and OAuth error bodies are
  redacted before replying.

## [0.1.28-beta.7] - 2026-07-18

### Fixed
- **Kimi disconnect/remove now leaves Kimi working.** If you had selected an
  `alex/*` model as Kimi's default, removing the Kimi connection used to leave
  Kimi pointing at a deleted model — every request then failed with `401 run
  key expired or revoked` (or `No model configured`). Disconnect now restores
  your native default (`kimi-code/k3` from the pre-connect backup, else a
  surviving `kimi-code/*` model). Live-verified with the real Kimi CLI.
- **Harnesses pane refreshes immediately** after connect/remove instead of
  showing stale state until the next poll; the action button now reads
  **Install** when a harness is disconnected and **Update** only when it is
  connected.
- **Release appcast publishing no longer hangs.** The Sparkle appcast is now
  written to gh-pages via the GitHub Contents API instead of a full branch
  clone, which repeatedly hung ~30 min on the macOS runner and left the
  in-app updater showing an older version as "newest". (This is why beta.6's
  appcast was slow to flip.)

### Note
- The `kimi → claude/fable-5` "bio" refusal some users hit is **not** an Alex
  bug: it is Anthropic's server-side safety classifier reacting to genomics
  filenames that Kimi includes in its injected working-directory listing.
  Root-caused to a one-line repro; Alex already surfaces it cleanly. Use a
  non-Anthropic model for that content.

## [0.1.28-beta.6] - 2026-07-18

### Added
- **Tappable Telegram re-auth links** — when a provider logs out, the reauth
  alert now carries a tappable device-flow link: tap it, approve on the
  provider's site, and the daemon completes the re-login by itself (no codes
  to paste). Fires automatically from the reauth watchdog and the request
  path; trigger it manually with the new "Re-authenticate via Notification"
  button in Settings → Providers or `POST /admin/auth/reauth-notify`. If a
  link expires unused, the next watchdog tick sends a fresh one (never two
  concurrent flows for the same account).
- **Refusal handling + harness×provider test suite** — upstream refusals /
  empty completions surface as a clear non-empty assistant message with a
  clean stop instead of a silent empty 200; traces are marked
  `upstream_refusal`; harnesses stop retry-looping. Covered by offline
  fixtures for every client format plus env-gated live e2e.
- **OpenRouter curation** — pick exactly which OpenRouter models your
  harnesses see (known-catalog → exposed two-list picker, alphabetical
  everywhere); ships with a small curated starter set.
- **Exo as a first-class provider** — Exo now appears as a provider row
  (status/health/models) instead of a separate sidebar tab; all cluster
  config lives in the provider detail pane.
- **Providers pane arrow-key navigation**, and failover model-equivalency now
  covers every provider (kimi and amp included, derived from the canonical
  catalog so it can't drift).

### Fixed
- **Named logins no longer clobber the default account** — `alex auth login
  <provider> --name work` saves under the named account; previously it could
  silently overwrite and tombstone your default login.
- **Harness disconnect always revokes keys** — disconnecting while the daemon
  is down now revokes the harness keys from the local store instead of
  leaving them valid forever.
- **Dario menu line can no longer vanish** — it is a first-class menu section
  that always renders while Dario is enabled, red "down" when unhealthy or
  unreachable; transient status errors keep the last-known state instead of
  hiding the row; one shared health evaluator drives the menu, Dario window,
  preferences, and alerts.
- **Dario prompt-cache warm no longer permanently misses** for models with
  suffixed ids (e.g. `[1m]`): Rust and the capture preload now agree on the
  cache key. Also: never signals pid 0, quotes spaced install paths in
  NODE_OPTIONS (Application Support), and reports an instant child death with
  the stderr log path instead of a blind 60s timeout.
- **Proxy reliability** — a hung upstream can no longer wedge the daemon
  (120s response-head timeout; usage fetches get 15s timeouts); re-auth
  notifications fire for retained upstream 401s; queued error-injection
  fixtures are no longer consumed by simulate-header requests; unsaved
  routing edits survive the 60s snapshot poll; session delete removes >500
  traces and reports failures instead of beeping.
- **Installer/script fixes** — `install.sh` no longer dies on exit (undefined
  `anim_stop`); upgrade path refuses to kill non-Alex listeners on port 4100
  and anchors daemon pkill matches; DMG mounts are detached on failure;
  `install-macos.sh` stamps the real version; the harness regression suite's
  DARIO-RECOVER cell refuses to `kill -9` a remote-reported pid locally.

### Changed
- **Releases fail loudly** — tag releases hard-fail when signing/Sparkle/tap
  secrets are missing (no more green-but-unsigned releases); the stable
  workflow only triggers on exact `vX.Y.Z` tags; macOS `swift test` gates
  packaging; manual signed builds require an explicit tag and never overwrite
  published assets without opt-in.
- Internal dedup: one RFC 8628 device-flow driver, one `Vault::update`
  mutation API with atomic 0600 credential writes, one usage-cache fetcher,
  one mac-signing composite action + reusable DMG/appcast workflow, shared
  installer library; ui/ mock zips (5.7MB) removed.

### In progress (building, not yet in a beta)
- **`alexandria` → `alex` rename** — project-wide rename with a one-time,
  no-data-loss upgrade migration (`~/.alexandria` → `~/.alex`,
  `com.alexandria.daemon` → `com.alex.daemon`, `ALEXANDRIA_*` → `ALEX_*` with
  legacy fallback). Built; pending live migration test.
- **Blue-green daemon restart** — launchd socket-activation graceful restart
  with drain + hard-restart fallback (zero dropped connections). Built on both
  platforms; pending live zero-downtime verification.

## [0.1.28-beta.1] - 2026-07-18

### Added
- **Kimi Code integration** — log in to Kimi through Alex (`alex auth login
  kimi`, OAuth device flow) or adopt an existing `~/.kimi-code` login (`alex
  auth import kimi`); Kimi usage/quota shows in `alex status` and the menu;
  use `kimi/k3`, `kimi/kimi-for-coding`, and `-highspeed` in any harness; and
  `alex connect kimi` exposes your `alex/*` models inside the Kimi CLI's own
  model picker (reversible). Tokens auto-refresh on their 15-minute cycle.
- **Proactive re-auth notifications** — the Telegram/notification reauth alert
  now fires when a login expires while idle or a background token refresh fails
  on a revoked token, not only when a live request hits a 401. Debounced, and
  it never alarms on a token that can still silently refresh.
- Homebrew **cask is now published on every release** from the release
  pipeline, so the tap can't drift out of date again.
- `install-release.sh`/`install-beta.sh` recover from a broken/renamed Homebrew
  cask record, reclaim the port from stray daemons, and launch the app even if
  the daemon is busy. Adopt `open_app` and a `remove_legacy_app` guard —
  thanks **@khoaguin** (#5).

## [0.1.28-beta.5] - 2026-07-18

### Fixed
- **Settings no longer hangs on the Credentials page.** The "Active keys" table
  was a non-lazy `Grid` that laid out every row eagerly, freezing the main
  thread for ~9s once enough run keys accumulated (every harness connect mints
  one). Now a `LazyVStack` that only lays out visible rows.
- **Telegram token no longer looks lost.** After saving, the notifications pane
  now shows the saved chat + `✓ @yourbot` and a masked `•••` token (with a
  Replace button) instead of a blank field — the token was always saved; the
  API redacts it, and the empty field just read as data loss.

### Changed
- Kimi Code icon is used consistently — the provider/subscription icon now uses
  the same logo as the harness (was a drawn "K" chip).

## [0.1.28-beta.4] - 2026-07-18

### Fixed — update reliability (the big one)
- **No more false "you have the latest version."** Root-caused and fixed both
  independent causes: the daemon's version parser rejected anything but
  `X.Y.Z`/`X.Y.Z-beta.N` (so `rc`/`alpha`/`+build` tags silently became "up to
  date"), and a `-beta` app build defaulted to the **stable** update channel —
  so it checked the stable feed (older than the installed beta) and declared
  itself current. A beta build now defaults to the beta channel, and the parser
  handles the full range of tags.
- **Hard backstop:** if the running tag differs from the channel's latest tag
  and the two can't be ordered, the updater never claims you're current — it
  offers/flags the other version instead.
- **Post-update verification:** after a restart the daemon confirms the *running*
  version equals the intended one; a mismatch (stray daemon / launchd pin) is
  surfaced instead of silently "succeeding," and the update path now reclaims
  the port from stray daemons.
- The daemon and app version comparators are now aligned (a shared scheme:
  stable > rc > beta, higher `-beta.N` newer), covered by an edge-case matrix of
  tests on both sides.
- Kimi Code icon refreshed to the current brand mark.

## [0.1.28-beta.3] - 2026-07-18

### Fixed
- **Telegram bot token now persists.** A config writer holding a stale copy of
  the whole config (the Exo/Dario sections) could overwrite `config.toml` and
  silently drop the notifications section — wiping your bot token and killing
  all alerts (including the grok-expiry reauth). Every config write is now a
  read-modify-write that can't drop another section, and a token is kept even
  if the daemon momentarily can't reach Telegram to validate it.
- **Kimi models reach every harness.** `kimi/k3` (and siblings) were filtered
  out of harness model lists, and `alex connect kimi` crashed the request
  ("network connection lost") because the daemon had no Kimi writer. Both fixed:
  reconnect/refresh now lands `alex/kimi/k3` in pi, codex, grok, amp, claude,
  and Kimi. *(Verified end-to-end: routes real completions through `kimi/k3`,
  `claude-fable-5`, and `gpt-5.6` in both directions.)*
- **Grok no longer shows fake usage.** The "120/120 · 5m tokens" was per-minute
  API rate-limit headers masquerading as subscription usage; suppressed. Real
  Grok credit usage renders as a bar.
- **Kimi credit bars appear.** Fixed the usage window key + added Kimi to the
  usage snapshot, with an on-demand probe so a freshly-added account shows bars
  without waiting for the 15-minute heartbeat.
- **Settings no longer freezes loading Credentials** — the pane did synchronous
  disk I/O and per-cell date formatting on the main thread; now off-main with a
  timeout.

### Added
- **Blue-green daemon restart (opt-in, for testing)** — launchd socket-activation
  graceful restart with drain and a hard-restart fallback. Ships so it can be
  exercised live; the fallback means a plist reject can never leave you with no
  daemon.

## [0.1.28-beta.2] - 2026-07-18

### Added
- **Kimi login from the UI** — Settings → Providers → Kimi runs the device-flow
  panel (shows the code + authorize URL), plus a one-click "Use existing Kimi
  login" for an already-authed `~/.kimi-code`. The **Add-provider list is now
  complete** — Claude, Codex, Gemini, Grok, Kimi, OpenRouter, Amp, and Exo —
  each routing to its correct setup (OAuth / API-key / Exo's config).
- **Unified update channel** — one Release-channel picker sets **both** the app
  and the daemon by default (with an "either" scope), so beta/stable can no
  longer diverge between them. New `GET/POST /admin/update/channel`.

### Fixed
- **Provider health is now based on real pings.** A provider whose probes fail
  reads red (auth-failed / unreachable) in the menu and Providers pane instead
  of showing "Active" from mere credential presence.
- **Re-auth on any auth-class access failure** — a ping/heartbeat that returns
  401/403 (a confirmed logout) now fires the Telegram re-auth alert too; a
  transient 5xx/timeout stays "down" (failover) and never cries wolf.

## [0.1.27] - 2026-07-17

### Added
- **`alex auth merge <from> <into>`** — merge duplicate same-email accounts,
  unifying their split request/token history into one account (keeps the
  surviving valid credential; also exposed as `POST /admin/accounts/merge`).
- README **harness × subagent-tracing** support table (Claude Code is currently
  the only harness with true subagent lineage).

### Changed
- **Protection → Failover** — the settings pane and sidebar now read "Failover".
- Removed the per-harness **Tools** capture toggle from the harness config page
  (the connect/refresh sheet keeps its own opt-in).

### Fixed
- **Exo preferences pane crash** — the pane called `Bundle.module`, which traps
  when the SwiftPM resource bundle can't be resolved in the packaged app; now
  uses the safe resource resolver. The Exo tab is re-enabled.
- **Flaky updates** — the Sparkle appcast feed is cache-busted on every check so
  a stale CDN edge can't hide a freshly published build.
- Broke up an over-complex `UsageLineChart` `#Preview` that timed out the Swift
  type-checker in debug builds; gated a launchd plist test off Windows.
