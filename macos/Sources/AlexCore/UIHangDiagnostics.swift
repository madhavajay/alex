import Foundation

#if canImport(os)
import os
#endif

public enum TraceBrowserSignpost {
    public enum Operation: String, Sendable {
        case turnFetch = "turn fetch"
        case transcriptApply = "transcript apply"
        case transcriptRenderBuild = "TranscriptRender.build"
        case chatPaneUpdate = "chat-pane update"
        case classicPaneUpdate = "classic-pane update"
        case queryChange = "query change"
        case sessionFilter = "session filter"
        case sessionSummary = "session summary"
        case turnFilter = "turn filter"
        case visibleRowsApply = "visible-rows apply"
    }

    public struct Interval: @unchecked Sendable {
        fileprivate let key: UUID
        fileprivate let operation: Operation
        fileprivate let startedAt: ContinuousClock.Instant
        #if canImport(os)
        fileprivate let signpostID: OSSignpostID
        #endif
    }

    public struct ActiveInterval: Equatable, Sendable {
        public let operation: String
        public let metadata: String
        public let elapsedMilliseconds: Double
    }

    public struct Snapshot: Equatable, Sendable {
        public let active: [ActiveInterval]
        public let breadcrumbs: [String]
    }

    public static let breadcrumbLimit = 32

    #if canImport(os)
    private static let log = OSLog(subsystem: "com.alex.bar", category: "TraceBrowser")
    #endif
    private static let state = State()

    public static func begin(_ operation: Operation, _ metadata: String) -> Interval {
        #if canImport(os)
        let interval = Interval(
            key: UUID(), operation: operation, startedAt: ContinuousClock.now,
            signpostID: OSSignpostID(log: log))
        emit(.begin, interval: interval, metadata: metadata)
        #else
        let interval = Interval(
            key: UUID(), operation: operation, startedAt: ContinuousClock.now)
        #endif
        state.begin(interval, metadata: metadata)
        return interval
    }

    public static func end(_ interval: Interval, _ metadata: String = "") {
        let elapsed = milliseconds(interval.startedAt.duration(to: .now))
        let suffix = metadata.isEmpty
            ? "duration_ms=\(String(format: "%.1f", elapsed))"
            : "\(metadata) duration_ms=\(String(format: "%.1f", elapsed))"
        #if canImport(os)
        emit(.end, interval: interval, metadata: suffix)
        #endif
        state.end(interval, metadata: suffix)
    }

    public static func snapshot() -> Snapshot {
        state.snapshot(now: ContinuousClock.now)
    }

    public static func resetForTesting() {
        state.reset()
    }

    private static func milliseconds(_ duration: Duration) -> Double {
        Double(duration.components.seconds) * 1_000
            + Double(duration.components.attoseconds) / 1e15
    }

    #if canImport(os)
    private static func emit(
        _ type: OSSignpostType, interval: Interval, metadata: String
    ) {
        let value = metadata as NSString
        switch interval.operation {
        case .turnFetch:
            os_signpost(
                type, log: log, name: "turn fetch", signpostID: interval.signpostID,
                "%{public}@", value)
        case .transcriptApply:
            os_signpost(
                type, log: log, name: "transcript apply", signpostID: interval.signpostID,
                "%{public}@", value)
        case .transcriptRenderBuild:
            os_signpost(
                type, log: log, name: "TranscriptRender.build", signpostID: interval.signpostID,
                "%{public}@", value)
        case .chatPaneUpdate:
            os_signpost(
                type, log: log, name: "chat-pane update", signpostID: interval.signpostID,
                "%{public}@", value)
        case .classicPaneUpdate:
            os_signpost(
                type, log: log, name: "classic-pane update", signpostID: interval.signpostID,
                "%{public}@", value)
        case .queryChange:
            os_signpost(
                type, log: log, name: "query change", signpostID: interval.signpostID,
                "%{public}@", value)
        case .sessionFilter:
            os_signpost(
                type, log: log, name: "session filter", signpostID: interval.signpostID,
                "%{public}@", value)
        case .sessionSummary:
            os_signpost(
                type, log: log, name: "session summary", signpostID: interval.signpostID,
                "%{public}@", value)
        case .turnFilter:
            os_signpost(
                type, log: log, name: "turn filter", signpostID: interval.signpostID,
                "%{public}@", value)
        case .visibleRowsApply:
            os_signpost(
                type, log: log, name: "visible-rows apply", signpostID: interval.signpostID,
                "%{public}@", value)
        }
    }
    #endif

