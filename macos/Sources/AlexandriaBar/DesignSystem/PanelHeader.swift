import SwiftUI

/// Panel header band: height 48, 1px bottom hairline; `accentLeft` draws the
/// 2px blue "receiving" connection indicator and tightens the left padding
/// (shared.tsx:279-297).
struct PanelHeader<Left: View, Right: View>: View {
    var accentLeft = false
    private let left: Left
    private let right: Right

    init(
        accentLeft: Bool = false,
        @ViewBuilder left: () -> Left,
        @ViewBuilder right: () -> Right = { EmptyView() }
    ) {
        self.accentLeft = accentLeft
        self.left = left()
        self.right = right()
    }

    var body: some View {
        HStack(spacing: AlexTheme.Spacing.md) {
            HStack(spacing: AlexTheme.Spacing.md) { left }
                .frame(maxWidth: .infinity, alignment: .leading)
            HStack(spacing: AlexTheme.Spacing.xxs) { right }
                .fixedSize()
        }
        .padding(.leading, accentLeft ? 10 : 12)
        .padding(.trailing, 8)
        .frame(height: AlexTheme.Metrics.panelHeaderHeight)
        .overlay(alignment: .bottom) {
            Rectangle().fill(AlexTheme.Colors.cardBorder).frame(height: 1)
        }
        .overlay(alignment: .leading) {
            if accentLeft {
                Rectangle().fill(AlexTheme.Colors.primary).frame(width: 2)
            }
        }
    }
}

/// Count badge next to a panel title: mono 10px on a faint pill
/// (shared.tsx panel headers).
struct PanelCountBadge: View {
    let text: String
    var tint: Color = AlexTheme.Colors.mutedForeground

    init(text: String, tint: Color = AlexTheme.Colors.mutedForeground) {
        self.text = text
        self.tint = tint
    }

    init(count: Int, tint: Color = AlexTheme.Colors.mutedForeground) {
        self.init(text: "\(count)", tint: tint)
    }

    var body: some View {
        Text(text)
            .font(AlexTheme.Fonts.mono(10))
            .foregroundStyle(tint)
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(
                RoundedRectangle(cornerRadius: AlexTheme.Radius.xs)
                    .fill(AlexTheme.Colors.surfaceHover))
            .fixedSize()
    }
}

/// 28×28 icon button for the right slot of a `PanelHeader`
/// (radius 8, hover wash overlay(0.08)).
struct PanelIconButton: View {
    let systemImage: String
    var help: String?
    let action: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            Image(systemName: systemImage)
                .font(.system(size: 12))
                .foregroundStyle(
                    hovering ? AlexTheme.Colors.foreground : AlexTheme.Colors.textTertiary)
                .frame(width: 28, height: 28)
                .background(
                    RoundedRectangle(cornerRadius: AlexTheme.Radius.md)
                        .fill(hovering ? AlexTheme.Colors.overlay(0.08) : Color.clear))
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .help(help ?? "")
    }
}

/// Filter bar under a panel header: height 40, bottom hairline, 80% panel
/// background (shared.tsx:301-308).
struct FilterRow<Content: View>: View {
    private let content: Content

    init(@ViewBuilder content: () -> Content) {
        self.content = content()
    }

    var body: some View {
        HStack(spacing: AlexTheme.Spacing.md) { content }
            .padding(.horizontal, 12)
            .frame(height: AlexTheme.Metrics.filterRowHeight)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(AlexTheme.Colors.background.opacity(0.8))
            .overlay(alignment: .bottom) {
                Rectangle().fill(AlexTheme.Colors.cardBorder).frame(height: 1)
            }
    }
}

#Preview("PanelHeader") {
    struct Host: View {
        @State private var query = ""
        var body: some View {
            VStack(spacing: 0) {
                PanelHeader {
                    Text("Sessions")
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(AlexTheme.Colors.foreground)
                    PanelCountBadge(count: 24)
                } right: {
                    PanelIconButton(systemImage: "ellipsis") {}
                }
                PanelHeader(accentLeft: true) {
                    Text("Trace View")
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(AlexTheme.Colors.foreground)
                } right: {
                    CopyButton(value: "sess_a1b2c3", label: "Copy ID")
                }
                FilterRow {
                    SearchField(text: $query, placeholder: "Filter messages…")
                }
            }
            .frame(width: 360)
            .background(AlexTheme.Colors.background)
        }
    }
    return Host()
}
