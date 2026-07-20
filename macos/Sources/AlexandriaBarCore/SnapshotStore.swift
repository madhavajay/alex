import Foundation
import Observation

public struct StoreAlert: Sendable, Identifiable, Equatable {
    public enum Severity: Int, Sendable, Comparable {
        case warning = 1
        case critical = 2
        public static func < (lhs: Severity, rhs: Severity) -> Bool { lhs.rawValue < rhs.rawValue }
    }

    public let id: String
    public let severity: Severity
    public let title: String
    public let body: String
    public let provider: String?

    public init(id: String, severity: Severity, title: String, body: String, provider: String? = nil) {
        self.id = id
        self.severity = severity
        self.title = title
        self.body = body
        self.provider = provider
    }
}

@MainActor
@Observable
public final class SnapshotStore {
    public private(set) var config: DaemonConfig?
    public private(set) var daemonUp = false
    public private(set) var health: DaemonHealth?
    public private(set) var accounts: [Account] = []
    public private(set) var healthAccounts: [HealthAccount] = []
    public private(set) var limits: [ProviderLimits] = []
    public private(set) var providerPauses: [ProviderPause] = []
    public private(set) var analytics: Analytics?
    public private(set) var accountAnalytics: AccountAnalyticsResponse?
    public private(set) var codexRouting: CodexRoutingResponse?
    public private(set) var routingByProvider: [String: ProviderRoutingResponse] = [:]
    public private(set) var exoConfig: ExoConfig?
    public private(set) var exoStatus: ExoStatus?
    public private(set) var exoModels: [ExoModel] = []
    public private(set) var dario: DarioStatus?
    public private(set) var daemonUpdate: DaemonUpdateStatus?
    public private(set) var harnesses: [Harness] = []
    public private(set) var credentials: CredentialsResponse?
    public private(set) var harnessesSupported: Bool?
    public private(set) var harnessesCheckedMs: Int64?
    public private(set) var harnessesChecking = false
    /// The daemon does not currently list active login sessions. Retain IDs
    /// returned to this app so provider detail can resume/status/complete them.
    public private(set) var pendingLoginSessions: [String: LoginSession] = [:]
    public private(set) var recentSessions: [TraceSession] = []
    public private(set) var alerts: [StoreAlert] = []
    public private(set) var lastRefresh: Date?
    public private(set) var lastError: String?
    public private(set) var refreshing = false
    public private(set) var nodePath: String?

    public var onRefresh: (@MainActor () -> Void)?
    public var onWindowReset: (@MainActor (_ provider: String, _ window: String) -> Void)?

    private var pollTask: Task<Void, Never>?
    private var boundaryTask: Task<Void, Never>?
    private var attemptedBoundaries: Set<Int64> = []
    private var accountAnalyticsSinceMinutes = 24 * 60
    private var accountAnalyticsBucketMinutes = 60
    private var accountAnalyticsRequestGeneration = 0

    public init() {}

    public func pendingLoginSession(accountId: String, provider: String) -> LoginSession? {
        pendingLoginSessions[accountId] ?? pendingLoginSessions["provider:\(provider)"]
    }

    public func rememberLoginSession(
        _ session: LoginSession, accountId: String? = nil, provider: String? = nil
    ) {
        let providerKey = "provider:\(provider ?? session.provider)"
        let key = accountId ?? session.accountId ?? providerKey
        if session.isPending {
            pendingLoginSessions[key] = session
            if key != providerKey { pendingLoginSessions.removeValue(forKey: providerKey) }
        } else {
            pendingLoginSessions.removeValue(forKey: key)
            pendingLoginSessions.removeValue(forKey: providerKey)
        }
    }

    public var limitWarnPct: Double {
        let v = UserDefaults.standard.double(forKey: "limitWarnPct")
        return v > 0 ? v : 90
    }

