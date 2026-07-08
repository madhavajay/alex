# mac-ui.md — Alexandria macOS Menu Bar App Plan

> **Status:** M1 + M2 + M3 shipped. `macos/` has AlexandriaBar with hieroglyph icon (Eye of Horus, red/orange on issues),
> in-app auth windows (claude paste-back, codex loopback auto-complete, grok **device-code flow** via `auth.x.ai` — Amp-style
> link + code + waiting spinner), Ghostty-aware terminal launcher, Node.js detection for dario, codex "Start 5h Window" action
> (confirmed, untested by design), reset-boundary refresh + window-reset notifications.
> Daemon gained `/admin/auth/import`, `/admin/auth/login/start|complete|{id}` (`alexandria-auth/src/sessions.rs`) and **xai
> token refresh** (OIDC client `b1a00492-…` at `auth.x.ai/oauth2/token`).
> Build: `macos/Scripts/package_app.sh` → `macos/dist/AlexandriaBar.app`; dev loop: `macos/Scripts/run.sh`; tests: `swift test` in `macos/`.
> Remaining: M4 SSE events/adaptive refresh; codex reset-credits inventory display (read-only GET, needs daemon limits work).

Plan for `macos/` — a CodexBar-style menu bar app showing Alexandria status (accounts, limits, health, dario), with configuration and one-click re-auth when a subscription goes down.

## Legend
| Symbol | Meaning |
|--------|---------|
| → | leads to |
| & | and/with |

---

## 1. Key architectural decision: HTTP-first, not Rust-FFI-first

The question was whether to bridge the Rust crates into the app to keep it snappy vs shelling out to the CLI. **The answer is neither for the hot path — talk HTTP to the running daemon.**

Alexandria is already a long-lived localhost daemon (`127.0.0.1:4100`) with a JSON admin API, authenticated via `x-api-key: <local_key>` from `~/.alexandria/config.toml`:

| Endpoint | Gives the menu app |
|---|---|
| `GET /health` | up/down, version, uptime, in_flight, dario on/off |
| `GET /admin/accounts` | per-account provider/kind/label/status/`expires_in_s` |
| `GET /admin/health` | accounts + last heartbeat (ok/fail, latency, message) |
| `GET /admin/limits` | plan + per-window `used_pct` + `resets_at` (5h/7d etc.) |
| `GET /admin/analytics?since_minutes=N` | requests, tokens, cost_usd, errors, by-model |
| `GET /admin/dario` | generation phase, pid, probe status, pending update |
| `POST /admin/dario/restart` / `/update` | dario actions |

Rationale:
- Localhost HTTP is ~1ms; "snappy" is solved without any bridging. Shelling out to the CLI is only needed as a *fallback* when the daemon is down (start it) — and `alexandria status --json` already exists for that.
- The daemon owns the vault, SQLite store, cooldowns, and refresh singleflight. Embedding `alexandria-auth`/`alexandria-store` in-process would create a second writer to `~/.alexandria/accounts/*.json` & the DB — races, lock contention, divergent state. One owner (daemon) + thin clients (CLI, TUI, menu app) is the existing pattern; the ratatui TUI (`crates/alexandria-daemon/src/tui.rs`) already works exactly this way and is the blueprint.
- Risk avoided: rebuilt binaries get silently blocked by Little Snitch on this machine. A Swift app that only talks to `127.0.0.1` minimizes that surface; an embedded Rust core doing OAuth token refresh to `console.anthropic.com`/`auth.openai.com` would reintroduce it into a frequently-rebuilt GUI binary.

**Rust still gets reused — server-side.** Anything the app needs that doesn't exist yet becomes a new admin endpoint in `alexandria-proxy` (Rust we already have), not Swift reimplementation and not FFI. Single set of Rust code, exercised through one interface.

**Optional Phase 4 FFI escape hatch** (only if a real need appears): `alexandria-core` + `alexandria-auth` compile as a `staticlib` bridged with UniFFI or swift-bridge — e.g. offline parsing of `config.toml`/vault files when the daemon is down. Deliberately deferred; it adds a second build toolchain to the app for data we can already get.

---

## 2. What we copy from CodexBar

CodexBar (`~/dev/codexbar`) is SPM-based (no Xcode project), Swift 6.2 strict concurrency, macOS 14+. The reusable skeleton:

