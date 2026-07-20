# Dario broker

Dario is a supervised local proxy used to make a Claude subscription usable by
non-Claude-Code clients. Harnesses still call Alex. Alex chooses the
Anthropic account, sends eligible Messages requests to the active Dario
generation, and records the real Anthropic account as the billing/trace identity
rather than the synthetic Dario connection key.

This path is specific to Anthropic. It does not make Amp a `/v1` provider and it
does not replace Alex's general format translation.

## Routing modes

`anthropic_upstream` remains a tri-state stored value for compatibility, but
non-Claude-Code Anthropic traffic is always routed through Dario:

| Command | Stored value | Effective behavior after daemon restart |
| --- | --- | --- |
| `alex dario enable` | `dario` | Route eligible non-Claude-Code Anthropic requests through Dario. |
| `alex dario disable` | `direct` | Legacy spelling; direct remains reserved for genuine Claude Code. |
| `alex dario auto` | `auto` | Route eligible non-Claude-Code Anthropic requests through Dario. This is the default. |

Example setup:

```bash
alex auth import claude
alex dario bootstrap
alex dario auto
alex service restart
alex dario status
```

Bootstrap installs `@askalf/dario` with npm, pnpm, or Bun; the launched runtime
must be Node.js 18 or newer. `alex dario fix` discovers and persists the Node
and real Claude Code executable paths, then rolls a fresh generation.

## When Dario engages

The provider planner considers Dario only when all of these are true:

1. the routed provider is Anthropic;
2. the request is not positively identified as genuine Claude Code;
3. a healthy generation can be obtained and the model prompt cache is ready.

Genuine Claude Code detection deliberately requires a complete signature:

- Anthropic Messages ingress;
- `user-agent` beginning `claude-cli/`;
- `x-app: cli`;
- a non-empty `x-claude-code-session-id`;
- first `system` block text beginning `x-anthropic-billing-header:`; and
- if `x-alexandria-harness` is present, every value must identify Claude or
  Claude Code.

Only that complete signature bypasses Dario. A partial or conflicting signature
does not get direct-Claude-Code treatment.

## The three-block system prompt

Dario depends on Claude Code's model-specific wire shape. Alex captures
that shape by launching the real `claude` executable against a temporary local
Messages endpoint with:

```text
claude --model <model> --print -p hi
```

The captured request is expected to contain at least three `system` blocks:

| Index | Meaning in Alex's rewrite |
| --- | --- |
| `system[0]` | Claude billing/header block. It is left in place. |
| `system[1]` | Agent identity block. Replaced with the captured model-specific identity when available. |
| `system[2]` | Main Claude Code system prompt. Replaced with the captured model-specific prompt. |

For `system[2]`, Alex preserves the caller's task-specific suffix if the
original text contains the exact operator preface separator:

```text
\n\n---\n\nIMPORTANT: The operator of this session has supplied the following task-specific instructions.
```

The replacement is therefore `captured system prompt + original suffix`, not a
blind replacement of operator instructions. If the request has fewer than three
blocks, lacks text in block 2, or has no usable cache entry, the preload does not
apply this rewrite.

Prompt caches are per normalized model, stored under
`<data_dir>/dario-prompt-cache/`, and considered fresh for 24 hours. A cold
model is warmed before the client request is allowed through Dario. Concurrent
captures for one model share one in-flight capture.

## Headers and beta flags

There are two distinct header paths:

- Direct genuine Claude Code traffic receives only Alex's explicit
  `CLAUDE_CODE_PASSTHROUGH_HEADERS` allowlist: `accept`, `x-app`, Claude session
  and agent IDs, Stainless runtime metadata, and the dangerous-browser flag.
  Client auth, host/framing headers, Alex metadata, and spoofed Dario
  capture headers are not forwarded. The selected vault credential replaces
  client auth.
- Ordinary Anthropic OAuth traffic gets the required
  `oauth-2025-04-20` beta. If the client supplied `anthropic-beta`, Alex
  preserves it and adds the OAuth beta when absent. Anthropic API-key traffic
  passes the client's beta header through. `anthropic-version` is also
  normalized by the upstream header builder.

On the Dario connection, Alex authenticates to the local generation with
its generated Dario API key and adds `x-dario-capture-id` plus
`x-dario-capture-model`. Those fields let the preload associate Dario's actual
Anthropic fetch with the Alex trace and apply the prompt cache. Dario is
the component that produces the Claude-subscription upstream wire request;
Alex does not copy arbitrary CLI headers into it.

Concretely, the local Dario request follows Alex's non-OAuth Anthropic
header branch: `x-api-key` is the Dario key, a client `anthropic-beta` is passed
when present, and `anthropic-version` is preserved or defaults to `2023-06-01`.
The checked-in Rust does not define Dario npm's exact final CLI beta list; the
capture preload records the resulting Dario-to-Anthropic headers with secrets
redacted. Do not document additional beta flags as an Alex guarantee.

## Generational supervisor

Each generation has its own loopback port, process, version, logs, readiness
state, in-flight count, and probe history. Startup/roll behavior is:

1. resolve/install the selected Dario version;
2. resolve a Node runtime and create a private work directory;
3. start the child on a fresh port with capture/prompt-cache preload settings;
4. wait for `/health`;
5. when subscription validation is enabled, send a real tiny Messages probe;
6. promote the healthy generation, then drain the previous generation.

The supervisor probes periodically, marks a generation unhealthy after the
configured consecutive failure count, rolls replacements, and reaps processes
from a dead daemon. `alex dario restart` rolls the same version; `alex dario
update` checks npm and rolls only when a newer version exists.

## Failure behavior

Non-Claude-Code Anthropic traffic is fail-closed:

- a cold prompt cache is never served through Dario before its warm attempt
  finishes;
- an explicit `DarioPrepare::Unavailable` returns `503 Service Unavailable`;
- startup reports enabled-but-unavailable routing as fail-closed while the
  supervisor attempts on-demand repair;
- if on-demand repair, prompt warming, or the request-generation handoff fails,
  the proxy returns `503 Service Unavailable` instead of leaking the request to
  Anthropic's third-party-app billing path;
- a healthy generation is used only after acquisition of an in-flight guard,
  so a draining generation cannot accept an untracked new request.

Inspect the trace's `via_dario` and `dario_generation` fields to verify the
route used by an eligible request.

## Status and invariants

```bash
alex dario status
alex dario restart
alex dario update
alex ping dario
```

`GET /admin/dario` returns mode, effective route/reason, active generation,
runtime paths/version, known models, prompt-cache health, and every generation's
state. `POST /admin/dario/ping` performs a through-Dario completion, not merely
a listener health check.

The terminal dashboard always defines a Dario tab and renders explicit
`unknown`, `disabled`, or enabled state. The status/Telegram formatter always
emits a `Dario: ready|down` line before account lines. In other words, an
unhealthy status is represented as down rather than by dropping the Dario row.

## State and security

```text
<data_dir>/dario/                 installed package generations and logs
<data_dir>/dario-prompt-cache/    captured system prompts and cache metadata
<data_dir>/bodies/YYYY-MM-DD/     gzip Dario upstream request/response captures
```

Captured fetch headers redact authorization, API keys, cookies, and set-cookie.
Prompt cache files contain proprietary/request context and should still be
treated as sensitive local data. `dario_api_key` is generated into
`config.toml`; do not use it as a client or upstream Anthropic credential.

Next: [Configuration](configuration.md) · [Traces](traces.md) ·
[Providers and routing](providers-and-routing.md)
