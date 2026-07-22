# Providers, vault accounts, and routing

Alex routes a model name to a provider, selects an eligible account for
that provider, translates the request if needed, and injects credentials from
the local vault. Provider choice, account choice, and model fallback are three
separate decisions.

See [Configuration](configuration.md) for `[account_policy.*]`,
`[substitution]`, and `[protection]`; see [API and formats](api-and-formats.md)
for dialect conversion.

## Provider implementations

| Provider | Vault material and upstream | Formats accepted at ingress | Notes |
| --- | --- | --- | --- |
| Anthropic / Claude | OAuth subscription or API key; direct Messages API, or supervised Dario for eligible subscription traffic | All four dialects | Anthropic Messages is the translation pivot. Genuine Claude Code requests bypass Dario and preserve an allowlist of CLI headers. |
| OpenAI / Codex | ChatGPT/Codex OAuth or OpenAI API key | All four dialects | OAuth uses the ChatGPT Codex Responses backend and thread/account affinity. API keys use public OpenAI Chat/Responses. |
| Gemini / Google | Gemini CLI OAuth (Code Assist) or Google AI Studio API key | All four dialects | API-key accounts are preferred. OAuth requests use a discovered/configured Google project and the Code Assist envelope. |
| Grok / xAI | Grok OAuth subscription | Anthropic Messages or OpenAI Chat | Upstream is OpenAI-chat-compatible. Responses and Gemini ingress return `501 Not Implemented` for this provider. |
| Kimi Code | Imported or device-flow OAuth | Anthropic Messages or OpenAI Chat | Routes to Kimi's coding Chat Completions endpoint. Responses and Gemini ingress are not implemented. |
| OpenRouter | API key plus optional HTTP-Referer/X-Title attribution | Anthropic Messages or OpenAI Chat | Model IDs are `openrouter/<vendor>/<model>`. Only the configured curated subset is advertised to harnesses. |
| CLIProxyAPI | User-managed URL and bearer credential | Anthropic Messages, OpenAI Chat, or OpenAI Responses | Upstream models use `cliproxyapi/<model>`. The reverse CLIProxyAPI → Alex arrangement is generated with `alex cliproxyapi export`. |
| Amp | Amp access token/API key | None | Billing and reverse-wrap capture only. `alex wrap amp` handles the product protocol; `/v1` routing returns `501`. |
| Exo | No vault account; configured local URL and dummy bearer auth | Anthropic Messages or OpenAI Chat | Local OpenAI-chat-compatible provider, enabled per model with `exo_enabled_models`. |

## Adding credentials

Native import reads installed tool credentials:

```bash
alex auth import all
alex auth import kimi --name work
```

Import sources are `claude`, `codex`, `gemini`, `grok`/`xai`, `amp`, `kimi`,
or `all`. Terminal login supports `claude`, `codex`, `grok`, `gemini`, `amp`,
and `kimi` (provider aliases are accepted):

```bash
alex auth login codex --name work
alex auth login claude --name personal
```

API-key-only registration has dedicated commands:

```bash
GEMINI_API_KEY='redacted' alex auth gemini-key
OPENROUTER_API_KEY='redacted' alex auth openrouter-key \
  --referer https://example.invalid --title 'Local Alex'
AMP_API_KEY='redacted' alex auth amp-key
```

Account names must match `[a-z0-9_-]{1,32}`. `--force` replaces an existing
named login. Account JSON files live under `<data_dir>/accounts/` and are
permission-restricted local secret material; encrypted portability is provided
by `alex vault export`, not by claiming those JSON files are ciphertext.

## Multi-account vault

Each account has a provider, kind (`oauth`, `api_key`, or product-specific),
name, status, pause state, optional refresh/expiry data, observed quota
metadata, and credentials. The vault:

- refreshes eligible OAuth accounts before use;
- skips accounts that are inactive, explicitly paused, disabled by policy, or
  excluded after a failed attempt;
- honors cooldowns after transient failures;
- records removed-account tombstones so historical traces remain attributable;
- can merge duplicate accounts and re-key their trace history with
  `alex auth merge`.

Pausing an account is persistent:

```bash
alex auth pause codex work
alex auth resume codex work
```

`alex provider pause` is different: it is an in-memory provider-wide fault
control used to simulate `down` or `logged_out` behavior and is cleared by
resume or daemon restart.

## Account selection policies

Policies are stored by canonical provider under `[account_policy.<provider>]`.
The implementation recognizes four modes:

| Mode | Selection order |
| --- | --- |
| `priority` | Prefer the configured `order`; reserve-blocked accounts sort after available accounts. This is the general default. |
| `round_robin` | Rotate among ready accounts in the best credential/routing rank. If every ready account is reserve-blocked, rotate through that ready set. |
| `threshold` | Sort accounts at or above `threshold_pct` (default 80) after accounts below it, then apply configured order. |
| `reset_first` | Prefer the account whose binding active quota window resets earliest, after credential and reserve checks. This is the OpenAI default. |

