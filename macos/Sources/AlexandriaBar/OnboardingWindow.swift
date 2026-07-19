import AppKit
import SwiftUI
import Observation
import AlexandriaBarCore

@MainActor
@Observable
final class OnboardingModel {
    enum OperationState: Equatable {
        case idle
        case working(String)
        case success(String)
        case failure(String)
    }

    struct Check: Identifiable {
        let id: String
        let label: String
        let ok: Bool
        let detail: String
    }

    static let completedDefaultsKey = "onboardingCompletedVersion"
    static let currentVersion = "1"
    static let stepTitles = [
        "Meet Alex", "Pick a provider", "Pick a harness", "Your models are ready",
        "Make your first request", "See it in the Trace Browser",
        "Credentials for any app", "Never lose a login", "Keep your agents running",
    ]

    let store: SnapshotStore
    let authenticate: @MainActor (String, @escaping @MainActor (Result<String, Error>) -> Void) -> Void
    let openTraceBrowser: @MainActor (String?) -> Void
    let finish: @MainActor () -> Void

    var step = 0
    var selectedProvider: String?
    var selectedHarness: String?
    var providerState: OperationState = .idle
    var harnessState: OperationState = .idle
    var models: [String] = []
    var modelsLoading = false
    var traceState: OperationState = .idle
    var discoveredTrace: TraceSession?
    var troubleshootExpanded = false
    var checks: [Check] = []
    var checksRunning = false
    private var traceEnteredMs: Int64?
    private var pollTask: Task<Void, Never>?

    init(
        store: SnapshotStore,
        authenticate: @escaping @MainActor (String, @escaping @MainActor (Result<String, Error>) -> Void) -> Void,
        openTraceBrowser: @escaping @MainActor (String?) -> Void,
        finish: @escaping @MainActor () -> Void
    ) {
        self.store = store
        self.authenticate = authenticate
        self.openTraceBrowser = openTraceBrowser
        self.finish = finish
    }

    var exampleModel: String {
        models.first ?? OnboardingSupport.fallbackModels(for: selectedProvider).first!
    }

    var connectableHarnesses: [Harness] {
        HarnessCatalog.rows(store.harnesses).filter { $0.installed && $0.supportsConnect }
    }

    var canAdvance: Bool {
        switch step {
        case 1:
            if case .success = providerState { return true }
            return false
        case 2:
            if case .success = harnessState { return true }
            return false
        case 4:
            if case .success = traceState { return true }
            return false
        default: return true
        }
    }

    func chooseProvider(_ provider: String) {
        selectedProvider = provider
        providerState = .working("Waiting for browser authorization…")
        authenticate(provider) { [weak self] result in
            guard let self, self.selectedProvider == provider else { return }
            switch result {
            case .success:
                Task {
                    await self.store.refresh()
                    let account = self.store.accounts.last { $0.provider == provider }
                    let identity = account?.email ?? account?.label ?? account?.name
                        ?? ProviderInfo.displayName(provider)
                    self.providerState = .success(identity)
                    try? await Task.sleep(for: .milliseconds(650))
                    if self.step == 1 { self.go(to: 2) }
                }
            case .failure(let error):
                self.providerState = .failure(error.localizedDescription)
            }
        }
    }

    func chooseHarness(_ harness: Harness) {
        selectedHarness = harness.name
        harnessState = .working("Connecting \(HarnessCatalog.displayName(harness.name))…")
        guard let config = store.config else {
            harnessState = .failure("The Alex daemon configuration is not available.")
            return
        }
        Task {
            do {
                let response = try await AlexandriaClient(config: config).connectHarness(harness.name)
                harnessState = .success("Connected · \(response.modelsTotal) models ready")
                await store.refreshHarnesses(using: AlexandriaClient(config: config))
            } catch {
                harnessState = .failure(error.localizedDescription)
            }
        }
    }

    func next() {
        if step == Self.stepTitles.count - 1 {
            completeTutorial()
        } else if canAdvance {
            go(to: step + 1)
        }
    }

    func back() {
        guard step > 0 else { return }
        go(to: step - 1)
    }

    func skipStep() {
        if step == 1 {
            selectedProvider = nil
            providerState = .idle
        } else if step == 2 {
            selectedHarness = nil
            harnessState = .idle
        }
        if step < Self.stepTitles.count - 1 { go(to: step + 1) }
        else { completeTutorial() }
    }

