import SwiftUI
import AlexCore

/// Preferences → Protection. This keeps policy edits local until Save so an
/// experiment can be reviewed or reverted before changing daemon behavior.
struct ProtectionPreferencesSection: View {
    let store: SnapshotStore

    @State private var enabled = false
    @State private var rerouteOnAuth = false
    @State private var autoReturn = false
    @State private var retries = 0
    @State private var equivalencies: [ProtectionEquivalencyRow] = []
    @State private var isLoading = true
    @State private var isSaving = false
    @State private var loadError: String?
    @State private var actionResult: String?

    private static let providers = ProviderInfo.supportedProviders

    var body: some View {
        VStack(spacing: 0) {
            paneHeader
            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    if isLoading {
                        loadingState
                    } else if let loadError {
                        errorState(loadError)
                    } else {
                        protectionSections
                    }
                }
                .padding(.horizontal, 24)
                .padding(.bottom, 20)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        .task { await load() }
    }

    private var paneHeader: some View {
        VStack(alignment: .leading, spacing: 0) {
            VStack(alignment: .leading, spacing: 1) {
                Text("Failover")
                    .font(AlexTheme.Fonts.panelTitle)
                    .foregroundStyle(AlexTheme.Colors.foreground)
                Text("Provider fail-over and model equivalencies")
                    .font(.system(size: 12))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
            }
            .padding(.horizontal, 24)
            .padding(.top, 16)
            .padding(.bottom, 12)
            .frame(maxWidth: .infinity, alignment: .leading)
            Rectangle()
                .fill(AlexTheme.Colors.overlay(0.06))
                .frame(height: 1)
                .padding(.horizontal, 24)
        }
    }

    private var loadingState: some View {
        HStack(spacing: 8) {
            ProgressView().controlSize(.small)
            Text("Loading protection policy…")
        }
        .font(.system(size: 12))
        .foregroundStyle(AlexTheme.Colors.textSecondary)
        .padding(.vertical, 28)
        .frame(maxWidth: .infinity)
    }

