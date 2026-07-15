import Foundation

/// Tree-aware JSON pretty-printer for the inspector's "Formatted" (non-Raw)
/// body view. Unlike the linear `JsonSyntax` tokenizer (which just colors
/// whatever text it's handed), this parses the document so it can:
///  - detect string values that are themselves valid JSON objects/arrays and
///    expand them as indented, annotated sub-blocks instead of one long
///    escaped string, and
///  - render literal newlines inside long string values as real line breaks
///    instead of the visually-flat "line1\nline2".
///
/// Raw mode bypasses this entirely and shows the exact original body — this
/// type only feeds the formatted view.
public enum JsonFormatted {
    public enum TokenKind: Equatable, Sendable {
        case key
        case string
        case number
        case boolean
        case null
        case punctuation
        case whitespace
        /// Dim "(json string)" markers and truncation notices — never part
        /// of the underlying document, purely a rendering aid.
        case annotation
    }

    public struct Token: Equatable, Sendable {
        public let kind: TokenKind
        public let text: String

        public init(_ kind: TokenKind, _ text: String) {
            self.kind = kind
            self.text = text
        }
    }

    /// Depth cap against pathological/adversarial nesting; real-world
    /// request/response bodies never come close.
    static let maxDepth = 40

    /// Returns nil when `raw` isn't a JSON object/array at the top level —
    /// callers should fall back to plain text or the linear `JsonSyntax`
    /// tokenizer in that case.
    ///
    /// `maxChars` bounds the total size of emitted token text so a huge body
    /// can't blow up formatting cost; once the budget is exhausted, emission
    /// stops and a trailing `.annotation` truncation notice is appended.
    /// Bounded scan cost regardless of document size, and safe to call off
    /// the main actor.
    public static func tokens(_ raw: String, maxChars: Int = BodyPretty.displayCap) -> [Token]? {
        let trimmed = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard trimmed.hasPrefix("{") || trimmed.hasPrefix("["),
            let data = trimmed.data(using: .utf8),
            let obj = try? JSONSerialization.jsonObject(with: data)
        else { return nil }
        var out: [Token] = []
        var budget = Budget(remaining: maxChars)
        emit(obj, indent: 0, depth: 0, budget: &budget, into: &out)
        return out
    }

    private struct Budget {
        var remaining: Int
        var exhausted = false
    }

    private static func append(_ token: Token, _ budget: inout Budget, into out: inout [Token]) {
        guard !budget.exhausted else { return }
        if token.text.count > budget.remaining {
            let clipped = String(token.text.prefix(max(0, budget.remaining)))
            if !clipped.isEmpty { out.append(Token(token.kind, clipped)) }
            out.append(Token(.annotation, " … (truncated)"))
            budget.exhausted = true
            return
        }
        budget.remaining -= token.text.count
        out.append(token)
    }

    private static func emit(
        _ value: Any, indent: Int, depth: Int, budget: inout Budget, into out: inout [Token]
    ) {
        guard !budget.exhausted else { return }
        guard depth < maxDepth else {
            append(Token(.annotation, "…"), &budget, into: &out)
            return
        }
        switch value {
        case let dict as [String: Any]:
            emitObject(dict, indent: indent, depth: depth, budget: &budget, into: &out)
        case let array as [Any]:
            emitArray(array, indent: indent, depth: depth, budget: &budget, into: &out)
        case let string as String:
            emitString(string, indent: indent, depth: depth, budget: &budget, into: &out)
        case let number as NSNumber:
            if CFGetTypeID(number) == CFBooleanGetTypeID() {
                append(Token(.boolean, number.boolValue ? "true" : "false"), &budget, into: &out)
            } else {
                append(Token(.number, number.stringValue), &budget, into: &out)
            }
        case is NSNull:
            append(Token(.null, "null"), &budget, into: &out)
        default:
            append(Token(.punctuation, "\(value)"), &budget, into: &out)
        }
    }

