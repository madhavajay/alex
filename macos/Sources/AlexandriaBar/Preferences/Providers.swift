import AppKit
import SwiftUI
import AlexandriaBarCore

private enum ProvidersChartRange: String, CaseIterable {
    case twentyFourHours = "24h"
    case sevenDays = "7d"
    case thirtyDays = "30d"
    case all = "All"

    var sinceMinutes: Int {
        switch self {
        case .twentyFourHours: 1_440
        case .sevenDays: 10_080
        case .thirtyDays, .all: 43_200
        }
    }

    var bucketMinutes: Int {
        switch self {
        case .twentyFourHours: 60
        case .sevenDays: 360
        case .thirtyDays, .all: 1_440
        }
    }

    /// Ascending lookback spans, matching `allCases` order — the input
    /// `UsageChartMath.enabledRanges` needs to tell which tabs actually
    /// reveal more history than the tab before them.
    static var spansMinutes: [Int] { allCases.map(\.sinceMinutes) }
}

/// The Preferences → Providers tab: provider sidebar, per-account usage and
/// quota, and routing rules. Restyled to the "Accounts section with graph"
/// design (ui/Accounts section with graph/src/app/App.tsx) on top of the
/// existing data wiring.
struct ProvidersPreferencesSection: View {
    let store: SnapshotStore
    let onAuthenticate: (String, String?, Bool) -> Void
    /// Deep-link to another Preferences tab (used by "Add Exo", which is a local
    /// cluster configured by endpoint URL rather than an OAuth/key login).
    var onOpenSection: (PreferencesSection) -> Void = { _ in }
    @State private var providerToAdd: String?
    @State private var selectedProvider: String? = "openai"
    @AppStorage("ProvidersChartRange") private var chartRangeValue =
        ProvidersChartRange.twentyFourHours.rawValue
    @State private var accountAnalyticsLoadID: UUID?
    /// Earliest bucket timestamp seen across a one-off 30d fetch, used to
    /// decide which range tabs have enough history to be worth showing.
    /// `nil` until loaded (or if the lookup fails) — `enabledRanges` fails
    /// open in that case, so every tab stays enabled rather than getting
    /// stuck disabled.
    @State private var earliestActivityMs: Int64?

    private var chartRange: ProvidersChartRange {
        ProvidersChartRange(rawValue: chartRangeValue) ?? .twentyFourHours
    }

    /// Per-tab enabled state for the 24h/7d/30d/All chart-range switcher —
    /// see `UsageChartMath.enabledRanges`.
    private var enabledRanges: [Bool] {
        UsageChartMath.enabledRanges(
            spansMinutes: ProvidersChartRange.spansMinutes,
            earliestActivityMs: earliestActivityMs,
            nowMs: Int64(Date().timeIntervalSince1970 * 1_000))
    }

    private var chartRangeSelection: Binding<Int> {
        Binding(
            get: { ProvidersChartRange.allCases.firstIndex(of: chartRange) ?? 0 },
            set: { index in
                guard ProvidersChartRange.allCases.indices.contains(index) else { return }
                chartRangeValue = ProvidersChartRange.allCases[index].rawValue
            })
    }

    /// Providers that get a detail panel in the sidebar. Exo is intentionally
    /// omitted: it is a local cluster with its own Preferences tab and no
    /// per-account credentials, so it lives in the Add menu (deep-linking to
    /// that tab) rather than as a usage/routing panel here.
    private var providers: [String] {
        Array(Set(["anthropic", "openai", "gemini", "xai", "kimi", "openrouter", "amp"] + store.accounts.map(\.provider))).sorted {
            ProviderInfo.displayName($0) < ProviderInfo.displayName($1)
        }
    }

    /// Every provider the Add menu can start a setup flow for, in a stable
    /// display order. Reconciled against `ProviderInfo.supportedProviders`
    /// (+ kimi). Setup type varies by provider — see `addAccount`:
    /// OAuth-login (anthropic/openai/gemini/xai/kimi), API-key or CLI import
    /// (openrouter/amp), and local endpoint config (exo).
    private var connectableProviders: [String] {
        ["anthropic", "openai", "gemini", "xai", "kimi", "openrouter", "amp", "exo"]
    }

    private var usageByAccount: [String: AccountUsage] {
        Dictionary(uniqueKeysWithValues: (store.accountAnalytics?.byAccount ?? []).map { ($0.accountId, $0) })
    }

    /// OAuth without a supplied local name uses the compatible `default`
    /// account id. Codex is the exception: its automatic identity flow gives
    /// each upstream account a distinct generated local id.
    private var addableProviders: [String] {
        connectableProviders.filter {
            // openai supports multiple accounts; exo is reconfigurable (no
            // account concept). Everything else hides once it has an account.
            $0 == "openai" || $0 == "exo"
                || !ProviderPresentation.hasAccount(for: $0, in: store.accounts)
        }
    }

