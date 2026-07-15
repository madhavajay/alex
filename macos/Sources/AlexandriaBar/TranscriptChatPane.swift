import SwiftUI
import AlexandriaBarCore

/// Chat-style transcript pane built from the DesignSystem components. Renders
/// the same turns as TranscriptTextPane (kept for A/B during review) as
/// MessageBubbles in a lazy scroll view, with role-change dividers, thread
/// connectors, and client-side message filtering (mock TB App.tsx:584-723).
struct TranscriptChatPane: View {
    let model: TraceBrowserModel

    private static let bottomAnchor = "transcript-bottom"

    var body: some View {
        let entries = TranscriptChatEntries.entries(
            turns: model.turns, filterTab: model.transcriptFilterTab,
            query: model.transcriptQuery)
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 0) {
                    ForEach(Array(entries.enumerated()), id: \.element.id) { index, entry in
                        entryView(
                            entry,
                            roleChanged: index > 0 && entries[index - 1].role != entry.role,
                            isThreaded: index + 1 < entries.count
                                && entries[index + 1].role == entry.role)
                    }
                    if entries.isEmpty, !model.turns.isEmpty {
                        EmptyStateView(message: "No messages match")
                    }
                    Color.clear
                        .frame(height: 1)
                        .id(Self.bottomAnchor)
                        .onAppear { model.setUserAtBottom(true) }
                        .onDisappear { model.setUserAtBottom(false) }
                }
                .padding(.vertical, AlexTheme.Spacing.md)
            }
            .background(AlexTheme.Colors.background)
            .onAppear {
                proxy.scrollTo(Self.bottomAnchor, anchor: .bottom)
            }
            .onChange(of: model.turns) {
                if model.userAtBottom {
                    proxy.scrollTo(Self.bottomAnchor, anchor: .bottom)
                }
            }
            .onChange(of: model.scrollCommand) {
                proxy.scrollTo(Self.bottomAnchor, anchor: .bottom)
            }
            .onChange(of: model.inspectorTraceId) { _, traceId in
                guard let traceId,
                    let target = entries.first(where: { $0.turn.traceId == traceId })
                else { return }
                withAnimation(.easeInOut(duration: 0.15)) {
                    proxy.scrollTo(target.id)
                }
            }
        }
    }

    @ViewBuilder
    private func entryView(
        _ entry: TranscriptChatEntry, roleChanged: Bool, isThreaded: Bool
    ) -> some View {
        if roleChanged {
            RoleChangeDivider(role: entry.role)
        }
        if let message = displayMessage(entry) {
            let selected = model.inspectorTraceId == entry.turn.traceId
            MessageBubble(
                message: message,
                selected: selected,
                onSelect: { model.openInspector(traceId: entry.turn.traceId) },
                onFollowSubagent: { model.followSubagent($0) },
                onViewToolBody: { id, kind in model.openToolBody(id: id, kind: kind) })
                .overlay(alignment: entry.role == .user ? .topLeading : .topTrailing) {
                    if isThreaded {
                        threadConnector(role: entry.role)
                    }
                }
                .overlay(alignment: .trailing) {
                    if selected {
                        Rectangle()
                            .fill(AlexTheme.Colors.primary)
                            .frame(width: 2)
                    }
                }
                .id(entry.id)
        }
    }

    private func displayMessage(_ entry: TranscriptChatEntry) -> MessageDisplay? {
        let session = model.selectedSession
        let harnessName = HarnessName.display(harness: session?.harness, tags: session?.tags)
        let messages = TranscriptChatMessages.messages(
            for: entry.turn, harnessName: harnessName)
        guard var message = messages.first(where: { $0.role == entry.role }) else {
            return nil
        }
        // Turn-number gutter (mock TB App.tsx:534-539). MessageBubble has no
        // dedicated slot, so prefix it onto the header token text.
        message.tokenText = ["#\(entry.turnNumber)", message.tokenText]
            .compactMap(\.self)
            .joined(separator: " · ")
        return message
    }

    /// 1px vertical line below the avatar linking consecutive same-role
    /// messages (mock TB App.tsx:503-521). Positioned over the avatar column:
    /// 16pt row padding + half the 24pt avatar ≈ 27.5pt inset, starting just
    /// under the avatar (≈36pt down).
    private func threadConnector(role: MessageDisplay.Role) -> some View {
        Rectangle()
            .fill(
                role == .user
                    ? AlexTheme.Colors.overlay(0.07)
                    : AlexTheme.Colors.primary.opacity(0.18))
            .frame(width: 1)
            .frame(maxHeight: .infinity)
            .padding(.top, 36)
            .padding(role == .user ? .leading : .trailing, 27.5)
    }
}

