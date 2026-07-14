import Foundation
import CoreFoundation

#if canImport(AppKit)
import AppKit
#endif

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
    public let providers: [String]?
    public let harness: String?
    public let totalInputTokens: Int64?
    public let totalOutputTokens: Int64?
    public let totalCostUsd: Double?
    public let errors: Int64?
    public let lastStatus: Int?
    public let tags: [String: String]?
    public let efforts: [String]?
    public let accountIds: [String]?

    public var id: String { sessionId }

    public var isPingOrTest: Bool {
        SessionKind.isPingOrTest(sessionId: sessionId, harness: harness, tags: tags)
    }

    enum CodingKeys: String, CodingKey {
        case models, providers, harness, errors, tags, efforts
        case sessionId = "session_id"
        case runId = "run_id"
        case firstTsMs = "first_ts_ms"
        case lastTsMs = "last_ts_ms"
        case traceCount = "trace_count"
        case totalInputTokens = "total_input_tokens"
        case totalOutputTokens = "total_output_tokens"
        case totalCostUsd = "total_cost_usd"
        case lastStatus = "last_status"
        case accountIds = "account_ids"
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

public struct TranscriptTurn: Codable, Sendable, Identifiable, Equatable {
    public let traceId: String
    public let tsRequestMs: Int64
    public let tsResponseMs: Int64?
    public let model: String?
    public let provider: String?
    public let status: Int?
    public let inputTokens: Int64?
    public let outputTokens: Int64?
    public let reasoningEffort: String?
    public let thinkingBudget: Int64?
    public let costUsd: Double?
    public let accountId: String?
    public let error: String?
    public let user: String?
    public let assistant: String?
    public let toolCalls: [ToolCall]?
    public let assistantBlocks: [AssistantBlock]?

    public var id: String { traceId }

    enum CodingKeys: String, CodingKey {
        case model, provider, status, error, user, assistant
        case traceId = "trace_id"
        case tsRequestMs = "ts_request_ms"
        case tsResponseMs = "ts_response_ms"
        case inputTokens = "input_tokens"
        case outputTokens = "output_tokens"
        case reasoningEffort = "reasoning_effort"
        case thinkingBudget = "thinking_budget"
        case costUsd = "cost_usd"
        case accountId = "account_id"
        case toolCalls = "tool_calls"
        case assistantBlocks = "assistant_blocks"
    }
}

public struct AssistantBlock: Codable, Sendable, Equatable {
    public let type: String
    public let text: String?
    public let name: String?
    public let arguments: String?

    public init(type: String, text: String? = nil, name: String? = nil, arguments: String? = nil) {
        self.type = type
        self.text = text
        self.name = name
        self.arguments = arguments
    }
}

public struct ToolCall: Codable, Sendable, Equatable {
    public let name: String
    public let arguments: String?

    public init(name: String, arguments: String?) {
        self.name = name
        self.arguments = arguments
    }

    public var argumentSummary: String { Self.summary(arguments ?? "") }

    public var command: String? {
        guard let arguments, let data = arguments.data(using: .utf8),
            let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        else { return nil }
        return obj["command"] as? String
    }

    public static func summary(_ arguments: String) -> String {
        let trimmed = arguments.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return "" }
        guard let data = trimmed.data(using: .utf8),
            let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        else { return trimmed }
        if let command = obj["command"] as? String { return command }
        if let pretty = try? JSONSerialization.data(
            withJSONObject: obj,
            options: [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes]),
            let text = String(data: pretty, encoding: .utf8) {
            return text
        }
        return trimmed
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
    public let reasoningEffort: String?
    public let thinkingBudget: Int64?
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
        case reasoningEffort = "reasoning_effort"
        case thinkingBudget = "thinking_budget"
        case costUsd = "cost_usd"
        case reqHeadersJson = "req_headers_json"
        case respHeadersJson = "resp_headers_json"
        case reqBodyPath = "req_body_path"
        case upstreamReqBodyPath = "upstream_req_body_path"
        case respBodyPath = "resp_body_path"
        case tagsJson = "tags_json"
    }
}

public struct DarioCaptureExtras: Codable, Sendable {
    public let requestPath: String?
    public let responsePath: String?
    public let requestAvailable: Bool
    public let responseAvailable: Bool
    public let promptCache: DarioPromptCacheUse?

    public init(
        requestPath: String?,
        responsePath: String?,
        requestAvailable: Bool,
        responseAvailable: Bool,
        promptCache: DarioPromptCacheUse? = nil
    ) {
        self.requestPath = requestPath
        self.responsePath = responsePath
        self.requestAvailable = requestAvailable
        self.responseAvailable = responseAvailable
        self.promptCache = promptCache
    }

    enum CodingKeys: String, CodingKey {
        case requestPath = "request_path"
        case responsePath = "response_path"
        case requestAvailable = "request_available"
        case responseAvailable = "response_available"
        case promptCache = "prompt_cache"
    }
}

public struct DarioPromptCacheUse: Codable, Sendable, Equatable {
    public let key: String?
    public let model: String?
    public let status: String?
    public let applied: Bool?
    public let path: String?
    public let capturedAt: String?
    public let lastUsedAt: String?
    public let systemPromptChars: Int?
    public let agentIdentityChars: Int?
    public let claudeVersion: String?
    public let error: String?

    enum CodingKeys: String, CodingKey {
        case key, model, status, applied, path, error
        case capturedAt = "captured_at"
        case lastUsedAt = "last_used_at"
        case systemPromptChars = "system_prompt_chars"
        case agentIdentityChars = "agent_identity_chars"
        case claudeVersion = "claude_version"
    }
}

public struct TraceExtras: Codable, Sendable {
    public let reasoningEffort: String?
    public let thinkingBudget: Int64?
    public let maxTokens: Int64?
    public let temperature: Double?
    public let messageCount: Int?
    public let systemChars: Int?
    public let systemPrompt: String?
    public let darioCapture: DarioCaptureExtras?

    public var hasAny: Bool {
        reasoningEffort != nil || thinkingBudget != nil || maxTokens != nil
            || temperature != nil || messageCount != nil || systemChars != nil
            || darioCapture != nil
    }

    enum CodingKeys: String, CodingKey {
        case temperature
        case reasoningEffort = "reasoning_effort"
        case thinkingBudget = "thinking_budget"
        case maxTokens = "max_tokens"
        case messageCount = "message_count"
        case systemChars = "system_chars"
        case systemPrompt = "system_prompt"
        case darioCapture = "dario_capture"
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