    private final class State: @unchecked Sendable {
        private struct Active {
            let interval: Interval
            let metadata: String
        }

        private let lock = NSLock()
        private var active: [UUID: Active] = [:]
        private var breadcrumbs: [String] = []

        func begin(_ interval: Interval, metadata: String) {
            lock.lock()
            active[interval.key] = Active(interval: interval, metadata: metadata)
            appendBreadcrumb("BEGIN \(interval.operation.rawValue) \(metadata)")
            lock.unlock()
        }

        func end(_ interval: Interval, metadata: String) {
            lock.lock()
            active.removeValue(forKey: interval.key)
            appendBreadcrumb("END \(interval.operation.rawValue) \(metadata)")
            lock.unlock()
        }

        func snapshot(now: ContinuousClock.Instant) -> Snapshot {
            lock.lock()
            let activeSnapshot = active.values.map { item in
                ActiveInterval(
                    operation: item.interval.operation.rawValue,
                    metadata: item.metadata,
                    elapsedMilliseconds: TraceBrowserSignpost.milliseconds(
                        item.interval.startedAt.duration(to: now)))
            }.sorted { $0.elapsedMilliseconds > $1.elapsedMilliseconds }
            let breadcrumbSnapshot = breadcrumbs
            lock.unlock()
            return Snapshot(active: activeSnapshot, breadcrumbs: breadcrumbSnapshot)
        }

        func reset() {
            lock.lock()
            active.removeAll()
            breadcrumbs.removeAll()
            lock.unlock()
        }

        private func appendBreadcrumb(_ value: String) {
            breadcrumbs.append(value)
            if breadcrumbs.count > TraceBrowserSignpost.breadcrumbLimit {
                breadcrumbs.removeFirst(breadcrumbs.count - TraceBrowserSignpost.breadcrumbLimit)
            }
        }
    }
}

public enum UIHangLog {
    public static let maxFileBytes: UInt64 = 1_024 * 1_024

    public static func fileURL(home: URL = FileManager.default.homeDirectoryForCurrentUser) -> URL {
        home.appendingPathComponent("Library/Logs/Alex", isDirectory: true)
            .appendingPathComponent("ui-hangs.log")
    }

