import AppKit
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
        SessionKind.isPingOrTest(sessionId: sessionId, harness: harness, tags: tags)
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

public struct TraceDetailResponse: Codable, Sendable {
    public let trace: TraceDetail
    public let extras: TraceExtras?
}

public struct TraceDetail: Codable, Sendable {
    public let id: String
    public let sessionId: String?
    public let runId: String?
    public let harness: String?
    public let method: String?
    public let path: String?
    public let status: Int?
    public let error: String?
    public let tsRequestMs: Int64?
    public let tsResponseMs: Int64?
    public let latencyMs: Int64?
    public let requestedModel: String?
    public let routedModel: String?
    public let clientFormat: String?
    public let upstreamFormat: String?
    public let upstreamProvider: String?
    public let billingBucket: String?
    public let accountId: String?
    public let clientIp: String?
    public let keyFingerprint: String?
    public let inputTokens: Int64?
    public let cachedInputTokens: Int64?
    public let cacheCreationTokens: Int64?
    public let outputTokens: Int64?
    public let reasoningTokens: Int64?
    public let costUsd: Double?
    public let reqHeadersJson: String?
    public let respHeadersJson: String?
    public let reqBodyPath: String?
    public let upstreamReqBodyPath: String?
    public let respBodyPath: String?
    public let tagsJson: String?

    enum CodingKeys: String, CodingKey {
        case id, harness, method, path, status, error
        case sessionId = "session_id"
        case runId = "run_id"
        case tsRequestMs = "ts_request_ms"
        case tsResponseMs = "ts_response_ms"
        case latencyMs = "latency_ms"
        case requestedModel = "requested_model"
        case routedModel = "routed_model"
        case clientFormat = "client_format"
        case upstreamFormat = "upstream_format"
        case upstreamProvider = "upstream_provider"
        case billingBucket = "billing_bucket"
        case accountId = "account_id"
        case clientIp = "client_ip"
        case keyFingerprint = "key_fingerprint"
        case inputTokens = "input_tokens"
        case cachedInputTokens = "cached_input_tokens"
        case cacheCreationTokens = "cache_creation_tokens"
        case outputTokens = "output_tokens"
        case reasoningTokens = "reasoning_tokens"
        case costUsd = "cost_usd"
        case reqHeadersJson = "req_headers_json"
        case respHeadersJson = "resp_headers_json"
        case reqBodyPath = "req_body_path"
        case upstreamReqBodyPath = "upstream_req_body_path"
        case respBodyPath = "resp_body_path"
        case tagsJson = "tags_json"
    }
}

public struct TraceExtras: Codable, Sendable {
    public let reasoningEffort: String?
    public let thinkingBudget: Int64?
    public let maxTokens: Int64?
    public let temperature: Double?
    public let messageCount: Int?
    public let systemChars: Int?

    public var hasAny: Bool {
        reasoningEffort != nil || thinkingBudget != nil || maxTokens != nil
            || temperature != nil || messageCount != nil || systemChars != nil
    }

    enum CodingKeys: String, CodingKey {
        case temperature
        case reasoningEffort = "reasoning_effort"
        case thinkingBudget = "thinking_budget"
        case maxTokens = "max_tokens"
        case messageCount = "message_count"
        case systemChars = "system_chars"
    }
}

public struct HeaderPair: Equatable, Sendable {
    public let name: String
    public let value: String

    public init(name: String, value: String) {
        self.name = name
        self.value = value
    }
}

public enum TraceHeaders {
    public static func sortedPairs(_ json: String?) -> [HeaderPair] {
        guard let json, let data = json.data(using: .utf8),
            let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        else { return [] }
        return obj
            .map { HeaderPair(name: $0.key, value: Self.string($0.value)) }
            .sorted { $0.name.localizedCaseInsensitiveCompare($1.name) == .orderedAscending }
    }

    static func string(_ value: Any) -> String {
        switch value {
        case let s as String: s
        case let n as NSNumber: n.stringValue
        default: "\(value)"
        }
    }
}

public struct HeaderDelta: Equatable, Sendable {
    public let added: Set<String>
    public let removed: Set<String>
    public let changed: Set<String>

    public init(added: Set<String>, removed: Set<String>, changed: Set<String>) {
        self.added = added
        self.removed = removed
        self.changed = changed
    }

    public var isEmpty: Bool { added.isEmpty && removed.isEmpty && changed.isEmpty }

