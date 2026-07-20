import AppKit
import AlexandriaBarCore

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
        let minted = try await AlexandriaClient(config: config).mintRunKey(
            label: "\(harness)-remote",
            model: nil,
            ttlSeconds: nil,
            kind: .harness)
        guard minted.kind == RunKeyKind.harness.rawValue else {
            throw CopyError.unexpectedKeyKind
        }
        guard minted.key.hasPrefix("alxk-") else {
            throw CopyError.invalidScopedKey
        }
        let baseURL = RemoteOneLiner.daemonBaseURL(
            config: config,
            availableLANHosts: NetworkInterfaces.addresses().map(\.address))
        let command = RemoteOneLiner.build(
            harness: harness, baseURL: baseURL, key: minted.key)
        NSPasteboard.general.clearContents()
        guard NSPasteboard.general.setString(command, forType: .string) else {
            throw CopyError.pasteboardWriteFailed
        }
    }
}