    public var refreshSeconds: TimeInterval {
        let v = UserDefaults.standard.double(forKey: "refreshSeconds")
        return v >= 10 ? v : 60
    }

    public func startPolling() {
        pollTask?.cancel()
        pollTask = Task { [weak self] in
            while !Task.isCancelled {
                guard let self else { return }
                await self.refresh()
                try? await Task.sleep(for: .seconds(self.refreshSeconds))
            }
        }
    }

    public func stopPolling() {
        pollTask?.cancel()
        pollTask = nil
        boundaryTask?.cancel()
        boundaryTask = nil
    }

    public func refresh() async {
        if refreshing { return }
        refreshing = true
        defer {
            refreshing = false
            lastRefresh = Date()
            alerts = deriveAlerts()
            onRefresh?()
        }

        guard let cfg = DaemonDiscovery.load() else {
            config = nil
            daemonUp = false
            harnesses = []
            credentials = nil
            harnessesSupported = nil
            recentSessions = []
            daemonUpdate = nil
            codexRouting = nil
            routingByProvider = [:]
            exoConfig = nil
            exoStatus = nil
            exoModels = []
            providerPauses = []
            lastError = "no config at ~/.alexandria/config.toml"
            return
        }
        config = cfg
        nodePath = NodeDetection.findNode()
        let client = AlexandriaClient(config: cfg)

        do {
            health = try await client.health()
            daemonUp = true
            lastError = nil
        } catch {
            daemonUp = false
            health = nil
            harnesses = []
            credentials = nil
            harnessesSupported = nil
            recentSessions = []
            daemonUpdate = nil
            codexRouting = nil
            routingByProvider = [:]
            exoConfig = nil
            exoStatus = nil
            exoModels = []
            providerPauses = []
            lastError = error.localizedDescription
            return
        }

        let accountAnalyticsSinceMinutes = self.accountAnalyticsSinceMinutes
        let accountAnalyticsBucketMinutes = self.accountAnalyticsBucketMinutes
        let accountAnalyticsRequestGeneration = self.accountAnalyticsRequestGeneration

        async let accountsR = try? client.accounts()
        async let healthR = try? client.accountHealth()
        async let limitsR = try? client.limits()
        async let providerPausesR = try? client.providerPauses()
        async let analyticsR = try? client.analytics(sinceMinutes: 60)
        async let accountAnalyticsR = try? client.accountAnalytics(
            sinceMinutes: accountAnalyticsSinceMinutes,
            bucketMinutes: accountAnalyticsBucketMinutes)
        async let darioR = Self.fetchDario(using: client)
        async let daemonUpdateR = try? client.daemonUpdateStatus()
        async let harnessesR = client.harnessSnapshot()
        async let credentialsR = try? client.credentials()
        async let recentSessionsR = try? client.traceSessions(since: "24h", limit: 12)
        async let exoConfigR = try? client.exoConfig()
        async let exoStatusR = try? client.exoStatus()
        async let exoModelsR = try? client.exoModels()

        accounts = await accountsR ?? []
        healthAccounts = await healthR ?? []
        let oldLimits = limits
        limits = await limitsR ?? []
        providerPauses = await providerPausesR ?? []
        analytics = await analyticsR
        if accountAnalyticsRequestGeneration == self.accountAnalyticsRequestGeneration,
           let fetchedAccountAnalytics = await accountAnalyticsR
        {
            accountAnalytics = fetchedAccountAnalytics
        }
        let providerIDs = Set(accounts.map(\.provider)).union(ProviderInfo.supportedProviders)
        let routings = await withTaskGroup(
            of: (String, ProviderRoutingResponse?).self,
            returning: [String: ProviderRoutingResponse].self
        ) { group in
            for provider in providerIDs {
                group.addTask { (provider, try? await client.routing(provider: provider)) }
            }
            var values: [String: ProviderRoutingResponse] = [:]
            for await (provider, routing) in group {
                if let routing { values[provider] = routing }
            }
            return values
        }
        routingByProvider = routings
        codexRouting = routings["openai"]
        exoConfig = await exoConfigR
        exoStatus = await exoStatusR
        exoModels = await exoModelsR ?? []
        if let fetchedCredentials = await credentialsR {
            credentials = fetchedCredentials
        }
        switch await darioR {
        case .fetched(let fetched):
            // A successful nil is the endpoint's explicit 404/disabled signal.
            dario = fetched
        case .failed:
            // A rolling restart or other transient error must not make Dario
            // disappear. Retain the last-known status; an enabled menu with no
            // prior status is rendered as down by DarioHealth.
            break
        }
        daemonUpdate = await daemonUpdateR
        recentSessions = Array(
            (await recentSessionsR ?? [])
                .filter { !$0.isPingOrTest }
                .sorted { $0.lastTsMs > $1.lastTsMs }
                .prefix(4))
        do {
            if let fetched = try await harnessesR {
                harnesses = fetched.harnesses
                harnessesCheckedMs = fetched.checkedMs
                harnessesSupported = true
            } else {
                harnesses = []
                harnessesCheckedMs = nil
                harnessesSupported = false
            }
        } catch {
            if harnesses.isEmpty {
                harnessesSupported = nil
            }
        }
        detectWindowResets(old: oldLimits, new: limits)
        scheduleBoundaryRefresh()
    }

