import AppKit
import AlexCore

@MainActor
enum RemoteOneLinerClipboard {
    enum CopyError: LocalizedError {
        case unexpectedKeyKind
        case invalidScopedKey
        case pasteboardWriteFailed

        var errorDescription: String? {
            switch self {
            case .unexpectedKeyKind:
                "The daemon did not return a harness-scoped key. Update Alex and try again."
            case .invalidScopedKey:
                "The daemon returned an invalid scoped key."
            case .pasteboardWriteFailed:
                "The command could not be written to the clipboard."
            }
        }
    }

    /// Mints on every call. The local/admin key only authenticates this request;
    /// it is never passed to the builder or written to the pasteboard.
    static func copy(harness: String, config: DaemonConfig) async throws {
        let baseURL = RemoteOneLiner.daemonBaseURL(
            config: config,
            availableLANHosts: NetworkInterfaces.rankedForRemoteAccess(
                NetworkInterfaces.addresses()).map(\.address))
        try await copy(
            options: RemoteOneLiner.Options(harness: harness),
            baseURL: baseURL,
            config: config)
    }

    static func copy(
        options: RemoteOneLiner.Options, baseURL: URL, config: DaemonConfig
    ) async throws {
        var key: String?
        if options.includeKey {
            let minted = try await AlexClient(config: config).mintRunKey(
                label: "\(options.harness)-remote",
                model: nil,
                ttlSeconds: nil,
                kind: .harness)
            guard minted.kind == RunKeyKind.harness.rawValue else {
                throw CopyError.unexpectedKeyKind
            }
            guard minted.key.hasPrefix("alxk-") else {
                throw CopyError.invalidScopedKey
            }
            key = minted.key
        }
        let command = RemoteOneLiner.build(options: options, baseURL: baseURL, key: key)
        NSPasteboard.general.clearContents()
        guard NSPasteboard.general.setString(command, forType: .string) else {
            throw CopyError.pasteboardWriteFailed
        }
    }
}
