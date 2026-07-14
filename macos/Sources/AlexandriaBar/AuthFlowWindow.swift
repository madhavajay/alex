import AppKit
import SwiftUI
import Observation
import AlexandriaBarCore

@MainActor
@Observable
final class AuthFlowModel {
    enum Stage: Equatable {
        case starting
        case awaiting
        case done(String)
        case failed(String)
    }

    let provider: String
    let accountName: String?
    let autoIdentity: Bool
    let store: SnapshotStore
    private(set) var stage: Stage = .starting
    private(set) var session: LoginSession?
    var pasteInput = ""
    var onAuthenticated: (@MainActor (_ provider: String) -> Void)?
    private var pollTask: Task<Void, Never>?

    init(
        provider: String, accountName: String? = "default", autoIdentity: Bool = false,
        store: SnapshotStore
    ) {
        self.provider = provider
        self.accountName = accountName
        self.autoIdentity = autoIdentity
        self.store = store
    }

    var providerName: String { ProviderInfo.displayName(provider) }

    var isAddingAccount: Bool { accountName == nil || autoIdentity }

    var authorizeUrl: String? { session?.authorizeUrl }

    func begin() {
        stage = .starting
        session = nil
        pasteInput = ""
        pollTask?.cancel()
        guard let config = store.config else {
            stage = .failed("Daemon config not found — is alexandria installed?")
            return
        }
        let client = AlexandriaClient(config: config)
        Task { [weak self] in
            do {
                let session = try await client.authLoginStart(
                    provider: ProviderInfo.loginArg(self?.provider ?? ""),
                    name: self?.accountName,
                    autoIdentity: self?.autoIdentity ?? false)
                self?.sessionUpdated(session)
            } catch {
                self?.stage = .failed(error.localizedDescription)
            }
        }
    }

    private func sessionUpdated(_ session: LoginSession) {
        self.session = session
        switch session.state {
        case "done":
            stage = .done(session.accountId ?? "")
            pollTask?.cancel()
            Task { await store.refresh() }
            onAuthenticated?(provider)
        case "failed":
            stage = .failed(session.error ?? "login failed")
            pollTask?.cancel()
        default:
            stage = .awaiting
            if session.mode == "device" || session.mode == "loopback" {
                startPolling(id: session.loginId)
            }
        }
    }

    private func startPolling(id: String) {
        guard pollTask == nil || pollTask?.isCancelled == true else { return }
        pollTask = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(2))
                guard let self, let config = self.store.config else { return }
                if case .awaiting = self.stage {} else { return }
                let client = AlexandriaClient(config: config)
                if let session = try? await client.authLoginStatus(id: id) {
                    self.sessionUpdated(session)
                }
            }
        }
    }

    func submitPaste() {
        guard let session, !pasteInput.trimmingCharacters(in: .whitespaces).isEmpty,
              let config = store.config else { return }
        let client = AlexandriaClient(config: config)
        let input = pasteInput.trimmingCharacters(in: .whitespacesAndNewlines)
        Task { [weak self] in
            do {
                let updated = try await client.authLoginComplete(id: session.loginId, input: input)
                self?.sessionUpdated(updated)
            } catch {
                self?.stage = .failed(error.localizedDescription)
            }
        }
    }

    func openAuthorizeUrl() {
        if let url = authorizeUrl.flatMap(URL.init(string:)) {
            NSWorkspace.shared.open(url)
        }
    }

    func cancel() {
        pollTask?.cancel()
    }
}