- **App shape**: `@main` SwiftUI `App` + `@NSApplicationDelegateAdaptor(AppDelegate)`; `Settings { }` scene for prefs; hidden 1×1 keepalive `WindowGroup`; `LSUIElement=true` (agent app, no dock icon).
- **Menu bar**: AppKit `NSStatusItem` (variable length) + `NSMenu`, with SwiftUI cards hosted in menu items via `NSHostingView`. More control than SwiftUI `MenuBarExtra` (icon animation, dynamic width, autosave position).
- **State**: one `@MainActor @Observable` store polled on a timer; adaptive cadence (their `AdaptiveRefreshPolicy.swift`: slower when menu unused / Low Power Mode / thermal pressure) plus refresh-on-menu-open.
- **Notifications**: `UNUserNotificationCenter` wrapper (`AppNotifications.swift` pattern), request auth at launch.
- **Launch at login**: `SMAppService.mainApp.register()` (`LaunchAtLoginManager.swift` pattern).
- **Packaging**: SPM can't emit `.app` bundles — copy their `Scripts/package_app.sh` approach: `swift build` → hand-assemble `Contents/MacOS`, heredoc-generate `Info.plist` (+ `LSUIElement`) & entitlements, codesign.

What we *don't* copy: 60-provider registry, WKWebView cookie scraping, PTY runners, Sparkle, widget appex, multi-status-item merging. Alexandria's daemon replaces all data acquisition — our app is a thin dashboard + action panel.

---

## 3. Repo layout

```
macos/
├── Package.swift                      # swift-tools 6.2, macOS 14+
├── Sources/
│   ├── AlexandriaBarCore/             # platform-agnostic client library
│   │   ├── AlexandriaClient.swift     # URLSession client for admin API
│   │   ├── Models.swift               # Codable: Health, Account, Limits, Dario, Analytics
│   │   ├── DaemonDiscovery.swift      # read ~/.alexandria/config.toml → host/port/local_key
│   │   ├── DaemonController.swift     # fallback: launch daemon --background, service install
│   │   └── SnapshotStore.swift        # @Observable polled state + derived alert conditions
│   └── AlexandriaBar/                 # the menu bar app
│       ├── AlexandriaBarApp.swift     # @main + AppDelegate
│       ├── StatusItemController.swift # NSStatusItem, icon states, NSMenu
│       ├── MenuCardView.swift         # SwiftUI cards: limits gauges, accounts, dario
│       ├── ReauthWindow.swift         # login flows (URL open + code paste)
│       ├── Notifications.swift        # UNUserNotificationCenter + alert dedupe
│       ├── PreferencesView.swift      # Settings scene panes
│       └── LaunchAtLogin.swift        # SMAppService
├── Tests/AlexandriaBarCoreTests/      # client decode + alert-derivation tests (fixture JSON)
└── Scripts/
    ├── package_app.sh                 # bundle + Info.plist + entitlements + codesign
    └── run.sh                         # build & launch debug bundle
```

Config discovery: parse `~/.alexandria/config.toml` for `host`/`port`/`local_key` (tiny hand-rolled TOML read or a small TOML dep). No secrets duplicated into the app; the key stays in the 0600 file the daemon owns.

---

## 4. Menu bar UI

**Icon states** (template image, monochrome; lighthouse/pharos glyph from `logo.png` simplified):
- Normal: static icon, optional compact text like `82%` (highest window utilization) — user-toggleable.
- Degraded: badge dot — orange = account expiring soon / cooldown / limit ≥ threshold; red = daemon down, heartbeat failing, or account needs re-auth.
- Dario pending update: subtle up-arrow badge.

**Menu contents** (top → bottom):
1. **Daemon header** — up/down, version, uptime, in-flight; if down: "Start Daemon" action (shell `alexandria daemon --background` or `launchctl kickstart`).
2. **Limits card** — per provider: plan label + horizontal gauges per window (`5h`, `7d`, `7d opus`…) with `used_pct` and reset countdown. This is the primary at-a-glance content.
3. **Accounts card** — dot (green/orange/red) + provider + label + expiry/cooldown; rows with problems get an inline **"Re-auth…"** button.
4. **Dario section** (when enabled) — generation phase, probe latency; actions: Restart, Update (POST to existing endpoints).
5. **Usage footer** — last-hour requests / cost from `/admin/analytics`.
6. Standard: Refresh Now, Settings…, Open TUI in Terminal (nice-to-have), Quit.

---

## 5. Notifications & alert rules

Derived in `SnapshotStore` on each poll; deduped (notify on state *transition*, re-notify at most every N hours while unresolved):

| Condition | Source | Notification |
|---|---|---|
| Account `status != active` or refresh failing | `/admin/accounts` | "Claude subscription needs re-auth" → click opens re-auth flow |
| Heartbeat fail (≥2 consecutive) | `/admin/health` | "xAI provider failing pings" |
| Token expires < 24h & no refresh path (xai/gemini) | `expires_in_s` | "Grok token expires in Nh — re-login" |
| Limit window ≥ threshold (default 90%, configurable) | `/admin/limits` | "Claude 5h window at 93%, resets 14:00" |
| Daemon unreachable (was up) | `/health` timeout | "Alexandria daemon down" + Start action |
| Dario unhealthy / pending update | `/admin/dario` | actionable notification (Restart / Update buttons) |

Notification actions use `UNNotificationAction` so re-auth/restart is one click from the banner even when the menu is closed.

---

## 6. Re-auth flows (the core feature)

