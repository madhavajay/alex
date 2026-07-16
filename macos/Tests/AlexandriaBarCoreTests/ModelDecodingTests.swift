import Foundation
import Testing
@testable import AlexandriaBarCore

@Suite struct ModelDecodingTests {
    func decode<T: Decodable>(_ json: String, as type: T.Type) throws -> T {
        try JSONDecoder().decode(T.self, from: Data(json.utf8))
    }

    @Test func health() throws {
        let json = #"{"dario":true,"in_flight":0,"service":"alexandria","status":"ok","uptime_s":479,"version":"0.1.0"}"#
        let h = try decode(json, as: DaemonHealth.self)
        #expect(h.version == "0.1.0")
        #expect(h.dario)
        #expect(h.uptimeS == 479)
    }

    @Test func accounts() throws {
        let json = #"""
        {"accounts":[
          {"email":"person@example.com","expires_at_ms":1783504994142,"expires_in_s":27538,"id":"anthropic-oauth","kind":"oauth","label":"claude-code (max)","name":"default","paused":false,"provider":"anthropic","status":"active"},
          {"expires_at_ms":null,"expires_in_s":null,"id":"gemini-oauth","kind":"oauth","label":"gemini-cli","name":"default","paused":false,"provider":"gemini","status":"active"},
          {"expires_at_ms":1783439548981,"expires_in_s":-37906,"id":"xai-oauth","kind":"oauth","label":"grok (me@madhavajay.com)","name":"default","paused":false,"provider":"xai","status":"active"}
        ]}
        """#
        let accounts = try decode(json, as: AccountsResponse.self).accounts
        #expect(accounts.count == 3)
        #expect(accounts[0].email == "person@example.com")
        #expect(accounts[1].email == nil)
        #expect(!accounts[0].isExpired)
        #expect(!accounts[1].isExpired)
        #expect(accounts[2].isExpired)
    }

    @Test func accountHealth() throws {
        let json = #"""
        {"accounts":[
          {"id":"anthropic-oauth","kind":"oauth","last_heartbeat":{"account_id":"anthropic-oauth","latency_ms":2244,"message":"creds ok","ok":true,"provider":"anthropic","status":200,"ts_ms":1783477017897},"provider":"anthropic","status":"active","token_expires_in_s":27538},
          {"id":"gemini-oauth","kind":"oauth","last_heartbeat":null,"provider":"gemini","status":"active","token_expires_in_s":null}
        ]}
        """#
        let accounts = try decode(json, as: HealthResponse.self).accounts
        #expect(accounts[0].lastHeartbeat?.ok == true)
        #expect(accounts[0].lastHeartbeat?.latencyMs == 2244)
        #expect(accounts[1].lastHeartbeat == nil)
    }

    @Test func limitsHeterogeneous() throws {
        let json = #"""
        {"providers":[
          {"extra_usage":{"is_enabled":false},"plan":"claude-code (max)","provider":"anthropic","source":"oauth usage endpoint","windows":[{"resets_at":"2026-07-08T03:40:00.730958+00:00","used_pct":7.0,"window":"5h"},{"resets_at":"2026-07-13T17:00:00.730976+00:00","used_pct":22.0,"window":"7d"}]},
          {"active_limit":"premium","credits":{"balance":"","has_credits":"False","unlimited":"False"},"observed_at_ms":1783477280438,"plan":"pro","provider":"openai","source":"captured response headers","windows":[{"resets_at_s":1783477712,"used_pct":6.0,"window":"5h"},{"resets_at_s":1783667025,"used_pct":82.0,"window":"7d"}]},
          {"observed_at_ms":1783477015654,"provider":"xai","quota":{"kind":"out_of_credits","label":"Out of credits","top_up_url":"https://grok.com/settings/billing"},"requests":{"limit":120,"remaining":120},"source":"captured response headers","tokens":{"limit":5000000,"remaining":5000000}}
        ]}
        """#
        let providers = try decode(json, as: LimitsResponse.self).providers
        #expect(providers.count == 3)
        #expect(providers[0].windows?.count == 2)
        #expect(providers[0].windows?[0].resetsDate != nil)
        #expect(providers[1].windows?[1].usedPct == 82.0)
        #expect(providers[1].windows?[0].resetsDate == Date(timeIntervalSince1970: 1783477712))
        #expect(providers[2].windows == nil)
        #expect(providers[2].requests?.limit == 120)
        #expect(providers[2].tokens?.remaining == 5_000_000)
        #expect(providers[2].quota?.kind == "out_of_credits")
        #expect(providers[2].quota?.topUpURL == "https://grok.com/settings/billing")
    }

    @Test func codexAccountRoutingAndLimitWindows() throws {
        let json = #"""
        {"provider":"openai","strategy":"reset_first","reserve_pct":10,"accounts":[
          {"account_id":"openai-oauth-personal","eligible":true,"priority":0,"observed_at_ms":1783477280438,"windows":[{"window":"5h","used_pct":6,"resets_at_s":1783477712},{"window":"7d","used_pct":82,"resets_at_s":1783667025}]},
          {"account_id":"openai-oauth-work","eligible":false,"priority":1,"windows":[]}
        ]}
        """#
        let routing = try decode(json, as: CodexRoutingResponse.self)
        #expect(routing.provider == "openai")
        #expect(routing.strategy == .resetFirst)
        #expect(routing.reservePct == 10)
        #expect(routing.allowMidThreadFailover)
        #expect(routing.accounts.count == 2)
        #expect(routing.accounts[0].reservePct == nil)
        #expect(!routing.accounts[0].reserveBlocked)
        #expect(routing.accounts[0].resetSelection == nil)
        #expect(routing.accounts[0].eligible)
        #expect(routing.accounts[0].windows[0].remainingPct == 94)
        #expect(routing.accounts[0].windows[1].remainingPct == 18)
        #expect(routing.accounts[0].windows[0].resetsDate == Date(timeIntervalSince1970: 1783477712))
        #expect(!routing.accounts[1].eligible)
    }

    @Test func codexRoutingDecodesPerAccountReserveFailoverAndResetSelection() throws {
        let json = #"""
        {"provider":"openai","strategy":"reset_first","reserve_pct":10,"allow_mid_thread_failover":false,"accounts":[
          {"account_id":"openai-oauth-personal","eligible":true,"priority":0,"reserve_pct":15,"reserve_blocked":true,"observed_at_ms":1783477280438,"windows":[{"window":"5h","used_pct":65,"resets_at_s":1783477712}],"reset_selection":{"window":"5h","used_pct":65,"resets_at_s":1783477712}},
          {"account_id":"openai-oauth-work","eligible":true,"priority":1,"reserve_pct":5,"windows":[],"reset_selection":null}
        ]}
        """#
        let routing = try decode(json, as: CodexRoutingResponse.self)
        #expect(!routing.allowMidThreadFailover)
        #expect(routing.accounts.map(\.reservePct) == [15, 5])
        #expect(routing.accounts[0].reserveBlocked)
        #expect(!routing.accounts[1].reserveBlocked)
        #expect(routing.accounts[0].resetSelection?.window == "5h")
        #expect(routing.accounts[0].resetSelection?.usedPct == 65)
        #expect(
            routing.accounts[0].resetSelection?.resetsDate
                == Date(timeIntervalSince1970: 1_783_477_712))
        #expect(routing.accounts[1].resetSelection == nil)
    }

    @Test func perAccountCodexLimitsPreserveIdentityAndSeparateWindows() throws {
        let json = #"""
        {"accounts":[
          {"id":"openai-oauth-personal","provider":"openai","name":"acct-personal","kind":"oauth","label":"codex (personal@example.com)","description":"personal@example.com","email":"personal@example.com","paused":false,"status":"active","expires_at_ms":null,"expires_in_s":null,"limits":{"plan":"pro","source":"Codex usage API","observed_at_ms":1783477280438,"windows":[{"window":"5h","used_pct":49,"resets_at_s":1783477712},{"window":"7d","used_pct":8,"resets_at_s":1783667025}]}},
          {"id":"openai-oauth-work","provider":"openai","name":"acct-work","kind":"oauth","label":"codex (work@example.com)","description":"work@example.com","email":"work@example.com","paused":false,"status":"active","expires_at_ms":null,"expires_in_s":null,"limits":{"plan":"team","source":"Codex usage API","observed_at_ms":1783477300000,"windows":[{"window":"5h","used_pct":12,"resets_at_s":1783478800},{"window":"7d","used_pct":31,"resets_at_s":1783670000}]}}
        ]}
        """#
        let accounts = try decode(json, as: AccountsResponse.self).accounts
        #expect(accounts.count == 2)
        #expect(accounts.map(\.email) == ["personal@example.com", "work@example.com"])
        #expect(accounts[0].limits?.plan == "pro")
        #expect(accounts[0].limits?.windows?[0].usedPct == 49)
        #expect(accounts[1].limits?.plan == "team")
        #expect(accounts[1].limits?.windows?[1].usedPct == 31)
        #expect(accounts[0].limits?.observedAtMs == 1_783_477_280_438)
    }

    @Test func allowancePresentationUsesRemainingQuotaAndLegacyUsedThreshold() throws {
        let json = #"""
        [
          {"window":"5h","used_pct":0},
          {"window":"5h","used_pct":70},
          {"window":"5h","used_pct":90},
          {"window":"5h","used_pct":100}
        ]
        """#
        let windows = try decode(json, as: [LimitWindow].self)
        #expect(windows.map(\.remainingPct) == [100, 30, 10, 0])
        #expect(windows[0].remainingSeverity(warnUsedPct: 90) == .healthy)
        #expect(windows[1].remainingSeverity(warnUsedPct: 90) == .warning)
        #expect(windows[2].remainingSeverity(warnUsedPct: 90) == .critical)
        #expect(windows[3].remainingSeverity(warnUsedPct: 90) == .critical)
    }

    @Test func accountUsageSeries() throws {
        let json = #"""
        {"since_ms":1783470000000,"bucket_ms":3600000,"by_account":[
          {"account_id":"openai-oauth-personal","provider":"openai","requests":7,"input_tokens":1200,"output_tokens":300,"cost_usd":0.0125,"errors":1,"last_ts_ms":1783477280438}
        ],"series":[
          {"bucket_ms":1783470000000,"account_id":"openai-oauth-personal","requests":4,"input_tokens":700,"output_tokens":100,"cost_usd":0.007,"errors":0},
          {"bucket_ms":1783473600000,"account_id":"openai-oauth-personal","requests":3,"input_tokens":500,"output_tokens":200,"cost_usd":0.0055,"errors":1}
        ]}
        """#
        let analytics = try decode(json, as: AccountAnalyticsResponse.self)
        #expect(analytics.byAccount[0].costUsd == 0.0125)
        #expect(analytics.byAccount[0].errors == 1)
        #expect(analytics.series.count == 2)
        #expect(analytics.series[0].inputTokens + analytics.series[0].outputTokens == 800)
    }

    @Test func analytics() throws {
        let json = #"""
        {"by_model":[
          {"avg_latency_ms":2631.5,"billing_bucket":"subscription","cached_input_tokens":69632,"cost_usd":0.0506865,"errors":1,"input_tokens":93498,"output_tokens":1215,"requests":29,"routed_model":"gpt-5.5","upstream_provider":"openai"},
          {"avg_latency_ms":null,"billing_bucket":null,"cached_input_tokens":0,"cost_usd":0.0,"errors":12,"input_tokens":0,"output_tokens":0,"requests":12,"routed_model":"grok-code-fast-1","upstream_provider":"xai"}
        ],"since_ms":1783473855977,"totals":{"cost_by_bucket":{"subscription":0.10335838,"unknown":0.0},"cost_usd":0.10335838,"errors":13,"requests":56}}
        """#
        let analytics = try decode(json, as: Analytics.self)
        #expect(analytics.totals.requests == 56)
        #expect(analytics.byModel.count == 2)
        #expect(analytics.byModel[1].avgLatencyMs == nil)
    }

    @Test func dario() throws {
        let json = #"""
        {"active_generation_id":"gen-4.8.139-61993","should_be_healthy":true,"issue":{"code":"node_missing","message":"cannot find Node runtime","fixable":true},"resolved_node_bin":"/opt/homebrew/bin/node","resolved_claude_bin":"/usr/local/bin/claude","runtime_version":"v22.14.0","route_enabled":true,"prompt_caches":[{"key":"cache-1","model":"claude-sonnet-4-5"}],"generations":[{"consecutive_failures":0,"drain_started_at":null,"id":"gen-4.8.139-61993","in_flight":0,"last_activity_ms":1783476977580,"last_probe":{"at_ms":1783477427269,"error":null,"latency_ms":1410,"ok":true,"status":null},"phase":"ready","pid":80392,"port":61993,"promoted_at":1783476975856,"started_at":1783476973704,"state":"active","stderr_log":"/x.log","stdout_log":"/y.log","version":"4.8.139"}]}
        """#
        let dario = try decode(json, as: DarioStatus.self)
        #expect(dario.activeGenerationId == "gen-4.8.139-61993")
        #expect(dario.generations[0].phase == "ready")
        #expect(dario.generations[0].lastProbe?.ok == true)
        #expect(dario.shouldBeHealthy == true)
        #expect(dario.issue?.code == "node_missing")
        #expect(dario.issue?.fixable == true)
        #expect(dario.resolvedNodeBin == "/opt/homebrew/bin/node")
        #expect(dario.resolvedClaudeBin == "/usr/local/bin/claude")
        #expect(dario.runtimeVersion == "v22.14.0")
        #expect(dario.routeEnabled == true)
        #expect(dario.promptCaches?.first?.model == "claude-sonnet-4-5")
    }

    @Test func daemonUpdateStatus() throws {
        let json = #"""
        {"current":"0.1.0","latest":"0.2.0","update_available":true,"notes_url":"https://example.test/notes","checked_at_ms":1783477427269}
        """#
        let update = try decode(json, as: DaemonUpdateStatus.self)
        #expect(update.current == "0.1.0")
        #expect(update.latest == "0.2.0")
        #expect(update.updateAvailable)
        #expect(update.notesUrl == "https://example.test/notes")
        #expect(update.checkedAtMs == 1783477427269)
    }

    @Test func daemonUpdateApplyResponse() throws {
        let json = #"""
        {"applying":false,"current":"0.1.0","latest":"0.2.0","update_available":true,"reason":"alex is managed by Homebrew - run `brew upgrade alex`"}
        """#
        let update = try decode(json, as: DaemonUpdateApplyResponse.self)
        #expect(!update.applying)
        #expect(update.updateAvailable == true)
        #expect(update.reason == "alex is managed by Homebrew - run `brew upgrade alex`")
    }

    @Test func harnesses() throws {
        let json = #"""
        {"harnesses":[
          {"name":"pi","installed":true,"binary":"/opt/alex/pi","version":"0.80.3","version_warning":null,"config_dir":"/Users/x/.pi/agent","config_dir_exists":true,"connected":true,"supports_connect":true,"override":{"binary":null,"config_dir":null},"daemon_reachable":true,"extra":"ignored"},
          {"name":"codex","installed":true,"binary":"/opt/alex/codex","version":"0.144.3","version_warning":null,"config_dir":"/Users/x/.codex","config_dir_exists":true,"connected":true,"supports_connect":true,"override":{"binary":"/tmp/codex","config_dir":null},"daemon_reachable":true,"default_route":"alex","backup_path":"/Users/x/.codex/alexandria-original-config.toml"}
        ],"extra":"ignored"}
        """#
        let harnesses = try decode(json, as: HarnessesResponse.self).harnesses
        #expect(harnesses.count == 2)
        #expect(harnesses[0].name == "pi")
        #expect(harnesses[0].versionWarning == nil)
        #expect(harnesses[0].override?.binary == nil)
        #expect(harnesses[0].connected)
        #expect(harnesses[1].versionWarning == nil)
        #expect(harnesses[1].override?.binary == "/tmp/codex")
        #expect(harnesses[1].configDirExists)
        #expect(harnesses[1].defaultRoute == "alex")
        #expect(harnesses[1].backupPath?.hasSuffix("alexandria-original-config.toml") == true)
    }

    @Test func harnessRefreshConfigResponse() throws {
        let json = #"""
        {"refreshed":true,"path":"/Users/x/.pi/agent/models.json","models_total":28,"added":["alex/claude-fable-5"],"removed":[],"unchanged":27,"key":"reused","base_url":"http://127.0.0.1:4100","description":"Alexandria adds alex/* models."}
        """#
        let response = try decode(json, as: HarnessConfigWriteResponse.self)
        #expect(response.refreshed == true)
        #expect(response.modelsTotal == 28)
        #expect(response.models == 28)
        #expect(response.added == ["alex/claude-fable-5"])
        #expect(response.removed.isEmpty)
        #expect(response.unchanged == 27)
        #expect(response.key == "reused")
        #expect(response.path.hasSuffix("models.json"))
        #expect(response.baseUrl == "http://127.0.0.1:4100")
        #expect(response.description == "Alexandria adds alex/* models.")
    }

    @Test func harnessConnectConfigWriteResponse() throws {
        let json = #"""
        {"path":"/tmp/pi/models.json","models_total":12,"added":["alex/a","alex/b"],"removed":["alex/z"],"unchanged":10,"key":"minted","base_url":"http://127.0.0.1:4100","key_id":"rk-abc"}
        """#
        let response = try decode(json, as: HarnessConfigWriteResponse.self)
        #expect(response.refreshed == nil)
        #expect(response.modelsTotal == 12)
        #expect(response.key == "minted")
        #expect(response.keyId == "rk-abc")
        #expect(response.added.count == 2)
        #expect(response.removed == ["alex/z"])
    }

    @Test func harnessPlanResponse() throws {
        let json = #"""
        {"plan":[
          {"path":"/Users/x/.pi/agent/models.json","action":"create","detail":"add provider 'alexandria' with 28 models"},
          {"path":"run-keys","action":"create","detail":"mint harness key"}
        ]}
        """#
        let response = try decode(json, as: HarnessPlanResponse.self)
        #expect(response.plan.count == 2)
        #expect(response.plan[0].action == "create")
        #expect(response.plan[0].path.hasSuffix("models.json"))
        #expect(response.plan[1].detail == "mint harness key")
    }

    @Test func harnessDisconnectResponse() throws {
        let json = #"""
        {"path":"/Users/x/.pi/agent/models.json","models_total":0,"added":[],"removed":["alex/a"],"unchanged":0,"key":"revoked","base_url":"http://127.0.0.1:4100","revoked":1,"was_connected":true}
        """#
        let response = try decode(json, as: HarnessDisconnectResponse.self)
        #expect(response.wasConnected)
        #expect(response.revoked == 1)
        #expect(response.key == "revoked")
        #expect(response.removed == ["alex/a"])
        #expect(response.path.hasSuffix("models.json"))
    }

    @Test func harnessCatalogRows() {
        let rows = HarnessCatalog.rows([
            Harness(
                name: "codex", installed: true, binary: "/bin/codex", version: "1.0",
                versionWarning: nil, configDir: nil, configDirExists: false,
                connected: false, supportsConnect: true, override: nil, daemonReachable: true),
        ])
        #expect(Array(rows.map(\.name).prefix(7)) == ["pi", "claude", "codex", "grok", "amp", "gemini", "opencode"])
        #expect(rows[2].installed)
        #expect(!rows[0].installed)
        #expect(HarnessCatalog.displayName("opencode") == "OpenCode")
        #expect(HarnessCatalog.displayName("amp") == "Amp")
    }

    @Test func harnessRefreshTargetsFiltersConnectedSupport() {
        let harnesses = [
            Harness(
                name: "pi", installed: true, binary: "/bin/pi", version: "1",
                versionWarning: nil, configDir: "/tmp/pi", configDirExists: true,
                connected: true, supportsConnect: true, override: nil, daemonReachable: true),
            Harness(
                name: "codex", installed: true, binary: "/bin/codex", version: "1",
                versionWarning: nil, configDir: nil, configDirExists: false,
                connected: false, supportsConnect: true, override: nil, daemonReachable: true),
            Harness(
                name: "claude", installed: true, binary: "/bin/claude", version: "1",
                versionWarning: nil, configDir: nil, configDirExists: true,
                connected: true, supportsConnect: false, override: nil, daemonReachable: true),
            Harness(
                name: "future", installed: true, binary: "/bin/future", version: "1",
                versionWarning: nil, configDir: "/tmp/f", configDirExists: true,
                connected: true, supportsConnect: true, override: nil, daemonReachable: true),
        ]
        let targets = HarnessCatalog.refreshTargets(harnesses)
        #expect(targets.map(\.name) == ["pi", "future"])
        #expect(HarnessCatalog.refreshTargets([]).isEmpty)
    }

    @Test func configToml() {
        let toml = """
        # Alexandria config
        host = "127.0.0.1"
        port = 4100
        local_key = "alx-abc123"
        heartbeat_minutes = 15
        """
        let config = DaemonDiscovery.parse(toml: toml)
        #expect(config == DaemonConfig(host: "127.0.0.1", port: 4100, localKey: "alx-abc123"))
        #expect(config?.baseURL.absoluteString == "http://127.0.0.1:4100")
        #expect(DaemonDiscovery.parse(toml: "port = 4100") == nil)

        let lan = DaemonConfig(host: "0.0.0.0", port: 4100, localKey: "alx-lan")
        #expect(lan.lanEnabled)
        #expect(lan.baseURL.absoluteString == "http://127.0.0.1:4100")
        let ipv6 = DaemonConfig(host: "::1", port: 4100, localKey: "alx-v6")
        #expect(!ipv6.lanEnabled)
        #expect(ipv6.baseURL.absoluteString == "http://[::1]:4100")
    }

    @Test func daemonBoundToTailscaleStillConnectsLocallyOverLoopback() {
        let config = DaemonConfig(host: "100.101.102.103", port: 4100, localKey: "alx-abc123")
        #expect(config.connectHost == "127.0.0.1")
        #expect(config.baseURL.absoluteString == "http://127.0.0.1:4100")
        #expect(NetworkInterfaces.friendlyName("utun4", address: "100.101.102.103") == "Tailscale")
    }

    @Test func providerMapping() {
        #expect(ProviderInfo.displayName("anthropic") == "Claude")
        #expect(ProviderInfo.loginArg("xai") == "grok")
        #expect(ProviderInfo.pingArg("xai") == "grok")
        #expect(ProviderInfo.pingArg("gemini") == "gemini")
        #expect(ProviderInfo.pingArg("unknown") == nil)
    }

    @Test func openRouterProviderMetadata() {
        #expect(ProviderInfo.displayName("openrouter") == "OpenRouter")
        #expect(ProviderInfo.loginArg("openrouter") == "openrouter")
        #expect(ProviderInfo.pingArg("openrouter") == "openrouter")
        #expect(ProviderInfo.usesAPIKeySheet("openrouter"))
    }

    @Test func routingReserveResolutionAndDisplay() {
        #expect(RoutingReserve.resolved(account: nil, provider: 10) == 10)
        #expect(RoutingReserve.resolved(account: 25, provider: 10) == 25)
        #expect(RoutingReserve.resolved(account: -1, provider: 10) == 0)
        #expect(RoutingReserve.display(0) == "0% (never block)")
        #expect(RoutingReserve.display(15) == "15% remaining")
    }

    @Test func formatDuration() {
        #expect(Format.duration(27538) == "7h 38m")
        #expect(Format.duration(-37906) == "10h 31m")
        #expect(Format.duration(45) == "45s")
        #expect(Format.duration(90061) == "1d 1h")
    }

    @Test func traceRowsDecodeEffortAndThinking() throws {
        let sessionJson = #"""
        {"session_id":"s1","first_ts_ms":1000,"last_ts_ms":253000,"trace_count":2,"efforts":["high","minimal"]}
        """#
        let session = try decode(sessionJson, as: TraceSession.self)
        #expect(session.efforts == ["high", "minimal"])

        let transcriptJson = #"""
        {"session_id":"s1","turns":[{"trace_id":"t1","ts_request_ms":1000,"ts_response_ms":2000,"reasoning_effort":"high","thinking_budget":16000}]}
        """#
        let turn = try decode(transcriptJson, as: TranscriptResponse.self).turns[0]
        #expect(turn.reasoningEffort == "high")
        #expect(turn.thinkingBudget == 16_000)

        let traceJson = #"""
        {"trace":{"id":"t1","reasoning_effort":"minimal","thinking_budget":4096},"extras":null}
        """#
        let detail = try decode(traceJson, as: TraceDetailResponse.self).trace
        #expect(detail.reasoningEffort == "minimal")
        #expect(detail.thinkingBudget == 4096)
    }
}
