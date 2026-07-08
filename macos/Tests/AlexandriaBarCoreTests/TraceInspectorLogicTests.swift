import Foundation
import Testing
@testable import AlexandriaBarCore

@Suite struct TraceInspectorLogicTests {
    @Test func turnHeaderDuration() {
        #expect(TurnHeader.duration(requestMs: 1000, responseMs: 2800) == "1.8s")
        #expect(TurnHeader.duration(requestMs: 1000, responseMs: 1000) == "0.0s")
        #expect(TurnHeader.duration(requestMs: 1000, responseMs: nil) == nil)
        #expect(TurnHeader.duration(requestMs: 2000, responseMs: 1000) == nil)
        #expect(TurnHeader.duration(requestMs: 0, responseMs: 12_449) == "12.4s")
    }

    @Test func turnSeparatorFacts() {
        let full = TurnHeader.separatorFacts(
            turnNumber: 3, time: "12:31:04", status: 200,
            requestMs: 1000, responseMs: 2800, costUsd: 0.02)
        #expect(full == "turn 3 · 12:31:04 · 200 · 1.8s · $0.02")
        let pending = TurnHeader.separatorFacts(
            turnNumber: 1, time: "09:00:00", status: nil, requestMs: 5000, responseMs: nil)
        #expect(pending == "turn 1 · 09:00:00")
        let failed = TurnHeader.separatorFacts(
            turnNumber: 2, time: "09:00:01", status: 429, requestMs: 0, responseMs: nil)
        #expect(failed == "turn 2 · 09:00:01 · 429")
        let zeroCost = TurnHeader.separatorFacts(
            turnNumber: 4, time: "10:10:10", status: 200,
            requestMs: 0, responseMs: 100, costUsd: 0)
        #expect(zeroCost == "turn 4 · 10:10:10 · 200 · 0.1s")
    }

