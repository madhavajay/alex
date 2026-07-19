import Foundation
import Testing
@testable import AlexandriaBarCore

struct OnboardingSupportTests {
    @Test func harnessModelHints() {
        #expect(OnboardingSupport.modelHint(harness: "claude", model: "alex/claude-sonnet-4") ==
            "In Claude Code, enter `/model alex/claude-sonnet-4`.")
        #expect(OnboardingSupport.modelHint(harness: "codex", model: "alex/gpt-5").contains("-m alex/gpt-5"))
        #expect(OnboardingSupport.modelHint(harness: "pi", model: "alex/gpt-5").contains("model picker"))
    }

    @Test func harnessTestCommands() {
        #expect(OnboardingSupport.testCommand(harness: "claude", model: "alex/claude-sonnet-4") ==
            "claude --settings ~/.claude/alexandria-settings.json -p \"test\" --model alex/claude-sonnet-4")
        #expect(OnboardingSupport.testCommand(harness: "kimi", model: "alex/kimi-k2") ==
            "kimi -m alex/kimi-k2 -p \"test\"")
        #expect(OnboardingSupport.testCommand(harness: "codex", model: "alex/gpt-5") ==
            "codex --profile alex exec \"test\"")
    }

    @Test func environmentUsesLiveDaemonBaseURL() {
        let snippets = OnboardingSupport.environmentSnippets(
            baseURL: URL(string: "http://127.0.0.1:9876/"))
        #expect(snippets == [
            "OPENAI_BASE_URL=http://127.0.0.1:9876/v1 OPENAI_API_KEY=<your scoped key>",
            "ANTHROPIC_BASE_URL=http://127.0.0.1:9876 ANTHROPIC_API_KEY=<your scoped key>",
        ])
    }

    @Test func providerModelFilteringAndFallbacks() {
        #expect(OnboardingSupport.models(
            ["claude-sonnet-4", "alex/gpt-5", "alex/claude-opus-4"],
            for: "anthropic") == ["alex/claude-sonnet-4", "alex/claude-opus-4"])
        #expect(OnboardingSupport.fallbackModels(for: "openai").allSatisfy { $0.hasPrefix("alex/gpt-") })
    }
}
