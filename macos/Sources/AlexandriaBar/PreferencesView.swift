import AppKit
import SwiftUI
import Observation
import AlexandriaBarCore

enum PreferencesSection: String, CaseIterable, Hashable {
    case general = "General"
    case subscriptions = "Subscriptions"
    case harnesses = "Harnesses"
}

@MainActor
@Observable
final class PreferencesViewState {
    var section = PreferencesSection.general
}

struct PreferencesView: View {
    @Bindable var state: PreferencesViewState
    let store: SnapshotStore
    let onAuthenticate: (String, String) -> Void
    @AppStorage("refreshSeconds") private var refreshSeconds: Double = 60
    @AppStorage("limitWarnPct") private var limitWarnPct: Double = 90
    @AppStorage("notifyEnabled") private var notifyEnabled = true
    @AppStorage("binaryPath") private var binaryPath = ""
    @AppStorage("terminalApp") private var terminalApp = "auto"
    @AppStorage("menuIconStyle") private var menuIconStyle = "logo"

    var body: some View {
        VStack(spacing: 0) {
            Picker("", selection: $state.section) {
                ForEach(PreferencesSection.allCases, id: \.self) { section in
                    Text(section.rawValue).tag(section)
                }
            }
            .pickerStyle(.segmented)
            .padding([.horizontal, .top], 16)

            Form {
                switch state.section {
                case .general:
                    generalSections
                case .subscriptions:
                    SubscriptionsPreferencesSection(store: store, onAuthenticate: onAuthenticate)
                case .harnesses:
                    HarnessesPreferencesSection(store: store)
                }
            }
            .formStyle(.grouped)
        }
        .frame(width: 560)
        .fixedSize(horizontal: false, vertical: true)
    }

    @ViewBuilder
    private var generalSections: some View {
        Section("Refresh") {
            Picker("Poll interval", selection: $refreshSeconds) {
                Text("30 seconds").tag(30.0)
                Text("1 minute").tag(60.0)
                Text("5 minutes").tag(300.0)
                Text("15 minutes").tag(900.0)
            }
        }
        Section("Menu Bar") {
            Picker("Icon", selection: $menuIconStyle) {
                Text("Alexandria logo").tag("logo")
                Text("Hieroglyph (𓂀)").tag("glyph")
            }
        }
        Section("Alerts") {
            Toggle("Show notifications", isOn: $notifyEnabled)
            Picker("Warn when a limit window reaches", selection: $limitWarnPct) {
                Text("75%").tag(75.0)
                Text("80%").tag(80.0)
                Text("90%").tag(90.0)
                Text("95%").tag(95.0)
            }
        }
        Section("Terminal") {
            Picker("Open commands in", selection: $terminalApp) {
                Text("Auto (\(TerminalLauncher.resolved.displayName))").tag("auto")
                ForEach(TerminalLauncher.installedApps, id: \.rawValue) { app in
                    Text(app.displayName).tag(app.rawValue)
                }
            }
            if TerminalLauncher.resolved == .ghostty {
                Text("Ghostty can't accept commands while already running — Terminal is used instead in that case.")
                    .font(.system(size: 10))
                    .foregroundStyle(.secondary)
            }
        }
        Section("Daemon") {
            TextField("alexandria binary path (blank = auto)", text: $binaryPath)
                .font(.system(size: 11, design: .monospaced))
            LabeledContent("Config") {
                Text(DaemonDiscovery.configPath.path)
                    .font(.system(size: 10, design: .monospaced))
                    .foregroundStyle(.secondary)
                    .textSelection(.enabled)
            }
        }
    }
}

private struct SubscriptionsPreferencesSection: View {
    let store: SnapshotStore
    let onAuthenticate: (String, String) -> Void
    @State private var providerToAdd: String?
    @State private var accountName = ""

    private let providers = ["anthropic", "openai", "gemini", "xai"]

    private var usageByAccount: [String: AccountUsage] {
        Dictionary(uniqueKeysWithValues: (store.accountAnalytics?.byAccount ?? []).map { ($0.accountId, $0) })
    }

