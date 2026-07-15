import SwiftUI

/// Tiny tinted text chip. The original form is the status pill (9px mono on a
/// 10%-tint background); the Settings mock adds fixed-color semibold badges
/// (Create Settings App.tsx:170-181) and a mini "upd" variant.
struct StatusChip: View {
    /// Visual treatments:
    /// - `.mono`: 9px mono, padding 2×6, radius 6, tint @10% bg (original).
    /// - `.badge`: 10px semibold, padding 2×6, radius 4, tint @15% bg.
    /// - `.mini`: 9px semibold, padding 1×4, radius 3, tint @15% bg ("upd").
    enum Style {
        case mono
        case badge
        case mini
    }

    let tint: Color
    var text: String
    var style: Style = .mono

    init(status: DisplayStatus, text: String? = nil) {
        self.tint = status.tint
        self.text = text ?? status.rawValue
        self.style = .mono
    }

    init(tint: Color, text: String, style: Style = .badge) {
        self.tint = tint
        self.text = text
        self.style = style
    }

    var body: some View {
        Text(text)
            .font(font)
            .foregroundStyle(tint)
            .padding(.horizontal, horizontalPadding)
            .padding(.vertical, verticalPadding)
            .background(
                RoundedRectangle(cornerRadius: cornerRadius)
                    .fill(tint.opacity(backgroundOpacity)))
            .fixedSize()
    }

    private var font: Font {
        switch style {
        case .mono: AlexTheme.Fonts.chipMono
        case .badge: .system(size: 10, weight: .semibold)
        case .mini: .system(size: 9, weight: .semibold)
        }
    }

    private var horizontalPadding: CGFloat {
        switch style {
        case .mono, .badge: AlexTheme.Spacing.sm
        case .mini: AlexTheme.Spacing.xs
        }
    }

    private var verticalPadding: CGFloat {
        switch style {
        case .mono, .badge: AlexTheme.Spacing.xxs
        case .mini: 1
        }
    }

    private var cornerRadius: CGFloat {
        switch style {
        case .mono: AlexTheme.Radius.sm
        case .badge: AlexTheme.Radius.xs
        case .mini: 3
        }
    }

    private var backgroundOpacity: Double {
        switch style {
        case .mono: 0.1
        case .badge, .mini: 0.15
        }
    }
}

#Preview("StatusChip") {
    VStack(alignment: .leading, spacing: AlexTheme.Spacing.md) {
        HStack(spacing: AlexTheme.Spacing.md) {
            StatusChip(status: .success)
            StatusChip(status: .error)
            StatusChip(status: .running)
            StatusChip(status: .pending)
            StatusChip(status: .running, text: "requested")
        }
        HStack(spacing: AlexTheme.Spacing.md) {
            StatusChip(tint: AlexTheme.Colors.primary, text: "configured")
            StatusChip(tint: AlexTheme.Colors.success, text: "connected")
            StatusChip(tint: AlexTheme.Colors.warningOrange, text: "not set")
            StatusChip(tint: AlexTheme.Colors.warningOrange, text: "upd", style: .mini)
        }
    }
    .padding()
    .background(AlexTheme.Colors.background)
}
