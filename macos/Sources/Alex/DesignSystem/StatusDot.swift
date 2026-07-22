import SwiftUI

/// Plain filled status circle (Trace Browser rows 6px, Dario/menu 5px,
/// Accounts 7-8px with glow). Tint comes from `DisplayStatus.tint` or is
/// passed directly for provider/brand dots.
struct StatusDot: View {
    var tint: Color
    var size: CGFloat = 6
    var glow: Bool = false
    /// Outline-only ring (not-installed / absent semantics).
    var hollow: Bool = false

    init(tint: Color, size: CGFloat = 6, glow: Bool = false, hollow: Bool = false) {
        self.tint = tint
        self.size = size
        self.glow = glow
        self.hollow = hollow
    }

    init(status: DisplayStatus, size: CGFloat = 6, glow: Bool = false, hollow: Bool = false) {
        self.init(tint: status.tint, size: size, glow: glow, hollow: hollow)
    }

    var body: some View {
        Group {
            if hollow {
                Circle().strokeBorder(tint, lineWidth: 1)
            } else {
                Circle().fill(tint)
            }
        }
        // Mock glow: boxShadow 0 0 5px <tint>88 (Accounts App.tsx:182-189).
        .shadow(color: glow ? tint.opacity(0.53) : .clear, radius: glow ? 2.5 : 0)
        .frame(width: size, height: size)
    }
}

/// Accounts "StatusBadge": 7px glowing dot + 11px medium tinted label
/// (Accounts App.tsx:182-189).
struct StatusBadge: View {
    var text = "Active"
    var tint: Color = AlexTheme.Colors.success

    var body: some View {
        HStack(spacing: 5) {
            StatusDot(tint: tint, size: 7, glow: true)
            Text(text)
                .font(.system(size: 11, weight: .medium))
                .foregroundStyle(tint)
        }
        .fixedSize()
    }
}

#if DEBUG
#Preview("StatusDot") {
    VStack(alignment: .leading, spacing: AlexTheme.Spacing.lg) {
        HStack(spacing: AlexTheme.Spacing.md) {
            StatusDot(status: .success)
            StatusDot(status: .error)
            StatusDot(status: .running)
            StatusDot(status: .pending)
            StatusDot(tint: AlexTheme.Colors.warningOrange, size: 5)
            StatusDot(tint: AlexTheme.Colors.textTertiary, size: 8, hollow: true)
        }
        StatusBadge()
        StatusBadge(text: "Paused", tint: AlexTheme.Colors.warningOrange)
    }
    .padding()
    .background(AlexTheme.Colors.background)
}
#endif
