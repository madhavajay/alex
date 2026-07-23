import AppKit
import Foundation
import Testing
@testable import Alex
@testable import AlexCore

@Suite(.serialized) struct TraceBrowserPerformanceTests {
    private static func makeTurn(
        _ index: Int, text: String, traceId: String? = nil
    ) -> TranscriptTurn {
        TranscriptTurn(
            traceId: traceId ?? "perf-\(index)", tsRequestMs: Int64(index), tsResponseMs: Int64(index + 1),
            model: "gpt-5.6-sol", provider: "openai", status: 200,
            inputTokens: 100, outputTokens: 100, reasoningEffort: nil,
            thinkingBudget: nil, costUsd: nil, billingBucket: nil, accountId: nil,
            viaDario: nil, darioGeneration: nil, error: nil, errorKind: nil,
            errorCode: nil, errorClass: nil, user: "user \(index) \(text)",
            assistant: "assistant \(index) \(text)", toolCalls: nil,
            assistantBlocks: nil, executedTools: nil, attempts: nil,
            substituted: nil, substitutionReason: nil)
    }

    private func milliseconds(_ duration: Duration) -> Double {
        Double(duration.components.seconds) * 1_000
            + Double(duration.components.attoseconds) / 1e15
    }

    @Test @MainActor func fiveHundredTurnFilterWindowAndRectCacheBudgets() {
        let longText = String(repeating: "long transcript payload ", count: 360)
        let turns = (0..<500).map { Self.makeTurn($0, text: longText) }

        let filterStart = ContinuousClock.now
        let filtered = TranscriptFilter.result(
            turns: turns, filterTab: 0, query: "not-present")
        let filterMs = milliseconds(filterStart.duration(to: .now))
        #expect(filtered.entries.isEmpty)
        #expect(filterMs < 1_000)

        let renderStart = ContinuousClock.now
        let document = TranscriptRender.build(
            turns: turns, firstTurnNumber: 1, harnessName: "Codex",
            icons: .none, rawMode: false)
        let renderMs = milliseconds(renderStart.duration(to: .now))
        #expect(document.turnRanges.count == 500)
        #expect(renderMs < 2_000)

        let textView = TranscriptTextView(usingTextLayoutManager: true)
        textView.frame = NSRect(x: 0, y: 0, width: 820, height: 640)
        textView.isVerticallyResizable = true
        textView.textContainer?.widthTracksTextView = true
        textView.textStorage?.setAttributedString(document.text)
        textView.invalidateBubbleRects()
        let rectStart = ContinuousClock.now
        textView.rebuildBubbleRects()
        let rectBuildMs = milliseconds(rectStart.duration(to: .now))
        #expect(textView.bubbleRectCount >= 500)
        #expect(rectBuildMs < 5_000)

        textView.invalidateBubbleRects()
        let legacyStart = ContinuousClock.now
        textView.rebuildBubbleRects()
        let legacyRectPassMs = milliseconds(legacyStart.duration(to: .now))
        let cachedStart = ContinuousClock.now
        var observed = 0
        for _ in 0..<10_000 { observed &+= textView.bubbleRectCount }
        let cachedLookupMs = milliseconds(cachedStart.duration(to: .now))
        #expect(observed > 0)
        #expect(cachedLookupMs < 100)
        print(
            "T3 trace-browser filter_ms=\(filterMs) window_rebuild_ms=\(renderMs) rect_build_ms=\(rectBuildMs) legacy_rect_pass_ms=\(legacyRectPassMs) cached_rect_lookups_ms=\(cachedLookupMs)")
    }

