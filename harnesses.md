# Harness integration and session tracing

Alexandria has two separate integration jobs:

1. Route a harness through the Alexandria model proxy and tag every request with the harness name.
2. Recover the harness's native session identity, lifecycle, and parent/child agent relationships.

Those jobs should not be conflated. A provider header is reliable request attribution, but is usually static. A lifecycle hook sees rich session metadata, but often cannot mutate the model request. The strongest integration uses both.

## Techniques

### Provider configuration and static headers

Configure the harness's model provider with Alexandria's base URL and a harness-scoped key. Add static request metadata where the provider format allows it:

```text
x-alexandria-harness: codex
x-alexandria-harness-version: 0.144.3
```

This is the simplest and most reliable way to identify the client. It does not distinguish sessions or sub-agents unless the harness supports a dynamic header callback.

### Dynamic provider-header hooks

Pi exposes `before_provider_headers`. Alexandria's Pi extension uses it to add Pi's current session ID only when the selected provider is `alexandria`:

```ts
pi.on("before_provider_headers", (event, ctx) => {
  if (ctx.model.provider !== "alexandria") return;
  event.headers["x-session-id"] = ctx.sessionManager.getSessionId();
});
```

This is ideal: the session identity travels on the same request that is traced. Use this technique whenever a harness exposes a request/header interceptor.

### Command lifecycle hooks

Claude Code, Codex, Kimi, Copilot, Devin, Droid, Qoder CLI, Cursor, and Mastra Code expose lifecycle events that can invoke a script. The event is normally JSON on `stdin`. Herdr installs a small shell or PowerShell adapter, reads fields such as `session_id`, `transcript_path`, `source`, and `agent_id`, then reports them over its local socket.

Useful event classes include:

- Session identity: `SessionStart`, `sessionStart`
- Turn state: `UserPromptSubmit`, `Stop`, `Interrupt`
- Tool state: `PreToolUse`, `PostToolUse`, `PostToolUseFailure`
- Blocking state: `PermissionRequest`, `PermissionResult`, notifications or questions
- Delegation: `SubagentStart`, `SubagentStop`, `SubagentEnd`
- Teardown: `SessionEnd`, `AgentEnd`

Hooks should be best-effort, short-lived, locally authenticated, and idempotent. A failed observability hook must not break an agent turn.

### In-process extensions and plugins

Pi and OMP load TypeScript extensions. OpenCode and Kilo load JavaScript plugins. Hermes loads a Python plugin. These integrations can subscribe directly to richer event APIs, retain session state in memory, debounce noisy events, and distinguish root sessions from child sessions.

Herdr's OpenCode plugin is an especially useful lineage example: `session.created` and `session.updated` expose an `info.parentID`. The plugin records child session IDs and prevents child lifecycle events from overwriting the root pane's state, while still surfacing a child permission/question as blocked.

### Wrapper and inherited environment

A launcher or terminal wrapper can mint a run ID and inject environment variables before starting the harness, for example:

```text
ALEXANDRIA_RUN_ID=<uuid>
ALEXANDRIA_PARENT_RUN_ID=<uuid-or-empty>
ALEXANDRIA_HARNESS=codex
```

This works well for process-level correlation and is how Herdr makes pane/socket identity available to its integrations through variables such as `HERDR_PANE_ID` and `HERDR_SOCKET_PATH`.

Environment inheritance is not sufficient for logical sub-agents that run as threads inside one harness process. All of those agents see the same process environment. It becomes useful again when a harness actually spawns a child process: the child can inherit the run ID and add its own child ID.

Codex's `shell_environment_policy` controls which variables reach shell commands launched by an agent. That can prove that a command belongs to a run, but it does not identify which logical Codex sub-agent requested the command unless another signal supplies the child agent ID.

### Request-body session keys

Some harnesses already put a stable conversation identifier in the request body. Codex Responses requests currently include `prompt_cache_key`; Alexandria extracts it after explicit session headers and metadata. This can group model calls without a request-header hook.

