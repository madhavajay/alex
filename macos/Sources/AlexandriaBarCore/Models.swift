import Foundation

public struct DaemonHealth: Codable, Sendable {
    public let status: String
    public let service: String
    public let version: String
    public let inFlight: Int
    public let uptimeS: Int64
    public let dario: Bool

    enum CodingKeys: String, CodingKey {
        case status, service, version, dario
        case inFlight = "in_flight"
        case uptimeS = "uptime_s"
    }
}

public struct AccountsResponse: Codable, Sendable {
    public let accounts: [Account]
}

public struct Account: Codable, Sendable, Identifiable {
    public let id: String
    public let provider: String
    public let kind: String
    public let label: String?
    public let status: String
    public let expiresAtMs: Int64?
    public let expiresInS: Int64?

    enum CodingKeys: String, CodingKey {
        case id, provider, kind, label, status
        case expiresAtMs = "expires_at_ms"
        case expiresInS = "expires_in_s"
    }

    public var isExpired: Bool { (expiresInS ?? 1) < 0 }
}

public struct HealthResponse: Codable, Sendable {
    public let accounts: [HealthAccount]
}

public struct HealthAccount: Codable, Sendable, Identifiable {
    public let id: String
    public let provider: String
    public let kind: String
    public let status: String
    public let tokenExpiresInS: Int64?
    public let lastHeartbeat: Heartbeat?

    enum CodingKeys: String, CodingKey {
        case id, provider, kind, status
        case tokenExpiresInS = "token_expires_in_s"
        case lastHeartbeat = "last_heartbeat"
    }
}

public struct Heartbeat: Codable, Sendable {
    public let ok: Bool
    public let status: Int?
    public let latencyMs: Int64?
    public let message: String?
    public let tsMs: Int64

    enum CodingKeys: String, CodingKey {
        case ok, status, message
        case latencyMs = "latency_ms"
        case tsMs = "ts_ms"
    }
}

public struct LimitsResponse: Codable, Sendable {
    public let providers: [ProviderLimits]
}

public struct ProviderLimits: Codable, Sendable {
    public let provider: String
    public let plan: String?
    public let source: String?
    public let error: String?
    public let windows: [LimitWindow]?
    public let requests: CountPair?
    public let tokens: CountPair?
    public let observedAtMs: Int64?

    enum CodingKeys: String, CodingKey {
        case provider, plan, source, error, windows, requests, tokens
        case observedAtMs = "observed_at_ms"
    }
}

public struct CountPair: Codable, Sendable {
    public let limit: Int64?
    public let remaining: Int64?
}

public struct LimitWindow: Codable, Sendable {
    public let window: String
    public let usedPct: Double?
    public let resetsAt: String?
    public let resetsAtS: Int64?

    enum CodingKeys: String, CodingKey {
        case window
        case usedPct = "used_pct"
        case resetsAt = "resets_at"
        case resetsAtS = "resets_at_s"
    }

    public var resetsDate: Date? {
        if let s = resetsAtS { return Date(timeIntervalSince1970: TimeInterval(s)) }
        guard let iso = resetsAt else { return nil }
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        if let d = f.date(from: iso) { return d }
        f.formatOptions = [.withInternetDateTime]
        return f.date(from: iso)
    }
}

public struct Analytics: Codable, Sendable {
    public let totals: AnalyticsTotals
    public let byModel: [ModelAnalytics]
    public let sinceMs: Int64

    enum CodingKeys: String, CodingKey {
        case totals
        case byModel = "by_model"
        case sinceMs = "since_ms"
    }
}

public struct AnalyticsTotals: Codable, Sendable {
    public let requests: Int64
    public let costUsd: Double
    public let errors: Int64
    public let costByBucket: [String: Double]?

    enum CodingKeys: String, CodingKey {
        case requests, errors
        case costUsd = "cost_usd"
        case costByBucket = "cost_by_bucket"
    }
}

public struct ModelAnalytics: Codable, Sendable {
    public let routedModel: String
    public let upstreamProvider: String?
    public let requests: Int64
    public let errors: Int64
    public let costUsd: Double
    public let avgLatencyMs: Double?

    enum CodingKeys: String, CodingKey {
        case requests, errors
        case routedModel = "routed_model"
        case upstreamProvider = "upstream_provider"
        case costUsd = "cost_usd"
        case avgLatencyMs = "avg_latency_ms"
    }
}

public struct DarioStatus: Codable, Sendable {
    public let activeGenerationId: String?
    public let generations: [DarioGeneration]

    enum CodingKeys: String, CodingKey {
        case activeGenerationId = "active_generation_id"
        case generations
    }
}

public struct DarioGeneration: Codable, Sendable, Identifiable {
    public let id: String
    public let version: String
    public let phase: String
    public let pid: Int?
    public let port: Int?
    public let inFlight: Int?
    public let lastProbe: DarioProbe?

    enum CodingKeys: String, CodingKey {
        case id, version, phase, pid, port
        case inFlight = "in_flight"
        case lastProbe = "last_probe"
    }
}

public struct DarioProbe: Codable, Sendable {
    public let ok: Bool
    public let latencyMs: Int64?
    public let error: String?
    public let atMs: Int64?

    enum CodingKeys: String, CodingKey {
        case ok, error
        case latencyMs = "latency_ms"
        case atMs = "at_ms"
    }
}

public struct LoginSession: Codable, Sendable, Identifiable {
    public let loginId: String
    public let provider: String
    public let mode: String
    public let state: String
    public let accountId: String?
    public let error: String?
    public let authorizeUrl: String?
    public let userCode: String?
    public let verificationUri: String?
    public let verificationUriComplete: String?
    public let expiresAtMs: Int64?

    public var id: String { loginId }
    public var isPending: Bool { state == "pending" }
    public var isDone: Bool { state == "done" }

    enum CodingKeys: String, CodingKey {
        case provider, mode, state, error
        case loginId = "login_id"
        case accountId = "account_id"
        case authorizeUrl = "authorize_url"
        case userCode = "user_code"
        case verificationUri = "verification_uri"
        case verificationUriComplete = "verification_uri_complete"
        case expiresAtMs = "expires_at_ms"
    }
}

public struct ImportOutcomes: Codable, Sendable {
    public let outcomes: [ImportOutcome]
}

public struct ImportOutcome: Codable, Sendable {
    public let source: String
    public let imported: [String]
    public let note: String?
}

public enum ProviderInfo {
    public static func displayName(_ provider: String) -> String {
        switch provider {
        case "anthropic": "Claude"
        case "openai": "Codex"
        case "xai": "Grok"
        case "gemini": "Gemini"
        default: provider.capitalized
        }
    }

    public static func loginArg(_ provider: String) -> String {
        switch provider {
        case "anthropic": "claude"
        case "openai": "codex"
        case "xai": "grok"
        default: provider
        }
    }

    public static func pingArg(_ provider: String) -> String? {
        switch provider {
        case "anthropic", "openai": provider
        case "xai": "grok"
        default: nil
        }
    }
}

public enum Format {
    public static func duration(_ seconds: Int64) -> String {
        let s = abs(seconds)
        if s >= 86400 { return "\(s / 86400)d \((s % 86400) / 3600)h" }
        if s >= 3600 { return "\(s / 3600)h \((s % 3600) / 60)m" }
        if s >= 60 { return "\(s / 60)m" }
        return "\(s)s"
    }

    public static func countdown(to date: Date, from now: Date = Date()) -> String {
        let delta = Int64(date.timeIntervalSince(now))
        if delta <= 0 { return "now" }
        return duration(delta)
    }
}
