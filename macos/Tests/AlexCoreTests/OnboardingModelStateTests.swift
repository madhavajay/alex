#if os(macOS)
import Foundation
import Testing
@testable import Alex
@testable import AlexCore

@MainActor
@Suite struct OnboardingModelStateTests {
    private func makeModel() -> OnboardingModel {
        let model = OnboardingModel(
            store: SnapshotStore(),
            openProviderSettings: {},
            openTraceBrowser: { _ in },
            finish: {})
        // Unit tests do not run the view's discovery task. Mark discovery as
        // complete unless a test is specifically exercising detected logins.
        model.credentialImportCandidatesLoaded = true
        return model
    }

    private func account(
        id: String = "openai-oauth-acct", provider: String = "openai"
    ) throws -> Account {
        try JSONDecoder().decode(Account.self, from: Data(
            """
            {"id":"\(id)","provider":"\(provider)","name":"default","kind":"oauth","paused":false,"status":"active"}
            """.utf8))
    }

    @Test func backwardAndForwardNavigationAllowsAProviderReselection() {
        let model = makeModel()
        model.step = 2
        model.selectedProvider = "openai"
        model.selectedProviderAccountID = "stale-account"
        model.selectedHarness = "pi"
        model.harnessState = .success("36 models ready")
        model.traceState = .success("stale request")
        model.traceCheckRunning = true
        model.troubleshootExpanded = true

        model.back()

        #expect(model.step == 1)
        #expect(model.selectedProvider == nil)
        #expect(model.selectedProviderAccountID == nil)
        #expect(model.traceState == .idle)
        #expect(!model.traceCheckRunning)
        #expect(!model.troubleshootExpanded)

        model.selectedProvider = "anthropic"
        model.selectedProviderAccountID = "new-account"
        model.providerState = .success("new@example.com")
        model.next()
        #expect(model.step == 2)
        #expect(model.selectedProvider == "anthropic")
        #expect(model.selectedProviderAccountID == "new-account")
    }

    @Test func changingHarnessKeepsProviderButClearsStaleRequestState() {
        let model = makeModel()
        model.selectedProvider = "openai"
        model.selectedProviderAccountID = "account"
        model.selectedHarness = "pi"
        model.harnessPlanState = .success("ready")
        model.harnessState = .success("36 models ready")
        model.connectedModelsCount = 36
        model.traceState = .failure("old failure")
        model.traceCheckRunning = true
        model.troubleshootExpanded = true
        model.checksRunning = true

        model.changeHarness()

        #expect(model.selectedProvider == "openai")
        #expect(model.selectedProviderAccountID == "account")
        #expect(model.selectedHarness == nil)
        #expect(model.harnessPlanState == .idle)
        #expect(model.harnessState == .idle)
        #expect(model.connectedModelsCount == 0)
        #expect(model.traceState == .idle)
        #expect(!model.traceCheckRunning)
        #expect(!model.troubleshootExpanded)
        #expect(!model.checksRunning)
    }

    @Test func existingAndNewAccountChoicesAreBothReusable() async throws {
        let model = makeModel()
        model.selectedProvider = "openai"
        model.traceState = .failure("stale request")

        model.useExistingProviderAccount(try account())
        while model.providerState == .working("Using connected account…") {
            await Task.yield()
        }
        #expect(model.selectedProviderAccountID == "openai-oauth-acct")
        #expect(model.providerState == .success("default"))

        model.chooseProvider("openrouter")
        #expect(model.selectedProvider == "openrouter")
        #expect(model.selectedProviderAccountID == nil)
        #expect(model.addingProviderAccount)
        #expect(model.traceState == .idle)
    }

    @Test func providerTilesCanSwitchProviderWhileAuthorizationContentIsVisible() {
        let model = makeModel()
        model.selectedProvider = "gemini"
        model.authModel = AuthFlowModel(
            provider: "gemini", accountName: nil, autoIdentity: true,
            store: model.store)
        model.providerState = .working("Waiting for authorization…")
        model.traceState = .failure("stale request")

        model.chooseProvider("openrouter")

        #expect(model.selectedProvider == "openrouter")
        #expect(model.authModel == nil)
        #expect(model.addingProviderAccount)
        #expect(model.providerState == .idle)
        #expect(model.traceState == .idle)
    }

    @Test func detectedCredentialRequiresExplicitImportChoice() throws {
        let model = makeModel()
        let response = try JSONDecoder().decode(
            CredentialImportCandidatesResponse.self,
            from: Data(
                #"{"candidates":[{"source":"amp","provider":"amp","label":"Amp","kind":"api_key","source_path":"~/.local/share/amp/secrets.json","requires_confirmation":true}],"requires_confirmation":true}"#.utf8))
        model.credentialImportCandidates = response.candidates
        model.selectedCredentialImports = Set(response.candidates.map(\.source))

        model.chooseProvider("amp")

        #expect(model.selectedProvider == "amp")
        #expect(model.authModel == nil)
        #expect(!model.addingProviderAccount)
        #expect(model.selectedCredentialImports == ["amp"])
    }

    @Test func experimentalPamStepIsHiddenFromOnboarding() {
        #expect(!OnboardingModel.stepTitles.contains { $0.localizedCaseInsensitiveContains("pam") })
        #expect(OnboardingModel.stepTitles.last == "Beyond single provider")
    }

    @Test func networkStepFollowsCredentialsAndPrecedesNotifications() {
        #expect(OnboardingModel.stepTitles.count == 8)
        #expect(OnboardingModel.stepTitles == [
            "Meet Alex", "Pick a provider", "Connect and test",
            "Credentials for compatible apps", "Network", "Never lose a login",
            "Keep your agents running", "Beyond single provider",
        ])

        let model = makeModel()
        model.step = 3
        model.next()
        #expect(model.step == 4)
        #expect(model.canAdvance)
        model.next()
        #expect(model.step == 5)
        model.back()
        #expect(model.step == 4)
    }

    @Test func supportedOnboardingWidthUsesThreeProviderColumnsAndRoomyChips() {
        #expect(OnboardingUILayout.contentWidth == 700)
        #expect(OnboardingUILayout.adaptiveColumnCount(
            availableWidth: OnboardingUILayout.contentWidth,
            minimumWidth: OnboardingUILayout.providerTileMinimumWidth,
            spacing: 10) == 3)
        #expect(OnboardingUILayout.adaptiveColumnCount(
            availableWidth: OnboardingUILayout.contentWidth,
            minimumWidth: OnboardingUILayout.compatibleAppChipMinimumWidth,
            spacing: 7) == 6)
    }

    @Test func settingsResetClosesSettingsBeforeLaunchingOnboarding() {
        var events: [String] = []

        SettingsResetOnboardingTransition.perform(
            closeSettings: { events.append("close settings") },
            launchOnboarding: { events.append("launch onboarding") })

        #expect(events == ["close settings", "launch onboarding"])
    }

    @Test func checkForRequestActivelyRechecksAfterAStaleFailure() async {
        let model = makeModel()
        model.selectedHarness = "kimi"
        model.harnessState = .success("models ready")
        model.traceState = .failure("stale provider failure")

        model.checkForTrace()
        while model.traceCheckRunning { await Task.yield() }

        #expect(model.traceState == .working(
            "No new matching request yet — run the command, then check again."))
    }
}
#endif