    func skipTutorial() { completeTutorial() }

    func completeTutorial() {
        pollTask?.cancel()
        UserDefaults.standard.set(Self.currentVersion, forKey: Self.completedDefaultsKey)
        finish()
    }

    func go(to next: Int) {
        pollTask?.cancel()
        step = min(max(next, 0), Self.stepTitles.count - 1)
        if step == 3 { loadModels() }
        if step == 4 { beginTracePolling() }
    }

    func loadModels() {
        modelsLoading = true
        let fallback = OnboardingSupport.fallbackModels(for: selectedProvider)
        guard let config = store.config else {
            models = fallback
            modelsLoading = false
            return
        }
        Task {
            let fetched = try? await AlexandriaClient(config: config).modelCatalog()
            let filtered = OnboardingSupport.models(fetched ?? [], for: selectedProvider)
            models = filtered.isEmpty ? fallback : filtered
            modelsLoading = false
        }
    }

    func beginTracePolling() {
        discoveredTrace = nil
        traceState = .working("Waiting for a new traced request…")
        traceEnteredMs = Int64(Date().timeIntervalSince1970 * 1_000)
        pollTask?.cancel()
        pollTask = Task { [weak self] in
            while !Task.isCancelled {
                await self?.pollForTrace()
                if case .success = self?.traceState { return }
                try? await Task.sleep(for: .seconds(2))
            }
        }
    }

    private func pollForTrace() async {
        guard let config = store.config, let since = traceEnteredMs else { return }
        guard let sessions = try? await AlexandriaClient(config: config)
            .traceSessions(since: "1h", limit: 100) else { return }
        let harness = selectedHarness?.lowercased()
        if let match = sessions
            .filter({ $0.lastTsMs >= since })
            .filter({ harness == nil || $0.harness?.lowercased() == harness })
            .max(by: { $0.lastTsMs < $1.lastTsMs })
        {
            discoveredTrace = match
            let model = match.models?.first ?? "alex model"
            let tokens = (match.totalInputTokens ?? 0) + (match.totalOutputTokens ?? 0)
            traceState = .success("\(model) · \(tokens) tokens")
        }
    }

    func runTroubleshooting() {
        troubleshootExpanded = true
        checksRunning = true
        checks = []
        guard let config = store.config else {
            checks = [Check(id: "daemon", label: "Alex daemon", ok: false, detail: "Configuration not found")]
            checksRunning = false
            return
        }
        Task {
            let client = AlexandriaClient(config: config)
            do {
                _ = try await client.health()
                checks.append(Check(id: "daemon", label: "Alex daemon /health", ok: true, detail: "Healthy"))
            } catch {
                checks.append(Check(id: "daemon", label: "Alex daemon /health", ok: false, detail: error.localizedDescription))
            }

            await store.refresh()
            if let provider = selectedProvider {
                let present = store.accounts.contains { $0.provider == provider }
                checks.append(Check(
                    id: "account", label: "Provider account", ok: present,
                    detail: present ? "Present" : "No \(ProviderInfo.displayName(provider)) account found"))
                if let target = ProviderInfo.pingArg(provider) {
                    let ping = await DaemonController.ping(target)
                    checks.append(Check(
                        id: "ping", label: "Live provider ping", ok: ping.ok,
                        detail: ping.ok ? "Passed" : "Failed (exit \(ping.exitCode))"))
                }
            } else {
                checks.append(Check(id: "account", label: "Provider account", ok: true, detail: "Skipped — not filtered"))
            }

            if let harness = selectedHarness {
                let harnesses = try? await client.harnesses(refresh: true)
                let connected = harnesses?.contains {
                    $0.name == harness && $0.connected
                } ?? false
                checks.append(Check(
                    id: "harness", label: "Harness connection", ok: connected,
                    detail: connected ? "Connected" : "Not connected"))
            } else {
                checks.append(Check(id: "harness", label: "Harness connection", ok: true, detail: "Skipped — not filtered"))
            }
            checksRunning = false
        }
    }

    func openBrowser() {
        openTraceBrowser(selectedHarness.map { "harness:\($0)" })
    }

    func cancel() { pollTask?.cancel() }
}

