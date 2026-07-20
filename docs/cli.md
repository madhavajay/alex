# `alex` command reference

`alex` and `alexandria` are identical binaries. Examples use `alex`. With no
subcommand and an interactive terminal, the binary opens the TUI; with
non-terminal stdout it errors and asks for an explicit command.

This page follows the clap definitions in `crates/alex/src/main.rs`. Values in
angle brackets are required; square brackets are optional. Clap converts enum
names to kebab case. Every command also supports generated `--help`; the root
supports `--version`.

## `daemon`

Run the proxy in the foreground, or detach to `~/.alexandria/daemon.log`.

| Syntax | Important arguments | Example |
| --- | --- | --- |
| `alex daemon` | `--host <IP>` and `--port <PORT>` override config for this run; `--background` detaches. | `alex daemon --host 127.0.0.1 --port 4100` |

The persisted listener is changed with `alex config host` or `alex service
bind`, not by one-run daemon overrides.

## `auth`

Manage provider accounts in `<data_dir>/accounts/`.

| Subcommand | Important arguments | Example |
| --- | --- | --- |
| `import [SOURCE]` | Source defaults to `all`; implemented sources: `claude`, `codex`, `gemini`, `grok`/`xai`, `amp`, `kimi`, `all`. `--name` defaults to `default`; `--force` replaces. | `alex auth import codex --name work` |
| `login [PROVIDER]` | OAuth/device flow for `claude`, `codex`, `grok`, `gemini`, `amp`, or `kimi`. Omit in a terminal for a picker. `--name`, `--force`. | `alex auth login kimi --name work` |
| `pause <PROVIDER> <NAME>` | Persistently excludes the named account. | `alex auth pause codex work` |
| `resume <PROVIDER> <NAME>` | Makes a paused account selectable again. | `alex auth resume codex work` |
| `gemini-key [KEY]` | Omit the argument to read `GEMINI_API_KEY`. | `GEMINI_API_KEY='<redacted>' alex auth gemini-key` |
| `amp-key [KEY]` | Omit the argument to read `AMP_API_KEY`. Used for billing/wrap, not `/v1` routing. | `AMP_API_KEY='<redacted>' alex auth amp-key` |
| `openrouter-key [KEY]` | Omit key to read `OPENROUTER_API_KEY`; optional `--referer`, `--title`; `--remove` cannot be combined with them. | `OPENROUTER_API_KEY='<redacted>' alex auth openrouter-key --title 'Local Alex'` |
| `list` | Shows provider, account name/kind/state, expiry, and account-file path without printing secrets. | `alex auth list` |
| `merge <FROM> <INTO>` | Daemon-backed merge of account and trace history; `--allow-mismatch` bypasses provider/email checks. | `alex auth merge openai-oauth-old openai-oauth-work` |

Account names must match `[a-z0-9_-]{1,32}`. A non-default `--name` with
`source=all` is rejected as ambiguous; import one provider at a time. Provider
aliases are described in [Providers and routing](providers-and-routing.md).

## `vault`

Create or import encrypted portable credential bundles. Passphrases are command
arguments in the current interface; take shell-history precautions.

| Subcommand | Important arguments | Example |
| --- | --- | --- |
| `export` | Required `--passphrase`, `--out`; `--accounts` and `--harnesses` default to `all` and accept selection strings. | `alex vault export --passphrase '<redacted>' --accounts claude,codex --harnesses pi --out alex-vault.bundle` |
| `import <FILE>` | Required `--passphrase`; decrypts and merges into this machine. | `alex vault import alex-vault.bundle --passphrase '<redacted>'` |
| `pull` | Required `--from <URL>`, `--admin-key`, `--passphrase`; remote selection defaults to all accounts/harnesses. | `alex vault pull --from https://alex.example.invalid --admin-key '<redacted>' --passphrase '<redacted>' --accounts codex` |

## `traces`

Without a trace subcommand, read the local SQLite store offline:

```bash
alex traces --limit 50 --session ses_123 --model claude-sonnet-5 --json
```

