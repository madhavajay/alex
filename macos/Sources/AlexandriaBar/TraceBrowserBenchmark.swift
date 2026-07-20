import AppKit
import Foundation

struct TraceBrowserBenchmarkConfiguration {
    static let enabledEnvironment = "ALEX_TRACE_BROWSER_BENCHMARK"

    let resultPath: URL
    let longSessionId: String
    let shortSessionId: String

    static func fromEnvironment(_ environment: [String: String]) -> Self? {
        guard environment[enabledEnvironment] == "1" else { return nil }
        guard let result = environment["ALEX_TRACE_BROWSER_BENCHMARK_RESULT"],
            !result.isEmpty,
            let longSession = environment["ALEX_TRACE_BROWSER_BENCHMARK_LONG_SESSION"],
            !longSession.isEmpty,
            let shortSession = environment["ALEX_TRACE_BROWSER_BENCHMARK_SHORT_SESSION"],
            !shortSession.isEmpty
        else { return nil }
        return Self(
            resultPath: URL(fileURLWithPath: result),
            longSessionId: longSession,
            shortSessionId: shortSession)
    }
}

private struct TraceBrowserBenchmarkPhase: Codable {
    let name: String
    let durationMs: Double
    let passed: Bool
}

private struct TraceBrowserBenchmarkWindowObservation: Codable {
    let isVisible: Bool
    let isKeyWindow: Bool
    let contentAttached: Bool
    let width: Double
    let height: Double
    let viewCommitCount: Int
    let activeViewMarkers: [String]
    let markerActivationCounts: [String: Int]
}

private struct TraceBrowserBenchmarkModelObservation: Codable {
    let selectedSessionId: String?
    let loadedTurns: Int
    let availableTurns: Int
    let firstTraceId: String?
    let lastTraceId: String?
    let inspectorTraceId: String?
    let sessionsLoading: Bool
    let sessionsUnreachable: Bool
    let transcriptLoading: Bool
    let transcriptPageLoading: Bool
    let transcriptUnreachable: Bool
    let daemonDown: Bool
}

private struct TraceBrowserBenchmarkReport: Codable {
    let schema: String
    let status: String
    let passed: Bool
    let fixtureKind: String
    let longSessionTurns: Int
    let longSessionDurationHours: Int
    let shortSessionTurns: Int
    let transcriptPageSize: Int
    let stablePollIntervals: Int
    let mainActorHeartbeatSamples: Int
    let maxMainActorHeartbeatGapMs: Double
    let phases: [TraceBrowserBenchmarkPhase]
    let failures: [String]
    let window: TraceBrowserBenchmarkWindowObservation
    let model: TraceBrowserBenchmarkModelObservation
}

@MainActor
final class TraceBrowserPackagedBenchmark {
    private let configuration: TraceBrowserBenchmarkConfiguration
    private let controller: TraceBrowserWindowController
    private let probe: TraceBrowserBenchmarkViewProbe
    private let initialStartedAt: ContinuousClock.Instant
    private var phases: [TraceBrowserBenchmarkPhase] = []
    private var failures: [String] = []
    private var maxMainActorHeartbeatGapMs = 0.0
    private var mainActorHeartbeatSamples = 0
    private var heartbeatTask: Task<Void, Never>?
    private var task: Task<Void, Never>?
    private var stablePollIntervals = 0

    init(
        configuration: TraceBrowserBenchmarkConfiguration,
        controller: TraceBrowserWindowController,
        probe: TraceBrowserBenchmarkViewProbe,
        initialStartedAt: ContinuousClock.Instant
    ) {
        self.configuration = configuration
        self.controller = controller
        self.probe = probe
        self.initialStartedAt = initialStartedAt
    }

    func start() {
        startHeartbeat()
        task = Task { [weak self] in
            guard let self else { return }
            await self.run()
        }
    }