/// Centered "USER"/"MODEL" divider between role groups
/// (mock TB App.tsx:685-696).
private struct RoleChangeDivider: View {
    let role: MessageDisplay.Role

    var body: some View {
        HStack(spacing: AlexTheme.Spacing.lg) {
            line
            Text(role == .user ? "USER" : "MODEL")
                .font(AlexTheme.Fonts.mono(9.5, weight: .semibold))
                .tracking(0.66)
                .foregroundStyle(
                    role == .user
                        ? AlexTheme.Colors.textTertiary
                        : AlexTheme.Colors.primary.opacity(0.5))
            line
        }
        .padding(.horizontal, AlexTheme.Spacing.xl)
        .padding(.vertical, AlexTheme.Spacing.ml)
    }

    private var line: some View {
        Rectangle()
            .fill(AlexTheme.Colors.hairline)
            .frame(height: 1)
            .frame(maxWidth: .infinity)
    }
}

/// One renderable message slot of a turn (user or assistant half).
struct TranscriptChatEntry: Identifiable {
    let turn: TranscriptTurn
    let turnNumber: Int
    let role: MessageDisplay.Role

    var id: String {
        turn.traceId + (role == .user ? "#user" : "#assistant")
    }
}

/// Flattens turns into filterable message entries. Mirrors the message
/// presence rules of `TranscriptChatMessages.messages(for:harnessName:)`
/// without building the (heavier) display structs.
enum TranscriptChatEntries {
    static let filterTabs = ["All", "User", "Model", "Tools", "Agents"]

    static func entries(
        turns: [TranscriptTurn], filterTab: Int, query: String
    ) -> [TranscriptChatEntry] {
        var out: [TranscriptChatEntry] = []
        for (index, turn) in turns.enumerated() {
            if hasUserMessage(turn),
                matches(role: .user, turn: turn, filterTab: filterTab, query: query)
            {
                out.append(
                    TranscriptChatEntry(turn: turn, turnNumber: index + 1, role: .user))
            }
            if hasAssistantMessage(turn),
                matches(role: .assistant, turn: turn, filterTab: filterTab, query: query)
            {
                out.append(
                    TranscriptChatEntry(turn: turn, turnNumber: index + 1, role: .assistant))
            }
        }
        return out
    }

    static func hasUserMessage(_ turn: TranscriptTurn) -> Bool {
        turn.user?.isEmpty == false
    }

    static func hasAssistantMessage(_ turn: TranscriptTurn) -> Bool {
        !TranscriptChatMessages.assistantText(turn).isEmpty
            || hasTools(turn)
            || turn.error?.isEmpty == false
    }

    static func hasTools(_ turn: TranscriptTurn) -> Bool {
        let blocks = turn.assistantBlocks ?? []
        if blocks.contains(where: { $0.type == "tool_call" }) { return true }
        if blocks.isEmpty, turn.toolCalls?.isEmpty == false { return true }
        return turn.executedTools?.isEmpty == false
    }

    static func matches(
        role: MessageDisplay.Role, turn: TranscriptTurn, filterTab: Int, query: String
    ) -> Bool {
        switch filterTab {
        case 1: guard role == .user else { return false }
        case 2: guard role == .assistant else { return false }
        case 3: guard role == .assistant, hasTools(turn) else { return false }
        case 4: return false  // Agents: per-message subagent data has no source yet.
        default: break
        }
        let trimmed = query.trimmingCharacters(in: .whitespaces)
        guard !trimmed.isEmpty else { return true }
        let text = role == .user
            ? (turn.user ?? "")
            : [TranscriptChatMessages.assistantText(turn), turn.error ?? ""]
                .joined(separator: "\n")
        return text.localizedCaseInsensitiveContains(trimmed)
    }
}

