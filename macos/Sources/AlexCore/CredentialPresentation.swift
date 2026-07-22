import Foundation

/// Client protocols supported by the Settings connect helper.
public enum ConnectClientAPI: String, CaseIterable, Sendable, Identifiable {
    case anthropicMessages
    case openAIChat
    case openAIResponses
    case geminiGenerateContent

    public var id: String { rawValue }

    public var displayName: String {
        switch self {
        case .anthropicMessages: "Anthropic Messages"
        case .openAIChat: "OpenAI Chat"
        case .openAIResponses: "OpenAI Responses"
        case .geminiGenerateContent: "Gemini generateContent"
        }
    }
}

public enum ConnectOutputFormat: String, CaseIterable, Sendable, Identifiable {
    case env
    case curl

    public var id: String { rawValue }
}

/// Pure shell-snippet generation kept out of SwiftUI so every protocol's
/// endpoint and environment names are covered by unit tests.
public enum ConnectSnippetBuilder {
    public static func build(
        format: ConnectOutputFormat = .env,
        api: ConnectClientAPI,
        baseURL: String,
        key: String,
        label: String? = nil,
        model: String? = nil
    ) -> String {
        let base = baseURL.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        let label = normalized(label)
        let model = model?.trimmingCharacters(in: .whitespacesAndNewlines)

        if format == .curl {
            return curl(api: api, base: base, key: key, label: label, model: model)
        }

        var lines: [String]

        switch api {
        case .anthropicMessages:
            lines = [
                "# Anthropic Messages — POST $ANTHROPIC_BASE_URL/v1/messages",
                "export ANTHROPIC_BASE_URL=\(base)",
                "export ANTHROPIC_API_KEY=\(key)",
            ]
        case .openAIChat:
            lines = [
                "# OpenAI Chat — POST $OPENAI_BASE_URL/chat/completions",
                "export OPENAI_BASE_URL=\(base)/v1",
                "export OPENAI_API_KEY=\(key)",
            ]
        case .openAIResponses:
            lines = [
                "# OpenAI Responses — POST $OPENAI_BASE_URL/responses",
                "export OPENAI_BASE_URL=\(base)/v1",
                "export OPENAI_API_KEY=\(key)",
            ]
        case .geminiGenerateContent:
            lines = [
                "# Gemini generateContent — POST $GOOGLE_GEMINI_BASE_URL/v1beta/models/…:generateContent",
                "export GOOGLE_GEMINI_BASE_URL=\(base)",
                "export GEMINI_API_KEY=\(key)",
            ]
        }

        if let label {
            lines.append("# next key label: \(label)")
        }
        if let model, !model.isEmpty {
            lines.append("export MODEL=\(model)")
        }
        return lines.joined(separator: "\n")
    }

    private static func curl(
        api: ConnectClientAPI,
        base: String,
        key: String,
        label: String?,
        model inputModel: String?
    ) -> String {
        let explicitModel = normalized(inputModel)
        let model = explicitModel ?? OnboardingSupport.defaultExampleModel
        let prompt = OnboardingSupport.credentialsDemoPrompt
        let endpoint: String
        let body: String
        var headers: [(String, String)]

        switch api {
        case .anthropicMessages:
            endpoint = "\(base)/v1/messages"
            headers = [
                ("x-api-key", key),
                ("anthropic-version", "2023-06-01"),
                ("content-type", "application/json"),
            ]
            body = "{\"model\":\(jsonQuoted(model)),\"max_tokens\":256,\"messages\":[{\"role\":\"user\",\"content\":\(jsonQuoted(prompt))}]}"
        case .openAIChat:
            endpoint = "\(base)/v1/chat/completions"
            headers = [
                ("Authorization", "Bearer \(key)"),
                ("content-type", "application/json"),
            ]
            body = "{\"model\":\(jsonQuoted(model)),\"messages\":[{\"role\":\"user\",\"content\":\(jsonQuoted(prompt))}]}"
        case .openAIResponses:
            endpoint = "\(base)/v1/responses"
            headers = [
                ("Authorization", "Bearer \(key)"),
                ("content-type", "application/json"),
            ]
            // The codex upstream requires the list form of `input`; a bare
            // string (though valid per the OpenAI spec) is rejected upstream.
            body = "{\"model\":\(jsonQuoted(model)),\"input\":[{\"role\":\"user\",\"content\":\(jsonQuoted(prompt))}]}"
        case .geminiGenerateContent:
            let pathModel = model.addingPercentEncoding(
                withAllowedCharacters: CharacterSet.urlPathAllowed
                    .subtracting(CharacterSet(charactersIn: "/"))) ?? model
            endpoint = "\(base)/v1beta/models/\(pathModel):generateContent"
            headers = [
                ("x-goog-api-key", key),
                ("content-type", "application/json"),
            ]
            body = "{\"contents\":[{\"role\":\"user\",\"parts\":[{\"text\":\(jsonQuoted(prompt))}]}]}"
        }

        if let label {
            headers.append(("x-session-id", label))
        }
        if let explicitModel {
            headers.append(("x-alex-task", explicitModel))
        }

        var lines = [
            "curl -sS -X POST \\",
            "  \(shellQuote(endpoint)) \\",
        ]
        lines.append(contentsOf: headers.map {
            "  -H \(shellQuote("\($0.0): \($0.1)")) \\"
        })
        lines.append("  --data \(shellQuote(body))")
        return lines.joined(separator: "\n")
    }