struct AuthFlowView: View {
    @Bindable var model: AuthFlowModel
    let close: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text(model.isAddingAccount
                ? "Add \(model.providerName) Account"
                : "Re-authenticate \(model.providerName)")
                .font(.title2.bold())
            content
            Spacer(minLength: 0)
            HStack {
                Spacer()
                Button(model.stage.isTerminal ? "Close" : "Cancel") {
                    model.cancel()
                    close()
                }
                .keyboardShortcut(.cancelAction)
            }
        }
        .padding(20)
        .frame(width: 460, height: 400, alignment: .topLeading)
    }

    @ViewBuilder
    private var content: some View {
        switch model.stage {
        case .starting:
            HStack(spacing: 8) {
                ProgressView().controlSize(.small)
                Text("Starting login session…")
            }
        case .awaiting:
            awaiting
        case .done:
            VStack(alignment: .leading, spacing: 8) {
                Label("Account added", systemImage: "checkmark.circle.fill")
                    .foregroundStyle(.green)
                    .font(.headline)
                Text("The account identity was detected automatically and saved. Alexandria also requested its current Codex usage without sending a model prompt.")
                    .font(.callout)
                    .foregroundStyle(.secondary)
            }
        case .failed(let error):
            VStack(alignment: .leading, spacing: 10) {
                Label("Login failed", systemImage: "xmark.octagon.fill")
                    .foregroundStyle(.red)
                    .font(.headline)
                Text(error)
                    .font(.callout)
                    .textSelection(.enabled)
                    .lineLimit(5)
                Button("Try Again") { model.begin() }
            }
        }
    }

    @ViewBuilder
    private var awaiting: some View {
        let mode = model.session?.mode ?? ""
        VStack(alignment: .leading, spacing: 14) {
            step(1, "Open the authorization page.") {
                HStack(spacing: 8) {
                    Button {
                        model.openAuthorizeUrl()
                    } label: {
                        Label("Open in Browser", systemImage: "arrow.up.forward.square")
                    }
                    if let url = model.authorizeUrl {
                        Button {
                            NSPasteboard.general.clearContents()
                            NSPasteboard.general.setString(url, forType: .string)
                        } label: {
                            Label("Copy Link", systemImage: "doc.on.doc")
                        }
                        .help(url)
                    }
                }
            }
            if mode == "device", let code = model.session?.userCode {
                step(2, "Enter this code when \(model.providerName) asks for it:") {
                    HStack(spacing: 10) {
                        Text(code)
                            .font(.system(size: 26, weight: .semibold, design: .monospaced))
                            .padding(.horizontal, 16)
                            .padding(.vertical, 8)
                            .background(RoundedRectangle(cornerRadius: 8).fill(.quaternary))
                            .textSelection(.enabled)
                        Button {
                            NSPasteboard.general.clearContents()
                            NSPasteboard.general.setString(code, forType: .string)
                        } label: {
                            Image(systemName: "doc.on.doc")
                        }
                        .help("Copy code")
                    }
                }
                waitingBox("Waiting for authorization — keep this window open.")
            } else if mode == "paste" {
                step(2, "Approve access, then paste the code shown (format: code#state):") {
                    HStack(spacing: 8) {
                        TextField("code#state", text: $model.pasteInput)
                            .textFieldStyle(.roundedBorder)
                            .font(.system(size: 12, design: .monospaced))
                            .onSubmit { model.submitPaste() }
                        Button("Continue") { model.submitPaste() }
                            .disabled(model.pasteInput.trimmingCharacters(in: .whitespaces).isEmpty)
                    }
                }
            } else if mode == "loopback" {
                step(2, "Approve access in the browser.") {
                    Text("The browser will redirect to localhost and finish automatically.")
                        .font(.callout)
                        .foregroundStyle(.secondary)
                }
                waitingBox("Waiting for the browser callback — keep this window open.")
            }
        }
    }

    @ViewBuilder
    private func step(_ n: Int, _ title: String, @ViewBuilder body: () -> some View) -> some View {
        HStack(alignment: .top, spacing: 12) {
            Text("\(n)")
                .font(.system(size: 13, weight: .bold))
                .frame(width: 24, height: 24)
                .background(Circle().fill(.quaternary))
            VStack(alignment: .leading, spacing: 8) {
                Text(title).font(.system(size: 13))
                body()
            }
        }
    }

    @ViewBuilder
    private func waitingBox(_ message: String) -> some View {
        HStack(spacing: 8) {
            ProgressView().controlSize(.small)
            Text(message)
                .font(.callout)
                .foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, 14)
        .background(RoundedRectangle(cornerRadius: 8).fill(.quaternary.opacity(0.5)))
    }
}

extension AuthFlowModel.Stage {
    var isTerminal: Bool {
        switch self {
        case .done, .failed: true
        default: false
        }
    }
}

@MainActor
final class AuthWindowController {
    private var windows: [String: NSWindow] = [:]
    private var models: [String: AuthFlowModel] = [:]

    func show(
        provider: String, accountName: String? = "default", autoIdentity: Bool = false,
        store: SnapshotStore,
        onAuthenticated: (@MainActor (_ provider: String) -> Void)? = nil
    ) {
        let isAddingAccount = accountName == nil || autoIdentity
        let key = isAddingAccount
            ? "\(provider):add"
            : "\(provider):reauth:\(accountName ?? "default")"
        if let window = windows[key] {
            NSApp.activate(ignoringOtherApps: true)
            window.makeKeyAndOrderFront(nil)
            return
        }
        let model = AuthFlowModel(
            provider: provider, accountName: accountName, autoIdentity: autoIdentity, store: store)
        model.onAuthenticated = onAuthenticated
        models[key] = model
        let view = AuthFlowView(model: model) { [weak self] in
            self?.closeWindow(key: key)
        }
        let host = NSHostingController(rootView: view)
        let window = NSWindow(contentViewController: host)
        window.title = isAddingAccount
            ? "Add \(ProviderInfo.displayName(provider)) Account"
            : "Re-authenticate \(ProviderInfo.displayName(provider))"
        window.styleMask = [.titled, .closable]
        window.isReleasedWhenClosed = false
        window.center()
        windows[key] = window
        model.begin()
        DockIconManager.shared.track(window)
        window.makeKeyAndOrderFront(nil)
    }

    private func closeWindow(key: String) {
        models[key]?.cancel()
        windows[key]?.close()
        windows[key] = nil
        models[key] = nil
    }
}
