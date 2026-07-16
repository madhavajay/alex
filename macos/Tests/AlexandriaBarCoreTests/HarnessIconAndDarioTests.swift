import Foundation
import Testing
@testable import AlexandriaBarCore

@Suite struct HarnessIconTests {
    @Test func tagExactMatches() {
        #expect(HarnessIcon.assetName(harness: nil, tags: ["harness": "pi"]) == "pi.svg")
        #expect(HarnessIcon.assetName(harness: nil, tags: ["harness": "codex"]) == "codex.png")
        #expect(
            HarnessIcon.assetName(harness: nil, tags: ["harness": "claude-code"])
                == "claude-code.png")
        #expect(
            HarnessIcon.assetName(harness: nil, tags: ["harness": "grok-build"])
                == "grok-build.png")
        #expect(HarnessIcon.assetName(harness: nil, tags: ["harness": "opencode"]) == "opencode.png")
        #expect(
            HarnessIcon.assetName(harness: nil, tags: ["harness": "qwen-code"]) == "qwen-code.png")
        #expect(
            HarnessIcon.assetName(harness: nil, tags: ["harness": "gemini-cli"])
                == "gemini-cli.png")
        #expect(
            HarnessIcon.assetName(harness: nil, tags: ["harness": "mini-swe-agent"])
                == "mini-swe-agent.png")
        #expect(
            HarnessIcon.assetName(harness: nil, tags: ["harness": "kimi-code"]) == "kimi-code.jpg")
        #expect(HarnessIcon.assetName(harness: nil, tags: ["harness": "goose"]) == "goose.jpg")
        #expect(HarnessIcon.assetName(harness: nil, tags: ["harness": "hermes"]) == "hermes.png")
        #expect(
            HarnessIcon.assetName(harness: nil, tags: ["harness": "droid-cli"]) == "droid-cli.svg")
        #expect(
            HarnessIcon.assetName(harness: nil, tags: ["harness": "cursor-cli"])
                == "cursor-cli.png")
        #expect(HarnessIcon.assetName(harness: nil, tags: ["harness": "amp-code"]) == "amp-code.svg")
        #expect(
            HarnessIcon.assetName(harness: nil, tags: ["harness": "opensage-adk"])
                == "opensage-adk.png")
        #expect(HarnessIcon.assetName(harness: nil, tags: ["harness": "stirrup"]) == "stirrup.ico")
        #expect(
            HarnessIcon.assetName(harness: nil, tags: ["harness": "oh-my-pi"])
                == "oh-my-pi.png")
        #expect(
            HarnessIcon.assetName(harness: nil, tags: ["harness": "pydantic-ai-harness"])
                == "pydantic-ai-harness.png")
        #expect(HarnessIcon.assetName(harness: nil, tags: ["harness": "jcode"]) == "jcode.png")
    }

    @Test func tagAliases() {
        #expect(
            HarnessIcon.assetName(harness: nil, tags: ["harness": "claude"]) == "claude-code.png")
        #expect(HarnessIcon.assetName(harness: nil, tags: ["harness": "grok"]) == "grok-build.png")
        #expect(HarnessIcon.assetName(harness: nil, tags: ["harness": "qwen"]) == "qwen-code.png")
        #expect(HarnessIcon.assetName(harness: nil, tags: ["harness": "gemini"]) == "gemini-cli.png")
        #expect(
            HarnessIcon.assetName(harness: nil, tags: ["harness": "mini"]) == "mini-swe-agent.png")
        #expect(HarnessIcon.assetName(harness: nil, tags: ["harness": "kimi"]) == "kimi-code.jpg")
        #expect(HarnessIcon.assetName(harness: nil, tags: ["harness": "droid"]) == "droid-cli.svg")
        #expect(HarnessIcon.assetName(harness: nil, tags: ["harness": "cursor"]) == "cursor-cli.png")
        #expect(HarnessIcon.assetName(harness: nil, tags: ["harness": "agent"]) == "cursor-cli.png")
        #expect(
            HarnessIcon.assetName(harness: nil, tags: ["harness": "cursor-agent"])
                == "cursor-cli.png")
        #expect(HarnessIcon.assetName(harness: nil, tags: ["harness": "amp"]) == "amp-code.svg")
        #expect(
            HarnessIcon.assetName(harness: nil, tags: ["harness": "opensage"])
                == "opensage-adk.png")
        #expect(HarnessIcon.assetName(harness: nil, tags: ["harness": "CODEX"]) == "codex.png")
        #expect(HarnessIcon.assetName(harness: nil, tags: ["harness": "omp"]) == "oh-my-pi.png")
        #expect(
            HarnessIcon.assetName(harness: nil, tags: ["harness": "pydantic-ai"])
                == "pydantic-ai-harness.png")
    }

    @Test func userAgentSniffing() {
        #expect(
            HarnessIcon.assetName(harness: "claude-cli/2.1.202 (external, cli)", tags: nil)
                == "claude-code.png")
        #expect(HarnessIcon.assetName(harness: "codex-tui/1.0", tags: nil) == "codex.png")
        #expect(HarnessIcon.assetName(harness: "codex_exec/1.0", tags: nil) == "codex.png")
        #expect(
            HarnessIcon.assetName(harness: "grok-shell/0.2.91 (linux; aarch64)", tags: nil)
                == "grok-build.png")
        #expect(
            HarnessIcon.assetName(
                harness: "opencode/1.17.14 ai-sdk/provider-utils/4.0.23 runtime/bun/1.3.14",
                tags: nil) == "opencode.png")
        #expect(
            HarnessIcon.assetName(harness: "QwenCode/0.19.3 (linux; arm64)", tags: nil)
                == "qwen-code.png")
        #expect(HarnessIcon.assetName(harness: "factory-cli/1.2.3", tags: nil) == "droid-cli.svg")
        #expect(
            HarnessIcon.assetName(harness: "kimi-code-cli/0.20.2", tags: nil) == "kimi-code.jpg")
        #expect(HarnessIcon.assetName(harness: "omp", tags: nil) == "oh-my-pi.png")
        #expect(HarnessIcon.assetName(harness: "pydantic-ai", tags: nil) == "pydantic-ai-harness.png")
        #expect(HarnessIcon.assetName(harness: "jcode", tags: nil) == "jcode.png")
        #expect(HarnessIcon.assetName(harness: "Bun/1.3.14", tags: nil) == nil)
        #expect(HarnessIcon.assetName(harness: "OpenAI/JS 6.26.0", tags: nil) == nil)
        #expect(HarnessIcon.assetName(harness: "OpenAI/Python 2.44.0", tags: nil) == nil)
        #expect(HarnessIcon.assetName(harness: "curl/8.7.1", tags: nil) == nil)
        #expect(HarnessIcon.assetName(harness: "python-requests/2.31", tags: nil) == nil)
        #expect(HarnessIcon.assetName(harness: nil, tags: nil) == nil)
        #expect(HarnessIcon.assetName(harness: "alexandria-ping", tags: [:]) == nil)
    }

    @Test func tagBeatsUserAgent() {
        #expect(
            HarnessIcon.assetName(
                harness: "OpenAI/JS 6.26.0", tags: ["harness": "pi"]) == "pi.svg")
        #expect(
            HarnessIcon.assetName(
                harness: "claude-cli/2.0", tags: ["harness": "codex"]) == "codex.png")
        #expect(
            HarnessIcon.assetName(
                harness: "grok-shell/0.2.91", tags: ["harness": "unknown-thing"])
                == "grok-build.png")
    }
}

