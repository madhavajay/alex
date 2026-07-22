# Handoff: Pi harness does not reroute on real streamed Fable refusals

Repo: `/Users/madhavajay/dev/alex/v1-integration` (branch `v1/integration`, uncommitted regex-first middleware work on top of `864d05d`). Daemon/app build: `0.1.29-beta.21`, installed and running (daemon pid started 15:19:16, continuous).

## Symptom

The built-in middleware rule `alex.fable-5-to-gpt-5.6-sol` reroutes a Fable-5 refusal to `gpt-5.6-sol` correctly in the **claude** harness, but **not** in the **pi** harness. In Pi the refusal is delivered straight to the user ("Error: The model refused…") with no reroute, no session pin, and no model-switch notice.

Rule (active `~/.alex/middleware/rules.toml`, `settings.enabled=true`, rule `enabled=true`):
- `model_regex = ["^claude-fable-5$"]`, `provider_regex = ["^anthropic$"]`, `status_regex = ["^200$"]`
- `body_regex = ['(?m)^event:\s*message_delta\r?$\ndata:\s*\{[^\r\n]*"delta"\s*:\s*\{[^\r\n]*"stop_reason"\s*:\s*"refusal"']`
- `then.reroute` → model `gpt-5.6-sol`, providers `["openai"]`, scope `session`, ttl 86400, effort `max`, `required_capabilities.portable_history=true`; match `stable_session=true`.

## Reproductions (trace store `~/.alex/alex.sqlite3`, table `traces`)

**Pi — FAILS (real streamed refusal, no reroute):**
- Session `019f8846-d5c1-7525-b843-8521c74a5edc`, refusal trace **`0882167a`** @ 15:23:18 — category `bio`.
- Session `019f8857-f807-7d90-8b0e-e4df4a71a4c4`, refusal trace **`12922b41`** @ 15:41:51 — category `cyber`. (Rule armed since 15:22:40, ~19 min earlier — timing ruled out.)
- Both: `upstream_provider=anthropic`, `routed_model=claude-fable-5`, `status=200`, `streamed=1`, `substituted=0`, `error_kind` empty, `fixture_name=null`, `injected=false`, `attempts=[{… middleware_decisions:[] …}]` (single attempt, engine recorded **no** decision).
- Response body **contains the exact refusal SSE the rule targets**, e.g. `12922b41`:
  ```
  event: message_delta
  data: {"type":"message_delta","delta":{"stop_reason":"refusal","stop_details":{"type":"refusal","category":"cyber","explanation":"This request triggered restrictions on violative cyber content…","fallback_credit_token":"…"}},"usage":{…}}
  ```

**Claude — WORKS (real streamed refusal, rerouted):**
- Session `cf835cdd-e2f8-4493-9411-b6fe3d3a6418`. Greeting turn `ce345e97` @ 15:23:40 (normal). Refusal turn @ 15:23:57 was **intercepted and folded into the reroute** — no standalone anthropic refusal row; reroute traces `053deab4` + `e13ce819` (`upstream_provider=openai`, `routed_model=gpt-5.6-sol`, `substituted=1`). Session-pin lease written `~/.alex/middleware/leases.json` @ 15:24:09.

**Earlier "successes" were fixtures, not real traffic:** the pre-15:23 Claude refusal catches (`515ab5ef`, `ab64c43f` @ 13:14) were the injected fixture `anthropic-fable-refusal-200` (`injected=true`, `streamed=0`). The 15:23 Claude session is the first *real streamed* catch.

## Request-level difference between the two harnesses

Pi request headers (from `traces.req_headers_json` of `0882167a`) include:
- `anthropic-beta: interleaved-thinking-2025-05-14`
- `x-stainless-*` (node client), `x-pi-turn-id: 0`, `x-alex-harness: pi`, `x-alex-harness-version: 0.81.1`
- **No** `x-alex-no-substitute`. Both harnesses send `"stream": true`.

Claude does not send the interleaved-thinking beta header.

## Ruled out (with evidence)

