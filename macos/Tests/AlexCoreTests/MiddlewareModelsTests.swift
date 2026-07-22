import Foundation
import Testing
@testable import AlexCore

@Suite struct MiddlewareModelsTests {
    @Test func fableWizardBuildsCanonicalRule() throws {
        let rule = try MiddlewareWizardDraft.fableToSolExample.makeRule(
            id: "fable-overload-to-sol")

        #expect(rule.id == "fable-overload-to-sol")
        #expect(rule.name == "Fable 5 → GPT-5.6 Sol")
        #expect(rule.enabled)
        #expect(!rule.debug)
        #expect(rule.priority == 100)
        #expect(rule.hook == .attemptResult)
        #expect(rule.capabilities == [
            "attempt.read_error_body", "route.override", "session.pin",
        ])
        #expect(rule.expression == nil)
        #expect(rule.when.harnessNames == nil)
        #expect(rule.when.harnessNameRegex == nil)
        #expect(rule.when.harnessVersionRegex == nil)
        #expect(rule.when.models == nil)
        #expect(rule.when.modelRegex == ["^claude-fable-5$"])
        #expect(rule.when.efforts == nil)
        #expect(rule.when.providers == nil)
        #expect(rule.when.providerRegex == ["^anthropic$"])
        #expect(rule.when.status == nil)
        #expect(rule.when.statusRegex == ["^200$"])
        #expect(rule.when.responseHeaderRegex == nil)
        #expect(rule.when.errorClasses == nil)
        #expect(rule.when.errorKinds == nil)
        #expect(rule.when.bodyContainsAny == nil)
        #expect(rule.when.bodyRegex == [MiddlewareWizardDraft.fableRefusalBodyRegex])
        #expect(rule.when.stableSession == true)
        #expect(rule.then.retrySameRoute == nil)
        #expect(rule.then.reroute?.model == "gpt-5.6-sol")
        #expect(rule.then.reroute?.providerMode == .only)
        #expect(rule.then.reroute?.providers == ["openai"])
        #expect(rule.then.reroute?.scope == .session)
        #expect(rule.then.reroute?.ttlSeconds == 86_400)
        #expect(rule.then.reroute?.notice == nil)
        #expect(rule.then.reroute?.effort == "high")
        #expect(rule.then.reroute?.maxAttempts == 3)
        #expect(rule.then.reroute?.requiredCapabilities.portableHistory == true)
    }

    @Test func ruleDebugFlagDefaultsOffAndRoundTrips() throws {
        let legacyJSON = #"{"id":"rule","name":"Rule","hook":"attempt_result","when":{},"then":{"continue":true}}"#
        let legacy = try JSONDecoder().decode(
            MiddlewareRuleSpecV1.self, from: Data(legacyJSON.utf8))
        #expect(!legacy.debug)

        var debugRule = legacy
        debugRule.debug = true
        let data = try JSONEncoder().encode(debugRule)
        let decoded = try JSONDecoder().decode(MiddlewareRuleSpecV1.self, from: data)
        #expect(decoded.debug)
    }

    @Test func fableRuleRoundTripsWithSnakeCaseRegexFields() throws {
        let rule = try MiddlewareWizardDraft.fableToSolExample.makeRule(
            id: "fable-overload-to-sol")
        let data = try JSONEncoder().encode(rule)
        let json = try #require(JSONSerialization.jsonObject(with: data) as? [String: Any])
        let match = try #require(json["when"] as? [String: Any])
        let action = try #require(json["then"] as? [String: Any])
        let reroute = try #require(action["reroute"] as? [String: Any])

        #expect(json["api_version"] == nil)
        #expect(json["built_in"] == nil)
        #expect(match["harness_names"] == nil)
        #expect(match["harness_name_regex"] == nil)
        #expect(match["body_contains_any"] == nil)
        #expect(match["models"] == nil)
        #expect(match["model_regex"] as? [String] == ["^claude-fable-5$"])
        #expect(match["providers"] == nil)
        #expect(match["provider_regex"] as? [String] == ["^anthropic$"])
        #expect(match["status"] == nil)
        #expect(match["status_regex"] as? [String] == ["^200$"])
        #expect(match["response_header_regex"] == nil)
        #expect(match["body_regex"] as? [String] == [
            MiddlewareWizardDraft.fableRefusalBodyRegex,
        ])
        #expect(match["error_kinds"] == nil)
        #expect(reroute["ttl_seconds"] as? Int == 86_400)
        #expect(reroute["scope"] as? String == "session")
        #expect(reroute["provider_mode"] as? String == "only")
        #expect(reroute["effort"] as? String == "high")

        let decoded = try JSONDecoder().decode(MiddlewareRuleSpecV1.self, from: data)
        #expect(decoded == rule)
    }

    @Test func dryRunResponseDerivesMatchAndSummaryFromDaemonRecords() throws {
        let json = #"{"decision":{"decision":"reroute"},"records":[{"state":"matched","explanation":"Fable fallback matched"}],"valid":true}"#
        let response = try JSONDecoder().decode(
            MiddlewareTestResponse.self, from: Data(json.utf8))

        #expect(response.matched)
        #expect(response.summary == "Fable fallback matched")
        #expect(response.proposedAction == "reroute")
    }

    @Test func middlewareActivityDecodesRealTraceMatches() throws {
        let json = #"{"events":[{"id":"trace-1","ts_ms":123,"session_id":"session-1","harness":"pi","requested_model":"alex/claude-fable-5","routed_model":"gpt-5.6-sol","served_model":"gpt-5.6-sol","status":200,"substituted":true,"attempts":[{"provider":"anthropic","model":"claude-fable-5","status":200,"error_kind":"upstream_refusal","error_code":"bio","middleware_decisions":[{"rule_id":"alex.fable-5-to-gpt-5.6-sol","rule_name":"Fable 5 → GPT-5.6 Sol","state":"matched","action":"reroute","executed":true}]}]}]}"#
        let response = try JSONDecoder().decode(
            MiddlewareActivityResponse.self, from: Data(json.utf8))
        let event = try #require(response.events.first)

        #expect(event.harness == "pi")
        #expect(event.finalModel == "gpt-5.6-sol")
        #expect(event.attempts.first?.errorCode == "bio")
        #expect(event.matchedDecisions.first?.ruleId == "alex.fable-5-to-gpt-5.6-sol")
        #expect(event.matchedDecisions.first?.executed == true)
    }

    @Test func middlewareStatusDecodingUsesBetaSafeDefaults() throws {
        let json = #"{"generation":"gen-7","rules":[],"errors":[]}"#
        let status = try JSONDecoder().decode(
            MiddlewareRuntimeStatus.self, from: Data(json.utf8))

        #expect(status.generation == "gen-7")
        #expect(status.settings.enabled)
        #expect(status.settings.errorBodyLimitBytes == 65_536)
        #expect(status.settings.maxAttempts == 3)
        #expect(status.scripts.isEmpty)
        #expect(status.leases.isEmpty)
    }

    @Test func canonicalLeaseAndStructuredValidationIssueDecode() throws {
        let leaseJSON = #"""
        {"id":"lease-1","harness":"claude","session_id":"session-7","original_model":"claude-fable-5","target":{"kind":"exact","model":"gpt-5.6-sol","providers":{"only":["openai"]}},"source_middleware_id":"fable-overload-to-sol","reason":"overloaded","created_ms":1783477800000,"last_used_ms":1783477900000,"expires_ms":1783564200000}
        """#
        let lease = try JSONDecoder().decode(MiddlewareRouteLease.self, from: Data(leaseJSON.utf8))
        #expect(lease.target.displayModel == "gpt-5.6-sol")
        #expect(lease.target.providers == .only(["openai"]))
        #expect(lease.sourceMiddlewareId == "fable-overload-to-sol")
        #expect(lease.expiresMs == 1_783_564_200_000)

        let validationJSON = #"""
        {"valid":false,"errors":[{"code":"missing_capability","path":"rules[0].capabilities","message":"session.pin is required"}],"warnings":["fallback response will be buffered"]}
        """#
        let validation = try JSONDecoder().decode(
            MiddlewareValidationResponse.self, from: Data(validationJSON.utf8))
        #expect(!validation.valid)
        #expect(validation.errors[0].code == "missing_capability")
        #expect(validation.errors[0].displayText.contains("session.pin is required"))
        #expect(validation.warnings[0].message == "fallback response will be buffered")
    }

    @Test func ruleRoundTripsDaemonStatusMetadata() throws {
        let json = #"""
        {
          "api_version": 1,
          "id": "alex.account-failover",
          "name": "Account Failover",
          "enabled": true,
          "priority": 50,
          "hook": "attempt_result",
          "capabilities": ["route.override"],
          "when": {"status": [429, "500-599"]},
          "then": {"retry_same_route": {"reason": "another account"}},
          "built_in": true,
          "hit_count": 12,
          "last_matched_ms": 1783477900000
        }
        """#
        let rule = try JSONDecoder().decode(MiddlewareRuleSpecV1.self, from: Data(json.utf8))
        #expect(rule.isBuiltIn)
        #expect(rule.hitCount == 12)
        #expect(rule.when.status == [.exact(429), .pattern("500-599")])
        #expect(rule.then.retrySameRoute?.reason == "another account")

        let encoded = try JSONEncoder().encode(rule)
        let decoded = try JSONDecoder().decode(MiddlewareRuleSpecV1.self, from: encoded)
        #expect(decoded == rule)
    }

    @Test func wizardReportsRequiredFieldsInvalidRegexAndProviderSelection() {
        var draft = MiddlewareWizardDraft()
        draft.harnessNameRegex = "["
        draft.modelRegex = "(unterminated"
        draft.providerMode = .only
        let errors = draft.localValidationErrors
        #expect(errors.contains("Enter a name."))
        #expect(errors.contains("Harness name regex is invalid."))
        #expect(errors.contains("Model regex is invalid."))
        #expect(errors.contains("Choose a target model."))
        #expect(errors.contains("Choose at least one target provider."))
        #expect(throws: MiddlewareWizardBuildError.self) {
            try draft.makeRule()
        }
    }

    @Test func legacyRuleProjectsListsAndRefusalIntoRegexDraft() {
        let rule = MiddlewareRuleSpecV1(
            id: "legacy-fable-fallback",
            name: "Legacy Fable fallback",
            priority: 42,
            hook: .attemptResult,
            capabilities: ["route.override", "session.pin"],
            when: .init(
                harnessNames: ["claude", "pi"],
                harnessVersions: ["1.0+beta"],
                models: ["claude-fable-5", "claude-fable-5.1"],
                efforts: ["high"],
                providers: ["anthropic", "bedrock"],
                status: [.exact(200), .exact(429), .pattern("5xx")],
                errorKinds: ["upstream_refusal"]),
            then: .init(reroute: .init(
                model: "gpt-5.6-sol",
                providerMode: .only,
                providers: ["openai"],
                scope: .session,
                ttlSeconds: 3_600,
                effort: "xhigh")))
        let draft = MiddlewareWizardDraft(rule: rule)

        #expect(draft.harnessNameRegex == "^(claude|pi)$")
        #expect(draft.harnessVersionRegex == #"^1\.0\+beta$"#)
        #expect(draft.modelRegex == "^(claude-fable-5|claude-fable-5\\.1)$")
        #expect(draft.providerRegex == "^(anthropic|bedrock)$")
        #expect(draft.sourceEffort == "high")
        #expect(draft.statusRegex == #"^(200|429|5\d\d)$"#)
        #expect(draft.bodyRegex == MiddlewareWizardDraft.fableRefusalBodyRegex)
        #expect(draft.targetModel == "gpt-5.6-sol")
        #expect(draft.targetEffort == "xhigh")
        #expect(draft.providerMode == .only)
        #expect(draft.targetProviders == ["openai"])
        #expect(draft.ttlSeconds == 3_600)
        #expect(draft.priority == 42)
        #expect(!draft.includeNotice)
        #expect(draft.notice == MiddlewareWizardDraft.defaultNoticeTemplate)
    }

    @Test func sourceEffortBecomesAnOptionalMatchCondition() throws {
        var draft = MiddlewareWizardDraft.fableToSolExample
        draft.sourceEffort = "high"
        let rule = try draft.makeRule(id: "fable-high-only")
        #expect(rule.when.efforts == ["high"])
    }

    @Test func checkedNoticeUsesModelTemplates() throws {
        var draft = MiddlewareWizardDraft.fableToSolExample
        draft.includeNotice = true
        let rule = try draft.makeRule(id: "fable-notice")
        #expect(rule.then.reroute?.notice == MiddlewareWizardDraft.defaultNoticeTemplate)
        #expect(rule.capabilities.contains("response.prepend_text"))
    }

    @Test func responseHeaderLinesParseIntoKeyValueRegexMatchers() throws {
        var draft = MiddlewareWizardDraft(
            name: "Route throttled JSON responses",
            responseHeaderRegexText: #"^x-request-id$ => ^req-\d+$"# + "\n"
                + #"^content-type$ => ^application/json(?:;.*)?$"#,
            targetModel: "gpt-5.6-sol",
            providerMode: .any)

        #expect(draft.responseHeaderMatchers == [
            .init(key: "^x-request-id$", value: #"^req-\d+$"#),
            .init(key: "^content-type$", value: "^application/json(?:;.*)?$"),
        ])
        #expect(draft.localValidationErrors.isEmpty)

        let rule = try draft.makeRule(id: "header-route")
        #expect(rule.when.responseHeaderRegex == draft.responseHeaderMatchers)

        draft.responseHeaderRegexText = "missing separator\n[ => ok\nok => ("
        #expect(draft.localValidationErrors.contains(
            "Header matcher line 1 must use key-regex => value-regex."))
        #expect(draft.localValidationErrors.contains(
            "Header matcher line 2 has an invalid key regex."))
        #expect(draft.localValidationErrors.contains(
            "Header matcher line 3 has an invalid value regex."))
    }

    @Test func summaryDescribesRegexConditionsAndSessionReroute() {
        let draft = MiddlewareWizardDraft(
            name: "Fable JSON refusal",
            harnessNameRegex: "^claude$",
            modelRegex: "^claude-fable-5$",
            providerRegex: "^anthropic$",
            sourceEffort: "high",
            statusRegex: "^200$",
            responseHeaderRegexText: "^content-type$ => ^text/event-stream$",
            bodyRegex: MiddlewareWizardDraft.fableRefusalBodyRegex,
            targetModel: "gpt-5.6-sol",
            targetEffort: "xhigh",
            providerMode: .only,
            targetProviders: ["openai"])

        #expect(draft.summary == "When ^claude$ requests ^claude-fable-5$ at high effort through ^anthropic$ and the status matches ^200$, a response header matches, and the body matches the configured regex, route to gpt-5.6-sol at xhigh effort using OpenAI and keep it for the session.")
    }
}
