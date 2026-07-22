import Foundation
import Testing
@testable import AlexCore

@Suite struct ResetTests {
    @Test func appSettingsResetClearsAppUIAndOnboardingButPreservesUpdateChannel() {
        let suite = "AlexCoreTests.ResetTests.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }

        for key in AppSettingsReset.keys + AppSettingsReset.preservedKeys {
            defaults.set("value", forKey: key)
        }

        AppSettingsReset.clear(defaults: defaults)

        for key in AppSettingsReset.keys {
            #expect(defaults.object(forKey: key) == nil)
        }
        #expect(defaults.object(
            forKey: OnboardingLaunchPolicy.completedDefaultsKey) == nil)
        #expect(defaults.string(forKey: UpdateChannelSetting.defaultsKey) == "value")
    }
}

@Suite @MainActor struct SnapshotAlertTests {
    @Test func expiredGrokTokenWithFailingHeartbeatMergesIntoOneCriticalAlert() throws {
        let alerts = SnapshotStore.authAndHealthAlerts(
            accounts: [try account(expiresInS: -1)],
            healthAccounts: [try healthAccount(ok: false)])

        #expect(alerts.count == 1)
        #expect(alerts[0].id == "acct-xai-oauth-expired")
        #expect(alerts[0].severity == .critical)
        #expect(alerts[0].body.contains("Requests are failing"))
        #expect(alerts[0].body.contains("re-authentication is required"))
    }

    @Test func healthyTokenWithFailingHeartbeatKeepsHeartbeatAlert() throws {
        let alerts = SnapshotStore.authAndHealthAlerts(
            accounts: [try account(expiresInS: 3_600)],
            healthAccounts: [try healthAccount(ok: false)])

        #expect(alerts.count == 1)
        #expect(alerts[0].id == "hb-xai-oauth")
        #expect(alerts[0].severity == .critical)
    }

    @Test func expiredGrokTokenWithHealthyHeartbeatStaysWarningOnly() throws {
        let alerts = SnapshotStore.authAndHealthAlerts(
            accounts: [try account(expiresInS: -1)],
            healthAccounts: [try healthAccount(ok: true)])

        #expect(alerts.count == 1)
        #expect(alerts[0].id == "acct-xai-oauth-expired")
        #expect(alerts[0].severity == .warning)
        #expect(alerts[0].body == "Run the grok CLI to refresh, then re-import")
    }

    private func account(expiresInS: Int64) throws -> Account {
        try JSONDecoder().decode(Account.self, from: Data(#"""
        {"id":"xai-oauth","provider":"xai","name":"Grok","kind":"oauth","label":null,"description":null,"email":null,"limits":null,"paused":false,"status":"active","expires_at_ms":1,"expires_in_s":\#(expiresInS)}
        """#.utf8))
    }

    private func healthAccount(ok: Bool) throws -> HealthAccount {
        try JSONDecoder().decode(HealthAccount.self, from: Data(#"""
        {"id":"xai-oauth","provider":"xai","kind":"oauth","status":"active","token_expires_in_s":0,"last_heartbeat":{"ok":\#(ok),"status":401,"latency_ms":10,"message":"unauthorized","ts_ms":1}}
        """#.utf8))
    }
}
