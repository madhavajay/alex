# Changelog

All notable changes to Alex are documented here. Format loosely follows
[Keep a Changelog](https://keepachangelog.com/); versions follow the `vX.Y.Z`
git tags (stable) and `vX.Y.Z-beta.N` (beta channel). Releases before 0.1.27
predate this file — see the git history and GitHub releases.

## [Unreleased]

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