`--limit` defaults to 20. The daemon-backed `search` and `export` share these
filters: `--since`, `--until`, `--run-id`, `--session`, `--model`, `--provider`,
`--account-id`, `--path`, `--harness`, `--status`, `--errors`,
`--key-fingerprint`, and `--limit`. Time values accept RFC3339 or relative
forms such as `30m`, `2h`, `7d`, and `45s`.

| Subcommand | Important arguments | Example |
| --- | --- | --- |
| `search` | Shared filters; `--json` for machine output. Requires the daemon. | `alex traces search --since 2h --provider anthropic --errors --limit 100` |
| `export` | Shared filters; `--bodies` inlines base64 artifacts; `--out <FILE>` otherwise writes stdout. | `alex traces export --run-id job-42 --bodies --out job-42.ndjson` |
| `reattach` | Offline orphan listing by default. Supply `--orphan-account-id`, `--to-account-id`, and `--yes` to mutate; optional `--json`. | `alex traces reattach --orphan-account-id old-id --to-account-id openai-oauth-work --yes` |
| `path` | Required `--run-id`; prints data root, SQLite path, and artifact paths for that run. | `alex traces path --run-id wrap-amp-20260719` |
| `prune` | `--older-than` defaults to `30d`; bodies/headers only by default; `--rows` also deletes rows; `--dry-run`, `--json`. `--bodies-only` is an explicit mutually exclusive spelling of the default. | `alex traces prune --older-than 30d --dry-run --json` |
| `du` | Offline SQLite/body disk use; optional `--json`. | `alex traces du --json` |
| `repair-agent` | Required `--transcript-id`; `--dry-run`, `--json`. Reconciles a wrapped Cursor Agent transcript from local JSONL. | `alex traces repair-agent --transcript-id 8f3a --dry-run` |
| `repair-amp` | Optional `--run-id`; `--json`. Reimports the latest wrapped Amp WebSocket capture, including error-only turns. | `alex traces repair-amp --run-id wrap-amp-20260719 --json` |
| `push` | Required `--run-id`; remote trace flags below. Replays a local wrap spool. | `alex traces push --run-id wrap-amp-20260719 --trace-url https://alex.example.invalid --trace-key-file ~/.config/alex/wrap.key` |

Remote trace flags are `--trace-url` (alias `--alex-url`), `--run-id`,
`--trace-key-file`, and `--allow-insecure-http`. Environment alternatives are
documented in [Configuration](configuration.md).

## `lar`

Inspect, migrate, verify, and extract bodies from the LLM Archive store. LAR
migration is additive: validated archive pointers are preferred, legacy gzip
paths remain available for fallback, and startup migration never deletes the
source files.

