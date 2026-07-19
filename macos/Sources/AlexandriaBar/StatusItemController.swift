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
    private var onboardingWindow: OnboardingWindowController?
    private let authWindows = AuthWindowController()
    private let pingWindow = PingWindowController()
    private let geminiKeyWindow = GeminiKeyWindowController()
    private let harnessActionWindow = HarnessActionWindowController()
    private let updaterController = UpdaterController()
    private var daemonUpdateApplying = false
    private var daemonUpdateTarget: String?
    private var daemonUpdateMessage: String?
    private var daemonUpdateDismissedVersion: String?
    private var appUpdateDismissedVersion: String?
    private weak var updateBannerItem: NSMenuItem?
    private static let accountsExpandedDefaultsKey = "MenuAccountsExpanded"
    private static let harnessesExpandedDefaultsKey = "MenuHarnessesExpanded"

    init(store: SnapshotStore) {
        self.store = store
        self.statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        super.init()
        statusItem.autosaveName = "AlexandriaBar"
        // Explicit enabling keeps interactive hosted SwiftUI items (update
        // banner, providers card buttons) clickable.
        menu.autoenablesItems = false
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
        updaterController.onAppUpdateStateChanged = { [weak self] in
            self?.reconcileAppUpdateState()
            self?.updateIcon()
        }
    }

    func snapshotDidChange() {
        reconcileDaemonUpdateState()
        updateIcon()
    }

    func showOnboardingIfNeeded() {
        guard UserDefaults.standard.object(
            forKey: OnboardingModel.completedDefaultsKey) == nil else { return }
        showOnboarding()
    }

    func menuWillOpen(_ menu: NSMenu) {
        Task { await store.refresh() }
        rebuildMenu()
    }

    private func updateIcon() {
        guard let button = statusItem.button else { return }
        let daemonUp = store.daemonUp || store.lastRefresh == nil
        let severity = store.worstSeverity
        button.image = IconRenderer.statusIcon()

        let dotColor: NSColor? = if !daemonUp {
            .systemRed
        } else if severity == .critical {
            .systemRed
        } else if severity == .warning {
            .systemOrange
        } else {
            nil
        }
        if let dotColor {
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

    // Section order follows the system-menu mock (ui/Design macOS system
    // menu App.tsx:665-795): header · update banner · stats · providers ·
    // accounts · harnesses · traces · footer actions.
    private func rebuildMenu() {
        menu.removeAllItems()
        buildHeader()
        buildIssues()
        // The app (Sparkle) update is independent of daemon health, so this
        // stays outside the `daemonUp` gate — unlike the daemon row, it can
        // still render "Update Alex UI" while the daemon itself is down.
        buildUpdateBanner()
        if store.daemonUp {
            buildStats()
            buildProviderEmptyState()
        }
        // Dario lives under Anthropic and remains visible there even when the
        // daemon is down, so the providers card cannot share the daemon gate.
        buildLimits()
        if store.daemonUp {
            buildAccounts()
            buildHarnesses()
        }
        buildTraces()
        buildActions()
    }

    /// Adds a hosted SwiftUI view as a (non-highlighting) menu item.
    @discardableResult
    private func addHostedView<Content: View>(
        _ view: Content, edgeToEdge: Bool = false
    ) -> NSMenuItem {
        let item = NSMenuItem()
        let host = NSHostingView(rootView: view)
        if edgeToEdge {
            // Custom NSMenuItem hosting views inherit the menu's safe-area
            // inset. The update tint must meet the adjacent separator.
            host.safeAreaRegions = []
        }
        host.frame = NSRect(origin: .zero, size: host.fittingSize)
        item.view = host
        menu.addItem(item)
        return item
    }

    private func addSectionLabel(_ text: String) {
        let item = addHostedView(MenuSectionLabelView(text: text))
        item.isEnabled = false
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

    private func addAction(
        _ title: String, indent: Int = 0, symbol: String? = nil, key: String = "",
        handler: @escaping @MainActor () -> Void
    ) {
        let item = NSMenuItem(title: title, action: #selector(runHandler(_:)), keyEquivalent: key)
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
            addHostedView(MenuHeaderView(
                appVersion: appVersion,
                daemonVersion: health.version,
                uptimeS: health.uptimeS,
                inFlight: health.inFlight,
                onCheckUpdates: updaterController.canCheckForUpdates
                    ? { [weak self] in self?.updaterController.checkForUpdates() }
                    : nil))
        } else if store.lastRefresh == nil {
            addInfo("Alex UI — connecting…")
        } else {
            addInfo("Alex daemon is not running")
            if let err = store.lastError {
                addInfo(String(err.prefix(70)), indent: 1)
            }
            addAction("Start Daemon", symbol: "play.circle") { [weak self] in
                self?.startDaemon()
            }
        }
        menu.addItem(.separator())
    }

    /// Update banner (mock App.tsx:592-635, `UpdateSection`): an "App" row
    /// (Sparkle, via `UpdaterController.availableAppUpdateVersion`), a
    /// "Daemon" row (via `store.daemonUpdate`), or both — with a single
    /// button that reads "Update Both" when both are pending.
    private func buildUpdateBanner() {
        let appVersion = pendingAppUpdateVersion
        let daemonVersion = daemonUpdateApplying ? daemonUpdateTarget : pendingDaemonUpdateVersion
        guard appVersion != nil || daemonVersion != nil || daemonUpdateMessage != nil else { return }
        let banner = MenuUpdateBannerView(
            appVersion: appVersion,
            daemonCurrentVersion: store.daemonUpdate?.current ?? store.health?.version,
            daemonVersion: daemonVersion,
            daemonApplying: daemonUpdateApplying,
            daemonMessage: daemonUpdateMessage,
            onUpdate: { [weak self] in
                self?.menu.cancelTrackingWithoutAnimation()
                self?.applyPendingUpdates(appVersion: appVersion, daemonVersion: daemonVersion)
            },
            onLater: { [weak self] in
                guard let self else { return }
                if appVersion != nil { self.appUpdateDismissedVersion = appVersion }
                if daemonVersion != nil { self.daemonUpdateDismissedVersion = daemonVersion }
                if let item = self.updateBannerItem, self.menu.items.contains(item) {
                    self.menu.removeItem(item)
                }
            })
        updateBannerItem = addHostedView(banner, edgeToEdge: true)
    }

    /// Debug-only manual preview hook: `defaults write
    /// com.madhavajay.alex DebugFakeUpdateBanner app|daemon|both`
    /// (see UpdaterController.applyDebugFakeUpdateOverride for the app half).
    /// This covers the daemon half, since daemon-update state normally comes
    /// from the live daemon's `/admin/update/status` response.
    private var debugFakeUpdateBannerMode: String? {
        UserDefaults.standard.string(forKey: UpdaterController.debugFakeUpdateBannerKey)
    }

    private var pendingAppUpdateVersion: String? {
        guard let version = updaterController.availableAppUpdateVersion,
              version != appUpdateDismissedVersion
        else { return nil }
        return version
    }

    private var pendingDaemonUpdateVersion: String? {
        if let mode = debugFakeUpdateBannerMode, mode == "daemon" || mode == "both" {
            let fake = "0.1.99-debug"
            return fake != daemonUpdateDismissedVersion ? fake : nil
        }
        guard let update = store.daemonUpdate, update.updateAvailable,
              let latest = update.latest, latest != daemonUpdateDismissedVersion
        else { return nil }
        return latest
    }

    /// "Update Both" per the mock: the daemon update first since applying it
    /// doesn't restart this app, then hand off to Sparkle's own install flow
    /// (which does restart the app, so it must go last).
    private func applyPendingUpdates(appVersion: String?, daemonVersion: String?) {
        if let daemonVersion {
            applyDaemonUpdate(latest: daemonVersion)
        }
        if appVersion != nil {
            updaterController.checkForUpdates()
        }
    }

    /// Requests / cost / errors stats bar (mock App.tsx:696-708).
    private func buildStats() {
        guard let analytics = store.analytics, analytics.totals.requests > 0 else { return }
        addHostedView(MenuStatsBarView(totals: analytics.totals))
        menu.addItem(.separator())
    }

    private var heartbeatsById: [String: Heartbeat] {
        Dictionary(uniqueKeysWithValues: store.healthAccounts.compactMap { account in
            account.lastHeartbeat.map { (account.id, $0) }
        })
    }

    private func buildLimits() {
        let darioEnabled = store.config?.darioEnabled == true || store.dario != nil
        let limits = store.daemonUp ? store.limits : []
        let accounts = store.daemonUp ? store.accounts : []
        guard ProviderPresentation.shouldShowLimitsCard(
            limits: limits,
            accounts: accounts,
            includeAnthropicDario: darioEnabled
        ) else {
            return
        }
        let card = LimitsCardView(
            limits: limits,
            accounts: accounts,
            warnPct: store.limitWarnPct,
            providerPauses: Dictionary(
                uniqueKeysWithValues: store.providerPauses.map { ($0.provider, $0) }),
            heartbeats: heartbeatsById,
            routing: store.routingByProvider,
            darioEnabled: darioEnabled,
            dario: store.daemonUp ? store.dario : nil,
            onRefresh: { [weak self] in
                guard let self else { return false }
                await self.store.refresh()
                // menuWillOpen may already have started the coalesced store
                // refresh. In that case refresh() returns immediately, so
                // keep the hosted spinner up until that in-flight work ends.
                while self.store.refreshing {
                    guard !Task.isCancelled else { return false }
                    try? await Task.sleep(for: .milliseconds(50))
                }
                return self.store.lastError == nil
            },
            onPing: { [weak self] in
                self?.menu.cancelTrackingWithoutAnimation()
                self?.runPing(target: "all", name: "All providers")
            },
            onOpenDario: { [weak self] in
                guard let self else { return }
                self.menu.cancelTrackingWithoutAnimation()
                self.openDario()
            },
            onReauthDario: { [weak self] in
                guard let self else { return }
                self.menu.cancelTrackingWithoutAnimation()
                self.reauthDario()
            },
            onSetProviderPause: { [weak self] provider, mode in
                guard let self, let config = self.store.config else { return }
                Task { @MainActor in
                    do {
                        let client = AlexandriaClient(config: config)
                        if let mode {
                            try await client.pauseProvider(provider, mode: mode)
                        } else {
                            try await client.resumeProvider(provider)
                        }
                        await self.store.refresh()
                    } catch {
                        self.store.onRefresh?()
                    }
                }
            })
        addHostedView(card)
        menu.addItem(.separator())
    }

    private func buildProviderEmptyState() {
        guard ProviderPresentation.hasNoAccounts(store.accounts) else { return }
        addInfo("No token providers connected")
        addAction("Connect a Token Provider", symbol: "plus.circle") { [weak self] in
            self?.openPreferences(section: .providers)
        }
        menu.addItem(.separator())
    }

    private func buildAccounts() {
        guard !store.accounts.isEmpty else { return }
        let expanded = UserDefaults.standard.bool(
            forKey: Self.accountsExpandedDefaultsKey)
        addHostedView(MenuCollapsibleSectionHeaderView(
            title: "Accounts",
            itemCount: store.accounts.count,
            singularItemName: "account",
            isExpanded: expanded,
            onToggle: { [weak self] in self?.toggleAccountsExpanded() }))
        guard expanded else {
            menu.addItem(.separator())
            return
        }
        let heartbeats = heartbeatsById
        for account in store.accounts {
            let heartbeat = heartbeats[account.id]
            let item = NSMenuItem(title: accountTitle(account), action: nil, keyEquivalent: "")
            item.attributedTitle = accountRowTitle(account)
            item.image = MenuItemIcon.provider(
                account.provider,
                health: MenuHealthStatus.forAccount(account, heartbeat: heartbeat))
            item.submenu = accountSubmenu(account, heartbeat: heartbeat)
            menu.addItem(item)
        }
        menu.addItem(.separator())
    }

    /// NSMenu supports changing its items while tracking. Defer the rebuild
    /// until the hosted button's click has unwound so its NSHostingView is not
    /// removed in the middle of dispatching the action.
    private func toggleAccountsExpanded() {
        let defaults = UserDefaults.standard
        let expanded = defaults.bool(forKey: Self.accountsExpandedDefaultsKey)
        defaults.set(!expanded, forKey: Self.accountsExpandedDefaultsKey)
        DispatchQueue.main.async { [weak self] in
            self?.rebuildMenu()
        }
    }

    private func accountTitle(_ account: Account) -> String {
        var title = ProviderInfo.displayName(account.provider)
        if let email = account.email, !email.isEmpty { title += " · \(email)" }
        else if let label = account.label, !label.isEmpty { title += " · \(label)" }
        else if account.name != "default" { title += " · \(account.name)" }
        return title
    }

    /// Row title per the mock (App.tsx:766-772): provider name in medium
    /// weight, email/label detail in the secondary tier.
    private func accountRowTitle(_ account: Account) -> NSAttributedString {
        let title = NSMutableAttributedString(
            string: ProviderInfo.displayName(account.provider),
            attributes: [.font: NSFont.systemFont(ofSize: 12, weight: .medium)])
        var detail: String?
        if let email = account.email, !email.isEmpty {
            detail = email
        } else if let label = account.label, !label.isEmpty {
            detail = label
        } else if account.name != "default" {
            detail = account.name
        }
        if let detail {
            title.append(NSAttributedString(
                string: "  \(detail)",
                attributes: [
                    .font: NSFont.systemFont(ofSize: 11),
                    .foregroundColor: NSColor.secondaryLabelColor,
                ]))
        }
        return title
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

        info("Email: \(account.email ?? "not supplied by provider")")
        let displayState = account.displayState(
            lastPingOK: heartbeat?.ok, lastPingStatus: heartbeat?.status)
        let displayStatus = switch displayState {
        case .active: "Active"
        case .needsReauth: "Needs re-auth"
        case .degraded: "Degraded"
        case .unreachable: "Unreachable"
        case .unknown: "Unknown"
        }
        info("\(account.id) · \(account.kind) · \(displayStatus)")
        if let expires = account.expiresInS {
            info(expires <= 0
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
            self?.openAuth(provider: account.provider, accountName: account.name)
        }
        action("Re-auth in Terminal…", symbol: "terminal") {
            let bin = DaemonController.findBinary() ?? "alexandria"
            TerminalLauncher.launch(
                command: "\(bin) auth login \(ProviderInfo.loginArg(account.provider)) --name \(account.name) --force")
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
        if account.provider == "openai" {
            sub.addItem(.separator())
            action("Add another \(name) account…", symbol: "person.badge.plus") { [weak self] in
                self?.addAnotherAccount(provider: account.provider)
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

    /// The mock places Trace Browser access in a Traces section immediately
    /// above the footer, rather than among footer commands.
    private func buildTraces() {
        addHostedView(MenuTracesSectionView(
            sessions: store.recentSessions,
            onOpen: { [weak self] in
                guard let self else { return }
                self.menu.cancelTrackingWithoutAnimation()
                self.openTraceBrowser()
            },
            onOpenSession: { [weak self] id in
                guard let self else { return }
                self.menu.cancelTrackingWithoutAnimation()
                self.openTraceBrowser(selectSessionId: id)
            }))
        menu.addItem(.separator())
    }

    /// Harness rows sit directly in the menu under a section label (mock
    /// App.tsx:343-417); each row's native submenu is the mock's flyout panel,
    /// with the system menu-highlight standing in for the mock's #0057d8.
    private func buildHarnesses() {
        guard store.harnessesSupported == true else { return }
        // Only show harnesses with a complete connect/update/disconnect workflow.
        let installed = HarnessCatalog.rows(store.harnesses).filter {
            $0.installed && $0.supportsConnect
        }
        guard !installed.isEmpty else { return }
        let expanded = UserDefaults.standard.bool(
            forKey: Self.harnessesExpandedDefaultsKey)
        addHostedView(MenuCollapsibleSectionHeaderView(
            title: "Harnesses",
            itemCount: installed.count,
            singularItemName: "harness",
            isExpanded: expanded,
            onToggle: { [weak self] in self?.toggleHarnessesExpanded() }))
        guard expanded else {
            menu.addItem(.separator())
            return
        }
        for harness in installed {
            let item = NSMenuItem(
                title: HarnessCatalog.displayName(harness.name), action: nil, keyEquivalent: "")
            item.attributedTitle = harnessRowTitle(harness)
            item.image = MenuItemIcon.harness(
                harness.name, health: harness.connected ? .ok : .pending)
                ?? harnessDotImage(harness)
            item.submenu = harnessSubmenu(harness)
            menu.addItem(item)
        }
        menu.addItem(.separator())
    }

    private func toggleHarnessesExpanded() {
        let defaults = UserDefaults.standard
        let expanded = defaults.bool(forKey: Self.harnessesExpandedDefaultsKey)
        defaults.set(!expanded, forKey: Self.harnessesExpandedDefaultsKey)
        DispatchQueue.main.async { [weak self] in
            self?.rebuildMenu()
        }
    }

    private func harnessRowTitle(_ harness: Harness) -> NSAttributedString {
        let title = NSMutableAttributedString(
            string: HarnessCatalog.displayName(harness.name),
            attributes: [.font: NSFont.systemFont(ofSize: 12, weight: .medium)])
        if let version = harness.version, !version.isEmpty {
            title.append(NSAttributedString(
                string: "  v\(version)",
                attributes: [
                    .font: NSFont.monospacedSystemFont(ofSize: 10, weight: .regular),
                    .foregroundColor: NSColor.secondaryLabelColor,
                ]))
        }
        if !harness.connected {
            title.append(NSAttributedString(
                string: "  not connected",
                attributes: [
                    .font: NSFont.systemFont(ofSize: 10),
                    .foregroundColor: NSColor.tertiaryLabelColor,
                ]))
        }
        return title
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
        if harness.name == "codex", harness.connected {
            info("Profiles: codex --profile openai · codex --profile alex")
            if let backupPath = harness.backupPath, !backupPath.isEmpty {
                info("Backup: \(backupPath)")
            }
        }
        if harness.name == "amp", harness.connected {
            info("Lifecycle: native Amp T-* thread IDs")
            info("Traffic capture: alex wrap amp")
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
            if harness.name == "amp" {
                action("Launch Wrapped Amp", symbol: "terminal") {
                    let bin = DaemonController.findBinary() ?? "alex"
                    let quoted = "'" + bin.replacingOccurrences(of: "'", with: "'\\''") + "'"
                    TerminalLauncher.launch(command: "\(quoted) wrap amp")
                }
            }
            if harness.name == "codex" {
                let useAlex = harness.defaultRoute == "alex"
                let toggle = NSMenuItem(
                    title: "Use Alex by Default",
                    action: #selector(runHandler(_:)), keyEquivalent: "")
                toggle.target = self
                toggle.state = useAlex ? .on : .off
                toggle.image = NSImage(
                    systemSymbolName: "arrow.left.arrow.right", accessibilityDescription: nil)
                toggle.representedObject = MenuHandler { [weak self] in
                    self?.setCodexDefaultRoute(useAlex ? "openai" : "alex")
                }
                sub.addItem(toggle)
            }
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

    private func setCodexDefaultRoute(_ route: String) {
        guard let config = store.config else { return }
        Task { [weak self] in
            do {
                _ = try await AlexandriaClient(config: config).setCodexDefaultRoute(route)
                await self?.store.refresh()
                self?.notify(
                    title: "Codex default updated",
                    body: route == "alex"
                        ? "New Codex sessions use Alex."
                        : "New Codex sessions use normal OpenAI authentication.")
            } catch {
                self?.notify(title: "Could not update Codex default", body: error.localizedDescription)
            }
        }
    }

    private func buildActions() {
        addAction("Report a Bug or Feature…", symbol: "exclamationmark.bubble") {
            NSWorkspace.shared.open(PreferencesView.issuesURL)
        }
        addAction("Settings…", symbol: "gearshape", key: ",") { [weak self] in
            self?.openPreferences()
        }
        addAction("Star GitHub Project", symbol: "star") {
            NSWorkspace.shared.open(URL(string: "https://github.com/madhavajay/alex")!)
        }
        if LaunchAtLogin.available {
            let item = NSMenuItem(
                title: "Launch at Login", action: #selector(runHandler(_:)), keyEquivalent: "")
            item.target = self
            item.state = LaunchAtLogin.isEnabled ? .on : .off
            item.representedObject = MenuHandler { LaunchAtLogin.toggle() }
            menu.addItem(item)
        }
        let quit = NSMenuItem(
            title: "Quit Alex UI", action: #selector(NSApplication.terminate(_:)),
            keyEquivalent: "q")
        quit.target = NSApp
        menu.addItem(quit)
    }

    private func startDaemon() {
        Task { [weak self] in
            let result = await DaemonController.startDaemon()
            await self?.store.refresh()
            self?.notify(
                title: result.ok ? "Alex daemon started" : "Failed to start daemon",
                body: String(result.combined.suffix(200)))
        }
    }

    private func runPing(target: String, name: String) {
        pingWindow.show(target: target, title: name, store: store)
    }

    private func openAuth(
        provider: String, accountName: String? = "default", autoIdentity: Bool = false
    ) {
        let callback: (@MainActor (String) -> Void)?
        if autoIdentity {
            callback = nil
        } else {
            callback = { [weak self] provider in
                self?.pingAfterAuth(provider: provider)
            }
        }
        authWindows.show(
            provider: provider, accountName: accountName, autoIdentity: autoIdentity, store: store,
            onAuthenticated: callback)
    }

    private func addAnotherAccount(provider: String) {
        guard provider == "openai" else { return }
        openAuth(provider: provider, accountName: nil, autoIdentity: true)
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
        alert.informativeText = "Alex will stop using and pinging it."
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
                    let current = response.current ?? latest
                    self?.daemonUpdateMessage = "Daemon is already up to date at \(current)"
                    self?.rebuildMenu()
                    self?.notify(title: "Daemon already up to date", body: current)
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

    /// Mirrors `reconcileDaemonUpdateState`'s dismissal-reset for the app row:
    /// once Sparkle no longer has a pending update, clear the stale dismissal
    /// so a later update isn't accidentally hidden.
    private func reconcileAppUpdateState() {
        if updaterController.availableAppUpdateVersion == nil {
            appUpdateDismissedVersion = nil
        }
    }

    private func reconcileDaemonUpdateState() {
        if store.daemonUpdate?.updateAvailable == false {
            daemonUpdateDismissedVersion = nil
        }
        guard daemonUpdateApplying, let target = daemonUpdateTarget else { return }
        let healthMatches = store.health.map { versionsMatch($0.version, target) } ?? false
        let statusMatches = store.daemonUpdate.map {
            versionsMatch($0.current, target) && !$0.updateAvailable
        } ?? false
        if healthMatches || statusMatches {
            daemonUpdateApplying = false
            daemonUpdateTarget = nil
            daemonUpdateMessage = "Daemon updated to \(target)"
            notify(title: "Daemon updated", body: "Alex \(target)")
        }
    }

    private func versionsMatch(_ lhs: String, _ rhs: String) -> Bool {
        lhs.trimmingCharacters(in: CharacterSet(charactersIn: "v"))
            == rhs.trimmingCharacters(in: CharacterSet(charactersIn: "v"))
    }

    private func notify(title: String, body: String) {
        (NSApp.delegate as? AppDelegate)?.postNotification(title: title, body: body)
    }

    private func openTraceBrowser(
        harness: String? = nil, query: String? = nil, selectSessionId: String? = nil
    ) {
        if traceBrowser == nil {
            traceBrowser = TraceBrowserWindowController(store: store)
        }
        traceBrowser?.show(harness: harness, query: query, selectSessionId: selectSessionId)
    }

    private func openDario() {
        if darioWindow == nil {
            darioWindow = DarioWindowController(store: store)
        }
        darioWindow?.show()
    }

    private func reauthDario() {
        let binary = DaemonController.findBinary() ?? "alexandria"
        TerminalLauncher.launchDarioReauth(daemonBinary: binary)
    }

    private func openPreferences(section: PreferencesSection = .general) {
        if prefsController == nil {
            prefsController = PreferencesWindowController(
                store: store,
                onOpenDario: { [weak self] in self?.openDario() },
                onOpenTraceBrowser: { [weak self] query in
                    self?.openTraceBrowser(query: query)
                },
                onRunOnboarding: { [weak self] in self?.showOnboarding() })
        }
        prefsController?.show(section: section)
    }

    private func showOnboarding() {
        if onboardingWindow == nil {
            onboardingWindow = OnboardingWindowController(
                store: store,
                openProviderSettings: { [weak self] in
                    self?.openPreferences(section: .providers)
                },
                openTraceBrowser: { [weak self] query in
                    self?.openTraceBrowser(query: query)
                })
        }
        onboardingWindow?.show()
    }
}

@MainActor
private final class MenuHandler: NSObject {
    let run: @MainActor () -> Void
    init(_ run: @escaping @MainActor () -> Void) {
        self.run = run
    }
}

/// Renders menu-item icons: a brand logo (rounded 3) with a bottom-right
/// health dot separated from the artwork by a punched-out transparent ring so
/// the badge reads on plain and highlighted row backgrounds alike (mock
/// ProviderIcon, ui/Design macOS system menu App.tsx:130-170).
@MainActor
private enum MenuItemIcon {
    static func provider(_ provider: String, health: MenuHealthStatus?) -> NSImage {
        let base: NSImage = if let alias = ProviderMenuIcon.harnessAlias[provider],
                               let logo = HarnessIconLoader.image(harness: alias, tags: nil)
        {
            logo
        } else {
            ProviderChipRenderer.image(for: provider)
        }
        return badged(base, health: health)
    }

    static func harness(_ name: String, health: MenuHealthStatus?) -> NSImage? {
        guard let logo = HarnessIconLoader.image(harness: name, tags: nil) else { return nil }
        return badged(logo, health: health)
    }

    static func badged(
        _ base: NSImage, size: CGFloat = 14, health: MenuHealthStatus?
    ) -> NSImage {
        let badge = max(4, (size * 0.38).rounded())
        let canvas = NSSize(width: size + 2, height: size + 2)
        return NSImage(size: canvas, flipped: false) { _ in
            let baseRect = NSRect(x: 0, y: 2, width: size, height: size)
            NSGraphicsContext.current?.saveGraphicsState()
            NSBezierPath(roundedRect: baseRect, xRadius: 3, yRadius: 3).addClip()
            base.draw(in: baseRect)
            NSGraphicsContext.current?.restoreGraphicsState()
            if let health {
                let dotRect = NSRect(
                    x: canvas.width - badge, y: 0, width: badge, height: badge)
                if let cg = NSGraphicsContext.current?.cgContext {
                    cg.setBlendMode(.clear)
                    cg.fillEllipse(in: dotRect.insetBy(dx: -1.5, dy: -1.5))
                    cg.setBlendMode(.normal)
                }
                tint(health)
                    .withAlphaComponent(health == .pending ? 0.5 : 1)
                    .setFill()
                NSBezierPath(ovalIn: dotRect).fill()
            }
            return true
        }
    }

    private static func tint(_ health: MenuHealthStatus) -> NSColor {
        switch health {
        case .ok: .systemGreen
        case .slow: .systemOrange
        case .error: .systemRed
        case .pending: .systemGray
        }
    }
}