1. **Regex / "different response":** No. Both Pi and Claude refusal bodies contain the identical `"delta":{"stop_reason":"refusal"` SSE. The regex matches Pi's body. Verified by decompressing both `resp_body_path` gzips.
2. **Rule not armed / hot-reload delay:** No. Wizard saves go through the admin API → `MiddlewareRuntime::apply_stored` (`crates/alex-proxy/src/middleware.rs` ~365), which under one write-lock BOTH `persist_stored` (writes `rules.toml`) AND swaps `self.rules`/`self.compiled`. Arming is synchronous with the file write; there is no watcher/poll. `rules.toml` mtime 15:22:40 = arm time; both Pi refusals are after it. (Only hand-editing `rules.toml` on disk needs `/admin/middleware/reload` or restart — the wizard doesn't do that.)
3. **Model-name mismatch:** No. `canonical_model_alias` (`crates/alex-proxy/src/lib.rs`) strips the `alex/` prefix, so `alex/claude-fable-5` → `claude-fable-5`. Both harnesses' `routed_model` is `claude-fable-5`.
4. **`x-alex-no-substitute` / `substitution_disabled`:** No. Header absent on Pi; `no_substitute(&headers)` (`lib.rs` ~12801, checks `x-alex-no-substitute == "1"`) is false for both.

## Diagnosis / where it is

On a **streamed** response, Alex does **not** evaluate the rule's `body_regex`. Streamed Fable refusals are caught by a bespoke interceptor, gated by `should_inspect_refusal` (`crates/alex-proxy/src/lib.rs` ~13425):

```rust
let should_inspect_refusal = resp.status().is_success()
    && current_provider == Provider::Anthropic
    && canonical_model_alias(&current_model) == "claude-fable-5"
    && !substitution_disabled
    && runtime.fable_refusal_interception_enabled();   // middleware.rs:245
```

If true, `inspect_anthropic_refusal_prefix` (`lib.rs` ~14686) holds the SSE prefix, and `SseErrorObserver` (`lib.rs` ~14931) sets `anthropic_refusal` via `anthropic_refusal_from_value` (~14630) or trips `response_committed` via `anthropic_event_commits_response` (~14648). On refusal it reroutes; otherwise the held prefix is replayed and streaming resumes. The generic `body_regex` engine only runs on the **buffered/non-streamed** path (`observe_buffered_response`, ~14370).

Because Pi's refusal finalized as a **standalone 200 trace with the full body and `middleware_decisions:[]`**, `successful_refusal` was `None` — i.e. for the Pi request the interceptor **either never ran or never detected**, and the request fell through to the normal streaming-forward-and-finalize path. Every readable clause of `should_inspect_refusal` evaluates the same for Pi as for Claude, yet the outcome differs — so the divergence is harness-specific and not explained by the trace fields.

## Candidate root causes to investigate (ranked), with confirmation steps

1. **Pi takes a different upstream-forwarding branch that lacks the `should_inspect_refusal` block.** Verify the `should_inspect_refusal` interceptor is on the ONLY streaming-forward path. Search every place that forwards/streams an upstream `reqwest::Response` to the client (buffered vs streamed, dario, cliproxyapi, translation paths). The `interleaved-thinking` beta header and/or the stainless client may steer Pi through a translate/forward path that never reaches ~13425. **Confirm:** add a `tracing::info!` at the `should_inspect_refusal` computation logging harness, `current_model`, provider, and each clause; reproduce in Pi.
2. **Post-detection reroute skipped by a capability/affinity gate.** Even if detected, the reroute requires `required_capabilities.portable_history=true` and a stable session; check how portable-history / `stable_session` is derived per request and whether a Pi (stainless/interleaved-thinking) session qualifies differently than Claude Code. Also `retry_failover_allowed` (`lib.rs` ~11929) — for anthropic provider it returns true, so not the blocker, but the `same_route_plan`/`middleware_attempt_context` path (~13536+) should be traced. **Confirm:** if interception logs show refusal detected but no reroute, this is it.
3. **`response_committed()` trips before the refusal for interleaved-thinking streams.** The stored Pi bodies show no content_block/thinking events before the refusal, so this is unlikely, but confirm the *live* stream ordering (thinking `content_block_start`/`thinking_delta` should NOT commit per `anthropic_event_commits_response`, but verify).

`fable_refusal_interception_enabled()` depends only on the rule set (any enabled rule with non-empty `body_regex` or the fable error-kind), not per-request state — ruled out as the differentiator.

## Fastest confirmation
Reproduce in Pi with a bio/cyber prompt (already reproducible on demand) while tailing daemon logs with the added `should_inspect_refusal` instrumentation. That single log line disambiguates "gate false" (candidate 1) from "detected but reroute skipped" (candidate 2).

## Note on the design question (2 body regexes?)
Not the lever. One tolerant regex (`"stop_reason"\s*:\s*"refusal"`) matches the pattern in both SSE and plain-JSON bodies; the current builtin is over-specific to SSE and would MISS a real non-streamed JSON refusal (a separate, latent gap). But the streamed interceptor doesn't use `body_regex` at all, so no regex change fixes the Pi path.
