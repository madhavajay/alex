import SwiftUI

/// Round (or rounded-square for subagents) role marker used beside messages.
struct RoleAvatar: View {
    enum Variant {
        case user
        case assistant
        case subagent
        /// Tool-result reply fed back to the model by the harness (mock has
        /// no equivalent — this app added a distinct "Harness" role for
        /// what used to render under the user's own vocabulary).
        case harness
    }

    let variant: Variant
    var size: CGFloat = 24

    var body: some View {
        ZStack {
            switch variant {
            case .user:
                Circle()
                    .fill(AlexTheme.Colors.secondaryFill)
                Circle()
                    .strokeBorder(AlexTheme.Colors.avatarBorder)
                Image(systemName: "person.fill")
                    .font(.system(size: size * 0.5))
                    .foregroundStyle(AlexTheme.Colors.mutedForeground)
            case .assistant:
                Circle()
                    .fill(
                        LinearGradient(
                            colors: [
                                AlexTheme.Colors.primary.opacity(0.4),
                                AlexTheme.Colors.indigo.opacity(0.4),
                            ],
                            startPoint: .topLeading, endPoint: .bottomTrailing))
                Circle()
                    .strokeBorder(AlexTheme.Colors.primary.opacity(0.4))
                Image(systemName: "cpu")
                    .font(.system(size: size * 0.5, weight: .medium))
                    .foregroundStyle(AlexTheme.Colors.primaryBright)
            case .subagent:
                RoundedRectangle(cornerRadius: AlexTheme.Radius.md)
                    .fill(AlexTheme.Colors.primary.opacity(0.15))
                Image(systemName: "arrow.triangle.branch")
                    .font(.system(size: size * 0.46, weight: .semibold))
                    .foregroundStyle(AlexTheme.Colors.primary)
            case .harness:
                Circle()
                    .fill(AlexTheme.Colors.warningOrange.opacity(0.15))
                Circle()
                    .strokeBorder(AlexTheme.Colors.warningOrange.opacity(0.4))
                Image(systemName: "wrench.and.screwdriver.fill")
                    .font(.system(size: size * 0.46, weight: .semibold))
                    .foregroundStyle(AlexTheme.Colors.warningOrange)
            }
        }
        .frame(width: size, height: size)
    }
}

#Preview("RoleAvatar") {
    HStack(spacing: AlexTheme.Spacing.lg) {
        RoleAvatar(variant: .user)
        RoleAvatar(variant: .assistant)
        RoleAvatar(variant: .subagent, size: 28)
        RoleAvatar(variant: .harness)
    }
    .padding()
    .background(AlexTheme.Colors.background)
}
