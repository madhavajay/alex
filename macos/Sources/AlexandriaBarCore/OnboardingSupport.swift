import Foundation

/// Pure presentation builders shared by the onboarding UI and its tests.
public enum OnboardingSupport {
    public static func fallbackModels(for provider: String?) -> [String] {
        switch provider?.lowercased() {
        case "anthropic": ["alex/claude-sonnet-4", "alex/claude-opus-4"]
        case "openai": ["alex/gpt-5", "alex/gpt-5-mini"]
        case "gemini": ["alex/gemini-2.5-pro", "alex/gemini-2.5-flash"]
        case "xai": ["alex/grok-4"]
        case "kimi": ["alex/kimi-k2"]
        case "openrouter": ["alex/openrouter/anthropic/claude-sonnet-4"]
        case "exo": ["alex/exo/local-model"]
        case "amp": ["alex/claude-sonnet-4"]
        default: ["alex/claude-sonnet-4", "alex/gpt-5"]
        }
    }

    public static func models(_ ids: [String], for provider: String?) -> [String] {
        let prefix: String?
        switch provider?.lowercased() {
        case "anthropic", "amp": prefix = "alex/claude-"
        case "openai": prefix = "alex/gpt-"
        case "gemini": prefix = "alex/gemini-"
        case "xai": prefix = "alex/grok-"
        case "kimi": prefix = "alex/kimi-"
        case "openrouter": prefix = "alex/openrouter/"
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
        case "pi": "Open Pi's model picker and choose `\(model)`."
        case let name?: "Choose `\(model)` in \(HarnessCatalog.displayName(name))'s model picker."
        case nil: "Choose `\(model)` in your harness's model picker."
        }
    }

    public static func testCommand(harness: String?, model: String) -> String {
        switch harness?.lowercased() {
        case "claude":
            "claude --settings ~/.claude/alexandria-settings.json -p \"test\" --model \(model)"
        case "kimi": "kimi -m \(model) -p \"test\""
        case "codex": "codex --profile alex exec \"test\""
        case "pi": "pi --model \(model) -p \"test\""
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
