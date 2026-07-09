import AppKit
import SwiftUI
import AlexandriaBarCore

@MainActor
final class StatusItemController: NSObject, NSMenuDelegate {
    private let store: SnapshotStore
    private let statusItem: NSStatusItem
    private let menu = NSMenu()
    private var prefsController: PreferencesWindowController?
    private var traceBrowser: TraceBrowserWindowController?
    private var darioWindow: DarioWindowController?
    private let authWindows = AuthWindowController()
    private let pingWindow = PingWindowController()
    private let geminiKeyWindow = GeminiKeyWindowController()
    private let harnessActionWindow = HarnessActionWindowController()
    private let updaterController = UpdaterController()
    private var daemonUpdateApplying = false
    private var daemonUpdateTarget: String?
    private var daemonUpdateMessage: String?

    init(store: SnapshotStore) {
        self.store = store
        self.statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        super.init()
        statusItem.autosaveName = "AlexandriaBar"
        menu.delegate = self
        statusItem.menu = menu
        updateIcon()
        NotificationCenter.default.addObserver(
            forName: UserDefaults.didChangeNotification, object: nil, queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated {
                self?.updateIcon()
            }
        }
    }

    func snapshotDidChange() {
        reconcileDaemonUpdateState()
        updateIcon()
    }

    func menuWillOpen(_ menu: NSMenu) {
        Task { await store.refresh() }
        rebuildMenu()
    }

    private func updateIcon() {
        guard let button = statusItem.button else { return }
        let daemonUp = store.daemonUp || store.lastRefresh == nil
        let severity = store.worstSeverity
        button.image = IconRenderer.statusIcon(severity: severity, daemonUp: daemonUp)

        let dotColor: NSColor? = if !daemonUp {
            .systemRed
        } else if severity == .critical {
            .systemRed
        } else if severity == .warning {
            .systemOrange
        } else {
            nil
        }
        if IconRenderer.style == "logo", let dotColor {
            button.imagePosition = .imageLeading
            button.attributedTitle = NSAttributedString(string: "●", attributes: [
                .foregroundColor: dotColor,
                .font: NSFont.systemFont(ofSize: 8),
                .baselineOffset: 3,
            ])
        } else {
            button.imagePosition = .imageOnly
            button.attributedTitle = NSAttributedString(string: "")
        }
    }

    private func rebuildMenu() {
        menu.removeAllItems()
        buildHeader()
        buildIssues()
        if store.daemonUp {
            buildLimits()
            buildAccounts()
            buildHarnesses()
            buildDario()
            buildAnalytics()
        }
        buildActions()
    }

    private func buildIssues() {
        let issues = store.alerts.filter { $0.id != "daemon-down" }
        guard !issues.isEmpty else { return }
        for alert in issues {
            let item = NSMenuItem(title: alert.title, action: nil, keyEquivalent: "")
            item.isEnabled = false
            item.image = NSImage(
                systemSymbolName: "exclamationmark.triangle.fill",
                accessibilityDescription: nil)?
                .withSymbolConfiguration(.init(paletteColors: [
                    alert.severity == .critical ? .systemRed : .systemOrange,
                ]))
            item.toolTip = alert.body
            menu.addItem(item)
        }
        menu.addItem(.separator())
    }

    private func addInfo(_ title: String, indent: Int = 0) {
        let item = NSMenuItem(title: title, action: nil, keyEquivalent: "")
        item.isEnabled = false
        item.indentationLevel = indent
        menu.addItem(item)
    }

