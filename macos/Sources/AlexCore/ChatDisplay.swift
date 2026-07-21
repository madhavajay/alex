import Foundation

/// Pure formatting helpers for the chat-style transcript pane. Keeping these in
/// Core keeps them unit-testable and independent of SwiftUI.
public enum ChatDisplayFormat {
    /// Keys tried first when summarising a tool call's arguments, mirroring the
    /// "first meaningful argument" preview in the design mock.
    public static let previewKeyPriority = [
        "command", "file_path", "path", "pattern", "url", "query", "prompt", "description",
    ]

    /// A one-line preview of the most meaningful argument of a tool call.
    /// Falls back to the raw text when the arguments are not a JSON object.
    public static func firstArgumentPreview(_ arguments: String?) -> String? {
        guard let arguments else { return nil }
        let trimmed = arguments.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return nil }
        guard let data = trimmed.data(using: .utf8),
            let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        else { return singleLine(trimmed) }
        guard !obj.isEmpty else { return nil }
        for key in previewKeyPriority {
            if let value = obj[key] {
                return singleLine(JsonNice.scalarText(value))
            }
        }
        guard let first = obj.sorted(by: { $0.key < $1.key }).first else { return nil }
        return singleLine(JsonNice.scalarText(first.value))
    }

    /// A tool call's single meaningful argument rendered as plain text (not
    /// escaped JSON) when the arguments are "single-string-arg" shaped:
    /// exactly one key, and that key is one of `previewKeyPriority`. E.g.
    /// for `{"command": "cargo test -p alex"}` this returns
    /// `cargo test -p alex` rather than the pretty-printed JSON object, so
    /// tools like Bash/Read/Grep read naturally in the transcript's Input
    /// tab. Returns nil (callers should fall back to pretty JSON) for
    /// multi-argument calls, where hiding the other arguments would lose
    /// information.
    public static func meaningfulArgumentText(_ arguments: String) -> String? {
        let trimmed = arguments.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, let data = trimmed.data(using: .utf8),
            let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
            obj.count == 1, let onlyKey = obj.keys.first,
            previewKeyPriority.contains(onlyKey), let value = obj[onlyKey] as? String
        else { return nil }
        return value
    }

    /// Sub-second durations render as milliseconds ("42ms"); longer ones as
    /// seconds with one decimal ("3.2s").
    public static func toolDuration(startMs: Int64, endMs: Int64?) -> String? {
        guard let endMs, endMs >= startMs else { return nil }
        let ms = endMs - startMs
        guard ms >= 1000 else { return "\(ms)ms" }
        return String(format: "%.1fs", Double(ms) / 1000)
    }

    public static func tokenLabel(_ count: Int64?) -> String? {
        guard let count else { return nil }
        return "\(TraceNumberFormat.tokens(count)) tok"
    }

    public static func truncated(_ text: String, max: Int = 48) -> String {
        let line = singleLine(text)
        guard line.count > max else { return line }
        return line.prefix(max) + "…"
    }

    static func singleLine(_ text: String) -> String {
        text.split(separator: "\n", omittingEmptySubsequences: true)
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .joined(separator: " ")
    }
}
