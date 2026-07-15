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

// MARK: - Shared auth vocabulary (Subscription Authentication Screen mock)

/// Provider-facing copy for the auth identity header.
enum AuthProviderCopy {
    static func byline(_ provider: String) -> String {
        switch provider {
        case "anthropic": "by Anthropic"
        case "openai": "by OpenAI"
        case "gemini": "by Google"
        case "xai": "by xAI"
        case "amp": "by Sourcegraph"
        case "openrouter": "by OpenRouter"
        default: "OAuth sign-in"
        }
    }

    static func modeBadge(_ mode: String?) -> String? {
        switch mode {
        case "device": "OAuth Device Flow"
        case "paste": "OAuth Code Paste"
        case "loopback": "OAuth Loopback"
        default: nil
        }
    }
}

/// Service identity row: 44×44 brand-tinted logo tile + name/byline + mode
/// pill (Auth App.tsx:56-91).
struct AuthIdentityHeader: View {
    let provider: String
    let title: String
    let byline: String
    var badgeText: String?

    private var accent: Color {
        AlexTheme.ProviderBrand.brand(for: provider).authAccent
    }

    var body: some View {
        HStack(spacing: 14) {
            RoundedRectangle(cornerRadius: 10)
                .fill(accent.opacity(0.15))
                .overlay(RoundedRectangle(cornerRadius: 10).strokeBorder(accent.opacity(0.28)))
                .overlay(
                    HarnessIconView(
                        harness: ProviderInfo.loginArg(provider), tags: nil, size: 26,
                        showsFallback: true))
                .frame(width: 44, height: 44)
            VStack(alignment: .leading, spacing: 3) {
                Text(title)
                    .font(.system(size: 15, weight: .semibold))
                    .kerning(-0.2)
                    .foregroundStyle(AlexTheme.Colors.foreground)
                Text(byline)
                    .font(.system(size: 12))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
            }
            Spacer(minLength: 0)
            if let badgeText {
                HStack(spacing: 5) {
                    StatusDot(tint: accent, size: 5)
                    Text(badgeText)
                        .font(.system(size: 10, weight: .medium))
                        .foregroundStyle(accent)
                }
                .padding(.horizontal, 8)
                .padding(.vertical, 3)
                .background(Capsule().fill(accent.opacity(0.15)))
                .overlay(Capsule().strokeBorder(accent.opacity(0.28)))
            }
        }
    }
}

/// 22×22 numbered step badge in the provider auth accent (Auth App.tsx:184-198).
struct AuthStepBadge: View {
    let n: Int
    var accent: Color

    var body: some View {
        Text("\(n)")
            .font(.system(size: 11, weight: .semibold))
            .foregroundStyle(accent)
            .frame(width: 22, height: 22)
            .background(RoundedRectangle(cornerRadius: 5).fill(accent.opacity(0.15)))
            .overlay(RoundedRectangle(cornerRadius: 5).strokeBorder(accent.opacity(0.28)))
    }
}

/// Provider-accent tinted action button: height 30, radius 6, 12px medium,
/// bg accent@0.12, border accent@0.25, hover brighten, press scale
/// (Auth App.tsx:200-224). Applied to native Buttons so keyboard shortcuts
/// attach directly.
struct AuthTintButtonStyle: ButtonStyle {
    var accent: Color

    func makeBody(configuration: Configuration) -> some View {
        StyledLabel(configuration: configuration, accent: accent)
    }

    private struct StyledLabel: View {
        let configuration: Configuration
        let accent: Color
        @Environment(\.isEnabled) private var isEnabled
        @State private var hovering = false

        var body: some View {
            configuration.label
                .font(.system(size: 12, weight: .medium))
                .foregroundStyle(accent)
                .padding(.horizontal, 10)
                .frame(height: 30)
                .background(RoundedRectangle(cornerRadius: 6).fill(accent.opacity(0.12)))
                .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(accent.opacity(0.25)))
                .brightness(hovering && isEnabled ? 0.06 : 0)
                .opacity(isEnabled ? 1 : 0.4)
                .scaleEffect(configuration.isPressed ? 0.95 : 1)
                .contentShape(Rectangle())
                .onHover { hovering = $0 }
        }
    }
}

/// Footer button (mock "Cancel"): height 28, radius 6, 12px medium, faint
/// fill + hairline border (Auth App.tsx:166-178).
struct AuthFooterButtonStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        StyledLabel(configuration: configuration)
    }

    private struct StyledLabel: View {
        let configuration: Configuration
        @State private var hovering = false

        var body: some View {
            configuration.label
                .font(.system(size: 12, weight: .medium))
                .foregroundStyle(AlexTheme.Colors.foreground)
                .padding(.horizontal, 16)
                .frame(height: 28)
                .background(
                    RoundedRectangle(cornerRadius: 6)
                        .fill(AlexTheme.Colors.overlay(hovering ? 0.10 : 0.06)))
                .overlay(
                    RoundedRectangle(cornerRadius: 6)
                        .strokeBorder(AlexTheme.Colors.overlay(0.1)))
                .scaleEffect(configuration.isPressed ? 0.95 : 1)
                .contentShape(Rectangle())
                .onHover { hovering = $0 }
        }
    }
}

