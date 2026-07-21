import AppKit
import SwiftUI
import Observation
import AlexandriaBarCore

@MainActor
@Observable
final class DarioModel {
    enum LogChannel: String, CaseIterable {
        case stdout, stderr
    }

    private let store: SnapshotStore

    private(set) var status: DarioAdminStatus?
    private(set) var disabled = false
    private(set) var daemonDown = false
    private(set) var logs: DarioLogsResponse?
    private(set) var actionResult: String?
    private(set) var actionInFlight = false

    var logChannel = LogChannel.stdout
    var selectedGenerationId: String? {
        didSet {
            guard oldValue != selectedGenerationId else { return }
            logs = nil
            userAtBottom = true
            Task { await pollLogs() }
        }
    }

    private(set) var userAtBottom = true

    func setUserAtBottom(_ value: Bool) {
        guard userAtBottom != value else { return }
        userAtBottom = value
    }

    private var statusTask: Task<Void, Never>?
    private var logsTask: Task<Void, Never>?
    private var actionClearTask: Task<Void, Never>?

    init(store: SnapshotStore) {
        self.store = store
    }

    var generations: [DarioGenerationDetail] {
        status?.generations ?? []
    }

    var promptCaches: [DarioPromptCacheSummary] {
        status?.promptCaches ?? []
    }

    var activeGeneration: DarioGenerationDetail? {
        guard let id = status?.activeGenerationId else { return nil }
        return generations.first { $0.id == id }
    }

    var selectedGeneration: DarioGenerationDetail? {
        generations.first { $0.id == selectedGenerationId }
    }

    var logText: String {
        guard let logs else { return "" }
        return logChannel == .stdout ? logs.stdout : logs.stderr
    }

    func start() {
        stop()
        statusTask = Task { [weak self] in
            while !Task.isCancelled {
                await self?.pollStatus()
                try? await Task.sleep(for: .seconds(2))
            }
        }
        logsTask = Task { [weak self] in
            while !Task.isCancelled {
                await self?.pollLogs()
                try? await Task.sleep(for: .seconds(2))
            }
        }
    }

    func stop() {
        statusTask?.cancel()
        statusTask = nil
        logsTask?.cancel()
        logsTask = nil
    }

    private func client() -> AlexandriaClient? {
        guard let cfg = store.config ?? DaemonDiscovery.load() else { return nil }
        return AlexandriaClient(config: cfg)
    }

    private func pollStatus() async {
        guard let client = client() else {
            daemonDown = true
            return
        }
        do {
            let fetched = try await client.darioDetail()
            daemonDown = false
            disabled = fetched == nil
            status = fetched
            let ids = Set(generations.map(\.id))
            if selectedGenerationId == nil || !ids.contains(selectedGenerationId ?? "") {
                selectedGenerationId = status?.activeGenerationId ?? generations.first?.id
            }
        } catch is AlexandriaClient.ClientError {
            daemonDown = false
        } catch {
            if !(error is CancellationError) { daemonDown = true }
        }
    }

    private func pollLogs() async {
        guard let id = selectedGenerationId, let client = client() else { return }
        do {
            let fetched = try await client.darioLogs(generationId: id, lines: 300)
            guard fetched.generationId == selectedGenerationId else { return }
            logs = fetched
        } catch {
            BarLog.warn(.ui, "dario logs \(id) failed: \(error.localizedDescription)")
        }
    }