    var body: some View {
        VStack(spacing: 0) {
            if ProviderPresentation.hasNoAccounts(store.accounts) {
                HStack(spacing: 12) {
                    VStack(alignment: .leading, spacing: 3) {
                        Text("Connect a Token Provider")
                            .font(.system(size: 13, weight: .semibold))
                            .foregroundStyle(AlexTheme.Colors.foreground)
                        Text("Connect an account to see its usage and routing settings.")
                            .font(.system(size: 11))
                            .foregroundStyle(AlexTheme.Colors.textTertiary)
                    }
                    Spacer()
                    addProviderMenu(prominent: true)
                }
                .padding(16)
                Rectangle().fill(AlexTheme.Colors.cardBorder).frame(height: 1)
            }

            HStack(spacing: 0) {
                sidebar
                Rectangle().fill(AlexTheme.Colors.cardBorder).frame(width: 1)

                if let provider = selectedProvider {
                    ProviderPreferencesDetail(
                        provider: provider,
                        store: store,
                        usageByAccount: usageByAccount,
                        chartRangeSelection: chartRangeSelection,
                        chartRangeEnabled: enabledRanges,
                        accountAnalyticsLoading: accountAnalyticsLoadID != nil,
                        onConnect: addAccount,
                        onAuthenticate: onAuthenticate)
                } else {
                    EmptyStateView(
                        message: "Choose a provider",
                        style: .panel(icon: "network"))
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                }
            }
        }
        .background(AlexTheme.Colors.background)
        .task(id: chartRange) {
            let loadID = UUID()
            accountAnalyticsLoadID = loadID
            await store.refreshAccountAnalytics(
                sinceMinutes: chartRange.sinceMinutes,
                bucketMinutes: chartRange.bucketMinutes)
            if accountAnalyticsLoadID == loadID {
                accountAnalyticsLoadID = nil
            }
        }
        .task {
            await loadEarliestActivity()
        }
        .onChange(of: earliestActivityMs) { _, _ in
            guard let index = ProvidersChartRange.allCases.firstIndex(of: chartRange),
                  enabledRanges.indices.contains(index), !enabledRanges[index]
            else { return }
            chartRangeValue = ProvidersChartRange.twentyFourHours.rawValue
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

    /// Provider sidebar (§1.26 of the design spec): brand dot rows with a
    /// 2px accent left border when active, count badge, dashed add footer.
    private var sidebar: some View {
        VStack(spacing: 0) {
            HStack {
                Text("Providers")
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(AlexTheme.Colors.foreground)
                Spacer()
            }
            .padding(12)
            .overlay(alignment: .bottom) {
                Rectangle().fill(AlexTheme.Colors.hairline).frame(height: 1)
            }

            ScrollView {
                VStack(spacing: 0) {
                    ForEach(providers, id: \.self) { provider in
                        ProviderSidebarRow(
                            provider: provider,
                            count: store.accounts.filter { $0.provider == provider }.count,
                            selected: selectedProvider == provider
                        ) {
                            selectedProvider = provider
                        }
                    }
                }
                .padding(.vertical, 4)
            }

            Rectangle().fill(AlexTheme.Colors.cardBorder).frame(height: 1)
            addProviderMenu()
                .padding(12)
        }
        .frame(width: 180)
    }

    @ViewBuilder
    private func addProviderMenu(prominent: Bool = false) -> some View {
        Menu {
            ForEach(addableProviders, id: \.self) { provider in
                Button("Add \(ProviderInfo.displayName(provider))") {
                    addAccount(provider)
                }
            }
        } label: {
            DashedAddLabel(
                title: prominent ? "+ Connect a Token Provider" : "+ Add provider",
                cornerRadius: AlexTheme.Radius.md,
                fontSize: 11,
                verticalPadding: 7,
                horizontalPadding: prominent ? 12 : 0,
                fillsWidth: !prominent)
        }
        .menuStyle(.button)
        .buttonStyle(.plain)
        .menuIndicator(.hidden)
        .fixedSize(horizontal: prominent, vertical: true)
        .help("Add a token provider")
    }

    private func addAccount(_ provider: String) {
        switch provider {
        case "exo":
            // Local cluster: no login. Deep-link to its endpoint/config tab.
            onOpenSection(.exo)
        case _ where ProviderInfo.usesAPIKeySheet(provider):
            // API-key providers (openrouter) capture a long-lived key.
            providerToAdd = provider
        default:
            // OAuth-login providers (anthropic/openai/gemini/xai/kimi) run the
            // device/browser auth flow; amp adopts its CLI credentials via the
            // same window's import path. OAuth providers capture their account
            // email during login; the local account name stays the compatible
            // `default` identifier.
            onAuthenticate(provider, nil, provider == "openai")
        }
    }

    /// One-off, range-independent lookup of how far back activity exists
    /// (across all providers/accounts), so the chart-range tabs can be
    /// disabled where they wouldn't reveal anything new. Runs once per
    /// section appearance; failures leave `earliestActivityMs` nil, which
    /// `enabledRanges` treats as "unknown" and fails open.
    private func loadEarliestActivity() async {
        guard let config = store.config else { return }
        do {
            let response = try await AlexandriaClient(config: config)
                .accountAnalytics(sinceMinutes: 43_200, bucketMinutes: 1_440)
            earliestActivityMs = response.series.map(\.bucketMs).min()
        } catch {
            earliestActivityMs = nil
        }
    }
}

private struct ProviderSidebarRow: View {
    let provider: String
    let count: Int
    let selected: Bool
    let action: () -> Void
    @State private var hovering = false

    private var accent: Color { AlexTheme.ProviderBrand.brand(for: provider).accent }

    var body: some View {
        Button(action: action) {
            HStack(spacing: 8) {
                StatusDot(tint: accent, size: 8)
                Text(ProviderInfo.displayName(provider))
                    .font(.system(size: 12, weight: selected ? .semibold : .regular))
                    .foregroundStyle(
                        selected ? AlexTheme.Colors.foreground : AlexTheme.Colors.textSecondary)
                Spacer(minLength: 4)
                if count > 0 {
                    Text("\(count)")
                        .font(AlexTheme.Fonts.mono(10, weight: .medium))
                        .foregroundStyle(selected ? .white : AlexTheme.Colors.textTertiary)
                        .padding(.horizontal, 6)
                        .padding(.vertical, 1)
                        .background(
                            RoundedRectangle(cornerRadius: AlexTheme.Radius.xs)
                                .fill(selected ? accent : AlexTheme.Colors.overlay(0.08)))
                }
            }
            .padding(.vertical, 7)
            .padding(.horizontal, 10)
            .background(
                selected
                    ? AlexTheme.Colors.overlay(0.08)
                    : (hovering ? AlexTheme.Colors.overlay(0.04) : .clear))
            .overlay(alignment: .leading) {
                Rectangle()
                    .fill(selected ? accent : .clear)
                    .frame(width: 2)
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
    }
}

private struct ProviderPreferencesDetail: View {
    let provider: String
    let store: SnapshotStore
    let usageByAccount: [String: AccountUsage]
    @Binding var chartRangeSelection: Int
    let chartRangeEnabled: [Bool]
    let accountAnalyticsLoading: Bool
    let onConnect: (String) -> Void
    let onAuthenticate: (String, String?, Bool) -> Void

    private var accounts: [Account] { store.accounts.filter { $0.provider == provider } }
    private var routing: ProviderRoutingResponse? { store.routingByProvider[provider] }
    private var routingByAccount: [String: ProviderRoutingAccount] {
        Dictionary(uniqueKeysWithValues: (routing?.accounts ?? []).map { ($0.accountId, $0) })
    }

    private var showsAddAccount: Bool {
        provider == "openai" || accounts.isEmpty
    }

    /// Stable per-account series/bar color, assigned by list position.
    private func accountColor(_ accountId: String) -> Color {
        let palette = AlexTheme.Colors.chartPalette
        guard let index = accounts.firstIndex(where: { $0.id == accountId }) else {
            return palette[0]
        }
        return palette[index % palette.count]
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 20) {
                usageHeader

                if let analytics = store.accountAnalytics, !accounts.isEmpty {
                    UsageChartCard(
                        analytics: analytics,
                        accounts: accounts,
                        rangeSelection: $chartRangeSelection,
                        rangeEnabled: chartRangeEnabled,
                        isLoading: accountAnalyticsLoading,
                        colorFor: accountColor)
                }

                accountsSection

                ProviderRoutingPreferencesSection(
                    store: store, provider: provider, accounts: accounts, routing: routing)
            }
            .padding(20)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private var usageHeader: some View {
        HStack {
            Text("Usage")
                .font(.system(size: 13, weight: .semibold))
                .foregroundStyle(AlexTheme.Colors.foreground)
            Spacer()
            StatusDot(tint: AlexTheme.ProviderBrand.brand(for: provider).accent, size: 8)
            Text(ProviderInfo.displayName(provider))
                .font(.system(size: 12, weight: .semibold))
                .foregroundStyle(AlexTheme.Colors.textSecondary)
        }
    }

    private var accountsSection: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(alignment: .firstTextBaseline) {
                Text(ProviderInfo.displayName(provider))
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(AlexTheme.Colors.foreground)
                Spacer()
                Text("Accounts are separate credentials. Pause and routing eligibility are controlled independently.")
                    .font(.system(size: 10))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
                    .multilineTextAlignment(.trailing)
                    .frame(maxWidth: 260, alignment: .trailing)
            }
            .padding(.bottom, 10)
            .overlay(alignment: .bottom) {
                Rectangle().fill(AlexTheme.Colors.cardBorder).frame(height: 1)
            }

            if accounts.isEmpty {
                EmptyStateView(
                    message: "No accounts connected for \(ProviderInfo.displayName(provider))",
                    style: .card)
            } else {
                ForEach(accounts) { account in
                    SubscriptionAccountRow(
                        account: account,
                        usage: usageByAccount[account.id],
                        routing: routingByAccount[account.id],
                        reservePct: routing?.reservePct ?? 10,
                        warnUsedPct: store.limitWarnPct,
                        accentColor: accountColor(account.id),
                        store: store
                    ) {
                        onAuthenticate(account.provider, account.name, false)
                    }
                }
            }

            if showsAddAccount {
                DashedAddButton(
                    title: "+ Add account",
                    cornerRadius: AlexTheme.Radius.xl
                ) {
                    onConnect(provider)
                }
            }
        }
    }
}

/// The Accounts chart card (§1.27): "Tokens routed over time" caption, a
/// time-range switcher, per-account legend, and the shared `UsageLineChart`.
private struct UsageChartCard: View {
    let analytics: AccountAnalyticsResponse
    let accounts: [Account]
    @Binding var rangeSelection: Int
    let rangeEnabled: [Bool]
    let isLoading: Bool
    let colorFor: (String) -> Color
    @State private var preparedSeries: [UsageChartSeries] = []
    @State private var preparedLabels: [String] = []

    private var accountIds: Set<String> { Set(accounts.map(\.id)) }

    private var points: [AccountUsageBucket] {
        analytics.series.filter { accountIds.contains($0.accountId) }
    }

    private var hasTokenActivity: Bool {
        preparedSeries.contains { $0.values.contains { $0 > 0 } }
    }

    /// The full bucket-aligned timeline for the requested range, not just
    /// the (often sparse, per-provider) buckets that have data — see
    /// `UsageChartMath.canonicalBuckets`. Deriving the x-axis from
    /// `points` alone collapsed the real time gaps between an account's
    /// infrequent activity into adjacent chart columns, which is what made
    /// the 7d/30d date labels look wrong.
    private var buckets: [Int64] {
        UsageChartMath.canonicalBuckets(
            sinceMs: analytics.sinceMs,
            bucketMs: analytics.bucketMs,
            nowMs: Int64(Date().timeIntervalSince1970 * 1_000))
    }

    private var chartSnapshotKey: Int {
        var hasher = Hasher()
        hasher.combine(analytics.sinceMs)
        hasher.combine(analytics.bucketMs)
        if let plotSeries = analytics.plotSeries {
            for series in plotSeries {
                hasher.combine(series.accountId)
                hasher.combine(series.values)
            }
        } else {
            for point in analytics.series { hasher.combine(point.id) }
        }
        return hasher.finalize()
    }

    private var uncachedChartSeries: [UsageChartSeries] {
        if let precomputed = analytics.plotSeries {
            return BarLog.measure(.ui, label: "chart data prep precomputed series=\(precomputed.count)") {
                precomputed
                    .filter { accountIds.contains($0.accountId) }
                    .map { line in
                        let account = accounts.first { $0.id == line.accountId }
                        return UsageChartSeries(
                            id: line.accountId,
                            name: account?.email ?? account?.description ?? account?.name ?? line.name,
                            color: colorFor(line.accountId), values: line.values)
                    }
            }
        }
        // Mixed daemon/app versions retain the old sparse-point fallback.
        let byAccount = Dictionary(grouping: points, by: \.accountId)
        let bucketIndex = Dictionary(
            uniqueKeysWithValues: buckets.enumerated().map { ($0.element, $0.offset) })
        return accounts.compactMap { account in
            guard let accountPoints = byAccount[account.id] else { return nil }
            var values = [Double](repeating: 0, count: buckets.count)
            for point in accountPoints {
                if let index = bucketIndex[point.bucketMs] {
                    values[index] = Double(point.inputTokens + point.outputTokens)
                }
            }
            return UsageChartSeries(
                id: account.id,
                name: account.email ?? account.description ?? account.name,
                color: colorFor(account.id),
                values: values)
        }
    }

    private var uncachedXLabels: [String] {
        if let labels = analytics.xLabels { return labels }
        return buckets.map {
            UsageChartMath.axisLabel(bucketMs: $0, bucketSizeMs: analytics.bucketMs)
        }
    }

    private var usages: [AccountUsage] {
        analytics.byAccount.filter { accountIds.contains($0.accountId) }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                Text("Tokens routed over time")
                    .font(.system(size: 11))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
                Spacer()
                UsageRangeTabs(
                    tabs: ["24h", "7d", "30d", "All"],
                    selection: $rangeSelection,
                    enabled: rangeEnabled)
            }

            if hasTokenActivity {
                UsageLineChart(series: preparedSeries, xLabels: preparedLabels)
            } else {
                EmptyStateView(message: "No per-account token activity to graph yet.")
            }

            if usages.count > 1 {
                RequestShareBar(usages: usages, colorFor: colorFor)
            }
        }
        .opacity(isLoading ? 0.55 : 1)
        .task(id: chartSnapshotKey) {
            // Build presentation once for an analytics snapshot. Hover state
            // belongs to UsageLineChart and no longer re-enters this work.
            let result = BarLog.measure(.ui, label: "chart data prep snapshot") {
                (uncachedChartSeries, uncachedXLabels)
            }
            preparedSeries = result.0
            preparedLabels = result.1
        }
        .overlay {
            if isLoading {
                ProgressView()
                    .controlSize(.small)
                    .padding(8)
                    .background(
                        RoundedRectangle(cornerRadius: AlexTheme.Radius.md)
                            .fill(AlexTheme.Colors.card.opacity(0.9)))
                    .allowsHitTesting(false)
            }
        }
        .padding(.top, 16)
        .padding(.horizontal, 16)
        .padding(.bottom, 8)
        .alexCard(background: AlexTheme.Colors.overlay(0.03))
    }
}

