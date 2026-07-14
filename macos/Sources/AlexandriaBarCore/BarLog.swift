import Foundation
#if canImport(os)
import os
#endif

public enum BarLog {
    public enum Category: String, CaseIterable, Sendable {
        case net, browser, ui
    }

    public enum Level: String, Sendable {
        case info = "INFO"
        case warn = "WARN"
        case error = "ERROR"
    }

    public static let maxFileBytes: UInt64 = 5 * 1024 * 1024
    public static let slowThresholdMs: Double = 100

    public static var fileURL: URL {
        FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent(".alexandria/bar.log")
    }

    public static func info(_ category: Category, _ message: String) {
        log(.info, category, message)
    }

    public static func warn(_ category: Category, _ message: String) {
        log(.warn, category, message)
    }

    public static func error(_ category: Category, _ message: String) {
        log(.error, category, message)
    }

    @MainActor
    @discardableResult
    public static func measure<T>(
        _ category: Category, label: @autoclosure () -> String, _ body: () throws -> T
    ) rethrows -> T {
        let start = ContinuousClock.now
        defer {
            let elapsed = start.duration(to: .now)
            let ms = Double(elapsed.components.seconds) * 1000
                + Double(elapsed.components.attoseconds) / 1e15
            if ms >= slowThresholdMs {
                warn(category, "SLOW \(label()) \(String(format: "%.0f", ms))ms")
            } else {
                info(category, "\(label()) \(String(format: "%.1f", ms))ms")
            }
        }
        return try body()
    }

    public static func formatLine(
        timestamp: Date, level: Level, category: Category, message: String
    ) -> String {
        let flat = message
            .replacingOccurrences(of: "\r\n", with: "\\n")
            .replacingOccurrences(of: "\n", with: "\\n")
        return "\(iso.string(from: timestamp)) \(level.rawValue) [\(category.rawValue)] \(flat)"
    }

    public static func shouldRotate(fileBytes: UInt64, limit: UInt64 = maxFileBytes) -> Bool {
        fileBytes > limit
    }

    #if canImport(os)
    private static let subsystem = "com.alexandria.bar"
    private static let netLogger = Logger(subsystem: subsystem, category: "net")
    private static let browserLogger = Logger(subsystem: subsystem, category: "browser")
    private static let uiLogger = Logger(subsystem: subsystem, category: "ui")
    #endif

    private static let queue = DispatchQueue(label: "com.alexandria.bar.log")
    nonisolated(unsafe) private static var handle: FileHandle?
    nonisolated(unsafe) private static var fileBytes: UInt64 = 0
    nonisolated(unsafe) private static let iso: ISO8601DateFormatter = {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return f
    }()

    #if canImport(os)
    private static func logger(_ category: Category) -> Logger {
        switch category {
        case .net: netLogger
        case .browser: browserLogger
        case .ui: uiLogger
        }
    }
    #endif

    private static func log(_ level: Level, _ category: Category, _ message: String) {
        #if canImport(os)
        let logger = logger(category)
        switch level {
        case .info: logger.info("\(message, privacy: .public)")
        case .warn: logger.warning("\(message, privacy: .public)")
        case .error: logger.error("\(message, privacy: .public)")
        }
        #endif
        let line = formatLine(timestamp: Date(), level: level, category: category, message: message)
        queue.async { appendLocked(line + "\n") }
    }

    private static func appendLocked(_ line: String) {
        if handle == nil { openLocked() }
        if shouldRotate(fileBytes: fileBytes) { rotateLocked() }
        guard let handle else { return }
        let data = Data(line.utf8)
        do {
            try handle.write(contentsOf: data)
            fileBytes += UInt64(data.count)
        } catch {
            self.handle = nil
        }
    }

    private static func openLocked() {
        let url = fileURL
        let fm = FileManager.default
        try? fm.createDirectory(
            at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        if !fm.fileExists(atPath: url.path) {
            fm.createFile(atPath: url.path, contents: nil)
        }
        guard let fh = try? FileHandle(forWritingTo: url) else { return }
        fileBytes = (try? fh.seekToEnd()) ?? 0
        handle = fh
    }

    private static func rotateLocked() {
        try? handle?.close()
        handle = nil
        let fm = FileManager.default
        let rotated = fileURL.deletingLastPathComponent().appendingPathComponent("bar.log.1")
        try? fm.removeItem(at: rotated)
        try? fm.moveItem(at: fileURL, to: rotated)
        fileBytes = 0
        openLocked()
    }
}
