import AppKit
import SwiftUI
import AlexCore

/// The Preferences → General tab: launch at login, refresh cadence, alerts,
/// updates, terminal, daemon, network exposure, reset, feedback, and about.
/// Restyled per ui/Create Settings Page (§2.3 of the
/// design spec): section labels + hairline-divided setting rows.
struct GeneralPreferencesPane: View {
    let store: SnapshotStore
    let onRunOnboarding: () -> Void
    let onResetCompleted: () -> Void
    @AppStorage("refreshSeconds") private var refreshSeconds: Double = 60
    @AppStorage("limitWarnPct") private var limitWarnPct: Double = 90
    @AppStorage("notifyEnabled") private var notifyEnabled = true
    @AppStorage("binaryPath") private var binaryPath = ""
    @AppStorage("terminalApp") private var terminalApp = "auto"
    // B2: default the picker to the channel this build actually follows. A
    // pre-release build defaults to beta, so the UI never shows "Stable" while
    // the updater is (correctly) checking the beta appcast. An explicit user
    // choice persists and overrides this default.
    @AppStorage(UpdateChannelSetting.defaultsKey) private var updateChannel =
        UpdateChannelSetting.defaultChannel(forRunningVersion: PreferencesView.appVersion).rawValue
    @State private var selectedChannel =
        UpdateChannelSetting.defaultChannel(forRunningVersion: PreferencesView.appVersion).rawValue
    @State private var channelScope: UpdateChannelScope = .both
    @State private var showChannelScope = false
    /// The daemon's live channel; nil while unknown or the daemon is unreachable.
    @State private var daemonChannel: String?
    @State private var applyingChannel = false
    @State private var channelStatus: String?
    @State private var applyingDaemonUpdate = false
    @State private var daemonUpdateTarget: String?
    @State private var daemonUpdateStatus: String?
    @State private var launchAtLogin = false
    @State private var showingResetSheet = false
    @State private var networkExposure = "loopback"
    @State private var selectedInterfaceAddress = ""
    @State private var networkInterfaces: [NetworkInterfaceAddress] = []
    @State private var savingNetworkExposure = false
    @State private var networkExposureStatus: String?

    var body: some View {
        VStack(spacing: 0) {
            paneHeader
            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    generalSections
                }
                .padding(.horizontal, 24)
                .padding(.bottom, 20)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        .sheet(isPresented: $showingResetSheet) {
            ResetSettingsSheet(store: store, onResetCompleted: onResetCompleted)
        }
        .onAppear {
            launchAtLogin = LaunchAtLogin.isEnabled
            loadNetworkExposure()
            selectedChannel = updateChannel
            loadDaemonChannel()
        }
        .onChange(of: store.config?.host) {
            loadNetworkExposure()
            loadDaemonChannel()
        }
        .onChange(of: store.daemonUpdate) { _, update in
            reconcileDaemonUpdate(update)
        }
    }

    private var paneHeader: some View {
        VStack(alignment: .leading, spacing: 0) {
            VStack(alignment: .leading, spacing: 1) {
                Text("General")
                    .font(AlexTheme.Fonts.panelTitle)
                    .foregroundStyle(AlexTheme.Colors.foreground)
                Text("App behavior and preferences")
                    .font(.system(size: 12))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
            }
            .padding(.horizontal, 24)
            .padding(.top, 16)
            .padding(.bottom, 12)
            .frame(maxWidth: .infinity, alignment: .leading)
            Rectangle()
                .fill(AlexTheme.Colors.overlay(0.06))
                .frame(height: 1)
                .padding(.horizontal, 24)
        }
    }

