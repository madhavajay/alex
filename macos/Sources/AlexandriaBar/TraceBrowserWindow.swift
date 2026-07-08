import AppKit
import SwiftUI
import Observation
import AlexandriaBarCore

@MainActor
@Observable
final class TraceBrowserModel {
    private let store: SnapshotStore

    private(set) var sessions: [TraceSession] = []
    private(set) var turns: [TranscriptTurn] = []
    private(set) var visibleRows: [SessionRow] = []
    private(set) var parsedQuery = OmniQuery()
    private(set) var daemonDown = false
    private(set) var searchSessionIds: Set<String>?
    private(set) var searchMatchCount = 0
    private(set) var searchScanned = 0
    private var sessionsFingerprint = ""
    private var turnsFingerprint = ""
    private var rowsById: [String: SessionRow] = [:]
    private var selectionState = SessionSelection()

    var selectedSessionId: String? { selectionState.selectedId }
    var pinned: Bool { selectionState.pinned }
    var sortOrder = SessionTable.defaultSortOrder() {
        didSet { recomputeVisible() }
    }
    var showPings = false {
        didSet { recomputeVisible() }
    }
    var queryText = "" {
        didSet {
            parsedQuery = OmniQuery.parse(queryText)
            queryChanged()
        }
    }

    private(set) var userAtBottom = true {
        didSet {
            if userAtBottom {
                scrolledAwayAt = nil
            } else if scrolledAwayAt == nil {
                scrolledAwayAt = Date()
            }
        }
    }
    private var scrolledAwayAt: Date?

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

    private(set) var inspectorTraceId: String?
    private(set) var firstTraceDetail: TraceDetailResponse?
    private var firstDetailKey: String?
    private var firstDetailTask: Task<Void, Never>?

    var firstTurnTraceId: String? { turns.first?.traceId }

    var firstRequestHeaders: [HeaderPair] {
        TraceHeaders.sortedPairs(firstTraceDetail?.trace.reqHeadersJson)
    }

    func openInspector(traceId: String) {
        inspectorTraceId = traceId
    }

    func closeInspector() {
        inspectorTraceId = nil
    }

    func detailClient() -> AlexandriaClient? { client() }

    func ensureFirstTraceDetail() {
        guard let sid = selectedSessionId, let first = turns.first else { return }
        let key = "\(sid)|\(first.traceId)"
        guard key != firstDetailKey else { return }
        firstDetailKey = key
        firstTraceDetail = nil
        firstDetailTask?.cancel()
        firstDetailTask = Task { [weak self] in
            guard let client = self?.client() else { return }
            guard let detail = try? await client.traceDetail(id: first.traceId) else { return }
            guard !Task.isCancelled, let self, self.firstDetailKey == key else { return }
            self.firstTraceDetail = detail
        }
    }

