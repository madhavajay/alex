import Foundation
import Testing
@testable import AlexCore

@Suite struct TraceBrowserLogicTests {
    func decode<T: Decodable>(_ json: String, as type: T.Type) throws -> T {
        try JSONDecoder().decode(T.self, from: Data(json.utf8))
    }

    @Test func omniQueryTokens() {
        let q = OmniQuery.parse(
            "auth failed model:grok provider:xai harness:claude status:401 run:r-1 session:abc middleware:rule.fallback")
        #expect(q.freeText == "auth failed")
        #expect(q.model == "grok")
        #expect(q.provider == "xai")
        #expect(q.harness == "claude")
        #expect(q.status == "401")
        #expect(q.run == "r-1")
        #expect(q.session == "abc")
        #expect(q.middleware == "rule.fallback")
        let effort = OmniQuery.parse("effort:high duration:5m")
        #expect(effort.effort == "high")
        #expect(effort.duration == "5m")
        let account = OmniQuery.parse("account:openai-oauth-acct-2")
        #expect(account.account == "openai-oauth-acct-2")
        let key = OmniQuery.parse("key:5effb978eb304b0b")
        #expect(key.key == "5effb978eb304b0b")
        #expect(key.freeText.isEmpty)
        #expect(key.hasTokenFilters)
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

    @Test func onboardingHarnessMatchAcceptsVersionedKimiUserAgent() {
        let kimi = makeSession(
            id: "auto-456d7766a9aa8ea2", models: ["gpt-5.6-sol"],
            harness: "kimi-code-cli/0.27.0", runId: nil, lastStatus: 200,
            tags: ["harness": "kimi"])
        #expect(OnboardingSupport.traceMatchesHarness(kimi, harness: "kimi"))
        #expect(!OnboardingSupport.traceMatchesHarness(kimi, harness: "codex"))
        #expect(OnboardingSupport.traceMatchesHarness(kimi, harness: nil))
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
                lastStatus: nil, tags: ["model": "gpt-5.5", "task": "algebra"],
                efforts: ["minimal"], accountIds: ["openai-oauth-b"]),
            makeSession(id: "s3", models: nil, harness: nil, runId: nil, lastStatus: nil),
        ]
        #expect(TagFilterDimension.harness.values(in: sessions) == ["codex", "codex-cli"])
        #expect(TagFilterDimension.task.values(in: sessions) == ["algebra", "sparql-university"])
        #expect(TagFilterDimension.job.values(in: sessions) == ["job-42"])
        #expect(TagFilterDimension.model.values(in: sessions) == ["gpt-5.5", "grok-code-fast-1"])
        #expect(TagFilterDimension.effort.values(in: sessions) == ["minimal"])
        #expect(TagFilterDimension.account.values(in: sessions) == ["openai-oauth-b"])
        #expect(TagFilterDimension.duration.values(in: sessions) == ["1m", "5m", "15m", "1h"])
        #expect(TagFilterDimension.harness.activeValue(in: OmniQuery.parse("harness:codex")) == "codex")
        #expect(TagFilterDimension.task.activeValue(in: OmniQuery.parse("job:j")) == nil)
        #expect(
            TagFilterDimension.account.activeValue(
                in: OmniQuery.parse("account:openai-oauth-b")) == "openai-oauth-b")
        #expect(
            TagFilterDimension.middleware.activeValue(
                in: OmniQuery.parse("middleware:rule.fallback")) == "rule.fallback")
    }

    @Test func multiModelFilterPipeline() {
        let multi = makeSession(
            id: "s-multi", models: ["claude-haiku-4-5", "gpt-5.5"], harness: "codex-cli",
            runId: nil, lastStatus: 200, lastTsMs: 2000,
            accountIds: ["openai-oauth-a"])
        let other = makeSession(
            id: "s-grok", models: ["grok-code-fast-1"], harness: "pi/0.1", runId: nil,
            lastStatus: 500, tags: ["task": "algebra"], lastTsMs: 1000)
        let sessions = [multi, other]
        let rowsById = SessionTable.rowsById(sessions)
        func visible(_ raw: String) -> [String] {
            SessionTable.visibleRows(
                sessions: sessions, rowsById: rowsById, showPings: true,
                query: OmniQuery.parse(raw), serverMatches: nil,
                sortOrder: SessionTable.defaultSortOrder()
            ).map(\.id)
        }
        #expect(visible("") == ["s-multi", "s-grok"])
        #expect(visible("model:gpt-5.5") == ["s-multi"])
        #expect(visible("model:claude-haiku-4-5") == ["s-multi"])
        #expect(visible("model:GPT-5.5") == ["s-multi"])
        #expect(visible("model:grok") == ["s-grok"])
        #expect(visible("model:nonexistent").isEmpty)
        #expect(visible("harness:pi") == ["s-grok"])
        #expect(visible("harness:codex") == ["s-multi"])
        #expect(visible("task:algebra") == ["s-grok"])
        #expect(visible("status:200") == ["s-multi"])
        #expect(visible("status:500") == ["s-grok"])
        #expect(visible("account:openai-oauth-a") == ["s-multi"])
        #expect(visible("account:openai-oauth-b").isEmpty)
    }

    @Test func effortAndDurationFilters() {
        let high = makeSession(
            id: "s-high", models: nil, harness: nil, runId: nil, lastStatus: nil,
            firstTsMs: 0, lastTsMs: 5 * 60_000, efforts: ["high"])
        let old = makeSession(
            id: "s-old", models: nil, harness: nil, runId: nil, lastStatus: nil,
            firstTsMs: 0, lastTsMs: 45_000)
        #expect(OmniQuery.parse("effort:high").matches(high))
        #expect(!OmniQuery.parse("effort:high").matches(old))
        #expect(OmniQuery.parse("duration:5m").matches(high))
        #expect(!OmniQuery.parse("duration:1m").matches(old))

        let json = #"""
        {"session_id":"s-high","turns":[{"trace_id":"a","ts_request_ms":0,"reasoning_effort":"high"},{"trace_id":"b","ts_request_ms":1,"reasoning_effort":null,"thinking_budget":16000}]}
        """#
        let turns = try! decode(json, as: TranscriptResponse.self).turns
        #expect(OmniQuery.parse("effort:high").matches(turns[0]))
        #expect(!OmniQuery.parse("effort:high").matches(turns[1]))
    }

    @Test func accountFilterMatchesIndividualTurns() throws {
        let json = #"""
        {"session_id":"s","turns":[{"trace_id":"a","ts_request_ms":0,"account_id":"openai-oauth-a"},{"trace_id":"b","ts_request_ms":1,"account_id":"openai-oauth-b"}]}
        """#
        let turns = try decode(json, as: TranscriptResponse.self).turns
        let query = OmniQuery.parse("account:openai-oauth-b")
        #expect(!query.matches(turns[0]))
        #expect(query.matches(turns[1]))
    }

    @Test func accountIdentityPrefersEmailAndKeepsExactId() throws {
        let json = #"""
        {"id":"openai-oauth-a","provider":"openai","name":"acct-a","kind":"oauth","label":"Personal","description":"Codex account","email":"person@example.com","paused":false,"status":"active","expires_at_ms":null,"expires_in_s":100}
        """#
        let account = try decode(json, as: Account.self)
        #expect(
            AccountIdentity.name(accountId: account.id, accounts: [account])
                == "person@example.com")
        #expect(
            AccountIdentity.label(accountId: account.id, accounts: [account])
                == "person@example.com · openai-oauth-a")
        #expect(
            AccountIdentity.summary(
                accountIds: [account.id, account.id, "openai-oauth-b"], accounts: [account])
                == "person@example.com · openai-oauth-a, openai-oauth-b")
    }

    @Test func modelDropdownSplitsJoinedValues() {
        let joinedTag = makeSession(
            id: "s-tag", models: nil, harness: nil, runId: nil, lastStatus: nil,
            tags: ["model": "claude-haiku-4-5, gpt-5.5"])
        #expect(TagFilterDimension.model.values(in: [joinedTag])
            == ["claude-haiku-4-5", "gpt-5.5"])
        let multi = makeSession(
            id: "s-multi", models: ["claude-haiku-4-5", "gpt-5.5"], harness: nil,
            runId: nil, lastStatus: nil)
        #expect(TagFilterDimension.model.values(in: [multi])
            == ["claude-haiku-4-5", "gpt-5.5"])
        let token = OmniQuery.settingToken(in: "", key: "model", value: "claude-haiku-4-5, gpt-5.5")
        #expect(token == "model:claude-haiku-4-5")
        let parsed = OmniQuery.parse(token)
        #expect(parsed.model == "claude-haiku-4-5")
        #expect(parsed.freeText.isEmpty)
        #expect(OmniQuery.settingToken(in: "free", key: "model", value: "  ") == "free")
        #expect(parsed.matches(multi))
    }

    @Test func pingClassification() {
        #expect(SessionKind.isPingOrTest(sessionId: "auto-1", harness: "alex-ping"))
        #expect(SessionKind.isPingOrTest(sessionId: "auto-1", harness: "x/alex-ping/1.0"))
        #expect(SessionKind.isPingOrTest(sessionId: "tsh-42", harness: nil))
        #expect(SessionKind.isPingOrTest(sessionId: "alex-e2e-run", harness: "claude-code"))
        #expect(SessionKind.isPingOrTest(sessionId: "smoke-9", harness: nil))
        #expect(!SessionKind.isPingOrTest(sessionId: "auto-1", harness: "claude-code"))
        #expect(!SessionKind.isPingOrTest(sessionId: "my-smoke-9", harness: nil))
        #expect(!SessionKind.isPingOrTest(sessionId: "real-session", harness: nil))
    }

    @Test func sessionsDecoding() throws {
        let json = #"""
        {"sessions":[{"account_ids":["openai-oauth-a","openai-oauth-b"],"efforts":["minimal"],"errors":0,"first_ts_ms":1783484392318,"harness":"alex-ping","last_status":200,"last_ts_ms":1783484841250,"models":["grok-code-fast-1"],"run_id":null,"session_id":"auto-36237cced1dcc659","tags":{},"total_cost_usd":0.00005262,"total_input_tokens":426,"total_output_tokens":9,"trace_count":3}]}
        """#
        let sessions = try decode(json, as: TraceSessionsResponse.self).sessions
        #expect(sessions.count == 1)
        #expect(sessions[0].sessionId == "auto-36237cced1dcc659")
        #expect(sessions[0].traceCount == 3)
        #expect(sessions[0].models == ["grok-code-fast-1"])
        #expect(sessions[0].efforts == ["minimal"])
        #expect(sessions[0].lastStatus == 200)
        #expect(sessions[0].accountIds == ["openai-oauth-a", "openai-oauth-b"])
        #expect(sessions[0].isPingOrTest)
    }

    @Test func transcriptDecoding() throws {
        let json = #"""
        {"session_id":"auto-1","turns":[{"assistant":"creds ok","cost_usd":0.0000214,"error":null,"input_tokens":142,"model":"grok-code-fast-1","output_tokens":3,"reasoning_effort":"high","status":200,"thinking_budget":null,"trace_id":"3290c574","ts_request_ms":1783484392318,"ts_response_ms":1783484394631,"user":"hello"},{"assistant":null,"cost_usd":null,"error":"upstream 429","input_tokens":null,"model":null,"output_tokens":null,"status":429,"thinking_budget":16000,"trace_id":"deadbeef","ts_request_ms":1783484392400,"ts_response_ms":null,"user":null}]}
        """#
        let transcript = try decode(json, as: TranscriptResponse.self)
        #expect(transcript.sessionId == "auto-1")
        #expect(transcript.turns.count == 2)
        #expect(transcript.turns[0].assistant == "creds ok")
        #expect(transcript.turns[0].status == 200)
        #expect(transcript.turns[0].reasoningEffort == "high")
        #expect(transcript.turns[1].error == "upstream 429")
        #expect(transcript.turns[1].model == nil)
        #expect(transcript.turns[1].thinkingBudget == 16_000)
    }

    @Test func transcriptDecodesInlineMiddlewareAttemptEvents() throws {
        let json = #"""
        {"session_id":"s","turns":[{"trace_id":"t","ts_request_ms":1,"ts_response_ms":2,"model":"gpt-5.6-sol","provider":"openai","status":200,"substituted":true,"substitution_reason":"fallback","attempts":[{"provider":"anthropic","model":"claude-fable-5","status":200,"error":{"class":"other","kind":"upstream_refusal","code":"bio","message":null},"middleware_decisions":[{"rule_id":"alex.fable","rule_name":"Fable fallback","state":"matched","action":"reroute","executed":true,"suppressed":false,"explanation":"selected openai/gpt-5.6-sol"}]},{"provider":"openai","model":"gpt-5.6-sol","status":200,"middleware_decisions":[]}]}]}
        """#
        let turn = try decode(json, as: TranscriptResponse.self).turns[0]
        #expect(turn.hasInlineAttemptEvents)
        #expect(turn.attempts?.first?.error?.kind == "upstream_refusal")
        #expect(turn.attempts?.first?.middlewareDecisions?.first?.ruleName == "Fable fallback")
    }

    @Test func searchDecoding() throws {
        let json = #"""
        {"scan_cap":300,"scanned":300,"traces":[{"id":"42f56581","reasoning_effort":"minimal","session_id":"auto-1","status":200,"thinking_budget":null},{"id":"aa","session_id":null}]}
        """#
        let resp = try decode(json, as: TraceSearchResponse.self)
        #expect(resp.scanned == 300)
        #expect(resp.traces.count == 2)
        #expect(resp.traces[0].sessionId == "auto-1")
        #expect(resp.traces[0].reasoningEffort == "minimal")
        #expect(resp.traces[1].sessionId == nil)
    }

    @Test func logLineFormatting() {
        let ts = Date(timeIntervalSince1970: 1_700_000_000)
        let line = BarLog.formatLine(
            timestamp: ts, level: .warn, category: .net, message: "GET /health 200 12ms")
        #expect(line == "2023-11-14T22:13:20.000Z WARN [net] GET /health 200 12ms")
        let multiline = BarLog.formatLine(
            timestamp: ts, level: .error, category: .browser, message: "a\nb\r\nc")
        #expect(multiline == "2023-11-14T22:13:20.000Z ERROR [browser] a\\nb\\nc")
        let info = BarLog.formatLine(timestamp: ts, level: .info, category: .ui, message: "x")
        #expect(info.hasSuffix("INFO [ui] x"))
    }

    @Test func appLogUsesAlexStateDirectory() {
        let home = FileManager.default.temporaryDirectory
            .appendingPathComponent("alex-bar-log-\(UUID().uuidString)")
        #expect(BarLog.stateDirectory(home: home).lastPathComponent == ".alex")
    }

    @Test func logRotationDecision() {
        #expect(!BarLog.shouldRotate(fileBytes: 0))
        #expect(!BarLog.shouldRotate(fileBytes: BarLog.maxFileBytes))
        #expect(BarLog.shouldRotate(fileBytes: BarLog.maxFileBytes + 1))
        #expect(BarLog.shouldRotate(fileBytes: 10, limit: 9))
        #expect(!BarLog.shouldRotate(fileBytes: 9, limit: 9))
    }

    @Test func turnTextCappingChars() {
        let exact = String(repeating: "a", count: 4000)
        let atLimit = TurnTextCap.cap(exact)
        #expect(!atLimit.isTruncated)
        #expect(atLimit.text == exact)
        #expect(atLimit.fullCharCount == 4000)
        let over = TurnTextCap.cap(exact + "b")
        #expect(over.isTruncated)
        #expect(over.text.count == 4000)
        #expect(over.fullCharCount == 4001)
        let empty = TurnTextCap.cap("")
        #expect(!empty.isTruncated)
        #expect(empty.fullCharCount == 0)
    }

    @Test func turnTextCappingLines() {
        let sixty = Array(repeating: "line", count: 60).joined(separator: "\n")
        let atLimit = TurnTextCap.cap(sixty)
        #expect(!atLimit.isTruncated)
        #expect(atLimit.text == sixty)
        let over = TurnTextCap.cap(sixty + "\nline61")
        #expect(over.isTruncated)
        #expect(over.text == sixty)
        let short = TurnTextCap.cap("one\ntwo", maxChars: 100, maxLines: 2)
        #expect(!short.isTruncated)
        let both = TurnTextCap.cap(String(repeating: "x\n", count: 5000))
        #expect(both.isTruncated)
        #expect(both.text.count <= 4000)
        #expect(both.text.split(separator: "\n", omittingEmptySubsequences: false).count <= 60)
    }

    @Test func sessionsFingerprintSkip() {
        let a = makeSession(id: "s1", models: nil, harness: nil, runId: nil, lastStatus: nil, lastTsMs: 100)
        let b = makeSession(id: "s2", models: nil, harness: nil, runId: nil, lastStatus: nil, lastTsMs: 200)
        #expect(TraceFingerprint.sessions([a, b]) == TraceFingerprint.sessions([a, b]))
        #expect(TraceFingerprint.sessions([a, b]) != TraceFingerprint.sessions([a]))
        #expect(TraceFingerprint.sessions([]) != TraceFingerprint.sessions([a]))
        let bNewer = makeSession(
            id: "s2", models: nil, harness: nil, runId: nil, lastStatus: nil, lastTsMs: 300)
        #expect(TraceFingerprint.sessions([a, b]) != TraceFingerprint.sessions([a, bNewer]))
        let bWithUsage = makeSession(
            id: "s2", models: nil, harness: nil, runId: nil, lastStatus: nil,
            lastTsMs: 200, traceCount: 2, totalInputTokens: 12, totalOutputTokens: 34,
            totalCostUsd: 0.001, errors: 1)
        #expect(TraceFingerprint.sessions([a, b]) != TraceFingerprint.sessions([a, bWithUsage]))
    }

    @Test func clientDisconnectsAreEventsNotErrors() {
        #expect(TraceClassification.isClientDisconnect(errorKind: "client_disconnect"))
        #expect(!TraceClassification.isError(
            status: 499, errorKind: "client_disconnect", error: "client went away"))
        #expect(TraceClassification.isError(
            status: 500, errorKind: "upstream_error", error: "failed"))
        #expect(TraceClassification.isError(
            status: 429, errorKind: nil, error: nil))

        let eventOnly = SessionRow(session: makeSession(
            id: "event", models: nil, harness: "pi", runId: nil, lastStatus: 499,
            errors: 1, errorClassCounts: ["client_disconnect": 1]))
        #expect(eventOnly.errors == 0)
        #expect(eventOnly.clientDisconnects == 1)

        let mixed = SessionRow(session: makeSession(
            id: "mixed", models: nil, harness: "pi", runId: nil, lastStatus: 500,
            errors: 3,
            errorClassCounts: ["client_disconnect": 2, "upstream_error": 1]))
        #expect(mixed.errors == 1)
        #expect(mixed.clientDisconnects == 2)
    }

    @Test func turnsFingerprintSkip() throws {
        let json = #"""
        {"session_id":"auto-1","turns":[{"assistant":"creds ok","cost_usd":0.0000214,"error":null,"input_tokens":142,"model":"grok-code-fast-1","output_tokens":3,"status":200,"trace_id":"3290c574","ts_request_ms":1783484392318,"ts_response_ms":1783484394631,"user":"hello"},{"assistant":null,"cost_usd":null,"error":"upstream 429","input_tokens":null,"model":null,"output_tokens":null,"status":429,"trace_id":"deadbeef","ts_request_ms":1783484392400,"ts_response_ms":null,"user":null}]}
        """#
        let turns = try decode(json, as: TranscriptResponse.self).turns
        #expect(TraceFingerprint.turns(turns) == TraceFingerprint.turns(turns))
        #expect(TraceFingerprint.turns(turns) != TraceFingerprint.turns(Array(turns.prefix(1))))
        #expect(TraceFingerprint.turns([]) != TraceFingerprint.turns(turns))
        #expect(TraceFingerprint.turns([]) == TraceFingerprint.turns([]))
        #expect(TraceFingerprint.turns([turns[0]]) != TraceFingerprint.turns([turns[1]]))

        let changedContentJson = #"""
        {"session_id":"auto-1","turns":[{"assistant":"creds ok, now complete","cost_usd":0.0000214,"error":null,"input_tokens":142,"model":"grok-code-fast-1","output_tokens":3,"status":200,"trace_id":"3290c574","ts_request_ms":1783484392318,"ts_response_ms":1783484394631,"user":"hello"},{"assistant":null,"cost_usd":null,"error":"upstream 429","input_tokens":null,"model":null,"output_tokens":null,"status":429,"trace_id":"deadbeef","ts_request_ms":1783484392400,"ts_response_ms":null,"user":null}]}
        """#
        let changedContent = try decode(changedContentJson, as: TranscriptResponse.self).turns
        #expect(TraceFingerprint.turns(turns) != TraceFingerprint.turns(changedContent))

        let changedToolsJson = #"""
        {"session_id":"auto-1","turns":[{"assistant":"creds ok","cost_usd":0.0000214,"error":null,"input_tokens":142,"model":"grok-code-fast-1","output_tokens":3,"status":200,"tool_calls":[{"name":"Shell","arguments":"{\"command\":\"git status\"}"}],"trace_id":"3290c574","ts_request_ms":1783484392318,"ts_response_ms":1783484394631,"user":"hello"},{"assistant":null,"cost_usd":null,"error":"upstream 429","input_tokens":null,"model":null,"output_tokens":null,"status":429,"trace_id":"deadbeef","ts_request_ms":1783484392400,"ts_response_ms":null,"user":null}]}
        """#
        let changedTools = try decode(changedToolsJson, as: TranscriptResponse.self).turns
        #expect(TraceFingerprint.turns(turns) != TraceFingerprint.turns(changedTools))
    }

    private func makeSession(
        id: String, models: [String]?, harness: String?, runId: String?, lastStatus: Int?,
        tags: [String: String]? = nil, firstTsMs: Int64 = 0, lastTsMs: Int64 = 0,
        efforts: [String]? = nil, accountIds: [String]? = nil, traceCount: Int = 1,
        totalInputTokens: Int64? = nil, totalOutputTokens: Int64? = nil,
        totalCostUsd: Double? = nil, errors: Int64? = nil,
        errorClassCounts: [String: Int64]? = nil
    ) -> TraceSession {
        let json: [String: Any] = [
            "session_id": id,
            "run_id": runId as Any,
            "first_ts_ms": firstTsMs,
            "last_ts_ms": lastTsMs,
            "trace_count": traceCount,
            "models": models as Any,
            "harness": harness as Any,
            "last_status": lastStatus as Any,
            "tags": tags as Any,
            "efforts": efforts as Any,
            "account_ids": accountIds as Any,
            "total_input_tokens": totalInputTokens as Any,
            "total_output_tokens": totalOutputTokens as Any,
            "total_cost_usd": totalCostUsd as Any,
            "errors": errors as Any,
            "error_class_counts": errorClassCounts as Any,
        ]
        let data = try! JSONSerialization.data(withJSONObject: json)
        return try! JSONDecoder().decode(TraceSession.self, from: data)
    }
}
