import AppKit
import Charts
import SwiftUI
import Observation
import AlexandriaBarCore

enum PreferencesSection: String, CaseIterable, Hashable {
    case general = "General"
    case providers = "Providers"
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
    let onAuthenticate: (String, String?, Bool) -> Void
    @AppStorage("refreshSeconds") private var refreshSeconds: Double = 60
    @AppStorage("limitWarnPct") private var limitWarnPct: Double = 90
    @AppStorage("notifyEnabled") private var notifyEnabled = true
    @AppStorage("binaryPath") private var binaryPath = ""
    @AppStorage("terminalApp") private var terminalApp = "auto"
    @AppStorage("menuIconStyle") private var menuIconStyle = "logo"
    @AppStorage(UpdateChannelSetting.defaultsKey) private var updateChannel =
        UpdateChannelSetting.stable.rawValue
    @State private var copyingCredentials = false
    @State private var credentialCopyStatus: String?
    @State private var showingResetSheet = false

    var body: some View {
        VStack(spacing: 0) {
            Picker("", selection: $state.section) {
                ForEach(PreferencesSection.allCases, id: \.self) { section in
                    Text(section.rawValue).tag(section)
                }
            }
            .pickerStyle(.segmented)
            .padding([.horizontal, .top], 16)

            switch state.section {
            case .general:
                Form {
                    generalSections
                }
                .formStyle(.grouped)
            case .providers:
                ProvidersPreferencesSection(store: store, onAuthenticate: onAuthenticate)
            case .harnesses:
                Form {
                    HarnessesPreferencesSection(store: store)
                }
                .formStyle(.grouped)
            }
        }
        .frame(width: 780, height: 680)
        .sheet(isPresented: $showingResetSheet) {
            ResetSettingsSheet(store: store)
        }
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
        Section("Updates") {
            Picker("Release channel", selection: $updateChannel) {
                ForEach(UpdateChannelSetting.allCases, id: \.rawValue) { channel in
                    Text(channel.label).tag(channel.rawValue)
                }
            }
            .onChange(of: updateChannel) {
                NotificationCenter.default.post(
                    name: UpdateChannelSetting.changedNotification, object: nil)
            }
            if UpdateChannelSetting.from(updateChannel) == .beta {
                Text("Beta builds are pre-release test versions. When the matching final release ships, the app updates to it automatically. The daemon channel is set separately: alex update --set-channel beta.")
                    .font(.system(size: 10))
                    .foregroundStyle(.secondary)
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
        Section("Reset Alexandria") {
            Text("Remove selected Alexandria data after reviewing a real-count dry run.")
                .font(.system(size: 10))
                .foregroundStyle(.secondary)
            Button("Reset…", role: .destructive) {
                showingResetSheet = true
            }
            .disabled(store.config == nil)
        }
        Section("Proxy credentials") {
            Text("Copy credentials for generic API clients, or a ready-to-edit command that mints a tagged run key. Both use the currently loaded local daemon settings.")
                .font(.system(size: 10))
                .foregroundStyle(.secondary)
            HStack(spacing: 8) {
                Button {
                    copyGenericCredentials()
                } label: {
                    Label("Copy environment block", systemImage: "doc.on.doc")
                }
                .disabled(copyingCredentials || store.config == nil)
                .help("Copy the same shell exports printed by alex credentials")

                Button {
                    copyRunKeyCurl()
                } label: {
                    Label("Copy run-key curl", systemImage: "terminal")
                }
                .disabled(store.config == nil)
                .help("Copy an editable POST /admin/run-keys example using this daemon")

                if copyingCredentials { ProgressView().controlSize(.small) }
            }
            if let credentialCopyStatus {
                Text(credentialCopyStatus)
                    .font(.system(size: 10))
                    .foregroundStyle(.secondary)
            }
        }
        Section("Feedback") {
            Link("Report a bug or request a feature", destination: Self.issuesURL)
            Text("Bugs, ideas, and feature requests all go to GitHub Issues.")
                .font(.system(size: 10))
                .foregroundStyle(.secondary)
        }
        Section("About") {
            LabeledContent("Built by") {
                Link("github.com/madhavajay", destination: Self.authorURL)
            }
            LabeledContent("Message me on X") {
                Link("@madhavajay", destination: Self.authorXURL)
            }
        }
    }

    static let issuesURL = URL(string: "https://github.com/madhavajay/alex/issues/new")!
    static let authorURL = URL(string: "https://github.com/madhavajay/")!
    static let authorXURL = URL(string: "https://x.com/madhavajay")!

    private func copyGenericCredentials() {
        guard let config = store.config else { return }
        copyingCredentials = true
        credentialCopyStatus = nil
        Task {
            do {
                let environment = try await AlexandriaClient(config: config)
                    .credentialsEnvironment()
                copyToPasteboard(environment)
                credentialCopyStatus = "Environment credential block copied."
            } catch {
                NSSound.beep()
                credentialCopyStatus = "Could not load credentials from the local daemon."
            }
            copyingCredentials = false
        }
    }

    private func copyRunKeyCurl() {
        guard let config = store.config else { return }
        let endpoint = config.baseURL.appendingPathComponent("admin/run-keys").absoluteString
        let body = #"{"run_id":"demo-run-001","tags":{"harness":"pi","project":"my-project"},"ttl_seconds":86400,"label":"example tagged run"}"#
        let command = """
        curl -sS -X POST \\
          -H \(shellQuote("x-api-key: \(config.localKey)")) \\
          -H 'content-type: application/json' \\
          --data \(shellQuote(body)) \\
          \(shellQuote(endpoint))
        """
        copyToPasteboard(command)
        credentialCopyStatus = "Editable run-key curl command copied."
    }

    private func copyToPasteboard(_ value: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(value, forType: .string)
    }

    private func shellQuote(_ value: String) -> String {
        "'" + value.replacingOccurrences(of: "'", with: "'\"'\"'") + "'"
    }
}

private struct ProvidersPreferencesSection: View {
    let store: SnapshotStore
    let onAuthenticate: (String, String?, Bool) -> Void
    @State private var providerToAdd: String?
    @State private var selectedProvider: String? = "openai"

    private var providers: [String] {
        Array(Set(["anthropic", "openai", "gemini", "xai", "openrouter"] + store.accounts.map(\.provider))).sorted {
            ProviderInfo.displayName($0) < ProviderInfo.displayName($1)
        }
    }

    private var usageByAccount: [String: AccountUsage] {
        Dictionary(uniqueKeysWithValues: (store.accountAnalytics?.byAccount ?? []).map { ($0.accountId, $0) })
    }

    /// OAuth without a supplied local name uses the compatible `default`
    /// account id. Codex is the exception: its automatic identity flow gives
    /// each upstream account a distinct generated local id.
    private var addableProviders: [String] {
        providers.filter {
            $0 == "openai" || !ProviderPresentation.hasAccount(for: $0, in: store.accounts)
        }
    }

    var body: some View {
        VStack(spacing: 0) {
            if ProviderPresentation.hasNoAccounts(store.accounts) {
                HStack(spacing: 12) {
                    VStack(alignment: .leading, spacing: 3) {
                        Text("Connect a Token Provider")
                            .font(.headline)
                        Text("Connect an account to see its usage and routing settings.")
                            .font(.system(size: 11))
                            .foregroundStyle(.secondary)
                    }
                    Spacer()
                    addProviderMenu(label: "Connect a Token Provider")
                }
                .padding(16)
                Divider()
            }

            HStack(spacing: 0) {
                VStack(spacing: 0) {
                    HStack {
                        Text("Providers")
                            .font(.headline)
                        Spacer()
                        addProviderMenu()
                    }
                    .padding(12)

                    List(selection: $selectedProvider) {
                        ForEach(providers, id: \.self) { provider in
                            HStack {
                                Label(ProviderInfo.displayName(provider), systemImage: "network")
                                Spacer()
                                Text("\(store.accounts.filter { $0.provider == provider }.count)")
                                    .foregroundStyle(.secondary)
                                    .monospacedDigit()
                            }
                            .tag(Optional(provider))
                        }
                    }
                    .listStyle(.sidebar)
                }
                .frame(minWidth: 190, idealWidth: 210, maxWidth: 230)

                Divider()

                Form {
                    if let provider = selectedProvider {
                        ProviderPreferencesDetail(
                            provider: provider,
                            store: store,
                            usageByAccount: usageByAccount,
                            onConnect: addAccount,
                            onAuthenticate: onAuthenticate)
                    } else {
                        ContentUnavailableView("Choose a provider", systemImage: "network")
                    }
                }
                .formStyle(.grouped)
            }
        }
        .sheet(
            isPresented: Binding(
                get: { providerToAdd != nil },
                set: { if !$0 { providerToAdd = nil } }
            )
        ) {
            if let provider = providerToAdd {
                if ProviderInfo.usesAPIKeySheet(provider) {
                    ProviderAPIKeySheet(provider: provider, store: store) {
                        providerToAdd = nil
                    }
                }
            }
        }
    }

    @ViewBuilder
    private func addProviderMenu(label: String? = nil) -> some View {
        Menu {
            ForEach(addableProviders, id: \.self) { provider in
                Button("Add \(ProviderInfo.displayName(provider))") {
                    addAccount(provider)
                }
            }
        } label: {
            if let label {
                Label(label, systemImage: "plus.circle")
            } else {
                Image(systemName: "plus.circle")
            }
        }
        .help("Add a token provider")
    }

    private func addAccount(_ provider: String) {
        if ProviderInfo.usesAPIKeySheet(provider) {
            providerToAdd = provider
        } else {
            // OAuth providers capture their account email during login. The
            // local account name remains the compatible default identifier.
            onAuthenticate(provider, nil, provider == "openai")
        }
    }
}

private struct ProviderPreferencesDetail: View {
    let provider: String
    let store: SnapshotStore
    let usageByAccount: [String: AccountUsage]
    let onConnect: (String) -> Void
    let onAuthenticate: (String, String?, Bool) -> Void

    private var accounts: [Account] { store.accounts.filter { $0.provider == provider } }
    private var routing: ProviderRoutingResponse? { store.routingByProvider[provider] }
    private var routingByAccount: [String: ProviderRoutingAccount] {
        Dictionary(uniqueKeysWithValues: (routing?.accounts ?? []).map { ($0.accountId, $0) })
    }

    var body: some View {
        switch ProviderPresentation.paneState(for: provider, accounts: accounts) {
        case .connectAccount:
            Section(ProviderInfo.displayName(provider)) {
                ContentUnavailableView(
                    "Connect \(ProviderInfo.displayName(provider))",
                    systemImage: "plus.circle",
                    description: Text("Usage and routing settings appear after this account is connected."))
                Button("Connect \(ProviderInfo.displayName(provider))") {
                    onConnect(provider)
                }
            }
        case .connected:
            if let analytics = store.accountAnalytics {
                Section("Usage · last 24 hours") {
                    SubscriptionUsageChart(usages: analytics.byAccount.filter { usage in
                        accounts.contains { $0.id == usage.accountId }
                    })
                    SubscriptionTokenTimeline(series: analytics.series.filter { point in
                        accounts.contains { $0.id == point.accountId }
                    }, accounts: accounts)
                }
            }

            Section(ProviderInfo.displayName(provider)) {
                Text("Accounts are separate credentials. Pause and routing eligibility are controlled independently.")
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
                ForEach(accounts) { account in
                    SubscriptionAccountRow(
                        account: account,
                        usage: usageByAccount[account.id],
                        routing: routingByAccount[account.id],
                        reservePct: routing?.reservePct ?? 10,
                        warnUsedPct: store.limitWarnPct,
                        store: store
                    ) {
                        onAuthenticate(account.provider, account.name, false)
                    }
                }
            }

            ProviderRoutingPreferencesSection(
                store: store, provider: provider, accounts: accounts, routing: routing)
        }
    }
}

private struct ResetSettingsSheet: View {
    let store: SnapshotStore
    @Environment(\.dismiss) private var dismiss
    @State private var selection = ResetSelection()
    @State private var plan: ResetResponse?
    @State private var busy = false
    @State private var error: String?
    @State private var confirmed = false
    @State private var applied = false

    private var hasSelection: Bool {
        selection.credentials || selection.settings || selection.traces
            || selection.harnesses || selection.cache
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("Reset Alexandria")
                .font(.title2.weight(.semibold))
            Text("Choose the data to remove. Alexandria first runs a dry run with real counts; you can only apply the reset after reviewing it.")
                .font(.system(size: 12))
                .foregroundStyle(.secondary)

            VStack(alignment: .leading, spacing: 8) {
                Toggle("Provider logins & credentials", isOn: $selection.credentials)
                Toggle("Settings", isOn: $selection.settings)
                Toggle("Trace history", isOn: $selection.traces)
                Toggle("Uninstall from all harnesses", isOn: $selection.harnesses)
                Text("This is the only option that edits files outside Alexandria (in ~/.claude, ~/.codex, and ~/.pi).")
                    .font(.system(size: 10))
                    .foregroundStyle(.orange)
                    .padding(.leading, 22)
                Toggle("Cached data", isOn: $selection.cache)
            }

            Text("Your update channel is preserved, so beta users stay on beta.")
                .font(.system(size: 11))
                .foregroundStyle(.secondary)

            if let plan {
                counts(plan.counts)
                Toggle(
                    "I understand the selected data will be permanently removed.",
                    isOn: $confirmed)
                    .font(.system(size: 11))
            } else {
                Text("Run the dry run to see the scale of this reset before it can be applied.")
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
            }

            if let error {
                Text(error)
                    .font(.system(size: 11))
                    .foregroundStyle(.red)
            }
            if applied {
                Text("Reset complete.")
                    .font(.system(size: 11))
                    .foregroundStyle(.green)
            }

            HStack {
                // Once the reset has been applied there is nothing left to cancel, and
                // labelling the only way out "Cancel" reads as "undo" -- so nobody dares
                // click it and the sheet looks frozen.
                Button(applied ? "Done" : "Cancel") { dismiss() }
                    .disabled(busy)
                    .keyboardShortcut(applied ? .defaultAction : .cancelAction)
                if !applied {
                    Spacer()
                    Button(plan == nil ? "Run dry run" : "Run dry run again") {
                        runDryRun()
                    }
                    .disabled(!hasSelection || busy)
                    if plan != nil {
                        Button("Reset now", role: .destructive) {
                            applyReset()
                        }
                        .disabled(!confirmed || busy)
                    }
                }
            }
        }
        .padding(24)
        .frame(width: 540)
        .onChange(of: selection) {
            plan = nil
            confirmed = false
            error = nil
        }
    }

    @ViewBuilder
    // Show ONLY what the current selection will actually destroy. The daemon's plan
    // reports a full inventory (it does not know what is ticked), so listing all of
    // it here put untouched data -- an entire trace history -- on a destructive
    // confirmation screen. A confirm dialog that lists data it will not delete is
    // indistinguishable from one that will.
    private func counts(_ counts: ResetCounts) -> some View {
        GroupBox("Will be permanently deleted") {
            Grid(alignment: .leading, horizontalSpacing: 24, verticalSpacing: 5) {
                if selection.credentials {
                    GridRow { Text("Accounts"); Text("\(counts.accounts)") }
                }
                if selection.traces {
                    GridRow { Text("Traces"); Text("\(counts.traces)") }
                    GridRow {
                        Text("Trace body data")
                        Text("\(ByteCountFormatter.string(fromByteCount: counts.bodies.bytes, countStyle: .file)) in \(counts.bodies.files) file\(counts.bodies.files == 1 ? "" : "s")")
                    }
                }
                if selection.harnesses {
                    GridRow { Text("Harnesses to disconnect"); Text("\(counts.connectedHarnesses)") }
                }
                if selection.settings {
                    GridRow { Text("Settings"); Text("restored to defaults") }
                }
                if selection.cache {
                    GridRow { Text("Cached data"); Text("pricing + prompt cache (rebuilt automatically)") }
                }
            }
            .font(.system(size: 11))
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    private func runDryRun() {
        guard let config = store.config else { return }
        busy = true
        error = nil
        confirmed = false
        Task {
            do {
                plan = try await AlexandriaClient(config: config).resetPlan(selection)
            } catch {
                self.error = "Could not get reset counts: \(error.localizedDescription)"
            }
            busy = false
        }
    }

    private func applyReset() {
        guard let config = store.config, plan != nil, confirmed else { return }
        busy = true
        error = nil
        Task {
            do {
                _ = try await AlexandriaClient(config: config).reset(selection)
                if selection.settings {
                    AppSettingsReset.clear()
                }
                applied = true
                await store.refresh()
            } catch {
                self.error = "Could not apply reset: \(error.localizedDescription)"
            }
            busy = false
        }
    }
}

private struct ProviderRoutingPreferencesSection: View {
    let store: SnapshotStore
    let provider: String
    let accounts: [Account]
    let routing: CodexRoutingResponse?
    @State private var strategy = CodexRoutingStrategy.resetFirst
    @State private var fallbackReservePct = 10.0
    @State private var allowMidThreadFailover = true
    @State private var draftAccounts: [CodexRoutingAccountUpdate] = []
    @State private var resetSelections: [String: CodexResetSelection] = [:]
    @State private var reserveBlocked: [String: Bool] = [:]
    @State private var savedSignature = ""
    @State private var busy = false
    @State private var error: String?

    private var routingKey: String {
        guard let routing else {
            return "unavailable|" + accounts.map(\.id).sorted().joined(separator: "|")
        }
        let accountKey = routing.accounts
            .sorted { $0.priority < $1.priority }
            .map {
                "\($0.accountId):\($0.eligible):\($0.priority):\($0.reservePct ?? routing.reservePct):\($0.reserveBlocked):\($0.resetSelection?.resetsAtS ?? 0):\($0.observedAtMs ?? 0)"
            }
            .joined(separator: "|")
        return "\(routing.strategy.rawValue)|\(routing.reservePct)|\(routing.allowMidThreadFailover)|\(accountKey)"
    }

    private var isDirty: Bool {
        !savedSignature.isEmpty && savedSignature != currentSignature
    }

    var body: some View {
        Section("\(ProviderInfo.displayName(provider)) routing") {
            if routing == nil {
                HStack(spacing: 8) {
                    Image(systemName: "exclamationmark.triangle")
                        .foregroundStyle(.orange)
                    Text("The running daemon does not expose per-account routing yet. Update and restart alex to configure it here.")
                }
                .font(.system(size: 11))
                .foregroundStyle(.secondary)
            } else {
                Text("Choose which connected accounts may receive requests. Pausing an account disables it more broadly and always overrides this setting.")
                    .font(.system(size: 10))
                    .foregroundStyle(.secondary)

                if accounts.isEmpty {
                    Text("This provider has no accounts yet. Its policy will apply when you add one.")
                        .font(.system(size: 10))
                        .foregroundStyle(.secondary)
                }

                Picker("Selection mode", selection: $strategy) {
                    ForEach(CodexRoutingStrategy.allCases, id: \.self) { value in
                        Text(value.displayName).tag(value)
                    }
                }
                .pickerStyle(.menu)

                Text(strategy.explanation)
                    .font(.system(size: 10))
                    .foregroundStyle(.secondary)

                Stepper(value: $fallbackReservePct, in: 0...100, step: 5) {
                    LabeledContent("Provider-wide reserve") {
                        Text(RoutingReserve.display(fallbackReservePct))
                            .font(.system(size: 10, design: .monospaced))
                            .monospacedDigit()
                    }
                }
                .controlSize(.small)
                .help("Headroom applied when an account has no separate reserve. 0% means reserve never blocks an account.")

                Text("Changing this updates accounts still using the previous provider value; change an account below to give it its own reserve.")
                    .font(.system(size: 10))
                    .foregroundStyle(.secondary)

                Toggle(
                    "Allow mid-thread account failover",
                    isOn: $allowMidThreadFailover)
                    .help(
                        "Retry an active thread on a different eligible account when its assigned account is unavailable")

                Text(allowMidThreadFailover
                    ? "If the assigned account hits an auth, rate-limit, or server failure, Alexandria may move that thread to another eligible account. This keeps work moving but can reduce prompt-cache reuse."
                    : "Auth, rate-limit, and server failures stay on the thread’s assigned account instead of retrying another one. Explicitly pausing, disabling, or removing that account can still reassign the thread.")
                    .font(.system(size: 10))
                    .foregroundStyle(.secondary)

                ForEach(Array(displayedAccounts.enumerated()), id: \.element.accountId) { index, draft in
                    VStack(alignment: .leading, spacing: 5) {
                        HStack {
                            Toggle("Use for requests", isOn: eligibleBinding(accountId: draft.accountId))
                                .toggleStyle(.switch)
                                .controlSize(.small)
                                .disabled(account(draft.accountId)?.paused == true || busy)
                                .help("Include this account in routing and failover")
                            Spacer()
                            Text(accountName(draft.accountId))
                                .font(.system(size: 11))
                                .lineLimit(1)
                            strategyControls(accountId: draft.accountId, displayedIndex: index)
                        }
                        Stepper(
                            value: reserveBinding(accountId: draft.accountId),
                            in: 0...100, step: 5
                        ) {
                            LabeledContent("Keep unused for this account") {
                                Text(RoutingReserve.display(draftReserve(draft.accountId)))
                                    .font(.system(size: 10, design: .monospaced))
                                    .monospacedDigit()
                            }
                        }
                        .controlSize(.small)
                        .help(
                            "Prefer another eligible account once this account reaches its remaining-quota reserve. 0% never blocks it.")
                        if account(draft.accountId)?.paused == true {
                            Text("This account is paused, so it cannot receive proxy traffic even while selected here.")
                                .font(.system(size: 10))
                                .foregroundStyle(.orange)
                        }
                    }
                    .padding(.vertical, 2)
                }

                if !draftAccounts.contains(where: \.eligible) {
                    Label(
                        "No account is selected. Requests for this provider will fail until at least one account is enabled.",
                        systemImage: "exclamationmark.triangle.fill")
                        .font(.system(size: 10))
                        .foregroundStyle(.red)
                }

                HStack {
                    Button("Save routing") { save() }
                        .disabled(busy || !isDirty)
                    if busy { ProgressView().controlSize(.small) }
                    if !busy && !isDirty && error == nil {
                        Text("Saved")
                            .font(.system(size: 10))
                            .foregroundStyle(.secondary)
                    }
                }

                if let error {
                    Text(error)
                        .font(.system(size: 10))
                        .foregroundStyle(.red)
                }
            }
        }
        .task(id: routingKey) {
            loadRouting()
        }
        .onChange(of: fallbackReservePct) { oldValue, newValue in
            // The daemon snapshot exposes effective values, not an override bit.
            // Treat values equal to the former provider reserve as inherited.
            draftAccounts = draftAccounts.map { account in
                guard account.reservePct == oldValue else { return account }
                return CodexRoutingAccountUpdate(
                    accountId: account.accountId,
                    eligible: account.eligible,
                    priority: account.priority,
                    reservePct: newValue)
            }
        }
    }

    private var currentSignature: String {
        let accountKey = draftAccounts.enumerated().map { index, account in
            "\(account.accountId):\(account.eligible):\(index):\(account.reservePct ?? fallbackReservePct)"
        }.joined(separator: "|")
        return "\(strategy.rawValue)|\(fallbackReservePct)|\(allowMidThreadFailover)|\(accountKey)"
    }

    private var displayedAccounts: [CodexRoutingAccountUpdate] {
        guard strategy == .resetFirst else { return draftAccounts }
        return draftAccounts.sorted { lhs, rhs in
            let leftUsable = isUsable(lhs)
            let rightUsable = isUsable(rhs)
            if leftUsable != rightUsable { return leftUsable && !rightUsable }
            let leftBlocked = reserveBlocked[lhs.accountId] ?? false
            let rightBlocked = reserveBlocked[rhs.accountId] ?? false
            if leftBlocked != rightBlocked { return !leftBlocked && rightBlocked }
            let left = resetSelections[lhs.accountId]?.resetsAtS ?? Int64.max
            let right = resetSelections[rhs.accountId]?.resetsAtS ?? Int64.max
            if left != right { return left < right }
            return lhs.priority < rhs.priority
        }
    }

    private func account(_ id: String) -> Account? {
        accounts.first { $0.id == id }
    }

    private func eligibleBinding(accountId: String) -> Binding<Bool> {
        Binding {
            draftAccounts.first { $0.accountId == accountId }?.eligible ?? false
        } set: { value in
            guard let index = draftAccounts.firstIndex(where: { $0.accountId == accountId }) else { return }
            let item = draftAccounts[index]
            draftAccounts[index] = CodexRoutingAccountUpdate(
                accountId: item.accountId,
                eligible: value,
                priority: item.priority,
                reservePct: item.reservePct ?? fallbackReservePct)
        }
    }

    private func reserveBinding(accountId: String) -> Binding<Double> {
        Binding {
            draftReserve(accountId)
        } set: { value in
            guard let index = draftAccounts.firstIndex(where: { $0.accountId == accountId })
            else { return }
            let item = draftAccounts[index]
            draftAccounts[index] = CodexRoutingAccountUpdate(
                accountId: item.accountId,
                eligible: item.eligible,
                priority: item.priority,
                reservePct: value)
        }
    }

    private func draftReserve(_ accountId: String) -> Double {
        draftAccounts.first { $0.accountId == accountId }?.reservePct ?? fallbackReservePct
    }

    private func isUsable(_ draft: CodexRoutingAccountUpdate) -> Bool {
        draft.eligible && account(draft.accountId)?.paused != true
    }

    private func loadRouting() {
        guard let routing else {
            draftAccounts = []
            savedSignature = ""
            return
        }
        strategy = routing.strategy
        fallbackReservePct = routing.reservePct
        allowMidThreadFailover = routing.allowMidThreadFailover
        resetSelections = Dictionary(uniqueKeysWithValues: routing.accounts.compactMap {
            guard let selection = $0.resetSelection else { return nil }
            return ($0.accountId, selection)
        })
        reserveBlocked = Dictionary(uniqueKeysWithValues: routing.accounts.map {
            ($0.accountId, $0.reserveBlocked)
        })
        let responseAccounts = routing.accounts.sorted { $0.priority < $1.priority }
        var draft = responseAccounts.map {
            CodexRoutingAccountUpdate(
                accountId: $0.accountId,
                eligible: $0.eligible,
                priority: $0.priority,
                reservePct: $0.reservePct ?? routing.reservePct)
        }
        for account in accounts where !draft.contains(where: { $0.accountId == account.id }) {
            draft.append(CodexRoutingAccountUpdate(
                accountId: account.id,
                eligible: !account.paused,
                priority: draft.count,
                reservePct: routing.reservePct))
        }
        draftAccounts = normalized(draft)
        error = nil
        savedSignature = currentSignature
    }

    private func move(_ index: Int, by offset: Int) {
        let destination = index + offset
        guard draftAccounts.indices.contains(index), draftAccounts.indices.contains(destination) else { return }
        draftAccounts.swapAt(index, destination)
        draftAccounts = normalized(draftAccounts)
    }

    private func normalized(_ values: [CodexRoutingAccountUpdate]) -> [CodexRoutingAccountUpdate] {
        values.enumerated().map { index, value in
            CodexRoutingAccountUpdate(
                accountId: value.accountId,
                eligible: value.eligible,
                priority: index,
                reservePct: value.reservePct ?? fallbackReservePct)
        }
    }

    @ViewBuilder
    private func strategyControls(accountId: String, displayedIndex: Int) -> some View {
        switch strategy {
        case .priority:
            let index = draftAccounts.firstIndex { $0.accountId == accountId } ?? displayedIndex
            Text("#\(index + 1)")
                .font(.system(size: 10, design: .monospaced))
                .foregroundStyle(.secondary)
            Button { move(index, by: -1) } label: {
                Image(systemName: "arrow.up")
            }
            .buttonStyle(.borderless)
            .disabled(index == 0 || busy)
            .help("Move earlier in priority order")
            Button { move(index, by: 1) } label: {
                Image(systemName: "arrow.down")
            }
            .buttonStyle(.borderless)
            .disabled(index == draftAccounts.count - 1 || busy)
            .help("Move later in priority order")
        case .roundRobin:
            Label("alternates", systemImage: "arrow.triangle.2.circlepath")
                .font(.system(size: 10))
                .foregroundStyle(.secondary)
                .help("New threads cycle across enabled subscriptions")
        case .resetFirst:
            Label(resetLabel(accountId: accountId, index: displayedIndex), systemImage: "clock.arrow.circlepath")
                .font(.system(size: 10))
                .foregroundStyle(.secondary)
                .help(resetHelp(accountId))
        }
    }

    private func accountName(_ id: String) -> String {
        account(id)?.email
            ?? account(id)?.description
            ?? account(id)?.name
            ?? id
    }

    private func resetLabel(accountId: String, index: Int) -> String {
        guard let selection = resetSelections[accountId] else { return "reset unavailable" }
        let window = selection.window ?? "window"
        let draft = draftAccounts.first { $0.accountId == accountId }
        let position = draft.map(isUsable) == false ? "excluded" : (index == 0 ? "sooner" : "later")
        let reserve = reserveBlocked[accountId] == true ? "reserve reached · " : ""
        let reset = selection.resetsDate.formatted(date: .abbreviated, time: .shortened)
        return "\(reserve)\(window) · resets \(reset) · \(position)"
    }

    private func resetHelp(_ accountId: String) -> String {
        guard let selection = resetSelections[accountId] else {
            return "Waiting for this account's reset data"
        }
        return "Backend selected the \(selection.window ?? "active") window at \(selection.usedPct.formatted(.number.precision(.fractionLength(0))))% used; exact reset: \(selection.resetsDate.formatted(date: .abbreviated, time: .standard))"
    }

    private func save() {
        guard let config = store.config else { return }
        busy = true
        error = nil
        let update = ProviderRoutingUpdate(
            strategy: strategy,
            reservePct: fallbackReservePct,
            allowMidThreadFailover: allowMidThreadFailover,
            accounts: normalized(draftAccounts))
        Task {
            do {
                try await AlexandriaClient(config: config).updateRouting(provider: provider, update)
                await store.refresh()
            } catch {
                self.error = error.localizedDescription
            }
            busy = false
        }
    }
}

private extension CodexRoutingStrategy {
    var displayName: String {
        switch self {
        case .resetFirst: "Reset first"
        case .priority: "Priority"
        case .roundRobin: "Round robin"
        }
    }

    var explanation: String {
        switch self {
        case .resetFirst:
            "Assign each new session to an eligible account whose active limit resets sooner, while respecting the reserve."
        case .priority:
            "Assign each new session to the first eligible account below that remains above the reserve."
        case .roundRobin:
            "Alternate new sessions across eligible accounts, skipping accounts that have reached the reserve."
        }
    }
}

private struct CodexLimitWindowsView: View {
    let routing: CodexRoutingAccount?
    let reservePct: Double
    let warnUsedPct: Double

    var body: some View {
        if let routing, !routing.windows.isEmpty {
            VStack(alignment: .leading, spacing: 5) {
                ForEach(Array(routing.windows.enumerated()), id: \.offset) { _, window in
                    HStack(spacing: 8) {
                        Text(window.window)
                            .font(.system(size: 10, weight: .medium, design: .monospaced))
                            .frame(width: 24, alignment: .leading)
                        if let remaining = window.remainingPct {
                            ProgressView(value: remaining, total: 100)
                                .progressViewStyle(.linear)
                                .tint(barColor(window))
                                .frame(width: 90)
                            Text("\(remaining.formatted(.number.precision(.fractionLength(0))))% remaining")
                                .font(.system(size: 10))
                                .monospacedDigit()
                        } else {
                            Text("usage unavailable")
                                .font(.system(size: 10))
                                .foregroundStyle(.secondary)
                        }
                        Spacer()
                        if let reset = window.resetsDate {
                            Text("resets \(relative(reset))")
                                .font(.system(size: 10))
                                .foregroundStyle(.secondary)
                        }
                    }
                }
                if let observed = routing.observedAtMs {
                    Text("Quota observed \(relative(Date(timeIntervalSince1970: Double(observed) / 1_000)))")
                        .font(.system(size: 9))
                        .foregroundStyle(.tertiary)
                }
            }
        } else {
            Text("Codex quota: waiting for limit data from this account’s first proxied response.")
                .font(.system(size: 10))
                .foregroundStyle(.secondary)
        }
    }

    private func relative(_ date: Date) -> String {
        let formatter = RelativeDateTimeFormatter()
        formatter.unitsStyle = .abbreviated
        return formatter.localizedString(for: date, relativeTo: Date())
    }

    private func barColor(_ window: LimitWindow) -> Color {
        switch window.remainingSeverity(warnUsedPct: warnUsedPct) {
        case .critical: .red
        case .warning: .orange
        case .healthy, .none: .green
        }
    }
}

private struct SubscriptionAccountRow: View {
    let account: Account
    let usage: AccountUsage?
    let routing: CodexRoutingAccount?
    let reservePct: Double
    let warnUsedPct: Double
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
                Text(account.email ?? account.description ?? account.label ?? account.name)
                    .font(.system(size: 11))
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
            Text("Email: \(account.email ?? "not supplied by provider")")
                .font(.system(size: 10))
                .foregroundStyle(.secondary)
            if let usage {
                Text("Last 24h: \(usage.requests) requests · \(TraceFormat.tokens(usage.inputTokens + usage.outputTokens)) tokens · \(usage.errors ?? 0) errors")
                    .font(.system(size: 10))
                    .foregroundStyle(.secondary)
            }
            if account.provider == "openai" {
                CodexLimitWindowsView(
                    routing: routing,
                    reservePct: reservePct,
                    warnUsedPct: warnUsedPct)
            }
            HStack(spacing: 8) {
                Button(account.paused ? "Resume account" : "Pause account") { setPaused(!account.paused) }
                    .controlSize(.small)
                    .disabled(busy)
                if account.provider == "openrouter" {
                    Text("Use the sidebar + to replace the API key")
                        .font(.system(size: 10))
                        .foregroundStyle(.secondary)
                } else {
                    Button("Re-authenticate") { reauthenticate() }
                        .controlSize(.small)
                }
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

private struct SubscriptionTokenTimeline: View {
    let series: [AccountUsageBucket]
    let accounts: [Account]

    private var tokenSeries: [AccountUsageBucket] {
        series
            .filter { $0.inputTokens + $0.outputTokens > 0 }
            .sorted { lhs, rhs in
                if lhs.bucketMs == rhs.bucketMs { return lhs.accountId < rhs.accountId }
                return lhs.bucketMs < rhs.bucketMs
            }
    }

    var body: some View {
        if tokenSeries.isEmpty {
            Text("No per-account token activity to graph yet.")
                .font(.system(size: 10))
                .foregroundStyle(.secondary)
        } else {
            VStack(alignment: .leading, spacing: 4) {
                Text("Tokens routed over time")
                    .font(.system(size: 10, weight: .medium))
                Chart(tokenSeries) { point in
                    let date = Date(timeIntervalSince1970: Double(point.bucketMs) / 1_000)
                    let tokens = point.inputTokens + point.outputTokens
                    LineMark(
                        x: .value("Time", date),
                        y: .value("Tokens", tokens),
                        series: .value("Account", point.accountId))
                        .foregroundStyle(by: .value("Account", accountLabel(point.accountId)))
                        .symbol(by: .value("Account", accountLabel(point.accountId)))
                        .interpolationMethod(.linear)
                }
                .chartLegend(position: .bottom, alignment: .leading, spacing: 6)
                .frame(height: 145)
            }
        }
    }

    private func accountLabel(_ id: String) -> String {
        guard let account = accounts.first(where: { $0.id == id }) else { return id }
        return "\(ProviderInfo.displayName(account.provider)) · \(account.name)"
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

private struct ProviderAPIKeySheet: View {
    let provider: String
    let store: SnapshotStore
    let onDone: () -> Void
    @State private var key = ""
    @State private var httpReferer = ""
    @State private var xTitle = ""
    @State private var saving = false
    @State private var error: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("Add \(ProviderInfo.displayName(provider)) API key")
                .font(.title3.bold())
            Text("OpenRouter uses a long-lived API key, not OAuth. The key is sent only to your local Alexandria daemon for encrypted vault storage.")
                .font(.system(size: 12))
                .foregroundStyle(.secondary)
            SecureField("API key", text: $key)
                .textFieldStyle(.roundedBorder)
            TextField("HTTP-Referer (optional)", text: $httpReferer)
                .textFieldStyle(.roundedBorder)
            TextField("X-Title (optional)", text: $xTitle)
                .textFieldStyle(.roundedBorder)
            if let error {
                Text(error)
                    .font(.system(size: 10))
                    .foregroundStyle(.red)
            }
            HStack {
                Spacer()
                Button("Cancel", action: onDone)
                    .keyboardShortcut(.cancelAction)
                    .disabled(saving)
                Button("Save key") { save() }
                    .keyboardShortcut(.defaultAction)
                    .disabled(saving || key.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                if saving { ProgressView().controlSize(.small) }
            }
        }
        .padding(20)
        .frame(width: 440)
    }

    private func save() {
        guard let config = store.config else { return }
        saving = true
        error = nil
        let cleanKey = key.trimmingCharacters(in: .whitespacesAndNewlines)
        let cleanReferer = httpReferer.trimmingCharacters(in: .whitespacesAndNewlines)
        let cleanTitle = xTitle.trimmingCharacters(in: .whitespacesAndNewlines)
        Task {
            do {
                try await AlexandriaClient(config: config).setOpenRouterKey(
                    cleanKey,
                    httpReferer: cleanReferer.isEmpty ? nil : cleanReferer,
                    xTitle: cleanTitle.isEmpty ? nil : cleanTitle)
                await store.refresh()
                onDone()
            } catch {
                self.error = error.localizedDescription
            }
            saving = false
        }
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
    @State private var routeUpdating = false
    @State private var toolCaptureUpdating = false

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
                if harness.name == "codex", harness.connected {
                    Toggle(
                        "Alexandria default",
                        isOn: Binding(
                            get: { harness.defaultRoute == "alex" },
                            set: { setCodexDefaultRoute($0 ? "alex" : "openai") }))
                        .toggleStyle(.switch)
                        .controlSize(.small)
                        .disabled(routeUpdating || actionModel != nil)
                        .help(
                            "Plain `codex` follows this setting. Explicit --profile openai and --profile alex commands always remain available."
                        )
                }
                if harness.name == "pi", harness.connected {
                    Toggle("Capture tools", isOn: Binding(
                        get: { harness.toolCaptureEnabled ?? false },
                        set: { setToolCapture($0) }))
                        .toggleStyle(.switch).controlSize(.small)
                        .disabled(toolCaptureUpdating || actionModel != nil)
                        .help("Opt in to storing Pi tool arguments and results locally. Secrets are redacted before storage.")
                }
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

    private func setCodexDefaultRoute(_ route: String) {
        guard let config = store.config else { return }
        routeUpdating = true
        error = nil
        Task {
            do {
                _ = try await AlexandriaClient(config: config).setCodexDefaultRoute(route)
                await store.refresh()
            } catch {
                self.error = error.localizedDescription
            }
            routeUpdating = false
        }
    }

    private func setToolCapture(_ enabled: Bool) {
        guard let config = store.config else { return }
        toolCaptureUpdating = true; error = nil
        Task {
            do { try await AlexandriaClient(config: config).setHarnessToolCapture(harness.name, enabled: enabled); await store.refresh() }
            catch { self.error = error.localizedDescription }
            toolCaptureUpdating = false
        }
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
                onAuthenticate: { [weak self] provider, name, autoIdentity in
                    guard let self else { return }
                    self.authWindows.show(
                        provider: provider, accountName: name, autoIdentity: autoIdentity,
                        store: self.store)
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
