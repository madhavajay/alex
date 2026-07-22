/// Pure account selection shared by the menu-card entry point and its tests.
///
/// This deliberately delegates to `Account.displayState` with the latest
/// heartbeat, matching the Providers pane's user-facing state derivation.
public enum Reauthentication {
    public static func accountsNeedingReauthentication(
        _ accounts: [Account], healthAccounts: [HealthAccount]
    ) -> [Account] {
        let heartbeats = Dictionary(
            healthAccounts.compactMap { health in
                health.lastHeartbeat.map { (health.id, $0) }
            },
            uniquingKeysWith: { _, latest in latest })

        return accounts.filter { account in
            let heartbeat = heartbeats[account.id]
            return account.displayState(
                lastPingOK: heartbeat?.ok,
                lastPingStatus: heartbeat?.status) == .needsReauth
        }
    }
}
