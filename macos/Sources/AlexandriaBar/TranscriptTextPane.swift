import AppKit
import SwiftUI
import AlexandriaBarCore

final class TranscriptTextView: NSTextView {
    var onTurnClick: ((Int) -> Void)?

    override func mouseDown(with event: NSEvent) {
        let downPoint = convert(event.locationInWindow, from: nil)
        super.mouseDown(with: event)
        let upEvent = NSApp.currentEvent
        let upPoint = upEvent.map { convert($0.locationInWindow, from: nil) } ?? downPoint
        let moved = hypot(upPoint.x - downPoint.x, upPoint.y - downPoint.y)
        guard moved < 4, selectedRange().length == 0 else { return }
        let index = characterIndexForInsertion(at: upPoint)
        guard index >= 0, index < (textStorage?.length ?? 0) else { return }
        if textStorage?.attribute(.link, at: index, effectiveRange: nil) != nil { return }
        onTurnClick?(index)
    }
}

struct TranscriptTextPane: NSViewRepresentable {
    let model: TraceBrowserModel

    func makeNSView(context: Context) -> NSScrollView {
        let textView = TranscriptTextView(usingTextLayoutManager: true)
        textView.isEditable = false
        textView.isSelectable = true
        textView.isRichText = false
        textView.drawsBackground = false
        textView.delegate = context.coordinator
        textView.displaysLinkToolTips = false
        textView.linkTextAttributes = [.cursor: NSCursor.pointingHand]
        textView.usesFindBar = true
        textView.isIncrementalSearchingEnabled = true
        textView.textContainerInset = NSSize(width: 8, height: 12)
        textView.isVerticallyResizable = true
        textView.isHorizontallyResizable = false
        textView.autoresizingMask = [.width]
        textView.textContainer?.widthTracksTextView = true
        textView.textLayoutManager?.delegate = context.coordinator
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
    final class Coordinator: NSObject, NSTextViewDelegate, @preconcurrency NSTextLayoutManagerDelegate {
        func textView(_ textView: NSTextView, clickedOnLink link: Any, at charIndex: Int) -> Bool {
            let url = link as? URL ?? (link as? String).flatMap(URL.init(string:))
            guard let url, let traceId = TraceLink.traceId(from: url) else { return false }
            model?.openInspector(traceId: traceId)
            return true
        }

        private weak var scroll: NSScrollView?
        private weak var textView: TranscriptTextView?
        private weak var model: TraceBrowserModel?
        private var lastVersion = 0
        private var lastScrollCommand = 0
        private var lastFindCommand = 0
        private var lastScrollToRange = 0
        private var lastSelectedTurn: String?
        nonisolated(unsafe) private var observer: NSObjectProtocol?
        nonisolated(unsafe) private var keyMonitor: Any?

        func attach(scroll: NSScrollView, textView: TranscriptTextView) {
            self.scroll = scroll
            self.textView = textView
            textView.onTurnClick = { [weak self] index in
                guard let model = self?.model,
                    let traceId = TurnHitTest.traceId(at: index, in: model.turnRanges)
                else { return }
                model.openInspector(traceId: traceId)
            }
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

        // MARK: - Bubble layout fragments

        func textLayoutManager(
            _ textLayoutManager: NSTextLayoutManager,
            textLayoutFragmentFor location: NSTextLocation,
            in textElement: NSTextElement
        ) -> NSTextLayoutFragment {
            guard let paragraph = textElement as? NSTextParagraph else {
                return NSTextLayoutFragment(
                    textElement: textElement, range: textElement.elementRange)
            }
            let str = paragraph.attributedString
            guard str.length > 0,
                let turnId = str.attribute(.transcriptTurnId, at: 0, effectiveRange: nil)
                    as? String
            else {
                return NSTextLayoutFragment(
                    textElement: textElement, range: textElement.elementRange)
            }
            let fragment = BubbleLayoutFragment(
                textElement: textElement, range: textElement.elementRange)
            fragment.turnId = turnId
            fragment.selectedTurnProvider = { [weak self] in self?.model?.inspectorTraceId }
            if let kindRaw = str.attribute(.transcriptBubbleKind, at: 0, effectiveRange: nil)
                as? String,
                let kind = TranscriptBubbleKind(rawValue: kindRaw) {
                fragment.kind = kind
                if let para = str.attribute(.paragraphStyle, at: 0, effectiveRange: nil)
                    as? NSParagraphStyle {
                    fragment.leftInset = para.headIndent
                    fragment.rightInset = abs(para.tailIndent)
                    fragment.isRightAligned = para.alignment == .right
                }
                if let storage = textView?.textStorage,
                    let contentManager = textLayoutManager.textContentManager {
                    let offset = contentManager.offset(
                        from: contentManager.documentRange.location, to: location)
                    fragment.roundedTop = !neighborMatches(
                        storage, index: offset - 1, kind: kindRaw, turn: turnId)
                    fragment.roundedBottom = !neighborMatches(
                        storage, index: offset + str.length, kind: kindRaw, turn: turnId)
                }
            }
            return fragment
        }

        private func neighborMatches(
            _ storage: NSTextStorage, index: Int, kind: String, turn: String
        ) -> Bool {
            guard index >= 0, index < storage.length else { return false }
            let neighborKind = storage.attribute(
                .transcriptBubbleKind, at: index, effectiveRange: nil) as? String
            let neighborTurn = storage.attribute(
                .transcriptTurnId, at: index, effectiveRange: nil) as? String
            return neighborKind == kind && neighborTurn == turn
        }

        // MARK: - Find bar

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
            syncFindBarVisible(scroll.isFindBarVisible)
            let visible = scroll.contentView.bounds
            let atBottom = visible.maxY >= doc.frame.height - 24
            model?.setUserAtBottom(atBottom)
        }

        // MARK: - Model apply

        func apply(model: TraceBrowserModel) {
            self.model = model
            guard let textView, let storage = textView.textStorage else { return }
            syncFindBarVisible(findBarVisible)
            if let render = model.renderOp, render.version != lastVersion {
                lastVersion = render.version
                BarLog.measure(.browser, label: "transcript apply v\(render.version)") {
                    switch render.op {
                    case let .set(doc): storage.setAttributedString(doc)
                    case let .append(doc): storage.append(doc)
                    }
                }
                if model.userAtBottom, !findBarVisible { scrollToBottom() }
            }
            if lastSelectedTurn != model.inspectorTraceId {
                lastSelectedTurn = model.inspectorTraceId
                textView.needsDisplay = true
            }
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

        private func scrollToBottom() {
            textView?.scrollToEndOfDocument(nil)
        }
    }
}
