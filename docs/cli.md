# `alex` command reference

`alex` and `alexandria` are identical binaries. Examples use `alex`. With no
subcommand and an interactive terminal, the binary opens the TUI; with
non-terminal stdout it errors and asks for an explicit command.

This page follows the clap definitions in `crates/alex/src/main.rs`. Values in
angle brackets are required; square brackets are optional. Clap converts enum
names to kebab case. Every command also supports generated `--help`; the root
supports `--version`.

## `daemon`

Run the proxy in the foreground, or detach to `~/.alex/daemon.log`.

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

## `resume`

Start a new interactive Pi, Claude, or Codex session from a captured session's
conversation history:

```bash
alex resume 5d56cba0-b43b-464a-99fb-5bbca2bcc46d
alex resume 5d56cba0-b43b-464a-99fb-5bbca2bcc46d codex
alex resume shared-id pi --source-harness claude --dry-run
```

Omitting the target harness opens a picker containing installed, connected
harnesses. `--source-harness` disambiguates an ID captured from more than one
harness. `--dry-run` validates and summarizes the fork without launching or
printing the conversation.

The fork keeps ordered user/assistant messages and tool calls/results while
excluding captured system instructions and hidden reasoning. If the prompt is
too large for safe process launch, the oldest complete entries are omitted and
the summary reports that truncation.

Alex traces do not currently store a canonical working directory. The launcher
therefore accepts a directory only from exact local harness metadata: Pi's
session header, Claude's latest matching transcript record, or Codex's latest
state database (with rollout files as a fallback). The directory must still
exist. Otherwise the command visibly falls back to the directory from which
`alex resume` was invoked.

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
| `--url <URL>` | Remote daemon URL, or the upstream URL for `cliproxyapi`; environment alternatives are provider-specific. |
| `--key <KEY>` | Pre-minted harness key, or the `cliproxyapi` bearer credential; environment alternatives are provider-specific. |
| `--key-id <ID>` | Cosmetic ID recorded for a pre-minted key. |
| `--tool-capture` | Install tool-execution hooks during this connection. |
| `--json` | Machine-readable status/result. |

```bash
alex connect codex --tool-capture
alex connect pi --url https://alex.example.invalid --key '<redacted>' --key-id rk-abcd1234
alex connect cliproxyapi --url http://127.0.0.1:8317/v1 --key '<redacted>'
```

The fully remote pre-minted form is handled before local config loading, so it
does not create `~/.alex/config.toml` in a worker/container.

`cliproxyapi` is a provider connection rather than a harness connection. Alex
probes the upstream `/v1/models` endpoint with the bearer credential before it
saves or replaces the connection. Use HTTPS for remote servers; plain HTTP is
accepted only for loopback addresses. Discovered models are exposed through
Alex as `cliproxyapi/<upstream-model-id>`.

## `cliproxyapi`

Manage the reverse CLIProxyAPI → Alex arrangement. `export` probes
`/v1/alex/capabilities`, reads Alex's model catalog, mints a dedicated scoped
harness key for a local daemon, and creates (without overwriting) a mode-`0600`
CLIProxyAPI v7 config fragment:

```bash
alex cliproxyapi capabilities
alex cliproxyapi export \
  --output ./alex-provider.yaml \
  --cliproxyapi-version v7.4.1
```

Repeat `--model` to export only selected `alex/*` models. For a remote Alex,
provide an existing scoped key through `--key-file` or
`ALEXANDRIA_HARNESS_KEY`; the remote local/admin key is never requested. The
generated file contains the scoped credential and must be merged privately
into CLIProxyAPI's `config.yaml`. See [CLIProxyAPI integration](cliproxyapi.md).

Run `./test.sh cliproxyapi` (or
`./scripts/cliproxyapi-v1-integration.sh` directly) for the pinned real-binary
compatibility gate.

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
| `disable` | Persist the legacy `direct` value; genuine Claude Code is still the only direct Anthropic path. | `alex dario disable` |
| `auto` | Persist the default always-Dario route for eligible Anthropic traffic; restart required. | `alex dario auto` |
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

## `doctor`

Run bounded, secret-safe diagnostics for the activation path:

```bash
alex doctor
alex doctor --json
```

The report checks the running executable and detected harnesses, duplicate Alex
installs, data/config permissions, storage writability and SQLite integrity,
the OS user service, the local daemon port, connected credential state,
provider health, and Dario readiness/runtime prerequisites. It does not print
tokens, API keys, request bodies, or response bodies. The command exits nonzero
when a blocking check fails; warnings leave the exit status successful.

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