    public enum Status: Equatable, Sendable {
        case same, added, changed
    }

    public func status(for name: String) -> Status {
        let key = name.lowercased()
        if added.contains(key) { return .added }
        if changed.contains(key) { return .changed }
        return .same
    }
}

public enum HeaderDiff {
    public static func delta(first: [HeaderPair], other: [HeaderPair]) -> HeaderDelta {
        let firstMap = Dictionary(
            first.map { ($0.name.lowercased(), $0.value) }, uniquingKeysWith: { a, _ in a })
        let otherMap = Dictionary(
            other.map { ($0.name.lowercased(), $0.value) }, uniquingKeysWith: { a, _ in a })
        var added = Set<String>()
        var changed = Set<String>()
        for (key, value) in otherMap {
            guard let firstValue = firstMap[key] else {
                added.insert(key)
                continue
            }
            if firstValue != value { changed.insert(key) }
        }
        let removed = Set(firstMap.keys.filter { otherMap[$0] == nil })
        return HeaderDelta(added: added, removed: removed, changed: changed)
    }
}

public enum TraceLink {
    public static let scheme = "alexandria"
    public static let host = "trace"

    public static func url(forTraceId id: String) -> URL? {
        guard !id.isEmpty else { return nil }
        var comps = URLComponents()
        comps.scheme = scheme
        comps.host = host
        comps.path = "/" + id
        return comps.url
    }

    public static func traceId(from url: URL) -> String? {
        guard url.scheme == scheme, url.host == host else { return nil }
        let raw = url.path.hasPrefix("/") ? String(url.path.dropFirst()) : url.path
        let id = raw.removingPercentEncoding ?? raw
        return id.isEmpty ? nil : id
    }
}

public enum TurnHeader {
    public static let toolResultPrefix = "[tool result]"

    public static func duration(requestMs: Int64, responseMs: Int64?) -> String? {
        guard let responseMs, responseMs >= requestMs else { return nil }
        return String(format: "%.1fs", Double(responseMs - requestMs) / 1000.0)
    }

    public static func separatorFacts(
        turnNumber: Int, time: String, status: Int?,
        requestMs: Int64, responseMs: Int64?, costUsd: Double? = nil
    ) -> String {
        var parts = ["turn \(turnNumber)", time]
        if let status { parts.append("\(status)") }
        if let dur = duration(requestMs: requestMs, responseMs: responseMs) {
            parts.append(dur)
        }
        if let costUsd, costUsd > 0 { parts.append(TraceNumberFormat.cost(costUsd)) }
        return parts.joined(separator: " · ")
    }

    public static func requestLabel(harness: String, isToolResult: Bool = false) -> String {
        "⬆ \(harness) · \(isToolResult ? "tool result" : "user")"
    }

    public static func responseLabel(model: String?) -> String {
        model.map { "⬇ \($0) · assistant" } ?? "⬇ assistant"
    }

    public static func toolResultBody(_ text: String) -> String? {
        guard text.hasPrefix(toolResultPrefix) else { return nil }
        return String(text.dropFirst(toolResultPrefix.count))
            .trimmingCharacters(in: .whitespacesAndNewlines)
    }
}

public enum BodyPretty {
    public static let displayCap = 200_000