    @ViewBuilder
    private var generalSections: some View {
        SettingRow(
            label: "Run Onboarding Tutorial",
            hint: "Walk from your first provider through traces, credentials, notifications, and failover"
        ) {
            PillButton(
                title: "Run Tutorial", variant: .solidAccent,
                systemImage: "sparkles", action: onRunOnboarding)
        }
        .padding(.top, 12)
        RowDivider()

        SectionLabel(text: "System")
            .settingsSectionSpacing()
        if LaunchAtLogin.available {
            SettingRow(label: "Launch at login", hint: "Start Alex when you log in") {
                Toggle("", isOn: $launchAtLogin)
                    .settingsSwitch()
                    .onChange(of: launchAtLogin) {
                        if launchAtLogin != LaunchAtLogin.isEnabled {
                            LaunchAtLogin.toggle()
                        }
                    }
            }
            RowDivider()
        }
        SettingRow(label: "Poll interval", hint: "How often daemon data is refreshed") {
            Picker("", selection: $refreshSeconds) {
                Text("30 seconds").tag(30.0)
                Text("1 minute").tag(60.0)
                Text("5 minutes").tag(300.0)
                Text("15 minutes").tag(900.0)
            }
            .settingsPicker()
        }
        RowDivider()
        SettingRow(
            label: "Open commands in",
            hint: TerminalLauncher.resolved == .ghostty
                ? "Ghostty can't accept commands while already running — Terminal is used instead in that case."
                : nil
        ) {
            Picker("", selection: $terminalApp) {
                Text("Auto (\(TerminalLauncher.resolved.displayName))").tag("auto")
                ForEach(TerminalLauncher.installedApps, id: \.rawValue) { app in
                    Text(app.displayName).tag(app.rawValue)
                }
            }
            .settingsPicker()
        }

        SectionLabel(text: "Updates")
            .settingsSectionSpacing()
        SettingRow(label: "Release channel") {
            HStack(spacing: 8) {
                if applyingChannel { ProgressView().controlSize(.small) }
                Picker("", selection: channelBinding) {
                    ForEach(UpdateChannelSetting.allCases, id: \.rawValue) { channel in
                        Text(channel.label).tag(channel.rawValue)
                    }
                }
                .settingsPicker()
                .disabled(applyingChannel)
            }
        }
        SettingCaption(channelStateSummary)
        if let daemonChannel, daemonChannel != updateChannel {
            PillButton(
                title: "Sync daemon to \(UpdateChannelSetting.from(updateChannel).label)",
                variant: .bordered, isEnabled: !applyingChannel && store.config != nil
            ) {
                applyDaemonChannel(updateChannel)
            }
            .help("The app and daemon are on different channels. This sets the daemon to match the app.")
            .padding(.vertical, 6)
        }
        if showChannelScope {
            SettingRow(
                label: "Apply to",
                hint: "Choose which side this channel change updates."
            ) {
                Picker("", selection: $channelScope) {
                    ForEach(UpdateChannelScope.allCases, id: \.rawValue) { scope in
                        Text(scope.label).tag(scope)
                    }
                }
                .pickerStyle(.segmented)
                .frame(maxWidth: 220)
                .disabled(applyingChannel)
            }
        }
        Button(showChannelScope ? "Use one control for both" : "Set app and daemon individually") {
            showChannelScope.toggle()
            if !showChannelScope { channelScope = .both }
        }
        .buttonStyle(.link)
        .font(.system(size: 11))
        .padding(.vertical, 2)
        if let channelStatus {
            SettingCaption(channelStatus)
        }
        if let update = store.daemonUpdate, update.updateAvailable, let latest = update.latest {
            RowDivider()
            SettingRow(
                label: "Daemon update",
                hint: "\(update.current) → \(latest)"
            ) {
                PillButton(
                    title: applyingDaemonUpdate ? "Updating…" : "Update",
                    variant: .solidOrange,
                    isEnabled: !applyingDaemonUpdate && store.config != nil
                ) {
                    applyDaemonUpdate(current: update.current, latest: latest)
                }
            }
        }
        if let daemonUpdateStatus {
            SettingCaption(daemonUpdateStatus)
        }
        if UpdateChannelSetting.from(selectedChannel) == .beta {
            SettingCaption(
                "Beta builds are pre-release test versions. When the matching final release ships, the app updates to it automatically. Picking a channel here sets both the app and the daemon by default, so `alex update` and the daemon offer the matching build.")
        }

        SectionLabel(text: "Notifications")
            .settingsSectionSpacing()
        SettingRow(label: "Show notifications") {
            Toggle("", isOn: $notifyEnabled)
                .settingsSwitch()
        }
        RowDivider()
        SettingRow(label: "Warn when a limit window reaches") {
            Picker("", selection: $limitWarnPct) {
                Text("75%").tag(75.0)
                Text("80%").tag(80.0)
                Text("90%").tag(90.0)
                Text("95%").tag(95.0)
            }
            .settingsPicker()
        }

        SectionLabel(text: "Daemon")
            .settingsSectionSpacing()
        SettingRow(label: "Binary path", hint: "Blank = auto-discover the Alex CLI") {
            TextField("auto", text: $binaryPath)
                .settingsField()
        }
        RowDivider()
        SettingRow(label: "Config") {
            Text(DaemonDiscovery.configPath.path)
                .font(AlexTheme.Fonts.metaMicro)
                .foregroundStyle(AlexTheme.Colors.textTertiary)
                .lineLimit(1)
                .truncationMode(.middle)
                .textSelection(.enabled)
                .frame(maxWidth: 320, alignment: .trailing)
        }

        networkExposureSection

        SectionLabel(text: "Reset")
            .settingsSectionSpacing()
        SettingRow(
            label: "Reset Alex",
            hint: "Remove selected Alex data after reviewing a real-count dry run."
        ) {
            PillButton(
                title: "Reset…", variant: .danger, horizontalPadding: 12,
                verticalPadding: 5, cornerRadius: 7, showsBorder: true,
                isEnabled: store.config != nil
            ) {
                showingResetSheet = true
            }
        }

        SectionLabel(text: "Feedback")
            .settingsSectionSpacing()
        SettingRow(
            label: "Report a bug or request a feature",
            hint: "Bugs, ideas, and feature requests all go to GitHub Issues."
        ) {
            Link("Open GitHub Issues", destination: PreferencesView.issuesURL)
                .font(.system(size: 12, weight: .medium))
        }

        SectionLabel(text: "About")
            .settingsSectionSpacing()
        SettingRow(label: "Version") {
            Text("v\(PreferencesView.appVersion)")
                .font(AlexTheme.Fonts.metaLabel)
                .foregroundStyle(AlexTheme.Colors.textTertiary)
        }
        RowDivider()
        SettingRow(label: "Built by") {
            Link("github.com/madhavajay", destination: PreferencesView.authorURL)
                .font(.system(size: 12, weight: .medium))
        }
        RowDivider()
        SettingRow(label: "Message me on X") {
            Link("@madhavajay", destination: PreferencesView.authorXURL)
                .font(.system(size: 12, weight: .medium))
        }
    }