    public static func effort(reasoningEffort: String?, thinkingBudget: Int64?) -> String {
        if let reasoningEffort, !reasoningEffort.isEmpty { return reasoningEffort }
        if let thinkingBudget { return "\(TraceNumberFormat.tokens(thinkingBudget)) think" }
        return "-"
    }

    public static func separatorFacts(
        turnNumber: Int, time: String, status: Int?,
        requestMs: Int64, responseMs: Int64?, costUsd: Double? = nil,
        reasoningEffort: String? = nil, thinkingBudget: Int64? = nil, accountId: String? = nil
    ) -> String {
        var parts = ["turn \(turnNumber)", time]
        if let status { parts.append("\(status)") }
        let effortLabel = effort(reasoningEffort: reasoningEffort, thinkingBudget: thinkingBudget)
        if effortLabel != "-" { parts.append(effortLabel) }
        if let dur = duration(requestMs: requestMs, responseMs: responseMs) {
            parts.append(dur)
        }
        if let costUsd, costUsd > 0 { parts.append(TraceNumberFormat.cost(costUsd)) }
        if let accountId, !accountId.isEmpty { parts.append("acct \(accountId)") }
        return parts.joined(separator: " · ")
    }

    public static func requestLabel(harness: String, isToolResult: Bool = false) -> String {
        "⬆ \(harness) · \(isToolResult ? "tool result" : "user")"
    }

    public static func responseLabel(model: String?) -> String {
        model.map { "⬇ \($0) · model" } ?? "⬇ model"
    }

    public static func toolResultBody(_ text: String) -> String? {
        guard text.hasPrefix(toolResultPrefix) else { return nil }
        return String(text.dropFirst(toolResultPrefix.count))
            .trimmingCharacters(in: .whitespacesAndNewlines)
    }
}

#if canImport(AppKit)
public struct TranscriptIcons: @unchecked Sendable {
    public let harness: NSImage?
    public let providers: [String: NSImage]

    public init(harness: NSImage? = nil, providers: [String: NSImage] = [:]) {
        self.harness = harness
        self.providers = providers
    }

    public static let none = TranscriptIcons()
}
#endif

public enum BodyPretty {
    public static let displayCap = 200_000

    public static func display(_ raw: String, cap: Int = displayCap) -> CappedText {
        var text = raw
        if isJSON(raw),
            let data = raw.data(using: .utf8),
            let obj = try? JSONSerialization.jsonObject(with: data),
            let pretty = try? JSONSerialization.data(
                withJSONObject: obj,
                options: [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes]),
            let str = String(data: pretty, encoding: .utf8) {
            text = str
        }
        return capped(text, cap: cap)
    }

    public static func capped(_ text: String, cap: Int = displayCap) -> CappedText {
        let full = text.count
        guard full > cap else {
            return CappedText(text: text, isTruncated: false, fullCharCount: full)
        }
        let out = String(text.prefix(cap)) + "\n… (+\(full - cap) chars truncated)"
        return CappedText(text: out, isTruncated: true, fullCharCount: full)
    }

    public static func isJSON(_ text: String) -> Bool {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard trimmed.hasPrefix("{") || trimmed.hasPrefix("[") else { return false }
        guard let data = trimmed.data(using: .utf8) else { return false }
        return (try? JSONSerialization.jsonObject(with: data)) != nil
    }
}

public enum JsonHighlight {
#if canImport(AppKit)
    public struct Colors: @unchecked Sendable {
        public let key: NSColor
        public let string: NSColor
        public let number: NSColor
        public let keyword: NSColor
        public let punctuation: NSColor

        public init(
            key: NSColor, string: NSColor, number: NSColor,
            keyword: NSColor, punctuation: NSColor
        ) {
            self.key = key
            self.string = string
            self.number = number
            self.keyword = keyword
            self.punctuation = punctuation
        }

        public static let standard = Colors(
            key: .systemBlue, string: .systemOrange, number: .systemPurple,
            keyword: .systemTeal, punctuation: .secondaryLabelColor)
    }
#endif

    public enum Kind: Equatable, Sendable {
        case key, string, number, keyword
    }

#if canImport(AppKit)
    public static func attributed(
        _ text: String, font: NSFont, colors: Colors = .standard,
        cap: Int = BodyPretty.displayCap
    ) -> NSAttributedString {
        let input = text.count > cap ? String(text.prefix(cap)) : text
        let out = NSMutableAttributedString(
            string: input,
            attributes: [.font: font, .foregroundColor: colors.punctuation])
        for (range, kind) in spans(input) {
            let color: NSColor = switch kind {
            case .key: colors.key
            case .string: colors.string
            case .number: colors.number
            case .keyword: colors.keyword
            }
            out.addAttribute(
                .foregroundColor, value: color,
                range: NSRange(location: range.lowerBound, length: range.count))
        }
        return out
    }
#endif

    public static func spans(_ text: String) -> [(range: Range<Int>, kind: Kind)] {
        let units = Array(text.utf16)
        var spans: [(Range<Int>, Kind)] = []
        var i = 0
        let quote: UInt16 = 34
        let backslash: UInt16 = 92
        func isWhitespace(_ u: UInt16) -> Bool { u == 32 || u == 10 || u == 9 || u == 13 }
        func matches(_ word: [UInt16], at index: Int) -> Bool {
            guard index + word.count <= units.count else { return false }
            for (offset, u) in word.enumerated() where units[index + offset] != u {
                return false
            }
            return true
        }
        let trueWord = Array("true".utf16)
        let falseWord = Array("false".utf16)
        let nullWord = Array("null".utf16)
        while i < units.count {
            let u = units[i]
            if u == quote {
                let start = i
                i += 1
                while i < units.count {
                    if units[i] == backslash {
                        i += 2
                        continue
                    }
                    if units[i] == quote {
                        i += 1
                        break
                    }
                    i += 1
                }
                var j = i
                while j < units.count, isWhitespace(units[j]) { j += 1 }
                let isKey = j < units.count && units[j] == 58
                spans.append((start..<min(i, units.count), isKey ? .key : .string))
            } else if (u >= 48 && u <= 57) || u == 45 {
                let start = i
                i += 1
                while i < units.count {
                    let n = units[i]
                    let isNumberUnit = (n >= 48 && n <= 57) || n == 46 || n == 101
                        || n == 69 || n == 43 || n == 45
                    guard isNumberUnit else { break }
                    i += 1
                }
                spans.append((start..<i, .number))
            } else if matches(trueWord, at: i) {
                spans.append((i..<i + 4, .keyword))
                i += 4
            } else if matches(falseWord, at: i) {
                spans.append((i..<i + 5, .keyword))
                i += 5
            } else if matches(nullWord, at: i) {
                spans.append((i..<i + 4, .keyword))
                i += 4
            } else {
                i += 1
            }
        }
        return spans
    }
}

public enum NiceBlock: Equatable, Sendable {
    case row(key: String, value: String)
    case block(key: String, text: String)
    case text(String)
}

public enum JsonNice {
    public static let longStringThreshold = 120

