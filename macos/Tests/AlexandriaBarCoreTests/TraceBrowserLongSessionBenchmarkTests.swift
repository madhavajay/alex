import Foundation
import Testing
@testable import AlexandriaBarCore

/// PR-safe algorithmic regression coverage for the aggregate shape of the
/// observed long-running Trace Browser session. This deliberately generates
/// all text and identifiers locally from the small JSON shape resource.
///
/// This suite does not instantiate `TraceBrowserModel`, SwiftUI, an NSWindow,
/// URLSession, or the daemon. The packaged-app benchmark remains a separate
/// rollout gate; see docs/trace-browser-long-session-core-benchmark.md.
@Suite(.serialized)
struct TraceBrowserLongSessionBenchmarkTests {
    private struct Shape: Decodable, Sendable {
        let schema: String
        let provenance: String
        let sessionId: String
        let turnCount: Int
        let durationMs: Int64
        let pageSize: Int
        let duplicateTimestampGroup: Int
        let userChars: Int
        let assistantChars: Int
        let toolEvery: Int
        let searchTurn: Int
        let searchNeedle: String

        enum CodingKeys: String, CodingKey {
            case schema, provenance
            case sessionId = "session_id"
            case turnCount = "turn_count"
            case durationMs = "duration_ms"
            case pageSize = "page_size"
            case duplicateTimestampGroup = "duplicate_timestamp_group"
            case userChars = "user_chars"
            case assistantChars = "assistant_chars"
            case toolEvery = "tool_every"
            case searchTurn = "search_turn"
            case searchNeedle = "search_needle"
        }
    }

    private struct Fixture: Sendable {
        let shape: Shape
        let turns: [TranscriptTurn]
    }

    private static let fixture: Fixture = {
        let resource = Bundle.module.url(
            forResource: "long-session-shape-v1", withExtension: "json",
            subdirectory: "Fixtures")
            ?? Bundle.module.url(
                forResource: "long-session-shape-v1", withExtension: "json")
        guard let resource else {
            fatalError("long-session aggregate shape resource is missing")
        }
        do {
            let shape = try JSONDecoder().decode(Shape.self, from: Data(contentsOf: resource))
            return Fixture(shape: shape, turns: try makeTurns(shape))
        } catch {
            fatalError("invalid long-session aggregate shape: \(error)")
        }
    }()

    @Test func aggregateShapeAndCorePagingStayBounded() {
        let shape = Self.fixture.shape
        let turns = Self.fixture.turns
        #expect(shape.schema == "alex-trace-browser-long-session-shape-v1")
        #expect(shape.provenance.contains("no captured text"))
        #expect(turns.count == 1_277)
        #expect(turns.last!.tsRequestMs - turns.first!.tsRequestMs == 54_000_000)
        #expect(turns[0].tsRequestMs == turns[1].tsRequestMs)
        #expect(turns[1].tsRequestMs == turns[2].tsRequestMs)
        #expect(turns[2].tsRequestMs != turns[3].tsRequestMs)

        let started = ContinuousClock.now
        var lower = max(0, turns.count - shape.pageSize)
        var merged = Array(turns[lower...])
        while lower > 0 {
            let nextLower = max(0, lower - shape.pageSize)
            merged = TranscriptPaging.merge(
                existing: merged, incoming: Array(turns[nextLower..<lower]))
            lower = nextLower
        }
        // Repeated tail refreshes must replace, not duplicate, durable trace ids.
        merged = TranscriptPaging.merge(
            existing: merged, incoming: Array(turns.suffix(shape.pageSize)))
        let elapsed = started.duration(to: .now)

        #expect(merged.count == shape.turnCount)
        #expect(Set(merged.map(\.traceId)).count == shape.turnCount)
        #expect(merged.map(\.traceId) == turns.map(\.traceId))
        #expect(elapsed < .seconds(5))
    }

    @Test func filteringWindowingNavigationAndStaleSelectionStayBounded() {
        let shape = Self.fixture.shape
        let turns = Self.fixture.turns

        let filterStarted = ContinuousClock.now
        let miss = TranscriptFilter.result(
            turns: turns, filterTab: 0, query: "needle-that-is-not-present")
        let match = TranscriptFilter.result(
            turns: turns, filterTab: 0, query: shape.searchNeedle)
        let tools = TranscriptFilter.result(turns: turns, filterTab: 3, query: "")
        let filterElapsed = filterStarted.duration(to: .now)

        let expectedToolTurns = (shape.turnCount - 1) / shape.toolEvery + 1
        #expect(miss.entries.isEmpty)
        #expect(miss.totalCount == shape.turnCount * 2)
        #expect(match.entries.count == 1)
        #expect(match.entries[0].turnId == traceId(shape.searchTurn))
        #expect(match.entries[0].role == .assistant)
        #expect(tools.entries.count == expectedToolTurns * 2)
        #expect(filterElapsed < .seconds(5))

        let windowStarted = ContinuousClock.now
        let start = TranscriptWindow.startIndex(
            turns: turns, maxTurns: TranscriptWindow.defaultMaxTurns,
            maxChars: TranscriptWindow.defaultMaxChars)
        let window = Array(turns[start...])
        let windowElapsed = windowStarted.duration(to: .now)
        let windowChars = window.reduce(0) {
            $0 + ($1.user?.count ?? 0) + ($1.assistant?.count ?? 0) + 64
        }
        #expect(window.count <= TranscriptWindow.defaultMaxTurns)
        #expect(windowChars <= TranscriptWindow.defaultMaxChars + shape.userChars
            + shape.assistantChars + 64)
        #expect(windowElapsed < .seconds(1))