struct OnboardingView: View {
    @Bindable var model: OnboardingModel

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider().overlay(AlexTheme.Colors.cardBorder)
            ScrollView {
                stepContent
                    .padding(30)
                    .frame(maxWidth: .infinity, minHeight: 410, alignment: .topLeading)
            }
            Divider().overlay(AlexTheme.Colors.cardBorder)
            footer
        }
        .frame(width: 760, height: 560)
        .background(AlexTheme.Colors.background)
        .focusable()
        .onMoveCommand { direction in
            if direction == .left { model.back() }
            if direction == .right { model.next() }
        }
    }

    private var header: some View {
        HStack {
            VStack(alignment: .leading, spacing: 2) {
                Text("ALEX ONBOARDING")
                    .font(AlexTheme.Fonts.metaMono)
                    .foregroundStyle(AlexTheme.Colors.primary)
                Text(OnboardingModel.stepTitles[model.step])
                    .font(AlexTheme.Fonts.panelTitle)
                    .foregroundStyle(AlexTheme.Colors.foreground)
            }
            Spacer()
            Text("\(model.step + 1) of \(OnboardingModel.stepTitles.count)")
                .font(AlexTheme.Fonts.metaLabel)
                .foregroundStyle(AlexTheme.Colors.textTertiary)
        }
        .padding(.horizontal, 24)
        .frame(height: 62)
    }

    @ViewBuilder private var stepContent: some View {
        switch model.step {
        case 0: meetAlex
        case 1: providerPicker
        case 2: harnessPicker
        case 3: modelsReady
        case 4: firstRequest
        case 5: traceBrowser
        case 6: credentials
        case 7: notifications
        default: failover
        }
    }

    private var meetAlex: some View {
        VStack(alignment: .leading, spacing: 16) {
            if let image = HarnessIconLoader.image(
                resource: "header", extension: "jpg", subdirectory: "onboarding")
            {
                Image(nsImage: image)
                    .resizable().aspectRatio(contentMode: .fill)
                    .frame(maxWidth: .infinity, maxHeight: 220)
                    .clipShape(RoundedRectangle(cornerRadius: AlexTheme.Radius.lg))
            }
            Text("One local daemon that speaks every provider's API and converts between all of them.")
                .font(.system(size: 18, weight: .semibold))
                .foregroundStyle(AlexTheme.Colors.foreground)
            Text("Alex lets you manage and combine your token providers in novel ways — and use them from any harness or application.")
                .font(.system(size: 13))
                .foregroundStyle(AlexTheme.Colors.textSecondary)
        }
    }

    private var providerPicker: some View {
        VStack(alignment: .leading, spacing: 16) {
            intro("Connect a real provider", "Choose a provider. Alex opens its normal secure authentication flow and waits for the account to arrive.")
            LazyVGrid(columns: [GridItem(.adaptive(minimum: 150), spacing: 10)], spacing: 10) {
                ForEach(ProviderInfo.supportedProviders, id: \.self) { provider in
                    choiceButton(
                        title: ProviderInfo.displayName(provider),
                        subtitle: provider == model.selectedProvider ? "Selected" : "Connect",
                        icon: ProviderInfo.loginArg(provider), selected: provider == model.selectedProvider
                    ) { model.chooseProvider(provider) }
                }
            }
            operation(model.providerState)
        }
    }

    private var harnessPicker: some View {
        VStack(alignment: .leading, spacing: 16) {
            intro("Connect a harness", "Alex writes the same managed configuration used by Settings → Harnesses.")
            if model.connectableHarnesses.isEmpty {
                statusCard(icon: "terminal", tint: AlexTheme.Colors.warningOrange,
                           text: "No installed, connectable harnesses were detected. You can skip this step and continue with generic examples.")
            } else {
                LazyVGrid(columns: [GridItem(.adaptive(minimum: 180), spacing: 10)], spacing: 10) {
                    ForEach(model.connectableHarnesses) { harness in
                        choiceButton(
                            title: HarnessCatalog.displayName(harness.name),
                            subtitle: harness.connected ? "Reconnect" : "Connect",
                            icon: harness.name, selected: harness.name == model.selectedHarness
                        ) { model.chooseHarness(harness) }
                    }
                }
            }
            operation(model.harnessState)
        }
    }

    private var modelsReady: some View {
        VStack(alignment: .leading, spacing: 18) {
            intro("Your models are ready", "Connected models appear inside your harness with the alex/ prefix.")
            if model.modelsLoading { ProgressView("Loading Alex's live model catalog…") }
            else {
                VStack(alignment: .leading, spacing: 8) {
                    ForEach(model.models.isEmpty ? OnboardingSupport.fallbackModels(for: model.selectedProvider) : model.models, id: \.self) { name in
                        HStack { Image(systemName: "checkmark.circle.fill").foregroundStyle(AlexTheme.Colors.success); Text(name).font(AlexTheme.Fonts.mono(13)) }
                    }
                }
                .padding(16).cardStyle()
                Text(OnboardingSupport.modelHint(harness: model.selectedHarness, model: model.exampleModel))
                    .font(.system(size: 13)).foregroundStyle(AlexTheme.Colors.textSecondary)
            }
        }
        .onAppear { if model.models.isEmpty { model.loadModels() } }
    }

    private var firstRequest: some View {
        VStack(alignment: .leading, spacing: 16) {
            intro("Make your first request", "Open your harness, choose an alex/* model, and send any prompt. This page will notice the new trace automatically.")
            HStack(spacing: 12) {
                if case .success = model.traceState { Image(systemName: "checkmark.circle.fill").foregroundStyle(AlexTheme.Colors.success).font(.system(size: 28)) }
                else { ProgressView().controlSize(.regular) }
                operationText(model.traceState)
            }.padding(16).cardStyle()
            PillButton(title: "Troubleshoot", variant: .bordered, systemImage: "wrench.and.screwdriver", isBusy: model.checksRunning) { model.runTroubleshooting() }
            if model.troubleshootExpanded { troubleshootPanel }
        }
        .onAppear { if case .idle = model.traceState { model.beginTracePolling() } }
    }

    private var troubleshootPanel: some View {
        VStack(alignment: .leading, spacing: 8) {
            ForEach(model.checks) { check in
                HStack {
                    Image(systemName: check.ok ? "checkmark.circle.fill" : "xmark.circle.fill")
                        .foregroundStyle(check.ok ? AlexTheme.Colors.success : AlexTheme.Colors.destructive)
                    Text(check.label).font(.system(size: 12, weight: .medium))
                    Spacer(); Text(check.detail).font(AlexTheme.Fonts.metaLabel).foregroundStyle(AlexTheme.Colors.textTertiary).lineLimit(1)
                }
            }
            CopyableCode(value: OnboardingSupport.testCommand(harness: model.selectedHarness, model: model.exampleModel))
            Text("Copy this command to run yourself. Alex never executes harness CLIs from the app.")
                .font(AlexTheme.Fonts.metaLabel).foregroundStyle(AlexTheme.Colors.textTertiary)
        }.padding(14).cardStyle()
    }

    private var traceBrowser: some View {
        VStack(alignment: .leading, spacing: 24) {
            intro("See it in the Trace Browser", "Inspect requests, responses, model routing, tokens, costs, and errors in the real Trace Browser.")
            PillButton(title: "Open Trace Browser", variant: .solidAccent, systemImage: "list.bullet.rectangle") { model.openBrowser() }
            if let harness = model.selectedHarness {
                Text("The browser opens pre-filtered with `harness:\(harness)`.").font(.system(size: 12)).foregroundStyle(AlexTheme.Colors.textTertiary)
            } else {
                Text("Because the harness step was skipped, the browser opens without a harness filter.").font(.system(size: 12)).foregroundStyle(AlexTheme.Colors.textTertiary)
            }
        }
    }

    private var credentials: some View {
        VStack(alignment: .leading, spacing: 14) {
            intro("Credentials for any app", "Settings → Credentials can mint scoped, model-only keys for any application that speaks one of Alex's client APIs.")
            VStack(alignment: .leading, spacing: 7) {
                api("Anthropic Messages", "POST /v1/messages")
                api("OpenAI Chat Completions", "POST /v1/chat/completions")
                api("OpenAI Responses", "POST /v1/responses")
                api("Gemini generateContent", "POST /v1beta/models/{model}:generateContent")
            }.padding(14).cardStyle()
            ForEach(OnboardingSupport.environmentSnippets(baseURL: model.store.config?.baseURL), id: \.self) { CopyableCode(value: $0) }
            Text("Scoped keys are revocable and auditable in the Credentials table.")
                .font(.system(size: 12)).foregroundStyle(AlexTheme.Colors.textSecondary)
        }
    }

    private var notifications: some View {
        VStack(alignment: .leading, spacing: 20) {
            Image(systemName: "paperplane.circle.fill").font(.system(size: 54)).foregroundStyle(AlexTheme.Colors.primary)
            intro("Never lose a login", "Connect Telegram in Settings → Notifications. Alex alerts you when a credential needs re-authenticating, and you can reply from your phone to re-auth.")
            statusCard(icon: "text.bubble", tint: AlexTheme.Colors.success, text: "/status shows subscriptions, usage, and ping health wherever you are.")
        }
    }

    private var failover: some View {
        VStack(alignment: .leading, spacing: 20) {
            Image(systemName: "shield.lefthalf.filled.badge.checkmark").font(.system(size: 54)).foregroundStyle(AlexTheme.Colors.success)
            intro("Keep your agents running", "Settings → Failover lets you choose models to switch to automatically, preventing capacity errors, logout errors, and guardrail refusals from killing a long-running agent.")
            statusCard(icon: "arrow.triangle.branch", tint: AlexTheme.Colors.primary, text: "Work moves to the next eligible model or account and keeps going.")
        }
    }

    private var footer: some View {
        HStack(spacing: 12) {
            PillButton(title: "Skip tutorial", variant: .standard, tint: AlexTheme.Colors.textTertiary) { model.skipTutorial() }
            PillButton(title: "Skip for now", variant: .bordered) { model.skipStep() }
            Spacer()
            HStack(spacing: 5) {
                ForEach(OnboardingModel.stepTitles.indices, id: \.self) { index in
                    Circle().fill(index == model.step ? AlexTheme.Colors.primary : AlexTheme.Colors.textFaintest)
                        .frame(width: index == model.step ? 8 : 6, height: index == model.step ? 8 : 6)
                }
            }
            Spacer()
            PillButton(title: "Back", variant: .bordered, isEnabled: model.step > 0) { model.back() }
            PillButton(
                title: model.step == OnboardingModel.stepTitles.count - 1 ? "Get started" : "Next",
                variant: .solidAccent, isEnabled: model.canAdvance,
                keyboardShortcut: .defaultAction
            ) { model.next() }
        }.padding(.horizontal, 20).frame(height: 64)
    }

    private func intro(_ title: String, _ detail: String) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(title).font(.system(size: 22, weight: .semibold)).foregroundStyle(AlexTheme.Colors.foreground)
            Text(detail).font(.system(size: 13)).foregroundStyle(AlexTheme.Colors.textSecondary).fixedSize(horizontal: false, vertical: true)
        }
    }

    private func choiceButton(title: String, subtitle: String, icon: String, selected: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack(spacing: 10) {
                HarnessIconView(harness: icon, tags: nil, size: 28, showsFallback: true)
                VStack(alignment: .leading, spacing: 2) { Text(title).font(.system(size: 13, weight: .semibold)); Text(subtitle).font(AlexTheme.Fonts.metaLabel).foregroundStyle(AlexTheme.Colors.textTertiary) }
                Spacer()
                if selected { Image(systemName: "checkmark.circle.fill").foregroundStyle(AlexTheme.Colors.primary) }
            }.padding(12).background(RoundedRectangle(cornerRadius: AlexTheme.Radius.md).fill(selected ? AlexTheme.Colors.primary.opacity(0.10) : AlexTheme.Colors.card)).overlay(RoundedRectangle(cornerRadius: AlexTheme.Radius.md).strokeBorder(selected ? AlexTheme.Colors.primary.opacity(0.45) : AlexTheme.Colors.cardBorder))
        }.buttonStyle(.plain)
    }

    @ViewBuilder private func operation(_ state: OnboardingModel.OperationState) -> some View {
        if state != .idle {
            HStack(spacing: 9) {
                operationIcon(state)
                operationText(state)
                if state.isFailure {
                    Spacer()
                    PillButton(title: "Skip for now", variant: .solidOrange) {
                        model.skipStep()
                    }
                }
            }
            .padding(12).cardStyle()
        }
    }

    @ViewBuilder private func operationIcon(_ state: OnboardingModel.OperationState) -> some View {
        switch state {
        case .working: ProgressView().controlSize(.small)
        case .success: Image(systemName: "checkmark.circle.fill").foregroundStyle(AlexTheme.Colors.success)
        case .failure: Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(AlexTheme.Colors.destructive)
        case .idle: EmptyView()
        }
    }

    private func operationText(_ state: OnboardingModel.OperationState) -> some View {
        let text: String = switch state { case .idle: ""; case .working(let x), .success(let x), .failure(let x): x }
        return Text(text).font(.system(size: 12)).foregroundStyle(state.isFailure ? AlexTheme.Colors.destructive : AlexTheme.Colors.textSecondary).textSelection(.enabled)
    }

    private func api(_ name: String, _ endpoint: String) -> some View {
        HStack { Text(name).font(.system(size: 12, weight: .medium)); Spacer(); Text(endpoint).font(AlexTheme.Fonts.mono(11)).foregroundStyle(AlexTheme.Colors.primary) }
    }

    private func statusCard(icon: String, tint: Color, text: String) -> some View {
        HStack(spacing: 10) { Image(systemName: icon).foregroundStyle(tint); Text(text).font(.system(size: 12)).foregroundStyle(AlexTheme.Colors.textSecondary) }.padding(14).cardStyle()
    }
}

