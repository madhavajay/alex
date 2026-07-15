import AppKit
import SwiftUI
import AlexandriaBarCore

/// The Preferences → Harnesses tab: connected-harness rows, refresh/update
/// flows, and per-harness overrides. Moved verbatim out of
/// `PreferencesView.swift` (Phase-0 file split); restyled with the shared
/// design system (icon tiles, status dots, pill buttons).
struct HarnessesPreferencesSection: View {
    let store: SnapshotStore
    @State private var updateAllModel: MultiHarnessRefreshSheetModel?

    private var refreshTargets: [Harness] {
        HarnessCatalog.refreshTargets(store.harnesses)
    }

    var body: some View {
        Section {
            if store.harnessesSupported == false {
                HStack(spacing: 8) {
                    Image(systemName: "exclamationmark.triangle")
                        .foregroundStyle(AlexTheme.Colors.warningOrange)
                    Text("daemon update required — run ")
                    Text("alex update")
                        .font(AlexTheme.Fonts.metaLabel)
                }
                .font(.system(size: 11))
                .foregroundStyle(AlexTheme.Colors.textSecondary)
            } else if store.harnessesSupported == nil {
                HStack(spacing: 8) {
                    ProgressView()
                        .controlSize(.small)
                    Text("Checking harness support…")
                        .font(.system(size: 11))
                        .foregroundStyle(AlexTheme.Colors.textSecondary)
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
                        .foregroundStyle(AlexTheme.Colors.textTertiary)
                        Spacer()
                        PillButton(title: "Update All Harnesses", variant: .primary) {
                            let model = MultiHarnessRefreshSheetModel(store: store)
                            updateAllModel = model
                            model.start()
                        }
                    }
                }
                ForEach(HarnessCatalog.rows(store.harnesses)) { harness in
                    HarnessRowView(harness: harness, store: store)
                }
            }
        } header: {
            SectionLabel(text: "Harnesses")
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
            HStack(alignment: .center, spacing: 12) {
                iconTile
                VStack(alignment: .leading, spacing: 3) {
                    HStack(spacing: 6) {
                        Text(HarnessCatalog.displayName(harness.name))
                            .font(.system(size: 13, weight: .semibold))
                            .foregroundStyle(AlexTheme.Colors.foreground)
                        if let version = harness.version, !version.isEmpty {
                            Text("v\(version)")
                                .font(AlexTheme.Fonts.metaMicro)
                                .foregroundStyle(AlexTheme.Colors.textFaint)
                        }
                        if let warning = harness.versionWarning, !warning.isEmpty {
                            Image(systemName: "exclamationmark.triangle.fill")
                                .font(.system(size: 10))
                                .foregroundStyle(AlexTheme.Colors.warningOrange)
                                .help(warning)
                        }
                    }
                    Text(harness.configDir ?? "No config directory")
                        .font(AlexTheme.Fonts.metaLabel)
                        .foregroundStyle(AlexTheme.Colors.textFaintest)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
                Spacer()
                if harness.name == "codex", harness.connected {
                    Toggle(isOn: Binding(
                        get: { harness.defaultRoute == "alex" },
                        set: { setCodexDefaultRoute($0 ? "alex" : "openai") })
                    ) {
                        Text("Alexandria default")
                            .font(.system(size: 11))
                            .foregroundStyle(AlexTheme.Colors.textTertiary)
                    }
                    .toggleStyle(.switch)
                    .controlSize(.mini)
                    .disabled(routeUpdating || actionModel != nil)
                    .help(
                        "Plain `codex` follows this setting. Explicit --profile openai and --profile alex commands always remain available."
                    )
                }
                if HarnessCatalog.toolCaptureHarnesses.contains(harness.name), harness.connected {
                    Toggle(isOn: Binding(
                        get: { harness.toolCaptureEnabled ?? false },
                        set: { setToolCapture($0) })
                    ) {
                        Text("Capture tools")
                            .font(.system(size: 11))
                            .foregroundStyle(AlexTheme.Colors.textTertiary)
                    }
                    .toggleStyle(.switch)
                    .controlSize(.mini)
                    .disabled(toolCaptureUpdating || actionModel != nil)
                    .help("Opt in to storing this harness's tool arguments and results locally. Secrets are redacted before storage.")
                }
                if harness.supportsConnect {
                    if harness.connected {
                        PillButton(
                            title: HarnessActionKind.refresh.label,
                            variant: .standard,
                            isEnabled: actionModel == nil
                        ) {
                            beginAction(.refresh)
                        }
                    }
                    actionButton
                }
                PillButton(
                    title: "Override…", variant: .bordered,
                    fontSize: 11, horizontalPadding: 8, verticalPadding: 3,
                    cornerRadius: 5, isEnabled: actionModel == nil
                ) {
                    showOverride = true
                }
            }
            if let error {
                Text(error)
                    .font(.system(size: 10))
                    .foregroundStyle(AlexTheme.Colors.destructive)
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

    /// 32px brand tile (Create Settings App.tsx:90-123) with a bottom-right
    /// StatusDot-style health badge: green = connected, dim = installed only,
    /// dim @50% = not installed. The badge ring matches the grouped-form row
    /// surface this tile sits on, not the window background.
    private var iconTile: some View {
        IconWithHealthBadge(
            size: 32,
            tint: harness.connected
                ? AlexTheme.Colors.success : AlexTheme.Colors.textTertiary,
            pending: !harness.installed,
            ringColor: AlexTheme.Colors.card
        ) {
            HarnessIconView(
                harness: harness.name, tags: nil, size: 32,
                background: AlexTheme.HarnessBrand.tileBackground(for: harness.name),
                backgroundPadding: AlexTheme.HarnessBrand.tilePadding(for: harness.name),
                cornerRadius: 8,
                showsFallback: true)
        }
        .help(connectionHelp)
    }

    private var connectionHelp: String {
        if harness.connected { return "Connected" }
        if harness.installed { return "Installed, not connected" }
        return "Not installed"
    }

    @ViewBuilder
    private var actionButton: some View {
        if actionModel != nil {
            ProgressView()
                .controlSize(.small)
        } else if harness.connected {
            PillButton(title: HarnessActionKind.disconnect.label, variant: .danger) {
                beginAction(.disconnect)
            }
        } else {
            PillButton(title: HarnessActionKind.connect.label, variant: .primary) {
                beginAction(.connect)
            }
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
            toolCapture: model.showsToolCapture ? $model.captureToolCalls : nil,
            captureWarning: model.captureWarning,
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
                .font(.system(size: 13, weight: .semibold))
                .foregroundStyle(AlexTheme.Colors.foreground)
            Grid(alignment: .leading, horizontalSpacing: 10, verticalSpacing: 10) {
                GridRow {
                    SectionLabel(text: "Binary")
                    HStack(spacing: 6) {
                        TextField("Binary path (blank = clear)", text: $binary)
                            .font(AlexTheme.Fonts.metaLabel)
                        Button {
                            chooseBinary()
                        } label: {
                            Image(systemName: "folder")
                        }
                    }
                }
                GridRow {
                    SectionLabel(text: "Config dir")
                    HStack(spacing: 6) {
                        TextField("Config dir (blank = clear)", text: $configDir)
                            .font(AlexTheme.Fonts.metaLabel)
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
                    .foregroundStyle(AlexTheme.Colors.destructive)
            }
            HStack {
                Spacer()
                PillButton(
                    title: "Cancel", variant: .standard,
                    horizontalPadding: 12, verticalPadding: 5, cornerRadius: 6,
                    keyboardShortcut: .cancelAction
                ) {
                    dismiss()
                }
                if saving {
                    ProgressView()
                        .controlSize(.small)
                } else {
                    PillButton(
                        title: "Save", variant: .solidAccent,
                        keyboardShortcut: .defaultAction
                    ) {
                        save()
                    }
                }
            }
        }
        .padding(18)
        .frame(width: 480)
        .background(AlexTheme.Colors.background)
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
