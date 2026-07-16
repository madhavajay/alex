# Alexandria — current session handoff

Updated: 2026-07-13 AEST  
Workspace: `/Users/madhavajay/dev/alexandria`

This file supersedes the stale 2026-07-09/10 machine-state and next-step claims that were
previously in `fri.md`. It distinguishes released work, the installed local build, changes that
exist only in a dirty integration worktree, and work that is still backlog.

---

## 0. Executive status

- `origin/main` is `b26c98e` / tagged release `v0.1.23`. The major Dario, full-model-summary,
  and multi/bonded-Codex routing slice is released there.
- Codex quota-window refreshing is committed as `d915b73` (`fix: refresh Codex quota windows`),
  pushed to `origin/agent/codex-usage-refresh`, and represented by draft PR #2:
  <https://github.com/madhavajay/alex/pull/2>. That exact revision was built and installed
  locally under version `0.1.23`; it is not yet part of tagged `origin/main`.
- The requested Grok, combined bonded-Codex panel, exact-account re-auth alert, Harness/Pi,
  launchd PATH, and Pi session-hook follow-ups exist in `/private/tmp/alex-subscription-ui`.
  They are uncommitted, unpushed, and not installed as the app/daemon.
- The follow-up worktree is **not build-ready**. A rejected launchd replacement implementation
  is half-removed: helper code and tests remain, but its hidden CLI command/dispatch and
  `OsString` import do not. `alex` currently fails to compile. Do not commit, package, or install
  this worktree until that is resolved and the full suites are rerun.
- The Pi TypeScript session hook is already installed live and verified independently of the
  pending app/daemon build. New Pi processes now send a unique `x-session-id` per Pi session.
- `fri.md` is **not fully implemented**. OpenRouter and several older backlog items remain open.

## 1. Guardrails

- The root `main` checkout is user-owned and heavily dirty: HEAD `3401fb4`, 19 commits behind
  `origin/main`, with 47 status entries spanning AWP/wrap, Amp, GPT-5.6, UI, docs, and other work.
  Do not clean, stash, reset, rebase, commit, or absorb it.
- The Pi hook originally existed in the root checkout's uncommitted work. It was intentionally
  copied into the isolated follow-up worktree for this requested integration; the root edits were
  preserved.
- Continue follow-up work only in `/private/tmp/alex-subscription-ui` unless a new clean worktree
  is deliberately created.
- Do not bump, tag, notarize, release, or claim the pending follow-up is installed before the
  launchd blocker is fixed and tests/build/live checks pass.
- Local app builds use the current production bundle ID so only one menu-bar
  identity runs.
- The collaboration runtime does not expose a model/tier selector, so delegated agents cannot be
  selected or verified as “Codex Terra High.” Keep delegated tasks bounded and review their work.

## 2. Current machine state

### Installed app and daemon

- Historical snapshot: a pre-rename menu-bar build, version `0.1.23`, was
  ad-hoc signed and running as PID `12044` at the last check.
- App executable SHA-256:
  `00ca17ee010d0e9da870d4e4588af2d7f97101a34e1999d00c8459dff976cae5`.
- Daemon/CLI: `~/.local/bin/alex`, version `0.1.23`, launchd PID `12043` at the last check.
- Daemon SHA-256:
  `15f5d2d033a399a2330b0bc1de2c7709bd710027aca1e10927ac88c6d041a763`.
- Live `/health` check on 2026-07-13 returned `status=ok`, `version=0.1.23`, `dario=true`,
  `in_flight=0`.
- LaunchAgent: `~/Library/LaunchAgents/com.alexandria.daemon.plist`.
- The loaded launchd job still reports its default sparse PATH
  `/usr/bin:/bin:/usr/sbin:/sbin`. The pending daemon code independently searches known user,
  NVM, Homebrew, and system binary directories, so Pi detection does not have to depend solely
  on launchd PATH once that code is installed.

### Pi connection and session hook

- Pi version: `0.80.6`.
- Provider config: `~/.pi/agent/models.json`; Alexandria was refreshed with 34 models and a new
  harness run key.
- Session extension: `~/.pi/agent/extensions/alexandria-session.ts`.
- The extension handles Pi's `before_provider_headers` event, affects only the `alexandria`
  provider, and writes Pi's real session ID to `x-session-id`.