| Subcommand | Behavior |
| --- | --- |
| `import SOURCE [--format auto\|lar\|jsonl]` | Auto-detect a sealed LAR archive or Alex JSONL v1 export, or require the selected format explicitly. Sealed LAR remains a validate-then-attach operation. JSONL is read one bounded line at a time; schema/types, duplicate trace IDs, base64 lengths, and BLAKE3 body hashes are validated before writes. Each completed JSONL trace publishes its SQLite row last and exact re-imports are skipped. |
| `detach --file-uuid FILE_UUID` | Mark exactly one sealed cataloged archive `archived_offline`. The required 32-hex-digit file UUID prevents path-based ambiguity. Detach first validates or records the immutable file identity, changes catalog state only, and never moves or deletes bytes. Active writers, repairing files, and retired files are rejected. |
| `reattach --file-uuid FILE_UUID --archive PATH` | Reattach an offline, missing, or relocated clean sealed archive. Before one catalog transaction, Alex verifies the candidate's role, UUID, format/features, length, whole-file BLAKE3, footer, chunks, manifests, and reconstructed bodies against the identity recorded at detach. A different file at the expected path is rejected. |
| `import-legacy` | Run the shared resumable importer. `--dry-run` inventories only; `--verify` also verifies dry-run gzip sources; `--limit N` bounds one invocation; `--json` reports counters and failures. Published pointers always pass normal-reader length and BLAKE3 validation. |
| `migration status` | Show all durable jobs, leases, counters, deduplicated bytes, and the last error. |
| `migration pause` / `resume` | Control the one incomplete migration job without changing converted artifacts. |
| `migration verify` | Read every active/sealed archive and reconstruct every cataloged manifest, then compare migrated source identities. Read-only; JSON reports carry a canonical BLAKE3 checksum so saved verification evidence is tamper-detectable. |
| `gc plan` / `apply` / `resume RUN_ID` | Compute reachability from trace and stage roots, then persist, recheck, and logically sweep unreachable manifest/chunk catalog objects. Planning is non-mutating and interrupted runs resume from durable state. Immutable pack bytes are reclaimed separately by repacking. |
| `repack plan` / `apply` / `resume RUN_ID` | Select clean sealed body packs by `--min-garbage-bytes` and `--min-garbage-ratio`, copy reachable chunks plus the complete canonical manifest/header/stream/stage/exchange/metadata/conversation graph, verify identities and reconstructed bodies, atomically switch catalog locations, and move the source to recoverable quarantine. Cross-pack manifests stay external so shared chunks are not duplicated. Unknown extensions or unsupported schemas are conservatively ineligible. Reports distinguish logical from physical reclamation. |
| `verify [ARCHIVE]` | Verify the live store when no path is supplied, or frame/checksum/reconstruction integrity for one archive. |
| `repair INPUT --output OUTPUT` | Copy only the valid prefix into a different file and verify it; never modifies the input. |
| `upgrade INPUT --output OUTPUT` | Rewrite one clean, sealed, exactly supported v1 archive into a newly sealed latest-v1 file with a new physical UUID. Canonical body/header/stream/exchange/conversation records and opaque unknown optional outer records remain byte-identical and in order; derived indexes/footer are regenerated. The command rejects existing or aliased output paths and file/header/schema extensions it cannot reproduce, verifies bodies plus exact canonical equivalence, then publishes atomically without modifying the input or live catalog. `--json` includes UUIDs, SHA-256 identities, byte/count totals, and verification state. |
| `ls [ARCHIVE]` | Summarize one archive, or list live catalog archive-file UUIDs, paths, identity/availability states, and migration jobs. |
| `grep LITERAL [ARCHIVE...]` | Search exact raw body bytes in the live catalog plus any supplied sealed archives. This body-only, deduplicated scan remains the default. `--scope whole-record` additionally searches safe ordered header/trailer atoms and canonical model/provider/error/control metadata. Sensitive header values and privacy-sensitive metadata fields are excluded and listed in the JSON `record_coverage` report; the command never searches arbitrary SQLite columns or unreferenced file bytes. Shared chunks are decompressed once per source, matches may span manifest ranges, and results include manifest/stage/trace/session/time anchors where available. The default 512 MiB charged chunk-cache limit is a RAM budget: verified evictions spill to an auto-cleaned temporary file and remain reusable without decompression. Scan-limit, temporary-disk, corruption, and `--limit N` failures abort rather than return an incomplete result set. |
| `extract --trace-id ID --artifact KIND` | Write exact mixed legacy/LAR bytes to stdout or `--output`; kinds are `request`, `upstream-request`, `response`, and `raw-stream`. `--force` is required to replace an output. |
| `replay ARCHIVE --trace-id ID` | Replay a captured stream from a standalone or sealed archive. Raw mode preserves observed HTTP-client read boundaries; `--parsed` independently emits recorded SSE/NDJSON ranges. `--speed instant\|0.25x\|0.5x\|1x\|2x\|4x` controls timing and defaults to instant so a long capture cannot accidentally block for hours. Use `--stage-id` when a trace has multiple streams and `--output` for a file. |
| `transaction --trace-id ID --output FILE [--archive SEALED.lar]` | Export one complete transaction as a verified RFC 7464 JSON sequence. Live canonical data is streamed directly from catalog packs; a sealed archive can be selected explicitly. The source-neutral timeline preserves each stage's actual Exchange identity while ordering strict late tool supplements and excluding ordinary child/subagent lineage. Header/trailer atoms, body bytes/content IDs, transport/routing/usage/error metadata, and stream timing remain addressable; shared bodies are emitted once in decoded pieces of at most 48 KiB. Legacy-only traces use a direct bounded `synthesized_legacy` path. Output is synced and atomically published; `--force` is required to replace it. See [complete-transaction format](lar-transaction-json-seq.md). |
| `transaction-replay FILE` | Validate a complete transaction sequence, then replay one captured stream's exact observed reads or `--parsed` frame ranges. Use `--stage-id` when several streams exist and `--speed instant\|0.25x\|0.5x\|1x\|2x\|4x`. File output is atomically published and never partially replaces an existing destination on corruption or truncation. No HTTP/provider framing is invented. |
| `export OUTPUT --format lar\|har\|warc\|jsonl\|otel\|openinference` | Export one `--trace-id`, one `--session`, or the complete live trace catalog. For cataloged LAR traces, all formats derive from the exact canonical exchange/stage timeline, including retries, ordered duplicate-preserving headers/trailers, stream indexes, late linked tool supplements, and one descriptor per distinct logical body. Unavailable canonical sources fail instead of degrading; genuinely legacy-only traces use a separate declared-loss path. Bodies are copied/encoded in fixed-size windows, interchange trace selection is cursor-paged from a stable high-water mark, and a deletion/filter mutation that changes the emitted count aborts publication. Output is written to a synced sibling temp file before atomic publication. Native LAR also copies the conversation closure and verifies every reconstructed body. JSONL v1 remains the legacy/import-compatible shape; canonical or mixed output is JSONL v2 with bounded body-part records and is currently export-only. HAR/WARC/semantic adapters preserve the canonical graph in Alex extensions while reporting losses in their standard projections. `otel` uses current `gen_ai.*` names and `openinference` uses its distinct vocabulary. `--force` is required to replace output. |
| `cleanup --dry-run` | Run a full verification pass and report the legacy files/bytes eligible for cleanup. No file is moved. |
| `cleanup --apply` | Only after every migration job is complete with no pending/failed items and full verification passes, move legacy body files into a recoverable, audited quarantine below `lar/quarantine/`. LAR data is never removed. |

