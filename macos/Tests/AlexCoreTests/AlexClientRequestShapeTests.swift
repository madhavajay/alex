import Foundation
import Testing
@testable import AlexCore

extension AlexClientTests {
@Suite struct RequestShapes {
    @Test func authLoginStatusCompletionAndReauthRequestsHaveExpectedShapes() async throws {
        AlexClientURLProtocolStub.reset()
        defer { AlexClientURLProtocolStub.reset() }
        AlexClientURLProtocolStub.handler = { request in
            switch request.path {
            case "/admin/auth/reauth-notify":
                return .json(#"{"login_id":"login-reauth","provider":"xai","state":"pending","verification_uri_complete":"https://auth.example/device?code=XYZ","notification_sent":true,"reused":false,"fallback":false}"#)
            default:
                return .json(#"{"login_id":"login-123","provider":"codex","mode":"device","state":"pending","authorize_url":"https://auth.openai.com/codex/device","user_code":"ABCD-EFGH","verification_uri":"https://auth.openai.com/codex/device","expires_at_ms":1783477900000}"#)
            }
        }
        let client = makeStubbedAlexClient()

        _ = try await client.authLoginStart(
            provider: "codex", name: nil, autoIdentity: true, force: true)
        _ = try await client.authLoginStatus(id: "login-123")
        _ = try await client.authLoginComplete(id: "login-123", input: "callback-code#state")
        _ = try await client.reauthNotify(
            provider: "xai", accountId: "xai-oauth-work", force: true)

        let requests = AlexClientURLProtocolStub.requests
        expectStandardRequest(requests[0], method: "POST", path: "/admin/auth/login/start")
        let start = try requests[0].jsonBody()
        #expect(start["provider"] as? String == "codex")
        #expect(start["auto_identity"] as? Bool == true)
        #expect(start["force"] as? Bool == true)
        #expect(start["name"] == nil)

        expectStandardRequest(requests[1], path: "/admin/auth/login/login-123")
        #expect(requests[1].body == nil)

        expectStandardRequest(requests[2], method: "POST", path: "/admin/auth/login/complete")
        let complete = try requests[2].jsonBody()
        #expect(complete["login_id"] as? String == "login-123")
        #expect(complete["input"] as? String == "callback-code#state")

        expectStandardRequest(requests[3], method: "POST", path: "/admin/auth/reauth-notify")
        let reauth = try requests[3].jsonBody()
        #expect(reauth["provider"] as? String == "xai")
        #expect(reauth["account_id"] as? String == "xai-oauth-work")
        #expect(reauth["force"] as? Bool == true)
    }

    @Test func darioTraceAccountAndRunKeyMutationsUseExactRoutes() async throws {
        AlexClientURLProtocolStub.reset()
        defer { AlexClientURLProtocolStub.reset() }
        AlexClientURLProtocolStub.handler = { _ in .json("{}", status: 204) }
        let client = makeStubbedAlexClient()

        try await client.darioRestart()
        try await client.darioUpdate()
        try await client.darioRepair()
        try await client.darioPromptCacheClear(key: "cache-abc")
        try await client.deleteTrace(id: "trace-123")
        try await client.removeAccount(id: "openai-oauth-work")
        try await client.setAccountPaused(id: "openai-oauth-work", paused: true)
        try await client.revokeRunKey(id: "rk-123")

        let requests = AlexClientURLProtocolStub.requests
        let expected: [(String, String)] = [
            ("POST", "/admin/dario/restart"),
            ("POST", "/admin/dario/update"),
            ("POST", "/admin/dario/repair"),
            ("DELETE", "/admin/dario/prompt-caches/cache-abc"),
            ("DELETE", "/traces/trace-123"),
            ("DELETE", "/admin/accounts/openai-oauth-work"),
            ("PUT", "/admin/accounts/openai-oauth-work"),
            ("DELETE", "/admin/run-keys/rk-123"),
        ]
        #expect(requests.count == expected.count)
        for (request, expected) in zip(requests, expected) {
            expectStandardRequest(request, method: expected.0, path: expected.1)
        }
        let pause = try requests[6].jsonBody()
        #expect(pause["paused"] as? Bool == true)
        #expect(requests[6].header("content-type") == "application/json")
    }

    @Test func providerKeyAndOpenRouterMutationsEncodeJSON() async throws {
        AlexClientURLProtocolStub.reset()
        defer { AlexClientURLProtocolStub.reset() }
        AlexClientURLProtocolStub.handler = { request in
            switch request.path {
            case "/admin/openrouter/exposed":
                return .json(#"{"exposed":["anthropic/claude-sonnet-4.5","openai/gpt-5.5"],"available":[]}"#)
            case "/admin/auth/openrouter-key":
                return .json(#"{"saved":"openrouter-api-key"}"#)
            default:
                return .json("{}", status: 204)
            }
        }
        let client = makeStubbedAlexClient()

        try await client.pauseProvider("openrouter", mode: .down)
        try await client.resumeProvider("openrouter")
        try await client.setGeminiKey("gemini-secret")
        _ = try await client.setOpenRouterKey(
            "or-secret", displayName: "Personal", httpReferer: "https://alex.example", xTitle: "Alex")
        _ = try await client.updateOpenRouterExposed([
            "openai/gpt-5.5", "anthropic/claude-sonnet-4.5",
        ])

        let requests = AlexClientURLProtocolStub.requests
        expectStandardRequest(requests[0], method: "POST", path: "/admin/providers/openrouter/pause")
        #expect(try requests[0].jsonBody()["mode"] as? String == "down")
        expectStandardRequest(requests[1], method: "POST", path: "/admin/providers/openrouter/resume")
        expectStandardRequest(requests[2], method: "POST", path: "/admin/auth/gemini-key")
        #expect(try requests[2].jsonBody()["key"] as? String == "gemini-secret")
        expectStandardRequest(requests[3], method: "POST", path: "/admin/auth/openrouter-key")
        let key = try requests[3].jsonBody()
        #expect(key["key"] as? String == "or-secret")
        #expect(key["display_name"] as? String == "Personal")
        #expect(key["http_referer"] as? String == "https://alex.example")
        #expect(key["x_title"] as? String == "Alex")
        expectStandardRequest(requests[4], method: "POST", path: "/admin/openrouter/exposed")
        #expect(try requests[4].jsonBody()["exposed"] as? [String] == [
            "openai/gpt-5.5", "anthropic/claude-sonnet-4.5",
        ])
    }

    @Test func middlewareExoNotificationAndTraceBodyRequestsCarryAuthAndBodies() async throws {
        AlexClientURLProtocolStub.reset()
        defer { AlexClientURLProtocolStub.reset() }
        AlexClientURLProtocolStub.handler = { request in
            switch request.path {
            case "/admin/middleware/settings", "/admin/middleware/reload":
                return .json(#"{"settings":{"enabled":false},"generation":"mw-13","rules":[],"scripts":[],"leases":[],"errors":[]}"#)
            case "/admin/exo":
                return .json(#"{"url":"http://127.0.0.1:52415","enabled_models":["llama-3.2"]}"#)
            case "/traces/trace-2/body/response":
                return .text("response body", headers: ["x-alex-body-path": "/tmp/trace-2-response"])
            default:
                return .json("{}", status: 204)
            }
        }
        let client = makeStubbedAlexClient()

        _ = try await client.updateMiddlewareSettings(.init(
            enabled: false, errorBodyLimitBytes: 32_768, maxAttempts: 2,
            defaultScriptTimeoutMs: 20, defaultScriptMaxOperations: 5_000,
            failMode: "closed"))
        _ = try await client.reloadMiddleware()
        _ = try await client.updateExoConfig(.init(
            url: "http://127.0.0.1:52415", enabledModels: ["llama-3.2"]))
        try await client.removeNotification(id: "telegram-main")
        let body = try await client.traceBody(id: "trace-2", kind: .response)

        let requests = AlexClientURLProtocolStub.requests
        expectStandardRequest(requests[0], method: "PUT", path: "/admin/middleware/settings")
        let settings = try requests[0].jsonBody()
        #expect(settings["enabled"] as? Bool == false)
        #expect(settings["error_body_limit_bytes"] as? Int == 32_768)
        #expect(settings["max_attempts"] as? Int == 2)
        #expect(settings["fail_mode"] as? String == "closed")
        expectStandardRequest(requests[1], method: "POST", path: "/admin/middleware/reload")
        #expect(try requests[1].jsonBody().isEmpty)
        expectStandardRequest(requests[2], method: "PUT", path: "/admin/exo")
        let exo = try requests[2].jsonBody()
        #expect(exo["url"] as? String == "http://127.0.0.1:52415")
        #expect(exo["enabled_models"] as? [String] == ["llama-3.2"])
        expectStandardRequest(requests[3], method: "DELETE", path: "/admin/notifications/telegram-main")
        expectStandardRequest(requests[4], path: "/traces/trace-2/body/response")
        #expect(body.diskPath == "/tmp/trace-2-response")
        #expect(body.text == "response body")
    }

    @Test func middlewareRuleAndLeaseLifecycleUsesCanonicalIdentifiers() async throws {
        AlexClientURLProtocolStub.reset()
        defer { AlexClientURLProtocolStub.reset() }
        AlexClientURLProtocolStub.handler = { request in
            switch (request.method, request.path) {
            case ("POST", "/admin/middleware/rules"),
                 ("PUT", "/admin/middleware/rules/fable-overload-to-sol"):
                return .json(#"{"generation":"mw-14"}"#)
            case ("GET", "/admin/middleware/leases"):
                return .json(#"{"leases":[]}"#)
            default:
                return .json("{}", status: 204)
            }
        }
        let client = makeStubbedAlexClient()
        let rule = try MiddlewareWizardDraft.fableToSolExample.makeRule(
            id: "fable-overload-to-sol")

        #expect(try await client.createMiddlewareRule(rule).generation == "mw-14")
        #expect(try await client.updateMiddlewareRule(rule).generation == "mw-14")
        try await client.deleteMiddlewareRule(id: rule.id)
        #expect(try await client.middlewareLeases().isEmpty)
        try await client.clearMiddlewareLease(id: "session-1")

        let requests = AlexClientURLProtocolStub.requests
        expectStandardRequest(requests[0], method: "POST", path: "/admin/middleware/rules")
        #expect(try requests[0].jsonBody()["id"] as? String == rule.id)
        expectStandardRequest(
            requests[1], method: "PUT",
            path: "/admin/middleware/rules/fable-overload-to-sol")
        #expect(try requests[1].jsonBody()["id"] as? String == rule.id)
        expectStandardRequest(
            requests[2], method: "DELETE",
            path: "/admin/middleware/rules/fable-overload-to-sol")
        expectStandardRequest(requests[3], path: "/admin/middleware/leases")
        expectStandardRequest(
            requests[4], method: "DELETE",
            path: "/admin/middleware/leases/session-1")
    }
}
}
