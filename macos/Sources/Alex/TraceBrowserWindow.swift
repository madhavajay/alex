import AppKit
import SwiftUI
import Observation
import AlexCore

@MainActor
@Observable
final class TraceBrowserModel {
    typealias TranscriptPageFetcher = @Sendable (
        _ sessionId: String, _ limit: Int, _ cursor: TranscriptCursor?
    ) async throws -> TranscriptPageResponse
    typealias TraceTurnFetcher = @Sendable (_ traceId: String) async throws -> TranscriptTurn

    private let store: SnapshotStore
    let renderedArtifacts = RenderedArtifactCache()
    private let transcriptPageFetcher: TranscriptPageFetcher?
    private let traceTurnFetcher: TraceTurnFetcher?

    private(set) var sessions: [TraceSession] = [] {
        didSet { recomputeSessionSummary(); recomputeTranscriptSummary() }
    }
    private(set) var turns: [TranscriptTurn] = [] {
        didSet { scheduleTranscriptFilter(debounce: false); recomputeTranscriptSummary() }
    }
    private(set) var visibleRows: [SessionRow] = []
    private(set) var parsedQuery = OmniQuery()
    private(set) var daemonDown = false
    private(set) var sessionsLoading = true
    private(set) var sessionsUnreachable = false
    private(set) var simulationFixtures: [ErrorSimulationFixture] = []
    private(set) var middlewareRules: [MiddlewareRuleSpecV1] = []
    private var lastSessionsMiddlewareId: String?
    private(set) var fixturesLoading = false
    private(set) var fixtureLoadError: String?
    private(set) var simulationNotice: String?
    private(set) var transcriptLoading = false
    private(set) var transcriptUnreachable = false
    private var transcriptLoadedSessionId: String?
    private(set) var searchSessionIds: Set<String>?
    private(set) var searchMatchCount = 0
    private(set) var searchScanned = 0
    /// True while a body-text search request is in flight (debounce elapsed,
    /// awaiting `/traces/search`); drives the omni search field's trailing
    /// spinner.
    private(set) var searchInFlight = false
    private var sessionsFingerprint = ""
    private var turnsFingerprint = ""
    private var rowsById: [String: SessionRow] = [:]
    private var collapsedLineageRoots = Set<String>()
    private var allTurns: [TranscriptTurn] = []
    private var selectionState = SessionSelection()

    /// Table selection, driving shift/cmd-click multi-select. A single id
    /// keeps `selectionState` (and therefore transcript/inspector loading,
    /// pin, live-follow) behaving exactly as before multi-select existed;
    /// more than one suspends all of that and shows an empty state instead
    /// (see `TranscriptView.multiSelectionState`).
    private(set) var multiSelection: Set<String> = []
    var isMultiSelected: Bool { multiSelection.count > 1 }

    var selectedSessionId: String? { selectionState.selectedId }
    var pinned: Bool { selectionState.pinned }

    /// Table selection binding target. Ignores an empty set the same way
    /// the old `Binding<String?>` ignored `nil` — Table reports a transient
    /// empty selection during some data updates, and this app always wants
    /// a "last selected" session to stick for the detail panes.
    func updateSelection(_ ids: Set<String>) {
        guard !ids.isEmpty else { return }
        multiSelection = ids
        if ids.count == 1, let only = ids.first {
            selectFromUser(only)
        }
    }
    var sortOrder = SessionTable.defaultSortOrder() {
        didSet { recomputeVisible() }
    }
    var showPings = false {
        didSet { recomputeVisible() }
    }
    var nestSubagents = true {
        didSet { recomputeVisible() }
    }
    var queryText = "" {
        didSet {
            parsedQuery = OmniQuery.parse(queryText)
            queryChanged()
        }
    }
    /// Quick status filter pills (All | Running | Error | Done) layered on top
    /// of the omni query (mock TB App.tsx:313-326).
    var statusPill = SessionStatusPill.all {
        didSet { recomputeVisible() }
    }

    /// Transcript message filter row state (mock TB App.tsx:646-649). Typing
    /// drives a debounced, off-main-actor recompute of `transcriptEntries`
    /// rather than filtering synchronously in the view body — see
    /// `scheduleTranscriptFilter` (previously froze the window on large
    /// sessions; the filter now runs via `TranscriptFilter` in Core).
    var transcriptQuery = "" {
        didSet {
            guard oldValue != transcriptQuery else { return }
            scheduleTranscriptFilter(debounce: true)
        }
    }
    var transcriptFilterTab = 0 {
        didSet {
            guard oldValue != transcriptFilterTab else { return }
            scheduleTranscriptFilter(debounce: false)
        }
    }
    private(set) var transcriptEntries: [TranscriptChatEntry] = []
    private(set) var transcriptTotalCount = 0
    private(set) var transcriptTabCounts: TranscriptTabCounts?
    private var transcriptFilterTask: Task<Void, Never>?
    private var transcriptMetadata: [TranscriptTurnMetadata] = []
    private var expandedTurns: [String: TranscriptTurn] = [:]
    private var turnFetchTasks: [String: Task<Void, Never>] = [:]
    private var expandedRebuildTask: Task<Void, Never>?
    private var turnApplyTask: Task<Void, Never>?
    private var turnApplyGeneration = 0
    private var forceChunkedPaneUpdate = false
    private(set) var transcriptRendering = false
    private(set) var lastTurnApplyBatchCharacterCounts: [Int] = []
    private(set) var lastChatPaneBatchCharacterCounts: [Int] = []
    private var actionTasks: [UUID: Task<Void, Never>] = [:]
    private var metadataHiddenTurnCount = 0

    /// Recomputes `transcriptEntries`/`transcriptTotalCount` off the main
    /// actor. `debounce` is true for keystrokes (200ms settle) and false for
    /// structural changes (new turns, tab switch) that should apply
    /// immediately. Cancels any in-flight recompute first, so a burst of
    /// keystrokes only pays for the final one.
    private func scheduleTranscriptFilter(debounce: Bool) {
        transcriptFilterTask?.cancel()
        let turnsSnapshot = turns
        let tab = transcriptFilterTab
        let query = transcriptQuery
        let forceIncremental = forceChunkedPaneUpdate
        transcriptFilterTask = Task { [weak self] in
            if debounce {
                try? await Task.sleep(for: .milliseconds(200))
                guard !Task.isCancelled else { return }
            }
            let result = await Task.detached(priority: .userInitiated) {
                () -> ([TranscriptFilterEntry], Int) in
                let start = ContinuousClock.now
                defer {
                    let elapsed = start.duration(to: .now)
                    BarLog.timing(
                        .browser,
                        label: "transcript filter turns=\(turnsSnapshot.count) tab=\(tab)",
                        milliseconds: Double(elapsed.components.seconds) * 1000
                            + Double(elapsed.components.attoseconds) / 1e15)
                }
                let filtered = TranscriptFilter.result(
                    turns: turnsSnapshot, filterTab: tab, query: query)
                return (filtered.entries, filtered.totalCount)
            }.value
            guard !Task.isCancelled, let self else { return }
            let mapped = self.mapFilterEntries(result.0, turns: turnsSnapshot)
            let characterCounts = mapped.map(Self.entryCharacterCount)
            let ranges = TranscriptApplyPolicy.messageBatchRanges(
                characterCounts: characterCounts,
                forceIncremental: forceIncremental
                    || mapped.count > TranscriptApplyPolicy.largeMessageCount)
            self.lastChatPaneBatchCharacterCounts = []
            self.transcriptTotalCount = result.1
            if ranges.isEmpty {
                self.transcriptEntries = []
            } else {
                for range in ranges {
                    guard !Task.isCancelled else { return }
                    let batchChars = characterCounts[range].reduce(0, +)
                    let interval = TraceBrowserSignpost.begin(
                        .chatPaneUpdate,
                        "batch_entries=\(range.count) batch_chars=\(batchChars) total_entries=\(mapped.count)")
                    self.transcriptEntries = Array(mapped[..<range.upperBound])
                    self.lastChatPaneBatchCharacterCounts.append(batchChars)
                    DispatchQueue.main.async { TraceBrowserSignpost.end(interval) }
                    if range.upperBound < mapped.count {
                        await Task.yield()
                        try? await Task.sleep(for: .milliseconds(1))
                    }
                }
            }
            guard !Task.isCancelled else { return }
            self.forceChunkedPaneUpdate = false
            self.transcriptRendering = false
        }
    }

    private nonisolated static func entryCharacterCount(_ entry: TranscriptChatEntry) -> Int {
        switch entry.role {
        case .user:
            entry.turn.user?.count ?? 0
        case .assistant:
            max(
                0,
                TranscriptApplyPolicy.inlineCharacterCount(entry.turn)
                    - (entry.turn.user?.count ?? 0))
        case .event:
            min(
                TurnTextCap.maxChars,
                (entry.turn.attempts ?? []).reduce(0) {
                    $0 + ($1.error?.message?.count ?? 0)
                        + ($1.middlewareDecisions ?? []).reduce(0) {
                            $0 + ($1.explanation?.count ?? 0)
                        }
                })
        }
    }

    private func mapFilterEntries(
        _ filtered: [TranscriptFilterEntry], turns: [TranscriptTurn]
    ) -> [TranscriptChatEntry] {
        filtered.flatMap { entry -> [TranscriptChatEntry] in
            guard turns.indices.contains(entry.turnIndex) else { return [] }
            let turn = turns[entry.turnIndex]
            guard turn.traceId == entry.turnId else { return [] }
            let role: TranscriptChatEntry.Role = entry.role == .user ? .user : .assistant
            if entry.role == .assistant, turn.hasInlineAttemptEvents {
                return [
                    TranscriptChatEntry(
                        turn: turn, turnNumber: entry.turnIndex + 1, role: .event),
                    TranscriptChatEntry(
                        turn: turn, turnNumber: entry.turnIndex + 1, role: role),
                ]
            }
            return [TranscriptChatEntry(
                turn: turn, turnNumber: entry.turnIndex + 1, role: role)]
        }
    }

    /// Session id of a subagent the user followed via "Follow trace"; drives
    /// the follow banner at the top of the transcript (mock TB App.tsx:652-672).
    private(set) var followedSubagentId: String?

    func followSubagent(_ id: String) {
        guard sessions.contains(where: { $0.sessionId == id }) else {
            // Fall back to the previous behavior: treat the id as a trace id.
            openInspector(traceId: id)
            return
        }
        selectFromUser(id)
        followedSubagentId = id
    }

    func dismissFollowBanner() {
        followedSubagentId = nil
    }

    private(set) var userAtBottom = true

    func setUserAtBottom(_ value: Bool) {
        guard userAtBottom != value else { return }
        userAtBottom = value
    }

    enum TranscriptRenderOp {
        case set(NSAttributedString)
        case append(NSAttributedString)
    }

    private(set) var renderOp: (version: Int, op: TranscriptRenderOp)?
    private(set) var scrollCommand = 0
    private(set) var hiddenTurnCount = 0
    private var renderVersion = 0
    private var renderState: TranscriptRender.State?
    private var renderChain: Task<Void, Never>?
    private var windowStart = 0
    private var windowMaxTurns = TranscriptWindow.defaultMaxTurns

    private var sessionsTask: Task<Void, Never>?
    private var transcriptTask: Task<Void, Never>?
    private var searchTask: Task<Void, Never>?
    private var sessionFilterTask: Task<Void, Never>?
    private var fixtureLoadTask: Task<Void, Never>?
    private var simulationNoticeTask: Task<Void, Never>?
    private var sessionFilterGeneration = 0

    private(set) var detailsVisible = false
    private(set) var inspectorTraceId: String?
    private(set) var firstTraceDetail: TraceDetailResponse?
    private var firstDetailKey: String?
    private var firstDetailTask: Task<Void, Never>?

    private(set) var turnRanges: [TurnRange] = []
    private var renderedLength = 0
    private(set) var scrollToRangeCommand: (version: Int, range: NSRange)?
    private var scrollToRangeVersion = 0
    private(set) var findCommand = 0
    private(set) var findBarVisible = false

    var transcriptRawMode = UserDefaults.standard.bool(forKey: "TranscriptRawMode") {
        didSet {
            guard oldValue != transcriptRawMode else { return }
            UserDefaults.standard.set(transcriptRawMode, forKey: "TranscriptRawMode")
            renderState = nil
            scheduleRender()
        }
    }

    var firstTurnTraceId: String? { turns.first?.traceId }

    func previousTraceId(before traceId: String) -> String? {
        TraceInspectorSelection.previous(before: traceId, in: turns.map(\.traceId))
    }

    var sessionSystemPrompt: String? {
        guard let prompt = firstTraceDetail?.extras?.systemPrompt, !prompt.isEmpty else {
            return nil
        }
        return prompt
    }

    private var bodyCache = TraceBodyCache(capacity: 20)

    func fetchTraceBody(
        id: String, kind: TraceBodyKind, loadFull: Bool = false
    ) async throws -> TraceBodyContent {
        let key = TraceBodyCache.key(id: id, kind: kind) + (loadFull ? "|full" : "")
        if let cached = bodyCache.value(for: key) { return cached }
        guard let client = client() else {
            throw AlexClient.ClientError.http(0, "daemon unavailable")
        }
        let content = try await client.traceBody(
            id: id, kind: kind, maxBytes: loadFull ? nil : AlexClient.displayBodyByteLimit)
        bodyCache.insert(content, for: key)
        return content
    }

    private var toolBodyCache = TraceBodyCache(capacity: 20)

