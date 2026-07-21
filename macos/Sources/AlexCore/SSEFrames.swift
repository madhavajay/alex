import Foundation

/// Splits a Server-Sent Events response body ("event: X\ndata: {...}\n\n"
/// frames) for the inspector's formatted body view. Pure and streaming-style
/// (stops once `maxFrames` are found), so it stays cheap even against a
/// multi-MB body — safe to call off the main actor.
public enum SSEFrames {
    public struct Frame: Equatable, Sendable {
        /// The frame's "event:" field, when present (SSE's implicit default
        /// event type is "message").
        public let event: String?
        /// Concatenated "data:" line(s), joined by "\n" per the SSE spec.
        public let data: String

        public init(event: String?, data: String) {
            self.event = event
            self.data = data
        }
    }

    /// Heuristic: does this look like an SSE stream rather than a plain
    /// body? True when some line starts with "data:" (the one field every
    /// real SSE frame has) — checked without requiring "event:" since not
    /// every frame names an event.
    public static func isSSE(_ text: String) -> Bool {
        for line in text.split(separator: "\n", omittingEmptySubsequences: false) {
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            if trimmed.isEmpty { continue }
            if trimmed.hasPrefix("data:") || trimmed.hasPrefix("data ") { return true }
            // A body that opens with ordinary JSON/text isn't SSE even if a
            // "data:" substring shows up deep inside a string value later —
            // bail after the first few non-empty lines don't look like SSE.
            if !trimmed.hasPrefix("event:") && !trimmed.hasPrefix(":") { return false }
        }
        return false
    }

    /// Parses up to `maxFrames` frames (blank-line-delimited per the SSE
    /// spec); `truncated` is true when the text contained more than that.
    /// Comment lines (leading ":") and fields other than "event"/"data"
    /// ("id", "retry", …) are ignored, matching what a real EventSource
    /// consumer would surface.
    public static func parse(_ text: String, maxFrames: Int = 500) -> (frames: [Frame], truncated: Bool) {
        var frames: [Frame] = []
        var event: String?
        var dataLines: [String] = []
        var sawFieldThisBlock = false

        func flush() {
            guard sawFieldThisBlock else { return }
            frames.append(Frame(event: event, data: dataLines.joined(separator: "\n")))
            event = nil
            dataLines = []
            sawFieldThisBlock = false
        }

        for rawLine in text.split(separator: "\n", omittingEmptySubsequences: false) {
            if frames.count >= maxFrames {
                return (frames, true)
            }
            let line = String(rawLine).trimmingCharacters(in: .whitespaces)
            if line.isEmpty {
                flush()
                continue
            }
            if line.hasPrefix(":") { continue }
            guard let colon = line.firstIndex(of: ":") else { continue }
            let field = line[..<colon]
            var value = String(line[line.index(after: colon)...])
            if value.hasPrefix(" ") { value.removeFirst() }
            switch field {
            case "event":
                event = value
                sawFieldThisBlock = true
            case "data":
                dataLines.append(value)
                sawFieldThisBlock = true
            default:
                break
            }
        }
        flush()
        return (frames, false)
    }
}
