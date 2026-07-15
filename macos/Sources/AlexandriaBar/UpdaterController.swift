import Combine
import Foundation
import Sparkle
import AlexandriaBarCore

/// Swaps the Sparkle feed to the beta appcast when the user opts into the
/// beta channel. Returning nil keeps the SUFeedURL baked into Info.plist,
/// so switching back to stable always works.
///
/// Also mirrors Sparkle's found/not-found delegate callbacks into
/// `UpdaterController.availableAppUpdateVersion` so the menu banner can show
/// an "App" row. This is purely observational: `SPUBasicUpdateDriver` (the
/// shared base for the automatic, UI-based, and probing drivers alike) calls
/// `updater(_:didFindValidUpdate:)` / `updaterDidNotFindUpdate` for *every*
/// update check — scheduled or user-initiated — before any driver-specific
/// UI happens, so recording state here never suppresses or duplicates
/// Sparkle's own flow. And because `automaticallyDownloadsUpdates = true`
/// below selects `SPUAutomaticUpdateDriver`, Sparkle already skips its own
/// "update found" alert on scheduled background checks (it downloads
/// silently and only prompts once ready to relaunch); a user-initiated check
/// via `checkForUpdates()` still uses the UI-based driver and shows Sparkle's
/// normal "update found" dialog. Verified in the vendored Sparkle 2.9 sources
/// under .build/checkouts/Sparkle (SPUBasicUpdateDriver.m calls the delegate
/// methods; SPUAutomaticUpdateDriver.m / SPUUpdater.h document that
/// automaticallyDownloadsUpdates skips the found-update prompt for
/// background checks).
final class ChannelFeedDelegate: NSObject, SPUUpdaterDelegate {
    weak var updaterController: UpdaterController?

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

    func updater(_ updater: SPUUpdater, didFindValidUpdate item: SUAppcastItem) {
        BarLog.info(.ui, "sparkle didFindValidUpdate: \(item.displayVersionString)")
        MainActor.assumeIsolated {
            updaterController?.recordAppUpdate(version: item.displayVersionString)
        }
    }

    func updaterDidNotFindUpdate(_ updater: SPUUpdater, error: Error) {
        BarLog.info(.ui, "sparkle updaterDidNotFindUpdate: \(error.localizedDescription)")
        MainActor.assumeIsolated {
            updaterController?.recordAppUpdate(version: nil)
        }
    }
}

@MainActor
final class UpdaterController: ObservableObject {
    /// Version string of a Sparkle update Sparkle has confirmed is available
    /// (mirrors `ChannelFeedDelegate.updater(_:didFindValidUpdate:)`), or nil
    /// once Sparkle reports no update / the update installs. Purely a mirror
    /// of Sparkle's own state — never gates or drives Sparkle's own UI.
    @Published private(set) var availableAppUpdateVersion: String? {
        didSet {
            guard availableAppUpdateVersion != oldValue else { return }
            onAppUpdateStateChanged?()
        }
    }

    /// Fired on the main actor whenever `availableAppUpdateVersion` changes,
    /// so an imperative NSMenu owner (StatusItemController) can rebuild
    /// without needing SwiftUI's Combine observation machinery.
    var onAppUpdateStateChanged: (() -> Void)?

    private let updaterController: SPUStandardUpdaterController?
    private let feedDelegate = ChannelFeedDelegate()
    private var channelObserver: NSObjectProtocol?

    init() {
        guard Bundle.main.object(forInfoDictionaryKey: "SUFeedURL") != nil else {
            updaterController = nil
            applyDebugFakeUpdateOverride()
            return
        }
        updaterController = SPUStandardUpdaterController(
            startingUpdater: true,
            updaterDelegate: feedDelegate,
            userDriverDelegate: nil)
        feedDelegate.updaterController = self
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
        applyDebugFakeUpdateOverride()
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

    fileprivate func recordAppUpdate(version: String?) {
        availableAppUpdateVersion = version
    }

    /// Debug-only manual preview hook (see MenuUpdateBannerView doc) so the
    /// orange banner can be exercised without a real Sparkle/daemon release.
    /// `defaults write com.madhavajay.alexandria-macos DebugFakeUpdateBanner app|daemon|both`
    /// injects a fake pending app-update version; `defaults delete
    /// com.madhavajay.alexandria-macos DebugFakeUpdateBanner` clears it. No
    /// effect when the key is unset.
    private func applyDebugFakeUpdateOverride() {
        guard let mode = UserDefaults.standard.string(forKey: Self.debugFakeUpdateBannerKey) else {
            return
        }
        if mode == "app" || mode == "both" {
            availableAppUpdateVersion = "99.0.0-debug"
        }
    }

    static let debugFakeUpdateBannerKey = "DebugFakeUpdateBanner"
}