```bash
alex lar import-legacy --dry-run --verify --json
alex lar import cold-archive.lar --json
alex lar ls --json
alex lar detach --file-uuid FILE_UUID --json
mv ~/.alexandria/archives/source.lar /Volumes/archive/alex/source.lar
alex lar reattach --file-uuid FILE_UUID \
  --archive /Volumes/archive/alex/source.lar --json
alex lar import traces.jsonl --format jsonl --json
alex lar export traces.openinference.jsonl --format openinference --session SESSION_ID
alex lar migration status
alex lar migration verify --json
alex lar gc plan --json
alex lar repack plan --min-garbage-ratio 0.25 --json
alex lar upgrade archived-v1.lar --output archived-latest.lar --json
alex lar grep 'tool_call_id' archived-2026-07-19.lar --limit 500 --json
alex lar grep 'rate_limit' archived-2026-07-19.lar --scope whole-record --json
alex lar cleanup --dry-run --json
alex lar extract --trace-id 019f6872-a3ee-7431-b4bb-2bafbabb7235 \
  --artifact response --output response.sse
alex lar replay session.lar --trace-id 019f6872-a3ee-7431-b4bb-2bafbabb7235 \
  --speed 2x
alex lar replay session.lar --trace-id 019f6872-a3ee-7431-b4bb-2bafbabb7235 \
  --parsed --speed instant --output parsed-events.sse
alex lar transaction --trace-id 019f6872-a3ee-7431-b4bb-2bafbabb7235 \
  --output trace.transaction.jsonseq --json
alex lar transaction-replay trace.transaction.jsonseq \
  --stage-id STAGE_CONTENT_ID --speed instant --output raw-stream.sse
```

JSONL v1 import defaults cap a physical line at 768 MiB, each decoded body at
256 MiB, metadata at 2 MiB, headers at 65,536 fields/8 MiB, the stream at one
million trace records, and total input at 4 TiB. These are rejection limits,
not buffering targets: only one line and its at-most-three decoded bodies are
resident. The source manifest's fidelity-loss list and per-record header
fidelity are returned in the import report. Header arrays must agree with the
raw header JSON retained in trace metadata, which makes export/import/export
stable for current Alex JSONL v1 files. JSONL v2 preserves the canonical graph
and emits body bytes in 48 KiB records, but the v1 importer rejects it with an
explicit explanation rather than discarding retries, trailers, streams, or
tool links. Use standalone LAR for a currently re-importable lossless export.