    func copyGenerationId(_ generation: DarioGenerationDetail) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(generation.id, forType: .string)
    }

    func revealLog(_ generation: DarioGenerationDetail) {
        guard let path = generation.stdoutLog else {
            NSSound.beep()
            return
        }
        NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: path)])
    }

    func clearPromptCache(_ cache: DarioPromptCacheSummary) {
        guard let client = client() else { return }
        actionInFlight = true
        Task { [weak self] in
            do {
                try await client.darioPromptCacheClear(key: cache.key)
                self?.showActionResult("cleared \(cache.model ?? cache.key)")
            } catch {
                self?.showActionResult("failed: \(error.localizedDescription)")
            }
            self?.actionInFlight = false
            await self?.pollStatus()
        }
    }

    func confirmAction(update: Bool) {
        let alert = NSAlert()
        alert.messageText = update ? "Check for dario update?" : "Restart dario?"
        alert.informativeText = update
            ? "Asks the daemon to check for and roll out a new dario generation."
            : "Spawns a fresh dario generation and drains the current one. In-flight requests finish on the old generation."
        alert.addButton(withTitle: update ? "Check Update" : "Restart")
        alert.addButton(withTitle: "Cancel")
        NSApp.activate(ignoringOtherApps: true)
        guard alert.runModal() == .alertFirstButtonReturn else { return }
        runAction(update: update)
    }

    private func runAction(update: Bool) {
        guard let client = client() else { return }
        actionInFlight = true
        BarLog.info(.ui, "dario \(update ? "update" : "restart") requested")
        Task { [weak self] in
            do {
                if update {
                    try await client.darioUpdate()
                } else {
                    try await client.darioRestart()
                }
                self?.showActionResult(update ? "update check triggered" : "restart triggered")
            } catch {
                self?.showActionResult("failed: \(error.localizedDescription)")
            }
            self?.actionInFlight = false
            await self?.pollStatus()
        }
    }

    private func showActionResult(_ text: String) {
        actionResult = text
        actionClearTask?.cancel()
        actionClearTask = Task { [weak self] in
            try? await Task.sleep(for: .seconds(6))
            guard !Task.isCancelled else { return }
            self?.actionResult = nil
        }
    }
}

/// Local styling helpers for the Dario window (mock: ui/Dario/src/app/App.tsx).
private enum DarioStyle {
    static func cacheStatusTint(_ status: String) -> Color {
        switch status.lowercased() {
        case "hit", "ready", "ok", "cached": AlexTheme.Colors.success
        case "miss", "stale": AlexTheme.Colors.warningOrange
        case "error", "failed": AlexTheme.Colors.destructive
        default: AlexTheme.Colors.textSecondary
        }
    }

    /// Per-line console tinting (mock STDOUT_LINES / STDERR_LINES palette).
    static func lineColor(_ line: String) -> Color {
        let trimmed = line.trimmingCharacters(in: .whitespaces)
        let lower = trimmed.lowercased()
        if trimmed.isEmpty { return AlexTheme.Colors.foreground }
        if lower.hasPrefix("[warn]") || lower.hasPrefix("warn") {
            return AlexTheme.Colors.warningOrange
        }
        if lower.hasPrefix("[error]") || lower.contains("error:")
            || lower.hasPrefix("failed") {
            return AlexTheme.Colors.destructive
        }
        if trimmed.contains("http://") || trimmed.contains("https://") {
            return AlexTheme.Colors.primary
        }
        if lower.contains("healthy") || lower.contains("is now an api")
            || trimmed.hasPrefix("✓") {
            return AlexTheme.Colors.success
        }
        if lower.hasPrefix("dario |") {
            return AlexTheme.Colors.mutedForeground
        }
        if line.hasPrefix("  ") || lower.hasPrefix("usage:") {
            return AlexTheme.Colors.textSecondary
        }
        return AlexTheme.Colors.foreground
    }

    static func styledLog(_ text: String) -> AttributedString {
        var result = AttributedString()
        let lines = text.split(separator: "\n", omittingEmptySubsequences: false)
        for (index, sub) in lines.enumerated() {
            var attr = AttributedString(String(sub))
            attr.foregroundColor = lineColor(String(sub))
            result += attr
            if index < lines.count - 1 {
                result += AttributedString("\n")
            }
        }
        return result
    }
}

private extension DarioHealthState {
    var displayStatus: DisplayStatus {
        switch self {
        case .ready: .success
        case .warming: .running
        case .down: .error
        }
    }
}

