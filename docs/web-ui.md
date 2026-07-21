# Shared web UI (preview)

The daemon serves Alex's first shared macOS, Linux, and Windows product surface. Start the daemon when needed and open it with:

```bash
alex web
```

For a terminal, remote shell, or browser-launch problem, print the loopback URL instead:

```bash
alex web --no-open
```

`alex web` starts a background daemon if the configured local daemon is not already healthy. The URL always uses loopback even when Alex also listens on a LAN or VPN interface. The page obtains its administrative credential from the loopback-only `/connect` bootstrap endpoint; it does not put the credential in a URL, HTML asset, or browser storage.

On Windows 11, install the signed release and its per-user Task Scheduler service from PowerShell:

```powershell
irm https://raw.githubusercontent.com/madhavajay/alex/main/install-release.ps1 | iex
alex web
```

The installer verifies the release archive's SHA-256 checksum, installs under `%LOCALAPPDATA%\Alex\bin`, adds that directory to the user `PATH`, and runs `alex service install`. Use `alex service restart` and `alex service uninstall` for the same lifecycle exposed on macOS and Linux.

This preview includes:

- daemon and account status;
- existing-account display and OAuth onboarding for Claude, Codex, Gemini, Grok, Kimi, and Amp;
- OpenRouter key and Exo endpoint setup;
- a middleware browser for built-in and user rules, including readable conditions/actions, live enable/disable, and fixture dry-runs;
- cursor-paginated trace summaries (25 at a time, maximum 100 per request);
- metadata filters for model, provider, harness, status, and errors;
- per-trace summary, provenance, attempts, and middleware decisions;
- explicit lazy loading of individual request/response bodies and session transcripts.

The menu-bar app and native notifications remain macOS-only. Linux uses the systemd user service; Windows uses a per-user Task Scheduler entry. Both are managed with `alex service install`, `alex service restart`, and `alex service uninstall`. The CI Windows job remains advisory until a clean Windows 11 runner has exercised install, restart, routing, and trace inspection end to end.

## HTTP surface

The static assets are served directly by the daemon under `/ui/`; there is no separate web server or build-time service. Administrative calls require the normal local key.

`GET /traces/summaries` is the bounded list endpoint used by the page. It accepts `limit` (1–100), the normal trace filters, and an optional stable `before_ms` + `before_id` cursor returned as `next_cursor`. It never returns captured bodies. Open body-free detail with `GET /traces/{id}/metadata`; fetch a body with `GET /traces/{id}/body/{kind}` or a transcript with `GET /traces/sessions/{session_id}/transcript` only when its disclosure is opened.

The Middleware view reads `GET /admin/middleware` and `GET /admin/fixtures`. Toggling a rule replaces its canonical rule document through `PUT /admin/middleware/rules/{id}`. A dry-run calls `POST /admin/middleware/test` against one named fixture; testing a disabled rule does not enable it in the live runtime.

## Deterministic smoke foundation

CI runs the cross-platform `deterministic_platform_smoke` test with local TCP listeners and mock OpenAI-compatible OpenAI/Exo routes. It checks daemon health, the shared UI, a basic request, a streamed and reassembled tool call, an OpenAI-to-Exo middleware reroute with recorded decisions/provenance, bounded trace listing, and persistence of traces, streamed bodies, and the rule after the daemon/store is reopened. No provider credential or public network is involved.

The same macOS, Linux, and Windows matrix also runs pure browser-launch and background-daemon lifecycle contracts. These prove URLs, executable paths containing spaces, and daemon arguments stay OS-native and shell-free without installing or mutating a real platform service. Windows remains advisory until its separate service lifecycle and existing Windows-only failures are fixed.

Provider OAuth itself remains a short manual/live smoke because public CI cannot safely hold subscription credentials.
