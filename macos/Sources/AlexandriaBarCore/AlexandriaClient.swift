import Foundation

public enum TraceBodyKind: String, Sendable, CaseIterable {
    case request
    case upstreamRequest = "upstream-request"
    case response
    case darioUpstreamRequest = "dario-upstream-request"
    case darioUpstreamResponse = "dario-upstream-response"
}

public struct TraceBodyContent: Sendable, Equatable {
    public let text: String
    public let diskPath: String?

    public init(text: String, diskPath: String?) {
        self.text = text
        self.diskPath = diskPath
    }
}

public struct AlexandriaClient: Sendable {
    public let config: DaemonConfig
    private let session: URLSession

    public init(config: DaemonConfig) {
        self.config = config
        let cfg = URLSessionConfiguration.ephemeral
        cfg.timeoutIntervalForRequest = 5
        cfg.timeoutIntervalForResource = 10
        self.session = URLSession(configuration: cfg)
    }

    init(config: DaemonConfig, session: URLSession) {
        self.config = config
        self.session = session
    }

    public enum ClientError: Error, LocalizedError {
        case http(Int, String)
        public var errorDescription: String? {
            switch self {
            case let .http(code, body): "HTTP \(code): \(body.prefix(200))"
            }
        }
    }

    private func url(_ path: String, query: [URLQueryItem] = []) -> URL {
        var comps = URLComponents(url: config.baseURL, resolvingAgainstBaseURL: false)!
        comps.path = "/" + path
        let items = query.filter { $0.value?.isEmpty == false }
        if !items.isEmpty { comps.queryItems = items }
        return comps.url ?? config.baseURL.appendingPathComponent(path)
    }

    private func request(
        _ path: String, query: [URLQueryItem] = [], method: String = "GET",
        body: Data? = nil
    ) async throws -> Data {
        var req = URLRequest(url: url(path, query: query))
        req.httpMethod = method
        req.setValue(config.localKey, forHTTPHeaderField: "x-api-key")
        if let body {
            req.setValue("application/json", forHTTPHeaderField: "content-type")
            req.httpBody = body
        }
        let start = ContinuousClock.now
        let data: Data
        let resp: URLResponse
        do {
            (data, resp) = try await session.data(for: req)
        } catch {
            BarLog.error(.net, "\(method) /\(path) error \(Self.ms(since: start))ms: \(error.localizedDescription)")
            throw error
        }
        let status = (resp as? HTTPURLResponse)?.statusCode ?? -1
        if status >= 400 {
            BarLog.error(.net, "\(method) /\(path) \(status) \(Self.ms(since: start))ms")
            throw ClientError.http(status, String(data: data, encoding: .utf8) ?? "")
        }
        BarLog.info(.net, "\(method) /\(path) \(status) \(Self.ms(since: start))ms")
        return data
    }

    private func body<T: Encodable>(_ value: T) throws -> Data {
        try JSONEncoder().encode(value)
    }

    private func encodedPathComponent(_ value: String) -> String {
        value.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? value
    }

    private static func ms(since start: ContinuousClock.Instant) -> Int {
        let elapsed = start.duration(to: .now)
        return Int(elapsed.components.seconds * 1000)
            + Int(elapsed.components.attoseconds / 1_000_000_000_000_000)
    }

    private func get<T: Decodable & Sendable>(
        _ path: String, query: [URLQueryItem] = [], as type: T.Type
    ) async throws -> T {
        let data = try await request(path, query: query)
        do {
            return try await Task.detached { try JSONDecoder().decode(T.self, from: data) }.value
        } catch {
            let snippet = String(data: data, encoding: .utf8).map { String($0.prefix(200)) } ?? "<non-utf8>"
            BarLog.error(.net, "decode \(T.self) failed for /\(path): \(error) body=\(snippet)")
            throw error
        }
    }

    public func health() async throws -> DaemonHealth {
        try await get("health", as: DaemonHealth.self)
    }

    public func accounts() async throws -> [Account] {
        try await get("admin/accounts", as: AccountsResponse.self).accounts
    }

    public func accountHealth() async throws -> [HealthAccount] {
        try await get("admin/health", as: HealthResponse.self).accounts
    }

    public func limits() async throws -> [ProviderLimits] {
        try await get("admin/limits", as: LimitsResponse.self).providers
    }

    public func analytics(sinceMinutes: Int = 60) async throws -> Analytics {
        try await get(
            "admin/analytics",
            query: [URLQueryItem(name: "since_minutes", value: "\(sinceMinutes)")],
            as: Analytics.self)
    }

    public func dario() async throws -> DarioStatus? {
        do {
            return try await get("admin/dario", as: DarioStatus.self)
        } catch ClientError.http(404, _) {
            return nil
        }
    }

    public func darioDetail() async throws -> DarioAdminStatus? {
        do {
            return try await get("admin/dario", as: DarioAdminStatus.self)
        } catch ClientError.http(404, _) {
            return nil
        }
    }

    public func darioLogs(generationId: String, lines: Int = 300) async throws -> DarioLogsResponse {
        try await get(
            "admin/dario/logs/\(generationId)",
            query: [URLQueryItem(name: "lines", value: "\(lines)")],
            as: DarioLogsResponse.self)
    }

    public func darioRestart() async throws {
        _ = try await request("admin/dario/restart", method: "POST")
    }

    public func darioUpdate() async throws {
        _ = try await request("admin/dario/update", method: "POST")
    }

    public func darioPromptCacheClear(key: String) async throws {
        _ = try await request("admin/dario/prompt-caches/\(key)", method: "DELETE")
    }

