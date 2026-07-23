import AppKit
import SwiftUI
import Observation
import AlexCore

private enum OnboardingSetupError: LocalizedError {
    case message(String)

    var errorDescription: String? {
        switch self { case .message(let message): message }
    }
}

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

    static let completedDefaultsKey = OnboardingLaunchPolicy.completedDefaultsKey
    static let currentVersion = OnboardingLaunchPolicy.currentVersion
    static let stepTitles = [
        "Meet Alex", "Pick a provider", "Network access", "Connect and test",
        "Credentials for compatible apps", "Never lose a login", "Keep your agents running",
        "Beyond single provider",
    ]
    // Named indices for the steps other logic branches on.
    static let providerStep = 1
    static let networkStep = 2
    static let connectStep = 3

    let store: SnapshotStore
    let openProviderSettings: @MainActor () -> Void
    let openTraceBrowser: @MainActor (String?) -> Void
    let finish: @MainActor () -> Void

    var step = 0
    var selectedProvider: String?
    var selectedProviderAccountID: String?
    var addingProviderAccount = false
    var openRouterAccountName = ""
    var openRouterAPIKey = ""
    var revealOpenRouterAPIKey = false
    var cliProxyAPIEndpoint = "http://127.0.0.1:8317/v1"
    var cliProxyAPICredential = ""
    var revealCLIProxyAPICredential = false
    var exoEndpoint = "http://localhost:52415"
    var selectedHarness: String?
    var authModel: AuthFlowModel?
    var credentialImportCandidates: [CredentialImportCandidate] = []
    var selectedCredentialImports: Set<String> = []
    var credentialImportCandidatesLoaded = false
    var credentialImportCandidatesLoading = false
    var providerState: OperationState = .idle
    var harnessPlanState: OperationState = .idle
    var harnessPlan: [HarnessPlanStep] = []
    var harnessState: OperationState = .idle
    var connectedModelsCount = 0
    var exampleModel = OnboardingSupport.defaultExampleModel
    var exampleModelLoading = false
    var traceState: OperationState = .idle
    var discoveredTrace: TraceSession?
    var networkChoice = "loopback"
    var networkInterfaces: [NetworkInterfaceAddress] = []
    var selectedInterfaceAddress = ""
    var networkSaveState: OperationState = .idle
    var networkChoiceLoaded = false
    var mintedOnboardingKey: MintedRunKey?
    var onboardingKeyFingerprint: String?
    var credentialMintState: OperationState = .idle
    var credentialRunState: OperationState = .idle
    var credentialResponseText: String?
    var troubleshootExpanded = false
    var checks: [Check] = []
    var checksRunning = false
    var traceCheckRunning = false
    private var traceEnteredMs: Int64?
    private var pollTask: Task<Void, Never>?
    private var lastRejectedSessionId: String?

    init(
        store: SnapshotStore,
        openProviderSettings: @escaping @MainActor () -> Void,
        openTraceBrowser: @escaping @MainActor (String?) -> Void,
        finish: @escaping @MainActor () -> Void
    ) {
        self.store = store
        self.openProviderSettings = openProviderSettings
        self.openTraceBrowser = openTraceBrowser
        self.finish = finish
    }

    var connectableHarnesses: [Harness] {
        HarnessCatalog.rows(store.harnesses).filter { $0.installed && $0.supportsConnect }
    }

    var credentialsCurl: String? {
        guard let key = mintedOnboardingKey else { return nil }
        return OnboardingSupport.credentialsCurlExample(
            baseURL: store.config?.baseURL, key: key.key, model: exampleModel)
    }

    var canAdvance: Bool {
        switch step {
        case Self.providerStep:
            if case .success = providerState { return true }
            return false
        case Self.connectStep:
            return false
        default: return true
        }
    }

    func chooseProvider(_ provider: String) {
        authModel?.cancel()
        resetProviderDependentState()
        selectedProvider = provider
        exampleModel = OnboardingSupport.exampleModel(for: provider)
        providerState = .idle
        if !accounts(for: provider).isEmpty {
            // Resuming onboarding must not silently pick an arbitrary account.
            // Let the user choose an existing subscription or deliberately add
            // another one.
            return
        }
        if !credentialImportCandidatesLoaded || !importCandidates(for: provider).isEmpty {
            // Discovery is read-only. Keep the choice visible until the user
            // explicitly confirms an import or chooses a fresh browser flow.
            return
        }
        addProviderAccount()
    }

    func loadCredentialImportCandidates() async {
        guard !credentialImportCandidatesLoaded, !credentialImportCandidatesLoading else { return }
        guard let config = store.config ?? DaemonDiscovery.load() else { return }
        credentialImportCandidatesLoading = true
        defer { credentialImportCandidatesLoading = false }
        do {
            let response = try await AlexClient(config: config).credentialImportCandidates()
            credentialImportCandidates = response.candidates
            selectedCredentialImports = Set(response.candidates.map(\.source))
            credentialImportCandidatesLoaded = true
            if let provider = selectedProvider,
               accounts(for: provider).isEmpty,
               importCandidates(for: provider).isEmpty,
               authModel == nil,
               !addingProviderAccount
            {
                addProviderAccount()
            }
        } catch {
            // Discovery is a convenience and must never block a normal login.
            credentialImportCandidatesLoaded = true
            if let provider = selectedProvider,
               accounts(for: provider).isEmpty,
               authModel == nil,
               !addingProviderAccount
            {
                addProviderAccount()
            }
        }
    }

    func importCandidates(for provider: String) -> [CredentialImportCandidate] {
        credentialImportCandidates.filter { $0.provider == provider }
    }

    func credentialImportBinding(_ source: String) -> Binding<Bool> {
        Binding {
            self.selectedCredentialImports.contains(source)
        } set: { selected in
            if selected {
                self.selectedCredentialImports.insert(source)
            } else {
                self.selectedCredentialImports.remove(source)
            }
        }
    }

    func importDetectedCredentials(for provider: String) {
        let selected = importCandidates(for: provider).filter {
            selectedCredentialImports.contains($0.source)
        }
        guard !selected.isEmpty else {
            providerState = .failure("Select a detected credential to import, or connect a new account.")
            return
        }
        guard let config = store.config ?? DaemonDiscovery.load() else {
            providerState = .failure("The Alex daemon configuration is not available.")
            return
        }
        providerState = .working("Importing the selected credential…")
        Task {
            do {
                var importedIDs: [String] = []
                var notes: [String] = []
                for candidate in selected {
                    let outcomes = try await AlexClient(config: config)
                        .authImport(source: candidate.source)
                    importedIDs.append(contentsOf: outcomes.flatMap(\.imported))
                    notes.append(contentsOf: outcomes.compactMap(\.note))
                }
                await refreshStore()
                guard selectedProvider == provider else { return }
                guard let account = importedIDs.compactMap({ id in
                    store.accounts.first { $0.id == id }
                }).first ?? store.accounts.last(where: { $0.provider == provider }) else {
                    throw OnboardingSetupError.message(
                        notes.first ?? "The detected credential could not be imported.")
                }
                selectedProviderAccountID = account.id
                await completeProviderSelection(provider, account: account)
            } catch {
                guard selectedProvider == provider else { return }
                providerState = .failure(error.localizedDescription)
            }
        }
    }

    func clearProviderSelection() {
        authModel?.cancel()
        authModel = nil
        resetProviderDependentState()
        selectedProvider = nil
        providerState = .idle
        exampleModel = OnboardingSupport.defaultExampleModel
    }

    func accounts(for provider: String) -> [Account] {
        store.accounts.filter { $0.provider == provider }.sorted {
            accountDisplayName($0).localizedCaseInsensitiveCompare(accountDisplayName($1))
                == .orderedAscending
        }
    }

    func accountDisplayName(_ account: Account) -> String {
        account.email ?? account.label ?? account.name
    }

    func accountDisplayDetail(_ account: Account) -> String {
        if accountDisplayName(account) != account.name { return account.name }
        return account.kind == "oauth" ? "Connected subscription" : account.kind
    }

    func useExistingProviderAccount(_ account: Account) {
        guard selectedProvider == account.provider else { return }
        authModel?.cancel()
        authModel = nil
        addingProviderAccount = false
        selectedProviderAccountID = account.id
        providerState = .working(
            account.provider == "anthropic"
                ? "Preparing Claude routing…" : "Using connected account…")
        Task { await completeProviderSelection(account.provider, account: account) }
    }

    func addProviderAccount() {
        guard let provider = selectedProvider else { return }
        authModel?.cancel()
        authModel = nil
        selectedProviderAccountID = nil
        providerState = .idle
        if provider == "openrouter" || provider == "cliproxyapi" || provider == "exo" {
            addingProviderAccount = true
            return
        }
        beginProviderAuthorization(provider: provider, accountName: nil)
    }

    private func beginProviderAuthorization(provider: String, accountName: String?) {
        providerState = .working("Starting secure authorization…")
        let auth = AuthFlowModel(
            provider: provider, accountName: accountName,
            autoIdentity: accountName == nil, store: store)
        auth.onAuthenticated = { [weak self] authenticatedProvider in
            guard let self, self.selectedProvider == authenticatedProvider else { return }
            Task {
                await self.refreshStore()
                let authenticatedID = self.authModel?.session?.accountId
                let account = authenticatedID.flatMap { id in
                    self.store.accounts.first { $0.id == id }
                } ?? accountName.flatMap { name in
                    self.store.accounts.first {
                        $0.provider == authenticatedProvider && $0.name == name
                    }
                } ?? self.store.accounts.last { $0.provider == authenticatedProvider }
                self.selectedProviderAccountID = account?.id
                await self.completeProviderSelection(authenticatedProvider, account: account)
            }
        }
        auth.onFailed = { [weak self] message in self?.providerState = .failure(message) }
        authModel = auth
        auth.begin()
    }

    func connectOpenRouter() {
        let name = openRouterAccountName.trimmingCharacters(in: .whitespacesAndNewlines)
        let key = openRouterAPIKey.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty else {
            providerState = .failure("Enter a name for this OpenRouter key.")
            return
        }
        guard !key.isEmpty else {
            providerState = .failure("Enter an OpenRouter API key.")
            return
        }
        guard let config = store.config ?? DaemonDiscovery.load() else {
            providerState = .failure("The Alex daemon configuration is not available.")
            return
        }
        providerState = .working("Saving the OpenRouter key…")
        Task {
            do {
                let id = try await AlexClient(config: config).setOpenRouterKey(
                    key, displayName: name)
                openRouterAPIKey = ""
                revealOpenRouterAPIKey = false
                await refreshStore()
                guard selectedProvider == "openrouter" else { return }
                selectedProviderAccountID = id
                addingProviderAccount = false
                let account = store.accounts.first { $0.id == id }
                providerState = .success(accountIdentity(account, provider: "openrouter"))
            } catch {
                guard selectedProvider == "openrouter" else { return }
                providerState = .failure(error.localizedDescription)
            }
        }
    }

    func connectExo() {
        let endpoint = exoEndpoint.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let url = URL(string: endpoint), ["http", "https"].contains(url.scheme?.lowercased()) else {
            providerState = .failure("Enter a valid http:// or https:// Exo endpoint.")
            return
        }
        guard let config = store.config ?? DaemonDiscovery.load() else {
            providerState = .failure("The Alex daemon configuration is not available.")
            return
        }
        providerState = .working("Checking the Exo endpoint…")
        Task {
            do {
                let client = AlexClient(config: config)
                let current = (try? await client.exoConfig()) ?? ExoConfig()
                _ = try await client.updateExoConfig(ExoConfig(
                    url: endpoint, enabledModels: current.enabledModels))
                let status = try await client.exoStatus()
                guard status.running else {
                    throw OnboardingSetupError.message(
                        status.error ?? "Exo did not respond at \(endpoint).")
                }
                let models = try await client.exoModels()
                if current.enabledModels.isEmpty, !models.isEmpty {
                    _ = try await client.updateExoConfig(ExoConfig(
                        url: endpoint, enabledModels: models.map(\.id)))
                }
                await refreshStore()
                guard selectedProvider == "exo" else { return }
                addingProviderAccount = false
                providerState = .success(
                    "Exo online — \(models.count) model\(models.count == 1 ? "" : "s") found")
            } catch {
                guard selectedProvider == "exo" else { return }
                providerState = .failure(error.localizedDescription)
            }
        }
    }

    func connectCLIProxyAPI() {
        let endpoint = cliProxyAPIEndpoint.trimmingCharacters(in: .whitespacesAndNewlines)
        let credential = cliProxyAPICredential.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let url = URL(string: endpoint), ["http", "https"].contains(url.scheme?.lowercased()) else {
            providerState = .failure("Enter a valid http:// or https:// CLIProxyAPI endpoint.")
            return
        }
        guard !credential.isEmpty else {
            providerState = .failure("Enter the CLIProxyAPI credential.")
            return
        }
        guard let config = store.config ?? DaemonDiscovery.load() else {
            providerState = .failure("The Alex daemon configuration is not available.")
            return
        }
        providerState = .working("Probing CLIProxyAPI capabilities…")
        Task {
            do {
                let result = try await AlexClient(config: config).connectCLIProxyAPI(
                    url: endpoint, credential: credential)
                cliProxyAPICredential = ""
                revealCLIProxyAPICredential = false
                await refreshStore()
                guard selectedProvider == "cliproxyapi" else { return }
                selectedProviderAccountID = result.saved
                exampleModel = OnboardingSupport.exampleModel(
                    for: "cliproxyapi", cliProxyAPIModels: result.models)
                addingProviderAccount = false
                providerState = .success(
                    "CLIProxyAPI ready — \(result.models.count) model\(result.models.count == 1 ? "" : "s") found")
            } catch {
                guard selectedProvider == "cliproxyapi" else { return }
                providerState = .failure(error.localizedDescription)
            }
        }
    }

    private func resetProviderDependentState() {
        pollTask?.cancel()
        selectedProviderAccountID = nil
        addingProviderAccount = false
        openRouterAccountName = ""
        openRouterAPIKey = ""
        revealOpenRouterAPIKey = false
        cliProxyAPICredential = ""
        revealCLIProxyAPICredential = false
        traceState = .idle
        traceCheckRunning = false
        discoveredTrace = nil
        traceEnteredMs = nil
        lastRejectedSessionId = nil
        troubleshootExpanded = false
        checks = []
        mintedOnboardingKey = nil
        onboardingKeyFingerprint = nil
        credentialMintState = .idle
        credentialRunState = .idle
        credentialResponseText = nil
    }

    private func accountIdentity(_ account: Account?, provider: String) -> String {
        account?.email ?? account?.label ?? account?.name
            ?? ProviderInfo.displayName(provider)
    }

    private func refreshStore() async {
        await store.refresh()
        while store.refreshing {
            try? await Task.sleep(for: .milliseconds(50))
        }
    }

    /// Auto-Dario routing is chosen when the daemon starts. If Claude was the
    /// first account added after a fresh/reset launch, restart once so the
    /// daemon sees that subscription before onboarding asks Pi to test it.
    private func completeProviderSelection(_ provider: String, account: Account?) async {
        guard selectedProvider == provider else { return }
        if provider == "cliproxyapi", let config = store.config ?? DaemonDiscovery.load() {
            do {
                let status = try await AlexClient(config: config).cliProxyAPIStatus()
                guard status.connected, !status.models.isEmpty else {
                    providerState = .failure("CLIProxyAPI is saved but did not return a usable model catalogue.")
                    return
                }
                exampleModel = OnboardingSupport.exampleModel(
                    for: provider, cliProxyAPIModels: status.models)
            } catch {
                providerState = .failure(error.localizedDescription)
                return
            }
        }
        if provider == "anthropic", store.dario?.routeEnabled != true {
            providerState = .working("Starting Claude subscription routing through Dario…")
            let result = await DaemonController.restartDaemon()
            DaemonDiscovery.invalidateCache()
            await refreshStore()
            guard selectedProvider == provider else { return }
            guard result.ok, store.dario?.routeEnabled == true else {
                let detail = result.combined.trimmingCharacters(in: .whitespacesAndNewlines)
                providerState = .failure(
                    detail.isEmpty
                        ? "Claude connected, but Dario routing did not start."
                        : "Claude connected, but Dario routing did not start: \(detail)")
                return
            }
        }
        let refreshedAccount = account.flatMap { selected in
            store.accounts.first { $0.id == selected.id }
        } ?? account ?? store.accounts.last { $0.provider == provider }
        selectedProviderAccountID = refreshedAccount?.id
        providerState = .success(accountIdentity(refreshedAccount, provider: provider))
    }

    func selectHarness(_ harness: Harness) {
        pollTask?.cancel()
        selectedHarness = harness.name
        harnessPlan = []
        harnessState = .idle
        connectedModelsCount = 0
        traceState = .idle
        traceCheckRunning = false
        discoveredTrace = nil
        traceEnteredMs = nil
        lastRejectedSessionId = nil
        troubleshootExpanded = false
        checks = []
        harnessPlanState = .working("Previewing changes…")
        guard let config = store.config else {
            harnessPlanState = .failure("The Alex daemon configuration is not available.")
            return
        }
        Task {
            do {
                let response = try await AlexClient(config: config).connectHarnessPlan(harness.name)
                guard selectedHarness == harness.name else { return }
                harnessPlan = response.plan
                harnessPlanState = .success(OnboardingSupport.harnessInstallDescription(harness.name))
            } catch {
                guard selectedHarness == harness.name else { return }
                harnessPlanState = .failure(error.localizedDescription)
            }
        }
    }

    func changeHarness() {
        pollTask?.cancel()
        selectedHarness = nil
        harnessPlan = []
        harnessPlanState = .idle
        harnessState = .idle
        connectedModelsCount = 0
        exampleModelLoading = false
        exampleModel = OnboardingSupport.exampleModel(for: selectedProvider)
        traceState = .idle
        traceCheckRunning = false
        discoveredTrace = nil
        traceEnteredMs = nil
        lastRejectedSessionId = nil
        troubleshootExpanded = false
        checks = []
        checksRunning = false
    }

    func connectSelectedHarness() {
        guard let harness = selectedHarness, let config = store.config else {
            harnessState = .failure("Choose a harness and load its connection plan first.")
            return
        }
        harnessState = .working("Connecting \(HarnessCatalog.displayName(harness))…")
        Task {
            do {
                let client = AlexClient(config: config)
                let response = try await client.connectHarness(harness)
                guard selectedHarness == harness else { return }
                connectedModelsCount = response.modelsTotal
                harnessState = .success("\(response.modelsTotal) models ready ✓")
                await store.refreshHarnesses(using: client)
                await loadExampleModel(using: client)
                beginTracePolling()
            } catch {
                guard selectedHarness == harness else { return }
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
        // Returning to the provider page is an explicit opportunity to choose
        // again. Do not preserve an expanded OAuth flow (Gemini loopback in
        // particular can otherwise leave the provider grid above the retained
        // scroll position), and do the same when leaving the picker backward.
        if step == Self.providerStep || step == Self.connectStep {
            clearProviderSelection()
        }
        go(to: step - 1)
    }

    func skipStep() {
        if step == Self.providerStep {
            authModel?.cancel()
            authModel = nil
            selectedProvider = nil
            providerState = .idle
        } else if step == Self.connectStep {
            selectedHarness = nil
            harnessPlanState = .idle
            harnessPlan = []
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
        if step == Self.connectStep, harnessState.isSuccess, !traceState.isSuccess {
            beginTracePolling()
        }
    }

    private func loadExampleModel(using client: AlexClient) async {
        exampleModelLoading = true
        defer { exampleModelLoading = false }
        let openRouterExposed = selectedProvider == "openrouter"
            ? (try? await client.openRouterExposed().exposed) ?? [] : []
        let exoModels = selectedProvider == "exo"
            ? (try? await client.exoModels()) ?? [] : []
        exampleModel = OnboardingSupport.exampleModel(
            for: selectedProvider,
            openRouterExposed: openRouterExposed,
            exoModels: exoModels)
    }

    func mintCredentialsDemoKey() {
        guard !credentialMintState.isWorking else { return }
        guard let config = store.config else {
            credentialMintState = .failure("The Alex daemon configuration is not available.")
            return
        }
        credentialMintState = .working("Minting a one-hour model-only key…")
        credentialRunState = .idle
        credentialResponseText = nil
        Task {
            do {
                let client = AlexClient(config: config)
                await loadExampleModel(using: client)
                let minted = try await client.mintRunKey(
                    label: "onboarding", model: exampleModel, ttlSeconds: 3_600)
                var fingerprint = minted.keyFingerprint
                if fingerprint == nil {
                    let inventory = try? await client.credentials()
                    fingerprint = inventory?.inbound.runKeys
                        .first(where: { $0.id == minted.id })?.keyFingerprint
                }
                mintedOnboardingKey = minted
                onboardingKeyFingerprint = fingerprint
                    ?? OnboardingSupport.runKeyFingerprint(minted.key)
                credentialMintState = .success("One-hour onboarding key ready.")
            } catch {
                credentialMintState = .failure(error.localizedDescription)
            }
        }
    }

    func runCredentialsDemo() {
        guard !credentialRunState.isWorking else { return }
        guard let config = store.config, let key = mintedOnboardingKey else {
            credentialRunState = .failure("Mint the onboarding key first.")
            return
        }
        credentialRunState = .working("Sending the request through Alex…")
        credentialResponseText = nil
        Task {
            do {
                var request = URLRequest(
                    url: OnboardingSupport.credentialsDemoURL(baseURL: config.baseURL))
                request.httpMethod = "POST"
                request.timeoutInterval = 60
                for header in OnboardingSupport.credentialsDemoHeaders(key: key.key) {
                    request.setValue(header.value, forHTTPHeaderField: header.name)
                }
                request.httpBody = Data(
                    OnboardingSupport.credentialsDemoBody(model: exampleModel).utf8)
                let (data, response) = try await URLSession.shared.data(for: request)
                let text = String(data: data, encoding: .utf8) ?? "<non-UTF-8 response>"
                credentialResponseText = text
                let status = (response as? HTTPURLResponse)?.statusCode ?? 0
                if (200..<300).contains(status) {
                    credentialRunState = .success("Request completed · HTTP \(status)")
                } else {
                    credentialRunState = .failure("Request failed · HTTP \(status)")
                }
            } catch {
                credentialResponseText = error.localizedDescription
                credentialRunState = .failure(error.localizedDescription)
            }
        }
    }

    func openCredentialsTrace() {
        guard let onboardingKeyFingerprint else { return }
        openTraceBrowser("key:\(onboardingKeyFingerprint)")
    }

    func beginTracePolling() {
        discoveredTrace = nil
        lastRejectedSessionId = nil
        traceState = .working("Waiting for a new traced request…")
        traceEnteredMs = Int64(Date().timeIntervalSince1970 * 1_000)
        pollTask?.cancel()
        pollTask = Task { [weak self] in
            while !Task.isCancelled {
                let found = await self?.pollForTrace() ?? false
                if found, self?.traceState.isTerminal == true { return }
                try? await Task.sleep(for: .seconds(2))
            }
        }
    }

    func checkForTrace() {
        guard harnessState.isSuccess, !traceCheckRunning else { return }
        if traceEnteredMs == nil {
            traceEnteredMs = Int64(Date().timeIntervalSince1970 * 1_000)
        }
        traceCheckRunning = true
        traceState = .working("Checking for a new matching request…")
        Task {
            let found = await pollForTrace()
            if !found {
                traceState = .working(
                    "No new matching request yet — run the command, then check again.")
            }
            traceCheckRunning = false
        }
    }

    @discardableResult
    private func pollForTrace() async -> Bool {
        guard let config = store.config, let since = traceEnteredMs else { return false }
        guard let sessions = try? await AlexClient(config: config)
            .traceSessions(since: "1h", limit: 100) else { return false }
        let harness = selectedHarness?.lowercased()
        if let match = sessions
            .filter({ $0.lastTsMs >= since })
            .filter({ OnboardingSupport.traceMatchesHarness($0, harness: harness) })
            .max(by: { $0.lastTsMs < $1.lastTsMs })
        {
            let initial = OnboardingSupport.traceOutcome(
                status: match.lastStatus, errorCount: match.errors, error: nil)
            switch initial {
            case .clean:
                discoveredTrace = match
                let model = match.models?.first ?? "alex model"
                let tokens = (match.totalInputTokens ?? 0) + (match.totalOutputTokens ?? 0)
                traceState = .success("\(model) · \(tokens) tokens")
                return true
            case .rejected:
                guard lastRejectedSessionId != match.sessionId else { return true }
                lastRejectedSessionId = match.sessionId
                let transcript = try? await AlexClient(config: config)
                    .traceTranscript(sessionId: match.sessionId)
                let rejectedTurn = transcript?.turns.reversed().first {
                    ($0.status ?? 0) >= 400 || $0.error?.isEmpty == false
                }
                let detail = rejectedTurn?.error
                    ?? rejectedTurn?.errorCode
                    ?? match.statusLabel
                if case .rejected(let message) = OnboardingSupport.traceOutcome(
                    status: match.lastStatus, errorCount: match.errors, error: detail)
                {
                    traceState = .failure(
                        "Your request reached Alex but the provider rejected it: \(message)")
                }
                return true
            }
        }
        return false
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
            let client = AlexClient(config: config)
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
        if step == Self.connectStep { go(to: Self.connectStep + 1) }
    }

    func loadNetworkChoice() {
        guard !networkChoiceLoaded else { return }
        networkChoiceLoaded = true
        networkInterfaces = NetworkInterfaces.rankedForRemoteAccess(
            NetworkInterfaces.addresses())
        switch store.config?.host ?? "" {
        case "127.0.0.1", "localhost", "::1", "[::1]", "":
            networkChoice = "loopback"
        case "0.0.0.0", "::", "*":
            networkChoice = "all"
        case let host:
            networkChoice = "interface"
            selectedInterfaceAddress = host
        }
        if selectedInterfaceAddress.isEmpty {
            selectedInterfaceAddress = networkInterfaces.first?.address ?? ""
        }
    }

    /// Applies the chosen bind target through `alex service bind` and restarts
    /// the daemon service so the choice is live before harness connect.
    func saveNetworkChoice() {
        guard !networkSaveState.isWorking else { return }
        let target: String
        switch networkChoice {
        case "all": target = "all"
        case "interface":
            guard !selectedInterfaceAddress.isEmpty else {
                networkSaveState = .failure("Choose an interface address first.")
                return
            }
            target = selectedInterfaceAddress
        default: target = "loopback"
        }
        networkSaveState = .working("Saving network access…")
        Task {
            let currentHost = store.config?.host ?? "127.0.0.1"
            let currentTarget: String
            switch currentHost {
            case "127.0.0.1", "localhost", "::1", "[::1]", "": currentTarget = "loopback"
            case "0.0.0.0", "::", "*": currentTarget = "all"
            default: currentTarget = currentHost
            }
            if currentTarget == target {
                networkSaveState = .success(target == "loopback"
                    ? "Alex is reachable from this Mac only."
                    : "Alex already listens on \(target == "all" ? "all interfaces" : target).")
                return
            }
            let bind = await DaemonController.run(args: ["service", "bind", target])
            guard bind.ok else {
                networkSaveState = .failure(bind.combined.isEmpty
                    ? "Could not save the network access choice." : bind.combined)
                return
            }
            networkSaveState = .working("Restarting the daemon service…")
            let restart = await DaemonController.run(args: ["service", "restart"])
            if restart.ok {
                await store.refresh()
                networkSaveState = .success(target == "loopback"
                    ? "Alex is reachable from this Mac only."
                    : "Alex now listens on \(target == "all" ? "all interfaces" : target).")
            } else {
                networkSaveState = .failure(restart.combined.isEmpty
                    ? "Could not restart the daemon service." : restart.combined)
            }
        }
    }

    func cancel() {
        pollTask?.cancel()
        authModel?.cancel()
    }
}

enum OnboardingUILayout {
    static let windowWidth: CGFloat = 760
    static let contentHorizontalPadding: CGFloat = 30
    static let providerTileMinimumWidth: CGFloat = 205
    static let compatibleAppChipMinimumWidth: CGFloat = 108

    static var contentWidth: CGFloat {
        windowWidth - (contentHorizontalPadding * 2)
    }

    static func adaptiveColumnCount(
        availableWidth: CGFloat,
        minimumWidth: CGFloat,
        spacing: CGFloat
    ) -> Int {
        max(1, Int((availableWidth + spacing) / (minimumWidth + spacing)))
    }
}

struct OnboardingView: View {
    @Bindable var model: OnboardingModel

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider().overlay(AlexTheme.Colors.cardBorder)
            ScrollViewReader { proxy in
                ScrollView {
                    Color.clear.frame(height: 0).id("onboarding-step-top")
                    stepContent
                        .padding(OnboardingUILayout.contentHorizontalPadding)
                        .frame(maxWidth: .infinity, minHeight: 410, alignment: .topLeading)
                }
                .onChange(of: model.step) { _, _ in scrollToStepTop(proxy) }
                .onChange(of: model.selectedProvider) { _, _ in scrollToStepTop(proxy) }
                .onChange(of: model.addingProviderAccount) { _, _ in scrollToStepTop(proxy) }
            }
            Divider().overlay(AlexTheme.Colors.cardBorder)
            footer
        }
        .frame(width: OnboardingUILayout.windowWidth, height: 560)
        .background(AlexTheme.Colors.background)
        .focusable()
        .onMoveCommand { direction in
            if direction == .left { model.back() }
            if direction == .right { model.next() }
        }
        .task { await model.loadCredentialImportCandidates() }
    }

    private func scrollToStepTop(_ proxy: ScrollViewProxy) {
        Task { @MainActor in
            await Task.yield()
            proxy.scrollTo("onboarding-step-top", anchor: .top)
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
        case OnboardingModel.providerStep: providerPicker
        case OnboardingModel.networkStep: networkAccess
        case OnboardingModel.connectStep: stagedConnect
        case 4: credentials
        case 5: notifications
        case 6: failover
        default: beyondSingleProvider
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
            Text("One local daemon exposing four client APIs and routing each request through supported provider and model paths.")
                .font(.system(size: 18, weight: .semibold))
                .foregroundStyle(AlexTheme.Colors.foreground)
            Text("Alex lets you manage and combine your token providers — and use them from compatible connected harnesses and API clients.")
                .font(.system(size: 13))
                .foregroundStyle(AlexTheme.Colors.textSecondary)
        }
    }

    private var providerPicker: some View {
        VStack(alignment: .leading, spacing: 16) {
            intro("Connect a real provider", "Choose a provider and complete its secure authentication here. You can skip for now at any point.")
            LazyVGrid(
                columns: [GridItem(
                    .adaptive(minimum: OnboardingUILayout.providerTileMinimumWidth),
                    spacing: 10)],
                spacing: 10
            ) {
                ForEach(ProviderInfo.supportedProviders, id: \.self) { provider in
                    let connectedCount = model.accounts(for: provider).count
                    let detectedCount = model.importCandidates(for: provider).count
                    choiceButton(
                        title: ProviderInfo.displayName(provider),
                        subtitle: provider == model.selectedProvider
                            ? "Selected"
                            : (connectedCount > 0
                                ? "\(connectedCount) connected"
                                : (detectedCount > 0 ? "Detected login" : "Connect")),
                        icon: ProviderInfo.loginArg(provider), selected: provider == model.selectedProvider
                    ) { model.chooseProvider(provider) }
                }
            }
            if let provider = model.selectedProvider,
               model.addingProviderAccount,
               provider == "openrouter"
            {
                openRouterOnboarding
                operation(model.providerState)
            } else if let provider = model.selectedProvider,
                      model.addingProviderAccount,
                      provider == "cliproxyapi"
            {
                cliProxyAPIOnboarding
                operation(model.providerState)
            } else if let provider = model.selectedProvider,
                      model.addingProviderAccount,
                      provider == "exo"
            {
                exoOnboarding
                operation(model.providerState)
            } else if let authModel = model.authModel {
                AuthFlowView(model: authModel, close: {}, embedded: true)
                    .padding(.top, 4)
                    .cardStyle()
            } else if let provider = model.selectedProvider,
                      model.accounts(for: provider).isEmpty,
                      !model.credentialImportCandidatesLoaded
            {
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text("Checking for existing provider logins…")
                        .font(.system(size: 11))
                        .foregroundStyle(AlexTheme.Colors.textTertiary)
                }
                .padding(12)
                .cardStyle()
            } else if let provider = model.selectedProvider,
                      model.accounts(for: provider).isEmpty,
                      !model.importCandidates(for: provider).isEmpty
            {
                detectedCredentialChooser(provider)
                operation(model.providerState)
            } else if let provider = model.selectedProvider,
                      !model.accounts(for: provider).isEmpty
            {
                providerAccountChooser(provider)
                if !model.providerState.isSuccess {
                    operation(model.providerState)
                }
            } else {
                operation(model.providerState)
            }
        }
    }

    private var openRouterOnboarding: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Connect OpenRouter")
                .font(.system(size: 13, weight: .semibold))
                .foregroundStyle(AlexTheme.Colors.foreground)
            Text("OpenRouter keys do not include an account identity, so give this key a recognizable name.")
                .font(.system(size: 11))
                .foregroundStyle(AlexTheme.Colors.textTertiary)
            TextField("Account name, e.g. Personal", text: $model.openRouterAccountName)
                .textFieldStyle(.roundedBorder)
            HStack(spacing: 8) {
                Group {
                    if model.revealOpenRouterAPIKey {
                        TextField("OpenRouter API key", text: $model.openRouterAPIKey)
                    } else {
                        SecureField("OpenRouter API key", text: $model.openRouterAPIKey)
                    }
                }
                .textFieldStyle(.roundedBorder)
                .font(AlexTheme.Fonts.mono(11))
                PillButton(
                    title: model.revealOpenRouterAPIKey ? "Hide" : "Show",
                    variant: .bordered,
                    systemImage: model.revealOpenRouterAPIKey ? "eye.slash" : "eye"
                ) { model.revealOpenRouterAPIKey.toggle() }
            }
            PillButton(
                title: "Save and connect", variant: .solidAccent,
                systemImage: "key",
                isEnabled: !model.providerState.isWorking
                    && !model.openRouterAccountName.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                    && !model.openRouterAPIKey.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
                isBusy: model.providerState.isWorking
            ) { model.connectOpenRouter() }
        }
        .padding(12)
        .cardStyle()
    }

    private var exoOnboarding: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Connect Exo")
                .font(.system(size: 13, weight: .semibold))
                .foregroundStyle(AlexTheme.Colors.foreground)
            Text("Enter the Exo API endpoint. Alex will save it and verify that it responds before continuing.")
                .font(.system(size: 11))
                .foregroundStyle(AlexTheme.Colors.textTertiary)
            TextField("http://localhost:52415", text: $model.exoEndpoint)
                .textFieldStyle(.roundedBorder)
                .font(AlexTheme.Fonts.mono(11))
                .onSubmit { model.connectExo() }
            PillButton(
                title: "Check and connect", variant: .solidAccent,
                systemImage: "network",
                isEnabled: !model.providerState.isWorking
                    && !model.exoEndpoint.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
                isBusy: model.providerState.isWorking
            ) { model.connectExo() }
        }
        .padding(12)
        .cardStyle()
    }

    private var cliProxyAPIOnboarding: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Connect CLIProxyAPI")
                .font(.system(size: 13, weight: .semibold))
                .foregroundStyle(AlexTheme.Colors.foreground)
            Text("Enter your existing CLIProxyAPI endpoint and credential. Alex probes /v1/models before saving either setting.")
                .font(.system(size: 11))
                .foregroundStyle(AlexTheme.Colors.textTertiary)
            TextField("http://127.0.0.1:8317/v1", text: $model.cliProxyAPIEndpoint)
                .textFieldStyle(.roundedBorder)
                .font(AlexTheme.Fonts.mono(11))
            HStack(spacing: 8) {
                Group {
                    if model.revealCLIProxyAPICredential {
                        TextField("CLIProxyAPI credential", text: $model.cliProxyAPICredential)
                    } else {
                        SecureField("CLIProxyAPI credential", text: $model.cliProxyAPICredential)
                    }
                }
                .textFieldStyle(.roundedBorder)
                .font(AlexTheme.Fonts.mono(11))
                PillButton(
                    title: model.revealCLIProxyAPICredential ? "Hide" : "Show",
                    variant: .bordered,
                    systemImage: model.revealCLIProxyAPICredential ? "eye.slash" : "eye"
                ) { model.revealCLIProxyAPICredential.toggle() }
            }
            PillButton(
                title: "Probe and connect", variant: .solidAccent,
                systemImage: "arrow.triangle.branch",
                isEnabled: !model.providerState.isWorking
                    && !model.cliProxyAPIEndpoint.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                    && !model.cliProxyAPICredential.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
                isBusy: model.providerState.isWorking
            ) { model.connectCLIProxyAPI() }
        }
        .padding(12)
        .cardStyle()
    }

    private func providerAccountChooser(_ provider: String) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Choose an existing account or add a new one")
                .font(.system(size: 13, weight: .semibold))
                .foregroundStyle(AlexTheme.Colors.foreground)
            ForEach(model.accounts(for: provider)) { account in
                Button { model.useExistingProviderAccount(account) } label: {
                    HStack(spacing: 10) {
                        Image(systemName: "person.crop.circle")
                            .font(.system(size: 18))
                            .foregroundStyle(AlexTheme.Colors.primary)
                        VStack(alignment: .leading, spacing: 2) {
                            Text(model.accountDisplayName(account))
                                .font(.system(size: 12, weight: .semibold))
                                .foregroundStyle(AlexTheme.Colors.foreground)
                            Text(model.accountDisplayDetail(account))
                                .font(AlexTheme.Fonts.metaLabel)
                                .foregroundStyle(AlexTheme.Colors.textTertiary)
                        }
                        Spacer()
                        if model.selectedProviderAccountID == account.id {
                            Image(systemName: "checkmark.circle.fill")
                                .foregroundStyle(AlexTheme.Colors.success)
                        } else {
                            Text("Use")
                                .font(.system(size: 11, weight: .medium))
                                .foregroundStyle(AlexTheme.Colors.primary)
                        }
                    }
                    .padding(10)
                    .background(
                        RoundedRectangle(cornerRadius: AlexTheme.Radius.sm)
                            .fill(AlexTheme.Colors.overlay(0.035)))
                    .overlay(
                        RoundedRectangle(cornerRadius: AlexTheme.Radius.sm)
                            .strokeBorder(
                                model.selectedProviderAccountID == account.id
                                    ? AlexTheme.Colors.success.opacity(0.4)
                                    : AlexTheme.Colors.cardBorder))
                }
                .buttonStyle(.plain)
            }

            if !model.importCandidates(for: provider).isEmpty {
                Divider().overlay(AlexTheme.Colors.cardBorder)
                Text("Detected outside Alex")
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundStyle(AlexTheme.Colors.textSecondary)
                Text("These remain owned by their original apps. Alex imports only the credentials you leave checked.")
                    .font(.system(size: 10))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
                detectedCredentialRows(provider)
                PillButton(
                    title: "Import checked credential", variant: .bordered,
                    systemImage: "square.and.arrow.down",
                    isEnabled: !model.providerState.isWorking
                        && model.importCandidates(for: provider).contains {
                            model.selectedCredentialImports.contains($0.source)
                        },
                    isBusy: model.providerState.isWorking
                ) { model.importDetectedCredentials(for: provider) }
            }

            PillButton(
                title: "Add another \(ProviderInfo.displayName(provider)) account",
                variant: .bordered, systemImage: "plus"
            ) { model.addProviderAccount() }
        }
        .padding(12)
        .cardStyle()
    }

    private func detectedCredentialChooser(_ provider: String) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Existing login detected")
                .font(.system(size: 13, weight: .semibold))
                .foregroundStyle(AlexTheme.Colors.foreground)
            Text("Choose whether Alex may import it. Reset never deletes credentials owned by another app.")
                .font(.system(size: 11))
                .foregroundStyle(AlexTheme.Colors.textTertiary)
            detectedCredentialRows(provider)
            HStack(spacing: 8) {
                PillButton(
                    title: "Import checked credential", variant: .solidAccent,
                    systemImage: "square.and.arrow.down",
                    isEnabled: !model.providerState.isWorking
                        && model.importCandidates(for: provider).contains {
                            model.selectedCredentialImports.contains($0.source)
                        },
                    isBusy: model.providerState.isWorking
                ) { model.importDetectedCredentials(for: provider) }
                PillButton(
                    title: "Connect a new account", variant: .bordered,
                    systemImage: "plus", isEnabled: !model.providerState.isWorking
                ) { model.addProviderAccount() }
            }
        }
        .padding(12)
        .cardStyle()
    }

    @ViewBuilder
    private func detectedCredentialRows(_ provider: String) -> some View {
        ForEach(model.importCandidates(for: provider)) { candidate in
            Toggle(isOn: model.credentialImportBinding(candidate.source)) {
                VStack(alignment: .leading, spacing: 2) {
                    Text(candidate.label)
                        .font(.system(size: 11, weight: .semibold))
                        .foregroundStyle(AlexTheme.Colors.foreground)
                    Text("\(candidate.kind.replacingOccurrences(of: "_", with: " ")) · \(candidate.sourcePath)")
                        .font(AlexTheme.Fonts.metaLabel)
                        .foregroundStyle(AlexTheme.Colors.textTertiary)
                        .lineLimit(1)
                }
            }
            .toggleStyle(.checkbox)
        }
    }

    private var networkAccess: some View {
        VStack(alignment: .leading, spacing: 14) {
            intro(
                "Who can reach Alex?",
                "Alex is an HTTP proxy. Harnesses on other machines — a Linux box, a homelab server, a laptop on your tailnet — can only connect if Alex listens beyond this Mac.")
            VStack(alignment: .leading, spacing: 10) {
                networkChoiceRow(
                    value: "loopback",
                    title: "This Mac only (recommended)",
                    subtitle: "Listens on 127.0.0.1. Nothing else on your network can reach Alex.",
                    icon: "lock.shield")
                networkChoiceRow(
                    value: "interface",
                    title: "A specific network",
                    subtitle: "Pick one interface — your LAN or Tailscale. Remote 1-liners embed this address.",
                    icon: "network")
                networkChoiceRow(
                    value: "all",
                    title: "All interfaces",
                    subtitle: "Listens on 0.0.0.0 — every network this Mac joins, now and later.",
                    icon: "globe")
            }
            if model.networkChoice == "interface" {
                if model.networkInterfaces.isEmpty {
                    Text("No non-loopback interface addresses are available.")
                        .font(.system(size: 11))
                        .foregroundStyle(AlexTheme.Colors.textTertiary)
                } else {
                    HStack {
                        Text("Interface")
                            .font(.system(size: 12, weight: .medium))
                        Spacer()
                        Picker("", selection: $model.selectedInterfaceAddress) {
                            ForEach(model.networkInterfaces) { interface in
                                Text(interface.displayName).tag(interface.address)
                            }
                        }
                        .labelsHidden()
                        .frame(maxWidth: 300)
                    }
                    .padding(12)
                    .cardStyle()
                }
            }
            if model.networkChoice != "loopback" {
                VStack(alignment: .leading, spacing: 5) {
                    Label("Anyone on that network with your local key gains admin access", systemImage: "exclamationmark.triangle.fill")
                        .font(.system(size: 12, weight: .bold))
                        .foregroundStyle(AlexTheme.Colors.warningOrange)
                    Text("Model calls still require a scoped key, but the admin API — credential vault, key minting, data reset — is protected only by your local key. Prefer a trusted network such as Tailscale, and rotate the local key if you share this network.")
                        .font(.system(size: 11.5))
                        .foregroundStyle(AlexTheme.Colors.textSecondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
                .padding(12)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(AlexTheme.Colors.warningOrange.opacity(0.10),
                    in: RoundedRectangle(cornerRadius: AlexTheme.Radius.md))
            }
            PillButton(
                title: "Apply network access", variant: .solidAccent,
                isEnabled: !model.networkSaveState.isWorking,
                isBusy: model.networkSaveState.isWorking
            ) { model.saveNetworkChoice() }
            operation(model.networkSaveState)
            Text("You can change this anytime in Settings → General → Network exposure.")
                .font(.system(size: 11))
                .foregroundStyle(AlexTheme.Colors.textTertiary)
        }
        .onAppear { model.loadNetworkChoice() }
    }

    private func networkChoiceRow(
        value: String, title: String, subtitle: String, icon: String
    ) -> some View {
        Button {
            model.networkChoice = value
            model.networkSaveState = .idle
        } label: {
            HStack(spacing: 10) {
                Image(systemName: icon)
                    .font(.system(size: 16))
                    .foregroundStyle(AlexTheme.Colors.primary)
                    .frame(width: 24)
                VStack(alignment: .leading, spacing: 2) {
                    Text(title)
                        .font(.system(size: 13, weight: .semibold))
                        .foregroundStyle(AlexTheme.Colors.foreground)
                    Text(subtitle)
                        .font(.system(size: 11))
                        .foregroundStyle(AlexTheme.Colors.textSecondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
                Spacer()
                if model.networkChoice == value {
                    Image(systemName: "checkmark.circle.fill")
                        .foregroundStyle(AlexTheme.Colors.primary)
                }
            }
            .padding(12)
            .background(RoundedRectangle(cornerRadius: AlexTheme.Radius.md)
                .fill(model.networkChoice == value
                    ? AlexTheme.Colors.primary.opacity(0.10) : AlexTheme.Colors.card))
            .overlay(RoundedRectangle(cornerRadius: AlexTheme.Radius.md)
                .strokeBorder(model.networkChoice == value
                    ? AlexTheme.Colors.primary.opacity(0.45) : AlexTheme.Colors.cardBorder))
        }
        .buttonStyle(.plain)
    }

    private var stagedConnect: some View {
        VStack(alignment: .leading, spacing: 12) {
            intro("Connect, test, and inspect", "Complete each stage to unlock the next. Finished stages collapse so your current action stays in view.")
            stageOne
            stageTwo
            stageThree
        }
    }

    private var stageOne: some View {
        stageCard(number: 1, title: "Pick your harness", completed: model.harnessState.isSuccess,
                  summary: model.harnessState.message,
                  completedActionTitle: "Change harness", completedAction: model.changeHarness) {
            if model.connectableHarnesses.isEmpty {
                statusCard(icon: "terminal", tint: AlexTheme.Colors.warningOrange,
                           text: "No installed, connectable harnesses were detected. You can skip this page and continue.")
            } else {
                LazyVGrid(columns: [GridItem(.adaptive(minimum: 180), spacing: 8)], spacing: 8) {
                    ForEach(model.connectableHarnesses) { harness in
                        choiceButton(
                            title: HarnessCatalog.displayName(harness.name),
                            subtitle: harness.name == model.selectedHarness ? "Plan loaded" : "Preview plan",
                            icon: harness.name, selected: harness.name == model.selectedHarness
                        ) { model.selectHarness(harness) }
                    }
                }
            }
            operation(model.harnessPlanState)
            if model.harnessPlanState.isSuccess {
                VStack(alignment: .leading, spacing: 8) {
                    Text("FILES CHANGED")
                        .font(AlexTheme.Fonts.metaMono).foregroundStyle(AlexTheme.Colors.textTertiary)
                    if model.harnessPlan.isEmpty {
                        Text("No file changes are needed; Connect will refresh the harness model list.")
                            .font(.system(size: 11)).foregroundStyle(AlexTheme.Colors.textSecondary)
                    }
                    ForEach(model.harnessPlan) { item in
                        VStack(alignment: .leading, spacing: 2) {
                            HStack(spacing: 7) {
                                Text(item.action.uppercased())
                                    .font(AlexTheme.Fonts.metaMono).foregroundStyle(AlexTheme.Colors.primary)
                                Text(item.path).font(AlexTheme.Fonts.mono(11)).textSelection(.enabled)
                            }
                            Text(item.detail).font(.system(size: 11)).foregroundStyle(AlexTheme.Colors.textSecondary)
                        }
                    }
                    PillButton(
                        title: "Connect", variant: .solidAccent, systemImage: "link",
                        isEnabled: model.harnessPlanState.isSuccess && !model.harnessState.isWorking,
                        isBusy: model.harnessState.isWorking
                    ) { model.connectSelectedHarness() }
                    operation(model.harnessState)
                    if model.harnessState.isSuccess,
                       let harness = model.selectedHarness,
                       let command = OnboardingSupport.launchCommand(harness: harness)
                    {
                        Text("Launch the connected profile with:")
                            .font(.system(size: 11, weight: .semibold))
                        CopyableCode(value: command)
                        if harness == "claude" {
                            Text("Plain `claude` still uses your normal authentication. `alex wrap claude` is the equivalent shortcut.")
                                .font(.system(size: 10))
                                .foregroundStyle(AlexTheme.Colors.textTertiary)
                        }
                    }
                }
                .padding(12).cardStyle()
            }
        }
    }

    private var stageTwo: some View {
        stageCard(number: 2, title: "Send a test request", completed: model.traceState.isSuccess,
                  summary: model.traceState.message, locked: !model.harnessState.isSuccess) {
            if model.exampleModelLoading {
                ProgressView("Choosing the verified example model…")
            } else {
                Text(OnboardingSupport.modelHint(harness: model.selectedHarness, model: model.exampleModel))
                    .font(.system(size: 13)).foregroundStyle(AlexTheme.Colors.textSecondary)
                CopyableCode(value: OnboardingSupport.testCommand(
                    harness: model.selectedHarness, model: model.exampleModel))
            }
            HStack(spacing: 10) {
                if model.traceState.isSuccess {
                    Image(systemName: "checkmark.circle.fill").foregroundStyle(AlexTheme.Colors.success)
                } else if model.traceState.isFailure {
                    Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(AlexTheme.Colors.destructive)
                } else {
                    ProgressView().controlSize(.small)
                }
                operationText(model.traceState)
            }
            .padding(12).cardStyle()
            PillButton(
                title: "Check for Request",
                variant: .bordered,
                systemImage: "arrow.clockwise",
                isEnabled: !model.traceState.isSuccess,
                isBusy: model.traceCheckRunning
            ) {
                model.checkForTrace()
            }
            if model.traceState.isFailure {
                PillButton(title: "Troubleshoot", variant: .bordered,
                           systemImage: "wrench.and.screwdriver", isBusy: model.checksRunning) {
                    model.runTroubleshooting()
                }
            }
            if model.troubleshootExpanded { troubleshootPanel }
        }
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

    private var stageThree: some View {
        stageCard(number: 3, title: "See your trace", completed: false, summary: nil,
                  locked: !model.traceState.isSuccess) {
            if let trace = model.discoveredTrace {
                traceSummary(trace)
                PillButton(title: "Open Trace Browser", variant: .solidAccent,
                           systemImage: "list.bullet.rectangle") { model.openBrowser() }
                Text(model.selectedHarness.map { "Opens filtered with `harness:\($0)`." }
                     ?? "Opens without a harness filter.")
                    .font(.system(size: 11)).foregroundStyle(AlexTheme.Colors.textTertiary)
            }
        }
    }

    private var credentials: some View {
        VStack(alignment: .leading, spacing: 14) {
            intro("Credentials for compatible apps", "Settings → Credentials can mint scoped, model-only keys for compatible API clients.")
            VStack(alignment: .leading, spacing: 7) {
                Text("APIs your app can speak")
                    .font(AlexTheme.Fonts.metaMono).foregroundStyle(AlexTheme.Colors.textTertiary)
                api("Anthropic Messages", "POST /v1/messages")
                api("OpenAI Chat", "POST /v1/chat/completions")
                api("OpenAI Responses", "POST /v1/responses")
                api("Gemini generateContent", "POST /v1beta/models/{model}:generateContent")
            }.padding(14).cardStyle()
            VStack(alignment: .leading, spacing: 9) {
                Text("Models you reach through them")
                    .font(AlexTheme.Fonts.metaMono).foregroundStyle(AlexTheme.Colors.textTertiary)
                LazyVGrid(
                    columns: [GridItem(
                        .adaptive(minimum: OnboardingUILayout.compatibleAppChipMinimumWidth),
                        spacing: 7)],
                    spacing: 7
                ) {
                    ForEach(["Claude", "GPT/Codex", "Gemini", "Grok", "Kimi", "OpenRouter", "Exo", "CLIProxyAPI"], id: \.self) { name in
                        Text(name)
                            .font(.system(size: 11, weight: .medium))
                            .lineLimit(1)
                            .minimumScaleFactor(0.9)
                            .padding(.horizontal, 9).padding(.vertical, 5)
                            .frame(maxWidth: .infinity)
                            .background(Capsule().fill(AlexTheme.Colors.primary.opacity(0.10)))
                            .overlay(Capsule().strokeBorder(AlexTheme.Colors.primary.opacity(0.25)))
                    }
                }
                Text("Your app speaks one of these formats — Alex routes supported client and provider combinations and reports unsupported pairs explicitly.")
                    .font(.system(size: 12, weight: .medium)).foregroundStyle(AlexTheme.Colors.foreground)
            }.padding(14).cardStyle()
            credentialsDemo
        }
    }

    @ViewBuilder private var credentialsDemo: some View {
        if let curl = model.credentialsCurl, let key = model.mintedOnboardingKey {
            VStack(alignment: .leading, spacing: 10) {
                HStack {
                    VStack(alignment: .leading, spacing: 2) {
                        Text("ONE-HOUR MODEL-ONLY KEY")
                            .font(AlexTheme.Fonts.metaMono).foregroundStyle(AlexTheme.Colors.success)
                        Text("Scoped to \(model.exampleModel) · label onboarding")
                            .font(.system(size: 11)).foregroundStyle(AlexTheme.Colors.textSecondary)
                    }
                    Spacer()
                    Text(key.expiresMs == nil ? "1 hour" : "expires in 1 hour")
                        .font(AlexTheme.Fonts.metaLabel).foregroundStyle(AlexTheme.Colors.textTertiary)
                }
                ScrollView(.horizontal) {
                    Text(curl)
                        .font(AlexTheme.Fonts.mono(10.5))
                        .foregroundStyle(AlexTheme.Colors.foreground)
                        .textSelection(.enabled)
                        .fixedSize(horizontal: true, vertical: false)
                }
                .padding(10)
                .background(RoundedRectangle(cornerRadius: AlexTheme.Radius.sm)
                    .fill(AlexTheme.Colors.consoleBackground))
                optionalHeader("x-session-id: my-first-session", "groups requests into one session")
                optionalHeader("x-alex-task: quickstart", "tags the trace with a task name")
                optionalHeader("x-alex-kind: experiment", "labels the kind of work")
                Text("All three tagging headers are optional and Alex strips them before forwarding the request upstream.")
                    .font(.system(size: 11, weight: .medium))
                    .foregroundStyle(AlexTheme.Colors.textSecondary)
                HStack(spacing: 8) {
                    PillButton(title: "Copy", variant: .bordered, systemImage: "doc.on.doc") {
                        NSPasteboard.general.clearContents()
                        NSPasteboard.general.setString(curl, forType: .string)
                    }
                    PillButton(
                        title: "Run", variant: .solidAccent, systemImage: "play.fill",
                        isEnabled: !model.credentialRunState.isWorking,
                        isBusy: model.credentialRunState.isWorking
                    ) { model.runCredentialsDemo() }
                    if model.credentialRunState.isTerminal,
                       model.onboardingKeyFingerprint != nil
                    {
                        PillButton(
                            title: "Show in Trace Browser", variant: .bordered,
                            systemImage: "magnifyingglass"
                        ) { model.openCredentialsTrace() }
                    }
                }
                operation(model.credentialRunState)
                if let response = model.credentialResponseText {
                    Text(response)
                        .font(AlexTheme.Fonts.mono(10.5))
                        .foregroundStyle(AlexTheme.Colors.foreground)
                        .textSelection(.enabled)
                        .lineLimit(12)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(10)
                        .background(RoundedRectangle(cornerRadius: AlexTheme.Radius.sm)
                            .fill(AlexTheme.Colors.consoleBackground))
                }
            }
            .padding(14).cardStyle()
        } else {
            VStack(alignment: .leading, spacing: 10) {
                Text("Mint a real one-hour key to try Alex from an OpenAI-compatible app.")
                    .font(.system(size: 12)).foregroundStyle(AlexTheme.Colors.textSecondary)
                PillButton(
                    title: "Mint onboarding key", variant: .solidAccent, systemImage: "key",
                    isEnabled: !model.credentialMintState.isWorking,
                    isBusy: model.credentialMintState.isWorking
                ) { model.mintCredentialsDemoKey() }
                operation(model.credentialMintState)
                Text("Scoped keys are revocable and auditable in Settings → Credentials.")
                    .font(.system(size: 11)).foregroundStyle(AlexTheme.Colors.textTertiary)
            }
            .padding(14).cardStyle()
        }
    }

    private var notifications: some View {
        VStack(alignment: .leading, spacing: 20) {
            Image(systemName: "paperplane.circle.fill").font(.system(size: 54)).foregroundStyle(AlexTheme.Colors.primary)
            intro("Never lose a login", "Alex detects when credentials need re-authenticating and can message you to refresh them. Enabled middleware can reroute eligible failures.")
            statusCard(icon: "text.bubble", tint: AlexTheme.Colors.success, text: "/status shows subscriptions, usage, and ping health wherever you are.")
        }
    }

    private var failover: some View {
        VStack(alignment: .leading, spacing: 20) {
            Image(systemName: "shield.lefthalf.filled.badge.checkmark").font(.system(size: 54)).foregroundStyle(AlexTheme.Colors.success)
            intro("Keep your agents running", "Settings → Middleware lets you enable or edit rules that can move eligible work between models.")
            VStack(alignment: .leading, spacing: 9) {
                failoverPair("claude-fable-5", "gpt-5.6-sol")
            }
            .padding(14).cardStyle()
            statusCard(
                icon: "arrow.triangle.branch", tint: AlexTheme.Colors.primary,
                text: "The default middleware catches any structured Fable refusal, retries with high-effort GPT-5.6 Sol, and keeps that route for the session for 24 hours.")
        }
    }

    private var beyondSingleProvider: some View {
        VStack(alignment: .leading, spacing: 22) {
            HStack(spacing: 18) {
                Image(systemName: "point.3.connected.trianglepath.dotted")
                Image(systemName: "plus")
                    .font(.system(size: 22, weight: .light))
                Image(systemName: "cpu")
                Image(systemName: "arrow.right")
                    .font(.system(size: 22, weight: .light))
                Image(systemName: "sparkles")
            }
            .font(.system(size: 46, weight: .medium))
            .foregroundStyle(AlexTheme.Colors.primary)
            .frame(maxWidth: .infinity)
            .padding(.vertical, 18)
            intro(
                "Beyond single provider",
                "The future is fusion models and mixtures of agents. Get the best coding experience by using multiple models at the same time in supported harnesses — or build your own experimental harness tools on Alex.")
            statusCard(
                icon: "square.stack.3d.up",
                tint: AlexTheme.Colors.primary,
                text: "Combine distinct model strengths instead of asking one model to do everything.")
        }
    }

    private var pam: some View {
        VStack(alignment: .leading, spacing: 16) {
            if let image = HarnessIconLoader.image(
                resource: "pi", extension: "png", subdirectory: "onboarding")
            {
                Image(nsImage: image)
                    .resizable()
                    .aspectRatio(contentMode: .fit)
                    .frame(maxWidth: .infinity, maxHeight: 250)
                    .clipShape(RoundedRectangle(cornerRadius: AlexTheme.Radius.lg))
            } else {
                Image(systemName: "person.3.sequence.fill")
                    .font(.system(size: 72))
                    .foregroundStyle(AlexTheme.Colors.primary)
                    .frame(maxWidth: .infinity, minHeight: 180)
            }
            intro(
                "PAM — Experimental",
                "PAM is a mixture-of-agents plugin for Pi that runs multiple models at once as Agent and Oracle roles — like the AMP Dial. It ships with Alex (plugins/pam) — point it at your alex/* models and experiment.")
        }
    }

    private var footer: some View {
        HStack(spacing: 12) {
            PillButton(title: "Skip tutorial", variant: .bordered) { model.skipTutorial() }
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

    private func stageCard<Content: View>(
        number: Int, title: String, completed: Bool, summary: String?, locked: Bool = false,
        completedActionTitle: String? = nil, completedAction: (() -> Void)? = nil,
        @ViewBuilder content: () -> Content
    ) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 9) {
                Image(systemName: completed ? "checkmark.circle.fill" : (locked ? "lock.fill" : "\(number).circle.fill"))
                    .foregroundStyle(completed ? AlexTheme.Colors.success : (locked ? AlexTheme.Colors.textFaintest : AlexTheme.Colors.primary))
                Text(title).font(.system(size: 14, weight: .semibold))
                if completed {
                    Spacer()
                    if let summary {
                        Text(summary).font(.system(size: 11, weight: .medium))
                            .foregroundStyle(AlexTheme.Colors.success)
                    }
                    if let completedActionTitle, let completedAction {
                        PillButton(
                            title: completedActionTitle, variant: .bordered,
                            systemImage: "arrow.triangle.2.circlepath"
                        ) { completedAction() }
                    }
                }
            }
            if !completed && !locked { content() }
            if locked {
                Text("Complete the previous stage to unlock this one.")
                    .font(.system(size: 11)).foregroundStyle(AlexTheme.Colors.textTertiary)
            }
        }
        .padding(14).cardStyle()
    }

    private func traceSummary(_ trace: TraceSession) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            summaryRow("Model", trace.models?.first ?? "Unknown")
            summaryRow("Tokens", "\(trace.totalInputTokens ?? 0) in · \(trace.totalOutputTokens ?? 0) out")
            if let cost = trace.totalCostUsd { summaryRow("Cost", TraceNumberFormat.cost(cost)) }
            summaryRow("Status", trace.statusLabel ?? trace.lastStatus.map(String.init) ?? "Complete")
            let seconds = max(0, Int64(Date().timeIntervalSince1970) - trace.lastTsMs / 1_000)
            summaryRow("Time", seconds < 10 ? "now" : "\(Format.duration(seconds)) ago")
        }
        .padding(12).cardStyle()
    }

    private func summaryRow(_ label: String, _ value: String) -> some View {
        HStack {
            Text(label).font(AlexTheme.Fonts.metaLabel).foregroundStyle(AlexTheme.Colors.textTertiary)
            Spacer()
            Text(value).font(.system(size: 11, weight: .medium)).foregroundStyle(AlexTheme.Colors.foreground)
        }
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
                VStack(alignment: .leading, spacing: 2) {
                    Text(title)
                        .font(.system(size: 13, weight: .semibold))
                        .lineLimit(1)
                        .minimumScaleFactor(0.9)
                    Text(subtitle)
                        .font(AlexTheme.Fonts.metaLabel)
                        .foregroundStyle(AlexTheme.Colors.textTertiary)
                        .lineLimit(1)
                }
                .layoutPriority(1)
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

    private func optionalHeader(_ header: String, _ explanation: String) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Text(header).font(AlexTheme.Fonts.mono(10.5)).foregroundStyle(AlexTheme.Colors.primary)
            Text("— \(explanation)").font(.system(size: 10.5)).foregroundStyle(AlexTheme.Colors.textTertiary)
        }
    }

    private func failoverPair(_ primary: String, _ fallback: String) -> some View {
        HStack(spacing: 10) {
            Text(primary).font(AlexTheme.Fonts.mono(11.5))
            Image(systemName: "arrow.right").foregroundStyle(AlexTheme.Colors.primary)
            Text(fallback).font(AlexTheme.Fonts.mono(11.5))
            Spacer()
        }
        .foregroundStyle(AlexTheme.Colors.foreground)
    }

    private func statusCard(icon: String, tint: Color, text: String) -> some View {
        HStack(spacing: 10) { Image(systemName: icon).foregroundStyle(tint); Text(text).font(.system(size: 12)).foregroundStyle(AlexTheme.Colors.textSecondary) }.padding(14).cardStyle()
    }
}

