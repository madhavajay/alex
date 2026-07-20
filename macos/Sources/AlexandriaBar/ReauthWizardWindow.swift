import AppKit
import SwiftUI
import Observation
import AlexandriaBarCore

@MainActor
@Observable
final class ReauthWizardModel {
    let accounts: [Account]
    let store: SnapshotStore
    let close: @MainActor () -> Void

    private(set) var accountIndex = 0
    private(set) var reauthenticatedCount = 0
    private(set) var skippedCount = 0
    private(set) var authModel: AuthFlowModel?
    private(set) var isFinished = false
    private(set) var isRefreshing = false
    private var started = false

    init(
        accounts: [Account], store: SnapshotStore,
        close: @escaping @MainActor () -> Void
    ) {
        self.accounts = accounts
        self.store = store
        self.close = close
    }

    var currentAccount: Account? {
        guard accounts.indices.contains(accountIndex) else { return nil }
        return accounts[accountIndex]
    }

    func start() {
        guard !started else { return }
        started = true
        startCurrentAccount()
    }

    func skipCurrentAccount() {
        guard currentAccount != nil, !isFinished else { return }
        authModel?.cancel()
        authModel = nil
        skippedCount += 1
        accountIndex += 1
        startCurrentAccount()
    }

    func cancel() {
        authModel?.cancel()
        authModel = nil
        close()
    }

    func closeFinishedWizard() {
        close()
    }

    private func startCurrentAccount() {
        guard let account = currentAccount else {
            finish()
            return
        }

        let accountID = account.id
        let model = AuthFlowModel(
            provider: account.provider,
            accountName: account.name,
            store: store)
        model.onAuthenticated = { [weak self] _ in
            guard let self, self.currentAccount?.id == accountID else { return }
            self.authModel?.cancel()
            self.authModel = nil
            self.reauthenticatedCount += 1
            self.accountIndex += 1
            self.startCurrentAccount()
        }
        authModel = model
        model.begin()
    }

    private func finish() {
        authModel?.cancel()
        authModel = nil
        isFinished = true
        isRefreshing = true
        Task { [weak self] in
            guard let self else { return }
            await self.store.refresh()
            // AuthFlowModel also refreshes on success. If that refresh won
            // the coalescing race, wait for it rather than declaring the
            // wizard refreshed while the shared store is still in flight.
            while self.store.refreshing {
                guard !Task.isCancelled else { return }
                try? await Task.sleep(for: .milliseconds(50))
            }
            self.isRefreshing = false
        }
    }
}

struct ReauthWizardView: View {
    @Bindable var model: ReauthWizardModel

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider().overlay(AlexTheme.Colors.cardBorder)
            ScrollView {
                content
                    .padding(26)
                    .frame(maxWidth: .infinity, minHeight: 470, alignment: .topLeading)
            }
            Divider().overlay(AlexTheme.Colors.cardBorder)
            footer
        }
        .frame(width: 620, height: 650)
        .background(AlexTheme.Colors.background)
        .task { model.start() }
    }

    private var header: some View {
        HStack {
            VStack(alignment: .leading, spacing: 2) {
                Text("SUBSCRIPTION RE-AUTHENTICATION")
                    .font(AlexTheme.Fonts.metaMono)
                    .foregroundStyle(AlexTheme.Colors.destructive)
                Text(model.isFinished ? "Complete" : "Restore your subscriptions")
                    .font(AlexTheme.Fonts.panelTitle)
                    .foregroundStyle(AlexTheme.Colors.foreground)
            }
            Spacer()
            if !model.isFinished {
                Text("\(min(model.accountIndex + 1, model.accounts.count)) of \(model.accounts.count)")
                    .font(AlexTheme.Fonts.metaLabel)
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
            }
        }
        .padding(.horizontal, 24)
        .frame(height: 62)
    }

    @ViewBuilder
    private var content: some View {
        if model.isFinished {
            VStack(alignment: .leading, spacing: 16) {
                Image(systemName: "checkmark.circle.fill")
                    .font(.system(size: 42))
                    .foregroundStyle(AlexTheme.Colors.success)
                Text("\(model.reauthenticatedCount) re-authenticated, \(model.skippedCount) skipped")
                    .font(.system(size: 18, weight: .semibold))
                    .foregroundStyle(AlexTheme.Colors.foreground)
                HStack(spacing: 8) {
                    if model.isRefreshing {
                        ProgressView().controlSize(.small)
                    } else {
                        Image(systemName: "arrow.clockwise.circle.fill")
                            .foregroundStyle(AlexTheme.Colors.success)
                    }
                    Text(model.isRefreshing ? "Refreshing account status…" : "Account status refreshed")
                        .font(.system(size: 12))
                        .foregroundStyle(AlexTheme.Colors.textSecondary)
                }
            }
            .frame(maxWidth: .infinity, minHeight: 360, alignment: .center)
        } else if let account = model.currentAccount, let authModel = model.authModel {
            VStack(alignment: .leading, spacing: 14) {
                HStack(spacing: 8) {
                    Text("Account")
                        .font(.system(size: 11, weight: .medium))
                        .foregroundStyle(AlexTheme.Colors.textTertiary)
                    Text("\(ProviderInfo.displayName(account.provider)) · \(account.name)")
                        .font(AlexTheme.Fonts.mono(11, weight: .semibold))
                        .foregroundStyle(AlexTheme.Colors.foreground)
                }
                AuthFlowView(model: authModel, close: {}, embedded: true)
                    .background(
                        RoundedRectangle(cornerRadius: AlexTheme.Radius.md)
                            .fill(AlexTheme.Colors.card))
                    .overlay(
                        RoundedRectangle(cornerRadius: AlexTheme.Radius.md)
                            .strokeBorder(AlexTheme.Colors.cardBorder))
            }
        }
    }

    private var footer: some View {
        HStack(spacing: 10) {
            if model.isFinished {
                Spacer()
                PillButton(
                    title: "Done",
                    variant: .solidAccent,
                    isEnabled: !model.isRefreshing,
                    keyboardShortcut: .defaultAction
                ) { model.closeFinishedWizard() }
            } else {
                PillButton(
                    title: "Cancel",
                    variant: .bordered,
                    keyboardShortcut: .cancelAction
                ) { model.cancel() }
                Spacer()
                PillButton(title: "Skip", variant: .bordered) {
                    model.skipCurrentAccount()
                }
            }
        }
        .padding(.horizontal, 20)
        .frame(height: 64)
    }
}

@MainActor
final class ReauthWizardWindowController: NSObject, NSWindowDelegate {
    private let store: SnapshotStore
    private var window: NSWindow?
    private var model: ReauthWizardModel?

    init(store: SnapshotStore) {
        self.store = store
    }

    func show(accounts: [Account]) {
        guard !accounts.isEmpty else { return }
        if window == nil {
            let model = ReauthWizardModel(
                accounts: accounts,
                store: store,
                close: { [weak self] in self?.window?.close() })
            self.model = model
            let window = NSWindow(
                contentViewController: NSHostingController(
                    rootView: ReauthWizardView(model: model)))
            window.title = "Re-authenticate Subscriptions"
            window.styleMask = [.titled, .closable, .miniaturizable]
            window.isReleasedWhenClosed = false
            window.delegate = self
            window.center()
            self.window = window
        }

        NSApp.activate(ignoringOtherApps: true)
        if let window {
            DockIconManager.shared.track(window)
            window.makeKeyAndOrderFront(nil)
        }
    }

    func windowWillClose(_ notification: Notification) {
        model?.authModel?.cancel()
        model = nil
        window = nil
    }
}
