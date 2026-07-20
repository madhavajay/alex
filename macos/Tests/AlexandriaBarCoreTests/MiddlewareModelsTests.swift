import Foundation
import Testing
@testable import AlexandriaBarCore

@Suite struct MiddlewareModelsTests {
    @Test func fableWizardBuildsCanonicalRule() throws {
        let rule = try MiddlewareWizardDraft.fableToSolExample.makeRule(
            id: "fable-overload-to-sol")

        #expect(rule.id == "fable-overload-to-sol")
        #expect(rule.name == "Move overloaded Fable chats to Sol")
        #expect(rule.enabled)
        #expect(rule.priority == 100)
        #expect(rule.hook == .attemptResult)
        #expect(rule.capabilities == [
            "attempt.read_error_body",
            "route.override",
            "session.pin",
            "response.prepend_text",
        ])
        #expect(rule.expression == nil)
        #expect(rule.when.harnessNames == ["claude", "codex", "pi"])
        #expect(rule.when.models == ["claude-fable-5", "fable-*"])
        #expect(rule.when.providers == ["anthropic"])
        #expect(rule.when.status == [.exact(429), .pattern("500-599")])
        #expect(rule.when.errorClasses == ["capacity", "server"])
        #expect(rule.when.bodyContainsAny == [
            "model is currently overloaded",
            "subscription is unavailable",
        ])
        #expect(rule.when.stableSession == true)
        #expect(rule.then.retrySameRoute == nil)
        #expect(rule.then.reroute?.model == "gpt-5.6-sol")
        #expect(rule.then.reroute?.providerMode == .only)
        #expect(rule.then.reroute?.providers == ["openai"])
        #expect(rule.then.reroute?.scope == .session)
        #expect(rule.then.reroute?.ttlSeconds == 86_400)
        #expect(rule.then.reroute?.notice == "We moved this chat from Fable 5 to GPT 5.6 Sol.")
        #expect(rule.then.reroute?.maxAttempts == 3)
        #expect(rule.then.reroute?.requiredCapabilities.portableHistory == true)
    }

    @Test func fableRuleEncodesSnakeCaseAndHeterogeneousStatuses() throws {
        let rule = try MiddlewareWizardDraft.fableToSolExample.makeRule(
            id: "fable-overload-to-sol")
        let data = try JSONEncoder().encode(rule)
        let json = try #require(JSONSerialization.jsonObject(with: data) as? [String: Any])
        let match = try #require(json["when"] as? [String: Any])
        let action = try #require(json["then"] as? [String: Any])
        let reroute = try #require(action["reroute"] as? [String: Any])

        #expect(json["api_version"] == nil)
        #expect(json["built_in"] == nil)
        #expect(match["harness_names"] as? [String] == ["claude", "codex", "pi"])
        #expect(match["body_contains_any"] as? [String] == [
            "model is currently overloaded", "subscription is unavailable",
        ])
        let statuses = try #require(match["status"] as? [Any])
        #expect(statuses[0] as? Int == 429)
        #expect(statuses[1] as? String == "500-599")
        #expect(reroute["ttl_seconds"] as? Int == 86_400)
        #expect(reroute["provider_mode"] as? String == "only")
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
        #expect(alternatives.count == 3)
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
        draft.stableSessionRequired = false
        let errors = draft.localValidationErrors
        #expect(errors.contains("Failure and body conditions require the Failed attempt hook."))
        #expect(errors.contains("Session routing requires a stable session identifier policy."))
        #expect(throws: MiddlewareWizardBuildError.self) {
            try draft.makeRule()
        }
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
