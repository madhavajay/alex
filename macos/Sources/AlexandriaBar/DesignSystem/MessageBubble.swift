import SwiftUI

/// One transcript message: avatar, role/metadata header, chat bubble, then any
/// tool-call and subagent cards. User messages align left with a gray bubble;
/// assistant messages align right with a blue-tinted bubble.
struct MessageBubble: View {
    let message: MessageDisplay
    var selected = false
    var onSelect: (() -> Void)?
    var onFollowSubagent: ((String) -> Void)?
    /// (toolExecutionId, kind) where kind is "args" or "result".
    var onViewToolBody: ((String, String) -> Void)?

    private var isUser: Bool { message.role == .user }

    var body: some View {
        HStack(alignment: .top, spacing: AlexTheme.Spacing.ml) {
            if isUser {
                RoleAvatar(variant: .user)
                    .padding(.top, AlexTheme.Spacing.xxs)
                column
            } else {
                column
                RoleAvatar(variant: .assistant)
                    .padding(.top, AlexTheme.Spacing.xxs)
            }
        }
        .padding(.horizontal, AlexTheme.Spacing.xl)
        .padding(.vertical, AlexTheme.Spacing.xs)
        .background(selected ? AlexTheme.Colors.selectionWash : Color.clear)
        .contentShape(Rectangle())
        .onTapGesture { onSelect?() }
    }

    private var column: some View {
        VStack(alignment: isUser ? .leading : .trailing, spacing: AlexTheme.Spacing.sm) {
            headerRow
            if !message.content.isEmpty {
                bubble
            }
            if !message.toolCalls.isEmpty {
                toolCallsSection
            }
            if let subagent = message.subagent {
                SubagentCard(subagent: subagent, onFollow: onFollowSubagent)
                    .padding(isUser ? .trailing : .leading, cardInset)
            }
            if let error = message.error, !error.isEmpty {
                errorCard(error)
            }
        }
        .frame(maxWidth: .infinity, alignment: isUser ? .leading : .trailing)
    }

    private var cardInset: CGFloat { 32 }

    private var headerRow: some View {
        HStack(alignment: .firstTextBaseline, spacing: AlexTheme.Spacing.md) {
            if isUser {
                roleText
                modelText
                detailText
                Spacer(minLength: AlexTheme.Spacing.md)
                tokenText
                timestampText
            } else {
                timestampText
                tokenText
                Spacer(minLength: AlexTheme.Spacing.md)
                detailText
                modelText
                roleText
            }
        }
    }

    private var roleText: some View {
        Text(message.roleLabel)
            .font(AlexTheme.Fonts.roleLabel)
            .foregroundStyle(
                isUser ? AlexTheme.Colors.mutedForeground : AlexTheme.Colors.primaryBright)
            .lineLimit(1)
    }

    @ViewBuilder private var modelText: some View {
        if let model = message.model {
            Text(model)
                .font(AlexTheme.Fonts.metaMicro)
                .foregroundStyle(AlexTheme.Colors.textFaint)
                .lineLimit(1)
                .truncationMode(.middle)
        }
    }

    @ViewBuilder private var detailText: some View {
        if let detail = message.detail {
            Text(detail)
                .font(AlexTheme.Fonts.metaMicro)
                .foregroundStyle(AlexTheme.Colors.textFaint)
                .lineLimit(1)
        }
    }

    @ViewBuilder private var tokenText: some View {
        if let tokens = message.tokenText {
            Text(tokens)
                .font(AlexTheme.Fonts.metaMicro)
                .foregroundStyle(AlexTheme.Colors.textFaint)
        }
    }

    @ViewBuilder private var timestampText: some View {
        if let timestamp = message.timestamp {
            Text(timestamp)
                .font(AlexTheme.Fonts.metaMicro)
                .foregroundStyle(AlexTheme.Colors.textFaintest)
        }
    }

    private var bubble: some View {
        Text(message.content)
            .font(message.isMonospaced ? AlexTheme.Fonts.mono(11.5) : AlexTheme.Fonts.bubbleBody)
            .lineSpacing(3)
            .foregroundStyle(
                isUser ? AlexTheme.Colors.userBubbleText : AlexTheme.Colors.assistantBubbleText)
            .textSelection(.enabled)
            .padding(.horizontal, 14)
            .padding(.vertical, AlexTheme.Spacing.ml)
            .background(bubbleShape.fill(bubbleFill))
            .overlay(bubbleShape.strokeBorder(bubbleBorder))
            .padding(isUser ? .trailing : .leading, 48)
            .frame(maxWidth: .infinity, alignment: isUser ? .leading : .trailing)
    }

