import AppKit
import Foundation
import AlexCore

struct RenderedArtifactKey: Hashable, Sendable {
    let traceId: String
    let completedMs: Int64
    let discriminator: String
}

struct RenderedSSEPages: Equatable, Sendable {
    let pages: [[SSEFrames.Frame]]
    let truncated: Bool

    var frameCount: Int { pages.reduce(0) { $0 + $1.count } }

    static func parse(_ source: String, pageSize: Int = 20) -> Self {
        let result = SSEFrames.parse(source)
        let size = max(1, pageSize)
        let pages = stride(from: 0, to: result.frames.count, by: size).map { start in
            Array(result.frames[start..<min(start + size, result.frames.count)])
        }
        return Self(pages: pages, truncated: result.truncated)
    }
}

struct AttributedStringBox: @unchecked Sendable {
    let value: NSAttributedString

    init(_ value: NSAttributedString) {
        self.value = value
    }
}

@MainActor
final class RenderedArtifactCache {
    struct Stats: Equatable {
        let hits: Int
        let misses: Int
    }

    private enum Artifact {
        case chat([MessageDisplay])
        case formatted(AttributedStringBox)
        case sse(RenderedSSEPages)
    }

    private struct Entry {
        let artifact: Artifact
        let byteCost: Int
        var lastUse: UInt64
        var lastTouched: TimeInterval
    }

    let countLimit: Int
    let byteLimit: Int
    let idleInterval: TimeInterval

    private let clock: () -> TimeInterval
    private var entries: [RenderedArtifactKey: Entry] = [:]
    private var storedByteCost = 0
    private var useCounter: UInt64 = 0
    private var hitCount = 0
    private var missCount = 0
    private var pressureSource: DispatchSourceMemoryPressure?
    private var idleSource: DispatchSourceTimer?

    init(
        countLimit: Int = 1_024,
        byteLimit: Int = 96 * 1_024 * 1_024,
        idleInterval: TimeInterval = 5 * 60,
        clock: @escaping () -> TimeInterval = { ProcessInfo.processInfo.systemUptime },
        startMaintenance: Bool = true
    ) {
        self.countLimit = max(1, countLimit)
        self.byteLimit = max(1, byteLimit)
        self.idleInterval = max(0, idleInterval)
        self.clock = clock
        if startMaintenance { startMaintenanceSources() }
    }

    var count: Int { entries.count }
    var approximateByteCost: Int { storedByteCost }
    var stats: Stats { Stats(hits: hitCount, misses: missCount) }

    func chat(for key: RenderedArtifactKey) -> [MessageDisplay]? {
        guard case let .chat(value)? = value(for: key) else { return nil }
        return value
    }

    func formatted(for key: RenderedArtifactKey) -> AttributedStringBox? {
        guard case let .formatted(value)? = value(for: key) else { return nil }
        return value
    }

    func sse(for key: RenderedArtifactKey) -> RenderedSSEPages? {
        guard case let .sse(value)? = value(for: key) else { return nil }
        return value
    }

    func insertChat(_ value: [MessageDisplay], for key: RenderedArtifactKey) {
        insert(.chat(value), byteCost: Self.chatByteCost(value), for: key)
    }

    func insertFormatted(_ value: AttributedStringBox, for key: RenderedArtifactKey) {
        insert(.formatted(value), byteCost: max(1, value.value.length * 4), for: key)
    }

    func insertSSE(_ value: RenderedSSEPages, for key: RenderedArtifactKey) {
        let cost = value.pages.flatMap { $0 }.reduce(0) { total, frame in
            total + (frame.event?.utf8.count ?? 0) + frame.data.utf8.count + 32
        }
        insert(.sse(value), byteCost: max(1, cost), for: key)
    }

    func clear() {
        entries.removeAll(keepingCapacity: false)
        storedByteCost = 0
    }

    func handleMemoryPressure() {
        clear()
    }

    func evictIdle() {
        let cutoff = clock() - idleInterval
        let stale = entries.filter { $0.value.lastTouched <= cutoff }.map(\.key)
        stale.forEach { remove($0) }
    }

    private func value(for key: RenderedArtifactKey) -> Artifact? {
        guard var entry = entries[key] else {
            missCount += 1
            return nil
        }
        hitCount += 1
        useCounter &+= 1
        entry.lastUse = useCounter
        entry.lastTouched = clock()
        entries[key] = entry
        return entry.artifact
    }

    private func insert(_ artifact: Artifact, byteCost: Int, for key: RenderedArtifactKey) {
        useCounter &+= 1
        if let existing = entries[key] { storedByteCost -= existing.byteCost }
        let cost = max(1, byteCost)
        entries[key] = Entry(
            artifact: artifact, byteCost: cost,
            lastUse: useCounter, lastTouched: clock())
        storedByteCost += cost
        evictToLimits()
    }

    private func evictToLimits() {
        while entries.count > countLimit || approximateByteCost > byteLimit {
            guard let oldest = entries.min(by: { $0.value.lastUse < $1.value.lastUse })?.key
            else { return }
            remove(oldest)
        }
    }

    private func remove(_ key: RenderedArtifactKey) {
        guard let removed = entries.removeValue(forKey: key) else { return }
        storedByteCost -= removed.byteCost
    }

    private func startMaintenanceSources() {
        let pressure = DispatchSource.makeMemoryPressureSource(
            eventMask: [.warning, .critical], queue: .main)
        pressure.setEventHandler { [weak self] in self?.handleMemoryPressure() }
        pressure.resume()
        pressureSource = pressure

        let idle = DispatchSource.makeTimerSource(queue: .main)
        idle.schedule(deadline: .now() + 60, repeating: 60)
        idle.setEventHandler { [weak self] in self?.evictIdle() }
        idle.resume()
        idleSource = idle
    }

    private static func chatByteCost(_ messages: [MessageDisplay]) -> Int {
        messages.reduce(0) { total, message in
            total + [
                message.id, message.turnId, message.roleLabel, message.content,
                message.model, message.detail, message.timestamp, message.tokenText,
                message.error, message.event,
            ].compactMap { $0 }.reduce(0) { $0 + $1.utf8.count }
                + (message.attributedContent?.characters.count ?? 0) * 4
                + message.toolCalls.reduce(0) { subtotal, tool in
                    subtotal + [
                        tool.id, tool.name, tool.argumentPreview, tool.input, tool.output,
                        tool.statusText, tool.durationText,
                    ].compactMap { $0 }.reduce(0) { $0 + $1.utf8.count }
                } + 128
        }
    }
}