    var body: some View {
        Section("Subscriptions") {
            Text("Each account is a separate subscription or API credential. Pause one to keep it out of routing without deleting it.")
                .font(.system(size: 11))
                .foregroundStyle(.secondary)

            if store.accounts.isEmpty {
                Text("No accounts found. Add an account to start routing requests.")
                    .foregroundStyle(.secondary)
            } else {
                ForEach(store.accounts) { account in
                    SubscriptionAccountRow(account: account, usage: usageByAccount[account.id], store: store) {
                        onAuthenticate(account.provider, account.name)
                    }
                }
            }
        }

        if let analytics = store.accountAnalytics {
            Section("Usage · last 24 hours") {
                SubscriptionUsageChart(usages: analytics.byAccount)
                ForEach(analytics.byAccount) { usage in
                    HStack {
                        Text(usage.accountId)
                            .font(.system(size: 10, design: .monospaced))
                            .lineLimit(1)
                        Spacer()
                        Text("\(usage.requests) requests · \(TraceFormat.tokens(usage.inputTokens + usage.outputTokens)) tokens")
                            .font(.system(size: 10))
                            .foregroundStyle(.secondary)
                    }
                }
            }
        }

        Section("Add subscription") {
            ForEach(providers, id: \.self) { provider in
                Button {
                    accountName = ""
                    providerToAdd = provider
                } label: {
                    Label("Add another \(ProviderInfo.displayName(provider)) account", systemImage: "person.badge.plus")
                }
            }
        }
        .sheet(
            isPresented: Binding(
                get: { providerToAdd != nil },
                set: { if !$0 { providerToAdd = nil } }
            )
        ) {
            if let provider = providerToAdd {
                SubscriptionNameSheet(provider: provider) { name in
                    providerToAdd = nil
                    onAuthenticate(provider, name)
                } onCancel: {
                    providerToAdd = nil
                }
            }
        }
    }
}

private struct SubscriptionAccountRow: View {
    let account: Account
    let usage: AccountUsage?
    let store: SnapshotStore
    let reauthenticate: () -> Void
    @State private var deleting = false
    @State private var busy = false
    @State private var error: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 8) {
                Image(systemName: account.paused ? "pause.circle.fill" : "checkmark.circle.fill")
                    .foregroundStyle(account.paused ? .orange : .green)
                Text(ProviderInfo.displayName(account.provider))
                    .fontWeight(.medium)
                Text(account.name)
                    .font(.system(size: 11, design: .monospaced))
                    .foregroundStyle(.secondary)
                Spacer()
                Text(account.paused ? "Paused" : account.status.capitalized)
                    .font(.system(size: 10, weight: .medium))
                    .foregroundStyle(account.paused ? .orange : .secondary)
            }
            Text(account.id)
                .font(.system(size: 10, design: .monospaced))
                .foregroundStyle(.secondary)
                .textSelection(.enabled)
            if let usage {
                Text("Last 24h: \(usage.requests) requests · \(TraceFormat.tokens(usage.inputTokens + usage.outputTokens)) tokens · \(usage.errors ?? 0) errors")
                    .font(.system(size: 10))
                    .foregroundStyle(.secondary)
            }
            HStack(spacing: 8) {
                Button(account.paused ? "Resume" : "Pause") { setPaused(!account.paused) }
                    .controlSize(.small)
                    .disabled(busy)
                Button("Re-authenticate") { reauthenticate() }
                    .controlSize(.small)
                Button("Remove", role: .destructive) { deleting = true }
                    .controlSize(.small)
                    .disabled(busy)
            }
            if let error {
                Text(error)
                    .font(.system(size: 10))
                    .foregroundStyle(.red)
            }
        }
        .padding(.vertical, 4)
        .alert("Remove \(ProviderInfo.displayName(account.provider)) account ‘\(account.name)’?", isPresented: $deleting) {
            Button("Remove", role: .destructive) { remove() }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("Alexandria will stop using and pinging this account.")
        }
    }

    private func setPaused(_ paused: Bool) {
        guard let config = store.config else { return }
        busy = true
        error = nil
        Task {
            do {
                try await AlexandriaClient(config: config).setAccountPaused(id: account.id, paused: paused)
                await store.refresh()
            } catch {
                self.error = error.localizedDescription
            }
            busy = false
        }
    }

    private func remove() {
        guard let config = store.config else { return }
        busy = true
        error = nil
        Task {
            do {
                try await AlexandriaClient(config: config).removeAccount(id: account.id)
                await store.refresh()
            } catch {
                self.error = error.localizedDescription
            }
            busy = false
        }
    }
}

