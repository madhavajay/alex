import SwiftUI

/// Plain display values for one compact session list row
/// (TB App.tsx:184-255; Dario App.tsx:142-190).
struct SessionListRowDisplay: Identifiable, Sendable {
    let id: String
    var shortId: String
    var status: DisplayStatus?
    var harness: String?
    var harnessTags: [String: String]?
    var providers: [String] = []
    var model: String?
    var turnsText: String?
    var costText: String?
    var durationText: String?
    var accountText: String?
    var isChild = false
    var hasChildren = false
}

/// Fixed column widths of the compact grid: `1fr 26px 50px 44px 72px`.
enum SessionListColumns {
    static let turns: CGFloat = 26
    static let cost: CGFloat = 50
    static let duration: CGFloat = 44
    static let account: CGFloat = 72
    /// Leading chevron / tree-connector slot.
    static let leadingSlot: CGFloat = 16
}

/// Compact 30px-high session row: [chevron|connector] + status dot + harness
/// icon + provider badge + short id + model badge, then fixed numeric columns.
/// Selection = faint blue wash + 2px right accent border pointing at the next
/// panel; hover = 2.5% white wash.
struct SessionListRow: View {
    let session: SessionListRowDisplay
    var isSelected = false
    var isExpanded = true
    var onToggleExpand: (() -> Void)?
    var action: (() -> Void)?
    @State private var hovering = false

    var body: some View {
        HStack(spacing: 6) {
            leadingSlot
            if let status = session.status {
                StatusDot(status: status)
            }
            HarnessIconView(
                harness: session.harness, tags: session.harnessTags, size: 17,
                showsFallback: true)
            if let provider = session.providers.first {
                ProviderBadgeView(provider: provider, size: 17, style: .tinted)
            }
            Text(session.shortId)
                .font(AlexTheme.Fonts.mono(11))
                .kerning(0.11)
                .foregroundStyle(
                    isSelected ? AlexTheme.Colors.foreground : AlexTheme.Colors.textSecondary)
                .lineLimit(1)
                .layoutPriority(1)
            if let model = session.model {
                ModelBadge(model: model)
            }
            Spacer(minLength: 0)
            numeric(session.turnsText, width: SessionListColumns.turns)
            numeric(session.costText, width: SessionListColumns.cost)
            numeric(session.durationText, width: SessionListColumns.duration, size: 10)
            Text(session.accountText ?? "")
                .font(.system(size: 10))
                .foregroundStyle(AlexTheme.Colors.textFaintest)
                .lineLimit(1)
                .truncationMode(.middle)
                .frame(width: SessionListColumns.account, alignment: .trailing)
        }
        .padding(.leading, 4)
        .padding(.trailing, 8)
        .frame(height: AlexTheme.Metrics.listRowHeight)
        .background(rowBackground)
        .overlay(alignment: .trailing) {
            if isSelected {
                Rectangle().fill(AlexTheme.Colors.primary).frame(width: 2)
            }
        }
        .contentShape(Rectangle())
        .onTapGesture { action?() }
        .onHover { hovering = $0 }
    }

    private var rowBackground: Color {
        if isSelected { return AlexTheme.Colors.primary.opacity(0.07) }
        if hovering { return AlexTheme.Colors.overlay(0.025) }
        return .clear
    }

