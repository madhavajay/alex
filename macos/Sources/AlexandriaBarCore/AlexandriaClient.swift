import Foundation
#if canImport(FoundationNetworking)
import FoundationNetworking
#endif

private struct AuthLoginStartBody: Encodable {
    let provider: String
    let name: String?
    let autoIdentity: Bool

    enum CodingKeys: String, CodingKey {
        case provider, name
        case autoIdentity = "auto_identity"
    }
}

private struct OpenRouterKeyBody: Encodable {
    let key: String?
    let httpReferer: String?
    let xTitle: String?
    let remove: Bool?

    enum CodingKeys: String, CodingKey {
        case key, remove
        case httpReferer = "http_referer"
        case xTitle = "x_title"
    }
}

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
        case daemonUpdateRejected(String)
        public var errorDescription: String? {
            switch self {
            case let .http(code, body): "HTTP \(code): \(body.prefix(200))"
            case let .daemonUpdateRejected(reason): reason
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

    /// Returns the daemon-generated shell exports used by `alex credentials`.
    /// Keep this value ephemeral: callers should copy it directly rather than log it.
    public func credentialsEnvironment() async throws -> String {
        let data = try await request(
            "connect",
            query: [URLQueryItem(name: "format", value: "env")])
        guard let environment = String(data: data, encoding: .utf8) else {
            throw ClientError.http(0, "credential response was not UTF-8")
        }
        return environment
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

    public func accountAnalytics(
        sinceMinutes: Int = 24 * 60, bucketMinutes: Int = 60
    ) async throws -> AccountAnalyticsResponse {
        try await get(
            "admin/accounts/analytics",
            query: [
                URLQueryItem(name: "since_minutes", value: "\(sinceMinutes)"),
                URLQueryItem(name: "bucket_minutes", value: "\(bucketMinutes)"),
            ],
            as: AccountAnalyticsResponse.self)
    }

    /// Gets a real-count dry-run plan before a user can commit a reset.
    public func resetPlan(_ selection: ResetSelection) async throws -> ResetResponse {
        try await reset(selection, dryRun: true)
    }

    /// Applies the selected reset categories. Call `resetPlan` first to show its counts to the user.
    public func reset(_ selection: ResetSelection, dryRun: Bool = false) async throws -> ResetResponse {
        let data = try await request(
            "admin/reset", method: "POST",
            body: body(ResetRequest(selection: selection, dryRun: dryRun)))
        return try JSONDecoder().decode(ResetResponse.self, from: data)
    }

    public func routing(provider: String) async throws -> ProviderRoutingResponse {
        try await get(
            "admin/routing/\(encodedPathComponent(provider))",
            as: ProviderRoutingResponse.self)
    }

    public func updateRouting(provider: String, _ update: ProviderRoutingUpdate) async throws {
        _ = try await request(
            "admin/routing/\(encodedPathComponent(provider))",
            method: "PUT",
            body: body(update))
    }

    /// Compatibility entry points for app versions which only exposed Codex.
    public func codexRouting() async throws -> CodexRoutingResponse {
        try await get("admin/accounts/routing/openai", as: CodexRoutingResponse.self)
    }

    public func updateCodexRouting(_ update: CodexRoutingUpdate) async throws {
        _ = try await request(
            "admin/accounts/routing/openai", method: "PUT", body: body(update))
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

    public func daemonUpdateStatus() async throws -> DaemonUpdateStatus {
        try await get("admin/update", as: DaemonUpdateStatus.self)
    }

    public func daemonUpdateApply() async throws -> DaemonUpdateApplyResponse {
        do {
            let data = try await request("admin/update", method: "POST")
            return try JSONDecoder().decode(DaemonUpdateApplyResponse.self, from: data)
        } catch ClientError.http(409, let body) {
            throw ClientError.daemonUpdateRejected(Self.updateRejectionReason(from: body))
        }
    }

    private static func updateRejectionReason(from body: String) -> String {
        let data = Data(body.utf8)
        if let decoded = try? JSONDecoder().decode(DaemonUpdateApplyResponse.self, from: data),
           let reason = decoded.reason, !reason.isEmpty {
            return reason
        }
        if let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any] {
            if let reason = obj["reason"] as? String, !reason.isEmpty {
                return reason
            }
            if let error = obj["error"] as? [String: Any],
               let message = error["message"] as? String, !message.isEmpty {
                return message
            }
        }
        return String(body.prefix(200))
    }

    public func darioPromptCacheClear(key: String) async throws {
        _ = try await request("admin/dario/prompt-caches/\(key)", method: "DELETE")
    }

    public func authLoginStart(
        provider: String,
        name: String? = "default",
        autoIdentity: Bool = false
    ) async throws -> LoginSession {
        let data = try await request(
            "admin/auth/login/start", method: "POST",
            body: body(AuthLoginStartBody(
                provider: provider, name: name, autoIdentity: autoIdentity)))
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

    public func connectHarness(_ name: String) async throws -> HarnessConfigWriteResponse {
        let encoded = encodedPathComponent(name)
        let data = try await request("admin/harnesses/\(encoded)/connect", method: "POST")
        return try JSONDecoder().decode(HarnessConfigWriteResponse.self, from: data)
    }

    public func connectHarnessPlan(_ name: String) async throws -> HarnessPlanResponse {
        let encoded = encodedPathComponent(name)
        let data = try await request(
            "admin/harnesses/\(encoded)/connect",
            query: [URLQueryItem(name: "dry_run", value: "true")],
            method: "POST")
        return try JSONDecoder().decode(HarnessPlanResponse.self, from: data)
    }

    public func disconnectHarness(_ name: String) async throws -> HarnessDisconnectResponse {
        let encoded = encodedPathComponent(name)
        let data = try await request("admin/harnesses/\(encoded)/disconnect", method: "POST")
        return try JSONDecoder().decode(HarnessDisconnectResponse.self, from: data)
    }

    public func disconnectHarnessPlan(_ name: String) async throws -> HarnessPlanResponse {
        let encoded = encodedPathComponent(name)
        let data = try await request(
            "admin/harnesses/\(encoded)/disconnect",
            query: [URLQueryItem(name: "dry_run", value: "true")],
            method: "POST")
        return try JSONDecoder().decode(HarnessPlanResponse.self, from: data)
    }

    public func refreshHarnessConfig(_ name: String) async throws -> HarnessConfigWriteResponse {
        let encoded = encodedPathComponent(name)
        let data = try await request("admin/harnesses/\(encoded)/refresh-config", method: "POST")
        return try JSONDecoder().decode(HarnessConfigWriteResponse.self, from: data)
    }

    public func setCodexDefaultRoute(_ route: String) async throws -> CodexDefaultRouteResponse {
        let data = try await request(
            "admin/harnesses/codex/default-route", method: "PUT",
            body: body(["route": route]))
        return try JSONDecoder().decode(CodexDefaultRouteResponse.self, from: data)
    }

    public func setHarnessToolCapture(_ name: String, enabled: Bool) async throws {
        let encoded = encodedPathComponent(name)
        _ = try await request(
            "admin/harnesses/\(encoded)/tool-capture", method: "PUT",
            body: body(["enabled": enabled]))
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
                URLQueryItem(name: "effort", value: filters.effort),
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

    public func toolBody(id: String, kind: String) async throws -> TraceBodyContent {
        var req = URLRequest(url: url("tools/\(id)/body/\(kind)"))
        req.setValue(config.localKey, forHTTPHeaderField: "x-api-key")
        let (data, response) = try await session.data(for: req)
        let http = response as? HTTPURLResponse
        guard (http?.statusCode ?? -1) < 400 else {
            throw ClientError.http(http?.statusCode ?? -1, String(data: data, encoding: .utf8) ?? "")
        }
        return TraceBodyContent(text: String(data: data, encoding: .utf8) ?? "", diskPath: nil)
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

    public func setAccountPaused(id: String, paused: Bool) async throws {
        let encoded = id.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? id
        _ = try await request(
            "admin/accounts/\(encoded)", method: "PUT", body: body(["paused": paused]))
    }

    public func setGeminiKey(_ key: String) async throws {
        _ = try await request("admin/auth/gemini-key", method: "POST", body: body(["key": key]))
    }

    public func setOpenRouterKey(
        _ key: String, httpReferer: String? = nil, xTitle: String? = nil
    ) async throws {
        _ = try await request(
            "admin/auth/openrouter-key",
            method: "POST",
            body: body(OpenRouterKeyBody(
                key: key, httpReferer: httpReferer, xTitle: xTitle, remove: nil)))
    }

    public func removeOpenRouterKey() async throws {
        _ = try await request(
            "admin/auth/openrouter-key",
            method: "POST",
            body: body(OpenRouterKeyBody(
                key: nil, httpReferer: nil, xTitle: nil, remove: true)))
    }
}