    @ViewBuilder
    private var networkExposureSection: some View {
        SectionLabel(text: "Network exposure")
            .settingsSectionSpacing()
        SettingRow(label: "Listen on") {
            Picker("", selection: $networkExposure) {
                Text("Loopback only (recommended)").tag("loopback")
                Text("A specific interface").tag("interface")
                Text("All interfaces").tag("all")
            }
            .settingsPicker()
            .onChange(of: networkExposure) { saveNetworkExposure() }
        }

        if networkExposure == "interface" {
            if networkInterfaces.isEmpty {
                SettingCaption("No non-loopback interface addresses are available.")
            } else {
                RowDivider()
                SettingRow(
                    label: "Interface",
                    hint: "A LAN address can change under DHCP. If the saved address is unavailable at startup, Alex reports it loudly and falls back to loopback."
                ) {
                    Picker("", selection: $selectedInterfaceAddress) {
                        ForEach(networkInterfaces) { interface in
                            Text(interface.displayName).tag(interface.address)
                        }
                    }
                    .settingsPicker()
                    .onChange(of: selectedInterfaceAddress) { saveNetworkExposure() }
                }
            }
        }

        if networkExposure != "loopback" {
            VStack(alignment: .leading, spacing: 5) {
                Label("Remote admin access enabled", systemImage: "exclamationmark.triangle.fill")
                    .fontWeight(.bold)
                    .foregroundStyle(.red)
                (Text("This exposes Alex's admin API — your credential vault, key minting, and data reset — to that network. Anyone who can reach this port ")
                    + Text("and has your local key").bold()
                    + Text(" can control Alex and delete your data. Rotate your local key when enabling this."))
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(.red)
            }
            .padding(10)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(.red.opacity(0.12), in: RoundedRectangle(cornerRadius: AlexTheme.Radius.md))
            .padding(.vertical, 8)
        }

        if savingNetworkExposure {
            ProgressView("Saving network exposure…")
                .controlSize(.small)
                .padding(.vertical, 4)
        }
        if let networkExposureStatus {
            SettingCaption(networkExposureStatus)
        }
        if networkExposure != "loopback" {
            PillButton(
                title: "Restart daemon service to apply", variant: .bordered,
                isEnabled: !savingNetworkExposure
            ) {
                restartDaemonService()
            }
            .help("Network exposure is not live until alex service restart completes.")
            .padding(.vertical, 6)
        }
    }

