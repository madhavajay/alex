import Foundation
import Testing
@testable import AlexCore

@Suite struct AccountDisplayStateTests {
    @Test func activeOAuthButExpiredNeedsReauth() {
        let state = AccountDisplayState.derive(
            status: "active",
            kind: "oauth",
            needsReauth: false,
            expiresInS: -1,
            health: .healthy)

        #expect(state == .needsReauth)
    }

    @Test func explicitNeedsReauthWinsOverHealthySignals() {
        let state = AccountDisplayState.derive(
            status: "active",
            kind: "oauth",
            needsReauth: true,
            expiresInS: 3_600,
            health: .healthy,
            lastPingOK: true,
            lastPingStatus: 200)

        #expect(state == .needsReauth)
    }

    @Test func healthyUsableAccountIsActive() {
        let state = AccountDisplayState.derive(
            status: "active",
            kind: "oauth",
            needsReauth: false,
            expiresInS: 3_600,
            health: .healthy,
            lastPingOK: true,
            lastPingStatus: 200)

        #expect(state == .active)
    }

    @Test func newlyAuthenticatedAccountIsActiveBeforeFirstProbe() {
        let state = AccountDisplayState.derive(
            status: "active",
            kind: "oauth",
            needsReauth: false,
            expiresInS: 3_600,
            health: .unknown)

        #expect(state == .active)
    }

    @Test func non2xxLastPingNeedsReauth() {
        let state = AccountDisplayState.derive(
            status: "active",
            kind: "oauth",
            needsReauth: false,
            expiresInS: 3_600,
            health: .healthy,
            lastPingOK: false,
            lastPingStatus: 401)

        #expect(state == .needsReauth)
    }

    @Test func unknownAfterErrorHealthDecodesAndNeedsReauth() throws {
        let health = try JSONDecoder().decode(
            AccountHealth.self, from: Data(#""unknown-after-error""#.utf8))
        let state = AccountDisplayState.derive(
            status: "active",
            kind: "oauth",
            needsReauth: false,
            expiresInS: 3_600,
            health: health)

        #expect(health == .unknownAfterError)
        #expect(state == .needsReauth)
    }
}
