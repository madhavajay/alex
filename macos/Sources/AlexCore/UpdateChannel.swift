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

    /// Whether a build version is a pre-release (`0.1.28-beta.3`, `-rc.1`,
    /// `-alpha.2`, or any recognized pre-release core). Mirrors the daemon's
    /// `UpdateChannel::default_for_version`, which treats any non-stable
    /// parsed version as a pre-release.
    public static func isPrerelease(version: String) -> Bool {
        guard let parsed = AlexVersion.parse(version) else { return false }
        return !parsed.isStable
    }

    /// The channel a build should follow when the user has NOT explicitly
    /// chosen one (B2). A pre-release build defaults to beta so a refresh
    /// actually checks the beta appcast — the old hardcoded `.stable` default
    /// made a beta build compare against the older latest *stable* and report
    /// "you're up to date" while a newer beta existed.
    public static func defaultChannel(forRunningVersion version: String) -> UpdateChannelSetting {
        isPrerelease(version: version) ? .beta : .stable
    }

    /// The channel to actually use for an update check: an explicit stored
    /// `stable`/`beta` wins (the user's choice), otherwise fall back to the
    /// build-derived default (B2). `rawStored` is the raw UserDefaults string
    /// (nil/unrecognized when the user has never picked).
    public static func resolved(rawStored: String?, runningVersion: String) -> UpdateChannelSetting {
        if let rawStored, let explicit = UpdateChannelSetting(rawValue: rawStored) {
            return explicit
        }
        return defaultChannel(forRunningVersion: runningVersion)
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

/// Which target(s) a Release-channel change applies to. The user was explicit:
/// the default sets BOTH, so picking a channel in the UI can never again leave
/// the daemon on a different channel than the app. `app` / `daemon` let an
/// advanced user steer just one.
public enum UpdateChannelScope: String, CaseIterable, Sendable {
    case both
    case app
    case daemon

    public var label: String {
        switch self {
        case .both: return "Both"
        case .app: return "App"
        case .daemon: return "Daemon"
        }
    }

    public var appliesToApp: Bool { self != .daemon }
    public var appliesToDaemon: Bool { self != .app }
}
