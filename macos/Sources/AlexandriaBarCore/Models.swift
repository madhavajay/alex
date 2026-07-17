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

/// A delivery threshold for daemon runtime notifications.
public enum NotificationLevel: String, Codable, Sendable, CaseIterable {
    case info
    case warn
    case critical
}

/// Redacted notification settings returned by the local admin API. Tokens and
/// webhook URLs are deliberately not part of this representation.
public struct NotificationSettingsResponse: Codable, Sendable {
    public let channels: [NotificationChannel]
    public let cooldownSeconds: Int
    public let timeoutSeconds: Int

    enum CodingKeys: String, CodingKey {
        case channels
        case cooldownSeconds = "cooldown_seconds"
        case timeoutSeconds = "timeout_seconds"
    }
}

/// One redacted runtime notification channel.
public struct NotificationChannel: Codable, Sendable {
    public let index: Int
    public let id: String?
    public let kind: String
    public let format: String
    public let host: String?
    public let botUsername: String?
    public let chatID: String?
    public let supportsReplies: Bool?
    public let minLevel: NotificationLevel
    public let categories: [String]
    public let lastSentMs: Int64?
    public let lastError: String?

    public var stableID: String { id ?? "channel-\(index)" }

    enum CodingKeys: String, CodingKey {
        case index, id, kind, format, host, categories
        case botUsername = "bot_username"
        case chatID = "chat_id"
        case supportsReplies = "supports_replies"
        case minLevel = "min_level"
        case lastSentMs = "last_sent_ms"
        case lastError = "last_error"
    }
}

/// The credential-bearing request used only while creating or testing a
/// Telegram channel. Do not persist or render this value outside a secure field.
public struct TelegramNotificationChannelRequest: Encodable, Sendable {
    public let format = "telegram"
    public let token: String
    public let chatID: String
    public let minLevel: NotificationLevel
    public let categories: [String]

    public init(token: String, chatID: String, minLevel: NotificationLevel, categories: [String]) {
        self.token = token
        self.chatID = chatID
        self.minLevel = minLevel
        self.categories = categories
    }

    enum CodingKeys: String, CodingKey {
        case format, token, categories
        case chatID = "chat_id"
        case minLevel = "min_level"
    }
}

/// A short-lived Telegram token request used for validation and chat discovery.
public struct TelegramNotificationTokenRequest: Encodable, Sendable {
    public let format = "telegram"
    public let token: String

    public init(token: String) {
        self.token = token
    }
}

public struct NotificationValidationResponse: Codable, Sendable {
    public let ok: Bool
    public let botUsername: String?
    public let botName: String?
    public let error: String?

    enum CodingKeys: String, CodingKey {
        case ok, error
        case botUsername = "bot_username"
        case botName = "bot_name"
    }
}

public struct NotificationChat: Codable, Sendable, Identifiable {
    public let chatID: String
    public let chatName: String

    public var id: String { chatID }

    enum CodingKeys: String, CodingKey {
        case chatID = "chat_id"
        case chatName = "chat_name"
    }
}

/// Supports both the runtime API's `{ ok, chats }` response and the original
/// bare-array contract.
public struct NotificationChatDiscoveryResponse: Decodable, Sendable {
    public let ok: Bool
    public let chats: [NotificationChat]
    public let error: String?

    private enum CodingKeys: String, CodingKey { case ok, chats, error }

    public init(from decoder: Decoder) throws {
        if let chats = try? [NotificationChat](from: decoder) {
            self.ok = true
            self.chats = chats
            self.error = nil
            return
        }
        let container = try decoder.container(keyedBy: CodingKeys.self)
        self.ok = try container.decodeIfPresent(Bool.self, forKey: .ok) ?? true
        self.chats = try container.decodeIfPresent([NotificationChat].self, forKey: .chats) ?? []
        self.error = try container.decodeIfPresent(String.self, forKey: .error)
    }
}

public struct NotificationTestResult: Codable, Sendable {
    public let ok: Bool
    public let error: String?
}

/// Supports the runtime API's `{ channels: [...] }` response and a direct
/// per-channel result.
public struct NotificationTestResponse: Decodable, Sendable {
    public let channels: [NotificationTestResult]

    private enum CodingKeys: String, CodingKey { case channels }

    public init(from decoder: Decoder) throws {
        if let channels = try? [NotificationTestResult](from: decoder) {
            self.channels = channels
            return
        }
        let container = try decoder.container(keyedBy: CodingKeys.self)
        if let channels = try container.decodeIfPresent([NotificationTestResult].self, forKey: .channels) {
            self.channels = channels
        } else {
            self.channels = [try NotificationTestResult(from: decoder)]
        }
    }
}

