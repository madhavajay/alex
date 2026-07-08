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
    private(set) var daemonDown = false
    private(set) var searchSessionIds: Set<String>?
    private(set) var searchMatchCount = 0
    private(set) var searchScanned = 0

    var selectedSessionId: String?
    var pinned = false
    var showPings = false
    var queryText = "" {
        didSet { queryChanged() }
    }

    var userAtBottom = true {
        didSet {
            if userAtBottom {
                scrolledAwayAt = nil
            } else if scrolledAwayAt == nil {
                scrolledAwayAt = Date()
            }
        }
    }
    private var scrolledAwayAt: Date?

    private var sessionsTask: Task<Void, Never>?
    private var transcriptTask: Task<Void, Never>?
    private var searchTask: Task<Void, Never>?

    init(store: SnapshotStore) {
        self.store = store
    }

    var parsedQuery: OmniQuery { OmniQuery.parse(queryText) }

    var visibleSessions: [TraceSession] {
        let query = parsedQuery
        return sessions.filter { session in
            if !showPings, session.isPingOrTest { return false }
            return query.isVisible(session, serverMatches: searchSessionIds)
        }
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
    }

    func select(_ session: TraceSession) {
        guard session.sessionId != selectedSessionId else {
            pinned = true
            return
        }
        selectedSessionId = session.sessionId
        pinned = true
        turns = []
        userAtBottom = true
        Task { await pollTranscript() }
    }

    func setLive(_ live: Bool) {
        pinned = !live
        if live { applyLiveFollow() }
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
            sessions = fetched.sorted { $0.lastTsMs > $1.lastTsMs }
            daemonDown = false
            applyLiveFollow()
        } catch is AlexandriaClient.ClientError {
            daemonDown = false
        } catch {
            if !(error is CancellationError) { daemonDown = true }
        }
    }

    private func applyLiveFollow() {
        guard let candidate = visibleSessions.first else { return }
        guard candidate.sessionId != selectedSessionId else { return }
        guard selectedSessionId != nil else {
            if !pinned {
                selectedSessionId = candidate.sessionId
                turns = []
                userAtBottom = true
            }
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
        selectedSessionId = candidate.sessionId
        turns = []
        userAtBottom = true
        Task { await pollTranscript() }
    }

    private func pollTranscript() async {
        guard let sid = selectedSessionId, let client = client() else { return }
        do {
            let resp = try await client.traceTranscript(sessionId: sid, limit: 500)
            if resp.sessionId == selectedSessionId { turns = resp.turns }
            daemonDown = false
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
            return
        }
        searchTask = Task { [weak self] in
            try? await Task.sleep(for: .milliseconds(400))
            guard !Task.isCancelled else { return }
            await self?.runSearch(query)
        }
    }

    private func runSearch(_ query: OmniQuery) async {
        guard let client = client() else { return }
        do {
            let resp = try await client.searchTraces(text: query.freeText, filters: query)
            guard parsedQuery == query else { return }
            searchSessionIds = Set(resp.traces.compactMap(\.sessionId))
            searchMatchCount = resp.traces.count
            searchScanned = resp.scanned ?? 0
        } catch {
            guard parsedQuery == query else { return }
            searchSessionIds = []
            searchMatchCount = 0
            searchScanned = 0
        }
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
                    selectedSessionId = nil
                    turns = []
                }
                await pollSessions()
            } catch {
                NSSound.beep()
            }
        }
    }
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
                    .frame(minWidth: 300, idealWidth: 380, maxWidth: 560, maxHeight: .infinity)
                TranscriptView(model: model)
                    .frame(minWidth: 380, maxWidth: .infinity, maxHeight: .infinity)
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

private struct SessionListView: View {
    @Bindable var model: TraceBrowserModel

