import AppKit
import SwiftUI
import AlexCore

@MainActor
@Observable
private final class MiddlewareWizardWindowModel {
    var draft: MiddlewareWizardDraft
    var editingRuleID: String?

    init(draft: MiddlewareWizardDraft, editingRuleID: String?) {
        self.draft = draft
        self.editingRuleID = editingRuleID
    }
}

/// Owns the movable Middleware Wizard window independently of Settings.
/// Reopening while it is visible replaces the draft in the existing window.
@MainActor
final class MiddlewareWizardWindowController: NSObject, NSWindowDelegate {
    private var window: NSWindow?
    private var model: MiddlewareWizardWindowModel?

    func show(
        store: SnapshotStore,
        draft: MiddlewareWizardDraft,
        editingRuleID: String?,
        onSaved: @escaping () -> Void,
        onOpenTraceBrowser: @escaping (String) -> Void
    ) {
        window?.close()

        let model = MiddlewareWizardWindowModel(
            draft: draft, editingRuleID: editingRuleID)
        self.model = model

        let root = MiddlewareWizardWindowRoot(
            store: store,
            model: model,
            onSaved: { [weak self] in
                onSaved()
                self?.close()
            },
            onOpenTraceBrowser: onOpenTraceBrowser,
            onClose: { [weak self] in self?.close() })
        let window = NSWindow(contentViewController: NSHostingController(rootView: root))
        window.title = editingRuleID == nil
            ? "Alex UI — New Middleware" : "Alex UI — Edit Middleware"
        window.styleMask = [.titled, .closable, .miniaturizable, .resizable]
        window.isReleasedWhenClosed = false
        window.delegate = self
        window.minSize = NSSize(width: 900, height: 560)
        window.setContentSize(NSSize(width: 1_100, height: 680))
        window.center()
        window.setFrameAutosaveName("AlexMiddlewareWizard")
        self.window = window

        DockIconManager.shared.track(window)
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
    }

    private func close() {
        window?.close()
    }

    func windowWillClose(_ notification: Notification) {
        window = nil
        model = nil
    }
}

private struct MiddlewareWizardWindowRoot: View {
    let store: SnapshotStore
    @Bindable var model: MiddlewareWizardWindowModel
    let onSaved: () -> Void
    let onOpenTraceBrowser: (String) -> Void
    let onClose: () -> Void

    var body: some View {
        MiddlewareWizard(
            store: store,
            draft: $model.draft,
            editingRuleID: $model.editingRuleID,
            onSaved: onSaved,
            onOpenTraceBrowser: onOpenTraceBrowser,
            onClose: onClose)
    }
}