    func revealSessionBodies(_ session: TraceSession) {
        Task {
            guard let client = client() else { return }
            var bodyPath: String?
            if let last = try? await client.traceTranscript(sessionId: session.sessionId).turns.last,
                let detail = try? await client.traceDetail(id: last.traceId) {
                bodyPath = detail.trace.reqBodyPath
                    ?? detail.trace.respBodyPath ?? detail.trace.upstreamReqBodyPath
            }
            if let bodyPath {
                NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: bodyPath)])
            } else {
                let fallback = FileManager.default.homeDirectoryForCurrentUser
                    .appendingPathComponent(".alexandria/bodies")
                NSWorkspace.shared.activateFileViewerSelecting([fallback])
            }
        }
    }

    init(store: SnapshotStore) {
        self.store = store
    }

    private func recomputeVisible() {
        let query = parsedQuery
        BarLog.measure(.browser, label: "filter sessions=\(sessions.count)") {
            visibleRows = SessionTable.visibleRows(
                sessions: sessions, rowsById: rowsById, showPings: showPings,
                query: query, serverMatches: searchSessionIds, sortOrder: sortOrder)
        }
    }

    private var newestVisibleRow: SessionRow? {
        visibleRows.max { $0.lastTsMs < $1.lastTsMs }
    }

    var showsTagFilterBar: Bool {
        sessions.contains { $0.tags?.isEmpty == false }
    }

    func filterValues(_ dimension: TagFilterDimension) -> [String] {
        dimension.values(in: sessions)
    }

    func activeFilter(_ dimension: TagFilterDimension) -> String? {
        dimension.activeValue(in: parsedQuery)
    }

    func setFilter(_ dimension: TagFilterDimension, _ value: String?) {
        queryText = OmniQuery.settingToken(in: queryText, key: dimension.rawValue, value: value)
    }

    var selectedSession: TraceSession? {
        sessions.first { $0.sessionId == selectedSessionId }
    }

    func start() {
        stop()
        sessionsTask = Task { [weak self] in
            while !Task.isCancelled {
                await self?.pollSessions()
                try? await Task.sleep(for: .seconds(2))
            }
        }
        transcriptTask = Task { [weak self] in
            while !Task.isCancelled {
                await self?.pollTranscript()
                try? await Task.sleep(for: .seconds(1))
            }
        }
    }

    func stop() {
        sessionsTask?.cancel()
        sessionsTask = nil
        transcriptTask?.cancel()
        transcriptTask = nil
        searchTask?.cancel()
        searchTask = nil
        renderChain?.cancel()
        renderChain = nil
        firstDetailTask?.cancel()
        firstDetailTask = nil
    }

    func selectFromUser(_ id: String) {
        apply(selectionState.userSelect(id))
    }

    func selectFromFollow(_ id: String) {
        apply(selectionState.followSelect(id))
    }

    func selectFromBinding(_ id: String?) {
        apply(selectionState.bindingSelect(id))
    }

    private func apply(_ change: SessionSelection.Change) {
        guard case .selected = change else { return }
        resetTurns()
        setUserAtBottom(true)
        Task { await pollTranscript() }
    }

    private func resetTurns() {
        turns = []
        turnsFingerprint = ""
        inspectorTraceId = nil
        firstTraceDetail = nil
        firstDetailKey = nil
        firstDetailTask?.cancel()
        firstDetailTask = nil
        renderChain?.cancel()
        renderChain = nil
        renderState = TranscriptRender.state(for: [])
        windowStart = 0
        windowMaxTurns = TranscriptWindow.defaultMaxTurns
        hiddenTurnCount = 0
        renderVersion += 1
        renderOp = (renderVersion, .set(NSAttributedString()))
    }

    func loadEarlierTurns() {
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
        var plan = TranscriptRender.plan(previous: renderState, turns: windowed)
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
        hiddenTurnCount = windowStart
        guard plan != .unchanged else { return }
        renderState = TranscriptRender.state(for: windowed)
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
        let prev = renderChain
        renderChain = Task { [weak self] in
            await prev?.value
            let built = await Task.detached { () -> BuiltDocument in
                let start = ContinuousClock.now
                let doc = TranscriptRender.document(turns: slice, firstTurnNumber: firstNumber)
                let elapsed = start.duration(to: .now)
                let ms = Int(elapsed.components.seconds * 1000)
                    + Int(elapsed.components.attoseconds / 1_000_000_000_000_000)
                return BuiltDocument(doc: doc, ms: ms)
            }.value
            let (doc, ms) = (built.doc, built.ms)
            let label = "render build turns=\(slice.count) append=\(isAppend) len=\(doc.length) \(ms)ms"
            if Double(ms) >= BarLog.slowThresholdMs {
                BarLog.warn(.browser, "SLOW \(label)")
            } else {
                BarLog.info(.browser, label)
            }
            guard let self, !Task.isCancelled, self.selectedSessionId == sid else { return }
            self.renderVersion += 1
            self.renderOp = (self.renderVersion, isAppend ? .append(doc) : .set(doc))
        }
    }

    func setLive(_ live: Bool) {
        apply(selectionState.setLive(live, newestVisibleId: newestVisibleRow?.id))
    }

    private func client() -> AlexandriaClient? {
        guard let cfg = store.config ?? DaemonDiscovery.load() else { return nil }
        return AlexandriaClient(config: cfg)
    }

    private func pollSessions() async {
        guard let client = client() else {
            daemonDown = true
            return
        }
        do {
            let fetched = try await client.traceSessions(since: "24h", limit: 200)
            daemonDown = false
            let fingerprint = TraceFingerprint.sessions(fetched)
            if fingerprint != sessionsFingerprint {
                sessionsFingerprint = fingerprint
                BarLog.measure(.browser, label: "sessions apply count=\(fetched.count)") {
                    sessions = fetched.sorted { $0.lastTsMs > $1.lastTsMs }
                    rowsById = SessionTable.rowsById(sessions)
                    recomputeVisible()
                }
            }
            applyLiveFollow()
        } catch is AlexandriaClient.ClientError {
            daemonDown = false
        } catch {
            if !(error is CancellationError) { daemonDown = true }
        }
    }

    private func applyLiveFollow() {
        guard let candidate = newestVisibleRow else { return }
        guard candidate.id != selectedSessionId else { return }
        guard selectedSessionId != nil else {
            if !pinned { selectFromFollow(candidate.id) }
            return
        }
        let now = Int64(Date().timeIntervalSince1970 * 1000)
        let currentLast = selectedSession?.lastTsMs
        let idleMs = currentLast.map { max(0, now - $0) } ?? Int64.max
        let awayMs = scrolledAwayAt.map { Int64(-$0.timeIntervalSinceNow * 1000) } ?? 0
        guard LiveFollow.shouldSwitch(
            pinned: pinned, currentIdleMs: idleMs,
            userAtBottom: userAtBottom, awayFromBottomMs: awayMs)
        else { return }
        if let currentLast, candidate.lastTsMs <= currentLast { return }
        selectFromFollow(candidate.id)
    }

    private func pollTranscript() async {
        guard let sid = selectedSessionId, let client = client() else { return }
        do {
            let resp = try await client.traceTranscript(sessionId: sid, limit: 500)
            daemonDown = false
            guard resp.sessionId == selectedSessionId else { return }
            let fingerprint = TraceFingerprint.turns(resp.turns)
            if fingerprint != turnsFingerprint {
                turnsFingerprint = fingerprint
                BarLog.measure(.browser, label: "transcript apply \(sid) turns=\(resp.turns.count)") {
                    turns = resp.turns
                }
                scheduleRender()
                ensureFirstTraceDetail()
            }
        } catch is AlexandriaClient.ClientError {
            daemonDown = false
        } catch {
            if !(error is CancellationError) { daemonDown = true }
        }
    }

    private func queryChanged() {
        searchTask?.cancel()
        let query = parsedQuery
        guard !query.freeText.isEmpty else {
            searchSessionIds = nil
            recomputeVisible()
            return
        }
        recomputeVisible()
        searchTask = Task { [weak self] in
            try? await Task.sleep(for: .milliseconds(400))
            guard !Task.isCancelled else { return }
            await self?.runSearch(query)
        }
    }

    private func runSearch(_ query: OmniQuery) async {
        guard let client = client() else { return }
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
            searchSessionIds = []
            searchMatchCount = 0
            searchScanned = 0
            BarLog.warn(.browser, "search \"\(query.freeText)\" failed: \(error.localizedDescription)")
        }
        recomputeVisible()
    }

    func copySessionId(_ session: TraceSession) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(session.sessionId, forType: .string)
    }

    func copyLastReply(_ session: TraceSession) {
        Task {
            guard let client = client() else { return }
            do {
                let transcript = try await client.traceTranscript(sessionId: session.sessionId)
                guard let last = transcript.turns.last else { return }
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
        Task {
            guard let client = client() else { return }
            do {
                let transcript = try await client.traceTranscript(sessionId: session.sessionId)
                let markdown = Self.exportMarkdown(
                    sessionId: session.sessionId, turns: transcript.turns)
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
            if let error = turn.error, !error.isEmpty {
                out += "\n**Error:** \(error)\n"
            }
        }
        return out
    }

    func deleteSessionTraces(_ session: TraceSession) {
        let alert = NSAlert()
        alert.messageText = "Delete all traces for this session?"
        alert.informativeText =
            "Removes \(session.traceCount) trace(s) of session \(session.sessionId) from the daemon. This cannot be undone."
        alert.alertStyle = .warning
        alert.addButton(withTitle: "Delete")
        alert.addButton(withTitle: "Cancel")
        NSApp.activate(ignoringOtherApps: true)
        guard alert.runModal() == .alertFirstButtonReturn else { return }
        Task {
            guard let client = client() else { return }
            do {
                let transcript = try await client.traceTranscript(sessionId: session.sessionId)
                for turn in transcript.turns {
                    try await client.deleteTrace(id: turn.traceId)
                }
                if selectedSessionId == session.sessionId {
                    selectionState.clear()
                    resetTurns()
                }
                await pollSessions()
            } catch {
                NSSound.beep()
            }
        }
    }
}

private struct BuiltDocument: @unchecked Sendable {
    let doc: NSAttributedString
    let ms: Int
}

struct TraceBrowserView: View {
    @Bindable var model: TraceBrowserModel

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
                        maxWidth: 640, maxHeight: .infinity)
                    .background(
                        GeometryReader { proxy in
                            Color.clear.onChange(of: proxy.size.width) { _, width in
                                SplitStore.saveLeftWidth(width)
                            }
                        })
                TranscriptView(model: model)
                    .frame(minWidth: 400, maxWidth: .infinity, maxHeight: .infinity)
                if let traceId = model.inspectorTraceId {
                    TraceInspectorView(traceId: traceId, model: model)
                        .frame(
                            minWidth: 300, idealWidth: 340, maxWidth: 520, maxHeight: .infinity)
                }
            }
        }
        .frame(minWidth: 720, minHeight: 400)
    }

    private var toolbar: some View {
        HStack(spacing: 12) {
            HStack(spacing: 6) {
                Image(systemName: "magnifyingglass")
                    .foregroundStyle(.secondary)
                TextField(
                    "Search — free text + model: harness: task: job: tag:key=value status: run: session:",
                    text: $model.queryText
                )
                .textFieldStyle(.plain)
                .font(.system(size: 12, design: .monospaced))
                .onExitCommand { model.queryText = "" }
                if !model.queryText.isEmpty {
                    Button {
                        model.queryText = ""
                    } label: {
                        Image(systemName: "xmark.circle.fill")
                            .foregroundStyle(.secondary)
                    }
                    .buttonStyle(.plain)
                }
            }
            .padding(.horizontal, 8)
            .padding(.vertical, 5)
            .background(RoundedRectangle(cornerRadius: 6).fill(.quaternary.opacity(0.5)))
            Toggle("Live", isOn: Binding(
                get: { !model.pinned },
                set: { model.setLive($0) }
            ))
            .toggleStyle(.switch)
            .controlSize(.small)
            Toggle("Show pings", isOn: $model.showPings)
                .controlSize(.small)
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
                menuItemLabel("All", checked: active == nil)
            }
            Divider()
            ForEach(values, id: \.self) { value in
                Button {
                    model.setFilter(dimension, value)
                } label: {
                    menuItemLabel(value, checked: active == value)
                }
            }
        } label: {
            HStack(spacing: 3) {
                Text(active.map { "\(dimension.rawValue): \($0)" } ?? dimension.title)
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
        guard stored >= 280, stored <= 640 else { return 380 }
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
            ScrollViewReader { proxy in
                table
                    .onChange(of: model.selectedSessionId) { _, id in
                        if let id { proxy.scrollTo(id) }
                    }
            }
            Divider()
            footer
        }
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
            if let id = ids.first,
                let session = model.sessions.first(where: { $0.sessionId == id }) {
                contextMenu(session)
            }
        }
        .overlay {
            if model.visibleRows.isEmpty {
                Text(model.sessions.isEmpty ? "No sessions in the last 24h" : "No sessions match")
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
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
                showPingBadge: model.showPings && row.isPingOrTest)
        }
        .width(min: 180)
        .customizationID("session")
        .disabledCustomizationBehavior(.visibility)
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
        TableColumn("Model(s)", value: \.models) { (row: SessionRow) in
            Text(row.models)
                .font(.system(size: 10))
                .foregroundStyle(.secondary)
                .lineLimit(1)
        }
        .customizationID("models")
        TableColumn("Harness", value: \.harness) { (row: SessionRow) in
            Text(row.harness)
                .font(.system(size: 10))
                .foregroundStyle(.secondary)
                .lineLimit(1)
        }
        .customizationID("harness")
        .defaultVisibility(.hidden)
        TableColumn("Run", value: \.runId) { (row: SessionRow) in
            Text(row.runId)
                .font(.system(size: 10, design: .monospaced))
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .truncationMode(.middle)
        }
        .customizationID("run")
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
            if model.searchSessionIds != nil {
                Text("\(model.searchMatchCount) matches · scanned \(model.searchScanned)")
                    .foregroundStyle(.secondary)
            }
            Spacer()
            Text("Right-click headers to show/hide columns")
                .foregroundStyle(.tertiary)
        }
        .font(.system(size: 10))
        .padding(.horizontal, 10)
        .padding(.vertical, 4)
    }

    private func numericCell(_ text: String) -> some View {
        Text(text)
            .font(.system(size: 10, design: .monospaced))
            .foregroundStyle(.secondary)
            .frame(maxWidth: .infinity, alignment: .trailing)
    }

    private var selectionBinding: Binding<String?> {
        Binding(
            get: { model.selectedSessionId },
            set: { id in
                model.selectFromBinding(id)
                listFocused = true
            })
    }

    @ViewBuilder
    private func contextMenu(_ session: TraceSession) -> some View {
        let isPinnedRow = model.pinned && session.sessionId == model.selectedSessionId
        Button(isPinnedRow ? "Unpin" : "Pin") {
            if isPinnedRow {
                model.setLive(true)
            } else {
                model.selectFromUser(session.sessionId)
            }
        }
        Button("Copy Session ID") { model.copySessionId(session) }
        Button("Copy Last Reply as Markdown") { model.copyLastReply(session) }
        Button("Export Session…") { model.exportSession(session) }
        Button("Reveal Bodies in Finder") { model.revealSessionBodies(session) }
        Divider()
        Button("Delete Session's Traces…", role: .destructive) {
            model.deleteSessionTraces(session)
        }
    }
}

