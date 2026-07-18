import AppKit
import SwiftUI
import AlexandriaBarCore

/// Providers → Exo detail. Exo is a local (or LAN/Tailscale)
/// OpenAI-compatible inference cluster; checked models are published through
/// Alexandria.
struct ExoPreferencesSection: View {
    let store: SnapshotStore

    @State private var endpoint = "http://localhost:52415"
    @State private var models: [ExoModel] = []
    @State private var status: ExoStatus?
    @State private var isLoading = true
    @State private var isChecking = false
    @State private var isSaving = false
    @State private var error: String?
    @State private var result: String?

    var body: some View {
        VStack(spacing: 0) {
            paneHeader
            ScrollView {
                VStack(alignment: .leading, spacing: 18) {
                    endpointSection
                    statusSection
                    if status?.running == true { modelSection }
                }
                .padding(24)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        .task { await load() }
    }

    private var paneHeader: some View {
        HStack(spacing: 10) {
            exoLogo
                .resizable()
                .scaledToFit()
                .frame(width: 26, height: 26)
            VStack(alignment: .leading, spacing: 1) {
                Text("Exo")
                    .font(AlexTheme.Fonts.panelTitle)
                    .foregroundStyle(AlexTheme.Colors.foreground)
                Text("Local distributed inference")
                    .font(.system(size: 12))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
            }
            Spacer()
        }
        .padding(.horizontal, 24)
        .padding(.vertical, 14)
        .overlay(alignment: .bottom) {
            Rectangle().fill(AlexTheme.Colors.overlay(0.06)).frame(height: 1)
                .padding(.horizontal, 24)
        }
    }

    private var endpointSection: some View {
        VStack(alignment: .leading, spacing: 8) {
            SectionLabel(text: "Endpoint URL", style: .prominent)
            TextField("http://localhost:52415", text: $endpoint)
                .settingsField()
            Text("Use localhost or a LAN/Tailscale address, for example http://192.168.1.150:52415.")
                .font(.system(size: 11))
                .foregroundStyle(AlexTheme.Colors.textTertiary)
        }
    }

    @ViewBuilder
    private var statusSection: some View {
        HStack(alignment: .center, spacing: 10) {
            PillButton(
                title: isChecking ? "Checking…" : "Check that Exo is running",
                variant: .bordered, systemImage: "heart.text.square",
                isEnabled: !isChecking && !isSaving, isBusy: isChecking
            ) { Task { await check() } }
            if let status, status.running {
                Text("✓ Running — \(status.modelCount) models available")
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(AlexTheme.Colors.success)
            } else if !isLoading {
                Text("✗ Exo not detected")
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(AlexTheme.Colors.destructive)
            }
            Spacer()
        }

        if status?.running != true && !isLoading {
            VStack(alignment: .leading, spacing: 5) {
                if let error { Text(error) .font(.system(size: 11)).foregroundStyle(AlexTheme.Colors.textTertiary) }
                HStack(spacing: 10) {
                    Link("Install Exo", destination: URL(string: "https://assets.exolabs.net/EXO-latest.dmg")!)
                    Link("GitHub", destination: URL(string: "https://github.com/exo-explore/exo")!)
                }
                .font(.system(size: 12, weight: .medium))
            }
            .padding(12)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(RoundedRectangle(cornerRadius: AlexTheme.Radius.md)
                .fill(AlexTheme.Colors.destructive.opacity(0.07)))
        }
    }

    private var modelSection: some View {
        VStack(alignment: .leading, spacing: 10) {
            SectionLabel(text: "Models to expose", style: .prominent)
            Text("Checked models become callable as alex/<model> by any harness pointed at Alexandria. They are also available as exo/<model>.")
                .font(.system(size: 11))
                .foregroundStyle(AlexTheme.Colors.textTertiary)
            ForEach($models) { $model in
                Toggle(isOn: $model.enabled) {
                    VStack(alignment: .leading, spacing: 2) {
                        HStack(spacing: 6) {
                            Text(model.name).font(.system(size: 12, weight: .medium))
                            if model.running == true {
                                Text("Running").font(.system(size: 10, weight: .semibold))
                                    .foregroundStyle(AlexTheme.Colors.success)
                            }
                        }
                        Text(modelDetail(model))
                            .font(AlexTheme.Fonts.metaMono)
                            .foregroundStyle(AlexTheme.Colors.textTertiary)
                    }
                }
                .settingsSwitch()
                .padding(.vertical, 3)
                RowDivider()
            }
            HStack(spacing: 8) {
                PillButton(title: isSaving ? "Saving…" : "Save exposed models", variant: .primary,
                    isEnabled: !isSaving, isBusy: isSaving) { Task { await save() } }
                if let result {
                    Text(result).font(.system(size: 11))
                        .foregroundStyle(result.hasPrefix("Save failed") ? AlexTheme.Colors.destructive : AlexTheme.Colors.success)
                }
                Spacer()
            }
        }
    }

    private var exoLogo: Image {
        // NEVER Bundle.module here — it traps when the SwiftPM resource bundle
        // can't be resolved in the hand-packaged .app and took the whole app
        // down (0.1.27-beta.4 crash on opening this pane). Use the shared safe
        // resolver, fall back to an SF Symbol when the asset is unavailable.
        if let url = HarnessIconLoader.resourceBundle?.url(
            forResource: "exo", withExtension: "png", subdirectory: "logos"),
           let image = NSImage(contentsOf: url), image.isValid {
            Image(nsImage: image)
        } else {
            Image(systemName: "cpu")
        }
    }

    private func modelDetail(_ model: ExoModel) -> String {
        [model.id, model.family, model.quantization,
         model.contextLength.map { "\($0) context" }]
            .compactMap { $0 }.joined(separator: " · ")
    }

    private func client() -> AlexandriaClient? {
        guard let config = store.config ?? DaemonDiscovery.load() else { return nil }
        return AlexandriaClient(config: config)
    }

    private func load() async {
        isLoading = true
        defer { isLoading = false }
        guard let client = client() else { error = "No Alexandria daemon configuration was found."; return }
        do {
            let config = try await client.exoConfig()
            endpoint = config.url
            try await refreshStatus(client)
        } catch is CancellationError {
        } catch {
            self.error = error.localizedDescription
        }
    }

    private func check() async {
        guard let client = client() else { error = "No Alexandria daemon configuration was found."; return }
        isChecking = true
        error = nil
        defer { isChecking = false }
        do {
            // Apply the field first, so the check always probes the URL shown.
            _ = try await client.updateExoConfig(ExoConfig(
                url: endpoint, enabledModels: models.filter(\.enabled).map(\.id)))
            try await refreshStatus(client)
        } catch is CancellationError {
        } catch {
            self.status = nil
            self.error = error.localizedDescription
        }
    }

    private func refreshStatus(_ client: AlexandriaClient) async throws {
        let current = try await client.exoStatus()
        status = current
        error = current.error
        if current.running { models = try await client.exoModels() }
    }

    private func save() async {
        guard let client = client() else { result = "Save failed: no daemon configuration"; return }
        isSaving = true
        defer { isSaving = false }
        do {
            _ = try await client.updateExoConfig(ExoConfig(
                url: endpoint, enabledModels: models.filter(\.enabled).map(\.id)))
            result = "Exo settings saved"
        } catch is CancellationError {
        } catch {
            result = "Save failed: \(error.localizedDescription)"
        }
    }
}
