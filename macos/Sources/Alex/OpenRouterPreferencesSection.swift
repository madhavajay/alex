import AppKit
import SwiftUI
import AlexCore

/// Preferences → OpenRouter. OpenRouter exposes hundreds of models; injecting
/// all of them makes every harness picker unusable, so exposure is curated. The
/// pane is a two-list transfer: LEFT = the full known catalog (searchable,
/// alphabetical), RIGHT = the exposed set that is actually injected into
/// connected harnesses. The API key is set/replaced above, masked with a Show
/// toggle (the daemon never returns a stored key, so the field only captures a
/// replacement).
struct OpenRouterPreferencesSection: View {
    let store: SnapshotStore

    // Key entry (write-only: the daemon never hands a stored key back).
    @State private var keyDraft = ""
    @State private var httpReferer = ""
    @State private var xTitle = ""
    @State private var revealKey = false
    @State private var isSavingKey = false
    @State private var keyResult: String?

    // Transfer lists.
    @State private var catalog: [String] = []
    @State private var exposed: [String] = []
    @State private var search = ""
    @State private var selectedAvailable: Set<String> = []
    @State private var selectedExposed: Set<String> = []

    @State private var isLoading = true
    @State private var isSaving = false
    @State private var error: String?
    @State private var saveResult: String?

    private var hasKey: Bool {
        ProviderPresentation.hasAccount(for: "openrouter", in: store.accounts)
    }

    private var creditBalanceText: String? {
        ProviderPresentation.creditBalanceText(
            store.limits.first { $0.provider == "openrouter" })
    }

    private var availableModels: [String] {
        OpenRouterCuration.available(catalog: catalog, exposed: exposed, search: search)
    }

