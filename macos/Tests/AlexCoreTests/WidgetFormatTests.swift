import Foundation
import Testing
@testable import AlexCore

@Suite struct WidgetFormatTests {
    @Test func modelBadgeLabelStripsClaudeAndDotsVersion() {
        #expect(ModelBadgeFormat.label(for: "claude-opus-4-8") == "opus 4.8")
        #expect(ModelBadgeFormat.label(for: "claude-sonnet-4-6") == "sonnet 4.6")
        #expect(ModelBadgeFormat.label(for: "claude-haiku-4-5") == "haiku 4.5")
    }

    @Test func modelBadgeLabelPassesThroughNonClaude() {
        #expect(ModelBadgeFormat.label(for: "gpt-4o") == "gpt-4o")
        #expect(ModelBadgeFormat.label(for: "gemini-2.0-flash") == "gemini-2.0-flash")
    }

    @Test func modelBadgeLabelOnlyRewritesTrailingVersionPair() {
        #expect(ModelBadgeFormat.label(for: "gpt-4-1") == "gpt 4.1")
        #expect(ModelBadgeFormat.label(for: "claude-opus") == "opus")
    }

    @Test func modelBadgeFamilies() {
        #expect(ModelBadgeFormat.family(of: "claude-opus-4-8") == .opus)
        #expect(ModelBadgeFormat.family(of: "claude-sonnet-4-6") == .sonnet)
        #expect(ModelBadgeFormat.family(of: "claude-haiku-4-5") == .haiku)
        #expect(ModelBadgeFormat.family(of: "gpt-4o") == .gpt)
        #expect(ModelBadgeFormat.family(of: "gemini-2.0-flash") == .other)
        #expect(ModelBadgeFormat.family(of: "Claude-Opus-5") == .opus)
    }
}