    public static func display(_ raw: String, cap: Int = displayCap) -> CappedText {
        var text = raw
        if let data = raw.data(using: .utf8),
            let obj = try? JSONSerialization.jsonObject(with: data),
            let pretty = try? JSONSerialization.data(
                withJSONObject: obj,
                options: [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes]),
            let str = String(data: pretty, encoding: .utf8) {
            text = str
        }
        let full = text.count
        guard full > cap else {
            return CappedText(text: text, isTruncated: false, fullCharCount: full)
        }
        let capped = String(text.prefix(cap)) + "\n… (+\(full - cap) chars truncated)"
        return CappedText(text: capped, isTruncated: true, fullCharCount: full)
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
    static let pingKinds: Set<String> = ["ping", "health", "preflight", "heartbeat"]
    static let pingPhases: Set<String> = ["preflight", "health", "ping"]
    static let testKinds: Set<String> = ["test", "smoke"]

    public static func isPingOrTest(
        sessionId: String, harness: String?, tags: [String: String]? = nil
    ) -> Bool {
        badge(sessionId: sessionId, harness: harness, tags: tags) != nil
    }

    public static func badge(
        sessionId: String, harness: String?, tags: [String: String]? = nil
    ) -> String? {
        if let harness, harness.contains("alexandria-ping") { return "ping" }
        if let kind = tags?["kind"]?.lowercased() {
            if pingKinds.contains(kind) { return "ping" }
            if testKinds.contains(kind) { return "test" }
        }
        if let phase = tags?["phase"]?.lowercased(), pingPhases.contains(phase) {
            return "ping"
        }
        if sessionId.hasPrefix("tsh-")
            || sessionId.hasPrefix("alexandria-e2e-")
            || sessionId.hasPrefix("smoke-") {
            return "test"
        }
        return nil
    }
}

public struct SessionRow: Identifiable, Sendable, Equatable {
    public let id: String
    public let lastTsMs: Int64
    public let lastTs: Date
    public let sessionShort: String
    public let models: String
    public let providers: [String]
    public let harness: String
    public let harnessRaw: String?
    public let tags: [String: String]?
    public let turns: Int
    public let tokensIn: Int64
    public let tokensOut: Int64
    public let cost: Double
    public let errors: Int
    public let runId: String
    public let tagsSummary: String
    public let kindBadge: String?
    public let iconAsset: String?

    public var isPingOrTest: Bool { kindBadge != nil }

    public init(session: TraceSession) {
        id = session.sessionId
        lastTsMs = session.lastTsMs
        lastTs = Date(timeIntervalSince1970: Double(session.lastTsMs) / 1000)
        sessionShort = Self.shortId(session.sessionId)
        let modelsList = session.models ?? []
        models = modelsList.joined(separator: ", ")
        providers = ModelProvider.providers(in: modelsList)
        harnessRaw = session.harness
        harness = session.harness ?? ""
        tags = session.tags
        turns = session.traceCount
        tokensIn = session.totalInputTokens ?? 0
        tokensOut = session.totalOutputTokens ?? 0
        cost = session.totalCostUsd ?? 0
        errors = Int(session.errors ?? 0)
        runId = session.runId ?? ""
        tagsSummary = (session.tags ?? [:])
            .filter { !$0.value.isEmpty }
            .sorted { $0.key < $1.key }
            .map { "\($0.key)=\($0.value)" }
            .joined(separator: " ")
        kindBadge = SessionKind.badge(
            sessionId: session.sessionId, harness: session.harness, tags: session.tags)
        iconAsset = HarnessIcon.assetName(harness: session.harness, tags: session.tags)
    }

    static func shortId(_ id: String, maxLength: Int = 22) -> String {
        guard id.count > maxLength else { return id }
        return "\(id.prefix(10))…\(id.suffix(8))"
    }
}

public enum SessionTable {
    public static func rowsById(_ sessions: [TraceSession]) -> [String: SessionRow] {
        Dictionary(
            sessions.map { ($0.sessionId, SessionRow(session: $0)) },
            uniquingKeysWith: { first, _ in first })
    }

    public static func defaultSortOrder() -> [KeyPathComparator<SessionRow>] {
        [KeyPathComparator(\.lastTs, order: .reverse)]
    }

    public static func visibleRows(
        sessions: [TraceSession],
        rowsById: [String: SessionRow],
        showPings: Bool,
        query: OmniQuery,
        serverMatches: Set<String>?,
        sortOrder: [KeyPathComparator<SessionRow>]
    ) -> [SessionRow] {
        var rows: [SessionRow] = []
        for session in sessions {
            if !showPings, session.isPingOrTest { continue }
            guard query.isVisible(session, serverMatches: serverMatches) else { continue }
            rows.append(rowsById[session.sessionId] ?? SessionRow(session: session))
        }
        return rows.sorted(using: sortOrder)
    }
}

public struct SessionSelection: Equatable, Sendable {
    public private(set) var selectedId: String?
    public private(set) var pinned = false
    private var lastFollowId: String?

    public init() {}

    public enum Change: Equatable, Sendable {
        case none
        case selected(String)
    }

    @discardableResult
    public mutating func userSelect(_ id: String) -> Change {
        lastFollowId = nil
        pinned = true
        guard selectedId != id else { return .none }
        selectedId = id
        return .selected(id)
    }

    @discardableResult
    public mutating func followSelect(_ id: String) -> Change {
        guard selectedId != id else { return .none }
        selectedId = id
        lastFollowId = id
        return .selected(id)
    }

