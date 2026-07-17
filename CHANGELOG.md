# Changelog

All notable changes to Alex are documented here. Format loosely follows
[Keep a Changelog](https://keepachangelog.com/); versions follow the `vX.Y.Z`
git tags (stable) and `vX.Y.Z-beta.N` (beta channel). Releases before 0.1.27
predate this file — see the git history and GitHub releases.

## [Unreleased]

### In progress (building, not yet in a beta)
- **Kimi Code integration** — log in to Kimi via Alex (OAuth device flow) or
  import an existing `~/.kimi-code` login; see Kimi usage; use `kimi/*` models
  (k3, kimi-for-coding, -highspeed) in any harness; and use `alex/*` models
  inside the Kimi harness. Kimi subscription + agent added to the e2e suite.
- **`alexandria` → `alex` rename** — project-wide rename with a one-time,
  no-data-loss upgrade migration (`~/.alexandria` → `~/.alex`,
  `com.alexandria.daemon` → `com.alex.daemon`, `ALEXANDRIA_*` → `ALEX_*` with
  legacy fallback). Built; pending live migration test.
- **Blue-green daemon restart** — launchd socket-activation graceful restart
  with drain + hard-restart fallback (zero dropped connections). Built on both
  platforms; pending live zero-downtime verification.

### Added (on `main`, for the next release)
- **Proactive re-auth notifications** — the Telegram/notification reauth alert
  now fires when a login expires while idle or a background token refresh fails
  on a revoked token, not only when a live request hits a 401. Debounced, and
  it never alarms on a token that can still silently refresh.
- Homebrew **cask is now published on every release** from the release
  pipeline, so the tap can't drift out of date again.
- `install-release.sh` recovers from a broken/renamed Homebrew cask record,
  force-installs the current app, and launches the app even if the daemon is
  busy. Adopts `open_app` (launch whichever app the cask installed) and a
  `remove_legacy_app` guard — thanks **@khoaguin** (#5).

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
