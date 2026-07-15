import SwiftUI

/// Small pill button covering the mocks' recurring button treatments
/// (Create Settings App.tsx:183-205; Dario App.tsx:338-347; Accounts
/// App.tsx:306-311, 713-723; menu App.tsx:631-633). Each variant carries the
/// spec's metrics; individual metrics can be overridden per call site.
struct PillButton: View {
    enum Variant {
        /// bg overlay(0.08), foreground text, hover overlay(0.13).
        case standard
        /// Tinted accent: rgba(10,132,255,0.18) / #0a84ff, hover 0.28.
        case primary
        /// Tinted destructive: rgba(255,69,58,0.1) / #ff453a, hover 0.2.
        case danger
        /// Faint fill + hairline border (Dario "Restart" / "Check Update").
        case bordered
        /// Solid accent, white text; disabled → faint fill + ghost text.
        case solidAccent
        /// Solid orange CTA (menu "Update Both").
        case solidOrange
    }

    let title: String
    var variant: Variant = .standard
    /// Replaces the variant's accent (text tint on `.standard`/`.bordered`,
    /// accent on `.primary`/`.solidAccent`) — e.g. provider brand or the
    /// Accounts "Resume account" green.
    var tint: Color?
    /// Optional leading SF Symbol.
    var systemImage: String?
    var fontSize: CGFloat?
    var horizontalPadding: CGFloat?
    var verticalPadding: CGFloat?
    var cornerRadius: CGFloat?
    var showsBorder: Bool?
    var isEnabled = true
    /// Shows a small trailing spinner (typically paired with `isEnabled: false`).
    var isBusy = false
    var keyboardShortcut: KeyboardShortcut?
    let action: () -> Void
    @State private var hovering = false

