import SwiftUI
import AlexandriaBarCore

/// Shared layout constants for the hosted SwiftUI sections of the status menu
/// (mock panel width 340, ui/Design macOS system menu App.tsx:666-676).
enum MenuMetrics {
    static let width: CGFloat = 340
    static let inset: CGFloat = 12
}

// MARK: - Health

/// Health-badge status for menu icons, derived from account heartbeats
/// (mock PingStatus: ok #34c759 / slow #ff9500 / error #ff3b30 / pending).
enum MenuHealthStatus: Equatable {
    case ok
    case slow
    case error
    case pending

    var tint: Color {
        switch self {
        case .ok: AlexTheme.Colors.success
        case .slow: AlexTheme.Colors.warningOrange
        case .error: AlexTheme.Colors.destructive
        case .pending: AlexTheme.Colors.textTertiary
        }
    }

    /// Heartbeat latency at or above this reads as "slow" (mock: 310ms row).
    static let slowLatencyMs: Int64 = 300

    static func forAccount(_ account: Account, heartbeat: Heartbeat?) -> MenuHealthStatus {
        if account.status != "active" || heartbeat?.ok == false { return .error }
        if account.isExpired { return .slow }
        guard let heartbeat else { return .pending }
        if let latency = heartbeat.latencyMs, latency >= slowLatencyMs { return .slow }
        return .ok
    }

    static func forProvider(
        _ provider: String, accounts: [Account], heartbeats: [String: Heartbeat]
    ) -> MenuHealthStatus? {
        let statuses = accounts
            .filter { $0.provider == provider }
            .map { forAccount($0, heartbeat: heartbeats[$0.id]) }
        guard !statuses.isEmpty else { return nil }
        if statuses.contains(.error) { return .error }
        if statuses.contains(.slow) { return .slow }
        if statuses.allSatisfy({ $0 == .pending }) { return .pending }
        return .ok
    }
}

// MARK: - Header

/// Menu header (mock App.tsx:678-690): app name + daemon status dot on the
/// first line, app/daemon versions in faint mono on the second.
struct MenuHeaderView: View {
    let appVersion: String
    let daemonVersion: String
    let uptimeS: Int64
    let inFlight: Int
    /// Triggers an on-demand Sparkle update check; nil hides the refresh control.
    var onCheckUpdates: (() -> Void)? = nil

    var body: some View {
        VStack(spacing: 2) {
            HStack {
                Text("Alex UI")
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(AlexTheme.Colors.foreground)
                if let onCheckUpdates {
                    Button(action: onCheckUpdates) {
                        Image(systemName: "arrow.clockwise")
                            .font(.system(size: 10, weight: .semibold))
                            .foregroundStyle(AlexTheme.Colors.textFaint)
                    }
                    .buttonStyle(.plain)
                    .help("Check for updates")
                }
                Spacer()
                HStack(spacing: 5) {
                    StatusDot(tint: AlexTheme.Colors.success, size: 5)
                    Text(statusText)
                        .font(.system(size: 10))
                        .foregroundStyle(AlexTheme.Colors.success)
                }
            }
            HStack {
                Text("v\(appVersion)")
                Spacer()
                Text("daemon v\(daemonVersion)")
            }
            .font(AlexTheme.Fonts.mono(10))
            .foregroundStyle(AlexTheme.Colors.textFaint)
        }
        .padding(.horizontal, MenuMetrics.inset)
        .padding(.vertical, 10)
        .frame(width: MenuMetrics.width)
    }

    private var statusText: String {
        var text = "daemon up \(Format.duration(uptimeS))"
        if inFlight > 0 { text += " · \(inFlight) in flight" }
        return text
    }
}

// MARK: - Stats bar

/// 3-up stats strip under the header (mock App.tsx:696-708).
struct MenuStatsBarView: View {
    let totals: AnalyticsTotals