    private var bubbleShape: UnevenRoundedRectangle {
        UnevenRoundedRectangle(
            topLeadingRadius: isUser ? AlexTheme.Radius.xs : AlexTheme.Radius.bubble,
            bottomLeadingRadius: AlexTheme.Radius.bubble,
            bottomTrailingRadius: AlexTheme.Radius.bubble,
            topTrailingRadius: isUser ? AlexTheme.Radius.bubble : AlexTheme.Radius.xs)
    }

    private var bubbleFill: Color {
        if isUser {
            return selected
                ? AlexTheme.Colors.userBubbleSelected : AlexTheme.Colors.userBubble
        }
        return selected
            ? AlexTheme.Colors.assistantBubbleSelected : AlexTheme.Colors.assistantBubble
    }

    private var bubbleBorder: Color {
        if isUser {
            return selected ? AlexTheme.Colors.borderStrong : AlexTheme.Colors.cardBorder
        }
        return AlexTheme.Colors.primary.opacity(selected ? 0.45 : 0.22)
    }

    private var toolCallsSection: some View {
        VStack(alignment: isUser ? .leading : .trailing, spacing: AlexTheme.Spacing.sm) {
            Text("\(message.toolCalls.count) tool call\(message.toolCalls.count == 1 ? "" : "s")")
                .font(AlexTheme.Fonts.metaMicro)
                .foregroundStyle(AlexTheme.Colors.textFaint)
            ForEach(message.toolCalls) { tool in
                ToolCallCard(
                    tool: tool,
                    onViewArgs: toolBodyAction(tool, kind: "args"),
                    onViewOutput: toolBodyAction(tool, kind: "result"))
            }
        }
        .padding(isUser ? .trailing : .leading, cardInset)
        .frame(maxWidth: .infinity, alignment: isUser ? .leading : .trailing)
    }

    private func toolBodyAction(_ tool: ToolCallDisplay, kind: String) -> (() -> Void)? {
        let available = kind == "args" ? tool.hasArgsBody : tool.hasResultBody
        guard available, let onViewToolBody else { return nil }
        return { onViewToolBody(tool.id, kind) }
    }

    private func errorCard(_ error: String) -> some View {
        Text(error)
            .font(AlexTheme.Fonts.metaMono)
            .foregroundStyle(AlexTheme.Colors.destructive)
            .textSelection(.enabled)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(AlexTheme.Spacing.ml)
            .background(
                RoundedRectangle(cornerRadius: AlexTheme.Radius.md)
                    .fill(AlexTheme.Colors.destructive.opacity(0.08)))
            .overlay(
                RoundedRectangle(cornerRadius: AlexTheme.Radius.md)
                    .strokeBorder(AlexTheme.Colors.destructive.opacity(0.25)))
            .padding(isUser ? .trailing : .leading, cardInset)
    }
}

#Preview("MessageBubble") {
    ScrollView {
        VStack(spacing: 0) {
            MessageBubble(
                message: MessageDisplay(
                    id: "m1", turnId: "t1", role: .user,
                    roleLabel: "claude-code · user",
                    content: "Can you refactor the auth module to use the new JWT middleware?",
                    timestamp: "14:23:01", tokenText: "34 tok"))
            MessageBubble(
                message: MessageDisplay(
                    id: "m2", turnId: "t2", role: .assistant,
                    roleLabel: "Model",
                    content: "I'll read the auth module first, then look at the middleware.",
                    model: "claude-opus-4-8", detail: "high",
                    timestamp: "14:23:02", tokenText: "892 tok",
                    toolCalls: [
                        ToolCallDisplay(
                            id: "tc1", name: "Read",
                            argumentPreview: "/src/auth/middleware.ts",
                            input: "{\n  \"file_path\": \"/src/auth/middleware.ts\"\n}",
                            status: .success, durationText: "42ms"),
                        ToolCallDisplay(
                            id: "tc2", name: "Bash",
                            argumentPreview: "npm test",
                            input: "npm test",
                            status: .error, durationText: "3.2s", exitStatus: 1),
                    ]),
                selected: true)
            MessageBubble(
                message: MessageDisplay(
                    id: "m3", turnId: "t3", role: .assistant,
                    roleLabel: "Model",
                    content: "Delegating the admin routes to a subagent.",
                    model: "claude-opus-4-8", timestamp: "14:23:08",
                    subagent: SubagentDisplay(
                        id: "sa1", traceId: "B7F2A9C1",
                        model: "claude-sonnet-4-6",
                        prompt: "Analyze src/routes/admin.ts and update legacyAuth.",
                        status: .success, durationText: "8.4s", turnCount: 6)),
                onFollowSubagent: { _ in })
            MessageBubble(
                message: MessageDisplay(
                    id: "m4", turnId: "t4", role: .assistant,
                    roleLabel: "Model", model: "claude-opus-4-8",
                    timestamp: "14:23:45",
                    error: "429 rate_limit_error: rate limited, retry in 12s"))
        }
        .padding(.vertical)
    }
    .frame(width: 560, height: 640)
    .background(AlexTheme.Colors.background)
}
