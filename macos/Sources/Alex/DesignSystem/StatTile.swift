import SwiftUI

/// One labeled statistic for a `StatTilesRow`.
struct StatTileData: Identifiable, Sendable {
    let label: String
    let value: String
    var valueTint: Color?

    var id: String { label }
}

/// Three visual homes for the same 3-up stats strip:
/// - `.menu`: menu stats bar — 14px mono semibold value over 9px uppercase
///   label, 1×20 dividers (menu App.tsx:696-708).
/// - `.inset`: Accounts 24h grid — inset faint panel, label above 13px value
///   (Accounts App.tsx:238-253).
/// - `.bordered`: TB event quick stats — right hairlines between columns,
///   bottom border under the row (TB App.tsx:788-801).
struct StatTilesRow: View {
    enum Style {
        case menu
        case inset
        case bordered
    }

    let items: [StatTileData]
    var style: Style = .menu

    var body: some View {
        switch style {
        case .menu: menuRow
        case .inset: insetRow
        case .bordered: borderedRow
        }
    }

    private var menuRow: some View {
        HStack(spacing: 0) {
            ForEach(Array(items.enumerated()), id: \.element.id) { index, item in
                VStack(spacing: 2) {
                    Text(item.value)
                        .font(AlexTheme.Fonts.statValue)
                        .foregroundStyle(item.valueTint ?? AlexTheme.Colors.foreground)
                    Text(item.label.uppercased())
                        .font(.system(size: 9))
                        .tracking(0.45)
                        .foregroundStyle(AlexTheme.Colors.textFaint)
                }
                .frame(maxWidth: .infinity)
                if index < items.count - 1 {
                    Rectangle()
                        .fill(AlexTheme.Colors.overlay(0.08))
                        .frame(width: 1, height: 20)
                }
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
    }

    private var insetRow: some View {
        HStack(spacing: 0) {
            ForEach(items) { item in
                VStack(alignment: .leading, spacing: 3) {
                    Text(item.label.uppercased())
                        .font(.system(size: 10))
                        .tracking(0.5)
                        .foregroundStyle(AlexTheme.Colors.textTertiary)
                    Text(item.value)
                        .font(.system(size: 13, weight: .semibold))
                        .foregroundStyle(item.valueTint ?? AlexTheme.Colors.foreground)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        .padding(12)
        .background(
            RoundedRectangle(cornerRadius: AlexTheme.Radius.md)
                .fill(AlexTheme.Colors.surfaceFaint))
    }

    private var borderedRow: some View {
        HStack(spacing: 0) {
            ForEach(Array(items.enumerated()), id: \.element.id) { index, item in
                VStack(alignment: .leading, spacing: 2) {
                    Text(item.value)
                        .font(AlexTheme.Fonts.mono(12, weight: .semibold))
                        .foregroundStyle(item.valueTint ?? AlexTheme.Colors.textSecondary)
                    Text(item.label)
                        .font(.system(size: 9.5))
                        .foregroundStyle(AlexTheme.Colors.textTertiary)
                }
                .padding(.horizontal, 10)
                .padding(.vertical, 6)
                .frame(maxWidth: .infinity, alignment: .leading)
                .overlay(alignment: .trailing) {
                    if index < items.count - 1 {
                        Rectangle().fill(AlexTheme.Colors.cardBorder).frame(width: 1)
                    }
                }
            }
        }
        .overlay(alignment: .bottom) {
            Rectangle().fill(AlexTheme.Colors.cardBorder).frame(height: 1)
        }
    }
}

#if DEBUG
#Preview("StatTilesRow") {
    VStack(spacing: AlexTheme.Spacing.xl) {
        StatTilesRow(items: [
            StatTileData(label: "requests", value: "39"),
            StatTileData(label: "last hour", value: "$0.0039"),
            StatTileData(
                label: "errors", value: "4", valueTint: AlexTheme.Colors.destructive),
        ])
        StatTilesRow(
            items: [
                StatTileData(label: "Requests 24h", value: "334"),
                StatTileData(label: "Tokens 24h", value: "33.0M"),
                StatTileData(
                    label: "Errors", value: "5", valueTint: AlexTheme.Colors.destructive),
            ], style: .inset)
        StatTilesRow(
            items: [
                StatTileData(label: "Method", value: "POST"),
                StatTileData(label: "Duration", value: "1.24s"),
                StatTileData(label: "Tokens", value: "2,847"),
            ], style: .bordered)
    }
    .padding()
    .frame(width: 340)
    .background(AlexTheme.Colors.background)
}
#endif
