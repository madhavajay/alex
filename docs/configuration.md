# Configuration and local state

The main configuration file is `~/.alexandria/config.toml`. Set
`ALEXANDRIA_HOME` to move that root. `data_dir` then controls the SQLite,
account, body, Dario, fixture, and wrap state root; on a fresh install it is the
same directory as `ALEXANDRIA_HOME`.

The loader creates the directory and config on first use. On Unix it writes
`config.toml` mode `0600`. The file contains administrative and notification
secrets: do not publish it.

## Minimal generated shape

Values that are generated per installation are redacted here:

```toml
host = "127.0.0.1"
port = 4100
data_dir = "/home/example/.alexandria"
local_key = "<redacted-alx-key>"
heartbeat_minutes = 15
reauth_check_minutes = 5
anthropic_upstream = "auto"
dario_mode_migrated = true
dario_api_key = "<redacted-dario-key>"
trace_body_retention_days = 30
lar_body_store_mode = "legacy"
lar_durability = "sync"
lar_migration_batch_size = 32
trace_row_retention_days = 0
update_check_hours = 24
update_channel = "stable"
upstream_stream_idle_timeout_seconds = 900

[substitution]
enabled = false

[protection]
enabled = false
reroute_on_auth = false
retries = 1
auto_return = true

[lar_migration_resources]
worker_count = 1
cpu_budget_percent = 100
yield_every_artifacts = 0
max_memory_bytes = 134217728
max_pack_bytes = 536870912
max_pack_index_entries = 262144
```

The actual serializer also writes the other non-`None` fields and empty
tables/lists; absent optional Dario paths/version are omitted. The
`update_channel` default is `beta` for a prerelease daemon build and `stable`
for a stable build.

## Top-level keys

| Key | Type | Fresh-install default | Behavior |
| --- | --- | --- | --- |
| `host` | string | `127.0.0.1` | Bind IP. Must parse as IPv4/IPv6. Local harness URLs still use loopback for non-loopback binds. |
| `port` | integer | `4100` | HTTP listener port. |
| `data_dir` | path | Alex home | Runtime state root. Relative paths are accepted by TOML/path parsing but an absolute path is easier to operate. |
| `local_key` | string | random `alx-...` | Full local/admin and model-ingress credential. |
| `heartbeat_minutes` | integer | `15` | Provider heartbeat interval; `0` disables. |
| `reauth_check_minutes` | integer | `5` | Idle OAuth logout watchdog interval; `0` disables. |
| `ping_anthropic_model` | string | `claude-sonnet-5` | Anthropic health-check model. |
| `ping_openai_model` | string | `gpt-5.5` | OpenAI/Codex health-check model. |
| `ping_xai_model` | string | `grok-code-fast-1` | Grok health-check model. |
| `ping_gemini_model` | string | `gemini-2.5-flash` | Gemini health-check model. |
| `ping_openrouter_model` | string | `google/gemma-4-26b-a4b-it:free` | OpenRouter health-check model (bare OpenRouter ID). |
| `exo_url` | string | `http://localhost:52415` | Local OpenAI-compatible Exo base URL. |
| `exo_enabled_models` | string array | `[]` | Models exposed/routed through Exo. |
| `openrouter_exposed_models` | string array | seven-model starter list below | Bare OpenRouter IDs advertised to `/v1/models` and harnesses. Explicit `[]` exposes none. |
| `gemini_project` | string | `""` | Optional Code Assist project override/cache. `GOOGLE_CLOUD_PROJECT` takes precedence when set. |
| `anthropic_upstream` | string | `auto` | `auto`, `dario`, or `direct`; set through `alex dario auto|enable|disable`. |
| `dario_mode_migrated` | boolean | `true` | Internal one-time legacy `direct` to `auto` migration marker. Do not use as a routing control. |
| `dario_api_key` | string | empty, generated/saved at daemon start | Credential used only between Alex and its local Dario child. |
| `dario_claude_bin` | path or absent | absent | Explicit real Claude Code binary for model prompt capture. An invalid explicit path does not fall through. |
| `dario_node_path` | path or absent | absent | Explicit Node runtime. `alex dario fix` persists discovered Node/Claude paths. |
| `dario_update_check_minutes` | integer | `60` | Dario npm update-check interval. |
| `dario_version` | string or absent | absent | Optional installed/launched Dario version pin. |
| `dario_probe_seconds` | integer | `90` | Active generation probe interval. |
| `dario_probe_failures` | integer | `2` | Consecutive failures before a generation is marked unhealthy. |
| `dario_probe_model` | string | `claude-haiku-4-5` | Tiny through-Dario readiness model. |
| `trace_body_retention_days` | integer | `30` | Gzip body/header retention window. |
| `lar_body_store_mode` | string | `legacy` | Body-store rollout mode: `legacy`, shadow-only `dual-write-validated`, or `lar-with-fallback`. Experimental modes retain gzip rollback copies. Shadow mode disables automatic startup import so it cannot publish owner pointers; manual import remains explicit. |
| `lar_durability` | string | `sync` | LAR publication durability: full `sync`, per-capture data-only `batch`, or shadow-only `best-effort`. `best-effort` is rejected with authoritative `lar-with-fallback` mode. |
| `lar_migration_batch_size` | integer | `32` | Maximum legacy artifacts attempted in one startup migration pass; valid range `1..=4096`. |
| `trace_row_retention_days` | integer | `0` | SQLite trace-row retention; `0` means unlimited. |
| `update_check_hours` | integer | `24` | Release check interval; `0` disables. |
| `update_channel` | string | build-dependent `stable`/`beta` | Persistent release channel. Only `stable` and `beta` are accepted by update controls. |
| `upstream_stream_idle_timeout_seconds` | integer | `900` | Maximum quiet period between upstream chunks; runtime clamps to at least one second. |
| `notification_cooldown_seconds` | integer | `1800` | Duplicate notification cooldown. |
| `notification_timeout_seconds` | integer | `10` | Per-delivery timeout. |