    @discardableResult
    public mutating func bindingSelect(_ id: String?) -> Change {
        guard let id else { return .none }
        if id == lastFollowId {
            lastFollowId = nil
            return .none
        }
        return userSelect(id)
    }

    @discardableResult
    public mutating func setLive(_ live: Bool, newestVisibleId: String?) -> Change {
        pinned = !live
        guard live, let newestVisibleId else { return .none }
        return followSelect(newestVisibleId)
    }

    public mutating func clear() {
        selectedId = nil
        lastFollowId = nil
    }
}

public enum HarnessIcon {
    static let files: [String: String] = [
        "pi": "pi.svg",
        "codex": "codex.png",
        "claude-code": "claude-code.png",
        "grok-build": "grok-build.png",
        "opencode": "opencode.png",
        "qwen-code": "qwen-code.png",
        "gemini-cli": "gemini-cli.png",
        "mini-swe-agent": "mini-swe-agent.png",
        "kimi-code": "kimi-code.jpg",
        "goose": "goose.jpg",
        "hermes": "hermes.png",
        "droid-cli": "droid-cli.svg",
        "cursor-cli": "cursor-cli.png",
        "amp-code": "amp-code.svg",
        "opensage-adk": "opensage-adk.png",
        "stirrup": "stirrup.ico",
    ]

    static let aliases: [String: String] = [
        "claude": "claude-code",
        "grok": "grok-build",
        "qwen": "qwen-code",
        "gemini": "gemini-cli",
        "mini": "mini-swe-agent",
        "kimi": "kimi-code",
        "droid": "droid-cli",
        "cursor": "cursor-cli",
        "amp": "amp-code",
        "opensage": "opensage-adk",
    ]

    static let userAgentAliases: [String: String] = [
        "claude-cli": "claude-code",
        "codex-tui": "codex",
        "codex_exec": "codex",
        "grok-shell": "grok-build",
        "qwencode": "qwen-code",
        "factory-cli": "droid-cli",
        "kimi-code-cli": "kimi-code",
    ]

    public static func assetName(harness: String?, tags: [String: String]?) -> String? {
        canonicalKey(harness: harness, tags: tags).flatMap { files[$0] }
    }

    public static func canonicalKey(harness: String?, tags: [String: String]?) -> String? {
        if let tag = tags?["harness"] {
            let key = tag.lowercased().trimmingCharacters(in: .whitespaces)
            if let canonical = canonical(key)
                ?? canonical(key.replacingOccurrences(of: "_", with: "-")) {
                return canonical
            }
        }
        guard let token = userAgentToken(harness) else { return nil }
        if let canonical = userAgentAliases[token] { return canonical }
        return canonical(token)
    }

    public static func userAgentToken(_ harness: String?) -> String? {
        guard let harness else { return nil }
        let head = harness.split(whereSeparator: \.isWhitespace).first.map(String.init) ?? harness
        guard let token = head.split(separator: "/").first.map({ String($0).lowercased() }),
            !token.isEmpty
        else { return nil }
        return token
    }

    static func canonical(_ key: String) -> String? {
        if files[key] != nil { return key }
        return aliases[key]
    }

    static func resolve(_ key: String) -> String? {
        canonical(key).flatMap { files[$0] }
    }
}

public enum HarnessName {
    public static func display(harness: String?, tags: [String: String]?) -> String {
        if let tag = tags?["harness"]?.trimmingCharacters(in: .whitespaces), !tag.isEmpty {
            return tag
        }
        if let key = HarnessIcon.canonicalKey(harness: harness, tags: tags) { return key }
        if let token = HarnessIcon.userAgentToken(harness) { return token }
        return "harness"
    }
}

public enum ModelProvider {
    public static func provider(forModel model: String) -> String? {
        let m = model.lowercased()
        if m.hasPrefix("claude") { return "anthropic" }
        if m.hasPrefix("gpt") { return "openai" }
        if m.hasPrefix("o"), m.count > 1,
            m[m.index(after: m.startIndex)].isNumber {
            return "openai"
        }
        if m.hasPrefix("grok") { return "xai" }
        if m.hasPrefix("gemini") { return "gemini" }
        return nil
    }

    public static func initial(for provider: String) -> String {
        switch provider.lowercased() {
        case "anthropic": "A"
        case "openai": "O"
        case "xai": "X"
        case "gemini": "G"
        default: provider.first.map { String($0).uppercased() } ?? "?"
        }
    }