    public func authLoginStart(provider: String) async throws -> LoginSession {
        let data = try await request(
            "admin/auth/login/start", method: "POST", body: body(["provider": provider]))
        return try JSONDecoder().decode(LoginSession.self, from: data)
    }

    public func authLoginStatus(id: String) async throws -> LoginSession {
        try await get("admin/auth/login/\(id)", as: LoginSession.self)
    }

    public func authLoginComplete(id: String, input: String) async throws -> LoginSession {
        let data = try await request(
            "admin/auth/login/complete", method: "POST",
            body: body(["login_id": id, "input": input]))
        return try JSONDecoder().decode(LoginSession.self, from: data)
    }

    public func authImport(source: String = "all") async throws -> [ImportOutcome] {
        let data = try await request(
            "admin/auth/import", method: "POST", body: body(["source": source]))
        return try JSONDecoder().decode(ImportOutcomes.self, from: data).outcomes
    }

    public func harnesses() async throws -> [Harness]? {
        do {
            return try await get("admin/harnesses", as: HarnessesResponse.self).harnesses
        } catch ClientError.http(404, _) {
            return nil
        }
    }

    public func connectHarness(_ name: String) async throws -> HarnessConnectResponse {
        let encoded = encodedPathComponent(name)
        let data = try await request("admin/harnesses/\(encoded)/connect", method: "POST")
        return try JSONDecoder().decode(HarnessConnectResponse.self, from: data)
    }

    public func disconnectHarness(_ name: String) async throws -> HarnessDisconnectResponse {
        let encoded = encodedPathComponent(name)
        let data = try await request("admin/harnesses/\(encoded)/disconnect", method: "POST")
        return try JSONDecoder().decode(HarnessDisconnectResponse.self, from: data)
    }

    public func setHarnessOverride(_ name: String, binary: String?, configDir: String?) async throws -> Harness {
        let encoded = encodedPathComponent(name)
        let data = try await request(
            "admin/harnesses/\(encoded)/override", method: "PUT",
            body: body(HarnessOverride(binary: binary, configDir: configDir)))
        return try JSONDecoder().decode(Harness.self, from: data)
    }

    public func traceSessions(since: String = "24h", limit: Int = 200) async throws -> [TraceSession] {
        try await get(
            "traces/sessions",
            query: [
                URLQueryItem(name: "since", value: since),
                URLQueryItem(name: "limit", value: "\(limit)"),
            ],
            as: TraceSessionsResponse.self
        ).sessions
    }

    public func traceTranscript(sessionId: String, limit: Int = 500) async throws -> TranscriptResponse {
        try await get(
            "traces/sessions/\(sessionId)/transcript",
            query: [URLQueryItem(name: "limit", value: "\(limit)")],
            as: TranscriptResponse.self)
    }

    public func searchTraces(text: String, since: String = "24h", filters: OmniQuery = OmniQuery()) async throws -> TraceSearchResponse {
        try await get(
            "traces/search",
            query: [
                URLQueryItem(name: "text", value: text),
                URLQueryItem(name: "since", value: since),
                URLQueryItem(name: "model", value: filters.model),
                URLQueryItem(name: "provider", value: filters.provider),
                URLQueryItem(name: "harness", value: filters.harness),
                URLQueryItem(name: "status", value: filters.status),
                URLQueryItem(name: "session", value: filters.session),
                URLQueryItem(name: "run_id", value: filters.run),
            ],
            as: TraceSearchResponse.self)
    }

    public func traceDetail(id: String) async throws -> TraceDetailResponse {
        try await get("traces/\(id)", as: TraceDetailResponse.self)
    }

    public func traceBody(id: String, kind: TraceBodyKind) async throws -> TraceBodyContent {
        var req = URLRequest(url: url("traces/\(id)/body/\(kind.rawValue)"))
        req.setValue(config.localKey, forHTTPHeaderField: "x-api-key")
        let start = ContinuousClock.now
        let data: Data
        let resp: URLResponse
        do {
            (data, resp) = try await session.data(for: req)
        } catch {
            BarLog.error(
                .net,
                "GET /traces/\(id)/body/\(kind.rawValue) error \(Self.ms(since: start))ms: \(error.localizedDescription)")
            throw error
        }
        let http = resp as? HTTPURLResponse
        let status = http?.statusCode ?? -1
        if status >= 400 {
            BarLog.error(.net, "GET /traces/\(id)/body/\(kind.rawValue) \(status) \(Self.ms(since: start))ms")
            throw ClientError.http(status, String(data: data, encoding: .utf8) ?? "")
        }
        BarLog.info(.net, "GET /traces/\(id)/body/\(kind.rawValue) \(status) \(Self.ms(since: start))ms")
        return TraceBodyContent(
            text: String(data: data, encoding: .utf8) ?? "",
            diskPath: http?.value(forHTTPHeaderField: "x-alexandria-body-path"))
    }

    public func traceReplyMarkdown(traceId: String) async throws -> String {
        let data = try await request("traces/\(traceId)/reply.md")
        return String(data: data, encoding: .utf8) ?? ""
    }

    public func deleteTrace(id: String) async throws {
        _ = try await request("traces/\(id)", method: "DELETE")
    }

    public func removeAccount(id: String) async throws {
        let encoded = id.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? id
        _ = try await request("admin/accounts/\(encoded)", method: "DELETE")
    }

    public func setGeminiKey(_ key: String) async throws {
        _ = try await request("admin/auth/gemini-key", method: "POST", body: body(["key": key]))
    }
}