    /// Captured args/result body for a tool execution (kind: "args" |
    /// "result"). Used both to backfill the Input/Output tabs when the wire
    /// arguments were empty and to drive `inspectorToolBody`. Cached per
    /// (tool id, kind) since a card can request it more than once (tab
    /// switch, reopening the inspector route).
    func fetchToolBody(
        id: String, kind: String, loadFull: Bool = false
    ) async throws -> TraceBodyContent {
        let key = "\(id)|\(kind)" + (loadFull ? "|full" : "")
        if let cached = toolBodyCache.value(for: key) { return cached }
        guard let client = client() else {
            throw AlexClient.ClientError.http(0, "daemon unavailable")
        }
        let content = try await client.toolBody(
            id: id, kind: kind, maxBytes: loadFull ? nil : AlexClient.displayBodyByteLimit)
        toolBodyCache.insert(content, for: key)
        return content
    }

    /// "View captured args"/"View output" route into the inspector column
    /// (with a breadcrumb back to the turn) instead of a popup window.
    struct ToolBodyRoute: Equatable {
        let toolId: String
        let toolName: String
        let kind: String
        let turnId: String
    }

    private(set) var inspectorToolBody: ToolBodyRoute?

    func openInspectorToolBody(toolId: String, toolName: String, kind: String, turnId: String) {
        inspectorToolBody = ToolBodyRoute(toolId: toolId, toolName: toolName, kind: kind, turnId: turnId)
        detailsVisible = true
    }

    func closeInspectorToolBody() {
        inspectorToolBody = nil
    }

    /// Legacy (id, kind)-only entry point — still used by the classic text
    /// pane's clickable tool links, which don't carry a tool name/turn id at
    /// the click site. Resolves both by scanning the loaded turns.
    func openToolBody(id: String, kind: String) {
        for turn in turns {
            guard let executed = turn.executedTools?.first(where: { $0.id == id }) else { continue }
            openInspectorToolBody(
                toolId: id, toolName: executed.toolName, kind: kind, turnId: turn.traceId)
            return
        }
        openInspectorToolBody(toolId: id, toolName: "Tool", kind: kind, turnId: inspectorTraceId ?? "")
    }

    var firstRequestHeaders: [HeaderPair] {
        TraceHeaders.sortedPairs(firstTraceDetail?.trace.reqHeadersJson)
    }

    func openInspector(traceId: String) {
        inspectorTraceId = traceId
        detailsVisible = true
    }

    func closeInspector() {
        detailsVisible = false
        inspectorTraceId = nil
        inspectorToolBody = nil
    }

    func setDetailsVisible(_ visible: Bool) {
        guard detailsVisible != visible else {
            if visible { retargetInspector() }
            return
        }
        detailsVisible = visible
        if visible {
            retargetInspector()
        } else {
            inspectorTraceId = nil
        }
    }

    func requestFind() {
        findCommand += 1
    }

    func setFindBarVisible(_ visible: Bool) {
        guard findBarVisible != visible else { return }
        findBarVisible = visible
    }

    private func inspectorTurnIndex() -> Int? {
        guard let inspectorTraceId else { return nil }
        return turns.firstIndex { $0.traceId == inspectorTraceId }
    }

    private func retargetInspector() {
        guard detailsVisible else { return }
        inspectorTraceId = TraceInspectorSelection.target(
            currentTraceId: inspectorTraceId, in: turns.map(\.traceId))
    }

    func canStepInspector(_ offset: Int) -> Bool {
        guard let index = inspectorTurnIndex() else { return false }
        return turns.indices.contains(index + offset)
    }

    func stepInspector(_ offset: Int) {
        guard let index = inspectorTurnIndex(),
            turns.indices.contains(index + offset)
        else { return }
        let target = turns[index + offset]
        openInspector(traceId: target.traceId)
        if let range = turnRanges.first(where: { $0.traceId == target.traceId })?.range {
            scrollToRangeVersion += 1
            scrollToRangeCommand = (scrollToRangeVersion, range)
        }
    }

    func detailClient() -> AlexClient? { client() }

    func ensureFirstTraceDetail() {
        guard let sid = selectedSessionId,
            let traceId = transcriptMetadata.first?.traceId ?? turns.first?.traceId
        else { return }
        let key = "\(sid)|\(traceId)"
        guard key != firstDetailKey else { return }
        firstDetailKey = key
        firstTraceDetail = nil
        firstDetailTask?.cancel()
        firstDetailTask = Task { [weak self] in
            guard let client = self?.client() else { return }
            guard let detail = try? await client.traceDetail(id: traceId) else { return }
            guard !Task.isCancelled, let self, self.firstDetailKey == key else { return }
            self.firstTraceDetail = detail
        }
    }

