import SwiftUI

/// Uppercase section label. Three tiers across the mocks:
/// - `.settings`: 10px semibold #48484a, tracking 0.07em (Create Settings).
/// - `.menu`: 10px semibold #636366, tracking-wider (macOS menu).
/// - `.prominent`: 11px semibold #8e8e93 (Dario "GENERATION"/"PROMPT CACHE").
/// An optional trailing accessory turns it into a full-width header row
/// (label — spacer — accessory), e.g. the menu's "Refresh / Ping" buttons.
struct SectionLabel<Accessory: View>: View {
    enum Style {
        case settings
        case menu
        case prominent
    }

    let text: String
    var style: Style = .settings
    private let accessory: Accessory?

    init(text: String, style: Style = .settings) where Accessory == EmptyView {
        self.text = text
        self.style = style
        self.accessory = nil
    }

    init(
        text: String, style: Style = .settings,
        @ViewBuilder accessory: () -> Accessory
    ) {
        self.text = text
        self.style = style
        self.accessory = accessory()
    }

    var body: some View {
        if let accessory {
            HStack(spacing: AlexTheme.Spacing.xxs) {
                label
                Spacer(minLength: AlexTheme.Spacing.md)
                accessory
            }
        } else {
            label
        }
    }

    private var label: some View {
        Text(text.uppercased())
            .font(.system(size: fontSize, weight: .semibold))
            .tracking(fontSize * 0.07)
            .foregroundStyle(color)
    }

    private var fontSize: CGFloat {
        switch style {
        case .settings, .menu: 10
        case .prominent: 11
        }
    }

    private var color: Color {
        switch style {
        case .settings: AlexTheme.Colors.textFaint
        case .menu: AlexTheme.Colors.textTertiary
        case .prominent: AlexTheme.Colors.mutedForeground
        }
    }
}

#Preview("SectionLabel") {
    VStack(alignment: .leading, spacing: AlexTheme.Spacing.md) {
        SectionLabel(text: "System")
        SectionLabel(text: "Providers", style: .menu)
        SectionLabel(text: "Generation", style: .prominent)
        SectionLabel(text: "Providers", style: .menu) {
            Text("2 active")
                .font(AlexTheme.Fonts.mono(9))
                .foregroundStyle(AlexTheme.Colors.textFaint)
        }
    }
    .padding()
    .frame(width: 220)
    .background(AlexTheme.Colors.background)
}