    /// Makes a focused credentials-pane refresh available to the Harnesses
    /// pane immediately, without waiting for the next full polling interval.
    public func rememberCredentials(_ credentials: CredentialsResponse) {
        self.credentials = credentials
        onRefresh?()
    }

    /// Reload just the harness snapshot after a harness mutation. Wait for an
    /// in-flight full poll so its older response cannot overwrite this result.
    public func refreshHarnesses(using client: AlexandriaClient) async {
        while refreshing {
            guard !Task.isCancelled else { return }
            try? await Task.sleep(for: .milliseconds(10))
        }
        harnessesChecking = true
        defer { harnessesChecking = false }
        do {
            if let fetched = try await client.harnessSnapshot(refresh: true) {
                harnesses = fetched.harnesses
                harnessesCheckedMs = fetched.checkedMs
                harnessesSupported = true
            } else {
                harnesses = []
                harnessesCheckedMs = nil
                harnessesSupported = false
            }
        } catch {
            if harnesses.isEmpty {
                harnessesSupported = nil
            }
        }
        onRefresh?()
    }

    public var harnessesAreStale: Bool {
        guard let harnessesCheckedMs else { return false }
        let nowMs = Int64(Date().timeIntervalSince1970 * 1_000)
        return nowMs - harnessesCheckedMs > 60_000
    }

    /// Re-probe stale harness metadata while retaining the last rows on screen.
    public func refreshHarnessesIfStale() async {
        guard harnessesAreStale, !harnessesChecking, let config else { return }
        harnessesChecking = true
        defer {
            harnessesChecking = false
            onRefresh?()
        }
        do {
            if let fetched = try await AlexandriaClient(config: config).harnessSnapshot(refresh: true) {
                harnesses = fetched.harnesses
                harnessesCheckedMs = fetched.checkedMs
                harnessesSupported = true
            }
        } catch {
            // The cached rows are still useful; the next pane open or poll can retry.
        }
    }

    /// Refreshes only the per-account chart data and leaves the rest of the
    /// snapshot untouched. The selected range is retained for later polling
    /// refreshes so a full snapshot cannot silently switch the chart to 24h.
    public func refreshAccountAnalytics(
        sinceMinutes: Int = 24 * 60,
        bucketMinutes: Int = 60
    ) async {
        accountAnalyticsSinceMinutes = sinceMinutes
        accountAnalyticsBucketMinutes = bucketMinutes
        accountAnalyticsRequestGeneration += 1
        let requestGeneration = accountAnalyticsRequestGeneration

        guard let config else { return }
        let client = AlexandriaClient(config: config)
        do {
            let fetched = try await client.accountAnalytics(
                sinceMinutes: sinceMinutes,
                bucketMinutes: bucketMinutes)
            guard !Task.isCancelled,
                  requestGeneration == accountAnalyticsRequestGeneration
            else { return }
            accountAnalytics = fetched
        } catch {
            // Keep the prior chart visible on cancellation or a transient
            // range-fetch failure. The client already records network errors.
        }
    }

