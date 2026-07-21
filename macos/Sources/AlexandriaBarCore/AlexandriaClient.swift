import Foundation
#if canImport(FoundationNetworking)
import FoundationNetworking
#endif

private struct AuthLoginStartBody: Encodable {
    let provider: String
    let name: String?
    let autoIdentity: Bool
    let force: Bool?

    enum CodingKeys: String, CodingKey {
        case provider, name, force
        case autoIdentity = "auto_identity"
    }
}

private struct ReauthNotifyBody: Encodable {
    let provider: String
    let accountId: String?
    let force: Bool?

    enum CodingKeys: String, CodingKey {
        case provider, force
        case accountId = "account_id"
    }
}

private struct OpenRouterKeyBody: Encodable {
    let key: String?
    let displayName: String?
    let httpReferer: String?
    let xTitle: String?
    let remove: Bool?

    enum CodingKeys: String, CodingKey {
        case key, remove
        case displayName = "display_name"
        case httpReferer = "http_referer"
        case xTitle = "x_title"
    }
}

private struct OpenRouterKeyResponse: Decodable {
    let saved: String
}

private struct OpenRouterExposedBody: Encodable {
    let exposed: [String]
}

private struct CLIProxyAPIConnectBody: Encodable {
    let url: String
    let credential: String
}

private struct FixtureInjectionBody: Encodable {
    let fixture: String
    let count: Int?
    let direction: String?
}

private struct FixtureCaptureBody: Encodable {
    let name: String
    let fromTraceId: String
    let kind: String

    enum CodingKeys: String, CodingKey {
        case name, kind
        case fromTraceId = "from_trace_id"
    }
}

private struct NotificationCommandsBody: Encodable {
    let channelId: String
    let allowCommands: Bool

    enum CodingKeys: String, CodingKey {
        case channelId = "channel_id"
        case allowCommands = "allow_commands"
    }
}

private struct SavedNotificationTestBody: Encodable {
    let channelId: String

    enum CodingKeys: String, CodingKey {
        case channelId = "channel_id"
    }
}

private struct MintRunKeyBody: Encodable {
    let kind: String
    let label: String?
    let tags: [String: String]
    let ttlSeconds: Int?

    enum CodingKeys: String, CodingKey {
        case kind, label, tags
        case ttlSeconds = "ttl_seconds"
    }
}

public enum RunKeyKind: String, Sendable {
    case run
    case harness
    case wrap
}

private struct RunKeysRevokedResponse: Decodable {
    let revoked: Int
}

private struct RunKeysRemovedResponse: Decodable {
    let removed: Int
}

