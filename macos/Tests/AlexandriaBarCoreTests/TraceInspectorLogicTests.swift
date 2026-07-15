#if canImport(AppKit)
import AppKit
#endif
import Foundation
import Testing
@testable import AlexandriaBarCore

@Suite struct TraceInspectorLogicTests {
    @Test func errorDisplayFormatterIsPortableAndStructured() {
        #expect(TraceErrorDisplay.line(
            kind: "rate_limit_error", code: "429", message: "slow down")
            == "rate_limit_error · 429 — slow down")
        #expect(TraceErrorDisplay.line(kind: nil, code: nil, message: "network failed") == "network failed")
    }

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
        let unavailable = TurnHeader.separatorFacts(
            turnNumber: 5, time: "10:10:11", status: 200,
            requestMs: 0, responseMs: 100, costUnavailable: true)
        #expect(unavailable == "turn 5 · 10:10:11 · 200 · 0.1s · cost unavailable")
    }

    @Test func bubbleLabels() {
        #expect(TurnHeader.requestLabel(harness: "pi") == "pi · user")
        #expect(TurnHeader.requestLabel(harness: "claude-code", isToolResult: false)
            == "claude-code · user")
        #expect(TurnHeader.requestLabel(harness: "codex", isToolResult: true)
            == "codex · tool result")
        #expect(TurnHeader.harnessResultLabel() == "Harness · tool result")
        #expect(TurnHeader.harnessResultLabel(toolName: "Read") == "Harness · tool result · Read")
        #expect(TurnHeader.harnessResultLabel(toolName: "") == "Harness · tool result")
        #expect(TurnHeader.responseLabel(model: "gpt-5.5") == "gpt-5.5 · model")
        #expect(TurnHeader.responseLabel(
            model: "gpt-5.6-sol", reasoningEffort: "high",
            billingBucket: "subscription")
            == "gpt-5.6-sol · model · high · subscription")
        #expect(TurnHeader.responseLabel(model: nil) == "model")
    }

    @Test func toolCallDecoding() throws {
        let json = #"""
        {"session_id":"s","turns":[{"trace_id":"t1","ts_request_ms":0,"ts_response_ms":1,"model":"gpt-5.5","status":200,"user":null,"assistant":"ok","tool_calls":[{"id":"call-1","name":"bash","arguments":"{\"command\":\"ls -l\"}"},{"name":"read_file","arguments":null}]},{"trace_id":"t2","ts_request_ms":2,"ts_response_ms":3,"model":"m","status":200,"user":"hi","assistant":"yo"}]}
        """#
        let turns = try JSONDecoder().decode(TranscriptResponse.self, from: Data(json.utf8)).turns
        #expect(turns[0].toolCalls?.count == 2)
        #expect(turns[0].toolCalls?[0] == ToolCall(
            name: "bash", arguments: #"{"command":"ls -l"}"#, id: "call-1"))
        #expect(turns[0].toolCalls?[1] == ToolCall(name: "read_file", arguments: nil))
        #expect(turns[1].toolCalls == nil)
    }

    @Test func toolCallArgumentSummary() {
        #expect(ToolCall.summary(#"{"command":"ls -l /app"}"#) == "ls -l /app")
        #expect(ToolCall.summary(
            #"{"command":"sed -n '1,10p' f","intent":"read","timeout":10000}"#)
            == "sed -n '1,10p' f")
        let multi = ToolCall.summary(#"{"path":"/a","limit":5}"#)
        #expect(multi.contains("\n"))
        #expect(multi.contains("\"path\""))
        #expect(multi.contains("\"limit\""))
        #expect(ToolCall.summary("not json {") == "not json {")
        #expect(ToolCall.summary("") == "")
        #expect(ToolCall.summary("  \n ") == "")
        #expect(ToolCall.summary(#"{"command":123}"#).contains("\"command\""))
        #expect(ToolCall(name: "bash", arguments: nil).argumentSummary == "")
    }

    @Test func toolLifecyclePairsExactIdsThenSafelyFallsBack() throws {
        let executions = try JSONDecoder().decode([ExecutedTool].self, from: Data(#"""
        [
          {"id":"e1","tool_call_id":"call-2","trace_id":null,"tool_name":"bash","turn_id":"0","ts_start_ms":1000,"ts_end_ms":1100,"is_error":false,"exit_status":0,"args_body_path":"/args","result_body_path":"/result"},
          {"id":"e2","tool_call_id":null,"trace_id":null,"tool_name":"read","turn_id":"0","ts_start_ms":1200,"ts_end_ms":null,"is_error":null,"exit_status":null,"args_body_path":null,"result_body_path":null}
        ]
        """#.utf8))
        let requests = [
            ToolCall(name: "bash", arguments: "{}", id: "call-2"),
            ToolCall(name: "read", arguments: "{}"),
        ]
        let paired = ToolLifecycle.pair(requests: requests, executions: executions)
        #expect(paired.count == 2)
        #expect(paired[0].execution?.id == "e1")
        #expect(paired[0].status == .executed)
        #expect(paired[1].execution?.id == "e2")
        #expect(paired[1].status == .running)

        let mismatched = ToolLifecycle.pair(
            requests: [ToolCall(name: "bash", arguments: "{}", id: "different-call")],
            executions: [executions[0]])
        #expect(mismatched.count == 2)
        #expect(mismatched[0].status == .requested)
        #expect(mismatched[1].status == .executed)
    }

    @Test func toolLifecycleReportsFailedExecution() throws {
        let execution = try JSONDecoder().decode(ExecutedTool.self, from: Data(#"""
        {"id":"e1","tool_call_id":"call-1","tool_name":"bash","turn_id":"0","ts_start_ms":1000,"ts_end_ms":1100,"is_error":true,"exit_status":1,"args_body_path":"/args","result_body_path":"/result"}
        """#.utf8))
        let lifecycle = ToolLifecycle.pair(
            requests: [ToolCall(name: "bash", arguments: "{}", id: "call-1")],
            executions: [execution])[0]
        #expect(lifecycle.status == .failed)
    }

    #if canImport(AppKit)
    @Test func documentRendersToolCalls() {
        let json = #"""
        {"session_id":"s","turns":[{"trace_id":"t1","ts_request_ms":0,"ts_response_ms":1,"model":"gpt-5.5","status":200,"user":null,"assistant":null,"tool_calls":[{"name":"bash","arguments":"{\"command\":\"ls -l /app\",\"intent\":\"look\"}"},{"name":"str_replace","arguments":"{\"path\":\"/a\",\"old\":\"x\"}"}]}]}
        """#
        let turns = try! JSONDecoder().decode(TranscriptResponse.self, from: Data(json.utf8)).turns
        let text = TranscriptRender.document(turns: turns).string
        #expect(text.contains("⚙ bash"))
        #expect(text.contains("ls -l /app"))
        #expect(!text.contains("intent"))
        #expect(text.contains("⚙ str_replace"))
        #expect(text.contains("old: x"))
        #expect(text.contains("path: /a"))
        #expect(text.contains("gpt-5.5 · model"))
        let raw = TranscriptRender.document(turns: turns, rawMode: true).string
        #expect(raw.contains(#"{"command":"ls -l /app","intent":"look"}"#))
        #expect(raw.contains(#"{"path":"/a","old":"x"}"#))
    }

    @Test func documentMergesRequestedAndExecutedToolIntoOneCard() throws {
        let json = #"""
        {"session_id":"s","turns":[{"trace_id":"t1","ts_request_ms":1000,"ts_response_ms":2000,"model":"gpt-5.6-sol","status":200,"user":"run it","assistant":null,"tool_calls":[{"id":"call-1","name":"bash","arguments":"{\"command\":\"printf ok\"}"}],"executed_tools":[{"id":"exec-1","tool_call_id":"call-1","trace_id":null,"tool_name":"bash","turn_id":"0","ts_start_ms":2100,"ts_end_ms":2112,"is_error":false,"exit_status":null,"args_body_path":"/args","result_body_path":"/result"}]}]}
        """#
        let turns = try JSONDecoder().decode(TranscriptResponse.self, from: Data(json.utf8)).turns
        let document = TranscriptRender.document(turns: turns)
        let text = document.string
        #expect(text.components(separatedBy: "⚙ bash").count - 1 == 1)
        #expect(text.contains("bash · executed · 0.0s"))
        #expect(text.contains("printf ok"))
        #expect(text.contains("view captured arguments  ·  view output"))
        #expect(!text.contains("arguments captured"))

        var toolLinks: [String] = []
        document.enumerateAttribute(
            .link, in: NSRange(location: 0, length: document.length)
        ) { value, _, _ in
            guard let url = value as? URL, url.host == "tool" else { return }
            toolLinks.append(url.absoluteString)
        }
        #expect(toolLinks == [
            "alexandria://tool/exec-1/args",
            "alexandria://tool/exec-1/result",
        ])
    }

    @Test func documentLabelsRequestedRunningAndFailedToolStates() throws {
        let json = #"""
        {"session_id":"s","turns":[{"trace_id":"t1","ts_request_ms":1000,"ts_response_ms":2000,"model":"m","status":200,"user":null,"assistant":null,"tool_calls":[{"id":"requested","name":"read","arguments":"{}"},{"id":"running","name":"bash","arguments":"{}"},{"id":"failed","name":"edit","arguments":"{}"}],"executed_tools":[{"id":"e-running","tool_call_id":"running","tool_name":"bash","ts_start_ms":2100,"ts_end_ms":null,"is_error":null,"exit_status":null},{"id":"e-failed","tool_call_id":"failed","tool_name":"edit","ts_start_ms":2200,"ts_end_ms":2300,"is_error":true,"exit_status":2}]}]}
        """#
        let turns = try JSONDecoder().decode(TranscriptResponse.self, from: Data(json.utf8)).turns
        let text = TranscriptRender.document(turns: turns).string
        #expect(text.contains("read · requested"))
        #expect(text.contains("bash · running"))
        #expect(text.contains("edit · failed · exit 2 · 0.1s"))
    }

    @Test func documentPreservesOrderedAssistantBlocks() throws {
        let json = #"""
        {"session_id":"s","turns":[{"trace_id":"t1","ts_request_ms":0,"ts_response_ms":1,"model":"cursor-agent","provider":"cursor","status":200,"user":"what files are here?","assistant":"Listing.\n\nHere are the files.","tool_calls":[{"name":"Shell","arguments":"{\"command\":\"ls -la\"}"}],"assistant_blocks":[{"type":"text","text":"Listing."},{"type":"tool_call","name":"Shell","arguments":"{\"command\":\"ls -la\"}"},{"type":"text","text":"Here are the files."}]}]}
        """#
        let turns = try JSONDecoder().decode(TranscriptResponse.self, from: Data(json.utf8)).turns
        let text = TranscriptRender.document(turns: turns).string as NSString
        let progress = text.range(of: "Listing.").location
        let tool = text.range(of: "⚙ Shell").location
        let answer = text.range(of: "Here are the files.").location
        #expect(progress < tool)
        #expect(tool < answer)

        let original = TranscriptRender.state(for: turns)
        let changed = json.replacingOccurrences(of: "Listing.", with: "Checking")
        let changedTurns = try JSONDecoder().decode(
            TranscriptResponse.self, from: Data(changed.utf8)).turns
        #expect(TranscriptRender.plan(previous: original, turns: changedTurns) == .rebuild)
    }

    #endif

    @Test func jsonHighlightSpans() {
        let json = #"{"a": "b", "n": -1.5, "t": true, "f": false, "x": null}"#
        let tokens = JsonHighlight.spans(json).map { span -> (String, JsonHighlight.Kind) in
            let units = Array(json.utf16)[span.range]
            return (String(utf16CodeUnits: Array(units), count: units.count), span.kind)
        }
        #expect(tokens.map(\.0) == [
            "\"a\"", "\"b\"", "\"n\"", "-1.5", "\"t\"", "true", "\"f\"", "false", "\"x\"", "null",
        ])
        #expect(tokens.map(\.1) == [
            .key, .string, .key, .number, .key, .keyword, .key, .keyword, .key, .keyword,
        ])
        let escaped = JsonHighlight.spans(#"{"k": "a\"b"}"#)
        #expect(escaped.count == 2)
        #expect(escaped[1].kind == .string)
    }

    #if canImport(AppKit)
    @Test func jsonHighlightAttributes() {
        let pretty = "{\n  \"name\" : \"bash\",\n  \"count\" : 3\n}"
        let attributed = JsonHighlight.attributed(
            pretty, font: NSFont.monospacedSystemFont(ofSize: 10, weight: .regular))
        let ns = pretty as NSString
        let colors = JsonHighlight.Colors.standard
        let keyAt = ns.range(of: "\"name\"").location
        let stringAt = ns.range(of: "\"bash\"").location
        let numberAt = ns.range(of: "3", options: .backwards).location
        let braceAt = 0
        func color(_ at: Int) -> NSColor? {
            attributed.attribute(.foregroundColor, at: at, effectiveRange: nil) as? NSColor
        }
        #expect(color(keyAt) == colors.key)
        #expect(color(stringAt) == colors.string)
        #expect(color(numberAt) == colors.number)
        #expect(color(braceAt) == colors.punctuation)
    }

    #endif

    @Test func jsonNiceBlocks() {
        let bashResult = #"{"returncode":0,"output":"total 12\n-rw-r--r-- 1"}"#
        #expect(JsonNice.blocks(bashResult) == [
            .row(key: "returncode", value: "0"),
            .block(key: "output", text: "total 12\n-rw-r--r-- 1"),
        ])
        let long = String(repeating: "x", count: 121)
        #expect(JsonNice.blocks(#"{"s":"\#(long)"}"#) == [.block(key: "s", text: long)])
        let exactly = String(repeating: "x", count: 120)
        #expect(JsonNice.blocks(#"{"s":"\#(exactly)"}"#) == [.row(key: "s", value: exactly)])
        #expect(JsonNice.blocks("plain text") == [.text("plain text")])
        #expect(JsonNice.blocks("[1,2]") == [.text("[1,2]")])
        #expect(JsonNice.blocks("{}") == [.text("{}")])
        #expect(JsonNice.blocks(#"{"b":true,"n":null,"o":{"k":1}}"#) == [
            .row(key: "b", value: "true"),
            .row(key: "n", value: "null"),
            .row(key: "o", value: #"{"k":1}"#),
        ])
    }

    @Test func rawModeInRenderSignature() throws {
        let json = #"""
        {"session_id":"s","turns":[{"trace_id":"t1","ts_request_ms":0,"ts_response_ms":1,"model":"m","status":200,"user":"hi","assistant":"yo"}]}
        """#
        let turns = try JSONDecoder().decode(TranscriptResponse.self, from: Data(json.utf8)).turns
        let nice = TranscriptRender.state(for: turns, rawMode: false)
        #expect(TranscriptRender.plan(previous: nice, turns: turns, rawMode: false) == .unchanged)
        #expect(TranscriptRender.plan(previous: nice, turns: turns, rawMode: true) == .rebuild)
        let raw = TranscriptRender.state(for: turns, rawMode: true)
        #expect(TranscriptRender.plan(previous: raw, turns: turns, rawMode: true) == .unchanged)
        #expect(TranscriptRender.plan(previous: raw, turns: turns, rawMode: false) == .rebuild)
    }

    #if canImport(AppKit)
    @Test func toolResultNiceVersusRaw() throws {
        let json = #"""
        {"session_id":"s","turns":[{"trace_id":"t1","ts_request_ms":0,"ts_response_ms":1,"model":"m","status":200,"user":"[tool result] {\"returncode\":0,\"output\":\"line1\\nline2\"}","assistant":null}]}
        """#
        let turns = try JSONDecoder().decode(TranscriptResponse.self, from: Data(json.utf8)).turns
        let nice = TranscriptRender.document(turns: turns).string
        #expect(nice.contains("returncode: 0"))
        #expect(nice.contains("output:\nline1\nline2"))
        #expect(!nice.contains(#"\n"#))
        let raw = TranscriptRender.document(turns: turns, rawMode: true).string
        #expect(raw.contains(#"{"returncode":0,"output":"line1\nline2"}"#))
        #expect(!raw.contains("returncode: 0"))
    }

    @Test func bubbleAttributeTagging() throws {
        let json = #"""
        {"session_id":"s","turns":[{"trace_id":"t1","ts_request_ms":0,"ts_response_ms":1,"model":"gpt-5.5","status":200,"user":"hi","assistant":"yo","tool_calls":[{"name":"bash","arguments":"{\"command\":\"ls\"}"}]},{"trace_id":"t2","ts_request_ms":2,"ts_response_ms":3,"model":"m","status":200,"user":"[tool result] output text","assistant":null}]}
        """#
        let turns = try JSONDecoder().decode(TranscriptResponse.self, from: Data(json.utf8)).turns
        let built = TranscriptRender.build(turns: turns)
        var kinds: [String] = []
        built.text.enumerateAttribute(
            .transcriptBubbleKind, in: NSRange(location: 0, length: built.text.length)
        ) { value, _, _ in
            guard let kind = value as? String else { return }
            if kinds.last != kind { kinds.append(kind) }
        }
        #expect(kinds == [TranscriptBubbleKind.turn.rawValue])
        for range in built.turnRanges {
            let atStart = built.text.attribute(
                .transcriptTurnId, at: range.range.location, effectiveRange: nil) as? String
            let atEnd = built.text.attribute(
                .transcriptTurnId, at: range.range.upperBound - 1, effectiveRange: nil) as? String
            #expect(atStart == range.traceId)
            #expect(atEnd == range.traceId)
        }
        var groups: [String] = []
        built.text.enumerateAttribute(
            .transcriptBubbleGroup, in: NSRange(location: 0, length: built.text.length)
        ) { value, range, _ in
            guard let group = value as? String else { return }
            groups.append(group)
            #expect(built.text.attribute(
                .transcriptBubbleKind, at: range.location, effectiveRange: nil) != nil)
        }
        #expect(groups == ["t1#turn", "t2#turn"])
        #expect(groups.count == Set(groups).count)
    }

    @Test func turnHitTestMapping() throws {
        let json = #"""
        {"session_id":"s","turns":[{"trace_id":"t1","ts_request_ms":0,"ts_response_ms":1,"model":"m","status":200,"user":"hi","assistant":"yo"},{"trace_id":"t2","ts_request_ms":2,"ts_response_ms":3,"model":"m","status":200,"user":"more","assistant":"text"}]}
        """#
        let turns = try JSONDecoder().decode(TranscriptResponse.self, from: Data(json.utf8)).turns
        let built = TranscriptRender.build(turns: turns)
        let ranges = built.turnRanges
        #expect(TurnHitTest.traceId(at: 0, in: ranges) == "t1")
        #expect(TurnHitTest.traceId(at: ranges[0].range.upperBound - 1, in: ranges) == "t1")
        #expect(TurnHitTest.traceId(at: ranges[1].range.location, in: ranges) == "t2")
        #expect(TurnHitTest.traceId(at: built.text.length, in: ranges) == nil)
        #expect(TurnHitTest.traceId(at: -1, in: ranges) == nil)
        #expect(TurnHitTest.traceId(at: 5, in: []) == nil)
    }

    #endif

    @Test func inspectorSelectionRetargetsToCurrentOrLastTurn() {
        let ids = ["t1", "t2", "t3"]
        #expect(TraceInspectorSelection.target(currentTraceId: "t2", in: ids) == "t2")
        #expect(TraceInspectorSelection.target(currentTraceId: "missing", in: ids) == "t3")
        #expect(TraceInspectorSelection.target(currentTraceId: nil, in: ids) == "t3")
        #expect(TraceInspectorSelection.target(currentTraceId: "t2", in: []) == nil)
        #expect(TraceInspectorSelection.previous(before: "t3", in: ids) == "t2")
        #expect(TraceInspectorSelection.previous(before: "t1", in: ids) == nil)
        #expect(TraceInspectorSelection.previous(before: "missing", in: ids) == nil)
    }

    @Test func bodyCacheLRU() {
        var cache = TraceBodyCache(capacity: 2)
        let a = TraceBodyContent(text: "a", diskPath: "/a")
        let b = TraceBodyContent(text: "b", diskPath: nil)
        let c = TraceBodyContent(text: "c", diskPath: nil)
        let keyA = TraceBodyCache.key(id: "t1", kind: .request)
        let keyB = TraceBodyCache.key(id: "t1", kind: .response)
        let keyC = TraceBodyCache.key(id: "t2", kind: .request)
        #expect(keyA == "t1|request")
        #expect(keyA != keyB)
        cache.insert(a, for: keyA)
        cache.insert(b, for: keyB)
        #expect(cache.value(for: keyA) == a)
        cache.insert(c, for: keyC)
        #expect(cache.count == 2)
        #expect(cache.value(for: keyB) == nil)
        #expect(cache.value(for: keyA) == a)
        #expect(cache.value(for: keyC) == c)
        cache.insert(a, for: keyA)
        #expect(cache.count == 2)
    }

    @Test func turnExportMarkdown() throws {
        let json = #"""
        {"extras":{"max_tokens":8,"message_count":1,"reasoning_effort":null,"system_chars":null,"temperature":null,"thinking_budget":null},"trace":{"account_id":"openai-oauth","billing_bucket":"subscription","client_format":"openai-chat","cost_usd":0.0002,"id":"9829-abc","input_tokens":11,"output_tokens":17,"requested_model":"gpt-5.5","routed_model":"gpt-5.6","session_id":"auto-1","status":200,"ts_request_ms":1783485291841,"ts_response_ms":1783485293618,"upstream_format":"openai-responses","upstream_provider":"openai"}}
        """#
        let decoded = try JSONDecoder().decode(TraceDetailResponse.self, from: Data(json.utf8))
        let md = TurnExport.markdown(
            detail: decoded.trace, extras: decoded.extras,
            reqHeaders: [HeaderPair(name: "accept", value: "*/*")],
            respHeaders: [],
            reqBody: #"{"a":1}"#, respBody: nil)
        #expect(md.hasPrefix("# Trace 9829-abc"))
        #expect(md.contains("## Overview"))
        #expect(md.contains("- status: 200"))
        #expect(md.contains("- model: gpt-5.5 → gpt-5.6"))
        #expect(md.contains("(translated)"))
        #expect(md.contains("## Extras"))
        #expect(md.contains("- max tokens: 8"))
        #expect(md.contains("## Request headers\n```\naccept: */*\n```"))
        #expect(md.contains("## Response headers\n_not available_"))
        #expect(md.contains("## Request body\n```json"))
        #expect(md.contains("\"a\""))
        #expect(md.contains("## Response body\n_not available_"))
        let fenceCount = md.components(separatedBy: "```").count - 1
        #expect(fenceCount == 4)
    }

    @Test func systemPromptExtraDecoding() throws {
        let json = #"""
        {"extras":{"system_prompt":"You are a helpful agent.","max_tokens":null,"message_count":null,"reasoning_effort":null,"system_chars":24,"temperature":null,"thinking_budget":null},"trace":{"id":"x"}}
        """#
        let decoded = try JSONDecoder().decode(TraceDetailResponse.self, from: Data(json.utf8))
        #expect(decoded.extras?.systemPrompt == "You are a helpful agent.")
        let absent = #"{"extras":{"max_tokens":1},"trace":{"id":"y"}}"#
        let decodedAbsent = try JSONDecoder().decode(
            TraceDetailResponse.self, from: Data(absent.utf8))
        #expect(decodedAbsent.extras?.systemPrompt == nil)
    }

    #if canImport(AppKit)
    @Test func turnRangeBookkeeping() throws {
        let json = #"""
        {"session_id":"s","turns":[{"trace_id":"t1","ts_request_ms":0,"ts_response_ms":1,"model":"m","status":200,"user":"hi","assistant":"yo"},{"trace_id":"t2","ts_request_ms":2,"ts_response_ms":3,"model":"m","status":200,"user":"more","assistant":"text"},{"trace_id":"t3","ts_request_ms":4,"ts_response_ms":null,"model":null,"status":429,"user":null,"assistant":null,"error":"boom"}]}
        """#
        let turns = try JSONDecoder().decode(TranscriptResponse.self, from: Data(json.utf8)).turns
        let built = TranscriptRender.build(turns: turns)
        #expect(built.turnRanges.map(\.traceId) == ["t1", "t2", "t3"])
        #expect(built.turnRanges[0].range.location == 0)
        #expect(built.turnRanges[1].range.location == built.turnRanges[0].range.upperBound)
        #expect(built.turnRanges[2].range.location == built.turnRanges[1].range.upperBound)
        #expect(built.turnRanges[2].range.upperBound == built.text.length)
        let slice = (built.text.string as NSString).substring(with: built.turnRanges[1].range)
        #expect(slice.contains("turn 2"))
        #expect(slice.contains("more"))
        #expect(!slice.contains("turn 3"))
        let shifted = TranscriptRender.shifted(built.turnRanges, by: 100)
        #expect(shifted[0].range.location == 100)
        #expect(shifted[2].range.length == built.turnRanges[2].range.length)
        #expect(shifted.map(\.traceId) == ["t1", "t2", "t3"])
    }

    #endif

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
        #expect(
            HarnessName.display(
                harness: "Anthropic/JS 0.91.1", tags: ["harness": "pi"])
                == "pi")
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

    @Test func requestJSONDiffShowsOnlyStructuralChanges() throws {
        let previous = #"{"changed":1,"messages":[{"role":"system","content":"long prompt"},{"role":"user","content":"hello"}],"removed":{"secret":1},"same":"keep"}"#
        let current = #"{"added":true,"changed":2,"messages":[{"role":"system","content":"long prompt"},{"role":"user","content":"hello"},{"role":"assistant","content":"hi"}],"same":"keep"}"#
        let presentation = RequestJSONDiff.presentation(previous: previous, current: current)
        #expect(presentation.kind == .diff)
        #expect(presentation.note == nil)
        #expect(!presentation.text.contains("long prompt"))
        #expect(!presentation.text.contains(#""same""#))

        let data = try #require(presentation.text.data(using: .utf8))
        let operations = try #require(
            JSONSerialization.jsonObject(with: data) as? [[String: Any]])
        #expect(operations.count == 4)
        #expect(operations.map { $0["op"] as? String }
            == ["remove", "add", "replace", "add"])
        #expect(operations.map { $0["path"] as? String }
            == ["/removed", "/added", "/changed", "/messages/2"])
        #expect((operations[0]["previous"] as? [String: Any])?["secret"] as? Int == 1)
        #expect(operations[2]["previous"] as? Int == 1)
        #expect(operations[2]["value"] as? Int == 2)
        #expect((operations[3]["value"] as? [String: Any])?["role"] as? String
            == "assistant")
    }

    @Test func requestJSONDiffHandlesArrayChangesAndRemovals() throws {
        let presentation = RequestJSONDiff.presentation(
            previous: #"{"items":[{"id":1},{"id":2},{"id":3}]}"#,
            current: #"{"items":[{"id":1},{"id":20}]}"#)
        let data = try #require(presentation.text.data(using: .utf8))
        let operations = try #require(
            JSONSerialization.jsonObject(with: data) as? [[String: Any]])
        #expect(operations.map { $0["op"] as? String } == ["replace", "remove"])
        #expect(operations.map { $0["path"] as? String } == ["/items/1/id", "/items/2"])
        #expect((operations[1]["previous"] as? [String: Any])?["id"] as? Int == 3)
        #expect(operations[1]["value"] == nil)

        let escaped = RequestJSONDiff.presentation(
            previous: #"{"a/b":1,"til~de":1}"#,
            current: #"{"a/b":2,"til~de":2}"#)
        #expect(escaped.text.contains(#""path": "/a~1b""#))
        #expect(escaped.text.contains(#""path": "/til~0de""#))
    }

    @Test func requestJSONDiffFallbackStates() {
        let first = RequestJSONDiff.presentation(previous: nil, current: #"{"b":2,"a":1}"#)
        #expect(first.kind == .firstRequest)
        #expect(first.text.range(of: #""a""#)!.lowerBound
            < first.text.range(of: #""b""#)!.lowerBound)

        let unchanged = RequestJSONDiff.presentation(
            previous: #"{"a":1}"#, current: #"{ "a": 1 }"#)
        #expect(unchanged.kind == .unchanged)
        #expect(unchanged.text == "[]")

        let invalidCurrent = RequestJSONDiff.presentation(
            previous: #"{"a":1}"#, current: "not json")
        #expect(invalidCurrent.kind == .invalidCurrent)
        #expect(invalidCurrent.text == "not json")

        let invalidPrevious = RequestJSONDiff.presentation(
            previous: "not json", current: #"{"a":1}"#)
        #expect(invalidPrevious.kind == .invalidPrevious)
        #expect(invalidPrevious.text.contains(#""a": 1"#))
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
        #expect(ModelProvider.initial(for: "cursor") == "C")
        #expect(ModelProvider.initial(for: "amp") == "A")
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

    #if canImport(AppKit)
    @Test func documentTurnNumbersAndLinks() {
        let json = #"""
        {"session_id":"s","turns":[{"trace_id":"t1","ts_request_ms":1000,"ts_response_ms":2800,"model":"m","status":200,"user":"hi","assistant":"yo"},{"trace_id":"t2","ts_request_ms":3000,"ts_response_ms":null,"model":"m","status":null,"user":"[tool result] grep output","assistant":null}]}
        """#
        let turns = try! JSONDecoder().decode(TranscriptResponse.self, from: Data(json.utf8)).turns
        let doc = TranscriptRender.document(turns: turns, firstTurnNumber: 42, harnessName: "pi")
        let text = doc.string
        #expect(text.contains("turn 42"))
        #expect(text.contains("turn 43"))
        #expect(!text.contains("Details"))
        #expect(text.contains("pi · user"))
        #expect(text.contains("m · model"))
        #expect(text.contains("pi · tool result"))
        #expect(text.contains("grep output"))
        #expect(!text.contains("[tool result]"))
        var foundLinks: [URL] = []
        doc.enumerateAttribute(.link, in: NSRange(location: 0, length: doc.length)) { value, _, _ in
            guard let url = value as? URL else { return }
            if foundLinks.last != url { foundLinks.append(url) }
        }
        #expect(foundLinks.map(\.absoluteString)
            == ["alexandria://trace/t1", "alexandria://trace/t2"])
    }
    #endif
}
