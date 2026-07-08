import Foundation
import Testing
@testable import AlexandriaBarCore

@Suite struct TraceBrowserLogicTests {
    func decode<T: Decodable>(_ json: String, as type: T.Type) throws -> T {
        try JSONDecoder().decode(T.self, from: Data(json.utf8))
    }

    @Test func omniQueryTokens() {
        let q = OmniQuery.parse(
            "auth failed model:grok provider:xai harness:claude status:401 run:r-1 session:abc")
        #expect(q.freeText == "auth failed")
        #expect(q.model == "grok")
        #expect(q.provider == "xai")
        #expect(q.harness == "claude")
        #expect(q.status == "401")
        #expect(q.run == "r-1")
        #expect(q.session == "abc")
        #expect(q.hasTokenFilters)
        #expect(!q.isEmpty)
    }

    @Test func omniQueryFreeTextOnly() {
        let q = OmniQuery.parse("  hello   world  ")
        #expect(q.freeText == "hello world")
        #expect(!q.hasTokenFilters)
    }

    @Test func omniQueryEdgeCases() {
        #expect(OmniQuery.parse("").isEmpty)
        let unknownKey = OmniQuery.parse("http://example.com model:gpt")
        #expect(unknownKey.freeText == "http://example.com")
        #expect(unknownKey.model == "gpt")
        let emptyValue = OmniQuery.parse("model: foo")
        #expect(emptyValue.model == nil)
        #expect(emptyValue.freeText == "model: foo")
        let leadingColon = OmniQuery.parse(":weird")
        #expect(leadingColon.freeText == ":weird")
        let upperKey = OmniQuery.parse("MODEL:Grok")
        #expect(upperKey.model == "Grok")
    }

    @Test func omniQuerySessionMatching() {
        let session = makeSession(
            id: "sess-abc123", models: ["grok-code-fast-1"], harness: "claude-code",
            runId: "run-77", lastStatus: 401)
        #expect(OmniQuery.parse("model:grok").matches(session))
        #expect(!OmniQuery.parse("model:gpt").matches(session))
        #expect(OmniQuery.parse("harness:claude").matches(session))
        #expect(OmniQuery.parse("session:abc").matches(session))
        #expect(OmniQuery.parse("run:77").matches(session))
        #expect(OmniQuery.parse("status:401").matches(session))
        #expect(!OmniQuery.parse("status:200").matches(session))
        #expect(OmniQuery.parse("free text only").matches(session))
        #expect(OmniQuery.parse("model:grok status:401 session:sess").matches(session))
        #expect(!OmniQuery.parse("model:grok status:500").matches(session))
    }

    @Test func pingClassification() {
        #expect(SessionKind.isPingOrTest(sessionId: "auto-1", harness: "alexandria-ping"))
        #expect(SessionKind.isPingOrTest(sessionId: "auto-1", harness: "x/alexandria-ping/1.0"))
        #expect(SessionKind.isPingOrTest(sessionId: "tsh-42", harness: nil))
        #expect(SessionKind.isPingOrTest(sessionId: "alexandria-e2e-run", harness: "claude-code"))
        #expect(SessionKind.isPingOrTest(sessionId: "smoke-9", harness: nil))
        #expect(!SessionKind.isPingOrTest(sessionId: "auto-1", harness: "claude-code"))
        #expect(!SessionKind.isPingOrTest(sessionId: "my-smoke-9", harness: nil))
        #expect(!SessionKind.isPingOrTest(sessionId: "real-session", harness: nil))
    }