struct DarioView: View {
    @Bindable var model: DarioModel
    @State private var showingWhatIsDario = false

    var body: some View {
        VStack(spacing: 0) {
            if model.daemonDown {
                daemonBanner
            }
            if model.disabled {
                EmptyStateView(message: "dario mode disabled", style: .panel(icon: "cpu"))
            } else {
                header
                helpBand
                statStrip
                VSplitView {
                    ScrollView {
                        VStack(spacing: 0) {
                            GenerationSection(model: model)
                            PromptCacheSection(model: model)
                        }
                    }
                    .frame(minHeight: 180, idealHeight: 280)
                    LogSection(model: model)
                        .frame(minHeight: 160, maxHeight: .infinity)
                }
            }
        }
        .background(AlexTheme.Colors.background)
        .frame(minWidth: 760, minHeight: 420)
    }

    private var daemonBanner: some View {
        HStack(spacing: AlexTheme.Spacing.sm) {
            Image(systemName: "bolt.slash")
            Text("daemon not running — retrying…")
            Spacer()
        }
        .font(.system(size: 11))
        .foregroundStyle(AlexTheme.Colors.warningOrange)
        .padding(.horizontal, 12)
        .padding(.vertical, 5)
        .background(AlexTheme.Colors.warningOrange.opacity(0.12))
        .overlay(alignment: .bottom) {
            Rectangle().fill(AlexTheme.Colors.cardBorder).frame(height: 1)
        }
    }

    // Mock Header: 48px band — 30×30 cpu tile, title over dim subtitle,
    // bordered Restart / Check Update pills (Dario App.tsx:320-352).
    private var header: some View {
        HStack(spacing: 12) {
            RoundedRectangle(cornerRadius: AlexTheme.Radius.md)
                .fill(AlexTheme.Colors.primary.opacity(0.15))
                .overlay(
                    RoundedRectangle(cornerRadius: AlexTheme.Radius.md)
                        .strokeBorder(AlexTheme.Colors.primary.opacity(0.22)))
                .overlay(
                    Image(systemName: "cpu")
                        .font(.system(size: 13, weight: .medium))
                        .foregroundStyle(AlexTheme.Colors.primary))
                .frame(width: 30, height: 30)
            VStack(alignment: .leading, spacing: 1) {
                Text("Alex UI — Dario")
                    .font(.system(size: 14, weight: .semibold))
                    .foregroundStyle(AlexTheme.Colors.foreground)
                Text(headerSubtitle)
                    .font(.system(size: 10))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
            if let active = model.activeGeneration {
                let health = DarioHealth.evaluate(
                    phase: active.phase, lastProbeOK: active.lastProbe?.ok)
                StatusChip(
                    tint: health.tint.color, text: health.label)
            }
            Spacer()
            if let result = model.actionResult {
                Text(result)
                    .font(.system(size: 11))
                    .foregroundStyle(
                        result.hasPrefix("failed")
                            ? AlexTheme.Colors.destructive
                            : AlexTheme.Colors.textSecondary)
                    .lineLimit(1)
            }
            PanelIconButton(
                systemImage: "questionmark.circle", help: "What is Dario?"
            ) {
                showingWhatIsDario = true
            }
            .popover(isPresented: $showingWhatIsDario, arrowEdge: .bottom) {
                whatIsDarioPopover
            }
            PillButton(
                title: "Restart", variant: .bordered,
                isEnabled: !model.actionInFlight
            ) {
                model.confirmAction(update: false)
            }
            PillButton(
                title: "Check Update", variant: .bordered,
                isEnabled: !model.actionInFlight
            ) {
                model.confirmAction(update: true)
            }
        }
        .padding(.horizontal, 20)
        .frame(height: AlexTheme.Metrics.panelHeaderHeight)
        .overlay(alignment: .bottom) {
            Rectangle().fill(AlexTheme.Colors.cardBorder).frame(height: 1)
        }
    }

    private var whatIsDarioPopover: some View {
        VStack(alignment: .leading, spacing: 12) {
            Label("What is Dario?", systemImage: "questionmark.circle")
                .font(.system(size: 13, weight: .semibold))
                .foregroundStyle(AlexTheme.Colors.foreground)
            Text("Dario is the supervised Anthropic path for non-Claude-Code clients, with health probes, automatic updates, and rolling restarts. Genuine Claude Code remains direct.")
                .font(.system(size: 11))
                .foregroundStyle(AlexTheme.Colors.textSecondary)
                .fixedSize(horizontal: false, vertical: true)
            Link(destination: URL(string: "https://github.com/askalf/dario")!) {
                Label("Open Dario on GitHub", systemImage: "arrow.up.right.square")
                    .font(.system(size: 11, weight: .medium))
            }
        }
        .padding(16)
        .frame(width: 320, alignment: .leading)
    }

    private var headerSubtitle: String {
        guard let active = model.activeGeneration else {
            return "Dario — no active generation"
        }
        return "Dario \(active.version) — active \(active.id)"
    }

    // Mock SubtitleHelp: #141414 band, mono dim copy with accent span
    // (Dario App.tsx:570-582).
    private var helpBand: some View {
        (Text("Process/generation health + logs. Dario-routed traffic shows up in the Trace Browser under account ")
            .foregroundColor(AlexTheme.Colors.textTertiary)
            + Text("dario:<generation>")
            .foregroundColor(AlexTheme.Colors.primary))
            .font(AlexTheme.Fonts.mono(10.5))
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 20)
            .padding(.vertical, 8)
            .background(AlexTheme.Colors.surfaceSunken)
            .overlay(alignment: .bottom) {
                Rectangle().fill(AlexTheme.Colors.cardBorder).frame(height: 1)
            }
    }

