# Robust Testing Plan — Fake Provider Server + Full-Matrix Coverage

Goal: every provider × model × harness × dialect × outcome combination testable
locally and in CI with **zero live credentials**, by standing up a fake
upstream API server ("fakeprov") that impersonates every provider Alex talks
to — model calls, OAuth/signup, usage stats, model lists, logout, errors, and
ping/smoke — driven by recorded real responses.

---

## 0. Current state (what exists today)

| Asset | Location | Notes |
| --- | --- | --- |
| Runtime upstream override | `crates/alex-proxy/src/lib.rs:1097` `upstream_base()` + `:1111` `set_upstream_base_override()` | **Only Anthropic dispatch reads it** (`:9345`, `:9367`) and OpenAI reads it at `:9500`. Grok/Kimi/OpenRouter/Gemini dispatch use hardcoded consts. |
| OAuth token override | `crates/alex-auth/src/lib.rs:1383` `set_refresh_endpoint_override` | Test-only, refresh endpoint only. |
| Env-overridable endpoints | Kimi OAuth host (`KIMI_CODE_OAUTH_HOST`), update URLs (`ALEX_UPDATE_MANIFEST_URL`/`_RELEASES_URL`), Amp wrap upstream (`AMP_URL`), Exo URL (config), CLIProxyAPI base (config), Telegram base (state field) | Everything else hardcoded. |
| Inline mock servers | 89 ad-hoc axum routers inside `alex-proxy/src/lib.rs` tests | No shared library; per-test duplication. |
| Recorded fixtures | `crates/alex-proxy/tests/fixtures/{middleware,toolcalls,transcript}` | Fable refusal SSE, 4-dialect toolcall pairs, one Chat SSE stream. Thin. |
| Error fixture/injection runtime | `alex fixtures` + `alex simulate inject` CLI, `/admin/fixtures`, `/admin/sessions/{id}/inject`, `/admin/middleware/test` | Already supports save-from-trace — this is our recording path for error bodies. |
| Deterministic CI smoke | `deterministic_platform_smoke` (`alex-proxy/src/lib.rs:18282`) | Mock OpenAI+Exo, streaming, reroute, restart persistence. The seed of this plan. |
| Live-gated suites | `test.sh` (unit/wire W1–W12/harness H1–H7/cliproxyapi/dario), `scripts/harness-regression.sh` (I1–I10D), `crates/alex/tests/harness_matrix.rs` (`ALEX_LIVE_INTEROP`) | Require real accounts/Docker; SKIP-heavy. Stay as the live tier. |
| Swift tests | `macos/Tests/AlexCoreTests` (30 files) | Logic-only; no UI automation; `AlexClient` untested against a live daemon. |
| Web UI tests | none | `crates/alex-proxy/web/{index.html,app.js}` untested. |

**Out of scope**: `site/` (marketing site, has its own `node --test` check) and
the six React/Vite design prototypes in `ui/` (Figma exports, no API calls, not
shipped). The four shipped surfaces are: Rust crates, the daemon (`alex-proxy`),
the SwiftUI macOS app, and the daemon-served web UI at `/ui`.

## 1. The combination space

Axes (from `alex-core` `Provider` enum, harness catalog, dialect enum):

- **Provider (9)**: anthropic, openai, gemini, xai/grok, openrouter, kimi, cliproxyapi, exo, amp(billing/wrap only)
- **Credential kind (per provider)**: oauth | api_key | device-flow oauth | none(exo) | product(amp)
- **Ingress dialect (4)**: anthropic-messages, openai-chat, openai-responses, gemini-generateContent (+ streaming variant of each)
- **Upstream wire format**: anthropic, openai-chat, openai-responses(Codex), gemini API, gemini Code Assist, grok chat, kimi chat, openrouter chat, exo chat
- **Harness (runnable set)**: claude, codex, grok-build, kimi (+ pi, opencode, gemini-cli, amp-wrap, cursor-wrap in the catalog)
- **Outcome**: ok, ok-streamed, tool-call, refusal (Fable), 401/403 auth-expired, 429 rate-limit, 429-quota-exhausted (Kimi body), 5xx server, 529 overloaded, timeout/idle-stream-stall, malformed JSON, disconnect mid-stream
- **Account state**: active, expired-needs-refresh, refresh-fails, paused, reserve-blocked, multi-account failover

