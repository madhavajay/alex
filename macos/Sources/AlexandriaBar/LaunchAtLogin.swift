import Foundation
import ServiceManagement

@MainActor
enum LaunchAtLogin {
    static var available: Bool {
        Bundle.main.bundleURL.pathExtension == "app"
    }

    static var isEnabled: Bool {
        SMAppService.mainApp.status == .enabled
    }

    static func toggle() {
        guard available else { return }
        do {
            if isEnabled {
                try SMAppService.mainApp.unregister()
            } else {
                try SMAppService.mainApp.register()
            }
        } catch {
            NSLog("LaunchAtLogin toggle failed: \(error)")
        }
    }
}
