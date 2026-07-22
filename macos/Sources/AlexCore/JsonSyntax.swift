import Foundation

public enum JSONTextFormatting {
    /// Pretty-prints valid JSON while preserving scalar fragments and slash characters.
    /// Returns nil for incomplete or invalid input so editors can leave it untouched.
    public static func prettyPrinted(_ source: String) -> String? {
        guard let data = source.data(using: .utf8),
            let value = try? JSONSerialization.jsonObject(with: data, options: [.fragmentsAllowed]),
            let formatted = try? JSONSerialization.data(
                withJSONObject: value,
                options: [.prettyPrinted, .sortedKeys, .fragmentsAllowed, .withoutEscapingSlashes])
        else { return nil }
        return String(data: formatted, encoding: .utf8)
    }
}

/// Lightweight JSON tokenizer for syntax highlighting, ported from the Trace
/// Browser mock (shared.tsx:344-378). It is intentionally forgiving: invalid
/// input degrades to punctuation/whitespace tokens rather than failing.
public enum JsonSyntax {
    public enum TokenType: Equatable, Sendable {
        case key
        case string
        case number
        case boolean
        case null
        case punctuation
        case whitespace
    }

    public struct Token: Equatable, Sendable {
        public let type: TokenType
        public let text: String

        public init(_ type: TokenType, _ text: String) {
            self.type = type
            self.text = text
        }
    }

    public static func tokenize(_ source: String) -> [Token] {
        var tokens: [Token] = []
        let characters = Array(source)
        var index = 0

        func isWhitespace(_ character: Character) -> Bool {
            character == " " || character == "\t" || character == "\n" || character == "\r"
        }

        while index < characters.count {
            let character = characters[index]

            if isWhitespace(character) {
                var text = ""
                while index < characters.count, isWhitespace(characters[index]) {
                    text.append(characters[index])
                    index += 1
                }
                tokens.append(Token(.whitespace, text))
                continue
            }

            if character == "\"" {
                var text = "\""
                index += 1
                while index < characters.count {
                    if characters[index] == "\\" {
                        text.append(characters[index])
                        if index + 1 < characters.count {
                            text.append(characters[index + 1])
                        }
                        index += 2
                    } else if characters[index] == "\"" {
                        text.append("\"")
                        index += 1
                        break
                    } else {
                        text.append(characters[index])
                        index += 1
                    }
                }
                var lookahead = index
                while lookahead < characters.count, characters[lookahead] == " " {
                    lookahead += 1
                }
                let isKey = lookahead < characters.count && characters[lookahead] == ":"
                tokens.append(Token(isKey ? .key : .string, text))
                continue
            }

            if character == "-" || character.isNumber {
                var text = ""
                while index < characters.count,
                    "-0123456789.eE+".contains(characters[index])
                {
                    text.append(characters[index])
                    index += 1
                }
                tokens.append(Token(.number, text))
                continue
            }

            if matches("true", in: characters, at: index) {
                tokens.append(Token(.boolean, "true"))
                index += 4
                continue
            }
            if matches("false", in: characters, at: index) {
                tokens.append(Token(.boolean, "false"))
                index += 5
                continue
            }
            if matches("null", in: characters, at: index) {
                tokens.append(Token(.null, "null"))
                index += 4
                continue
            }

            tokens.append(Token(.punctuation, String(character)))
            index += 1
        }
        return tokens
    }

    private static func matches(_ word: String, in characters: [Character], at index: Int) -> Bool {
        let wordCharacters = Array(word)
        guard index + wordCharacters.count <= characters.count else { return false }
        for offset in 0..<wordCharacters.count
        where characters[index + offset] != wordCharacters[offset] {
            return false
        }
        return true
    }
}