    func revealSessionBodies(_ session: TraceSession) {
        launchAction { [weak self] in
            guard let self else { return }
            guard let client = client() else { return }
            var bodyPath: String?
            if let last = try? await allTranscriptMetadata(
                client: client, sessionId: session.sessionId).last,
                let detail = try? await client.traceDetail(id: last.traceId)
            {
                bodyPath = detail.trace.reqBodyPath
                    ?? detail.trace.respBodyPath ?? detail.trace.upstreamReqBodyPath
            }
            if let bodyPath {
                NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: bodyPath)])
            } else {
                let fallback = FileManager.default.homeDirectoryForCurrentUser
                    .appendingPathComponent(".alex/bodies")
                NSWorkspace.shared.activateFileViewerSelecting([fallback])
            }
        }
    }

    init(
        store: SnapshotStore, initialHarness: String? = nil, initialQuery: String? = nil,
        transcriptPageFetcher: TranscriptPageFetcher? = nil,
        traceTurnFetcher: TraceTurnFetcher? = nil
    ) {
        self.store = store
        self.transcriptPageFetcher = transcriptPageFetcher
        self.traceTurnFetcher = traceTurnFetcher
        if let initialQuery, !initialQuery.isEmpty {
            queryText = initialQuery
            parsedQuery = OmniQuery.parse(queryText)
        } else if let initialHarness {
            queryText = "harness:\(initialHarness)"
            parsedQuery = OmniQuery.parse(queryText)
        }
    }

    func setHarnessFilter(_ harness: String) {
        queryText = OmniQuery.settingToken(in: queryText, key: "harness", value: harness)
    }

    func setQueryFilter(_ query: String) {
        queryText = query
    }

    private func recomputeVisible(debounce: Bool = false) {
        sessionFilterTask?.cancel()
        sessionFilterGeneration += 1
        let generation = sessionFilterGeneration
        let input = SessionFilterInput(
            sessions: sessions, rowsById: rowsById, showPings: showPings,
            query: parsedQuery, serverMatches: searchSessionIds, sortOrder: sortOrder,
            nestSubagents: nestSubagents, collapsedRoots: collapsedLineageRoots,
            statusPill: statusPill)
        sessionFilterTask = Task { [weak self] in
            if debounce {
                try? await Task.sleep(for: .milliseconds(175))
                guard !Task.isCancelled else { return }
            }
            let rows = await Task.detached(priority: .userInitiated) {
                let start = ContinuousClock.now
                defer {
                    let elapsed = start.duration(to: .now)
                    BarLog.timing(
                        .browser, label: "session filter sessions=\(input.sessions.count)",
                        milliseconds: Double(elapsed.components.seconds) * 1000
                            + Double(elapsed.components.attoseconds) / 1e15)
                }
                let raw = SessionTable.visibleRows(
                    sessions: input.sessions, rowsById: input.rowsById, showPings: input.showPings,
                    query: input.query, serverMatches: input.serverMatches, sortOrder: input.sortOrder,
                    nestSubagents: input.nestSubagents, collapsedRoots: input.collapsedRoots)
                return Self.applyStatusPill(input.statusPill, rows: raw)
            }.value
            guard !Task.isCancelled, let self, generation == self.sessionFilterGeneration else { return }
            self.visibleRows = rows
        }
    }

    private struct SessionFilterInput: @unchecked Sendable {
        let sessions: [TraceSession]
        let rowsById: [String: SessionRow]
        let showPings: Bool
        let query: OmniQuery
        let serverMatches: Set<String>?
        let sortOrder: [KeyPathComparator<SessionRow>]
        let nestSubagents: Bool
        let collapsedRoots: Set<String>
        let statusPill: SessionStatusPill
    }

    /// Keeps rows matching the pill plus any descendants of a kept row so a
    /// nested lineage stays attached to its parent.
    nonisolated static func applyStatusPill(
        _ pill: SessionStatusPill, rows: [SessionRow], now: Date = Date()
    ) -> [SessionRow] {
        guard pill != .all else { return rows }
        var kept: [SessionRow] = []
        var keptIds = Set<String>()
        for row in rows {
            if let parent = row.parentSessionId, keptIds.contains(parent) {
                kept.append(row)
                keptIds.insert(row.id)
            } else if pill == .alexError
                ? row.isAlexError
                : pill.matches(SessionDisplayStatus.status(for: row, now: now))
            {
                kept.append(row)
                keptIds.insert(row.id)
            }
        }
        return kept
    }

    func isLineageCollapsed(_ sessionId: String) -> Bool {
        collapsedLineageRoots.contains(sessionId)
    }

    func toggleLineage(_ sessionId: String) {
        if !collapsedLineageRoots.insert(sessionId).inserted {
            collapsedLineageRoots.remove(sessionId)
        }
        recomputeVisible()
    }

    private var newestVisibleRow: SessionRow? {
        visibleRows.max { $0.lastTsMs < $1.lastTsMs }
    }

    var showsTagFilterBar: Bool {
        !sessions.isEmpty
    }

    func filterValues(_ dimension: TagFilterDimension) -> [String] {
        if dimension == .middleware {
            return middlewareRules.map(\.id).sorted {
                filterLabel(.middleware, value: $0).localizedCaseInsensitiveCompare(
                    filterLabel(.middleware, value: $1)) == .orderedAscending
            }
        }
        if dimension == .account {
            let known = Set(billingAccounts.map(\.id))
            let observed = dimension.values(in: sessions).filter { known.contains($0) }
            return Array(Set(observed).union(known)).sorted {
                AccountIdentity.name(accountId: $0, accounts: billingAccounts)
                    .localizedCaseInsensitiveCompare(
                        AccountIdentity.name(accountId: $1, accounts: billingAccounts))
                    == .orderedAscending
            }
        }
        return dimension.values(in: sessions)
    }

    func activeFilter(_ dimension: TagFilterDimension) -> String? {
        dimension.activeValue(in: parsedQuery)
    }

    func filterLabel(_ dimension: TagFilterDimension, value: String) -> String {
        if dimension == .middleware {
            return middlewareRules.first(where: { $0.id == value })?.name ?? value
        }
        if dimension == .account {
            return AccountIdentity.label(accountId: value, accounts: billingAccounts)
        }
        return dimension.label(for: value)
    }

    func accountIdentity(_ accountId: String?) -> String? {
        guard let accountId, !accountId.isEmpty else { return nil }
        guard billingAccounts.contains(where: { $0.id == accountId }) else { return nil }
        return AccountIdentity.label(accountId: accountId, accounts: billingAccounts)
    }

    func accountIdentity(_ accountIds: [String]) -> String? {
        AccountIdentity.summary(
            accountIds: billingAccountIds(accountIds), accounts: billingAccounts)
    }

    func accountNames(_ accountIds: [String]) -> String? {
        AccountIdentity.nameSummary(
            accountIds: billingAccountIds(accountIds), accounts: billingAccounts)
    }

    func internalRoute(_ accountId: String?) -> String? {
        guard let accountId, !accountId.isEmpty, accountIdentity(accountId) == nil else {
            return nil
        }
        return accountId
    }

    func harnessName(for trace: TraceDetail) -> String {
        HarnessName.display(
            harness: trace.harness ?? selectedSession?.harness,
            tags: selectedSession?.tags)
    }

    private var billingAccounts: [Account] {
        store.accounts.filter { $0.kind == "oauth" || $0.provider == "openrouter" }
    }

    private func billingAccountIds(_ accountIds: [String]) -> [String] {
        let known = Set(billingAccounts.map(\.id))
        return accountIds.filter { known.contains($0) }
    }

    func setFilter(_ dimension: TagFilterDimension, _ value: String?) {
        queryText = OmniQuery.settingToken(in: queryText, key: dimension.rawValue, value: value)
    }

    private func loadMiddlewareRules() async {
        guard let client = client() else { return }
        do {
            middlewareRules = try await client.middlewareStatus().rules
        } catch is CancellationError {
        } catch {
            BarLog.warn(.browser, "middleware filter rules unavailable: \(error.localizedDescription)")
        }
    }

    private(set) var errorClassSummaryLine: String?
    private(set) var transcriptToolCount = 0
    private(set) var transcriptSubagentCount = 0
    private(set) var transcriptTokensTotal: Int64 = 0

    private func recomputeSessionSummary() {
        let counts = sessions
            .filter { parsedQuery.matches($0) }
            .reduce(into: [String: Int64]()) { totals, session in
                for (errorClass, count) in session.errorClassCounts ?? [:] {
                    totals[errorClass, default: 0] += count
                }
            }
        let realCounts = counts.filter {
            $0.key != TraceClassification.clientDisconnectKind
        }
        guard !realCounts.isEmpty else {
            errorClassSummaryLine = nil
            return
        }
        let real = realCounts.values.reduce(0, +)
        let detail = realCounts.sorted { $0.key < $1.key }
            .map { "\($0.key) \($0.value)" }
            .joined(separator: " · ")
        errorClassSummaryLine = "\(real) errored · \(detail)"
    }

    var selectedSession: TraceSession? {
        sessions.first { $0.sessionId == selectedSessionId }
    }

    /// Recompute only when sessions/turns or the selected session changes,
    /// never from a SwiftUI view body.
    private func recomputeTranscriptSummary() {
        transcriptToolCount = turns.reduce(0) { $0 + TranscriptChatMessages.toolCount(for: $1) }
        transcriptTokensTotal = turns.reduce(0) {
            $0 + ($1.inputTokens ?? 0) + ($1.outputTokens ?? 0)
        }
        transcriptSubagentCount = selectedSessionId.map { sid in
            sessions.filter { $0.parentSessionId == sid }.count
        } ?? 0
    }

    func start() {
        stop()
        fixtureLoadTask = Task { [weak self] in
            await self?.loadSimulationFixtures()
            await self?.loadMiddlewareRules()
        }
        sessionsTask = Task { [weak self] in
            while !Task.isCancelled {
                await self?.pollSessions()
                try? await Task.sleep(for: .seconds(1))
            }
        }
        startTranscriptPolling()
        // `stop()` cancels any in-flight search when an existing window is
        // reopened. Re-apply the current query so initial/preset key filters
        // always populate their server-backed session id set.
        queryChanged()
    }

    func stop() {
        sessionsTask?.cancel()
        sessionsTask = nil
        transcriptTask?.cancel()
        transcriptTask = nil
        turnFetchTasks.values.forEach { $0.cancel() }
        turnFetchTasks.removeAll()
        expandedRebuildTask?.cancel()
        expandedRebuildTask = nil
        turnApplyTask?.cancel()
        turnApplyTask = nil
        turnApplyGeneration += 1
        actionTasks.values.forEach { $0.cancel() }
        actionTasks.removeAll()
        renderedArtifacts.clear()
        searchTask?.cancel()
        searchTask = nil
        renderChain?.cancel()
        renderChain = nil
        firstDetailTask?.cancel()
        firstDetailTask = nil
        transcriptFilterTask?.cancel()
        transcriptFilterTask = nil
        sessionFilterTask?.cancel()
        sessionFilterTask = nil
        fixtureLoadTask?.cancel()
        fixtureLoadTask = nil
        simulationNoticeTask?.cancel()
        simulationNoticeTask = nil
    }

    func selectFromUser(_ id: String) {
        apply(selectionState.userSelect(id))
    }

    func selectFromFollow(_ id: String) {
        apply(selectionState.followSelect(id))
    }

    private func apply(_ change: SessionSelection.Change) {
        guard case let .selected(id) = change else { return }
        // Any programmatic single-selection (follow, arrow-key nav, table
        // click) collapses a prior multi-selection so the table's highlight
        // and the transcript pane agree on one session again.
        multiSelection = [id]
        followedSubagentId = nil
        recomputeTranscriptSummary()
        resetTurns()
        setUserAtBottom(true)
        startTranscriptPolling()
    }

    private func resetTurns() {
        turnFetchTasks.values.forEach { $0.cancel() }
        turnFetchTasks.removeAll()
        expandedRebuildTask?.cancel()
        expandedRebuildTask = nil
        turnApplyTask?.cancel()
        turnApplyTask = nil
        turnApplyGeneration += 1
        forceChunkedPaneUpdate = false
        transcriptRendering = false
        lastTurnApplyBatchCharacterCounts = []
        lastChatPaneBatchCharacterCounts = []
        transcriptMetadata = []
        expandedTurns = [:]
        metadataHiddenTurnCount = 0
        allTurns = []
        turns = []
        transcriptLoadedSessionId = nil
        transcriptLoading = selectedSessionId != nil
        transcriptUnreachable = false
        inspectorToolBody = nil
        transcriptFilterTask?.cancel()
        transcriptEntries = []
        transcriptTotalCount = 0
        turnsFingerprint = ""
        if !detailsVisible {
            inspectorTraceId = nil
        }
        firstTraceDetail = nil
        firstDetailKey = nil
        firstDetailTask?.cancel()
        firstDetailTask = nil
        renderChain?.cancel()
        renderChain = nil
        renderState = TranscriptRender.state(for: [], rawMode: transcriptRawMode)
        windowStart = 0
        windowMaxTurns = TranscriptWindow.defaultMaxTurns
        hiddenTurnCount = 0
        turnRanges = []
        renderedLength = 0
        renderVersion += 1
        renderOp = (renderVersion, .set(NSAttributedString()))
    }

    func loadEarlierTurns() {
        if let first = transcriptMetadata.firstIndex(where: { expandedTurns[$0.traceId] != nil }),
            first > 0
        {
            requestTurnWindow(
                indices: max(0, first - TranscriptApplyPolicy.earlierTurnPageCount)..<first)
            return
        }
        windowMaxTurns += TranscriptWindow.defaultMaxTurns
        renderState = nil
        scheduleRender()
    }

    func requestScrollToBottom() {
        scrollCommand += 1
    }

    func moveSelection(_ move: ListNavigation.Move) {
        let visible = visibleRows
        let current = selectedSessionId.flatMap { id in
            visible.firstIndex { $0.id == id }
        }
        guard let index = ListNavigation.targetIndex(
            selected: current, count: visible.count, move: move)
        else { return }
        selectFromUser(visible[index].id)
    }

    private func scheduleRender() {
        let all = turns
        windowStart = min(windowStart, all.count)
        var windowed = Array(all[windowStart...])
        let rawMode = transcriptRawMode
        var plan = TranscriptRender.plan(previous: renderState, turns: windowed, rawMode: rawMode)
        if case .append = plan, windowed.count > windowMaxTurns + 100 {
            windowMaxTurns = TranscriptWindow.defaultMaxTurns
            plan = .rebuild
        }
        if plan == .rebuild {
            let budget = TranscriptWindow.defaultMaxChars
                * max(1, windowMaxTurns / TranscriptWindow.defaultMaxTurns)
            windowStart = TranscriptWindow.startIndex(
                turns: all, maxTurns: windowMaxTurns, maxChars: budget)
            windowed = Array(all[windowStart...])
            if windowStart > 0 {
                BarLog.info(
                    .browser, "transcript windowed: showing \(windowed.count)/\(all.count) turns")
            }
        }
        hiddenTurnCount = metadataHiddenTurnCount + windowStart
        guard plan != .unchanged else { return }
        renderState = TranscriptRender.state(for: windowed, rawMode: rawMode)
        let slice: [TranscriptTurn]
        let isAppend: Bool
        let firstNumber: Int
        switch plan {
        case .unchanged:
            return
        case .rebuild:
            slice = windowed
            isAppend = false
            firstNumber = windowStart + 1
        case let .append(from):
            slice = Array(windowed[from...])
            isAppend = true
            firstNumber = windowStart + from + 1
        }
        let sid = selectedSessionId
        let session = selectedSession
        let harnessName = HarnessName.display(harness: session?.harness, tags: session?.tags)
        let providerNames = Set(
            slice.compactMap {
                $0.provider ?? $0.model.flatMap(ModelProvider.provider(forModel:))
            })
        let icons = TranscriptIcons(
            harness: HarnessIconLoader.image(harness: session?.harness, tags: session?.tags),
            providers: Dictionary(uniqueKeysWithValues: providerNames.map {
                ($0, ProviderChipRenderer.image(for: $0))
            }))
        let prev = renderChain
        renderChain = Task { [weak self] in
            await prev?.value
            let built = await Task.detached { () -> BuiltDocument in
                let start = ContinuousClock.now
                let interval = TraceBrowserSignpost.begin(
                    .transcriptRenderBuild,
                    "turns=\(slice.count) first=\(firstNumber) raw=\(rawMode)")
                let doc = TranscriptRender.build(
                    turns: slice, firstTurnNumber: firstNumber, harnessName: harnessName,
                    icons: icons, rawMode: rawMode)
                let elapsed = start.duration(to: .now)
                let ms = Int(elapsed.components.seconds * 1000)
                    + Int(elapsed.components.attoseconds / 1_000_000_000_000_000)
                TraceBrowserSignpost.end(interval, "chars=\(doc.text.length)")
                return BuiltDocument(doc: doc, ms: ms)
            }.value
            let (doc, ms) = (built.doc, built.ms)
            let label = "render build turns=\(slice.count) append=\(isAppend) len=\(doc.text.length) \(ms)ms"
            if Double(ms) >= BarLog.slowThresholdMs {
                BarLog.warn(.browser, "SLOW \(label)")
            } else {
                BarLog.info(.browser, label)
            }
            guard let self, !Task.isCancelled, self.selectedSessionId == sid else { return }
            self.renderVersion += 1
            if isAppend {
                self.turnRanges += TranscriptRender.shifted(doc.turnRanges, by: self.renderedLength)
                self.renderedLength += doc.text.length
                self.renderOp = (self.renderVersion, .append(doc.text))
            } else {
                self.turnRanges = doc.turnRanges
                self.renderedLength = doc.text.length
                self.renderOp = (self.renderVersion, .set(doc.text))
            }
        }
    }

    func setLive(_ live: Bool) {
        apply(selectionState.setLive(live, newestVisibleId: newestVisibleRow?.id))
    }

    private func client() -> AlexClient? {
        // SnapshotStore owns config discovery/refresh. Reading and reparsing
        // config.toml here used to synchronously hit disk on every 1s/500ms
        // poll, directly on the main actor.
        guard let cfg = store.config else { return nil }
        return AlexClient(config: cfg)
    }

    private func launchAction(_ operation: @escaping @MainActor () async -> Void) {
        let id = UUID()
        actionTasks[id] = Task { [weak self] in
            await operation()
            self?.actionTasks[id] = nil
        }
    }

    private func allTranscriptMetadata(
        client: AlexClient, sessionId: String
    ) async throws -> [TranscriptTurnMetadata] {
        var output: [TranscriptTurnMetadata] = []
        var cursor: TranscriptCursor?
        repeat {
            let page = try await client.traceTranscriptPage(
                sessionId: sessionId, limit: 50, cursor: cursor)
            try Task.checkCancellation()
            output.append(contentsOf: page.turns)
            cursor = page.nextCursor
            if !page.hasMore { break }
        } while cursor != nil
        return output
    }

    private func startTranscriptPolling() {
        transcriptTask?.cancel()
        transcriptTask = Task { [weak self] in
            while !Task.isCancelled {
                await self?.pollTranscript()
                try? await Task.sleep(for: .milliseconds(500))
            }
        }
    }

    private func pollSessions() async {
        guard let client = client() else {
            daemonDown = true
            sessionsLoading = false
            sessionsUnreachable = true
            return
        }
        let wasInitialLoad = sessionsLoading
        if wasInitialLoad {
            Task { [weak self] in
                try? await Task.sleep(for: .milliseconds(1500))
                guard !Task.isCancelled, let self, self.sessionsLoading else { return }
                self.sessionsUnreachable = true
            }
        }
        do {
            let middlewareId = parsedQuery.middleware
            let fetched = try await client.traceSessions(
                since: "24h", limit: 200, middlewareId: middlewareId)
            guard middlewareId == parsedQuery.middleware else { return }
            lastSessionsMiddlewareId = middlewareId
            daemonDown = false
            sessionsLoading = false
            sessionsUnreachable = false
            let fingerprint = TraceFingerprint.sessions(fetched)
            if fingerprint != sessionsFingerprint {
                sessionsFingerprint = fingerprint
                BarLog.measure(.browser, label: "sessions apply count=\(fetched.count)") {
                    sessions = fetched.sorted { $0.lastTsMs > $1.lastTsMs }
                    rowsById = SessionTable.rowsById(sessions)
                    recomputeVisible()
                }
            }
            if let pending = pendingSelectSessionId,
                sessions.contains(where: { $0.sessionId == pending })
            {
                selectSessionWhenLoaded(pending)
            }
            applyLiveFollow()
        } catch {
            guard !(error is CancellationError) else { return }
            daemonDown = true
            sessionsLoading = false
            sessionsUnreachable = true
        }
    }

    private func applyLiveFollow() {
        guard selectedSessionId == nil, !pinned, !isMultiSelected,
            let candidate = newestVisibleRow
        else { return }
        selectFromFollow(candidate.id)
    }

    private func pollTranscript() async {
        guard let sid = selectedSessionId else { return }
        guard transcriptPageFetcher != nil || client() != nil else {
            daemonDown = true
            transcriptLoading = false
            transcriptUnreachable = true
            return
        }
        let needsInitialLoad = transcriptLoadedSessionId != sid
        if needsInitialLoad { transcriptLoading = true }
        if needsInitialLoad {
            Task { [weak self] in
                try? await Task.sleep(for: .milliseconds(1500))
                guard !Task.isCancelled, let self, self.selectedSessionId == sid,
                    self.transcriptLoading
                else { return }
                self.transcriptUnreachable = true
            }
        }
        do {
            var fetched: [TranscriptTurnMetadata] = []
            var cursor: TranscriptCursor?
            repeat {
                let page: TranscriptPageResponse
                if let transcriptPageFetcher {
                    page = try await transcriptPageFetcher(sid, 50, cursor)
                } else if let client = client() {
                    page = try await client.traceTranscriptPage(
                        sessionId: sid, limit: 50, cursor: cursor)
                } else {
                    return
                }
                try Task.checkCancellation()
                guard page.sessionId == sid, selectedSessionId == sid else { return }
                fetched.append(contentsOf: page.turns)
                cursor = page.nextCursor
                if !page.hasMore { break }
            } while cursor != nil
            daemonDown = false
            guard selectedSessionId == sid else { return }
            let previous = Dictionary(uniqueKeysWithValues: transcriptMetadata.map {
                ($0.traceId, $0)
            })
            let changed = Set(fetched.compactMap { metadata in
                previous[metadata.traceId] == metadata ? nil : metadata.traceId
            })
            let validIds = Set(fetched.map(\.traceId))
            expandedTurns = expandedTurns.filter { validIds.contains($0.key) }
            transcriptMetadata = fetched
            transcriptLoadedSessionId = sid
            transcriptLoading = !fetched.isEmpty
            transcriptUnreachable = false
            transcriptTabCounts = nil
            if fetched.isEmpty {
                rebuildExpandedTurns()
                return
            }
            let start = max(0, fetched.count - TranscriptApplyPolicy.initialTurnCount)
            requestTurnWindow(indices: start..<fetched.count)
            for traceId in changed where expandedTurns[traceId] != nil {
                requestTurn(traceId: traceId, sessionId: sid, replacing: true)
            }
            ensureFirstTraceDetail()
        } catch {
            guard !(error is CancellationError) else { return }
            daemonDown = true
            transcriptLoading = false
            transcriptUnreachable = true
        }
    }

    func retrySessions() { launchAction { [weak self] in await self?.pollSessions() } }
    func retryTranscript() { startTranscriptPolling() }

    func transcriptTurnBecameVisible(_ traceId: String) {
        guard !userAtBottom else { return }
        guard let index = transcriptMetadata.firstIndex(where: { $0.traceId == traceId }) else {
            return
        }
        requestTurnWindow(
            indices: max(0, index - TranscriptApplyPolicy.earlierTurnPageCount)..<index)
    }

    private func requestTurnWindow(indices: Range<Int>) {
        guard let sid = selectedSessionId else { return }
        for index in indices where transcriptMetadata.indices.contains(index) {
            requestTurn(traceId: transcriptMetadata[index].traceId, sessionId: sid)
        }
    }

    private func requestTurn(traceId: String, sessionId: String, replacing: Bool = false) {
        if !replacing, expandedTurns[traceId] != nil { return }
        turnFetchTasks[traceId]?.cancel()
        turnFetchTasks[traceId] = Task { [weak self] in
            guard let self else { return }
            let interval = TraceBrowserSignpost.begin(.turnFetch, "trace_id=\(traceId)")
            var fetchEnded = false
            do {
                let turn: TranscriptTurn
                let byteCount: Int
                if let traceTurnFetcher {
                    turn = try await traceTurnFetcher(traceId)
                    byteCount = (try? JSONEncoder().encode(turn).count) ?? 0
                } else if let client = client() {
                    let payload = try await client.traceTurnPayload(id: traceId)
                    turn = payload.turn
                    byteCount = payload.byteCount
                } else {
                    TraceBrowserSignpost.end(interval, "bytes=0 unavailable=true")
                    return
                }
                TraceBrowserSignpost.end(interval, "bytes=\(byteCount)")
                fetchEnded = true
                try Task.checkCancellation()
                guard selectedSessionId == sessionId, turn.traceId == traceId else { return }
                expandedTurns[traceId] = turn
                turnFetchTasks[traceId] = nil
                scheduleExpandedRebuild()
            } catch is CancellationError {
                if !fetchEnded { TraceBrowserSignpost.end(interval, "cancelled=true") }
                turnFetchTasks[traceId] = nil
            } catch {
                if !fetchEnded { TraceBrowserSignpost.end(interval, "error=true") }
                turnFetchTasks[traceId] = nil
                guard selectedSessionId == sessionId else { return }
                transcriptUnreachable = true
            }
        }
    }

    private func scheduleExpandedRebuild() {
        expandedRebuildTask?.cancel()
        expandedRebuildTask = Task { [weak self] in
            try? await Task.sleep(for: .milliseconds(15))
            guard !Task.isCancelled else { return }
            self?.rebuildExpandedTurns()
        }
    }

    private func rebuildExpandedTurns() {
        let loaded = transcriptMetadata.compactMap { expandedTurns[$0.traceId] }
        allTurns = loaded
        if let first = transcriptMetadata.firstIndex(where: { expandedTurns[$0.traceId] != nil }) {
            metadataHiddenTurnCount = first
        } else {
            metadataHiddenTurnCount = transcriptMetadata.count
        }
        transcriptLoading = !transcriptMetadata.isEmpty && loaded.isEmpty
        turnsFingerprint = TraceFingerprint.turns(loaded)
        applyTurnFilter()
    }

    private func queryChanged() {
        searchTask?.cancel()
        let query = parsedQuery
        if query.middleware != lastSessionsMiddlewareId {
            sessionsFingerprint = ""
            Task { [weak self] in await self?.pollSessions() }
        }
        recomputeSessionSummary()
        applyTurnFilter()
        guard !query.freeText.isEmpty || query.key != nil else {
            searchSessionIds = nil
            searchInFlight = false
            recomputeVisible(debounce: true)
            return
        }
        recomputeVisible(debounce: true)
        searchTask = Task { [weak self] in
            try? await Task.sleep(for: .milliseconds(400))
            guard !Task.isCancelled else { return }
            self?.searchInFlight = true
            await self?.runSearch(query)
        }
    }

    private func applyTurnFilter() {
        let filtered = parsedQuery.effort == nil && parsedQuery.account == nil
            ? allTurns
            : allTurns.filter(parsedQuery.matches)
        defer { retargetInspector() }
        turnApplyTask?.cancel()
        turnApplyGeneration += 1
        let generation = turnApplyGeneration
        let requiresIncremental = TranscriptApplyPolicy.requiresIncrementalApply(filtered)
        transcriptRendering = !filtered.isEmpty && requiresIncremental
        forceChunkedPaneUpdate = requiresIncremental
        turnApplyTask = Task { [weak self] in
            let capped = await Task.detached(priority: .userInitiated) {
                filtered.map(TranscriptInlineDisplay.capped)
            }.value
            guard let self, !Task.isCancelled, generation == self.turnApplyGeneration else {
                return
            }
            if capped == self.turns {
                self.transcriptRendering = false
                self.forceChunkedPaneUpdate = false
                return
            }
            self.lastTurnApplyBatchCharacterCounts = []
            guard requiresIncremental else {
                self.commitDisplayTurns(
                    capped,
                    batchChars: capped.reduce(0) {
                        $0 + TranscriptApplyPolicy.inlineCharacterCount($1)
                    }, resetRenderState: true)
                return
            }
            let existing = self.turns
            let prepending = !existing.isEmpty
                && capped.count >= existing.count
                && Array(capped.suffix(existing.count)) == existing
            let appending = capped.count >= existing.count
                && Array(capped.prefix(existing.count)) == existing
            var applied = appending || prepending ? existing : []
            let pending: [TranscriptTurn]
            if appending {
                pending = Array(capped.dropFirst(existing.count))
            } else if prepending {
                pending = Array(capped.dropLast(existing.count))
            } else {
                pending = capped
            }
            let batches = TranscriptApplyPolicy.turnBatches(pending)
            let orderedBatches = prepending ? Array(batches.reversed()) : batches
            for batch in orderedBatches {
                guard !Task.isCancelled, generation == self.turnApplyGeneration else { return }
                if prepending {
                    applied.insert(contentsOf: batch, at: 0)
                } else {
                    applied.append(contentsOf: batch)
                }
                self.commitDisplayTurns(
                    applied,
                    batchChars: batch.reduce(0) {
                        $0 + TranscriptApplyPolicy.inlineCharacterCount($1)
                    }, resetRenderState: prepending || (!appending && applied.count == batch.count))
                if applied.count < capped.count {
                    await Task.yield()
                    try? await Task.sleep(for: .milliseconds(1))
                }
            }
        }
    }

    private func commitDisplayTurns(
        _ value: [TranscriptTurn], batchChars: Int, resetRenderState: Bool
    ) {
        let totalChars = value.reduce(0) {
            $0 + TranscriptApplyPolicy.inlineCharacterCount($1)
        }
        let interval = TraceBrowserSignpost.begin(
            .transcriptApply,
            "turns=\(value.count) total_chars=\(totalChars) batch_chars=\(batchChars)")
        turns = value
        lastTurnApplyBatchCharacterCounts.append(batchChars)
        if resetRenderState { renderState = nil }
        scheduleRender()
        DispatchQueue.main.async { TraceBrowserSignpost.end(interval) }
    }

    private func runSearch(_ query: OmniQuery) async {
        guard let client = client() else {
            searchInFlight = false
            return
        }
        let start = ContinuousClock.now
        do {
            let resp = try await client.searchTraces(text: query.freeText, filters: query)
            guard parsedQuery == query else { return }
            searchSessionIds = Set(resp.traces.compactMap(\.sessionId))
            searchMatchCount = resp.traces.count
            searchScanned = resp.scanned ?? 0
            let elapsed = start.duration(to: .now)
            BarLog.info(.browser, "search \"\(query.freeText)\" matches=\(resp.traces.count) scanned=\(resp.scanned ?? 0) in \(elapsed.components.seconds * 1000 + Int64(elapsed.components.attoseconds / 1_000_000_000_000_000))ms")
        } catch {
            guard parsedQuery == query else { return }
            // Degrade silently to metadata-only matching (tag matches still
            // apply via `OmniQuery.isVisible`; server-side body matches just
            // don't contribute) rather than surfacing an error UI.
            searchSessionIds = []
            searchMatchCount = 0
            searchScanned = 0
            BarLog.warn(.browser, "search \"\(query.freeText)\" failed: \(error.localizedDescription)")
        }
        guard parsedQuery == query else { return }
        searchInFlight = false
        recomputeVisible()
    }

    func copySessionId(_ session: TraceSession) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(session.sessionId, forType: .string)
    }

    func copyForkCommand(_ session: TraceSession) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(
            SessionForkCommand.command(sessionId: session.sessionId), forType: .string)
    }

    /// Fetches the fixture menu once per window opening. The menu reads this
    /// small cache so right-clicking a row stays instant.
    func loadSimulationFixtures() async {
        guard let client = client() else {
            fixtureLoadError = "daemon unavailable"
            return
        }
        fixturesLoading = true
        fixtureLoadError = nil
        defer { fixturesLoading = false }
        do {
            simulationFixtures = try await client.errorSimulationFixtures()
        } catch is CancellationError {
            return
        } catch {
            fixtureLoadError = error.localizedDescription
        }
    }

    func injectFixture(_ fixture: ErrorSimulationFixture, into session: TraceSession) {
        guard !session.sessionId.isEmpty else {
            showSimulationNotice("Cannot simulate: no session id")
            return
        }
        launchAction { [weak self] in
            guard let self else { return }
            guard let client = self.client() else {
                self.showSimulationNotice("Simulation failed: daemon unavailable")
                return
            }
            do {
                try await client.injectFixture(sessionId: session.sessionId, fixture: fixture.name)
                self.showSimulationNotice("Queued \(fixture.name)")
                await self.pollSessions()
            } catch is CancellationError {
                return
            } catch {
                self.showSimulationNotice("Simulation failed: \(error.localizedDescription)")
            }
        }
    }

    func clearFixtureInjections(for session: TraceSession) {
        guard !session.sessionId.isEmpty else {
            showSimulationNotice("Cannot clear: no session id")
            return
        }
        launchAction { [weak self] in
            guard let self else { return }
            guard let client = self.client() else {
                self.showSimulationNotice("Clear failed: daemon unavailable")
                return
            }
            do {
                try await client.clearFixtureInjections(sessionId: session.sessionId)
                self.showSimulationNotice("Cleared pending injections")
                await self.pollSessions()
            } catch is CancellationError {
                return
            } catch {
                self.showSimulationNotice("Clear failed: \(error.localizedDescription)")
            }
        }
    }

    func promptSaveFixture(from session: TraceSession) {
        guard TraceClassification.realErrorCount(
            total: session.errors, errorClassCounts: session.errorClassCounts) > 0
        else { return }
        let field = NSTextField(string: "")
        field.placeholderString = "fixture name"
        field.frame = NSRect(x: 0, y: 0, width: 260, height: 24)
        let alert = NSAlert()
        alert.messageText = "Save error as fixture"
        alert.informativeText = "Capture a response error from this session for later simulation."
        alert.accessoryView = field
        alert.addButton(withTitle: "Save")
        alert.addButton(withTitle: "Cancel")
        NSApp.activate(ignoringOtherApps: true)
        guard alert.runModal() == .alertFirstButtonReturn else { return }
        let name = field.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty else {
            showSimulationNotice("Fixture name is required")
            return
        }
        launchAction { [weak self] in
            guard let self else { return }
            guard let client = self.client() else {
                self.showSimulationNotice("Save failed: daemon unavailable")
                return
            }
            do {
                let metadata = try await self.allTranscriptMetadata(
                    client: client, sessionId: session.sessionId)
                guard let errorTrace = metadata.reversed().first(where: {
                    TraceClassification.isError(
                        status: $0.status, errorKind: $0.errorKind, error: $0.error)
                }) else {
                    self.showSimulationNotice("Save failed: no error trace found")
                    return
                }
                try await client.createErrorSimulationFixture(name: name, fromTraceId: errorTrace.traceId)
                await self.loadSimulationFixtures()
                self.showSimulationNotice("Saved fixture \(name)")
            } catch is CancellationError {
                return
            } catch {
                self.showSimulationNotice("Save failed: \(error.localizedDescription)")
            }
        }
    }

    private func showSimulationNotice(_ message: String) {
        simulationNotice = message
        simulationNoticeTask?.cancel()
        simulationNoticeTask = Task { [weak self] in
            try? await Task.sleep(for: .seconds(3))
            guard !Task.isCancelled else { return }
            self?.simulationNotice = nil
        }
    }

    func approveRejectedClient(_ session: TraceSession) {
        guard let fingerprint = session.approvableCredentialFingerprint else {
            showSimulationNotice("This rejected credential cannot be safely approved")
            return
        }
        launchAction { [weak self] in
            guard let self else { return }
            guard let client = self.client() else {
                self.showSimulationNotice("Approval failed: daemon unavailable")
                return
            }
            do {
                try await client.approveAlexErrorCredential(fingerprint: fingerprint)
                self.sessionsFingerprint = ""
                await self.pollSessions()
                self.showSimulationNotice("Client approved — retry its request")
            } catch is CancellationError {
                return
            } catch {
                self.showSimulationNotice("Approval failed: \(error.localizedDescription)")
            }
        }
    }

    func copyLastReply(_ session: TraceSession) {
        launchAction { [weak self] in
            guard let self else { return }
            guard let client = client() else { return }
            do {
                let metadata = try await allTranscriptMetadata(
                    client: client, sessionId: session.sessionId)
                guard let last = metadata.last else { return }
                let markdown = try await client.traceReplyMarkdown(traceId: last.traceId)
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(markdown, forType: .string)
            } catch {
                NSSound.beep()
            }
        }
    }

    func exportSession(_ session: TraceSession) {
        let panel = NSSavePanel()
        panel.nameFieldStringValue = "\(session.sessionId).md"
        panel.allowedContentTypes = [.init(filenameExtension: "md") ?? .plainText]
        NSApp.activate(ignoringOtherApps: true)
        guard panel.runModal() == .OK, let dest = panel.url else { return }
        launchAction { [weak self] in
            guard let self else { return }
            guard let client = client() else { return }
            do {
                let metadata = try await allTranscriptMetadata(
                    client: client, sessionId: session.sessionId)
                var turns: [TranscriptTurn] = []
                for item in metadata {
                    turns.append(try await client.traceTurn(id: item.traceId))
                    try Task.checkCancellation()
                }
                let markdown = Self.exportMarkdown(
                    sessionId: session.sessionId, turns: turns)
                try markdown.write(to: dest, atomically: true, encoding: .utf8)
            } catch {
                NSSound.beep()
            }
        }
    }

    static func exportMarkdown(sessionId: String, turns: [TranscriptTurn]) -> String {
        var out = "# Session \(sessionId)\n"
        let formatter = ISO8601DateFormatter()
        for turn in turns {
            let ts = formatter.string(
                from: Date(timeIntervalSince1970: Double(turn.tsRequestMs) / 1000))
            var header = "\n## \(ts)"
            if let model = turn.model { header += " · \(model)" }
            if let status = turn.status { header += " · \(status)" }
            out += header + "\n"
            if let user = turn.user, !user.isEmpty {
                out += "\n**User:**\n\n\(user)\n"
            }
            if let assistant = turn.assistant, !assistant.isEmpty {
                out += "\n**Assistant:**\n\n\(assistant)\n"
            }
            if TraceClassification.isClientDisconnect(errorKind: turn.errorKind) {
                out += "\n**Event:** client closed\n"
            } else if let error = turn.error, !error.isEmpty {
                out += "\n**Error:** \(error)\n"
            }
        }
        return out
    }

    /// Single-session delete, e.g. from the row context menu. Routes through
    /// the same confirm+bulk-delete flow as the Delete key / multi-select
    /// bulk delete so there's exactly one confirmation dialog in the app.
    func deleteSessionTraces(_ session: TraceSession) {
        confirmDeleteSessions([session])
    }

    /// Delete-key handler for the sessions table: acts on the full
    /// multi-selection when more than one row is selected, otherwise on the
    /// single selected session — same confirmation dialog either way.
    func deleteSelectedSessions() {
        let ids = isMultiSelected ? multiSelection : Set(selectedSessionId.map { [$0] } ?? [])
        guard !ids.isEmpty else { return }
        confirmDeleteSessions(sessions.filter { ids.contains($0.sessionId) })
    }

    private func confirmDeleteSessions(_ toDelete: [TraceSession]) {
        guard !toDelete.isEmpty else { return }
        let alert = NSAlert()
        if toDelete.count == 1, let session = toDelete.first {
            alert.messageText = "Delete all traces for this session?"
            alert.informativeText =
                "Removes \(session.traceCount) trace(s) of session \(session.sessionId) from the daemon. This cannot be undone."
        } else {
            let totalTraces = toDelete.reduce(0) { $0 + $1.traceCount }
            alert.messageText = "Delete all traces for \(toDelete.count) sessions?"
            alert.informativeText =
                "Removes \(totalTraces) trace(s) across \(toDelete.count) sessions from the daemon. This cannot be undone."
        }
        alert.alertStyle = .warning
        alert.addButton(withTitle: "Delete")
        alert.addButton(withTitle: "Cancel")
        NSApp.activate(ignoringOtherApps: true)
        guard alert.runModal() == .alertFirstButtonReturn else { return }

        let deletedIds = Set(toDelete.map(\.sessionId))
        // Captured before the (async) delete so we can restore the list to
        // roughly the same visual position afterwards instead of it jumping
        // to the top — SwiftUI's Table reloads/re-sorts on any data change
        // rather than diffing incrementally, so without this the whole list
        // scrolls back to row 0 every time.
        let anchorId = Self.nearestSurvivor(deletedIds: deletedIds, in: visibleRows)

        launchAction { [weak self] in
            guard let self else { return }
            guard let client = client() else { return }
            var completedSessionIds = Set<String>()
            var deletionFailures = 0
            var transcriptFailures = 0
            var orphanCount = 0
            for session in toDelete {
                var previousIds: Set<String>?
                while true {
                    let ids: Set<String>
                    do {
                        let metadata = try await allTranscriptMetadata(
                            client: client, sessionId: session.sessionId)
                        ids = Set(metadata.map(\.traceId))
                    } catch {
                        transcriptFailures += 1
                        break
                    }
                    if ids.isEmpty {
                        completedSessionIds.insert(session.sessionId)
                        break
                    }
                    if ids == previousIds {
                        orphanCount += ids.count
                        break
                    }
                    previousIds = ids

                    var deletedThisPass = 0
                    for id in ids {
                        do {
                            try await client.deleteTrace(id: id)
                            deletedThisPass += 1
                        } catch {
                            deletionFailures += 1
                        }
                    }
                    if deletedThisPass == 0 {
                        orphanCount += ids.count
                        break
                    }
                }
            }
            if let sid = selectedSessionId, completedSessionIds.contains(sid) {
                selectionState.clear()
                resetTurns()
            }
            multiSelection.subtract(completedSessionIds)
            await pollSessions()
            if deletionFailures > 0 || transcriptFailures > 0 || orphanCount > 0 {
                let failureAlert = NSAlert()
                failureAlert.messageText = "Couldn’t delete all session traces"
                var details: [String] = []
                if deletionFailures > 0 {
                    details.append("\(deletionFailures) trace deletion(s) failed")
                }
                if orphanCount > 0 {
                    details.append("\(orphanCount) trace(s) remain")
                }
                if transcriptFailures > 0 {
                    details.append(
                        "\(transcriptFailures) transcript fetch(es) failed, so remaining traces could not be verified")
                }
                failureAlert.informativeText = details.joined(separator: ". ") + "."
                failureAlert.alertStyle = .critical
                failureAlert.addButton(withTitle: "OK")
                NSApp.activate(ignoringOtherApps: true)
                failureAlert.runModal()
            }
            guard let anchorId, visibleRows.contains(where: { $0.id == anchorId }) else { return }
            requestScrollAnchor(anchorId)
            if selectedSessionId == nil, !isMultiSelected {
                selectFromUser(anchorId)
            }
        }
    }

    /// Nearest row that will still exist after removing `deletedIds`,
    /// searching outward from the first deleted row's position (checking
    /// the row right after it, then right before, then two after, …) so the
    /// scroll/selection lands as close as possible to where the deleted
    /// block was.
    static func nearestSurvivor(deletedIds: Set<String>, in rows: [SessionRow]) -> String? {
        guard let anchorIndex = rows.firstIndex(where: { deletedIds.contains($0.id) }) else {
            return nil
        }
        for offset in 1..<rows.count {
            let after = anchorIndex + offset
            if after < rows.count, !deletedIds.contains(rows[after].id) { return rows[after].id }
            let before = anchorIndex - offset
            if before >= 0, !deletedIds.contains(rows[before].id) { return rows[before].id }
        }
        return nil
    }

    private(set) var scrollAnchorId: String?
    private(set) var scrollAnchorVersion = 0

    private func requestScrollAnchor(_ id: String) {
        scrollAnchorId = id
        scrollAnchorVersion += 1
    }

    /// Session id a caller (e.g. the status-item menu's "recent session"
    /// rows, via `TraceBrowserWindowController.show(selectSessionId:)`)
    /// asked to land on before the session list had loaded. Consumed by the
    /// next `pollSessions()` that finds it.
    private var pendingSelectSessionId: String?

    /// Selects `sessionId` once it's present in `sessions` and scrolls it
    /// into view (reusing the delete flow's scroll-anchor mechanism), rather
    /// than opening the browser un-targeted. Selects immediately if the
    /// session list is already loaded.
    func selectSessionWhenLoaded(_ sessionId: String) {
        guard sessions.contains(where: { $0.sessionId == sessionId }) else {
            pendingSelectSessionId = sessionId
            return
        }
        pendingSelectSessionId = nil
        selectFromUser(sessionId)
        requestScrollAnchor(sessionId)
    }
}

