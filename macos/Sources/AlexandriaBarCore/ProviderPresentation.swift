import Foundation

/// The provider UI only represents credentials that the daemon has confirmed
/// are present in its vault. Keeping these decisions in core prevents a stale
/// limits snapshot from creating a convincing-looking card for no account.
public enum ProviderPaneState: Equatable, Sendable {
    case connectAccount
    case connected
}

public enum ProviderPresentation {
    public static func hasNoAccounts(_ accounts: [Account]) -> Bool {
        accounts.isEmpty
    }

    public static func hasAccount(for provider: String, in accounts: [Account]) -> Bool {
        accounts.contains { $0.provider == provider }
    }

    /// Dario is an Anthropic-subscription transport, not an independent token
    /// provider. Keep it out of fresh-install UI until an Anthropic account is
    /// actually present in the daemon vault.
    public static func shouldPresentDario(for accounts: [Account]) -> Bool {
        hasAccount(for: "anthropic", in: accounts)
    }

    public static func visibleLimits(
        _ limits: [ProviderLimits], for accounts: [Account]
    ) -> [ProviderLimits] {
        let providersWithAccounts = Set(accounts.map(\.provider))
        return limits.filter { providersWithAccounts.contains($0.provider) }
    }

    public static func menuProviders(
        limits: [ProviderLimits], accounts: [Account], includeAnthropicDario: Bool = false
    ) -> [String] {
        var providers = Set(visibleLimits(limits, for: accounts).map(\.provider))
        if accounts.contains(where: { $0.provider == "openai" && $0.kind == "oauth" }) {
            providers.insert("openai")
        }
        if includeAnthropicDario, shouldPresentDario(for: accounts) {
            providers.insert("anthropic")
        }
        return providers.sorted()
    }

    /// Codex supplies limits per account, so a connected Codex OAuth account
    /// still has a useful loading card before a provider-wide snapshot exists.
    /// Dario similarly keeps its Anthropic parent visible while enabled, but
    /// only after an Anthropic subscription has been connected.
    public static func shouldShowLimitsCard(
        limits: [ProviderLimits], accounts: [Account], includeAnthropicDario: Bool = false
    ) -> Bool {
        !menuProviders(
            limits: limits,
            accounts: accounts,
            includeAnthropicDario: includeAnthropicDario
        ).isEmpty
    }

    public static func paneState(for provider: String, accounts: [Account]) -> ProviderPaneState {
        hasAccount(for: provider, in: accounts) ? .connected : .connectAccount
    }
}