/// A 24h/7d/30d/All range switcher with per-tab disabled state. `SegmentedTabs`
/// (DesignSystem) has no notion of a disabled tab — it only takes a flat
/// `tabs: [String]` label list and a single `selection` binding — so this
/// reimplements its `.contained` visual style locally rather than editing the
/// shared component. A tab is disabled (dimmed, non-interactive,
/// `.help("Not enough history")`) when `enabled` says the daemon doesn't have
/// data old enough to make it show anything a shorter tab wouldn't already —
/// see `UsageChartMath.enabledRanges`.
private struct UsageRangeTabs: View {
    let tabs: [String]
    @Binding var selection: Int
    let enabled: [Bool]

    private func isEnabled(_ index: Int) -> Bool {
        guard enabled.indices.contains(index) else { return true }
        return enabled[index]
    }

    var body: some View {
        HStack(spacing: AlexTheme.Spacing.xxs) {
            ForEach(tabs.indices, id: \.self) { index in
                let selected = selection == index
                let active = isEnabled(index)
                Button {
                    selection = index
                } label: {
                    Text(tabs[index])
                        .font(.system(size: 10, weight: .medium))
                        .foregroundStyle(
                            !active
                                ? AlexTheme.Colors.textTertiary.opacity(0.4)
                                : (selected ? AlexTheme.Colors.foreground : AlexTheme.Colors.textTertiary))
                        .padding(.horizontal, AlexTheme.Spacing.ml)
                        .padding(.vertical, AlexTheme.Spacing.xs)
                        .background(
                            RoundedRectangle(cornerRadius: AlexTheme.Radius.sm)
                                .fill(active && selected ? AlexTheme.Colors.surfaceActive : .clear))
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .disabled(!active)
                .help(active ? "Server retains up to 30 days" : "Not enough history")
            }
        }
        .padding(2)
        .background(
            RoundedRectangle(cornerRadius: AlexTheme.Radius.md)
                .fill(AlexTheme.Colors.overlay(0.05)))
        .overlay(
            RoundedRectangle(cornerRadius: AlexTheme.Radius.md)
                .strokeBorder(AlexTheme.Colors.cardBorder))
    }
}

/// Existing "share of requests" strip kept from the previous design, restyled
/// so segment colors match the chart series.
private struct RequestShareBar: View {
    let usages: [AccountUsage]
    let colorFor: (String) -> Color