    public static func blocks(_ text: String) -> [NiceBlock] {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard trimmed.hasPrefix("{"),
            let data = trimmed.data(using: .utf8),
            let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
            !obj.isEmpty
        else { return [.text(text)] }
        var rows: [NiceBlock] = []
        var blocks: [NiceBlock] = []
        for (key, value) in obj.sorted(by: { $0.key < $1.key }) {
            if let str = value as? String {
                if str.count > longStringThreshold || str.contains("\n") {
                    blocks.append(.block(key: key, text: str))
                } else {
                    rows.append(.row(key: key, value: str))
                }
                continue
            }
            rows.append(.row(key: key, value: scalarText(value)))
        }
        return rows + blocks
    }

    static func scalarText(_ value: Any) -> String {
        if value is NSNull { return "null" }
        if let number = value as? NSNumber {
            if CFGetTypeID(number) == CFBooleanGetTypeID() {
                return number.boolValue ? "true" : "false"
            }
            return number.stringValue
        }
        if JSONSerialization.isValidJSONObject(value),
            let data = try? JSONSerialization.data(
                withJSONObject: value, options: [.sortedKeys, .withoutEscapingSlashes]),
            let text = String(data: data, encoding: .utf8) {
            return text
        }
        return "\(value)"
    }
}

public struct TraceSearchResponse: Codable, Sendable {
    public let traces: [TraceSearchRow]
    public let scanned: Int?
}

public struct TraceSearchRow: Codable, Sendable {
    public let id: String
    public let sessionId: String?
    public let reasoningEffort: String?
    public let thinkingBudget: Int64?

    enum CodingKeys: String, CodingKey {
        case id
        case sessionId = "session_id"
        case reasoningEffort = "reasoning_effort"
        case thinkingBudget = "thinking_budget"
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
    public let firstTsMs: Int64
    public let lastTsMs: Int64
    public let lastTs: Date
    public let sessionShort: String
    public let models: String
    public let providers: [String]
    public let harness: String
    public let harnessRaw: String?
    public let accountIds: [String]
    public let accounts: String
    public let tags: [String: String]?
    public let turns: Int
    public let tokensIn: Int64
    public let tokensOut: Int64
    public let cost: Double
    public let errors: Int
    public let runId: String
    public let durationMs: Int64
    public let duration: String
    public let tagsSummary: String
    public let kindBadge: String?
    public let iconAsset: String?

    public var isPingOrTest: Bool { kindBadge != nil }