    var body: some View {
        StatTilesRow(items: [
            StatTileData(label: "requests", value: "\(totals.requests)"),
            StatTileData(label: "last hour", value: String(format: "$%.4f", totals.costUsd)),
            StatTileData(
                label: "errors", value: "\(totals.errors)",
                valueTint: totals.errors > 0 ? AlexTheme.Colors.destructive : nil),
        ])
        .frame(width: MenuMetrics.width)
    }
}

// MARK: - Update banner

/// Orange update band (mock App.tsx:592-635, `UpdateSection`). Shows an "App"
/// row (Sparkle), a "Daemon" row, or both — with a single trailing button
/// that reads "Update" for either alone or "Update Both" when both are
/// pending, matching the mock's `both ? "Update Both" : "Update"`.
struct MenuUpdateBannerView: View {
    var appVersion: String?
    var daemonVersion: String?
    var onUpdate: () -> Void
    var onLater: () -> Void

    private var orange: Color { AlexTheme.Colors.warningOrange }
    private var both: Bool { appVersion != nil && daemonVersion != nil }

    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            VStack(alignment: .leading, spacing: 4) {
                Text("UPDATE AVAILABLE")
                    .font(.system(size: 11, weight: .bold))
                    .tracking(0.55)
                    .foregroundStyle(orange)
                if let appVersion {
                    versionRow(label: "App", version: appVersion)
                }
                if let daemonVersion {
                    versionRow(label: "Daemon", version: daemonVersion)
                }
            }
            Spacer()
            VStack(alignment: .trailing, spacing: 8) {
                LaterButton(tint: orange, action: onLater)
                UpdateButton(tint: orange, label: both ? "Update Both" : "Update", action: onUpdate)
            }
        }
        .padding(.horizontal, MenuMetrics.inset)
        .padding(.vertical, 10)
        .frame(width: MenuMetrics.width)
        .background(orange.opacity(0.06))
        .overlay(alignment: .top) { Rectangle().fill(orange.opacity(0.15)).frame(height: 1) }
        .overlay(alignment: .bottom) { Rectangle().fill(orange.opacity(0.15)).frame(height: 1) }
    }

    private func versionRow(label: String, version: String) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 10) {
            Text(label)
                .font(.system(size: 11))
                .foregroundStyle(orange.opacity(0.5))
                .frame(width: 44, alignment: .leading)
            Text(version)
                .font(AlexTheme.Fonts.mono(12, weight: .semibold))
                .foregroundStyle(orange)
        }
    }

    private struct LaterButton: View {
        let tint: Color
        let action: () -> Void
        @State private var hovering = false

        var body: some View {
            Button(action: action) {
                Text("Later")
                    .font(.system(size: 11))
                    .foregroundStyle(tint.opacity(hovering ? 0.7 : 0.4))
                    .padding(.horizontal, 2)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .onHover { hovering = $0 }
        }
    }

    private struct UpdateButton: View {
        let tint: Color
        var label: String = "Update"
        let action: () -> Void
        @State private var hovering = false

        var body: some View {
            Button(action: action) {
                Text(label)
                    .font(.system(size: 11, weight: .bold))
                    .foregroundStyle(AlexTheme.Colors.background)
                    .padding(.horizontal, 11)
                    .padding(.vertical, 4)
                    .background(
                        RoundedRectangle(cornerRadius: 7)
                            .fill(hovering ? tint.opacity(0.85) : tint))
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .onHover { hovering = $0 }
        }
    }
}

// MARK: - Section label

/// Hosted uppercase section label row (mock SectionLabel, App.tsx:262-268).
struct MenuSectionLabelView: View {
    let text: String

    var body: some View {
        HStack {
            SectionLabel(text: text, style: .menu)
            Spacer()
        }
        .padding(.horizontal, MenuMetrics.inset)
        .padding(.top, 9)
        .padding(.bottom, 3)
        .frame(width: MenuMetrics.width, alignment: .leading)
    }
}

/// Recent daemon sessions in the mock's compact Traces section
/// (App.tsx:543-588), with the browser affordance kept outside the footer.
struct MenuTracesSectionView: View {
    let sessions: [TraceSession]
    let onOpen: () -> Void
    let onOpenSession: (String) -> Void

    var body: some View {
        VStack(spacing: 0) {
            SectionLabel(text: "Traces", style: .menu) {
                MenuMiniButton(
                    label: "Open Browser", systemImage: "scope", action: onOpen)
            }
            .padding(.horizontal, MenuMetrics.inset)
            .padding(.top, 9)
            .padding(.bottom, 4)

            if sessions.isEmpty {
                Text("No recent traces")
                    .font(.system(size: 11))
                    .foregroundStyle(AlexTheme.Colors.textFaint)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, MenuMetrics.inset)
                    .padding(.vertical, 6)
            } else {
                ForEach(sessions.prefix(4)) { session in
                    MenuTraceRowView(session: session) {
                        onOpenSession(session.sessionId)
                    }
                }
            }
        }
        .padding(.bottom, 6)
        .frame(width: MenuMetrics.width)
    }
}