    private var statStrip: some View {
        StatTilesRow(
            items: [
                StatTileData(
                    label: "Generations",
                    value: "\(model.generations.count)",
                    valueTint: unhealthyCount > 0 ? AlexTheme.Colors.destructive : nil),
                StatTileData(
                    label: "In flight",
                    value: "\(inFlightTotal)"),
                StatTileData(
                    label: "Prompt caches",
                    value: "\(model.promptCaches.count)"),
            ],
            style: .bordered)
    }

    private var unhealthyCount: Int {
        model.generations.filter { $0.phase == "unhealthy" }.count
    }

    private var inFlightTotal: Int {
        model.generations.compactMap(\.inFlight).reduce(0, +)
    }
}

// MARK: - Generation table

private struct GenerationSection: View {
    @Bindable var model: DarioModel

    private static let columns: [MiniTableColumn] = [
        MiniTableColumn(title: "generation"),
        MiniTableColumn(title: "version", width: 60),
        MiniTableColumn(title: "phase", width: 80),
        MiniTableColumn(title: "port", width: 60),
        MiniTableColumn(title: "pid", width: 60),
        MiniTableColumn(title: "busy", width: 40),
        MiniTableColumn(title: "probe", width: 110),
        MiniTableColumn(title: "age", width: 60, alignment: .trailing),
    ]

    var body: some View {
        VStack(alignment: .leading, spacing: AlexTheme.Spacing.md) {
            SectionLabel(text: "Generation", style: .prominent)
            MiniTable(
                columns: Self.columns, rows: rows,
                emptyMessage: "No generations")
        }
        .padding(16)
    }