private struct BuiltDocument: @unchecked Sendable {
    let doc: TranscriptDocument
    let ms: Int
}

/// Quick session status filter (mock TB App.tsx:259-260).
enum SessionStatusPill: Int, CaseIterable {
    case all
    case alexError
    case running
    case error
    case done

    var title: String {
        switch self {
        case .all: "All"
        case .alexError: "Alex Error"
        case .running: "Running"
        case .error: "Error"
        case .done: "Done"
        }
    }

    func matches(_ status: DisplayStatus) -> Bool {
        switch self {
        case .all: true
        case .alexError: false
        case .running: status == .running
        case .error: status == .error
        case .done: status == .success
        }
    }
}

/// Derives the mock's session status from real fields: errors → error,
/// 0 turns → pending, recent activity → running, else success (spec §2.1).
enum SessionDisplayStatus {
    static let runningWindowMs: Int64 = 120_000

    static func status(for row: SessionRow, now: Date = Date()) -> DisplayStatus {
        if row.errors > 0 { return .error }
        if row.turns == 0 { return .pending }
        let nowMs = Int64(now.timeIntervalSince1970 * 1000)
        if nowMs - row.lastTsMs < runningWindowMs { return .running }
        return .success
    }
}

/// Local mirror of the session short-id rule used by `SessionRow` in Core.
enum SessionShortId {
    static func shorten(_ id: String, maxLength: Int = 22) -> String {
        guard id.count > maxLength else { return id }
        return "\(id.prefix(10))…\(id.suffix(8))"
    }
}

