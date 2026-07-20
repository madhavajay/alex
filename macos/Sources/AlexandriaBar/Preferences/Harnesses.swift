import AppKit
import SwiftUI
import AlexandriaBarCore

/// The Preferences → Harnesses tab: connected-harness rows, refresh/connect
/// flows, and per-row tools opt-in. Restyled to match
/// `ui/Create Settings Page/src/app/App.tsx` (HarnessRow / HarnessesPanel,
/// ~lines 466-591) exactly: flat panel header (title + subtitle + trailing
/// "Update All" link), hairline row dividers, and a hover-revealed delete
/// action.
///
/// The mock's `hasUpdate` signal (per-row "upd" chip + "Update" button) has
/// no backing field on `Harness` (see AlexandriaBarCore/HarnessModels.swift)
/// — there is no update-availability data from the daemon today, so those
/// elements are omitted and the subtitle reads "N connected" instead of
/// "N connected · M with updates". "Update All" stays wired to the existing
/// multi-refresh flow.
struct HarnessesPreferencesSection: View {
    let store: SnapshotStore
    let onOpenTraceBrowser: (String) -> Void
    @State private var updateAllModel: MultiHarnessRefreshSheetModel?

    private var rows: [Harness] {
        HarnessCatalog.rows(store.harnesses)
    }

    private var refreshTargets: [Harness] {
        HarnessCatalog.refreshTargets(store.harnesses)
    }

    private var connectedCount: Int {
        rows.filter(\.connected).count
    }

    var body: some View {
        VStack(spacing: 0) {
            header
            Rectangle()
                .fill(AlexTheme.Colors.overlay(0.06))
                .frame(height: 1)
                .padding(.horizontal, 20)
            content
        }
        .background(AlexTheme.Colors.background)
        .sheet(item: $updateAllModel) { sheet in
            MultiHarnessRefreshSheetHost(sheet: sheet) {
                updateAllModel = nil
            }
        }
        .task(id: store.harnessesCheckedMs) {
            await store.refreshHarnessesIfStale()
            guard let config = store.config,
                  let fetched = try? await AlexandriaClient(config: config).credentials()
            else { return }
            store.rememberCredentials(fetched)
        }
    }

    // MARK: Header (App.tsx:552-567)

    private var header: some View {
        HStack(alignment: .center) {
            VStack(alignment: .leading, spacing: 1) {
                Text("Harnesses")
                    .font(AlexTheme.Fonts.panelTitle)
                    .foregroundStyle(AlexTheme.Colors.foreground)
                HStack(spacing: 5) {
                    Text("\(connectedCount) connected")
                    if store.harnessesChecking {
                        ProgressView()
                            .controlSize(.mini)
                        Text("checking…")
                    }
                }
                .font(.system(size: 12))
                .foregroundStyle(AlexTheme.Colors.textTertiary)
            }
            Spacer()
            if !refreshTargets.isEmpty {
                UpdateAllLink {
                    let model = MultiHarnessRefreshSheetModel(store: store)
                    updateAllModel = model
                    model.start()
                }
            }
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 16)
    }

    // MARK: Content

    @ViewBuilder
    private var content: some View {
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
            .padding(.horizontal, 20)
            .padding(.vertical, 12)
        } else if store.harnessesSupported == nil {
            HStack(spacing: 8) {
                ProgressView()
                    .controlSize(.small)
                Text("Checking harness support…")
                    .font(.system(size: 11))
                    .foregroundStyle(AlexTheme.Colors.textSecondary)
            }
            .padding(.horizontal, 20)
            .padding(.vertical, 12)
            .task {
                await store.refresh()
            }
        } else {
            ScrollView {
                VStack(spacing: 0) {
                    ForEach(Array(rows.enumerated()), id: \.element.id) { index, harness in
                        HarnessRowView(
                            harness: harness,
                            store: store,
                            onOpenTraceBrowser: onOpenTraceBrowser)
                        if index < rows.count - 1 {
                            Rectangle()
                                .fill(AlexTheme.Colors.divider)
                                .frame(height: 1)
                                .padding(.horizontal, 16)
                        }
                    }
                }
                .padding(.vertical, 4)
            }
        }
    }
}

