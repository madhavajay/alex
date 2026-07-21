import Foundation
import Testing
@testable import AlexandriaBarCore

struct OnboardingSupportTests {
    @Test func credentialsCurlBuilderIncludesRunnableRequestAndOptionalTags() {
        let curl = OnboardingSupport.credentialsCurlExample(
            baseURL: URL(string: "http://127.0.0.1:9876/"),
            key: "rk-secret", model: "alex/gpt-5.6-sol")

        #expect(curl.contains(#"curl "http://127.0.0.1:9876/v1/chat/completions""#))
        #expect(curl.contains(#"-H "Authorization: Bearer rk-secret""#))
        #expect(curl.contains(#"-H "x-session-id: my-first-session""#))
        #expect(curl.contains(#"-H "x-alexandria-task: quickstart""#))
        #expect(curl.contains(#"-H "x-alexandria-kind: experiment""#))
        #expect(curl.contains(
            #"-d '{"model":"alex/gpt-5.6-sol","messages":[{"role":"user","content":"Say hello from Alex onboarding."}]}'"#))
    }

    @Test func verifiedExampleModelsAreExplicit() {
        #expect(OnboardingSupport.exampleModel(for: "anthropic") == "alex/claude-haiku-4-5")
        #expect(OnboardingSupport.exampleModel(for: "openai") == "alex/gpt-5.6-sol")
        #expect(OnboardingSupport.exampleModel(for: "xai") == "alex/grok-code-fast-1")
        #expect(OnboardingSupport.exampleModel(for: "kimi") == "alex/kimi/k3")
        #expect(OnboardingSupport.exampleModel(for: "gemini") == "alex/gemini-2.5-flash")
        #expect(OnboardingSupport.exampleModel(for: "amp") == "alex/claude-haiku-4-5")
        #expect(OnboardingSupport.exampleModel(
            for: "cliproxyapi", cliProxyAPIModels: ["openai/gpt-5"])
            == "alex/cliproxyapi/openai/gpt-5")
    }

    @Test func dynamicExampleModelsPreserveDaemonOrder() {
        #expect(OnboardingSupport.exampleModel(
            for: "openrouter", openRouterExposed: ["z/model", "a/model"]
        ) == "alex/openrouter/z/model")
        #expect(OnboardingSupport.exampleModel(
            for: "openrouter", openRouterExposed: ["alex/openrouter/z/model"]
        ) == "alex/openrouter/z/model")

        let exo = [
            ExoModel(id: "z-model", name: "Z", family: nil, quantization: nil,
                     contextLength: nil, enabled: false, running: nil),
            ExoModel(id: "m-model", name: "M", family: nil, quantization: nil,
                     contextLength: nil, enabled: true, running: nil),
            ExoModel(id: "a-model", name: "A", family: nil, quantization: nil,
                     contextLength: nil, enabled: true, running: nil),
        ]
        #expect(OnboardingSupport.exampleModel(for: "exo", exoModels: exo) ==
            "alex/exo/m-model")
    }

    @Test func traceOutcomeOnlyUnlocksForCleanTrace() {
        #expect(OnboardingSupport.traceOutcome(status: 200, errorCount: 0, error: nil) == .clean)
        #expect(OnboardingSupport.traceOutcome(status: 429, errorCount: 1, error: "rate limited") ==
            .rejected("rate limited"))
        #expect(OnboardingSupport.traceOutcome(status: 503, errorCount: 0, error: nil) ==
            .rejected("HTTP 503"))
        #expect(OnboardingSupport.traceOutcome(status: 200, errorCount: 1, error: nil) ==
            .rejected("Provider returned an error"))
    }

    @Test func harnessModelHints() {
        #expect(OnboardingSupport.modelHint(harness: "claude", model: "alex/claude-sonnet-4") ==
            "In Claude Code, enter `/model alex/claude-sonnet-4`.")
        #expect(OnboardingSupport.modelHint(harness: "codex", model: "alex/gpt-5").contains("-m alex/gpt-5"))
        #expect(OnboardingSupport.modelHint(harness: "pi", model: "alex/gpt-5").contains("/model alex/gpt-5"))
        #expect(OnboardingSupport.modelHint(harness: "amp", model: "alex/gpt-5")
            .contains("alex wrap amp"))
    }

    @Test func harnessTestCommands() {
        #expect(OnboardingSupport.testCommand(harness: "claude", model: "alex/claude-sonnet-4") ==
            "claude --settings ~/.claude/alexandria-settings.json -p \"test\" --model alex/claude-sonnet-4")
        #expect(OnboardingSupport.testCommand(harness: "kimi", model: "alex/kimi-k2") ==
            "kimi -m alex/kimi-k2 -p \"test\"")
        #expect(OnboardingSupport.testCommand(harness: "codex", model: "alex/gpt-5") ==
            "codex --profile alex exec --skip-git-repo-check -m alex/gpt-5 \"test\"")
        #expect(OnboardingSupport.testCommand(harness: "amp", model: "alex/gpt-5") ==
            "alex wrap amp -- -x \"test\"")
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
        #expect(OnboardingSupport.models(["alex/claude-sonnet-4"], for: "amp").isEmpty)
        #expect(OnboardingSupport.fallbackModels(for: "amp").isEmpty)
    }

    @Test func ampInstallCopyDescribesWrapInsteadOfModelRouting() {
        let amp = OnboardingSupport.harnessInstallDescription("amp")
        #expect(amp.contains("alex wrap amp"))
        #expect(amp.contains("native models"))
        #expect(!amp.contains("exposed model list"))

        let pi = OnboardingSupport.harnessInstallDescription("pi")
        #expect(pi.contains("exposed model list"))
    }
}
