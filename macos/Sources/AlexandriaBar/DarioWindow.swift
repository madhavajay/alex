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

struct DarioView: View {
    @Bindable var model: DarioModel

    var body: some View {
        VStack(spacing: 0) {
            if model.daemonDown {
                banner
                Divider()
            }
            if model.disabled {
                Text("dario mode disabled")
                    .font(.system(size: 13))
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                header
                Divider()
                VSplitView {
                    VStack(spacing: 0) {
                        GenerationTable(model: model)
                            .frame(minHeight: 110, idealHeight: 160)
                        Divider()
                        PromptCacheTable(model: model)
                            .frame(minHeight: 92, idealHeight: 120)
                    }
                    LogPane(model: model)
                        .frame(minHeight: 160, maxHeight: .infinity)
                }
            }
        }
        .frame(minWidth: 760, minHeight: 420)
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

    private var header: some View {
        VStack(alignment: .leading, spacing: 3) {
            HStack(spacing: 10) {
                Text(headerTitle)
                    .font(.system(size: 12, weight: .semibold, design: .monospaced))
                    .lineLimit(1)
                    .truncationMode(.middle)
                Spacer()
                if let result = model.actionResult {
                    Text(result)
                        .font(.system(size: 11))
                        .foregroundStyle(result.hasPrefix("failed") ? .red : .secondary)
                        .lineLimit(1)
                }
                Button("Restart") { model.confirmAction(update: false) }
                    .controlSize(.small)
                    .disabled(model.actionInFlight)
                Button("Check Update") { model.confirmAction(update: true) }
                    .controlSize(.small)
                    .disabled(model.actionInFlight)
            }
            Text("Process/generation health + logs. Dario-routed traffic shows up in the Trace Browser under account dario:<generation>.")
                .font(.system(size: 10))
                .foregroundStyle(.tertiary)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
    }

    private var headerTitle: String {
        guard let active = model.activeGeneration else { return "dario — no active generation" }
        return "dario \(active.version) · active \(active.id)"
    }
}

private struct PromptCacheTable: View {
    @Bindable var model: DarioModel

    var body: some View {
        VStack(spacing: 0) {
            headerRow
            Divider()
            ScrollView {
                LazyVStack(spacing: 1) {
                    ForEach(model.promptCaches) { cache in
                        PromptCacheRow(cache: cache, actionInFlight: model.actionInFlight) {
                            model.clearPromptCache(cache)
                        }
                    }
                    if model.promptCaches.isEmpty {
                        Text("No prompt caches yet")
                            .font(.system(size: 11))
                            .foregroundStyle(.secondary)
                            .padding(.top, 14)
                    }
                }
                .padding(4)
            }
        }
    }

    private var headerRow: some View {
        HStack(spacing: 0) {
            GenerationRow.cell("prompt cache", width: nil, alignment: .leading)
            GenerationRow.cell("status", width: 80)
            GenerationRow.cell("chars", width: 72)
            GenerationRow.cell("version", width: 80)
            GenerationRow.cell("last used", width: 92)
            GenerationRow.cell("", width: 54)
        }
        .font(.system(size: 10, weight: .semibold))
        .foregroundStyle(.secondary)
        .padding(.horizontal, 10)
        .padding(.vertical, 4)
    }
}

private struct PromptCacheRow: View {
    let cache: DarioPromptCacheSummary
    let actionInFlight: Bool
    let clear: () -> Void

    var body: some View {
        HStack(spacing: 0) {
            VStack(alignment: .leading, spacing: 1) {
                Text(cache.model ?? cache.key)
                    .font(.system(size: 11, design: .monospaced))
                    .lineLimit(1)
                    .truncationMode(.middle)
                if let path = cache.path {
                    Text(path)
                        .font(.system(size: 9, design: .monospaced))
                        .foregroundStyle(.tertiary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            GenerationRow.cell(cache.runs?.first?.status ?? "cached", width: 80)
            GenerationRow.cell(cache.systemPromptChars.map(String.init) ?? "-", width: 72)
            GenerationRow.cell(cache.claudeVersion ?? "-", width: 80)
            GenerationRow.cell(relative(cache.lastUsedAt ?? cache.capturedAt), width: 92)
            Button("Clear", action: clear)
                .controlSize(.mini)
                .disabled(actionInFlight)
                .frame(width: 54)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 5)
    }

    private func relative(_ iso: String?) -> String {
        guard let iso, let date = ISO8601DateFormatter().date(from: iso) else { return "-" }
        return TraceFormat.relative(Int64(date.timeIntervalSince1970 * 1000))
    }
}

private struct GenerationTable: View {
    @Bindable var model: DarioModel

    var body: some View {
        VStack(spacing: 0) {
            headerRow
            Divider()
            ScrollView {
                LazyVStack(spacing: 1) {
                    ForEach(model.generations) { generation in
                        GenerationRow(
                            generation: generation,
                            isActive: generation.id == model.status?.activeGenerationId,
                            selected: generation.id == model.selectedGenerationId
                        )
                        .contentShape(Rectangle())
                        .onTapGesture { model.selectedGenerationId = generation.id }
                        .contextMenu {
                            Button("Copy Generation ID") { model.copyGenerationId(generation) }
                            Button("Reveal Log in Finder") { model.revealLog(generation) }
                        }
                    }
                    if model.generations.isEmpty {
                        Text("No generations")
                            .font(.system(size: 11))
                            .foregroundStyle(.secondary)
                            .padding(.top, 16)
                    }
                }
                .padding(4)
            }
        }
    }

    private var headerRow: some View {
        HStack(spacing: 0) {
            GenerationRow.cell("generation", width: nil, alignment: .leading)
            GenerationRow.cell("version", width: 66)
            GenerationRow.cell("phase", width: 76)
            GenerationRow.cell("port", width: 52)
            GenerationRow.cell("pid", width: 58)
            GenerationRow.cell("busy", width: 40)
            GenerationRow.cell("probe", width: 110)
            GenerationRow.cell("age", width: 64)
        }
        .font(.system(size: 10, weight: .semibold))
        .foregroundStyle(.secondary)
        .padding(.horizontal, 10)
        .padding(.vertical, 4)
    }
}

private struct GenerationRow: View {
    let generation: DarioGenerationDetail
    let isActive: Bool
    let selected: Bool

    var body: some View {
        HStack(spacing: 0) {
            Text((isActive ? "☥ " : "") + generation.id)
                .font(.system(size: 11, weight: isActive ? .bold : .regular, design: .monospaced))
                .lineLimit(1)
                .truncationMode(.middle)
                .frame(maxWidth: .infinity, alignment: .leading)
            Self.cell(generation.version, width: 66, bold: isActive)
            Text(generation.phase)
                .font(.system(size: 11, weight: isActive ? .bold : .regular, design: .monospaced))
                .foregroundStyle(phaseColor)
                .frame(width: 76)
            Self.cell(generation.port.map(String.init) ?? "–", width: 52)
            Self.cell(generation.pid.map(String.init) ?? "–", width: 58)
            Self.cell(generation.inFlight.map(String.init) ?? "–", width: 40)
            probeCell
                .frame(width: 110)
            Self.cell(age, width: 64)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 5)
        .background(
            RoundedRectangle(cornerRadius: 5)
                .fill(selected ? Color.accentColor.opacity(0.18) : Color.clear)
        )
    }

    static func cell(
        _ text: String, width: CGFloat?, alignment: Alignment = .center, bold: Bool = false
    ) -> some View {
        Text(text)
            .font(.system(size: 11, weight: bold ? .bold : .regular, design: .monospaced))
            .lineLimit(1)
            .frame(width: width)
            .frame(maxWidth: width == nil ? .infinity : width, alignment: alignment)
    }

    private var phaseColor: Color {
        switch generation.phase {
        case "ready": .green
        case "starting": .orange
        case "unhealthy": .red
        case "draining": .yellow
        case "dead": .gray
        default: .secondary
        }
    }

    @ViewBuilder
    private var probeCell: some View {
        if let probe = generation.lastProbe {
            if probe.ok {
                Text("✓ \(probe.latencyMs.map { "\($0)ms" } ?? "ok")")
                    .font(.system(size: 11, design: .monospaced))
                    .foregroundStyle(.green)
            } else {
                Text("✗ \(probe.error ?? "failed")")
                    .font(.system(size: 11, design: .monospaced))
                    .foregroundStyle(.red)
                    .lineLimit(1)
                    .truncationMode(.tail)
                    .help(probe.error ?? "probe failed")
            }
        } else {
            Text("–")
                .font(.system(size: 11, design: .monospaced))
                .foregroundStyle(.secondary)
        }
    }

    private var age: String {
        guard let started = generation.startedAt else { return "–" }
        let delta = Int64(Date().timeIntervalSince1970) - started / 1000
        return Format.duration(max(0, delta))
    }
}

private struct LogPane: View {
    @Bindable var model: DarioModel

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 8) {
                Picker("", selection: $model.logChannel) {
                    ForEach(DarioModel.LogChannel.allCases, id: \.self) { channel in
                        Text(channel.rawValue).tag(channel)
                    }
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                .frame(width: 160)
                if let generation = model.selectedGeneration {
                    Text(generation.id)
                        .font(.system(size: 10, design: .monospaced))
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
                Spacer()
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 6)
            Divider()
            ScrollViewReader { proxy in
                ZStack(alignment: .bottom) {
                    ScrollView {
                        VStack(alignment: .leading, spacing: 0) {
                            Text(model.logText.isEmpty ? "(empty)" : model.logText)
                                .font(.system(size: 11, design: .monospaced))
                                .textSelection(.enabled)
                                .foregroundStyle(model.logText.isEmpty ? .secondary : .primary)
                                .frame(maxWidth: .infinity, alignment: .leading)
                            Color.clear
                                .frame(height: 1)
                                .id("bottom")
                                .onAppear { model.setUserAtBottom(true) }
                                .onDisappear { model.setUserAtBottom(false) }
                        }
                        .padding(10)
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
        }
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
            win.title = "Alexandria — Dario"
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
