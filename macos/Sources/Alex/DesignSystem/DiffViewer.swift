import SwiftUI

/// One line of a config diff.
struct DiffLineDisplay: Equatable, Sendable {
    enum Kind: Equatable, Sendable {
        case added
        case removed
        case context
    }

    let kind: Kind
    let content: String

    init(_ kind: Kind, _ content: String) {
        self.kind = kind
        self.content = content
    }
}

/// One changed file in the diff viewer's sidebar.
struct DiffFileDisplay: Identifiable, Equatable, Sendable {
    let path: String
    var added: Int
    var removed: Int
    var lines: [DiffLineDisplay]

    var id: String { path }

    var fileName: String {
        (path as NSString).lastPathComponent
    }
}

/// Config-changes diff viewer (Create Settings ChangesModal,
/// App.tsx:362-464): header with title + total ±counts, 180pt file-list
/// sidebar, path bar, and +/− gutter diff lines. Presentation (sheet/scrim)
/// is left to the caller; the content is sized 580×420 by default.
struct DiffViewer<Icon: View>: View {
    let title: String
    var subtitle = "config changes"
    let files: [DiffFileDisplay]
    var onClose: (() -> Void)?
    private let icon: Icon
    @State private var selectedIndex = 0

    init(
        title: String,
        subtitle: String = "config changes",
        files: [DiffFileDisplay],
        onClose: (() -> Void)? = nil,
        @ViewBuilder icon: () -> Icon = { EmptyView() }
    ) {
        self.title = title
        self.subtitle = subtitle
        self.files = files
        self.onClose = onClose
        self.icon = icon()
    }

    private var selectedFile: DiffFileDisplay? {
        files.indices.contains(selectedIndex) ? files[selectedIndex] : nil
    }

    var body: some View {
        VStack(spacing: 0) {
            header
            HStack(spacing: 0) {
                sidebar
                Rectangle()
                    .fill(AlexTheme.Colors.overlay(0.06))
                    .frame(width: 1)
                diffPane
            }
        }
        .frame(width: 580, height: 420)
        .background(
            RoundedRectangle(cornerRadius: AlexTheme.Radius.xl)
                .fill(AlexTheme.Colors.dynamic(light: 0xFFFFFF, dark: 0x222224)))
        .overlay(
            RoundedRectangle(cornerRadius: AlexTheme.Radius.xl)
                .strokeBorder(AlexTheme.Colors.overlay(0.1), lineWidth: 0.5))
        .clipShape(RoundedRectangle(cornerRadius: AlexTheme.Radius.xl))
        .shadow(color: .black.opacity(0.8), radius: 32, y: 24)
    }

    private var totalAdded: Int { files.reduce(0) { $0 + $1.added } }
    private var totalRemoved: Int { files.reduce(0) { $0 + $1.removed } }