We do **not** test the full cross-product. Strategy:
- **Pairwise at the wire tier**: every (provider × outcome) and every (ingress dialect × upstream format) pair at least once, streamed and unstreamed.
- **Depth at the routing tier**: account selection/failover/affinity tested per provider against scripted outcome sequences.
- **Thin at the harness tier**: one representative model per harness against
  fakeprov. The infrastructure already exists: `run_harness()`
  (`crates/alex/src/harness_e2e.rs:294`) launches real harness CLIs in Docker
  and `docker_env()` (`:531`) already points them at the proxy
  (`ANTHROPIC_BASE_URL`/`OPENAI_BASE_URL`). Today's H1–H7 cells hit live
  providers with real credentials; adding **offline cells** = same
  `run_harness()` + daemon upstream overridden to fakeprov. Real harness →
  real proxy → fake upstream, deterministic, no credentials, no network.

## 2. Fake provider server ("fakeprov")

New crate: `crates/alex-fakeprov` — a standalone axum binary + library.
- **Library**: used in-process by cargo tests (replaces the 89 inline mocks over time).
- **Binary**: `alex-fakeprov --port 0 --scenario <name>` for test.sh, CI, Swift/web UI tests, and manual UI poking. Prints its bound port + a control-endpoint key as JSON on stdout.

### 2.1 Endpoint inventory to implement (by provider)

**Anthropic / Claude**
- `POST /v1/messages` (json + SSE; tool calls; Fable refusal stream)
- `POST /v1/oauth/token` (authorize-code exchange + refresh; console.anthropic.com)
- `GET /api/oauth/profile` (account identity)
- `GET /api/oauth/usage` (usage/quota windows; `anthropic-ratelimit-unified-*` headers)
- OAuth authorize page stub (`/oauth/authorize` → redirect w/ code) for login-flow tests

**OpenAI / Codex**
- `POST /v1/chat/completions`, `POST /v1/responses` (api-key path)
- `POST /backend-api/codex/responses` (oauth path; thread affinity, `x-codex-*` headers)
- `GET /v1/models`
- `POST /oauth/token` (auth.openai.com), device trio: `/api/accounts/deviceauth/usercode`, `/deviceauth/token`, verification page
- `GET /backend-api/wham/usage` (usage stats)

**Gemini / Google**
- `POST /v1beta/models/{model}:generateContent` and `:streamGenerateContent` (API key)
- `POST /v1internal:generateContent` etc. (Code Assist), `:loadCodeAssist`, `:onboardUser`
- `POST /token` (oauth2.googleapis.com)

**Grok / xAI**
- `POST /v1/chat/completions` (cli-chat-proxy)
- `POST /oauth2/device/code`, `/oauth2/token`, `GET /oauth2/userinfo`
- `POST /grok_api_v2.GrokBuildBilling/GetGrokCreditsConfig` (gRPC-web framed body!)

**Kimi**
- `POST /coding/v1/chat/completions` (incl. quota-exhausted 200-level error body)
- `GET /coding/v1/usages`
- `POST /api/oauth/device_authorization`, `/api/oauth/token` (15-min tokens → exercises refresh)

**OpenRouter**
- `POST /api/v1/chat/completions`, `GET /api/v1/models` (catalog for curation tests)

**Amp**
- `POST /api/internal?userDisplayBalanceInfo` (balance/usage)
- Wrap-mode upstream stub (WebSocket + HTTP product protocol — reuse `alex wrap smoke` mock)

**CLIProxyAPI**
- `GET /v1/models`, `POST /v1/chat/completions` (v7-compatible shell; existing example `cliproxyapi_v1_fixture.rs` folds in)

**Exo**
- OpenAI-chat-compatible `POST /v1/chat/completions`, model list

**Non-provider externals**
- GitHub releases: `manifest.json` + `/repos/.../releases` (update checks)
- npm registry: `/@askalf%2Fdario/latest` (dario bootstrap/update)
- Telegram: `/bot{token}/sendMessage`, `/getUpdates` (notifications)

### 2.2 Scenario & fixture model

- Fixtures are **files on disk**, one directory per provider:
  `crates/alex-fakeprov/fixtures/<provider>/<endpoint>/<name>.{json,sse,txt}` with a
  small YAML/JSON sidecar (status, headers, latency, chunk pacing for SSE).