    static func writeLaunchFailure(environment: [String: String], failure: String) {
        guard let resultPath = environment["ALEX_TRACE_BROWSER_BENCHMARK_RESULT"],
            !resultPath.isEmpty
        else {
            FileHandle.standardError.write(Data("Trace Browser benchmark: \(failure)\n".utf8))
            NSApp.terminate(nil)
            return
        }
        let result: [String: Any] = [
            "schema": "alex-trace-browser-packaged-benchmark-v1",
            "status": "failed",
            "passed": false,
            "failures": [failure],
        ]
        do {
            let data = try JSONSerialization.data(
                withJSONObject: result, options: [.prettyPrinted, .sortedKeys])
            let url = URL(fileURLWithPath: resultPath)
            try FileManager.default.createDirectory(
                at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
            try data.write(to: url, options: .atomic)
        } catch {
            FileHandle.standardError.write(
                Data("Trace Browser benchmark could not write launch failure: \(error)\n".utf8))
        }
        NSApp.terminate(nil)
    }

    private func run() async {
        guard let model = controller.model, let window = controller.window else {
            failures.append("benchmark launch did not create a Trace Browser model and window")
            finish()
            return
        }

        let initialPassed = await waitUntil(timeoutSeconds: 20) {
            self.windowIsCommitted(window)
                && model.selectedSessionId == self.configuration.longSessionId
                && model.transcriptLoadedTurnCount == 50
                && model.transcriptAvailableTurnCount == 1_277
                && model.transcriptHasMoreBefore
                && !model.sessionsLoading
                && !model.sessionsUnreachable
                && !model.transcriptLoading
                && !model.transcriptPageLoading
                && !model.transcriptUnreachable
                && !model.daemonDown
                && model.transcriptBodyErrorCount == 0
                && model.transcriptBodyTruncationCount == 0
                && model.turnRanges.count == 50
                && self.loadingMarkers().isEmpty
        }
        recordPhase(
            "initial_targeted_session_load",
            startedAt: initialStartedAt,
            passed: initialPassed,
            failure: "targeted 1,277-turn session did not settle to a visible 50-turn tail page")

        let olderStarted = ContinuousClock.now
        model.loadEarlierTurns()
        let olderPassed = await waitUntil(timeoutSeconds: 10) {
            model.selectedSessionId == self.configuration.longSessionId
                && model.transcriptLoadedTurnCount == 100
                && model.transcriptAvailableTurnCount == 1_277
                && !model.transcriptPageLoading
                && !model.transcriptUnreachable
                && !model.daemonDown
                && !self.probe.activeMarkers.contains("page-loading")
        }
        recordPhase(
            "one_older_page",
            startedAt: olderStarted,
            passed: olderPassed,
            failure: "older-page navigation did not settle at 100/1277 turns")

        let staleStarted = ContinuousClock.now
        model.loadEarlierTurns()
        let delayedRequestInFlight = await waitUntil(timeoutSeconds: 3) {
            model.transcriptPageLoading || self.probe.activeMarkers.contains("page-loading")
        }
        model.selectFromUser(configuration.shortSessionId)
        let shortLoaded = await waitUntil(timeoutSeconds: 10) {
            model.selectedSessionId == self.configuration.shortSessionId
                && model.transcriptLoadedTurnCount == 3
                && model.transcriptAvailableTurnCount == 3
                && model.turns.allSatisfy { $0.traceId.hasPrefix("short-synthetic-") }
                && !model.transcriptLoading
                && !model.transcriptPageLoading
                && !model.sessionsUnreachable
                && !model.transcriptUnreachable
                && !model.daemonDown
        }
        // The proxy keeps the old page response in flight for 1.5 seconds.
        // Waiting past that boundary verifies generation-based stale-result
        // suppression; it does not claim that URLSession transport was aborted.
        try? await Task.sleep(for: .milliseconds(1_800))
        let staleSuppressed = model.selectedSessionId == configuration.shortSessionId
            && model.transcriptLoadedTurnCount == 3
            && model.turns.allSatisfy { $0.traceId.hasPrefix("short-synthetic-") }
            && !model.transcriptUnreachable
            && !model.daemonDown
        let stalePassed = delayedRequestInFlight && shortLoaded && staleSuppressed
        recordPhase(
            "stale_page_suppression_after_navigation",
            startedAt: staleStarted,
            passed: stalePassed,
            failure: "a delayed long-session page was not observed or was allowed to overwrite the short session")

        let navigationStarted = ContinuousClock.now
        model.selectFromUser(configuration.longSessionId)
        let returnedToLong = await waitUntil(timeoutSeconds: 10) {
            model.selectedSessionId == self.configuration.longSessionId
                && model.transcriptLoadedTurnCount == 50
                && model.turns.last?.traceId == "synthetic-001276"
                && !model.transcriptLoading
                && !model.sessionsUnreachable
                && !model.transcriptUnreachable
                && !model.daemonDown
        }
        model.jumpToLatestTurns()
        let jumpedToLatest = await waitUntil(timeoutSeconds: 10) {
            model.selectedSessionId == self.configuration.longSessionId
                && model.transcriptLoadedTurnCount == 50
                && model.turns.last?.traceId == "synthetic-001276"
                && !model.transcriptLoading
                && !model.transcriptPageLoading
                && !model.sessionsUnreachable
                && !model.transcriptUnreachable
        }
        var adjacentTracePassed = false
        if model.turns.count >= 2, let latest = model.turns.last?.traceId {
            let previous = model.turns[model.turns.count - 2].traceId
            model.openInspector(traceId: latest)
            _ = await waitUntil(timeoutSeconds: 3) {
                model.detailsVisible && model.inspectorTraceId == latest
            }
            model.stepInspector(-1)
            adjacentTracePassed = await waitUntil(timeoutSeconds: 3) {
                model.detailsVisible && model.inspectorTraceId == previous
            }
        }
        let navigationPassed = returnedToLong && jumpedToLatest && adjacentTracePassed
        recordPhase(
            "back_latest_adjacent_trace_navigation",
            startedAt: navigationStarted,
            passed: navigationPassed,
            failure: "back/latest/adjacent trace navigation did not keep the expected visible selection")

        let stabilityStarted = ContinuousClock.now
        let watchedMarkers = [
            "sessions-loading", "transcript-loading", "page-loading",
            "conversation-loading", "daemon-down",
        ]
        let activationBaseline = probe.markerActivationCounts
        var stable = true
        for _ in 0..<10 {
            try? await Task.sleep(for: .seconds(1))
            controller.window?.contentView?.layoutSubtreeIfNeeded()
            let modelStable = model.selectedSessionId == configuration.longSessionId
                && model.transcriptLoadedTurnCount == 50
                && !model.sessionsLoading
                && !model.sessionsUnreachable
                && !model.transcriptLoading
                && !model.transcriptPageLoading
                && !model.transcriptUnreachable
                && !model.daemonDown
            let markersStable = watchedMarkers.allSatisfy {
                probe.markerActivationCounts[$0, default: 0]
                    == activationBaseline[$0, default: 0]
            }
            stable = stable && modelStable && markersStable && windowIsCommitted(window)
            stablePollIntervals += 1
        }
        recordPhase(
            "ten_stable_poll_intervals",
            startedAt: stabilityStarted,
            passed: stable,
            failure: "loading or daemon-down state reappeared during ten stable poll intervals")

        if maxMainActorHeartbeatGapMs > 250 {
            failures.append(
                "maximum main-actor heartbeat gap \(rounded(maxMainActorHeartbeatGapMs))ms exceeded 250ms")
        }
        if mainActorHeartbeatSamples < 100 {
            failures.append(
                "main-actor heartbeat produced only \(mainActorHeartbeatSamples) samples; expected at least 100")
        }
        finish()
    }

    private func startHeartbeat() {
        heartbeatTask = Task { [weak self] in
            var previous = ContinuousClock.now
            while !Task.isCancelled {
                try? await Task.sleep(for: .milliseconds(10))
                guard !Task.isCancelled else { return }
                let now = ContinuousClock.now
                let gap = Self.milliseconds(previous.duration(to: now))
                self?.mainActorHeartbeatSamples += 1
                self?.maxMainActorHeartbeatGapMs = max(
                    self?.maxMainActorHeartbeatGapMs ?? 0, gap)
                previous = now
            }
        }
    }

    private func waitUntil(
        timeoutSeconds: Double,
        condition: @MainActor () -> Bool
    ) async -> Bool {
        let deadline = ContinuousClock.now.advanced(by: .seconds(timeoutSeconds))
        while ContinuousClock.now < deadline {
            controller.window?.contentView?.layoutSubtreeIfNeeded()
            controller.window?.contentView?.displayIfNeeded()
            if condition() { return true }
            try? await Task.sleep(for: .milliseconds(20))
        }
        return condition()
    }

    private func windowIsCommitted(_ window: NSWindow) -> Bool {
        window.contentView?.layoutSubtreeIfNeeded()
        return window.isVisible
            && window.isKeyWindow
            && window.contentView?.window === window
            && (window.contentView?.bounds.width ?? 0) > 0
            && (window.contentView?.bounds.height ?? 0) > 0
            && probe.commitCount > 0
            && probe.activeMarkers.isSuperset(of: ["session-pane", "transcript-pane"])
    }

    private func loadingMarkers() -> Set<String> {
        probe.activeMarkers.intersection(
            [
                "sessions-loading", "transcript-loading", "page-loading",
                "conversation-loading", "daemon-down",
            ])
    }

    private func recordPhase(
        _ name: String,
        startedAt: ContinuousClock.Instant,
        passed: Bool,
        failure: String
    ) {
        phases.append(
            TraceBrowserBenchmarkPhase(
                name: name,
                durationMs: Self.milliseconds(startedAt.duration(to: .now)),
                passed: passed))
        if !passed { failures.append(failure) }
    }

    private func finish() {
        heartbeatTask?.cancel()
        controller.model?.stop()
        let window = controller.window
        window?.contentView?.layoutSubtreeIfNeeded()
        let model = controller.model
        let report = TraceBrowserBenchmarkReport(
            schema: "alex-trace-browser-packaged-benchmark-v1",
            status: failures.isEmpty ? "passed" : "failed",
            passed: failures.isEmpty,
            fixtureKind: "aggregate-only-generated-lar",
            longSessionTurns: 1_277,
            longSessionDurationHours: 15,
            shortSessionTurns: 3,
            transcriptPageSize: 50,
            stablePollIntervals: stablePollIntervals,
            mainActorHeartbeatSamples: mainActorHeartbeatSamples,
            maxMainActorHeartbeatGapMs: rounded(maxMainActorHeartbeatGapMs),
            phases: phases,
            failures: failures,
            window: TraceBrowserBenchmarkWindowObservation(
                isVisible: window?.isVisible ?? false,
                isKeyWindow: window?.isKeyWindow ?? false,
                contentAttached: window.map { $0.contentView?.window === $0 } ?? false,
                width: window?.contentView?.bounds.width ?? 0,
                height: window?.contentView?.bounds.height ?? 0,
                viewCommitCount: probe.commitCount,
                activeViewMarkers: probe.activeMarkers.sorted(),
                markerActivationCounts: probe.markerActivationCounts),
            model: TraceBrowserBenchmarkModelObservation(
                selectedSessionId: model?.selectedSessionId,
                loadedTurns: model?.transcriptLoadedTurnCount ?? 0,
                availableTurns: model?.transcriptAvailableTurnCount ?? 0,
                firstTraceId: model?.turns.first?.traceId,
                lastTraceId: model?.turns.last?.traceId,
                inspectorTraceId: model?.inspectorTraceId,
                sessionsLoading: model?.sessionsLoading ?? false,
                sessionsUnreachable: model?.sessionsUnreachable ?? true,
                transcriptLoading: model?.transcriptLoading ?? false,
                transcriptPageLoading: model?.transcriptPageLoading ?? false,
                transcriptUnreachable: model?.transcriptUnreachable ?? true,
                daemonDown: model?.daemonDown ?? true))
        do {
            let encoder = JSONEncoder()
            encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
            encoder.keyEncodingStrategy = .convertToSnakeCase
            let data = try encoder.encode(report)
            try FileManager.default.createDirectory(
                at: configuration.resultPath.deletingLastPathComponent(),
                withIntermediateDirectories: true)
            try data.write(to: configuration.resultPath, options: .atomic)
        } catch {
            let message = "Trace Browser benchmark could not write result: \(error)\n"
            FileHandle.standardError.write(Data(message.utf8))
        }
        NSApp.terminate(nil)
    }

    private static func milliseconds(_ duration: Duration) -> Double {
        Double(duration.components.seconds) * 1_000
            + Double(duration.components.attoseconds) / 1e15
    }

    private func rounded(_ value: Double) -> Double {
        (value * 100).rounded() / 100
    }
}
