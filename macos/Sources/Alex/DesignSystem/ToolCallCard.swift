import SwiftUI
import AlexCore

/// Collapsible tool-call row: icon, name, first-argument preview, duration,
/// status; expands into an Input/Output tab view with mono content.
struct ToolCallCard: View {
    let tool: ToolCallDisplay
    var onViewArgs: (() -> Void)?
    var onViewOutput: (() -> Void)?
    /// Falls back to the captured execution body when the wire-parsed
    /// arguments/result were empty (root cause of "(no input)" showing even
    /// though "View captured args" had real content: the inline tabs only
    /// ever rendered the wire args, never the captured body).
    var loadArgsBody: (() async -> String?)?
    var loadOutputBody: (() async -> String?)?

    @State private var tab = 0
    @State private var capturedArgs: String?
    @State private var capturedOutput: String?
    @State private var loadingArgs = false
    @State private var loadingOutput = false

    var body: some View {
        CollapsibleCard(initiallyExpanded: TranscriptPresentationDefaults.chatBodiesExpanded) {
            headerContent
        } expanded: {
            expandedContent
        }
        .task(id: tool.id) { await loadCapturedIfNeeded() }
    }

    private var headerContent: some View {
        HStack(spacing: AlexTheme.Spacing.md) {
            Image(systemName: tool.iconSystemName)
                .font(.system(size: 10))
                .foregroundStyle(AlexTheme.Colors.mutedForeground)
                .frame(width: 12)
            Text(tool.name)
                .font(AlexTheme.Fonts.metaLabel)
                .foregroundStyle(AlexTheme.Colors.foreground)
                .lineLimit(1)
            if let preview = headerPreview, !preview.isEmpty {
                Text(preview)
                    .font(AlexTheme.Fonts.mono(11))
                    .foregroundStyle(AlexTheme.Colors.mutedForeground)
                    .lineLimit(1)
                    .truncationMode(.tail)
            }
            Spacer(minLength: AlexTheme.Spacing.md)
            if let statusText = tool.statusText, let status = tool.status {
                StatusChip(status: status, text: statusText)
            }
            if let exit = tool.exitStatus, exit != 0 {
                MetaLabel(
                    text: "exit \(exit)", color: AlexTheme.Colors.destructive)
            }
            if let duration = tool.durationText {
                MetaLabel(text: duration)
            }
            if let status = tool.status {
                Image(systemName: status.systemImage)
                    .font(.system(size: 10))
                    .foregroundStyle(status.tint)
            }
        }
    }

    private var expandedContent: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: AlexTheme.Spacing.md) {
                SegmentedTabs(tabs: ["Input", "Output"], selection: $tab)
                Spacer()
                if tool.hasArgsBody, let onViewArgs {
                    bodyLink("View captured args", action: onViewArgs)
                }
                if tool.hasResultBody, let onViewOutput {
                    bodyLink("View output", action: onViewOutput)
                }
            }
            .padding(.horizontal, AlexTheme.Spacing.lg)
            .padding(.top, AlexTheme.Spacing.md)
            ScrollView(.vertical) {
                Text(paneText)
                    .font(AlexTheme.Fonts.metaMono)
                    .foregroundStyle(AlexTheme.Colors.textSecondary)
                    .lineSpacing(2)
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, AlexTheme.Spacing.lg)
                    .padding(.vertical, AlexTheme.Spacing.ml)
            }
            .frame(maxHeight: 200)
        }
    }

    private var paneText: String { tab == 0 ? inputText : outputText }

    /// Header preview prefers the wire-parsed summary; once the captured
    /// args body loads (only fetched when the wire args were empty — see
    /// `loadCapturedIfNeeded`) it's derived from that instead, so commands
    /// like Bash still show inline after the tool name.
    private var headerPreview: String? {
        if let preview = tool.argumentPreview, !preview.isEmpty { return preview }
        guard let capturedArgs, !capturedArgs.isEmpty else { return nil }
        return ChatDisplayFormat.firstArgumentPreview(capturedArgs)
    }

    private var inputText: String {
        if !tool.input.isEmpty { return tool.input }
        if let capturedArgs {
            guard !capturedArgs.isEmpty else { return "(no input)" }
            // Single-string-arg tools (command, file_path, …) render as
            // plain text, not escaped JSON — same treatment as the
            // wire-args path (TranscriptChatMessages.display).
            return ChatDisplayFormat.meaningfulArgumentText(capturedArgs)
                ?? AlexCore.ToolCall.summary(capturedArgs)
        }
        return loadingArgs ? "loading…" : "(no input)"
    }

    private var outputText: String {
        if let output = tool.output, !output.isEmpty { return output }
        if let capturedOutput {
            return capturedOutput.isEmpty ? "(no output captured)" : capturedOutput
        }
        return loadingOutput ? "loading…" : "(no output captured)"
    }

    private func loadCapturedIfNeeded() async {
        if tool.input.isEmpty, tool.hasArgsBody, capturedArgs == nil, let loadArgsBody {
            loadingArgs = true
            capturedArgs = await loadArgsBody() ?? ""
            loadingArgs = false
        }
        if (tool.output ?? "").isEmpty, tool.hasResultBody, capturedOutput == nil,
            let loadOutputBody
        {
            loadingOutput = true
            capturedOutput = await loadOutputBody() ?? ""
            loadingOutput = false
        }
    }

    private func bodyLink(_ title: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack(spacing: AlexTheme.Spacing.xs) {
                Text(title)
                Image(systemName: "arrow.up.right")
                    .font(.system(size: 8, weight: .semibold))
            }
            .font(AlexTheme.Fonts.smallControl)
            .foregroundStyle(AlexTheme.Colors.primary)
            .padding(.horizontal, AlexTheme.Spacing.md)
            .padding(.vertical, AlexTheme.Spacing.xs)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}

#if DEBUG
#Preview("ToolCallCard") {
    VStack(spacing: AlexTheme.Spacing.sm) {
        ToolCallCard(
            tool: ToolCallDisplay(
                id: "tc1", name: "Read",
                argumentPreview: "/src/auth/middleware.ts",
                input: "{\n  \"file_path\": \"/src/auth/middleware.ts\"\n}",
                output: "import jwt from 'jsonwebtoken';\n// …",
                status: .success, durationText: "42ms"))
        ToolCallCard(
            tool: ToolCallDisplay(
                id: "tc2", name: "Bash",
                argumentPreview: "npm test -- --testPathPattern=auth",
                input: "npm test -- --testPathPattern=auth",
                status: .error, durationText: "3.2s", exitStatus: 1,
                hasArgsBody: true, hasResultBody: true),
            onViewArgs: {}, onViewOutput: {})
        ToolCallCard(
            tool: ToolCallDisplay(
                id: "tc3", name: "Grep",
                argumentPreview: "legacyAuth",
                input: "{\n  \"pattern\": \"legacyAuth\"\n}",
                status: .running, statusText: "running"))
    }
    .padding()
    .frame(width: 440)
    .background(AlexTheme.Colors.background)
}
#endif
