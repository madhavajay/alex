import SwiftUI

/// Collapsible tool-call row: icon, name, first-argument preview, duration,
/// status; expands into an Input/Output tab view with mono content.
struct ToolCallCard: View {
    let tool: ToolCallDisplay
    var onViewArgs: (() -> Void)?
    var onViewOutput: (() -> Void)?

    @State private var tab = 0

    var body: some View {
        CollapsibleCard {
            headerContent
        } expanded: {
            expandedContent
        }
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
            if let preview = tool.argumentPreview, !preview.isEmpty {
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

    private var paneText: String {
        if tab == 0 {
            return tool.input.isEmpty ? "(no input)" : tool.input
        }
        return tool.output ?? "(no output captured)"
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