private struct SubscriptionUsageChart: View {
    let usages: [AccountUsage]

    private var total: Int64 { usages.reduce(0) { $0 + $1.requests } }

    var body: some View {
        if usages.isEmpty {
            Text("No routed account activity in this period yet.")
                .font(.system(size: 11))
                .foregroundStyle(.secondary)
        } else {
            VStack(alignment: .leading, spacing: 6) {
                GeometryReader { geometry in
                    HStack(spacing: 2) {
                        ForEach(Array(usages.enumerated()), id: \.element.id) { index, usage in
                            Capsule()
                                .fill(color(index))
                                .frame(width: max(3, geometry.size.width * share(usage)))
                                .help("\(usage.accountId): \(usage.requests) requests")
                        }
                    }
                }
                .frame(height: 10)
                HStack(spacing: 10) {
                    ForEach(Array(usages.enumerated()), id: \.element.id) { index, usage in
                        HStack(spacing: 3) {
                            Circle().fill(color(index)).frame(width: 6, height: 6)
                            Text("\(usage.accountId) \(Int(share(usage) * 100))%")
                                .lineLimit(1)
                        }
                        .font(.system(size: 9, design: .monospaced))
                        .foregroundStyle(.secondary)
                    }
                }
            }
        }
    }

    private func share(_ usage: AccountUsage) -> CGFloat {
        guard total > 0 else { return 0 }
        return CGFloat(Double(usage.requests) / Double(total))
    }

    private func color(_ index: Int) -> Color {
        [.blue, .green, .orange, .purple, .pink, .teal][index % 6]
    }
}

private struct SubscriptionNameSheet: View {
    let provider: String
    let onContinue: (String) -> Void
    let onCancel: () -> Void
    @State private var name = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("Add \(ProviderInfo.displayName(provider)) account")
                .font(.title3.bold())
            Text("Give this subscription a local name so you can choose, pause, and order it later.")
                .font(.system(size: 12))
                .foregroundStyle(.secondary)
            TextField("Name, e.g. personal", text: $name)
                .textFieldStyle(.roundedBorder)
            Text("Lowercase letters, numbers, _ and - only.")
                .font(.system(size: 10))
                .foregroundStyle(.secondary)
            HStack {
                Spacer()
                Button("Cancel", action: onCancel)
                    .keyboardShortcut(.cancelAction)
                Button("Continue") {
                    onContinue(name.trimmingCharacters(in: .whitespacesAndNewlines).lowercased())
                }
                .keyboardShortcut(.defaultAction)
                .disabled(name.range(of: "^[a-z0-9_-]{1,32}$", options: .regularExpression) == nil)
            }
        }
        .padding(20)
        .frame(width: 420)
    }
}

private struct HarnessesPreferencesSection: View {
    let store: SnapshotStore
    @State private var updateAllModel: MultiHarnessRefreshSheetModel?

    private var refreshTargets: [Harness] {
        HarnessCatalog.refreshTargets(store.harnesses)
    }

    var body: some View {
        Section("Harnesses") {
            if store.harnessesSupported == false {
                HStack(spacing: 8) {
                    Image(systemName: "exclamationmark.triangle")
                        .foregroundStyle(.orange)
                    Text("daemon update required — run ")
                    Text("alex update")
                        .font(.system(size: 11, design: .monospaced))
                }
                .font(.system(size: 11))
                .foregroundStyle(.secondary)
            } else if store.harnessesSupported == nil {
                HStack(spacing: 8) {
                    ProgressView()
                        .controlSize(.small)
                    Text("Checking harness support…")
                        .foregroundStyle(.secondary)
                }
                .task {
                    await store.refresh()
                }
            } else {
                if !refreshTargets.isEmpty {
                    HStack {
                        Text(
                            "\(refreshTargets.count) connected harness\(refreshTargets.count == 1 ? "" : "es") can update"
                        )
                        .font(.system(size: 11))
                        .foregroundStyle(.secondary)
                        Spacer()
                        Button("Update All Harnesses") {
                            let model = MultiHarnessRefreshSheetModel(store: store)
                            updateAllModel = model
                            model.start()
                        }
                        .controlSize(.small)
                    }
                }
                ForEach(HarnessCatalog.rows(store.harnesses)) { harness in
                    HarnessRowView(harness: harness, store: store)
                }
            }
        }
        .sheet(item: $updateAllModel) { sheet in
            MultiHarnessRefreshSheetHost(sheet: sheet) {
                updateAllModel = nil
            }
        }
    }
}

