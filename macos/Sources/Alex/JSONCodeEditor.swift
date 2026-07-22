import AppKit
import SwiftUI
import AlexCore

/// Editable JSON text view used by the middleware Code pane. It reapplies
/// syntax colors as the user types while preserving the selection and scroll
/// position. Command-Shift-F formats valid JSON without taking focus away.
struct JSONCodeEditor: NSViewRepresentable {
    @Binding var text: String
    let onFormatRequest: () -> Void

    func makeCoordinator() -> Coordinator {
        Coordinator(text: $text, onFormatRequest: onFormatRequest)
    }

    func makeNSView(context: Context) -> NSScrollView {
        let textView = JSONFormattingTextView(usingTextLayoutManager: true)
        textView.delegate = context.coordinator
        textView.onFormatRequest = context.coordinator.onFormatRequest
        textView.isEditable = true
        textView.isSelectable = true
        textView.isRichText = false
        textView.importsGraphics = false
        textView.allowsUndo = true
        textView.usesFindBar = true
        textView.isIncrementalSearchingEnabled = true
        textView.drawsBackground = false
        textView.font = Self.font
        textView.textContainerInset = NSSize(width: 8, height: 8)
        textView.isVerticallyResizable = true
        textView.isHorizontallyResizable = false
        textView.autoresizingMask = [.width]
        textView.textContainer?.widthTracksTextView = true

        let scrollView = NSScrollView()
        scrollView.documentView = textView
        scrollView.hasVerticalScroller = true
        scrollView.autohidesScrollers = true
        scrollView.drawsBackground = false
        scrollView.borderType = .noBorder

        context.coordinator.apply(text, to: textView, replacingText: true)
        return scrollView
    }

    func updateNSView(_ scrollView: NSScrollView, context: Context) {
        context.coordinator.text = $text
        context.coordinator.onFormatRequest = onFormatRequest
        guard let textView = scrollView.documentView as? JSONFormattingTextView else { return }
        textView.onFormatRequest = onFormatRequest
        context.coordinator.apply(text, to: textView, replacingText: textView.string != text)
    }

    @MainActor
    final class Coordinator: NSObject, NSTextViewDelegate {
        var text: Binding<String>
        var onFormatRequest: () -> Void
        private var isApplying = false

        init(text: Binding<String>, onFormatRequest: @escaping () -> Void) {
            self.text = text
            self.onFormatRequest = onFormatRequest
        }

        func textDidChange(_ notification: Notification) {
            guard !isApplying, let textView = notification.object as? NSTextView else { return }
            text.wrappedValue = textView.string
            apply(textView.string, to: textView, replacingText: false)
        }

        func textDidEndEditing(_ notification: Notification) {
            onFormatRequest()
        }

        func apply(_ source: String, to textView: NSTextView, replacingText: Bool) {
            guard let storage = textView.textStorage else { return }
            let selection = textView.selectedRange()
            let visibleOrigin = textView.enclosingScrollView?.contentView.bounds.origin

            isApplying = true
            storage.beginEditing()
            if replacingText {
                storage.replaceCharacters(in: NSRange(location: 0, length: storage.length), with: source)
            }
            let fullRange = NSRange(location: 0, length: storage.length)
            storage.setAttributes([
                .font: JSONCodeEditor.font,
                .foregroundColor: JSONCodeEditor.colors.punctuation
            ], range: fullRange)
            for (range, kind) in JsonHighlight.spans(storage.string) {
                storage.addAttribute(
                    .foregroundColor,
                    value: JSONCodeEditor.color(for: kind),
                    range: NSRange(location: range.lowerBound, length: range.count))
            }
            storage.endEditing()
            isApplying = false

            let safeLocation = min(selection.location, storage.length)
            let safeLength = min(selection.length, storage.length - safeLocation)
            textView.setSelectedRange(NSRange(location: safeLocation, length: safeLength))
            if let visibleOrigin, let scrollView = textView.enclosingScrollView {
                scrollView.contentView.scroll(to: visibleOrigin)
                scrollView.reflectScrolledClipView(scrollView.contentView)
            }
        }
    }

    private static let font = NSFont.monospacedSystemFont(ofSize: 11, weight: .regular)

    private nonisolated static let colors = JsonHighlight.Colors(
        key: dynamicColor(light: 0x33708E, dark: 0x79B8D4),
        string: dynamicColor(light: 0x4A7A3E, dark: 0x87BD78),
        number: dynamicColor(light: 0x9C5A28, dark: 0xD49668),
        keyword: dynamicColor(light: 0x7C4FA8, dark: 0xB48ADE),
        punctuation: dynamicColor(light: 0x6E6E78, dark: 0xA8A8B2))

    private nonisolated static func dynamicColor(light: UInt32, dark: UInt32) -> NSColor {
        NSColor(name: nil) { appearance in
            let hex = appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua ? dark : light
            return NSColor(
                srgbRed: CGFloat((hex >> 16) & 0xFF) / 255,
                green: CGFloat((hex >> 8) & 0xFF) / 255,
                blue: CGFloat(hex & 0xFF) / 255,
                alpha: 1)
        }
    }

    private static func color(for kind: JsonHighlight.Kind) -> NSColor {
        switch kind {
        case .key: colors.key
        case .string: colors.string
        case .number: colors.number
        case .keyword: colors.keyword
        }
    }
}

private final class JSONFormattingTextView: NSTextView {
    var onFormatRequest: (() -> Void)?

    override func paste(_ sender: Any?) {
        guard let pasted = NSPasteboard.general.string(forType: .string) else {
            super.paste(sender)
            return
        }
        insertText(JSONTextFormatting.prettyPrinted(pasted) ?? pasted, replacementRange: selectedRange())
    }

    override func keyDown(with event: NSEvent) {
        let modifiers = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
        if modifiers == [.command, .shift], event.charactersIgnoringModifiers?.lowercased() == "f" {
            onFormatRequest?()
            return
        }
        super.keyDown(with: event)
    }
}
