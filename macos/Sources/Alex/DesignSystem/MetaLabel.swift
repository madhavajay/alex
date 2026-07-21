import SwiftUI

/// Icon + micro mono text pair for metadata rows (model id, turn count,
/// durations, token counts).
struct MetaLabel: View {
    var systemImage: String?
    let text: String
    var font: Font = AlexTheme.Fonts.metaMicro
    var color: Color = AlexTheme.Colors.textTertiary

    var body: some View {
        HStack(spacing: AlexTheme.Spacing.xs) {
            if let systemImage {
                Image(systemName: systemImage)
                    .font(.system(size: 9))
            }
            Text(text)
                .font(font)
                .lineLimit(1)
                .truncationMode(.middle)
        }
        .foregroundStyle(color)
    }
}

#if DEBUG
#Preview("MetaLabel") {
    VStack(alignment: .leading, spacing: AlexTheme.Spacing.md) {
        MetaLabel(systemImage: "cpu", text: "claude-sonnet-4-6")
        MetaLabel(systemImage: "square.3.layers.3d", text: "6 turns")
        MetaLabel(systemImage: "clock", text: "8.4s")
        MetaLabel(text: "892 tok", color: AlexTheme.Colors.textFaint)
    }
    .padding()
    .background(AlexTheme.Colors.background)
}
#endif
