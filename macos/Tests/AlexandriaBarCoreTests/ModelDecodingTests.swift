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
          {"expires_at_ms":1783504994142,"expires_in_s":27538,"id":"anthropic-oauth","kind":"oauth","label":"claude-code (max)","provider":"anthropic","status":"active"},
          {"expires_at_ms":null,"expires_in_s":null,"id":"gemini-oauth","kind":"oauth","label":"gemini-cli","provider":"gemini","status":"active"},
          {"expires_at_ms":1783439548981,"expires_in_s":-37906,"id":"xai-oauth","kind":"oauth","label":"grok (me@madhavajay.com)","provider":"xai","status":"active"}
        ]}
        """#
        let accounts = try decode(json, as: AccountsResponse.self).accounts
        #expect(accounts.count == 3)
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
          {"observed_at_ms":1783477015654,"provider":"xai","requests":{"limit":120,"remaining":120},"source":"captured response headers","tokens":{"limit":5000000,"remaining":5000000}}
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
        {"active_generation_id":"gen-4.8.139-61993","generations":[{"consecutive_failures":0,"drain_started_at":null,"id":"gen-4.8.139-61993","in_flight":0,"last_activity_ms":1783476977580,"last_probe":{"at_ms":1783477427269,"error":null,"latency_ms":1410,"ok":true,"status":null},"phase":"ready","pid":80392,"port":61993,"promoted_at":1783476975856,"started_at":1783476973704,"state":"active","stderr_log":"/x.log","stdout_log":"/y.log","version":"4.8.139"}]}
        """#
        let dario = try decode(json, as: DarioStatus.self)
        #expect(dario.activeGenerationId == "gen-4.8.139-61993")
        #expect(dario.generations[0].phase == "ready")
        #expect(dario.generations[0].lastProbe?.ok == true)
    }

    @Test func harnesses() throws {
        let json = #"""
        {"harnesses":[
          {"name":"pi","installed":true,"binary":"/opt/alex/pi","version":"0.80.3","version_warning":null,"config_dir":"/Users/x/.pi/agent","config_dir_exists":true,"connected":true,"supports_connect":true,"override":{"binary":null,"config_dir":null},"daemon_reachable":true,"extra":"ignored"},
          {"name":"codex","installed":false,"binary":null,"version":null,"version_warning":"install codex >= 1.2","config_dir":null,"config_dir_exists":false,"connected":false,"supports_connect":true,"override":{"binary":"/tmp/codex","config_dir":null},"daemon_reachable":true}
        ],"extra":"ignored"}
        """#
        let harnesses = try decode(json, as: HarnessesResponse.self).harnesses
        #expect(harnesses.count == 2)
        #expect(harnesses[0].name == "pi")
        #expect(harnesses[0].versionWarning == nil)
        #expect(harnesses[0].override?.binary == nil)
        #expect(harnesses[0].connected)
        #expect(harnesses[1].versionWarning == "install codex >= 1.2")
        #expect(harnesses[1].override?.binary == "/tmp/codex")
        #expect(!harnesses[1].configDirExists)
    }

    @Test func harnessCatalogRows() {
        let rows = HarnessCatalog.rows([
            Harness(
                name: "codex", installed: true, binary: "/bin/codex", version: "1.0",
                versionWarning: nil, configDir: nil, configDirExists: false,
                connected: false, supportsConnect: true, override: nil, daemonReachable: true),
        ])
        #expect(Array(rows.map(\.name).prefix(6)) == ["pi", "claude", "codex", "gemini", "grok", "opencode"])
        #expect(rows[2].installed)
        #expect(!rows[0].installed)
        #expect(HarnessCatalog.displayName("opencode") == "OpenCode")
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
    }

    @Test func providerMapping() {
        #expect(ProviderInfo.displayName("anthropic") == "Claude")
        #expect(ProviderInfo.loginArg("xai") == "grok")
        #expect(ProviderInfo.pingArg("xai") == "grok")
        #expect(ProviderInfo.pingArg("gemini") == "gemini")
        #expect(ProviderInfo.pingArg("unknown") == nil)
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