    @ViewBuilder
    private var leadingSlot: some View {
        if session.isChild {
            // Tree connector: 1×12 vertical + 8×1 horizontal bar in a 24px slot
            // with 8px left pad (TB App.tsx:196-207).
            HStack(spacing: 0) {
                VStack(spacing: 0) {
                    Rectangle()
                        .fill(AlexTheme.Colors.textFaintest)
                        .frame(width: 1, height: 12)
                        .offset(y: -3)
                    Spacer(minLength: 0)
                }
                Rectangle()
                    .fill(AlexTheme.Colors.textFaintest)
                    .frame(width: 8, height: 1)
                Spacer(minLength: 0)
            }
            .padding(.leading, 8)
            .frame(width: 24, height: AlexTheme.Metrics.listRowHeight)
        } else if session.hasChildren {
            Button {
                onToggleExpand?()
            } label: {
                Image(systemName: "chevron.right")
                    .font(.system(size: 8, weight: .semibold))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
                    .rotationEffect(.degrees(isExpanded ? 90 : 0))
                    .frame(
                        width: SessionListColumns.leadingSlot,
                        height: SessionListColumns.leadingSlot)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
        } else {
            Color.clear.frame(width: SessionListColumns.leadingSlot, height: 1)
        }
    }

    private func numeric(_ text: String?, width: CGFloat, size: CGFloat = 10.5) -> some View {
        Text(text ?? "")
            .font(AlexTheme.Fonts.mono(size))
            .foregroundStyle(AlexTheme.Colors.textTertiary)
            .lineLimit(1)
            .frame(width: width, alignment: .trailing)
    }
}

/// Column header row: height 24, 10px dim labels over the fixed grid
/// (TB App.tsx:270-280).
struct SessionListColumnHeader: View {
    var labels = ["Session", "T", "Cost", "Dur", "Account"]

    var body: some View {
        HStack(spacing: 6) {
            Text(labels[0])
                .font(.system(size: 10, weight: .medium))
                .frame(maxWidth: .infinity, alignment: .leading)
            Text(labels[1]).font(.system(size: 10))
                .frame(width: SessionListColumns.turns, alignment: .trailing)
            Text(labels[2]).font(.system(size: 10))
                .frame(width: SessionListColumns.cost, alignment: .trailing)
            Text(labels[3]).font(.system(size: 10))
                .frame(width: SessionListColumns.duration, alignment: .trailing)
            Text(labels[4]).font(.system(size: 10))
                .frame(width: SessionListColumns.account, alignment: .trailing)
        }
        .foregroundStyle(AlexTheme.Colors.textTertiary)
        .padding(.leading, 20)
        .padding(.trailing, 8)
        .frame(height: AlexTheme.Metrics.columnHeaderHeight)
        .overlay(alignment: .bottom) {
            Rectangle().fill(AlexTheme.Colors.cardBorder).frame(height: 1)
        }
    }
}

/// Footer/status strip: height 28, top hairline, mono 10px dim text and an
/// optional leading green dot (TB App.tsx:341-347, transcript footer).
struct SessionListFooter: View {
    let text: String
    var showsDot = false
    var trailingText: String?

    var body: some View {
        HStack(spacing: 6) {
            if showsDot {
                StatusDot(status: .success, size: 6)
            }
            Text(text)
                .font(AlexTheme.Fonts.mono(10))
                .foregroundStyle(AlexTheme.Colors.textTertiary)
            Spacer(minLength: 0)
            if let trailingText {
                Text(trailingText)
                    .font(AlexTheme.Fonts.mono(10))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
            }
        }
        .padding(.horizontal, 12)
        .frame(height: AlexTheme.Metrics.footerHeight)
        .overlay(alignment: .top) {
            Rectangle().fill(AlexTheme.Colors.cardBorder).frame(height: 1)
        }
    }
}

#Preview("SessionListRow") {
    struct Host: View {
        @State private var expanded = true
        var parent = SessionListRowDisplay(
            id: "1", shortId: "sess_a1b2c3", status: .running,
            harness: "claude-code", providers: ["anthropic"],
            model: "claude-opus-4-8", turnsText: "12", costText: "$0.48",
            durationText: "84s", accountText: "me@example.com", hasChildren: true)
        var child = SessionListRowDisplay(
            id: "2", shortId: "agent_d4e5", status: .success,
            harness: "claude-code", providers: ["anthropic"],
            model: "claude-haiku-4-5", turnsText: "6", costText: "$0.02",
            durationText: "8.4s", accountText: "me@example.com", isChild: true)
        var body: some View {
            VStack(spacing: 0) {
                SessionListColumnHeader()
                SessionListRow(
                    session: parent, isSelected: true, isExpanded: expanded,
                    onToggleExpand: { expanded.toggle() })
                if expanded {
                    SessionListRow(session: child)
                }
                SessionListFooter(text: "2 of 24 sessions", showsDot: true)
            }
            .frame(width: 360)
            .background(AlexTheme.Colors.background)
        }
    }
    return Host()
}
