import Foundation
import UserNotifications
import AlexandriaBarCore

@MainActor
final class AlertNotifier {
    private var notifiedIds: Set<String> = []
    private let enabled: Bool

    init() {
        enabled = Bundle.main.bundleURL.pathExtension == "app"
    }

    func requestAuthorization() {
        guard enabled, UserDefaults.standard.object(forKey: "notifyEnabled") as? Bool ?? true else { return }
        UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound]) {
            @Sendable _, _ in
        }
    }

    func sync(alerts: [StoreAlert]) {
        guard enabled, UserDefaults.standard.object(forKey: "notifyEnabled") as? Bool ?? true else { return }
        let current = Set(alerts.map(\.id))
        notifiedIds.subtract(notifiedIds.subtracting(current))
        for alert in alerts where !notifiedIds.contains(alert.id) {
            notifiedIds.insert(alert.id)
            post(alert)
        }
    }

    func post(_ alert: StoreAlert) {
        let content = UNMutableNotificationContent()
        content.title = alert.title
        content.body = alert.body
        content.sound = alert.severity == .critical ? .default : nil
        let request = UNNotificationRequest(
            identifier: "alexandria-\(alert.id)", content: content, trigger: nil)
        UNUserNotificationCenter.current().add(request)
    }

    func postInfo(title: String, body: String) {
        guard enabled else { return }
        let content = UNMutableNotificationContent()
        content.title = title
        content.body = body
        let request = UNNotificationRequest(
            identifier: "alexandria-info-\(UUID().uuidString)", content: content, trigger: nil)
        UNUserNotificationCenter.current().add(request)
    }
}
