import SwiftUI

/// Compact search input: height 28, radius 8, faint fill, magnifier icon,
/// mono 11px text (shared.tsx:238-253; Dario App.tsx:234-250).
struct SearchField: View {
    @Binding var text: String
    var placeholder = "Search…"

    var body: some View {
        HStack(spacing: AlexTheme.Spacing.md) {
            Image(systemName: "magnifyingglass")
                .font(.system(size: 11))
                .foregroundStyle(AlexTheme.Colors.textTertiary)
            TextField(placeholder, text: $text)
                .textFieldStyle(.plain)
                .font(AlexTheme.Fonts.mono(11))
                .foregroundStyle(AlexTheme.Colors.foreground)
        }
        .padding(.horizontal, AlexTheme.Spacing.ml)
        .frame(height: 28)
        .background(
            RoundedRectangle(cornerRadius: AlexTheme.Radius.md)
                .fill(AlexTheme.Colors.surfaceHover))
        .overlay(
            RoundedRectangle(cornerRadius: AlexTheme.Radius.md)
                .strokeBorder(AlexTheme.Colors.cardBorder))
    }
}

#Preview("SearchField") {
    struct Host: View {
        @State private var text = ""
        var body: some View {
            VStack(spacing: AlexTheme.Spacing.md) {
                SearchField(text: $text, placeholder: "Search sessions…")
                SearchField(text: .constant("status:error"), placeholder: "Filter messages…")
            }
            .padding()
            .frame(width: 320)
            .background(AlexTheme.Colors.background)
        }
    }
    return Host()
}