public struct NotificationSaveResponse: Codable, Sendable {
    public let ok: Bool
    public let channel: NotificationChannel?
    public let error: String?
}

public struct Account: Codable, Sendable, Identifiable {
    public let id: String
    public let provider: String
    public let name: String
    public let kind: String
    public let label: String?
    public let description: String?
    public let email: String?
    public let limits: AccountLimits?
    public let paused: Bool
    public let status: String
    public let expiresAtMs: Int64?
    public let expiresInS: Int64?

    enum CodingKeys: String, CodingKey {
        case id, provider, name, kind, label, description, email, limits, paused, status
        case expiresAtMs = "expires_at_ms"
        case expiresInS = "expires_in_s"
    }

    public var isExpired: Bool { (expiresInS ?? 1) < 0 }
}

/// A quota snapshot tied to one credential rather than a provider-wide aggregate.
///
/// The daemon currently supplies these for Codex accounts. Keeping the shape
/// provider-neutral lets other subscription providers expose the same data later.
public struct AccountLimits: Codable, Sendable {
    public let plan: String?
    public let source: String?
    public let error: String?
    public let windows: [LimitWindow]?
    public let requests: CountPair?
    public let tokens: CountPair?
    public let observedAtMs: Int64?
    public let quota: QuotaState?

    enum CodingKeys: String, CodingKey {
        case plan, source, error, windows, requests, tokens, quota
        case observedAtMs = "observed_at_ms"
    }
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

/// Transient, provider-wide fault injection configured through `/admin/providers`.
/// It is intentionally separate from per-account `paused` routing state.
public enum ProviderPauseMode: String, Codable, Sendable, CaseIterable {
    case down
    case loggedOut = "logged_out"
}

public struct ProviderPause: Codable, Sendable, Identifiable {
    public let provider: String
    public let paused: Bool
    public let mode: ProviderPauseMode?

    public var id: String { provider }
}

public struct ProvidersResponse: Codable, Sendable {
    public let providers: [ProviderPause]
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
    /// The daemon-selected binding quota. Clients must not infer this from windows.
    public let quota: QuotaState?

    enum CodingKeys: String, CodingKey {
        case provider, plan, source, error, windows, requests, tokens, quota
        case observedAtMs = "observed_at_ms"
    }
}

/// The daemon's single source of truth for the quota users should act on first.
public struct QuotaState: Codable, Sendable {
    public let kind: String
    public let label: String
    public let balance: String?
    public let usedPct: Double?
    public let remainingPct: Double?
    public let topUpURL: String?

    enum CodingKeys: String, CodingKey {
        case kind, label, balance
        case usedPct = "used_pct"
        case remainingPct = "remaining_pct"
        case topUpURL = "top_up_url"
    }

    public var isCreditPrimary: Bool { kind != "rate_window" }
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
    /// Amp paid balance remaining (USD), when the window is credits / workspace.
    public let remainingUsd: Double?

    enum CodingKeys: String, CodingKey {
        case window
        case usedPct = "used_pct"
        case resetsAt = "resets_at"
        case resetsAtS = "resets_at_s"
        case remainingUsd = "remaining_usd"
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
    /// Plot-ready, bucket-aligned values from newer daemons. `series` above
    /// remains the legacy sparse-point response for mixed-version support.
    public let plotSeries: [AccountPlotSeries]?
    public let xLabels: [String]?
    public let bucketCount: Int?

    enum CodingKeys: String, CodingKey {
        case sinceMs = "since_ms"
        case bucketMs = "bucket_ms"
        case byAccount = "by_account"
        case series
        case plotSeries = "plot_series"
        case xLabels = "x_labels"
        case bucketCount = "bucket_count"
    }
}

public struct AccountPlotSeries: Codable, Sendable, Identifiable {
    public let accountId: String
    public let name: String
    public let values: [Double]

    public var id: String { accountId }

    enum CodingKeys: String, CodingKey {
        case name, values
        case accountId = "account_id"
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

/// The routing policy is provider-neutral.  The Codex names below remain as
/// source-compatible aliases for clients released before `/admin/routing`.
public typealias ProviderRoutingStrategy = CodexRoutingStrategy

public struct CodexRoutingResponse: Codable, Sendable {
    public let provider: String
    public let strategy: CodexRoutingStrategy
    public let reservePct: Double
    public let allowMidThreadFailover: Bool
    public let accounts: [CodexRoutingAccount]

