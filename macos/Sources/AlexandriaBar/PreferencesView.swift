import AppKit
import SwiftUI
import Observation
import AlexandriaBarCore

// The tab panes live in Preferences/General.swift, Preferences/Providers.swift,
// and Preferences/Harnesses.swift; this file keeps the shell and the window
// controller. Shell design: ui/Create Settings Page (left nav sidebar
// replacing the segmented picker, §1.25 + §2.3 of the design spec).

enum PreferencesSection: String, CaseIterable, Hashable {
    case general = "General"
    case providers = "Providers"
    case harnesses = "Harnesses"
    case credentials = "Credentials"
    case dario = "Dario"
    case protection = "Failover"
    case middleware = "Middleware"
    case notifications = "Notifications"

    var icon: String {
        switch self {
        case .general: "gearshape"
        case .providers: "bolt"
        case .harnesses: "terminal"
        case .credentials: "key"
        case .dario: "server.rack"
        case .protection: "shield"
        case .middleware: "arrow.triangle.branch"
        case .notifications: "paperplane"
        }
    }
}

@MainActor
@Observable
final class PreferencesViewState {
    var section = PreferencesSection.general
}

struct PreferencesView: View {
    @Bindable var state: PreferencesViewState
    let store: SnapshotStore
    let onAuthenticate: (String, String?, Bool, Bool) -> Void
    let onOpenDario: () -> Void
    let onOpenTraceBrowser: (String) -> Void
    let onRunOnboarding: () -> Void
    let onResetCompleted: () -> Void

    var body: some View {
        HStack(spacing: 0) {
            sidebar
            Rectangle()
                .fill(AlexTheme.Colors.cardBorder)
                .frame(width: 1)
            content
        }
        .background(AlexTheme.Colors.background)
        .frame(minWidth: 960, maxWidth: .infinity, minHeight: 680, maxHeight: .infinity)
    }

    // MARK: Sidebar (mock: 180px, bg rgba(0,0,0,0.25), traffic-light zone on top)

    private var sidebar: some View {
        VStack(alignment: .leading, spacing: 0) {
            // Native traffic lights live in this zone (transparent titlebar +
            // fullSizeContentView); just reserve the height.
            Spacer().frame(height: 52)

            SectionLabel(text: "Alex UI")
                .padding(.horizontal, 14)
                .padding(.bottom, 12)

            VStack(spacing: 2) {
                // Failover is retained as a compatibility deep-link for one
                // release. New navigation goes directly to Middleware.
                ForEach(PreferencesSection.allCases.filter { $0 != .protection }, id: \.self) { section in
                    SettingsNavItem(
                        label: section.rawValue,
                        icon: section.icon,
                        active: state.section == section
                    ) {
                        state.section = section
                    }
                }
            }
            .padding(.horizontal, 8)

            Spacer()

            Text("v\(PreferencesView.appVersion)")
                .font(AlexTheme.Fonts.metaMicro)
                .foregroundStyle(AlexTheme.Colors.textFaintest)
                .frame(maxWidth: .infinity)
                .padding(12)
        }
        .frame(width: 180)
        .background(AlexTheme.Colors.sidebarWash)
    }

    // MARK: Content column (52px header bar + hosted pane)