    private static func emitObject(
        _ dict: [String: Any], indent: Int, depth: Int, budget: inout Budget, into out: inout [Token]
    ) {
        guard !dict.isEmpty else {
            append(Token(.punctuation, "{}"), &budget, into: &out)
            return
        }
        append(Token(.punctuation, "{\n"), &budget, into: &out)
        let childIndent = indent + 2
        let keys = dict.keys.sorted()
        for (index, key) in keys.enumerated() {
            guard !budget.exhausted else { return }
            append(Token(.whitespace, spaces(childIndent)), &budget, into: &out)
            append(Token(.key, "\"\(jsonEscape(key))\": "), &budget, into: &out)
            emit(dict[key]!, indent: childIndent, depth: depth + 1, budget: &budget, into: &out)
            append(
                Token(.punctuation, index < keys.count - 1 ? ",\n" : "\n"), &budget, into: &out)
        }
        guard !budget.exhausted else { return }
        append(Token(.whitespace, spaces(indent)), &budget, into: &out)
        append(Token(.punctuation, "}"), &budget, into: &out)
    }

    private static func emitArray(
        _ array: [Any], indent: Int, depth: Int, budget: inout Budget, into out: inout [Token]
    ) {
        guard !array.isEmpty else {
            append(Token(.punctuation, "[]"), &budget, into: &out)
            return
        }
        append(Token(.punctuation, "[\n"), &budget, into: &out)
        let childIndent = indent + 2
        for (index, element) in array.enumerated() {
            guard !budget.exhausted else { return }
            append(Token(.whitespace, spaces(childIndent)), &budget, into: &out)
            emit(element, indent: childIndent, depth: depth + 1, budget: &budget, into: &out)
            append(
                Token(.punctuation, index < array.count - 1 ? ",\n" : "\n"), &budget, into: &out)
        }
        guard !budget.exhausted else { return }
        append(Token(.whitespace, spaces(indent)), &budget, into: &out)
        append(Token(.punctuation, "]"), &budget, into: &out)
    }

    private static func emitString(
        _ string: String, indent: Int, depth: Int, budget: inout Budget, into out: inout [Token]
    ) {
        let trimmed = string.trimmingCharacters(in: .whitespacesAndNewlines)
        if !trimmed.isEmpty, BodyPretty.isJSON(trimmed),
            let data = trimmed.data(using: .utf8),
            let nested = try? JSONSerialization.jsonObject(with: data)
        {
            append(Token(.annotation, "(json string)"), &budget, into: &out)
            append(Token(.whitespace, "\n" + spaces(indent + 2)), &budget, into: &out)
            emit(nested, indent: indent + 2, depth: depth + 1, budget: &budget, into: &out)
            return
        }
        guard string.contains("\n") else {
            append(Token(.string, "\"\(jsonEscape(string))\""), &budget, into: &out)
            return
        }
        // Render literal newlines as real line breaks instead of the
        // escaped two-character "\n" a strict JSON re-serialization would
        // produce, so long strings (log dumps, file contents, …) read
        // naturally instead of as one flat escaped line.
        append(Token(.string, "\""), &budget, into: &out)
        let lines = string.components(separatedBy: "\n")
        for (index, line) in lines.enumerated() {
            guard !budget.exhausted else { return }
            append(Token(.string, jsonEscape(line)), &budget, into: &out)
            if index < lines.count - 1 {
                append(Token(.whitespace, "\n" + spaces(indent + 2)), &budget, into: &out)
            }
        }
        guard !budget.exhausted else { return }
        append(Token(.string, "\""), &budget, into: &out)
    }

    private static func jsonEscape(_ value: String) -> String {
        guard let data = try? JSONSerialization.data(
            withJSONObject: value, options: [.fragmentsAllowed, .withoutEscapingSlashes]),
            let encoded = String(data: data, encoding: .utf8), encoded.count >= 2
        else { return value }
        // Strip the surrounding quotes JSONSerialization adds for a
        // fragment-encoded string — callers add their own.
        return String(encoded.dropFirst().dropLast())
    }

    private static func spaces(_ count: Int) -> String {
        String(repeating: " ", count: max(0, count))
    }
}
