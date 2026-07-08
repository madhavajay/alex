import Foundation
import SwiftUI
import Testing
@testable import AlexandriaBarCore

@Suite struct SessionTableTests {
    private func session(_ json: [String: Any]) -> TraceSession {
        var full: [String: Any] = ["session_id": "s", "first_ts_ms": 0, "last_ts_ms": 0, "trace_count": 0]
        full.merge(json) { _, new in new }
        let data = try! JSONSerialization.data(withJSONObject: full)
        return try! JSONDecoder().decode(TraceSession.self, from: data)
    }

    @Test func rowFromFullJSON() throws {
        let json = #"""
        {"errors":2,"first_ts_ms":1783484392318,"harness":"codex","last_status":200,"last_ts_ms":1783484841250,"models":["grok-code-fast-1","claude-4"],"run_id":"run-77","session_id":"auto-36237cced1dcc659-extra","tags":{"task":"sparql","job":"j1","empty":""},"total_cost_usd":0.00005262,"total_input_tokens":426,"total_output_tokens":9,"trace_count":3}
        """#
        let decoded = try JSONDecoder().decode(TraceSession.self, from: Data(json.utf8))
        let row = SessionRow(session: decoded)
        #expect(row.id == decoded.sessionId)
        #expect(row.sessionShort == "auto-36237…59-extra")
        #expect(row.lastTsMs == 1_783_484_841_250)
        #expect(row.lastTs == Date(timeIntervalSince1970: 1_783_484_841.250))
        #expect(row.models == "grok-code-fast-1, claude-4")
        #expect(row.providers == ["xai", "anthropic"])
        #expect(row.harness == "codex")
        #expect(row.turns == 3)
        #expect(row.tokensIn == 426)
        #expect(row.tokensOut == 9)
        #expect(row.cost == 0.00005262)
        #expect(row.errors == 2)
        #expect(row.runId == "run-77")
        #expect(row.tagsSummary == "job=j1 task=sparql")
        #expect(row.kindBadge == nil)
        #expect(!row.isPingOrTest)
        #expect(row.iconAsset == "codex.png")
    }

    @Test func rowFromSparseJSON() {
        let row = SessionRow(session: session(["session_id": "short-id", "trace_count": 1]))
        #expect(row.sessionShort == "short-id")
        #expect(row.models.isEmpty)
        #expect(row.providers.isEmpty)
        #expect(row.harness.isEmpty)
        #expect(row.harnessRaw == nil)
        #expect(row.tokensIn == 0)
        #expect(row.tokensOut == 0)
        #expect(row.cost == 0)
        #expect(row.errors == 0)
        #expect(row.runId.isEmpty)
        #expect(row.tagsSummary.isEmpty)
        #expect(row.iconAsset == nil)
    }

    @Test func shortSessionId() {
        #expect(SessionRow.shortId("exactly-22-characters-") == "exactly-22-characters-")
        #expect(SessionRow.shortId("exactly-23-characters-x") == "exactly-23…acters-x")
        #expect(SessionRow.shortId("abcdefghijklmnopqrstuvwxyz") == "abcdefghij…stuvwxyz")
    }

    @Test func kindBadges() {
        #expect(SessionKind.badge(sessionId: "a", harness: "alexandria-ping") == "ping")
        #expect(SessionKind.badge(sessionId: "a", harness: nil, tags: ["kind": "smoke"]) == "test")
        #expect(SessionKind.badge(sessionId: "a", harness: nil, tags: ["phase": "preflight"]) == "ping")
        #expect(SessionKind.badge(sessionId: "tsh-1", harness: nil) == "test")
        #expect(SessionKind.badge(sessionId: "smoke-2", harness: nil) == "test")
        #expect(SessionKind.badge(sessionId: "real", harness: "claude-code") == nil)
        let row = SessionRow(session: session(["session_id": "tsh-9"]))
        #expect(row.kindBadge == "test")
        #expect(row.isPingOrTest)
    }

    @Test func filterThenSortPipeline() {
        let sessions = [
            session(["session_id": "old-cheap", "last_ts_ms": 100, "total_cost_usd": 0.01]),
            session(["session_id": "new-pricey", "last_ts_ms": 300, "total_cost_usd": 0.90]),
            session(["session_id": "tsh-ping", "last_ts_ms": 400, "total_cost_usd": 0.50]),
            session(["session_id": "mid-free", "last_ts_ms": 200]),
        ]
        let rowsById = SessionTable.rowsById(sessions)
        #expect(rowsById.count == 4)

        let newestFirst = SessionTable.visibleRows(
            sessions: sessions, rowsById: rowsById, showPings: false,
            query: OmniQuery(), serverMatches: nil,
            sortOrder: SessionTable.defaultSortOrder())
        #expect(newestFirst.map(\.id) == ["new-pricey", "mid-free", "old-cheap"])

        let withPings = SessionTable.visibleRows(
            sessions: sessions, rowsById: rowsById, showPings: true,
            query: OmniQuery(), serverMatches: nil,
            sortOrder: SessionTable.defaultSortOrder())
        #expect(withPings.map(\.id) == ["tsh-ping", "new-pricey", "mid-free", "old-cheap"])

        let byCost = SessionTable.visibleRows(
            sessions: sessions, rowsById: rowsById, showPings: false,
            query: OmniQuery(), serverMatches: nil,
            sortOrder: [KeyPathComparator(\.cost)])
        #expect(byCost.map(\.id) == ["mid-free", "old-cheap", "new-pricey"])

        let filtered = SessionTable.visibleRows(
            sessions: sessions, rowsById: rowsById, showPings: false,
            query: OmniQuery.parse("session:e"), serverMatches: nil,
            sortOrder: [KeyPathComparator(\.sessionShort)])
        #expect(filtered.map(\.id) == ["mid-free", "new-pricey", "old-cheap"])

        let missingRows = SessionTable.visibleRows(
            sessions: sessions, rowsById: [:], showPings: false,
            query: OmniQuery(), serverMatches: nil,
            sortOrder: SessionTable.defaultSortOrder())
        #expect(missingRows.map(\.id) == ["new-pricey", "mid-free", "old-cheap"])
    }

    @Test func numberFormatting() {
        #expect(TraceNumberFormat.tokens(nil) == "–")
        #expect(TraceNumberFormat.tokens(0) == "0")
        #expect(TraceNumberFormat.tokens(999) == "999")
        #expect(TraceNumberFormat.tokens(1_500) == "1.5k")
        #expect(TraceNumberFormat.tokens(25_000) == "25k")
        #expect(TraceNumberFormat.tokens(2_400_000) == "2.4M")
        #expect(TraceNumberFormat.cost(0.5) == "$0.50")
        #expect(TraceNumberFormat.cost(0.00005262) == "$0.0001")
    }

    @Test func selectionMachine() {
        var machine = SessionSelection()
        #expect(machine.setLive(true, newestVisibleId: "A") == .selected("A"))
        #expect(!machine.pinned)
        #expect(machine.selectedId == "A")

        #expect(machine.userSelect("B") == .selected("B"))
        #expect(machine.pinned)
        #expect(machine.selectedId == "B")

        #expect(machine.setLive(true, newestVisibleId: "A") == .selected("A"))
        #expect(!machine.pinned)
        #expect(machine.selectedId == "A")

        #expect(machine.setLive(false, newestVisibleId: "A") == .none)
        #expect(machine.pinned)
    }

    @Test func selectionBindingGuard() {
        var machine = SessionSelection()
        machine.setLive(true, newestVisibleId: "A")
        #expect(machine.bindingSelect("A") == .none)
        #expect(!machine.pinned)
        #expect(machine.selectedId == "A")

        #expect(machine.bindingSelect("A") == .none)
        #expect(machine.pinned)

        machine.setLive(true, newestVisibleId: "A")
        #expect(!machine.pinned)
        #expect(machine.bindingSelect("B") == .selected("B"))
        #expect(machine.pinned)
        #expect(machine.selectedId == "B")

        #expect(machine.bindingSelect(nil) == .none)
        #expect(machine.selectedId == "B")

        machine.clear()
        #expect(machine.selectedId == nil)
    }

    @Test func selectionFollowRepeatIsNoop() {
        var machine = SessionSelection()
        #expect(machine.followSelect("A") == .selected("A"))
        #expect(machine.followSelect("A") == .none)
        #expect(!machine.pinned)
        #expect(machine.userSelect("A") == .none)
        #expect(machine.pinned)
    }

    @Test func customizationRoundtrip() throws {
        var customization = TableColumnCustomization<SessionRow>()
        customization[visibility: "errors"] = .visible
        customization[visibility: "harness"] = .hidden
        let data = try JSONEncoder().encode(customization)
        let decoded = try JSONDecoder().decode(
            TableColumnCustomization<SessionRow>.self, from: data)
        #expect(decoded[visibility: "errors"] == .visible)
        #expect(decoded[visibility: "harness"] == .hidden)
        #expect(decoded[visibility: "tags"] == .automatic)
    }
}
