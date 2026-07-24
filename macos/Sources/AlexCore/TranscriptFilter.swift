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
/// This used to live inline in `TranscriptChatPane.body` (Alex
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
            if Task.isCancelled { break }
            let userText = turn.user ?? ""
            let userIsToolResult = userText.hasPrefix(TurnHeader.toolResultPrefix)
            let userSearchText = trimmed.isEmpty ? "" : bounded(userText)
            if !userText.isEmpty,
                matches(
                    role: .user, searchText: userSearchText, hasTools: false,
                    isToolResult: userIsToolResult, filterTab: filterTab, query: trimmed)
            {
                out.append(TranscriptFilterEntry(turnId: turn.traceId, turnIndex: index, role: .user))
            }
            if !userText.isEmpty { total += 1 }
            let toolsPresent = hasTools(turn)
            let attemptEventPresent = turn.hasInlineAttemptEvents
            let clientClosed = TraceClassification.isClientDisconnect(errorKind: turn.errorKind)
            let errorPresent = turn.error?.isEmpty == false
            let assistantPresent = hasAssistantText(turn)
            guard assistantPresent || toolsPresent || errorPresent || clientClosed
                || attemptEventPresent
            else { continue }
            total += 1
            let searchText = trimmed.isEmpty
                ? ""
                : boundedAssistantSearchText(
                    turn, clientClosed: clientClosed,
                    attemptEventPresent: attemptEventPresent)
            if matches(
                role: .assistant, searchText: searchText, hasTools: toolsPresent,
                filterTab: filterTab, query: trimmed)
            {
                out.append(TranscriptFilterEntry(turnId: turn.traceId, turnIndex: index, role: .assistant))
            }
        }
        return TranscriptFilterResult(entries: out, totalCount: total)
    }

    /// Mirrors the presence test in `TranscriptChatMessages.assistantText`
    /// without joining potentially multi-megabyte blocks just to decide
    /// whether an assistant slot exists.
    static func hasAssistantText(_ turn: TranscriptTurn) -> Bool {
        let blocks = turn.assistantBlocks ?? []
        guard !blocks.isEmpty else { return turn.assistant?.isEmpty == false }
        return blocks.contains { $0.type == "text" && $0.text?.isEmpty == false }
    }

    /// Builds only the searchable prefix. The old implementation joined all
    /// assistant blocks, errors, and middleware explanations before throwing
    /// away everything after `searchCharLimit`.
    static func boundedAssistantSearchText(
        _ turn: TranscriptTurn, clientClosed: Bool,
        attemptEventPresent: Bool
    ) -> String {
        var result = ""
        func append(_ value: String?, separator: String = "\n") {
            guard let value, !value.isEmpty, result.count < searchCharLimit else { return }
            if !result.isEmpty {
                result.append(contentsOf: separator.prefix(searchCharLimit - result.count))
            }
            guard result.count < searchCharLimit else { return }
            result.append(contentsOf: value.prefix(searchCharLimit - result.count))
        }

        let blocks = turn.assistantBlocks ?? []
        if blocks.isEmpty {
            append(turn.assistant, separator: "")
        } else {
            for block in blocks where block.type == "text" {
                append(block.text, separator: "\n\n")
                if result.count >= searchCharLimit { return result }
            }
        }
        append(turn.error)
        if clientClosed { append("client closed") }
        if attemptEventPresent {
            append(turn.substitutionReason)
            for attempt in turn.attempts ?? [] {
                append(attempt.provider)
                append(attempt.model)
                append(attempt.error?.kind)
                append(attempt.error?.code)
                append(attempt.error?.message)
                for decision in attempt.middlewareDecisions ?? [] {
                    append(decision.ruleId)
                    append(decision.ruleName)
                    append(decision.action)
                    append(decision.explanation)
                }
                if result.count >= searchCharLimit { return result }
            }
        }
        return result
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
        return bounded(searchText).localizedCaseInsensitiveContains(query)
    }

    private static func bounded(_ value: String) -> String {
        String(value.prefix(searchCharLimit))
    }
}
