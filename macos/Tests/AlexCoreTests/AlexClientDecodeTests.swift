import Foundation
import Testing
@testable import AlexCore

extension AlexClientTests {
@Suite struct Decode {
    @Test func coreStatusAccountsLimitsAndAnalyticsDecodeDaemonShapes() async throws {
        AlexClientURLProtocolStub.reset()
        defer { AlexClientURLProtocolStub.reset() }
        AlexClientURLProtocolStub.handler = { request in
            switch request.path {
            case "/health":
                return .json(#"{"status":"ok","service":"alex","version":"0.9.2","in_flight":2,"uptime_s":481,"dario":true}"#)
            case "/admin/accounts":
                return .json(#"{"accounts":[{"id":"openai-oauth-work","provider":"openai","name":"work","kind":"oauth","label":"Codex (work@example.com)","email":"work@example.com","paused":false,"status":"active","health":"healthy","needs_reauth":false,"expires_at_ms":1783504994142,"expires_in_s":27538,"last_probe":{"ok":true,"status":200,"latency_ms":84,"health":"healthy","checked_at_ms":1783477017897}}]}"#)
            case "/admin/health":
                return .json(#"{"accounts":[{"id":"openai-oauth-work","provider":"openai","kind":"oauth","status":"active","token_expires_in_s":27538,"last_heartbeat":{"account_id":"openai-oauth-work","provider":"openai","ok":true,"status":200,"latency_ms":84,"message":"creds ok","ts_ms":1783477017897}}]}"#)
            case "/admin/limits":
                return .json(#"{"providers":[{"provider":"openai","plan":"pro","source":"captured response headers","observed_at_ms":1783477280438,"windows":[{"window":"5h","used_pct":21,"resets_at_s":1783477712}],"quota":{"kind":"available","label":"Available"}}]}"#)
            case "/admin/analytics":
                return .json(#"{"since_ms":1783473855977,"totals":{"requests":56,"cost_usd":0.10335838,"errors":3,"cost_by_bucket":{"subscription":0.10335838}},"by_model":[{"routed_model":"gpt-5.5","upstream_provider":"openai","requests":29,"errors":1,"cost_usd":0.0506865,"avg_latency_ms":2631.5}]}"#)
            case "/admin/accounts/analytics":
                return .json(#"{"since_ms":1783470000000,"bucket_ms":900000,"by_account":[{"account_id":"openai-oauth-work","provider":"openai","requests":7,"input_tokens":1200,"output_tokens":300,"cost_usd":0.0125,"errors":1,"last_ts_ms":1783477280438}],"series":[{"bucket_ms":1783470000000,"account_id":"openai-oauth-work","requests":7,"input_tokens":1200,"output_tokens":300,"cost_usd":0.0125,"errors":1}],"plot_series":[{"account_id":"openai-oauth-work","name":"work@example.com","values":[0,7]}],"x_labels":["12:00","12:15"],"bucket_count":2}"#)
            default:
                Issue.record("Unexpected request: \(request.method) \(request.path)")
                return .json("{}")
            }
        }
        let client = makeStubbedAlexClient()

        let health = try await client.health()
        let accounts = try await client.accounts()
        let accountHealth = try await client.accountHealth()
        let limits = try await client.limits()
        let analytics = try await client.analytics(sinceMinutes: 90)
        let accountAnalytics = try await client.accountAnalytics(sinceMinutes: 360, bucketMinutes: 15)

        #expect(health.version == "0.9.2")
        #expect(health.inFlight == 2)
        #expect(accounts[0].email == "work@example.com")
        #expect(accounts[0].lastProbe?.latencyMs == 84)
        #expect(accountHealth[0].lastHeartbeat?.status == 200)
        #expect(limits[0].windows?[0].usedPct == 21)
        #expect(analytics.totals.requests == 56)
        #expect(analytics.byModel[0].upstreamProvider == "openai")
        #expect(accountAnalytics.byAccount[0].inputTokens == 1_200)
        #expect(accountAnalytics.plotSeries?[0].values == [0, 7])
        #expect(accountAnalytics.bucketCount == 2)

        let requests = AlexClientURLProtocolStub.requests
        #expect(requests.count == 6)
        for request in requests {
            #expect(request.method == "GET")
            #expect(request.header("x-api-key") == "local-test-key")
        }
        #expect(requests[4].query == ["since_minutes": "90"])
        #expect(requests[5].query == ["since_minutes": "360", "bucket_minutes": "15"])
    }

    @Test func darioStatusDetailAndLogsDecodeRealisticGeneration() async throws {
        AlexClientURLProtocolStub.reset()
        defer { AlexClientURLProtocolStub.reset() }
        let status = #"{"active_generation_id":"gen-4.8.139-61993","should_be_healthy":true,"route_enabled":true,"runtime_version":"v22.14.0","prompt_caches":[{"key":"cache-1","model":"claude-sonnet-4-5"}],"generations":[{"id":"gen-4.8.139-61993","version":"4.8.139","phase":"ready","state":"active","pid":80392,"port":61993,"in_flight":0,"consecutive_failures":0,"last_probe":{"ok":true,"status":200,"latency_ms":1410,"error":null,"at_ms":1783477427269},"started_at":1783476973704,"promoted_at":1783476975856,"stdout_log":"/tmp/dario.out","stderr_log":"/tmp/dario.err"}]}"#
        AlexClientURLProtocolStub.handler = { request in
            if request.path.hasPrefix("/admin/dario/logs/") {
                return .json(#"{"generation_id":"gen-4.8.139-61993","stdout":"ready\n","stderr":"","lines":120}"#)
            }
            return .json(status)
        }
        let client = makeStubbedAlexClient()

        let compact = try #require(try await client.dario())
        let detail = try #require(try await client.darioDetail())
        let logs = try await client.darioLogs(generationId: "gen-4.8.139-61993", lines: 120)

        #expect(compact.generations[0].lastProbe?.ok == true)
        #expect(compact.runtimeVersion == "v22.14.0")
        #expect(detail.generations[0].state == "active")
        #expect(detail.generations[0].stdoutLog == "/tmp/dario.out")
        #expect(logs.stdout == "ready\n")
        #expect(logs.lines == 120)
        let requests = AlexClientURLProtocolStub.requests
        expectStandardRequest(requests[0], path: "/admin/dario")
        expectStandardRequest(requests[1], path: "/admin/dario")
        expectStandardRequest(requests[2], path: "/admin/dario/logs/gen-4.8.139-61993")
        #expect(requests[2].query == ["lines": "120"])
    }

    @Test func traceSearchSummariesDetailBodiesAndTranscriptDecode() async throws {
        AlexClientURLProtocolStub.reset()
        defer { AlexClientURLProtocolStub.reset() }
        AlexClientURLProtocolStub.handler = { request in
            switch request.path {
            case "/traces/search":
                return .json(#"{"traces":[{"id":"trace-1","session_id":"session-1","reasoning_effort":"high","thinking_budget":16000}],"scanned":41}"#)
            case "/traces/sessions":
                return .json(#"{"sessions":[{"session_id":"session-1","first_ts_ms":1783477000000,"last_ts_ms":1783477002000,"trace_count":1,"models":["gpt-5.5"],"providers":["openai"],"harness":"codex","last_status":200}]}"#)
            case "/traces/sessions/session-1/transcript":
                return .json(#"{"session_id":"session-1","turns":[{"trace_id":"trace-1","ts_request_ms":1783477000000,"ts_response_ms":1783477002000,"model":"gpt-5.5","provider":"openai","status":200,"assistant":"done"}]}"#)
            case "/traces/trace-1":
                return .json(#"{"trace":{"id":"trace-1","session_id":"session-1","method":"POST","path":"/v1/responses","status":200,"routed_model":"gpt-5.5","upstream_provider":"openai"},"extras":null}"#)
            case "/traces/trace-1/body/upstream-request":
                return .text(#"{"model":"gpt-5.5","input":"hello"}"#, headers: ["x-alex-body-path": "/tmp/bodies/trace-1-upstream.json"])
            case "/traces/trace-1/reply.md":
                return .text("## Reply\n\nDone.")
            default:
                Issue.record("Unexpected trace request: \(request.path)")
                return .json("{}")
            }
        }
        let client = makeStubbedAlexClient()
        var filters = OmniQuery()
        filters.model = "gpt-5.5"
        filters.provider = "openai"
        filters.status = "200"

        let search = try await client.searchTraces(text: "done", since: "7d", filters: filters)
        let sessions = try await client.traceSessions(since: "7d", limit: 25, middlewareId: "retry-529")
        let transcript = try await client.traceTranscript(sessionId: "session-1", limit: 50)
        let detail = try await client.traceDetail(id: "trace-1")
        let body = try await client.traceBody(id: "trace-1", kind: .upstreamRequest)
        let markdown = try await client.traceReplyMarkdown(traceId: "trace-1")

        #expect(search.scanned == 41)
        #expect(search.traces[0].thinkingBudget == 16_000)
        #expect(sessions[0].models == ["gpt-5.5"])
        #expect(transcript.turns[0].assistant == "done")
        #expect(detail.trace.upstreamProvider == "openai")
        #expect(body.text.contains("gpt-5.5"))
        #expect(body.diskPath == "/tmp/bodies/trace-1-upstream.json")
        #expect(markdown == "## Reply\n\nDone.")

        let requests = AlexClientURLProtocolStub.requests
        #expect(requests[0].query["text"] == "done")
        #expect(requests[0].query["since"] == "7d")
        #expect(requests[0].query["model"] == "gpt-5.5")
        #expect(requests[0].query["provider"] == "openai")
        #expect(requests[0].query["status"] == "200")
        #expect(requests[1].query == ["since": "7d", "limit": "25", "middleware_id": "retry-529"])
        #expect(requests[2].query == ["limit": "50"])
        for request in requests { #expect(request.header("x-api-key") == "local-test-key") }
    }

    @Test func modelsCredentialsRunKeyAndUpdateResponsesDecode() async throws {
        AlexClientURLProtocolStub.reset()
        defer { AlexClientURLProtocolStub.reset() }
        AlexClientURLProtocolStub.handler = { request in
            switch (request.method, request.path) {
            case ("GET", "/v1/models"):
                return .json(#"{"object":"list","data":[{"id":"alex/gpt-5.5","object":"model","owned_by":"alex"},{"id":"alex/claude-fable-5","object":"model","owned_by":"alex"}]}"#)
            case ("GET", "/connect"):
                return .text("export ALEX_BASE_URL=http://127.0.0.1:4100\n")
            case ("GET", "/admin/credentials"):
                return .json(#"{"inbound":{"admin_key":{"present":true},"local_key":{"present":true},"run_keys":[]},"outbound":[{"kind":"oauth","id":"openai-oauth-work","provider":"openai","present":true,"active":true,"identity":"work@example.com","expires_at_ms":null,"source":"vault"}]}"#)
            case ("POST", "/admin/run-keys"):
                return .json(#"{"id":"rk-abc","key":"alxk-secret-once","key_fingerprint":"0123456789abcdef","kind":"run","run_id":null,"label":"Trace tool","tags":{"model":"gpt-5.5"},"expires_ms":1783563400000}"#)
            case ("GET", "/admin/update"):
                return .json(#"{"current":"0.9.2","latest":"0.10.0","update_available":true,"update_channel":"beta","notes_url":"https://example.test/notes","checked_at_ms":1783477427269}"#)
            case ("GET", "/admin/update/channel"):
                return .json(#"{"channel":"beta"}"#)
            case ("POST", "/admin/update/channel"):
                return .json(#"{"channel":"stable","latest":"0.9.2","update_available":false}"#)
            default:
                Issue.record("Unexpected request: \(request.method) \(request.path)")
                return .json("{}")
            }
        }
        let client = makeStubbedAlexClient()

        #expect(try await client.modelCatalog() == ["alex/gpt-5.5", "alex/claude-fable-5"])
        #expect(try await client.credentialsEnvironment().contains("ALEX_BASE_URL"))
        #expect(try await client.credentials().outbound[0].identity == "work@example.com")
        let minted = try await client.mintRunKey(
            label: "  Trace tool  ", model: " gpt-5.5 ", ttlSeconds: 3_600)
        #expect(minted.keyFingerprint == "0123456789abcdef")
        #expect(minted.tags == ["model": "gpt-5.5"])
        #expect(try await client.daemonUpdateStatus().latest == "0.10.0")
        #expect(try await client.daemonUpdateChannel().channel == "beta")
        #expect(try await client.setDaemonUpdateChannel("stable").updateAvailable == false)

        let requests = AlexClientURLProtocolStub.requests
        #expect(requests[1].query == ["format": "env"])
        let mintBody = try requests[3].jsonBody()
        #expect(mintBody["label"] as? String == "Trace tool")
        #expect(mintBody["ttl_seconds"] as? Int == 3_600)
        #expect((mintBody["tags"] as? [String: String]) == ["model": "gpt-5.5"])
        let channelBody = try requests[6].jsonBody()
        #expect(channelBody["channel"] as? String == "stable")
    }

    @Test func notificationMiddlewareExoAndOpenRouterResponsesDecode() async throws {
        AlexClientURLProtocolStub.reset()
        defer { AlexClientURLProtocolStub.reset() }
        AlexClientURLProtocolStub.handler = { request in
            switch (request.method, request.path) {
            case ("GET", "/admin/notifications"):
                return .json(#"{"channels":[{"index":0,"id":"telegram-main","kind":"telegram","format":"telegram","host":"api.telegram.org","bot_username":"alex_bot","chat_id":"42","allow_commands":false,"supports_replies":true,"min_level":"warn","categories":["reauth"],"last_sent_ms":null,"last_error":null}],"cooldown_seconds":300,"timeout_seconds":10}"#)
            case ("POST", "/admin/notifications/validate"):
                return .json(#"{"ok":true,"bot_username":"alex_bot","bot_name":"Alex Bot","error":null}"#)
            case ("POST", "/admin/notifications/discover-chat"):
                return .json(#"{"ok":true,"chats":[{"chat_id":"42","chat_name":"Alex Alerts"}],"error":null}"#)
            case ("POST", "/admin/notifications"):
                return .json(#"{"ok":true,"channel":{"index":0,"id":"telegram-main","kind":"telegram","format":"telegram","host":"api.telegram.org","bot_username":"alex_bot","chat_id":"42","allow_commands":false,"supports_replies":true,"min_level":"warn","categories":["reauth"],"last_sent_ms":null,"last_error":null},"error":null}"#)
            case ("POST", "/admin/notifications/test"):
                return .json(#"{"channels":[{"ok":true,"error":null}]}"#)
            case ("POST", "/admin/notifications/commands"):
                return .json(#"{"ok":true,"channel":{"index":0,"id":"telegram-main","kind":"telegram","format":"telegram","host":"api.telegram.org","bot_username":"alex_bot","chat_id":"42","allow_commands":true,"supports_replies":true,"min_level":"warn","categories":["reauth"],"last_sent_ms":null,"last_error":null}}"#)
            case ("GET", "/admin/notifications/log"):
                return .json(#"{"messages":[{"ts":1783477900000,"direction":"out","channel_id":"telegram-main","kind":"reauth","ok":true,"error":null,"summary":"Re-authentication required"}]}"#)
            case ("GET", "/admin/middleware"):
                return .json(#"{"settings":{"enabled":true},"generation":"mw-12","last_reload_ms":1783477900000,"rules":[],"scripts":[],"leases":[],"errors":[]}"#)
            case ("GET", "/admin/exo"):
                return .json(#"{"url":"http://localhost:52415","enabled_models":["llama-3.2"]}"#)
            case ("GET", "/admin/exo/status"):
                return .json(#"{"running":true,"url":"http://localhost:52415","model_count":2,"error":null}"#)
            case ("GET", "/admin/exo/models"):
                return .json(#"{"models":[{"id":"llama-3.2","name":"Llama 3.2","family":"llama","quantization":"Q4_K_M","context_length":131072,"enabled":true,"running":true}]}"#)
            case ("GET", "/admin/openrouter/catalog"):
                return .json(#"{"models":["anthropic/claude-sonnet-4.5","openai/gpt-5.5"]}"#)
            case ("GET", "/admin/openrouter/exposed"):
                return .json(#"{"exposed":["openai/gpt-5.5"],"available":["anthropic/claude-sonnet-4.5","openai/gpt-5.5"]}"#)
            default:
                Issue.record("Unexpected request: \(request.method) \(request.path)")
                return .json("{}")
            }
        }
        let client = makeStubbedAlexClient()
        let channel = TelegramNotificationChannelRequest(
            token: "bot-secret", chatID: "42", minLevel: .warn, categories: ["reauth"])

        #expect(try await client.notificationSettings().channels[0].botUsername == "alex_bot")
        #expect(try await client.validateTelegramNotification(token: "bot-secret").botName == "Alex Bot")
        #expect(try await client.discoverTelegramChats(token: "bot-secret").chats[0].chatName == "Alex Alerts")
        #expect(try await client.saveTelegramNotification(channel).channel?.id == "telegram-main")
        #expect(try await client.testTelegramNotification(channel).channels[0].ok)
        #expect(try await client.testTelegramNotification(channelId: "telegram-main").channels[0].ok)
        #expect(try await client.setChannelCommands(channelId: "telegram-main", allowCommands: true).channel?.allowCommands == true)
        #expect(try await client.notificationsLog(limit: 5).messages[0].category == "reauth")
        #expect(try await client.middlewareStatus().generation == "mw-12")
        #expect(try await client.exoConfig().enabledModels == ["llama-3.2"])
        #expect(try await client.exoStatus().modelCount == 2)
        #expect(try await client.exoModels()[0].contextLength == 131_072)
        #expect(try await client.openRouterCatalog() == ["anthropic/claude-sonnet-4.5", "openai/gpt-5.5"])
        #expect(try await client.openRouterExposed().exposed == ["openai/gpt-5.5"])

        let requests = AlexClientURLProtocolStub.requests
        let validate = try requests[1].jsonBody()
        #expect(validate["format"] as? String == "telegram")
        #expect(validate["token"] as? String == "bot-secret")
        let savedTest = try requests[5].jsonBody()
        #expect(savedTest["channel_id"] as? String == "telegram-main")
        let commands = try requests[6].jsonBody()
        #expect(commands["allow_commands"] as? Bool == true)
        #expect(requests[7].query == ["limit": "5"])
        for request in requests { #expect(request.header("x-api-key") == "local-test-key") }
    }
}
}
