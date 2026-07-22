import Foundation

/// Categories accepted by the daemon's destructive reset endpoint.
public struct ResetSelection: Codable, Sendable, Equatable {
    public var credentials: Bool
    public var settings: Bool
    public var traces: Bool
    public var harnesses: Bool
    public var cache: Bool

    public init(
        credentials: Bool = false,
        settings: Bool = false,
        traces: Bool = false,
        harnesses: Bool = false,
        cache: Bool = false
    ) {
        self.credentials = credentials
        self.settings = settings
        self.traces = traces
        self.harnesses = harnesses
        self.cache = cache
    }
}

public enum ResetMode: String, Codable, Sendable, Equatable {
    case immediate
    case graceful
}

public struct ResetRequest: Codable, Sendable, Equatable {
    public let credentials: Bool
    public let settings: Bool
    public let traces: Bool
    public let harnesses: Bool
    public let cache: Bool
    public let dryRun: Bool
    public let mode: ResetMode

    enum CodingKeys: String, CodingKey {
        case credentials, settings, traces, harnesses, cache, mode
        case dryRun = "dry_run"
    }

    public init(
        selection: ResetSelection, dryRun: Bool, mode: ResetMode = .immediate
    ) {
        credentials = selection.credentials
        settings = selection.settings
        traces = selection.traces
        harnesses = selection.harnesses
        cache = selection.cache
        self.dryRun = dryRun
        self.mode = mode
    }
}

public struct ResetProgress: Codable, Sendable, Equatable {
    public let status: String
    public let phase: String
    public let detail: String
    public let inFlight: Int

    enum CodingKeys: String, CodingKey {
        case status, phase, detail
        case inFlight = "in_flight"
    }

    public init(status: String, phase: String, detail: String, inFlight: Int) {
        self.status = status
        self.phase = phase
        self.detail = detail
        self.inFlight = inFlight
    }
}

public struct ResetCancelResponse: Codable, Sendable, Equatable {
    public let cancelled: Bool
}

public struct ResetFileCount: Codable, Sendable, Equatable {
    public let files: Int
    public let bytes: Int64
}

public struct ResetCounts: Codable, Sendable, Equatable {
    public let accounts: Int
    public let runKeys: Int
    public let traces: Int
    public let heartbeats: Int
    public let bodies: ResetFileCount
    public let connectedHarnesses: Int
    public let pricing: Int
    public let darioPromptCache: ResetFileCount

    enum CodingKeys: String, CodingKey {
        case accounts, traces, heartbeats, bodies, pricing
        case runKeys = "run_keys"
        case connectedHarnesses = "connected_harnesses"
        case darioPromptCache = "dario_prompt_cache"
    }
}

public struct ResetActions: Codable, Sendable, Equatable {
    public let credentials: String?
    public let settings: String?
    public let traces: String?
    public let harnesses: String?
    public let cache: String?
}

public struct ResetSettingsResult: Codable, Sendable, Equatable {
    public let preservesUpdateChannel: Bool
    public let preservesLocalKey: Bool
    public let rotatesLocalKey: Bool

    enum CodingKeys: String, CodingKey {
        case preservesUpdateChannel = "preserves_update_channel"
        case preservesLocalKey = "preserves_local_key"
        case rotatesLocalKey = "rotates_local_key"
    }
}

/// The daemon's plan/result response for `POST /admin/reset`.
public struct ResetResponse: Codable, Sendable, Equatable {
    public let dryRun: Bool
    public let applied: Bool
    public let selected: [String]
    public let counts: ResetCounts
    public let harnesses: [String]
    public let actions: ResetActions
    public let settings: ResetSettingsResult

    enum CodingKeys: String, CodingKey {
        case applied, selected, counts, harnesses, actions, settings
        case dryRun = "dry_run"
    }
}

/// The app-owned preferences removed when daemon settings are reset.
/// `updateChannel` deliberately remains so a beta user stays on beta.
public enum AppSettingsReset {
    public static let keys = [
        "refreshSeconds",
        "notifyEnabled",
        "limitWarnPct",
        "terminalApp",
        "binaryPath",
        "TranscriptRawMode",
        "TraceBrowserDetailsOn",
        "TraceBrowserLeftPaneWidth",
        "TraceBrowserColumnCustomization",
        "SessionInfoExpanded",
        "InspectorRawMode",
        "InspectorReqHeadersOpen",
        "InspectorRespHeadersOpen",
        "InspectorReqBodyOpen",
        "InspectorRespBodyOpen",
        "InspectorDarioReqBodyOpen",
        "InspectorDarioRespBodyOpen",
        OnboardingLaunchPolicy.completedDefaultsKey,
    ]

    public static let preservedKeys = [UpdateChannelSetting.defaultsKey]

    public static func clear(defaults: UserDefaults = .standard) {
        for key in keys {
            defaults.removeObject(forKey: key)
        }
    }
}
