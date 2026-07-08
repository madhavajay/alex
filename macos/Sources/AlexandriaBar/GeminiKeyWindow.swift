import AppKit
import SwiftUI
import Observation
import AlexandriaBarCore

@MainActor
@Observable
final class GeminiKeyModel {
    enum Stage: Equatable {
        case editing
        case saving
        case failed(String)
    }

    let store: SnapshotStore
    var key = ""
    private(set) var stage: Stage = .editing
    let onSaved: (@MainActor () -> Void)?

    init(store: SnapshotStore, onSaved: (@MainActor () -> Void)? = nil) {
        self.store = store
        self.onSaved = onSaved
    }

    var canSave: Bool {
        stage != .saving && !key.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    func save(close: @escaping @MainActor @Sendable () -> Void) {
        let value = key.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !value.isEmpty, let config = store.config else { return }
        stage = .saving
        let client = AlexandriaClient(config: config)
        Task { [weak self] in
            do {
                try await client.setGeminiKey(value)
                await self?.store.refresh()
                self?.onSaved?()
                close()
            } catch {
                self?.stage = .failed(error.localizedDescription)
            }
        }
    }
}

struct GeminiKeyView: View {
    @Bindable var model: GeminiKeyModel
    let close: @MainActor @Sendable () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("Set AI Studio API Key")
                .font(.title2.bold())
            Text("Paste a key from aistudio.google.com/apikey")
                .font(.callout)
                .foregroundStyle(.secondary)
            SecureField("API key", text: $model.key)
                .textFieldStyle(.roundedBorder)
                .font(.system(size: 12, design: .monospaced))
                .onSubmit { if model.canSave { model.save(close: close) } }
            if case .failed(let error) = model.stage {
                Label(error, systemImage: "xmark.octagon.fill")
                    .foregroundStyle(.red)
                    .font(.callout)
                    .textSelection(.enabled)
                    .lineLimit(3)
            }
            Spacer(minLength: 0)
            HStack {
                Spacer()
                Button("Cancel") { close() }
                    .keyboardShortcut(.cancelAction)
                Button {
                    model.save(close: close)
                } label: {
                    if model.stage == .saving {
                        ProgressView().controlSize(.small)
                    } else {
                        Text("Save")
                    }
                }
                .keyboardShortcut(.defaultAction)
                .disabled(!model.canSave)
            }
        }
        .padding(20)
        .frame(width: 420, height: 220, alignment: .topLeading)
    }
}

@MainActor
final class GeminiKeyWindowController {
    private var window: NSWindow?
    private var model: GeminiKeyModel?

    func show(store: SnapshotStore, onSaved: (@MainActor () -> Void)? = nil) {
        if let window {
            NSApp.activate(ignoringOtherApps: true)
            window.makeKeyAndOrderFront(nil)
            return
        }
        let model = GeminiKeyModel(store: store, onSaved: onSaved)
        self.model = model
        let view = GeminiKeyView(model: model) { [weak self] in
            self?.closeWindow()
        }
        let host = NSHostingController(rootView: view)
        let win = NSWindow(contentViewController: host)
        win.title = "Set AI Studio API Key"
        win.styleMask = [.titled, .closable]
        win.isReleasedWhenClosed = false
        win.center()
        window = win
        DockIconManager.shared.track(win)
        win.makeKeyAndOrderFront(nil)
    }

    private func closeWindow() {
        window?.close()
        window = nil
        model = nil
    }
}