private struct SessionCellView: View {
    let row: SessionRow
    let pinned: Bool
    let showPingBadge: Bool

    var body: some View {
        HStack(spacing: 5) {
            HarnessIconView(harness: row.harnessRaw, tags: row.tags, size: 16)
            if !row.providers.isEmpty {
                HStack(spacing: 3) {
                    ForEach(row.providers, id: \.self) { provider in
                        ProviderBadgeView(provider: provider)
                    }
                }
            }
            Text(row.sessionShort)
                .font(.system(size: 11, weight: .medium, design: .monospaced))
                .lineLimit(1)
                .truncationMode(.middle)
            if row.errors > 0 {
                Text("✗ \(row.errors)")
                    .font(.system(size: 9, weight: .semibold, design: .monospaced))
                    .foregroundStyle(.white)
                    .padding(.horizontal, 4)
                    .padding(.vertical, 1)
                    .background(Capsule().fill(.red))
                    .help("\(row.errors) failed request\(row.errors == 1 ? "" : "s")")
            }
            if pinned {
                Image(systemName: "pin.fill")
                    .font(.system(size: 8))
                    .foregroundStyle(.orange)
            }
            if showPingBadge, let badge = row.kindBadge {
                Text("[\(badge)]")
                    .font(.system(size: 9))
                    .foregroundStyle(.tertiary)
            }
        }
    }
}