Treat body-derived keys as a discovered capability, not a permanent contract. Validate them against lifecycle-hook session IDs for each supported harness version.

### Transcript or session-file paths

Several hooks expose `transcript_path`, and Pi/OMP can expose the session file. A canonical path is a useful fallback identity and can be hashed before storage. Paths must not be sent upstream and should be treated as sensitive local metadata.

### Local side channel

When hooks cannot change the model request, send lifecycle records to Alexandria over a local authenticated HTTP or Unix-domain-socket endpoint. A record can contain:

```json
{
  "harness": "codex",
  "event": "SubagentStart",
  "session_id": "parent-session",
  "agent_id": "child-agent",
  "agent_type": "explorer",
  "turn_id": "turn-id",
  "cwd": "/workspace"
}
```

The daemon can then join that side-channel record to request traces using a native request session key, timestamp window, run ID, or an explicit dynamic header when available.

## What Herdr uses by harness

This table summarizes the integrations in `repos/herdr/src/integration` and the reusable technique, rather than promising API compatibility with every future harness version.

| Harness | Herdr integration surface | Identity/lifecycle approach | Notable detail |
| --- | --- | --- | --- |
| Pi | TypeScript extension | Session manager ID/path plus rich extension events | Debounces idle/retry state and releases only on a real quit |
| OMP | TypeScript extension | Same extension pattern as Pi | Avoids treating reload/new/resume/fork as process exit |
| Claude Code | Command hook in `settings.json` | `SessionStart` JSON supplies session ID and transcript path | Ignores sub-agent payloads so they do not replace root identity |
| Codex | Command hook in `hooks.json` plus `features.hooks` | `SessionStart` supplies session ID and start source | Herdr uses the hook for identity; process-level observation owns broader lifecycle state |
| Kimi Code | Inline TOML hook configuration | Broad session, tool, permission, stop, interrupt, and sub-agent events | A full command-hook lifecycle integration |
| GitHub Copilot CLI | Direct command hook entries | `SessionStart` identity | Removes obsolete lifecycle entries during upgrade |
| Devin CLI | Command hooks in `config.json` | Multiple events report the active session | Uses the same session-report action for all registered events |
| Factory Droid | Command hook in `settings.json` | `SessionStart` identity | Cleans legacy `hooks.json` registrations |
| Qoder CLI | Command hook in `settings.json` | `SessionStart` identity | Shell/PowerShell adapters parse JSON from `stdin` |
| Cursor | Simple command hook | `sessionStart` with several possible session/conversation field spellings | Handles camelCase and snake_case payload variants |
| Mastra Code | Flat command-hook list | Session, tool, permission, sub-agent, interrupt, agent-end, and session-end events | Maps events to working/blocked/idle/release actions |
| OpenCode | JavaScript plugin | Session/event API and `parentID` child detection | Explicitly filters child sessions from root state |
| Kilo | JavaScript plugin | Session/event API | Similar to OpenCode, with less child-specific handling |
| Hermes | Python plugin | Registered pre/post LLM, tool, approval, and session hooks | Direct in-process lifecycle callbacks |

Herdr has no integration assets for Gemini CLI or Grok CLI in this checkout. For those harnesses, Alexandria should start with provider/base-URL configuration, static harness headers if supported, body-derived session discovery, and an optional wrapper run ID. Add a native plugin or hook only when the harness exposes a documented lifecycle API.

## Alexandria's current integrations

### Pi

`Harnesses → Pi → Connect` adds models named `alex/*` and installs a small
session hook. The hook sets a local session header that the Alexandria proxy
uses for tracing, then the proxy removes the header before forwarding the
request upstream. The connection also installs a harness-scoped key and a
static `x-alexandria-harness: pi` header.

### Codex

`Harnesses → Codex → Connect` installs:

- A copy of the original `~/.codex/config.toml` at
  `~/.codex/alexandria-original-config.toml`, plus restorable copies of any
  pre-existing `openai` and `alex` profiles in Alexandria's managed state
