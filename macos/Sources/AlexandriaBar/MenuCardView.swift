import SwiftUI
import AlexandriaBarCore

struct LimitsCardView: View {
    let limits: [ProviderLimits]
    let accounts: [Account]
    let routing: CodexRoutingResponse?
    let warnPct: Double

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            ForEach(displayProviders, id: \.self) { providerName in
                if providerName == "openai", !codexAccounts.isEmpty {
                    if codexAccounts.count > 1 {
                        bondedCodexSection
                    } else {
                        ForEach(codexAccounts) { account in
                            accountSection(account)
                        }
                    }
                } else if let provider = limits.first(where: { $0.provider == providerName }) {
                    providerSection(provider)
                }
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 8)
        .frame(width: 320, alignment: .leading)
    }

    @ViewBuilder
    private var bondedCodexSection: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                Text("Codex · \(codexAccounts.count) bonded")
                    .font(.system(size: 12, weight: .semibold))
                Spacer()
                if let routing {
                    Text("\(routing.strategy.displayName) · \(routing.strategy.shortCode)")
                        .font(.system(size: 10, weight: .medium))
                        .foregroundStyle(.secondary)
                } else {
                    Text("Routing unavailable")
                        .font(.system(size: 10, weight: .medium))
                        .foregroundStyle(.orange)
                }
            }

            if let routing {
                let summary = CodexBondedOrderSummary(routing: routing, accounts: codexAccounts)
                Text("\(Int(routing.reservePct.rounded()))% reserve · configured \(summary.configuredOrderLabel)")
                    .font(.system(size: 9, design: .monospaced))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                Text(summary.effectiveOrderLabel)
                    .font(.system(size: 9, weight: .medium, design: .monospaced))
                    .foregroundStyle(.secondary)

                ForEach(summary.configuredAccounts) { order in
                    if let account = codexAccounts.first(where: { $0.id == order.accountId }) {
                        bondedAccountSection(
                            account,
                            accountAlias: order.accountAlias,
                            priorityAlias: order.priorityAlias,
                            status: order.status,
                            route: routing.accounts.first { $0.accountId == order.accountId })
                    }
                }
            } else {
                ForEach(Array(codexAccounts.enumerated()), id: \.element.id) { index, account in
                    bondedAccountSection(
                        account,
                        accountAlias: "A\(index + 1)",
                        priorityAlias: "P\(index + 1)",
                        status: fallbackStatus(account),
                        route: nil)
                }
            }
        }
    }

    @ViewBuilder
    private func bondedAccountSection(
        _ account: Account,
        accountAlias: String,
        priorityAlias: String,
        status: CodexBondedAccountStatus,
        route: CodexRoutingAccount?
    ) -> some View {
        let accountLimits = account.limits
        let routedWindows = route?.windows ?? []
        let windows = routedWindows.isEmpty ? (accountLimits?.windows ?? []) : routedWindows
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 4) {
                Text(accountAlias)
                    .font(.system(size: 10, weight: .bold, design: .monospaced))
                Text("· \(priorityAlias) ·")
                    .font(.system(size: 9, design: .monospaced))
                    .foregroundStyle(.secondary)
                Text(account.email ?? account.name)
                    .font(.system(size: 10))
                    .foregroundStyle(account.email == nil ? .orange : .secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
                Spacer(minLength: 4)
                Text(status.label)
                    .font(.system(size: 9, weight: .medium))
                    .foregroundStyle(statusColor(status))
                    .lineLimit(1)
            }
            if let plan = accountLimits?.plan {
                Text(plan)
                    .font(.system(size: 9))
                    .foregroundStyle(.tertiary)
            }
            if let error = accountLimits?.error {
                Text(error)
                    .font(.system(size: 9))
                    .foregroundStyle(.orange)
                    .lineLimit(2)
            }
            if windows.isEmpty {
                Text("Waiting for quota data from this account")
                    .font(.system(size: 9))
                    .foregroundStyle(.secondary)
            } else {
                ForEach(windows, id: \.window) { window in
                    windowRow(window)
                }
            }
        }
        .padding(.top, 2)
    }

    private func fallbackStatus(_ account: Account) -> CodexBondedAccountStatus {
        if account.paused { return .paused }
        if account.status != "active" { return .inactive }
        return .ready
    }

    private func statusColor(_ status: CodexBondedAccountStatus) -> Color {
        switch status {
        case .ready: .green
        case .paused, .proxyOff, .reserveHeld: .orange
        case .inactive: .red
        }
    }

    /// Keep the provider card's stable order while ensuring Codex accounts are
    /// visible even before a provider-wide response-header snapshot exists.
    private var displayProviders: [String] {
        var providers = Set(limits.map(\.provider))
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

    @ViewBuilder
    private func providerSection(_ provider: ProviderLimits) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack {
                Text(providerTitle(provider.provider))
                    .font(.system(size: 12, weight: .semibold))
                Spacer()
                if let plan = provider.plan {
                    Text(plan)
                        .font(.system(size: 10))
                        .foregroundStyle(.secondary)
                }
            }
            if let error = provider.error {
                Text(error)
                    .font(.system(size: 10))
                    .foregroundStyle(.orange)
                    .lineLimit(2)
            }
            providerIdentities(provider.provider)
            ForEach(provider.windows ?? [], id: \.window) { window in
                windowRow(window)
            }
            if provider.windows?.isEmpty != false, let requests = provider.requests {
                countRow("requests", requests)
                if let tokens = provider.tokens {
                    countRow("tokens", tokens)
                }
            }
        }
    }

    @ViewBuilder
    private func accountSection(_ account: Account) -> some View {
        let accountLimits = account.limits
        VStack(alignment: .leading, spacing: 4) {
            HStack {
                Text(ProviderInfo.displayName(account.provider))
                    .font(.system(size: 12, weight: .semibold))
                Spacer()
                if let plan = accountLimits?.plan {
                    Text(plan)
                        .font(.system(size: 10))
                        .foregroundStyle(.secondary)
                }
            }
            Text(account.email ?? "Email unavailable")
                .font(.system(size: 10))
                .foregroundStyle(account.email == nil ? .orange : .secondary)
                .lineLimit(1)
                .truncationMode(.middle)
            if account.paused {
                Text("Paused · not used for proxy routing")
                    .font(.system(size: 9, weight: .medium))
                    .foregroundStyle(.orange)
            }
            if let error = accountLimits?.error {
                Text(error)
                    .font(.system(size: 10))
                    .foregroundStyle(.orange)
                    .lineLimit(2)
            }
            let windows = accountLimits?.windows ?? []
            if windows.isEmpty {
                Text("Waiting for quota data from this account")
                    .font(.system(size: 9))
                    .foregroundStyle(.secondary)
            } else {
                ForEach(windows, id: \.window) { window in
                    windowRow(window)
                }
            }
            if windows.isEmpty, let requests = accountLimits?.requests {
                countRow("requests", requests)
                if let tokens = accountLimits?.tokens {
                    countRow("tokens", tokens)
                }
            }
        }
    }

    private func providerTitle(_ provider: String) -> String {
        let matchingAccounts = accounts.filter { $0.provider == provider }
        let suffix = matchingAccounts.count > 1 ? " · combined" : ""
        return ProviderInfo.displayName(provider) + suffix
    }

    @ViewBuilder
    private func providerIdentities(_ provider: String) -> some View {
        let matchingAccounts = accounts.filter { $0.provider == provider }
        ForEach(matchingAccounts) { account in
            Text(account.email ?? "Email unavailable")
                .font(.system(size: 10))
                .foregroundStyle(account.email == nil ? .orange : .secondary)
                .lineLimit(1)
                .truncationMode(.middle)
        }
    }

    @ViewBuilder
    private func windowRow(_ window: LimitWindow) -> some View {
        let resetPassed = window.resetHasPassed()
        let remaining = window.remainingPct(relativeTo: Date())
        HStack(spacing: 8) {
            Text(window.window)
                .font(.system(size: 10, design: .monospaced))
                .foregroundStyle(.secondary)
                .frame(width: 28, alignment: .leading)
            GeometryReader { geo in
                ZStack(alignment: .leading) {
                    Capsule().fill(Color.primary.opacity(0.12))
                    if let remaining {
                        Capsule()
                            .fill(barColor(window))
                            .frame(width: max(3, geo.size.width * remaining / 100))
                    }
                }
            }
            .frame(height: 6)
            Text(resetPassed ? "refresh pending" : (remaining.map { "\(Int($0.rounded()))% left" } ?? "—"))
                .font(.system(size: 10, design: .monospaced))
                .frame(width: 86, alignment: .trailing)
            Text(resetPassed ? "reset passed" : (window.resetsDate.map { Format.countdown(to: $0) } ?? ""))
                .font(.system(size: 9))
                .foregroundStyle(.secondary)
                .frame(width: 68, alignment: .trailing)
        }
    }

    @ViewBuilder
    private func countRow(_ label: String, _ pair: CountPair) -> some View {
        HStack(spacing: 8) {
            Text(label)
                .font(.system(size: 10, design: .monospaced))
                .foregroundStyle(.secondary)
                .frame(width: 58, alignment: .leading)
            Text("\(pair.remaining ?? 0) / \(pair.limit ?? 0) remaining")
                .font(.system(size: 10, design: .monospaced))
        }
    }

    private func barColor(_ window: LimitWindow) -> Color {
        switch window.remainingSeverity(warnUsedPct: warnPct) {
        case .critical: .red
        case .warning: .orange
        case .healthy, .none: .green
        }
    }
}
