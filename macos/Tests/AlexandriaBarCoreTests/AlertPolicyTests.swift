import Foundation
import Testing
@testable import AlexandriaBarCore

@Suite struct AlertPolicyTests {
    @Test func heartbeatAttributionRequiresExactAccountId() {
        #expect(StoreAlertPolicy.heartbeatBelongsToAccount(
            heartbeatAccountId: "openai-oauth-personal",
            enclosingAccountId: "openai-oauth-personal"))
        #expect(!StoreAlertPolicy.heartbeatBelongsToAccount(
            heartbeatAccountId: "openai-oauth-personal",
            enclosingAccountId: "openai-oauth-work"))
        #expect(!StoreAlertPolicy.heartbeatBelongsToAccount(
            heartbeatAccountId: nil,
            enclosingAccountId: "openai-oauth-personal"))
    }

    @Test func recognizesCredentialFailuresWithoutClassifyingCapacityErrors() {
        #expect(StoreAlertPolicy.isCredentialFailure(status: 401, message: nil))
        #expect(StoreAlertPolicy.isCredentialFailure(
            status: 502,
            message: "token refresh failed: invalid_grant; refresh token has been revoked"))
        #expect(!StoreAlertPolicy.isCredentialFailure(
            status: 503,
            message: "Dario is configured but no healthy generation is available"))
    }

    @Test func existingCredentialAlertSuppressesOnlyDuplicateCredentialHeartbeat() {
        #expect(StoreAlertPolicy.suppressHeartbeat(
            credentialFailure: true, alreadyHasCredentialAlert: true))
        #expect(!StoreAlertPolicy.suppressHeartbeat(
            credentialFailure: false, alreadyHasCredentialAlert: true))
        #expect(!StoreAlertPolicy.suppressHeartbeat(
            credentialFailure: true, alreadyHasCredentialAlert: false))
    }

    @Test func reauthenticationRemediationRetainsExactAccount() {
        let remediation = StoreAlert.Remediation.reauthenticate(
            provider: "xai", accountName: "default")
        #expect(remediation == .reauthenticate(provider: "xai", accountName: "default"))
    }
}