private struct MenuTraceRowView: View {
    let session: TraceSession
    let onOpen: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: onOpen) {
            HStack(spacing: 7) {
                StatusDot(
                    tint: (session.errors ?? 0) > 0
                        ? AlexTheme.Colors.destructive : AlexTheme.Colors.success,
                    size: 5)
                Text(label)
                    .font(.system(size: 11))
                    .foregroundStyle(AlexTheme.Colors.textSecondary)
                    .lineLimit(1)
                    .truncationMode(.tail)
                    .frame(maxWidth: .infinity, alignment: .leading)
                providerIcon
                Text(duration)
                    .font(AlexTheme.Fonts.mono(10))
                    .foregroundStyle(AlexTheme.Colors.textFaint)
                    .frame(width: 38, alignment: .trailing)
                Text(age)
                    .font(AlexTheme.Fonts.mono(10))
                    .foregroundStyle(AlexTheme.Colors.textFaint)
                    .frame(width: 26, alignment: .trailing)
            }
            .padding(.horizontal, MenuMetrics.inset)
            .padding(.vertical, 5)
            .background(hovering ? AlexTheme.Colors.surfaceHover : .clear)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
    }

    @ViewBuilder
    private var providerIcon: some View {
        if let provider = primaryProvider {
            if let alias = ProviderMenuIcon.harnessAlias[provider],
               HarnessIconLoader.image(harness: alias, tags: nil) != nil
            {
                HarnessIconView(
                    harness: alias, tags: nil, size: 17,
                    background: ProviderMenuIcon.background[provider], cornerRadius: 3)
            } else {
                ProviderBadgeView(provider: provider, size: 17, style: .tinted)
            }
        } else {
            HarnessIconView(
                harness: session.harness, tags: session.tags,
                size: 17, showsFallback: true)
        }
    }

    private var primaryProvider: String? {
        let providers = session.providers?.isEmpty == false
            ? session.providers ?? []
            : ModelProvider.providers(in: session.models)
        return SessionIdentity.primaryProvider(
            providers: providers, harness: session.harness, tags: session.tags)
    }

    private var label: String {
        if let task = session.tags?["task"]?.trimmingCharacters(
            in: .whitespacesAndNewlines), !task.isEmpty
        {
            return task
        }
        let harness = HarnessName.display(harness: session.harness, tags: session.tags)
        return "\(harness) · \(shortSessionID)"
    }

    private var shortSessionID: String {
        let id = session.sessionId
        guard id.count > 22 else { return id }
        return "\(id.prefix(10))…\(id.suffix(8))"
    }

    private var duration: String {
        let milliseconds = max(0, session.lastTsMs - session.firstTsMs)
        if milliseconds < 60_000 {
            return String(format: "%.1fs", Double(milliseconds) / 1_000)
        }
        return SessionDuration.format(ms: milliseconds)
    }

    private var age: String {
        let seconds = max(
            0, Int64(Date().timeIntervalSince1970) - session.lastTsMs / 1_000)
        if seconds < 10 { return "now" }
        if seconds < 60 { return "\(seconds)s" }
        if seconds < 3_600 { return "\(seconds / 60)m" }
        if seconds < 86_400 { return "\(seconds / 3_600)h" }
        return "\(seconds / 86_400)d"
    }
}