## `env`

Print model-client exports using the configured host, port, and local key.

```bash
eval "$(alex env)"
```

This command emits secrets to stdout. `credentials` is the faster config-only
connection export with JSON/host options.

## `connect`

Detect or configure one installed harness to use Alex.

| Argument | Meaning |
| --- | --- |
| `[HARNESS]` | Omit to show detection status. |
| `--config-dir <PATH>` | Override the native harness config directory. |
| `--url <URL>` | Remote daemon URL; environment alternative `ALEXANDRIA_URL`. |
| `--key <KEY>` | Pre-minted harness key; environment alternative `ALEXANDRIA_HARNESS_KEY`. Requires harness and remote URL. |
| `--key-id <ID>` | Cosmetic ID recorded for a pre-minted key. |
| `--tool-capture` | Install tool-execution hooks during this connection. |
| `--json` | Machine-readable status/result. |

```bash
alex connect codex --tool-capture
alex connect pi --url https://alex.example.invalid --key '<redacted>' --key-id rk-abcd1234
```

The fully remote pre-minted form is handled before local config loading, so it
does not create `~/.alexandria/config.toml` in a worker/container.

## `tool-capture`

Show or set explicit per-harness tool-capture consent. State is `on` or `off`;
omit it to inspect the current setting.

```bash
alex tool-capture pi on --json
```

## `disconnect`

Remove Alex-managed harness configuration and revoke its keys. Optional
`--config-dir` targets a non-default native directory.

```bash
alex disconnect codex
```

## `ping`

Send a tiny provider request using the configured ping model. Target defaults
to `all`; accepted provider aliases include Anthropic, OpenAI, Grok, Gemini,
OpenRouter, Kimi, plus the special `dario` target.

```bash
alex ping openrouter
alex ping dario
```

`all` pings each active pingable provider found in the vault and Dario.

## `harness`

Manage frozen Docker CLI smoke tests.

| Subcommand | Important arguments | Example |
| --- | --- | --- |
| `list` | `--json`; lists known smoke definitions. | `alex harness list --json` |
| `run <HARNESS>` | Harness definition (`claude`, `codex`, or `grok` in the clap contract); optional `--model`, `--prompt`, `--package-tarball`, `--docker-image`, `--container-base-url`, `--timeout-secs`, `--no-trace-check`, `--run-key-file`, `--run-id`, `--json`. | `alex harness run codex --model alex/gpt-5.6-sol --prompt PING --timeout-secs 90 --json` |
| `pack <TARGET>` | Harness or npm package; optional `--version`, `--force`, `--json`. | `alex harness pack @anthropic-ai/claude-code --version 2.1.0 --json` |

## `dario`

Install and operate the generational Claude-subscription broker.

| Subcommand | Effect | Example |
| --- | --- | --- |
| `bootstrap` | Install with npm/pnpm/Bun; `--json`. | `alex dario bootstrap --json` |
| `enable` | Persist `anthropic_upstream="dario"`; restart required. | `alex dario enable` |
| `disable` | Persist `anthropic_upstream="direct"`; restart required. | `alex dario disable` |
| `auto` | Persist automatic subscription-based routing; restart required. | `alex dario auto` |
| `status` | Query daemon generation/routing/prompt-cache state. | `alex dario status` |
| `restart` | Roll a fresh generation of the current version. | `alex dario restart` |
| `update` | Check npm and roll when newer. | `alex dario update` |
| `fix` | Discover/persist Node and Claude paths and start a fresh generation. | `alex dario fix` |

See [Dario](dario.md) for the request rewrite and fallback details.

## `notify`

Inspect notification channels or send a synthetic event.

| Subcommand | Important arguments | Example |
| --- | --- | --- |
| `list` | Returns redacted daemon configuration. | `alex notify list` |
| `test` | Optional `--channel <INDEX>`; otherwise tests every channel. | `alex notify test --channel 0` |