    private var total: Int64 { usages.reduce(0) { $0 + $1.requests } }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            GeometryReader { geometry in
                HStack(spacing: 2) {
                    ForEach(usages) { usage in
                        Capsule()
                            .fill(colorFor(usage.accountId))
                            .frame(width: max(3, geometry.size.width * share(usage)))
                            .help("\(usage.accountId): \(usage.requests) requests")
                    }
                }
            }
            .frame(height: 6)
            HStack(spacing: 10) {
                ForEach(usages) { usage in
                    HStack(spacing: 3) {
                        Circle()
                            .fill(colorFor(usage.accountId))
                            .frame(width: 6, height: 6)
                        Text("\(usage.accountId) \(Int(share(usage) * 100))%")
                            .lineLimit(1)
                    }
                    .font(AlexTheme.Fonts.mono(9))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
                }
            }
        }
        .padding(.bottom, 4)
    }

    private func share(_ usage: AccountUsage) -> CGFloat {
        guard total > 0 else { return 0 }
        return CGFloat(Double(usage.requests) / Double(total))
    }
}

private struct SubscriptionAccountRow: View {
    let account: Account
    let usage: AccountUsage?
    let routing: CodexRoutingAccount?
    let reservePct: Double
    let warnUsedPct: Double
    let accentColor: Color
    let store: SnapshotStore
    let reauthenticate: () -> Void
    @State private var deleting = false
    @State private var busy = false
    @State private var error: String?