@Suite struct ModelProviderTests {
    @Test func providerMapping() {
        #expect(ModelProvider.provider(forModel: "claude-haiku-4-5") == "anthropic")
        #expect(ModelProvider.provider(forModel: "gpt-5.5") == "openai")
        #expect(ModelProvider.provider(forModel: "o3-mini") == "openai")
        #expect(ModelProvider.provider(forModel: "grok-code-fast-1") == "xai")
        #expect(ModelProvider.provider(forModel: "gemini-2.5-pro") == "gemini")
        #expect(ModelProvider.provider(forModel: "cursor-agent") == "cursor")
        #expect(ModelProvider.provider(forModel: "composer-2.5") == "cursor")
        #expect(ModelProvider.provider(forModel: "amp-code") == "amp")
        #expect(ModelProvider.provider(forModel: "opencode") == nil)
        #expect(ModelProvider.provider(forModel: "llama-3") == nil)
        #expect(ModelProvider.provider(forModel: "") == nil)
    }

    @Test func providersDeduped() {
        #expect(
            ModelProvider.providers(in: ["gpt-5.5", "o3", "claude-haiku-4-5", "grok-4"])
                == ["openai", "anthropic", "xai"])
        #expect(ModelProvider.providers(in: nil) == [])
        #expect(ModelProvider.providers(in: ["mystery-model"]) == [])
    }
}

@Suite struct SessionKindTagTests {
    @Test func tagKindClassification() {
        for kind in ["ping", "health", "preflight", "heartbeat", "test", "smoke"] {
            #expect(
                SessionKind.isPingOrTest(sessionId: "real", harness: nil, tags: ["kind": kind]),
                "kind=\(kind)")
        }
        #expect(SessionKind.isPingOrTest(sessionId: "real", harness: nil, tags: ["kind": "PING"]))
        #expect(!SessionKind.isPingOrTest(sessionId: "real", harness: nil, tags: ["kind": "job"]))
    }

    @Test func tagPhaseClassification() {
        for phase in ["preflight", "health", "ping"] {
            #expect(
                SessionKind.isPingOrTest(sessionId: "real", harness: nil, tags: ["phase": phase]),
                "phase=\(phase)")
        }
        #expect(
            !SessionKind.isPingOrTest(sessionId: "real", harness: nil, tags: ["phase": "run"]))
        #expect(!SessionKind.isPingOrTest(sessionId: "real", harness: nil, tags: [:]))
        #expect(!SessionKind.isPingOrTest(sessionId: "real", harness: nil, tags: nil))
    }
}

