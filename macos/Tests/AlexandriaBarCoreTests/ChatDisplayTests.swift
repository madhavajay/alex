import Foundation
import Testing
@testable import AlexandriaBarCore

@Suite struct ChatDisplayTests {
    @Test func firstArgumentPreviewPrefersCommand() {
        let args = #"{"description": "run tests", "command": "npm test"}"#
        #expect(ChatDisplayFormat.firstArgumentPreview(args) == "npm test")
    }

    @Test func firstArgumentPreviewPrefersFilePath() {
        let args = #"{"old_string": "a", "file_path": "/src/auth.ts", "new_string": "b"}"#
        #expect(ChatDisplayFormat.firstArgumentPreview(args) == "/src/auth.ts")
    }

    @Test func firstArgumentPreviewFallsBackToSortedFirstKey() {
        let args = #"{"zeta": "last", "alpha": "first"}"#
        #expect(ChatDisplayFormat.firstArgumentPreview(args) == "first")
    }

    @Test func firstArgumentPreviewNonObjectPassesThrough() {
        #expect(ChatDisplayFormat.firstArgumentPreview("plain text") == "plain text")
        #expect(ChatDisplayFormat.firstArgumentPreview("  line1\nline2 ") == "line1 line2")
    }

    @Test func firstArgumentPreviewEmptyIsNil() {
        #expect(ChatDisplayFormat.firstArgumentPreview(nil) == nil)
        #expect(ChatDisplayFormat.firstArgumentPreview("") == nil)
        #expect(ChatDisplayFormat.firstArgumentPreview("   ") == nil)
        #expect(ChatDisplayFormat.firstArgumentPreview("{}") == nil)
    }

    @Test func firstArgumentPreviewNonStringScalars() {
        #expect(ChatDisplayFormat.firstArgumentPreview(#"{"command": 42}"#) == "42")
        #expect(ChatDisplayFormat.firstArgumentPreview(#"{"command": true}"#) == "true")
    }

    @Test func toolDurationFormatsMillisAndSeconds() {
        #expect(ChatDisplayFormat.toolDuration(startMs: 1000, endMs: 1042) == "42ms")
        #expect(ChatDisplayFormat.toolDuration(startMs: 1000, endMs: 1000) == "0ms")
        #expect(ChatDisplayFormat.toolDuration(startMs: 1000, endMs: 4241) == "3.2s")
        #expect(ChatDisplayFormat.toolDuration(startMs: 1000, endMs: nil) == nil)
        #expect(ChatDisplayFormat.toolDuration(startMs: 2000, endMs: 1000) == nil)
    }

    @Test func tokenLabel() {
        #expect(ChatDisplayFormat.tokenLabel(nil) == nil)
        #expect(ChatDisplayFormat.tokenLabel(892) == "892 tok")
        #expect(ChatDisplayFormat.tokenLabel(12_400) == "12k tok")
    }

    @Test func truncatedCollapsesAndCaps() {
        #expect(ChatDisplayFormat.truncated("short") == "short")
        #expect(ChatDisplayFormat.truncated("a\nb\nc") == "a b c")
        let long = String(repeating: "x", count: 60)
        let out = ChatDisplayFormat.truncated(long, max: 48)
        #expect(out.count == 49)
        #expect(out.hasSuffix("…"))
    }
}
