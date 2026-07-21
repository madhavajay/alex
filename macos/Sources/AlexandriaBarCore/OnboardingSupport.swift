import Foundation
#if canImport(CryptoKit)
import CryptoKit
#endif

/// Pure presentation builders shared by the onboarding UI and its tests.
public enum OnboardingSupport {
    public static let defaultExampleModel = "alex/claude-haiku-4-5"
    public static let credentialsDemoPrompt = "Say hello from Alex onboarding."

    public static func credentialsDemoHeaders(key: String) -> [(name: String, value: String)] {
        [
            ("Authorization", "Bearer \(key)"),
            ("Content-Type", "application/json"),
            ("x-session-id", "my-first-session"),
            ("x-alexandria-task", "quickstart"),
            ("x-alexandria-kind", "experiment"),
        ]
    }

    public static func credentialsDemoBody(model: String) -> String {
        let quotedModel = jsonQuoted(model)
        let quotedPrompt = jsonQuoted(credentialsDemoPrompt)
        return "{\"model\":\(quotedModel),\"messages\":[{\"role\":\"user\",\"content\":\(quotedPrompt)}]}"
    }

    public static func credentialsDemoURL(baseURL: URL?) -> URL {
        let base = baseURL?.absoluteString
            .trimmingCharacters(in: CharacterSet(charactersIn: "/"))
            ?? "http://127.0.0.1:4100"
        return URL(string: "\(base)/v1/chat/completions")!
    }

    /// A paste-ready request that is byte-for-byte equivalent to the request
    /// sent by the onboarding Run button (same URL, headers, and JSON body).
    public static func credentialsCurlExample(baseURL: URL?, key: String, model: String) -> String {
        let headerLines = credentialsDemoHeaders(key: key)
            .map { "  -H \"\($0.name): \($0.value)\" \\" }
            .joined(separator: "\n")
        return """
        curl "\(credentialsDemoURL(baseURL: baseURL).absoluteString)" \\
        \(headerLines)
          -d '\(credentialsDemoBody(model: model))'
        """
    }

    public static func runKeyFingerprint(_ key: String) -> String {
#if canImport(CryptoKit)
        SHA256.hash(data: Data(key.utf8)).prefix(8)
            .map { String(format: "%02x", $0) }.joined()
#else
        String(key.prefix(16))
#endif
    }

