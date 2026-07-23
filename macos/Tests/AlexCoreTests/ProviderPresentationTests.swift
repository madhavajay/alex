import Foundation
import Testing
@testable import AlexCore

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

    @Test func darioStaysHiddenUntilAnthropicAccountExists() throws {
        #expect(DarioHealth.evaluate(nil as DarioStatus?).tint == .red)
        #expect(!ProviderPresentation.shouldPresentDario(for: []))
        #expect(ProviderPresentation.menuProviders(
            limits: [], accounts: [], includeAnthropicDario: true).isEmpty)
        #expect(!ProviderPresentation.shouldShowLimitsCard(
            limits: [], accounts: [], includeAnthropicDario: true))

        let claudeAccount = try account(provider: "anthropic")
        #expect(ProviderPresentation.shouldPresentDario(for: [claudeAccount]))
        #expect(ProviderPresentation.menuProviders(
            limits: [], accounts: [claudeAccount],
            includeAnthropicDario: true) == ["anthropic"])
        #expect(ProviderPresentation.shouldShowLimitsCard(
            limits: [], accounts: [claudeAccount], includeAnthropicDario: true))
    }

    @Test func providerWithoutConnectedAccountShowsOnlyConnectState() throws {
        let codexAccount = try account()

        #expect(ProviderPresentation.paneState(for: "anthropic", accounts: [codexAccount]) == .connectAccount)
        #expect(ProviderPresentation.paneState(for: "openai", accounts: [codexAccount]) == .connected)
    }

    @Test func openRouterCreditsDecodeAndMatchTheirAccount() throws {
        let openRouter = try decode(
            """
            {"provider":"openrouter","account_id":"openrouter-api-key","source":"openrouter credits API","individual_credits_usd":12.34}
            """, as: ProviderLimits.self)
        let account = try decode(
            """
            {"id":"openrouter-api-key","provider":"openrouter","name":"default","kind":"api_key","paused":false,"status":"active"}
            """, as: Account.self)

        #expect(openRouter.individualCreditsUsd == 12.34)
        #expect(openRouter.formattedIndividualCredits == "$12.34")
        #expect(ProviderPresentation.creditBalanceText(openRouter) == "$12.34 credits")
        #expect(ProviderPresentation.limits(for: account, in: [openRouter])?.accountId == account.id)
    }

    @Test func codexDisabledCreditsRemainWindowBarsWithoutCreditBalance() throws {
        let codex = try decode(
            """
            {"provider":"openai","credits":{"has_credits":false,"unlimited":false,"balance":0},"quota":{"kind":"rate_window","label":"Rate window"},"windows":[{"window":"5h","used_pct":6},{"window":"7d","used_pct":82}]}
            """, as: ProviderLimits.self)

        #expect(codex.windows?.map(\.remainingPct) == [94, 18])
        #expect(codex.quota?.isCreditPrimary == false)
        #expect(ProviderPresentation.creditBalanceText(codex) == nil)
    }
}