    @Test func bubbleLabels() {
        #expect(TurnHeader.requestLabel(harness: "pi") == "⬆ pi · user")
        #expect(TurnHeader.requestLabel(harness: "claude-code", isToolResult: false)
            == "⬆ claude-code · user")
        #expect(TurnHeader.requestLabel(harness: "codex", isToolResult: true)
            == "⬆ codex · tool result")
        #expect(TurnHeader.responseLabel(model: "gpt-5.5") == "⬇ gpt-5.5 · assistant")
        #expect(TurnHeader.responseLabel(model: nil) == "⬇ assistant")
    }

    @Test func toolResultBodyStripping() {
        #expect(TurnHeader.toolResultBody("[tool result] file contents here")
            == "file contents here")
        #expect(TurnHeader.toolResultBody("[tool result]\nline1\nline2") == "line1\nline2")
        #expect(TurnHeader.toolResultBody("[tool result]") == "")
        #expect(TurnHeader.toolResultBody("plain user message") == nil)
        #expect(TurnHeader.toolResultBody(" [tool result] not at start") == nil)
    }

    @Test func harnessDisplayName() {
        #expect(HarnessName.display(harness: nil, tags: ["harness": "pi"]) == "pi")
        #expect(HarnessName.display(harness: "ureq/2.12.1", tags: ["harness": "custom-rig"])
            == "custom-rig")
        #expect(HarnessName.display(harness: "claude-cli/1.0 (darwin)", tags: nil)
            == "claude-code")
        #expect(HarnessName.display(harness: "codex_exec/0.4", tags: [:]) == "codex")
        #expect(HarnessName.display(harness: "ureq/2.12.1", tags: nil) == "ureq")
        #expect(HarnessName.display(harness: "curl/8.7.1", tags: ["harness": ""]) == "curl")
        #expect(HarnessName.display(harness: nil, tags: nil) == "harness")
        #expect(HarnessName.display(harness: "", tags: nil) == "harness")
    }

    @Test func traceLinkRoundtrip() {
        let id = "98290559-5c28-4ed6-a4f7-b1c13ba80caf"
        let url = TraceLink.url(forTraceId: id)
        #expect(url?.absoluteString == "alexandria://trace/\(id)")
        #expect(url.flatMap(TraceLink.traceId(from:)) == id)
        #expect(TraceLink.url(forTraceId: "") == nil)
        #expect(TraceLink.traceId(from: URL(string: "alexandria://trace/")!) == nil)
        #expect(TraceLink.traceId(from: URL(string: "alexandria://other/abc")!) == nil)
        #expect(TraceLink.traceId(from: URL(string: "https://trace/abc")!) == nil)
    }

    @Test func bodyPrettyPrintsJSON() {
        let compact = #"{"b":1,"a":{"x":true},"url":"https://e.com/p"}"#
        let out = BodyPretty.display(compact)
        #expect(!out.isTruncated)
        #expect(out.text.contains("\n"))
        #expect(out.text.contains("\"a\""))
        #expect(out.text.range(of: "\"a\"")!.lowerBound < out.text.range(of: "\"b\"")!.lowerBound)
        #expect(out.text.contains("https://e.com/p"))
        let plain = "event: response.created\ndata: {}"
        #expect(BodyPretty.display(plain).text == plain)
        #expect(BodyPretty.display("").text.isEmpty)
    }

    @Test func bodyPrettyCaps() {
        let long = String(repeating: "x", count: 1200)
        let capped = BodyPretty.display(long, cap: 1000)
        #expect(capped.isTruncated)
        #expect(capped.fullCharCount == 1200)
        #expect(capped.text.contains("… (+200 chars truncated)"))
        #expect(capped.text.hasPrefix(String(repeating: "x", count: 1000)))
        let exact = BodyPretty.display(long, cap: 1200)
        #expect(!exact.isTruncated)
        #expect(exact.text == long)
    }

    @Test func headersJsonParsing() {
        let json = #"{"user-agent":"ureq/2.12.1","accept":"*/*","content-length":"116","Zeta":1}"#
        let pairs = TraceHeaders.sortedPairs(json)
        #expect(pairs.map(\.name) == ["accept", "content-length", "user-agent", "Zeta"])
        #expect(pairs[0].value == "*/*")
        #expect(pairs[3].value == "1")
        #expect(TraceHeaders.sortedPairs(nil).isEmpty)
        #expect(TraceHeaders.sortedPairs("not json").isEmpty)
        #expect(TraceHeaders.sortedPairs("[1,2]").isEmpty)
    }

    @Test func headerDiffDelta() {
        let first = [
            HeaderPair(name: "accept", value: "*/*"),
            HeaderPair(name: "User-Agent", value: "ureq/2.12.1"),
            HeaderPair(name: "content-length", value: "116"),
        ]
        let other = [
            HeaderPair(name: "accept", value: "*/*"),
            HeaderPair(name: "user-agent", value: "curl/8.7.1"),
            HeaderPair(name: "x-run-id", value: "r1"),
        ]
        let delta = HeaderDiff.delta(first: first, other: other)
        #expect(delta.added == ["x-run-id"])
        #expect(delta.removed == ["content-length"])
        #expect(delta.changed == ["user-agent"])
        #expect(!delta.isEmpty)
        #expect(delta.status(for: "X-Run-Id") == .added)
        #expect(delta.status(for: "USER-AGENT") == .changed)
        #expect(delta.status(for: "accept") == .same)
        let same = HeaderDiff.delta(first: first, other: first)
        #expect(same.isEmpty)
        let empty = HeaderDiff.delta(first: [], other: first)
        #expect(empty.added.count == 3)
        #expect(empty.removed.isEmpty)
    }

    @Test func providerInitials() {
        #expect(ModelProvider.initial(for: "anthropic") == "A")
        #expect(ModelProvider.initial(for: "openai") == "O")
        #expect(ModelProvider.initial(for: "xai") == "X")
        #expect(ModelProvider.initial(for: "gemini") == "G")
        #expect(ModelProvider.initial(for: "Mistral") == "M")
        #expect(ModelProvider.initial(for: "") == "?")
    }

    @Test func traceDetailDecoding() throws {
        let json = #"""
        {"extras":{"max_tokens":8,"message_count":1,"reasoning_effort":null,"system_chars":null,"temperature":null,"thinking_budget":null},"trace":{"account_id":"openai-oauth","billing_bucket":"subscription","cached_input_tokens":0,"client_format":"openai-chat","client_ip":"127.0.0.1","cost_usd":0.00018375,"error":null,"harness":"ureq/2.12.1","id":"9829-abc","input_tokens":11,"key_fingerprint":"5effb978eb304b0b","latency_ms":1777,"output_tokens":17,"reasoning_tokens":10,"req_body_path":"/x/req.json.gz","req_headers_json":"{\"accept\":\"*/*\"}","requested_model":"gpt-5.5","resp_body_path":"/x/resp.gz","resp_headers_json":"{\"date\":\"now\"}","routed_model":"gpt-5.6","run_id":null,"session_id":"auto-1","status":200,"streamed":0,"tags_json":null,"ts_request_ms":1783485291841,"ts_response_ms":1783485293618,"upstream_format":"openai-responses","upstream_provider":"openai","upstream_req_body_path":"/x/up.json.gz"}}
        """#
        let decoded = try JSONDecoder().decode(TraceDetailResponse.self, from: Data(json.utf8))
        #expect(decoded.trace.id == "9829-abc")
        #expect(decoded.trace.status == 200)
        #expect(decoded.trace.requestedModel == "gpt-5.5")
        #expect(decoded.trace.routedModel == "gpt-5.6")
        #expect(decoded.trace.clientFormat == "openai-chat")
        #expect(decoded.trace.upstreamFormat == "openai-responses")
        #expect(decoded.trace.reqBodyPath == "/x/req.json.gz")
        #expect(decoded.trace.keyFingerprint == "5effb978eb304b0b")
        #expect(TraceHeaders.sortedPairs(decoded.trace.reqHeadersJson)
            == [HeaderPair(name: "accept", value: "*/*")])
        #expect(decoded.extras?.hasAny == true)
        #expect(decoded.extras?.maxTokens == 8)
        #expect(decoded.extras?.reasoningEffort == nil)
    }

    @Test func documentTurnNumbersAndLinks() {
        let json = #"""
        {"session_id":"s","turns":[{"trace_id":"t1","ts_request_ms":1000,"ts_response_ms":2800,"model":"m","status":200,"user":"hi","assistant":"yo"},{"trace_id":"t2","ts_request_ms":3000,"ts_response_ms":null,"model":"m","status":null,"user":"[tool result] grep output","assistant":null}]}
        """#
        let turns = try! JSONDecoder().decode(TranscriptResponse.self, from: Data(json.utf8)).turns
        let doc = TranscriptRender.document(turns: turns, firstTurnNumber: 42, harnessName: "pi")
        let text = doc.string
        #expect(text.contains("turn 42"))
        #expect(text.contains("turn 43"))
        #expect(text.contains("Details"))
        #expect(text.contains("⬆ pi · user"))
        #expect(text.contains("⬇ m · assistant"))
        #expect(text.contains("⬆ pi · tool result"))
        #expect(text.contains("❯ grep output"))
        #expect(!text.contains("[tool result]"))
        var foundLinks: [URL] = []
        doc.enumerateAttribute(.link, in: NSRange(location: 0, length: doc.length)) { value, _, _ in
            if let url = value as? URL { foundLinks.append(url) }
        }
        #expect(foundLinks.map(\.absoluteString)
            == ["alexandria://trace/t1", "alexandria://trace/t2"])
    }
}