/// Maps the transcript wire model onto DesignSystem display structs. Reuses the
/// same fields the classic pane consumes (user/assistant text, assistant
/// blocks, tool calls paired with executions via ToolLifecycle).
enum TranscriptChatMessages {
    @MainActor
    static func messages(for turn: TranscriptTurn, harnessName: String) -> [MessageDisplay] {
        var out: [MessageDisplay] = []
        if let text = turn.user, !text.isEmpty {
            let toolBody = TurnHeader.toolResultBody(text)
            out.append(MessageDisplay(
                id: turn.traceId + "#user",
                turnId: turn.traceId,
                role: .user,
                roleLabel: TurnHeader.requestLabel(
                    harness: harnessName, isToolResult: toolBody != nil),
                content: cap(toolBody ?? text),
                isMonospaced: toolBody != nil,
                timestamp: TraceFormat.time(turn.tsRequestMs),
                tokenText: ChatDisplayFormat.tokenLabel(turn.inputTokens)))
        }

        let toolCalls = toolDisplays(for: turn)
        let content = assistantText(turn)
        let hasError = turn.error?.isEmpty == false
        if !content.isEmpty || !toolCalls.isEmpty || hasError {
            let effort = TurnHeader.effort(
                reasoningEffort: turn.reasoningEffort, thinkingBudget: turn.thinkingBudget)
            out.append(MessageDisplay(
                id: turn.traceId + "#assistant",
                turnId: turn.traceId,
                role: .assistant,
                roleLabel: "Model",
                content: cap(content),
                model: turn.model,
                detail: effort == "-" ? nil : effort,
                timestamp: TraceFormat.time(turn.tsResponseMs ?? turn.tsRequestMs),
                tokenText: ChatDisplayFormat.tokenLabel(turn.outputTokens),
                toolCalls: toolCalls,
                error: turn.error))
        }
        return out
    }

    static func assistantText(_ turn: TranscriptTurn) -> String {
        let blocks = turn.assistantBlocks ?? []
        guard !blocks.isEmpty else { return turn.assistant ?? "" }
        return blocks
            .filter { $0.type == "text" }
            .compactMap(\.text)
            .filter { !$0.isEmpty }
            .joined(separator: "\n\n")
    }

    static func toolRequests(for turn: TranscriptTurn) -> [AlexandriaBarCore.ToolCall] {
        let blocks = turn.assistantBlocks ?? []
        if blocks.isEmpty {
            return turn.toolCalls ?? []
        }
        return blocks.compactMap { block in
            guard block.type == "tool_call", let name = block.name else { return nil }
            return AlexandriaBarCore.ToolCall(
                name: name, arguments: block.arguments, id: block.id)
        }
    }

    /// Number of tool lifecycles the turn renders (transcript header summary).
    static func toolCount(for turn: TranscriptTurn) -> Int {
        ToolLifecycle.pair(
            requests: toolRequests(for: turn), executions: turn.executedTools ?? []
        ).count
    }

    static func toolDisplays(for turn: TranscriptTurn) -> [ToolCallDisplay] {
        let lifecycles = ToolLifecycle.pair(
            requests: toolRequests(for: turn), executions: turn.executedTools ?? [])
        return lifecycles.enumerated().map { index, lifecycle in
            display(for: lifecycle, turnId: turn.traceId, index: index)
        }
    }

    static func display(
        for lifecycle: ToolLifecycle, turnId: String, index: Int
    ) -> ToolCallDisplay {
        let execution = lifecycle.execution
        let arguments = lifecycle.request?.arguments
        let input = arguments.map { AlexandriaBarCore.ToolCall.summary($0) } ?? ""
        return ToolCallDisplay(
            id: execution?.id
                ?? lifecycle.request?.id
                ?? "\(turnId)#tool\(index)",
            name: lifecycle.name,
            argumentPreview: ChatDisplayFormat.firstArgumentPreview(arguments)
                .map { ChatDisplayFormat.truncated($0) },
            input: TurnTextCap.cap(input, maxLines: .max).text,
            status: displayStatus(lifecycle.status),
            durationText: execution.flatMap {
                ChatDisplayFormat.toolDuration(startMs: $0.tsStartMs, endMs: $0.tsEndMs)
            },
            exitStatus: execution?.exitStatus,
            hasArgsBody: execution?.argsBodyPath != nil,
            hasResultBody: execution?.resultBodyPath != nil)
    }

    static func displayStatus(_ status: ToolLifecycle.Status) -> DisplayStatus? {
        switch status {
        case .requested: nil
        case .running: .running
        case .executed: .success
        case .failed: .error
        }
    }

    static func cap(_ text: String) -> String {
        TurnTextCap.cap(text, maxChars: TranscriptRender.maxTurnChars, maxLines: .max).text
    }
}