- Live verification used two clean temporary Pi sessions with the same prompt. Both returned the
  expected answer, and Alexandria recorded two distinct session IDs and matching redacted request
  headers. This directly verifies the crossover/isolation fix.
- Restart any Pi process that was already open before the extension was installed; Pi discovers
  extensions at startup.
- Correct refresh command: `alex connect pi`. The older text `alex harness connect pi` is wrong.
- This is a loose Pi extension, not a package registered in Pi settings. Therefore `pi list` can
  truthfully say “No packages installed” while the Alexandria hook is installed and active.
  General Pi packages would live under `~/.pi/agent/npm/node_modules` or
  `~/.pi/agent/git`; project-local packages use `<project>/.pi/...`.

## 3. Released or committed work

### Released in v0.1.23 / origin main

- Dario routing distinguishes genuine Claude Code, routes it directly, routes eligible non-Claude
  Anthropic traffic through Dario, fails closed when Dario is configured but unavailable, and uses
  a private Dario workspace.
- Full installed/removed model lists render in the harness result UI.
- Multi-Codex account identity and device login, per-account captured limits, proxy eligibility,
  reset-first/priority/round-robin strategies, reserve floors, N-account retry, sticky affinity,
  actual trace account attribution, account analytics, Subscriptions add/re-auth/pause/remove, and
  Billing Account trace filtering are shipped.
- `install-release.sh` and the macOS PR artifact installation bundle exist.

### Committed and locally installed, not in tagged main

`d915b73` adds safe per-workspace Codex allowance refresh:

- usage-only endpoint; no model prompt;
- five-minute due/age/reset refresh cadence;
- exact ChatGPT workspace validation;
- bounded token refresh without native credential re-import fallback;
- shared per-account mutation locking and bounded HTTP timeouts;
- expired quota windows are not presented as current while refresh is pending.

The quota patch passed 303 Rust test executions and 120 Swift tests before it was committed,
pushed, rebuilt from the clean commit, and installed.

## 4. Pending follow-up integration worktree

Worktree: `/private/tmp/alex-subscription-ui`  
Branch: `agent/subscription-ui-followups`  
HEAD/base: `d915b73`, directly on top of `b26c98e` / `v0.1.23`  
Git state: 12 modified tracked files plus 2 untracked Swift test files; no follow-up commit/upstream.

### Grok subscription truthfulness

- If an xAI OAuth subscription exists, Grok web billing owns the xAI limits slot even when auth or
  billing fails. The UI no longer falls through to historical generic `120 requests / 5,000,000
  tokens` trace headers.
- Generic captured xAI API limits are shown only when an actual xAI API-key account exists.
- Credential failures show an explicit re-authentication error with no quota windows/counts.
- gRPC unauthenticated/permission-denied statuses are classified as credential failures; capacity,
  internal, parse, and service errors are classified as temporary billing unavailability.
- A failed fetch replaces stale displayed limits with an error. The cache is keyed to a token
  fingerprint so a successful in-app re-auth immediately clears a long auth cooldown and retries.

### Combined bonded-Codex menu panel

- More than one OpenAI OAuth account renders in one `Codex · N bonded` panel.
- The panel shows `Priority · PRI`, `Reset first · RF`, or `Round robin · RR`.
- Stable `A1/P1`, `A2/P2`, ... aliases show configured order and the effective order/cycle.
- Each OAuth row retains its own email, state, plan/error, 5h/7d windows, and remaining-quota bars.
- API-key OpenAI routes are filtered before aliases are assigned, so an invisible API-key account
  cannot consume `A1/P1` or appear in the effective order.
- Single-account Codex rendering remains unchanged.

### Alerts and exact account attribution

- Credential/status/expired alerts carry exact-account re-auth remediation.
- Duplicate credential-like heartbeat failures are suppressed when the same account already has a
  token/status alert; non-credential health failures remain visible.
- Re-auth rows are clickable and invoke the selected provider/account rather than `default`.
- Health checks capture the account the proxy actually routed, including failover, and
  `/admin/health` attaches a heartbeat only to that exact `account_id`.