    private var rows: [MiniTableRow] {
        model.generations.map { generation in
            let isActive = generation.id == model.status?.activeGenerationId
            return MiniTableRow(
                id: generation.id,
                cells: [
                    .text(generation.id,
                          tint: isActive ? AlexTheme.Colors.primary : nil,
                          bold: isActive, truncation: .middle),
                    .text(generation.version, bold: isActive),
                    .text(generation.phase,
                          tint: DarioHealth.evaluate(
                              phase: generation.phase,
                              lastProbeOK: generation.lastProbe?.ok).tint.color,
                          bold: generation.phase == "ready"),
                    .text(generation.port.map(String.init) ?? "–"),
                    .text(generation.pid.map(String.init) ?? "–"),
                    .text(generation.inFlight.map(String.init) ?? "–"),
                    probeCell(generation),
                    .text(age(generation)),
                ],
                isActive: isActive,
                isSelected: generation.id == model.selectedGenerationId,
                onSelect: { [weak model] in
                    model?.selectedGenerationId = generation.id
                },
                contextMenu: [
                    MiniTableMenuItem(title: "Copy Generation ID") { [weak model] in
                        model?.copyGenerationId(generation)
                    },
                    MiniTableMenuItem(title: "Reveal Log in Finder") { [weak model] in
                        model?.revealLog(generation)
                    },
                ])
        }
    }

    private func probeCell(_ generation: DarioGenerationDetail) -> MiniTableCell {
        guard let probe = generation.lastProbe else {
            return .text("–", tint: AlexTheme.Colors.textTertiary)
        }
        if probe.ok {
            return .text(
                "✓ \(probe.latencyMs.map { "\($0)ms" } ?? "ok")",
                tint: AlexTheme.Colors.success)
        }
        return .text(
            "✗ \(probe.error ?? "failed")",
            tint: AlexTheme.Colors.destructive,
            help: probe.error ?? "probe failed")
    }

    private func age(_ generation: DarioGenerationDetail) -> String {
        guard let started = generation.startedAt else { return "–" }
        let delta = Int64(Date().timeIntervalSince1970) - started / 1000
        return Format.duration(max(0, delta))
    }
}

// MARK: - Prompt cache table

private struct PromptCacheSection: View {
    @Bindable var model: DarioModel

    var body: some View {
        VStack(alignment: .leading, spacing: AlexTheme.Spacing.md) {
            SectionLabel(text: "Prompt Cache", style: .prominent)
            if model.promptCaches.isEmpty {
                EmptyStateView(message: "No prompt caches yet")
                    .frame(maxWidth: .infinity)
            } else {
                MiniTable(columns: Self.columns, rows: rows)
            }
        }
        .padding(.horizontal, 16)
        .padding(.bottom, 16)
    }

    private static let columns: [MiniTableColumn] = [
        MiniTableColumn(title: "cache"),
        MiniTableColumn(title: "status", width: 60),
        MiniTableColumn(title: "chars", width: 64),
        MiniTableColumn(title: "version", width: 70),
        MiniTableColumn(title: "last used", width: 80),
        MiniTableColumn(title: "action", width: 60, alignment: .trailing),
    ]

    private var rows: [MiniTableRow] {
        model.promptCaches.map { cache in
            let status = cache.runs?.first?.status ?? "cached"
            return MiniTableRow(
                id: cache.key,
                cells: [
                    .stacked(cache.model ?? cache.key, cache.path ?? "–"),
                    .text(status, tint: DarioStyle.cacheStatusTint(status)),
                    .text(cache.systemPromptChars.map { "\($0)" } ?? "-"),
                    .text(cache.claudeVersion ?? "-"),
                    .text(
                        relative(cache.lastUsedAt ?? cache.capturedAt),
                        tint: AlexTheme.Colors.textTertiary),
                    .button("Clear", isEnabled: !model.actionInFlight, { [weak model] in
                        model?.clearPromptCache(cache)
                    }),
                ],
                height: 48)
        }
    }

    private func relative(_ iso: String?) -> String {
        guard let iso, let date = ISO8601DateFormatter().date(from: iso) else { return "-" }
        return TraceFormat.relative(Int64(date.timeIntervalSince1970 * 1000))
    }
}

// MARK: - Log console

private struct LogSection: View {
    @Bindable var model: DarioModel