    enum CodingKeys: String, CodingKey {
        case provider, strategy, accounts
        case reservePct = "reserve_pct"
        case allowMidThreadFailover = "allow_mid_thread_failover"
    }

    public init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        provider = try values.decode(String.self, forKey: .provider)
        strategy = try values.decode(CodexRoutingStrategy.self, forKey: .strategy)
        reservePct = try values.decode(Double.self, forKey: .reservePct)
        allowMidThreadFailover = try values.decodeIfPresent(
            Bool.self, forKey: .allowMidThreadFailover) ?? true
        accounts = try values.decode([CodexRoutingAccount].self, forKey: .accounts)
    }
}

public typealias ProviderRoutingResponse = CodexRoutingResponse

public struct CodexRoutingAccount: Codable, Sendable, Identifiable {
    public let accountId: String
    public let eligible: Bool
    public let priority: Int
    public let reservePct: Double?
    public let reserveBlocked: Bool
    public let observedAtMs: Int64?
    public let windows: [LimitWindow]
    public let resetSelection: CodexResetSelection?

    public var id: String { accountId }

    enum CodingKeys: String, CodingKey {
        case eligible, priority, windows
        case accountId = "account_id"
        case reservePct = "reserve_pct"
        case reserveBlocked = "reserve_blocked"
        case observedAtMs = "observed_at_ms"
        case resetSelection = "reset_selection"
    }

    public init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        accountId = try values.decode(String.self, forKey: .accountId)
        eligible = try values.decode(Bool.self, forKey: .eligible)
        priority = try values.decode(Int.self, forKey: .priority)
        reservePct = try values.decodeIfPresent(Double.self, forKey: .reservePct)
        reserveBlocked = try values.decodeIfPresent(Bool.self, forKey: .reserveBlocked) ?? false
        observedAtMs = try values.decodeIfPresent(Int64.self, forKey: .observedAtMs)
        windows = try values.decodeIfPresent([LimitWindow].self, forKey: .windows) ?? []
        resetSelection = try values.decodeIfPresent(
            CodexResetSelection.self, forKey: .resetSelection)
    }

    public init(
        accountId: String,
        eligible: Bool,
        priority: Int,
        reservePct: Double? = nil,
        reserveBlocked: Bool = false,
        observedAtMs: Int64? = nil,
        windows: [LimitWindow] = [],
        resetSelection: CodexResetSelection? = nil
    ) {
        self.accountId = accountId
        self.eligible = eligible
        self.priority = priority
        self.reservePct = reservePct
        self.reserveBlocked = reserveBlocked
        self.observedAtMs = observedAtMs
        self.windows = windows
        self.resetSelection = resetSelection
    }
}

public typealias ProviderRoutingAccount = CodexRoutingAccount

public struct CodexResetSelection: Codable, Sendable, Equatable {
    public let window: String?
    public let usedPct: Double
    public let resetsAtS: Int64

    enum CodingKeys: String, CodingKey {
        case window
        case usedPct = "used_pct"
        case resetsAtS = "resets_at_s"
    }

    public var resetsDate: Date {
        Date(timeIntervalSince1970: TimeInterval(resetsAtS))
    }
}

public struct CodexRoutingUpdate: Codable, Sendable {
    public let strategy: CodexRoutingStrategy
    public let reservePct: Double
    public let allowMidThreadFailover: Bool
    public let accounts: [CodexRoutingAccountUpdate]

    enum CodingKeys: String, CodingKey {
        case strategy, accounts
        case reservePct = "reserve_pct"
        case allowMidThreadFailover = "allow_mid_thread_failover"
    }

    public init(
        strategy: CodexRoutingStrategy,
        reservePct: Double,
        allowMidThreadFailover: Bool = true,
        accounts: [CodexRoutingAccountUpdate]
    ) {
        self.strategy = strategy
        self.reservePct = reservePct
        self.allowMidThreadFailover = allowMidThreadFailover
        self.accounts = accounts
    }
}

public typealias ProviderRoutingUpdate = CodexRoutingUpdate

public struct CodexRoutingAccountUpdate: Codable, Sendable {
    public let accountId: String
    public let eligible: Bool
    public let priority: Int
    public let reservePct: Double?

    enum CodingKeys: String, CodingKey {
        case eligible, priority
        case accountId = "account_id"
        case reservePct = "reserve_pct"
    }

