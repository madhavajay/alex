# Shared web UI

The daemon serves Alex's shared macOS and Linux product surface. Start the daemon when needed and open it with:

```bash
alex web
```

For a terminal, remote shell, or browser-launch problem, print the loopback URL instead:

```bash
alex web --no-open
```

`alex web` starts a background daemon if the configured local daemon is not already healthy. The URL always uses loopback even when Alex also listens on a LAN or VPN interface. The page obtains its administrative credential from the loopback-only `/connect` bootstrap endpoint; it does not put the credential in a URL, HTML asset, or browser storage.

To use the UI from another browser on a trusted LAN, bind Alex to the machine's
LAN address with `alex service bind <interface-ip>` and open
`http://<interface-ip>:4100/ui/`. The remote page asks for the admin key printed
by `alex credentials`; it retains the key only in page memory. The built-in
server uses HTTP, so do not expose it to an untrusted network or forward the
port to the public internet.

Windows support is not included in the stable release yet.

This preview includes:

- daemon and account status;
- on-demand live credential probes that immediately refresh account and tray
  health;
- one-click managed OAuth re-authentication on accounts with confirmed auth
  failures;
- existing-account display and OAuth onboarding for Claude, Codex, Gemini, Grok, and Kimi;
- Amp CLI/API-key import for wrap capture and billing status;
- OpenRouter key and Exo endpoint setup;
- a middleware browser for built-in and user rules, including readable conditions/actions, live enable/disable, and fixture dry-runs;
- cursor-paginated trace summaries (25 at a time, maximum 100 per request);
- metadata filters for model, provider, harness, status, and errors;
- per-trace summary, provenance, attempts, and middleware decisions;
- bounded, cursor-paginated session turns (20 at a time), with request/response
  bodies and tool data loaded only when one turn is expanded.

The full menu-bar app and native notifications remain macOS-only. Linux has a
lightweight StatusNotifierItem tray with live daemon/provider health and menu
actions for the Web UI, onboarding, traces, status refresh, and daemon restart:

```bash
alex tray                 # run in the current graphical session
alex tray install         # start automatically at graphical login
alex tray status
alex tray uninstall
```

The tray follows the freedesktop StatusNotifierItem and XDG autostart standards.
KDE Plasma, Xfce, Cinnamon, and other compatible panels show it directly. GNOME
requires an AppIndicator/StatusNotifierItem shell extension. The tray keeps
running if it starts before the desktop's notifier host is ready and registers
when that host appears.

Linux uses a separate systemd user service for the daemon, managed with `alex
service install`, `alex service restart`, and `alex service uninstall`. Quitting
the tray does not stop the daemon. Windows support is being developed separately
and is not a stable-release gate.

## HTTP surface

The static assets are served directly by the daemon under `/ui/`; there is no separate web server or build-time service. Administrative calls require the normal local key.

`GET /traces/summaries` is the bounded list endpoint used by the page. It accepts `limit` (1–100), the normal trace filters, and an optional stable `before_ms` + `before_id` cursor returned as `next_cursor`. It never returns captured bodies. Open body-free detail with `GET /traces/{id}/metadata` and fetch one body with `GET /traces/{id}/body/{kind}`.

Session history uses `GET /traces/sessions/{session_id}/transcript/page`, with a
limit of 1–50 and the returned `after_ms` + `after_id` cursor. Page rows contain
metadata and body-presence flags only. Expanding a row calls
`GET /traces/{id}/turn`, which reads only that trace's request, response, and
trace-linked tool payloads. The browser replaces the current 20-row page rather
than accumulating every turn in the DOM. The original
`GET /traces/sessions/{session_id}/transcript` endpoint remains available for
older clients.

The Middleware view reads `GET /admin/middleware` and `GET /admin/fixtures`. Toggling a rule replaces its canonical rule document through `PUT /admin/middleware/rules/{id}`. A dry-run calls `POST /admin/middleware/test` against one named fixture; testing a disabled rule does not enable it in the live runtime.

## Deterministic smoke foundation

CI runs the cross-platform `deterministic_platform_smoke` test with local TCP listeners and mock OpenAI-compatible OpenAI/Exo routes. It checks daemon health, the shared UI, a basic request, a streamed and reassembled tool call, an OpenAI-to-Exo middleware reroute with recorded decisions/provenance, bounded trace listing, and persistence of traces, streamed bodies, and the rule after the daemon/store is reopened. No provider credential or public network is involved.

The macOS and Linux matrix also runs browser-launch, background-daemon, and
platform service-lifecycle contracts. These prove URLs, executable paths
containing spaces, and daemon arguments stay OS-native and shell-free. A
failure on either supported Rust platform blocks the release candidate.

Provider OAuth itself remains a short manual/live smoke because public CI cannot safely hold subscription credentials.