private extension OnboardingModel.OperationState {
    var isFailure: Bool { if case .failure = self { true } else { false } }
}

private struct CopyableCode: View {
    let value: String
    @State private var copied = false
    var body: some View {
        HStack(spacing: 10) {
            Text(value).font(AlexTheme.Fonts.mono(10.5)).foregroundStyle(AlexTheme.Colors.foreground).textSelection(.enabled)
            Spacer(minLength: 4)
            PillButton(title: copied ? "Copied" : "Copy", variant: .bordered, systemImage: copied ? "checkmark" : "doc.on.doc") {
                NSPasteboard.general.clearContents(); NSPasteboard.general.setString(value, forType: .string); copied = true
            }
        }.padding(10).background(RoundedRectangle(cornerRadius: AlexTheme.Radius.sm).fill(AlexTheme.Colors.consoleBackground)).overlay(RoundedRectangle(cornerRadius: AlexTheme.Radius.sm).strokeBorder(AlexTheme.Colors.cardBorder))
    }
}

private extension View {
    func cardStyle() -> some View {
        background(RoundedRectangle(cornerRadius: AlexTheme.Radius.md).fill(AlexTheme.Colors.card))
            .overlay(RoundedRectangle(cornerRadius: AlexTheme.Radius.md).strokeBorder(AlexTheme.Colors.cardBorder))
    }
}