    private var header: some View {
        HStack(spacing: 10) {
            icon
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text(title)
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(AlexTheme.Colors.foreground)
                Text(subtitle)
                    .font(.system(size: 11))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            HStack(spacing: 8) {
                if totalAdded > 0 {
                    Text("+\(totalAdded)")
                        .font(AlexTheme.Fonts.mono(11))
                        .foregroundStyle(AlexTheme.Colors.success)
                }
                if totalRemoved > 0 {
                    Text("−\(totalRemoved)")
                        .font(AlexTheme.Fonts.mono(11))
                        .foregroundStyle(AlexTheme.Colors.destructive)
                }
            }
            .padding(.trailing, 8)
            if let onClose {
                Button(action: onClose) {
                    Image(systemName: "xmark")
                        .font(.system(size: 12, weight: .medium))
                        .foregroundStyle(AlexTheme.Colors.textFaint)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .help("Close")
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 12)
        .overlay(alignment: .bottom) {
            Rectangle().fill(AlexTheme.Colors.cardBorder).frame(height: 1)
        }
    }

    private var sidebar: some View {
        ScrollView {
            VStack(spacing: 0) {
                ForEach(Array(files.enumerated()), id: \.element.id) { index, file in
                    fileRow(file, index: index)
                }
            }
            .padding(.vertical, 6)
        }
        .frame(width: 180)
    }

    private func fileRow(_ file: DiffFileDisplay, index: Int) -> some View {
        Button {
            selectedIndex = index
        } label: {
            HStack(alignment: .top, spacing: 7) {
                Image(systemName: "doc.text")
                    .font(.system(size: 11))
                    .foregroundStyle(AlexTheme.Colors.textFaint)
                    .padding(.top, 1)
                VStack(alignment: .leading, spacing: 2) {
                    Text(file.fileName)
                        .font(AlexTheme.Fonts.mono(11))
                        .foregroundStyle(AlexTheme.Colors.foreground)
                        .lineLimit(1)
                        .truncationMode(.tail)
                    HStack(spacing: 4) {
                        if file.added > 0 {
                            Text("+\(file.added)")
                                .font(.system(size: 9, weight: .semibold))
                                .foregroundStyle(AlexTheme.Colors.success)
                        }
                        if file.removed > 0 {
                            Text("−\(file.removed)")
                                .font(.system(size: 9, weight: .semibold))
                                .foregroundStyle(AlexTheme.Colors.destructive)
                        }
                        if file.added == 0, file.removed == 0 {
                            Text("no changes")
                                .font(.system(size: 9))
                                .foregroundStyle(AlexTheme.Colors.textFaint)
                        }
                    }
                }
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 7)
            .background(
                selectedIndex == index ? AlexTheme.Colors.overlay(0.07) : Color.clear)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    private var diffPane: some View {
        VStack(spacing: 0) {
            HStack {
                Text(selectedFile?.path ?? "")
                    .font(AlexTheme.Fonts.mono(10))
                    .foregroundStyle(AlexTheme.Colors.textFaint)
                    .lineLimit(1)
                    .truncationMode(.middle)
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 8)
            .overlay(alignment: .bottom) {
                Rectangle().fill(AlexTheme.Colors.hairline).frame(height: 1)
            }
            ScrollView([.vertical, .horizontal]) {
                VStack(alignment: .leading, spacing: 0) {
                    ForEach(
                        Array((selectedFile?.lines ?? []).enumerated()), id: \.offset
                    ) { _, line in
                        diffLine(line)
                    }
                }
                .padding(.vertical, 6)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
    }

    private func diffLine(_ line: DiffLineDisplay) -> some View {
        HStack(alignment: .top, spacing: 0) {
            Text(gutter(for: line.kind))
                .font(AlexTheme.Fonts.mono(11))
                .foregroundStyle(gutterColor(for: line.kind))
                .frame(width: 14, alignment: .leading)
            Text(line.content.isEmpty ? " " : line.content)
                .font(AlexTheme.Fonts.mono(11))
                .foregroundStyle(textColor(for: line.kind))
                .lineLimit(1)
                .fixedSize(horizontal: true, vertical: false)
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 1)
        .frame(minHeight: 20, alignment: .leading)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(lineBackground(for: line.kind))
    }

    private func gutter(for kind: DiffLineDisplay.Kind) -> String {
        switch kind {
        case .added: "+"
        case .removed: "−"
        case .context: " "
        }
    }

    private func gutterColor(for kind: DiffLineDisplay.Kind) -> Color {
        switch kind {
        case .added: AlexTheme.Colors.success
        case .removed: AlexTheme.Colors.destructive
        case .context: AlexTheme.Colors.textFaintest
        }
    }

    private func textColor(for kind: DiffLineDisplay.Kind) -> Color {
        switch kind {
        case .added: AlexTheme.Colors.success
        case .removed: AlexTheme.Colors.destructive
        case .context: AlexTheme.Colors.mutedForeground
        }
    }

    private func lineBackground(for kind: DiffLineDisplay.Kind) -> Color {
        switch kind {
        case .added: AlexTheme.Colors.success.opacity(0.08)
        case .removed: AlexTheme.Colors.destructive.opacity(0.08)
        case .context: .clear
        }
    }
}

#if DEBUG
#Preview("DiffViewer") {
    DiffViewer(
        title: "Claude Code",
        files: [
            DiffFileDisplay(
                path: "~/.claude/settings.json", added: 3, removed: 1,
                lines: [
                    DiffLineDisplay(.context, "  \"model\": \"claude-sonnet-4-5\","),
                    DiffLineDisplay(.context, "  \"theme\": \"dark\","),
                    DiffLineDisplay(.removed, "  \"mcp_servers\": {}"),
                    DiffLineDisplay(.added, "  \"mcp_servers\": {"),
                    DiffLineDisplay(
                        .added,
                        "    \"alex\": { \"command\": \"alex-mcp\" }"),
                    DiffLineDisplay(.added, "  }"),
                ]),
            DiffFileDisplay(
                path: "~/.claude/CLAUDE.md", added: 2, removed: 0,
                lines: [
                    DiffLineDisplay(.context, "# Project context"),
                    DiffLineDisplay(.added, ""),
                    DiffLineDisplay(.added, "## Alex"),
                ]),
            DiffFileDisplay(
                path: "~/.claude/keybindings.json", added: 0, removed: 0,
                lines: [DiffLineDisplay(.context, "// No changes recorded")]),
        ],
        onClose: {}
    ) {
        RoundedRectangle(cornerRadius: 6)
            .fill(AlexTheme.Colors.surfaceHover)
            .frame(width: 24, height: 24)
    }
    .padding(40)
    .background(Color.black.opacity(0.6))
}
#endif
