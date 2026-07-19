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
        installMainMenu()
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
        statusController.showOnboardingIfNeeded()
    }

    func applicationWillTerminate(_ notification: Notification) {
        store.stopPolling()
    }

    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows: Bool) -> Bool {
        DockIconManager.shared.bringAllToFront()
        return false
    }

    private func installMainMenu() {
        let mainMenu = NSMenu()

        let appItem = NSMenuItem()
        let appMenu = NSMenu()
        appMenu.addItem(
            withTitle: "Close Window", action: #selector(NSWindow.performClose(_:)),
            keyEquivalent: "w")
        appMenu.addItem(.separator())
        appMenu.addItem(
            withTitle: "Quit Alex", action: #selector(NSApplication.terminate(_:)),
            keyEquivalent: "q")
        appItem.submenu = appMenu
        mainMenu.addItem(appItem)

        let editItem = NSMenuItem()
        let editMenu = NSMenu(title: "Edit")
        editMenu.addItem(withTitle: "Undo", action: Selector(("undo:")), keyEquivalent: "z")
        editMenu.addItem(withTitle: "Redo", action: Selector(("redo:")), keyEquivalent: "Z")
        editMenu.addItem(.separator())
        editMenu.addItem(withTitle: "Cut", action: #selector(NSText.cut(_:)), keyEquivalent: "x")
        editMenu.addItem(withTitle: "Copy", action: #selector(NSText.copy(_:)), keyEquivalent: "c")
        editMenu.addItem(withTitle: "Paste", action: #selector(NSText.paste(_:)), keyEquivalent: "v")
        editMenu.addItem(
            withTitle: "Select All", action: #selector(NSText.selectAll(_:)), keyEquivalent: "a")
        editItem.submenu = editMenu
        mainMenu.addItem(editItem)

        NSApp.mainMenu = mainMenu
    }

    func postNotification(title: String, body: String) {
        notifier.postInfo(title: title, body: body)
    }
}
