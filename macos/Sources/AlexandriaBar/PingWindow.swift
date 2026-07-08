import AppKit
import SwiftUI
import Observation
import AlexandriaBarCore

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
        task?.cancel()
        lines.append("$ alexandria ping \(target)")
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

    @State private var copied = false

    private func copyAll() {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(model.lines.joined(separator: "\n"), forType: .string)
        copied = true
        Task {
            try? await Task.sleep(for: .seconds(1.5))
            copied = false
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 10) {
                switch model.stage {
                case .running:
                    ProgressView().controlSize(.small)
                    Text("Pinging \(model.title)…").font(.headline)
                case .done(let code):
                    Image(systemName: code == 0 ? "checkmark.circle.fill" : "xmark.octagon.fill")
                        .foregroundStyle(code == 0 ? .green : .red)
                    Text(code == 0 ? "Ping OK" : "Ping failed").font(.headline)
                }
                Spacer()
                Button { copyAll() } label: {
                    Label(copied ? "Copied" : "Copy", systemImage: copied ? "checkmark" : "doc.on.doc")
                }
                .disabled(model.lines.isEmpty)
                .help("Copy all output")
            }
            ScrollViewReader { proxy in
                ScrollView {
                    Text(model.lines.joined(separator: "\n"))
                        .font(.system(size: 11, design: .monospaced))
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(10)
                        .overlay(alignment: .bottom) {
                            Color.clear.frame(height: 1).id("bottom")
                        }
                }
                .background(RoundedRectangle(cornerRadius: 8).fill(.quaternary.opacity(0.5)))
                .onChange(of: model.lines.count) {
                    proxy.scrollTo("bottom", anchor: .bottom)
                }
            }
            HStack {
                Button("Run Again") { model.start() }
                    .disabled(model.isRunning)
                Spacer()
                Button(model.isRunning ? "Cancel" : "Close") {
                    model.cancel()
                    close()
                }
                .keyboardShortcut(.cancelAction)
            }
        }
        .padding(16)
        .frame(width: 520, height: 340)
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