    private func addAction(_ title: String, indent: Int = 0, symbol: String? = nil, handler: @escaping @MainActor () -> Void) {
        let item = NSMenuItem(title: title, action: #selector(runHandler(_:)), keyEquivalent: "")
        item.target = self
        item.indentationLevel = indent
        if let symbol {
            item.image = NSImage(systemSymbolName: symbol, accessibilityDescription: nil)
        }
        item.representedObject = MenuHandler(handler)
        menu.addItem(item)
    }

    @objc private func runHandler(_ sender: NSMenuItem) {
        (sender.representedObject as? MenuHandler)?.run()
    }

    private var appVersion: String {
        Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "dev"
    }

    private func buildHeader() {
        if store.daemonUp, let health = store.health {
            var line = "Alexandria app v\(appVersion) · daemon v\(health.version)"
            line += " · up \(Format.duration(health.uptimeS))"
            if health.inFlight > 0 { line += " · \(health.inFlight) in flight" }
            addInfo(line)
        } else if store.lastRefresh == nil {
            addInfo("Alexandria — connecting…")
        } else {
            addInfo("Alexandria daemon is not running")
            if let err = store.lastError {
                addInfo(String(err.prefix(70)), indent: 1)
            }
            addAction("Start Daemon", symbol: "play.circle") { [weak self] in
                self?.startDaemon()
            }
        }
        menu.addItem(.separator())
    }

    private func buildLimits() {
        guard !store.limits.isEmpty else { return }
        let item = NSMenuItem()
        let card = LimitsCardView(limits: store.limits, warnPct: store.limitWarnPct)
        let host = NSHostingView(rootView: card)
        host.frame = NSRect(origin: .zero, size: host.fittingSize)
        item.view = host
        menu.addItem(item)
        menu.addItem(.separator())
    }

    private func buildAccounts() {
        guard !store.accounts.isEmpty else { return }
        addInfo("Accounts")
        let heartbeats = Dictionary(
            uniqueKeysWithValues: store.healthAccounts.map { ($0.id, $0.lastHeartbeat) })
        for account in store.accounts {
            let item = NSMenuItem(title: accountTitle(account), action: nil, keyEquivalent: "")
            item.image = dotImage(for: account, heartbeat: heartbeats[account.id] ?? nil)
            item.submenu = accountSubmenu(account, heartbeat: heartbeats[account.id] ?? nil)
            menu.addItem(item)
        }
        menu.addItem(.separator())
    }

    private func accountTitle(_ account: Account) -> String {
        var title = ProviderInfo.displayName(account.provider)
        if let label = account.label, !label.isEmpty { title += " · \(label)" }
        return title
    }

    private func dotImage(for account: Account, heartbeat: Heartbeat?) -> NSImage? {
        let color: NSColor
        if account.status != "active" || heartbeat?.ok == false {
            color = .systemRed
        } else if account.isExpired {
            color = .systemOrange
        } else {
            color = .systemGreen
        }
        return NSImage(systemSymbolName: "circle.fill", accessibilityDescription: nil)?
            .withSymbolConfiguration(.init(paletteColors: [color]))
    }

    private func accountSubmenu(_ account: Account, heartbeat: Heartbeat?) -> NSMenu {
        let sub = NSMenu()
        let name = ProviderInfo.displayName(account.provider)

        func info(_ title: String) {
            let item = NSMenuItem(title: title, action: nil, keyEquivalent: "")
            item.isEnabled = false
            sub.addItem(item)
        }
        func action(_ title: String, symbol: String? = nil, handler: @escaping @MainActor () -> Void) {
            let item = NSMenuItem(title: title, action: #selector(runHandler(_:)), keyEquivalent: "")
            item.target = self
            if let symbol {
                item.image = NSImage(systemSymbolName: symbol, accessibilityDescription: nil)
            }
            item.representedObject = MenuHandler(handler)
            sub.addItem(item)
        }

        info("\(account.id) · \(account.kind) · \(account.status)")
        if let expires = account.expiresInS {
            info(expires < 0
                ? "Token expired \(Format.duration(expires)) ago"
                : "Token expires in \(Format.duration(expires))")
        }
        if let hb = heartbeat {
            let age = Format.duration(Int64(Date().timeIntervalSince1970) - hb.tsMs / 1000)
            info(hb.ok
                ? "Heartbeat OK · \(hb.latencyMs ?? 0)ms · \(age) ago"
                : "Heartbeat FAILED \(age) ago")
        }
        sub.addItem(.separator())
        action("Re-auth \(name)…", symbol: "key") { [weak self] in
            self?.openAuth(provider: account.provider)
        }
        action("Re-auth in Terminal…", symbol: "terminal") {
            let bin = DaemonController.findBinary() ?? "alexandria"
            TerminalLauncher.launch(
                command: "\(bin) auth login \(ProviderInfo.loginArg(account.provider))")
        }
        action("Re-import credentials", symbol: "square.and.arrow.down") { [weak self] in
            self?.importCredentials()
        }
        if let ping = ProviderInfo.pingArg(account.provider) {
            action("Ping \(name)", symbol: "dot.radiowaves.left.and.right") { [weak self] in
                self?.runPing(target: ping, name: name)
            }
        }
        if account.provider == "gemini" {
            action("Set AI Studio API Key…", symbol: "key.horizontal") { [weak self] in
                self?.setGeminiKey()
            }
        }
        if account.provider == "openai" {
            sub.addItem(.separator())
            action("Start 5h Window Now…", symbol: "hourglass.bottomhalf.filled") { [weak self] in
                self?.confirmStartCodexWindow()
            }
        }
        sub.addItem(.separator())
        action("Remove Account", symbol: "trash") { [weak self] in
            self?.removeAccount(account)
        }
        return sub
    }

    private func confirmStartCodexWindow() {
        let alert = NSAlert()
        alert.messageText = "Start a fresh Codex 5-hour window?"
        alert.informativeText = "This sends one tiny request (a few tokens) through your Codex subscription so the 5-hour rate-limit window starts now instead of with your next real request. It consumes a negligible amount of quota."
        alert.addButton(withTitle: "Start Window")
        alert.addButton(withTitle: "Cancel")
        NSApp.activate(ignoringOtherApps: true)
        guard alert.runModal() == .alertFirstButtonReturn else { return }
        runPing(target: "openai", name: "Codex window start")
    }

    private func buildDario() {
        guard let dario = store.dario else { return }
        let active = dario.generations.first { $0.id == dario.activeGenerationId }
        var title = "Dario"
        if let active {
            title += " · v\(active.version) · \(active.phase)"
            if let probe = active.lastProbe, probe.ok, let ms = probe.latencyMs {
                title += " · \(ms)ms"
            }
        }
        let item = NSMenuItem(title: title, action: nil, keyEquivalent: "")
        let sub = NSMenu()
        let nodeInfo = NSMenuItem(
            title: store.nodePath.map { "Node: \($0)" } ?? "Node.js not found — install it for dario",
            action: nil, keyEquivalent: "")
        nodeInfo.isEnabled = false
        sub.addItem(nodeInfo)
        sub.addItem(.separator())
        let restart = NSMenuItem(title: "Restart Dario", action: #selector(runHandler(_:)), keyEquivalent: "")
        restart.target = self
        restart.representedObject = MenuHandler { [weak self] in self?.darioAction(update: false) }
        sub.addItem(restart)
        let update = NSMenuItem(title: "Check for Update", action: #selector(runHandler(_:)), keyEquivalent: "")
        update.target = self
        update.representedObject = MenuHandler { [weak self] in self?.darioAction(update: true) }
        sub.addItem(update)
        sub.addItem(.separator())
        let about = NSMenuItem(title: "What is Dario?", action: #selector(runHandler(_:)), keyEquivalent: "")
        about.target = self
        about.image = NSImage(systemSymbolName: "questionmark.circle", accessibilityDescription: nil)
        about.representedObject = MenuHandler {
            NSWorkspace.shared.open(URL(string: "https://github.com/askalf/dario")!)
        }
        sub.addItem(about)
        item.submenu = sub
        menu.addItem(item)
        menu.addItem(.separator())
    }

    private func buildHarnesses() {
        guard store.harnessesSupported == true else { return }
        let installed = HarnessCatalog.rows(store.harnesses).filter(\.installed)
        guard !installed.isEmpty else { return }
        let item = NSMenuItem(title: "Harnesses", action: nil, keyEquivalent: "")
        let sub = NSMenu()
        for harness in installed {
            let harnessItem = NSMenuItem(
                title: HarnessCatalog.displayName(harness.name), action: nil, keyEquivalent: "")
            harnessItem.image = harnessDotImage(harness)
            harnessItem.submenu = harnessSubmenu(harness)
            sub.addItem(harnessItem)
        }
        sub.addItem(.separator())
        let updateAll = NSMenuItem(
            title: "Update All Harnesses",
            action: #selector(runHandler(_:)),
            keyEquivalent: "")
        updateAll.target = self
        updateAll.image = NSImage(
            systemSymbolName: "arrow.triangle.2.circlepath", accessibilityDescription: nil)
        updateAll.representedObject = MenuHandler { [weak self] in
            guard let self else { return }
            self.harnessActionWindow.showUpdateAll(store: self.store)
        }
        sub.addItem(updateAll)
        item.submenu = sub
        menu.addItem(item)
        menu.addItem(.separator())
    }

    private func harnessDotImage(_ harness: Harness) -> NSImage? {
        NSImage(systemSymbolName: "circle.fill", accessibilityDescription: nil)?
            .withSymbolConfiguration(.init(paletteColors: [
                harness.connected ? .systemGreen : .secondaryLabelColor,
            ]))
    }

    private func harnessSubmenu(_ harness: Harness) -> NSMenu {
        let sub = NSMenu()
        let name = HarnessCatalog.displayName(harness.name)

        func info(_ title: String) {
            let item = NSMenuItem(title: title, action: nil, keyEquivalent: "")
            item.isEnabled = false
            sub.addItem(item)
        }
        func action(_ title: String, symbol: String? = nil, handler: @escaping @MainActor () -> Void) {
            let item = NSMenuItem(title: title, action: #selector(runHandler(_:)), keyEquivalent: "")
            item.target = self
            if let symbol {
                item.image = NSImage(systemSymbolName: symbol, accessibilityDescription: nil)
            }
            item.representedObject = MenuHandler(handler)
            sub.addItem(item)
        }

        info(harness.version.map { "\(name) · v\($0)" } ?? name)
        if let configDir = harness.configDir, !configDir.isEmpty {
            info(configDir)
        }
        sub.addItem(.separator())
        if harness.supportsConnect, !harness.connected {
            action(HarnessActionKind.connect.label, symbol: "arrow.down.circle") { [weak self] in
                guard let self else { return }
                self.harnessActionWindow.show(store: self.store, harness: harness, kind: .connect)
            }
        }
        action("Configure…", symbol: "gearshape") { [weak self] in
            self?.openPreferences(section: .harnesses)
        }
        if harness.connected {
            action(HarnessActionKind.refresh.label, symbol: "arrow.triangle.2.circlepath") {
                [weak self] in
                guard let self else { return }
                self.harnessActionWindow.show(store: self.store, harness: harness, kind: .refresh)
            }
            action(HarnessActionKind.disconnect.label, symbol: "trash") { [weak self] in
                guard let self else { return }
                self.harnessActionWindow.show(store: self.store, harness: harness, kind: .disconnect)
            }
        }
        action("View in Trace Browser", symbol: "list.bullet.rectangle") { [weak self] in
            self?.openTraceBrowser(harness: harness.name)
        }
        return sub
    }

    private func buildAnalytics() {
        guard let analytics = store.analytics, analytics.totals.requests > 0 else { return }
        let t = analytics.totals
        var line = "Last hour: \(t.requests) requests · $\(String(format: "%.4f", t.costUsd))"
        if t.errors > 0 { line += " · \(t.errors) errors" }
        addInfo(line)
        menu.addItem(.separator())
    }

    private func buildActions() {
        addAction("Run Ping Checks", symbol: "dot.radiowaves.left.and.right") { [weak self] in
            self?.runPing(target: "all", name: "All providers")
        }
        addAction("Re-auth Subscriptions…", symbol: "key") { }
        if let last = menu.items.last {
            let sub = NSMenu()
            for provider in ["anthropic", "openai", "xai", "gemini"] {
                let item = NSMenuItem(
                    title: ProviderInfo.displayName(provider),
                    action: #selector(runHandler(_:)), keyEquivalent: "")
                item.target = self
                item.representedObject = MenuHandler { [weak self] in
                    self?.openAuth(provider: provider)
                }
                sub.addItem(item)
            }
            sub.addItem(.separator())
            let importItem = NSMenuItem(title: "Re-import All Credentials", action: #selector(runHandler(_:)), keyEquivalent: "")
            importItem.target = self
            importItem.representedObject = MenuHandler { [weak self] in self?.importCredentials() }
            sub.addItem(importItem)
            last.submenu = sub
        }
        addAction("Refresh Now", symbol: "arrow.clockwise") { [weak self] in
            Task { await self?.store.refresh() }
        }
        addAction("Trace Browser…", symbol: "list.bullet.rectangle") { [weak self] in
            self?.openTraceBrowser()
        }
        addAction("Dario…", symbol: "server.rack") { [weak self] in
            self?.openDario()
        }
        addAction("Reveal Log File", symbol: "doc.text.magnifyingglass") {
            NSWorkspace.shared.activateFileViewerSelecting([BarLog.fileURL])
        }
        addAction("Open TUI in Terminal", symbol: "terminal") {
            let bin = DaemonController.findBinary() ?? "alexandria"
            TerminalLauncher.launch(command: "\(bin) tui")
        }
        menu.addItem(.separator())
        addAction("Settings…", symbol: "gearshape") { [weak self] in
            self?.openPreferences()
        }
        buildDaemonUpdateAction()
        let updateItem = NSMenuItem(title: "Check for Updates…", action: #selector(runHandler(_:)), keyEquivalent: "")
        updateItem.target = self
        updateItem.image = NSImage(systemSymbolName: "arrow.down.circle", accessibilityDescription: nil)
        updateItem.isEnabled = updaterController.canCheckForUpdates
        updateItem.representedObject = MenuHandler { [weak self] in
            self?.updaterController.checkForUpdates()
        }
        menu.addItem(updateItem)
        if LaunchAtLogin.available {
            let item = NSMenuItem(title: "Launch at Login", action: #selector(runHandler(_:)), keyEquivalent: "")
            item.target = self
            item.state = LaunchAtLogin.isEnabled ? .on : .off
            item.representedObject = MenuHandler { LaunchAtLogin.toggle() }
            menu.addItem(item)
        }
        menu.addItem(.separator())
        let quit = NSMenuItem(title: "Quit AlexandriaBar", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
        quit.target = NSApp
        menu.addItem(quit)
    }

    private func buildDaemonUpdateAction() {
        if daemonUpdateApplying {
            let target = daemonUpdateTarget ?? store.daemonUpdate?.latest ?? "latest"
            addInfo("Updating daemon to \(target)…")
            return
        }
        if let update = store.daemonUpdate, update.updateAvailable, let latest = update.latest {
            addAction("Update daemon to \(latest)…", symbol: "arrow.down.circle") { [weak self] in
                self?.applyDaemonUpdate(latest: latest)
            }
        }
        if let message = daemonUpdateMessage {
            addInfo(String(message.prefix(70)))
        }
    }

    private func startDaemon() {
        Task { [weak self] in
            let result = await DaemonController.startDaemon()
            await self?.store.refresh()
            self?.notify(
                title: result.ok ? "Alexandria daemon started" : "Failed to start daemon",
                body: String(result.combined.suffix(200)))
        }
    }

    private func runPing(target: String, name: String) {
        pingWindow.show(target: target, title: name, store: store)
    }

    private func openAuth(provider: String) {
        authWindows.show(provider: provider, store: store) { [weak self] provider in
            self?.pingAfterAuth(provider: provider)
        }
    }

    private func pingAfterAuth(provider: String) {
        guard let ping = ProviderInfo.pingArg(provider) else { return }
        runPing(target: ping, name: ProviderInfo.displayName(provider))
    }

    private func setGeminiKey() {
        guard store.config != nil else { return }
        geminiKeyWindow.show(store: store) { [weak self] in
            self?.pingAfterAuth(provider: "gemini")
        }
    }

    private func removeAccount(_ account: Account) {
        let name = ProviderInfo.displayName(account.provider)
        let alert = NSAlert()
        alert.messageText = "Remove \(name) account (\(account.id))?"
        alert.informativeText = "Alexandria will stop using and pinging it."
        alert.addButton(withTitle: "Remove")
        alert.addButton(withTitle: "Cancel")
        NSApp.activate(ignoringOtherApps: true)
        guard alert.runModal() == .alertFirstButtonReturn else { return }
        guard let config = store.config else { return }
        let client = AlexandriaClient(config: config)
        Task { [weak self] in
            do {
                try await client.removeAccount(id: account.id)
                await self?.store.refresh()
                self?.notify(title: "\(name) account removed", body: account.id)
            } catch {
                self?.notify(title: "Failed to remove account", body: error.localizedDescription)
            }
        }
    }

    private func importCredentials() {
        Task { [weak self] in
            let result = await DaemonController.importCredentials()
            await self?.store.refresh()
            let tail = result.combined.split(separator: "\n").suffix(4).joined(separator: "\n")
            self?.notify(
                title: result.ok ? "Credentials re-imported" : "Credential import failed",
                body: String(tail.prefix(300)))
        }
    }

    private func darioAction(update: Bool) {
        guard let config = store.config else { return }
        let client = AlexandriaClient(config: config)
        Task { [weak self] in
            do {
                if update {
                    try await client.darioUpdate()
                } else {
                    try await client.darioRestart()
                }
                await self?.store.refresh()
                self?.notify(title: update ? "Dario update triggered" : "Dario restart triggered", body: "")
            } catch {
                self?.notify(title: "Dario action failed", body: error.localizedDescription)
            }
        }
    }

    private func applyDaemonUpdate(latest: String) {
        guard let config = store.config else { return }
        let client = AlexandriaClient(config: config)
        daemonUpdateApplying = true
        daemonUpdateTarget = latest
        daemonUpdateMessage = nil
        rebuildMenu()
        Task { [weak self] in
            do {
                let response = try await client.daemonUpdateApply()
                if response.applying {
                    self?.notify(title: "Daemon update started", body: "Updating to \(response.latest ?? latest)")
                } else {
                    self?.daemonUpdateApplying = false
                    self?.daemonUpdateTarget = nil
                    self?.notify(title: "Daemon already up to date", body: response.current ?? "")
                }
                await self?.store.refresh()
            } catch AlexandriaClient.ClientError.daemonUpdateRejected(let reason) {
                self?.daemonUpdateApplying = false
                self?.daemonUpdateTarget = nil
                self?.daemonUpdateMessage = reason
                self?.rebuildMenu()
                self?.notify(title: "Daemon update unavailable", body: reason)
            } catch {
                self?.daemonUpdateApplying = false
                self?.daemonUpdateTarget = nil
                self?.daemonUpdateMessage = error.localizedDescription
                self?.rebuildMenu()
                self?.notify(title: "Daemon update failed", body: error.localizedDescription)
            }
        }
    }

    private func reconcileDaemonUpdateState() {
        if store.daemonUpdate?.updateAvailable == false {
            daemonUpdateMessage = nil
        }
        guard daemonUpdateApplying, let target = daemonUpdateTarget else { return }
        let healthMatches = store.health.map { versionsMatch($0.version, target) } ?? false
        let statusMatches = store.daemonUpdate.map {
            versionsMatch($0.current, target) && !$0.updateAvailable
        } ?? false
        if healthMatches || statusMatches {
            daemonUpdateApplying = false
            daemonUpdateTarget = nil
            daemonUpdateMessage = nil
            notify(title: "Daemon updated", body: "Alexandria \(target)")
        }
    }

    private func versionsMatch(_ lhs: String, _ rhs: String) -> Bool {
        lhs.trimmingCharacters(in: CharacterSet(charactersIn: "v"))
            == rhs.trimmingCharacters(in: CharacterSet(charactersIn: "v"))
    }

    private func notify(title: String, body: String) {
        (NSApp.delegate as? AppDelegate)?.postNotification(title: title, body: body)
    }

    private func openTraceBrowser(harness: String? = nil) {
        if traceBrowser == nil {
            traceBrowser = TraceBrowserWindowController(store: store)
        }
        traceBrowser?.show(harness: harness)
    }

    private func openDario() {
        if darioWindow == nil {
            darioWindow = DarioWindowController(store: store)
        }
        darioWindow?.show()
    }

    private func openPreferences(section: PreferencesSection = .general) {
        if prefsController == nil {
            prefsController = PreferencesWindowController(store: store)
        }
        prefsController?.show(section: section)
    }
}

@MainActor
private final class MenuHandler: NSObject {
    let run: @MainActor () -> Void
    init(_ run: @escaping @MainActor () -> Void) {
        self.run = run
    }
}