    public init(
        accountId: String, eligible: Bool, priority: Int, reservePct: Double? = nil
    ) {
        self.accountId = accountId
        self.eligible = eligible
        self.priority = priority
        self.reservePct = reservePct
    }
}

public typealias ProviderRoutingAccountUpdate = CodexRoutingAccountUpdate

public enum RoutingReserve {
    /// A response from an older daemon can omit the per-account value; it then
    /// inherits the provider-wide reserve.
    public static func resolved(account: Double?, provider: Double) -> Double {
        min(100, max(0, account ?? provider))
    }

    /// Keep the important zero case explicit: it means quota never blocks an
    /// otherwise eligible account.
    public static func display(_ reservePct: Double) -> String {
        let value = Int(resolved(account: reservePct, provider: reservePct))
        return value == 0 ? "0% (never block)" : "\(value)% remaining"
    }
}

public extension LimitWindow {
    var remainingPct: Double? {
        usedPct.map { max(0, min(100, 100 - $0)) }
    }

    /// Interprets the existing warning preference as a used-quota threshold,
    /// while presenting the allowance as quota remaining.
    func remainingSeverity(warnUsedPct: Double) -> RemainingQuotaSeverity? {
        guard let remainingPct else { return nil }
        let warnUsedPct = max(0, min(100, warnUsedPct))
        let criticalRemaining = 100 - warnUsedPct
        let warningRemaining = 100 - (warnUsedPct * 0.75)
        if remainingPct <= criticalRemaining { return .critical }
        if remainingPct <= warningRemaining { return .warning }
        return .healthy
    }
}

public enum RemainingQuotaSeverity: Sendable, Equatable {
    case healthy
    case warning
    case critical
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
    public let promptCaches: [DarioPromptCacheSummary]?
    public let shouldBeHealthy: Bool?
    public let issue: DarioIssue?
    public let resolvedNodeBin: String?
    public let resolvedClaudeBin: String?
    public let runtimeVersion: String?
    public let routeEnabled: Bool?

    enum CodingKeys: String, CodingKey {
        case activeGenerationId = "active_generation_id"
        case generations
        case promptCaches = "prompt_caches"
        case shouldBeHealthy = "should_be_healthy"
        case issue
        case resolvedNodeBin = "resolved_node_bin"
        case resolvedClaudeBin = "resolved_claude_bin"
        case runtimeVersion = "runtime_version"
        case routeEnabled = "route_enabled"
    }
}

public struct DarioIssue: Codable, Sendable, Equatable {
    public let code: String
    public let message: String
    public let fixable: Bool
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

public struct ExoConfig: Codable, Sendable, Equatable {
    public var url: String
    public var enabledModels: [String]

    public init(url: String = "http://localhost:52415", enabledModels: [String] = []) {
        self.url = url
        self.enabledModels = enabledModels
    }

    enum CodingKeys: String, CodingKey {
        case url
        case enabledModels = "enabled_models"
    }
}

public struct ExoStatus: Codable, Sendable, Equatable {
    public let running: Bool
    public let url: String
    public let modelCount: Int
    public let error: String?

    enum CodingKeys: String, CodingKey {
        case running, url, error
        case modelCount = "model_count"
    }
}

public struct ExoModelsResponse: Codable, Sendable {
    public let models: [ExoModel]
}

public struct ExoModel: Codable, Sendable, Identifiable, Equatable {
    public let id: String
    public let name: String
    public let family: String?
    public let quantization: String?
    public let contextLength: Int?
    public var enabled: Bool
    public let running: Bool?

    enum CodingKeys: String, CodingKey {
        case id, name, family, quantization, enabled, running
        case contextLength = "context_length"
    }
}

public enum ProviderInfo {
    public static func displayName(_ provider: String) -> String {
        switch provider {
        case "anthropic": "Claude"
        case "openai": "Codex"
        case "xai": "Grok"
        case "gemini": "Gemini"
        case "amp": "Amp"
        case "openrouter": "OpenRouter"
        case "exo": "Exo"
        default: provider.capitalized
        }
    }

    public static func loginArg(_ provider: String) -> String {
        switch provider {
        case "anthropic": "claude"
        case "openai": "codex"
        case "xai": "grok"
        case "amp": "amp"
        default: provider
        }
    }

    public static func pingArg(_ provider: String) -> String? {
        switch provider {
        case "anthropic", "openai", "gemini", "amp", "openrouter", "exo": provider
        case "xai": "grok"
        default: nil
        }
    }

    public static var supportedProviders: [String] {
        ["anthropic", "openai", "gemini", "xai", "openrouter", "exo", "amp"]
    }

    public static func usesAPIKeySheet(_ provider: String) -> Bool {
        provider == "openrouter"
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
