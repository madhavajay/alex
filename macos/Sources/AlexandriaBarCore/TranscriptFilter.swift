import Foundation

/// Which half of a turn a filtered entry represents.
public enum TranscriptEntryRole: Sendable, Equatable {
    case user
    case assistant
}

/// One filterable slot of a transcript turn (user or assistant half).
public struct TranscriptFilterEntry: Sendable, Equatable, Identifiable {
    public let turnId: String
    public let turnIndex: Int
    public let role: TranscriptEntryRole

    public var id: String { turnId + (role == .user ? "#user" : "#assistant") }

    public init(turnId: String, turnIndex: Int, role: TranscriptEntryRole) {
        self.turnId = turnId
        self.turnIndex = turnIndex
        self.role = role
    }
}

public struct TranscriptFilterResult: Sendable, Equatable {
    public let entries: [TranscriptFilterEntry]
    /// Number of displayable entries before the selected tab/query is applied.
    public let totalCount: Int
}

/// Pure transcript message filtering, safe to run off the main actor.
///
/// This used to live inline in `TranscriptChatPane.body` (AlexandriaBar
/// target) and ran synchronously on the main thread on every keystroke,
/// three times over (main pane + two footer counts), joining/searching the
/// *full* assistant text of every turn with no cap. On a large session
/// (hundreds of turns, multi-hundred-KB messages) that froze the window.
/// This version bounds the searched text per message and is designed to be
/// called from a background task with results cached/debounced by the
/// caller (see `TraceBrowserModel.scheduleTranscriptFilter` and
/// `TranscriptChatEntries` in TranscriptChatPane.swift).
public enum TranscriptFilter {
    public static let filterTabs = ["All", "User", "Model", "Tools", "Agents"]

    /// Cap on searched characters per message side. Message bodies aren't
    /// rendered in full either (see `TurnTextCap`), so searching more than
    /// this can never change what the user could have matched by eye.
    public static let searchCharLimit = 4000

    public static func entries(
        turns: [TranscriptTurn], filterTab: Int, query: String
    ) -> [TranscriptFilterEntry] {
        result(turns: turns, filterTab: filterTab, query: query).entries
    }

    /// Produces the filtered list and its unfiltered total in one pass. The
    /// total used to call `entries` again and scan every turn twice per key.
    public static func result(
        turns: [TranscriptTurn], filterTab: Int, query: String
    ) -> TranscriptFilterResult {
        let trimmed = query.trimmingCharacters(in: .whitespaces)
        var out: [TranscriptFilterEntry] = []
        var total = 0
        out.reserveCapacity(turns.count * 2)
        for (index, turn) in turns.enumerated() {
            let userText = turn.user ?? ""
            let userIsToolResult = TurnHeader.toolResultBody(userText) != nil
            if !userText.isEmpty,
                matches(
                    role: .user, searchText: userText, hasTools: false,
                    isToolResult: userIsToolResult, filterTab: filterTab, query: trimmed)
            {
                out.append(TranscriptFilterEntry(turnId: turn.traceId, turnIndex: index, role: .user))
            }
            if !userText.isEmpty { total += 1 }
            let toolsPresent = hasTools(turn)
            let errorPresent = turn.error?.isEmpty == false
            let assistant = assistantText(turn)
            guard !assistant.isEmpty || toolsPresent || errorPresent else { continue }
            total += 1
            let searchText = trimmed.isEmpty ? "" : assistant + (turn.error.map { "\n" + $0 } ?? "")
            if matches(
                role: .assistant, searchText: searchText, hasTools: toolsPresent,
                filterTab: filterTab, query: trimmed)
            {
                out.append(TranscriptFilterEntry(turnId: turn.traceId, turnIndex: index, role: .assistant))
            }
        }
        return TranscriptFilterResult(entries: out, totalCount: total)
    }

    /// Mirrors `TranscriptChatMessages.assistantText` (AlexandriaBar) without
    /// depending on that target. Kept in sync manually; both are pure and
    /// small.
    static func assistantText(_ turn: TranscriptTurn) -> String {
        let blocks = turn.assistantBlocks ?? []
        guard !blocks.isEmpty else { return turn.assistant ?? "" }
        return blocks
            .filter { $0.type == "text" }
            .compactMap(\.text)
            .filter { !$0.isEmpty }
            .joined(separator: "\n\n")
    }

    static func hasTools(_ turn: TranscriptTurn) -> Bool {
        let blocks = turn.assistantBlocks ?? []
        if blocks.contains(where: { $0.type == "tool_call" }) { return true }
        if blocks.isEmpty, turn.toolCalls?.isEmpty == false { return true }
        return turn.executedTools?.isEmpty == false
    }

    private static func matches(
        role: TranscriptEntryRole, searchText: String, hasTools: Bool,
        isToolResult: Bool = false, filterTab: Int, query: String
    ) -> Bool {
        switch filterTab {
        case 1: guard role == .user else { return false }
        case 2: guard role == .assistant else { return false }
        // Tools: the assistant message that made the call(s), plus the
        // "harness" reply (rendered as role .user structurally — see
        // TranscriptChatMessages) that carries their result back.
        case 3: guard (role == .assistant && hasTools) || (role == .user && isToolResult)
            else { return false }
        case 4: return false  // Agents: per-message subagent data has no source yet.
        default: break
        }
        guard !query.isEmpty else { return true }
        let bounded = searchText.count > searchCharLimit
            ? String(searchText.prefix(searchCharLimit))
            : searchText
        return bounded.localizedCaseInsensitiveContains(query)
    }
}
