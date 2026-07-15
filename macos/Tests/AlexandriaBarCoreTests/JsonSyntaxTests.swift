import Foundation
import Testing
@testable import AlexandriaBarCore

@Suite struct JsonSyntaxTests {
    private func types(_ source: String) -> [JsonSyntax.TokenType] {
        JsonSyntax.tokenize(source).map(\.type)
    }

    @Test func distinguishesKeysFromStringValues() {
        let tokens = JsonSyntax.tokenize(#"{"name": "value"}"#)
        #expect(tokens.contains(JsonSyntax.Token(.key, "\"name\"")))
        #expect(tokens.contains(JsonSyntax.Token(.string, "\"value\"")))
    }

    @Test func keyDetectionAllowsSpacesBeforeColon() {
        let tokens = JsonSyntax.tokenize(#"{"name"  : 1}"#)
        #expect(tokens.first { $0.text == "\"name\"" }?.type == .key)
    }

    @Test func numbersBooleansAndNull() {
        let tokens = JsonSyntax.tokenize(#"{"a": -1.5e+3, "b": true, "c": false, "d": null}"#)
        #expect(tokens.contains(JsonSyntax.Token(.number, "-1.5e+3")))
        #expect(tokens.contains(JsonSyntax.Token(.boolean, "true")))
        #expect(tokens.contains(JsonSyntax.Token(.boolean, "false")))
        #expect(tokens.contains(JsonSyntax.Token(.null, "null")))
    }

    @Test func escapedQuotesStayInsideStrings() {
        let tokens = JsonSyntax.tokenize(#"{"a": "say \"hi\""}"#)
        #expect(tokens.contains(JsonSyntax.Token(.string, #""say \"hi\""#  + "\"")))
    }

    @Test func roundTripPreservesSource() {
        let source = """
        {
          "model": "claude-opus-4-8",
          "max_tokens": 4096,
          "stream": true,
          "stop": null
        }
        """
        let joined = JsonSyntax.tokenize(source).map(\.text).joined()
        #expect(joined == source)
    }

    @Test func emptyAndInvalidInputDoNotCrash() {
        #expect(JsonSyntax.tokenize("") == [])
        #expect(!JsonSyntax.tokenize("not json at all").isEmpty)
        #expect(!JsonSyntax.tokenize("\"unterminated").isEmpty)
    }

    @Test func punctuationIsTokenized() {
        #expect(types("{}") == [.punctuation, .punctuation])
        #expect(types("[1]") == [.punctuation, .number, .punctuation])
    }
}