@Suite struct ListNavigationTests {
    @Test func clampingAndJumps() {
        #expect(ListNavigation.targetIndex(selected: nil, count: 0, move: .down) == nil)
        #expect(ListNavigation.targetIndex(selected: nil, count: 3, move: .down) == 0)
        #expect(ListNavigation.targetIndex(selected: nil, count: 3, move: .up) == 0)
        #expect(ListNavigation.targetIndex(selected: 0, count: 3, move: .up) == 0)
        #expect(ListNavigation.targetIndex(selected: 1, count: 3, move: .up) == 0)
        #expect(ListNavigation.targetIndex(selected: 1, count: 3, move: .down) == 2)
        #expect(ListNavigation.targetIndex(selected: 2, count: 3, move: .down) == 2)
        #expect(ListNavigation.targetIndex(selected: 2, count: 3, move: .home) == 0)
        #expect(ListNavigation.targetIndex(selected: 0, count: 3, move: .end) == 2)
        #expect(ListNavigation.targetIndex(selected: nil, count: 3, move: .end) == 2)
    }
}

@Suite struct TranscriptRenderTests {
    func makeTurn(
        id: String, ts: Int64 = 0, responseTs: Int64? = nil, model: String? = nil,
        status: Int? = nil, user: String? = nil, assistant: String? = nil, error: String? = nil
    ) -> TranscriptTurn {
        let json: [String: Any] = [
            "trace_id": id,
            "ts_request_ms": ts,
            "ts_response_ms": responseTs as Any,
            "model": model as Any,
            "status": status as Any,
            "user": user as Any,
            "assistant": assistant as Any,
            "error": error as Any,
        ]
        let data = try! JSONSerialization.data(withJSONObject: json)
        return try! JSONDecoder().decode(TranscriptTurn.self, from: data)
    }

    @Test func planDecisions() {
        let a = makeTurn(id: "a", user: "hi")
        let b = makeTurn(id: "b", assistant: "yo")
        let c = makeTurn(id: "c")
        #expect(TranscriptRender.plan(previous: nil, turns: [a]) == .rebuild)
        let empty = TranscriptRender.state(for: [])
        #expect(TranscriptRender.plan(previous: empty, turns: []) == .unchanged)
        #expect(TranscriptRender.plan(previous: empty, turns: [a]) == .rebuild)
        let one = TranscriptRender.state(for: [a])
        #expect(TranscriptRender.plan(previous: one, turns: [a]) == .unchanged)
        #expect(TranscriptRender.plan(previous: one, turns: [a, b]) == .append(from: 1))
        #expect(TranscriptRender.plan(previous: one, turns: [a, b, c]) == .append(from: 1))
        #expect(TranscriptRender.plan(previous: one, turns: []) == .rebuild)
        #expect(TranscriptRender.plan(previous: one, turns: [b]) == .rebuild)
        let two = TranscriptRender.state(for: [a, b])
        #expect(TranscriptRender.plan(previous: two, turns: [a, c]) == .rebuild)
        let pending = makeTurn(id: "p", user: "q")
        let done = makeTurn(id: "p", responseTs: 5, status: 200, user: "q", assistant: "answer")
        let pendingState = TranscriptRender.state(for: [a, pending])
        #expect(TranscriptRender.plan(previous: pendingState, turns: [a, done]) == .rebuild)
        #expect(TranscriptRender.plan(previous: pendingState, turns: [a, done, c]) == .rebuild)
    }

    #if canImport(AppKit)
    @Test func documentContents() {
        let turns = [
            makeTurn(
                id: "t1", ts: 1_700_000_000_000, responseTs: 1_700_000_001_000,
                model: "gpt-5.5", status: 200, user: "hello", assistant: "world"),
            makeTurn(id: "t2", ts: 1_700_000_002_000, status: 429, error: "upstream 429"),
        ]
        let doc = TranscriptRender.document(turns: turns)
        let text = doc.string
        #expect(text.contains("gpt-5.5"))
        #expect(text.contains("hello"))
        #expect(text.contains("world"))
        #expect(text.contains("upstream 429"))
        #expect(text.contains("· 429"))
        #expect(TranscriptRender.document(turns: []).length == 0)
    }

