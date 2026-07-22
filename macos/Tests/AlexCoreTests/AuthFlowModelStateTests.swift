#if os(macOS)
import Testing
@testable import Alex
@testable import AlexCore

@Suite @MainActor struct AuthFlowModelStateTests {
    @Test func pendingSessionLoginFailureTransitionsToPendingConflict() {
        let model = AuthFlowModel(
            provider: "anthropic", accountName: "default", store: SnapshotStore())

        model.loginStartFailed(AlexClient.ClientError.http(
            409, #"{"error":{"message":"login session is already pending"}}"#))

        #expect(model.stage == .pendingConflict)
    }

    @Test func ordinaryLoginFailureTransitionsToFailedAndNotifiesCaller() {
        let model = AuthFlowModel(
            provider: "anthropic", accountName: "default", store: SnapshotStore())
        var notified: String?
        model.onFailed = { notified = $0 }

        model.loginStartFailed(AlexClient.ClientError.http(
            500, #"{"error":{"message":"provider unavailable"}}"#))

        let expected = #"HTTP 500: {"error":{"message":"provider unavailable"}}"#
        #expect(model.stage == .failed(expected))
        #expect(notified == expected)
    }
}
#endif