private struct MultiHarnessRefreshSheetHost: View {
    let sheet: MultiHarnessRefreshSheetModel
    let onClose: () -> Void

    var body: some View {
        // Observe the inner model so sequential updates re-render.
        MultiHarnessRefreshRootProxy(model: sheet.model, onClose: onClose)
    }
}

private struct MultiHarnessRefreshRootProxy: View {
    @Bindable var model: MultiHarnessRefreshModel
    let onClose: () -> Void

    var body: some View {
        MultiHarnessRefreshResultView(
            items: model.items,
            finished: model.finished,
            totalsLine: model.totalsLine,
            onClose: onClose
        )
    }
}

private struct HarnessRowView: View {
    let harness: Harness
    let store: SnapshotStore
    @State private var error: String?
    @State private var showOverride = false
    @State private var actionModel: HarnessActionSheetModel?

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .center, spacing: 10) {
                statusDot
                    .frame(width: 10, height: 10)
                VStack(alignment: .leading, spacing: 2) {
                    HStack(spacing: 5) {
                        Text(HarnessCatalog.displayName(harness.name))
                            .fontWeight(.medium)
                        if let version = harness.version, !version.isEmpty {
                            Text("v\(version)")
                                .foregroundStyle(.secondary)
                        }
                        if let warning = harness.versionWarning, !warning.isEmpty {
                            Image(systemName: "exclamationmark.triangle.fill")
                                .foregroundStyle(.orange)
                                .help(warning)
                        }
                    }
                    Text(harness.configDir ?? "No config directory")
                        .font(.system(size: 10, design: .monospaced))
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
                Spacer()
                if harness.supportsConnect {
                    if harness.connected {
                        Button(HarnessActionKind.refresh.label) {
                            beginAction(.refresh)
                        }
                        .controlSize(.small)
                        .disabled(actionModel != nil)
                    }
                    actionButton
                }
                Button("Override…") {
                    showOverride = true
                }
                .controlSize(.small)
                .disabled(actionModel != nil)
            }
            if let error {
                Text(error)
                    .font(.system(size: 10))
                    .foregroundStyle(.red)
            }
        }
        .font(.system(size: 12))
        .sheet(isPresented: $showOverride) {
            HarnessOverrideSheet(harness: harness, store: store)
        }
        .sheet(item: $actionModel) { model in
            HarnessActionSheetHost(model: model) {
                actionModel = nil
            }
        }
    }

    @ViewBuilder
    private var statusDot: some View {
        if !harness.installed {
            Circle().stroke(.secondary, lineWidth: 1.5)
        } else {
            Circle().fill(harness.connected ? Color.green : Color.secondary)
        }
    }

    @ViewBuilder
    private var actionButton: some View {
        if actionModel != nil {
            ProgressView()
                .controlSize(.small)
        } else {
            Button(
                harness.connected
                    ? HarnessActionKind.disconnect.label : HarnessActionKind.connect.label
            ) {
                beginAction(harness.connected ? .disconnect : .connect)
            }
            .controlSize(.small)
        }
    }

    private func beginAction(_ kind: HarnessActionKind) {
        error = nil
        let model = HarnessActionSheetModel(store: store, harness: harness, kind: kind)
        actionModel = model
        model.start()
    }
}

private struct HarnessActionSheetHost: View {
    @Bindable var model: HarnessActionSheetModel
    let onClose: () -> Void