    private func detectWindowResets(old: [ProviderLimits], new: [ProviderLimits]) {
        for provider in new {
            guard let oldProvider = old.first(where: { $0.provider == provider.provider }) else {
                continue
            }
            for window in provider.windows ?? [] {
                guard let oldWindow = oldProvider.windows?.first(where: { $0.window == window.window }),
                      let oldPct = oldWindow.usedPct, let newPct = window.usedPct,
                      oldPct >= 30, newPct <= 10
                else { continue }
                onWindowReset?(provider.provider, window.window)
            }
        }
    }

    private func scheduleBoundaryRefresh() {
        boundaryTask?.cancel()
        let now = Date()
        let upcoming = limits
            .flatMap { $0.windows ?? [] }
            .compactMap(\.resetsDate)
            .filter { $0 > now }
        guard let next = upcoming.min() else { return }
        let key = Int64(next.timeIntervalSince1970)
        guard !attemptedBoundaries.contains(key) else { return }
        if attemptedBoundaries.count > 64 { attemptedBoundaries.removeAll() }
        attemptedBoundaries.insert(key)
        let delay = next.timeIntervalSince(now) + 30
        boundaryTask = Task { [weak self] in
            try? await Task.sleep(for: .seconds(delay))
            guard !Task.isCancelled else { return }
            await self?.refresh()
        }
    }

    public var worstSeverity: StoreAlert.Severity? {
        alerts.map(\.severity).max()
    }

    private func deriveAlerts() -> [StoreAlert] {
        var out: [StoreAlert] = []

        if config == nil {
            out.append(StoreAlert(
                id: "no-config", severity: .critical,
                title: "Alex not configured",
                body: "No config found at ~/.alexandria/config.toml"))
            return out
        }
        if config?.darioEnabled == true, nodePath == nil {
            out.append(StoreAlert(
                id: "node-missing", severity: .critical,
                title: "Node.js not found — dario needs it",
                body: "Dario mode runs the dario npm package. Install Node first (e.g. brew install node)."))
        }

        if !daemonUp {
            out.append(StoreAlert(
                id: "daemon-down", severity: .critical,
                title: "Alex daemon is down",
                body: lastError ?? "Health check failed"))
            return out
        }

        let outOfCreditsProviders = Set(limits.compactMap { provider in
            provider.quota?.kind == "out_of_credits" ? provider.provider : nil
        })
        out += Self.authAndHealthAlerts(
            accounts: accounts,
            healthAccounts: healthAccounts,
            suppressHeartbeatProviders: outOfCreditsProviders)

        for provider in limits {
            if provider.quota?.kind == "out_of_credits" {
                let topUp = provider.quota?.topUpURL.map { " Top up: \($0)" } ?? ""
                out.append(StoreAlert(
                    id: "credits-\(provider.provider)", severity: .critical,
                    title: "\(ProviderInfo.displayName(provider.provider)) out of credits",
                    body: "This account cannot serve requests.\(topUp)", provider: provider.provider))
                continue
            }
            for window in provider.windows ?? [] {
                if provider.quota?.isCreditPrimary == true { continue }
                // An expired window is a stale snapshot awaiting daemon
                // refresh, not a live 0% — never alert on it.
                if window.resetHasPassed() { continue }
                if let pct = window.usedPct, pct >= limitWarnPct {
                    let resets = window.resetsDate.map { " · resets in \(Format.countdown(to: $0))" } ?? ""
                    out.append(StoreAlert(
                        id: "limit-\(provider.provider)-\(window.window)",
                        severity: pct >= 100 ? .critical : .warning,
                        title: "\(ProviderInfo.displayName(provider.provider)) \(window.window) window at \(Int(pct))%",
                        body: "Plan \(provider.plan ?? "?")\(resets)", provider: provider.provider))
                }
            }
        }

        if config?.darioEnabled == true || dario != nil {
            let evaluation = DarioHealth.evaluate(dario)
            if evaluation.state != .ready {
                let active = dario.flatMap { status in
                    status.generations.first { $0.id == status.activeGenerationId }
                        ?? status.generations.first
                }
                let body = dario?.issue?.message
                    ?? active?.lastProbe?.error
                    ?? active.map { "\($0.id) (v\($0.version))" }
                    ?? "Dario status is unavailable"
                out.append(StoreAlert(
                    id: "dario-health",
                    severity: evaluation.state == .down ? .critical : .warning,
                    title: "Dario \(evaluation.label)",
                    body: body))
            }
        }

        return out
    }

