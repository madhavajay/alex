import Foundation

public struct DaemonConfig: Sendable, Equatable {
    public let host: String
    public let port: Int
    public let localKey: String
    public let anthropicUpstream: String

    // A daemon bound to 0.0.0.0/:: listens on all interfaces, but those are
    // not connectable addresses — the app runs on the same host, so connect
    // over loopback. (This also keeps App Transport Security happy, which
    // exempts loopback but not 0.0.0.0.)
    public var connectHost: String {
        switch host {
        case "0.0.0.0", "::", "*", "": "127.0.0.1"
        default: host
        }
    }

    public var baseURL: URL { URL(string: "http://\(connectHost):\(port)")! }
    public var darioEnabled: Bool { anthropicUpstream == "dario" }

    public init(host: String, port: Int, localKey: String, anthropicUpstream: String = "direct") {
        self.host = host
        self.port = port
        self.localKey = localKey
        self.anthropicUpstream = anthropicUpstream
    }
}

public enum DaemonDiscovery {
    public static var configPath: URL {
        FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent(".alexandria/config.toml")
    }

    public static func load() -> DaemonConfig? {
        guard let text = try? String(contentsOf: configPath, encoding: .utf8) else { return nil }
        return parse(toml: text)
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