private extension OnboardingModel.OperationState {
    var isFailure: Bool { if case .failure = self { true } else { false } }
    var isSuccess: Bool { if case .success = self { true } else { false } }
    var isWorking: Bool { if case .working = self { true } else { false } }
    var isTerminal: Bool { if case .success = self { true } else if case .failure = self { true } else { false } }
    var message: String? {
        switch self {
        case .working(let message), .success(let message), .failure(let message): message
        case .idle: nil
        }
    }
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
    private let openProviderSettings: @MainActor () -> Void
    private let openTraceBrowser: @MainActor (String?) -> Void
    private let onCompleted: @MainActor () -> Void

    init(
        store: SnapshotStore,
        openProviderSettings: @escaping @MainActor () -> Void,
        openTraceBrowser: @escaping @MainActor (String?) -> Void,
        onCompleted: @escaping @MainActor () -> Void = {}
    ) {
        self.store = store
        self.openProviderSettings = openProviderSettings
        self.openTraceBrowser = openTraceBrowser
        self.onCompleted = onCompleted
    }

    func show() {
        if window == nil {
            let model = OnboardingModel(
                store: store, openProviderSettings: openProviderSettings,
                openTraceBrowser: openTraceBrowser,
                finish: { [weak self] in
                    self?.window?.close()
                    self?.onCompleted()
                })
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
