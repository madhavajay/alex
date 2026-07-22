# Handoff: beta.21 regex middleware (in progress)

Repo: `/Users/madhavajay/dev/alex/v1-integration` (all work uncommitted, on top of `864d05d`).

## Goal
Replace the exact-match/error-kind middleware rule system with a regex-first one
(harness name/version, model, provider, HTTP status, response header key=>value,
body — all full regexes), simplify the wizard UI, add a wizard↔code JSON view,
then build and install beta.21 for the user to test.

## Done and verified
- **Rust — compiles clean** (`cargo check -p alex-middleware -p alex-proxy` passed):
  - `crates/alex-middleware/src/rule.rs`: new `MatchConditionsV1` fields
    `harness_name_regex`, `harness_version_regex`, `model_regex`, `provider_regex`,
    `status_regex`, `response_header_regex` (`HeaderRegexMatcherV1 {key, value}`),
    `body_regex`. Values within one field are ORed. Plus `engine.rs`, `builtins.rs`,
    `validate.rs`.
  - Built-in Fable rule (`alex.fable-5-to-gpt-5.6-sol` in `builtins.rs`) now matches
    visibly: `model_regex ^claude-fable-5$`, `provider_regex ^anthropic$`,
    `status_regex ^200$`, body regex for SSE `message_delta` with
    `"stop_reason":"refusal"`; capability `attempt.read_error_body` added.
  - `crates/alex-proxy/src/middleware.rs`: `migrate_legacy_fable_matcher` converts
    user-customized legacy rules to the regex fields, preserving effort/notice.
  - `render_middleware_notice` (`crates/alex-proxy/src/lib.rs` ~12094) now takes
    from/to provider and supports `{from_provider}`/`{to_provider}` in addition to
    `{from_model}`, `{to_model}`, `{requested_model}`, `{target_model}`. Call site
    (~line 13346) and its unit test updated; `cargo check -p alex-proxy` passes.
- **Swift models — written, NOT yet compiled**:
  `macos/Sources/AlexCore/MiddlewareModels.swift`. Old wizard draft
  (errorKinds/conditionMode/statusText/bodyPhrasesText/equivalenceClass, wizard
  error-kind enum) fully removed; new regex-first `MiddlewareWizardDraft` spliced
  in at the `// MARK: - Middleware Wizard` section (~line 929+): regex fields,
  `responseHeaderRegexText` (one `key => value` per line), `localValidationErrors`
  with `NSRegularExpression` syntax checks, `makeRule` emitting the new snake_case
  fields, `init(rule:)` projecting legacy rules via `exactAlternation`/
  `statusAlternation`, `fableToSolExample`, notice placeholders incl. providers.
  Action is always session-scoped reroute to a specific model (request-only scope
  removed).
- **Swift wizard UI — written, NOT yet compiled**:
  `macos/Sources/Alex/MiddlewareWizard.swift` fully rewritten: 4 steps
  (Name / When / Action / Review), regex text fields with harness/provider chips
  that stay in sync with anchored alternations, simple Action page (target model,
  provider mode + picker, effort, TTL stepper, notice with template hints,
  priority), and a Wizard/Code segmented toggle. Code view shows formatted
  RuleSpecV1 JSON, editable, with Copy/Paste/Reset; JSON that fails to decode as
  `MiddlewareRuleSpecV1` blocks Save and blocks switching back to wizard view;
  daemon `validateMiddlewareRule` must pass before Save enables.

## Remaining work (in order)
1. Update call sites in `macos/Sources/Alex/MiddlewarePreferencesSection.swift`
   (uses `fableToSolExample` and `MiddlewareWizardDraft(rule:)` around lines
   21, 61, 702, 708, 742, 782) to the new draft shape.
2. Fix tests asserting the old shape:
   `macos/Tests/AlexCoreTests/MiddlewareModelsTests.swift` (e.g. expects
   `errorKinds == ["upstream_refusal"]`) and
   `macos/Tests/AlexCoreTests/HarnessClientTests.swift` (~line 879).
3. Swift compile check (`swift build` in `macos/`). The new wizard references
   `MiddlewareValidationResponse.canonicalRule`, `ProviderInfo.supportedProviders`,
   `ProviderInfo.displayName/loginArg`, `HarnessIconView`, `AlexTheme.*` — verify
   these exist; adjust to actual API names.
4. Not yet implemented: live "which traces would this rule match" testing against
   the trace explorer while composing a rule (explicit user requirement).
5. Version bump to beta.21 (location not yet found — check `build-signed.sh`,
   `packaging/`, `install-beta.sh`, Info.plist generation) and build/install.
   User wants **local debug builds without notarization**: `build-signed.sh
   --skip-notarize` exists; `macos/Scripts/run.sh` + `package_app.sh` may support
   unsigned dev builds — prefer that path for local iteration.
6. Extensive end-to-end testing after the UI is up (user will test in parallel).

## Gotchas
- The Read tool misbehaved on `MiddlewareModels.swift` in a prior session
  (ignored offset); use `sed -n 'START,ENDp'` for targeted reads if it recurs.
- Rust JSON contract is snake_case; Swift Codable already maps the new fields.
- Wizard efforts: low/medium/high/xhigh/max. Harnesses: claude, codex, pi, amp,
  gemini, opencode.
- beta.20 is currently installed on the user's machine.
