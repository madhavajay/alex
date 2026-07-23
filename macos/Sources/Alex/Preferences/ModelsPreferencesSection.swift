import AppKit
import SwiftUI
import AlexCore

/// Preferences → Models. Cross-provider curation modeled on the OpenRouter
/// pane: LEFT = the live merged catalog grouped by provider (searchable),
/// RIGHT = the curated list actually published to harnesses, in user-chosen
/// order (drag to reorder). Curation off publishes everything. Refresh
/// re-fetches provider catalogs; Check verifies each curated model still
/// exists and flags missing ones in red.
struct ModelsPreferencesSection: View {
    let store: SnapshotStore

    @State private var catalog: [ModelAdminRow] = []
    @State private var curated: [ModelAdminRow] = []
    @State private var curationEnabled = false
    @State private var search = ""
    @State private var isLoading = true
    @State private var isSaving = false
    @State private var isChecking = false
    @State private var error: String?
    @State private var statusLine: String?

    var body: some View {
        VStack(spacing: 0) {
            paneHeader
            if isLoading {
                Spacer()
                ProgressView("Fetching model catalogs…")
                    .controlSize(.small)
                Spacer()
            } else {
                ScrollView {
                    VStack(alignment: .leading, spacing: 16) {
                        curationToggle
                        if curationEnabled {
                            transferColumns
                        }
                        if let error {
                            Text(error)
                                .font(.system(size: 11))
                                .foregroundStyle(AlexTheme.Colors.destructive)
                                .textSelection(.enabled)
                        }
                        if let statusLine {
                            Text(statusLine)
                                .font(.system(size: 11))
                                .foregroundStyle(AlexTheme.Colors.textSecondary)
                        }
                    }
                    .padding(24)
                    .frame(maxWidth: .infinity, alignment: .topLeading)
                }
            }
        }
        .task { await load() }
    }

    // MARK: Header

    private var paneHeader: some View {
        HStack(spacing: 10) {
            Image(systemName: "square.grid.2x2")
                .font(.system(size: 20))
                .foregroundStyle(AlexTheme.Colors.primary)
            VStack(alignment: .leading, spacing: 1) {
                Text("Models")
                    .font(AlexTheme.Fonts.panelTitle)
                    .foregroundStyle(AlexTheme.Colors.foreground)
                Text("Curate which models your harnesses see, across every provider")
                    .font(.system(size: 12))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
            }
            Spacer()
            PillButton(
                title: "Refresh models", variant: .bordered,
                systemImage: "arrow.triangle.2.circlepath",
                isEnabled: !isLoading && !isChecking
            ) {
                Task { await load(status: "Catalogs refreshed.") }
            }
            .help("Re-fetch every provider's model list from its subscription endpoint")
            PillButton(
                title: "Check models", variant: .bordered,
                systemImage: "checkmark.shield",
                isEnabled: curationEnabled && !isChecking && !isLoading,
                isBusy: isChecking
            ) {
                Task { await check() }
            }
            .help("Verify each curated model still exists on its provider; missing models show red")
        }
        .padding(.horizontal, 24)
        .padding(.vertical, 14)
        .overlay(alignment: .bottom) {
            Rectangle().fill(AlexTheme.Colors.overlay(0.06)).frame(height: 1)
                .padding(.horizontal, 24)
        }
    }

