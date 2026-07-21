import SwiftUI

/// Column spec for `MiniTable`. `width == nil` means the column flexes.
struct MiniTableColumn: Identifiable {
    let title: String
    var width: CGFloat?
    var alignment: Alignment = .leading

    var id: String { title }
}

/// One cell of a `MiniTable` row.
enum MiniTableCell {
    /// Mono 11px value; optional tint, bold emphasis, truncation mode, and
    /// hover help.
    case text(
        String, tint: Color? = nil, bold: Bool = false,
        truncation: Text.TruncationMode = .tail, help: String? = nil)
    /// Bold mono value over a 9.5px dim truncating subline (Dario cache col).
    case stacked(String, String)
    /// Small bordered action pill (Dario "Clear").
    case button(String, isEnabled: Bool, () -> Void)
    case empty

    /// Enabled action pill (original shorthand).
    static func button(_ title: String, _ action: @escaping () -> Void) -> MiniTableCell {
        .button(title, isEnabled: true, action)
    }
}

/// One context-menu entry for a `MiniTable` row.
struct MiniTableMenuItem: Identifiable {
    let title: String
    let action: () -> Void

    var id: String { title }
}

struct MiniTableRow: Identifiable {
    let id: String
    var cells: [MiniTableCell]
    var isActive = false
    /// Selected row: stronger blue wash + 2px leading accent (Dario
    /// generation table). Takes precedence over `isActive`.
    var isSelected = false
    var height: CGFloat = 36
    /// Row tap handler (row selection).
    var onSelect: (() -> Void)?
    /// Right-click menu entries.
    var contextMenu: [MiniTableMenuItem] = []
}

/// Dario-style data table: 28px `#141414` header band with 10.5px semibold
/// dim labels, hairline-separated mono rows, faint blue active-row wash,
/// radius-8 hairline container (Dario App.tsx:354-506).
struct MiniTable: View {
    let columns: [MiniTableColumn]
    let rows: [MiniTableRow]
    /// Shown under the header band when there are no rows.
    var emptyMessage: String?

    var body: some View {
        VStack(spacing: 0) {
            header
            if rows.isEmpty, let emptyMessage {
                EmptyStateView(message: emptyMessage)
                    .overlay(alignment: .top) {
                        Rectangle().fill(AlexTheme.Colors.cardBorder).frame(height: 1)
                    }
            }
            ForEach(rows) { row in
                rowView(row)
            }
        }
        .background(
            RoundedRectangle(cornerRadius: AlexTheme.Radius.md)
                .fill(AlexTheme.Colors.overlay(0.01)))
        .overlay(
            RoundedRectangle(cornerRadius: AlexTheme.Radius.md)
                .strokeBorder(AlexTheme.Colors.cardBorder))
        .clipShape(RoundedRectangle(cornerRadius: AlexTheme.Radius.md))
    }

    private var header: some View {
        HStack(spacing: 0) {
            ForEach(columns) { column in
                Text(column.title)
                    .font(.system(size: 10.5, weight: .semibold))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
                    .lineLimit(1)
                    .frame(width: column.width, alignment: column.alignment)
                    .frame(maxWidth: column.width == nil ? .infinity : nil,
                           alignment: column.alignment)
            }
        }
        .padding(.horizontal, 16)
        .frame(height: 28)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(AlexTheme.Colors.surfaceSunken)
    }

    private func rowView(_ row: MiniTableRow) -> some View {
        HStack(spacing: 0) {
            ForEach(Array(row.cells.enumerated()), id: \.offset) { index, cell in
                let column = index < columns.count ? columns[index] : nil
                cellView(cell, alignment: column?.alignment ?? .leading)
                    .frame(width: column?.width, alignment: column?.alignment ?? .leading)
                    .frame(maxWidth: column?.width == nil ? .infinity : nil,
                           alignment: column?.alignment ?? .leading)
            }
        }
        .padding(.horizontal, 16)
        .frame(height: row.height)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(rowBackground(row))
        .overlay(alignment: .top) {
            Rectangle().fill(AlexTheme.Colors.cardBorder).frame(height: 1)
        }
        .overlay(alignment: .leading) {
            if row.isSelected {
                Rectangle().fill(AlexTheme.Colors.primary).frame(width: 2)
            }
        }
        .contentShape(Rectangle())
        .onTapGesture { row.onSelect?() }
        .contextMenu {
            ForEach(row.contextMenu) { item in
                Button(item.title, action: item.action)
            }
        }
    }