Today re-auth = `alexandria auth login <provider>` in a terminal. Per provider, in-app UX:

- **Claude (code-paste PKCE)**: cleanest to move server-side. New daemon endpoints:
  - `POST /admin/auth/login/start {provider}` → daemon generates PKCE, returns `{login_id, authorize_url}`; app opens URL in browser.
  - `POST /admin/auth/login/complete {login_id, code}` → daemon exchanges & upserts into vault.
  - App shows a small window: "1. Browser opened → 2. Paste `code#state` here → Done". Reuses `alexandria-auth::login` logic verbatim — the Rust reuse the FFI idea was after, with zero bridging.
- **Codex (loopback :1455)**: daemon can run the whole flow: `start` spawns the loopback listener + returns authorize_url; app opens browser; daemon completes automatically; app polls `GET /admin/auth/login/{login_id}` for done/failed. No paste needed.
- **Grok (delegated to grok CLI)**: no programmatic flow exists. Menu action opens Terminal running `grok` (login) and, on window close / button press, calls `POST /admin/auth/import {provider: grok}` (new endpoint wrapping existing `import_all`). Fallback for all providers: "Re-import credentials" menu item.
- **Gemini**: import-only (matches current CLI behavior).

Daemon-side work this implies (Rust, in `alexandria-proxy` + `alexandria-auth`):
1. `POST /admin/auth/import` — wrap existing `import_all()`.
2. `POST /admin/auth/login/start|complete`, `GET /admin/auth/login/{id}` — lift `login.rs` flows out of the interactive-terminal path into resumable state keyed by `login_id` (in-memory map with TTL). The PKCE/exchange code already exists; this is re-plumbing, not new logic.
3. (Nice-to-have) `GET /admin/events` SSE — push account-status/heartbeat changes so the app can react instantly instead of on next poll. TODO.md M5 already lists an SSE trace stream as unfinished; same mechanism. Polling every 30–60s is acceptable for v1.

---

## 7. Settings (Preferences window)

- Refresh cadence: manual / 30s / 1m / 5m / adaptive (default adaptive).
- Icon: show % text on/off; which metric drives it (max window util / cost today).
- Alert thresholds: limit % warn level, expiry warn hours, heartbeat-fail count.
- Notification toggles per category (limits / re-auth / daemon / dario).
- Launch at login toggle (SMAppService).
- Daemon: host/port override (defaults from config.toml), "Install as service" button (`alexandria service install`).
- Storage: UserDefaults for all of the above; nothing secret in the app.

---

## 8. Packaging & signing

- `Scripts/package_app.sh` modeled on CodexBar's: `swift build -c release` → assemble `AlexandriaBar.app` → generate `Info.plist` (`LSUIElement=true`, bundle id `com.alexandria.bar`, min macOS 14) → codesign.
- **Sign with a stable identity, not adhoc** — adhoc-signed rebuilds change identity every build, and Little Snitch has already bitten rebuilt binaries on this machine (see memory: alexandria binary silently losing network). A stable Developer ID (or at least a persistent self-signed cert) keeps firewall rules sticky across rebuilds. App only needs `127.0.0.1` outbound, which also helps.
- No Sparkle/notarization for v1 (personal tool); `install.sh` gains a `--with-app` flag later if wanted.
- No sandbox (needs to read `~/.alexandria/config.toml`, launch `alexandria`, open Terminal).

---

## 9. Milestones

**M1 — Read-only dashboard** (ship first, immediately useful)
- `macos/` SPM scaffold, `AlexandriaBarCore` client + models + fixture-based decode tests.
- Status item + menu with daemon header, limits gauges, accounts list, analytics footer. Poll loop + refresh-on-open.
- `package_app.sh` + `run.sh`.

**M2 — Alerts & actions**
- Notification engine + dedupe + thresholds; icon badge states.
- Daemon down detection + Start Daemon; dario Restart/Update actions; Launch at login; Preferences window.

**M3 — In-app re-auth** (needs daemon work)
- Rust: `/admin/auth/import` + `/admin/auth/login/*` endpoints (+ tests in the existing wire tier of `test.sh`).
- Swift: ReauthWindow (claude paste flow, codex auto flow, grok import flow); actionable notifications.

**M4 — Polish / optional**
- SSE events endpoint + instant updates; adaptive refresh policy; cost sparkline; TUI-parity extras; only-if-needed Rust FFI for offline config/vault reads.

---

## 10. Open questions

1. Icon %-text default: on or off? (CodexBar shows text; minimal icon is quieter.)
2. Bundle id / app name: `AlexandriaBar` vs `Alexandria` (`com.alexandria.bar` assumed).
3. Signing: is a paid Developer ID available, or self-signed cert + one-time Little Snitch rule?
4. Should M3's login endpoints be loopback-only *and* key-gated (assumed yes, same as `/connect`)?
5. Min macOS: 14 (matches CodexBar, enables `@Observable`) — acceptable?