Fresh OpenRouter exposure list:

```toml
openrouter_exposed_models = [
  "tencent/hy3:free",
  "xiaomi/mimo-v2.5",
  "deepseek/deepseek-v4-flash",
  "deepseek/deepseek-v4-pro",
  "minimax/minimax-m3",
  "z-ai/glm-5.2",
  "nvidia/nemotron-3-ultra-550b-a55b:free",
]
```

## Harness settings

Harness overrides are keyed tables. Both fields are optional:

```toml
[harness_overrides.pi]
binary = "/opt/local/bin/pi"
config_dir = "/home/example/.pi/agent"

[harness_tool_capture]
pi = true
codex = false
```

`harness_overrides.<id>.binary` overrides executable detection;
`config_dir` overrides the harness's native configuration directory.
`harness_tool_capture.<id>` is explicit consent for command args/results. Missing
entries are false.

## Account policy

Each table key is a canonical provider such as `anthropic`, `openai`, `xai`,
`gemini`, `openrouter`, `amp`, or `kimi` (aliases are accepted while opening the
vault).

```toml
[account_policy.openai]
order = ["work", "personal"]
mode = "reset_first"       # priority | round_robin | threshold | reset_first
threshold_pct = 80
reserve_pct = 10
allow_mid_thread_failover = true
disabled = []

[account_policy.openai.account_reserve_pct]
work = 15
```

| Field | Serde default | Meaning |
| --- | --- | --- |
| `order` | `[]` | Account-name priority. |
| `mode` | `priority` | Selection algorithm. When no OpenAI table exists, the provider's effective built-in default is `reset_first`. |
| `threshold_pct` | absent (algorithm uses 80) | Threshold-mode boundary. |
| `reserve_pct` | absent (router uses 10) | Provider capacity reserve. OpenAI's built-in policy explicitly sets 10. |
| `account_reserve_pct` | `{}` | Per-name or per-ID reserve overrides. |
| `allow_mid_thread_failover` | `true` | Permit an affined Codex thread to move accounts after an eligible failure. |
| `disabled` | `[]` | Account names/IDs excluded from proxy selection. |

`alex routing set` persists the reserve policy to the vault's
`.routing-policies` file as well as applying it to the opened vault. On open,
that sidecar is loaded first; any provider also present in
`config.toml`'s `account_policy` map is then replaced by the config policy. See
[Providers and routing](providers-and-routing.md).

## Substitution and protection

