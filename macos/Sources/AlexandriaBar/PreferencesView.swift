import AppKit
import Charts
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
        .frame(width: 620, height: 680)
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

private struct SubscriptionsPreferencesSection: View {
    let store: SnapshotStore
    let onAuthenticate: (String, String?, Bool) -> Void
    @State private var providerToAdd: String?

    private let providers = ["anthropic", "openai", "gemini", "xai"]

    private var usageByAccount: [String: AccountUsage] {
        Dictionary(uniqueKeysWithValues: (store.accountAnalytics?.byAccount ?? []).map { ($0.accountId, $0) })
    }

    private var codexAccounts: [Account] {
        store.accounts.filter { $0.provider == "openai" }
    }

    private var routingByAccount: [String: CodexRoutingAccount] {
        Dictionary(uniqueKeysWithValues: (store.codexRouting?.accounts ?? []).map { ($0.accountId, $0) })
    }

    var body: some View {
        Section("Subscriptions") {
            Text("Each account is a separate subscription or API credential. Account pause and proxy selection are controlled separately.")
                .font(.system(size: 11))
                .foregroundStyle(.secondary)

            if store.accounts.isEmpty {
                Text("No accounts found. Add an account to start routing requests.")
                    .foregroundStyle(.secondary)
            } else {
                ForEach(store.accounts) { account in
                    SubscriptionAccountRow(
                        account: account,
                        usage: usageByAccount[account.id],
                        routing: routingByAccount[account.id],
                        reservePct: store.codexRouting?.reservePct ?? 10,
                        warnUsedPct: store.limitWarnPct,
                        store: store
                    ) {
                        onAuthenticate(account.provider, account.name, false)
                    }
                }
            }

            Button {
                onAuthenticate("openai", nil, true)
            } label: {
                Label("Add another Codex account", systemImage: "person.badge.plus")
            }
        }


        CodexRoutingPreferencesSection(
            store: store,
            accounts: codexAccounts,
            routing: store.codexRouting)

        if let analytics = store.accountAnalytics {
            Section("Usage · last 24 hours") {
                SubscriptionUsageChart(usages: analytics.byAccount)
                SubscriptionTokenTimeline(series: analytics.series, accounts: store.accounts)
                ForEach(analytics.byAccount) { usage in
                    HStack {
                        Text(usage.accountId)
                            .font(.system(size: 10, design: .monospaced))
                            .lineLimit(1)
                        Spacer()
                        Text(usageSummary(usage))
                            .font(.system(size: 10))
                            .foregroundStyle(.secondary)
                    }
                }
            }
        }

        Section("Add subscription") {
            ForEach(providers.filter { $0 != "openai" }, id: \.self) { provider in
                Button {
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
                    onAuthenticate(provider, name, false)
                } onCancel: {
                    providerToAdd = nil
                }
            }
        }
    }

    private func usageSummary(_ usage: AccountUsage) -> String {
        var pieces = [
            "\(usage.requests) requests",
            "\(TraceFormat.tokens(usage.inputTokens + usage.outputTokens)) tokens",
            String(format: "$%.4f", usage.costUsd),
        ]
        if let errors = usage.errors, errors > 0 {
            pieces.append("\(errors) errors")
        }
        return pieces.joined(separator: " · ")
    }
}

private struct CodexRoutingPreferencesSection: View {
    let store: SnapshotStore
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
        Section("Codex proxy routing") {
            if accounts.isEmpty {
                Text("Add a Codex account above to configure proxy routing.")
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
            } else if routing == nil {
                HStack(spacing: 8) {
                    Image(systemName: "exclamationmark.triangle")
                        .foregroundStyle(.orange)
                    Text("The running daemon does not expose per-account Codex routing yet. Update and restart alex to configure it here.")
                }
                .font(.system(size: 11))
                .foregroundStyle(.secondary)
            } else {
                Text("Choose which connected accounts may receive Codex requests. Pausing an account disables it more broadly and always overrides this setting.")
                    .font(.system(size: 10))
                    .foregroundStyle(.secondary)

                Picker("Selection mode", selection: $strategy) {
                    ForEach(CodexRoutingStrategy.allCases, id: \.self) { value in
                        Text(value.displayName).tag(value)
                    }
                }
                .pickerStyle(.menu)

                Text(strategy.explanation)
                    .font(.system(size: 10))
                    .foregroundStyle(.secondary)

                Toggle(
                    "Allow mid-thread subscription failover",
                    isOn: $allowMidThreadFailover)
                    .help(
                        "Retry an active thread on a different eligible Codex subscription when its assigned account is unavailable")

                Text(allowMidThreadFailover
                    ? "If the assigned subscription hits an auth, rate-limit, or server failure, Alexandria may move that thread to another eligible subscription. This keeps work moving but can reduce prompt-cache reuse."
                    : "Auth, rate-limit, and server failures stay on the thread’s assigned subscription instead of retrying another one. Explicitly pausing, disabling, or removing that account can still reassign the thread.")
                    .font(.system(size: 10))
                    .foregroundStyle(.secondary)

                ForEach(Array(displayedAccounts.enumerated()), id: \.element.accountId) { index, draft in
                    VStack(alignment: .leading, spacing: 5) {
                        HStack {
                            Toggle("Use for Codex requests", isOn: eligibleBinding(accountId: draft.accountId))
                                .toggleStyle(.switch)
                                .controlSize(.small)
                                .disabled(account(draft.accountId)?.paused == true || busy)
                                .help("Include this subscription in Codex request routing and failover")
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
                                Text("\(Int(draftReserve(draft.accountId)))% remaining")
                                    .font(.system(size: 10, design: .monospaced))
                                    .monospacedDigit()
                            }
                        }
                        .controlSize(.small)
                        .help(
                            "Prefer another eligible Codex subscription once this account reaches its remaining-quota reserve")
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
                        "No Codex account is selected. Codex proxy requests will fail until at least one account is enabled.",
                        systemImage: "exclamationmark.triangle.fill")
                        .font(.system(size: 10))
                        .foregroundStyle(.red)
                }

                HStack {
                    Button("Save proxy routing") { save() }
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
    }

    private var currentSignature: String {
        let accountKey = draftAccounts.enumerated().map { index, account in
            "\(account.accountId):\(account.eligible):\(index):\(account.reservePct ?? fallbackReservePct)"
        }.joined(separator: "|")
        return "\(strategy.rawValue)|\(allowMidThreadFailover)|\(accountKey)"
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
            return "Waiting for this subscription's Codex reset data"
        }
        return "Backend selected the \(selection.window ?? "active") window at \(selection.usedPct.formatted(.number.precision(.fractionLength(0))))% used; exact reset: \(selection.resetsDate.formatted(date: .abbreviated, time: .standard))"
    }

    private func save() {
        guard let config = store.config else { return }
        busy = true
        error = nil
        let update = CodexRoutingUpdate(
            strategy: strategy,
            reservePct: fallbackReservePct,
            allowMidThreadFailover: allowMidThreadFailover,
            accounts: normalized(draftAccounts))
        Task {
            do {
                try await AlexandriaClient(config: config).updateCodexRouting(update)
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