@MainActor
final class OnboardingWindowController: NSObject, NSWindowDelegate {
    private var window: NSWindow?
    private var model: OnboardingModel?
    private let store: SnapshotStore
    private let authenticate: @MainActor (String, @escaping @MainActor (Result<String, Error>) -> Void) -> Void
    private let openTraceBrowser: @MainActor (String?) -> Void

    init(
        store: SnapshotStore,
        authenticate: @escaping @MainActor (String, @escaping @MainActor (Result<String, Error>) -> Void) -> Void,
        openTraceBrowser: @escaping @MainActor (String?) -> Void
    ) {
        self.store = store
        self.authenticate = authenticate
        self.openTraceBrowser = openTraceBrowser
    }

    func show() {
        if window == nil {
            let model = OnboardingModel(
                store: store, authenticate: authenticate, openTraceBrowser: openTraceBrowser,
                finish: { [weak self] in self?.window?.close() })
            self.model = model
            let win = NSWindow(contentViewController: NSHostingController(rootView: OnboardingView(model: model)))
            win.title = "Welcome to Alex"
            win.styleMask = [.titled, .closable, .miniaturizable]
            win.isReleasedWhenClosed = false
            win.delegate = self
            win.center()
            window = win
        }
        NSApp.activate(ignoringOtherApps: true)
        if let window { DockIconManager.shared.track(window); window.makeKeyAndOrderFront(nil) }
    }

    func windowWillClose(_ notification: Notification) {
        model?.cancel()
        model = nil
        window = nil
    }
}