    var body: some View {
        let button = Button(action: action) {
            HStack(spacing: AlexTheme.Spacing.xs) {
                if let systemImage {
                    Image(systemName: systemImage)
                        .font(.system(size: resolvedFontSize, weight: fontWeight))
                }
                Text(title)
                    .font(.system(size: resolvedFontSize, weight: fontWeight))
                if isBusy {
                    ProgressView()
                        .controlSize(.small)
                        .scaleEffect(0.6)
                        .frame(width: resolvedFontSize, height: resolvedFontSize)
                }
            }
            .foregroundStyle(textColor)
            .opacity(contentOpacity)
            .padding(.horizontal, horizontalPadding ?? defaultHorizontalPadding)
            .padding(.vertical, verticalPadding ?? defaultVerticalPadding)
            .background(
                RoundedRectangle(cornerRadius: radius).fill(backgroundColor))
            .overlay {
                if showsBorder ?? (variant == .bordered) {
                    RoundedRectangle(cornerRadius: radius)
                        .strokeBorder(borderColor)
                }
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(!isEnabled)
        .onHover { hovering = $0 }
        if let keyboardShortcut {
            button.keyboardShortcut(keyboardShortcut)
        } else {
            button
        }
    }

    private var resolvedFontSize: CGFloat {
        if let fontSize { return fontSize }
        switch variant {
        case .solidAccent: return 12
        default: return 11
        }
    }

    private var fontWeight: Font.Weight {
        switch variant {
        case .bordered: .semibold
        case .solidOrange: .bold
        default: .medium
        }
    }

    private var defaultHorizontalPadding: CGFloat {
        switch variant {
        case .standard, .primary, .danger: 8
        case .bordered: 12
        case .solidAccent: 16
        case .solidOrange: 11
        }
    }

    private var defaultVerticalPadding: CGFloat {
        switch variant {
        case .standard, .primary, .danger: 3
        case .bordered: 6
        case .solidAccent: 7
        case .solidOrange: 4
        }
    }

    private var radius: CGFloat {
        if let cornerRadius { return cornerRadius }
        switch variant {
        case .standard, .primary, .danger: return 5
        case .bordered: return 6
        case .solidAccent: return 8
        case .solidOrange: return 7
        }
    }

    /// `.solidAccent` keeps its dedicated disabled treatment (faint fill +
    /// ghost text); other variants dim their content.
    private var contentOpacity: CGFloat {
        if variant == .solidAccent { return 1 }
        return isEnabled ? 1 : 0.5
    }

    private var textColor: Color {
        switch variant {
        case .standard, .bordered:
            tint ?? AlexTheme.Colors.foreground
        case .primary:
            tint ?? AlexTheme.Colors.primary
        case .danger:
            tint ?? AlexTheme.Colors.destructive
        case .solidAccent:
            isEnabled ? .white : AlexTheme.Colors.textFaintest
        case .solidOrange:
            AlexTheme.Colors.dynamic(light: 0xFFFFFF, dark: 0x1C1C1E)
        }
    }

    private var backgroundColor: Color {
        let hovered = hovering && isEnabled
        switch variant {
        case .standard:
            return AlexTheme.Colors.overlay(hovered ? 0.13 : 0.08)
        case .primary:
            return (tint ?? AlexTheme.Colors.primary).opacity(hovered ? 0.28 : 0.18)
        case .danger:
            return (tint ?? AlexTheme.Colors.destructive).opacity(hovered ? 0.2 : 0.1)
        case .bordered:
            return AlexTheme.Colors.overlay(hovered ? 0.10 : 0.06)
        case .solidAccent:
            return isEnabled
                ? (tint ?? AlexTheme.Colors.primary) : AlexTheme.Colors.surfaceHover
        case .solidOrange:
            return hovered
                ? AlexTheme.Colors.dynamic(light: 0xFFAD33, dark: 0xFFAD33)
                : AlexTheme.Colors.dynamic(light: 0xFF9500, dark: 0xFF9500)
        }
    }

    private var borderColor: Color {
        switch variant {
        case .danger: (tint ?? AlexTheme.Colors.destructive).opacity(0.18)
        default: AlexTheme.Colors.cardBorder
        }
    }
}

/// The dashed "add" treatment as a plain label — for wrapping in a `Menu`
/// (whose label cannot own an action). `DashedAddButton` composes this.
struct DashedAddLabel: View {
    var title: String
    var style: DashedAddButton.Style = .accent
    var cornerRadius: CGFloat = 8
    var fontSize: CGFloat = 12
    var verticalPadding: CGFloat = 8
    var horizontalPadding: CGFloat = 0
    var fillsWidth = true
    /// External hover state (`.gray` brightens on hover); the owning control
    /// tracks hover since a bare label cannot.
    var hovering = false

    var body: some View {
        Text(title)
            .font(.system(size: fontSize, weight: .medium))
            .foregroundStyle(textColor)
            .padding(.vertical, verticalPadding)
            .padding(.horizontal, horizontalPadding)
            .frame(maxWidth: fillsWidth ? .infinity : nil)
            .background(
                RoundedRectangle(cornerRadius: cornerRadius).fill(backgroundColor))
            .overlay(
                RoundedRectangle(cornerRadius: cornerRadius)
                    .strokeBorder(
                        borderColor, style: StrokeStyle(lineWidth: 1, dash: [4, 3])))
            .contentShape(Rectangle())
    }

    private var textColor: Color {
        switch style {
        case .accent:
            AlexTheme.Colors.primary
        case .gray:
            hovering ? AlexTheme.Colors.textTertiary : AlexTheme.Colors.textFaint
        }
    }

    private var backgroundColor: Color {
        switch style {
        case .accent: AlexTheme.Colors.primary.opacity(0.10)
        case .gray: .clear
        }
    }

    private var borderColor: Color {
        switch style {
        case .accent: AlexTheme.Colors.primary.opacity(0.3)
        case .gray: AlexTheme.Colors.overlay(0.1)
        }
    }
}

/// Full-width dashed "add" button ("+ Add provider" / "+ Add account";
/// Accounts App.tsx:465-476, 563-568; Settings gray variant App.tsx:712-715).
struct DashedAddButton: View {
    enum Style {
        case accent
        case gray
    }

    let title: String
    var style: Style = .accent
    var cornerRadius: CGFloat = 8
    let action: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            DashedAddLabel(
                title: title, style: style, cornerRadius: cornerRadius,
                hovering: hovering)
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
    }
}

#Preview("PillButton") {
    VStack(alignment: .leading, spacing: AlexTheme.Spacing.lg) {
        HStack(spacing: AlexTheme.Spacing.md) {
            PillButton(title: "Configure") {}
            PillButton(title: "Update", variant: .primary) {}
            PillButton(
                title: "Remove", variant: .danger, horizontalPadding: 12,
                verticalPadding: 5, cornerRadius: 7, showsBorder: true) {}
        }
        HStack(spacing: AlexTheme.Spacing.md) {
            PillButton(title: "Restart", variant: .bordered) {}
            PillButton(title: "Check Update", variant: .bordered) {}
            PillButton(title: "Clear", variant: .bordered, fontSize: 9.5,
                horizontalPadding: 8, verticalPadding: 3, cornerRadius: 4) {}
        }
        HStack(spacing: AlexTheme.Spacing.md) {
            PillButton(title: "Save routing", variant: .solidAccent) {}
            PillButton(title: "Saved", variant: .solidAccent, isEnabled: false) {}
            PillButton(title: "Update Both", variant: .solidOrange) {}
        }
        HStack(spacing: AlexTheme.Spacing.md) {
            PillButton(
                title: "Resume account", variant: .standard,
                tint: AlexTheme.Colors.success, horizontalPadding: 12,
                verticalPadding: 5, cornerRadius: 7, showsBorder: true) {}
            PillButton(
                title: "Copy env", systemImage: "doc.on.doc",
                horizontalPadding: 12, verticalPadding: 5, cornerRadius: 7) {}
            PillButton(
                title: "Connect", variant: .primary,
                tint: AlexTheme.ProviderBrand.brand(for: "anthropic").authAccent) {}
        }
        HStack(spacing: AlexTheme.Spacing.md) {
            PillButton(title: "Refresh", variant: .bordered, isEnabled: false) {}
            PillButton(title: "Remove", variant: .danger, isEnabled: false) {}
            PillButton(
                title: "Updating", variant: .bordered, isEnabled: false,
                isBusy: true) {}
            PillButton(
                title: "Save", variant: .solidAccent,
                keyboardShortcut: .defaultAction) {}
        }
        DashedAddButton(title: "+ Add account") {}
        DashedAddButton(title: "+ Add harness", style: .gray, cornerRadius: 10) {}
        Menu {
            Button("Add Anthropic") {}
            Button("Add OpenAI") {}
        } label: {
            DashedAddLabel(title: "+ Add provider", fontSize: 11, verticalPadding: 7)
        }
        .menuStyle(.button)
        .buttonStyle(.plain)
        .menuIndicator(.hidden)
    }
    .padding()
    .frame(width: 380)
    .background(AlexTheme.Colors.background)
}
