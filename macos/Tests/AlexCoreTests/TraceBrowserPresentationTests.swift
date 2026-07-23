#if os(macOS)
import Foundation
import Testing
@testable import Alex
@testable import AlexCore

@Suite struct TraceBrowserPresentationTests {
    @Test func transcriptChatBodiesStartExpanded() {
        #expect(TranscriptPresentationDefaults.chatBodiesExpanded)
    }

    @Test func singleSessionMenuContainsEveryStandardActionInOrder() {
        #expect(SessionContextMenuAction.standard.map(\.title) == [
            "Fork session with…",
            "Copy Session ID",
            "Copy Last Reply as Markdown",
            "Export Session…",
            "Reveal Bodies in Finder",
            "",
            "Delete Session's Traces…",
        ])
    }

    @Test func menuSelectionFallsBackToTableSelection() throws {
        let first = try session("session-a")
        let second = try session("session-b")

        let emptyResolved = SessionContextMenuSelection.resolve(
            ids: [], fallbackIds: ["session-b"], sessions: [first, second])
        let staleResolved = SessionContextMenuSelection.resolve(
            ids: ["stale-table-id"], fallbackIds: ["session-b"], sessions: [first, second])

        #expect(emptyResolved.map(\.sessionId) == ["session-b"])
        #expect(staleResolved.map(\.sessionId) == ["session-b"])
    }

    private func session(_ id: String) throws -> TraceSession {
        try JSONDecoder().decode(
            TraceSession.self,
            from: Data(
                """
                {"session_id":"\(id)","first_ts_ms":1,"last_ts_ms":2,"trace_count":1,"errors":0}
                """.utf8))
    }
}
#endif