private struct ModelCatalogResponse: Decodable {
    struct Model: Decodable { let id: String }
    let data: [Model]
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
        // Resource timeout bounds the WHOLE transfer and overrides any
        // per-request budget — keep it high enough for slow endpoints
        // (reset dry-run walks 100k+ body files; large trace bodies).
        cfg.timeoutIntervalForResource = 300
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
        body: Data? = nil, timeout: TimeInterval? = nil
    ) async throws -> Data {
        var req = URLRequest(url: url(path, query: query))
        if let timeout { req.timeoutInterval = timeout }
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
            return try await Task.detached {
                let start = ContinuousClock.now
                defer {
                    let elapsed = start.duration(to: .now)
                    let ms = Double(elapsed.components.seconds) * 1000
                        + Double(elapsed.components.attoseconds) / 1e15
                    BarLog.timing(.net, label: "decode /\(path) bytes=\(data.count)", milliseconds: ms)
                }
                return try JSONDecoder().decode(T.self, from: data)
            }.value
        } catch {
            let snippet = String(data: data, encoding: .utf8).map { String($0.prefix(200)) } ?? "<non-utf8>"
            BarLog.error(.net, "decode \(T.self) failed for /\(path): \(error) body=\(snippet)")
            throw error
        }
    }

    public func health() async throws -> DaemonHealth {
        try await get("health", as: DaemonHealth.self)
    }

    /// Models exposed by the daemon to OpenAI-compatible clients.
    public func modelCatalog() async throws -> [String] {
        try await get("v1/models", as: ModelCatalogResponse.self).data.map(\.id)
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

    /// Lists redacted inbound and outbound credential metadata. This response
    /// never includes a key value.
    public func credentials() async throws -> CredentialsResponse {
        try await get("admin/credentials", as: CredentialsResponse.self)
    }

    /// Creates a one-time scoped key response. The returned secret must be
    /// handled ephemerally by the caller and is never available from `credentials()`.
    public func mintRunKey(
        label: String?, model: String?, ttlSeconds: Int?, kind: RunKeyKind = .run
    ) async throws -> MintedRunKey {
        let normalizedLabel = label?.trimmingCharacters(in: .whitespacesAndNewlines)
        var tags: [String: String] = [:]
        if let model, !model.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            tags["model"] = model.trimmingCharacters(in: .whitespacesAndNewlines)
        }
        let data = try await request(
            "admin/run-keys", method: "POST",
            body: body(MintRunKeyBody(
                kind: kind.rawValue,
                label: normalizedLabel?.isEmpty == false ? normalizedLabel : nil,
                tags: tags, ttlSeconds: ttlSeconds)))
        return try JSONDecoder().decode(MintedRunKey.self, from: data)
    }

    public func revokeRunKey(id: String) async throws {
        _ = try await request(
            "admin/run-keys/\(encodedPathComponent(id))", method: "DELETE")
    }

    @discardableResult
    public func revokeAllRunKeys(includeHarness: Bool = true) async throws -> Int {
        // Current daemons revoke every kind and safely ignore this query item.
        // A scope-aware daemon can use it without requiring a UI/client update.
        let data = try await request(
            "admin/run-keys/revoke-all",
            query: [URLQueryItem(name: "include_harness", value: includeHarness ? "true" : "false")],
            method: "POST")
        return try JSONDecoder().decode(RunKeysRevokedResponse.self, from: data).revoked
    }

    @discardableResult
    public func clearRevokedRunKeys() async throws -> Int {
        let data = try await request("admin/run-keys/revoked", method: "DELETE")
        return try JSONDecoder().decode(RunKeysRemovedResponse.self, from: data).removed
    }

    /// Re-authorizes an exact, previously known client credential represented
    /// by the redacted fingerprint on an Alex Error trace.
    public func approveAlexErrorCredential(fingerprint: String) async throws {
        _ = try await request(
            "admin/alex-errors/\(encodedPathComponent(fingerprint))/approve",
            method: "POST")
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

    public func providerPauses() async throws -> [ProviderPause] {
        try await get("admin/providers", as: ProvidersResponse.self).providers
    }

    public func pauseProvider(_ provider: String, mode: ProviderPauseMode) async throws {
        _ = try await request(
            "admin/providers/\(encodedPathComponent(provider))/pause",
            method: "POST", body: body(["mode": mode.rawValue]))
    }

    public func resumeProvider(_ provider: String) async throws {
        _ = try await request(
            "admin/providers/\(encodedPathComponent(provider))/resume", method: "POST")
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
        try await reset(selection, mode: .immediate, dryRun: true)
    }

    /// Applies the selected reset categories. Call `resetPlan` first to show its counts to the user.
    public func reset(
        _ selection: ResetSelection, mode: ResetMode = .immediate, dryRun: Bool = false
    ) async throws -> ResetResponse {
        // Counting 100k+ captured body files on a cold filesystem cache can
        // take well past the 5s default — this endpoint gets its own budget.
        let data = try await request(
            "admin/reset", method: "POST",
            body: body(ResetRequest(selection: selection, dryRun: dryRun, mode: mode)),
            timeout: 120)
        return try JSONDecoder().decode(ResetResponse.self, from: data)
    }

    public func resetProgress() async throws -> ResetProgress {
        try await get("admin/reset/progress", as: ResetProgress.self)
    }

    @discardableResult
    public func cancelResetDrain() async throws -> ResetCancelResponse {
        let data = try await request("admin/reset", method: "DELETE")
        return try JSONDecoder().decode(ResetCancelResponse.self, from: data)
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

    public func darioRepair() async throws {
        _ = try await request("admin/dario/repair", method: "POST")
    }

    public func daemonUpdateStatus() async throws -> DaemonUpdateStatus {
        try await get("admin/update", as: DaemonUpdateStatus.self)
    }

    /// The channel the daemon currently follows (config.toml `update_channel`).
    public func daemonUpdateChannel() async throws -> DaemonChannelResponse {
        try await get("admin/update/channel", as: DaemonChannelResponse.self)
    }

    /// Persists the daemon's release channel (`stable`/`beta`) and returns it
    /// with the update availability recomputed against the new channel.
    public func setDaemonUpdateChannel(_ channel: String) async throws -> DaemonChannelResponse {
        let data = try await request(
            "admin/update/channel", method: "POST",
            body: body(["channel": channel]))
        return try JSONDecoder().decode(DaemonChannelResponse.self, from: data)
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
        autoIdentity: Bool = false,
        force: Bool = false
    ) async throws -> LoginSession {
        // BACKEND: POST /admin/auth/login/start must replace the account's
        // pending login when the existing request body includes `force: true`.
        let data = try await request(
            "admin/auth/login/start", method: "POST",
            body: body(AuthLoginStartBody(
                provider: provider, name: name, autoIdentity: autoIdentity,
                force: force ? true : nil)))
        return try JSONDecoder().decode(LoginSession.self, from: data)
    }

    public func authLoginStatus(id: String) async throws -> LoginSession {
        try await get("admin/auth/login/\(id)", as: LoginSession.self)
    }

    public func reauthNotify(
        provider: String,
        accountId: String? = nil,
        force: Bool = false
    ) async throws -> ReauthNotifyResponse {
        // BACKEND: POST /admin/auth/reauth-notify must replace the account's
        // pending login when the existing request body includes `force: true`.
        let data = try await request(
            "admin/auth/reauth-notify", method: "POST",
            body: body(ReauthNotifyBody(
                provider: provider, accountId: accountId, force: force ? true : nil)))
        return try JSONDecoder().decode(ReauthNotifyResponse.self, from: data)
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

    /// Read-only discovery for onboarding. Imports remain a separate, explicit
    /// `authImport` call after the user confirms selected candidates.
    public func credentialImportCandidates() async throws -> CredentialImportCandidatesResponse {
        try await get(
            "admin/auth/import-candidates",
            as: CredentialImportCandidatesResponse.self)
    }

    public func harnessSnapshot(refresh: Bool = false) async throws -> HarnessesResponse? {
        do {
            let query = refresh ? [URLQueryItem(name: "refresh", value: "1")] : []
            return try await get("admin/harnesses", query: query, as: HarnessesResponse.self)
        } catch ClientError.http(404, _) {
            return nil
        }
    }

    public func harnesses(refresh: Bool = false) async throws -> [Harness]? {
        try await harnessSnapshot(refresh: refresh)?.harnesses
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

    /// Lists response fixtures available for local error simulation.
    public func errorSimulationFixtures() async throws -> [ErrorSimulationFixture] {
        try await get("admin/fixtures", as: [ErrorSimulationFixture].self)
    }

    /// Queues one fixture for the next matching request in a trace session.
    public func injectFixture(
        sessionId: String,
        fixture: String,
        count: Int? = nil,
        direction: String? = nil
    ) async throws {
        let encoded = encodedPathComponent(sessionId)
        _ = try await request(
            "admin/sessions/\(encoded)/inject",
            method: "POST",
            body: body(FixtureInjectionBody(
                fixture: fixture, count: count, direction: direction)))
    }

    /// Removes all queued fixture injections for a trace session.
    public func clearFixtureInjections(sessionId: String) async throws {
        let encoded = encodedPathComponent(sessionId)
        _ = try await request("admin/sessions/\(encoded)/injections", method: "DELETE")
    }

    /// Captures a recorded error response as a named replay fixture.
    public func createErrorSimulationFixture(
        name: String,
        fromTraceId: String,
        kind: String = "resp"
    ) async throws {
        _ = try await request(
            "admin/fixtures",
            method: "POST",
            body: body(FixtureCaptureBody(name: name, fromTraceId: fromTraceId, kind: kind)))
    }

    public func protectionPolicy() async throws -> ProtectionPolicy {
        try await get("admin/protection", as: ProtectionPolicy.self)
    }

    public func updateProtectionPolicy(_ policy: ProtectionPolicy) async throws {
        _ = try await request("admin/protection", method: "PUT", body: body(policy))
    }

    // MARK: Middleware

    /// Returns the active middleware generation and its rules. The response
    /// models use defensive defaults so an app can remain usable while a beta
    /// daemon adds new status fields.
    public func middlewareStatus() async throws -> MiddlewareRuntimeStatus {
        try await get("admin/middleware", as: MiddlewareRuntimeStatus.self)
    }

    @discardableResult
    public func updateMiddlewareSettings(
        _ settings: MiddlewareSettings
    ) async throws -> MiddlewareRuntimeStatus {
        let data = try await request(
            "admin/middleware/settings", method: "PUT", body: body(settings))
        return try JSONDecoder().decode(MiddlewareRuntimeStatus.self, from: data)
    }

    public func validateMiddlewareRule(
        _ rule: MiddlewareRuleSpecV1
    ) async throws -> MiddlewareValidationResponse {
        let data = try await request(
            "admin/middleware/validate", method: "POST",
            body: body(MiddlewareValidationRequest(rule: rule)))
        return try JSONDecoder().decode(MiddlewareValidationResponse.self, from: data)
    }

    @discardableResult
    public func reloadMiddleware() async throws -> MiddlewareRuntimeStatus {
        let data = try await request(
            "admin/middleware/reload", method: "POST", body: body([String: String]()))
        return try JSONDecoder().decode(MiddlewareRuntimeStatus.self, from: data)
    }

    @discardableResult
    public func createMiddlewareRule(
        _ rule: MiddlewareRuleSpecV1
    ) async throws -> MiddlewareMutationResponse {
        let data = try await request(
            "admin/middleware/rules", method: "POST", body: body(rule))
        return try JSONDecoder().decode(MiddlewareMutationResponse.self, from: data)
    }

    @discardableResult
    public func updateMiddlewareRule(
        _ rule: MiddlewareRuleSpecV1
    ) async throws -> MiddlewareMutationResponse {
        let data = try await request(
            "admin/middleware/rules/\(encodedPathComponent(rule.id))",
            method: "PUT", body: body(rule))
        return try JSONDecoder().decode(MiddlewareMutationResponse.self, from: data)
    }

    public func deleteMiddlewareRule(id: String) async throws {
        _ = try await request(
            "admin/middleware/rules/\(encodedPathComponent(id))", method: "DELETE")
    }

    public func testMiddleware(
        _ test: MiddlewareTestRequest
    ) async throws -> MiddlewareTestResponse {
        let data = try await request(
            "admin/middleware/test", method: "POST", body: body(test))
        return try JSONDecoder().decode(MiddlewareTestResponse.self, from: data)
    }

    public func middlewareLeases() async throws -> [MiddlewareRouteLease] {
        // During beta the status response carries leases as well. Keeping the
        // dedicated endpoint makes refresh/clear operations cheap later.
        let data = try await request("admin/middleware/leases")
        if let direct = try? JSONDecoder().decode([MiddlewareRouteLease].self, from: data) {
            return direct
        }
        return try JSONDecoder().decode(MiddlewareLeasesResponse.self, from: data).leases
    }

    public func clearMiddlewareLease(id: String) async throws {
        _ = try await request(
            "admin/middleware/leases/\(encodedPathComponent(id))", method: "DELETE")
    }

    public func notificationSettings() async throws -> NotificationSettingsResponse {
        try await get("admin/notifications", as: NotificationSettingsResponse.self)
    }

    public func validateTelegramNotification(token: String) async throws -> NotificationValidationResponse {
        let data = try await request(
            "admin/notifications/validate", method: "POST",
            body: body(TelegramNotificationTokenRequest(token: token)))
        return try JSONDecoder().decode(NotificationValidationResponse.self, from: data)
    }

    public func discoverTelegramChats(token: String) async throws -> NotificationChatDiscoveryResponse {
        let data = try await request(
            "admin/notifications/discover-chat", method: "POST",
            body: body(TelegramNotificationTokenRequest(token: token)))
        return try JSONDecoder().decode(NotificationChatDiscoveryResponse.self, from: data)
    }

    @discardableResult
    public func saveTelegramNotification(
        _ channel: TelegramNotificationChannelRequest
    ) async throws -> NotificationSaveResponse {
        let data = try await request(
            "admin/notifications", method: "POST", body: body(channel))
        return try JSONDecoder().decode(NotificationSaveResponse.self, from: data)
    }

    public func testTelegramNotification(
        _ channel: TelegramNotificationChannelRequest
    ) async throws -> NotificationTestResponse {
        let data = try await request(
            "admin/notifications/test", method: "POST", body: body(channel))
        return try JSONDecoder().decode(NotificationTestResponse.self, from: data)
    }

    /// Tests a persisted channel using the token retained by the daemon.
    public func testTelegramNotification(channelId: String) async throws -> NotificationTestResponse {
        let data = try await request(
            "admin/notifications/test", method: "POST",
            body: body(SavedNotificationTestBody(channelId: channelId)))
        return try JSONDecoder().decode(NotificationTestResponse.self, from: data)
    }

    @discardableResult
    public func setChannelCommands(
        channelId: String, allowCommands: Bool
    ) async throws -> NotificationCommandsResponse {
        let data = try await request(
            "admin/notifications/commands", method: "POST",
            body: body(NotificationCommandsBody(
                channelId: channelId, allowCommands: allowCommands)))
        return try JSONDecoder().decode(NotificationCommandsResponse.self, from: data)
    }

    public func notificationsLog(limit: Int = 50) async throws -> NotificationLogResponse {
        try await get(
            "admin/notifications/log",
            query: [URLQueryItem(name: "limit", value: "\(limit)")],
            as: NotificationLogResponse.self)
    }

    public func removeNotification(id: String) async throws {
        _ = try await request(
            "admin/notifications/\(encodedPathComponent(id))", method: "DELETE")
    }

    public func exoConfig() async throws -> ExoConfig {
        try await get("admin/exo", as: ExoConfig.self)
    }

    public func exoStatus() async throws -> ExoStatus {
        try await get("admin/exo/status", as: ExoStatus.self)
    }

    public func exoModels() async throws -> [ExoModel] {
        try await get("admin/exo/models", as: ExoModelsResponse.self).models
    }

    @discardableResult
    public func updateExoConfig(_ config: ExoConfig) async throws -> ExoConfig {
        let data = try await request("admin/exo", method: "PUT", body: body(config))
        return try JSONDecoder().decode(ExoConfig.self, from: data)
    }

    /// The full OpenRouter catalog for the settings picker. Not injected into
    /// harnesses; only the curated exposed subset is.
    public func openRouterCatalog() async throws -> [String] {
        try await get("admin/openrouter/catalog", as: OpenRouterCatalogResponse.self).models
    }

    /// The curated exposed list plus the catalog it is drawn from, for the
    /// two-list transfer picker.
    public func openRouterExposed() async throws -> OpenRouterExposedResponse {
        try await get("admin/openrouter/exposed", as: OpenRouterExposedResponse.self)
    }

    /// Persist the curated exposed list. Only these models reach `/v1/models`
    /// and connected harnesses. Returns the normalized, sorted list.
    @discardableResult
    public func updateOpenRouterExposed(_ exposed: [String]) async throws -> [String] {
        let data = try await request(
            "admin/openrouter/exposed", method: "POST",
            body: body(OpenRouterExposedBody(exposed: exposed)))
        return try JSONDecoder().decode(OpenRouterExposedResponse.self, from: data).exposed
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
                URLQueryItem(name: "key_fingerprint", value: filters.key),
                URLQueryItem(name: "effort", value: filters.effort),
                URLQueryItem(name: "error_class", value: filters.errorClass),
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

    @discardableResult
    public func setOpenRouterKey(
        _ key: String, displayName: String? = nil,
        httpReferer: String? = nil, xTitle: String? = nil
    ) async throws -> String {
        let data = try await request(
            "admin/auth/openrouter-key",
            method: "POST",
            body: body(OpenRouterKeyBody(
                key: key, displayName: displayName, httpReferer: httpReferer,
                xTitle: xTitle, remove: nil)))
        return try JSONDecoder().decode(OpenRouterKeyResponse.self, from: data).saved
    }

    public func removeOpenRouterKey() async throws {
        _ = try await request(
            "admin/auth/openrouter-key",
            method: "POST",
            body: body(OpenRouterKeyBody(
                key: nil, displayName: nil, httpReferer: nil, xTitle: nil, remove: true)))
    }

    @discardableResult
    public func connectCLIProxyAPI(
        url: String, credential: String
    ) async throws -> CLIProxyAPIConnectResponse {
        let data = try await request(
            "admin/auth/cliproxyapi", method: "POST",
            body: body(CLIProxyAPIConnectBody(url: url, credential: credential)))
        return try JSONDecoder().decode(CLIProxyAPIConnectResponse.self, from: data)
    }

    public func cliProxyAPIStatus() async throws -> CLIProxyAPIStatusResponse {
        try await get("admin/cliproxyapi", as: CLIProxyAPIStatusResponse.self)
    }
}