    /// Ensures Finder has a concrete file to select even before the first
    /// detected hang has been written.
    public static func prepareForReveal(at url: URL = fileURL()) throws {
        let manager = FileManager.default
        try manager.createDirectory(
            at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        if !manager.fileExists(atPath: url.path) {
            guard manager.createFile(atPath: url.path, contents: nil) else {
                throw CocoaError(.fileWriteUnknown)
            }
        }
    }

    public static func shouldRotate(
        fileBytes: UInt64, incomingBytes: UInt64 = 0, limit: UInt64 = maxFileBytes
    ) -> Bool {
        fileBytes + incomingBytes > limit
    }

    public static func formatLine(
        timestamp: Date, durationMilliseconds: Double,
        snapshot: TraceBrowserSignpost.Snapshot
    ) -> String {
        let operation = snapshot.active.first?.operation ?? "main runloop"
        let active = snapshot.active.map {
            "\($0.operation){\($0.metadata),elapsed_ms=\(String(format: "%.1f", $0.elapsedMilliseconds))}"
        }.joined(separator: ";")
        let breadcrumbs = snapshot.breadcrumbs.joined(separator: " | ")
        return "\(iso.string(from: timestamp)) duration_ms=\(String(format: "%.1f", durationMilliseconds)) operation=\(flatten(operation)) active=[\(flatten(active))] breadcrumbs=[\(flatten(breadcrumbs))]"
    }

    static func append(durationMilliseconds: Double, snapshot: TraceBrowserSignpost.Snapshot) {
        let line = formatLine(
            timestamp: Date(), durationMilliseconds: durationMilliseconds, snapshot: snapshot) + "\n"
        queue.async { appendLocked(line) }
    }

    private static let queue = DispatchQueue(label: "com.alex.ui-hang-log")
    nonisolated(unsafe) private static var handle: FileHandle?
    nonisolated(unsafe) private static var fileBytes: UInt64 = 0
    nonisolated(unsafe) private static let iso: ISO8601DateFormatter = {
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return formatter
    }()

    private static func flatten(_ value: String) -> String {
        value.replacingOccurrences(of: "\r\n", with: "\\n")
            .replacingOccurrences(of: "\n", with: "\\n")
    }

    private static func appendLocked(_ line: String) {
        if handle == nil { openLocked() }
        let data = Data(line.utf8)
        if shouldRotate(fileBytes: fileBytes, incomingBytes: UInt64(data.count)) {
            rotateLocked()
        }
        guard let handle else { return }
        do {
            try handle.write(contentsOf: data)
            fileBytes += UInt64(data.count)
        } catch {
            self.handle = nil
        }
    }

    private static func openLocked() {
        let url = fileURL()
        let manager = FileManager.default
        try? manager.createDirectory(
            at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        if !manager.fileExists(atPath: url.path) {
            manager.createFile(atPath: url.path, contents: nil)
        }
        guard let file = try? FileHandle(forWritingTo: url) else { return }
        fileBytes = (try? file.seekToEnd()) ?? 0
        handle = file
    }

    private static func rotateLocked() {
        try? handle?.close()
        handle = nil
        let url = fileURL()
        let rotated = url.deletingLastPathComponent().appendingPathComponent("ui-hangs.log.1")
        let manager = FileManager.default
        try? manager.removeItem(at: rotated)
        try? manager.moveItem(at: url, to: rotated)
        fileBytes = 0
        openLocked()
    }
}

public final class UIHangWatchdog: @unchecked Sendable {
    public static let shared = UIHangWatchdog()
    public static let defaultsKey = "UIHangWatchdogEnabled"

    public static func isEnabled(
        defaults: UserDefaults = .standard, bundle: Bundle = .main
    ) -> Bool {
        if defaults.object(forKey: defaultsKey) != nil {
            return defaults.bool(forKey: defaultsKey)
        }
        #if DEBUG
        return true
        #else
        let version = bundle.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String
        return version?.localizedCaseInsensitiveContains("-beta.") == true
        #endif
    }

    private struct PendingPing {
        let sentAt: DispatchTime
        var reported: Bool
    }

    private let intervalNanoseconds: UInt64
    private let thresholdNanoseconds: UInt64
    private let queue = DispatchQueue(label: "com.alex.ui-hang-watchdog", qos: .utility)
    private var timer: DispatchSourceTimer?
    private var pending: PendingPing?

    public init(intervalMilliseconds: UInt64 = 250, thresholdMilliseconds: UInt64 = 500) {
        intervalNanoseconds = intervalMilliseconds * 1_000_000
        thresholdNanoseconds = thresholdMilliseconds * 1_000_000
    }

    public func startIfEnabled(
        defaults: UserDefaults = .standard, bundle: Bundle = .main
    ) {
        guard Self.isEnabled(defaults: defaults, bundle: bundle) else { return }
        queue.async { [weak self] in self?.startLocked() }
    }

    public func stop() {
        queue.async { [weak self] in
            self?.timer?.cancel()
            self?.timer = nil
            self?.pending = nil
        }
    }

    private func startLocked() {
        guard timer == nil else { return }
        let timer = DispatchSource.makeTimerSource(queue: queue)
        timer.schedule(
            deadline: .now() + .nanoseconds(Int(intervalNanoseconds)),
            repeating: .nanoseconds(Int(intervalNanoseconds)), leeway: .milliseconds(25))
        timer.setEventHandler { [weak self] in self?.tick() }
        self.timer = timer
        timer.resume()
    }

    private func tick() {
        let now = DispatchTime.now()
        if var pending {
            let elapsed = now.uptimeNanoseconds - pending.sentAt.uptimeNanoseconds
            if elapsed >= thresholdNanoseconds, !pending.reported {
                pending.reported = true
                self.pending = pending
                report(durationNanoseconds: elapsed)
            }
            return
        }
        pending = PendingPing(sentAt: now, reported: false)
        DispatchQueue.main.async { [weak self] in
            self?.queue.async { [weak self] in self?.pending = nil }
        }
    }

    private func report(durationNanoseconds: UInt64) {
        let durationMilliseconds = Double(durationNanoseconds) / 1_000_000
        let snapshot = TraceBrowserSignpost.snapshot()
        let operation = snapshot.active.first?.operation ?? "main runloop"
        #if canImport(os)
        os_log(
            .fault, log: OSLog(subsystem: "com.alex.bar", category: "UIHangWatchdog"),
            "main thread stalled %{public}.1fms operation=%{public}@",
            durationMilliseconds, operation as NSString)
        #endif
        UIHangLog.append(durationMilliseconds: durationMilliseconds, snapshot: snapshot)
    }
}