    public static func providers(in models: [String]?) -> [String] {
        var seen = Set<String>()
        var out: [String] = []
        for model in models ?? [] {
            guard let provider = provider(forModel: model), seen.insert(provider).inserted
            else { continue }
            out.append(provider)
        }
        return out
    }
}

public enum ListNavigation {
    public enum Move: Sendable {
        case up, down, home, end
    }

    public static func targetIndex(selected: Int?, count: Int, move: Move) -> Int? {
        guard count > 0 else { return nil }
        switch move {
        case .home: return 0
        case .end: return count - 1
        case .up:
            guard let selected else { return 0 }
            return max(0, selected - 1)
        case .down:
            guard let selected else { return 0 }
            return min(count - 1, selected + 1)
        }
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
    public var task: String?
    public var job: String?
    public var tag: String?

    public init() {}

    public var isEmpty: Bool {
        freeText.isEmpty && !hasTokenFilters
    }

    public var hasTokenFilters: Bool {
        model != nil || provider != nil || harness != nil
            || status != nil || run != nil || session != nil
            || task != nil || job != nil || tag != nil
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
                    case "task": query.task = value; continue
                    case "job": query.job = value; continue
                    case "tag": query.tag = value; continue
                    default: break
                    }
                }
            }
            free.append(String(word))
        }
        query.freeText = free.joined(separator: " ")
        return query
    }

    public static func settingToken(in raw: String, key: String, value: String?) -> String {
        let prefix = key.lowercased() + ":"
        var words = raw.split(whereSeparator: \.isWhitespace).map(String.init)
        words.removeAll { $0.lowercased().hasPrefix(prefix) }
        if let value, !value.isEmpty { words.append(prefix + value) }
        return words.joined(separator: " ")
    }

    public func matches(_ session: TraceSession) -> Bool {
        let tags = session.tags ?? [:]
        if let model {
            let models = session.models ?? []
            let inModels = models.contains { $0.localizedCaseInsensitiveContains(model) }
            let inTag = tags["model"]?.localizedCaseInsensitiveContains(model) == true
            guard inModels || inTag else { return false }
        }
        if let harness {
            let inField = session.harness?.localizedCaseInsensitiveContains(harness) == true
            let inTag = tags["harness"]?.localizedCaseInsensitiveContains(harness) == true
            guard inField || inTag else { return false }
        }
        if let task {
            guard tags["task"]?.localizedCaseInsensitiveContains(task) == true else { return false }
        }
        if let job {
            guard tags["job"]?.localizedCaseInsensitiveContains(job) == true else { return false }
        }
        if let tag {
            guard Self.tagTokenMatches(tag, tags: tags) else { return false }
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

    public func freeTextMatchesTags(_ session: TraceSession) -> Bool {
        guard !freeText.isEmpty, let tags = session.tags else { return false }
        return tags.values.contains { $0.localizedCaseInsensitiveContains(freeText) }
    }

    public func isVisible(_ session: TraceSession, serverMatches: Set<String>?) -> Bool {
        guard matches(session) else { return false }
        if freeText.isEmpty { return true }
        if freeTextMatchesTags(session) { return true }
        return serverMatches?.contains(session.sessionId) == true
    }

    static func tagTokenMatches(_ token: String, tags: [String: String]) -> Bool {
        if let eq = token.firstIndex(of: "="), eq != token.startIndex {
            let key = token[..<eq].lowercased()
            let value = String(token[token.index(after: eq)...])
            return tags.contains { pair in
                guard pair.key.lowercased() == key else { return false }
                return value.isEmpty || pair.value.localizedCaseInsensitiveContains(value)
            }
        }
        return tags.values.contains { $0.localizedCaseInsensitiveContains(token) }
    }
}

public struct TagChip: Equatable, Sendable {
    public let key: String
    public let value: String

    public init(key: String, value: String) {
        self.key = key
        self.value = value
    }

    public func label(maxValueLength: Int = 18) -> String {
        let shown = value.count > maxValueLength
            ? value.prefix(maxValueLength) + "…"
            : value[...]
        return "\(key)=\(shown)"
    }
}

public enum SessionTagChips {
    public static func chips(
        tags: [String: String]?, harness: String?, models: [String]?, limit: Int = 3
    ) -> [TagChip] {
        guard let tags else { return [] }
        var pool = tags.filter { !$0.value.isEmpty }
        if let model = pool["model"], (models ?? []).contains(model) {
            pool.removeValue(forKey: "model")
        }
        if let tagHarness = pool["harness"], tagHarness == harness {
            pool.removeValue(forKey: "harness")
        }
        var ordered: [TagChip] = []
        for key in ["task", "job"] {
            if let value = pool.removeValue(forKey: key) {
                ordered.append(TagChip(key: key, value: value))
            }
        }
        ordered += pool.sorted { $0.key < $1.key }.map { TagChip(key: $0.key, value: $0.value) }
        return Array(ordered.prefix(limit))
    }
}

public enum TagFilterDimension: String, CaseIterable, Sendable {
    case harness, task, job, model

    public var title: String { rawValue.capitalized }

    public func values(in sessions: [TraceSession]) -> [String] {
        var seen = Set<String>()
        var out: [String] = []
        func add(_ value: String?) {
            guard let value, !value.isEmpty, seen.insert(value).inserted else { return }
            out.append(value)
        }
        for session in sessions {
            let tags = session.tags ?? [:]
            switch self {
            case .harness:
                add(session.harness)
                add(tags["harness"])
            case .model:
                (session.models ?? []).forEach { add($0) }
                add(tags["model"])
            case .task:
                add(tags["task"])
            case .job:
                add(tags["job"])
            }
        }
        return out.sorted { $0.localizedCaseInsensitiveCompare($1) == .orderedAscending }
    }

    public func activeValue(in query: OmniQuery) -> String? {
        switch self {
        case .harness: query.harness
        case .task: query.task
        case .job: query.job
        case .model: query.model
        }
    }
}

public enum TraceFingerprint {
    public static func sessions(_ sessions: [TraceSession]) -> String {
        let newest = sessions.max { $0.lastTsMs < $1.lastTsMs }
        let totalTraces = sessions.reduce(0) { $0 + $1.traceCount }
        return "\(sessions.count)|\(newest?.sessionId ?? "")|\(newest?.lastTsMs ?? 0)|\(totalTraces)"
    }

    public static func turns(_ turns: [TranscriptTurn]) -> String {
        let last = turns.last
        return "\(turns.count)|\(last?.traceId ?? "")|\(last?.tsRequestMs ?? 0)"
            + "|\(last?.tsResponseMs ?? -1)|\(last?.status ?? -1)"
    }
}

public struct CappedText: Equatable, Sendable {
    public let text: String
    public let isTruncated: Bool
    public let fullCharCount: Int

    public init(text: String, isTruncated: Bool, fullCharCount: Int) {
        self.text = text
        self.isTruncated = isTruncated
        self.fullCharCount = fullCharCount
    }
}

public enum TurnTextCap {
    public static let maxChars = 4000
    public static let maxLines = 60

    public static func cap(
        _ text: String, maxChars: Int = maxChars, maxLines: Int = maxLines
    ) -> CappedText {
        let fullCount = text.count
        var out = fullCount > maxChars ? String(text.prefix(maxChars)) : text
        var truncated = fullCount > maxChars
        let lines = out.split(separator: "\n", omittingEmptySubsequences: false)
        if lines.count > maxLines {
            out = lines.prefix(maxLines).joined(separator: "\n")
            truncated = true
        }
        return CappedText(text: out, isTruncated: truncated, fullCharCount: fullCount)
    }
}

public enum TraceNumberFormat {
    public static func tokens(_ count: Int64?) -> String {
        guard let count else { return "–" }
        if count >= 1_000_000 { return String(format: "%.1fM", Double(count) / 1_000_000) }
        if count >= 10_000 { return "\(count / 1000)k" }
        if count >= 1_000 { return String(format: "%.1fk", Double(count) / 1000) }
        return "\(count)"
    }

    public static func cost(_ usd: Double) -> String {
        usd >= 0.01 ? String(format: "$%.2f", usd) : String(format: "$%.4f", usd)
    }
}

public enum TranscriptWindow {
    public static let defaultMaxTurns = 200
    public static let defaultMaxChars = 1_500_000

    public static func startIndex(
        turns: [TranscriptTurn], maxTurns: Int, maxChars: Int = defaultMaxChars
    ) -> Int {
        var chars = 0
        var count = 0
        var index = turns.count
        while index > 0, count < maxTurns {
            let turn = turns[index - 1]
            chars += (turn.user?.count ?? 0) + (turn.assistant?.count ?? 0)
                + (turn.error?.count ?? 0) + 64
            if count > 0, chars > maxChars { break }
            index -= 1
            count += 1
        }
        return index
    }
}

public enum TranscriptRender {
    public struct State: Equatable, Sendable {
        public let count: Int
        public let firstId: String?
        public let lastId: String?
        public let lastSignature: String

        public init(count: Int, firstId: String?, lastId: String?, lastSignature: String) {
            self.count = count
            self.firstId = firstId
            self.lastId = lastId
            self.lastSignature = lastSignature
        }
    }

    public enum Plan: Equatable, Sendable {
        case unchanged
        case rebuild
        case append(from: Int)
    }

    public static let maxTurnChars = 100_000

    public static func state(for turns: [TranscriptTurn]) -> State {
        State(
            count: turns.count, firstId: turns.first?.traceId, lastId: turns.last?.traceId,
            lastSignature: turns.last.map(signature) ?? "")
    }

    public static func plan(previous: State?, turns: [TranscriptTurn]) -> Plan {
        guard let previous else { return .rebuild }
        if previous.count == 0 { return turns.isEmpty ? .unchanged : .rebuild }
        if turns.count < previous.count { return .rebuild }
        if turns.first?.traceId != previous.firstId { return .rebuild }
        let overlapLast = turns[previous.count - 1]
        if overlapLast.traceId != previous.lastId
            || signature(overlapLast) != previous.lastSignature {
            return .rebuild
        }
        return turns.count == previous.count ? .unchanged : .append(from: previous.count)
    }

    static func signature(_ turn: TranscriptTurn) -> String {
        "\(turn.tsResponseMs ?? -1)|\(turn.status ?? -1)|\(turn.user?.count ?? -1)"
            + "|\(turn.assistant?.count ?? -1)|\(turn.error?.count ?? -1)"
    }

    public static func document(
        turns: [TranscriptTurn], firstTurnNumber: Int = 1, harnessName: String = "harness"
    ) -> NSAttributedString {
        let formatter = DateFormatter()
        formatter.dateFormat = "HH:mm:ss"
        let labelFont = NSFont.monospacedSystemFont(ofSize: 10, weight: .bold)
        let separatorFont = NSFont.monospacedSystemFont(ofSize: 9, weight: .regular)
        let bodyFont = NSFont.monospacedSystemFont(ofSize: 12, weight: .regular)

        let separatorPara = NSMutableParagraphStyle()
        separatorPara.paragraphSpacing = 4
        let userPara = NSMutableParagraphStyle()
        userPara.firstLineHeadIndent = 48
        userPara.headIndent = 48
        userPara.tailIndent = -8
        userPara.paragraphSpacing = 2
        let assistantPara = NSMutableParagraphStyle()
        assistantPara.firstLineHeadIndent = 8
        assistantPara.headIndent = 8
        assistantPara.tailIndent = -48
        assistantPara.paragraphSpacing = 2

        let separator: [NSAttributedString.Key: Any] = [
            .font: separatorFont, .foregroundColor: NSColor.tertiaryLabelColor,
            .paragraphStyle: separatorPara,
        ]
        let badSeparator: [NSAttributedString.Key: Any] = [
            .font: NSFont.monospacedSystemFont(ofSize: 9, weight: .bold),
            .foregroundColor: NSColor.systemRed,
            .paragraphStyle: separatorPara,
        ]
        let userLabel: [NSAttributedString.Key: Any] = [
            .font: labelFont, .foregroundColor: NSColor.controlAccentColor,
            .paragraphStyle: userPara,
        ]
        let assistantLabel: [NSAttributedString.Key: Any] = [
            .font: labelFont, .foregroundColor: NSColor.secondaryLabelColor,
            .paragraphStyle: assistantPara,
        ]
        let user: [NSAttributedString.Key: Any] = [
            .font: bodyFont, .foregroundColor: NSColor.secondaryLabelColor,
            .paragraphStyle: userPara,
            .backgroundColor: NSColor.controlAccentColor.withAlphaComponent(0.08),
        ]
        let assistant: [NSAttributedString.Key: Any] = [
            .font: bodyFont, .foregroundColor: NSColor.labelColor,
            .paragraphStyle: assistantPara,
            .backgroundColor: NSColor.secondaryLabelColor.withAlphaComponent(0.06),
        ]
        let error: [NSAttributedString.Key: Any] = [
            .font: bodyFont, .foregroundColor: NSColor.systemRed,
            .paragraphStyle: assistantPara,
        ]
        let out = NSMutableAttributedString()
        for (index, turn) in turns.enumerated() {
            let facts = TurnHeader.separatorFacts(
                turnNumber: firstTurnNumber + index,
                time: formatter.string(
                    from: Date(timeIntervalSince1970: Double(turn.tsRequestMs) / 1000)),
                status: turn.status,
                requestMs: turn.tsRequestMs, responseMs: turn.tsResponseMs,
                costUsd: turn.costUsd)
            let isError = (turn.status ?? 0) >= 400
            let sepAttrs = isError ? badSeparator : separator
            out.append(NSAttributedString(string: "· \(facts) · ", attributes: sepAttrs))
            if let link = TraceLink.url(forTraceId: turn.traceId) {
                var linkAttrs = sepAttrs
                linkAttrs[.link] = link
                out.append(NSAttributedString(string: "Details", attributes: linkAttrs))
                out.append(NSAttributedString(string: " ·", attributes: sepAttrs))
            }
            out.append(NSAttributedString(string: "\n", attributes: sepAttrs))
            if let text = turn.user, !text.isEmpty {
                let toolBody = TurnHeader.toolResultBody(text)
                let label = TurnHeader.requestLabel(
                    harness: harnessName, isToolResult: toolBody != nil)
                out.append(NSAttributedString(string: "\(label)\n", attributes: userLabel))
                out.append(NSAttributedString(
                    string: "❯ \(cap(toolBody ?? text))\n", attributes: user))
            }
            if let text = turn.assistant, !text.isEmpty {
                out.append(NSAttributedString(
                    string: "\(TurnHeader.responseLabel(model: turn.model))\n",
                    attributes: assistantLabel))
                out.append(NSAttributedString(string: "\(cap(text))\n", attributes: assistant))
            }
            if let text = turn.error, !text.isEmpty {
                out.append(NSAttributedString(string: "\(cap(text))\n", attributes: error))
            }
            out.append(NSAttributedString(string: "\n", attributes: separator))
        }
        return NSAttributedString(attributedString: out)
    }

    static func cap(_ text: String, maxChars: Int = maxTurnChars) -> String {
        guard text.count > maxChars else { return text }
        return text.prefix(maxChars) + "\n… (+\(text.count - maxChars) chars truncated)"
    }

    static func tokens(_ count: Int64?) -> String { TraceNumberFormat.tokens(count) }

    static func cost(_ usd: Double) -> String { TraceNumberFormat.cost(usd) }
}

public struct DarioAdminStatus: Codable, Sendable {
    public let activeGenerationId: String?
    public let generations: [DarioGenerationDetail]

    enum CodingKeys: String, CodingKey {
        case generations
        case activeGenerationId = "active_generation_id"
    }
}

public struct DarioGenerationDetail: Codable, Sendable, Identifiable {
    public let id: String
    public let version: String
    public let port: Int?
    public let pid: Int?
    public let state: String?
    public let phase: String
    public let inFlight: Int?
    public let consecutiveFailures: Int?
    public let lastProbe: DarioProbeDetail?
    public let startedAt: Int64?
    public let promotedAt: Int64?
    public let stdoutLog: String?
    public let stderrLog: String?

    enum CodingKeys: String, CodingKey {
        case id, version, port, pid, state, phase
        case inFlight = "in_flight"
        case consecutiveFailures = "consecutive_failures"
        case lastProbe = "last_probe"
        case startedAt = "started_at"
        case promotedAt = "promoted_at"
        case stdoutLog = "stdout_log"
        case stderrLog = "stderr_log"
    }
}

public struct DarioProbeDetail: Codable, Sendable {
    public let ok: Bool
    public let status: Int?
    public let latencyMs: Int64?
    public let error: String?
    public let atMs: Int64?

    enum CodingKeys: String, CodingKey {
        case ok, status, error
        case latencyMs = "latency_ms"
        case atMs = "at_ms"
    }
}

public struct DarioLogsResponse: Codable, Sendable {
    public let generationId: String
    public let stdout: String
    public let stderr: String
    public let lines: Int?

    enum CodingKeys: String, CodingKey {
        case stdout, stderr, lines
        case generationId = "generation_id"
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
