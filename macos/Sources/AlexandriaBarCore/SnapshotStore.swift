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
    public private(set) var analytics: Analytics?
    public private(set) var dario: DarioStatus?
    public private(set) var daemonUpdate: DaemonUpdateStatus?
    public private(set) var harnesses: [Harness] = []
    public private(set) var harnessesSupported: Bool?
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

    public init() {}

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
            harnessesSupported = nil
            daemonUpdate = nil
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
            harnessesSupported = nil
            daemonUpdate = nil
            lastError = error.localizedDescription
            return
        }

        async let accountsR = try? client.accounts()
        async let healthR = try? client.accountHealth()
        async let limitsR = try? client.limits()
        async let analyticsR = try? client.analytics(sinceMinutes: 60)
        async let darioR = try? client.dario()
        async let daemonUpdateR = try? client.daemonUpdateStatus()
        async let harnessesR = client.harnesses()

        accounts = await accountsR ?? []
        healthAccounts = await healthR ?? []
        let oldLimits = limits
        limits = await limitsR ?? []
        analytics = await analyticsR
        dario = await darioR ?? nil
        daemonUpdate = await daemonUpdateR
        do {
            if let fetched = try await harnessesR {
                harnesses = fetched
                harnessesSupported = true
            } else {
                harnesses = []
                harnessesSupported = false
            }
        } catch {
            harnesses = []
            harnessesSupported = nil
        }
        detectWindowResets(old: oldLimits, new: limits)
        scheduleBoundaryRefresh()
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
                title: "Alexandria not configured",
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
                title: "Alexandria daemon is down",
                body: lastError ?? "Health check failed"))
            return out
        }

        for account in accounts {
            let name = ProviderInfo.displayName(account.provider)
            if account.status != "active" {
                out.append(StoreAlert(
                    id: "acct-\(account.id)-status", severity: .critical,
                    title: "\(name) account \(account.status)",
                    body: "\(account.id) needs re-auth", provider: account.provider))
            } else if account.isExpired, account.kind == "oauth" {
                let hint = account.provider == "xai"
                    ? "Run the grok CLI to refresh, then re-import"
                    : "Token expired — re-auth if requests fail"
                out.append(StoreAlert(
                    id: "acct-\(account.id)-expired", severity: .warning,
                    title: "\(name) token expired",
                    body: hint, provider: account.provider))
            }
        }

        for account in healthAccounts {
            if let hb = account.lastHeartbeat, !hb.ok {
                out.append(StoreAlert(
                    id: "hb-\(account.id)", severity: .critical,
                    title: "\(ProviderInfo.displayName(account.provider)) failing health checks",
                    body: hb.message ?? "heartbeat failed (status \(hb.status.map(String.init) ?? "?"))",
                    provider: account.provider))
            }
        }

        for provider in limits {
            for window in provider.windows ?? [] {
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

        if let dario, let active = dario.generations.first(where: { $0.id == dario.activeGenerationId }) {
            if active.phase != "ready" {
                out.append(StoreAlert(
                    id: "dario-phase", severity: .warning,
                    title: "Dario generation \(active.phase)",
                    body: "\(active.id) (v\(active.version))"))
            } else if let probe = active.lastProbe, !probe.ok {
                out.append(StoreAlert(
                    id: "dario-probe", severity: .warning,
                    title: "Dario probe failing",
                    body: probe.error ?? "probe failed"))
            }
        }

        return out
    }
}