struct TraceBrowserView: View {
    @Bindable var model: TraceBrowserModel
    @AppStorage("TraceBrowserDetailsOn") private var persistedDetailsOn = false

    var body: some View {
        VStack(spacing: 0) {
            toolbar
            if model.showsTagFilterBar {
                TagFilterBar(model: model)
            }
            Divider()
            if model.daemonDown {
                banner
                Divider()
            }
            HSplitView {
                SessionListView(model: model)
                    .frame(
                        minWidth: 280, idealWidth: SplitStore.leftWidth(),
                        maxWidth: 1200, maxHeight: .infinity)
                    .background(
                        GeometryReader { proxy in
                            Color.clear.onChange(of: proxy.size.width) { _, width in
                                SplitStore.saveLeftWidth(width)
                            }
                        })
                    .background(DefaultArrowCursorRegion())
                TranscriptView(model: model)
                    .frame(minWidth: 280, maxWidth: .infinity, maxHeight: .infinity)
                    .background(DefaultArrowCursorRegion())
                if model.detailsVisible {
                    if let route = model.inspectorToolBody {
                        ToolBodyInspectorView(route: route, model: model)
                            .frame(
                                minWidth: 300, idealWidth: 340, maxWidth: 520,
                                maxHeight: .infinity)
                            .background(DefaultArrowCursorRegion())
                    } else if let traceId = model.inspectorTraceId {
                        TraceInspectorView(traceId: traceId, model: model)
                            .frame(
                                minWidth: 300, idealWidth: 340, maxWidth: 520,
                                maxHeight: .infinity)
                            .background(DefaultArrowCursorRegion())
                    } else {
                        TraceInspectorPlaceholderView(model: model)
                            .frame(
                                minWidth: 300, idealWidth: 340, maxWidth: 520,
                                maxHeight: .infinity)
                            .background(DefaultArrowCursorRegion())
                    }
                }
            }
        }
        .frame(minWidth: 720, minHeight: 400)
        .overlay(alignment: .bottom) {
            if let notice = model.simulationNotice {
                Text(notice)
                    .font(.system(size: 11, weight: .medium))
                    .foregroundStyle(AlexTheme.Colors.foreground)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 8)
                    .background(.regularMaterial, in: Capsule())
                    .padding(.bottom, 12)
                    .accessibilityLabel("Error simulation: \(notice)")
            }
        }
        .onAppear {
            model.setDetailsVisible(persistedDetailsOn)
        }
        .onChange(of: persistedDetailsOn) { _, value in
            model.setDetailsVisible(value)
        }
        .onChange(of: model.detailsVisible) { _, value in
            if persistedDetailsOn != value {
                persistedDetailsOn = value
            }
        }
    }

    private var detailsBinding: Binding<Bool> {
        Binding(
            get: { model.detailsVisible },
            set: { model.setDetailsVisible($0) })
    }

    private var toolbar: some View {
        HStack(spacing: 12) {
            HStack(spacing: AlexTheme.Spacing.md) {
                Image(systemName: "magnifyingglass")
                    .font(.system(size: 11))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
                TextField(
                    "Search — free text + model: harness: account: effort: duration: task: job: tag:key=value status: run: session:",
                    text: $model.queryText
                )
                .textFieldStyle(.plain)
                .font(AlexTheme.Fonts.mono(11))
                .foregroundStyle(AlexTheme.Colors.foreground)
                .onExitCommand { model.queryText = "" }
                if model.searchInFlight {
                    ProgressView()
                        .controlSize(.small)
                        .scaleEffect(0.5)
                        .frame(width: 12, height: 12)
                        .help("Searching message bodies…")
                }
                if !model.queryText.isEmpty {
                    Button {
                        model.queryText = ""
                    } label: {
                        Image(systemName: "xmark.circle.fill")
                            .foregroundStyle(AlexTheme.Colors.textTertiary)
                    }
                    .buttonStyle(.plain)
                }
            }
            .padding(.horizontal, AlexTheme.Spacing.ml)
            .frame(height: 28)
            .background(
                RoundedRectangle(cornerRadius: AlexTheme.Radius.md)
                    .fill(AlexTheme.Colors.surfaceHover))
            .overlay(
                RoundedRectangle(cornerRadius: AlexTheme.Radius.md)
                    .strokeBorder(AlexTheme.Colors.cardBorder))
            Label("Live", systemImage: "dot.radiowaves.left.and.right")
                .font(.system(size: 11, weight: .medium))
                .foregroundStyle(.green)
                .help("The selected transcript refreshes automatically")
            Toggle("Details", isOn: detailsBinding)
                .toggleStyle(.switch)
                .controlSize(.small)
                .help("Show turn details")
            if let summary = model.errorClassSummaryLine {
                Text(summary)
                    .font(.system(size: 10, weight: .medium))
                    .foregroundStyle(AlexTheme.Colors.textSecondary)
                    .lineLimit(1)
                    .help("Errored traces grouped by Alex error class")
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
    }

    private var banner: some View {
        HStack(spacing: 6) {
            Image(systemName: "bolt.slash")
            Text("daemon not running — retrying…")
            Spacer()
        }
        .font(.system(size: 11))
        .foregroundStyle(.orange)
        .padding(.horizontal, 12)
        .padding(.vertical, 5)
        .background(.orange.opacity(0.12))
    }
}

/// SwiftUI's macOS `HSplitView` can install a resize cursor rect that extends
/// beyond its divider after the hosted panes are resized. Giving each pane an
/// explicit arrow cursor rect lets AppKit's more-specific controls (including
/// links) keep their own cursors while leaving only the uncovered divider hit
/// areas with the split view's resize cursor.
private struct DefaultArrowCursorRegion: NSViewRepresentable {
    func makeNSView(context: Context) -> CursorRegionView { CursorRegionView() }

    func updateNSView(_ nsView: CursorRegionView, context: Context) {
        nsView.window?.invalidateCursorRects(for: nsView)
    }

    final class CursorRegionView: NSView {
        private static let dividerHotZoneInset: CGFloat = 4

        override func resetCursorRects() {
            super.resetCursorRects()
            addCursorRect(
                bounds.insetBy(dx: Self.dividerHotZoneInset, dy: 0), cursor: .arrow)
        }

        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            window?.invalidateCursorRects(for: self)
        }
    }
}