```toml
[substitution]
enabled = true

[substitution.fallbacks]
"claude-fable-5" = ["openai/gpt-5.6-sol"]

[protection]
enabled = true
reroute_on_auth = false
retries = 1
auto_return = true

[protection.equivalencies.claude-fable-5]
openai = "gpt-5.6-sol"
```

| Key | Default | Meaning |
| --- | --- | --- |
| `substitution.enabled` | `false` | Enable ordered, explicit cross-model fallbacks. |
| `substitution.fallbacks.<model>` | absent | Array of provider-prefixed fallback model IDs. |
| `protection.enabled` | `false` | Enable retry/equivalency protection. |
| `protection.reroute_on_auth` | `false` | Allow equivalency rerouting for auth-class errors. |
| `protection.retries` | `1` | Protection retry count. |
| `protection.auto_return` | `true` | Persisted protection behavior flag exposed by admin controls. |
| `protection.equivalencies.<model>.<provider>` | absent | Equivalent model ID for a different provider. |

`alex protection preset anthropic-openai` writes the current Fable/Sol
equivalencies but intentionally does not turn `protection.enabled` on.

## Legacy LAR migration resources

The startup importer begins only after the daemon health endpoint responds. Its
defaults preserve the previous single-worker, unthrottled behavior. Configure
resource limits under `lar_migration_resources` when conversion must share a
busy machine with live capture and Trace Browser reads:

```toml
lar_migration_batch_size = 32

[lar_migration_resources]
worker_count = 2
io_bytes_per_second = 52428800
cpu_budget_percent = 50
yield_every_artifacts = 8
max_memory_bytes = 134217728
max_pack_bytes = 536870912
max_pack_index_entries = 262144
min_free_disk_bytes = 10737418240
```

| Field | Default | Valid values and behavior |
| --- | --- | --- |
| `worker_count` | `1` | `1..=16`. Bounds the parallel source-provenance workers; archive appends remain serialized. |
| `io_bytes_per_second` | absent | Positive bytes/second. When absent, importer reads are not rate-limited. The limit is shared by all workers. |
| `cpu_budget_percent` | `100` | `1..=100`. Adds cooperative rest between completed artifacts to approximate this duty-cycle budget. |
| `yield_every_artifacts` | `0` | Explicitly yield after this many completed artifacts; `0` disables explicit yields. |
| `max_memory_bytes` | `134217728` | At least 1 MiB. Bounds predecessor buffers and caches and derives caps for pending inventory (`memory / 4096`) and retained pack-index entries (`memory / 256`). Individual bodies are streamed; one artifact may temporarily exceed these planning estimates according to its own bounded size. |
| `max_pack_bytes` | `536870912` | Positive soft size cap for one importer pack. After a validated artifact crosses the cap, the pack is sealed and the next artifact continues in a deterministic successor pack. |
| `max_pack_index_entries` | `262144` | Positive cap on retained chunk/manifest index entries. The effective cap is the smaller of this value and `max_memory_bytes / 256`. Rotation occurs between artifacts, so one unusually large artifact is the maximum overshoot. |
| `min_free_disk_bytes` | absent | Pause before writes or between batches when available bytes fall below this threshold. Legacy files stay readable and the durable job can resume later. |

Each migration pass reports durable item totals and completion percentage,
bytes/artifacts per second, deduplication ratio, ETA, last error, worker usage,
throttled time, yields, free disk, configured/effective batch and pack caps,
pack sequence/rotations, any disk-pressure pause reason, and whether detailed
errors were truncated after 256 entries. Invalid limits fail that migration
pass without deleting or switching legacy sources.

## Notifications

Notifications use top-level array-of-table entries:

```toml
notification_cooldown_seconds = 1800
notification_timeout_seconds = 10

[[notifications]]
id = "ops-webhook"
kind = "webhook"
format = "slack"            # generic | telegram | slack | discord
url = "https://hooks.example.invalid/<redacted>"
min_level = "warn"          # info | warn | critical
categories = ["reauth"]

[[notifications]]
kind = "webhook"
format = "telegram"
token = "<redacted>"
bot_username = "alex_status_bot"
chat_id = "<redacted>"
allow_commands = false
min_level = "info"
categories = []
```

Every notification field is optional in deserialization. Defaults are: no ID,
`kind="webhook"`, `format="generic"`, empty URL, no token/bot/chat, commands
off, `min_level="info"`, and all categories. The admin view never returns
webhook URLs or Telegram tokens unredacted.

