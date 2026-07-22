# Alex V1 release checklist

Updated: 2026-07-21

This is the operational companion to the V1 adoption roadmap. A checked item
must have evidence from the named automated gate or a recorded manual result.
The final release is cut only when every required item is checked.

## Beta cadence

Each local beta checkpoint follows the same order:

1. Merge one coherent feature batch into `v1/integration`.
2. Run focused tests, then the full Rust and Swift suites.
3. Require Linux, macOS, site, bundle, and secret-scanning CI gates.
4. Stamp the next `0.1.29-beta.N` version in an isolated build tree.
5. Build and install the CLI, daemon, and macOS app locally.
6. Publish a short manual test card containing only behavior that automation
   cannot prove.
7. Record pass/fail evidence here before starting the next checkpoint.

On macOS the repeatable local checkpoint command is:

```bash
./scripts/install-local-beta.sh 0.1.29-beta.N --ref v1/integration
```

It stamps and builds in a disposable Git worktree, installs the CLI/daemon and
app with the same version, verifies both installed versions, and never creates
a tag or changes the source worktree.

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
- [x] Reproducible 55,000-trace/9.4-GB benchmark passes with 65.1 MB peak
  RSS, 14.747 ms trace-summary p95, 1.943 ms filtered-search p95, 0.864 ms
  random-body-read p95, and 4.022 ms one-turn-open p95
  (`docs/benchmarks/lar-v1-full-macos-m2-max.json`).
- [x] Export, sanitize, reopen, and replay the release fixture end to end
  (`alex-lar-scale fixture`; structural redaction and archive verification).
- [ ] Manually open and search a real archive at current production scale.

## Shared platform path

- [x] The web UI covers onboarding, provider health, middleware, and traces.
- [x] Deterministic proxy tests cover streaming, tool calls, middleware,
  persistence, and restart recovery.
- Windows support is deferred and is not part of the `0.1.29` release gate.
- [x] Linux Rust/service/web CI is green on the release candidate (PR run
  `29800336043`).
- [x] macOS Rust, Swift, and app-bundle CI are green on the release candidate
  (PR run `29800336043`).
- [x] macOS packaged clean-machine smoke installs the app and CLI, manages the
  real launchd service, routes through a loopback provider, persists trace
  `ab299c6d-fee4-4f48-b332-d85ee6a76960`, replaces daemon PID `1692` with
  `1921`, and reads the same trace after restart (PR run `29796666004`).
- [x] Ubuntu x86-64 packaged clean-machine smoke downloads and verifies the
  release-format archive, installs both binaries, manages the real non-root
  `systemd --user` service, routes through loopback Exo, persists trace
  `64044200-49f9-48c4-b118-e95e018631f3`, replaces daemon PID `206` with
  `290`, reads the same trace/body after restart, and removes the isolated
  service/container (PR run `29798681461`).

## Fable to Sol middleware preset

- [x] The built-in preset, readable rule source, dry run, editing, disabling,
  replay, trace attempts, explanation, and session lease are implemented.
- [x] The public walkthrough is checked against the shared scenario fixture.
- [x] Explicit combined-branch fixture cases cover overload, lease-expiry
  recovery, non-match, and no healthy fallback account (`cargo test -p
  alex-proxy fable_`).
- [ ] Manually run the preset through the installed beta and inspect the trace,
  provenance, explanation, and lease expiry.

## CLIProxyAPI

- [x] Alex to CLIProxyAPI onboarding probes the URL/credential, filters models,
  sends a test request, and opens the resulting trace.
- [x] CLIProxyAPI to Alex export uses a scoped key, private config fragment,
  capability negotiation, correlation headers, and loop rejection.
- [x] Deterministic Chat, Responses, Anthropic, streaming tool-call, and
  structured-error tests cover both arrangements.
- [x] Pinned CLIProxyAPI v7.2.92 Docker matrix passes both arrangements,
  including OpenAI Chat/Responses, Anthropic Messages, streaming tool calls,
  structured failures, authentication, correlation, and loop rejection
  (`./test.sh cliproxyapi --only CLIPROXYAPI`).
- [ ] Manually test both arrangements through the installed beta and confirm
  model names, usage, errors, and trace correlation are not double-prefixed.
- [x] Track the documented CLIProxyAPI v7 limitation: non-2xx status and JSON
  survive the second hop, but upstream error headers do not.

## Second-model / oracle path

- [x] The bundled, opt-in Pam package exposes a mode-specific `pam_oracle`
  tool that uses configured `alex/*` models without changing the primary agent
  model; the public walkthrough uses its deterministic oracle-lineage vector.
- [ ] Through the installed beta, invoke Pam with distinct agent and oracle
  models backed by two connected subscriptions, then inspect both answers and
  their session lineage in the Trace Browser.

## Stable activation baseline

- [x] `alex connect pi` is used in Alex and PAM documentation.
- [x] `alex doctor` checks executables, credentials, Dario, ports, permissions,
  service state, storage, and provider health without printing secrets.
- [x] OpenAI-compatible model IDs use the `alex/*` namespace.
- [x] Complete the Alex naming and stale capability
  claim audit (public copy, onboarding, package descriptions, model metadata,
  Dario/Amp wording, and the bundled capability-map artwork).
- [ ] Verify reset returns to onboarding and a provider-less menu offers
  **Start Onboarding**.
- [ ] Verify onboarding can move backward/forward and freely change harness,
  provider, existing account, or new account without stale test state.
- [x] Non-Claude-Code Anthropic traffic is forced through Dario while genuine
  Claude Code may route directly (routing regression tests plus live beta.12
  Pi → `alex/claude-opus-4-8` trace
  `e780d4cc-884a-4473-a4eb-9a678f0f1691`, `via_dario: true`).
- [x] Fresh daemon and harness configuration paths are created with private
  permissions instead of blocking onboarding (Rust/Swift first-run regression
  tests plus the clean Ubuntu package install in PR run `29800336043`).
- [ ] Build, install, and record the final `0.1.29-beta.N` candidate.
- [ ] Complete the full clean-user launch story on macOS and Ubuntu Linux.
- [ ] Stamp `0.1.29`, generate signed/notarized release assets, publish stable
  update metadata, and verify a clean installation on macOS and Ubuntu Linux.

## Current checkpoint

- Installed checkpoint: `0.1.29-beta.14` at `9cef51b` (CLI, Alex.app,
  launchd service path, running app, daemon health, Dario, and
  `/v1/models` owner metadata verified; this is not yet the final release
  candidate).
- Branch: `v1/integration`
- Draft PR: <https://github.com/madhavajay/alex/pull/26>
- Combined local gates: Rust workspace and all targets pass; Swift passes 306
  Swift Testing + 14 XCTest cases; site passes 10 tests and deterministic build;
  pinned CLIProxyAPI Docker matrix passes both directions.
- Local browser visual automation was unavailable in the current environment;
  visual checks remain intentionally open rather than inferred from markup.
