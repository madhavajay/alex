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
    final class Coordinator: NSObject {
        private weak var scroll: NSScrollView?
        private weak var textView: NSTextView?
        private weak var model: TraceBrowserModel?
        private var lastVersion = 0
        private var lastScrollCommand = 0
        nonisolated(unsafe) private var observer: NSObjectProtocol?

        func attach(scroll: NSScrollView, textView: NSTextView) {
            self.scroll = scroll
            self.textView = textView
            observer = NotificationCenter.default.addObserver(
                forName: NSView.boundsDidChangeNotification,
                object: scroll.contentView, queue: .main
            ) { [weak self] _ in
                MainActor.assumeIsolated { self?.boundsChanged() }
            }
        }

        deinit {
            if let observer { NotificationCenter.default.removeObserver(observer) }
        }

        private func boundsChanged() {
            guard let scroll, let doc = scroll.documentView else { return }
            let visible = scroll.contentView.bounds
            let atBottom = visible.maxY >= doc.frame.height - 24
            model?.setUserAtBottom(atBottom)
        }

        func apply(model: TraceBrowserModel) {
            self.model = model
            guard let textView, let storage = textView.textStorage else { return }
            if let render = model.renderOp, render.version != lastVersion {
                lastVersion = render.version
                BarLog.measure(.browser, label: "transcript apply v\(render.version)") {
                    switch render.op {
                    case let .set(doc): storage.setAttributedString(doc)
                    case let .append(doc): storage.append(doc)
                    }
                }
                if model.userAtBottom { scrollToBottom() }
            }
            if model.scrollCommand != lastScrollCommand {
                lastScrollCommand = model.scrollCommand
                scrollToBottom()
            }
        }

        private func scrollToBottom() {
            textView?.scrollToEndOfDocument(nil)
        }
    }
}