    var body: some View {
        VStack(spacing: 0) {
            ScrollView {
                LazyVStack(spacing: 2) {
                    ForEach(model.visibleSessions) { session in
                        SessionRowView(
                            session: session,
                            selected: session.sessionId == model.selectedSessionId,
                            pinned: model.pinned && session.sessionId == model.selectedSessionId,
                            showPingBadge: model.showPings && session.isPingOrTest
                        )
                        .contentShape(Rectangle())
                        .onTapGesture { model.select(session) }
                        .contextMenu { contextMenu(session) }
                    }
                    if model.visibleSessions.isEmpty {
                        Text(model.sessions.isEmpty ? "No sessions in the last 24h" : "No sessions match")
                            .font(.system(size: 11))
                            .foregroundStyle(.secondary)
                            .padding(.top, 24)
                    }
                }
                .padding(6)
            }
            if model.searchSessionIds != nil {
                Divider()
                Text("\(model.searchMatchCount) matches · scanned \(model.searchScanned)")
                    .font(.system(size: 10))
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 10)
                    .padding(.vertical, 4)
            }
        }
    }

    @ViewBuilder
    private func contextMenu(_ session: TraceSession) -> some View {
        let isPinnedRow = model.pinned && session.sessionId == model.selectedSessionId
        Button(isPinnedRow ? "Unpin" : "Pin") {
            if isPinnedRow {
                model.setLive(true)
            } else {
                model.select(session)
            }
        }
        Button("Copy Session ID") { model.copySessionId(session) }
        Button("Copy Last Reply as Markdown") { model.copyLastReply(session) }
        Button("Export Session…") { model.exportSession(session) }
        Divider()
        Button("Delete Session's Traces…", role: .destructive) {
            model.deleteSessionTraces(session)
        }
    }
}

private struct SessionRowView: View {
    let session: TraceSession
    let selected: Bool
    let pinned: Bool
    let showPingBadge: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            HStack(spacing: 6) {
                Text(TraceFormat.relative(session.lastTsMs))
                    .font(.system(size: 10))
                    .foregroundStyle(.secondary)
                if pinned {
                    Image(systemName: "pin.fill")
                        .font(.system(size: 8))
                        .foregroundStyle(.orange)
                }
                Spacer()
                if let errors = session.errors, errors > 0 {
                    Text("\(errors) err")
                        .font(.system(size: 9, weight: .semibold))
                        .foregroundStyle(.white)
                        .padding(.horizontal, 5)
                        .padding(.vertical, 1)
                        .background(Capsule().fill(.red))
                }
                if showPingBadge {
                    Text("[ping]")
                        .font(.system(size: 9))
                        .foregroundStyle(.tertiary)
                }
            }
            Text(session.sessionId)
                .font(.system(size: 11, weight: .medium, design: .monospaced))
                .lineLimit(1)
                .truncationMode(.middle)
            HStack(spacing: 8) {
                Text((session.models ?? []).joined(separator: ", "))
                    .font(.system(size: 10))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                Spacer()
            }
            let chips = SessionTagChips.chips(
                tags: session.tags, harness: session.harness, models: session.models)
            if !chips.isEmpty {
                HStack(spacing: 4) {
                    ForEach(chips, id: \.key) { chip in
                        TagChipView(text: chip.label())
                    }
                    Spacer()
                }
            }
            HStack(spacing: 8) {
                Text("\(session.traceCount) turn\(session.traceCount == 1 ? "" : "s")")
                Text("\(TraceFormat.tokens(session.totalInputTokens))→\(TraceFormat.tokens(session.totalOutputTokens)) tok")
                if let cost = session.totalCostUsd, cost > 0 {
                    Text(TraceFormat.cost(cost))
                }
                Spacer()
            }
            .font(.system(size: 10, design: .monospaced))
            .foregroundStyle(.secondary)
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 6)
        .background(
            RoundedRectangle(cornerRadius: 6)
                .fill(selected ? Color.accentColor.opacity(0.18) : Color.clear)
        )
    }
}

