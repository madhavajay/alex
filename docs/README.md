# Alex docs

Implementation reference derived from the current Rust code. For planned work
use the root [`TODO.md`](../TODO.md); for released changes use
[`CHANGELOG.md`](../CHANGELOG.md).

## System reference

| Doc | What it covers |
| --- | --- |
| [Overview](overview.md) | Crate responsibilities, request/data flow, authentication scopes, and the local state model. |
| [CLI reference](cli.md) | Complete `alex` command tree, important flags, defaults, and runnable examples. |
| [Providers and routing](providers-and-routing.md) | Provider implementations, vault accounts, selection policies, reserves, model routing, affinity, and failover. |
| [API and formats](api-and-formats.md) | Model ingress, control/trace routes, the four API dialects, Anthropic-pivot translation, SSE, usage, and cost. |
| [Configuration](configuration.md) | Full `config.toml` key/default reference, environment variables, and on-disk layout. |
| [Dario](dario.md) | Dario routing modes, three-block prompt rewrite, header handling, generations, health, and fallback behavior. |
| [CLIProxyAPI integration](cliproxyapi.md) | Both routing directions, safe reverse config export, version/capability negotiation, correlation, and loop prevention. |
| [Traces](traces.md) | Trace rows, live gzip and migrated LAR bodies, redaction, transcripts/tool calls, browser API, scoped keys, export, and retention. |
| [Shared web UI](web-ui.md) | Cross-platform local status/onboarding/trace preview, bounded pagination, launch and smoke-test behavior. |

## Harnesses and capture

| Doc | What it covers |
| --- | --- |
| [Harness integration](harnesses.md) | Provider headers, dynamic hooks, lifecycle events, session/sub-agent identity, current connection behavior, and regression fixtures. |
| [Amp wrap](amp-wrap.md) | `alex wrap amp`, reverse HTTP/WebSocket capture, Amp auth/billing, remote trace upload, and protocol diagnostics. |

## Build and design records

| Doc | What it covers |
| --- | --- |
| [Signed macOS build](build-signed.md) | `build-signed.sh`, Developer ID signing, notarization, and release-workflow secrets. |
| [Credential plan](credentials-plan.md) | Credential-vault design/roadmap context; open implementation work remains tracked in `TODO.md`. |
