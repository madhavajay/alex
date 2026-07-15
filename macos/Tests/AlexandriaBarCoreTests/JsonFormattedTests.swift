import Foundation
import Testing
@testable import AlexandriaBarCore

@Suite struct JsonFormattedTests {
    private func text(_ tokens: [JsonFormatted.Token]) -> String {
        tokens.map(\.text).joined()
    }

    @Test func nonJSONReturnsNil() {
        #expect(JsonFormatted.tokens("plain text, not json") == nil)
        #expect(JsonFormatted.tokens("") == nil)
    }

    @Test func plainObjectRoundTripsAsValidJSON() throws {
        let raw = #"{"b": 1, "a": "hello", "c": true, "d": null}"#
        let tokens = try #require(JsonFormatted.tokens(raw))
        let rendered = text(tokens)
        let data = try #require(rendered.data(using: .utf8))
        let obj = try #require(try JSONSerialization.jsonObject(with: data) as? [String: Any])
        #expect(obj["a"] as? String == "hello")
        #expect(obj["b"] as? Int == 1)
        #expect(obj["c"] as? Bool == true)
        #expect(obj["d"] is NSNull)
        // Keys render in sorted order (matches BodyPretty's convention).
        #expect(tokens.contains { $0.kind == .key && $0.text.contains("\"a\"") })
    }

    @Test func embeddedJSONStringExpandsIntoAnnotatedSubBlock() throws {
        let raw = #"{"body": "{\"nested\": 1}"}"#
        let tokens = try #require(JsonFormatted.tokens(raw))
        #expect(tokens.contains { $0.kind == .annotation && $0.text.contains("json string") })
        // The nested value renders as real JSON tokens, not one long escaped
        // string.
        #expect(tokens.contains { $0.kind == .key && $0.text.contains("nested") })
        #expect(tokens.contains { $0.kind == .number && $0.text == "1" })
    }

    @Test func nonJSONStringStaysAPlainStringToken() throws {
        let raw = #"{"greeting": "hello world"}"#
        let tokens = try #require(JsonFormatted.tokens(raw))
        #expect(!tokens.contains { $0.kind == .annotation })
        #expect(tokens.contains { $0.kind == .string && $0.text.contains("hello world") })
    }

    @Test func longStringNewlinesBecomeRealLineBreaksNotEscapes() throws {
        let raw = #"{"log": "line one\nline two\nline three"}"#
        let tokens = try #require(JsonFormatted.tokens(raw))
        let rendered = text(tokens)
        #expect(rendered.contains("line one\n"))
        #expect(!rendered.contains(#"\n"#))
        #expect(tokens.contains { $0.kind == .string && $0.text == "line two" })
    }

    @Test func arraysRenderAllElements() throws {
        let raw = "[1, 2, 3]"
        let tokens = try #require(JsonFormatted.tokens(raw))
        let numbers = tokens.filter { $0.kind == .number }.map(\.text)
        #expect(numbers == ["1", "2", "3"])
    }

    @Test func truncatesAtCharBudgetInsteadOfHanging() throws {
        let bigArray = "[" + (0..<20_000).map(String.init).joined(separator: ",") + "]"
        let start = ContinuousClock.now
        let tokens = try #require(JsonFormatted.tokens(bigArray, maxChars: 2_000))
        let elapsed = start.duration(to: .now)
        #expect(elapsed < .seconds(1))
        #expect(tokens.contains { $0.kind == .annotation && $0.text.contains("truncated") })
        let totalNonAnnotationChars = tokens
            .filter { $0.kind != .annotation }
            .reduce(0) { $0 + $1.text.count }
        #expect(totalNonAnnotationChars <= 2_000)
    }
}