    /// Quota windows: openai keeps its routing-fed windows (richer, includes
    /// reset selection); other providers fall back to per-account limits.
    private var quotaWindows: [LimitWindow] {
        if account.provider == "openai" {
            return routing?.windows ?? []
        }
        return account.limits?.windows ?? []
    }

    private var creditQuota: QuotaState? {
        guard let quota = account.limits?.quota, quota.isCreditPrimary else { return nil }
        return quota
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            header
            statsGrid
            quotaBars
            routingSummary
            buttonRow
            if let error {
                Text(error)
                    .font(.system(size: 10))
                    .foregroundStyle(AlexTheme.Colors.destructive)
            }
        }
        .padding(16)
        .frame(maxWidth: .infinity, alignment: .leading)
        .alexCard()
        .alert("Remove \(ProviderInfo.displayName(account.provider)) account ‘\(account.name)’?", isPresented: $deleting) {
            Button("Remove", role: .destructive) { remove() }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("Alexandria will stop using and pinging this account.")
        }
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: 3) {
            HStack(spacing: 8) {
                Text(ProviderInfo.displayName(account.provider))
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(AlexTheme.Colors.foreground)
                if account.paused {
                    StatusBadge(text: "Paused", tint: AlexTheme.Colors.warningOrange)
                } else {
                    StatusBadge(text: account.status.capitalized)
                }
                Spacer()
            }
            Text(account.id)
                .font(AlexTheme.Fonts.mono(11))
                .foregroundStyle(AlexTheme.Colors.textTertiary)
                .textSelection(.enabled)
            Text("Email: \(account.email ?? "not supplied by provider")")
                .font(.system(size: 11))
                .foregroundStyle(AlexTheme.Colors.textSecondary)
        }
    }

    private var statsGrid: some View {
        let errors = usage?.errors ?? 0
        return StatTilesRow(
            items: [
                StatTileData(
                    label: "Requests 24h",
                    value: (usage?.requests ?? 0).formatted()),
                StatTileData(
                    label: "Tokens 24h",
                    value: TraceFormat.tokens((usage?.inputTokens ?? 0) + (usage?.outputTokens ?? 0))),
                StatTileData(
                    label: "Errors",
                    value: "\(errors)",
                    valueTint: errors > 0
                        ? AlexTheme.Colors.destructive : AlexTheme.Colors.success),
            ],
            style: .inset)
    }

    @ViewBuilder
    private var quotaBars: some View {
        if !quotaWindows.isEmpty || creditQuota != nil {
            VStack(alignment: .leading, spacing: 8) {
                ForEach(Array(quotaWindows.enumerated()), id: \.offset) { _, window in
                    if let remaining = window.remainingPct {
                        LabeledQuotaBar(
                            caption: "\(window.window) · Tokens",
                            valueText: "\(remaining.formatted(.number.precision(.fractionLength(0))))% remaining",
                            detailText: window.resetsDate.map { "resets \(Self.relative($0))" },
                            fraction: remaining / 100,
                            fill: barColor(window))
                    } else {
                        LabeledQuotaBar(
                            caption: "\(window.window) · Tokens",
                            valueText: "usage unavailable",
                            fraction: 0,
                            fill: accentColor)
                    }
                }
                if let quota = creditQuota, let remaining = quota.remainingPct {
                    LabeledQuotaBar(
                        caption: "Credits",
                        valueText: "\(remaining.formatted(.number.precision(.fractionLength(0))))% remaining",
                        detailText: quota.balance,
                        fraction: remaining / 100,
                        fill: accentColor)
                }
                if account.provider == "openai", let observed = routing?.observedAtMs {
                    Text("Quota observed \(Self.relative(Date(timeIntervalSince1970: Double(observed) / 1_000)))")
                        .font(.system(size: 9))
                        .foregroundStyle(AlexTheme.Colors.textFaint)
                }
            }
        } else if account.provider == "openai" {
            Text("Codex quota: waiting for limit data from this account’s first proxied response.")
                .font(.system(size: 10))
                .foregroundStyle(AlexTheme.Colors.textTertiary)
        }
    }

    /// Read-only summary of this account's saved routing state; edits live in
    /// the routing rules card below (single source of editing + dirty state).
    @ViewBuilder
    private var routingSummary: some View {
        if let routing {
            HStack {
                HStack(spacing: 6) {
                    StatusDot(
                        tint: routing.eligible && !account.paused
                            ? AlexTheme.Colors.primary : AlexTheme.Colors.textFaint,
                        size: 5)
                    Text(routing.eligible && !account.paused
                        ? "Used for requests" : "Skipped for requests")
                        .font(.system(size: 11))
                        .foregroundStyle(AlexTheme.Colors.textSecondary)
                }
                Spacer()
                Text("Keep unused: \(Int(routing.reservePct ?? reservePct))%")
                    .font(.system(size: 10, weight: .medium))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
                    .padding(.horizontal, 8)
                    .padding(.vertical, 3)
                    .background(
                        RoundedRectangle(cornerRadius: AlexTheme.Radius.sm)
                            .fill(AlexTheme.Colors.overlay(0.07)))
            }
        }
    }

    private var buttonRow: some View {
        HStack(spacing: 8) {
            PillButton(
                title: account.paused ? "Resume account" : "Pause account",
                tint: account.paused ? AlexTheme.Colors.success : nil,
                horizontalPadding: 12, verticalPadding: 5, cornerRadius: 7,
                showsBorder: true, isEnabled: !busy
            ) {
                setPaused(!account.paused)
            }
            if account.provider == "openrouter" {
                Text("Use the sidebar + to replace the API key")
                    .font(.system(size: 10))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
            } else {
                PillButton(
                    title: "Re-authenticate", horizontalPadding: 12,
                    verticalPadding: 5, cornerRadius: 7, showsBorder: true
                ) {
                    reauthenticate()
                }
            }
            Spacer()
            PillButton(
                title: "Remove", variant: .danger, horizontalPadding: 12,
                verticalPadding: 5, cornerRadius: 7, showsBorder: true,
                isEnabled: !busy
            ) {
                deleting = true
            }
        }
    }

    private func barColor(_ window: LimitWindow) -> Color {
        switch window.remainingSeverity(warnUsedPct: warnUsedPct) {
        case .critical: AlexTheme.Colors.destructive
        case .warning: AlexTheme.Colors.warningOrange
        case .healthy, .none: accentColor
        }
    }

    private static func relative(_ date: Date) -> String {
        let formatter = RelativeDateTimeFormatter()
        formatter.unitsStyle = .abbreviated
        return formatter.localizedString(for: date, relativeTo: Date())
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
        VStack(alignment: .leading, spacing: 12) {
            heading

            if routing == nil {
                HStack(spacing: 8) {
                    Image(systemName: "exclamationmark.triangle")
                        .foregroundStyle(AlexTheme.Colors.warningOrange)
                    Text("The running daemon does not expose per-account routing yet. Update and restart alex to configure it here.")
                }
                .font(.system(size: 11))
                .foregroundStyle(AlexTheme.Colors.textTertiary)
            } else {
                Text("Choose which connected accounts may receive requests. Pausing an account disables it more broadly and always overrides this setting.")
                    .font(.system(size: 11))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)

                if accounts.isEmpty {
                    Text("This provider has no accounts yet. Its policy will apply when you add one.")
                        .font(.system(size: 10))
                        .foregroundStyle(AlexTheme.Colors.textTertiary)
                }

                routingCard

                if !draftAccounts.contains(where: \.eligible) {
                    Label(
                        "No account is selected. Requests for this provider will fail until at least one account is enabled.",
                        systemImage: "exclamationmark.triangle.fill")
                        .font(.system(size: 10))
                        .foregroundStyle(AlexTheme.Colors.destructive)
                }

                saveBar

                if let error {
                    Text(error)
                        .font(.system(size: 10))
                        .foregroundStyle(AlexTheme.Colors.destructive)
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

    private var heading: some View {
        HStack {
            Text("\(ProviderInfo.displayName(provider)) routing")
                .font(.system(size: 13, weight: .semibold))
                .foregroundStyle(AlexTheme.Colors.foreground)
            Spacer()
        }
        .padding(.bottom, 10)
        .overlay(alignment: .bottom) {
            Rectangle().fill(AlexTheme.Colors.cardBorder).frame(height: 1)
        }
    }

    /// Grouped rules card: selection mode / provider reserve / failover rows
    /// on overlay(0.03), per-account rows on overlay(0.02), hairline dividers.
    private var routingCard: some View {
        VStack(spacing: 0) {
            selectionModeRow
            divider(0.07)
            providerReserveRow
            divider(0.07)
            failoverRow
            if !displayedAccounts.isEmpty {
                divider(0.07)
            }
            ForEach(Array(displayedAccounts.enumerated()), id: \.element.accountId) { index, draft in
                if index > 0 {
                    divider(0.05)
                }
                accountRow(draft: draft, displayedIndex: index)
            }
        }
        .clipShape(RoundedRectangle(cornerRadius: AlexTheme.Radius.xl))
        .overlay(
            RoundedRectangle(cornerRadius: AlexTheme.Radius.xl)
                .strokeBorder(AlexTheme.Colors.cardBorder))
    }

    private func divider(_ opacity: CGFloat) -> some View {
        Rectangle().fill(AlexTheme.Colors.overlay(opacity)).frame(height: 1)
    }

    private var selectionModeRow: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                Text("Selection mode")
                    .font(.system(size: 12, weight: .medium))
                    .foregroundStyle(AlexTheme.Colors.foreground)
                Spacer()
                Picker("", selection: $strategy) {
                    ForEach(CodexRoutingStrategy.allCases, id: \.self) { value in
                        Text(value.displayName).tag(value)
                    }
                }
                .pickerStyle(.menu)
                .labelsHidden()
                .controlSize(.small)
                .fixedSize()
            }
            Text(strategy.explanation)
                .font(.system(size: 11))
                .foregroundStyle(AlexTheme.Colors.textTertiary)
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(AlexTheme.Colors.overlay(0.03))
    }

    private var providerReserveRow: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                Text("Provider-wide reserve")
                    .font(.system(size: 12, weight: .medium))
                    .foregroundStyle(AlexTheme.Colors.foreground)
                    .help("Headroom applied when an account has no separate reserve. 0% means reserve never blocks an account.")
                Spacer()
                Slider(value: $fallbackReservePct, in: 0...100, step: 5)
                    .controlSize(.mini)
                    .frame(width: 80)
                reserveChip(fallbackReservePct)
            }
            Text("Accounts below this remaining-quota threshold are skipped for new sessions. Changing this updates accounts still using the previous provider value; change an account below to give it its own reserve.")
                .font(.system(size: 11))
                .foregroundStyle(AlexTheme.Colors.textTertiary)
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(AlexTheme.Colors.overlay(0.03))
    }

    private var failoverRow: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                Text("Allow mid-thread account failover")
                    .font(.system(size: 12, weight: .medium))
                    .foregroundStyle(AlexTheme.Colors.foreground)
                    .help(
                        "Retry an active thread on a different eligible account when its assigned account is unavailable")
                Spacer()
                Toggle("", isOn: $allowMidThreadFailover)
                    .toggleStyle(.switch)
                    .controlSize(.mini)
                    .labelsHidden()
            }
            Text(allowMidThreadFailover
                ? "If the assigned account hits an auth, rate-limit, or server failure, Alexandria may move that thread to another eligible account. This keeps work moving but can reduce prompt-cache reuse."
                : "Auth, rate-limit, and server failures stay on the thread’s assigned account instead of retrying another one. Explicitly pausing, disabling, or removing that account can still reassign the thread.")
                .font(.system(size: 11))
                .foregroundStyle(AlexTheme.Colors.textTertiary)
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(AlexTheme.Colors.overlay(0.03))
    }

    private func accountRow(draft: CodexRoutingAccountUpdate, displayedIndex: Int) -> some View {
        let eligible = draftAccounts.first { $0.accountId == draft.accountId }?.eligible ?? false
        return VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 8) {
                Toggle("", isOn: eligibleBinding(accountId: draft.accountId))
                    .toggleStyle(.switch)
                    .controlSize(.mini)
                    .labelsHidden()
                    .disabled(account(draft.accountId)?.paused == true || busy)
                    .help("Include this account in routing and failover")
                Text(accountName(draft.accountId))
                    .font(.system(size: 11, weight: .medium))
                    .foregroundStyle(AlexTheme.Colors.textSecondary)
                    .lineLimit(1)
                Text(eligible ? "active" : "skipped")
                    .font(.system(size: 10, weight: .medium))
                    .foregroundStyle(
                        eligible ? AlexTheme.Colors.primary : AlexTheme.Colors.textTertiary)
                    .padding(.horizontal, 8)
                    .padding(.vertical, 2)
                    .background(
                        RoundedRectangle(cornerRadius: AlexTheme.Radius.sm)
                            .fill(eligible
                                ? AlexTheme.Colors.primary.opacity(0.15)
                                : AlexTheme.Colors.overlay(0.06)))
                Spacer(minLength: 8)
                strategyControls(accountId: draft.accountId, displayedIndex: displayedIndex)
                Text("Reserve")
                    .font(.system(size: 10))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
                Slider(
                    value: reserveBinding(accountId: draft.accountId),
                    in: 0...100, step: 5)
                    .controlSize(.mini)
                    .frame(width: 60)
                    .help(
                        "Prefer another eligible account once this account reaches its remaining-quota reserve. 0% never blocks it.")
                reserveChip(draftReserve(draft.accountId))
            }
            Text(draft.accountId)
                .font(AlexTheme.Fonts.mono(10))
                .foregroundStyle(AlexTheme.Colors.textTertiary)
                .textSelection(.enabled)
            if account(draft.accountId)?.paused == true {
                Text("This account is paused, so it cannot receive proxy traffic even while selected here.")
                    .font(.system(size: 10))
                    .foregroundStyle(AlexTheme.Colors.warningOrange)
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(AlexTheme.Colors.overlay(0.02))
    }

    private func reserveChip(_ value: Double) -> some View {
        Text("\(Int(value))%")
            .font(AlexTheme.Fonts.mono(11, weight: .medium))
            .foregroundStyle(AlexTheme.Colors.textSecondary)
            .padding(.horizontal, 8)
            .padding(.vertical, 3)
            .frame(minWidth: 42)
            .background(
                RoundedRectangle(cornerRadius: AlexTheme.Radius.sm)
                    .fill(AlexTheme.Colors.overlay(0.08)))
    }

    private var saveBar: some View {
        HStack(spacing: 8) {
            Spacer()
            if busy {
                ProgressView().controlSize(.small)
            }
            PillButton(
                title: "Cancel",
                tint: AlexTheme.Colors.textSecondary,
                horizontalPadding: 12, verticalPadding: 5, cornerRadius: 7,
                showsBorder: true, isEnabled: isDirty && !busy
            ) {
                loadRouting()
            }
            PillButton(
                title: isDirty ? "Save routing" : "Saved",
                variant: .solidAccent,
                isEnabled: isDirty && !busy
            ) {
                save()
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
                .font(AlexTheme.Fonts.mono(10))
                .foregroundStyle(AlexTheme.Colors.textTertiary)
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
                .foregroundStyle(AlexTheme.Colors.textTertiary)
                .help("New threads cycle across enabled subscriptions")
        case .resetFirst:
            Label(resetLabel(accountId: accountId, index: displayedIndex), systemImage: "clock.arrow.circlepath")
                .font(.system(size: 10))
                .foregroundStyle(AlexTheme.Colors.textTertiary)
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
            HStack(spacing: 8) {
                StatusDot(
                    tint: AlexTheme.ProviderBrand.brand(for: provider).accent, size: 8)
                Text("Add \(ProviderInfo.displayName(provider)) API key")
                    .font(AlexTheme.Fonts.panelTitle)
                    .foregroundStyle(AlexTheme.Colors.foreground)
            }
            Text("OpenRouter uses a long-lived API key, not OAuth. The key is sent only to your local Alexandria daemon for encrypted vault storage.")
                .font(.system(size: 12))
                .foregroundStyle(AlexTheme.Colors.textTertiary)
            VStack(spacing: 8) {
                SecureField("API key", text: $key)
                    .textFieldStyle(.roundedBorder)
                TextField("HTTP-Referer (optional)", text: $httpReferer)
                    .textFieldStyle(.roundedBorder)
                TextField("X-Title (optional)", text: $xTitle)
                    .textFieldStyle(.roundedBorder)
            }
            .font(AlexTheme.Fonts.mono(11))
            if let error {
                Text(error)
                    .font(.system(size: 10))
                    .foregroundStyle(AlexTheme.Colors.destructive)
            }
            HStack(spacing: 8) {
                Spacer()
                if saving { ProgressView().controlSize(.small) }
                PillButton(
                    title: "Cancel",
                    tint: AlexTheme.Colors.textSecondary,
                    horizontalPadding: 12, verticalPadding: 5, cornerRadius: 7,
                    showsBorder: true, isEnabled: !saving,
                    keyboardShortcut: .cancelAction,
                    action: onDone)
                PillButton(
                    title: "Save key",
                    variant: .solidAccent,
                    isEnabled: !saving
                        && !key.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
                    keyboardShortcut: .defaultAction
                ) {
                    save()
                }
            }
        }
        .padding(20)
        .frame(width: 440)
        .background(AlexTheme.Colors.background)
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
