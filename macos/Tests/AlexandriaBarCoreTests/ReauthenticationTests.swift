import Foundation
import Testing
@testable import AlexandriaBarCore

@Suite struct ReauthenticationTests {
    @Test func selectsNeedsReauthAccountsUsingDisplayStateAndHeartbeat() throws {
        let accounts = try JSONDecoder().decode(
            AccountsResponse.self,
            from: Data(#"""
            {"accounts":[
              {"id":"explicit","provider":"anthropic","name":"work","kind":"oauth","paused":false,"status":"active","health":"healthy","needs_reauth":true,"expires_in_s":3600},
              {"id":"auth-health","provider":"openai","name":"default","kind":"oauth","paused":false,"status":"active","health":"auth_failed","needs_reauth":false,"expires_in_s":3600},
              {"id":"heartbeat","provider":"gemini","name":"personal","kind":"oauth","paused":false,"status":"active","health":"healthy","needs_reauth":false,"expires_in_s":3600},
              {"id":"healthy","provider":"xai","name":"default","kind":"oauth","paused":false,"status":"active","health":"healthy","needs_reauth":false,"expires_in_s":3600}
            ]}
            """#.utf8)).accounts
        let healthAccounts = try JSONDecoder().decode(
            HealthResponse.self,
            from: Data(#"""
            {"accounts":[
              {"id":"heartbeat","provider":"gemini","kind":"oauth","status":"active","last_heartbeat":{"account_id":"heartbeat","provider":"gemini","ok":false,"status":401,"latency_ms":40,"message":"unauthorized","ts_ms":1783477017897}},
              {"id":"healthy","provider":"xai","kind":"oauth","status":"active","last_heartbeat":{"account_id":"healthy","provider":"xai","ok":true,"status":200,"latency_ms":55,"message":"ok","ts_ms":1783477017897}}
            ]}
            """#.utf8)).accounts

        let selected = Reauthentication.accountsNeedingReauthentication(
            accounts, healthAccounts: healthAccounts)

        #expect(selected.map(\.id) == ["explicit", "auth-health", "heartbeat"])
    }
}