    @Test @MainActor func selectingAnotherSessionCancelsTurnFetches() async throws {
        let spy = TurnFetchSpy()
        let model = TraceBrowserModel(
            store: SnapshotStore(),
            transcriptPageFetcher: { sessionId, _, cursor in
                TranscriptPageResponse(
                    sessionId: sessionId,
                    turns: cursor == nil ? [Self.metadata(sessionId)] : [],
                    limit: 50, hasMore: false, nextCursor: nil)
            },
            traceTurnFetcher: { traceId in
                try await spy.fetch(traceId)
            })
        model.selectFromUser("session-a")
        try await waitUntil { await spy.didStart("session-a-turn") }
        let cancelStart = ContinuousClock.now
        model.selectFromUser("session-b")
        try await waitUntil { await spy.wasCancelled("session-a-turn") }
        let cancelMs = milliseconds(cancelStart.duration(to: .now))
        model.stop()
        #expect(cancelMs < 250)
        print("T3 trace-browser cancellation_ms=\(cancelMs)")
    }

    @Test @MainActor func modelPagesFiveHundredTurnsAndFetchesOnlyInitialWindow() async throws {
        let metadata = (0..<500).map { index in
            TranscriptTurnMetadata(
                traceId: "page-\(index)", tsRequestMs: Int64(index), tsResponseMs: Int64(index + 1),
                harness: "codex", model: "gpt-5.6-sol", provider: "openai",
                status: 200, inputTokens: 1, outputTokens: 1, error: nil,
                errorKind: nil, attempts: nil, substituted: nil,
                substitutionReason: nil, streamed: 1, hasRequest: true,
                hasResponse: true)
        }
        let spy = WindowFetchSpy()
        let model = TraceBrowserModel(
            store: SnapshotStore(),
            transcriptPageFetcher: { sessionId, limit, cursor in
                let start = cursor.flatMap { cursor in
                    metadata.firstIndex { $0.traceId == cursor.afterId }.map { $0 + 1 }
                } ?? 0
                let end = min(metadata.count, start + limit)
                let page = Array(metadata[start..<end])
                let hasMore = end < metadata.count
                let next = hasMore ? page.last.map {
                    TranscriptCursor(afterMs: $0.tsRequestMs, afterId: $0.traceId)
                } : nil
                return TranscriptPageResponse(
                    sessionId: sessionId, turns: page, limit: limit,
                    hasMore: hasMore, nextCursor: next)
            },
            traceTurnFetcher: { traceId in
                await spy.record(traceId)
                let index = Int(traceId.split(separator: "-").last ?? "0") ?? 0
                return Self.makeTurn(index, text: "loaded", traceId: traceId)
            })
        model.selectFromUser("paged-session")
        try await waitUntil { await MainActor.run { model.turns.count == 24 } }
        let count = await spy.count
        model.stop()
        #expect(count == 24)
        #expect(model.hiddenTurnCount == 476)
    }

    private static func metadata(_ sessionId: String) -> TranscriptTurnMetadata {
        TranscriptTurnMetadata(
            traceId: "\(sessionId)-turn", tsRequestMs: 1, tsResponseMs: 2,
            harness: "codex", model: "gpt-5.6-sol", provider: "openai",
            status: 200, inputTokens: 1, outputTokens: 1, error: nil,
            errorKind: nil, attempts: nil, substituted: nil,
            substitutionReason: nil, streamed: 1, hasRequest: true,
            hasResponse: true)
    }

    private func waitUntil(
        _ condition: @escaping @Sendable () async -> Bool
    ) async throws {
        for _ in 0..<100 {
            if await condition() { return }
            try await Task.sleep(for: .milliseconds(10))
        }
        Issue.record("condition did not become true")
    }
}

private actor TurnFetchSpy {
    private var started: Set<String> = []
    private var cancelled: Set<String> = []

    func fetch(_ traceId: String) async throws -> TranscriptTurn {
        started.insert(traceId)
        do {
            try await Task.sleep(for: .seconds(30))
        } catch {
            cancelled.insert(traceId)
            throw error
        }
        throw CancellationError()
    }

    func didStart(_ traceId: String) -> Bool { started.contains(traceId) }
    func wasCancelled(_ traceId: String) -> Bool { cancelled.contains(traceId) }
}

private actor WindowFetchSpy {
    private var traceIds: Set<String> = []

    var count: Int { traceIds.count }
    func record(_ traceId: String) { traceIds.insert(traceId) }
}
