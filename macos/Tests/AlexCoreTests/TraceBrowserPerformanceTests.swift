import AppKit
import Foundation
import Testing
@testable import Alex
@testable import AlexCore

@Suite(.serialized) struct TraceBrowserPerformanceTests {
    private static func makeTurn(
        _ index: Int, text: String, traceId: String? = nil, completedMs: Int64? = nil,
        accountId: String? = nil
    ) -> TranscriptTurn {
        TranscriptTurn(
            traceId: traceId ?? "perf-\(index)", tsRequestMs: Int64(index),
            tsResponseMs: completedMs ?? Int64(index + 1),
            model: "gpt-5.6-sol", provider: "openai", status: 200,
            inputTokens: 100, outputTokens: 100, reasoningEffort: nil,
            thinkingBudget: nil, costUsd: nil, billingBucket: nil, accountId: accountId,
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
        try await waitUntil {
            await MainActor.run { model.turns.count == TranscriptApplyPolicy.initialTurnCount }
        }
        let count = await spy.count
        model.stop()
        #expect(count == TranscriptApplyPolicy.initialTurnCount)
        #expect(model.hiddenTurnCount == 500 - TranscriptApplyPolicy.initialTurnCount)
    }

    @Test @MainActor func hugeTurnIsCappedAndAppliedWithinBatchBudgets() async throws {
        let huge = String(repeating: "multi-hundred-kb payload ", count: 14_000)
        let model = TraceBrowserModel(
            store: SnapshotStore(),
            transcriptPageFetcher: { sessionId, _, _ in
                TranscriptPageResponse(
                    sessionId: sessionId, turns: [Self.metadata(sessionId)], limit: 50,
                    hasMore: false, nextCursor: nil)
            },
            traceTurnFetcher: { traceId in
                Self.makeTurn(0, text: huge, traceId: traceId)
            })
        model.selectFromUser("huge-session")
        try await waitUntil {
            await MainActor.run {
                model.turns.count == 1 && model.transcriptEntries.count == 2
                    && !model.transcriptRendering
            }
        }
        let turn = try #require(model.turns.first)
        #expect(turn.user?.count == TurnTextCap.maxChars)
        #expect(turn.assistant?.count == TurnTextCap.maxChars)
        #expect(model.lastTurnApplyBatchCharacterCounts.count == 1)
        #expect(model.lastTurnApplyBatchCharacterCounts.max() ?? 0
            <= TranscriptApplyPolicy.maxCharsPerBatch)
        #expect(model.lastChatPaneBatchCharacterCounts.count == 2)
        #expect(model.lastChatPaneBatchCharacterCounts.max() ?? 0
            <= TranscriptApplyPolicy.maxCharsPerBatch)

