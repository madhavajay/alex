import AppKit
import Combine
import Foundation
import Sparkle
import AlexCore

/// Bridges the shared `AlexVersion` comparator (a 1:1 port of the daemon's
/// Rust ordering) into Sparkle's `SUVersionComparison` protocol, so Sparkle
/// orders our `-beta.N`/`-rc.N` versions identically to `alex update` (B4).
final class AlexVersionComparator: NSObject, SUVersionComparison {
    func compareVersion(_ versionA: String, toVersion versionB: String) -> ComparisonResult {
        AlexVersion.compare(versionA, versionB)
    }
}

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
        // B2: a pre-release build with no explicit channel choice must resolve
        // to beta, so refresh checks the beta appcast instead of the older
        // latest stable and falsely reporting "up to date".
        let runningVersion =
            Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? ""
        let channel = UpdateChannelSetting.resolved(
            rawStored: rawChannel, runningVersion: runningVersion)
        let stableFeed = Bundle.main.object(forInfoDictionaryKey: "SUFeedURL") as? String
        let resolved = stableFeed.flatMap { channel.feedURLString(stableFeed: $0) }
        // Diagnostic (Reveal Log File): this proves whether Sparkle consults the
        // delegate and which feed it is handed. If this line appears with a beta
        // URL but the check still finds nothing, the fault is downstream in Sparkle.
        guard let stableFeed else {
            BarLog.info(.ui, "sparkle feedURL: no baked SUFeedURL; updates disabled")
            return nil
        }
        // Cache-bust the feed on EVERY check. The appcast is served from GitHub
        // Pages through a CDN with `max-age=600` across geographic edges, and the
        // host honours conditional GETs — so a freshly published build can stay
        // invisible for 10+ minutes (longer on a stale edge), which is exactly
        // the "update is flaky / still says I'm on an old build" symptom. A
        // unique query param forces a cache MISS every time (it is part of the
        // CDN cache key but ignored by the static-file host), so Sparkle always
        // sees the current appcast. Applies to both channels (resolved==beta URL,
        // or the baked stable URL) so neither goes stale.
        let effective = Self.cacheBusted(resolved ?? stableFeed)
        BarLog.info(
            .ui,
            "sparkle feedURL: channel=\(rawChannel ?? "nil")->\(channel.rawValue) "
                + "baked=\(stableFeed) resolved=\(resolved ?? "nil(uses baked)") effective=\(effective)")
        return effective
    }

    /// B4: hand Sparkle a version comparator that matches the daemon's Rust
    /// ordering exactly, instead of relying on `SUStandardVersionComparator`
    /// (whose handling of our `-beta.N`/`-rc.N` SemVer scheme is not
    /// guaranteed to agree). `AlexVersion` is a 1:1 port of the Rust
    /// comparator, so `alex update` and Sparkle can never disagree about which
    /// build is newer.
    func versionComparator(for updater: SPUUpdater) -> (any SUVersionComparison)? {
        AlexVersionComparator()
    }

    /// Appends a unique `cb` query parameter so the CDN cannot serve a stale
    /// cached appcast (or answer a conditional GET with 304). The value is a
    /// millisecond timestamp — unique per check, ignored by the static host.
    static func cacheBusted(_ urlString: String) -> String {
        guard var components = URLComponents(string: urlString) else { return urlString }
        var items = components.queryItems ?? []
        items.append(URLQueryItem(name: "cb", value: String(Int(Date().timeIntervalSince1970 * 1000))))
        components.queryItems = items
        return components.string ?? urlString
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

    /// Any aborted update cycle (bad signature, download failure, failed
    /// validation, …) gets a curl-installer escape hatch. Sparkle's own alert
    /// dead-ends at "try again later"; a locally-signed or half-published
    /// build can stay broken indefinitely, so always offer the force path.
    func updater(_ updater: SPUUpdater, didAbortWithError error: Error) {
        let nsError = error as NSError
        BarLog.warn(
            .ui,
            "sparkle didAbortWithError: domain=\(nsError.domain) code=\(nsError.code) \(nsError.localizedDescription)")
        // 1001 = no update available, 4007/4008 = user canceled or deferred —
        // normal outcomes, not failures.
        guard nsError.domain == SUSparkleErrorDomain,
            ![1001, 4007, 4008].contains(nsError.code)
        else { return }
        MainActor.assumeIsolated {
            updaterController?.offerForceInstall(afterError: nsError)
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

    /// The channel-appropriate bootstrap installer: it reinstalls the CLI,
    /// re-pins the daemon, and replaces the app with the latest published
    /// build — exactly the recovery for a Sparkle update Sparkle itself
    /// cannot validate or install.
    static func forceInstallCommand() -> String {
        let rawChannel = UserDefaults.standard.string(forKey: UpdateChannelSetting.defaultsKey)
        let runningVersion =
            Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? ""
        let channel = UpdateChannelSetting.resolved(
            rawStored: rawChannel, runningVersion: runningVersion)
        let script = channel == .beta ? "install-beta.sh" : "install-release.sh"
        return "curl -fsSL https://raw.githubusercontent.com/madhavajay/alex/main/\(script) | sh"
    }

    /// Follow-up alert after Sparkle aborts: explain the failure and offer to
    /// run the curl installer in the user's terminal, which force-installs the
    /// latest published version regardless of what Sparkle choked on.
    fileprivate func offerForceInstall(afterError error: NSError) {
        let command = Self.forceInstallCommand()
        let alert = NSAlert()
        alert.messageText = "Update could not be installed"
        alert.informativeText =
            "\(error.localizedDescription)\n\nYou can force-install the latest version with the "
            + "official installer instead:\n\n\(command)\n\nIt reinstalls the CLI, daemon, and app, "
            + "then relaunches Alex."
        alert.alertStyle = .warning
        alert.addButton(withTitle: "Force Install in Terminal")
        alert.addButton(withTitle: "Copy Command")
        alert.addButton(withTitle: "Cancel")
        NSApp.activate(ignoringOtherApps: true)
        switch alert.runModal() {
        case .alertFirstButtonReturn:
            TerminalLauncher.launch(command: command)
        case .alertSecondButtonReturn:
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(command, forType: .string)
        default:
            break
        }
    }

    /// Debug-only manual preview hook (see MenuUpdateBannerView doc) so the
    /// orange banner can be exercised without a real Sparkle/daemon release.
    /// `defaults write com.madhavajay.alex DebugFakeUpdateBanner app|daemon|both`
    /// injects a fake pending app-update version; `defaults delete
    /// com.madhavajay.alex DebugFakeUpdateBanner` clears it. No
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
