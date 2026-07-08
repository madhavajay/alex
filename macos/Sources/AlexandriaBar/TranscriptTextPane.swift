import AppKit
import SwiftUI
import AlexandriaBarCore

struct TranscriptTextPane: NSViewRepresentable {
    let model: TraceBrowserModel

    func makeNSView(context: Context) -> NSScrollView {
        let textView = NSTextView(usingTextLayoutManager: true)
        textView.isEditable = false
        textView.isSelectable = true
        textView.isRichText = false
        textView.drawsBackground = false
        textView.delegate = context.coordinator
        textView.displaysLinkToolTips = false
        textView.linkTextAttributes = [.cursor: NSCursor.pointingHand]
        textView.usesFindBar = true
        textView.isIncrementalSearchingEnabled = true
        textView.textContainerInset = NSSize(width: 8, height: 10)
        textView.isVerticallyResizable = true
        textView.isHorizontallyResizable = false
        textView.autoresizingMask = [.width]
        textView.textContainer?.widthTracksTextView = true
        let scroll = NSScrollView()
        scroll.documentView = textView
        scroll.hasVerticalScroller = true
        scroll.drawsBackground = false
        scroll.contentView.postsBoundsChangedNotifications = true
        context.coordinator.attach(scroll: scroll, textView: textView)
        return scroll
    }

    func updateNSView(_ scroll: NSScrollView, context: Context) {
        context.coordinator.apply(model: model)
    }

    func makeCoordinator() -> Coordinator { Coordinator() }

    @MainActor
    final class Coordinator: NSObject, NSTextViewDelegate {
        func textView(_ textView: NSTextView, clickedOnLink link: Any, at charIndex: Int) -> Bool {
            let url = link as? URL ?? (link as? String).flatMap(URL.init(string:))
            guard let url, let traceId = TraceLink.traceId(from: url) else { return false }
            model?.openInspector(traceId: traceId)
            return true
        }

        private weak var scroll: NSScrollView?
        private weak var textView: NSTextView?
        private weak var model: TraceBrowserModel?
        private var lastVersion = 0
        private var lastScrollCommand = 0
        private var lastFindCommand = 0
        private var lastScrollToRange = 0
        private var highlight: (range: NSRange, saved: NSAttributedString)?
        nonisolated(unsafe) private var observer: NSObjectProtocol?
        nonisolated(unsafe) private var keyMonitor: Any?

        func attach(scroll: NSScrollView, textView: NSTextView) {
            self.scroll = scroll
            self.textView = textView
            observer = NotificationCenter.default.addObserver(
                forName: NSView.boundsDidChangeNotification,
                object: scroll.contentView, queue: .main
            ) { [weak self] _ in
                MainActor.assumeIsolated { self?.boundsChanged() }
            }
            keyMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
                let handled = MainActor.assumeIsolated { self?.handleKey(event) == true }
                return handled ? nil : event
            }
        }

        deinit {
            if let observer { NotificationCenter.default.removeObserver(observer) }
            if let keyMonitor { NSEvent.removeMonitor(keyMonitor) }
        }

        private func handleKey(_ event: NSEvent) -> Bool {
            guard let textView, let window = textView.window, window.isKeyWindow,
                event.modifierFlags.intersection(.deviceIndependentFlagsMask) == .command,
                event.charactersIgnoringModifiers?.lowercased() == "f"
            else { return false }
            if let editor = window.firstResponder as? NSTextView, editor.isFieldEditor {
                return false
            }
            showFindBar()
            return true
        }

        private func showFindBar() {
            guard let textView else { return }
            textView.window?.makeFirstResponder(textView)
            let item = NSMenuItem()
            item.tag = NSTextFinder.Action.showFindInterface.rawValue
            textView.performTextFinderAction(item)
            syncFindBarVisible(true)
        }

        private func syncFindBarVisible(_ visible: Bool) {
            guard let model, model.findBarVisible != visible else { return }
            DispatchQueue.main.async { [weak model] in
                model?.setFindBarVisible(visible)
            }
        }

        private var findBarVisible: Bool { scroll?.isFindBarVisible ?? false }

        private func boundsChanged() {
            guard let scroll, let doc = scroll.documentView else { return }
            model?.setFindBarVisible(scroll.isFindBarVisible)
            let visible = scroll.contentView.bounds
            let atBottom = visible.maxY >= doc.frame.height - 24
            model?.setUserAtBottom(atBottom)
        }

        func apply(model: TraceBrowserModel) {
            self.model = model
            guard let textView, let storage = textView.textStorage else { return }
            syncFindBarVisible(findBarVisible)
            if let render = model.renderOp, render.version != lastVersion {
                lastVersion = render.version
                BarLog.measure(.browser, label: "transcript apply v\(render.version)") {
                    switch render.op {
                    case let .set(doc):
                        highlight = nil
                        storage.setAttributedString(doc)
                    case let .append(doc):
                        storage.append(doc)
                    }
                }
                if model.userAtBottom, !findBarVisible { scrollToBottom() }
            }
            applyHighlight(model.inspectorHighlightRange, storage: storage)
            if model.scrollCommand != lastScrollCommand {
                lastScrollCommand = model.scrollCommand
                scrollToBottom()
            }
            if let command = model.scrollToRangeCommand, command.version != lastScrollToRange {
                lastScrollToRange = command.version
                if command.range.upperBound <= storage.length {
                    textView.scrollRangeToVisible(command.range)
                }
            }
            if model.findCommand != lastFindCommand {
                lastFindCommand = model.findCommand
                showFindBar()
            }
        }

        private func applyHighlight(_ range: NSRange?, storage: NSTextStorage) {
            guard highlight?.range != range else { return }
            if let current = highlight {
                if current.range.upperBound <= storage.length {
                    storage.replaceCharacters(in: current.range, with: current.saved)
                }
                highlight = nil
            }
            guard let range, range.length > 0, range.upperBound <= storage.length else { return }
            let saved = storage.attributedSubstring(from: range)
            storage.addAttribute(
                .backgroundColor,
                value: NSColor.controlAccentColor.withAlphaComponent(0.14),
                range: range)
            highlight = (range, saved)
        }

        private func scrollToBottom() {
            textView?.scrollToEndOfDocument(nil)
        }
    }
}