private struct TraceInspectorPlaceholderView: View {
    @Bindable var model: TraceBrowserModel

    var body: some View {
        VStack(spacing: 0) {
            PanelHeader(accentLeft: true) {
                Text("Turn Details")
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(AlexTheme.Colors.foreground)
            } right: {
                PanelIconButton(systemImage: "xmark", help: "Close details") {
                    model.closeInspector()
                }
            }
            EmptyStateView(
                message: model.selectedSessionId == nil
                    ? "Select a session" : "Select a message to inspect",
                style: .panel(icon: "waveform.path.ecg"))
        }
    }
}

private struct TagFilterBar: View {
    @Bindable var model: TraceBrowserModel

    var body: some View {
        HStack(spacing: 8) {
            ForEach(TagFilterDimension.allCases, id: \.rawValue) { dimension in
                menu(for: dimension)
            }
            Spacer()
        }
        .padding(.horizontal, 12)
        .padding(.bottom, 7)
    }

    @ViewBuilder
    private func menu(for dimension: TagFilterDimension) -> some View {
        let values = model.filterValues(dimension)
        let active = model.activeFilter(dimension)
        Menu {
            Button {
                model.setFilter(dimension, nil)
            } label: {
                menuItemLabel(
                    dimension == .account ? "All Accounts" : "Any",
                    checked: active == nil)
            }
            Divider()
            ForEach(values, id: \.self) { value in
                Button {
                    model.setFilter(dimension, value)
                } label: {
                    menuItemLabel(model.filterLabel(dimension, value: value), checked: active == value)
                }
            }
        } label: {
            HStack(spacing: 3) {
                Text(active.map { "\(dimension.rawValue): \(model.filterLabel(dimension, value: $0))" } ?? dimension.title)
                    .lineLimit(1)
                    .truncationMode(.middle)
                Image(systemName: "chevron.down")
                    .font(.system(size: 6, weight: .semibold))
            }
            .font(.system(size: 10, weight: active == nil ? .regular : .semibold))
            .foregroundStyle(active == nil ? AnyShapeStyle(.secondary) : AnyShapeStyle(Color.accentColor))
            .padding(.horizontal, 8)
            .padding(.vertical, 3)
            .background(
                Capsule().fill(
                    active == nil ? AnyShapeStyle(.quaternary.opacity(0.5))
                        : AnyShapeStyle(Color.accentColor.opacity(0.15))))
            .frame(maxWidth: 220)
        }
        .menuStyle(.button)
        .buttonStyle(.plain)
        .menuIndicator(.hidden)
        .fixedSize()
        .disabled(values.isEmpty && active == nil)
    }

    @ViewBuilder
    private func menuItemLabel(_ text: String, checked: Bool) -> some View {
        if checked {
            Label(text, systemImage: "checkmark")
        } else {
            Text(text)
        }
    }
}

private struct TagChipView: View {
    let text: String

    var body: some View {
        Text(text)
            .font(.system(size: 9, design: .monospaced))
            .foregroundStyle(.secondary)
            .lineLimit(1)
            .padding(.horizontal, 5)
            .padding(.vertical, 1)
            .background(Capsule().fill(.quaternary.opacity(0.6)))
    }
}

enum SplitStore {
    static let leftWidthKey = "TraceBrowserLeftPaneWidth"
    nonisolated(unsafe) private static var lastSaved: CGFloat = 0

    static func leftWidth(defaults: UserDefaults = .standard) -> CGFloat {
        let stored = defaults.double(forKey: leftWidthKey)
        guard stored >= 280, stored <= 1200 else { return 380 }
        return stored
    }

    static func saveLeftWidth(_ width: CGFloat, defaults: UserDefaults = .standard) {
        guard abs(width - lastSaved) > 2 else { return }
        lastSaved = width
        defaults.set(Double(width), forKey: leftWidthKey)
    }
}

private enum SessionColumnStore {
    static let key = "TraceBrowserColumnCustomization"

    static func load(defaults: UserDefaults = .standard) -> TableColumnCustomization<SessionRow> {
        guard let data = defaults.data(forKey: key),
            let decoded = try? JSONDecoder().decode(
                TableColumnCustomization<SessionRow>.self, from: data)
        else { return TableColumnCustomization<SessionRow>() }
        return decoded
    }

    static func save(
        _ customization: TableColumnCustomization<SessionRow>,
        defaults: UserDefaults = .standard
    ) {
        guard let data = try? JSONEncoder().encode(customization) else { return }
        defaults.set(data, forKey: key)
    }
}

private struct SessionListView: View {
    @Bindable var model: TraceBrowserModel
    @FocusState private var listFocused: Bool
    @State private var customization: TableColumnCustomization<SessionRow>

    init(model: TraceBrowserModel) {
        self.model = model
        _customization = State(initialValue: SessionColumnStore.load())
    }

    var body: some View {
        VStack(spacing: 0) {
            PanelHeader {
                Text("Sessions")
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(AlexTheme.Colors.foreground)
                PanelCountBadge(count: model.sessions.count)
            }
            statusPills
            ScrollViewReader { proxy in
                table
                    .onChange(of: model.selectedSessionId) { _, id in
                        if let id, !model.isMultiSelected { proxy.scrollTo(id) }
                    }
                    .onChange(of: model.scrollAnchorVersion) { _, _ in
                        if let id = model.scrollAnchorId { proxy.scrollTo(id, anchor: .top) }
                    }
            }
            footer
        }
    }

    private var statusPills: some View {
        HStack(spacing: AlexTheme.Spacing.sm) {
            SegmentedTabs(
                tabs: SessionStatusPill.allCases.map(\.title),
                selection: pillBinding, style: .bare)
            Spacer(minLength: 0)
            // These two only affect the session list, so they live here
            // rather than in the window toolbar (moved from there).
            Toggle("Nest sub-agents", isOn: $model.nestSubagents)
                .toggleStyle(.checkbox)
                .controlSize(.small)
                .font(.system(size: 10))
                .help("Group Codex sub-agent sessions under their parent session")
            Toggle("Pings", isOn: $model.showPings)
                .toggleStyle(.checkbox)
                .controlSize(.small)
                .font(.system(size: 10))
                .help("Show ping/test sessions in the list")
        }
        .padding(.horizontal, 12)
        .frame(height: 32)
        .overlay(alignment: .bottom) {
            Rectangle().fill(AlexTheme.Colors.cardBorder).frame(height: 1)
        }
    }

    private var pillBinding: Binding<Int> {
        Binding(
            get: { model.statusPill.rawValue },
            set: { model.statusPill = SessionStatusPill(rawValue: $0) ?? .all })
    }

    private var table: some View {
        Table(
            model.visibleRows, selection: selectionBinding, sortOrder: $model.sortOrder,
            columnCustomization: $customization
        ) {
            primaryColumns
            secondaryColumns
        }
        .contextMenu(forSelectionType: SessionRow.ID.self) { ids in
            let selected = SessionContextMenuSelection.resolve(
                ids: ids, fallbackIds: model.multiSelection, sessions: model.sessions)
            if selected.count > 1 {
                bulkContextMenu(selected)
            } else if let session = selected.first {
                contextMenu(session)
            }
        }
        .overlay {
            if model.visibleRows.isEmpty {
                if model.sessionsLoading {
                    VStack(spacing: 8) {
                        ProgressView().controlSize(.small)
                        Text(model.sessionsUnreachable ? "Connecting to daemon…" : "Loading sessions…")
                    }
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
                } else if model.sessionsUnreachable {
                    VStack(spacing: 8) {
                        Text("Daemon unreachable")
                        Button("Retry") { model.retrySessions() }.buttonStyle(.bordered)
                    }
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
                } else {
                    Text(model.sessions.isEmpty ? "No sessions in the last 24h" : "No sessions match")
                        .font(.system(size: 11))
                        .foregroundStyle(.secondary)
                }
            }
        }
        .focused($listFocused)
        .onKeyPress(.home) {
            model.moveSelection(.home)
            return .handled
        }
        .onKeyPress(.end) {
            model.moveSelection(.end)
            return .handled
        }
        .onDeleteCommand { model.deleteSelectedSessions() }
        .onAppear { listFocused = true }
        .onChange(of: customization) { _, updated in
            SessionColumnStore.save(updated)
        }
    }

    @TableColumnBuilder<SessionRow, KeyPathComparator<SessionRow>>
    private var primaryColumns: some TableColumnContent<SessionRow, KeyPathComparator<SessionRow>> {
        TableColumn("Session", value: \.sessionShort) { (row: SessionRow) in
            SessionCellView(
                row: row,
                pinned: model.pinned && row.id == model.selectedSessionId,
                showPingBadge: model.showPings && row.isPingOrTest,
                nestSubagents: model.nestSubagents,
                lineageCollapsed: model.isLineageCollapsed(row.id),
                toggleLineage: { model.toggleLineage(row.id) },
                bodyOnlyMatch: model.parsedQuery.isBodyOnlyMatch(
                    row, serverMatches: model.searchSessionIds))
        }
        .width(min: 240)
        .customizationID("session")
        .disabledCustomizationBehavior(.visibility)
        TableColumn("Model(s)", value: \.models) { (row: SessionRow) in
            Text(row.models)
                .font(.system(size: 10))
                .foregroundStyle(.secondary)
                .lineLimit(1)
        }
        .customizationID("models")
        TableColumn("Last activity", value: \.lastTs) { (row: SessionRow) in
            Text(TraceFormat.relative(row.lastTsMs))
                .font(.system(size: 10))
                .foregroundStyle(.secondary)
        }
        .width(min: 60, ideal: 76)
        .customizationID("lastActivity")
        TableColumn("Turns", value: \.turns) { (row: SessionRow) in
            numericCell("\(row.turns)")
        }
        .width(min: 36, ideal: 44)
        .customizationID("turns")
        TableColumn("Duration", value: \.durationMs) { (row: SessionRow) in
            numericCell(row.duration)
        }
        .width(min: 48, ideal: 58)
        .customizationID("duration")
        TableColumn("Tokens in", value: \.tokensIn) { (row: SessionRow) in
            numericCell(TraceFormat.tokens(row.tokensIn))
        }
        .width(min: 48, ideal: 60)
        .customizationID("tokensIn")
        .defaultVisibility(.hidden)
        TableColumn("Tokens out", value: \.tokensOut) { (row: SessionRow) in
            numericCell(TraceFormat.tokens(row.tokensOut))
        }
        .width(min: 48, ideal: 60)
        .customizationID("tokensOut")
        TableColumn("Cost", value: \.cost) { (row: SessionRow) in
            numericCell(row.cost > 0 ? TraceFormat.cost(row.cost) : "")
        }
        .width(min: 48, ideal: 60)
        .customizationID("cost")
    }

