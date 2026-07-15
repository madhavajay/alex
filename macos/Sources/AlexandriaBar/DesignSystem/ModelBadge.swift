import SwiftUI
import AlexandriaBarCore

/// Per-model tinted chip: mono medium 9.5px, padding 2×6, radius 5, 1px border
/// (shared.tsx:39-44, 163-174). Label derivation lives in Core
/// (`ModelBadgeFormat`) so it is unit-tested.
struct ModelBadge: View {
    let model: String

    var body: some View {
        let style = Self.style(for: ModelBadgeFormat.family(of: model))
        Text(ModelBadgeFormat.label(for: model))
            .font(AlexTheme.Fonts.mono(9.5, weight: .medium))
            .foregroundStyle(style.text)
            .lineLimit(1)
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(RoundedRectangle(cornerRadius: 5).fill(style.background))
            .overlay(RoundedRectangle(cornerRadius: 5).strokeBorder(style.border))
            .fixedSize()
    }

    private struct Style {
        let background: Color
        let text: Color
        let border: Color
    }

    private static func style(for family: ModelBadgeFormat.Family) -> Style {
        switch family {
        case .opus:
            Style(
                background: AlexTheme.Colors.purple.opacity(0.12),
                text: AlexTheme.Colors.purple,
                border: AlexTheme.Colors.purple.opacity(0.25))
        case .sonnet:
            Style(
                background: AlexTheme.Colors.primary.opacity(0.12),
                text: AlexTheme.Colors.primaryBright,
                border: AlexTheme.Colors.primary.opacity(0.25))
        case .haiku:
            Style(
                background: AlexTheme.Colors.success.opacity(0.10),
                text: AlexTheme.Colors.success,
                border: AlexTheme.Colors.success.opacity(0.22))
        case .gpt:
            Style(
                background: AlexTheme.Colors.teal.opacity(0.10),
                text: AlexTheme.Colors.teal,
                border: AlexTheme.Colors.teal.opacity(0.22))
        case .other:
            Style(
                background: AlexTheme.Colors.overlay(0.08),
                text: AlexTheme.Colors.mutedForeground,
                border: AlexTheme.Colors.cardBorder)
        }
    }
}

#if DEBUG
#Preview("ModelBadge") {
    VStack(alignment: .leading, spacing: AlexTheme.Spacing.md) {
        ModelBadge(model: "claude-opus-4-8")
        ModelBadge(model: "claude-sonnet-4-6")
        ModelBadge(model: "claude-haiku-4-5")
        ModelBadge(model: "gpt-4o")
        ModelBadge(model: "gemini-2.0-flash")
    }
    .padding()
    .background(AlexTheme.Colors.background)
}
#endif