    @Test func perTurnCap() {
        let huge = String(repeating: "x", count: TranscriptRender.maxTurnChars + 500)
        let capped = TranscriptRender.cap(huge)
        #expect(capped.count < huge.count)
        #expect(capped.contains("truncated"))
        let fine = String(repeating: "y", count: 1000)
        #expect(TranscriptRender.cap(fine) == fine)
    }

    @Test func stressBuild500TurnsOf8kChars() {
        let body = String(repeating: "a", count: 8000)
        let turns = (0..<500).map { i in
            makeTurn(
                id: "t\(i)", ts: Int64(i) * 1000, responseTs: Int64(i) * 1000 + 500,
                model: "gpt-5.5", status: 200, user: "q\(i)", assistant: body)
        }
        let start = ContinuousClock.now
        let doc = TranscriptRender.document(turns: turns)
        let elapsed = start.duration(to: .now)
        #expect(doc.length > 500 * 8000)
        #expect(elapsed < .seconds(10))
    }

    #endif

    @Test func windowStartIndex() {
        let small = (0..<10).map { makeTurn(id: "t\($0)", assistant: "x") }
        #expect(TranscriptWindow.startIndex(turns: small, maxTurns: 200) == 0)
        #expect(TranscriptWindow.startIndex(turns: small, maxTurns: 4) == 6)
        #expect(TranscriptWindow.startIndex(turns: [], maxTurns: 200) == 0)
        let big = (0..<8).map {
            makeTurn(id: "b\($0)", assistant: String(repeating: "z", count: 1000))
        }
        #expect(TranscriptWindow.startIndex(turns: big, maxTurns: 8, maxChars: 2500) == 6)
        let single = [makeTurn(id: "one", assistant: String(repeating: "z", count: 10_000))]
        #expect(TranscriptWindow.startIndex(turns: single, maxTurns: 8, maxChars: 100) == 0)
    }
}

@Suite struct DarioModelsTests {
    @Test func adminStatusDecoding() throws {
        let json = #"""
        {"active_generation_id":"gen-4.8.139-64986","should_be_healthy":true,"issue":null,"resolved_node_bin":"/opt/homebrew/bin/node","resolved_claude_bin":"/opt/homebrew/bin/claude","runtime_version":"v22.14.0","route_enabled":true,"prompt_caches":[{"key":"cache-1","model":"claude-sonnet-4-5","source":"trace"}],"generations":[{"consecutive_failures":0,"drain_started_at":null,"id":"gen-4.8.139-64986","in_flight":2,"last_activity_ms":1783488812322,"last_probe":{"at_ms":1783488905120,"error":null,"latency_ms":956,"ok":true,"status":null},"phase":"ready","pid":84167,"port":64986,"promoted_at":1783488814161,"started_at":1783488812322,"state":"active","stderr_log":"/x/gen.err.log","stdout_log":"/x/gen.out.log","version":"4.8.139"}]}
        """#
        let status = try JSONDecoder().decode(DarioAdminStatus.self, from: Data(json.utf8))
        #expect(status.activeGenerationId == "gen-4.8.139-64986")
        #expect(status.generations.count == 1)
        let gen = status.generations[0]
        #expect(gen.version == "4.8.139")
        #expect(gen.phase == "ready")
        #expect(gen.state == "active")
        #expect(gen.pid == 84167)
        #expect(gen.port == 64986)
        #expect(gen.inFlight == 2)
        #expect(gen.consecutiveFailures == 0)
        #expect(gen.startedAt == 1_783_488_812_322)
        #expect(gen.promotedAt == 1_783_488_814_161)
        #expect(gen.stdoutLog == "/x/gen.out.log")
        #expect(gen.stderrLog == "/x/gen.err.log")
        #expect(gen.lastProbe?.ok == true)
        #expect(gen.lastProbe?.latencyMs == 956)
        #expect(gen.lastProbe?.status == nil)
        #expect(status.shouldBeHealthy == true)
        #expect(status.issue == nil)
        #expect(status.resolvedNodeBin == "/opt/homebrew/bin/node")
        #expect(status.resolvedClaudeBin == "/opt/homebrew/bin/claude")
        #expect(status.runtimeVersion == "v22.14.0")
        #expect(status.routeEnabled == true)
        #expect(status.promptCaches?.first?.model == "claude-sonnet-4-5")
    }

    @Test func logsDecoding() throws {
        let json = #"""
        {"generation_id":"gen-1","lines":300,"stderr":"boom","stdout":"[dario] up"}
        """#
        let logs = try JSONDecoder().decode(DarioLogsResponse.self, from: Data(json.utf8))
        #expect(logs.generationId == "gen-1")
        #expect(logs.stdout == "[dario] up")
        #expect(logs.stderr == "boom")
        #expect(logs.lines == 300)
    }
}
