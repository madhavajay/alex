import SwiftUI

/// 3px rounded quota/usage track (menu App.tsx:249-260; Accounts
/// App.tsx:174-180). `fraction` is the filled portion (0…1). When
/// `warnBelow` is set, the fill turns destructive red once the fraction drops
/// under it (menu variant: pct < 20).
struct QuotaBar: View {
    var fraction: Double
    var fill: Color
    var warnBelow: Double?

    var body: some View {
        GeometryReader { geometry in
            ZStack(alignment: .leading) {
                Capsule().fill(AlexTheme.Colors.overlay(0.08))
                Capsule()
                    .fill(effectiveFill)
                    .frame(width: max(0, geometry.size.width * clamped))
            }
        }
        .frame(height: 3)
    }

    private var clamped: CGFloat {
        CGFloat(min(1, max(0, fraction)))
    }

    private var effectiveFill: Color {
        if let warnBelow, fraction < warnBelow {
            return AlexTheme.Colors.destructive
        }
        return fill
    }
}

/// Menu-row layout: bar + fixed-width mono percent + fixed-width time-left
/// (menu App.tsx:249-260). `leadingLabel` prepends the menu's fixed-width
/// mono window label ("5h" / "7d" / credit captions).
struct QuotaBarRow: View {
    var fraction: Double
    var fill: Color
    var percentText: String
    var timeLeftText: String?
    var warnBelow: Double? = 0.2
    var leadingLabel: String?
    var leadingLabelWidth: CGFloat = 40

    var body: some View {
        HStack(spacing: AlexTheme.Spacing.sm) {
            if let leadingLabel {
                Text(leadingLabel)
                    .font(AlexTheme.Fonts.mono(9))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
                    .lineLimit(1)
                    .frame(width: leadingLabelWidth, alignment: .leading)
            }
            HStack(spacing: AlexTheme.Spacing.md) {
                QuotaBar(fraction: fraction, fill: fill, warnBelow: warnBelow)
                Text(percentText)
                    .font(AlexTheme.Fonts.mono(10))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
                    .frame(width: 28, alignment: .trailing)
                Text(timeLeftText ?? "")
                    .font(AlexTheme.Fonts.mono(10))
                    .foregroundStyle(AlexTheme.Colors.textFaint)
                    .frame(width: 44, alignment: .trailing)
            }
        }
    }
}

/// Accounts-card layout: caption line ("7d · Tokens" / "94% remaining ·
/// resets in 8 days") above the bar (Accounts App.tsx:174-180).
struct LabeledQuotaBar: View {
    var caption: String
    var valueText: String
    var detailText: String?
    var fraction: Double
    var fill: Color

    var body: some View {
        VStack(alignment: .leading, spacing: AlexTheme.Spacing.xs) {
            HStack(spacing: 0) {
                Text(caption)
                    .font(.system(size: 11))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
                Spacer(minLength: AlexTheme.Spacing.md)
                Text(valueText)
                    .font(.system(size: 11))
                    .foregroundStyle(AlexTheme.Colors.textSecondary)
                if let detailText {
                    Text(" · \(detailText)")
                        .font(.system(size: 11))
                        .foregroundStyle(AlexTheme.Colors.textTertiary)
                }
            }
            QuotaBar(fraction: fraction, fill: fill)
        }
    }
}

#if DEBUG
#Preview("QuotaBar") {
    VStack(alignment: .leading, spacing: AlexTheme.Spacing.lg) {
        QuotaBarRow(
            fraction: 0.94, fill: AlexTheme.Colors.primary,
            percentText: "94%", timeLeftText: "3d 4h")
        QuotaBarRow(
            fraction: 0.12, fill: AlexTheme.Colors.primary,
            percentText: "12%", timeLeftText: "6h")
        QuotaBarRow(
            fraction: 0.62, fill: AlexTheme.Colors.success,
            percentText: "62%", timeLeftText: "2d 1h",
            leadingLabel: "7d")
        LabeledQuotaBar(
            caption: "7d · Tokens", valueText: "94% remaining",
            detailText: "resets in 8 days", fraction: 0.94,
            fill: AlexTheme.Colors.primary)
        LabeledQuotaBar(
            caption: "7d · Credits", valueText: "100% remaining",
            fraction: 1, fill: AlexTheme.Colors.chartPalette[1])
    }
    .padding()
    .frame(width: 320)
    .background(AlexTheme.Colors.background)
}
#endif