        let initialTurns = (0..<TranscriptApplyPolicy.initialTurnCount).map {
            TranscriptInlineDisplay.capped(Self.makeTurn($0, text: huge))
        }
        let turnBatches = TranscriptApplyPolicy.turnBatches(initialTurns)
        #expect(turnBatches.count > 1)
        #expect(turnBatches.allSatisfy { batch in
            batch.reduce(0) {
                $0 + TranscriptApplyPolicy.inlineCharacterCount($1)
            } <= TranscriptApplyPolicy.maxCharsPerBatch
        })
        let document = TranscriptRender.build(turns: initialTurns)
        let chunks = TranscriptStorageBatch.chunks(document.text)
        #expect(chunks.allSatisfy {
            $0.length <= TranscriptApplyPolicy.maxCharsPerBatch + 2
        })
        let textView = TranscriptTextView(usingTextLayoutManager: true)
        textView.frame = NSRect(x: 0, y: 0, width: 820, height: 640)
        textView.isVerticallyResizable = true
        textView.textContainer?.widthTracksTextView = true
        let layoutStart = ContinuousClock.now
        for (index, chunk) in chunks.enumerated() {
            if index == 0 {
                textView.textStorage?.setAttributedString(chunk)
            } else {
                textView.textStorage?.append(chunk)
            }
        }
        textView.invalidateBubbleRects()
        textView.rebuildBubbleRects()
        let layoutMs = milliseconds(layoutStart.duration(to: .now))
        #expect(layoutMs < 100)
        model.stop()
        print(
            "trace-browser huge_turn_inline_chars=\(TranscriptApplyPolicy.inlineCharacterCount(turn)) chat_batches=\(model.lastChatPaneBatchCharacterCounts.count) initial_turns=\(initialTurns.count) classic_batches=\(chunks.count) initial_window_layout_ms=\(layoutMs)")
    }

    @Test @MainActor func renderedArtifactCacheHitsAndCompletionChangeMisses() {
        let longText = String(repeating: "rendered artifact payload ", count: 500)
        let turns = (0..<500).map { Self.makeTurn($0, text: longText) }
        let cache = RenderedArtifactCache(
            countLimit: 600, byteLimit: 128 * 1_024 * 1_024,
            startMaintenance: false)

        let missStart = ContinuousClock.now
        let first = renderMessages(turns, cache: cache)
        let missMs = milliseconds(missStart.duration(to: .now))
        let afterMiss = cache.stats

        let hitStart = ContinuousClock.now
        let second = renderMessages(turns, cache: cache)
        let hitMs = milliseconds(hitStart.duration(to: .now))
        let afterHit = cache.stats

        #expect(first == second)
        #expect(afterMiss.misses == 500)
        #expect(afterHit.hits == 500)
        #expect(hitMs * 10 <= missMs || hitMs < 20)

        let changed = Self.makeTurn(
            0, text: longText, traceId: turns[0].traceId, completedMs: 99_999)
        _ = TranscriptChatMessages.cachedMessages(
            for: changed, harnessName: "Codex", cache: cache)
        #expect(cache.stats.misses == 501)
        print(
            "S5 rendered-artifact miss_ms=\(missMs) hit_ms=\(hitMs) speedup=\(missMs / max(hitMs, 0.001))x changed_completed_misses=1")
    }

    @Test @MainActor func renderedArtifactCachePressureIdleAndBounds() async {
        var now: TimeInterval = 1_000
        let cache = RenderedArtifactCache(
            countLimit: 2, byteLimit: 1_024, idleInterval: 300,
            clock: { now }, startMaintenance: false)
        let firstKey = RenderedArtifactKey(
            traceId: "first", completedMs: 1, discriminator: "json-v1")
        let secondKey = RenderedArtifactKey(
            traceId: "second", completedMs: 2, discriminator: "sse-v1")
        let thirdKey = RenderedArtifactKey(
            traceId: "third", completedMs: 3, discriminator: "chat-v1")
        cache.insertFormatted(
            AttributedStringBox(NSAttributedString(string: "{}")), for: firstKey)
        let parsed = RenderedSSEPages.parse("data: {\"ok\":true}\n\n")
        cache.insertSSE(parsed, for: secondKey)
        cache.insertChat(
            TranscriptChatMessages.messages(
                for: Self.makeTurn(3, text: "small"), harnessName: "Codex"),
            for: thirdKey)
        #expect(cache.count == 2)
        #expect(cache.approximateByteCost <= cache.byteLimit)
        #expect(cache.formatted(for: firstKey) == nil)
        #expect(cache.sse(for: secondKey) == parsed)

        now += 301
        cache.evictIdle()
        #expect(cache.count == 0)

        cache.insertSSE(parsed, for: secondKey)
        #expect(cache.count == 1)
        cache.handleMemoryPressure()
        #expect(cache.count == 0)

        cache.insertFormatted(
            AttributedStringBox(NSAttributedString(string: String(repeating: "x", count: 300))),
            for: firstKey)
        #expect(cache.count == 0)
        #expect(cache.approximateByteCost == 0)
    }

    @Test func hangBreadcrumbSnapshotAndLogRotationPolicy() {
        TraceBrowserSignpost.resetForTesting()
        for index in 0..<(TraceBrowserSignpost.breadcrumbLimit + 4) {
            let interval = TraceBrowserSignpost.begin(.turnFetch, "trace_id=trace-\(index)")
            TraceBrowserSignpost.end(interval, "bytes=\(index)")
        }
        let active = TraceBrowserSignpost.begin(
            .transcriptApply, "turns=7 total_chars=70000")
        let snapshot = TraceBrowserSignpost.snapshot()
        #expect(snapshot.breadcrumbs.count == TraceBrowserSignpost.breadcrumbLimit)
        #expect(snapshot.active.map(\.operation) == ["transcript apply"])
        let line = UIHangLog.formatLine(
            timestamp: Date(timeIntervalSince1970: 0), durationMilliseconds: 825,
            snapshot: snapshot)
        #expect(line.contains("duration_ms=825.0"))
        #expect(line.contains("operation=transcript apply"))
        #expect(line.contains("turns=7 total_chars=70000"))
        #expect(!UIHangLog.shouldRotate(fileBytes: UIHangLog.maxFileBytes))
        #expect(UIHangLog.shouldRotate(
            fileBytes: UIHangLog.maxFileBytes, incomingBytes: 1))
        #expect(UIHangLog.shouldRotate(fileBytes: 8, incomingBytes: 3, limit: 10))
        TraceBrowserSignpost.end(active)
    }

    @Test func hangLogCanBePreparedBeforeFirstFreeze() throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        let url = UIHangLog.fileURL(home: root)
        try UIHangLog.prepareForReveal(at: url)
        #expect(FileManager.default.fileExists(atPath: url.path))
        try FileManager.default.removeItem(at: root)
    }

    @Test func hangWatchdogHonorsExplicitUserSetting() throws {
        let suite = try #require(UserDefaults(suiteName: UUID().uuidString))
        suite.set(false, forKey: UIHangWatchdog.defaultsKey)
        #expect(!UIHangWatchdog.isEnabled(defaults: suite))
        suite.set(true, forKey: UIHangWatchdog.defaultsKey)
        #expect(UIHangWatchdog.isEnabled(defaults: suite))
    }

    @Test @MainActor func topBarTurnFilteringCapsAndFiltersInPreparationStep() throws {
        let huge = String(repeating: "x", count: TurnTextCap.maxChars + 500)
        let chosen = Self.makeTurn(1, text: huge, accountId: "chosen")
        let other = Self.makeTurn(2, text: "small", accountId: "other")
        let prepared = try #require(TraceBrowserModel.prepareDisplayTurns(
            [chosen, other], query: OmniQuery.parse("account:chosen")))
        #expect(prepared.turns.map(\.traceId) == [chosen.traceId])
        #expect((prepared.turns[0].user?.count ?? 0) <= TurnTextCap.maxChars)
    }

    @MainActor
    private func renderMessages(
        _ turns: [TranscriptTurn], cache: RenderedArtifactCache
    ) -> [[MessageDisplay]] {
        turns.enumerated().map { index, turn in
            TranscriptChatMessages.cachedMessages(
                for: turn, harnessName: "Codex",
                previousTurn: index > 0 ? turns[index - 1] : nil, cache: cache)
        }
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
