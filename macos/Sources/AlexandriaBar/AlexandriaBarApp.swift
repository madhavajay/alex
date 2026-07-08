import AppKit
import AlexandriaBarCore

@main
enum Main {
    @MainActor
    static func main() {
        let app = NSApplication.shared
        let delegate = AppDelegate()
        app.delegate = delegate
        app.run()
    }
}

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    private var store: SnapshotStore!
    private var statusController: StatusItemController!
    private var notifier: AlertNotifier!

    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.accessory)
        store = SnapshotStore()
        notifier = AlertNotifier()
        notifier.requestAuthorization()
        statusController = StatusItemController(store: store)
        store.onRefresh = { [weak self] in
            guard let self else { return }
            self.statusController.snapshotDidChange()
            self.notifier.sync(alerts: self.store.alerts)
        }
        store.onWindowReset = { [weak self] provider, window in
            self?.notifier.postInfo(
                title: "\(ProviderInfo.displayName(provider)) \(window) window reset",
                body: "A fresh \(window) rate-limit window is available.")
        }
        store.startPolling()
    }

    func applicationWillTerminate(_ notification: Notification) {
        store.stopPolling()
    }

    func postNotification(title: String, body: String) {
        notifier.postInfo(title: title, body: body)
    }
}