private struct TranscriptView: View {
    @Bindable var model: TraceBrowserModel
    @AppStorage("SessionInfoExpanded") private var infoExpanded = false

    var body: some View {
        VStack(spacing: 0) {
            header
            if infoExpanded, model.selectedSession != nil {
                Divider()
                SessionInfoCard(model: model)
            }
            Divider()
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
                TranscriptTextPane(model: model)
                if model.turns.isEmpty {
                    Text(model.selectedSessionId == nil ? "Select a session" : "No turns yet")
                        .font(.system(size: 11))
                        .foregroundStyle(.secondary)
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                }
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
                    .padding(.bottom, 12)
                }
            }
        }
    }

    private var header: some View {
        HStack(spacing: 8) {
            if let session = model.selectedSession {
                HarnessIconView(harness: session.harness, tags: session.tags, size: 18)
                if model.pinned {
                    Button {
                        model.setLive(true)
                    } label: {
                        Image(systemName: "pin.fill")
                            .foregroundStyle(.orange)
                    }
                    .buttonStyle(.plain)
                    .help("Pinned — click to unpin and follow live")
                } else {
                    Image(systemName: "dot.radiowaves.left.and.right")
                        .foregroundStyle(.green)
                        .help("Live — following the most recent session")
                }
                Text(session.sessionId)
                    .font(.system(size: 11, weight: .medium, design: .monospaced))
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .textSelection(.enabled)
                let chips = SessionTagChips.chips(
                    tags: session.tags, harness: session.harness, models: session.models)
                ForEach(chips, id: \.key) { chip in
                    TagChipView(text: chip.label())
                }
                if let models = session.models, !models.isEmpty {
                    Text(models.joined(separator: ", "))
                        .font(.system(size: 10))
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
                if let cost = session.totalCostUsd, cost > 0 {
                    Text(TraceFormat.cost(cost))
                        .font(.system(size: 10, design: .monospaced))
                        .foregroundStyle(.secondary)
                }
                if let runId = session.runId, !runId.isEmpty {
                    Text("run \(runId)")
                        .font(.system(size: 10, design: .monospaced))
                        .foregroundStyle(.tertiary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                        .frame(maxWidth: 120)
                }
                Spacer()
                Text("\(model.turns.count) turn\(model.turns.count == 1 ? "" : "s")")
                    .font(.system(size: 10))
                    .foregroundStyle(.secondary)
                Button {
                    infoExpanded.toggle()
                    if infoExpanded { model.ensureFirstTraceDetail() }
                } label: {
                    Image(systemName: infoExpanded ? "chevron.up" : "chevron.down")
                        .font(.system(size: 9, weight: .semibold))
                        .foregroundStyle(.secondary)
                }
                .buttonStyle(.plain)
                .help(infoExpanded ? "Hide session info" : "Show session info")
            } else {
                Text("No session selected")
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
                Spacer()
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 7)
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

    func show() {
        if window == nil {
            let model = TraceBrowserModel(store: store)
            self.model = model
            let host = NSHostingController(rootView: TraceBrowserView(model: model))
            let win = NSWindow(contentViewController: host)
            win.title = "Alexandria — Trace Browser"
            win.styleMask = [.titled, .closable, .miniaturizable, .resizable]
            win.isReleasedWhenClosed = false
            win.delegate = self
            win.setContentSize(NSSize(width: 980, height: 620))
            win.center()
            win.setFrameAutosaveName("AlexandriaTraceBrowser")
            window = win
        }
        BarLog.info(.ui, "trace browser opened")
        model?.start()
        if let window {
            DockIconManager.shared.track(window)
            window.makeKeyAndOrderFront(nil)
            NSApp.activate(ignoringOtherApps: true)
        }
    }

    func windowWillClose(_ notification: Notification) {
        BarLog.info(.ui, "trace browser closed")
        model?.stop()
    }
}
