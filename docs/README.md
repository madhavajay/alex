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
| [Traces](traces.md) | Trace rows and gzip bodies, redaction, transcripts/tool calls, browser API, scoped keys, export, and retention. |
| [LAR v1 wire format](lar-format-v1.md) | Container framing, content IDs, limits, recovery, versioning, and conformance fixtures for the deduplicated archive. |
| [LAR v1 conformance](lar-conformance.md) | Public vectors, verifier commands, MIME proposal, compatibility workflow, and security notes. |

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
| [LAR format and implementation plan](../lar-format.md) | Storage requirements, fidelity boundary, migration plan, performance targets, and phased implementation checklist. |
| [LAR storage inventory](lar-storage-inventory.md) | Legacy body/header writers and readers that must move through the unified archive seam. |
| [LAR operator runbook](lar-operations.md) | Safe beta rollout, migration, verification, cleanup, rollback, downgrade, GC/repack, repair, and incident handling. |
| [LAR benchmark](lar-benchmark.md) | Reproducible synthetic-corpus storage/verification benchmark and recorded design-gate results. |
| [LAR global-dedup ADR](adr/0001-lar-global-dedup.md) | Decision record for global chunk identity and cross-file deduplication. |
| [OTAP/Arrow analytics ADR](adr/0003-otap-arrow-derived-analytics.md) | Decision to keep OTAP/Arrow derived and defer a native Rust exporter until its official protocol and API stabilize. |