    @TableColumnBuilder<SessionRow, KeyPathComparator<SessionRow>>
    private var secondaryColumns: some TableColumnContent<SessionRow, KeyPathComparator<SessionRow>> {
        TableColumn("Errors", value: \.errors) { (row: SessionRow) in
            Text(row.errors > 0 ? "\(row.errors)" : "")
                .font(.system(size: 10, weight: .semibold, design: .monospaced))
                .foregroundStyle(.red)
                .frame(maxWidth: .infinity, alignment: .trailing)
        }
        .width(min: 40, ideal: 48)
        .customizationID("errors")
        .defaultVisibility(.hidden)
        TableColumn("Harness", value: \.harness) { (row: SessionRow) in
            Text(row.harness)
                .font(.system(size: 10))
                .foregroundStyle(.secondary)
                .lineLimit(1)
        }
        .customizationID("harness")
        .defaultVisibility(.hidden)
        TableColumn("Billing Account", value: \.accounts) { (row: SessionRow) in
            Text(model.accountNames(row.accountIds) ?? "")
                .font(.system(size: 10))
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .truncationMode(.middle)
                .help(model.accountIdentity(row.accountIds) ?? "")
        }
        .width(min: 110, ideal: 180)
        .customizationID("account")
        TableColumn("Run", value: \.runId) { (row: SessionRow) in
            Text(row.runId)
                .font(.system(size: 10, design: .monospaced))
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .truncationMode(.middle)
        }
        .customizationID("run")
        .defaultVisibility(.hidden)
        TableColumn("Fork lineage", value: \.forkRelationshipSummary) { (row: SessionRow) in
            Text(row.forkRelationshipSummary)
                .font(.system(size: 10, design: .monospaced))
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .truncationMode(.middle)
                .help(row.forkRelationshipTooltip ?? "")
        }
        .width(min: 110, ideal: 190)
        .customizationID("forkLineage")
        .defaultVisibility(.hidden)
        TableColumn("Tags", value: \.tagsSummary) { (row: SessionRow) in
            Text(row.tagsSummary)
                .font(.system(size: 9, design: .monospaced))
                .foregroundStyle(.secondary)
                .lineLimit(1)
        }
        .customizationID("tags")
        .defaultVisibility(.hidden)
    }

    private var footer: some View {
        HStack(spacing: 8) {
            Text("\(model.visibleRows.count) of \(model.sessions.count) sessions")
                .font(AlexTheme.Fonts.mono(10))
                .foregroundStyle(AlexTheme.Colors.textTertiary)
            if model.searchSessionIds != nil {
                Text("\(model.searchMatchCount) matches · scanned \(model.searchScanned)")
                    .font(AlexTheme.Fonts.mono(10))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
            }
            Spacer()
            Text("Right-click headers to show/hide columns")
                .font(.system(size: 10))
                .foregroundStyle(AlexTheme.Colors.textFaint)
        }
        .padding(.horizontal, 12)
        .frame(height: AlexTheme.Metrics.footerHeight)
        .overlay(alignment: .top) {
            Rectangle().fill(AlexTheme.Colors.cardBorder).frame(height: 1)
        }
    }

    private func numericCell(_ text: String) -> some View {
        Text(text)
            .font(.system(size: 10, design: .monospaced))
            .foregroundStyle(.secondary)
            .frame(maxWidth: .infinity, alignment: .trailing)
    }

    /// Set-based binding: SwiftUI's Table gives shift-click ranges and
    /// cmd-click toggles for free once selection is `Binding<Set<ID>>`
    /// (single selection still binds through `updateSelection`, which keeps
    /// today's single-select behavior — transcript load, pin, live-follow —
    /// unchanged).
    private var selectionBinding: Binding<Set<String>> {
        Binding(
            get: { model.multiSelection },
            set: { ids in
                model.updateSelection(ids)
                listFocused = true
            })
    }

    @ViewBuilder
    private func contextMenu(_ session: TraceSession) -> some View {
        let hasSessionId = !session.sessionId.isEmpty
        if session.isAlexError {
            if session.approvableCredentialFingerprint != nil {
                Button("Approve") { model.approveRejectedClient(session) }
                    .help("Re-enable this exact previously known client credential")
            } else {
                Text("Approval unavailable for unknown credentials")
            }
            Divider()
        }
        Menu("Simulate") {
            if model.fixturesLoading {
                Text("Loading fixtures…")
            } else if model.simulationFixtures.isEmpty {
                Text(model.fixtureLoadError == nil ? "No fixtures available" : "Fixtures unavailable")
                if model.fixtureLoadError != nil {
                    Button("Retry fixture load") {
                        Task { await model.loadSimulationFixtures() }
                    }
                }
            } else {
                ForEach(model.simulationFixtures) { fixture in
                    Button(fixtureMenuTitle(fixture)) {
                        model.injectFixture(fixture, into: session)
                    }
                }
            }
        }
        .disabled(!hasSessionId)
        .help(hasSessionId ? "Queue an error fixture for this session" : "no session id")
        Button("Clear pending injections") { model.clearFixtureInjections(for: session) }
            .disabled(!hasSessionId)
            .help(hasSessionId ? "Remove queued error fixtures" : "no session id")
        if TraceClassification.realErrorCount(
            total: session.errors, errorClassCounts: session.errorClassCounts) > 0
        {
            Button("Save as fixture…") { model.promptSaveFixture(from: session) }
        }
        Divider()
        // A data-driven group avoids the macOS Table context-menu bridge
        // truncating a large heterogeneous ViewBuilder tuple. Keep every
        // standard single-session command in this one flattened group.
        ForEach(SessionContextMenuAction.standard) { action in
            contextMenuItem(action, session: session, hasSessionId: hasSessionId)
        }
    }

    @ViewBuilder
    private func contextMenuItem(
        _ action: SessionContextMenuAction, session: TraceSession, hasSessionId: Bool
    ) -> some View {
        switch action {
        case .fork:
            Button(action.title) { model.copyForkCommand(session) }
                .disabled(!hasSessionId)
        case .copySessionId:
            Button(action.title) { model.copySessionId(session) }
        case .copyLastReply:
            Button(action.title) { model.copyLastReply(session) }
        case .export:
            Button(action.title) { model.exportSession(session) }
        case .revealBodies:
            Button(action.title) { model.revealSessionBodies(session) }
        case .destructiveDivider:
            Divider()
        case .delete:
            Button(action.title, role: .destructive) { model.deleteSessionTraces(session) }
        }
    }

    private func fixtureMenuTitle(_ fixture: ErrorSimulationFixture) -> String {
        let action = fixture.direction == "upstream_to_client" ? "Send" : "Replay"
        let status = fixture.status.map { " (\($0))" } ?? ""
        return "\(action): \(fixture.name)\(status)"
    }

    @ViewBuilder
    private func bulkContextMenu(_ selected: [TraceSession]) -> some View {
        Button("Delete \(selected.count) Sessions' Traces…", role: .destructive) {
            model.deleteSelectedSessions()
        }
    }
}

enum SessionContextMenuAction: String, CaseIterable, Identifiable {
    case fork
    case copySessionId
    case copyLastReply
    case export
    case revealBodies
    case destructiveDivider
    case delete

    var id: Self { self }

    static let standard = allCases

    var title: String {
        switch self {
        case .fork: "Fork session with…"
        case .copySessionId: "Copy Session ID"
        case .copyLastReply: "Copy Last Reply as Markdown"
        case .export: "Export Session…"
        case .revealBodies: "Reveal Bodies in Finder"
        case .destructiveDivider: ""
        case .delete: "Delete Session's Traces…"
        }
    }
}

enum SessionContextMenuSelection {
    static func resolve(
        ids: Set<String>, fallbackIds: Set<String>, sessions: [TraceSession]
    ) -> [TraceSession] {
        let requested = ids.isEmpty ? fallbackIds : ids
        guard !requested.isEmpty else { return [] }
        let byId = Dictionary(uniqueKeysWithValues: sessions.map { ($0.sessionId, $0) })
        let resolved = requested.sorted().compactMap { byId[$0] }
        if !resolved.isEmpty || requested == fallbackIds { return resolved }
        return fallbackIds.sorted().compactMap { byId[$0] }
    }
}

private struct SessionCellView: View {
    let row: SessionRow
    let pinned: Bool
    let showPingBadge: Bool
    let nestSubagents: Bool
    let lineageCollapsed: Bool
    let toggleLineage: () -> Void
    var bodyOnlyMatch: Bool = false

    /// Per-depth-level indent, within the "~14-18pt, unmistakable" range the
    /// original design called for (was effectively 0pt for a depth-1 row
    /// before this change — only the 16pt connector slot separated it from
    /// its parent).
    private let indentUnit: CGFloat = 16
    /// Fixed x-offset of the lineage rail, in the leading gutter. Constant
    /// across depths so a whole subtree (children *and* grandchildren)
    /// shares one continuous rail aligned under the root's status-dot
    /// column, rather than each depth drawing its own.
    private let railX: CGFloat = 8

    private var isDescendant: Bool { nestSubagents && row.lineageDepth > 0 }

    var body: some View {
        ZStack(alignment: .leading) {
            if isDescendant {
                // Group-identity rail: one continuous 2px line down the
                // leading edge of every descendant row of a lineage,
                // stronger than the old single "|—" glyph. Row content
                // (below) draws a short horizontal tick from this rail out
                // to its own status dot, so grandchildren still visibly
                // join the same rail as their sibling subtree.
                Rectangle()
                    .fill(AlexTheme.Colors.primary.opacity(0.35))
                    .frame(width: 2)
                    .frame(maxHeight: .infinity)
                    .offset(x: railX)
                Rectangle()
                    .fill(AlexTheme.Colors.primary.opacity(0.35))
                    .frame(height: 1)
                    .frame(width: CGFloat(row.lineageDepth) * indentUnit + 16 - railX)
                    .offset(x: railX)
            }
            HStack(spacing: 5) {
                if isDescendant {
                    Spacer().frame(width: CGFloat(row.lineageDepth) * indentUnit)
                }
                if nestSubagents, row.childCount > 0 {
                    Button(action: toggleLineage) {
                        Image(systemName: "chevron.right")
                            .font(.system(size: 8, weight: .semibold))
                            .foregroundStyle(AlexTheme.Colors.textTertiary)
                            .rotationEffect(.degrees(lineageCollapsed ? 0 : 90))
                            .frame(width: 16, height: 16)
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .help(lineageCollapsed ? "Show sub-agents" : "Hide sub-agents")
                } else {
                    Color.clear.frame(width: 16, height: 1)
                }
                StatusDot(status: SessionDisplayStatus.status(for: row))
                HarnessIconView(
                    harness: row.harnessRaw, tags: row.tags, size: 17, showsFallback: true)
            if let provider = SessionIdentity.primaryProvider(
                providers: row.providers, harness: row.harnessRaw, tags: row.tags)
            {
                ProviderBadgeView(provider: provider, size: 17, style: .tinted)
            }
            Text(row.sessionShort)
                .font(AlexTheme.Fonts.mono(11))
                .kerning(0.11)
                .foregroundStyle(AlexTheme.Colors.textSecondary)
                .lineLimit(1)
                .truncationMode(.middle)
            if let firstModel = row.models
                .split(separator: ",").first
                .map({ $0.trimmingCharacters(in: .whitespaces) }),
                !firstModel.isEmpty
            {
                ModelBadge(model: firstModel)
            }
            if nestSubagents, row.lineageDepth > 0 {
                Text(SessionIdentity.subagentLabel)
                    .font(.system(size: 9, weight: .medium))
                    .foregroundStyle(.purple)
                    .padding(.horizontal, 4)
                    .padding(.vertical, 1)
                    .background(Capsule().fill(.purple.opacity(0.12)))
                if let typeTag = SessionIdentity.agentTypeTag(agentType: row.agentType) {
                    Text(typeTag)
                        .font(.system(size: 9))
                        .foregroundStyle(.secondary)
                        .padding(.horizontal, 4)
                        .padding(.vertical, 1)
                        .overlay(Capsule().strokeBorder(.quaternary))
                }
            }
            if nestSubagents, row.childCount > 0 {
                Text("\(row.childCount) agent\(row.childCount == 1 ? "" : "s")")
                    .font(.system(size: 9, weight: .medium))
                    .foregroundStyle(.secondary)
            }
            if row.forkedFromSessionId != nil {
                Label("fork", systemImage: "arrow.triangle.branch")
                    .labelStyle(.titleAndIcon)
                    .font(.system(size: 9, weight: .medium))
                    .foregroundStyle(.teal)
                    .padding(.horizontal, 4)
                    .padding(.vertical, 1)
                    .background(Capsule().fill(.teal.opacity(0.12)))
                    .help(row.forkRelationshipTooltip ?? "Forked session")
                    .accessibilityLabel(row.forkRelationshipTooltip ?? "Forked session")
            }
            if row.forkCount > 0 {
                Text("\(row.forkCount) fork\(row.forkCount == 1 ? "" : "s")")
                    .font(.system(size: 9, weight: .medium))
                    .foregroundStyle(.secondary)
                    .help(row.forkRelationshipTooltip ?? "")
            }
            if row.errors > 0 {
                Text("✗ \(row.errors)")
                    .font(.system(size: 9, weight: .semibold, design: .monospaced))
                    .foregroundStyle(.white)
                    .padding(.horizontal, 4)
                    .padding(.vertical, 1)
                    .background(Capsule().fill(.red))
                    .help("\(row.errors) failed request\(row.errors == 1 ? "" : "s")")
            }
            if row.clientDisconnects > 0 {
                Text("client closed")
                    .font(.system(size: 9, weight: .medium))
                    .foregroundStyle(.secondary)
                    .padding(.horizontal, 5)
                    .padding(.vertical, 1)
                    .background(Capsule().fill(.quaternary.opacity(0.6)))
                    .help("Harness closed \(row.clientDisconnects) request\(row.clientDisconnects == 1 ? "" : "s")")
            }
            if pinned {
                Image(systemName: "pin.fill")
                    .font(.system(size: 8))
                    .foregroundStyle(.orange)
            }
            if bodyOnlyMatch {
                Image(systemName: "text.magnifyingglass")
                    .font(.system(size: 9))
                    .foregroundStyle(.secondary)
                    .help("Matched by a message body search, not visible session metadata")
            }
                if showPingBadge, let badge = row.kindBadge {
                    Text("[\(badge)]")
                        .font(.system(size: 9))
                        .foregroundStyle(.tertiary)
                }
            }
        }
    }
}

private struct TranscriptView: View {
    @Bindable var model: TraceBrowserModel
    @AppStorage("SessionInfoExpanded") private var infoExpanded = false
    @AppStorage("TranscriptClassicPane") private var classicPane = false
    @State private var showSystemPrompt = false