- Swift decodes heartbeat `account_id` and refuses to create remediation for missing/mismatched
  attribution, preventing one failed Codex account from targeting healthy siblings.

### Harness/Pi follow-ups

- A connected Pi remains visible in the menu even when the daemon cannot currently see its binary;
  the submenu explains `Binary not visible to daemon`.
- Preferences prioritize a connected harness as green.
- Binary discovery combines inherited PATH, known user tool directories, all installed NVM Node
  bins, Homebrew, and standard system directories. Explicit binary overrides stay authoritative.
- Generated launchd plist PATH is deduplicated, absolute-only, and XML-escaped.
- `alex connect pi` installs/removes the scoped `alexandria-session.ts` extension and returns its
  path. The macOS result view decodes and displays that path as `Session hook`.

### Explicit dirty files

- Rust/service: `crates/alex-proxy/src/lib.rs`,
  `crates/alex/config/launchd/com.alexandria.daemon.plist`,
  `crates/alex/src/harness_connect.rs`, `crates/alex/src/main.rs`.
- macOS app: `HarnessRefreshResultView.swift`, `MenuCardView.swift`, `PreferencesView.swift`,
  `StatusItemController.swift`.
- macOS core/tests: `HarnessModels.swift`, `Models.swift`, `SnapshotStore.swift`,
  `ModelDecodingTests.swift`, plus new `AlertPolicyTests.swift` and `HarnessMenuTests.swift`.

## 5. Blocking launchd state — fix before any build/install

An attempted automatic loaded-service replacement was rejected in review because it could still
interrupt the routed session and leave the daemon unavailable. A later partial removal made the
worktree internally inconsistent.

Current concrete failures:

- `LaunchdInstallMode`, handoff argument/command helpers, parent-wait logic, bootout/bootstrap,
  spawning, scheduling, and the handoff test remain in `main.rs`.
- The hidden `ServiceCommand::__launchd-handoff` variant and dispatch were removed.
- `OsString` references remain after its import was removed, producing 10 compile errors.
- Even with the import restored, the spawned helper command would be rejected by Clap.
- The rejected design's deeper issues remain: a fixed two-second delay does not protect later
  routed requests, redirected stdio is not true lifecycle detachment, and successful bootout plus
  failed bootstrap has no rollback.

Required resolution:

1. Fully remove the rejected handoff helpers/mode/test or replace them with a genuinely safe,
   reviewed design. Preserve the useful PATH rendering and known-directory detection.
2. Do not synchronously stop a launchd daemon from a request routed through that daemon.
3. Make loaded/unloaded behavior and failure reporting honest and testable; never report a
   replacement as successful if only an unobservable helper was scheduled.
4. Run formatting only on intentional touched Rust regions/files without absorbing unrelated root
   work.
5. Run the complete Rust workspace and Swift suites, inspect the final diff/status, then commit and
   push deliberately.
6. Build both binaries and the app from that exact clean commit before local installation.

## 6. Validation record

- Earlier combined follow-up snapshot, before the later hook/UI additions and launchd half-removal:
  full Rust workspace passed 319 test executions; Swift passed 130 tests in 13 suites.
- Pi hook addition: both `alex` bin targets passed 89 tests each before the launchd half-removal.
- Current unaffected crates: `alex-proxy` 34/34, `alex-auth` 31/31.
- Current Swift tree: 130/130 tests pass.
- `git diff --check` is clean; the launchd plist template passes `plutil -lint`.
- Current full Rust validation is **red** because `alex` does not compile in the half-removed
  launchd state. Earlier green counts must not be used to claim the present tree is green.
- Live Pi session-header verification is green, but it used the tested debug connector and the
  currently installed quota daemon—not the pending combined app/daemon build.

## 7. Backlog audit

### Implemented but pending integration/build/install

- Grok dummy-limit suppression and auth/billing error state.
- Credential-scoped Grok cache invalidation after re-auth.
- Exact-account heartbeat attribution and clickable/deduplicated re-auth alerts.
- Single combined bonded-Codex panel with routing strategy/order and per-account bars.
- Pi menu visibility, robust binary discovery, generated launchd PATH, and session hook packaging.

### Partial

- #16 launchd PATH: rendering/detection are implemented, but service replacement is currently
  broken and blocks the combined build.
