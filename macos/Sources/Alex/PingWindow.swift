import AppKit
import SwiftUI
import Observation
import AlexCore

@MainActor
@Observable
final class PingModel {
    enum Stage: Equatable {
        case running
        case done(Int32)
    }

    let target: String
    let title: String
    let store: SnapshotStore
    private(set) var stage: Stage = .running
    private(set) var lines: [String] = []
    private(set) var startedAt = Date()
    private(set) var finishedAt: Date?
    private var task: Task<Void, Never>?

    init(target: String, title: String, store: SnapshotStore) {
        self.target = target
        self.title = title
        self.store = store
    }

    var isRunning: Bool { stage == .running }

    func start() {
        stage = .running
        lines = []
        startedAt = Date()
        finishedAt = nil
        task?.cancel()
        lines.append("$ alex ping \(target)")
        task = Task { [weak self] in
            guard let self else { return }
            let result = await DaemonController.runStreaming(
                args: ["ping", self.target], timeout: 90
            ) { line in
                Task { @MainActor in
                    self.lines.append(line)
                }
            }
            if Task.isCancelled { return }
            self.stage = .done(result.exitCode)
            self.finishedAt = Date()
            let elapsed = Int(Date().timeIntervalSince(self.startedAt))
            self.lines.append(
                result.ok
                    ? "— all checks passed in \(elapsed)s —"
                    : "— finished with exit code \(result.exitCode) after \(elapsed)s —")
            await self.store.refresh()
        }
    }

    func cancel() {
        task?.cancel()
    }
}

struct PingView: View {
    @Bindable var model: PingModel
    let close: () -> Void

    var body: some View {
        VStack(spacing: 0) {
            PanelHeader {
                headerStatusIcon
                Text(headerTitle)
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(AlexTheme.Colors.foreground)
                    .lineLimit(1)
                if !model.lines.isEmpty {
                    PanelCountBadge(count: model.lines.count)
                }
            } right: {
                CopyButton(value: model.lines.joined(separator: "\n"), label: "Copy")
            }
            StatTilesRow(
                items: [
                    StatTileData(label: "Target", value: model.target),
                    StatTileData(
                        label: "Status", value: statusValue, valueTint: statusTint),
                    StatTileData(label: "Duration", value: durationValue),
                ],
                style: .bordered)
            console
            footer
        }
        .background(AlexTheme.Colors.background)
        .frame(width: 560, height: 380)
    }

    // MARK: Header

    @ViewBuilder
    private var headerStatusIcon: some View {
        switch model.stage {
        case .running:
            ProgressView()
                .controlSize(.small)
        case .done(let code):
            StatusDot(status: code == 0 ? .success : .error, size: 7, glow: true)
        }
    }

    private var headerTitle: String {
        switch model.stage {
        case .running: "Pinging \(model.title)…"
        case .done(let code): code == 0 ? "Ping OK — \(model.title)" : "Ping Failed — \(model.title)"
        }
    }

    // MARK: Stats

    private var statusValue: String {
        switch model.stage {
        case .running: "Running"
        case .done(let code): code == 0 ? "OK" : "Exit \(code)"
        }
    }

    private var statusTint: Color {
        switch model.stage {
        case .running: DisplayStatus.running.tint
        case .done(let code): code == 0 ? DisplayStatus.success.tint : DisplayStatus.error.tint
        }
    }

    private var durationValue: String {
        guard let finishedAt = model.finishedAt else { return "…" }
        return "\(Int(finishedAt.timeIntervalSince(model.startedAt)))s"
    }

    // MARK: Console

    private var console: some View {
        ScrollViewReader { proxy in
            ScrollView {
                VStack(alignment: .leading, spacing: 3) {
                    ForEach(Array(model.lines.enumerated()), id: \.offset) { _, line in
                        resultRow(line)
                    }
                    Color.clear.frame(height: 1).id("bottom")
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(12)
            }
            .background(
                RoundedRectangle(cornerRadius: AlexTheme.Radius.md)
                    .fill(AlexTheme.Colors.consoleBackground))
            .overlay(
                RoundedRectangle(cornerRadius: AlexTheme.Radius.md)
                    .strokeBorder(AlexTheme.Colors.cardBorder))
            .padding(.horizontal, 12)
            .padding(.vertical, 12)
            .onChange(of: model.lines.count) {
                proxy.scrollTo("bottom", anchor: .bottom)
            }
        }
    }

    @ViewBuilder
    private func resultRow(_ line: String) -> some View {
        let kind = LineKind(line)
        HStack(alignment: .firstTextBaseline, spacing: AlexTheme.Spacing.sm) {
            StatusDot(tint: kind.dotTint, size: 5)
                .padding(.top, 1)
            Text(line)
                .font(AlexTheme.Fonts.metaMono)
                .foregroundStyle(kind.textColor)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    /// Client-side classification of streamed `alex ping` lines
    /// (✓/✗ marks survive; ANSI is stripped upstream).
    private enum LineKind {
        case command
        case success
        case failure
        case summary
        case plain

        init(_ line: String) {
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            if trimmed.hasPrefix("$") {
                self = .command
            } else if trimmed.hasPrefix("✓") {
                self = .success
            } else if trimmed.hasPrefix("✗") || trimmed.localizedCaseInsensitiveContains("error") {
                self = .failure
            } else if trimmed.hasPrefix("—") {
                self = .summary
            } else {
                self = .plain
            }
        }

        var dotTint: Color {
            switch self {
            case .command: AlexTheme.Colors.textFaintest
            case .success: DisplayStatus.success.tint
            case .failure: DisplayStatus.error.tint
            case .summary: AlexTheme.Colors.textTertiary
            case .plain: AlexTheme.Colors.textFaintest
            }
        }

        var textColor: Color {
            switch self {
            case .command: AlexTheme.Colors.textTertiary
            case .success: AlexTheme.Colors.foreground
            case .failure: AlexTheme.Colors.destructive
            case .summary: AlexTheme.Colors.textTertiary
            case .plain: AlexTheme.Colors.textSecondary
            }
        }
    }

    // MARK: Footer

    private var footer: some View {
        HStack(spacing: AlexTheme.Spacing.md) {
            PillButton(
                title: "Run Again", variant: .bordered,
                isEnabled: !model.isRunning
            ) {
                model.start()
            }
            Spacer()
            PillButton(
                title: model.isRunning ? "Cancel" : "Close", variant: .standard,
                horizontalPadding: 12, verticalPadding: 5, cornerRadius: 6,
                keyboardShortcut: .cancelAction
            ) {
                model.cancel()
                close()
            }
        }
        .padding(.horizontal, 12)
        .padding(.bottom, 12)
    }
}

@MainActor
final class PingWindowController {
    private var window: NSWindow?
    private var model: PingModel?

    func show(target: String, title: String, store: SnapshotStore) {
        model?.cancel()
        let model = PingModel(target: target, title: title, store: store)
        self.model = model
        let view = PingView(model: model) { [weak self] in
            self?.window?.close()
        }
        if let window {
            window.contentViewController = NSHostingController(rootView: view)
            window.title = "Ping — \(title)"
        } else {
            let host = NSHostingController(rootView: view)
            let win = NSWindow(contentViewController: host)
            win.title = "Ping — \(title)"
            win.styleMask = [.titled, .closable]
            win.isReleasedWhenClosed = false
            win.center()
            window = win
        }
        model.start()
        if let window {
            DockIconManager.shared.track(window)
            window.makeKeyAndOrderFront(nil)
        }
    }
}