- A **scenario** is a named script: ordered/conditional responses per endpoint
  ("first call 429, then ok", "stream stalls after 3 chunks", "token expires
  after N seconds"). Scenarios compose fixtures; tests select one via
  `--scenario` or the control API.
- **Control API** (`POST /_control/...`, key-gated): queue next-response,
  assert-received-requests (for header/auth verification: did Alex send the
  right bearer? strip harness headers? preserve Claude Code allowlist?),
  reset, latency injection. Request log is queryable so tests assert on what
  Alex actually sent upstream — this is half the value.
- Streaming fidelity: SSE emitted with recorded chunk boundaries and optional
  pacing so idle-timeout and destream/retranslate paths are honest.

### 2.3 Making Alex point at fakeprov (code changes required)

1. **Extend `upstream_base_overrides` to all providers**: route every dispatch
   site (`XAI_BASE`, `KIMI_BASE`, `OPENROUTER_BASE`, `GEMINI_API_BASE`,
   `GEMINI_CODE_ASSIST_BASE`, `CODEX_BASE`/`OPENAI_BASE` oauth split) through
   `upstream_base()`. Mechanical, low-risk.
2. **Add an auth/usage endpoint override layer** in `alex-auth` + proxy usage
   pollers: one struct (`EndpointOverrides`) covering token URLs, profile,
   `wham/usage`, Grok billing, Amp balance, Kimi usages. Populated from either
   config (`[testing] upstream_overrides`) or env
   (`ALEX_FAKE_UPSTREAM=http://127.0.0.1:PORT` fans out to all, plus
   per-provider `ALEX_UPSTREAM_<PROVIDER>_URL` for split routing).
   Gate: refuse non-loopback values unless `ALEX_TESTING_ALLOW_REMOTE=1`.
3. **OAuth flows**: login flows open browsers; fakeprov's authorize/device
   endpoints auto-approve so `alex auth login` completes headlessly under
   `ALEX_FAKE_UPSTREAM` (needed for signup-flow tests incl. error paths:
   denied consent, expired device code, slow poll).

## 3. Recording real API data (fill the fixture library)

We already capture full request/response bodies (incl. SSE) in traces — reuse that.

1. **Model-call fixtures from traces**: extend `alex fixtures save --from-trace`
   into `alex fixtures record --provider X --out crates/alex-fakeprov/fixtures/...`
   that exports sanitized request/response pairs (happy, tool-call, streamed)
   from real traces. **Record full transactions, not just bodies**: the trace
   store already captures `req_headers_json`/`resp_headers_json`
   (`crates/alex-store/src/lib.rs:127-128`), `status`, and on-disk body paths
   with auth redaction — export status + headers + body (SSE as ordered frame
   list) so fakeprov replay is wire-correct. Keep semantically relevant headers
   (content-type, `anthropic-version`, rate-limit, request-id); strip
   authorization/cookies/org ids. Sanitizer strips: bearer/keys, account ids,
   emails, org ids, request ids → stable placeholder tokens (extend the
   existing Fable-fixture sanitizer in `alex-lar-scale`).
2. **Auth/usage fixtures**: a one-shot `scripts/record-provider-surfaces.sh`
   that, per logged-in provider, hits profile/usage/models/token-refresh via
   the existing vault creds and dumps sanitized bodies + headers. Run manually
   per provider on a machine with real accounts; commit sanitized output.
3. **Error fixtures**: trigger what we can cheaply (bad key → 401, bad model →
   404/400, oversized prompt → 413/400); harvest naturally occurring 429/529
   from historical traces (`alex traces search --errors`); for the rest, write
   fixtures from provider docs and mark `provenance: synthetic` in the sidecar.
4. **Fixture provenance ledger**: `fixtures/INVENTORY.md` — matrix of
   provider × surface × outcome with status: `recorded | synthetic | missing`.
   CI test fails if a fakeprov endpoint serves a fixture not in the ledger.

### Collection checklist (initial)

| Provider | ok | ok-sse | tool-call | 401 | 429 | 5xx | usage | models | oauth/token | profile |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| anthropic | trace | trace+fixture(fable) | fixture exists | record | record/synth | synth | record | n/a(embedded) | record | record |
| openai api | trace | record | fixture exists | record | record/synth | synth | n/a | record | record | jwt claim |
| codex oauth | trace | record | record | record | record/synth | synth | record(wham) | n/a | record+device | jwt claim |
| gemini api | trace | record | fixture exists | record | synth | synth | n/a | record | record | n/a |
| gemini code-assist | record | record | record | record | synth | synth | n/a | n/a | record | load/onboard |
| grok | trace | record | record | record | synth | synth | record(grpc-web) | n/a | record+device | userinfo |
| kimi | trace | record | record | record | have(quota body) | synth | record | n/a | record+device | n/a |
| openrouter | trace | record | record | record | synth | synth | n/a | record | n/a | n/a |
| amp | n/a | n/a | n/a | record | n/a | synth | record(balance) | n/a | n/a | n/a |
| exo | synth | synth | synth | n/a | n/a | synth | n/a | synth | n/a | n/a |
| cliproxyapi | example exists | example | synth | synth | n/a | synth | n/a | example | n/a | n/a |

("trace" = extractable from existing trace DB; "record" = run the recording
script against live; "synth" = write from docs; "have"/"fixture exists" = done.)

## 4. UI surfaces to test against the mock stack

Test stack for UI = **real daemon** (started on an ephemeral port with temp
`ALEX_HOME`) + **fakeprov behind it** + scripted vault accounts. The UI never
talks to fakeprov directly, so UI tests exercise real daemon behavior.

### 4.1 macOS SwiftUI app (`macos/Sources/Alex`)

Daemon-calling surfaces (via `AlexClient.swift`, base URL from `~/.alex/config.toml` — make `DaemonDiscovery` honor `ALEX_HOME` for test isolation):

| Surface | Daemon endpoints | Mock-driven cases |
| --- | --- | --- |
| OnboardingWindow | health, accounts, login/start+status, harnesses, credentials | fresh install, partial setup, daemon down |
| AuthFlowWindow (subscription signup/login) | `admin/auth/login/start`, `admin/auth/login/{id}` | success, pending-conflict, provider denies, device-code expiry, timeout |
| ReauthWizardWindow | `admin/auth/reauth/*` | expiring token, refresh-failure banner, cancel |
| Preferences→Providers | accounts, health, limits, providers, pause/resume, delete | multi-account, paused, reserve-blocked, logout(delete) |
| StatusItemController/MenuCardView | health, analytics, accounts/analytics, limits | usage graphs w/ known analytics fixtures, empty state, daemon restart |
| GeminiKeyWindow / OpenRouter / Amp key entry | `admin/auth/*-key` | valid key, invalid key (fakeprov 401 on validation), replace |
| PingWindow / Dario windows | `admin/dario`, `admin/dario/ping`, restart/update/repair | ok, dario down, npm update available (fake npm) |
| Middleware prefs + Wizard | `admin/middleware*`, `admin/fixtures`, test | rule toggle, dry-run vs fixture, fable→sol activity rows |
| TraceBrowserWindow | traces/search, summaries, {id}, body, reply.md | seeded trace DB from mock runs, pagination, error-only filter |
| Exo/CLIProxyAPI prefs | `admin/exo*`, `admin/cliproxyapi` | reachable/unreachable upstream (fakeprov as exo) |
| Updater | `admin/update`, channel | update available/none (fake GitHub) |
| Notifications prefs | `admin/notifications*` | Telegram validate/test against fake Telegram |
| CredentialsPreferencesSection | credentials, run-keys mint/revoke | key lifecycle |

Test layers:
1. **AlexCoreTests integration tier** (new): spin daemon+fakeprov from a Swift
   test fixture (launch prebuilt `alex` binary), run `AlexClient` methods
   against it, assert decoding of *real* daemon responses — kills the
   biggest gap (client/decoder drift vs daemon JSON).
2. **XCUITest smoke target** (new, small): launch app with
   `ALEX_HOME=<temp>` pointing at the test daemon; walk onboarding →
   fake login → menu shows account → trace appears. Keep to ~5 golden flows;
   accessibility-identifier every interactive control as we go.

### 4.2 Web UI (`crates/alex-proxy/web`, served at `/ui`)

- Views: onboarding (accounts/OAuth), middleware browser (+ fixture dry-run),
  trace browser (summaries pagination, turn expansion).
- Add **Playwright** suite (`webui-tests/`, pnpm, runs in CI on Linux):
  daemon+fakeprov, drive `/ui` through: connect bootstrap, OAuth onboarding
  happy+error, middleware toggle+dry-run, trace pagination against a seeded
  store, error badges. This also covers Linux (no SwiftUI there).

### 4.3 TUI + CLI

- `alex tui`: snapshot-test the ratatui frames against the scripted daemon
  (insta or golden text frames) for: sessions live view, limits, accounts,
  dario states.
- CLI commands against daemon+fakeprov (bash-level, new `test.sh` tier `mock`):
  `auth login <p>` (headless via fakeprov auto-approve), `auth list`, `ping all`,
  `limits --json`, `status --json`, `doctor`, `update --check` (fake GitHub),
  `fixtures`/`simulate`, `traces search/export`, `keys mint/revoke`,
  `provider pause/resume`. Assert JSON output shapes with `test-assert.py`
  conventions already in `scripts/`.

## 5. CI integration

- New job **`mock-matrix`** (Linux + macOS): build `alex` + `alex-fakeprov`,
  run `./test.sh mock` (CLI tier) + cargo integration tests using fakeprov
  library in-process. No Docker, no creds, fast (<5 min target).
- **Playwright job** (Linux): web UI suite vs daemon+fakeprov.
- **Swift job**: extend existing `swift test` with the daemon-backed
  integration tier (needs prebuilt alex binary — already built in the macOS
  bundle job; wire ordering).
- Existing live/Docker tiers unchanged — they validate fixtures stay truthful.
  Add a scheduled (weekly) **fixture-drift job** on a self-hosted/manual
  runner: run recording script against live, diff normalized shapes vs
  committed fixtures, open an issue on drift.
- Gate: `deterministic_platform_smoke` migrates onto fakeprov library instead
  of its inline mock (proves parity), then stays a release blocker.

## 6. Build phases (worktree-sized work orders)

Each phase is a frozen spec suitable for an implementation subagent in its own
worktree; land order matters for 1→2→3, then 4/5/6 parallelize.

1. **`fakeprov-core`**: crate skeleton, fixture format + sidecar schema,
   scenario engine, control API, SSE pacing, binary + library entry, port-0
   JSON handshake. Include anthropic + openai(chat/responses/codex) endpoints
   using existing fixtures. Unit tests self-contained.
2. **`override-plumbing`**: route all dispatch sites through `upstream_base()`;
   add `EndpointOverrides` for auth/usage/token URLs in alex-auth + pollers;
   `ALEX_FAKE_UPSTREAM` env fan-out with loopback gate; migrate
   `deterministic_platform_smoke` onto fakeprov lib as the parity proof.
3. **`recording-tools`**: `alex fixtures record`, sanitizer hardening,
   `scripts/record-provider-surfaces.sh`, `fixtures/INVENTORY.md` generator +
   ledger-enforcement test.
4. **`provider-fill`**: gemini (api + code-assist), grok (incl. gRPC-web
   billing), kimi (device flow + usages + quota body), openrouter, exo,
   cliproxyapi (fold example in), amp balance, github/npm/telegram stubs.
   Scenario library for the outcome axis (429-then-ok, stream-stall,
   refresh-expiry, failover sequences).
5. **`cli-mock-tier`**: `test.sh mock` tier + CLI assertions + TUI snapshots;
   offline Docker harness cells (M1–Mn: reuse `run_harness()`/`docker_env()`
   from `harness_e2e.rs` with the daemon's upstreams overridden to fakeprov —
   no preflight ping, no credentials, never SKIP for missing accounts);
   un-gate the `#[ignore]`d matrix in `crates/alex/tests/harness_matrix.rs`
   by running it against the fakeprov-backed daemon instead of
   `ALEX_LIVE_INTEROP`; CI `mock-matrix` job.
6. **`ui-suites`**: Playwright web UI suite + CI job; Swift daemon-backed
   integration tier; XCUITest 5-flow smoke; `DaemonDiscovery` ALEX_HOME
   honoring if missing.

## 7. Risks / decisions to confirm

- **Hardcoded OAuth client IDs/hosts**: fakeprov must accept the real client
  IDs Alex sends; fine — assert them in the request log instead of faking new ones.
- **Loopback safety**: override layer must never ship reachable in prod
  defaults — env-gated + loopback-only + absent from `alex credentials` output.
- **Fixture secrecy**: recording sanitizer is a release blocker for committing
  fixtures; secret-scan workflow (gitleaks) already exists — add fixture dir
  to its scope explicitly.
- **gRPC-web (Grok billing)** and **Amp product protocol** are the two
  non-plain-HTTP surfaces; both already have parsers/mocks in-repo to crib from.
- **XCUITest cost**: keep to golden flows; logic stays in AlexCoreTests.

## 8. Offline harness matrix

`./test.sh harness-mock` starts an isolated daemon and fakeprov, then runs the
real Claude, Codex, Grok Build, and Kimi harness images through Docker. The
data-driven matrix covers every harness/provider dialect combination, including
a native tool-call followed by a final canary response. Cells SKIP when Docker
or a harness image is unavailable. `--only`, `--provider`, `--harness`,
`--jobs`, and `--json` use the standard test reporter; cells are serialized
because each one resets fakeprov's ordered scenario queue.

The canonical fake models are `claude-fake-1` (Anthropic), `gpt-fake-1`
(OpenAI API), `codex-fake-1` (Codex OAuth), `gemini-fake-1`, `grok-fake-1`,
`kimi-fake-1`, `openrouter/fake/fake-1`, and `exo/fake-1`. The tier also runs
the real Dario sidecar cell and B1-B5 for account failover, Fable refusal
rerouting, stalled streams, Kimi quota cooldown, and model-list integrity.
