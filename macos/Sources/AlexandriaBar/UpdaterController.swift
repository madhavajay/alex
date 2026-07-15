import Foundation
import Sparkle
import AlexandriaBarCore

/// Swaps the Sparkle feed to the beta appcast when the user opts into the
/// beta channel. Returning nil keeps the SUFeedURL baked into Info.plist,
/// so switching back to stable always works.
final class ChannelFeedDelegate: NSObject, SPUUpdaterDelegate {
    func feedURLString(for updater: SPUUpdater) -> String? {
        let rawChannel = UserDefaults.standard.string(forKey: UpdateChannelSetting.defaultsKey)
        let channel = UpdateChannelSetting.from(rawChannel)
        let stableFeed = Bundle.main.object(forInfoDictionaryKey: "SUFeedURL") as? String
        let resolved = stableFeed.flatMap { channel.feedURLString(stableFeed: $0) }
        // Diagnostic (Reveal Log File): this proves whether Sparkle consults the
        // delegate and which feed it is handed. If this line appears with a beta
        // URL but the check still finds nothing, the fault is downstream in Sparkle.
        BarLog.info(
            .ui,
            "sparkle feedURL: channel=\(rawChannel ?? "nil")->\(channel.rawValue) "
                + "baked=\(stableFeed ?? "nil") resolved=\(resolved ?? "nil(uses baked)")")
        guard stableFeed != nil else { return nil }
        return resolved
    }
}

@MainActor
final class UpdaterController {
    private let updaterController: SPUStandardUpdaterController?
    private let feedDelegate = ChannelFeedDelegate()
    private var channelObserver: NSObjectProtocol?

    init() {
        guard Bundle.main.object(forInfoDictionaryKey: "SUFeedURL") != nil else {
            updaterController = nil
            return
        }
        updaterController = SPUStandardUpdaterController(
            startingUpdater: true,
            updaterDelegate: feedDelegate,
            userDriverDelegate: nil)
        if let updater = updaterController?.updater {
            updater.automaticallyChecksForUpdates = true
            updater.automaticallyDownloadsUpdates = true
        }
        channelObserver = NotificationCenter.default.addObserver(
            forName: UpdateChannelSetting.changedNotification,
            object: nil,
            queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated {
                self?.updaterController?.updater.checkForUpdatesInBackground()
            }
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