    private var curationToggle: some View {
        HStack(spacing: 10) {
            Toggle(isOn: Binding(
                get: { curationEnabled },
                set: { enabled in
                    curationEnabled = enabled
                    if enabled, curated.isEmpty {
                        curated = defaultCuratedSeed()
                    }
                    Task { await save() }
                }
            )) {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Curate the published model list")
                        .font(.system(size: 13, weight: .semibold))
                    Text(curationEnabled
                        ? "Harnesses see only your curated list, in this order."
                        : "Curation is off — harnesses see every model from every connected provider.")
                        .font(.system(size: 11))
                        .foregroundStyle(AlexTheme.Colors.textSecondary)
                }
            }
            .toggleStyle(.switch)
            Spacer()
            if isSaving {
                ProgressView().controlSize(.small)
            }
        }
        .padding(14)
        .background(RoundedRectangle(cornerRadius: AlexTheme.Radius.md)
            .fill(AlexTheme.Colors.card))
        .overlay(RoundedRectangle(cornerRadius: AlexTheme.Radius.md)
            .strokeBorder(AlexTheme.Colors.cardBorder))
    }

    // MARK: Columns

    private var filteredCatalog: [ModelAdminRow] {
        let curatedIDs = Set(curated.map(\.id))
        return catalog.filter { row in
            !curatedIDs.contains(row.id)
                && (search.isEmpty || row.id.localizedCaseInsensitiveContains(search))
        }
    }

    private var catalogByProvider: [(provider: String, rows: [ModelAdminRow])] {
        let groups = Dictionary(grouping: filteredCatalog) { $0.provider ?? "other" }
        return groups
            .map { (provider: $0.key, rows: $0.value) }
            .sorted { $0.provider < $1.provider }
    }

    private var transferColumns: some View {
        HStack(alignment: .top, spacing: 14) {
            availableColumn
            curatedColumn
        }
    }

    private var availableColumn: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("AVAILABLE (\(filteredCatalog.count))")
                .font(AlexTheme.Fonts.metaMono)
                .foregroundStyle(AlexTheme.Colors.textTertiary)
            TextField("Search models", text: $search)
                .textFieldStyle(.roundedBorder)
                .controlSize(.small)
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 2, pinnedViews: .sectionHeaders) {
                    ForEach(catalogByProvider, id: \.provider) { group in
                        Section {
                            ForEach(group.rows) { row in
                                availableRow(row)
                            }
                        } header: {
                            Text(ProviderInfo.displayName(group.provider))
                                .font(AlexTheme.Fonts.metaLabel)
                                .foregroundStyle(AlexTheme.Colors.textFaint)
                                .frame(maxWidth: .infinity, alignment: .leading)
                                .padding(.vertical, 3)
                                .background(AlexTheme.Colors.background)
                        }
                    }
                }
            }
            .frame(minHeight: 260, maxHeight: 380)
        }
        .padding(12)
        .frame(maxWidth: .infinity)
        .background(RoundedRectangle(cornerRadius: AlexTheme.Radius.md)
            .fill(AlexTheme.Colors.card))
        .overlay(RoundedRectangle(cornerRadius: AlexTheme.Radius.md)
            .strokeBorder(AlexTheme.Colors.cardBorder))
    }

    private func availableRow(_ row: ModelAdminRow) -> some View {
        HStack(spacing: 6) {
            Text(row.id)
                .font(AlexTheme.Fonts.mono(11))
                .lineLimit(1)
                .truncationMode(.middle)
            Spacer()
            Button {
                curated.append(row)
                Task { await save() }
            } label: {
                Image(systemName: "plus.circle")
                    .foregroundStyle(AlexTheme.Colors.primary)
            }
            .buttonStyle(.plain)
            .help("Add to curated list")
        }
        .padding(.vertical, 2)
    }

    private var curatedColumn: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("CURATED · PUBLISHED TO HARNESSES (\(curated.count))")
                .font(AlexTheme.Fonts.metaMono)
                .foregroundStyle(AlexTheme.Colors.textTertiary)
            Text("Drag to reorder — favourites at the top appear first in every picker.")
                .font(.system(size: 10.5))
                .foregroundStyle(AlexTheme.Colors.textFaint)
            List {
                ForEach(curated) { row in
                    curatedRow(row)
                        .listRowInsets(EdgeInsets(top: 2, leading: 4, bottom: 2, trailing: 4))
                        .listRowSeparator(.hidden)
                        .listRowBackground(Color.clear)
                }
                .onMove { indices, destination in
                    curated.move(fromOffsets: indices, toOffset: destination)
                    Task { await save() }
                }
            }
            .listStyle(.plain)
            .scrollContentBackground(.hidden)
            .frame(minHeight: 280, maxHeight: 400)
        }
        .padding(12)
        .frame(maxWidth: .infinity)
        .background(RoundedRectangle(cornerRadius: AlexTheme.Radius.md)
            .fill(AlexTheme.Colors.card))
        .overlay(RoundedRectangle(cornerRadius: AlexTheme.Radius.md)
            .strokeBorder(AlexTheme.Colors.cardBorder))
    }

    private func curatedRow(_ row: ModelAdminRow) -> some View {
        HStack(spacing: 6) {
            Image(systemName: "line.3.horizontal")
                .font(.system(size: 9))
                .foregroundStyle(AlexTheme.Colors.textFaintest)
            Text(row.id)
                .font(AlexTheme.Fonts.mono(11))
                .foregroundStyle(row.available
                    ? AlexTheme.Colors.foreground : AlexTheme.Colors.destructive)
                .lineLimit(1)
                .truncationMode(.middle)
            if !row.available {
                Image(systemName: "exclamationmark.triangle.fill")
                    .font(.system(size: 10))
                    .foregroundStyle(AlexTheme.Colors.destructive)
                    .help("No longer available from its provider — remove it or re-check after fixing the account")
            }
            Spacer()
            if let provider = row.provider {
                Text(ProviderInfo.displayName(provider))
                    .font(AlexTheme.Fonts.metaMicro)
                    .foregroundStyle(AlexTheme.Colors.textFaint)
            }
            Button {
                curated.removeAll { $0.id == row.id }
                Task { await save() }
            } label: {
                Image(systemName: "minus.circle")
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
            }
            .buttonStyle(.plain)
            .help("Remove from curated list")
        }
        .padding(.vertical, 1)
    }

    // MARK: Data

    /// Seed the curated list with each provider's first (alphabetical) models
    /// so switching curation on never starts from an empty published list.
    private func defaultCuratedSeed() -> [ModelAdminRow] {
        var seen: Set<String> = []
        var seed: [ModelAdminRow] = []
        for row in catalog {
            guard let provider = row.provider else { continue }
            if seen.insert(provider).inserted {
                seed.append(row)
            }
        }
        return seed
    }

    private func load(status: String? = nil) async {
        guard let config = store.config else { return }
        error = nil
        isLoading = true
        defer { isLoading = false }
        do {
            let response = try await AlexClient(config: config).modelsAdmin()
            catalog = response.catalog
            curated = response.curated
            curationEnabled = response.curationEnabled
            statusLine = status
        } catch {
            self.error = error.localizedDescription
        }
    }

    private func save() async {
        guard let config = store.config else { return }
        error = nil
        isSaving = true
        defer { isSaving = false }
        do {
            _ = try await AlexClient(config: config)
                .updateExposedModels(curationEnabled ? curated.map(\.id) : nil)
            statusLine = curationEnabled
                ? "Saved. Connected harness configs update automatically."
                : "Curation off — the full catalog is published."
        } catch {
            self.error = error.localizedDescription
        }
    }

    private func check() async {
        guard let config = store.config else { return }
        error = nil
        isChecking = true
        defer { isChecking = false }
        do {
            let response = try await AlexClient(config: config).checkExposedModels()
            let byId = Dictionary(
                uniqueKeysWithValues: response.checked.map { ($0.id, $0) })
            curated = curated.map { byId[$0.id] ?? $0 }
            statusLine = response.missing == 0
                ? "All \(curated.count) curated models are available."
                : "\(response.missing) curated model(s) are no longer available."
        } catch {
            self.error = error.localizedDescription
        }
    }
}