    @Test func liveSwitchDecision() {
        #expect(LiveFollow.shouldSwitch(
            pinned: false, currentIdleMs: 25_000, userAtBottom: true, awayFromBottomMs: 0))
        #expect(!LiveFollow.shouldSwitch(
            pinned: true, currentIdleMs: 25_000, userAtBottom: true, awayFromBottomMs: 0))
        #expect(!LiveFollow.shouldSwitch(
            pinned: false, currentIdleMs: 10_000, userAtBottom: true, awayFromBottomMs: 0))
        #expect(!LiveFollow.shouldSwitch(
            pinned: false, currentIdleMs: 20_000, userAtBottom: true, awayFromBottomMs: 0))
        #expect(!LiveFollow.shouldSwitch(
            pinned: false, currentIdleMs: 25_000, userAtBottom: false, awayFromBottomMs: 30_000))
        #expect(LiveFollow.shouldSwitch(
            pinned: false, currentIdleMs: 25_000, userAtBottom: false, awayFromBottomMs: 61_000))
        #expect(!LiveFollow.shouldSwitch(
            pinned: false, currentIdleMs: 5_000, userAtBottom: false, awayFromBottomMs: 61_000))
    }

    @Test func sessionsDecoding() throws {
        let json = #"""
        {"sessions":[{"errors":0,"first_ts_ms":1783484392318,"harness":"alexandria-ping","last_status":200,"last_ts_ms":1783484841250,"models":["grok-code-fast-1"],"run_id":null,"session_id":"auto-36237cced1dcc659","tags":{},"total_cost_usd":0.00005262,"total_input_tokens":426,"total_output_tokens":9,"trace_count":3}]}
        """#
        let sessions = try decode(json, as: TraceSessionsResponse.self).sessions
        #expect(sessions.count == 1)
        #expect(sessions[0].sessionId == "auto-36237cced1dcc659")
        #expect(sessions[0].traceCount == 3)
        #expect(sessions[0].models == ["grok-code-fast-1"])
        #expect(sessions[0].lastStatus == 200)
        #expect(sessions[0].isPingOrTest)
    }

    @Test func transcriptDecoding() throws {
        let json = #"""
        {"session_id":"auto-1","turns":[{"assistant":"creds ok","cost_usd":0.0000214,"error":null,"input_tokens":142,"model":"grok-code-fast-1","output_tokens":3,"status":200,"trace_id":"3290c574","ts_request_ms":1783484392318,"ts_response_ms":1783484394631,"user":"hello"},{"assistant":null,"cost_usd":null,"error":"upstream 429","input_tokens":null,"model":null,"output_tokens":null,"status":429,"trace_id":"deadbeef","ts_request_ms":1783484392400,"ts_response_ms":null,"user":null}]}
        """#
        let transcript = try decode(json, as: TranscriptResponse.self)
        #expect(transcript.sessionId == "auto-1")
        #expect(transcript.turns.count == 2)
        #expect(transcript.turns[0].assistant == "creds ok")
        #expect(transcript.turns[0].status == 200)
        #expect(transcript.turns[1].error == "upstream 429")
        #expect(transcript.turns[1].model == nil)
    }

    @Test func searchDecoding() throws {
        let json = #"""
        {"scan_cap":300,"scanned":300,"traces":[{"id":"42f56581","session_id":"auto-1","status":200},{"id":"aa","session_id":null}]}
        """#
        let resp = try decode(json, as: TraceSearchResponse.self)
        #expect(resp.scanned == 300)
        #expect(resp.traces.count == 2)
        #expect(resp.traces[0].sessionId == "auto-1")
        #expect(resp.traces[1].sessionId == nil)
    }

    private func makeSession(
        id: String, models: [String]?, harness: String?, runId: String?, lastStatus: Int?
    ) -> TraceSession {
        let json: [String: Any] = [
            "session_id": id,
            "run_id": runId as Any,
            "first_ts_ms": 0,
            "last_ts_ms": 0,
            "trace_count": 1,
            "models": models as Any,
            "harness": harness as Any,
            "last_status": lastStatus as Any,
        ]
        let data = try! JSONSerialization.data(withJSONObject: json)
        return try! JSONDecoder().decode(TraceSession.self, from: data)
    }
}
