import Foundation
import Testing
@testable import AlexandriaBarCore

@Suite struct TranscriptFilterTests {
    private func makeTurn(traceId: String, user: String, assistant: String) -> TranscriptTurn {
        TranscriptTurn(
            traceId: traceId, tsRequestMs: 0, tsResponseMs: nil, model: "gpt", provider: "openai",
            status: 200, inputTokens: nil, outputTokens: nil, reasoningEffort: nil,
            thinkingBudget: nil, costUsd: nil, billingBucket: nil, accountId: nil, viaDario: nil,
            darioGeneration: nil, error: nil, errorKind: nil, errorCode: nil, errorClass: nil,
            user: user, assistant: assistant, toolCalls: nil,
            assistantBlocks: nil, executedTools: nil, bodyErrors: nil, bodyTruncations: nil,
            stages: nil, stageError: nil)
    }

    /// Regression for the "typing in the transcript filter freezes the
    /// window" bug: `entries()` used to join+search the full, uncapped text
    /// of every turn synchronously. On a 1,000-turn session with
    /// multi-hundred-KB messages that took long enough to hang the UI. This
    /// asserts the (now capped) scan stays well under a generous wall-clock
    /// budget even when nothing matches (worst case: every turn scanned in
    /// full up to the cap).
    @Test func staysFastOnLargeSessionWithHugeMessages() {
        let bigText = String(repeating: "lorem ipsum dolor sit amet, consectetur adipiscing. ", count: 4000)
        #expect(bigText.count > 200_000)
        let turns = (0..<1000).map { i in
            makeTurn(
                traceId: "t\(i)", user: "user message \(i) " + bigText,
                assistant: "assistant reply \(i) " + bigText)
        }
        let start = ContinuousClock.now
        let entries = TranscriptFilter.entries(turns: turns, filterTab: 0, query: "needle-not-present")
        let elapsed = start.duration(to: .now)
        #expect(entries.isEmpty)
        #expect(elapsed < .seconds(1))
    }

    @Test func emptyQueryReturnsEveryMessage() {
        let turns = [
            makeTurn(traceId: "t1", user: "hello", assistant: "world"),
            makeTurn(traceId: "t2", user: "", assistant: "reply only"),
        ]
        let entries = TranscriptFilter.entries(turns: turns, filterTab: 0, query: "")
        #expect(entries.count == 3)
    }

    @Test func resultCarriesUnfilteredTotalWithoutSecondScan() {
        let turns = [
            makeTurn(traceId: "t1", user: "needle", assistant: "reply"),
            makeTurn(traceId: "t2", user: "", assistant: "other"),
        ]
        let result = TranscriptFilter.result(turns: turns, filterTab: 0, query: "needle")
        #expect(result.entries.count == 1)
        #expect(result.totalCount == 3)
    }

    @Test func queryMatchesWithinCapAndMissesBeyondIt() {
        let padding = String(repeating: "x", count: TranscriptFilter.searchCharLimit + 500)
        let turnNear = makeTurn(traceId: "near", user: "needle at start " + padding, assistant: "")
        let turnFar = makeTurn(traceId: "far", user: padding + " needle at end", assistant: "")
        let entries = TranscriptFilter.entries(
            turns: [turnNear, turnFar], filterTab: 0, query: "needle")
        #expect(entries.map(\.turnId) == ["near"])
    }

    @Test func filterTabRestrictsToRole() {
        let turns = [makeTurn(traceId: "t1", user: "hi", assistant: "there")]
        let userOnly = TranscriptFilter.entries(turns: turns, filterTab: 1, query: "")
        #expect(userOnly.map(\.role) == [.user])
        let modelOnly = TranscriptFilter.entries(turns: turns, filterTab: 2, query: "")
        #expect(modelOnly.map(\.role) == [.assistant])
    }

    /// Tools tab (3) must include the "harness" reply carrying a tool's
    /// result back to the model — it's rendered as role .user structurally
    /// (see TranscriptChatMessages), but is conceptually tool activity, not
    /// literal user input.
    @Test func toolsTabIncludesHarnessToolResultTurn() {
        let plainUserTurn = makeTurn(traceId: "t1", user: "hello", assistant: "hi there")
        let toolResultTurn = makeTurn(
            traceId: "t2", user: "[tool result] file contents", assistant: "next step")
        let turns = [plainUserTurn, toolResultTurn]
        let toolsTab = TranscriptFilter.entries(turns: turns, filterTab: 3, query: "")
        #expect(toolsTab.contains { $0.turnId == "t2" && $0.role == .user })
        #expect(!toolsTab.contains { $0.turnId == "t1" && $0.role == .user })
    }
}