    var body: some View {
        VStack(spacing: 0) {
            paneHeader
            ScrollView {
                VStack(alignment: .leading, spacing: 20) {
                    keySection
                    transferSection
                }
                .padding(24)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        .task { await load() }
    }

    // MARK: Header

    private var paneHeader: some View {
        HStack(spacing: 10) {
            openRouterLogo
                .resizable()
                .scaledToFit()
                .frame(width: 26, height: 26)
            VStack(alignment: .leading, spacing: 1) {
                Text("OpenRouter")
                    .font(AlexTheme.Fonts.panelTitle)
                    .foregroundStyle(AlexTheme.Colors.foreground)
                Text("Curate which OpenRouter models your harnesses see")
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

    // MARK: API key

    private var keySection: some View {
        VStack(alignment: .leading, spacing: 8) {
            SectionLabel(text: "API key", style: .prominent)
            HStack(spacing: 6) {
                StatusDot(
                    tint: hasKey ? AlexTheme.Colors.success : AlexTheme.Colors.textFaint, size: 7)
                Text(hasKey ? "A key is configured" : "No key configured yet")
                    .font(.system(size: 12, weight: .medium))
                    .foregroundStyle(
                        hasKey ? AlexTheme.Colors.success : AlexTheme.Colors.textSecondary)
            }
            if let creditBalanceText {
                HStack(spacing: 6) {
                    Text("💰")
                    Text(creditBalanceText)
                        .font(AlexTheme.Fonts.mono(11, weight: .semibold))
                        .foregroundStyle(AlexTheme.Colors.success)
                }
            }
            Text("OpenRouter uses a long-lived API key, not OAuth. The key is sent only to your local Alex daemon for encrypted vault storage; it is never displayed back.")
                .font(.system(size: 11))
                .foregroundStyle(AlexTheme.Colors.textTertiary)
            HStack(spacing: 8) {
                Group {
                    if revealKey {
                        TextField("Enter API key to save or replace", text: $keyDraft)
                    } else {
                        SecureField("Enter API key to save or replace", text: $keyDraft)
                    }
                }
                .textFieldStyle(.roundedBorder)
                .font(AlexTheme.Fonts.mono(11))
                .frame(maxWidth: 340)
                PillButton(
                    title: revealKey ? "Hide" : "Show", variant: .bordered,
                    systemImage: revealKey ? "eye.slash" : "eye"
                ) { revealKey.toggle() }
                PillButton(
                    title: isSavingKey ? "Saving…" : "Save key", variant: .primary,
                    isEnabled: !isSavingKey
                        && !keyDraft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
                    isBusy: isSavingKey
                ) { Task { await saveKey() } }
                if let keyResult {
                    Text(keyResult)
                        .font(.system(size: 11))
                        .foregroundStyle(
                            keyResult.hasPrefix("Save failed")
                                ? AlexTheme.Colors.destructive : AlexTheme.Colors.success)
                }
                Spacer()
            }
            HStack(spacing: 8) {
                TextField("HTTP-Referer (optional)", text: $httpReferer)
                    .textFieldStyle(.roundedBorder)
                    .font(AlexTheme.Fonts.mono(11))
                    .frame(maxWidth: 200)
                TextField("X-Title (optional)", text: $xTitle)
                    .textFieldStyle(.roundedBorder)
                    .font(AlexTheme.Fonts.mono(11))
                    .frame(maxWidth: 140)
            }
        }
    }

    // MARK: Two-list transfer

    private var transferSection: some View {
        VStack(alignment: .leading, spacing: 10) {
            SectionLabel(text: "Exposed models", style: .prominent)
            Text("The right list is exactly what shows up in your harnesses — these models are injected as openrouter/<id> (and alex/openrouter/<id>) when a harness connects or refreshes. The left list is OpenRouter's full catalog. Add the few you actually use; the rest stay hidden so the picker is scrollable.")
                .font(.system(size: 11))
                .foregroundStyle(AlexTheme.Colors.textTertiary)

            if let error {
                Text(error)
                    .font(.system(size: 11))
                    .foregroundStyle(AlexTheme.Colors.destructive)
            }

            if isLoading {
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text("Loading catalog…")
                        .font(.system(size: 11))
                        .foregroundStyle(AlexTheme.Colors.textTertiary)
                }
                .frame(maxWidth: .infinity, alignment: .center)
                .padding(.vertical, 24)
            } else {
                HStack(alignment: .top, spacing: 12) {
                    knownColumn
                    transferControls
                    exposedColumn
                }
            }
        }
    }

    private var knownColumn: some View {
        VStack(alignment: .leading, spacing: 8) {
            columnHeader(
                title: "Known models",
                count: availableModels.count,
                subtitle: "OpenRouter catalog")
            SearchField(text: $search, placeholder: "Search models…")
            listBox {
                if availableModels.isEmpty {
                    emptyListLabel(
                        catalog.isEmpty
                            ? "Catalog unavailable. Save a valid API key, then reopen this pane."
                            : "No matches.")
                } else {
                    List(availableModels, id: \.self, selection: $selectedAvailable) { id in
                        modelRow(id)
                    }
                    .listStyle(.plain)
                    .scrollContentBackground(.hidden)
                }
            }
        }
        .frame(maxWidth: .infinity)
    }

    private var exposedColumn: some View {
        VStack(alignment: .leading, spacing: 8) {
            columnHeader(
                title: "Exposed to harnesses",
                count: exposed.count,
                subtitle: "Injected on connect")
            // Height-match the search box in the known column.
            Color.clear.frame(height: 28)
            listBox {
                if exposed.isEmpty {
                    emptyListLabel("Nothing exposed. Add models from the left so your harnesses can pick them.")
                } else {
                    List(
                        OpenRouterCuration.exposedSorted(exposed), id: \.self,
                        selection: $selectedExposed
                    ) { id in
                        modelRow(id)
                    }
                    .listStyle(.plain)
                    .scrollContentBackground(.hidden)
                }
            }
        }
        .frame(maxWidth: .infinity)
    }

    private var transferControls: some View {
        VStack(spacing: 10) {
            Spacer().frame(height: 60)
            PillButton(
                title: "Add", variant: .primary, systemImage: "arrow.right",
                isEnabled: !selectedAvailable.isEmpty && !isSaving, isBusy: isSaving
            ) { Task { await addSelected() } }
            PillButton(
                title: "Remove", variant: .danger, systemImage: "arrow.left",
                isEnabled: !selectedExposed.isEmpty && !isSaving
            ) { Task { await removeSelected() } }
            if isSaving {
                ProgressView().controlSize(.small)
            }
            if let saveResult {
                Text(saveResult)
                    .font(.system(size: 10))
                    .multilineTextAlignment(.center)
                    .foregroundStyle(
                        saveResult.hasPrefix("Save failed")
                            ? AlexTheme.Colors.destructive : AlexTheme.Colors.success)
            }
        }
        .frame(width: 96)
    }

    private func columnHeader(title: String, count: Int, subtitle: String) -> some View {
        VStack(alignment: .leading, spacing: 1) {
            HStack(spacing: 6) {
                Text(title)
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(AlexTheme.Colors.foreground)
                Text("\(count)")
                    .font(AlexTheme.Fonts.mono(10, weight: .medium))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
                    .padding(.horizontal, 6)
                    .padding(.vertical, 1)
                    .background(
                        RoundedRectangle(cornerRadius: AlexTheme.Radius.xs)
                            .fill(AlexTheme.Colors.overlay(0.08)))
            }
            Text(subtitle)
                .font(.system(size: 10))
                .foregroundStyle(AlexTheme.Colors.textTertiary)
        }
    }

    private func modelRow(_ id: String) -> some View {
        Text(id)
            .font(AlexTheme.Fonts.mono(11))
            .foregroundStyle(AlexTheme.Colors.foreground)
            .lineLimit(1)
            .truncationMode(.middle)
    }

    private func listBox<Content: View>(@ViewBuilder _ content: () -> Content) -> some View {
        content()
            .frame(height: 260)
            .frame(maxWidth: .infinity)
            .background(
                RoundedRectangle(cornerRadius: AlexTheme.Radius.md)
                    .fill(AlexTheme.Colors.overlay(0.03)))
            .overlay(
                RoundedRectangle(cornerRadius: AlexTheme.Radius.md)
                    .strokeBorder(AlexTheme.Colors.cardBorder))
    }

    private func emptyListLabel(_ text: String) -> some View {
        Text(text)
            .font(.system(size: 11))
            .foregroundStyle(AlexTheme.Colors.textTertiary)
            .multilineTextAlignment(.center)
            .padding(16)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private var openRouterLogo: Image {
        // Mirror the Exo pane: never Bundle.module (it traps in the
        // hand-packaged .app); use the shared safe resolver with an SF Symbol
        // fallback.
        if let url = HarnessIconLoader.resourceBundle?.url(
            forResource: "openrouter", withExtension: "png", subdirectory: "logos"),
            let image = NSImage(contentsOf: url), image.isValid {
            Image(nsImage: image)
        } else {
            Image(systemName: "arrow.triangle.branch")
        }
    }

    // MARK: Data

    private func client() -> AlexClient? {
        guard let config = store.config ?? DaemonDiscovery.load() else { return nil }
        return AlexClient(config: config)
    }

    private func load() async {
        isLoading = true
        defer { isLoading = false }
        guard let client = client() else {
            error = "No Alex daemon configuration was found."
            return
        }
        do {
            let response = try await client.openRouterExposed()
            exposed = OpenRouterCuration.exposedSorted(response.exposed)
            catalog = response.available
            error = nil
        } catch is CancellationError {
        } catch {
            self.error = error.localizedDescription
        }
    }

    private func saveKey() async {
        guard let client = client() else {
            keyResult = "Save failed: no daemon configuration"
            return
        }
        isSavingKey = true
        defer { isSavingKey = false }
        let clean = keyDraft.trimmingCharacters(in: .whitespacesAndNewlines)
        let cleanReferer = httpReferer.trimmingCharacters(in: .whitespacesAndNewlines)
        let cleanTitle = xTitle.trimmingCharacters(in: .whitespacesAndNewlines)
        do {
            try await client.setOpenRouterKey(
                clean,
                httpReferer: cleanReferer.isEmpty ? nil : cleanReferer,
                xTitle: cleanTitle.isEmpty ? nil : cleanTitle)
            keyDraft = ""
            revealKey = false
            keyResult = "Key saved"
            await store.refresh()
            // A freshly-authorized key unlocks the catalog fetch.
            await load()
        } catch is CancellationError {
        } catch {
            keyResult = "Save failed: \(error.localizedDescription)"
        }
    }

    private func addSelected() async {
        let next = selectedAvailable.reduce(exposed) { OpenRouterCuration.adding($1, to: $0) }
        await persist(next) {
            selectedAvailable = []
        }
    }

    private func removeSelected() async {
        let next = selectedExposed.reduce(exposed) { OpenRouterCuration.removing($1, from: $0) }
        await persist(next) {
            selectedExposed = []
        }
    }

    /// Optimistically apply the new exposed list, POST it, and reconcile with
    /// the daemon's normalized response (reverting on failure).
    private func persist(_ next: [String], onSuccess: @escaping () -> Void) async {
        guard let client = client() else {
            saveResult = "Save failed: no daemon configuration"
            return
        }
        let previous = exposed
        isSaving = true
        exposed = next
        defer { isSaving = false }
        do {
            let saved = try await client.updateOpenRouterExposed(next)
            exposed = OpenRouterCuration.exposedSorted(saved)
            saveResult = "Saved — reconnect a harness to pick these up"
            onSuccess()
        } catch is CancellationError {
            exposed = previous
        } catch {
            exposed = previous
            saveResult = "Save failed: \(error.localizedDescription)"
        }
    }
}