        let ids = turns.map(\.traceId)
        #expect(ListNavigation.targetIndex(selected: nil, count: ids.count, move: .home) == 0)
        #expect(ListNavigation.targetIndex(selected: 0, count: ids.count, move: .end)
            == ids.count - 1)
        #expect(TraceInspectorSelection.target(currentTraceId: nil, in: ids) == ids.last)
        #expect(TraceInspectorSelection.previous(before: ids[638], in: ids) == ids[637])

        // Core owns the selection primitive used by the app's request-generation
        // guard. Simulate an in-flight long-session page followed by a user
        // selection change: a caller checking the captured id must reject it.
        // Actual Task/URLSession cancellation is intentionally left to the
        // packaged-app benchmark.
        var selection = SessionSelection()
        #expect(selection.userSelect(shape.sessionId) == .selected(shape.sessionId))
        let inFlightSession = selection.selectedId
        #expect(selection.userSelect("synthetic-short-session")
            == .selected("synthetic-short-session"))
        #expect(inFlightSession != selection.selectedId)
        let acceptStalePage = inFlightSession == selection.selectedId
        #expect(!acceptStalePage)
    }

    #if canImport(AppKit)
    @Test func boundedMacRenderBuildPreservesEveryWindowedTurn() {
        let shape = Self.fixture.shape
        let turns = Self.fixture.turns
        let start = TranscriptWindow.startIndex(
            turns: turns, maxTurns: TranscriptWindow.defaultMaxTurns,
            maxChars: TranscriptWindow.defaultMaxChars)
        let window = Array(turns[start...])

        let renderStarted = ContinuousClock.now
        let built = TranscriptRender.build(turns: window, firstTurnNumber: start + 1)
        let renderElapsed = renderStarted.duration(to: .now)

        #expect(built.turnRanges.count == window.count)
        #expect(built.turnRanges.map(\.traceId) == window.map(\.traceId))
        #expect(built.turnRanges.last?.range.upperBound == built.text.length)
        #expect(built.text.length <= window.count
            * (shape.userChars + shape.assistantChars + 2_048))
        #expect(renderElapsed < .seconds(15))

        let tail = Array(turns.suffix(shape.pageSize))
        let earlierAndTail = Array(turns.suffix(shape.pageSize * 2))
        let tailState = TranscriptRender.state(for: tail)
        #expect(TranscriptRender.plan(previous: tailState, turns: tail) == .unchanged)
        // Loading an earlier page changes the first durable id, so the bounded
        // classic document must rebuild instead of appending out of order.
        #expect(TranscriptRender.plan(previous: tailState, turns: earlierAndTail) == .rebuild)
    }
    #endif

    private static func makeTurns(_ shape: Shape) throws -> [TranscriptTurn] {
        precondition(shape.turnCount > 1)
        precondition(shape.pageSize > 0)
        precondition(shape.duplicateTimestampGroup > 0)
        precondition(shape.toolEvery > 0)
        precondition((0..<shape.turnCount).contains(shape.searchTurn))

        let baseMs: Int64 = 1_768_435_200_000
        let finalTimestampGroup = (shape.turnCount - 1) / shape.duplicateTimestampGroup
        let rows: [[String: Any]] = (0..<shape.turnCount).map { index in
            let timestampGroup = index / shape.duplicateTimestampGroup
            let offset = shape.durationMs * Int64(timestampGroup)
                / Int64(max(1, finalTimestampGroup))
            let toolTurn = index.isMultiple(of: shape.toolEvery)
            let userPrefix = toolTurn
                ? "[tool result] deterministic synthetic output for turn \(index)\n"
                : "synthetic user turn \(index)\n"
            let assistantPrefix = index == shape.searchTurn
                ? "\(shape.searchNeedle) synthetic assistant turn \(index)\n"
                : "synthetic assistant turn \(index)\n"
            var row: [String: Any] = [
                "trace_id": traceId(index),
                "ts_request_ms": baseMs + offset,
                "ts_response_ms": baseMs + offset + 250,
                "model": "gpt-synthetic-code",
                "provider": "synthetic",
                "status": 200,
                "input_tokens": 20_000 + index,
                "output_tokens": 500,
                "user": sizedText(
                    prefix: userPrefix, fill: "user-\(index)-abcdef0123456789|",
                    count: shape.userChars),
                "assistant": sizedText(
                    prefix: assistantPrefix, fill: "assistant-\(index)-fedcba9876543210|",
                    count: shape.assistantChars),
            ]
            if toolTurn {
                row["tool_calls"] = [[
                    "id": "call-\(index)",
                    "name": "exec_command",
                    "arguments": "{\"command\":\"synthetic command \(index)\"}",
                ]]
            }
            return row
        }
        let data = try JSONSerialization.data(withJSONObject: rows)
        return try JSONDecoder().decode([TranscriptTurn].self, from: data)
    }

    private static func sizedText(prefix: String, fill: String, count: Int) -> String {
        precondition(count > 0)
        if prefix.count >= count { return String(prefix.prefix(count)) }
        let remaining = count - prefix.count
        let repeated = String(repeating: fill, count: remaining / max(1, fill.count) + 1)
        return prefix + String(repeated.prefix(remaining))
    }

    private static func traceId(_ index: Int) -> String {
        String(format: "synthetic-trace-%06d", index)
    }
}
