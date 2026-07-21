import Foundation

public struct DaemonConfig: Sendable, Equatable {
    public let host: String
    public let port: Int
    public let localKey: String
    public let anthropicUpstream: String

    // `host` is a bind address, never the address a local client should use.
    // A daemon exposed through a LAN or Tailscale interface must still leave
    // this app and local harnesses on loopback, even if that network disappears.
    public var connectHost: String {
        switch host {
        case "localhost", "127.0.0.1", "::1", "[::1]": host
        default: "127.0.0.1"
        }
    }

    public var baseURL: URL {
        let renderedHost = connectHost.contains(":") ? "[\(connectHost)]" : connectHost
        return URL(string: "http://\(renderedHost):\(port)")!
    }
    public var lanEnabled: Bool {
        let normalized = host.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        return !["", "127.0.0.1", "::1", "[::1]", "localhost"].contains(normalized)
    }
    public var darioEnabled: Bool { anthropicUpstream == "dario" }

    public init(host: String, port: Int, localKey: String, anthropicUpstream: String = "direct") {
        self.host = host
        self.port = port
        self.localKey = localKey
        self.anthropicUpstream = anthropicUpstream
    }
}

public enum DaemonDiscovery {
    private static let cacheLock = NSLock()
    nonisolated(unsafe) private static var cachedModificationDate: Date?
    nonisolated(unsafe) private static var cachedConfig: DaemonConfig?
    public static var configPath: URL {
        let home = FileManager.default.homeDirectoryForCurrentUser
        let current = home.appendingPathComponent(".alex/config.toml")
        if FileManager.default.fileExists(atPath: current.path) {
            return current
        }
        let legacy = home.appendingPathComponent(".alexandria/config.toml")
        return FileManager.default.fileExists(atPath: legacy.path) ? legacy : current
    }

    public static func load() -> DaemonConfig? {
        let path = configPath
        let modificationDate = (try? path.resourceValues(forKeys: [.contentModificationDateKey]))?
            .contentModificationDate
        cacheLock.lock()
        if cachedModificationDate == modificationDate {
            let cached = cachedConfig
            cacheLock.unlock()
            return cached
        }
        cacheLock.unlock()

        // Only the first caller after a file change does I/O and parsing.
        // Polling clients consume SnapshotStore.config rather than calling
        // this directly, so this is normally reached at refresh boundaries.
        let parsed = (try? String(contentsOf: path, encoding: .utf8))
            .flatMap { Self.parse(toml: $0) }
        cacheLock.lock()
        cachedModificationDate = modificationDate
        cachedConfig = parsed
        cacheLock.unlock()
        return parsed
    }

    public static func invalidateCache() {
        cacheLock.lock()
        cachedModificationDate = nil
        cachedConfig = nil
        cacheLock.unlock()
    }

    public static func parse(toml: String) -> DaemonConfig? {
        var values: [String: String] = [:]
        for rawLine in toml.split(separator: "\n") {
            let line = rawLine.trimmingCharacters(in: .whitespaces)
            if line.isEmpty || line.hasPrefix("#") || line.hasPrefix("[") { continue }
            guard let eq = line.firstIndex(of: "=") else { continue }
            let key = line[..<eq].trimmingCharacters(in: .whitespaces)
            var value = line[line.index(after: eq)...].trimmingCharacters(in: .whitespaces)
            if let hash = value.firstIndex(of: "#"), !value.hasPrefix("\"") {
                value = String(value[..<hash]).trimmingCharacters(in: .whitespaces)
            }
            if value.hasPrefix("\""), value.hasSuffix("\""), value.count >= 2 {
                value = String(value.dropFirst().dropLast())
            }
            values[key] = value
        }
        guard let key = values["local_key"], !key.isEmpty else { return nil }
        let host = values["host"] ?? "127.0.0.1"
        let port = values["port"].flatMap { Int($0) } ?? 4100
        return DaemonConfig(
            host: host, port: port, localKey: key,
            anthropicUpstream: values["anthropic_upstream"] ?? "direct")
    }
}
