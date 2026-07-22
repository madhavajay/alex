import SwiftUI

/// Faint-surface card with a clickable header row and an expandable body.
/// The trailing chevron rotates when the card opens.
struct CollapsibleCard<Header: View, Expanded: View>: View {
    @State private var isExpanded: Bool
    private let header: Header
    private let expanded: Expanded

    init(
        initiallyExpanded: Bool = false,
        @ViewBuilder header: () -> Header,
        @ViewBuilder expanded: () -> Expanded
    ) {
        _isExpanded = State(initialValue: initiallyExpanded)
        self.header = header()
        self.expanded = expanded()
    }

    var body: some View {
        VStack(spacing: 0) {
            Button {
                withAnimation(.easeInOut(duration: 0.15)) { isExpanded.toggle() }
            } label: {
                HStack(spacing: AlexTheme.Spacing.md) {
                    header
                    Image(systemName: "chevron.right")
                        .font(.system(size: 9, weight: .semibold))
                        .foregroundStyle(AlexTheme.Colors.textTertiary)
                        .rotationEffect(.degrees(isExpanded ? 90 : 0))
                }
                .padding(.horizontal, AlexTheme.Spacing.lg)
                .padding(.vertical, AlexTheme.Spacing.md)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            if isExpanded {
                Rectangle()
                    .fill(AlexTheme.Colors.hairline)
                    .frame(height: 1)
                expanded
            }
        }
        .background(
            RoundedRectangle(cornerRadius: AlexTheme.Radius.md)
                .fill(AlexTheme.Colors.surfaceFaint))
        .overlay(
            RoundedRectangle(cornerRadius: AlexTheme.Radius.md)
                .strokeBorder(AlexTheme.Colors.cardBorder))
        .clipShape(RoundedRectangle(cornerRadius: AlexTheme.Radius.md))
    }
}

#if DEBUG
#Preview("CollapsibleCard") {
    VStack(spacing: AlexTheme.Spacing.md) {
        CollapsibleCard {
            Text("Collapsed by default")
                .font(AlexTheme.Fonts.metaLabel)
                .foregroundStyle(AlexTheme.Colors.foreground)
            Spacer()
        } expanded: {
            Text("Hidden details")
                .font(AlexTheme.Fonts.metaMono)
                .foregroundStyle(AlexTheme.Colors.textSecondary)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(AlexTheme.Spacing.lg)
        }
        CollapsibleCard(initiallyExpanded: true) {
            Text("Open by default")
                .font(AlexTheme.Fonts.metaLabel)
                .foregroundStyle(AlexTheme.Colors.foreground)
            Spacer()
        } expanded: {
            Text("Expanded body content")
                .font(AlexTheme.Fonts.metaMono)
                .foregroundStyle(AlexTheme.Colors.textSecondary)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(AlexTheme.Spacing.lg)
        }
    }
    .padding()
    .frame(width: 420)
    .background(AlexTheme.Colors.background)
}
#endif
