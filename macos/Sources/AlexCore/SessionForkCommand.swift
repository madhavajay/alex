import Foundation

/// Builds the shell command used to start a new harness session from a
/// captured session. Ordinary session IDs (including UUIDs) stay readable;
/// unusual IDs are quoted so pasting the command cannot change its meaning.
public enum SessionForkCommand {
    public static func command(sessionId: String) -> String {
        "alex resume \(shellArgument(sessionId))"
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
}
