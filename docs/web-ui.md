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

- daemon, account, and middleware status;
- existing-account display and OAuth onboarding for Claude, Codex, Gemini, Grok, Kimi, and Amp;
- OpenRouter key and Exo endpoint setup;
- cursor-paginated trace summaries (25 at a time, maximum 100 per request);
- lazy opening of one trace's metadata. Request and response bodies remain separate, on-demand endpoints.

The menu-bar app and native notifications remain macOS-only. Linux uses the systemd user service; Windows uses a per-user Task Scheduler entry. Both are managed with `alex service install`, `alex service restart`, and `alex service uninstall`. The CI Windows job remains advisory until a clean Windows 11 runner has exercised install, restart, routing, and trace inspection end to end.

## HTTP surface

The static assets are served directly by the daemon under `/ui/`; there is no separate web server or build-time service. Administrative calls require the normal local key.

`GET /traces/summaries` is the bounded list endpoint used by the page. It accepts `limit` (1–100), the normal trace filters, and an optional stable `before_ms` + `before_id` cursor returned as `next_cursor`. It never returns captured bodies. Open metadata with `GET /traces/{id}` and fetch a body only when required with `GET /traces/{id}/body/{kind}`.

## Deterministic smoke foundation

CI runs the cross-platform `deterministic_platform_smoke` test with local TCP listeners and a mock OpenAI-compatible Exo upstream. It checks daemon health, the shared UI, one routed request, bounded trace listing, trace opening, and persistence after the daemon/store is reopened. No provider credential or public network is involved.

Provider OAuth itself remains a short manual/live smoke because public CI cannot safely hold subscription credentials.