    private func loadNetworkExposure() {
        networkInterfaces = NetworkInterfaces.addresses()
        guard let host = store.config?.host else { return }
        switch host {
        case "127.0.0.1", "localhost", "::1", "[::1]", "":
            networkExposure = "loopback"
        case "0.0.0.0", "::", "*":
            networkExposure = "all"
        default:
            networkExposure = "interface"
            selectedInterfaceAddress = host
        }
        if selectedInterfaceAddress.isEmpty {
            selectedInterfaceAddress = networkInterfaces.first?.address ?? ""
        }
    }

    private func saveNetworkExposure() {
        let target: String
        switch networkExposure {
        case "all": target = "all"
        case "interface":
            guard !selectedInterfaceAddress.isEmpty else { return }
            target = selectedInterfaceAddress
        default: target = "loopback"
        }
        savingNetworkExposure = true
        networkExposureStatus = nil
        Task {
            let result = await DaemonController.run(args: ["service", "bind", target])
            savingNetworkExposure = false
            if result.ok {
                networkExposureStatus = "Saved. Restart the daemon service to apply this network exposure."
                await store.refresh()
            } else {
                NSSound.beep()
                networkExposureStatus = result.combined.isEmpty
                    ? "Could not save network exposure."
                    : result.combined
            }
        }
    }

    private func restartDaemonService() {
        savingNetworkExposure = true
        networkExposureStatus = "Restarting daemon service…"
        Task {
            let result = await DaemonController.run(args: ["service", "restart"])
            savingNetworkExposure = false
            if result.ok {
                networkExposureStatus = "Daemon service restarted with the saved network exposure."
                await store.refresh()
            } else {
                NSSound.beep()
                networkExposureStatus = result.combined.isEmpty
                    ? "Could not restart the daemon service."
                    : result.combined
            }
        }
    }

    // MARK: - Release channel

    /// Drives the picker without firing on programmatic loads: only a user's
    /// selection runs `applyChannel`, so opening the pane never re-POSTs.
    private var channelBinding: Binding<String> {
        Binding(
            get: { selectedChannel },
            set: { newValue in
                selectedChannel = newValue
                applyChannel(newValue)
            }
        )
    }

    /// Shows each side's channel so divergence (the original bug) is visible.
    private var channelStateSummary: String {
        let app = UpdateChannelSetting.from(updateChannel).label
        let daemon = daemonChannel.map { UpdateChannelSetting.from($0).label } ?? "unknown"
        return "App: \(app) · Daemon: \(daemon)"
    }

    private func loadDaemonChannel() {
        guard let cfg = store.config else {
            daemonChannel = nil
            return
        }
        Task {
            do {
                daemonChannel = try await AlexClient(config: cfg)
                    .daemonUpdateChannel().channel
            } catch {
                daemonChannel = nil
            }
        }
    }