    var body: some View {
        VStack(spacing: 0) {
            if model.isMultiSelected {
                multiSelectionState
            } else {
                singleSelectionBody
            }
        }
    }

    /// Multiple session rows selected (shift/cmd-click on the sessions
    /// table): no single transcript to show, so the right panes present an
    /// empty state instead. Delete acts on the whole selection.
    private var multiSelectionState: some View {
        EmptyStateView(
            message: "\(model.multiSelection.count) sessions selected\nDelete removes them all",
            style: .panel(icon: "checklist"))
    }

    private var singleSelectionBody: some View {
        VStack(spacing: 0) {
            header
            if infoExpanded, model.selectedSession != nil {
                SessionInfoCard(model: model)
                Divider()
            }
            if !classicPane, model.selectedSession != nil {
                filterRow
            }
            if let followed = model.followedSubagentId {
                followBanner(followed)
            }
            if model.hiddenTurnCount > 0 {
                Button("Load earlier turns (\(model.hiddenTurnCount) more)") {
                    model.loadEarlierTurns()
                }
                .buttonStyle(.link)
                .font(.system(size: 11))
                .padding(.vertical, 4)
                .frame(maxWidth: .infinity)
                Divider()
            }
            ZStack(alignment: .bottom) {
                if classicPane {
                    TranscriptTextPane(model: model)
                } else {
                    TranscriptChatPane(model: model)
                }
                if model.turns.isEmpty {
                    if model.selectedSessionId == nil {
                        Text("Select a session")
                            .font(.system(size: 11)).foregroundStyle(.secondary)
                            .frame(maxWidth: .infinity, maxHeight: .infinity)
                    } else if model.transcriptLoading {
                        VStack(spacing: 8) {
                            ProgressView().controlSize(.small)
                            Text(model.transcriptUnreachable ? "Connecting to daemon…" : "Loading transcript…")
                        }
                        .font(.system(size: 11)).foregroundStyle(.secondary)
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                    } else if model.transcriptUnreachable {
                        VStack(spacing: 8) {
                            Text("Daemon unreachable")
                            Button("Retry") { model.retryTranscript() }.buttonStyle(.bordered)
                        }
                        .font(.system(size: 11)).foregroundStyle(.secondary)
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                    } else {
                        Text("No turns yet")
                            .font(.system(size: 11)).foregroundStyle(.secondary)
                            .frame(maxWidth: .infinity, maxHeight: .infinity)
                    }
                }
                if model.transcriptRendering, !model.turns.isEmpty {
                    HStack(spacing: 6) {
                        ProgressView().controlSize(.small)
                        Text("rendering…")
                    }
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
                    .padding(.horizontal, 10)
                    .padding(.vertical, 6)
                    .background(Capsule().fill(.regularMaterial))
                    .padding(.bottom, 48)
                }
                VStack(spacing: 6) {
                    if !model.userAtBottom, !model.turns.isEmpty {
                        Button {
                            model.setUserAtBottom(true)
                            model.requestScrollToBottom()
                        } label: {
                            Label("Jump to latest", systemImage: "arrow.down.to.line")
                                .font(.system(size: 11, weight: .medium))
                                .padding(.horizontal, 10)
                                .padding(.vertical, 5)
                                .background(Capsule().fill(.thinMaterial))
                                .overlay(Capsule().strokeBorder(.quaternary))
                        }
                        .buttonStyle(.plain)
                    }
                }
                .padding(.bottom, 12)
            }
            if !classicPane {
                transcriptFooter
            }
        }
    }

    private var filterRow: some View {
        FilterRow {
            SearchField(text: $model.transcriptQuery, placeholder: "Filter messages…")
            SegmentedTabs(
                tabs: TranscriptChatEntries.filterTabs,
                selection: $model.transcriptFilterTab, style: .contained)
        }
    }

    private func followBanner(_ sessionId: String) -> some View {
        HStack(spacing: AlexTheme.Spacing.lg) {
            Image(systemName: "arrow.triangle.branch")
                .font(.system(size: 13))
                .foregroundStyle(AlexTheme.Colors.primary)
            (Text("Following subagent ")
                .font(.system(size: 11))
                .foregroundColor(AlexTheme.Colors.textSecondary)
                + Text(SessionShortId.shorten(sessionId))
                .font(AlexTheme.Fonts.mono(11))
                .foregroundColor(AlexTheme.Colors.primaryBright))
                .lineLimit(1)
                .truncationMode(.middle)
            Spacer(minLength: 0)
            Button {
                model.dismissFollowBanner()
            } label: {
                Text("Dismiss")
                    .font(.system(size: 10, weight: .medium))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
                    .padding(.horizontal, 8)
                    .padding(.vertical, 2)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 8)
        .background(
            RoundedRectangle(cornerRadius: AlexTheme.Radius.xl)
                .fill(
                    LinearGradient(
                        colors: [
                            AlexTheme.Colors.primary.opacity(0.12),
                            AlexTheme.Colors.indigo.opacity(0.08),
                        ],
                        startPoint: .topLeading, endPoint: .bottomTrailing)))
        .overlay(
            RoundedRectangle(cornerRadius: AlexTheme.Radius.xl)
                .strokeBorder(AlexTheme.Colors.primary.opacity(0.3)))
        .padding(.horizontal, 16)
        .padding(.top, 12)
        .padding(.bottom, 4)
    }

    private var transcriptFooter: some View {
        // Both counts come from the model's cached, debounced filter result
        // (see TraceBrowserModel.scheduleTranscriptFilter) instead of
        // recomputing over every turn here on each render.
        SessionListFooter(
            text: "\(model.transcriptEntries.count) of \(model.transcriptTotalCount) messages",
            showsDot: true,
            trailingText: "\(TraceFormat.tokens(model.transcriptTokensTotal)) tokens total")
    }

    private var header: some View {
        PanelHeader(accentLeft: true) {
            if let session = model.selectedSession {
                headerLeft(session)
            } else {
                Text("No session selected")
                    .font(.system(size: 11))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
            }
        } right: {
            if let session = model.selectedSession {
                CopyButton(value: session.sessionId, label: "Copy ID")
                Toggle("Raw", isOn: $model.transcriptRawMode)
                    .toggleStyle(.checkbox)
                    .controlSize(.mini)
                    .font(.system(size: 10))
                    .fixedSize()
                    .help("Show exact wire text without JSON formatting")
                if model.sessionSystemPrompt != nil {
                    PanelIconButton(systemImage: "doc.text", help: "View system prompt") {
                        showSystemPrompt = true
                    }
                    .popover(isPresented: $showSystemPrompt) {
                        SystemPromptView(
                            prompt: model.sessionSystemPrompt ?? "",
                            modelName: model.selectedSession?.models?.first)
                    }
                }
                PanelIconButton(
                    systemImage: "magnifyingglass", help: "Find in transcript (⌘F)"
                ) {
                    model.requestFind()
                }
                PanelIconButton(
                    systemImage: infoExpanded ? "chevron.up" : "chevron.down",
                    help: infoExpanded ? "Hide session info" : "Show session info"
                ) {
                    infoExpanded.toggle()
                    if infoExpanded { model.ensureFirstTraceDetail() }
                }
            }
        }
    }

    @ViewBuilder
    private func headerLeft(_ session: TraceSession) -> some View {
        ZStack {
            RoundedRectangle(cornerRadius: AlexTheme.Radius.md)
                .fill(AlexTheme.Colors.primary.opacity(0.15))
            RoundedRectangle(cornerRadius: AlexTheme.Radius.md)
                .strokeBorder(AlexTheme.Colors.primary.opacity(0.22))
            Image(systemName: "bolt.fill")
                .font(.system(size: 13))
                .foregroundStyle(AlexTheme.Colors.primary)
        }
        .frame(width: 30, height: 30)
        VStack(alignment: .leading, spacing: 1) {
            HStack(spacing: AlexTheme.Spacing.sm) {
                Text(SessionShortId.shorten(session.sessionId))
                    .font(AlexTheme.Fonts.mono(11, weight: .semibold))
                    .foregroundStyle(AlexTheme.Colors.foreground)
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .textSelection(.enabled)
                    .help(session.sessionId)
                Image(systemName: "dot.radiowaves.left.and.right")
                    .font(.system(size: 9))
                    .foregroundStyle(AlexTheme.Colors.success)
                    .help("Live — this transcript refreshes automatically")
            }
            Text(headerSubtitle(session))
                .font(.system(size: 10))
                .foregroundStyle(AlexTheme.Colors.textTertiary)
                .lineLimit(1)
                .truncationMode(.middle)
        }
        if let firstModel = session.models?.first {
            ModelBadge(model: firstModel)
        }
        if let accountName = model.accountNames(session.accountIds ?? []) {
            Text(accountName)
                .font(.system(size: 9, weight: .medium))
                .foregroundStyle(AlexTheme.Colors.primary)
                .lineLimit(1)
                .truncationMode(.middle)
                .padding(.horizontal, 5)
                .padding(.vertical, 1)
                .background(Capsule().fill(AlexTheme.Colors.primary.opacity(0.12)))
                .help(model.accountIdentity(session.accountIds ?? []) ?? accountName)
        }
        let chips = SessionTagChips.chips(
            tags: session.tags, harness: session.harness, models: session.models)
        ForEach(chips, id: \.key) { chip in
            TagChipView(text: chip.label())
        }
    }

    private func headerSubtitle(_ session: TraceSession) -> String {
        let toolCount = model.transcriptToolCount
        let agentCount = model.transcriptSubagentCount
        var parts = [
            "\(model.turns.count) turn\(model.turns.count == 1 ? "" : "s")",
            "\(toolCount) tool\(toolCount == 1 ? "" : "s")",
            "\(agentCount) subagent\(agentCount == 1 ? "" : "s")",
        ]
        if let cost = session.totalCostUsd, cost > 0 {
            parts.append(TraceFormat.cost(cost))
        }
        if let runId = session.runId, !runId.isEmpty {
            parts.append("run \(runId)")
        }
        return parts.joined(separator: " · ")
    }
}

enum TraceFormat {
    static func relative(_ tsMs: Int64, now: Date = Date()) -> String {
        let delta = Int64(now.timeIntervalSince1970) - tsMs / 1000
        if delta < 10 { return "now" }
        return "\(Format.duration(delta)) ago"
    }

    @MainActor private static let timeFormatter: DateFormatter = {
        let formatter = DateFormatter()
        formatter.dateFormat = "HH:mm:ss"
        return formatter
    }()

    @MainActor
    static func time(_ tsMs: Int64) -> String {
        timeFormatter.string(from: Date(timeIntervalSince1970: Double(tsMs) / 1000))
    }

    static func tokens(_ count: Int64?) -> String { TraceNumberFormat.tokens(count) }

    static func cost(_ usd: Double) -> String { TraceNumberFormat.cost(usd) }
}

@MainActor
final class TraceBrowserWindowController: NSObject, NSWindowDelegate {
    private var window: NSWindow?
    private var model: TraceBrowserModel?
    private let store: SnapshotStore

    init(store: SnapshotStore) {
        self.store = store
        super.init()
    }

    /// - Parameter selectSessionId: When set, the browser lands on this
    ///   session once the list loads (`TraceBrowserModel.selectSessionWhenLoaded`)
    ///   instead of opening un-targeted. One-line adoption for a caller that
    ///   already holds a `TraceBrowserWindowController`: pass the session id
    ///   here instead of calling `show(harness:)` alone.
    func show(
        harness: String? = nil, query: String? = nil, selectSessionId: String? = nil,
        above relativeWindow: NSWindow? = nil
    ) {
        if window == nil {
            let model = TraceBrowserModel(
                store: store, initialHarness: harness, initialQuery: query)
            self.model = model
            let host = NSHostingController(rootView: TraceBrowserView(model: model))
            let win = NSWindow(contentViewController: host)
            win.title = "Alex UI — Trace Browser"
            win.styleMask = [.titled, .closable, .miniaturizable, .resizable]
            win.isReleasedWhenClosed = false
            win.delegate = self
            win.setContentSize(NSSize(width: 980, height: 620))
            win.center()
            win.setFrameAutosaveName("AlexTraceBrowser")
            window = win
        } else if let query {
            model?.setQueryFilter(query)
        } else if let harness {
            model?.setHarnessFilter(harness)
        }
        BarLog.info(.ui, "trace browser opened")
        model?.start()
        if let selectSessionId {
            model?.selectSessionWhenLoaded(selectSessionId)
        }
        if let window {
            DockIconManager.shared.track(window)
            window.makeKeyAndOrderFront(nil)
            if let relativeWindow, relativeWindow.isVisible {
                window.order(.above, relativeTo: relativeWindow.windowNumber)
                // The originating button can make its window key again as
                // AppKit finishes dispatching the click. Reassert on the next
                // run-loop turn so the browser remains visibly in front.
                DispatchQueue.main.async { [weak window, weak relativeWindow] in
                    guard let window, let relativeWindow, relativeWindow.isVisible else { return }
                    window.makeKeyAndOrderFront(nil)
                    window.order(.above, relativeTo: relativeWindow.windowNumber)
                }
            }
            NSApp.activate(ignoringOtherApps: true)
        }
    }

    func windowWillClose(_ notification: Notification) {
        BarLog.info(.ui, "trace browser closed")
        model?.stop()
    }
}