    var body: some View {
        HarnessActionResultView(
            kind: model.kind,
            harnessDisplayName: model.displayName,
            phase: model.phase,
            onApprove: { model.approve() },
            onCancel: onClose,
            onClose: onClose
        )
    }
}

private struct HarnessOverrideSheet: View {
    let harness: Harness
    let store: SnapshotStore
    @Environment(\.dismiss) private var dismiss
    @State private var binary: String
    @State private var configDir: String
    @State private var saving = false
    @State private var error: String?

    init(harness: Harness, store: SnapshotStore) {
        self.harness = harness
        self.store = store
        _binary = State(initialValue: harness.override?.binary ?? "")
        _configDir = State(initialValue: harness.override?.configDir ?? "")
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("\(HarnessCatalog.displayName(harness.name)) Override")
                .font(.headline)
            Grid(alignment: .leading, horizontalSpacing: 10, verticalSpacing: 10) {
                GridRow {
                    Text("Binary")
                    HStack(spacing: 6) {
                        TextField("Binary path (blank = clear)", text: $binary)
                            .font(.system(size: 11, design: .monospaced))
                        Button {
                            chooseBinary()
                        } label: {
                            Image(systemName: "folder")
                        }
                    }
                }
                GridRow {
                    Text("Config dir")
                    HStack(spacing: 6) {
                        TextField("Config dir (blank = clear)", text: $configDir)
                            .font(.system(size: 11, design: .monospaced))
                        Button {
                            chooseConfigDir()
                        } label: {
                            Image(systemName: "folder")
                        }
                    }
                }
            }
            if let error {
                Text(error)
                    .font(.system(size: 10))
                    .foregroundStyle(.red)
            }
            HStack {
                Spacer()
                Button("Cancel") {
                    dismiss()
                }
                Button {
                    save()
                } label: {
                    if saving {
                        ProgressView()
                            .controlSize(.small)
                    } else {
                        Text("Save")
                    }
                }
                .keyboardShortcut(.defaultAction)
                .disabled(saving)
            }
        }
        .padding(18)
        .frame(width: 480)
    }

    private func save() {
        guard let config = store.config else { return }
        saving = true
        error = nil
        let client = AlexandriaClient(config: config)
        Task {
            do {
                _ = try await client.setHarnessOverride(
                    harness.name, binary: clean(binary), configDir: clean(configDir))
                await store.refresh()
                dismiss()
            } catch {
                self.error = error.localizedDescription
            }
            saving = false
        }
    }

    private func clean(_ value: String) -> String? {
        let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }

    private func chooseBinary() {
        let panel = NSOpenPanel()
        panel.canChooseFiles = true
        panel.canChooseDirectories = false
        panel.allowsMultipleSelection = false
        NSApp.activate(ignoringOtherApps: true)
        guard panel.runModal() == .OK, let url = panel.url else { return }
        binary = url.path
    }

    private func chooseConfigDir() {
        let panel = NSOpenPanel()
        panel.canChooseFiles = false
        panel.canChooseDirectories = true
        panel.allowsMultipleSelection = false
        NSApp.activate(ignoringOtherApps: true)
        guard panel.runModal() == .OK, let url = panel.url else { return }
        configDir = url.path
    }
}

@MainActor
final class PreferencesWindowController {
    private var window: NSWindow?
    private let state = PreferencesViewState()
    private let store: SnapshotStore
    private let authWindows = AuthWindowController()

    init(store: SnapshotStore) {
        self.store = store
    }

    func show(section: PreferencesSection = .general) {
        state.section = section
        if window == nil {
            let host = NSHostingController(rootView: PreferencesView(
                state: state,
                store: store,
                onAuthenticate: { [weak self] provider, name in
                    guard let self else { return }
                    self.authWindows.show(provider: provider, accountName: name, store: self.store)
                }))
            let win = NSWindow(contentViewController: host)
            win.title = "AlexandriaBar Settings"
            win.styleMask = [.titled, .closable]
            win.isReleasedWhenClosed = false
            win.center()
            window = win
        }
        if let window {
            DockIconManager.shared.track(window)
            window.makeKeyAndOrderFront(nil)
        }
    }
}