    /// Applies the picked channel to the app and/or the daemon per the current
    /// scope. Default scope is `both`, so one selection keeps them in lockstep.
    private func applyChannel(_ newValue: String) {
        channelStatus = nil
        if channelScope.appliesToApp, updateChannel != newValue {
            updateChannel = newValue
            NotificationCenter.default.post(
                name: UpdateChannelSetting.changedNotification, object: nil)
        }
        if channelScope.appliesToDaemon {
            applyDaemonChannel(newValue)
        } else {
            channelStatus =
                "App set to \(UpdateChannelSetting.from(newValue).label). Daemon left unchanged."
        }
    }

    private func applyDaemonChannel(_ newValue: String) {
        guard let cfg = store.config else {
            channelStatus = "Daemon unreachable — applied to the app only."
            return
        }
        applyingChannel = true
        Task {
            do {
                let response = try await AlexClient(config: cfg)
                    .setDaemonUpdateChannel(newValue)
                daemonChannel = response.channel
                let label = UpdateChannelSetting.from(response.channel).label
                channelStatus = channelScope == .daemon
                    ? "Daemon set to \(label)."
                    : "App and daemon set to \(label)."
                // Re-check so a now-available daemon update surfaces immediately.
                await store.refresh()
            } catch {
                NSSound.beep()
                let current = daemonChannel.map { UpdateChannelSetting.from($0).label }
                    ?? "its current channel"
                channelStatus =
                    "Could not set the daemon channel. It stays on \(current)."
            }
            applyingChannel = false
        }
    }

    private func applyDaemonUpdate(current: String, latest: String) {
        guard let cfg = store.config else {
            daemonUpdateStatus = "Daemon unreachable — update could not be started."
            return
        }
        applyingDaemonUpdate = true
        daemonUpdateTarget = latest
        daemonUpdateStatus = "Updating daemon: \(current) → \(latest)"
        Task {
            do {
                let response = try await AlexClient(config: cfg).daemonUpdateApply()
                if response.applying {
                    await store.refresh()
                } else {
                    applyingDaemonUpdate = false
                    daemonUpdateTarget = nil
                    daemonUpdateStatus =
                        "Daemon is already up to date at \(response.current ?? current)"
                }
            } catch AlexClient.ClientError.daemonUpdateRejected(let reason) {
                applyingDaemonUpdate = false
                daemonUpdateTarget = nil
                daemonUpdateStatus = reason
            } catch {
                applyingDaemonUpdate = false
                daemonUpdateTarget = nil
                daemonUpdateStatus = error.localizedDescription
            }
        }
    }

    private func reconcileDaemonUpdate(_ update: DaemonUpdateStatus?) {
        guard applyingDaemonUpdate, let target = daemonUpdateTarget, let update else { return }
        if versionsMatch(update.current, target), !update.updateAvailable {
            applyingDaemonUpdate = false
            daemonUpdateTarget = nil
            daemonUpdateStatus = "Daemon updated to \(target)"
        }
    }

    private func versionsMatch(_ lhs: String, _ rhs: String) -> Bool {
        lhs.trimmingCharacters(in: CharacterSet(charactersIn: "v"))
            == rhs.trimmingCharacters(in: CharacterSet(charactersIn: "v"))
    }

}

// MARK: - Row vocabulary (Create Settings App.tsx:595-609)

private struct SettingHintColorKey: EnvironmentKey {
    static let defaultValue = AlexTheme.Colors.textFaint
}

extension EnvironmentValues {
    var settingHintColor: Color {
        get { self[SettingHintColorKey.self] }
        set { self[SettingHintColorKey.self] = newValue }
    }
}

/// Setting row: 13px label + optional 11px hint, trailing control,
/// 11px vertical padding.
struct SettingRow<Control: View>: View {
    let label: String
    var hint: String?
    @ViewBuilder let control: Control
    @Environment(\.settingHintColor) private var hintColor

