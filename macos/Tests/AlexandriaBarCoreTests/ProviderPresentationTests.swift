import Foundation
import Testing
@testable import AlexandriaBarCore

@Suite struct ProviderPresentationTests {
    private func decode<T: Decodable>(_ json: String, as type: T.Type) throws -> T {
        try JSONDecoder().decode(T.self, from: Data(json.utf8))
    }

    private func account(provider: String = "openai") throws -> Account {
        try decode(
            """
            {"id":"\(provider)-oauth","provider":"\(provider)","name":"default","kind":"oauth","paused":false,"status":"active"}
            """, as: Account.self)
    }

    private func limits(provider: String) throws -> ProviderLimits {
        try decode(
            """
            {"provider":"\(provider)","source":"captured response headers","windows":[{"window":"5h","used_pct":12}]}
            """, as: ProviderLimits.self)
    }

    @Test func providerWithoutAccountShowsNoLimits() throws {
        let staleClaudeLimits = try limits(provider: "anthropic")
        let codexAccount = try account()

        #expect(ProviderPresentation.visibleLimits([staleClaudeLimits], for: [codexAccount]).isEmpty)
        #expect(!ProviderPresentation.shouldShowLimitsCard(
            limits: [staleClaudeLimits], accounts: []))
    }

    @Test func noAccountsUsesConnectProviderState() {
        #expect(ProviderPresentation.hasNoAccounts([]))
        #expect(ProviderPresentation.paneState(for: "anthropic", accounts: []) == .connectAccount)
    }

    @Test func enabledDownDarioKeepsAnthropicProviderVisibleWithoutAccounts() {
        #expect(DarioHealth.evaluate(nil as DarioStatus?).tint == .red)
        #expect(ProviderPresentation.menuProviders(
            limits: [], accounts: [], includeAnthropicDario: true) == ["anthropic"])
        #expect(ProviderPresentation.shouldShowLimitsCard(
            limits: [], accounts: [], includeAnthropicDario: true))
    }

    @Test func providerWithoutConnectedAccountShowsOnlyConnectState() throws {
        let codexAccount = try account()

        #expect(ProviderPresentation.paneState(for: "anthropic", accounts: [codexAccount]) == .connectAccount)
        #expect(ProviderPresentation.paneState(for: "openai", accounts: [codexAccount]) == .connected)
    }
}