## Environment variables

These are the user-facing overrides read by the current code. Internal test,
launchd, and Dario-child variables are omitted.

| Variable | Used by |
| --- | --- |
| `ALEXANDRIA_HOME` | Config/state root containing `config.toml`; fresh `data_dir` defaults to it. |
| `ALEXANDRIA_LAR_BODY_STORE` | Daemon-only override for `lar_body_store_mode`; invalid values fail startup instead of silently changing storage behavior. |
| `ALEXANDRIA_LAR_DURABILITY` | Daemon-only override for `lar_durability`; accepts `sync`, `batch`, or `best-effort`. Invalid values fail startup. |
| `ALEXANDRIA_URL` | Remote `alex connect`/`alex up` base URL. |
| `ALEXANDRIA_HARNESS_KEY` | Pre-minted remote harness key for `alex connect`. Requires a remote URL. |
| `ALEXANDRIA_RUN_ID` | Caller-selected wrap run ID. |
| `ALEXANDRIA_TRACE_URL` | Central daemon for wrapped trace upload. |
| `ALEXANDRIA_TRACE_KEY` | Inline `kind=wrap` upload key. Prefer a protected file for persistence. |
| `ALEXANDRIA_TRACE_KEY_FILE` | File containing the wrap upload key. |
| `ALEXANDRIA_TRACE_ALLOW_INSECURE_HTTP` | Truthy value permits non-loopback plaintext trace upload. |
| `ALEXANDRIA_NODE_BIN` | Dario Node runtime discovery override. Persist `dario_node_path` for services. |
| `ALEXANDRIA_REAL_CLAUDE_BIN` | Real Claude executable discovery override. Persist `dario_claude_bin` for services. |
| `ALEXANDRIA_DRAIN_TIMEOUT_SECONDS` | Graceful service-restart drain timeout. |
| `ALEXANDRIA_GRACEFUL_RESTART` | Set to `0` to opt out of graceful restart behavior. |
| `GEMINI_API_KEY` | Fallback input for `alex auth gemini-key`. |
| `OPENROUTER_API_KEY` | Fallback input for `alex auth openrouter-key`. |
| `AMP_API_KEY` | Fallback input for `alex auth amp-key` and wrap credential resolution. |
| `GOOGLE_CLOUD_PROJECT` | Gemini Code Assist project override before stored/discovered project. |
| `KIMI_CODE_HOME` | Native Kimi credential root for import. |
| `RUST_LOG` | Standard tracing filter; daemon default is `info,alexandria=debug`, other commands default to `warn`. |

`alex credentials` prints the common client variables (`ANTHROPIC_BASE_URL`,
`OPENAI_BASE_URL`, `GOOGLE_GEMINI_BASE_URL`, and matching key-bearing exports)
from config. It does not redact `local_key`; protect its stdout. Use its output
instead of hard-coding a bind wildcard as a client URL.

## On-disk layout

Paths below are relative to `data_dir` unless stated otherwise:

```text
~/.alexandria/config.toml         main config (always under ALEXANDRIA_HOME)
<data_dir>/accounts/*.json        active vault accounts, mode 0600 writes
<data_dir>/accounts/removed-accounts/*.json
<data_dir>/accounts/.routing-policies
<data_dir>/alexandria.sqlite3     trace/pricing/key/lineage store
<data_dir>/alexandria.sqlite3-wal
<data_dir>/alexandria.sqlite3-shm
<data_dir>/bodies/YYYY-MM-DD/*.gz
<data_dir>/dario/                 packages, generations, logs, update state
<data_dir>/dario-prompt-cache/*.json
<data_dir>/fixtures/              saved error fixtures
<data_dir>/wrap/<harness>/        settings, flows.jsonl, WS/body sidecars, logs
~/.alexandria/wrap-harnesses.json optional replacement wrap catalog
~/.alexandria/daemon.log          background-daemon log
```

Connected harness configuration is stored under each tool's native config
directory, not solely under `data_dir`. See [Harness integration](harnesses.md)
and [Amp wrap](amp-wrap.md).

Next: [Overview](overview.md) · [CLI reference](cli.md) · [Dario](dario.md)