    private var content: some View {
        VStack(spacing: 0) {
            ZStack {
                Text("Settings")
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
            }
            .frame(height: 52)
            .frame(maxWidth: .infinity)
            .overlay(alignment: .bottom) {
                Rectangle()
                    .fill(AlexTheme.Colors.cardBorder)
                    .frame(height: 1)
            }

            Group {
                switch state.section {
                case .general:
                    GeneralPreferencesPane(
                        store: store, onRunOnboarding: onRunOnboarding,
                        onResetCompleted: onResetCompleted)
                case .providers:
                    ProvidersPreferencesSection(
                        store: store,
                        onAuthenticate: onAuthenticate)
                case .harnesses:
                    HarnessesPreferencesSection(
                        store: store, onOpenTraceBrowser: onOpenTraceBrowser)
                case .credentials:
                    CredentialsPreferencesSection(
                        store: store,
                        onOpenTraceBrowser: onOpenTraceBrowser,
                        onOpenHarnesses: { state.section = .harnesses })
                case .dario:
                    DarioPreferencesSection(store: store, onOpenDario: onOpenDario)
                case .protection:
                    MiddlewarePreferencesSection(store: store, migratedFromFailover: true)
                case .middleware:
                    MiddlewarePreferencesSection(store: store)
                case .notifications:
                    NotificationsPreferencesSection(store: store)
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }

    static var appVersion: String {
        Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString")
            as? String ?? "dev"
    }

    static let issuesURL = URL(string: "https://github.com/madhavajay/alex/issues/new")!
    static let authorURL = URL(string: "https://github.com/madhavajay/")!
    static let authorXURL = URL(string: "https://x.com/madhavajay")!
}

/// Sidebar nav item (§1.25): icon 15px + 13px medium label, radius 8, active
/// bg overlay(0.1) with trailing chevron at 30%.
private struct SettingsNavItem: View {
    let label: String
    let icon: String
    let active: Bool
    let action: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            HStack(spacing: 9) {
                Image(systemName: icon)
                    .font(.system(size: 13, weight: .medium))
                    .foregroundStyle(iconColor)
                    .frame(width: 15, height: 15)
                Text(label)
                    .font(.system(size: 13, weight: .medium))
                    .foregroundStyle(labelColor)
                Spacer(minLength: 0)
                if active {
                    Image(systemName: "chevron.right")
                        .font(.system(size: 10, weight: .semibold))
                        .foregroundStyle(AlexTheme.Colors.foreground.opacity(0.3))
                }
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 7)
            .background(
                RoundedRectangle(cornerRadius: AlexTheme.Radius.md)
                    .fill(backgroundColor))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
    }

    private var iconColor: Color {
        if active { return AlexTheme.Colors.primary }
        return hovering ? AlexTheme.Colors.textTertiary : AlexTheme.Colors.textFaint
    }

    private var labelColor: Color {
        if active { return AlexTheme.Colors.foreground }
        return hovering ? AlexTheme.Colors.foreground : AlexTheme.Colors.textTertiary
    }

    private var backgroundColor: Color {
        if active { return AlexTheme.Colors.surfaceActive }
        return hovering ? AlexTheme.Colors.overlay(0.05) : .clear
    }
}

@MainActor
final class PreferencesWindowController {
    private var window: NSWindow?
    private let state = PreferencesViewState()
    private let store: SnapshotStore
    private let authWindows = AuthWindowController()
    private let onOpenDario: () -> Void
    private let onOpenTraceBrowser: (String) -> Void
    private let onRunOnboarding: () -> Void
    private let onResetCompleted: () -> Void

    init(
        store: SnapshotStore,
        onOpenDario: @escaping () -> Void = {},
        onOpenTraceBrowser: @escaping (String) -> Void = { _ in },
        onRunOnboarding: @escaping () -> Void = {},
        onResetCompleted: @escaping () -> Void = {}
    ) {
        self.store = store
        self.onOpenDario = onOpenDario
        self.onOpenTraceBrowser = onOpenTraceBrowser
        self.onRunOnboarding = onRunOnboarding
        self.onResetCompleted = onResetCompleted
    }

    func show(section: PreferencesSection = .general) {
        state.section = section
        if window == nil {
            let host = NSHostingController(rootView: PreferencesView(
                state: state,
                store: store,
                onAuthenticate: { [weak self] provider, name, autoIdentity, force in
                    guard let self else { return }
                    self.authWindows.show(
                        provider: provider, accountName: name, autoIdentity: autoIdentity,
                        force: force,
                        store: self.store)
                },
                onOpenDario: onOpenDario,
                onOpenTraceBrowser: onOpenTraceBrowser,
                onRunOnboarding: onRunOnboarding,
                onResetCompleted: onResetCompleted))
            let win = NSWindow(contentViewController: host)
            win.title = "Alex UI Settings"
            // Sidebar-hosted traffic lights per the Create Settings mock
            // (§1.30): content extends under a transparent titlebar.
            win.styleMask = [.titled, .closable, .resizable, .fullSizeContentView]
            win.titlebarAppearsTransparent = true
            win.titleVisibility = .hidden
            win.isReleasedWhenClosed = false
            win.setContentSize(NSSize(width: 960, height: 680))
            win.minSize = win.frame.size
            win.center()
            win.setFrameAutosaveName("AlexandriaPreferences")
            window = win
        }
        if let window {
            DockIconManager.shared.track(window)
            window.makeKeyAndOrderFront(nil)
        }
    }
}
