import Foundation
#if canImport(FoundationNetworking)
import FoundationNetworking
#endif
import Testing
@testable import AlexandriaBarCore

@Suite(.serialized) struct HarnessClientTests {
    @Test func alexErrorApprovalUsesFingerprintEndpoint() async throws {
        HarnessEndpointURLProtocol.handler = { request in
            #expect(request.url?.path == "/admin/alex-errors/0123456789abcdef/approve")
            #expect(request.httpMethod == "POST")
            #expect(request.value(forHTTPHeaderField: "x-api-key") == "local-test-key")
            let response = HTTPURLResponse(
                url: request.url!, statusCode: 200, httpVersion: nil, headerFields: nil)!
            return (response, Data(#"{"approved":true}"#.utf8))
        }
        defer { HarnessEndpointURLProtocol.handler = nil }

        let cfg = URLSessionConfiguration.ephemeral
        cfg.protocolClasses = [HarnessEndpointURLProtocol.self]
        let client = AlexandriaClient(
            config: DaemonConfig(host: "127.0.0.1", port: 4100, localKey: "local-test-key"),
            session: URLSession(configuration: cfg))

        try await client.approveAlexErrorCredential(fingerprint: "0123456789abcdef")
    }

    @Test func harnessKeyMintUsesHarnessKindLabelAndNoExpiry() async throws {
        HarnessEndpointURLProtocol.handler = { request in
            #expect(request.url?.path == "/admin/run-keys")
            #expect(request.httpMethod == "POST")
            #expect(request.value(forHTTPHeaderField: "x-api-key") == "local-test-key")
            let json = try #require(
                JSONSerialization.jsonObject(with: requestBody(request)) as? [String: Any])
            #expect(json["kind"] as? String == "harness")
            #expect(json["label"] as? String == "codex-remote")
            #expect(json["ttl_seconds"] == nil)
            let response = HTTPURLResponse(
                url: request.url!, statusCode: 200, httpVersion: nil, headerFields: nil)!
            let payload = #"{"id":"rk-remote","key":"alxk-fresh","kind":"harness","label":"codex-remote","tags":{},"expires_ms":null}"#
            return (response, Data(payload.utf8))
        }
        defer { HarnessEndpointURLProtocol.handler = nil }

        let cfg = URLSessionConfiguration.ephemeral
        cfg.protocolClasses = [HarnessEndpointURLProtocol.self]
        let client = AlexandriaClient(
            config: DaemonConfig(host: "127.0.0.1", port: 4100, localKey: "local-test-key"),
            session: URLSession(configuration: cfg))

        let minted = try await client.mintRunKey(
            label: "codex-remote", model: nil, ttlSeconds: nil, kind: .harness)
        #expect(minted.kind == "harness")
        #expect(minted.key == "alxk-fresh")
    }

    @Test func darioRepairPostsToRepairEndpoint() async throws {
        HarnessEndpointURLProtocol.handler = { request in
            #expect(request.url?.path == "/admin/dario/repair")
            #expect(request.httpMethod == "POST")
            #expect(request.value(forHTTPHeaderField: "x-api-key") == "local-test-key")
            let response = HTTPURLResponse(
                url: request.url!, statusCode: 200, httpVersion: nil, headerFields: nil)!
            return (response, Data("{}".utf8))
        }
        defer { HarnessEndpointURLProtocol.handler = nil }

        let cfg = URLSessionConfiguration.ephemeral
        cfg.protocolClasses = [HarnessEndpointURLProtocol.self]
        let client = AlexandriaClient(
            config: DaemonConfig(host: "127.0.0.1", port: 4100, localKey: "local-test-key"),
            session: URLSession(configuration: cfg))

        try await client.darioRepair()
    }

    @Test func codexAutoIdentityLoginOmitsNameAndRequestsDeviceFlow() async throws {
        HarnessEndpointURLProtocol.handler = { request in
            #expect(request.url?.path == "/admin/auth/login/start")
            #expect(request.httpMethod == "POST")
            #expect(request.value(forHTTPHeaderField: "x-api-key") == "local-test-key")
            let body = try requestBody(request)
            let json = try #require(JSONSerialization.jsonObject(with: body) as? [String: Any])
            #expect(json["provider"] as? String == "codex")
            #expect(json["auto_identity"] as? Bool == true)
            #expect(json["name"] == nil)
            #expect(json["force"] == nil)
            let response = HTTPURLResponse(
                url: request.url!, statusCode: 200, httpVersion: nil, headerFields: nil)!
            let payload = #"{"login_id":"login-test","provider":"codex","mode":"device","state":"pending","authorize_url":"https://auth.openai.com/codex/device","user_code":"ABCD-EFGH"}"#
            return (response, Data(payload.utf8))
        }
        defer { HarnessEndpointURLProtocol.handler = nil }

        let cfg = URLSessionConfiguration.ephemeral
        cfg.protocolClasses = [HarnessEndpointURLProtocol.self]
        let client = AlexandriaClient(
            config: DaemonConfig(host: "127.0.0.1", port: 4100, localKey: "local-test-key"),
            session: URLSession(configuration: cfg))

        let login = try await client.authLoginStart(
            provider: "codex", name: nil, autoIdentity: true)
        #expect(login.mode == "device")
        #expect(login.userCode == "ABCD-EFGH")
    }

    @Test func reauthNotificationPostsProviderAndAccount() async throws {
        HarnessEndpointURLProtocol.handler = { request in
            #expect(request.url?.path == "/admin/auth/reauth-notify")
            #expect(request.httpMethod == "POST")
            #expect(request.value(forHTTPHeaderField: "x-api-key") == "local-test-key")
            let json = try #require(
                JSONSerialization.jsonObject(with: requestBody(request)) as? [String: Any])
            #expect(json["provider"] as? String == "xai")
            #expect(json["account_id"] as? String == "xai-oauth-work")
            #expect(json["force"] == nil)
            let response = HTTPURLResponse(
                url: request.url!, statusCode: 200, httpVersion: nil, headerFields: nil)!
            let payload = #"{"login_id":"login-fresh","provider":"xai","state":"pending","verification_uri_complete":"https://auth.example/device?code=fresh","notification_sent":true,"reused":false,"fallback":false}"#
            return (response, Data(payload.utf8))
        }
        defer { HarnessEndpointURLProtocol.handler = nil }

        let cfg = URLSessionConfiguration.ephemeral
        cfg.protocolClasses = [HarnessEndpointURLProtocol.self]
        let client = AlexandriaClient(
            config: DaemonConfig(host: "127.0.0.1", port: 4100, localKey: "local-test-key"),
            session: URLSession(configuration: cfg))

        let response = try await client.reauthNotify(
            provider: "xai", accountId: "xai-oauth-work")
        #expect(response.notificationSent)
        #expect(response.loginId == "login-fresh")
    }

