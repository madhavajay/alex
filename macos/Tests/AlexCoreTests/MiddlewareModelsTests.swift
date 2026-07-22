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
        #expect(rule.priority == 100)
        #expect(rule.hook == .attemptResult)
        #expect(rule.capabilities == ["route.override", "session.pin"])
        #expect(rule.expression == nil)
        #expect(rule.when.harnessNames == nil)
        #expect(rule.when.models == ["claude-fable-5"])
        #expect(rule.when.efforts == nil)
        #expect(rule.when.providers == ["anthropic"])
        #expect(rule.when.status == nil)
        #expect(rule.when.errorClasses == nil)
        #expect(rule.when.errorKinds == ["upstream_refusal"])
        #expect(rule.when.bodyContainsAny == nil)
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

    @Test func fableRuleEncodesGuardrailAndReplacementEffort() throws {
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
        #expect(match["body_contains_any"] == nil)
        #expect(match["models"] as? [String] == ["claude-fable-5"])
        #expect(match["error_kinds"] as? [String] == ["upstream_refusal"])
        #expect(match["status"] == nil)
        #expect(reroute["ttl_seconds"] as? Int == 86_400)
        #expect(reroute["scope"] as? String == "session")
        #expect(reroute["provider_mode"] as? String == "only")
        #expect(reroute["effort"] as? String == "high")
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

    @Test func anyConditionBuildsNestedRuleExpression() throws {
        var draft = MiddlewareWizardDraft.fableToSolExample
        draft.conditionMode = .any
        let rule = try draft.makeRule(id: "fable-any")
        #expect(rule.when.status == nil)
        #expect(rule.when.errorClasses == nil)
        #expect(rule.when.bodyContainsAny == nil)
        guard case let .some(.any(alternatives)) = rule.expression else {
            Issue.record("Expected an any expression")
            return
        }
        #expect(alternatives.count == 1)
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

    @Test func wizardRejectsBodyMatchingAtRequestHookAndUnsafeSessionChoice() {
        var draft = MiddlewareWizardDraft.fableToSolExample
        draft.hook = .requestReceived
        draft.scope = .session
        draft.stableSessionRequired = false
        let errors = draft.localValidationErrors
        #expect(errors.contains("Failure and body conditions require the Failed attempt hook."))
        #expect(errors.contains("Session routing requires a stable session identifier policy."))
        #expect(throws: MiddlewareWizardBuildError.self) {
            try draft.makeRule()
        }
    }

    @Test func builtInRuleProjectsBackToExactWizardValues() throws {
        let rule = try MiddlewareWizardDraft.fableToSolExample.makeRule(
            id: "alex.fable-5-to-gpt-5.6-sol")
        let draft = MiddlewareWizardDraft(rule: rule)

        #expect(draft.modelPattern == "claude-fable-5")
        #expect(draft.sourceProvider == "anthropic")
        #expect(draft.sourceEffort.isEmpty)
        #expect(draft.errorKinds == [.refusal])
        #expect(draft.bodyPhrases.isEmpty)
        #expect(draft.targetModel == "gpt-5.6-sol")
        #expect(draft.targetEffort == "high")
        #expect(draft.targetProviders == ["openai"])
        #expect(draft.scope == .session)
        #expect(draft.ttlSeconds == 86_400)
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
        #expect(rule.then.reroute?.notice == "Alex switched from {from_model} to {to_model}.")
        #expect(rule.capabilities.contains("response.prepend_text"))
    }

    @Test func retryWizardBuildsRetryActionWithoutReroute() throws {
        var draft = MiddlewareWizardDraft(
            name: "Retry server errors",
            modelPattern: "gpt-*",
            hook: .attemptResult,
            errorKinds: [.server],
            action: .retrySame)
        draft.statusText = "5xx"

        let rule = try draft.makeRule()
        #expect(rule.id == "retry-server-errors")
        #expect(rule.then.retrySameRoute != nil)
        #expect(rule.then.reroute == nil)
        #expect(rule.when.status == [.pattern("5xx")])
    }
}
