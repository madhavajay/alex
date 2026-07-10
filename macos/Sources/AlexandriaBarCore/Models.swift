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
    public let name: String
    public let kind: String
    public let label: String?
    public let description: String?
    public let email: String?
    public let paused: Bool
    public let status: String
    public let expiresAtMs: Int64?
    public let expiresInS: Int64?

    enum CodingKeys: String, CodingKey {
        case id, provider, name, kind, label, description, email, paused, status
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

public struct AccountAnalyticsResponse: Codable, Sendable {
    public let sinceMs: Int64
    public let bucketMs: Int64
    public let byAccount: [AccountUsage]
    public let series: [AccountUsageBucket]

    enum CodingKeys: String, CodingKey {
        case sinceMs = "since_ms"
        case bucketMs = "bucket_ms"
        case byAccount = "by_account"
        case series
    }
}

public struct AccountUsage: Codable, Sendable, Identifiable {
    public let accountId: String
    public let provider: String?
    public let requests: Int64
    public let inputTokens: Int64
    public let outputTokens: Int64
    public let costUsd: Double
    public let errors: Int64?
    public let lastTsMs: Int64?

    public var id: String { accountId }

    enum CodingKeys: String, CodingKey {
        case provider, requests, errors
        case accountId = "account_id"
        case inputTokens = "input_tokens"
        case outputTokens = "output_tokens"
        case costUsd = "cost_usd"
        case lastTsMs = "last_ts_ms"
    }
}

public struct AccountUsageBucket: Codable, Sendable, Identifiable {
    public let bucketMs: Int64
    public let accountId: String
    public let requests: Int64
    public let inputTokens: Int64
    public let outputTokens: Int64
    public let costUsd: Double
    public let errors: Int64?

    public var id: String { "\(bucketMs):\(accountId)" }

    enum CodingKeys: String, CodingKey {
        case requests, errors
        case bucketMs = "bucket_ms"
        case accountId = "account_id"
        case inputTokens = "input_tokens"
        case outputTokens = "output_tokens"
        case costUsd = "cost_usd"
    }
}

public enum CodexRoutingStrategy: String, Codable, Sendable, CaseIterable, Hashable {
    case resetFirst = "reset_first"
    case priority
    case roundRobin = "round_robin"
}

public struct CodexRoutingResponse: Codable, Sendable {
    public let provider: String
    public let strategy: CodexRoutingStrategy
    public let reservePct: Double
    public let accounts: [CodexRoutingAccount]

    enum CodingKeys: String, CodingKey {
        case provider, strategy, accounts
        case reservePct = "reserve_pct"
    }
}

public struct CodexRoutingAccount: Codable, Sendable, Identifiable {
    public let accountId: String
    public let eligible: Bool
    public let priority: Int
    public let observedAtMs: Int64?
    public let windows: [LimitWindow]

    public var id: String { accountId }

    enum CodingKeys: String, CodingKey {
        case eligible, priority, windows
        case accountId = "account_id"
        case observedAtMs = "observed_at_ms"
    }

    public init(
        accountId: String,
        eligible: Bool,
        priority: Int,
        observedAtMs: Int64? = nil,
        windows: [LimitWindow] = []
    ) {
        self.accountId = accountId
        self.eligible = eligible
        self.priority = priority
        self.observedAtMs = observedAtMs
        self.windows = windows
    }
}

public struct CodexRoutingUpdate: Codable, Sendable {
    public let strategy: CodexRoutingStrategy
    public let reservePct: Double
    public let accounts: [CodexRoutingAccountUpdate]

    enum CodingKeys: String, CodingKey {
        case strategy, accounts
        case reservePct = "reserve_pct"
    }

    public init(
        strategy: CodexRoutingStrategy,
        reservePct: Double,
        accounts: [CodexRoutingAccountUpdate]
    ) {
        self.strategy = strategy
        self.reservePct = reservePct
        self.accounts = accounts
    }
}

public struct CodexRoutingAccountUpdate: Codable, Sendable {
    public let accountId: String
    public let eligible: Bool
    public let priority: Int

    enum CodingKeys: String, CodingKey {
        case eligible, priority
        case accountId = "account_id"
    }

    public init(accountId: String, eligible: Bool, priority: Int) {
        self.accountId = accountId
        self.eligible = eligible
        self.priority = priority
    }
}

public extension LimitWindow {
    var remainingPct: Double? {
        usedPct.map { max(0, min(100, 100 - $0)) }
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

public struct DaemonUpdateStatus: Codable, Sendable, Equatable {
    public let current: String
    public let latest: String?
    public let updateAvailable: Bool
    public let notesUrl: String?
    public let checkedAtMs: Int64?

    enum CodingKeys: String, CodingKey {
        case current, latest
        case updateAvailable = "update_available"
        case notesUrl = "notes_url"
        case checkedAtMs = "checked_at_ms"
    }
}

public struct DaemonUpdateApplyResponse: Codable, Sendable, Equatable {
    public let applying: Bool
    public let current: String?
    public let latest: String?
    public let updateAvailable: Bool?
    public let notesUrl: String?
    public let reason: String?

    enum CodingKeys: String, CodingKey {
        case applying, current, latest, reason
        case updateAvailable = "update_available"
        case notesUrl = "notes_url"
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
        case "anthropic", "openai", "gemini": provider
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