    var body: some View {
        HStack(alignment: .center, spacing: 12) {
            VStack(alignment: .leading, spacing: 1) {
                Text(label)
                    .font(.system(size: 13))
                    .foregroundStyle(AlexTheme.Colors.foreground)
                if let hint {
                    Text(hint)
                        .font(.system(size: 11))
                        .foregroundStyle(hintColor)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            Spacer(minLength: 12)
            control
        }
        .padding(.vertical, 11)
    }
}

/// Hairline divider between rows within a section (divide-y overlay(0.05)).
struct RowDivider: View {
    var body: some View {
        Rectangle()
            .fill(AlexTheme.Colors.hairline)
            .frame(height: 1)
    }
}

/// Full-width dim caption for section-level explanations and statuses.
struct SettingCaption: View {
    let text: String

    init(_ text: String) {
        self.text = text
    }

    var body: some View {
        Text(text)
            .font(.system(size: 11))
            .foregroundStyle(AlexTheme.Colors.textTertiary)
            .fixedSize(horizontal: false, vertical: true)
            .padding(.vertical, 4)
    }
}

extension View {
    func settingsSectionSpacing() -> some View {
        self.padding(.top, 14).padding(.bottom, 4)
    }

    /// Mini macOS switch (§1.11 — do not hand-build).
    func settingsSwitch() -> some View {
        self.toggleStyle(.switch)
            .controlSize(.mini)
            .labelsHidden()
    }

    func settingsPicker() -> some View {
        self.pickerStyle(.menu)
            .controlSize(.small)
            .labelsHidden()
            .fixedSize()
    }

    /// Mono inline text field (binary path).
    func settingsField(width: CGFloat? = 260) -> some View {
        self.textFieldStyle(.plain)
            .font(AlexTheme.Fonts.mono(11))
            .padding(.horizontal, 8)
            .padding(.vertical, 5)
            .frame(width: width)
            .background(
                RoundedRectangle(cornerRadius: AlexTheme.Radius.sm)
                    .fill(AlexTheme.Colors.surfaceHover))
            .overlay(
                RoundedRectangle(cornerRadius: AlexTheme.Radius.sm)
                    .strokeBorder(AlexTheme.Colors.cardBorder))
    }
}

// MARK: - Reset sheet

private struct ResetSettingsSheet: View {
    let store: SnapshotStore
    let onResetCompleted: () -> Void
    @Environment(\.dismiss) private var dismiss
    @State private var selection = ResetSelection()
    @State private var plan: ResetResponse?
    @State private var busy = false
    @State private var error: String?
    @State private var confirmed = false
    @State private var applied = false
    @State private var progress: ResetProgress?
    @State private var activeMode: ResetMode?
    @State private var drainStartedAt: Date?
    @State private var operationTask: Task<Void, Never>?
    @State private var operationID = UUID()

    private var hasSelection: Bool {
        selection.credentials || selection.settings || selection.traces
            || selection.harnesses || selection.cache
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            VStack(alignment: .leading, spacing: 2) {
                Text("Reset Alex")
                    .font(AlexTheme.Fonts.panelTitle)
                    .foregroundStyle(AlexTheme.Colors.foreground)
                Text("Choose the data to remove. Alex first runs a dry run with real counts; you can only apply the reset after reviewing it.")
                    .font(.system(size: 12))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
            }

            VStack(alignment: .leading, spacing: 8) {
                Toggle("Provider logins & credentials", isOn: $selection.credentials)
                Toggle("Settings", isOn: $selection.settings)
                Toggle("Trace history", isOn: $selection.traces)
                Toggle("Uninstall from all harnesses", isOn: $selection.harnesses)
                Text("This is the only option that edits files outside Alex (in ~/.claude, ~/.codex, and ~/.pi).")
                    .font(.system(size: 10))
                    .foregroundStyle(AlexTheme.Colors.warningOrange)
                    .padding(.leading, 22)
                Toggle("Cached data", isOn: $selection.cache)
            }
            .font(.system(size: 13))

            Text("Your update channel is preserved, so beta users stay on beta.")
                .font(.system(size: 11))
                .foregroundStyle(AlexTheme.Colors.textTertiary)

            if let plan {
                counts(plan.counts)
                Toggle(
                    "I understand the selected data will be permanently removed.",
                    isOn: $confirmed)
                    .font(.system(size: 11))
            } else {
                Text("Run the dry run to see the scale of this reset before it can be applied.")
                    .font(.system(size: 11))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
            }

            if let error {
                Text(error)
                    .font(.system(size: 11))
                    .foregroundStyle(AlexTheme.Colors.destructive)
            }
            if applied {
                Text("Reset complete.")
                    .font(.system(size: 11))
                    .foregroundStyle(AlexTheme.Colors.success)
            }

            if busy {
                VStack(alignment: .leading, spacing: 8) {
                    HStack(spacing: 8) {
                        ProgressView()
                            .controlSize(.small)
                        Text(progressCaption)
                            .font(.system(size: 11))
                            .foregroundStyle(AlexTheme.Colors.textSecondary)
                    }
                    if activeMode == .graceful {
                        HStack(spacing: 8) {
                            PillButton(
                                title: "Cancel drain", variant: .bordered,
                                fontSize: 11, isEnabled: true
                            ) {
                                cancelGracefulDrain()
                            }
                            if let drainStartedAt {
                                TimelineView(.periodic(from: drainStartedAt, by: 1)) { context in
                                    if context.date.timeIntervalSince(drainStartedAt) >= 60 {
                                        PillButton(
                                            title: "Still draining — reset immediately instead?",
                                            variant: .danger, fontSize: 11,
                                            horizontalPadding: 10, verticalPadding: 5,
                                            cornerRadius: 6, showsBorder: true,
                                            isEnabled: true
                                        ) {
                                            overrideWithImmediateReset()
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            HStack {
                // Once the reset has been applied there is nothing left to cancel, and
                // labelling the only way out "Cancel" reads as "undo" -- so nobody dares
                // click it and the sheet looks frozen.
                PillButton(
                    title: applied ? "Done" : "Cancel", variant: .bordered,
                    fontSize: 12, isEnabled: !busy
                ) {
                    dismiss()
                }
                .keyboardShortcut(applied ? .defaultAction : .cancelAction)
                if !applied {
                    Spacer()
                    PillButton(
                        title: plan == nil ? "Run dry run" : "Run dry run again",
                        variant: .bordered, fontSize: 12,
                        isEnabled: hasSelection && !busy
                    ) {
                        runDryRun()
                    }
                    if plan != nil && confirmed {
                        PillButton(
                            title: "Reset Immediately", variant: .danger, fontSize: 12,
                            horizontalPadding: 12, verticalPadding: 6,
                            cornerRadius: 6, showsBorder: true,
                            isEnabled: !busy
                        ) {
                            applyReset(mode: .immediate)
                        }
                        .keyboardShortcut(.defaultAction)
                        PillButton(
                            title: "Reset Gracefully", variant: .bordered, fontSize: 12,
                            isEnabled: !busy
                        ) {
                            applyReset(mode: .graceful)
                        }
                    }
                }
            }
        }
        .padding(24)
        .frame(width: 540)
        .background(AlexTheme.Colors.background)
        .onChange(of: selection) {
            plan = nil
            confirmed = false
            error = nil
        }
        .onDisappear {
            if activeMode == .graceful, let config = store.config {
                Task {
                    _ = try? await AlexClient(config: config).cancelResetDrain()
                }
            }
            operationID = UUID()
            operationTask?.cancel()
        }
    }

    @ViewBuilder
    // Show ONLY what the current selection will actually destroy. The daemon's plan
    // reports a full inventory (it does not know what is ticked), so listing all of
    // it here put untouched data -- an entire trace history -- on a destructive
    // confirmation screen. A confirm dialog that lists data it will not delete is
    // indistinguishable from one that will.
    private func counts(_ counts: ResetCounts) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            SectionLabel(text: "Will be permanently deleted")
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
            .foregroundStyle(AlexTheme.Colors.textSecondary)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .alexCard(
            radius: AlexTheme.Radius.lg,
            background: AlexTheme.Colors.destructive.opacity(0.06),
            border: AlexTheme.Colors.destructive.opacity(0.18))
    }

    private func runDryRun() {
        guard let config = store.config else { return }
        operationTask?.cancel()
        let id = UUID()
        operationID = id
        busy = true
        error = nil
        confirmed = false
        progress = ResetProgress(
            status: "idle", phase: "counting_bodies",
            detail: "Counting captured bodies", inFlight: 0)
        activeMode = nil
        operationTask = Task {
            let polling = pollResetProgress(config: config, operationID: id)
            defer { polling.cancel() }
            do {
                let result = try await AlexClient(config: config).resetPlan(selection)
                guard operationID == id else { return }
                plan = result
            } catch {
                guard operationID == id else { return }
                self.error = "Could not get reset counts: \(error.localizedDescription)"
            }
            if operationID == id {
                busy = false
            }
        }
    }

    private func applyReset(mode: ResetMode) {
        guard let config = store.config, plan != nil, confirmed else { return }
        operationTask?.cancel()
        let id = UUID()
        operationID = id
        busy = true
        error = nil
        activeMode = mode
        drainStartedAt = mode == .graceful ? Date() : nil
        progress = ResetProgress(
            status: mode == .graceful ? "draining" : "applying",
            phase: mode == .graceful ? "draining" : "aborting_requests",
            detail: mode == .graceful
                ? "Waiting for routed requests to finish" : "Closing routed requests",
            inFlight: progress?.inFlight ?? 0)
        operationTask = Task {
            let polling = pollResetProgress(config: config, operationID: id)
            defer { polling.cancel() }
            do {
                _ = try await AlexClient(config: config).reset(selection, mode: mode)
                guard operationID == id else { return }
                if selection.settings {
                    AppSettingsReset.clear()
                }
                applied = true
                await store.refresh()
                // Any deliberate reset returns the app to the guided setup.
                // Clear completion as well as opening it now, so closing the
                // window cannot suppress onboarding on the next launch.
                OnboardingLaunchPolicy.clearCompletion()
                onResetCompleted()
            } catch {
                guard operationID == id else { return }
                self.error = "Could not apply reset: \(error.localizedDescription)"
            }
            if operationID == id {
                busy = false
                activeMode = nil
                drainStartedAt = nil
            }
        }
    }

    private var progressCaption: String {
        guard let progress else {
            return activeMode == .graceful ? "Draining…" : "Preparing reset…"
        }
        switch progress.phase {
        case "draining":
            return "Draining — \(progress.inFlight) request\(progress.inFlight == 1 ? "" : "s") in flight…"
        case "aborting_requests": return "Closing routed requests…"
        case "counting_bodies": return "Counting captured bodies…"
        case "counting_traces": return "Counting traces and accounts…"
        case "counting_harnesses": return "Checking connected harnesses…"
        case "disconnecting_harnesses": return progress.detail + "…"
        case "removing_accounts": return "Removing provider accounts…"
        case "clearing_traces": return "Removing traces and captured bodies…"
        case "clearing_caches": return "Clearing derived caches…"
        case "restoring_settings": return "Restoring default settings…"
        case "complete": return "Reset complete."
        default: return progress.detail.isEmpty ? "Working…" : progress.detail
        }
    }

    private func pollResetProgress(
        config: DaemonConfig, operationID id: UUID
    ) -> Task<Void, Never> {
        Task {
            let client = AlexClient(config: config)
            while !Task.isCancelled && operationID == id {
                if let latest = try? await client.resetProgress(), operationID == id {
                    progress = latest
                }
                try? await Task.sleep(nanoseconds: 500_000_000)
            }
        }
    }

    private func cancelGracefulDrain() {
        guard let config = store.config, activeMode == .graceful else { return }
        Task {
            do {
                _ = try await AlexClient(config: config).cancelResetDrain()
                operationID = UUID()
                operationTask?.cancel()
                busy = false
                activeMode = nil
                drainStartedAt = nil
                progress = nil
                error = nil
            } catch {
                self.error = "Could not cancel graceful reset: \(error.localizedDescription)"
            }
        }
    }

    private func overrideWithImmediateReset() {
        operationID = UUID()
        operationTask?.cancel()
        applyReset(mode: .immediate)
    }
}
