import AppKit
import SwiftUI
import AlexandriaBarCore

/// The Preferences → General tab: launch at login, refresh cadence, alerts,
/// updates, terminal, daemon, network exposure, reset, proxy credentials,
/// feedback, and about. Restyled per ui/Create Settings Page (§2.3 of the
/// design spec): section labels + hairline-divided setting rows.
struct GeneralPreferencesPane: View {
    let store: SnapshotStore
    @AppStorage("refreshSeconds") private var refreshSeconds: Double = 60
    @AppStorage("limitWarnPct") private var limitWarnPct: Double = 90
    @AppStorage("notifyEnabled") private var notifyEnabled = true
    @AppStorage("binaryPath") private var binaryPath = ""
    @AppStorage("terminalApp") private var terminalApp = "auto"
    @AppStorage(UpdateChannelSetting.defaultsKey) private var updateChannel =
        UpdateChannelSetting.stable.rawValue
    @State private var launchAtLogin = false
    @State private var copyingCredentials = false
    @State private var credentialCopyStatus: String?
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
            ResetSettingsSheet(store: store)
        }
        .onAppear {
            launchAtLogin = LaunchAtLogin.isEnabled
            loadNetworkExposure()
        }
        .onChange(of: store.config?.host) { loadNetworkExposure() }
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
        SectionLabel(text: "System")
            .settingsSectionSpacing()
        if LaunchAtLogin.available {
            SettingRow(label: "Launch at login", hint: "Start AlexandriaBar when you log in") {
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
            Picker("", selection: $updateChannel) {
                ForEach(UpdateChannelSetting.allCases, id: \.rawValue) { channel in
                    Text(channel.label).tag(channel.rawValue)
                }
            }
            .settingsPicker()
            .onChange(of: updateChannel) {
                NotificationCenter.default.post(
                    name: UpdateChannelSetting.changedNotification, object: nil)
            }
        }
        if UpdateChannelSetting.from(updateChannel) == .beta {
            SettingCaption(
                "Beta builds are pre-release test versions. When the matching final release ships, the app updates to it automatically. The daemon channel is set separately: alex update --set-channel beta.")
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
        SettingRow(label: "Binary path", hint: "Blank = auto-discover the alexandria binary") {
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
            label: "Reset Alexandria",
            hint: "Remove selected Alexandria data after reviewing a real-count dry run."
        ) {
            PillButton(
                title: "Reset…", variant: .danger, horizontalPadding: 12,
                verticalPadding: 5, cornerRadius: 7, showsBorder: true,
                isEnabled: store.config != nil
            ) {
                showingResetSheet = true
            }
        }

        SectionLabel(text: "Proxy credentials")
            .settingsSectionSpacing()
        SettingCaption(
            "Copy credentials for generic API clients, or a ready-to-edit command that mints a tagged run key. Both use the currently loaded local daemon settings.")
        HStack(spacing: AlexTheme.Spacing.md) {
            PillButton(
                title: "Copy environment block", variant: .standard,
                systemImage: "doc.on.doc",
                horizontalPadding: 12, verticalPadding: 5, cornerRadius: 7,
                isEnabled: !copyingCredentials && store.config != nil
            ) {
                copyGenericCredentials()
            }
            .help("Copy the same shell exports printed by alex credentials")
            PillButton(
                title: "Copy run-key curl", variant: .standard,
                systemImage: "terminal",
                horizontalPadding: 12, verticalPadding: 5, cornerRadius: 7,
                isEnabled: store.config != nil
            ) {
                copyRunKeyCurl()
            }
            .help("Copy an editable POST /admin/run-keys example using this daemon")
            if copyingCredentials { ProgressView().controlSize(.small) }
        }
        .padding(.vertical, 8)
        if let credentialCopyStatus {
            SettingCaption(credentialCopyStatus)
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
                    hint: "A LAN address can change under DHCP. If the saved address is unavailable at startup, Alexandria reports it loudly and falls back to loopback."
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
                (Text("This exposes Alexandria's admin API — your credential vault, key minting, and data reset — to that network. Anyone who can reach this port ")
                    + Text("and has your local key").bold()
                    + Text(" can control Alexandria and delete your data. Rotate your local key when enabling this."))
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

// MARK: - Row vocabulary (Create Settings App.tsx:595-609)

/// Setting row: 13px label + optional 11px hint, trailing control,
/// 11px vertical padding.
struct SettingRow<Control: View>: View {
    let label: String
    var hint: String?
    @ViewBuilder let control: Control

    var body: some View {
        HStack(alignment: .center, spacing: 12) {
            VStack(alignment: .leading, spacing: 1) {
                Text(label)
                    .font(.system(size: 13))
                    .foregroundStyle(AlexTheme.Colors.foreground)
                if let hint {
                    Text(hint)
                        .font(.system(size: 11))
                        .foregroundStyle(AlexTheme.Colors.textFaint)
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

    fileprivate func settingsPicker() -> some View {
        self.pickerStyle(.menu)
            .controlSize(.small)
            .labelsHidden()
            .fixedSize()
    }

    /// Mono inline text field (binary path).
    func settingsField() -> some View {
        self.textFieldStyle(.plain)
            .font(AlexTheme.Fonts.mono(11))
            .padding(.horizontal, 8)
            .padding(.vertical, 5)
            .frame(width: 260)
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
            VStack(alignment: .leading, spacing: 2) {
                Text("Reset Alexandria")
                    .font(AlexTheme.Fonts.panelTitle)
                    .foregroundStyle(AlexTheme.Colors.foreground)
                Text("Choose the data to remove. Alexandria first runs a dry run with real counts; you can only apply the reset after reviewing it.")
                    .font(.system(size: 12))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
            }

            VStack(alignment: .leading, spacing: 8) {
                Toggle("Provider logins & credentials", isOn: $selection.credentials)
                Toggle("Settings", isOn: $selection.settings)
                Toggle("Trace history", isOn: $selection.traces)
                Toggle("Uninstall from all harnesses", isOn: $selection.harnesses)
                Text("This is the only option that edits files outside Alexandria (in ~/.claude, ~/.codex, and ~/.pi).")
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
                    if plan != nil {
                        PillButton(
                            title: "Reset now", variant: .danger, fontSize: 12,
                            horizontalPadding: 12, verticalPadding: 6,
                            cornerRadius: 6, showsBorder: true,
                            isEnabled: confirmed && !busy
                        ) {
                            applyReset()
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
