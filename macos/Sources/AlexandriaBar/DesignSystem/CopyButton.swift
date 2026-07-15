import AppKit
import SwiftUI

/// Copy-to-pasteboard button with a 1.5s "Copied" confirmation
/// (shared.tsx:257-274). `value` may be provided lazily so expensive strings
/// are built only on click.
struct CopyButton: View {
    enum Style {
        /// 10.5px ghost label, hover wash (the original look).
        case ghost
        /// 12px label on a tinted fill (defaults to the accent blue).
        case tinted
    }

    let value: () -> String
    var label: String?
    var style: Style
    var copiedLabel: String
    /// Fixed square side for an icon-only hit target (e.g. 44 → 44×44);
    /// ignores `label`.
    var square: CGFloat?
    /// Replaces the idle foreground (and the `.tinted` fill).
    var tint: Color?
    @State private var copied = false
    @State private var hovering = false
    @State private var revertTask: Task<Void, Never>?

    init(
        value: @escaping @autoclosure () -> String,
        label: String? = nil,
        style: Style = .ghost,
        copiedLabel: String = "Copied",
        square: CGFloat? = nil,
        tint: Color? = nil
    ) {
        self.value = value
        self.label = label
        self.style = style
        self.copiedLabel = copiedLabel
        self.square = square
        self.tint = tint
    }

    var body: some View {
        Button(action: copy) {
            content
                .foregroundStyle(copied ? AlexTheme.Colors.success : idleColor)
                .background(
                    RoundedRectangle(cornerRadius: AlexTheme.Radius.md)
                        .fill(fillColor))
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
    }

    @ViewBuilder
    private var content: some View {
        if let square {
            Image(systemName: copied ? "checkmark" : "doc.on.doc")
                .font(.system(size: max(11, square * 0.3)))
                .frame(width: square, height: square)
        } else {
            HStack(spacing: AlexTheme.Spacing.sm) {
                Image(systemName: copied ? "checkmark" : "doc.on.doc")
                    .font(.system(size: 11))
                if let label {
                    Text(copied ? copiedLabel : label)
                        .font(.system(size: labelSize, weight: .medium))
                }
            }
            .padding(.horizontal, AlexTheme.Spacing.md)
            .padding(.vertical, AlexTheme.Spacing.xs)
        }
    }

    private var labelSize: CGFloat {
        style == .tinted ? 12 : 10.5
    }

    private var idleColor: Color {
        tint ?? (style == .tinted ? AlexTheme.Colors.primary : AlexTheme.Colors.textTertiary)
    }

    private var fillColor: Color {
        switch style {
        case .ghost:
            hovering ? AlexTheme.Colors.surfaceHover : Color.clear
        case .tinted:
            (copied ? AlexTheme.Colors.success : idleColor)
                .opacity(hovering ? 0.25 : 0.15)
        }
    }

    private func copy() {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(value(), forType: .string)
        copied = true
        revertTask?.cancel()
        revertTask = Task {
            try? await Task.sleep(nanoseconds: 1_500_000_000)
            guard !Task.isCancelled else { return }
            copied = false
        }
    }
}

#if DEBUG
#Preview("CopyButton") {
    VStack(alignment: .leading, spacing: AlexTheme.Spacing.md) {
        HStack(spacing: AlexTheme.Spacing.md) {
            CopyButton(value: "sess_a1b2c3", label: "Copy ID")
            CopyButton(value: "https://example.com/auth", label: "Copy Link")
            CopyButton(value: "icon-only")
        }
        HStack(spacing: AlexTheme.Spacing.md) {
            CopyButton(value: "tinted", label: "Copy code", style: .tinted)
            CopyButton(
                value: "custom copied", label: "Copy command",
                style: .tinted, copiedLabel: "Command copied",
                tint: AlexTheme.Colors.success)
            CopyButton(value: "square", square: 44)
        }
    }
    .padding()
    .background(AlexTheme.Colors.background)
}
#endif
