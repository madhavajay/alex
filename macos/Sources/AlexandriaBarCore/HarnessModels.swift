import Foundation

public struct HarnessesResponse: Codable, Sendable {
    public let harnesses: [Harness]
}

public struct Harness: Codable, Sendable, Identifiable, Equatable {
    public let name: String
    public let installed: Bool
    public let binary: String?
    public let version: String?
    public let versionWarning: String?
    public let configDir: String?
    public let configDirExists: Bool
    public let connected: Bool
    public let supportsConnect: Bool
    public let override: HarnessOverride?
    public let daemonReachable: Bool

    public var id: String { name }

    public init(
        name: String,
        installed: Bool,
        binary: String?,
        version: String?,
        versionWarning: String?,
        configDir: String?,
        configDirExists: Bool,
        connected: Bool,
        supportsConnect: Bool,
        override: HarnessOverride?,
        daemonReachable: Bool
    ) {
        self.name = name
        self.installed = installed
        self.binary = binary
        self.version = version
        self.versionWarning = versionWarning
        self.configDir = configDir
        self.configDirExists = configDirExists
        self.connected = connected
        self.supportsConnect = supportsConnect
        self.override = override
        self.daemonReachable = daemonReachable
    }

    enum CodingKeys: String, CodingKey {
        case name, installed, binary, version, connected, override
        case versionWarning = "version_warning"
        case configDir = "config_dir"
        case configDirExists = "config_dir_exists"
        case supportsConnect = "supports_connect"
        case daemonReachable = "daemon_reachable"
    }

    public static func missing(name: String) -> Harness {
        Harness(
            name: name, installed: false, binary: nil, version: nil, versionWarning: nil,
            configDir: nil, configDirExists: false, connected: false, supportsConnect: false,
            override: nil, daemonReachable: true)
    }
}

public struct HarnessOverride: Codable, Sendable, Equatable {
    public let binary: String?
    public let configDir: String?

    public init(binary: String?, configDir: String?) {
        self.binary = binary
        self.configDir = configDir
    }

    enum CodingKeys: String, CodingKey {
        case binary
        case configDir = "config_dir"
    }
}

public struct HarnessConfigWriteResponse: Codable, Sendable, Equatable {
    public let refreshed: Bool?
    public let path: String
    public let modelsTotal: Int
    public let added: [String]
    public let removed: [String]
    public let unchanged: Int
    /// `"reused"` or `"minted"`.
    public let key: String
    public let baseUrl: String
    public let keyId: String?

    public init(
        refreshed: Bool? = nil,
        path: String,
        modelsTotal: Int,
        added: [String],
        removed: [String],
        unchanged: Int,
        key: String,
        baseUrl: String,
        keyId: String? = nil
    ) {
        self.refreshed = refreshed
        self.path = path
        self.modelsTotal = modelsTotal
        self.added = added
        self.removed = removed
        self.unchanged = unchanged
        self.key = key
        self.baseUrl = baseUrl
        self.keyId = keyId
    }

    enum CodingKeys: String, CodingKey {
        case refreshed, path, added, removed, unchanged, key
        case modelsTotal = "models_total"
        case baseUrl = "base_url"
        case keyId = "key_id"
    }

    /// Prefer this for connect notifications that previously used `models`.
    public var models: Int { modelsTotal }
}

/// Alias kept for call-site clarity.
public typealias HarnessConnectResponse = HarnessConfigWriteResponse
public typealias HarnessRefreshConfigResponse = HarnessConfigWriteResponse

public struct HarnessDisconnectResponse: Codable, Sendable, Equatable {
    public let path: String
    public let modelsTotal: Int
    public let added: [String]
    public let removed: [String]
    public let unchanged: Int
    /// `"revoked"` or `"none"`.
    public let key: String
    public let baseUrl: String
    public let revoked: Int
    public let wasConnected: Bool

    public init(
        path: String,
        modelsTotal: Int = 0,
        added: [String] = [],
        removed: [String] = [],
        unchanged: Int = 0,
        key: String = "none",
        baseUrl: String = "",
        revoked: Int,
        wasConnected: Bool
    ) {
        self.path = path
        self.modelsTotal = modelsTotal
        self.added = added
        self.removed = removed
        self.unchanged = unchanged
        self.key = key
        self.baseUrl = baseUrl
        self.revoked = revoked
        self.wasConnected = wasConnected
    }

    enum CodingKeys: String, CodingKey {
        case path, added, removed, unchanged, key, revoked
        case modelsTotal = "models_total"
        case baseUrl = "base_url"
        case wasConnected = "was_connected"
    }
}

public struct HarnessPlanStep: Codable, Sendable, Equatable, Identifiable {
    public let path: String
    public let action: String
    public let detail: String

    public var id: String { "\(action)|\(path)|\(detail)" }

    public init(path: String, action: String, detail: String) {
        self.path = path
        self.action = action
        self.detail = detail
    }
}

public struct HarnessPlanResponse: Codable, Sendable, Equatable {
    public let plan: [HarnessPlanStep]

    public init(plan: [HarnessPlanStep]) {
        self.plan = plan
    }
}

public enum HarnessCatalog {
    public static let names = ["pi", "claude", "codex", "gemini", "grok", "opencode"]

    public static func displayName(_ name: String) -> String {
        switch name {
        case "pi": "Pi"
        case "claude": "Claude"
        case "codex": "Codex"
        case "gemini": "Gemini"
        case "grok": "Grok"
        case "opencode": "OpenCode"
        default: name.capitalized
        }
    }

    public static func rows(_ harnesses: [Harness]) -> [Harness] {
        let byName = Dictionary(uniqueKeysWithValues: harnesses.map { ($0.name, $0) })
        var out = names.map { byName[$0] ?? Harness.missing(name: $0) }
        out += harnesses
            .filter { !names.contains($0.name) }
            .sorted { $0.name.localizedCaseInsensitiveCompare($1.name) == .orderedAscending }
        return out
    }

    /// Harnesses that can receive `refresh-config` (connected + supports connect).
    /// Order follows `rows(_:)`; not hard-coded to a name list beyond display ordering.
    public static func refreshTargets(_ harnesses: [Harness]) -> [Harness] {
        rows(harnesses).filter { $0.supportsConnect && $0.connected }
    }
}