/// Progress strip: spinner + dim message on a faint inset card
/// (Auth App.tsx:153-162).
struct AuthWaitingBox: View {
    let message: String

    init(_ message: String) {
        self.message = message
    }

    var body: some View {
        HStack(spacing: 10) {
            ProgressView()
                .controlSize(.small)
            Text(message)
                .font(.system(size: 12))
                .foregroundStyle(AlexTheme.Colors.mutedForeground)
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 8).fill(AlexTheme.Colors.overlay(0.035)))
        .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(AlexTheme.Colors.overlay(0.06)))
    }
}

/// Numbered step: badge + 13px medium title row, content below
/// (Auth App.tsx:96-151).
struct AuthStep<Content: View>: View {
    let n: Int
    let title: String
    var accent: Color
    @ViewBuilder var content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 10) {
                AuthStepBadge(n: n, accent: accent)
                Text(title)
                    .font(.system(size: 13, weight: .medium))
                    .foregroundStyle(AlexTheme.Colors.foreground)
            }
            content
        }
    }
}

// MARK: - Auth flow view

struct AuthFlowView: View {
    @Bindable var model: AuthFlowModel
    let close: () -> Void
    @State private var copiedLink = false
    @State private var copiedCode = false
    @State private var linkRevert: Task<Void, Never>?
    @State private var codeRevert: Task<Void, Never>?

    private var accent: Color {
        AlexTheme.ProviderBrand.brand(for: model.provider).authAccent
    }

    var body: some View {
        VStack(spacing: 0) {
            VStack(alignment: .leading, spacing: 20) {
                AuthIdentityHeader(
                    provider: model.provider,
                    title: model.providerName,
                    byline: AuthProviderCopy.byline(model.provider),
                    badgeText: AuthProviderCopy.modeBadge(model.session?.mode))
                Rectangle()
                    .fill(AlexTheme.Colors.overlay(0.06))
                    .frame(height: 1)
                content
            }
            .padding(.horizontal, 24)
            .padding(.top, 24)
            .padding(.bottom, 20)
            Spacer(minLength: 0)
            HStack {
                Spacer()
                Button(model.stage.isTerminal ? "Close" : "Cancel") {
                    model.cancel()
                    close()
                }
                .buttonStyle(AuthFooterButtonStyle())
                .keyboardShortcut(.cancelAction)
            }
            .padding(.horizontal, 16)
            .padding(.bottom, 16)
        }
        .frame(width: 480, height: 430, alignment: .top)
        .background(AlexTheme.Colors.background)
    }

    @ViewBuilder
    private var content: some View {
        switch model.stage {
        case .starting:
            AuthWaitingBox("Starting login session…")
        case .awaiting:
            awaiting
        case .done:
            doneCard
        case .failed(let error):
            failedCard(error)
        }
    }

