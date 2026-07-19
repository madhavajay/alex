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
        // Filtering runs off the main actor and is debounced by the model
        // (TraceBrowserModel.scheduleTranscriptFilter) — this view just
        // renders the latest cached result instead of recomputing it on
        // every keystroke (that synchronous, uncapped recompute used to
        // freeze the window on large sessions).
        let entries = model.transcriptEntries
        // Displayed role per entry (user / harness tool-result / assistant)
        // resolved once up front: role-change dividers and thread connectors
        // need the *actual* rendered role, not just which half of the turn
        // the entry structurally belongs to (a "user"-slot entry can render
        // as `.harness` — see `TranscriptChatMessages.messages`).
        let messages = entries.map { displayMessage($0) }
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 0) {
                    ForEach(Array(entries.enumerated()), id: \.element.id) { index, entry in
                        entryView(
                            entry, message: messages[index],
                            roleChanged: index > 0 && messages[index - 1]?.role != messages[index]?.role,
                            isThreaded: index + 1 < entries.count
                                && messages[index + 1]?.role == messages[index]?.role)
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
        _ entry: TranscriptChatEntry, message: MessageDisplay?, roleChanged: Bool, isThreaded: Bool
    ) -> some View {
        if let message {
            if roleChanged {
                RoleChangeDivider(role: message.role)
            }
            let selected = model.inspectorTraceId == entry.turn.traceId
            MessageBubble(
                message: message,
                selected: selected,
                onSelect: { model.openInspector(traceId: entry.turn.traceId) },
                onFollowSubagent: { model.followSubagent($0) },
                onViewToolBody: { id, kind in model.openToolBody(id: id, kind: kind) },
                fetchToolBodyText: { id, kind in
                    try? await model.fetchToolBody(id: id, kind: kind).text
                })
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
        // `entry.turnNumber` is 1-based; the previous turn (whose tool
        // call(s) a "[tool result]" user message answers) sits one index
        // back in `model.turns`.
        let previousTurn = model.turns.indices.contains(entry.turnNumber - 2)
            ? model.turns[entry.turnNumber - 2] : nil
        let messages = TranscriptChatMessages.messages(
            for: entry.turn, harnessName: harnessName, previousTurn: previousTurn)
        // `entry.role` only distinguishes which half of the turn this slot
        // is (structural), while the resolved message may be `.harness`
        // instead of `.user` for that same slot — match on "assistant half
        // vs. not" rather than exact role equality.
        guard var message = messages.first(where: { entry.role == .assistant ? $0.role == .assistant : $0.role != .assistant }) else {
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

/// Centered "USER"/"HARNESS"/"MODEL" divider between role groups (mock TB
/// App.tsx:685-696; "HARNESS" added so a tool-result reply visibly breaks
/// from a plain user message even though both are the "user" slot of a turn).
private struct RoleChangeDivider: View {
    let role: MessageDisplay.Role

    var body: some View {
        HStack(spacing: AlexTheme.Spacing.lg) {
            line
            Text(label)
                .font(AlexTheme.Fonts.mono(9.5, weight: .semibold))
                .tracking(0.66)
                .foregroundStyle(tint)
            line
        }
        .padding(.horizontal, AlexTheme.Spacing.xl)
        .padding(.vertical, AlexTheme.Spacing.ml)
    }

    private var label: String {
        switch role {
        case .user: "USER"
        case .harness: "HARNESS"
        case .assistant: "MODEL"
        }
    }

    private var tint: Color {
        switch role {
        case .user: AlexTheme.Colors.textTertiary
        case .harness: AlexTheme.Colors.warningOrange
        case .assistant: AlexTheme.Colors.primary.opacity(0.5)
        }
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

/// Segmented-tab labels for the transcript filter row. The actual filtering
/// (which used to live here as a synchronous, uncapped full-text scan run on
/// every keystroke — the root cause of a window freeze on large sessions) now
/// lives in `TranscriptFilter` (AlexandriaBarCore), invoked off the main
/// actor and debounced by `TraceBrowserModel.scheduleTranscriptFilter`.
enum TranscriptChatEntries {
    static let filterTabs = TranscriptFilter.filterTabs
}

/// Maps the transcript wire model onto DesignSystem display structs. Reuses the
/// same fields the classic pane consumes (user/assistant text, assistant
/// blocks, tool calls paired with executions via ToolLifecycle).
enum TranscriptChatMessages {
    @MainActor
    static func messages(
        for turn: TranscriptTurn, harnessName: String, previousTurn: TranscriptTurn? = nil
    ) -> [MessageDisplay] {
        var out: [MessageDisplay] = []
        if let text = turn.user, !text.isEmpty {
            let toolBody = TurnHeader.toolResultBody(text)
            let isHarness = toolBody != nil
            out.append(MessageDisplay(
                id: turn.traceId + "#user",
                turnId: turn.traceId,
                role: isHarness ? .harness : .user,
                roleLabel: isHarness
                    ? TurnHeader.harnessResultLabel(toolName: pairedToolName(previousTurn))
                    : TurnHeader.requestLabel(harness: harnessName),
                content: cap(toolBody ?? text),
                isMonospaced: isHarness,
                timestamp: TraceFormat.time(turn.tsRequestMs),
                tokenText: ChatDisplayFormat.tokenLabel(turn.inputTokens)))
        }

        let toolCalls = toolDisplays(for: turn)
        let content = assistantText(turn)
        let clientClosed = TraceClassification.isClientDisconnect(errorKind: turn.errorKind)
        let hasError = !clientClosed && turn.error?.isEmpty == false
        if !content.isEmpty || !toolCalls.isEmpty || hasError || clientClosed {
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
                error: hasError ? turn.error : nil,
                event: clientClosed ? "client closed" : nil))
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

    /// The tool name to show alongside "Harness · tool result" when it can
    /// be identified unambiguously: the previous turn's assistant made the
    /// tool call(s) this turn's "[tool result]" user text answers. Only
    /// resolves a name when the previous turn made exactly one distinct
    /// tool call — with more than one in flight, attributing the result to
    /// a specific one would be a guess.
    static func pairedToolName(_ previousTurn: TranscriptTurn?) -> String? {
        guard let previousTurn else { return nil }
        let names = Set(toolRequests(for: previousTurn).map(\.name))
        return names.count == 1 ? names.first : nil
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
        // Single-string-arg tools (Bash's "command", Read's "file_path", …)
        // render as the plain string, not escaped JSON; multi-argument
        // calls still get the full pretty-printed object.
        let input = arguments.map {
            ChatDisplayFormat.meaningfulArgumentText($0) ?? AlexandriaBarCore.ToolCall.summary($0)
        } ?? ""
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
