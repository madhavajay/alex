import AppKit

@MainActor
final class DockIconManager {
    static let shared = DockIconManager()

    private var tracked: [ObjectIdentifier: NSWindow] = [:]
    private var observers: [ObjectIdentifier: NSObjectProtocol] = [:]

    func track(_ window: NSWindow) {
        let key = ObjectIdentifier(window)
        if tracked[key] == nil {
            tracked[key] = window
            observers[key] = NotificationCenter.default.addObserver(
                forName: NSWindow.willCloseNotification, object: window, queue: .main
            ) { [weak self] _ in
                MainActor.assumeIsolated {
                    self?.untrack(key)
                }
            }
        }
        NSApp.setActivationPolicy(.regular)
        NSApp.activate(ignoringOtherApps: true)
    }

    private func untrack(_ key: ObjectIdentifier) {
        if let observer = observers.removeValue(forKey: key) {
            NotificationCenter.default.removeObserver(observer)
        }
        tracked.removeValue(forKey: key)
        if tracked.values.allSatisfy({ !$0.isVisible }) {
            tracked.removeAll()
            NSApp.setActivationPolicy(.accessory)
        }
    }

    func bringAllToFront() {
        NSApp.activate(ignoringOtherApps: true)
        for window in tracked.values where window.isVisible {
            window.makeKeyAndOrderFront(nil)
        }
    }

    var hasWindows: Bool { tracked.values.contains { $0.isVisible } }
}