## `fixtures`

Manage named upstream error fixtures through the daemon.

| Subcommand | Important arguments | Example |
| --- | --- | --- |
| `list` | List saved fixtures. | `alex fixtures list` |
| `show <NAME>` | Display one fixture. | `alex fixtures show anthropic-overloaded` |
| `save` | Required `--name`, `--provider`. Either `--from-trace <ID>` (optional `--kind`, default `resp`) or manual `--status`, `--kind`, `--body`; prefix body with `@` to read a file. | `alex fixtures save --name anthropic-overloaded --provider anthropic --status 529 --kind overloaded_error --body @error.json` |
| `rm <NAME>` | Delete one fixture. | `alex fixtures rm anthropic-overloaded` |

## `simulate`

Queue fixtures for the next matching live session/run request.

| Subcommand | Important arguments | Example |
| --- | --- | --- |
| `inject <SESSION> [FIXTURE]` | `--count` defaults to 1. Without a named fixture, require `--inline-status`, `--inline-kind`, and `--inline-body`. | `alex simulate inject ses_123 anthropic-overloaded --count 2` |
| `pending <SESSION>` | Show queued injections. | `alex simulate pending ses_123` |
| `clear <SESSION>` | Remove queued injections. | `alex simulate clear ses_123` |

## `protection`

Write a built-in equivalency preset. The only implemented preset name is
`anthropic-openai`; it writes Fable/Sol mappings and does not enable protection.

```bash
alex protection preset anthropic-openai
```

## `limits`

Show plan and quota-window utilization/reset times for configured providers.

```bash
alex limits --json
```

## `config`

The current config command only changes the persisted daemon host.

| Subcommand | Important arguments | Example |
| --- | --- | --- |
| `host <ADDRESS>` | Address must be a literal IPv4/IPv6 bind address; restart required. | `alex config host 127.0.0.1` |

For friendly `loopback`/`all`/interface choices use `alex service bind`.

## `routing`

Read or update provider/account reserve percentages.

| Subcommand | Important arguments | Example |
| --- | --- | --- |
| `get <PROVIDER>` | Optional `--json`. | `alex routing get codex --json` |
| `set <PROVIDER>` | Required `--reserve-pct <0..100>`; optional `--account <NAME_OR_ID>`. | `alex routing set codex --reserve-pct 15 --account work` |

## `provider`

Apply transient provider-wide fault controls. These are deliberate test/alert
controls, separate from persistent account pause.

| Subcommand | Important arguments | Example |
| --- | --- | --- |
| `list` | Show provider pause state. | `alex provider list` |
| `pause <PROVIDER>` | `--mode down|logged_out`, default `down`. | `alex provider pause anthropic --mode logged_out` |
| `resume <PROVIDER>` | Clear the transient pause. | `alex provider resume anthropic` |

## `service`

Manage the user launchd service on macOS or systemd user service on Linux.

| Subcommand | Important arguments | Example |
| --- | --- | --- |
| `install` | Install service pointing at the current binary. | `alex service install` |
| `bind <TARGET>` | Persist `loopback`, `all`, or a literal interface IP. | `alex service bind loopback` |
| `restart` | Gracefully drain/restart; `--force` uses the legacy hard restart. | `alex service restart` |
| `uninstall` | Stop and remove the user service. | `alex service uninstall` |
| `status` | Print detected service state. | `alex service status` |

## `wrap`

Launch connected Claude/Codex profiles or reverse-wrap catalog harnesses.

