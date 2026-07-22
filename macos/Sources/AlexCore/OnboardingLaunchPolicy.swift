import Foundation

/// Pure launch/menu policy shared by the native app and deterministic tests.
/// Window construction remains in the AppKit target; this type only decides
/// whether the existing onboarding entry point should be offered or opened.
public enum OnboardingLaunchPolicy {
    public static let completedDefaultsKey = "onboardingCompletedVersion"
    public static let currentVersion = "2"

    public static func shouldAutoPresent(
        completedVersion: String?,
        daemonUp: Bool,
        hasProviderAccounts: Bool,
        shownThisLaunch: Bool
    ) -> Bool {
        if completedVersion != currentVersion { return true }
        return !shownThisLaunch && daemonUp && !hasProviderAccounts
    }

    public static func shouldOfferStart(
        daemonUp: Bool,
        hasProviderAccounts: Bool
    ) -> Bool {
        daemonUp && !hasProviderAccounts
    }

    public static func clearCompletion(
        defaults: UserDefaults = .standard
    ) {
        defaults.removeObject(forKey: completedDefaultsKey)
    }
}