    @ViewBuilder
    private var awaiting: some View {
        let mode = model.session?.mode ?? ""
        VStack(alignment: .leading, spacing: 20) {
            AuthStep(n: 1, title: "Open the authorization page.", accent: accent) {
                HStack(spacing: 8) {
                    Button {
                        model.openAuthorizeUrl()
                    } label: {
                        HStack(spacing: 5) {
                            Image(systemName: "arrow.up.forward")
                                .font(.system(size: 11, weight: .semibold))
                            Text("Open in Browser")
                        }
                    }
                    .buttonStyle(AuthTintButtonStyle(accent: accent))
                    if let url = model.authorizeUrl {
                        Button {
                            copyLink(url)
                        } label: {
                            HStack(spacing: 5) {
                                Image(systemName: copiedLink ? "checkmark" : "doc.on.doc")
                                    .font(.system(
                                        size: 11, weight: copiedLink ? .bold : .regular))
                                Text(copiedLink ? "Copied!" : "Copy Link")
                            }
                        }
                        .buttonStyle(AuthTintButtonStyle(accent: accent))
                        .help(url)
                    }
                }
            }
            if mode == "device", let code = model.session?.userCode {
                AuthStep(
                    n: 2, title: "Enter this code when \(model.providerName) asks for it:",
                    accent: accent
                ) {
                    HStack(spacing: 10) {
                        Text(code)
                            .font(AlexTheme.Fonts.authCode)
                            .kerning(2.6)
                            .foregroundStyle(AlexTheme.Colors.foreground)
                            .textSelection(.enabled)
                            .frame(maxWidth: .infinity)
                            .frame(height: 44)
                            .background(
                                RoundedRectangle(cornerRadius: 8)
                                    .fill(AlexTheme.Colors.overlay(0.04)))
                            .overlay(
                                RoundedRectangle(cornerRadius: 8)
                                    .strokeBorder(AlexTheme.Colors.overlay(0.08)))
                        Button {
                            copyCode(code)
                        } label: {
                            Image(systemName: copiedCode ? "checkmark" : "doc.on.doc")
                                .font(.system(size: 14, weight: copiedCode ? .bold : .regular))
                                .foregroundStyle(
                                    copiedCode ? AlexTheme.Colors.success : accent)
                                .frame(width: 44, height: 44)
                                .background(
                                    RoundedRectangle(cornerRadius: 8)
                                        .fill(
                                            copiedCode
                                                ? AlexTheme.Colors.success.opacity(0.15)
                                                : accent.opacity(0.15)))
                                .overlay(
                                    RoundedRectangle(cornerRadius: 8)
                                        .strokeBorder(
                                            copiedCode
                                                ? AlexTheme.Colors.success.opacity(0.3)
                                                : accent.opacity(0.28)))
                                .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        .help("Copy code")
                    }
                }
                AuthWaitingBox("Waiting for authorization — keep this window open.")
            } else if mode == "paste" {
                AuthStep(
                    n: 2, title: "Approve access, then paste the code shown (format: code#state):",
                    accent: accent
                ) {
                    HStack(spacing: 8) {
                        TextField("code#state", text: $model.pasteInput)
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
                            .onSubmit { model.submitPaste() }
                        Button("Continue") { model.submitPaste() }
                            .buttonStyle(AuthTintButtonStyle(accent: accent))
                            .disabled(
                                model.pasteInput
                                    .trimmingCharacters(in: .whitespaces).isEmpty)
                    }
                }
            } else if mode == "loopback" {
                AuthStep(n: 2, title: "Approve access in the browser.", accent: accent) {
                    Text("The browser will redirect to localhost and finish automatically.")
                        .font(.system(size: 12))
                        .foregroundStyle(AlexTheme.Colors.textTertiary)
                }
                AuthWaitingBox("Waiting for the browser callback — keep this window open.")
            }
        }
    }

    private var doneCard: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 8) {
                StatusDot(status: .success, size: 7, glow: true)
                Text("Account added")
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(AlexTheme.Colors.success)
            }
            Text("The account identity was detected automatically and saved. Alexandria also requested its current Codex usage without sending a model prompt.")
                .font(.system(size: 12))
                .foregroundStyle(AlexTheme.Colors.textSecondary)
        }
        .padding(14)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(
            RoundedRectangle(cornerRadius: 8).fill(AlexTheme.Colors.success.opacity(0.06)))
        .overlay(
            RoundedRectangle(cornerRadius: 8)
                .strokeBorder(AlexTheme.Colors.success.opacity(0.18)))
    }

    private func failedCard(_ error: String) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 8) {
                StatusDot(status: .error, size: 7, glow: true)
                Text("Login failed")
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(AlexTheme.Colors.destructive)
                Spacer(minLength: 0)
                StatusChip(status: .error, text: "failed")
            }
            Text(error)
                .font(.system(size: 12))
                .foregroundStyle(AlexTheme.Colors.textSecondary)
                .textSelection(.enabled)
                .lineLimit(5)
            Button("Try Again") { model.begin() }
                .buttonStyle(AuthTintButtonStyle(accent: accent))
        }
        .padding(14)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(
            RoundedRectangle(cornerRadius: 8)
                .fill(AlexTheme.Colors.destructive.opacity(0.06)))
        .overlay(
            RoundedRectangle(cornerRadius: 8)
                .strokeBorder(AlexTheme.Colors.destructive.opacity(0.18)))
    }

    private func copyLink(_ url: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(url, forType: .string)
        copiedLink = true
        linkRevert?.cancel()
        linkRevert = Task {
            try? await Task.sleep(for: .seconds(1.8))
            guard !Task.isCancelled else { return }
            copiedLink = false
        }
    }

    private func copyCode(_ code: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(code, forType: .string)
        copiedCode = true
        codeRevert?.cancel()
        codeRevert = Task {
            try? await Task.sleep(for: .seconds(1.8))
            guard !Task.isCancelled else { return }
            copiedCode = false
        }
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

#if DEBUG
#Preview("Auth step components") {
    VStack(alignment: .leading, spacing: 20) {
        AuthIdentityHeader(
            provider: "anthropic", title: "Claude", byline: "by Anthropic",
            badgeText: "OAuth Device Flow")
        AuthStep(
            n: 1, title: "Open the authorization page.",
            accent: AlexTheme.ProviderBrand.brand(for: "anthropic").authAccent
        ) {
            HStack(spacing: 8) {
                Button("Open in Browser") {}
                    .buttonStyle(
                        AuthTintButtonStyle(
                            accent: AlexTheme.ProviderBrand.brand(for: "anthropic").authAccent))
                Button("Copy Link") {}
                    .buttonStyle(
                        AuthTintButtonStyle(
                            accent: AlexTheme.ProviderBrand.brand(for: "anthropic").authAccent))
            }
        }
        AuthWaitingBox("Waiting for authorization — keep this window open.")
        Button("Cancel") {}
            .buttonStyle(AuthFooterButtonStyle())
    }
    .padding(24)
    .frame(width: 480)
    .background(AlexTheme.Colors.background)
}
#endif
