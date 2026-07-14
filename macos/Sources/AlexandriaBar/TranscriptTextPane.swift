import AppKit
import SwiftUI
import AlexandriaBarCore

final class TranscriptTextView: NSTextView {
    var onTurnClick: ((Int) -> Void)?
    var selectedTurnProvider: (() -> String?)?

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

    // Draw one unified rounded bubble per message group, beneath text and
    // beneath the native selection band (super.drawBackground draws selection).
    override func drawBackground(in rect: NSRect) {
        drawBubbles(in: rect)
        super.drawBackground(in: rect)
    }

    private func drawBubbles(in dirtyRect: NSRect) {
        guard let layoutManager = textLayoutManager,
            let contentManager = layoutManager.textContentManager,
            let storage = textStorage, storage.length > 0,
            let context = NSGraphicsContext.current?.cgContext
        else { return }
        let viewport = layoutManager.textViewportLayoutController.viewportRange
            ?? layoutManager.documentRange
        let start = contentManager.offset(
            from: contentManager.documentRange.location, to: viewport.location)
        let end = contentManager.offset(
            from: contentManager.documentRange.location, to: viewport.endLocation)
        guard start >= 0, end > start, end <= storage.length else { return }
        let origin = textContainerOrigin
        let selectedTurn = selectedTurnProvider?()
        var drawnGroups = Set<String>()
        storage.enumerateAttribute(
            .transcriptBubbleGroup, in: NSRange(location: start, length: end - start)
        ) { value, partialRange, _ in
            guard let group = value as? String, drawnGroups.insert(group).inserted else {
                return
            }
            var groupRange = NSRange()
            guard storage.attribute(
                .transcriptBubbleGroup, at: partialRange.location,
                longestEffectiveRange: &groupRange,
                in: NSRange(location: 0, length: storage.length)) != nil
            else { return }
            guard let kindRaw = storage.attribute(
                .transcriptBubbleKind, at: groupRange.location, effectiveRange: nil) as? String,
                let kind = TranscriptBubbleKind(rawValue: kindRaw)
            else { return }
            let para = storage.attribute(
                .paragraphStyle, at: groupRange.location, effectiveRange: nil)
                as? NSParagraphStyle
            guard let bubble = unifiedRect(
                for: groupRange, kind: kind, paragraphStyle: para,
                layoutManager: layoutManager, contentManager: contentManager)
            else { return }
            let deviceRect = bubble.offsetBy(dx: origin.x, dy: origin.y)
            guard deviceRect.intersects(dirtyRect.insetBy(dx: -8, dy: -8)) else { return }
            let selected = selectedTurn.map { group.hasPrefix($0 + "#") } ?? false
            BubbleStyle.draw(kind: kind, rect: deviceRect, selected: selected, in: context)
        }
    }

    private func unifiedRect(
        for range: NSRange, kind: TranscriptBubbleKind, paragraphStyle: NSParagraphStyle?,
        layoutManager: NSTextLayoutManager, contentManager: NSTextContentManager
    ) -> CGRect? {
        guard let startLocation = contentManager.location(
            contentManager.documentRange.location, offsetBy: range.location),
            let endLocation = contentManager.location(startLocation, offsetBy: range.length),
            let textRange = NSTextRange(location: startLocation, end: endLocation)
        else { return nil }
        layoutManager.ensureLayout(for: textRange)
        var top: CGFloat?
        var bottom: CGFloat = 0
        var maxLineX: CGFloat = 0
        var containerWidth: CGFloat = 0
        layoutManager.enumerateTextLayoutFragments(
            from: textRange.location, options: []
        ) { fragment in
            guard fragment.rangeInElement.location.compare(textRange.endLocation)
                == .orderedAscending
            else { return false }
            let frame = fragment.layoutFragmentFrame
            containerWidth = max(containerWidth, frame.width)
            for line in fragment.textLineFragments {
                let bounds = line.typographicBounds
                if top == nil { top = frame.minY + bounds.minY }
                bottom = frame.minY + bounds.maxY
                maxLineX = max(maxLineX, frame.minX + bounds.maxX)
            }
            return true
        }
        guard let top, containerWidth > 0 else { return nil }
        let head = paragraphStyle?.headIndent ?? 8
        let tail = abs(paragraphStyle?.tailIndent ?? -8)
        let x0 = max(0, head - BubbleStyle.padX)
        var x1 = containerWidth - max(0, tail - BubbleStyle.padX)
        if kind == .user {
            x1 = min(x1, max(maxLineX + BubbleStyle.padX, x0 + 60))
        }
        return CGRect(
            x: x0, y: top - BubbleStyle.padY,
            width: max(0, x1 - x0), height: bottom - top + BubbleStyle.padY * 2)
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
        textView.selectedTextAttributes = [
            .backgroundColor: NSColor.selectedTextBackgroundColor
        ]
        textView.usesFindBar = true
        textView.isIncrementalSearchingEnabled = true
        textView.textContainerInset = NSSize(width: 8, height: 12)
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
            guard let url else { return false }
            if let traceId = TraceLink.traceId(from: url) { model?.openInspector(traceId: traceId); return true }
            if let target = ToolLink.target(from: url) { model?.openToolBody(id: target.id, kind: target.kind); return true }
            return false
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
            textView.selectedTurnProvider = { [weak self] in self?.model?.inspectorTraceId }
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
