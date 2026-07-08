import Foundation

public struct TraceSessionsResponse: Codable, Sendable {
    public let sessions: [TraceSession]
}

public struct TraceSession: Codable, Sendable, Identifiable {
    public let sessionId: String
    public let runId: String?
    public let firstTsMs: Int64
    public let lastTsMs: Int64
    public let traceCount: Int
    public let models: [String]?
    public let harness: String?
    public let totalInputTokens: Int64?
    public let totalOutputTokens: Int64?
    public let totalCostUsd: Double?
    public let errors: Int64?
    public let lastStatus: Int?
    public let tags: [String: String]?

    public var id: String { sessionId }

    public var isPingOrTest: Bool {
        SessionKind.isPingOrTest(sessionId: sessionId, harness: harness)
    }

    enum CodingKeys: String, CodingKey {
        case models, harness, errors, tags
        case sessionId = "session_id"
        case runId = "run_id"
        case firstTsMs = "first_ts_ms"
        case lastTsMs = "last_ts_ms"
        case traceCount = "trace_count"
        case totalInputTokens = "total_input_tokens"
        case totalOutputTokens = "total_output_tokens"
        case totalCostUsd = "total_cost_usd"
        case lastStatus = "last_status"
    }
}

public struct TranscriptResponse: Codable, Sendable {
    public let sessionId: String
    public let turns: [TranscriptTurn]

    enum CodingKeys: String, CodingKey {
        case turns
        case sessionId = "session_id"
    }
}

public struct TranscriptTurn: Codable, Sendable, Identifiable {
    public let traceId: String
    public let tsRequestMs: Int64
    public let tsResponseMs: Int64?
    public let model: String?
    public let status: Int?
    public let inputTokens: Int64?
    public let outputTokens: Int64?
    public let costUsd: Double?
    public let error: String?
    public let user: String?
    public let assistant: String?

    public var id: String { traceId }

    enum CodingKeys: String, CodingKey {
        case model, status, error, user, assistant
        case traceId = "trace_id"
        case tsRequestMs = "ts_request_ms"
        case tsResponseMs = "ts_response_ms"
        case inputTokens = "input_tokens"
        case outputTokens = "output_tokens"
        case costUsd = "cost_usd"
    }
}

public struct TraceSearchResponse: Codable, Sendable {
    public let traces: [TraceSearchRow]
    public let scanned: Int?
}

public struct TraceSearchRow: Codable, Sendable {
    public let id: String
    public let sessionId: String?

    enum CodingKeys: String, CodingKey {
        case id
        case sessionId = "session_id"
    }
}

public enum SessionKind {
    public static func isPingOrTest(sessionId: String, harness: String?) -> Bool {
        if let harness, harness.contains("alexandria-ping") { return true }
        return sessionId.hasPrefix("tsh-")
            || sessionId.hasPrefix("alexandria-e2e-")
            || sessionId.hasPrefix("smoke-")
    }
}

public struct OmniQuery: Equatable, Sendable {
    public var freeText = ""
    public var model: String?
    public var provider: String?
    public var harness: String?
    public var status: String?
    public var run: String?
    public var session: String?

    public init() {}

    public var isEmpty: Bool {
        freeText.isEmpty && !hasTokenFilters
    }

    public var hasTokenFilters: Bool {
        model != nil || provider != nil || harness != nil
            || status != nil || run != nil || session != nil
    }

    public static func parse(_ raw: String) -> OmniQuery {
        var query = OmniQuery()
        var free: [String] = []
        for word in raw.split(whereSeparator: \.isWhitespace) {
            if let colon = word.firstIndex(of: ":"), colon != word.startIndex {
                let key = word[..<colon].lowercased()
                let value = String(word[word.index(after: colon)...])
                if !value.isEmpty {
                    switch key {
                    case "model": query.model = value; continue
                    case "provider": query.provider = value; continue
                    case "harness": query.harness = value; continue
                    case "status": query.status = value; continue
                    case "run": query.run = value; continue
                    case "session": query.session = value; continue
                    default: break
                    }
                }
            }
            free.append(String(word))
        }
        query.freeText = free.joined(separator: " ")
        return query
    }

    public func matches(_ session: TraceSession) -> Bool {
        if let model {
            let models = session.models ?? []
            guard models.contains(where: { $0.localizedCaseInsensitiveContains(model) }) else {
                return false
            }
        }
        if let harness {
            guard session.harness?.localizedCaseInsensitiveContains(harness) == true else {
                return false
            }
        }
        if let sid = self.session {
            guard session.sessionId.localizedCaseInsensitiveContains(sid) else { return false }
        }
        if let run {
            guard session.runId?.localizedCaseInsensitiveContains(run) == true else { return false }
        }
        if let status {
            guard let last = session.lastStatus, String(last) == status else { return false }
        }
        return true
    }
}

public enum LiveFollow {
    public static func shouldSwitch(
        pinned: Bool, currentIdleMs: Int64, userAtBottom: Bool, awayFromBottomMs: Int64
    ) -> Bool {
        if pinned { return false }
        guard currentIdleMs > 20_000 else { return false }
        if userAtBottom { return true }
        return awayFromBottomMs >= 60_000
    }
}
