import SwiftUI

/// Empty placeholders (TB App.tsx:333-337, 733-745; Accounts App.tsx:552-555):
/// - `.list`: centered dim caption in an 80pt band ("No sessions match").
/// - `.panel(icon:)`: 20px icon at 40% opacity above the caption, filling the
///   panel ("Select a message to inspect").
/// - `.card`: dashed rounded card with a centered caption ("No accounts
///   connected for X").
struct EmptyStateView: View {
    enum Style {
        case list
        case panel(icon: String)
        case card
    }

    let message: String
    var style: Style = .list

    var body: some View {
        switch style {
        case .list:
            caption(size: 11)
                .frame(maxWidth: .infinity)
                .frame(height: 80)
        case .panel(let icon):
            VStack(spacing: AlexTheme.Spacing.ml) {
                Image(systemName: icon)
                    .font(.system(size: 20))
                    .foregroundStyle(AlexTheme.Colors.textTertiary.opacity(0.4))
                caption(size: 11)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        case .card:
            caption(size: 12)
                .frame(maxWidth: .infinity)
                .padding(24)
                .background(
                    RoundedRectangle(cornerRadius: AlexTheme.Radius.xl)
                        .fill(AlexTheme.Colors.overlay(0.03)))
                .overlay(
                    RoundedRectangle(cornerRadius: AlexTheme.Radius.xl)
                        .strokeBorder(
                            AlexTheme.Colors.overlay(0.1),
                            style: StrokeStyle(lineWidth: 1, dash: [4, 3])))
        }
    }

    private func caption(size: CGFloat) -> some View {
        Text(message)
            .font(.system(size: size))
            .foregroundStyle(AlexTheme.Colors.textTertiary)
            .multilineTextAlignment(.center)
    }
}

#Preview("EmptyStateView") {
    VStack(spacing: AlexTheme.Spacing.xl) {
        EmptyStateView(message: "No sessions match")
        EmptyStateView(
            message: "Select a message to inspect",
            style: .panel(icon: "waveform.path.ecg"))
            .frame(height: 120)
        EmptyStateView(message: "No accounts connected for Gemini", style: .card)
    }
    .padding()
    .frame(width: 320)
    .background(AlexTheme.Colors.background)
}