- `~/.codex/openai.config.toml`, used by `codex --profile openai`, with the
  native Codex model catalog and normal Codex authentication
- `~/.codex/alex.config.toml`, used by `codex --profile alex`, with the
  Alexandria model catalog, local proxy, and `alex/*` model names
- An `alexandria` Responses API provider in `~/.codex/config.toml`
- Separate native and merged model catalogs for the fixed profiles
- A 0600 harness credential read through Codex command-backed authentication
- Static Codex harness/version headers
- `SessionStart`, `SubagentStart`, and `SubagentStop` hooks in `~/.codex/hooks.json`

Plain `codex` follows the default-route toggle in the menu-bar app. Turning
“Use Alexandria by Default” off gives plain `codex` the same native route as
`--profile openai`; turning it on gives plain `codex` the proxied route used by
`--profile alex`. The two explicit profiles remain available regardless of the
toggle.

Codex hooks cannot mutate HTTP request headers. Alexandria therefore groups current Codex requests using `prompt_cache_key` and records hook events separately in `~/.codex/alexandria-session-events.jsonl` while the parent/child join is being validated. Codex requires the user to review and trust a newly installed command hook.

For example, Codex selects `alex/gpt-5.6-sol`, the proxy records that requested
ID, strips the Alexandria namespace, and routes `gpt-5.6-sol` to the inferred
upstream provider. The namespaced default is restored to the user's original
model when the Codex harness is disconnected.

Disconnect removes only Alexandria-managed provider, catalog, credential, and
hook entries. It restores the exact pre-connect top-level selection and any
pre-existing `openai` or `alex` profile files. The readable original-config
backup and captured event log are preserved for manual recovery and debugging.

## Sub-agent tracing experiments

Run these in order; each experiment answers a different question.

1. **Native Codex join.** Capture `SessionStart.session_id`, `SubagentStart.session_id`, and `SubagentStart.agent_id`. Compare them with each request's `prompt_cache_key`. The ideal result is root requests keyed by `session_id` and child requests keyed by `agent_id`; that produces an exact parent/child edge.
2. **Run-level environment marker.** Start Codex through a wrapper with `ALEXANDRIA_RUN_ID`. Allow it through `shell_environment_policy`, ask root and child agents to print it, and verify both inherit the same run. This proves shared ancestry, not child identity.
3. **Custom-agent config marker.** Give each Codex custom agent a distinct static provider header or model/provider layer if Codex preserves it for spawned sessions. This may identify agent type, but not an individual child instance.
4. **Hook-provided developer context.** Return a child correlation token from `SubagentStart.additionalContext` and instruct the child to include it in supported tool or MCP calls. This is useful for experiments but is not trustworthy request attribution because it depends on model compliance.
5. **MCP side channel.** Expose an Alexandria MCP tool such as `begin_child_span(parent, child)` and instruct custom agents to call it at startup. This gives rich spans but is also model-mediated unless paired with lifecycle hooks.
6. **Process wrapper.** If a future Codex mode launches sub-agents as OS child processes, wrap the executable, inherit `ALEXANDRIA_RUN_ID`, mint `ALEXANDRIA_AGENT_ID` per child, and inject it into provider headers through an environment-backed header. This does not apply to today's logical in-process threads.

The preferred durable design is native lifecycle hooks for the graph, request headers/body fields for model-call attribution, and a wrapper environment only for run/process ancestry.

## Safety and compatibility rules

- Never place a long-lived Alexandria local key directly in a world-readable config or command line.
- Scope generated hooks to Alexandria's provider where a request callback permits it.
- Preserve unrelated providers, hooks, comments, and project settings.
- Store enough managed state to make disconnect reversible.
- Do not let child events overwrite root-session identity.
- Treat hook payloads, transcript paths, prompts, and working directories as sensitive local data.
- Keep hook timeouts short and failures non-fatal.
- Version generated files so upgrades can replace only Alexandria-owned content.