/// The header's blue "Update All" text link (App.tsx:559-566: text-[#0a84ff]
/// hover:text-[#3a9fff], no background).
private struct UpdateAllLink: View {
    let action: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            Text("Update All")
                .font(.system(size: 12, weight: .medium))
                .foregroundStyle(hovering
                    ? AlexTheme.Colors.dynamic(light: 0x3A9FFF, dark: 0x3A9FFF)
                    : AlexTheme.Colors.primary)
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
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

/// A single harness row (App.tsx:468-543). Fixed-width name/path column,
/// a flexible spacer, then fixed-width right-hand columns so every row's
/// controls line up regardless of name/path length.
private struct HarnessRowView: View {
    let harness: Harness
    let store: SnapshotStore
    let onOpenTraceBrowser: (String) -> Void
    @State private var error: String?
    @State private var actionModel: HarnessActionSheetModel?
    @State private var hovered = false
    @State private var confirmingDisconnect = false

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .center, spacing: 10) {
                iconTile
                nameAndPath
                Spacer(minLength: 8)
                actionsColumn
            }
            if let error {
                Text(error)
                    .font(.system(size: 10))
                    .foregroundStyle(AlexTheme.Colors.destructive)
            }
        }
        .font(.system(size: 12))
        .padding(.horizontal, 16)
        .padding(.vertical, 9)
        .frame(maxWidth: .infinity, alignment: .leading)
        .contentShape(Rectangle())
        .background(hovered ? AlexTheme.Colors.divider : Color.clear)
        .onHover { hovered = $0 }
        .sheet(item: $actionModel) { model in
            HarnessActionSheetHost(model: model) {
                actionModel = nil
            }
        }
        .confirmationDialog(
            "Remove \(HarnessCatalog.displayName(harness.name))?",
            isPresented: $confirmingDisconnect,
            titleVisibility: .visible
        ) {
            Button("Remove", role: .destructive) {
                beginAction(.disconnect)
            }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("This disconnects \(HarnessCatalog.displayName(harness.name)) from Alex and revokes its harness key.")
        }
    }

    // MARK: Name + path/key column (widened for the key fingerprint)

    private var nameAndPath: some View {
        VStack(alignment: .leading, spacing: 1) {
            HStack(spacing: 5) {
                Text(HarnessCatalog.displayName(harness.name))
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(AlexTheme.Colors.foreground)
                    .lineLimit(1)
                if let version = harness.version, !version.isEmpty {
                    Text("v\(version)")
                        .font(AlexTheme.Fonts.metaMicro)
                        .foregroundStyle(AlexTheme.Colors.textFaint)
                        .fixedSize()
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
            if harness.connected, let key = harnessKey {
                HStack(spacing: 6) {
                    Text("key \(key.shortFingerprint)")
                        .font(AlexTheme.Fonts.metaMono)
                        .foregroundStyle(AlexTheme.Colors.textFaint)
                        .lineLimit(1)
                    PillButton(
                        title: "Traces", variant: .bordered,
                        systemImage: "magnifyingglass",
                        fontSize: 9, horizontalPadding: 6,
                        verticalPadding: 2, cornerRadius: 5
                    ) {
                        onOpenTraceBrowser("key:\(key.keyFingerprint)")
                    }
                }
            }
        }
        .frame(width: 220, alignment: .leading)
    }

    private var harnessKey: CredentialRunKey? {
        store.credentials?.inbound.runKeys.activeHarnessKey(named: harness.name)
    }

    // MARK: Col 2 — Install/Update + hover-revealed Delete (App.tsx:523-538)

    @ViewBuilder
    private var actionsColumn: some View {
        HStack(spacing: 6) {
            if actionModel != nil {
                ProgressView()
                    .controlSize(.small)
            } else {
                if harness.connected && harness.supportsConnect {
                    PillButton(
                        title: "Remove", variant: .danger
                    ) {
                        confirmingDisconnect = true
                    }
                    .opacity(hovered ? 1 : 0)
                    .allowsHitTesting(hovered)
                }
                if harness.supportsConnect {
                    PillButton(
                        title: harness.connected ? "Update" : "Install", variant: .standard
                    ) {
                        beginAction(harness.connected ? .refresh : .connect)
                    }
                }
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
            toolCapture: model.showsToolCapture ? $model.captureToolCalls : nil,
            captureWarning: model.captureWarning,
            onApprove: { model.approve() },
            onCancel: onClose,
            onClose: onClose
        )
    }
}