- #17 credentials phase 2: list/add/re-auth/pause/resume/remove exists; full edit/copy-secret/reveal
  CRUD does not.
- #20 account observability: trace attribution and analytics shipped; exact heartbeat/alerts are in
  the pending diff. Audit log, encrypted vault export, budget alerts, and full per-account scheduled
  heartbeat coverage remain incomplete.
- CI/release hardening: PR artifact support exists, but the complete immutable-tag/read-only/
  required-package-smoke design described by the older handoff is not merged.

### Not implemented

- #10 hide/alias unverified `gpt-5.5-codex` IDs.
- #11 Trace Browser Copy Path.
- #12 mark in-stream SSE `overloaded_error` events inside HTTP 200 streams as trace errors.
- #13 per-model harness show/hide.
- #18 token/dollar budgets and per-account model allowlists.
- #19 OpenRouter provider, vault, routing, safe headers, catalog/UI, and focused tests. The reserved
  `/private/tmp/alex-openrouter-support` worktree still has no implementation.
- Local-build channel marker/badge and `Install latest production build…` path.
- Transcript model-switch divider, `alex doctor`, Claude/Codex connect support, and broader
  budget-aware routing.

## 8. Relevant branches/worktrees

| Branch/worktree | Current role |
|---|---|
| root `main` at `/Users/madhavajay/dev/alexandria` | User-owned dirty work; 19 behind origin; do not touch |
| `agent/codex-usage-refresh` at `/private/tmp/alex-codex-usage-refresh` | Clean committed `d915b73`; pushed; draft PR #2; installed locally |
| `agent/subscription-ui-followups` at `/private/tmp/alex-subscription-ui` | Dirty pending integration; compile-blocked; not installed |
| `agent/openrouter-support` at `/private/tmp/alex-openrouter-support` | Reserved only; no OpenRouter edits |
| older Dario/full-model/Codex branches | Their relevant feature work is already represented in v0.1.23; do not infer they are the active install source |

Numerous legacy worktrees still exist. Do not remove them as part of this task without separately
confirming ownership and merge state.

## 9. Next steps, in order

1. Finish the launchd handoff removal/redesign until `alex` compiles and review the service behavior.
2. Run `cargo test --workspace --locked`, `swift test`, `git diff --check`, and plist validation.
3. Review only the 14 explicit follow-up files, then commit/push to a deliberate branch/PR target.
4. Build release `alex` + `alexandria` and package AlexandriaBar from that committed revision.
5. Install the matching app and daemon without synchronously killing the route carrying the install.
6. Live-verify:
   - one app and one daemon listener;
   - installed hashes match the committed build;
   - `/admin/limits` shows Grok auth/billing error with no dummy request/token limits;
   - `/admin/accounts/routing/openai` matches the combined bonded panel/order;
   - health alerts target only the real failing account and re-auth rows open the exact account;
   - `/admin/harnesses` sees Pi `0.80.6`, and the Pi menu/Settings action is present;
   - two fresh Pi sessions still produce distinct `x-session-id` traces.
7. Have the user visually confirm the bonded panel and click the Grok re-auth alert.
8. Only then resume OpenRouter or the remaining numbered backlog.

## 10. Key files

- Codex refresh: `crates/alex-auth/src/login.rs`, `crates/alex-auth/src/lib.rs`.
- Grok limits and heartbeat attribution: `crates/alex-proxy/src/lib.rs`.
- Pi connection/session extension and discovery: `crates/alex/src/harness_connect.rs`.
- launchd service generation/replacement: `crates/alex/src/main.rs`,
  `crates/alex/config/launchd/com.alexandria.daemon.plist`.
- Bonded panel: `macos/Sources/AlexandriaBar/MenuCardView.swift`,
  `macos/Sources/AlexandriaBarCore/Models.swift`.
- Alerts/re-auth: `macos/Sources/AlexandriaBarCore/SnapshotStore.swift`,
  `macos/Sources/AlexandriaBar/StatusItemController.swift`.
- Harness UI/result: `macos/Sources/AlexandriaBar/PreferencesView.swift`,
  `macos/Sources/AlexandriaBar/HarnessRefreshResultView.swift`,
  `macos/Sources/AlexandriaBarCore/HarnessModels.swift`.
