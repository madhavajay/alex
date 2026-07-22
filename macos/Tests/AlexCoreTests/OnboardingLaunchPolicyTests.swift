import Foundation
import Testing
@testable import AlexCore

@Suite struct OnboardingLaunchPolicyTests {
    @Test func firstRunAndResetDaemonReenterOnboarding() {
        #expect(OnboardingLaunchPolicy.shouldAutoPresent(
            hasCompletionRecord: false,
            daemonUp: false,
            hasProviderAccounts: false,
            shownThisLaunch: false))
        #expect(OnboardingLaunchPolicy.shouldAutoPresent(
            hasCompletionRecord: true,
            daemonUp: true,
            hasProviderAccounts: false,
            shownThisLaunch: false))
        #expect(!OnboardingLaunchPolicy.shouldAutoPresent(
            hasCompletionRecord: true,
            daemonUp: true,
            hasProviderAccounts: false,
            shownThisLaunch: true))
        #expect(!OnboardingLaunchPolicy.shouldAutoPresent(
            hasCompletionRecord: true,
            daemonUp: true,
            hasProviderAccounts: true,
            shownThisLaunch: false))
    }

    @Test func startOnboardingIsOfferedOnlyForLiveProviderlessDaemon() {
        #expect(OnboardingLaunchPolicy.shouldOfferStart(
            daemonUp: true, hasProviderAccounts: false))
        #expect(!OnboardingLaunchPolicy.shouldOfferStart(
            daemonUp: false, hasProviderAccounts: false))
        #expect(!OnboardingLaunchPolicy.shouldOfferStart(
            daemonUp: true, hasProviderAccounts: true))
    }

    @Test func resetClearsCompletionRecord() throws {
        let suite = "OnboardingLaunchPolicyTests-\(UUID().uuidString)"
        let defaults = try #require(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        defaults.set("1", forKey: OnboardingLaunchPolicy.completedDefaultsKey)

        OnboardingLaunchPolicy.clearCompletion(defaults: defaults)

        #expect(defaults.object(
            forKey: OnboardingLaunchPolicy.completedDefaultsKey) == nil)
    }
}