    private enum DarioRefreshResult: Sendable {
        case fetched(DarioStatus?)
        case failed
    }

    nonisolated private static func fetchDario(
        using client: AlexandriaClient
    ) async -> DarioRefreshResult {
        do {
            return .fetched(try await client.dario())
        } catch {
            return .failed
        }
    }

    /// Account IDs are shared by `/admin/accounts` and `/admin/health`; merge auth and
    /// heartbeat evidence for one account instead of presenting two symptoms of one failure.
    static func authAndHealthAlerts(
        accounts: [Account], healthAccounts: [HealthAccount],
        suppressHeartbeatProviders: Set<String> = []
    ) -> [StoreAlert] {
        let failedHeartbeatIDs = Set(healthAccounts.compactMap { account in
            account.lastHeartbeat?.ok == false ? account.id : nil
        })
        var authAlertIDs: Set<String> = []
        var alerts: [StoreAlert] = []

        for account in accounts {
            let heartbeatFailed = failedHeartbeatIDs.contains(account.id)
            let name = ProviderInfo.displayName(account.provider)
            if account.status != "active" {
                authAlertIDs.insert(account.id)
                alerts.append(StoreAlert(
                    id: "acct-\(account.id)-status", severity: .critical,
                    title: "\(name) account \(account.status)",
                    body: heartbeatFailed
                        ? "Requests are failing — re-authentication is required."
                        : "\(account.id) needs re-auth",
                    provider: account.provider))
            } else if account.isExpired, account.kind == "oauth" {
                authAlertIDs.insert(account.id)
                let hint: String
                if heartbeatFailed {
                    hint = account.provider == "xai"
                        ? "Requests are failing — re-authentication is required. Run the grok CLI to refresh, then re-import."
                        : "Requests are failing — re-authentication is required."
                } else {
                    hint = account.provider == "xai"
                        ? "Run the grok CLI to refresh, then re-import"
                        : "Token expired — re-auth if requests fail"
                }
                alerts.append(StoreAlert(
                    id: "acct-\(account.id)-expired",
                    severity: heartbeatFailed ? .critical : .warning,
                    title: "\(name) token expired",
                    body: hint, provider: account.provider))
            }
        }

        for account in healthAccounts {
            guard !authAlertIDs.contains(account.id),
                  !suppressHeartbeatProviders.contains(account.provider),
                  let hb = account.lastHeartbeat, !hb.ok
            else { continue }
            alerts.append(StoreAlert(
                id: "hb-\(account.id)", severity: .critical,
                title: "\(ProviderInfo.displayName(account.provider)) failing health checks",
                body: hb.message ?? "heartbeat failed (status \(hb.status.map(String.init) ?? "?"))",
                provider: account.provider))
        }
        return alerts
    }
}