    public init(session: TraceSession) {
        id = session.sessionId
        firstTsMs = session.firstTsMs
        lastTsMs = session.lastTsMs
        lastTs = Date(timeIntervalSince1970: Double(session.lastTsMs) / 1000)
        sessionShort = Self.shortId(session.sessionId)
        let modelsList = session.models ?? []
        models = modelsList.joined(separator: ", ")
        if let capturedProviders = session.providers, !capturedProviders.isEmpty {
            providers = capturedProviders
        } else {
            providers = ModelProvider.providers(in: modelsList)
        }
        harnessRaw = session.harness
        let taggedHarness = session.tags?["harness"]?.trimmingCharacters(in: .whitespaces)
        harness = session.harness != nil || taggedHarness?.isEmpty == false
            ? HarnessName.display(harness: session.harness, tags: session.tags)
            : ""
        tags = session.tags
        accountIds = session.accountIds ?? []
        accounts = accountIds.joined(separator: ", ")
        turns = session.traceCount
        tokensIn = session.totalInputTokens ?? 0
        tokensOut = session.totalOutputTokens ?? 0
        cost = session.totalCostUsd ?? 0
        errors = Int(session.errors ?? 0)
        runId = session.runId ?? ""
        durationMs = max(0, session.lastTsMs - session.firstTsMs)
        duration = SessionDuration.format(ms: durationMs)
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

public enum SessionDuration {
    public static func format(ms: Int64) -> String {
        let seconds = max(0, ms / 1000)
        if seconds >= 3600 { return String(format: "%dh %02dm", seconds / 3600, (seconds % 3600) / 60) }
        if seconds >= 60 { return "\(seconds / 60)m \(seconds % 60)s" }
        return "\(seconds)s"
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
        pinned = false
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
        if id == lastFollowId || id == selectedId {
            lastFollowId = nil
            return .none
        }
        return userSelect(id)
    }

    @discardableResult
    public mutating func setLive(_ live: Bool, newestVisibleId: String?) -> Change {
        pinned = false
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
        "agent": "cursor-cli",
        "cursor-agent": "cursor-cli",
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
        if m.hasPrefix("cursor") || m.hasPrefix("composer") { return "cursor" }
        if m.hasPrefix("amp") { return "amp" }
        return nil
    }

    public static func initial(for provider: String) -> String {
        switch provider.lowercased() {
        case "anthropic": "A"
        case "openai": "O"
        case "xai": "X"
        case "gemini": "G"
        case "cursor": "C"
        case "amp": "A"
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
    public var effort: String?
    public var duration: String?
    public var account: String?

    public init() {}

    public var isEmpty: Bool {
        freeText.isEmpty && !hasTokenFilters
    }

    public var hasTokenFilters: Bool {
        model != nil || provider != nil || harness != nil
            || status != nil || run != nil || session != nil
            || task != nil || job != nil || tag != nil
            || effort != nil || duration != nil || account != nil
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
                    case "effort": query.effort = value; continue
                    case "duration": query.duration = value; continue
                    case "account": query.account = value; continue
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
        if let value, let clean = Self.tokenValue(value) { words.append(prefix + clean) }
        return words.joined(separator: " ")
    }

    static func tokenValue(_ value: String) -> String? {
        var clean = value.trimmingCharacters(in: .whitespacesAndNewlines)
        if let cut = clean.firstIndex(where: { $0.isWhitespace || $0 == "," }) {
            clean = String(clean[..<cut])
        }
        clean = clean.trimmingCharacters(in: CharacterSet(charactersIn: ","))
        return clean.isEmpty ? nil : clean
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
        if let effort {
            guard (session.efforts ?? []).contains(effort) else { return false }
        }
        if let duration {
            guard let minimum = SessionDurationFilter(rawValue: duration)?.minimumMs else {
                return false
            }
            guard session.lastTsMs - session.firstTsMs >= minimum else { return false }
        }
        if let account {
            guard (session.accountIds ?? []).contains(where: {
                $0.caseInsensitiveCompare(account) == .orderedSame
            }) else { return false }
        }
        return true
    }

    public func matches(_ turn: TranscriptTurn) -> Bool {
        if let effort, turn.reasoningEffort != effort { return false }
        if let account, turn.accountId?.caseInsensitiveCompare(account) != .orderedSame {
            return false
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

public enum SessionDurationFilter: String, CaseIterable, Sendable {
    case oneMinute = "1m"
    case fiveMinutes = "5m"
    case fifteenMinutes = "15m"
    case oneHour = "1h"

    public var minimumMs: Int64 {
        switch self {
        case .oneMinute: 60_000
        case .fiveMinutes: 5 * 60_000
        case .fifteenMinutes: 15 * 60_000
        case .oneHour: 60 * 60_000
        }
    }

    public var label: String { ">=\(rawValue)" }
}

public enum TagFilterDimension: String, CaseIterable, Sendable {
    case harness, task, job, model, account, effort, duration

    public var title: String {
        self == .account ? "Billing Account" : rawValue.capitalized
    }

    public func label(for value: String) -> String {
        if self == .duration, let filter = SessionDurationFilter(rawValue: value) {
            return filter.label
        }
        return value
    }

    public func values(in sessions: [TraceSession]) -> [String] {
        if self == .duration {
            return SessionDurationFilter.allCases.map(\.rawValue)
        }
        var seen = Set<String>()
        var out: [String] = []
        func add(_ value: String?) {
            guard let value, !value.isEmpty, seen.insert(value).inserted else { return }
            out.append(value)
        }
        func addSplittingList(_ value: String?) {
            guard let value else { return }
            for part in value.split(separator: ",") {
                add(part.trimmingCharacters(in: .whitespaces))
            }
        }
        for session in sessions {
            let tags = session.tags ?? [:]
            switch self {
            case .harness:
                let tagged = tags["harness"]?.trimmingCharacters(in: .whitespaces)
                if session.harness != nil || tagged?.isEmpty == false {
                    add(HarnessName.display(harness: session.harness, tags: session.tags))
                }
            case .model:
                (session.models ?? []).forEach { addSplittingList($0) }
                addSplittingList(tags["model"])
            case .task:
                add(tags["task"])
            case .job:
                add(tags["job"])
            case .effort:
                (session.efforts ?? []).forEach { add($0) }
            case .account:
                (session.accountIds ?? []).forEach { add($0) }
            case .duration:
                break
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
        case .effort: query.effort
        case .duration: query.duration
        case .account: query.account
        }
    }
}

public enum AccountIdentity {
    public static func name(accountId: String, accounts: [Account]) -> String {
        guard let account = accounts.first(where: { $0.id == accountId }) else {
            return accountId
        }
        let identity = [account.email, account.description, account.label, account.name]
            .compactMap { value -> String? in
                guard let value = value?.trimmingCharacters(in: .whitespacesAndNewlines),
                    !value.isEmpty
                else { return nil }
                return value
            }
            .first
        return identity ?? accountId
    }

    public static func label(accountId: String, accounts: [Account]) -> String {
        let identity = name(accountId: accountId, accounts: accounts)
        return identity == accountId ? accountId : "\(identity) · \(accountId)"
    }

    public static func summary(accountIds: [String], accounts: [Account]) -> String? {
        let unique = accountIds.reduce(into: [String]()) { result, accountId in
            guard !accountId.isEmpty, !result.contains(accountId) else { return }
            result.append(accountId)
        }
        guard !unique.isEmpty else { return nil }
        return unique.map { label(accountId: $0, accounts: accounts) }
            .joined(separator: ", ")
    }

    public static func nameSummary(accountIds: [String], accounts: [Account]) -> String? {
        let unique = accountIds.reduce(into: [String]()) { result, accountId in
            guard !accountId.isEmpty, !result.contains(accountId) else { return }
            result.append(accountId)
        }
        guard !unique.isEmpty else { return nil }
        return unique.map { name(accountId: $0, accounts: accounts) }
            .joined(separator: ", ")
    }
}

public enum TraceFingerprint {
    public static func sessions(_ sessions: [TraceSession]) -> String {
        // The browser only requests recent sessions, so include every sidebar
        // field here. Otherwise a still-open session can gain tokens, cost, or
        // errors without changing its timestamp and leave the sidebar stale.
        var hasher = Hasher()
        hasher.combine(sessions.count)
        for session in sessions.sorted(by: { $0.sessionId < $1.sessionId }) {
            hasher.combine(session.sessionId)
            hasher.combine(session.runId)
            hasher.combine(session.firstTsMs)
            hasher.combine(session.lastTsMs)
            hasher.combine(session.traceCount)
            hasher.combine(session.models)
            hasher.combine(session.providers)
            hasher.combine(session.harness)
            hasher.combine(session.totalInputTokens)
            hasher.combine(session.totalOutputTokens)
            hasher.combine(session.totalCostUsd)
            hasher.combine(session.errors)
            hasher.combine(session.lastStatus)
            hasher.combine(session.efforts)
            hasher.combine(session.accountIds)
            for (key, value) in (session.tags ?? [:]).sorted(by: { $0.key < $1.key }) {
                hasher.combine(key)
                hasher.combine(value)
            }
        }
        return "\(sessions.count)|\(hasher.finalize())"
    }

    public static func turns(_ turns: [TranscriptTurn]) -> String {
        var hasher = Hasher()
        hasher.combine(turns.count)
        for turn in turns {
            hasher.combine(turn.traceId)
            hasher.combine(turn.tsRequestMs)
            hasher.combine(turn.tsResponseMs)
            hasher.combine(turn.model)
            hasher.combine(turn.provider)
            hasher.combine(turn.status)
            hasher.combine(turn.inputTokens)
            hasher.combine(turn.outputTokens)
            hasher.combine(turn.reasoningEffort)
            hasher.combine(turn.thinkingBudget)
            hasher.combine(turn.costUsd)
            hasher.combine(turn.error)
            hasher.combine(turn.user)
            hasher.combine(turn.assistant)
            hasher.combine(turn.toolCalls?.count)
            for call in turn.toolCalls ?? [] {
                hasher.combine(call.name)
                hasher.combine(call.arguments)
            }
            hasher.combine(turn.assistantBlocks?.count)
            for block in turn.assistantBlocks ?? [] {
                hasher.combine(block.type)
                hasher.combine(block.text)
                hasher.combine(block.name)
                hasher.combine(block.arguments)
            }
        }
        return "\(turns.count)|\(hasher.finalize())"
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
            let userChars: Int = turn.user?.count ?? 0
            let assistantChars: Int = turn.assistant?.count ?? 0
            let errorChars: Int = turn.error?.count ?? 0
            chars += userChars + assistantChars + errorChars + 64
            if count > 0, chars > maxChars { break }
            index -= 1
            count += 1
        }
        return index
    }
}

public struct TurnRange: Equatable, Sendable {
    public let traceId: String
    public let range: NSRange

    public init(traceId: String, range: NSRange) {
        self.traceId = traceId
        self.range = range
    }
}

public enum TranscriptBubbleKind: String, Sendable {
    case user, model, tool, toolResult, error
}

extension NSAttributedString.Key {
    public static let transcriptBubbleKind = NSAttributedString.Key("alexandriaBubbleKind")
    public static let transcriptBubbleGroup = NSAttributedString.Key("alexandriaBubbleGroup")
    public static let transcriptTurnId = NSAttributedString.Key("alexandriaTurnId")
}

public enum TurnHitTest {
    public static func traceId(at index: Int, in ranges: [TurnRange]) -> String? {
        ranges.first { NSLocationInRange(index, $0.range) }?.traceId
    }
}

public enum TraceInspectorSelection {
    public static func target(currentTraceId: String?, in traceIds: [String]) -> String? {
        guard !traceIds.isEmpty else { return nil }
        if let currentTraceId, traceIds.contains(currentTraceId) {
            return currentTraceId
        }
        return traceIds.last
    }
}

public struct TraceBodyCache {
    public let capacity: Int
    private var store: [String: TraceBodyContent] = [:]
    private var order: [String] = []

    public init(capacity: Int = 20) {
        self.capacity = max(1, capacity)
    }

    public var count: Int { store.count }

    public static func key(id: String, kind: TraceBodyKind) -> String {
        "\(id)|\(kind.rawValue)"
    }

    public mutating func value(for key: String) -> TraceBodyContent? {
        guard let value = store[key] else { return nil }
        touch(key)
        return value
    }

    public mutating func insert(_ value: TraceBodyContent, for key: String) {
        store[key] = value
        touch(key)
        while store.count > capacity, let oldest = order.first {
            order.removeFirst()
            store.removeValue(forKey: oldest)
        }
    }

    private mutating func touch(_ key: String) {
        order.removeAll { $0 == key }
        order.append(key)
    }
}

public enum TurnExport {
    public static func overviewLines(_ trace: TraceDetail) -> [String] {
        var lines: [String] = []
        func add(_ label: String, _ value: String?) {
            guard let value, !value.isEmpty else { return }
            lines.append("\(label): \(value)")
        }
        add("status", trace.status.map { "\($0)" })
        let endpoint = [trace.method, trace.path].compactMap(\.self).joined(separator: " ")
        add("endpoint", endpoint.isEmpty ? nil : endpoint)
        if let requestMs = trace.tsRequestMs {
            let formatter = ISO8601DateFormatter()
            add("time", formatter.string(
                from: Date(timeIntervalSince1970: Double(requestMs) / 1000)))
            add("duration", TurnHeader.duration(
                requestMs: requestMs, responseMs: trace.tsResponseMs)
                ?? trace.latencyMs.map { "\($0)ms" })
        }
        switch (trace.requestedModel, trace.routedModel) {
        case let (.some(requested), .some(routed)) where requested != routed:
            add("model", "\(requested) → \(routed)")
        case let (requested, routed):
            add("model", requested ?? routed)
        }
        add("provider", trace.upstreamProvider)
        if trace.clientFormat != nil || trace.upstreamFormat != nil {
            let client = trace.clientFormat ?? "?"
            let upstream = trace.upstreamFormat ?? "?"
            let translated = trace.clientFormat != nil && trace.upstreamFormat != nil
                && trace.clientFormat != trace.upstreamFormat
            add("format", "\(client) → \(upstream)\(translated ? " (translated)" : "")")
        }
        add("billing", trace.billingBucket)
        add("account", trace.accountId)
        add("session", trace.sessionId)
        add("run", trace.runId)
        add("client ip", trace.clientIp)
        add("key fingerprint", trace.keyFingerprint)
        if trace.inputTokens != nil || trace.outputTokens != nil {
            var parts = ["in \(TraceNumberFormat.tokens(trace.inputTokens))"]
            if let cached = trace.cachedInputTokens, cached > 0 {
                parts.append("cached \(TraceNumberFormat.tokens(cached))")
            }
            parts.append("out \(TraceNumberFormat.tokens(trace.outputTokens))")
            if let reasoning = trace.reasoningTokens, reasoning > 0 {
                parts.append("reasoning \(TraceNumberFormat.tokens(reasoning))")
            }
            add("tokens", parts.joined(separator: " · "))
        }
        if let cost = trace.costUsd, cost > 0 { add("cost", TraceNumberFormat.cost(cost)) }
        add("error", trace.error)
        return lines
    }

    public static func extrasLines(_ extras: TraceExtras?) -> [String] {
        guard let extras else { return [] }
        var lines: [String] = []
        func add(_ label: String, _ value: String?) {
            guard let value, !value.isEmpty else { return }
            lines.append("\(label): \(value)")
        }
        add("reasoning effort", extras.reasoningEffort)
        add("thinking budget", extras.thinkingBudget.map { "\($0)" })
        add("max tokens", extras.maxTokens.map { "\($0)" })
        add("temperature", extras.temperature.map { "\($0)" })
        add("messages", extras.messageCount.map { "\($0)" })
        add("system chars", extras.systemChars.map { "\($0)" })
        if let capture = extras.darioCapture {
            let states = [
                capture.requestAvailable ? "request" : nil,
                capture.responseAvailable ? "response" : nil,
            ].compactMap(\.self)
            add("Dario capture", states.isEmpty ? nil : states.joined(separator: ", "))
            if let prompt = capture.promptCache {
                add("Dario prompt cache", [prompt.model, prompt.status]
                    .compactMap { $0 }
                    .joined(separator: " · "))
            }
        }
        return lines
    }

    public static func headerLines(_ pairs: [HeaderPair]) -> [String] {
        pairs.map { "\($0.name): \($0.value)" }
    }

    public static func markdown(
        detail: TraceDetail, extras: TraceExtras?,
        reqHeaders: [HeaderPair], respHeaders: [HeaderPair],
        reqBody: String?, respBody: String?
    ) -> String {
        var out = "# Trace \(detail.id)\n\n## Overview\n"
        out += overviewLines(detail).map { "- \($0)" }.joined(separator: "\n")
        let extrasLines = extrasLines(extras)
        if !extrasLines.isEmpty {
            out += "\n\n## Extras\n"
            out += extrasLines.map { "- \($0)" }.joined(separator: "\n")
        }
        out += "\n\n## Request headers\n"
        out += fencedOrMissing(
            reqHeaders.isEmpty ? nil : headerLines(reqHeaders).joined(separator: "\n"),
            language: "")
        out += "\n\n## Response headers\n"
        out += fencedOrMissing(
            respHeaders.isEmpty ? nil : headerLines(respHeaders).joined(separator: "\n"),
            language: "")
        out += "\n\n## Request body\n"
        out += bodySection(reqBody)
        out += "\n\n## Response body\n"
        out += bodySection(respBody)
        out += "\n"
        return out
    }

    static func bodySection(_ raw: String?) -> String {
        guard let raw, !raw.isEmpty else { return "_not available_" }
        let display = BodyPretty.display(raw)
        return fencedOrMissing(
            display.text, language: BodyPretty.isJSON(raw) ? "json" : "")
    }

    static func fencedOrMissing(_ content: String?, language: String) -> String {
        guard let content, !content.isEmpty else { return "_not available_" }
        return "```\(language)\n\(content)\n```"
    }
}

#if canImport(AppKit)
public struct TranscriptDocument: @unchecked Sendable {
    public let text: NSAttributedString
    public let turnRanges: [TurnRange]

    public init(text: NSAttributedString, turnRanges: [TurnRange]) {
        self.text = text
        self.turnRanges = turnRanges
    }
}
#endif

public enum TranscriptRender {
    public struct State: Equatable, Sendable {
        public let count: Int
        public let firstId: String?
        public let lastId: String?
        public let lastSignature: String
        public let rawMode: Bool

        public init(
            count: Int, firstId: String?, lastId: String?, lastSignature: String,
            rawMode: Bool = false
        ) {
            self.count = count
            self.firstId = firstId
            self.lastId = lastId
            self.lastSignature = lastSignature
            self.rawMode = rawMode
        }
    }

    public enum Plan: Equatable, Sendable {
        case unchanged
        case rebuild
        case append(from: Int)
    }

    public static let maxTurnChars = 100_000

    public static func state(for turns: [TranscriptTurn], rawMode: Bool = false) -> State {
        State(
            count: turns.count, firstId: turns.first?.traceId, lastId: turns.last?.traceId,
            lastSignature: turns.last.map(signature) ?? "", rawMode: rawMode)
    }

    public static func plan(
        previous: State?, turns: [TranscriptTurn], rawMode: Bool = false
    ) -> Plan {
        guard let previous else { return .rebuild }
        if previous.rawMode != rawMode { return .rebuild }
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

    public static func shifted(_ ranges: [TurnRange], by offset: Int) -> [TurnRange] {
        ranges.map {
            TurnRange(
                traceId: $0.traceId,
                range: NSRange(location: $0.range.location + offset, length: $0.range.length))
        }
    }

    static func signature(_ turn: TranscriptTurn) -> String {
        var hasher = Hasher()
        hasher.combine(turn.tsResponseMs)
        hasher.combine(turn.status)
        hasher.combine(turn.user)
        hasher.combine(turn.assistant)
        hasher.combine(turn.error)
        hasher.combine(turn.reasoningEffort)
        hasher.combine(turn.thinkingBudget)
        for call in turn.toolCalls ?? [] {
            hasher.combine(call.name)
            hasher.combine(call.arguments)
        }
        for block in turn.assistantBlocks ?? [] {
            hasher.combine(block.type)
            hasher.combine(block.text)
            hasher.combine(block.name)
            hasher.combine(block.arguments)
        }
        return String(hasher.finalize())
    }

    #if canImport(AppKit)
    public static func document(
        turns: [TranscriptTurn], firstTurnNumber: Int = 1, harnessName: String = "harness",
        icons: TranscriptIcons = .none, rawMode: Bool = false,
        billingAccountIds: Set<String>? = nil
    ) -> NSAttributedString {
        build(
            turns: turns, firstTurnNumber: firstTurnNumber, harnessName: harnessName,
            icons: icons, rawMode: rawMode, billingAccountIds: billingAccountIds
        ).text
    }

    public static func build(
        turns: [TranscriptTurn], firstTurnNumber: Int = 1, harnessName: String = "harness",
        icons: TranscriptIcons = .none, rawMode: Bool = false,
        billingAccountIds: Set<String>? = nil
    ) -> TranscriptDocument {
        let formatter = DateFormatter()
        formatter.dateFormat = "HH:mm:ss"
        let labelFont = NSFont.monospacedSystemFont(ofSize: 10, weight: .bold)
        let separatorFont = NSFont.monospacedSystemFont(ofSize: 9, weight: .regular)
        let proseFont = NSFont.systemFont(ofSize: 13)
        let monoFont = NSFont.monospacedSystemFont(ofSize: 12.5, weight: .regular)
        let detailsFont = NSFont.systemFont(ofSize: 11, weight: .semibold)

        let separatorPara = NSMutableParagraphStyle()
        separatorPara.paragraphSpacing = 6
        separatorPara.paragraphSpacingBefore = 12
        let leftLabelPara = NSMutableParagraphStyle()
        leftLabelPara.firstLineHeadIndent = 12
        leftLabelPara.headIndent = 12
        leftLabelPara.paragraphSpacing = 1
        leftLabelPara.paragraphSpacingBefore = 3
        let rightLabelPara = NSMutableParagraphStyle()
        rightLabelPara.alignment = .right
        rightLabelPara.tailIndent = -12
        rightLabelPara.paragraphSpacing = 1
        rightLabelPara.paragraphSpacingBefore = 3
        let leftBodyPara = NSMutableParagraphStyle()
        leftBodyPara.firstLineHeadIndent = 18
        leftBodyPara.headIndent = 18
        leftBodyPara.tailIndent = -88
        leftBodyPara.lineHeightMultiple = 1.15
        leftBodyPara.paragraphSpacing = 2
        let rightBodyPara = NSMutableParagraphStyle()
        rightBodyPara.firstLineHeadIndent = 88
        rightBodyPara.headIndent = 88
        rightBodyPara.tailIndent = -18
        rightBodyPara.lineHeightMultiple = 1.15
        rightBodyPara.paragraphSpacing = 2
        let rightCardPara = NSMutableParagraphStyle()
        rightCardPara.firstLineHeadIndent = 88
        rightCardPara.headIndent = 88
        rightCardPara.tailIndent = -18
        rightCardPara.lineHeightMultiple = 1.15
        rightCardPara.paragraphSpacing = 2

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
            .paragraphStyle: leftLabelPara,
        ]
        let modelLabel: [NSAttributedString.Key: Any] = [
            .font: labelFont, .foregroundColor: NSColor.secondaryLabelColor,
            .paragraphStyle: rightLabelPara,
        ]
        let toolLabel: [NSAttributedString.Key: Any] = [
            .font: labelFont, .foregroundColor: NSColor.systemPurple,
            .paragraphStyle: rightCardPara,
        ]
        let user: [NSAttributedString.Key: Any] = [
            .font: proseFont, .foregroundColor: NSColor.labelColor,
            .paragraphStyle: leftBodyPara,
            .transcriptBubbleKind: TranscriptBubbleKind.user.rawValue,
        ]
        let toolResult: [NSAttributedString.Key: Any] = [
            .font: monoFont, .foregroundColor: NSColor.labelColor,
            .paragraphStyle: leftBodyPara,
            .transcriptBubbleKind: TranscriptBubbleKind.toolResult.rawValue,
        ]
        let assistant: [NSAttributedString.Key: Any] = [
            .font: proseFont, .foregroundColor: NSColor.labelColor,
            .paragraphStyle: rightBodyPara,
            .transcriptBubbleKind: TranscriptBubbleKind.model.rawValue,
        ]
        let tool: [NSAttributedString.Key: Any] = [
            .font: monoFont, .foregroundColor: NSColor.labelColor,
            .paragraphStyle: rightCardPara,
            .transcriptBubbleKind: TranscriptBubbleKind.tool.rawValue,
        ]
        let error: [NSAttributedString.Key: Any] = [
            .font: monoFont, .foregroundColor: NSColor.systemRed,
            .paragraphStyle: rightCardPara,
            .transcriptBubbleKind: TranscriptBubbleKind.error.rawValue,
        ]
        var toolResultKey = toolResult
        toolResultKey[.foregroundColor] = NSColor.secondaryLabelColor
        var toolKey = tool
        toolKey[.foregroundColor] = NSColor.secondaryLabelColor

        func linked(
            _ attrs: [NSAttributedString.Key: Any], _ traceId: String
        ) -> [NSAttributedString.Key: Any] {
            guard let url = TraceLink.url(forTraceId: traceId) else { return attrs }
            var out = attrs
            out[.link] = url
            return out
        }

        let out = NSMutableAttributedString()
        func appendTool(name: String, arguments: String?) {
            out.append(NSAttributedString(string: "⚙ \(name)\n", attributes: toolLabel))
            let arguments = arguments ?? ""
            if rawMode {
                if !arguments.isEmpty {
                    out.append(NSAttributedString(
                        string: "\(cap(arguments))\n", attributes: tool))
                }
            } else if let command = ToolCall(name: name, arguments: arguments).command {
                out.append(NSAttributedString(string: "\(cap(command))\n", attributes: tool))
            } else if !arguments.isEmpty {
                appendNice(arguments, to: out, keyAttrs: toolKey, valueAttrs: tool)
            }
        }
        var ranges: [TurnRange] = []
        for (index, turn) in turns.enumerated() {
            let turnStart = out.length
            let facts = TurnHeader.separatorFacts(
                turnNumber: firstTurnNumber + index,
                time: formatter.string(
                    from: Date(timeIntervalSince1970: Double(turn.tsRequestMs) / 1000)),
                status: turn.status,
                requestMs: turn.tsRequestMs, responseMs: turn.tsResponseMs,
                costUsd: turn.costUsd,
                reasoningEffort: turn.reasoningEffort,
                thinkingBudget: turn.thinkingBudget,
                accountId: turn.accountId.flatMap { accountId in
                    billingAccountIds?.contains(accountId) == false ? nil : accountId
                })
            let isError = (turn.status ?? 0) >= 400
            let sepAttrs = linked(isError ? badSeparator : separator, turn.traceId)
            out.append(NSAttributedString(string: "· \(facts) ·", attributes: sepAttrs))
            var detailsAttrs = sepAttrs
            detailsAttrs[.font] = detailsFont
            detailsAttrs[.foregroundColor] = NSColor.controlAccentColor
            out.append(NSAttributedString(string: "   ⓘ Details", attributes: detailsAttrs))
            out.append(NSAttributedString(string: "\n", attributes: sepAttrs))
            if let text = turn.user, !text.isEmpty {
                let toolBody = TurnHeader.toolResultBody(text)
                let label = TurnHeader.requestLabel(
                    harness: harnessName, isToolResult: toolBody != nil)
                let labelAttrs = linked(userLabel, turn.traceId)
                if let icon = icons.harness {
                    out.append(iconString(icon, attributes: labelAttrs))
                    out.append(NSAttributedString(string: " ", attributes: labelAttrs))
                }
                out.append(NSAttributedString(string: label, attributes: labelAttrs))
                out.append(infoMark(labelAttrs))
                if let toolBody {
                    if rawMode {
                        out.append(NSAttributedString(
                            string: "\(cap(toolBody))\n", attributes: toolResult))
                    } else {
                        appendNice(
                            toolBody, to: out, keyAttrs: toolResultKey, valueAttrs: toolResult)
                    }
                } else {
                    out.append(NSAttributedString(string: "\(cap(text))\n", attributes: user))
                }
            }
            let calls = turn.toolCalls ?? []
            let orderedBlocks = turn.assistantBlocks ?? []
            let hasModelText = turn.assistant?.isEmpty == false
            if hasModelText || !calls.isEmpty || !orderedBlocks.isEmpty {
                let labelAttrs = linked(modelLabel, turn.traceId)
                if let icon = providerIcon(
                    provider: turn.provider, model: turn.model, icons: icons)
                {
                    out.append(iconString(icon, attributes: labelAttrs))
                    out.append(NSAttributedString(string: " ", attributes: labelAttrs))
                }
                out.append(NSAttributedString(
                    string: TurnHeader.responseLabel(model: turn.model),
                    attributes: labelAttrs))
                out.append(infoMark(labelAttrs))
            }
            if !orderedBlocks.isEmpty {
                for block in orderedBlocks {
                    switch block.type {
                    case "text":
                        if let text = block.text, !text.isEmpty {
                            out.append(NSAttributedString(
                                string: "\(cap(text))\n", attributes: assistant))
                        }
                    case "tool_call":
                        if let name = block.name {
                            appendTool(name: name, arguments: block.arguments)
                        }
                    default:
                        continue
                    }
                }
            } else {
                if let text = turn.assistant, !text.isEmpty {
                    out.append(NSAttributedString(
                        string: "\(cap(text))\n", attributes: assistant))
                }
                for call in calls {
                    appendTool(name: call.name, arguments: call.arguments)
                }
            }
            if let text = turn.error, !text.isEmpty {
                out.append(NSAttributedString(string: "\(cap(text))\n", attributes: error))
            }
            out.append(NSAttributedString(string: "\n", attributes: separator))
            let turnRange = NSRange(location: turnStart, length: out.length - turnStart)
            out.addAttribute(.transcriptTurnId, value: turn.traceId, range: turnRange)
            var kindRuns: [NSRange] = []
            out.enumerateAttribute(.transcriptBubbleKind, in: turnRange) { value, range, _ in
                if value != nil { kindRuns.append(range) }
            }
            for (groupIndex, runRange) in kindRuns.enumerated() {
                out.addAttribute(
                    .transcriptBubbleGroup, value: "\(turn.traceId)#\(groupIndex)",
                    range: runRange)
            }
            ranges.append(TurnRange(traceId: turn.traceId, range: turnRange))
        }
        return TranscriptDocument(
            text: NSAttributedString(attributedString: out), turnRanges: ranges)
    }

    static func infoMark(_ labelAttrs: [NSAttributedString.Key: Any]) -> NSAttributedString {
        var attrs = labelAttrs
        attrs[.foregroundColor] = NSColor.controlAccentColor
        return NSAttributedString(string: "  ⓘ\n", attributes: attrs)
    }

    static func appendNice(
        _ body: String, to out: NSMutableAttributedString,
        keyAttrs: [NSAttributedString.Key: Any], valueAttrs: [NSAttributedString.Key: Any]
    ) {
        for block in JsonNice.blocks(body) {
            switch block {
            case let .row(key, value):
                out.append(NSAttributedString(string: "\(key): ", attributes: keyAttrs))
                out.append(NSAttributedString(string: "\(cap(value))\n", attributes: valueAttrs))
            case let .block(key, text):
                out.append(NSAttributedString(string: "\(key):\n", attributes: keyAttrs))
                out.append(NSAttributedString(string: "\(cap(text))\n", attributes: valueAttrs))
            case let .text(text):
                out.append(NSAttributedString(string: "\(cap(text))\n", attributes: valueAttrs))
            }
        }
    }

    static func providerIcon(
        provider: String?, model: String?, icons: TranscriptIcons
    ) -> NSImage? {
        guard let provider = provider ?? model.flatMap(ModelProvider.provider(forModel:)) else {
            return nil
        }
        return icons.providers[provider]
    }

    static func iconString(
        _ image: NSImage, attributes: [NSAttributedString.Key: Any]
    ) -> NSAttributedString {
        let attachment = NSTextAttachment()
        attachment.image = image
        attachment.bounds = CGRect(x: 0, y: -3, width: 13, height: 13)
        let out = NSMutableAttributedString(attachment: attachment)
        var attrs = attributes
        attrs.removeValue(forKey: .attachment)
        out.addAttributes(attrs, range: NSRange(location: 0, length: out.length))
        return out
    }

    static func cap(_ text: String, maxChars: Int = maxTurnChars) -> String {
        guard text.count > maxChars else { return text }
        return text.prefix(maxChars) + "\n… (+\(text.count - maxChars) chars truncated)"
    }

    static func tokens(_ count: Int64?) -> String { TraceNumberFormat.tokens(count) }

    static func cost(_ usd: Double) -> String { TraceNumberFormat.cost(usd) }
    #endif
}

public struct DarioAdminStatus: Codable, Sendable {
    public let activeGenerationId: String?
    public let generations: [DarioGenerationDetail]
    public let promptCaches: [DarioPromptCacheSummary]?

    enum CodingKeys: String, CodingKey {
        case generations
        case promptCaches = "prompt_caches"
        case activeGenerationId = "active_generation_id"
    }
}

public struct DarioPromptCacheSummary: Codable, Sendable, Identifiable, Equatable {
    public let key: String
    public let model: String?
    public let source: String?
    public let capturedAt: String?
    public let lastUsedAt: String?
    public let traceId: String?
    public let claudeBin: String?
    public let claudeVersion: String?
    public let systemPromptChars: Int?
    public let agentIdentityChars: Int?
    public let path: String?
    public let runs: [DarioPromptCacheRun]?

    public var id: String { key }

    enum CodingKeys: String, CodingKey {
        case key, model, source, path, runs
        case capturedAt = "captured_at"
        case lastUsedAt = "last_used_at"
        case traceId = "trace_id"
        case claudeBin = "claude_bin"
        case claudeVersion = "claude_version"
        case systemPromptChars = "system_prompt_chars"
        case agentIdentityChars = "agent_identity_chars"
    }
}

public struct DarioPromptCacheRun: Codable, Sendable, Equatable {
    public let traceId: String?
    public let usedAt: String?
    public let status: String?
    public let error: String?

    enum CodingKeys: String, CodingKey {
        case status, error
        case traceId = "trace_id"
        case usedAt = "used_at"
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
    public static func newerActivity(
        live: Bool, selectedId: String?, selectedLastTsMs: Int64?,
        newestId: String?, newestLastTsMs: Int64?
    ) -> Bool {
        guard live, let selectedId, let newestId, newestId != selectedId,
            let newestLastTsMs
        else { return false }
        return newestLastTsMs > (selectedLastTsMs ?? Int64.min)
    }
}