    private static func normalized(_ value: String?) -> String? {
        guard let value else { return nil }
        let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }

    private static func jsonQuoted(_ value: String) -> String {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.withoutEscapingSlashes]
        let data = try? encoder.encode(value)
        return data.flatMap { String(data: $0, encoding: .utf8) } ?? "\"\""
    }

    private static func shellQuote(_ value: String) -> String {
        "'" + value.replacingOccurrences(of: "'", with: "'\"'\"'") + "'"
    }
}

public extension CredentialRunKey {
    /// Harness name recorded by `alex connect`; nil for ordinary run keys.
    var harnessName: String? {
        guard kind.caseInsensitiveCompare("harness") == .orderedSame,
              case let .string(name)? = tags["harness"]
        else { return nil }
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }

    var shortFingerprint: String {
        String(keyFingerprint.prefix(10))
    }
}

public extension Array where Element == CredentialRunKey {
    /// Joins a connected harness to its newest active inventory key.
    func activeHarnessKey(named name: String) -> CredentialRunKey? {
        filter {
            !$0.revoked && $0.harnessName?.caseInsensitiveCompare(name) == .orderedSame
        }
        .max { lhs, rhs in
            if lhs.createdMs != rhs.createdMs { return lhs.createdMs < rhs.createdMs }
            return lhs.id.localizedCaseInsensitiveCompare(rhs.id) == .orderedAscending
        }
    }
}

public struct OutboundCredentialPresentation: Sendable, Equatable {
    public let kind: String
    public let source: String
    public let expiry: String?
    public let state: AccountDisplayState

    public var stateLabel: String {
        switch state {
        case .active: "Active"
        case .needsReauth: "Needs re-auth"
        case .degraded: "Degraded"
        case .unreachable: "Unreachable"
        case .unknown: "Unknown"
        }
    }
}

public extension OutboundCredential {
    /// Enriches the redacted outbound row with matching account/health data.
    func presentation(
        accounts: [Account],
        healthAccounts: [HealthAccount],
        now: Date = Date()
    ) -> OutboundCredentialPresentation {
        let account = accounts.first(where: { $0.id == credentialID })
            ?? accounts.first(where: { account in
                guard account.provider.caseInsensitiveCompare(provider ?? "") == .orderedSame
                else { return false }
                guard let name else { return true }
                return account.name.caseInsensitiveCompare(name) == .orderedSame
            })
        let health = healthAccounts.first(where: { $0.id == account?.id })
            ?? healthAccounts.first(where: { $0.id == credentialID })
        let state: AccountDisplayState
        if let account {
            state = account.displayState(
                lastPingOK: health?.lastHeartbeat?.ok,
                lastPingStatus: health?.lastHeartbeat?.status)
        } else {
            state = AccountDisplayState.derive(
                status: active ? "active" : "inactive",
                kind: kind,
                needsReauth: active ? false : true,
                expiresInS: nil,
                health: nil,
                lastPingOK: health?.lastHeartbeat?.ok,
                lastPingStatus: health?.lastHeartbeat?.status)
        }

        let expiryMs = expiresAtMs ?? account?.expiresAtMs
        let expiry = expiryMs.map {
            Self.relativeExpiry(expiresAtMs: $0, nowMs: Int64(now.timeIntervalSince1970 * 1_000))
        } ?? health?.tokenExpiresInS.map { Self.relativeExpiry(seconds: $0) }

        return OutboundCredentialPresentation(
            kind: Self.kindLabel(kind),
            source: Self.sourceLabel(source),
            expiry: expiry,
            state: state)
    }

    private static func kindLabel(_ raw: String) -> String {
        switch raw.lowercased() {
        case "oauth", "oauth_subscription": "OAuth subscription"
        case "api_key", "apikey": "API key"
        case "harness", "harness_login", "harness_sign_in": "Harness sign-in"
        default: raw.replacingOccurrences(of: "_", with: " ").capitalized
        }
    }

    private static func sourceLabel(_ raw: String?) -> String {
        guard let raw, !raw.isEmpty else { return "unknown source" }
        if raw.lowercased().contains("vault") { return "vault" }
        if raw.lowercased().contains("file") { return "file" }
        return raw.replacingOccurrences(of: "_", with: " ")
    }

    private static func relativeExpiry(expiresAtMs: Int64, nowMs: Int64) -> String {
        relativeExpiry(seconds: (expiresAtMs - nowMs) / 1_000)
    }

    private static func relativeExpiry(seconds: Int64) -> String {
        let future = seconds >= 0
        let amount = abs(seconds)
        let value: String
        if amount >= 86_400 { value = "\(max(1, amount / 86_400))d" }
        else if amount >= 3_600 { value = "\(max(1, amount / 3_600))h" }
        else if amount >= 60 { value = "\(max(1, amount / 60))m" }
        else { value = "\(amount)s" }
        return future ? "expires in \(value)" : "expired \(value) ago"
    }
}
