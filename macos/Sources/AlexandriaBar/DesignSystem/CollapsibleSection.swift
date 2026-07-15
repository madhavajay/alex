import SwiftUI

/// Flat collapsible section for inspector panes (shared.tsx:312-337): 1px top
/// hairline, header padding 8×12 with 12px chevron, 11px medium muted title,
/// optional mono "(N)" count, hover wash overlay(0.03). Unlike
/// `CollapsibleCard`, this is border-separated, not a card.
struct CollapsibleSection<Content: View>: View {
    let title: String
    var badge: String?
    @State private var internalIsOpen: Bool
    private let externalIsOpen: Binding<Bool>?
    private let content: Content
    @State private var hovering = false

    init(
        title: String,
        badge: String? = nil,
        defaultOpen: Bool = false,
        @ViewBuilder content: () -> Content
    ) {
        self.title = title
        self.badge = badge
        _internalIsOpen = State(initialValue: defaultOpen)
        self.externalIsOpen = nil
        self.content = content()
    }

    init(
        title: String,
        count: Int,
        defaultOpen: Bool = false,
        @ViewBuilder content: () -> Content
    ) {
        self.init(
            title: title, badge: "(\(count))", defaultOpen: defaultOpen,
            content: content)
    }

    /// Externally-owned open state (e.g. expand/collapse-all controls).
    init(
        title: String,
        badge: String? = nil,
        isOpen: Binding<Bool>,
        @ViewBuilder content: () -> Content
    ) {
        self.title = title
        self.badge = badge
        _internalIsOpen = State(initialValue: isOpen.wrappedValue)
        self.externalIsOpen = isOpen
        self.content = content()
    }

    private var isOpen: Bool {
        externalIsOpen?.wrappedValue ?? internalIsOpen
    }

    private func toggle() {
        withAnimation(.easeInOut(duration: 0.15)) {
            if let externalIsOpen {
                externalIsOpen.wrappedValue.toggle()
            } else {
                internalIsOpen.toggle()
            }
        }
    }

    var body: some View {
        VStack(spacing: 0) {
            Button {
                toggle()
            } label: {
                HStack(spacing: AlexTheme.Spacing.md) {
                    Image(systemName: "chevron.right")
                        .font(.system(size: 10, weight: .medium))
                        .foregroundStyle(AlexTheme.Colors.textTertiary)
                        .rotationEffect(.degrees(isOpen ? 90 : 0))
                    Text(title)
                        .font(.system(size: 11, weight: .medium))
                        .foregroundStyle(AlexTheme.Colors.mutedForeground)
                    if let badge {
                        Text(badge)
                            .font(AlexTheme.Fonts.mono(10))
                            .foregroundStyle(AlexTheme.Colors.textTertiary)
                    }
                    Spacer(minLength: 0)
                }
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
                .background(hovering ? AlexTheme.Colors.overlay(0.03) : Color.clear)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .onHover { hovering = $0 }
            if isOpen {
                content
            }
        }
        .overlay(alignment: .top) {
            Rectangle().fill(AlexTheme.Colors.cardBorder).frame(height: 1)
        }
    }
}

#if DEBUG
#Preview("CollapsibleSection") {
    struct Host: View {
        @State private var bodyOpen = true
        var body: some View {
            VStack(spacing: 0) {
                CollapsibleSection(title: "Request Headers", count: 8) {
                    Text("content-type: application/json")
                        .font(AlexTheme.Fonts.metaMono)
                        .foregroundStyle(AlexTheme.Colors.textSecondary)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.horizontal, 12)
                        .padding(.bottom, 8)
                }
                CollapsibleSection(title: "Request Body", isOpen: $bodyOpen) {
                    JsonBlock(
                        content: #"{"model": "claude-opus-4-8", "stream": true}"#,
                        maxHeight: 120)
                }
                Toggle("bound open state", isOn: $bodyOpen)
                    .font(.system(size: 10))
                    .padding(12)
            }
            .frame(width: 308)
            .background(AlexTheme.Colors.background)
        }
    }
    return Host()
}
#endif