    private var logTabSelection: Binding<Int> {
        Binding(
            get: { model.logChannel == .stdout ? 0 : 1 },
            set: { model.logChannel = $0 == 0 ? .stdout : .stderr })
    }

    var body: some View {
        VStack(spacing: AlexTheme.Spacing.md) {
            tabRow
            console
        }
        .padding(.horizontal, 16)
        .padding(.top, AlexTheme.Spacing.md)
        .padding(.bottom, 16)
    }

    private var tabRow: some View {
        HStack(spacing: AlexTheme.Spacing.md) {
            SegmentedTabs(
                tabs: DarioModel.LogChannel.allCases.map(\.rawValue),
                selection: logTabSelection,
                style: .solid)
            if let generation = model.selectedGeneration {
                let health = DarioHealth.evaluate(
                    phase: generation.phase, lastProbeOK: generation.lastProbe?.ok)
                StatusDot(
                    status: health.state.displayStatus, size: 5)
                Text(generation.id)
                    .font(AlexTheme.Fonts.mono(10))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
            Spacer()
        }
        .frame(height: 28)
    }

    private var console: some View {
        ScrollViewReader { proxy in
            ZStack(alignment: .bottom) {
                ScrollView {
                    VStack(alignment: .leading, spacing: 0) {
                        Group {
                            if model.logText.isEmpty {
                                Text("(empty)")
                                    .foregroundStyle(AlexTheme.Colors.textTertiary)
                            } else {
                                Text(DarioStyle.styledLog(model.logText))
                            }
                        }
                        .font(AlexTheme.Fonts.mono(10.5))
                        .lineSpacing(5)
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        Color.clear
                            .frame(height: 1)
                            .id("bottom")
                            .onAppear { model.setUserAtBottom(true) }
                            .onDisappear { model.setUserAtBottom(false) }
                    }
                    .padding(12)
                }
                if !model.userAtBottom, !model.logText.isEmpty {
                    Button {
                        model.setUserAtBottom(true)
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
                    .padding(.bottom, 10)
                }
            }
            .onChange(of: model.logText.count) {
                if model.userAtBottom {
                    proxy.scrollTo("bottom", anchor: .bottom)
                }
            }
            .onChange(of: model.selectedGenerationId) {
                proxy.scrollTo("bottom", anchor: .bottom)
            }
            .onChange(of: model.logChannel) {
                proxy.scrollTo("bottom", anchor: .bottom)
            }
        }
        .background(
            RoundedRectangle(cornerRadius: AlexTheme.Radius.md)
                .fill(AlexTheme.Colors.consoleBackground))
        .overlay(
            RoundedRectangle(cornerRadius: AlexTheme.Radius.md)
                .strokeBorder(AlexTheme.Colors.cardBorder))
        .clipShape(RoundedRectangle(cornerRadius: AlexTheme.Radius.md))
    }
}

@MainActor
final class DarioWindowController: NSObject, NSWindowDelegate {
    private var window: NSWindow?
    private var model: DarioModel?
    private let store: SnapshotStore

    init(store: SnapshotStore) {
        self.store = store
        super.init()
    }

    func show() {
        if window == nil {
            let model = DarioModel(store: store)
            self.model = model
            let host = NSHostingController(rootView: DarioView(model: model))
            let win = NSWindow(contentViewController: host)
            win.title = "Alex UI — Dario"
            win.styleMask = [.titled, .closable, .miniaturizable, .resizable]
            win.isReleasedWhenClosed = false
            win.delegate = self
            win.setContentSize(NSSize(width: 860, height: 540))
            win.center()
            win.setFrameAutosaveName("AlexandriaDario")
            window = win
        }
        BarLog.info(.ui, "dario window opened")
        model?.start()
        if let window {
            DockIconManager.shared.track(window)
            window.makeKeyAndOrderFront(nil)
            NSApp.activate(ignoringOtherApps: true)
        }
    }

    func windowWillClose(_ notification: Notification) {
        BarLog.info(.ui, "dario window closed")
        model?.stop()
    }
}