private struct TranscriptView: View {
    @Bindable var model: TraceBrowserModel

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider()
            ScrollViewReader { proxy in
                ZStack(alignment: .bottom) {
                    ScrollView {
                        LazyVStack(alignment: .leading, spacing: 14) {
                            ForEach(model.turns) { TurnView(turn: $0) }
                            if model.turns.isEmpty {
                                Text(model.selectedSessionId == nil
                                    ? "Select a session"
                                    : "No turns yet")
                                    .font(.system(size: 11))
                                    .foregroundStyle(.secondary)
                                    .padding(.top, 24)
                                    .frame(maxWidth: .infinity)
                            }
                            Color.clear
                                .frame(height: 1)
                                .id("bottom")
                                .onAppear { model.userAtBottom = true }
                                .onDisappear { model.userAtBottom = false }
                        }
                        .padding(12)
                    }
                    if !model.userAtBottom, !model.turns.isEmpty {
                        Button {
                            model.userAtBottom = true
                            proxy.scrollTo("bottom", anchor: .bottom)
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
                .onChange(of: model.turns.count) {
                    if model.userAtBottom {
                        proxy.scrollTo("bottom", anchor: .bottom)
                    }
                }
                .onChange(of: model.selectedSessionId) {
                    proxy.scrollTo("bottom", anchor: .bottom)
                }
            }
        }
    }

    private var header: some View {
        HStack(spacing: 8) {
            if let session = model.selectedSession {
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
                Spacer()
                Text("\(model.turns.count) turn\(model.turns.count == 1 ? "" : "s")")
                    .font(.system(size: 10))
                    .foregroundStyle(.secondary)
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

private struct TurnView: View {
    let turn: TranscriptTurn

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            headerLine
            if let user = turn.user, !user.isEmpty {
                HStack(alignment: .top, spacing: 8) {
                    RoundedRectangle(cornerRadius: 1.5)
                        .fill(Color.accentColor.opacity(0.7))
                        .frame(width: 3)
                    Text(user)
                        .font(.system(size: 12, design: .monospaced))
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                .padding(8)
                .background(
                    RoundedRectangle(cornerRadius: 6)
                        .fill(Color.accentColor.opacity(0.07)))
                .fixedSize(horizontal: false, vertical: true)
            }
            if let assistant = turn.assistant, !assistant.isEmpty {
                Text(assistant)
                    .font(.system(size: 12))
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .fixedSize(horizontal: false, vertical: true)
            }
            if let error = turn.error, !error.isEmpty {
                Text(error)
                    .font(.system(size: 11, design: .monospaced))
                    .foregroundStyle(.red)
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    private var headerLine: some View {
        HStack(spacing: 0) {
            Text(headerText)
                .font(.system(size: 10, design: .monospaced))
                .foregroundStyle(.secondary)
            if let status = turn.status {
                Text(" · \(status)")
                    .font(.system(size: 10, weight: status >= 400 ? .bold : .regular, design: .monospaced))
                    .foregroundStyle(status >= 400 ? .red : .secondary)
            }
            Spacer()
        }
    }

    private var headerText: String {
        var parts = [TraceFormat.time(turn.tsRequestMs)]
        if let model = turn.model { parts.append(model) }
        parts.append("\(TraceFormat.tokens(turn.inputTokens))→\(TraceFormat.tokens(turn.outputTokens)) tok")
        if let cost = turn.costUsd, cost > 0 { parts.append(TraceFormat.cost(cost)) }
        return parts.joined(separator: " · ")
    }
}

enum TraceFormat {
    static func relative(_ tsMs: Int64, now: Date = Date()) -> String {
        let delta = Int64(now.timeIntervalSince1970) - tsMs / 1000
        if delta < 10 { return "now" }
        return "\(Format.duration(delta)) ago"
    }

    static func time(_ tsMs: Int64) -> String {
        let formatter = DateFormatter()
        formatter.dateFormat = "HH:mm:ss"
        return formatter.string(from: Date(timeIntervalSince1970: Double(tsMs) / 1000))
    }

    static func tokens(_ count: Int64?) -> String {
        guard let count else { return "–" }
        if count >= 1_000_000 { return String(format: "%.1fM", Double(count) / 1_000_000) }
        if count >= 10_000 { return "\(count / 1000)k" }
        if count >= 1_000 { return String(format: "%.1fk", Double(count) / 1000) }
        return "\(count)"
    }

    static func cost(_ usd: Double) -> String {
        usd >= 0.01 ? String(format: "$%.2f", usd) : String(format: "$%.4f", usd)
    }
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
        model?.start()
        if let window {
            DockIconManager.shared.track(window)
            window.makeKeyAndOrderFront(nil)
            NSApp.activate(ignoringOtherApps: true)
        }
    }

    func windowWillClose(_ notification: Notification) {
        model?.stop()
    }
}