Common fields:

| Key | Meaning |
| --- | --- |
| `order = ["work", "personal"]` | Account-name priority. Unlisted accounts follow listed accounts. |
| `mode = "priority"` | One of the four modes above. |
| `threshold_pct = 80` | Threshold-mode utilization boundary. |
| `reserve_pct = 10` | Provider default capacity reserve. If absent, selection uses 10%. |
| `account_reserve_pct = { work = 20 }` | Per-account override by name or ID. |
| `allow_mid_thread_failover = true` | Whether an already-affined OpenAI/Codex thread can move accounts after an eligible failure. |
| `disabled = ["old-account"]` | Accounts excluded from proxy selection without changing their account file status. |

Example:

```toml
[account_policy.openai]
mode = "reset_first"
order = ["work", "personal"]
reserve_pct = 10
allow_mid_thread_failover = true
disabled = []

[account_policy.openai.account_reserve_pct]
work = 15
personal = 5
```

The CLI exposes reserve changes without requiring a manual TOML edit:

```bash
alex routing set codex --reserve-pct 15 --account work
alex routing get codex --json
```

An account is reserve-blocked when an active observed quota window reaches
`100 - reserve_pct` percent used. A reserve of zero disables this gate. `alex
limits` reports the captured/discovered windows and reset times used by the
router. Multiple accounts increase aggregate usable subscription capacity
(the project calls this tokenmax), but they remain independently metered
accounts rather than one upstream credential.

## Model-to-provider routing

`route_model` first strips a harness namespace (`alex/`,
`cove/`, or Claude-compatible `claude-alex/`), then resolves an explicit
provider prefix.

| Example client model | Provider | Upstream model |
| --- | --- | --- |
| `alex/claude/sonnet-5` | Anthropic | `claude-sonnet-5` |
| `openai/gpt-5.6-sol` | OpenAI | `gpt-5.6-sol` |
| `gemini:gemini-2.5-flash` | Gemini | `gemini-2.5-flash` |
| `openrouter/deepseek/deepseek-v4-pro` | OpenRouter | `deepseek/deepseek-v4-pro` |
| `kimi/k3` | Kimi | `k3` |
| `exo/local-model` | Exo | `local-model` |

Accepted explicit aliases include `claude`/`anthropic`,
`openai`/`codex`/`chatgpt`, `gemini`/`google`, `grok`/`xai`, `openrouter`,
`exo`, and `kimi`. Short Anthropic aliases such as `sonnet-5` and `opus-4.8`
are expanded after their provider prefix.

Without a provider prefix, names beginning with `claude`, `gpt`, `codex`,
`chatgpt`, `o<digit>`, `gemini`, or `grok` are inferred. Otherwise the client
dialect decides: Anthropic ingress defaults to Anthropic, OpenAI ingress to
OpenAI, and Gemini ingress to Gemini.

## Affinity and failover

Codex Responses conversations use a derived session key to remain on the same
OpenAI account for prompt-cache continuity. An eligible preferred account wins
unless it is paused, disabled, excluded, or (normally) cooling. The mapping is
bounded and expires after 30 days. When mid-thread failover is enabled, a
successful failover rebinds the session to the replacement account.

The proxy retries another account only for its coarse `capacity` and `server`
error classes. Auth, bad-request, client-disconnect, network, and other errors
are not ordinary account-failover triggers. A retry account must be ready and
not reserve-blocked; the selector's degraded "soonest cooldown" escape hatch is
not used for retries.

## Fable 5 → GPT-5.6 Sol fallback

Alex ships one default middleware rule: `alex.fable-5-to-gpt-5.6-sol`.
When an Anthropic Fable 5 request receives HTTP `529` and its bounded error
body contains Anthropic's documented `overloaded_error` type, Alex retries that
request with high-effort `gpt-5.6-sol` through OpenAI. The fallback is
request-scoped, so the next request starts on Fable 5 again. The request header
`x-alex-no-substitute: 1` disables the reroute for that request.

The starter fixture reproduces that documented envelope but is synthetic; it
is not presented as a captured production Fable failure. The rule is editable
in Settings → Middleware using the Middleware Wizard. Optional match and
replacement effort levels are represented by `when.efforts` and
`then.reroute.effort`. Request scope retries only the failed call; session
scope creates a time-bounded lease after success so later calls in the same
stable session use the replacement route. If “Tell the harness” is enabled,
notice text can use `{from_model}` and `{to_model}` placeholders.

If no eligible OpenAI account can serve Sol, Alex returns the original response
and records why the reroute could not execute. Trace middleware records include
the readable rule name and execution explanation.

The deterministic acceptance contract is
`crates/alex-proxy/tests/fixtures/middleware/fable-to-sol-acceptance.json`. It
covers the request-scoped reroute, the next request returning to Fable, and an
unavailable fallback account.

Next: [Configuration](configuration.md) · [API and formats](api-and-formats.md)
· [Traces](traces.md)
