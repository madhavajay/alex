import Foundation
import Sparkle

@MainActor
final class UpdaterController {
    private let updaterController: SPUStandardUpdaterController?

    init() {
        guard Bundle.main.object(forInfoDictionaryKey: "SUFeedURL") != nil else {
            updaterController = nil
            return
        }
        updaterController = SPUStandardUpdaterController(
            startingUpdater: true,
            updaterDelegate: nil,
            userDriverDelegate: nil)
        if let updater = updaterController?.updater {
            updater.automaticallyChecksForUpdates = true
            updater.automaticallyDownloadsUpdates = true
        }
    }

    var isAvailable: Bool {
        updaterController != nil
    }

    var canCheckForUpdates: Bool {
        updaterController?.updater.canCheckForUpdates ?? false
    }

    func checkForUpdates() {
        updaterController?.updater.checkForUpdates()
    }
}
