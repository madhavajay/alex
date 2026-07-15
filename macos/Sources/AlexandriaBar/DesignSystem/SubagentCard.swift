import SwiftUI

/// Blue-gradient card announcing a delegated subagent run, with a status pill,
/// prompt preview, and a metadata footer including a "Follow trace" action.
struct SubagentCard: View {
    let subagent: SubagentDisplay
    var onFollow: ((String) -> Void)?

    var body: some View {
        VStack(spacing: 0) {
            header
            Rectangle()
                .fill(AlexTheme.Colors.primary.opacity(0.12))
                .frame(height: 1)
            footer
        }
        .background(
            LinearGradient(
                colors: [
                    AlexTheme.Colors.primary.opacity(0.08),
                    AlexTheme.Colors.primary.opacity(0.03),
                ],
                startPoint: .topLeading, endPoint: .bottomTrailing))
        .overlay(
            RoundedRectangle(cornerRadius: AlexTheme.Radius.xl)
                .strokeBorder(AlexTheme.Colors.primary.opacity(0.25)))
        .clipShape(RoundedRectangle(cornerRadius: AlexTheme.Radius.xl))
    }

    private var header: some View {
        HStack(alignment: .top, spacing: AlexTheme.Spacing.ml) {
            RoleAvatar(variant: .subagent, size: 28)
            VStack(alignment: .leading, spacing: AlexTheme.Spacing.xxs) {
                HStack(spacing: AlexTheme.Spacing.md) {
                    Text("SUBAGENT")
                        .font(AlexTheme.Fonts.roleLabel)
                        .kerning(0.5)
                        .foregroundStyle(AlexTheme.Colors.primary)
                    StatusChip(status: subagent.status)
                }
                if !subagent.prompt.isEmpty {
                    Text(subagent.prompt)
                        .font(AlexTheme.Fonts.mono(11))
                        .foregroundStyle(AlexTheme.Colors.textSecondary)
                        .lineLimit(1)
                        .truncationMode(.tail)
                }
            }
            Spacer(minLength: 0)
        }
        .padding(.horizontal, AlexTheme.Spacing.lg)
        .padding(.vertical, AlexTheme.Spacing.ml)
    }

    private var footer: some View {
        HStack(spacing: AlexTheme.Spacing.xl) {
            if let model = subagent.model {
                MetaLabel(systemImage: "cpu", text: model)
            }
            if let turns = subagent.turnCount {
                MetaLabel(systemImage: "square.3.layers.3d", text: "\(turns) turns")
            }
            if let duration = subagent.durationText {
                MetaLabel(systemImage: "clock", text: duration)
            }
            Spacer(minLength: 0)
            if let onFollow {
                Button {
                    onFollow(subagent.traceId)
                } label: {
                    HStack(spacing: AlexTheme.Spacing.xs) {
                        Text("Follow trace")
                        Image(systemName: "arrow.up.right")
                            .font(.system(size: 8, weight: .semibold))
                    }
                    .font(AlexTheme.Fonts.smallControl)
                    .foregroundStyle(AlexTheme.Colors.primary)
                    .padding(.horizontal, AlexTheme.Spacing.md)
                    .padding(.vertical, AlexTheme.Spacing.xs)
                    .background(
                        RoundedRectangle(cornerRadius: AlexTheme.Radius.sm)
                            .fill(AlexTheme.Colors.primary.opacity(0.001)))
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            }
        }
        .padding(.horizontal, AlexTheme.Spacing.lg)
        .padding(.vertical, AlexTheme.Spacing.md)
    }
}

#if DEBUG
#Preview("SubagentCard") {
    VStack(spacing: AlexTheme.Spacing.md) {
        SubagentCard(
            subagent: SubagentDisplay(
                id: "sa1", traceId: "B7F2A9C1-admin-routes",
                model: "claude-sonnet-4-6",
                prompt: "Analyze src/routes/admin.ts and migrate legacyAuth to jwtMiddleware.",
                status: .success, durationText: "8.4s", turnCount: 6),
            onFollow: { _ in })
        SubagentCard(
            subagent: SubagentDisplay(
                id: "sa2", traceId: "C3D4",
                model: "claude-haiku-4",
                prompt: "Run the smoke tests and summarize failures.",
                status: .running))
    }
    .padding()
    .frame(width: 440)
    .background(AlexTheme.Colors.background)
}
#endif
