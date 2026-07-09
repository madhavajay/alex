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

public struct HarnessConnectResponse: Codable, Sendable, Equatable {
    public let keyId: String
    public let models: Int

    enum CodingKeys: String, CodingKey {
        case models
        case keyId = "key_id"
    }
}

public struct HarnessDisconnectResponse: Codable, Sendable, Equatable {
    public let revoked: Int
    public let wasConnected: Bool

    enum CodingKeys: String, CodingKey {
        case revoked
        case wasConnected = "was_connected"
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
}
