import SwiftUI
import AlexandriaBarCore

struct LimitsCardView: View {
    let limits: [ProviderLimits]
    let accounts: [Account]
    let warnPct: Double

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            ForEach(displayProviders, id: \.self) { providerName in
                if providerName == "openai", !codexAccounts.isEmpty {
                    ForEach(codexAccounts) { account in
                        accountSection(account)
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