/// Collapsible native-row section header. The controller owns persistence and
/// rebuilds the menu after this hosted button toggles.
struct MenuCollapsibleSectionHeaderView: View {
    let title: String
    let itemCount: Int
    let singularItemName: String
    let isExpanded: Bool
    let onToggle: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: onToggle) {
            SectionLabel(text: title, style: .menu) {
                HStack(spacing: 5) {
                    if !isExpanded {
                        Text("\(itemCount) \(itemName)")
                            .font(.system(size: 10))
                    }
                    Image(systemName: isExpanded ? "chevron.down" : "chevron.right")
                        .font(.system(size: 8, weight: .semibold))
                }
                .foregroundStyle(
                    hovering ? AlexTheme.Colors.textTertiary : AlexTheme.Colors.textFaint)
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .padding(.horizontal, MenuMetrics.inset)
        .padding(.top, 9)
        .padding(.bottom, 3)
        .frame(width: MenuMetrics.width)
    }

    private var itemName: String {
        itemCount == 1 ? singularItemName : "\(singularItemName)s"
    }
}

/// Small hover-wash text button used in hosted section headers
/// (mock "Refresh"/"Ping", App.tsx:712-733).
struct MenuMiniButton: View {
    let label: String
    var systemImage: String?
    let action: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            HStack(spacing: 4) {
                if let systemImage {
                    Image(systemName: systemImage)
                        .font(.system(size: 9, weight: .medium))
                }
                Text(label)
                    .font(.system(size: 10))
            }
            .foregroundStyle(AlexTheme.Colors.textTertiary)
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(
                RoundedRectangle(cornerRadius: 5)
                    .fill(hovering ? AlexTheme.Colors.overlay(0.08) : .clear))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
    }
}

/// Async refresh control with the same short-lived confirmation pattern as
/// `CopyButton`: spinner while awaiting the snapshot, then success/failure
/// feedback for two seconds before returning to its idle label.
struct MenuRefreshButton: View {
    private enum Phase: Equatable {
        case idle
        case refreshing
        case refreshed
        case failed
    }

    let action: @MainActor () async -> Bool
    @State private var phase: Phase = .idle
    @State private var hovering = false
    @State private var refreshTask: Task<Void, Never>?

