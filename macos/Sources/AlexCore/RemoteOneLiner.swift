import Foundation

/// Builds the paste-ready command used to bootstrap a harness on another machine.
/// Dynamic values are quoted twice: first for the inner `sh -c` script, then for
/// the outer shell's double-quoted argument.
public enum RemoteOneLiner {
    public static let installerURL =
        "https://raw.githubusercontent.com/madhavajay/alex/main/install-release.sh"

    public static let localhostWarning =
        "Daemon is bound to localhost — remote machines can't reach it. Bind a LAN address in General to use this on another machine."

    /// Everything a user can customize before copying a remote 1-liner.
    public struct Options: Sendable, Equatable {
        public var harness: String
        public var model: String?
        public var includeInstall: Bool
        public var includeKey: Bool

        public init(
            harness: String,
            model: String? = nil,
            includeInstall: Bool = true,
            includeKey: Bool = true
        ) {
            self.harness = harness
            self.model = model
            self.includeInstall = includeInstall
            self.includeKey = includeKey
        }
    }

    public static func build(harness: String, baseURL: URL, key: String) -> String {
        build(harness: harness, baseURL: baseURL.absoluteString, key: key)
    }

    public static func build(harness: String, baseURL: String, key: String) -> String {
        build(options: Options(harness: harness), baseURL: baseURL, key: key)
    }

    public static func build(options: Options, baseURL: URL, key: String?) -> String {
        build(options: options, baseURL: baseURL.absoluteString, key: key)
    }

    public static func build(options: Options, baseURL: String, key: String?) -> String {
        let normalizedBaseURL = baseURL.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        var script = ""
        if options.includeInstall {
            script += "command -v alex >/dev/null || curl -fsSL \(installerURL) | sh; "
        }
        script += "alex up \(shellArgument(options.harness)) "
            + "--url \(shellArgument(normalizedBaseURL))"
        if options.includeKey, let key, !key.isEmpty {
            script += " --key \(shellArgument(key))"
        }
        if let model = options.model, !model.isEmpty {
            script += " --model \(shellArgument(model))"
        }
        return "sh -c \"\(outerDoubleQuotedArgument(script))\""
    }

    /// Chooses the address remote machines should use. A specific configured
    /// interface is authoritative. An all-interface bind needs a concrete local
    /// address because 0.0.0.0/:: are listener sentinels, not destinations.
    public static func daemonBaseURL(
        config: DaemonConfig, availableLANHosts: [String] = []
    ) -> URL {
        let configuredHost = config.host.trimmingCharacters(in: .whitespacesAndNewlines)
        let normalizedHost = configuredHost.lowercased()
        let host: String
        switch normalizedHost {
        case "", "localhost", "127.0.0.1", "::1", "[::1]":
            return config.baseURL
        case "0.0.0.0", "::", "[::]", "*":
            guard let availableHost = availableLANHosts.first else { return config.baseURL }
            host = availableHost
        default:
            host = configuredHost
        }
        return url(host: host, port: config.port) ?? config.baseURL
    }

    /// Renders `http://host:port`, bracketing bare IPv6 addresses.
    public static func url(host: String, port: Int) -> URL? {
        let unwrappedHost = host.hasPrefix("[") && host.hasSuffix("]")
            ? String(host.dropFirst().dropLast()) : host
        let renderedHost = unwrappedHost.contains(":") ? "[\(unwrappedHost)]" : unwrappedHost
        return URL(string: "http://\(renderedHost):\(port)")
    }

    private static func shellArgument(_ value: String) -> String {
        guard !value.isEmpty else { return "''" }
        if value.unicodeScalars.allSatisfy(isSafeUnquoted) {
            return value
        }
        return "'\(value.replacingOccurrences(of: "'", with: "'\\''"))'"
    }

    private static func isSafeUnquoted(_ scalar: Unicode.Scalar) -> Bool {
        switch scalar.value {
        case 48...57, 65...90, 97...122:
            return true
        default:
            return "_@%+=:,./-".unicodeScalars.contains(scalar)
        }
    }

    private static func outerDoubleQuotedArgument(_ value: String) -> String {
        var escaped = ""
        escaped.reserveCapacity(value.count)
        for character in value {
            switch character {
            case "\\": escaped += "\\\\"
            case "\"": escaped += "\\\""
            case "$": escaped += "\\$"
            case "`": escaped += "\\`"
            default: escaped.append(character)
            }
        }
        return escaped
    }
}