    @Test func forcedReauthNotificationRequestsReplacement() async throws {
        HarnessEndpointURLProtocol.handler = { request in
            #expect(request.url?.path == "/admin/auth/reauth-notify")
            let json = try #require(
                JSONSerialization.jsonObject(with: requestBody(request)) as? [String: Any])
            #expect(json["provider"] as? String == "anthropic")
            #expect(json["account_id"] as? String == "anthropic-oauth")
            #expect(json["force"] as? Bool == true)
            let response = HTTPURLResponse(
                url: request.url!, statusCode: 200, httpVersion: nil, headerFields: nil)!
            let payload = #"{"login_id":"login-replacement","provider":"anthropic","state":"pending","notification_sent":true,"reused":false,"fallback":false}"#
            return (response, Data(payload.utf8))
        }
        defer { HarnessEndpointURLProtocol.handler = nil }

        let cfg = URLSessionConfiguration.ephemeral
        cfg.protocolClasses = [HarnessEndpointURLProtocol.self]
        let client = AlexandriaClient(
            config: DaemonConfig(host: "127.0.0.1", port: 4100, localKey: "local-test-key"),
            session: URLSession(configuration: cfg))

        _ = try await client.reauthNotify(
            provider: "anthropic", accountId: "anthropic-oauth", force: true)
    }

    @Test func forcedLoginStartRequestsReplacement() async throws {
        HarnessEndpointURLProtocol.handler = { request in
            #expect(request.url?.path == "/admin/auth/login/start")
            let json = try #require(
                JSONSerialization.jsonObject(with: requestBody(request)) as? [String: Any])
            #expect(json["provider"] as? String == "anthropic")
            #expect(json["name"] as? String == "default")
            #expect(json["force"] as? Bool == true)
            let response = HTTPURLResponse(
                url: request.url!, statusCode: 200, httpVersion: nil, headerFields: nil)!
            let payload = #"{"login_id":"login-replacement","provider":"anthropic","mode":"paste","state":"pending","authorize_url":"https://console.anthropic.com/oauth/authorize"}"#
            return (response, Data(payload.utf8))
        }
        defer { HarnessEndpointURLProtocol.handler = nil }

        let cfg = URLSessionConfiguration.ephemeral
        cfg.protocolClasses = [HarnessEndpointURLProtocol.self]
        let client = AlexandriaClient(
            config: DaemonConfig(host: "127.0.0.1", port: 4100, localKey: "local-test-key"),
            session: URLSession(configuration: cfg))

        _ = try await client.authLoginStart(
            provider: "anthropic", name: "default", force: true)
    }

    @Test func openRouterKeyPostsSecretAndAttributionInJSONBody() async throws {
        HarnessEndpointURLProtocol.handler = { request in
            #expect(request.url?.path == "/admin/auth/openrouter-key")
            #expect(request.httpMethod == "POST")
            #expect(request.value(forHTTPHeaderField: "x-api-key") == "local-test-key")
            #expect(request.value(forHTTPHeaderField: "content-type") == "application/json")
            let json = try #require(
                JSONSerialization.jsonObject(with: requestBody(request)) as? [String: Any])
            #expect(json["key"] as? String == "or-secret")
            #expect(json["display_name"] as? String == "Personal")
            #expect(json["http_referer"] as? String == "https://alexandria.example")
            #expect(json["x_title"] as? String == "Alexandria")
            #expect(json["remove"] == nil)
            let response = HTTPURLResponse(
                url: request.url!, statusCode: 200, httpVersion: nil, headerFields: nil)!
            return (response, Data(#"{"saved":"openrouter-api-key"}"#.utf8))
        }
        defer { HarnessEndpointURLProtocol.handler = nil }

        let cfg = URLSessionConfiguration.ephemeral
        cfg.protocolClasses = [HarnessEndpointURLProtocol.self]
        let client = AlexandriaClient(
            config: DaemonConfig(host: "127.0.0.1", port: 4100, localKey: "local-test-key"),
            session: URLSession(configuration: cfg))

        let saved = try await client.setOpenRouterKey(
            "or-secret", displayName: "Personal",
            httpReferer: "https://alexandria.example",
            xTitle: "Alexandria")
        #expect(saved == "openrouter-api-key")
    }

    @Test func openRouterKeyRemovalPostsNoSecret() async throws {
        HarnessEndpointURLProtocol.handler = { request in
            #expect(request.url?.path == "/admin/auth/openrouter-key")
            #expect(request.httpMethod == "POST")
            let json = try #require(
                JSONSerialization.jsonObject(with: requestBody(request)) as? [String: Any])
            #expect(json["remove"] as? Bool == true)
            #expect(json["key"] == nil)
            #expect(json["http_referer"] == nil)
            #expect(json["x_title"] == nil)
            let response = HTTPURLResponse(
                url: request.url!, statusCode: 200, httpVersion: nil, headerFields: nil)!
            return (response, Data(#"{"removed":"openrouter-api-key"}"#.utf8))
        }
        defer { HarnessEndpointURLProtocol.handler = nil }

        let cfg = URLSessionConfiguration.ephemeral
        cfg.protocolClasses = [HarnessEndpointURLProtocol.self]
        let client = AlexandriaClient(
            config: DaemonConfig(host: "127.0.0.1", port: 4100, localKey: "local-test-key"),
            session: URLSession(configuration: cfg))

        try await client.removeOpenRouterKey()
    }

    @Test func cliProxyAPIConnectProbesThroughDaemonAndDecodesCapabilities() async throws {
        HarnessEndpointURLProtocol.handler = { request in
            #expect(request.url?.path == "/admin/auth/cliproxyapi")
            #expect(request.httpMethod == "POST")
            #expect(request.value(forHTTPHeaderField: "x-api-key") == "local-test-key")
            let json = try #require(
                JSONSerialization.jsonObject(with: requestBody(request)) as? [String: Any])
            #expect(json["url"] as? String == "http://127.0.0.1:8317/v1")
            #expect(json["credential"] as? String == "cpa-secret")
            let response = HTTPURLResponse(
                url: request.url!, statusCode: 200, httpVersion: nil, headerFields: nil)!
            let payload = #"{"saved":"cliproxyapi-default","url":"http://127.0.0.1:8317/v1","models":["openai/gpt-5"],"capabilities":{"openai_chat":true,"openai_responses":true,"anthropic_translation":true,"streaming":true,"tool_calls":true}}"#
            return (response, Data(payload.utf8))
        }
        defer { HarnessEndpointURLProtocol.handler = nil }

        let cfg = URLSessionConfiguration.ephemeral
        cfg.protocolClasses = [HarnessEndpointURLProtocol.self]
        let client = AlexandriaClient(
            config: DaemonConfig(host: "127.0.0.1", port: 4100, localKey: "local-test-key"),
            session: URLSession(configuration: cfg))

        let result = try await client.connectCLIProxyAPI(
            url: "http://127.0.0.1:8317/v1", credential: "cpa-secret")
        #expect(result.saved == "cliproxyapi-default")
        #expect(result.models == ["openai/gpt-5"])
        #expect(result.capabilities.streaming)
        #expect(result.capabilities.toolCalls)
    }

    @Test func codexRoutingGetsPolicyAndPerAccountWindows() async throws {
        HarnessEndpointURLProtocol.handler = { request in
            #expect(request.url?.path == "/admin/accounts/routing/openai")
            #expect(request.httpMethod == "GET")
            #expect(request.value(forHTTPHeaderField: "x-api-key") == "local-test-key")
            let response = HTTPURLResponse(
                url: request.url!, statusCode: 200, httpVersion: nil, headerFields: nil)!
            let payload = #"""
            {"provider":"openai","strategy":"priority","reserve_pct":15,"allow_mid_thread_failover":false,"accounts":[{"account_id":"openai-oauth-work","eligible":true,"priority":0,"reserve_pct":20,"observed_at_ms":1783477280438,"windows":[{"window":"5h","used_pct":20,"resets_at_s":1783477712}],"reset_selection":{"window":"5h","used_pct":20,"resets_at_s":1783477712}}]}
            """#
            return (response, Data(payload.utf8))
        }
        defer { HarnessEndpointURLProtocol.handler = nil }

        let cfg = URLSessionConfiguration.ephemeral
        cfg.protocolClasses = [HarnessEndpointURLProtocol.self]
        let client = AlexandriaClient(
            config: DaemonConfig(host: "127.0.0.1", port: 4100, localKey: "local-test-key"),
            session: URLSession(configuration: cfg))

        let routing = try await client.codexRouting()
        #expect(routing.strategy == .priority)
        #expect(routing.reservePct == 15)
        #expect(!routing.allowMidThreadFailover)
        #expect(routing.accounts[0].reservePct == 20)
        #expect(routing.accounts[0].resetSelection?.window == "5h")
        #expect(routing.accounts[0].windows[0].remainingPct == 80)
    }

    @Test func codexRoutingPutsAtomicPolicy() async throws {
        HarnessEndpointURLProtocol.handler = { request in
            #expect(request.url?.path == "/admin/accounts/routing/openai")
            #expect(request.httpMethod == "PUT")
            #expect(request.value(forHTTPHeaderField: "content-type") == "application/json")
            let body = try requestBody(request)
            let json = try #require(JSONSerialization.jsonObject(with: body) as? [String: Any])
            #expect(json["strategy"] as? String == "round_robin")
            #expect(json["reserve_pct"] as? Double == 10)
            #expect(json["allow_mid_thread_failover"] as? Bool == false)
            let accounts = try #require(json["accounts"] as? [[String: Any]])
            #expect(accounts.count == 2)
            #expect(accounts[0]["account_id"] as? String == "openai-oauth-personal")
            #expect(accounts[0]["eligible"] as? Bool == true)
            #expect(accounts[0]["priority"] as? Int == 0)
            #expect(accounts[0]["reserve_pct"] as? Double == 15)
            #expect(accounts[1]["eligible"] as? Bool == false)
            let response = HTTPURLResponse(
                url: request.url!, statusCode: 204, httpVersion: nil, headerFields: nil)!
            return (response, Data())
        }
        defer { HarnessEndpointURLProtocol.handler = nil }

        let cfg = URLSessionConfiguration.ephemeral
        cfg.protocolClasses = [HarnessEndpointURLProtocol.self]
        let client = AlexandriaClient(
            config: DaemonConfig(host: "127.0.0.1", port: 4100, localKey: "local-test-key"),
            session: URLSession(configuration: cfg))
        let update = CodexRoutingUpdate(
            strategy: .roundRobin,
            reservePct: 10,
            allowMidThreadFailover: false,
            accounts: [
                CodexRoutingAccountUpdate(
                    accountId: "openai-oauth-personal", eligible: true, priority: 0,
                    reservePct: 15),
                CodexRoutingAccountUpdate(
                    accountId: "openai-oauth-work", eligible: false, priority: 1,
                    reservePct: 5),
            ])

        try await client.updateCodexRouting(update)
    }

    @Test func providerRoutingUsesGeneralizedEndpointAndDecodesReserve() async throws {
        HarnessEndpointURLProtocol.handler = { request in
            #expect(request.url?.path == "/admin/routing/openrouter")
            #expect(request.httpMethod == "GET")
            #expect(request.value(forHTTPHeaderField: "x-api-key") == "local-test-key")
            let response = HTTPURLResponse(
                url: request.url!, statusCode: 200, httpVersion: nil, headerFields: nil)!
            let payload = #"""
            {"provider":"openrouter","strategy":"round_robin","reserve_pct":0,"allow_mid_thread_failover":true,"accounts":[{"account_id":"openrouter-api-key","eligible":true,"priority":0,"reserve_pct":0,"reserve_blocked":false,"windows":[]}]}
            """#
            return (response, Data(payload.utf8))
        }
        defer { HarnessEndpointURLProtocol.handler = nil }

        let cfg = URLSessionConfiguration.ephemeral
        cfg.protocolClasses = [HarnessEndpointURLProtocol.self]
        let client = AlexandriaClient(
            config: DaemonConfig(host: "127.0.0.1", port: 4100, localKey: "local-test-key"),
            session: URLSession(configuration: cfg))

        let routing = try await client.routing(provider: "openrouter")
        #expect(routing.provider == "openrouter")
        #expect(routing.strategy == .roundRobin)
        #expect(routing.reservePct == 0)
        #expect(routing.accounts[0].reservePct == 0)
    }

    @Test func providerRoutingPutsCompleteProviderPolicy() async throws {
        HarnessEndpointURLProtocol.handler = { request in
            #expect(request.url?.path == "/admin/routing/anthropic")
            #expect(request.httpMethod == "PUT")
            let json = try #require(JSONSerialization.jsonObject(with: requestBody(request)) as? [String: Any])
            #expect(json["strategy"] as? String == "priority")
            #expect(json["reserve_pct"] as? Double == 25)
            let accounts = try #require(json["accounts"] as? [[String: Any]])
            #expect(accounts.count == 1)
            #expect(accounts[0]["account_id"] as? String == "anthropic:work")
            #expect(accounts[0]["eligible"] as? Bool == true)
            #expect(accounts[0]["priority"] as? Int == 0)
            #expect(accounts[0]["reserve_pct"] as? Double == 0)
            let response = HTTPURLResponse(
                url: request.url!, statusCode: 204, httpVersion: nil, headerFields: nil)!
            return (response, Data())
        }
        defer { HarnessEndpointURLProtocol.handler = nil }

        let cfg = URLSessionConfiguration.ephemeral
        cfg.protocolClasses = [HarnessEndpointURLProtocol.self]
        let client = AlexandriaClient(
            config: DaemonConfig(host: "127.0.0.1", port: 4100, localKey: "local-test-key"),
            session: URLSession(configuration: cfg))
        try await client.updateRouting(provider: "anthropic", ProviderRoutingUpdate(
            strategy: .priority, reservePct: 25,
            accounts: [ProviderRoutingAccountUpdate(
                accountId: "anthropic:work", eligible: true, priority: 0, reservePct: 0)]))
    }

    @Test func harnesses404MapsToUnsupported() async throws {
        HarnessEndpointURLProtocol.handler = { request in
            #expect(request.url?.path == "/admin/harnesses")
            #expect(request.value(forHTTPHeaderField: "x-api-key") == "local-test-key")
            let response = HTTPURLResponse(
                url: request.url!, statusCode: 404, httpVersion: nil, headerFields: nil)!
            return (response, Data(#"{"error":{"message":"not found"}}"#.utf8))
        }
        defer { HarnessEndpointURLProtocol.handler = nil }

        let cfg = URLSessionConfiguration.ephemeral
        cfg.protocolClasses = [HarnessEndpointURLProtocol.self]
        let session = URLSession(configuration: cfg)
        let client = AlexandriaClient(
            config: DaemonConfig(host: "127.0.0.1", port: 4100, localKey: "local-test-key"),
            session: session)

        let harnesses = try await client.harnesses()
        #expect(harnesses == nil)
    }

    @Test @MainActor func harnessSnapshotRefreshAppliesLatestConnectionState() async throws {
        HarnessEndpointURLProtocol.handler = { request in
            #expect(request.url?.path == "/admin/harnesses")
            #expect(request.url?.query == "refresh=1")
            let response = HTTPURLResponse(
                url: request.url!, statusCode: 200, httpVersion: nil, headerFields: nil)!
            let payload = #"{"harnesses":[{"name":"kimi","installed":true,"config_dir_exists":true,"connected":false,"supports_connect":true,"daemon_reachable":true}],"checked_ms":1}"#
            return (response, Data(payload.utf8))
        }
        defer { HarnessEndpointURLProtocol.handler = nil }

        let cfg = URLSessionConfiguration.ephemeral
        cfg.protocolClasses = [HarnessEndpointURLProtocol.self]
        let client = AlexandriaClient(
            config: DaemonConfig(host: "127.0.0.1", port: 4100, localKey: "local-test-key"),
            session: URLSession(configuration: cfg))
        let store = SnapshotStore()

        await store.refreshHarnesses(using: client)

        #expect(store.harnessesSupported == true)
        #expect(store.harnesses.map(\.name) == ["kimi"])
        #expect(store.harnesses[0].connected == false)
        #expect(store.harnessesCheckedMs == 1)
        #expect(store.harnessesAreStale)
    }

    @Test func daemonUpdateApplyPostsAndDecodesAccepted() async throws {
        HarnessEndpointURLProtocol.handler = { request in
            #expect(request.url?.path == "/admin/update")
            #expect(request.httpMethod == "POST")
            #expect(request.value(forHTTPHeaderField: "x-api-key") == "local-test-key")
            let response = HTTPURLResponse(
                url: request.url!, statusCode: 202, httpVersion: nil, headerFields: nil)!
            return (response, Data(#"{"applying":true,"current":"0.1.0","latest":"0.2.0","update_available":true}"#.utf8))
        }
        defer { HarnessEndpointURLProtocol.handler = nil }

        let cfg = URLSessionConfiguration.ephemeral
        cfg.protocolClasses = [HarnessEndpointURLProtocol.self]
        let session = URLSession(configuration: cfg)
        let client = AlexandriaClient(
            config: DaemonConfig(host: "127.0.0.1", port: 4100, localKey: "local-test-key"),
            session: session)

        let response = try await client.daemonUpdateApply()
        #expect(response.applying)
        #expect(response.latest == "0.2.0")
    }

    @Test func daemonUpdateStatusGetsAndDecodes() async throws {
        HarnessEndpointURLProtocol.handler = { request in
            #expect(request.url?.path == "/admin/update")
            #expect(request.httpMethod == "GET")
            #expect(request.value(forHTTPHeaderField: "x-api-key") == "local-test-key")
            let response = HTTPURLResponse(
                url: request.url!, statusCode: 200, httpVersion: nil, headerFields: nil)!
            return (response, Data(#"{"current":"0.1.0","latest":"0.2.0","update_available":true,"update_channel":"beta"}"#.utf8))
        }
        defer { HarnessEndpointURLProtocol.handler = nil }

        let cfg = URLSessionConfiguration.ephemeral
        cfg.protocolClasses = [HarnessEndpointURLProtocol.self]
        let client = AlexandriaClient(
            config: DaemonConfig(host: "127.0.0.1", port: 4100, localKey: "local-test-key"),
            session: URLSession(configuration: cfg))

        let response = try await client.daemonUpdateStatus()
        #expect(response.current == "0.1.0")
        #expect(response.latest == "0.2.0")
        #expect(response.updateAvailable)
        #expect(response.updateChannel == "beta")
    }

    @Test func daemonUpdateApply409UsesReason() async throws {
        HarnessEndpointURLProtocol.handler = { request in
            #expect(request.url?.path == "/admin/update")
            let response = HTTPURLResponse(
                url: request.url!, statusCode: 409, httpVersion: nil, headerFields: nil)!
            return (response, Data(#"{"applying":false,"reason":"alex is managed by Homebrew - run `brew upgrade alex`"}"#.utf8))
        }
        defer { HarnessEndpointURLProtocol.handler = nil }

        let cfg = URLSessionConfiguration.ephemeral
        cfg.protocolClasses = [HarnessEndpointURLProtocol.self]
        let session = URLSession(configuration: cfg)
        let client = AlexandriaClient(
            config: DaemonConfig(host: "127.0.0.1", port: 4100, localKey: "local-test-key"),
            session: session)

        do {
            _ = try await client.daemonUpdateApply()
            Issue.record("daemonUpdateApply should reject managed installs")
        } catch AlexandriaClient.ClientError.daemonUpdateRejected(let reason) {
            #expect(reason == "alex is managed by Homebrew - run `brew upgrade alex`")
        }
    }

    @Test func refreshHarnessConfigPostsAndDecodes() async throws {
        let payload = #"""
        {"refreshed":true,"path":"/Users/x/.pi/agent/models.json","models_total":31,"added":["alex/claude-fable-5","alex/grok-4.5"],"removed":["alex/old-model"],"unchanged":29,"key":"reused","base_url":"http://127.0.0.1:4100"}
        """#
        HarnessEndpointURLProtocol.handler = { request in
            #expect(request.url?.path == "/admin/harnesses/pi/refresh-config")
            #expect(request.httpMethod == "POST")
            #expect(request.value(forHTTPHeaderField: "x-api-key") == "local-test-key")
            let response = HTTPURLResponse(
                url: request.url!, statusCode: 200, httpVersion: nil, headerFields: nil)!
            return (response, Data(payload.utf8))
        }
        defer { HarnessEndpointURLProtocol.handler = nil }

        let cfg = URLSessionConfiguration.ephemeral
        cfg.protocolClasses = [HarnessEndpointURLProtocol.self]
        let session = URLSession(configuration: cfg)
        let client = AlexandriaClient(
            config: DaemonConfig(host: "127.0.0.1", port: 4100, localKey: "local-test-key"),
            session: session)

        let response = try await client.refreshHarnessConfig("pi")
        #expect(response.refreshed == true)
        #expect(response.modelsTotal == 31)
        #expect(response.path.hasSuffix("models.json"))
        #expect(response.added == ["alex/claude-fable-5", "alex/grok-4.5"])
        #expect(response.removed == ["alex/old-model"])
        #expect(response.unchanged == 29)
        #expect(response.key == "reused")
        #expect(response.baseUrl == "http://127.0.0.1:4100")
    }

    @Test func codexDefaultRoutePutsSelectionAndDecodesRestartRequirement() async throws {
        HarnessEndpointURLProtocol.handler = { request in
            #expect(request.url?.path == "/admin/harnesses/codex/default-route")
            #expect(request.httpMethod == "PUT")
            #expect(request.value(forHTTPHeaderField: "x-api-key") == "local-test-key")
            let body = try requestBody(request)
            let json = try #require(JSONSerialization.jsonObject(with: body) as? [String: Any])
            #expect(json["route"] as? String == "openai")
            let response = HTTPURLResponse(
                url: request.url!, statusCode: 200, httpVersion: nil, headerFields: nil)!
            return (
                response,
                Data(#"{"default_route":"openai","restart_required":true}"#.utf8))
        }
        defer { HarnessEndpointURLProtocol.handler = nil }

        let cfg = URLSessionConfiguration.ephemeral
        cfg.protocolClasses = [HarnessEndpointURLProtocol.self]
        let session = URLSession(configuration: cfg)
        let client = AlexandriaClient(
            config: DaemonConfig(host: "127.0.0.1", port: 4100, localKey: "local-test-key"),
            session: session)

        let response = try await client.setCodexDefaultRoute("openai")
        #expect(response.defaultRoute == "openai")
        #expect(response.restartRequired)
    }

    @Test func connectHarnessPostsAndDecodesRichSummary() async throws {
        let payload = #"""
        {"path":"/Users/x/.pi/agent/models.json","models_total":28,"added":["alex/claude-opus-4-8"],"removed":[],"unchanged":0,"key":"minted","base_url":"http://127.0.0.1:4100","key_id":"rk-deadbeef"}
        """#
        HarnessEndpointURLProtocol.handler = { request in
            #expect(request.url?.path == "/admin/harnesses/pi/connect")
            #expect(request.httpMethod == "POST")
            #expect(request.url?.query?.contains("dry_run") != true)
            let response = HTTPURLResponse(
                url: request.url!, statusCode: 200, httpVersion: nil, headerFields: nil)!
            return (response, Data(payload.utf8))
        }
        defer { HarnessEndpointURLProtocol.handler = nil }

        let cfg = URLSessionConfiguration.ephemeral
        cfg.protocolClasses = [HarnessEndpointURLProtocol.self]
        let session = URLSession(configuration: cfg)
        let client = AlexandriaClient(
            config: DaemonConfig(host: "127.0.0.1", port: 4100, localKey: "local-test-key"),
            session: session)

        let response = try await client.connectHarness("pi")
        #expect(response.refreshed == nil)
        #expect(response.modelsTotal == 28)
        #expect(response.key == "minted")
        #expect(response.keyId == "rk-deadbeef")
        #expect(response.added == ["alex/claude-opus-4-8"])
        #expect(response.removed.isEmpty)
    }

    @Test func connectHarnessPlanPostsDryRunAndDecodes() async throws {
        let payload = #"""
        {"plan":[
          {"path":"/Users/x/.pi/agent/models.json","action":"create","detail":"add provider 'alexandria' with 28 models"},
          {"path":"run-keys","action":"create","detail":"mint harness key"}
        ]}
        """#
        HarnessEndpointURLProtocol.handler = { request in
            #expect(request.url?.path == "/admin/harnesses/pi/connect")
            #expect(request.httpMethod == "POST")
            #expect(request.url?.query?.contains("dry_run=true") == true)
            let response = HTTPURLResponse(
                url: request.url!, statusCode: 200, httpVersion: nil, headerFields: nil)!
            return (response, Data(payload.utf8))
        }
        defer { HarnessEndpointURLProtocol.handler = nil }

        let cfg = URLSessionConfiguration.ephemeral
        cfg.protocolClasses = [HarnessEndpointURLProtocol.self]
        let session = URLSession(configuration: cfg)
        let client = AlexandriaClient(
            config: DaemonConfig(host: "127.0.0.1", port: 4100, localKey: "local-test-key"),
            session: session)

        let response = try await client.connectHarnessPlan("pi")
        #expect(response.plan.count == 2)
        #expect(response.plan[0].action == "create")
        #expect(response.plan[0].detail.contains("alexandria"))
        #expect(response.plan[1].detail == "mint harness key")
    }

    @Test func disconnectHarnessPlanAndResultDecode() async throws {
        var sawDryRun = false
        let planPayload = #"""
        {"plan":[
          {"path":"/Users/x/.pi/agent/models.json","action":"modify","detail":"remove provider block"},
          {"path":"rk-deadbeef","action":"delete","detail":"revoke harness key deadbeefcafebabe"}
        ]}
        """#
        let resultPayload = #"""
        {"path":"/Users/x/.pi/agent/models.json","models_total":0,"added":[],"removed":["alex/claude-opus-4-8"],"unchanged":0,"key":"revoked","base_url":"http://127.0.0.1:4100","revoked":1,"was_connected":true}
        """#
        HarnessEndpointURLProtocol.handler = { request in
            #expect(request.url?.path == "/admin/harnesses/pi/disconnect")
            #expect(request.httpMethod == "POST")
            let isDry = request.url?.query?.contains("dry_run=true") == true
            if isDry { sawDryRun = true }
            let response = HTTPURLResponse(
                url: request.url!, statusCode: 200, httpVersion: nil, headerFields: nil)!
            return (response, Data((isDry ? planPayload : resultPayload).utf8))
        }
        defer { HarnessEndpointURLProtocol.handler = nil }

        let cfg = URLSessionConfiguration.ephemeral
        cfg.protocolClasses = [HarnessEndpointURLProtocol.self]
        let session = URLSession(configuration: cfg)
        let client = AlexandriaClient(
            config: DaemonConfig(host: "127.0.0.1", port: 4100, localKey: "local-test-key"),
            session: session)

        let plan = try await client.disconnectHarnessPlan("pi")
        #expect(sawDryRun)
        #expect(plan.plan.count == 2)
        #expect(plan.plan[0].detail == "remove provider block")

        let result = try await client.disconnectHarness("pi")
        #expect(result.wasConnected)
        #expect(result.revoked == 1)
        #expect(result.key == "revoked")
        #expect(result.removed == ["alex/claude-opus-4-8"])
        #expect(result.path.hasSuffix("models.json"))
    }

    @Test func resetPostsAllCategoriesForDryRunAndApplyAndDecodesPlan() async throws {
        var requests = 0
        HarnessEndpointURLProtocol.handler = { request in
            #expect(request.url?.path == "/admin/reset")
            #expect(request.httpMethod == "POST")
            #expect(request.value(forHTTPHeaderField: "x-api-key") == "local-test-key")
            #expect(request.value(forHTTPHeaderField: "content-type") == "application/json")
            let json = try #require(
                JSONSerialization.jsonObject(with: requestBody(request)) as? [String: Any])
            #expect(json["credentials"] as? Bool == true)
            #expect(json["settings"] as? Bool == false)
            #expect(json["traces"] as? Bool == true)
            #expect(json["harnesses"] as? Bool == true)
            #expect(json["cache"] as? Bool == false)
            #expect(json["mode"] as? String == "immediate")
            let dryRun = try #require(json["dry_run"] as? Bool)
            #expect(dryRun == (requests == 0))
            requests += 1
            let response = HTTPURLResponse(
                url: request.url!, statusCode: 200, httpVersion: nil, headerFields: nil)!
            let payload = #"""
            {"dry_run":\#(dryRun),"applied":\#(!dryRun),"selected":["credentials","traces","harnesses"],"counts":{"accounts":2,"run_keys":4,"traces":12,"heartbeats":3,"bodies":{"files":5,"bytes":123456},"connected_harnesses":2,"pricing":8,"dario_prompt_cache":{"files":1,"bytes":44}},"harnesses":["claude","codex"],"actions":{"credentials":"remove account JSON; retain removed-accounts tombstones and known_accounts; revoke active run keys","settings":null,"traces":"delete traces and heartbeats; remove data_dir/bodies recursively","harnesses":"disconnect each connected harness through alex harness disconnect","cache":null},"settings":{"preserves_update_channel":false,"preserves_local_key":false,"rotates_local_key":false}}
            """#
            return (response, Data(payload.utf8))
        }
        defer { HarnessEndpointURLProtocol.handler = nil }

        let cfg = URLSessionConfiguration.ephemeral
        cfg.protocolClasses = [HarnessEndpointURLProtocol.self]
        let client = AlexandriaClient(
            config: DaemonConfig(host: "127.0.0.1", port: 4100, localKey: "local-test-key"),
            session: URLSession(configuration: cfg))
        let selection = ResetSelection(credentials: true, traces: true, harnesses: true)

        let plan = try await client.resetPlan(selection)
        #expect(plan.dryRun)
        #expect(!plan.applied)
        #expect(plan.counts.accounts == 2)
        #expect(plan.counts.traces == 12)
        #expect(plan.counts.bodies.bytes == 123_456)
        #expect(plan.counts.connectedHarnesses == 2)
        #expect(plan.harnesses == ["claude", "codex"])

        let result = try await client.reset(selection)
        #expect(!result.dryRun)
        #expect(result.applied)
        #expect(requests == 2)
    }

    @Test func resetSupportsGracefulModeProgressPollingAndDrainCancellation() async throws {
        var requests = 0
        HarnessEndpointURLProtocol.handler = { request in
            let response = HTTPURLResponse(
                url: request.url!, statusCode: 200, httpVersion: nil, headerFields: nil)!
            defer { requests += 1 }
            switch requests {
            case 0:
                #expect(request.url?.path == "/admin/reset")
                #expect(request.httpMethod == "POST")
                let json = try #require(
                    JSONSerialization.jsonObject(with: requestBody(request)) as? [String: Any])
                #expect(json["mode"] as? String == "graceful")
                #expect(json["dry_run"] as? Bool == false)
                return (response, Data(#"{"dry_run":false,"applied":true,"selected":["traces"],"counts":{"accounts":0,"run_keys":0,"traces":1,"heartbeats":0,"bodies":{"files":2,"bytes":30},"connected_harnesses":0,"pricing":0,"dario_prompt_cache":{"files":0,"bytes":0}},"harnesses":[],"actions":{"credentials":null,"settings":null,"traces":"clear","harnesses":null,"cache":null},"settings":{"preserves_update_channel":false,"preserves_local_key":false,"rotates_local_key":false}}"#.utf8))
            case 1:
                #expect(request.url?.path == "/admin/reset/progress")
                #expect(request.httpMethod == "GET")
                return (response, Data(#"{"status":"draining","phase":"draining","detail":"Waiting for routed requests to finish","in_flight":3}"#.utf8))
            default:
                #expect(request.url?.path == "/admin/reset")
                #expect(request.httpMethod == "DELETE")
                return (response, Data(#"{"cancelled":true}"#.utf8))
            }
        }
        defer { HarnessEndpointURLProtocol.handler = nil }

        let cfg = URLSessionConfiguration.ephemeral
        cfg.protocolClasses = [HarnessEndpointURLProtocol.self]
        let client = AlexandriaClient(
            config: DaemonConfig(host: "127.0.0.1", port: 4100, localKey: "local-test-key"),
            session: URLSession(configuration: cfg))

        let result = try await client.reset(ResetSelection(traces: true), mode: .graceful)
        #expect(result.applied)
        let progress = try await client.resetProgress()
        #expect(progress.status == "draining")
        #expect(progress.phase == "draining")
        #expect(progress.inFlight == 3)
        #expect(try await client.cancelResetDrain().cancelled)
        #expect(requests == 3)
    }

    @Test func runKeyBulkMethodsUseStaticRoutesAndDecodeCounts() async throws {
        var requests = 0
        HarnessEndpointURLProtocol.handler = { request in
            #expect(request.value(forHTTPHeaderField: "x-api-key") == "local-test-key")
            let response = HTTPURLResponse(
                url: request.url!, statusCode: 200, httpVersion: nil, headerFields: nil)!
            defer { requests += 1 }
            if requests == 0 {
                #expect(request.url?.path == "/admin/run-keys/revoke-all")
                #expect(request.url?.query == "include_harness=true")
                #expect(request.httpMethod == "POST")
                return (response, Data(#"{"revoked":3}"#.utf8))
            }
            #expect(request.url?.path == "/admin/run-keys/revoked")
            #expect(request.httpMethod == "DELETE")
            return (response, Data(#"{"removed":4}"#.utf8))
        }
        defer { HarnessEndpointURLProtocol.handler = nil }

        let cfg = URLSessionConfiguration.ephemeral
        cfg.protocolClasses = [HarnessEndpointURLProtocol.self]
        let client = AlexandriaClient(
            config: DaemonConfig(host: "127.0.0.1", port: 4100, localKey: "local-test-key"),
            session: URLSession(configuration: cfg))

        #expect(try await client.revokeAllRunKeys() == 3)
        #expect(try await client.clearRevokedRunKeys() == 4)
        #expect(requests == 2)
    }

    @Test func traceSearchSendsKeyFingerprintFilter() async throws {
        HarnessEndpointURLProtocol.handler = { request in
            #expect(request.url?.path == "/traces/search")
            let components = try #require(URLComponents(url: request.url!, resolvingAgainstBaseURL: false))
            let query = Dictionary(uniqueKeysWithValues: (components.queryItems ?? []).compactMap {
                item in item.value.map { (item.name, $0) }
            })
            #expect(query["key_fingerprint"] == "5effb978eb304b0b")
            let response = HTTPURLResponse(
                url: request.url!, statusCode: 200, httpVersion: nil, headerFields: nil)!
            return (response, Data(#"{"traces":[]}"#.utf8))
        }
        defer { HarnessEndpointURLProtocol.handler = nil }

        let cfg = URLSessionConfiguration.ephemeral
        cfg.protocolClasses = [HarnessEndpointURLProtocol.self]
        let client = AlexandriaClient(
            config: DaemonConfig(host: "127.0.0.1", port: 4100, localKey: "local-test-key"),
            session: URLSession(configuration: cfg))

        _ = try await client.searchTraces(
            text: "", filters: OmniQuery.parse("key:5effb978eb304b0b"))
    }

    @Test func middlewareValidationWrapsCanonicalRule() async throws {
        HarnessEndpointURLProtocol.handler = { request in
            #expect(request.url?.path == "/admin/middleware/validate")
            #expect(request.httpMethod == "POST")
            let json = try #require(
                JSONSerialization.jsonObject(with: requestBody(request)) as? [String: Any])
            let rule = try #require(json["rule"] as? [String: Any])
            #expect(rule["id"] as? String == "fable-overload-to-sol")
            let match = try #require(rule["when"] as? [String: Any])
            #expect(match["harness_names"] as? [String] == ["claude", "codex", "pi"])
            let response = HTTPURLResponse(
                url: request.url!, statusCode: 200, httpVersion: nil, headerFields: nil)!
            return (response, Data(#"{"valid":true,"errors":[],"warnings":[]}"#.utf8))
        }
        defer { HarnessEndpointURLProtocol.handler = nil }

        let cfg = URLSessionConfiguration.ephemeral
        cfg.protocolClasses = [HarnessEndpointURLProtocol.self]
        let client = AlexandriaClient(
            config: DaemonConfig(host: "127.0.0.1", port: 4100, localKey: "local-test-key"),
            session: URLSession(configuration: cfg))
        let rule = try MiddlewareWizardDraft.fableToSolExample.makeRule(
            id: "fable-overload-to-sol")

        let result = try await client.validateMiddlewareRule(rule)
        #expect(result.valid)
    }

    @Test func middlewareDryRunUsesCanonicalIdentifierFields() async throws {
        HarnessEndpointURLProtocol.handler = { request in
            #expect(request.url?.path == "/admin/middleware/test")
            #expect(request.httpMethod == "POST")
            let json = try #require(
                JSONSerialization.jsonObject(with: requestBody(request)) as? [String: Any])
            #expect(json["middleware_id"] as? String == "fable-overload-to-sol")
            #expect(json["fixture_name"] as? String == "fable-real-error")
            #expect(json["trace_id"] == nil)
            let response = HTTPURLResponse(
                url: request.url!, statusCode: 200, httpVersion: nil, headerFields: nil)!
            return (response, Data(#"{"matched":true,"proposed_action":"reroute"}"#.utf8))
        }
        defer { HarnessEndpointURLProtocol.handler = nil }

        let cfg = URLSessionConfiguration.ephemeral
        cfg.protocolClasses = [HarnessEndpointURLProtocol.self]
        let client = AlexandriaClient(
            config: DaemonConfig(host: "127.0.0.1", port: 4100, localKey: "local-test-key"),
            session: URLSession(configuration: cfg))

        let result = try await client.testMiddleware(.init(
            middlewareId: "fable-overload-to-sol", fixtureName: "fable-real-error"))
        #expect(result.matched)
        #expect(result.proposedAction == "reroute")
    }
}

private func requestBody(_ request: URLRequest) throws -> Data {
    if let body = request.httpBody { return body }
    let stream = try #require(request.httpBodyStream)
    stream.open()
    defer { stream.close() }
    var data = Data()
    var buffer = [UInt8](repeating: 0, count: 4_096)
    while stream.hasBytesAvailable {
        let count = stream.read(&buffer, maxLength: buffer.count)
        if count < 0 { throw stream.streamError ?? CocoaError(.fileReadUnknown) }
        if count == 0 { break }
        data.append(buffer, count: count)
    }
    return data
}

private final class HarnessEndpointURLProtocol: URLProtocol, @unchecked Sendable {
    nonisolated(unsafe) static var handler: ((URLRequest) throws -> (HTTPURLResponse, Data))?

    override class func canInit(with request: URLRequest) -> Bool {
        true
    }

    override class func canonicalRequest(for request: URLRequest) -> URLRequest {
        request
    }

    override func startLoading() {
        guard let handler = Self.handler else {
            client?.urlProtocol(self, didFailWithError: AlexandriaClient.ClientError.http(500, "missing handler"))
            return
        }
        do {
            let (response, data) = try handler(request)
            client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
            client?.urlProtocol(self, didLoad: data)
            client?.urlProtocolDidFinishLoading(self)
        } catch {
            client?.urlProtocol(self, didFailWithError: error)
        }
    }

    override func stopLoading() {}
}