    private static func jsonQuoted(_ value: String) -> String {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.withoutEscapingSlashes]
        let data = try? encoder.encode(value)
        return data.flatMap { String(data: $0, encoding: .utf8) } ?? "\"\""
    }

    /// Picks the deliberately verified onboarding model for a provider. Dynamic
    /// providers preserve daemon order: the first exposed OpenRouter model and
    /// the first enabled Exo model win. Nothing is alphabetically re-sorted.
    public static func exampleModel(
        for provider: String?,
        openRouterExposed: [String] = [],
        cliProxyAPIModels: [String] = [],
        exoModels: [ExoModel] = []
    ) -> String {
        switch provider?.lowercased() {
        case "anthropic": return "alex/claude-haiku-4-5"
        case "openai": return "alex/gpt-5.6-sol"
        case "xai": return "alex/grok-code-fast-1"
        case "kimi": return "alex/kimi/k3"
        case "gemini": return "alex/gemini-2.5-flash"
        case "openrouter":
            guard let first = openRouterExposed.first(where: { !$0.isEmpty }) else {
                return defaultExampleModel
            }
            if first.hasPrefix("alex/") { return first }
            return "alex/openrouter/\(first)"
        case "cliproxyapi":
            guard let first = cliProxyAPIModels.first(where: { !$0.isEmpty }) else {
                return defaultExampleModel
            }
            if first.hasPrefix("alex/") { return first }
            return "alex/cliproxyapi/\(first)"
        case "exo":
            guard let first = exoModels.first(where: \.enabled)?.id, !first.isEmpty else {
                return defaultExampleModel
            }
            if first.hasPrefix("alex/") { return first }
            return "alex/exo/\(first)"
        default: return defaultExampleModel
        }
    }

    public enum TraceOutcome: Equatable, Sendable {
        case clean
        case rejected(String)
    }

    /// Keeps the onboarding unlock rule independent from SwiftUI and network
    /// polling: any recorded error or HTTP failure is rejected.
    public static func traceOutcome(
        status: Int?, errorCount: Int64?, error: String?
    ) -> TraceOutcome {
        let rejected = (status ?? 0) >= 400 || (errorCount ?? 0) > 0
        guard rejected else { return .clean }
        let detail = error?.trimmingCharacters(in: .whitespacesAndNewlines)
        if let detail, !detail.isEmpty { return .rejected(detail) }
        if let status, status >= 400 { return .rejected("HTTP \(status)") }
        return .rejected("Provider returned an error")
    }

    /// Match the copied onboarding command using the same field-or-tag logic
    /// as Trace Browser. Some harnesses expose a versioned user agent in the
    /// session field (for example `kimi-code-cli/0.27.0`) while Alex stores the
    /// canonical connection name (`kimi`) in the trace tags.
    public static func traceMatchesHarness(_ session: TraceSession, harness: String?) -> Bool {
        guard let harness, !harness.isEmpty else { return true }
        return OmniQuery.parse("harness:\(harness)").matches(session)
    }

    public static func fallbackModels(for provider: String?) -> [String] {
        switch provider?.lowercased() {
        case "anthropic": ["alex/claude-sonnet-4", "alex/claude-opus-4"]
        case "openai": ["alex/gpt-5", "alex/gpt-5-mini"]
        case "gemini": ["alex/gemini-2.5-pro", "alex/gemini-2.5-flash"]
        case "xai": ["alex/grok-4"]
        case "kimi": ["alex/kimi-k2"]
        case "openrouter": ["alex/openrouter/anthropic/claude-sonnet-4"]
        case "cliproxyapi": ["alex/cliproxyapi/gpt-4o"]
        case "exo": ["alex/exo/local-model"]
        case "amp": []
        default: ["alex/claude-sonnet-4", "alex/gpt-5"]
        }
    }

    public static func models(_ ids: [String], for provider: String?) -> [String] {
        let prefix: String?
        switch provider?.lowercased() {
        case "anthropic": prefix = "alex/claude-"
        case "amp": return []
        case "openai": prefix = "alex/gpt-"
        case "gemini": prefix = "alex/gemini-"
        case "xai": prefix = "alex/grok-"
        case "kimi": prefix = "alex/kimi-"
        case "openrouter": prefix = "alex/openrouter/"
        case "cliproxyapi": prefix = "alex/cliproxyapi/"
        case "exo": prefix = "alex/exo/"
        default: prefix = nil
        }
        let normalized = ids.map { $0.hasPrefix("alex/") ? $0 : "alex/\($0)" }
        let filtered = prefix.map { wanted in
            normalized.filter { $0.lowercased().hasPrefix(wanted) }
        } ?? normalized
        return Array(filtered.prefix(4))
    }

    public static func modelHint(harness: String?, model: String) -> String {
        switch harness?.lowercased() {
        case "claude": "In Claude Code, enter `/model \(model)`."
        case "codex": "In Codex, use `/model \(model)` or start it with `-m \(model)`."
        case "kimi": "In Kimi, use `/model \(model)` or start it with `-m \(model)`."
        case "pi": "In Pi, enter `/model \(model)`."
        case "amp": "Amp keeps its native models. Launch it through `alex wrap amp` so Alex can capture the trace."
        case "gemini": "In Gemini CLI, enter `/model \(model)`."
        case "opencode": "In OpenCode, enter `/model \(model)`."
        case let name?: "In \(HarnessCatalog.displayName(name)), choose `\(model)` with `/model` or `-m`."
        case nil: "Choose `\(model)` with your harness's `/model` command or `-m` option."
        }
    }

    public static func harnessInstallDescription(_ harness: String) -> String {
        if harness.lowercased() == "amp" {
            return "Alex will install capture metadata and a scoped lifecycle credential for Amp. Amp keeps its native models and runs through `alex wrap amp`."
        }
        return "Alex will add its local endpoint, scoped credential, and exposed model list to \(HarnessCatalog.displayName(harness))."
    }

    public static func testCommand(harness: String?, model: String) -> String {
        switch harness?.lowercased() {
        case "claude":
            "claude --settings ~/.claude/alexandria-settings.json -p \"test\" --model \(model)"
        case "kimi": "kimi -m \(model) -p \"test\""
        case "codex":
            "codex --profile alex exec --skip-git-repo-check -m \(model) \"test\""
        case "pi": "pi --model \(model) -p \"test\""
        case "amp": "alex wrap amp -- -x \"test\""
        case let name?: "\(name) -m \(model) -p \"test\""
        case nil: "<your harness> --model \(model)"
        }
    }

    public static func environmentSnippets(baseURL: URL?) -> [String] {
        let base = baseURL?.absoluteString.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
            ?? "http://127.0.0.1:4100"
        return [
            "OPENAI_BASE_URL=\(base)/v1 OPENAI_API_KEY=<your scoped key>",
            "ANTHROPIC_BASE_URL=\(base) ANTHROPIC_API_KEY=<your scoped key>",
        ]
    }
}
