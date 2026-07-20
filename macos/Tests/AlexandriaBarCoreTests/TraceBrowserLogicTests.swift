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
        #expect(SessionKind.isPingOrTest(sessionId: "auto-1", harness: "alexandria-ping"))
        #expect(SessionKind.isPingOrTest(sessionId: "auto-1", harness: "x/alexandria-ping/1.0"))
        #expect(SessionKind.isPingOrTest(sessionId: "tsh-42", harness: nil))
        #expect(SessionKind.isPingOrTest(sessionId: "alexandria-e2e-run", harness: "claude-code"))
        #expect(SessionKind.isPingOrTest(sessionId: "smoke-9", harness: nil))
        #expect(!SessionKind.isPingOrTest(sessionId: "auto-1", harness: "claude-code"))
        #expect(!SessionKind.isPingOrTest(sessionId: "my-smoke-9", harness: nil))
        #expect(!SessionKind.isPingOrTest(sessionId: "real-session", harness: nil))
    }

    @Test func newerActivityPill() {
        #expect(LiveFollow.newerActivity(
            live: true, selectedId: "a", selectedLastTsMs: 100,
            newestId: "b", newestLastTsMs: 200))
        #expect(!LiveFollow.newerActivity(
            live: false, selectedId: "a", selectedLastTsMs: 100,
            newestId: "b", newestLastTsMs: 200))
        #expect(!LiveFollow.newerActivity(
            live: true, selectedId: "a", selectedLastTsMs: 200,
            newestId: "b", newestLastTsMs: 100))
        #expect(!LiveFollow.newerActivity(
            live: true, selectedId: "a", selectedLastTsMs: 100,
            newestId: "b", newestLastTsMs: 100))
        #expect(!LiveFollow.newerActivity(
            live: true, selectedId: "a", selectedLastTsMs: 100,
            newestId: "a", newestLastTsMs: 200))
        #expect(!LiveFollow.newerActivity(
            live: true, selectedId: nil, selectedLastTsMs: nil,
            newestId: "b", newestLastTsMs: 200))
        #expect(!LiveFollow.newerActivity(
            live: true, selectedId: "a", selectedLastTsMs: 100,
            newestId: nil, newestLastTsMs: nil))
        #expect(LiveFollow.newerActivity(
            live: true, selectedId: "a", selectedLastTsMs: nil,
            newestId: "b", newestLastTsMs: 1))
    }

    @Test func sessionsDecoding() throws {
        let json = #"""
        {"sessions":[{"account_ids":["openai-oauth-a","openai-oauth-b"],"efforts":["minimal"],"errors":0,"first_ts_ms":1783484392318,"harness":"alexandria-ping","last_status":200,"last_ts_ms":1783484841250,"models":["grok-code-fast-1"],"run_id":null,"session_id":"auto-36237cced1dcc659","tags":{},"total_cost_usd":0.00005262,"total_input_tokens":426,"total_output_tokens":9,"trace_count":3}]}
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
        {"body_byte_budget":16777216,"body_bytes_loaded":2048,"body_errors":[{"artifact_kind":"client_request","kind":"legacy_read","message":"bad gzip","trace_id":"deadbeef"}],"body_truncations":[{"artifact_kind":"client_response","budget_remaining_bytes":0,"reason":"page_body_byte_budget","total_bytes":4096,"trace_id":"deadbeef"}],"session_id":"auto-1","turns":[{"assistant":"creds ok","cost_usd":0.0000214,"error":null,"input_tokens":142,"model":"grok-code-fast-1","output_tokens":3,"reasoning_effort":"high","status":200,"thinking_budget":null,"trace_id":"3290c574","ts_request_ms":1783484392318,"ts_response_ms":1783484394631,"user":"hello"},{"assistant":null,"body_errors":[{"artifact_kind":"client_request","kind":"legacy_read","message":"bad gzip","trace_id":"deadbeef"}],"body_truncations":[{"artifact_kind":"client_response","budget_remaining_bytes":0,"reason":"page_body_byte_budget","total_bytes":4096,"trace_id":"deadbeef"}],"cost_usd":null,"error":"upstream 429","input_tokens":null,"model":null,"output_tokens":null,"status":429,"thinking_budget":16000,"trace_id":"deadbeef","ts_request_ms":1783484392400,"ts_response_ms":null,"user":null}]}
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
        #expect(transcript.bodyByteBudget == 16_777_216)
        #expect(transcript.bodyBytesLoaded == 2_048)
        #expect(transcript.bodyErrors?.first?.kind == "legacy_read")
        #expect(transcript.bodyTruncations?.first?.totalBytes == 4_096)
        #expect(transcript.turns[1].bodyErrors?.count == 1)
        #expect(transcript.turns[1].bodyTruncations?.count == 1)
    }

    @Test func transcriptArchiveAvailabilityIsTypedAndLegacyCompatible() throws {
        let current = try decode(
            #"{"session_id":"s","turns":[{"trace_id":"a","ts_request_ms":1,"body_errors":[{"trace_id":"a","artifact_kind":"client_request","kind":"archived_offline","message":"reattach","archive_availability":"archived_offline","archive_file_uuid":"abcd","archive_path":"cold/a.lar"},{"trace_id":"a","artifact_kind":"client_response","kind":"archived_missing","message":"locate","archive_availability":"archived_missing","archive_file_uuid":"ef01","archive_path":"cold/b.lar"}]}]}"#,
            as: TranscriptResponse.self)
        let issues = try #require(current.turns[0].bodyErrors)
        #expect(issues[0].resolvedArchiveAvailability == .archivedOffline)
        #expect(issues[0].archiveFileUuid == "abcd")
        #expect(issues[0].archivePath == "cold/a.lar")
        #expect(issues[1].resolvedArchiveAvailability == .archivedMissing)
        let summary = TranscriptArchiveSummary(issues: issues)
        #expect(summary.offlineBodyCount == 1)
        #expect(summary.missingBodyCount == 1)
        #expect(summary.unavailableBodyCount == 2)
        #expect(summary.isUnavailable)
        #expect(summary.title.contains("unavailable"))

        let legacy = try decode(
            #"{"session_id":"s","turns":[{"trace_id":"old","ts_request_ms":1,"body_errors":[{"trace_id":"old","artifact_kind":"client_request","kind":"archived_missing","message":"missing"}]}]}"#,
            as: TranscriptResponse.self)
        let legacyIssue = try #require(legacy.turns[0].bodyErrors?.first)
        #expect(legacyIssue.archiveAvailability == nil)
        #expect(legacyIssue.resolvedArchiveAvailability == .archivedMissing)

        let future = try decode(
            #"{"session_id":"s","turns":[{"trace_id":"new","ts_request_ms":1,"body_errors":[{"trace_id":"new","artifact_kind":"client_request","kind":"archive_future","archive_availability":"archive_future"}]}]}"#,
            as: TranscriptResponse.self)
        #expect(
            future.turns[0].bodyErrors?.first?.archiveAvailability
                == .unknown("archive_future"))
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

    @Test func transcriptPagingIsAdditiveAndBackwardsCompatible() throws {
        let legacy = try decode(
            #"{"session_id":"s","turns":[{"trace_id":"b","ts_request_ms":20,"assistant":"draft"}]}"#,
            as: TranscriptResponse.self)
        #expect(legacy.totalTurns == nil)
        #expect(legacy.oldestCursor == nil)

        let page = try decode(
            #"{"session_id":"s","turns":[{"trace_id":"b","ts_request_ms":20,"assistant":"final","status":200},{"trace_id":"c","ts_request_ms":20,"user":"next"}],"total_turns":3,"has_more_before":true,"has_more_after":false,"oldest_ts_ms":20,"oldest_trace_id":"b","newest_ts_ms":20,"newest_trace_id":"c"}"#,
            as: TranscriptResponse.self)
        let earlier = try decode(
            #"{"session_id":"s","turns":[{"trace_id":"a","ts_request_ms":10,"user":"hello","tool_calls":[{"name":"read","arguments":"{}"}]}]}"#,
            as: TranscriptResponse.self)
        let merged = TranscriptPaging.merge(
            existing: legacy.turns,
            incoming: page.turns + earlier.turns)
        #expect(merged.map(\.traceId) == ["a", "b", "c"])
        #expect(merged[1].assistant == "final")
        #expect(page.totalTurns == 3)
        #expect(page.oldestCursor == TranscriptCursor(tsMs: 20, traceId: "b"))
        #expect(page.newestCursor == TranscriptCursor(tsMs: 20, traceId: "c"))

        let counts = TranscriptTabCounts.counting(merged)
        #expect(counts.user == 2)
        #expect(counts.model == 2)
        #expect(counts.tools == 1)
        #expect(counts.all == 4)
    }

    @Test func transcriptStageTimelineShowsAttemptsAndTransportDifferences() throws {
        let response = try decode(
            #"""
            {"session_id":"s","turns":[{"trace_id":"trace-stage","ts_request_ms":20,"stages":[
              {"stage_id":"client-req","capture_sequence":0,"kind":"client_request","request_headers_ref":"h1","request_body_manifest_ref":"m1","fidelity":"captured"},
              {"stage_id":"router","capture_sequence":1,"kind":"router_decision","fidelity":"captured"},
              {"stage_id":"up-req","capture_sequence":2,"kind":"upstream_request","attempt_number":1,"request_headers_ref":"h2","request_body_manifest_ref":"m1","fidelity":"captured"},
              {"stage_id":"up-resp","capture_sequence":3,"kind":"upstream_response","attempt_number":1,"response_headers_ref":"h3","response_body_manifest_ref":"m2","stream_index_ref":"stream-1","fidelity":"captured"},
              {"stage_id":"client-resp","capture_sequence":4,"kind":"client_response","response_headers_ref":"h4","response_body_manifest_ref":"m3","fidelity":"captured"}
            ]}]}
            """#,
            as: TranscriptResponse.self)
        let stages = try #require(response.turns[0].stages)
        let summary = try #require(TranscriptStageTimeline.summary(stages))
        #expect(summary.contains("client request → router → upstream request #1"))
        #expect(summary.contains("shared request body"))
        #expect(summary.contains("changed request headers"))
        #expect(summary.contains("changed response body"))
        #expect(summary.contains("changed response headers"))
        #expect(summary.contains("timed stream"))

        let legacy = try decode(
            #"{"session_id":"s","turns":[{"trace_id":"legacy","ts_request_ms":1}]}"#,
            as: TranscriptResponse.self)
        #expect(legacy.turns[0].stages == nil)
        #expect(legacy.turns[0].stageError == nil)
    }

    @Test func streamReplayPageDecodesRawBytesAndFutureSources() throws {
        let page = try decode(
            #"{"trace_id":"trace","stage_id":"upstream","stage_kind":"upstream_response","source":"observed_reads","cursor":4,"next_cursor":5,"total_events":8,"page_bytes":6,"stream_index_id":"stream","raw_body_manifest_id":"body","archive_file_uuid":"archive","archive_state":"active","timing":"observed_delta_ns","server_sleep":false,"events":[{"index":4,"byte_offset":12,"byte_length":6,"observed_delta_ns":1500000000,"parser":null,"frame_kind":null,"bytes_b64":"ZGF0YTog"}]}"#,
            as: TraceStreamReplayPage.self)
        #expect(page.source == .observedReads)
        #expect(page.nextCursor == 5)
        #expect(page.serverSleep == false)
        #expect(String(data: try #require(page.events[0].bytes), encoding: .utf8) == "data: ")

        let future = try decode(
            #"{"trace_id":"trace","stage_id":"upstream","stage_kind":"upstream_response","source":"provider_frames_v2","cursor":0,"next_cursor":null,"total_events":0,"page_bytes":0,"stream_index_id":"stream","raw_body_manifest_id":"body","archive_file_uuid":"archive","archive_state":"sealed","timing":"observed_delta_ns","server_sleep":false,"events":[]}"#,
            as: TraceStreamReplayPage.self)
        #expect(future.source == .unknown("provider_frames_v2"))
    }

    @Test func streamReplayTimingScalesAbsoluteObservedDeltas() {
        #expect(TraceStreamReplayTiming.delayNanoseconds(
            previousDeltaNs: nil, currentDeltaNs: 1_000, speed: .instant) == 0)
        #expect(TraceStreamReplayTiming.delayNanoseconds(
            previousDeltaNs: 1_000, currentDeltaNs: 3_000, speed: .one) == 2_000)
        #expect(TraceStreamReplayTiming.delayNanoseconds(
            previousDeltaNs: 1_000, currentDeltaNs: 3_000, speed: .two) == 1_000)
        #expect(TraceStreamReplayTiming.delayNanoseconds(
            previousDeltaNs: 1_000, currentDeltaNs: 3_000, speed: .quarter) == 8_000)
        #expect(TraceStreamReplayTiming.delayNanoseconds(
            previousDeltaNs: 3_000, currentDeltaNs: 1_000, speed: .one) == 0)
    }

    @Test func streamReplayDisplayBufferIsBoundedAndBinarySafe() {
        let first = TraceStreamReplayBuffer.appending(
            Data("hello".utf8), to: Data(), limit: 7)
        let second = TraceStreamReplayBuffer.appending(
            Data(" world".utf8), to: first.data, limit: 7)
        #expect(String(data: second.data, encoding: .utf8) == "hello w")
        #expect(second.omittedBytes == 5)
        #expect(TraceStreamReplayBuffer.display(Data([0, 1, 255])).contains("binary stream"))
    }

    @Test func conversationGenerationSummaryDecodesAndPagesWithoutBodyBytes() throws {
        let page = try decode(
            #"{"session_id":"session","events":[{"trace_id":"trace-2","session_id":"session","ts_request_ms":20,"turn_view_id":"turn-2","generation_id":"gen-2","parent_generation_id":"gen-1","reason":"compaction","evidence":{"source":"capture","kind":"provider_event","id":"compact-7"},"upto_index":4,"entries":[],"response_entries":[]}],"total_events":77,"has_more_after":true,"next_after_ms":20,"next_after_id":"trace-2","entries_included":false}"#,
            as: TraceConversationEventPage.self)
        #expect(page.sessionId == "session")
        #expect(page.totalEvents == 77)
        #expect(page.entriesIncluded == false)
        #expect(page.nextCursor == TranscriptCursor(tsMs: 20, traceId: "trace-2"))
        #expect(page.events[0].reason == "compaction")
        #expect(page.events[0].isNotable)
        #expect(page.events[0].evidence?.id == "compact-7")
        #expect(page.events[0].entries.isEmpty)
    }

    @Test func conversationGenerationPagingReplacesStableTurnViews() throws {
        let first = try decode(
            #"{"session_id":"s","events":[{"trace_id":"a","session_id":"s","ts_request_ms":10,"turn_view_id":"turn-a","generation_id":"gen-a","parent_generation_id":null,"reason":"initial","evidence":null,"upto_index":0,"entries":[],"response_entries":[]}],"total_events":2,"has_more_after":true,"next_after_ms":10,"next_after_id":"a"}"#,
            as: TraceConversationEventPage.self)
        let second = try decode(
            #"{"session_id":"s","events":[{"trace_id":"b","session_id":"s","ts_request_ms":10,"turn_view_id":"turn-b","generation_id":"gen-b","parent_generation_id":"gen-a","reason":"branch","evidence":{"source":"capture","kind":"subagent","id":"child-1"},"upto_index":1,"entries":[],"response_entries":[]},{"trace_id":"a","session_id":"s","ts_request_ms":10,"turn_view_id":"turn-a","generation_id":"gen-a","parent_generation_id":null,"reason":"initial","evidence":null,"upto_index":0,"entries":[],"response_entries":[]}],"total_events":2,"has_more_after":false,"next_after_ms":10,"next_after_id":"b"}"#,
            as: TraceConversationEventPage.self)
        let merged = TraceConversationPaging.merge(
            existing: first.events, incoming: second.events)
        #expect(merged.map(\.traceId) == ["a", "b"])
        #expect(merged[1].reason == "branch")
        #expect(merged[1].isNotable)
    }

    @Test func traceSearchRowsCarryTranscriptAnchors() throws {
        let response = try decode(
            #"{"traces":[{"id":"match","session_id":"long-session","ts_request_ms":1784521342831}],"scanned":300}"#,
            as: TraceSearchResponse.self)
        #expect(response.traces[0].sessionId == "long-session")
        #expect(response.traces[0].tsRequestMs == 1_784_521_342_831)
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
