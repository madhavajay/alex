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
    private var store: SnapshotStore?
    private var statusController: StatusItemController?
    private var notifier: AlertNotifier?
    private var traceBrowserBenchmark: TraceBrowserPackagedBenchmark?
    private var traceBrowserBenchmarkController: TraceBrowserWindowController?

    func applicationDidFinishLaunching(_ notification: Notification) {
        let environment = ProcessInfo.processInfo.environment
        if environment[TraceBrowserBenchmarkConfiguration.enabledEnvironment] == "1" {
            launchTraceBrowserBenchmark(environment: environment)
            return
        }
        NSApp.setActivationPolicy(.accessory)
        installMainMenu()
        let store = SnapshotStore()
        self.store = store
        let notifier = AlertNotifier()
        self.notifier = notifier
        notifier.requestAuthorization()
        let statusController = StatusItemController(store: store)
        self.statusController = statusController
        store.onRefresh = { [weak self] in
            guard let self, let statusController = self.statusController,
                let notifier = self.notifier, let store = self.store
            else { return }
            statusController.snapshotDidChange()
            notifier.sync(alerts: store.alerts)
        }
        store.onWindowReset = { [weak self] provider, window in
            self?.notifier?.postInfo(
                title: "\(ProviderInfo.displayName(provider)) \(window) window reset",
                body: "A fresh \(window) rate-limit window is available.")
        }
        store.startPolling()
        statusController.showOnboardingIfNeeded()
    }

    func applicationWillTerminate(_ notification: Notification) {
        store?.stopPolling()
        traceBrowserBenchmarkController?.model?.stop()
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
        notifier?.postInfo(title: title, body: body)
    }

    private func launchTraceBrowserBenchmark(environment: [String: String]) {
        NSApp.setActivationPolicy(.regular)
        installMainMenu()
        guard let configuration = TraceBrowserBenchmarkConfiguration.fromEnvironment(environment)
        else {
            TraceBrowserPackagedBenchmark.writeLaunchFailure(
                environment: environment,
                failure: "benchmark mode requires result, long-session, and short-session environment variables")
            return
        }
        DaemonDiscovery.invalidateCache()
        guard let daemonConfig = DaemonDiscovery.load() else {
            TraceBrowserPackagedBenchmark.writeLaunchFailure(
                environment: environment,
                failure: "benchmark mode could not load isolated Alex daemon config")
            return
        }
        let store = SnapshotStore(config: daemonConfig)
        self.store = store
        let probe = TraceBrowserBenchmarkViewProbe()
        let controller = TraceBrowserWindowController(store: store, benchmarkProbe: probe)
        traceBrowserBenchmarkController = controller
        let initialStartedAt = ContinuousClock.now
        controller.show(selectSessionId: configuration.longSessionId)
        let benchmark = TraceBrowserPackagedBenchmark(
            configuration: configuration,
            controller: controller,
            probe: probe,
            initialStartedAt: initialStartedAt)
        traceBrowserBenchmark = benchmark
        benchmark.start()
    }
}
