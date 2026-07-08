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

    @Test func omniQueryTagTokens() {
        let q = OmniQuery.parse("sparql task:sparql-university job:job-42 tag:harness=codex")
        #expect(q.freeText == "sparql")
        #expect(q.task == "sparql-university")
        #expect(q.job == "job-42")
        #expect(q.tag == "harness=codex")
        #expect(q.hasTokenFilters)
        let bare = OmniQuery.parse("tag:codex")
        #expect(bare.tag == "codex")
        #expect(bare.freeText.isEmpty)
        let upper = OmniQuery.parse("TASK:X JOB:y TAG:k=v")
        #expect(upper.task == "X")
        #expect(upper.job == "y")
        #expect(upper.tag == "k=v")
    }

    @Test func omniQueryTagMatching() {
        let session = makeSession(
            id: "sess-1", models: ["gpt-5.5"], harness: "codex-cli", runId: nil, lastStatus: 200,
            tags: [
                "task": "sparql-university", "job": "job-42",
                "harness": "codex", "model": "gpt-5.5-mini",
            ])
        #expect(OmniQuery.parse("task:sparql").matches(session))
        #expect(OmniQuery.parse("task:SPARQL-UNI").matches(session))
        #expect(!OmniQuery.parse("task:other").matches(session))
        #expect(OmniQuery.parse("job:42").matches(session))
        #expect(!OmniQuery.parse("job:43").matches(session))
        #expect(OmniQuery.parse("harness:codex-cli").matches(session))
        #expect(OmniQuery.parse("harness:codex").matches(session))
        #expect(OmniQuery.parse("model:gpt-5.5").matches(session))
        #expect(OmniQuery.parse("model:mini").matches(session))
        #expect(!OmniQuery.parse("model:grok").matches(session))
        #expect(OmniQuery.parse("tag:task=sparql").matches(session))
        #expect(OmniQuery.parse("tag:TASK=Sparql").matches(session))
        #expect(!OmniQuery.parse("tag:task=other").matches(session))
        #expect(!OmniQuery.parse("tag:missing=x").matches(session))
        #expect(OmniQuery.parse("tag:job-42").matches(session))
        #expect(!OmniQuery.parse("tag:nowhere").matches(session))
        #expect(OmniQuery.parse("tag:job=").matches(session))
    }

    @Test func harnessTagFallbackWithoutField() {
        let tagOnly = makeSession(
            id: "sess-2", models: nil, harness: nil, runId: nil, lastStatus: nil,
            tags: ["harness": "codex", "model": "gpt-5.5"])
        #expect(OmniQuery.parse("harness:codex").matches(tagOnly))
        #expect(!OmniQuery.parse("harness:claude").matches(tagOnly))
        #expect(OmniQuery.parse("model:gpt").matches(tagOnly))
        let untagged = makeSession(
            id: "sess-3", models: nil, harness: nil, runId: nil, lastStatus: nil)
        #expect(!OmniQuery.parse("harness:codex").matches(untagged))
        #expect(!OmniQuery.parse("task:x").matches(untagged))
        #expect(!OmniQuery.parse("tag:x").matches(untagged))
    }

    @Test func freeTextMatchesTagValues() {
        let tagged = makeSession(
            id: "sess-4", models: nil, harness: nil, runId: nil, lastStatus: nil,
            tags: ["task": "sparql-university"])
        let untagged = makeSession(
            id: "sess-5", models: nil, harness: nil, runId: nil, lastStatus: nil)
        let q = OmniQuery.parse("sparql")
        #expect(q.freeTextMatchesTags(tagged))
        #expect(!q.freeTextMatchesTags(untagged))
        #expect(q.isVisible(tagged, serverMatches: nil))
        #expect(!q.isVisible(untagged, serverMatches: nil))
        #expect(q.isVisible(untagged, serverMatches: ["sess-5"]))
        #expect(!q.isVisible(untagged, serverMatches: ["other"]))
        #expect(q.isVisible(tagged, serverMatches: []))
        let empty = OmniQuery.parse("")
        #expect(empty.isVisible(untagged, serverMatches: nil))
        let tokenMiss = OmniQuery.parse("sparql task:other")
        #expect(!tokenMiss.isVisible(tagged, serverMatches: nil))
    }

    @Test func chipSelection() {
        let chips = SessionTagChips.chips(
            tags: [
                "alpha": "1", "task": "sparql-university", "job": "job-42", "beta": "2",
            ],
            harness: nil, models: nil)
        #expect(chips.map(\.key) == ["task", "job", "alpha"])
        #expect(chips[0].label() == "task=sparql-university")
        #expect(chips[1].label() == "job=job-42")
    }

    @Test func chipDedupAndEdgeCases() {
        #expect(SessionTagChips.chips(tags: nil, harness: nil, models: nil).isEmpty)
        #expect(SessionTagChips.chips(tags: [:], harness: "codex", models: nil).isEmpty)
        let deduped = SessionTagChips.chips(
            tags: ["model": "gpt-5.5", "harness": "codex", "task": "t1"],
            harness: "codex", models: ["gpt-5.5"])
        #expect(deduped.map(\.key) == ["task"])
        let kept = SessionTagChips.chips(
            tags: ["model": "gpt-5.5-mini", "harness": "codex"],
            harness: "codex-cli", models: ["gpt-5.5"])
        #expect(kept.map(\.key) == ["harness", "model"])
        let emptyValue = SessionTagChips.chips(
            tags: ["task": "", "job": "j1"], harness: nil, models: nil)
        #expect(emptyValue.map(\.key) == ["job"])
    }

    @Test func chipLabelTruncation() {
        let chip = TagChip(key: "task", value: "a-very-long-task-name-indeed")
        #expect(chip.label() == "task=a-very-long-task-n…")
        #expect(chip.label(maxValueLength: 100) == "task=a-very-long-task-name-indeed")
        #expect(TagChip(key: "job", value: "exactly-eighteen-c").label() == "job=exactly-eighteen-c")
    }

    @Test func settingTokenComposition() {
        #expect(OmniQuery.settingToken(in: "", key: "task", value: "t1") == "task:t1")
        #expect(OmniQuery.settingToken(in: "sparql", key: "job", value: "j1") == "sparql job:j1")
        #expect(
            OmniQuery.settingToken(in: "sparql task:old job:j1", key: "task", value: "new")
                == "sparql job:j1 task:new")
        #expect(OmniQuery.settingToken(in: "sparql task:old", key: "task", value: nil) == "sparql")
        #expect(OmniQuery.settingToken(in: "TASK:old free", key: "task", value: "t2") == "free task:t2")
        #expect(OmniQuery.settingToken(in: "model:a", key: "model", value: nil) == "")
    }

    @Test func filterDimensionValues() {
        let sessions = [
            makeSession(
                id: "s1", models: ["gpt-5.5"], harness: "codex-cli", runId: nil, lastStatus: nil,
                tags: ["harness": "codex", "task": "sparql-university", "job": "job-42"]),
            makeSession(
                id: "s2", models: ["grok-code-fast-1"], harness: "codex-cli", runId: nil,
                lastStatus: nil, tags: ["model": "gpt-5.5", "task": "algebra"]),
            makeSession(id: "s3", models: nil, harness: nil, runId: nil, lastStatus: nil),
        ]
        #expect(TagFilterDimension.harness.values(in: sessions) == ["codex", "codex-cli"])
        #expect(TagFilterDimension.task.values(in: sessions) == ["algebra", "sparql-university"])
        #expect(TagFilterDimension.job.values(in: sessions) == ["job-42"])
        #expect(TagFilterDimension.model.values(in: sessions) == ["gpt-5.5", "grok-code-fast-1"])
        #expect(TagFilterDimension.harness.activeValue(in: OmniQuery.parse("harness:codex")) == "codex")
        #expect(TagFilterDimension.task.activeValue(in: OmniQuery.parse("job:j")) == nil)
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
        id: String, models: [String]?, harness: String?, runId: String?, lastStatus: Int?,
        tags: [String: String]? = nil
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
            "tags": tags as Any,
        ]
        let data = try! JSONSerialization.data(withJSONObject: json)
        return try! JSONDecoder().decode(TraceSession.self, from: data)
    }
}
