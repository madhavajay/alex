import Foundation

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

    public enum ClientError: Error, LocalizedError {
        case http(Int, String)
        public var errorDescription: String? {
            switch self {
            case let .http(code, body): "HTTP \(code): \(body.prefix(200))"
            }
        }
    }

    private func request(
        _ path: String, method: String = "GET", body: [String: String]? = nil
    ) async throws -> Data {
        var req = URLRequest(url: config.baseURL.appendingPathComponent(path))
        req.httpMethod = method
        req.setValue(config.localKey, forHTTPHeaderField: "x-api-key")
        if let body {
            req.setValue("application/json", forHTTPHeaderField: "content-type")
            req.httpBody = try JSONEncoder().encode(body)
        }
        let (data, resp) = try await session.data(for: req)
        if let http = resp as? HTTPURLResponse, http.statusCode >= 400 {
            throw ClientError.http(http.statusCode, String(data: data, encoding: .utf8) ?? "")
        }
        return data
    }

    private func get<T: Decodable>(_ path: String, as type: T.Type) async throws -> T {
        try JSONDecoder().decode(T.self, from: try await request(path))
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
        try await get("admin/analytics?since_minutes=\(sinceMinutes)", as: Analytics.self)
    }

    public func dario() async throws -> DarioStatus? {
        do {
            return try await get("admin/dario", as: DarioStatus.self)
        } catch ClientError.http(404, _) {
            return nil
        }
    }

    public func darioRestart() async throws {
        _ = try await request("admin/dario/restart", method: "POST")
    }

    public func darioUpdate() async throws {
        _ = try await request("admin/dario/update", method: "POST")
    }

    public func authLoginStart(provider: String) async throws -> LoginSession {
        let data = try await request(
            "admin/auth/login/start", method: "POST", body: ["provider": provider])
        return try JSONDecoder().decode(LoginSession.self, from: data)
    }

    public func authLoginStatus(id: String) async throws -> LoginSession {
        try await get("admin/auth/login/\(id)", as: LoginSession.self)
    }

    public func authLoginComplete(id: String, input: String) async throws -> LoginSession {
        let data = try await request(
            "admin/auth/login/complete", method: "POST",
            body: ["login_id": id, "input": input])
        return try JSONDecoder().decode(LoginSession.self, from: data)
    }

    public func authImport(source: String = "all") async throws -> [ImportOutcome] {
        let data = try await request(
            "admin/auth/import", method: "POST", body: ["source": source])
        return try JSONDecoder().decode(ImportOutcomes.self, from: data).outcomes
    }
}
