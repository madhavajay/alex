import Foundation

/// Which release feed the app follows for Sparkle updates.
/// Stored in UserDefaults under `defaultsKey`; unknown values mean stable.
public enum UpdateChannelSetting: String, CaseIterable, Sendable {
    case stable
    case beta

    public static let defaultsKey = "updateChannel"
    public static let changedNotification = Notification.Name("alexUpdateChannelChanged")

    public static func from(_ raw: String?) -> UpdateChannelSetting {
        raw.flatMap(UpdateChannelSetting.init(rawValue:)) ?? .stable
    }

    public var label: String {
        switch self {
        case .stable: return "Stable"
        case .beta: return "Beta"
        }
    }

    /// The feed Sparkle should use for this channel, derived from the
    /// stable feed baked into Info.plist. Returns nil when the baked feed
    /// should be used as-is (stable channel, or an unrecognized feed URL —
    /// never guess a beta feed for a URL we don't understand).
    public func feedURLString(stableFeed: String) -> String? {
        guard self == .beta else { return nil }
        let suffix = "appcast.xml"
        guard stableFeed.hasSuffix(suffix) else { return nil }
        return String(stableFeed.dropLast(suffix.count)) + "appcast-beta.xml"
    }
}
