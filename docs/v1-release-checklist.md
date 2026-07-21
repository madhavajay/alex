# Alex V1 release checklist

Updated: 2026-07-21

This is the operational companion to the V1 adoption roadmap. A checked item
must have evidence from the named automated gate or a recorded manual result.
The final release is cut only when every required item is checked.

## Beta cadence

Each local beta checkpoint follows the same order:

1. Merge one coherent feature batch into `v1/integration`.
2. Run focused tests, then the full Rust and Swift suites.
3. Require Linux, Windows, macOS, site, bundle, and secret-scanning CI gates.
4. Stamp the next `0.1.29-beta.N` version in an isolated build tree.
5. Build and install the CLI, daemon, and macOS app locally.
6. Publish a short manual test card containing only behavior that automation
   cannot prove.
7. Record pass/fail evidence here before starting the next checkpoint.

## Public walkthrough and measurement

- [x] Three deterministic walkthroughs have pause, play, step, reset, rule
  inspection, and concrete outbound actions (`site` tests).
- [x] The static fallback, reduced-motion behavior, mobile contract, and
  privacy-safe event allowlist have automated coverage (`npm test`).
- [x] GitHub Pages has a reproducible build and deployment workflow.
- [ ] Visually verify all three walkthroughs on desktop and mobile.
- [ ] Visually verify the no-JavaScript and reduced-motion experiences.
- [ ] Merge to the deployment branch and verify the live GitHub Pages URL and
  outbound campaign parameters.

## LAR-backed Trace Browser

- [x] Trace summaries and session turns use bounded, stable cursor pages.
- [x] Summary and page requests read zero bodies; expanding one turn reads only
  that turn and its trace-linked tool bodies (125-turn regression).
- [x] Existing gzip bodies remain readable while resumable atomic migration
  moves validated pointers to LAR.
- [x] Interrupted append recovery and bounded body reads have regression tests.
- [ ] Pass the reproducible 55,000-trace/~9.4-GB benchmark and publish memory,
  initial-render, search, and individual-turn budgets.
- [ ] Export, sanitize, reopen, and replay the release fixture end to end.
- [ ] Manually open and search a real archive at current production scale.

## Shared platform path

- [x] The web UI covers onboarding, provider health, middleware, and traces.
- [x] Deterministic proxy tests cover streaming, tool calls, middleware,
  persistence, and restart recovery.
- [x] Windows Task Scheduler service support is implemented and is a required
  CI gate.
- [ ] Linux Rust/service/web CI is green on the release candidate.
- [ ] Windows Rust/service/web CI is green on the release candidate.
- [ ] macOS Rust, Swift, and app-bundle CI are green on the release candidate.
- [ ] Run clean-machine install/start/connect/route/trace/restart smoke tests on
  macOS, Ubuntu x86-64, and Windows 11 x86-64.

## Fable to Sol middleware preset

- [x] The built-in preset, readable rule source, dry run, editing, disabling,
  replay, trace attempts, explanation, and session lease are implemented.
- [x] The public walkthrough is checked against the shared scenario fixture.
- [ ] Verify explicit fixture cases for overload, recovery, non-match, and no
  healthy fallback account in the combined release branch.
- [ ] Manually run the preset through the installed beta and inspect the trace,
  provenance, explanation, and lease expiry.

## CLIProxyAPI

- [x] Alex to CLIProxyAPI onboarding probes the URL/credential, filters models,
  sends a test request, and opens the resulting trace.
- [x] CLIProxyAPI to Alex export uses a scoped key, private config fragment,
  capability negotiation, correlation headers, and loop rejection.
- [x] Deterministic Chat, Responses, Anthropic, streaming tool-call, and
  structured-error tests cover both arrangements.
- [ ] Run a pinned real CLIProxyAPI Docker/binary version matrix, including an
  end-to-end streaming tool call in both arrangements.
- [ ] Manually test both arrangements through the installed beta and confirm
  model names, usage, errors, and trace correlation are not double-prefixed.
- [ ] Track the documented CLIProxyAPI v7 limitation: non-2xx status and JSON
  survive the second hop, but upstream error headers do not.

## Stable activation baseline

- [x] `alex connect pi` is used in Alex and PAM documentation.
- [x] `alex doctor` checks executables, credentials, Dario, ports, permissions,
  service state, storage, and provider health without printing secrets.
- [x] OpenAI-compatible model IDs use the `alex/*` namespace.
- [ ] Complete the user-visible Alexandria-to-Alex naming and stale capability
  claim audit.
- [ ] Verify reset returns to onboarding and a provider-less menu offers
  **Start Onboarding**.
- [ ] Verify onboarding can move backward/forward and freely change harness,
  provider, existing account, or new account without stale test state.
- [ ] Verify non-Claude-Code Anthropic traffic always uses Dario, while Claude
  Code may route directly, including Pi to `alex/claude-opus-4-8`.
- [ ] Verify fresh installs create missing daemon/harness configuration files
  instead of blocking onboarding.
- [ ] Build, install, and record the final `0.1.29-beta.N` candidate.
- [ ] Complete the full clean-user launch story on every supported platform.
- [ ] Stamp `0.1.29`, generate signed/notarized release assets, publish stable
  update metadata, and verify upgrade/rollback behavior.

## Current checkpoint

- Candidate: `0.1.29-beta.12` (planned; not yet cut)
- Branch: `v1/integration`
- Draft PR: <https://github.com/madhavajay/alex/pull/26>
- Local browser visual automation was unavailable in the current environment;
  visual checks remain intentionally open rather than inferred from markup.
