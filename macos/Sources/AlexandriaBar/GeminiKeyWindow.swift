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

    private static let keyPageUrl = "https://aistudio.google.com/apikey"

    private var accent: Color {
        AlexTheme.ProviderBrand.brand(for: "gemini").authAccent
    }

    var body: some View {
        VStack(spacing: 0) {
            VStack(alignment: .leading, spacing: 20) {
                AuthIdentityHeader(
                    provider: "gemini", title: "AI Studio", byline: "by Google",
                    badgeText: "API Key")
                Rectangle()
                    .fill(AlexTheme.Colors.overlay(0.06))
                    .frame(height: 1)
                AuthStep(n: 1, title: "Create an API key in Google AI Studio.", accent: accent) {
                    Button {
                        if let url = URL(string: Self.keyPageUrl) {
                            NSWorkspace.shared.open(url)
                        }
                    } label: {
                        HStack(spacing: 5) {
                            Image(systemName: "arrow.up.forward")
                                .font(.system(size: 11, weight: .semibold))
                            Text("Open aistudio.google.com")
                        }
                    }
                    .buttonStyle(AuthTintButtonStyle(accent: accent))
                    .help(Self.keyPageUrl)
                }
                AuthStep(n: 2, title: "Paste the key here:", accent: accent) {
                    SecureField("API key", text: $model.key)
                        .textFieldStyle(.plain)
                        .font(AlexTheme.Fonts.mono(12))
                        .foregroundStyle(AlexTheme.Colors.foreground)
                        .padding(.horizontal, 12)
                        .frame(height: 30)
                        .background(
                            RoundedRectangle(cornerRadius: 8)
                                .fill(AlexTheme.Colors.overlay(0.04)))
                        .overlay(
                            RoundedRectangle(cornerRadius: 8)
                                .strokeBorder(AlexTheme.Colors.overlay(0.08)))
                        .onSubmit { if model.canSave { model.save(close: close) } }
                }
                if case .failed(let error) = model.stage {
                    HStack(alignment: .firstTextBaseline, spacing: 8) {
                        StatusDot(status: .error, size: 7, glow: true)
                        Text(error)
                            .font(.system(size: 12))
                            .foregroundStyle(AlexTheme.Colors.destructive)
                            .textSelection(.enabled)
                            .lineLimit(3)
                    }
                }
            }
            .padding(.horizontal, 24)
            .padding(.top, 24)
            .padding(.bottom, 20)
            Spacer(minLength: 0)
            HStack(spacing: 8) {
                Spacer()
                Button("Cancel") { close() }
                    .buttonStyle(AuthFooterButtonStyle())
                    .keyboardShortcut(.cancelAction)
                Button {
                    model.save(close: close)
                } label: {
                    if model.stage == .saving {
                        ProgressView()
                            .controlSize(.small)
                            .frame(width: 34)
                    } else {
                        Text("Save")
                    }
                }
                .buttonStyle(AuthTintButtonStyle(accent: accent))
                .keyboardShortcut(.defaultAction)
                .disabled(!model.canSave)
            }
            .padding(.horizontal, 16)
            .padding(.bottom, 16)
        }
        .frame(width: 480, height: 380, alignment: .top)
        .background(AlexTheme.Colors.background)
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