    private func rowBackground(_ row: MiniTableRow) -> Color {
        if row.isSelected { return AlexTheme.Colors.primary.opacity(0.10) }
        if row.isActive { return AlexTheme.Colors.primary.opacity(0.07) }
        return .clear
    }

    @ViewBuilder
    private func cellView(_ cell: MiniTableCell, alignment: Alignment) -> some View {
        switch cell {
        case .text(let value, let tint, let bold, let truncation, let help):
            Text(value)
                .font(AlexTheme.Fonts.mono(11, weight: bold ? .bold : .regular))
                .foregroundStyle(tint ?? AlexTheme.Colors.foreground)
                .lineLimit(1)
                .truncationMode(truncation)
                .help(help ?? "")
        case .stacked(let value, let subline):
            VStack(alignment: .leading, spacing: 2) {
                Text(value)
                    .font(AlexTheme.Fonts.mono(11, weight: .bold))
                    .foregroundStyle(AlexTheme.Colors.foreground)
                    .lineLimit(1)
                Text(subline)
                    .font(AlexTheme.Fonts.mono(9.5))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
                    .lineLimit(1)
                    .truncationMode(.tail)
            }
            .padding(.trailing, 8)
        case .button(let title, let isEnabled, let action):
            PillButton(
                title: title, variant: .bordered, fontSize: 9.5,
                horizontalPadding: 8, verticalPadding: 3, cornerRadius: 4,
                isEnabled: isEnabled, action: action)
        case .empty:
            Color.clear.frame(width: 1, height: 1)
        }
    }
}

#if DEBUG
#Preview("MiniTable") {
    VStack(alignment: .leading, spacing: AlexTheme.Spacing.md) {
        SectionLabel(text: "Generation", style: .prominent)
        MiniTable(
            columns: [
                MiniTableColumn(title: "generation", width: 140),
                MiniTableColumn(title: "version", width: 60),
                MiniTableColumn(title: "phase", width: 80),
                MiniTableColumn(title: "port", width: 60),
                MiniTableColumn(title: "busy", width: 40),
                MiniTableColumn(title: "age", alignment: .trailing),
            ],
            rows: [
                MiniTableRow(
                    id: "gen-1",
                    cells: [
                        .text("gen-5.1.1-50932",
                              tint: AlexTheme.Colors.primary, bold: true,
                              truncation: .middle),
                        .text("5.1.1"),
                        .text("ready", tint: AlexTheme.Colors.success, bold: true),
                        .text("50932"),
                        .text("0"),
                        .text("59s"),
                    ],
                    isActive: true,
                    isSelected: true,
                    onSelect: {},
                    contextMenu: [
                        MiniTableMenuItem(title: "Copy Generation ID", action: {}),
                        MiniTableMenuItem(title: "Reveal Log in Finder", action: {}),
                    ]),
                MiniTableRow(
                    id: "gen-2",
                    cells: [
                        .text("gen-5.1.0-50921", truncation: .middle),
                        .text("5.1.0"),
                        .text("draining", tint: AlexTheme.Colors.warning),
                        .text("50921"),
                        .text("1"),
                        .text("6m", help: "started 6 minutes ago"),
                    ],
                    onSelect: {}),
            ])
        SectionLabel(text: "Prompt Cache", style: .prominent)
        MiniTable(
            columns: [
                MiniTableColumn(title: "cache", width: 220),
                MiniTableColumn(title: "status", width: 60),
                MiniTableColumn(title: "chars", width: 60),
                MiniTableColumn(title: "action", alignment: .trailing),
            ],
            rows: [
                MiniTableRow(
                    id: "cache-1",
                    cells: [
                        .stacked(
                            "claude-haiku-4-5",
                            "~/.alex/dario-prompt-cache/claude-haiku.json"),
                        .text("hit", tint: AlexTheme.Colors.success),
                        .text("26,941"),
                        .button("Clear", {}),
                    ],
                    height: 48),
                MiniTableRow(
                    id: "cache-2",
                    cells: [
                        .stacked(
                            "claude-opus-4-8",
                            "~/.alex/dario-prompt-cache/claude-opus.json"),
                        .text("miss", tint: AlexTheme.Colors.warningOrange),
                        .text("–"),
                        .button("Clear", isEnabled: false, {}),
                    ],
                    height: 48),
            ])
        SectionLabel(text: "Empty", style: .prominent)
        MiniTable(
            columns: [
                MiniTableColumn(title: "generation"),
                MiniTableColumn(title: "age", alignment: .trailing),
            ],
            rows: [],
            emptyMessage: "No generations")
    }
    .padding()
    .frame(width: 560)
    .background(AlexTheme.Colors.background)
}
#endif
