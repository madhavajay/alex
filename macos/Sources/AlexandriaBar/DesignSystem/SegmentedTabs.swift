import SwiftUI

/// Tiny inline tab picker. Three visual variants appear across the mocks
/// (shared.tsx:213-234; TB App.tsx:313-326; Accounts App.tsx:397-408,
/// 504-518; Dario App.tsx:265-282, 538-551):
/// - `.bare` (default, the original look): free-standing pills, active =
///   faint white wash.
/// - `.contained`: pills inside a bordered container band.
/// - `.solid`: active pill is solid accent blue with white text.
struct SegmentedTabs: View {
    enum Style {
        case bare
        case contained
        case solid
    }

    let tabs: [String]
    @Binding var selection: Int
    var style: Style = .bare
    var fontSize: CGFloat?
    /// Tighter 2×8 pills (dense filter rows).
    var compact = false

    var body: some View {
        switch style {
        case .bare, .solid:
            pills
        case .contained:
            pills
                .padding(2)
                .background(
                    RoundedRectangle(cornerRadius: AlexTheme.Radius.md)
                        .fill(AlexTheme.Colors.overlay(0.05)))
                .overlay(
                    RoundedRectangle(cornerRadius: AlexTheme.Radius.md)
                        .strokeBorder(AlexTheme.Colors.cardBorder))
        }
    }

    private var pills: some View {
        HStack(spacing: AlexTheme.Spacing.xxs) {
            ForEach(tabs.indices, id: \.self) { index in
                Button {
                    selection = index
                } label: {
                    Text(tabs[index])
                        .font(font(selected: selection == index))
                        .foregroundStyle(itemForeground(selected: selection == index))
                        .padding(.horizontal, compact ? AlexTheme.Spacing.md : AlexTheme.Spacing.ml)
                        .padding(.vertical, compact ? AlexTheme.Spacing.xxs : AlexTheme.Spacing.xs)
                        .background(
                            RoundedRectangle(cornerRadius: AlexTheme.Radius.sm)
                                .fill(itemBackground(selected: selection == index)))
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            }
        }
    }

    private func font(selected: Bool) -> Font {
        let size = fontSize ?? defaultFontSize
        if style == .solid, selected {
            return .system(size: size, weight: .semibold)
        }
        return .system(size: size, weight: .medium)
    }

    private var defaultFontSize: CGFloat {
        switch style {
        case .bare: 10
        case .contained, .solid: 10.5
        }
    }

    private func itemForeground(selected: Bool) -> Color {
        switch style {
        case .bare, .contained:
            selected ? AlexTheme.Colors.foreground : AlexTheme.Colors.textTertiary
        case .solid:
            selected ? .white : AlexTheme.Colors.mutedForeground
        }
    }

    private func itemBackground(selected: Bool) -> Color {
        switch style {
        case .bare, .contained:
            selected ? AlexTheme.Colors.surfaceActive : .clear
        case .solid:
            selected ? AlexTheme.Colors.primary : AlexTheme.Colors.surfaceHover
        }
    }
}

#Preview("SegmentedTabs") {
    struct Host: View {
        @State private var first = 0
        @State private var second = 0
        @State private var third = 0
        var body: some View {
            VStack(alignment: .leading, spacing: AlexTheme.Spacing.lg) {
                SegmentedTabs(tabs: ["Input", "Output"], selection: $first)
                SegmentedTabs(
                    tabs: ["All", "User", "Model", "Tools", "Agents"],
                    selection: $second, style: .contained)
                SegmentedTabs(
                    tabs: ["stdout", "stderr"], selection: $third, style: .solid)
                SegmentedTabs(
                    tabs: ["All", "Errors", "Slow"], selection: $second,
                    style: .contained, compact: true)
            }
            .padding()
            .background(AlexTheme.Colors.background)
        }
    }
    return Host()
}