    private func errorState(_ message: String) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Protection policy unavailable")
                .font(.system(size: 12, weight: .semibold))
                .foregroundStyle(AlexTheme.Colors.destructive)
            Text(message)
                .font(AlexTheme.Fonts.metaMono)
                .foregroundStyle(AlexTheme.Colors.textSecondary)
                .textSelection(.enabled)
            PillButton(title: "Try again", variant: .bordered) {
                Task { await load() }
            }
        }
        .padding(.vertical, 24)
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    @ViewBuilder
    private var protectionSections: some View {
        SectionLabel(text: "Fail-over")
            .settingsSectionSpacing()
        SettingRow(label: "Enable protection", hint: "Allow configured retries and provider swaps") {
            Toggle("", isOn: $enabled)
                .settingsSwitch()
        }
        RowDivider()
        SettingRow(
            label: "Subscription fail-over",
            hint: "Fail over to another provider when a subscription is logged out (opt-in)"
        ) {
            Toggle("", isOn: $rerouteOnAuth)
                .settingsSwitch()
        }
        RowDivider()
        SettingRow(label: "Retries", hint: "Retry a failed request before failing over") {
            Stepper(value: $retries, in: 0...10) {
                Text("\(retries)")
                    .font(AlexTheme.Fonts.metaMono)
                    .frame(minWidth: 16, alignment: .trailing)
            }
            .frame(width: 96)
        }
        RowDivider()
        SettingRow(label: "Return automatically", hint: "Use the original provider again when it is healthy") {
            Toggle("", isOn: $autoReturn)
                .settingsSwitch()
        }

        SectionLabel(text: "Model equivalencies")
            .settingsSectionSpacing()
        SettingCaption("Reasoning effort is preserved across the swap.")
        equivalencyGrid
        HStack(spacing: 8) {
            PillButton(title: "Add equivalency", variant: .bordered, systemImage: "plus") {
                equivalencies.append(ProtectionEquivalencyRow())
            }
            PillButton(title: "Load preset: Anthropic ⇄ OpenAI", variant: .bordered) {
                loadAnthropicOpenAIPreset()
            }
            Spacer()
        }
        .padding(.top, 10)

        HStack(spacing: 8) {
            PillButton(
                title: isSaving ? "Saving…" : "Save",
                variant: .primary,
                isEnabled: !isSaving,
                isBusy: isSaving
            ) {
                Task { await save() }
            }
            PillButton(title: "Revert", variant: .bordered, isEnabled: !isSaving) {
                Task { await load() }
            }
            if let actionResult {
                Text(actionResult)
                    .font(.system(size: 11))
                    .foregroundStyle(actionResult.hasPrefix("Save failed")
                        ? AlexTheme.Colors.destructive : AlexTheme.Colors.success)
            }
            Spacer()
        }
        .padding(.top, 22)
    }

    private var equivalencyGrid: some View {
        VStack(alignment: .leading, spacing: 7) {
            HStack(spacing: 8) {
                Text("Model")
                    .frame(minWidth: 110, maxWidth: .infinity, alignment: .leading)
                Text("")
                    .frame(width: 10)
                Text("Provider")
                    .frame(width: 100, alignment: .leading)
                Text("")
                    .frame(width: 6)
                Text("Equivalent model")
                    .frame(minWidth: 110, maxWidth: .infinity, alignment: .leading)
                Text("")
                    .frame(width: 16)
            }
            .font(.system(size: 10, weight: .semibold))
            .foregroundStyle(AlexTheme.Colors.textTertiary)
            .padding(.top, 10)

            ForEach($equivalencies) { $row in
                HStack(spacing: 8) {
                    TextField("model", text: $row.model)
                        .settingsField(width: nil)
                        .frame(minWidth: 110, maxWidth: .infinity)
                    Text("→")
                        .foregroundStyle(AlexTheme.Colors.textTertiary)
                        .frame(width: 10)
                    Picker("Provider", selection: $row.provider) {
                        ForEach(Self.providers, id: \.self) { provider in
                            Text(provider).tag(provider)
                        }
                    }
                    .settingsPicker()
                    .frame(width: 100)
                    Text(":")
                        .foregroundStyle(AlexTheme.Colors.textTertiary)
                        .frame(width: 6)
                    TextField("equivalent model", text: $row.equivalentModel)
                        .settingsField(width: nil)
                        .frame(minWidth: 110, maxWidth: .infinity)
                    Button {
                        removeEquivalency(row.id)
                    } label: {
                        Image(systemName: "minus.circle")
                    }
                    .buttonStyle(.plain)
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
                    .frame(width: 16)
                    .help("Remove equivalency")
                }
            }
        }
        .padding(.bottom, 4)
    }

    private var policy: ProtectionPolicy {
        var map: [String: [String: String]] = [:]
        for row in equivalencies {
            let model = row.model.trimmingCharacters(in: .whitespacesAndNewlines)
            let equivalent = row.equivalentModel.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !model.isEmpty, !equivalent.isEmpty else { continue }
            map[model, default: [:]][row.provider] = equivalent
        }
        return ProtectionPolicy(
            enabled: enabled,
            rerouteOnAuth: rerouteOnAuth,
            retries: retries,
            autoReturn: autoReturn,
            equivalencies: map)
    }

    private func load() async {
        isLoading = true
        loadError = nil
        actionResult = nil
        defer { isLoading = false }
        guard let config = store.config ?? DaemonDiscovery.load() else {
            loadError = "No Alex daemon configuration was found."
            return
        }
        do {
            apply(try await AlexClient(config: config).protectionPolicy())
        } catch is CancellationError {
            return
        } catch {
            loadError = error.localizedDescription
        }
    }

    private func save() async {
        guard let config = store.config ?? DaemonDiscovery.load() else {
            actionResult = "Save failed: no daemon configuration"
            return
        }
        isSaving = true
        defer { isSaving = false }
        do {
            try await AlexClient(config: config).updateProtectionPolicy(policy)
            actionResult = "Protection policy saved"
        } catch is CancellationError {
            return
        } catch {
            actionResult = "Save failed: \(error.localizedDescription)"
        }
    }

    private func apply(_ policy: ProtectionPolicy) {
        enabled = policy.enabled
        rerouteOnAuth = policy.rerouteOnAuth
        retries = min(max(policy.retries, 0), 10)
        autoReturn = policy.autoReturn
        equivalencies = policy.equivalencies
            .flatMap { model, targets in
                targets.map { provider, equivalent in
                    ProtectionEquivalencyRow(
                        model: model, provider: provider, equivalentModel: equivalent)
                }
            }
            .sorted { lhs, rhs in
                if lhs.model != rhs.model { return lhs.model < rhs.model }
                if lhs.provider != rhs.provider { return lhs.provider < rhs.provider }
                return lhs.equivalentModel < rhs.equivalentModel
            }
    }

    private func removeEquivalency(_ id: UUID) {
        equivalencies.removeAll { $0.id == id }
    }

    private func loadAnthropicOpenAIPreset() {
        equivalencies = [
            ProtectionEquivalencyRow(
                model: "claude-fable-5", provider: "openai", equivalentModel: "gpt-5.6-sol"),
            ProtectionEquivalencyRow(
                model: "gpt-5.6-sol", provider: "anthropic", equivalentModel: "claude-fable-5"),
        ]
        actionResult = "Preset loaded — save to apply"
    }
}

private struct ProtectionEquivalencyRow: Identifiable {
    let id: UUID
    var model: String
    var provider: String
    var equivalentModel: String

    init(
        id: UUID = UUID(),
        model: String = "",
        provider: String = "anthropic",
        equivalentModel: String = ""
    ) {
        self.id = id
        self.model = model
        self.provider = provider
        self.equivalentModel = equivalentModel
    }
}