| Subcommand | Important arguments | Example |
| --- | --- | --- |
| `status` | List embedded/user wrap harnesses; `--json`. | `alex wrap status --json` |
| `env [HARNESS]` | Harness defaults to `amp`; optional `--mode`, `--wrap-url` (default `http://127.0.0.1:4101`), `--ca-cert`, `--json`. Writes settings and prints exports/plan. | `alex wrap env amp --mode base_url` |
| `claude [ARGS...]` | Passes all remaining args to connected `claude`. | `alex wrap claude -p 'Reply PONG'` |
| `codex [ARGS...]` | Passes all remaining args to `codex` with the Alex profile. | `alex wrap codex exec 'Reply PONG'` |
| `amp [ARGS...]` | Reverse wrap plus Amp; optional remote trace flags, `--mode`, `--bind` (default ephemeral), `--upstream`, `--serve-only`, `-q/--quiet`. Use `--` before Amp flags. | `alex wrap amp -- -x 'Reply PONG'` |
| `agent [ARGS...]` | Same controls for Cursor Agent; use `--` before agent flags. | `alex wrap agent -- --print --trust 'Reply PONG'` |
| `run <HARNESS> [ARGS...]` | Generic catalog equivalent of Amp/Agent with the same wrap and remote-trace controls. | `alex wrap run amp --bind 127.0.0.1:4101 -- -x PONG` |
| `smoke` | Mock-upstream reverse-wrap test; `--harness` defaults to `amp`; `--json`. | `alex wrap smoke --harness amp --json` |

See [Amp wrap](amp-wrap.md) for capture details and the remote-spool workflow.

## `up`

Install (when supported), connect, configure, and optionally launch a harness.

| Argument | Behavior |
| --- | --- |
| `[HARNESS]` | Defaults to `pi`. |
| `--url <URL>` | Remote daemon; supplying it never starts a local daemon. |
| `--key <KEY>` | Model-only scoped run key, never the local/admin key. |
| `--model <MODEL>` | Default `alex/gpt-5.6-sol`. |
| `--version <VERSION>` | npm package version when installation is needed. |
| `--no-launch` | Configure only. |
| `-y/--yes` | Reserved for non-interactive callers. |
| `-- [ARGS...]` | Arguments passed to the launched harness. |

```bash
alex up pi --model alex/claude-sonnet-5 -- --help
alex up codex --url https://alex.example.invalid --key '<redacted>' --no-launch
```

## `update`

Check or install Alex releases.

| Flag | Meaning |
| --- | --- |
| `--check` | Report only; never install. |
| `-y/--yes` | Install without confirmation. |
| `--no-restart` | Do not restart a running daemon. |
| `--json` | Machine-readable output for `--check`. |
| `--force` | Proceed when installation appears brew- or cargo-managed. |
| `--channel stable|beta` | One-run channel override. |
| `--set-channel stable|beta` | Persist channel to config, then use it. |

```bash
alex update --check --channel beta --json
alex update --set-channel stable -y
```

## `credentials` (`creds`)

Print client connection exports by reading config only. `--json` returns a
structured payload; `--host` rewrites only the emitted URL host.

```bash
alex credentials --host host.docker.internal
alex creds --json
```

## `status`

One-shot daemon, service, accounts, limits, and Dario overview.

```bash
alex status --json
```

## `keys`

Manage scoped run, harness, and wrap keys through the running daemon. Raw keys
are printed once.

| Subcommand | Important arguments | Example |
| --- | --- | --- |
| `mint` | `--kind run|harness|wrap` (default run), optional `--run-id`, repeatable `--tag k=v`, `--ttl` (default `24h`, cap `7d` for run keys), `--label`, `--json`. Harness/wrap require a label and do not expire until revoked. | `alex keys mint --kind run --run-id job-42 --tag team=infra --ttl 2h` |
| `list` | Active only by default; `--all`, `--json`. | `alex keys list --all --json` |
| `revoke <ID>` | Full ID or unique ID prefix. | `alex keys revoke rk-a1b2c3d4` |

## `reset`

Select local data categories; dry-run is the default. Categories are
`--credentials`, `--settings`, `--traces`, `--harnesses`, `--cache`, or
`--all`. `-y/--yes` applies the plan.

```bash
alex reset --traces
alex reset --traces --cache --yes
```

Credentials remove account JSON and revoke run keys while keeping tombstones;
settings restore defaults but preserve `update_channel`; traces remove rows,
heartbeats, and bodies; harnesses use normal disconnect; cache removes derived
pricing/prompt-cache data and re-seeds bundled pricing.

## `tui`

Open the live Sessions, Limits, Accounts, and Dario dashboard explicitly.

```bash
alex tui
```

Next: [Overview](overview.md) · [Configuration](configuration.md) ·
[API and formats](api-and-formats.md)