    var body: some View {
        Button(action: refresh) {
            HStack(spacing: 4) {
                switch phase {
                case .idle:
                    Image(systemName: "arrow.clockwise")
                        .font(.system(size: 9, weight: .medium))
                    Text("Refresh")
                case .refreshing:
                    ProgressView()
                        .controlSize(.mini)
                    Text("Refreshing…")
                case .refreshed:
                    Image(systemName: "checkmark")
                        .font(.system(size: 9, weight: .semibold))
                    Text("Refreshed")
                case .failed:
                    Image(systemName: "exclamationmark")
                        .font(.system(size: 9, weight: .semibold))
                    Text("Refresh failed")
                }
            }
            .font(.system(size: 10))
            .foregroundStyle(foreground)
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(
                RoundedRectangle(cornerRadius: 5)
                    .fill(hovering ? AlexTheme.Colors.overlay(0.08) : .clear))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(phase == .refreshing)
        .onHover { hovering = $0 }
        .onDisappear { refreshTask?.cancel() }
    }

    private var foreground: Color {
        switch phase {
        case .refreshed: AlexTheme.Colors.success
        case .failed: AlexTheme.Colors.destructive
        case .idle, .refreshing: AlexTheme.Colors.textTertiary
        }
    }

    private func refresh() {
        guard phase != .refreshing else { return }
        refreshTask?.cancel()
        withAnimation(.easeInOut(duration: 0.15)) { phase = .refreshing }
        refreshTask = Task {
            let succeeded = await action()
            guard !Task.isCancelled else { return }
            withAnimation(.easeInOut(duration: 0.15)) {
                phase = succeeded ? .refreshed : .failed
            }
            try? await Task.sleep(for: .seconds(2))
            guard !Task.isCancelled else { return }
            withAnimation(.easeInOut(duration: 0.15)) { phase = .idle }
        }
    }
}

// MARK: - Providers card

/// The Providers section of the status menu (mock App.tsx:710-744):
/// section header with Refresh/Ping, then one row per provider with a brand
/// icon (health-badged), quota bars, credit balances, the bonded multi-account
/// layout for Codex, and the Dario agent chip under Claude.
struct LimitsCardView: View {
    let limits: [ProviderLimits]
    let accounts: [Account]
    let warnPct: Double
    var heartbeats: [String: Heartbeat] = [:]
    var routing: [String: ProviderRoutingResponse] = [:]
    var dario: DarioStatus? = nil
    var onRefresh: (@MainActor () async -> Bool)? = nil
    var onPing: (() -> Void)? = nil
    var onOpenDario: (() -> Void)? = nil

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            headerRow
            ForEach(Array(displayProviders.enumerated()), id: \.element) { index, name in
                if index > 0 {
                    Rectangle()
                        .fill(AlexTheme.Colors.hairline)
                        .frame(height: 1)
                        .padding(.horizontal, MenuMetrics.inset)
                }
                if name == "openai", !codexAccounts.isEmpty {
                    bondedSection(provider: name, slots: codexAccounts)
                } else if let provider = visibleLimits.first(where: { $0.provider == name }) {
                    providerSection(provider)
                }
            }
        }
        .padding(.bottom, 4)
        .frame(width: MenuMetrics.width, alignment: .leading)
    }

    private var headerRow: some View {
        SectionLabel(text: "Providers", style: .menu) {
            if let onRefresh {
                MenuRefreshButton(action: onRefresh)
                    .help("Refresh Now ⌘R")
            }
            if onRefresh != nil, onPing != nil {
                Rectangle()
                    .fill(AlexTheme.Colors.overlay(0.08))
                    .frame(width: 1, height: 10)
            }
            if let onPing {
                MenuMiniButton(
                    label: "Ping", systemImage: "dot.radiowaves.left.and.right", action: onPing)
                    .help("Run Ping Checks")
            }
        }
        .padding(.horizontal, MenuMetrics.inset)
        .padding(.top, 9)
        .padding(.bottom, 3)
    }

    private var visibleLimits: [ProviderLimits] {
        ProviderPresentation.visibleLimits(limits, for: accounts)
    }

    /// Keep the provider card's stable order while ensuring Codex accounts are
    /// visible even before a provider-wide response-header snapshot exists.
    private var displayProviders: [String] {
        var providers = Set(visibleLimits.map(\.provider))
        if !codexAccounts.isEmpty { providers.insert("openai") }
        return providers.sorted()
    }

    private var codexAccounts: [Account] {
        accounts
            .filter { $0.provider == "openai" && $0.kind == "oauth" }
            .sorted {
                let lhs = $0.email ?? $0.name
                let rhs = $1.email ?? $1.name
                if lhs == rhs { return $0.id < $1.id }
                return lhs.localizedCaseInsensitiveCompare(rhs) == .orderedAscending
            }
    }

    // MARK: Single provider row

    @ViewBuilder
    private func providerSection(_ provider: ProviderLimits) -> some View {
        let brand = AlexTheme.ProviderBrand.brand(for: provider.provider).accent
        VStack(alignment: .leading, spacing: 5) {
            providerHeaderRow(provider.provider) {
                if let plan = provider.plan {
                    Text(plan)
                        .font(.system(size: 10))
                        .foregroundStyle(AlexTheme.Colors.textTertiary)
                }
            }
            if let error = provider.error {
                errorText(error)
                    .padding(.leading, 21)
            }
            VStack(alignment: .leading, spacing: 4) {
                quotaRow(provider.quota)
                ForEach(provider.windows ?? [], id: \.window) { window in
                    if shouldHide(window, for: provider.quota) {
                        EmptyView()
                    } else if window.window == "credits" || window.window.hasPrefix("ws:") {
                        balanceLine(window)
                    } else {
                        windowRow(
                            window,
                            label: secondaryLabel(window, quota: provider.quota),
                            brand: brand)
                    }
                }
                if provider.windows?.isEmpty != false, let requests = provider.requests {
                    countRow("requests", requests)
                    if let tokens = provider.tokens {
                        countRow("tokens", tokens)
                    }
                }
            }
            .padding(.leading, 21)
            if provider.provider == "anthropic" {
                agentChip
            }
        }
        .padding(.horizontal, MenuMetrics.inset)
        .padding(.vertical, 7)
    }

    // MARK: Bonded (multi-account) provider

    @ViewBuilder
    private func bondedSection(provider name: String, slots: [Account]) -> some View {
        let brand = AlexTheme.ProviderBrand.brand(for: name).accent
        VStack(alignment: .leading, spacing: 5) {
            providerHeaderRow(name) {
                if slots.count > 1, let strategy = routing[name]?.strategy {
                    modeChip(strategy)
                }
            }
            VStack(alignment: .leading, spacing: 7) {
                ForEach(Array(slots.enumerated()), id: \.element.id) { index, account in
                    slotView(index: index, account: account, brand: brand)
                }
            }
            .padding(.leading, 21)
        }
        .padding(.horizontal, MenuMetrics.inset)
        .padding(.vertical, 7)
    }

    @ViewBuilder
    private func slotView(index: Int, account: Account, brand: Color) -> some View {
        let accountLimits = account.limits
        let windows = accountLimits?.windows ?? []
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 5) {
                Text("\(index + 1)")
                    .font(AlexTheme.Fonts.mono(9))
                    .foregroundStyle(AlexTheme.Colors.textFaint)
                    .frame(width: 10, alignment: .leading)
                Text(account.email ?? account.name)
                    .font(.system(size: 10))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
                    .lineLimit(1)
                    .truncationMode(.middle)
                Spacer(minLength: 4)
                if let plan = accountLimits?.plan {
                    Text(plan)
                        .font(AlexTheme.Fonts.mono(9))
                        .foregroundStyle(AlexTheme.Colors.textFaint)
                }
            }
            if account.paused {
                Text("Paused · not used for proxy routing")
                    .font(.system(size: 9, weight: .medium))
                    .foregroundStyle(AlexTheme.Colors.warningOrange)
            }
            if let error = accountLimits?.error {
                errorText(error)
            }
            quotaRow(accountLimits?.quota)
            if windows.isEmpty, accountLimits?.error == nil {
                Text("Waiting for quota data from this account")
                    .font(.system(size: 9))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
            }
            ForEach(windows, id: \.window) { window in
                if shouldHide(window, for: accountLimits?.quota) {
                    EmptyView()
                } else if window.window == "credits" || window.window.hasPrefix("ws:") {
                    balanceLine(window)
                } else {
                    windowRow(
                        window,
                        label: secondaryLabel(window, quota: accountLimits?.quota),
                        brand: brand)
                }
            }
            if windows.isEmpty, let requests = accountLimits?.requests {
                countRow("requests", requests)
                if let tokens = accountLimits?.tokens {
                    countRow("tokens", tokens)
                }
            }
        }
        .padding(.leading, 8)
        .overlay(alignment: .leading) {
            Rectangle()
                .fill(brand.opacity(0.3))
                .frame(width: 1.5)
        }
    }

    // MARK: Row pieces

    @ViewBuilder
    private func providerHeaderRow(
        _ provider: String, @ViewBuilder trailing: () -> some View
    ) -> some View {
        HStack(spacing: 7) {
            providerIcon(provider)
            Text(ProviderInfo.displayName(provider))
                .font(.system(size: 11, weight: .semibold))
                .foregroundStyle(AlexTheme.Colors.foreground)
            Text(ProviderInfo.loginArg(provider))
                .font(AlexTheme.Fonts.mono(10))
                .foregroundStyle(AlexTheme.Colors.textFaint)
            Spacer()
            trailing()
        }
    }

    @ViewBuilder
    private func providerIcon(_ provider: String, size: CGFloat = 14) -> some View {
        let health = MenuHealthStatus.forProvider(
            provider, accounts: accounts, heartbeats: heartbeats)
        if let health {
            IconWithHealthBadge(size: size, tint: health.tint, pending: health == .pending) {
                baseProviderIcon(provider, size: size)
            }
        } else {
            baseProviderIcon(provider, size: size)
        }
    }

    @ViewBuilder
    private func baseProviderIcon(_ provider: String, size: CGFloat) -> some View {
        if let alias = ProviderMenuIcon.harnessAlias[provider],
           HarnessIconLoader.image(harness: alias, tags: nil) != nil
        {
            HarnessIconView(
                harness: alias, tags: nil, size: size,
                background: ProviderMenuIcon.background[provider], cornerRadius: 3)
        } else {
            ProviderBadgeView(provider: provider, size: size)
        }
    }

    private func modeChip(_ strategy: ProviderRoutingStrategy) -> some View {
        let (label, symbol): (String, String) = switch strategy {
        case .roundRobin: ("Round Robin", "shuffle")
        case .resetFirst: ("Expires First", "clock")
        case .priority: ("Priority", "list.number")
        }
        return HStack(spacing: 4) {
            Image(systemName: symbol)
                .font(.system(size: 8, weight: .medium))
            Text(label)
                .font(.system(size: 9, weight: .medium))
        }
        .foregroundStyle(AlexTheme.Colors.textTertiary)
        .padding(.horizontal, 6)
        .padding(.vertical, 2)
        .background(RoundedRectangle(cornerRadius: 5).fill(AlexTheme.Colors.overlay(0.07)))
    }

    /// Dario status as a plain line under the Claude row (not a flyout): a
    /// status dot + version + phase/latency, matching the pre-restyle layout.
    @ViewBuilder
    private var agentChip: some View {
        if let dario,
           let active = dario.generations.first(where: { $0.id == dario.activeGenerationId })
        {
            let tint = agentTint(active)
            HStack(spacing: 6) {
                StatusDot(tint: tint, size: 5)
                Text("Dario")
                    .font(.system(size: 10, weight: .medium))
                    .foregroundStyle(AlexTheme.Colors.textSecondary)
                Text("v\(active.version)")
                    .font(AlexTheme.Fonts.mono(10))
                    .foregroundStyle(AlexTheme.Colors.textFaint)
                Spacer()
                Text(agentStatusText(active))
                    .font(.system(size: 9))
                    .foregroundStyle(tint)
            }
            .padding(.leading, 21)
            .padding(.vertical, 1)
        }
    }

    private func agentTint(_ generation: DarioGeneration) -> Color {
        if generation.lastProbe?.ok == false { return AlexTheme.Colors.destructive }
        if generation.phase == "ready" { return AlexTheme.Colors.success }
        return AlexTheme.Colors.warningOrange
    }

    private func agentStatusText(_ generation: DarioGeneration) -> String {
        if let probe = generation.lastProbe, probe.ok, let ms = probe.latencyMs {
            return "\(generation.phase) · \(ms)ms"
        }
        return generation.phase
    }

    @ViewBuilder
    private func quotaRow(_ quota: QuotaState?) -> some View {
        if let quota, quota.isCreditPrimary {
            switch quota.kind {
            case "out_of_credits":
                VStack(alignment: .leading, spacing: 2) {
                    Text("OUT OF CREDITS")
                        .font(.system(size: 10, weight: .bold))
                        .foregroundStyle(AlexTheme.Colors.destructive)
                    if let url = quota.topUpURL, !url.isEmpty {
                        Text("Top up: \(url)")
                            .font(.system(size: 9))
                            .foregroundStyle(AlexTheme.Colors.textTertiary)
                            .lineLimit(1)
                    }
                }
            case "unlimited":
                creditLine("Credit balance", value: "Unlimited")
            case "balance":
                creditLine("Credit balance", value: quota.balance ?? "—")
            case "credit_window":
                QuotaBarRow(
                    fraction: (quota.remainingPct ?? 0) / 100,
                    fill: AlexTheme.Colors.success,
                    percentText: "\(Int((quota.remainingPct ?? 0).rounded()))%",
                    timeLeftText: nil,
                    warnBelow: 0.01,
                    leadingLabel: quota.label,
                    leadingLabelWidth: 78)
            default:
                EmptyView()
            }
        }
    }

    /// One limit window: small mono label + shared QuotaBarRow (bar, percent,
    /// time-to-reset).
    private func windowRow(_ window: LimitWindow, label: String?, brand: Color) -> some View {
        QuotaBarRow(
            fraction: (window.remainingPct ?? 0) / 100,
            fill: fillColor(window, brand: brand),
            percentText: window.remainingPct.map { "\(Int($0.rounded()))%" } ?? "—",
            timeLeftText: window.resetsDate.map { Format.countdown(to: $0) },
            warnBelow: nil,
            leadingLabel: label ?? window.window)
    }

    /// Keeps the existing warn-threshold semantics: healthy windows fill with
    /// the provider brand color (mock), low windows warn orange/red.
    private func fillColor(_ window: LimitWindow, brand: Color) -> Color {
        switch window.remainingSeverity(warnUsedPct: warnPct) {
        case .critical: AlexTheme.Colors.destructive
        case .warning: AlexTheme.Colors.warningOrange
        case .healthy: brand
        case nil: AlexTheme.Colors.overlay(0.2)
        }
    }

    /// Credit balance line (mock App.tsx:280-285).
    private func creditLine(_ label: String, value: String) -> some View {
        HStack(spacing: 8) {
            Text(label)
                .font(.system(size: 10))
                .foregroundStyle(AlexTheme.Colors.textTertiary)
            Text(value)
                .font(AlexTheme.Fonts.mono(10, weight: .semibold))
                .foregroundStyle(AlexTheme.Colors.success)
            Spacer()
        }
    }

    @ViewBuilder
    private func balanceLine(_ window: LimitWindow) -> some View {
        let label = window.window == "credits" ? "Credit balance" : window.window
        if let usd = window.remainingUsd {
            creditLine(label, value: String(format: "$%.2f", usd))
        } else {
            HStack(spacing: 8) {
                Text(label)
                    .font(.system(size: 10))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
                Text("—")
                    .font(AlexTheme.Fonts.mono(10))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
                Spacer()
            }
        }
    }

    private func countRow(_ label: String, _ pair: CountPair) -> some View {
        HStack(spacing: 8) {
            Text(label)
                .font(.system(size: 10))
                .foregroundStyle(AlexTheme.Colors.textTertiary)
            Text("\(pair.remaining ?? 0) / \(pair.limit ?? 0) remaining")
                .font(AlexTheme.Fonts.mono(10))
                .foregroundStyle(AlexTheme.Colors.textSecondary)
        }
    }

    private func errorText(_ error: String) -> some View {
        Text(error)
            .font(.system(size: 10))
            .foregroundStyle(AlexTheme.Colors.warningOrange)
            .lineLimit(2)
    }

    private func shouldHide(_ window: LimitWindow, for quota: QuotaState?) -> Bool {
        guard let quota, quota.isCreditPrimary else { return false }
        return quota.kind == "credit_window" || window.window == "credits"
    }

    private func secondaryLabel(_ window: LimitWindow, quota: QuotaState?) -> String? {
        quota?.isCreditPrimary == true && !window.window.hasPrefix("ws:")
            ? "rate \(window.window)"
            : nil
    }
}

/// Provider → brand-icon lookup for the menu (the mock shows the harness brand
/// logos for providers: Claude, Codex, Grok, Gemini, Amp).
enum ProviderMenuIcon {
    static let harnessAlias: [String: String] = [
        "anthropic": "claude",
        "openai": "codex",
        "xai": "grok",
        "gemini": "gemini",
        "amp": "amp",
    ]

    /// Brand tile backgrounds so dark logos stay legible (mock: Claude on
    /// #f5f4ef, Codex on white).
    static let background: [String: Color] = [
        "anthropic": AlexTheme.Colors.dynamic(light: 0xF5F4EF, dark: 0xF5F4EF),
        "openai": .white,
    ]
}
