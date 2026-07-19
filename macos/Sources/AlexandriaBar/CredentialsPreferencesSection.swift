import AppKit
import SwiftUI
import AlexandriaBarCore

/// Preferences → Credentials. The inventory response is redacted by the
/// daemon; this view only renders the configured local key or the ephemeral
/// one-time value returned when a run key is minted.
struct CredentialsPreferencesSection: View {
    let store: SnapshotStore
    let onOpenTraceBrowser: (String) -> Void

    @State private var credentials: CredentialsResponse?
    @State private var config: DaemonConfig?
    @State private var isLoading = true
    @State private var loadError: String?
    @State private var label = ""
    @State private var model = ""
    @State private var ttlSeconds = 86_400
    @State private var isMinting = false
    @State private var isRevoking: Set<String> = []
    @State private var isRevokingAll = false
    @State private var isClearingRevoked = false
    @State private var confirmingRevokeAll = false
    @State private var confirmingClearRevoked = false
    @State private var sortField = CredentialRunKeySortField.created
    @State private var sortDirection = CredentialRunKeySortDirection.descending
    @State private var actionStatus: String?
    @State private var mintedKey: MintedRunKey?

    private let ttlChoices = [(3_600, "1 hour"), (86_400, "1 day"), (604_800, "7 days"), (2_592_000, "30 days")]

    var body: some View {
        VStack(spacing: 0) {
            paneHeader
            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    if isLoading && credentials == nil {
                        loadingState
                    } else if let loadError, credentials == nil {
                        errorState(loadError)
                    } else {
                        credentialSections
                    }
                }
                .padding(.horizontal, 24)
                .padding(.bottom, 20)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        .task { await refresh() }
    }

    private var paneHeader: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(alignment: .center) {
                VStack(alignment: .leading, spacing: 1) {
                    Text("Credentials")
                        .font(AlexTheme.Fonts.panelTitle)
                        .foregroundStyle(AlexTheme.Colors.foreground)
                    Text("Connect other apps and control access keys")
                        .font(.system(size: 12))
                        .foregroundStyle(AlexTheme.Colors.textTertiary)
                }
                Spacer()
                PillButton(
                    title: "Refresh", variant: .bordered, systemImage: "arrow.clockwise",
                    isEnabled: !isLoading && !isMinting && isRevoking.isEmpty && !isBulkBusy
                ) {
                    Task { await refresh() }
                }
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
            Text("Loading credential inventory…")
        }
        .font(.system(size: 12))
        .foregroundStyle(AlexTheme.Colors.textSecondary)
        .padding(.vertical, 28)
        .frame(maxWidth: .infinity)
    }

    private func errorState(_ message: String) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Credentials unavailable")
                .font(.system(size: 12, weight: .semibold))
                .foregroundStyle(AlexTheme.Colors.destructive)
            Text(message)
                .font(AlexTheme.Fonts.metaMono)
                .foregroundStyle(AlexTheme.Colors.textSecondary)
                .textSelection(.enabled)
            PillButton(title: "Try again", variant: .bordered) {
                Task { await refresh() }
            }
        }
        .padding(.vertical, 24)
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    @ViewBuilder
    private var credentialSections: some View {
        useInAnotherAppSection
        mintScopedKeySection
        activeKeysSection
        if let outbound = credentials?.outbound, !outbound.isEmpty {
            outboundStatusSection(outbound)
        }
        if let loadError {
            SettingCaption("Last refresh failed: \(loadError)")
        }
        if let actionStatus {
            SettingCaption(actionStatus)
        }
    }

    private var useInAnotherAppSection: some View {
        Group {
            SectionLabel(text: "Use in another app")
                .settingsSectionSpacing()
            SettingCaption("The base URL is local to this Mac. A run key can call models, but cannot use Alex’s admin API.")
            SettingRow(label: "Local base URL") {
                HStack(spacing: 8) {
                    Text(baseURL)
                        .font(AlexTheme.Fonts.metaMono)
                        .foregroundStyle(AlexTheme.Colors.textSecondary)
                        .textSelection(.enabled)
                        .lineLimit(1)
                    PillButton(title: "Copy", variant: .bordered, systemImage: "doc.on.doc", isEnabled: config != nil) {
                        copy(baseURL, status: "Base URL copied.")
                    }
                }
            }
            RowDivider()
            SettingRow(
                label: activeCredentialLabel,
                hint: mintedKey == nil ? "Your local key can administer Alex." : "Model-only scoped key — safe to paste into another app."
            ) {
                PillButton(title: "Copy key", variant: .bordered, systemImage: "key", isEnabled: activeCredential != nil) {
                    guard let activeCredential else { return }
                    copy(activeCredential, status: "Credential copied.")
                }
            }
            RowDivider()
            SettingRow(label: "OpenAI environment") {
                PillButton(title: "Copy snippet", variant: .primary, systemImage: "doc.on.doc", isEnabled: activeCredential != nil) {
                    guard let activeCredential else { return }
                    copy(
                        "OPENAI_BASE_URL=\(openAIBaseURL) OPENAI_API_KEY=\(activeCredential)",
                        status: "OpenAI environment snippet copied.")
                }
            }
        }
    }

    private var mintScopedKeySection: some View {
        Group {
            SectionLabel(text: "Mint a scoped key")
                .settingsSectionSpacing()
            SettingCaption("Creates a model-only run key. It cannot read traces, mint keys, or administer Alex.")
            SettingRow(label: "Label", hint: "Optional name to recognize this key later") {
                TextField("e.g. VS Code", text: $label)
                    .settingsField()
            }
            RowDivider()
            SettingRow(label: "Model", hint: "Optional model tag for identification") {
                TextField("Any model", text: $model)
                    .settingsField()
            }
            RowDivider()
            SettingRow(label: "Expires") {
                Picker("", selection: $ttlSeconds) {
                    ForEach(ttlChoices, id: \.0) { choice in
                        Text(choice.1).tag(choice.0)
                    }
                }
                .settingsPicker()
            }
            HStack(spacing: 8) {
                PillButton(
                    title: isMinting ? "Minting…" : "Mint model-only key",
                    variant: .primary, systemImage: "plus",
                    isEnabled: !isMinting, isBusy: isMinting
                ) {
                    Task { await mintKey() }
                }
                if isMinting { ProgressView().controlSize(.small) }
                Spacer()
            }
            .padding(.vertical, 10)

            if let mintedKey {
                mintedKeyNotice(mintedKey)
            }
        }
    }

    private func mintedKeyNotice(_ key: MintedRunKey) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 7) {
                Image(systemName: "exclamationmark.triangle.fill")
                    .foregroundStyle(AlexTheme.Colors.warningOrange)
                Text("Copy this model-only key now — it will not be shown again.")
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(AlexTheme.Colors.foreground)
                Spacer()
                PillButton(title: "Copy", variant: .primary, systemImage: "doc.on.doc") {
                    copy(key.key, status: "New model-only key copied.")
                }
                PillButton(title: "Hide", variant: .bordered) {
                    mintedKey = nil
                }
            }
            Text(key.key)
                .font(AlexTheme.Fonts.metaMono)
                .foregroundStyle(AlexTheme.Colors.foreground)
                .textSelection(.enabled)
                .lineLimit(2)
                .truncationMode(.middle)
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: AlexTheme.Radius.md)
            .fill(AlexTheme.Colors.warningOrange.opacity(0.08)))
    }

    private var activeKeysSection: some View {
        Group {
            SectionLabel(text: "Active keys")
                .settingsSectionSpacing()
            SettingCaption("Secret values are never included in this inventory.")
            SettingRow(label: "Admin key", hint: presenceText(credentials?.inbound.adminKey.present)) {
                statusBadge(credentials?.inbound.adminKey.present == true)
            }
            RowDivider()
            SettingRow(label: "Local key", hint: presenceText(credentials?.inbound.localKey.present)) {
                PillButton(title: "Copy key", variant: .bordered, systemImage: "doc.on.doc", isEnabled: config != nil) {
                    guard let config else { return }
                    copy(config.localKey, status: "Local key copied.")
                }
            }

            if let keys = credentials?.inbound.runKeys, !keys.isEmpty {
                runKeyBulkActions(keys)
                runKeyTable(keys)
            } else {
                SettingCaption("No run keys have been minted.")
            }
        }
    }

    // Fixed column widths so a LazyVStack of HStack rows still reads as a table.
    // A `Grid` lays out EVERY row eagerly, which hung the main thread for ~9s once
    // enough run keys accumulated (each harness connect mints one). LazyVStack —
    // inside the section's ScrollView — only lays out visible rows, so the pane
    // stays responsive no matter how many keys exist.
    private static let keyCols: [CGFloat] = [96, 92, 110, 90, 90, 90, 38]
    private static let keyActionWidth: CGFloat = 132

    private func keyHeaderRow() -> some View {
        HStack(spacing: 10) {
            sortableHeader("Label", field: .label)
                .frame(width: Self.keyCols[0], alignment: .leading)
            tableHeader("Fingerprint").frame(width: Self.keyCols[1], alignment: .leading)
            tableHeader("Tags").frame(width: Self.keyCols[2], alignment: .leading)
            sortableHeader("Created", field: .created)
                .frame(width: Self.keyCols[3], alignment: .leading)
            sortableHeader("Expires", field: .expires)
                .frame(width: Self.keyCols[4], alignment: .leading)
            sortableHeader("Last used", field: .lastUsed)
                .frame(width: Self.keyCols[5], alignment: .leading)
            sortableHeader("Uses", field: .uses)
                .frame(width: Self.keyCols[6], alignment: .leading)
            Text("").frame(width: Self.keyActionWidth)
        }
    }

    private func keyRow(_ key: CredentialRunKey) -> some View {
        HStack(spacing: 10) {
            Text(key.label?.isEmpty == false ? key.label! : key.kind)
                .lineLimit(1).frame(width: Self.keyCols[0], alignment: .leading)
            Text(shortFingerprint(key.keyFingerprint))
                .font(AlexTheme.Fonts.metaMono).frame(width: Self.keyCols[1], alignment: .leading)
            Text(tagSummary(key))
                .lineLimit(1).truncationMode(.tail).frame(width: Self.keyCols[2], alignment: .leading)
            Text(dateText(key.createdMs)).frame(width: Self.keyCols[3], alignment: .leading)
            Text(key.revoked ? "Revoked" : optionalDateText(key.expiresMs))
                .frame(width: Self.keyCols[4], alignment: .leading)
            Text(optionalDateText(key.lastUsedMs, never: "Never"))
                .frame(width: Self.keyCols[5], alignment: .leading)
            Text("\(key.useCount)").frame(width: Self.keyCols[6], alignment: .leading)
            HStack(spacing: 6) {
                PillButton(
                    title: "Traces", variant: .bordered, systemImage: "magnifyingglass",
                    fontSize: 9, horizontalPadding: 7, verticalPadding: 4, cornerRadius: 6
                ) {
                    onOpenTraceBrowser("key:\(key.keyFingerprint)")
                }
                PillButton(
                    title: key.revoked ? "Revoked" : "Revoke", variant: .danger,
                    horizontalPadding: 9, verticalPadding: 4, cornerRadius: 6,
                    isEnabled: !key.revoked && !isRevoking.contains(key.id) && !isBulkBusy
                ) {
                    Task { await revoke(key) }
                }
            }
            .frame(width: Self.keyActionWidth, alignment: .leading)
        }
        .font(.system(size: 10))
        .foregroundStyle(AlexTheme.Colors.textSecondary)
    }

    private func runKeyTable(_ keys: [CredentialRunKey]) -> some View {
        LazyVStack(alignment: .leading, spacing: 8) {
            keyHeaderRow()
            ForEach(keys.sorted(by: sortField, direction: sortDirection)) { key in
                keyRow(key)
            }
        }
        .padding(.vertical, 10)
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private func outboundStatusSection(_ outbound: [OutboundCredential]) -> some View {
        Group {
            SectionLabel(text: "Outbound status")
                .settingsSectionSpacing()
            SettingCaption("Provider and harness sign-ins are read-only here; re-authenticate from their provider or harness settings.")
            ForEach(outbound) { credential in
                SettingRow(
                    label: ProviderInfo.displayName(credential.provider ?? credential.name ?? credential.kind),
                    hint: credential.identity ?? credential.source
                ) {
                    statusBadge(credential.active, label: credential.active ? "Active" : "Needs re-auth")
                }
                if credential.id != outbound.last?.id { RowDivider() }
            }
        }
    }

    private var baseURL: String { config?.baseURL.absoluteString.trimmingCharacters(in: CharacterSet(charactersIn: "/")) ?? "Not configured" }
    private var openAIBaseURL: String { baseURL == "Not configured" ? baseURL : "\(baseURL)/v1" }
    private var activeCredential: String? { mintedKey?.key ?? config?.localKey }
    private var activeCredentialLabel: String { mintedKey == nil ? "Local key" : "New model-only run key" }
    private var isBulkBusy: Bool { isRevokingAll || isClearingRevoked }

    private func statusBadge(_ active: Bool, label: String? = nil) -> some View {
        HStack(spacing: 5) {
            StatusDot(
                tint: active ? AlexTheme.Colors.success : AlexTheme.Colors.textFaint,
                size: 6, glow: active)
            Text(label ?? (active ? "Present" : "Missing"))
                .font(.system(size: 11, weight: .medium))
                .foregroundStyle(active ? AlexTheme.Colors.success : AlexTheme.Colors.textTertiary)
        }
    }

    private func tableHeader(_ value: String) -> some View {
        Text(value)
            .font(.system(size: 9, weight: .semibold))
            .foregroundStyle(AlexTheme.Colors.textFaint)
    }

    private func sortableHeader(
        _ value: String, field: CredentialRunKeySortField
    ) -> some View {
        Button {
            if sortField == field {
                sortDirection = sortDirection == .ascending ? .descending : .ascending
            } else {
                sortField = field
                sortDirection = .ascending
            }
        } label: {
            HStack(spacing: 2) {
                tableHeader(value)
                if sortField == field {
                    Text(sortDirection == .ascending ? "▲" : "▼")
                        .font(.system(size: 7, weight: .bold))
                        .foregroundStyle(AlexTheme.Colors.textTertiary)
                }
            }
        }
        .buttonStyle(.plain)
    }

    private func runKeyBulkActions(_ keys: [CredentialRunKey]) -> some View {
        HStack(spacing: 8) {
            Spacer()
            PillButton(
                title: isRevokingAll ? "Revoking…" : "Revoke all",
                variant: .danger, systemImage: "xmark.circle",
                isEnabled: keys.contains(where: { !$0.revoked }) && !isBulkBusy,
                isBusy: isRevokingAll
            ) {
                confirmingRevokeAll = true
            }
            .confirmationDialog(
                "Revoke all active run keys?",
                isPresented: $confirmingRevokeAll,
                titleVisibility: .visible
            ) {
                Button("Revoke all", role: .destructive) { Task { await revokeAll() } }
                Button("Cancel", role: .cancel) {}
            } message: {
                Text("Every active run key will stop working immediately. Audit rows will be retained.")
            }
            PillButton(
                title: isClearingRevoked ? "Clearing…" : "Clear revoked",
                variant: .bordered, systemImage: "trash",
                isEnabled: keys.contains(where: \.revoked) && !isBulkBusy,
                isBusy: isClearingRevoked
            ) {
                confirmingClearRevoked = true
            }
            .confirmationDialog(
                "Clear all revoked run keys?",
                isPresented: $confirmingClearRevoked,
                titleVisibility: .visible
            ) {
                Button("Clear revoked", role: .destructive) { Task { await clearRevoked() } }
                Button("Cancel", role: .cancel) {}
            } message: {
                Text("Revoked key audit rows will be permanently deleted. Active keys are not affected.")
            }
        }
        .padding(.top, 8)
    }

    private func presenceText(_ present: Bool?) -> String {
        present == true ? "Present" : "Missing"
    }

    private func shortFingerprint(_ value: String) -> String {
        String(value.prefix(10))
    }

    private func tagSummary(_ key: CredentialRunKey) -> String {
        var tags = key.tags.map { "\($0.key)=\($0.value.displayValue)" }.sorted()
        if let runID = key.runId, !runID.isEmpty { tags.append("job=\(runID)") }
        return tags.isEmpty ? "—" : tags.joined(separator: " · ")
    }

    // A fresh DateFormatter is one of the most expensive Foundation objects to
    // allocate. The active-keys table is a non-lazy Grid that calls this up to
    // three times per row, so allocating per cell froze the main thread as soon
    // as more than a handful of run keys existed. Share one formatter instead.
    nonisolated(unsafe) private static let keyDateFormatter: DateFormatter = {
        let formatter = DateFormatter()
        formatter.dateStyle = .short
        formatter.timeStyle = .short
        return formatter
    }()

    private func dateText(_ milliseconds: Int64) -> String {
        Self.keyDateFormatter.string(from: Date(timeIntervalSince1970: TimeInterval(milliseconds) / 1_000))
    }

    private func optionalDateText(_ milliseconds: Int64?, never: String = "Never") -> String {
        guard let milliseconds else { return never }
        return dateText(milliseconds)
    }

    private func copy(_ value: String, status: String) {
        CredentialClipboard.copy(value)
        actionStatus = status
    }

    /// Resolves the daemon config without touching the filesystem on the main
    /// actor. `DaemonDiscovery.load()` reads and parses config.toml synchronously;
    /// running it inside the view body (as the old computed `config` property did)
    /// put disk I/O on every render. Prefer the store's cached value, and only
    /// fall back to a detached load when the store has not polled yet.
    private func resolvedConfig() async -> DaemonConfig? {
        if let config = store.config { return config }
        return await Task.detached { DaemonDiscovery.load() }.value
    }

    private func refresh() async {
        isLoading = true
        loadError = nil
        defer { isLoading = false }
        guard let config = await resolvedConfig() else {
            loadError = "No Alex daemon configuration was found."
            return
        }
        self.config = config
        let client = AlexandriaClient(config: config)
        do {
            // Backstop above the client's own 5s/10s URLSession timeouts so a
            // wedged daemon can never leave the pane spinning indefinitely.
            credentials = try await Self.withLoadTimeout(seconds: 10) {
                try await client.credentials()
            }
        } catch is CancellationError {
            return
        } catch {
            loadError = error.localizedDescription
        }
    }

    /// Races `operation` against a timeout so the load always resolves. Used as a
    /// defensive backstop for the credential inventory fetch.
    private static func withLoadTimeout<T: Sendable>(
        seconds: Double,
        _ operation: @escaping @Sendable () async throws -> T
    ) async throws -> T {
        try await withThrowingTaskGroup(of: T.self) { group in
            group.addTask { try await operation() }
            group.addTask {
                try await Task.sleep(for: .seconds(seconds))
                throw CredentialsLoadError.timedOut
            }
            defer { group.cancelAll() }
            guard let result = try await group.next() else {
                throw CredentialsLoadError.timedOut
            }
            return result
        }
    }

    private func mintKey() async {
        guard let config else {
            actionStatus = "Could not mint a key: no daemon configuration."
            return
        }
        isMinting = true
        actionStatus = nil
        defer { isMinting = false }
        do {
            mintedKey = try await AlexandriaClient(config: config).mintRunKey(
                label: label, model: model, ttlSeconds: ttlSeconds)
            actionStatus = "Model-only key minted. Copy it before hiding it."
            await refresh()
        } catch is CancellationError {
            return
        } catch {
            actionStatus = "Could not mint a key: \(error.localizedDescription)"
        }
    }

    private func revoke(_ key: CredentialRunKey) async {
        guard let config else { return }
        isRevoking.insert(key.id)
        actionStatus = nil
        defer { isRevoking.remove(key.id) }
        do {
            try await AlexandriaClient(config: config).revokeRunKey(id: key.id)
            actionStatus = "Run key \(shortFingerprint(key.keyFingerprint)) revoked."
            await refresh()
        } catch is CancellationError {
            return
        } catch {
            actionStatus = "Could not revoke the key: \(error.localizedDescription)"
        }
    }

    private func revokeAll() async {
        guard let config else { return }
        isRevokingAll = true
        actionStatus = nil
        defer { isRevokingAll = false }
        do {
            let count = try await AlexandriaClient(config: config).revokeAllRunKeys()
            actionStatus = "Revoked \(count) active run key\(count == 1 ? "" : "s")."
            await refresh()
        } catch is CancellationError {
            return
        } catch {
            actionStatus = "Could not revoke all keys: \(error.localizedDescription)"
        }
    }

    private func clearRevoked() async {
        guard let config else { return }
        isClearingRevoked = true
        actionStatus = nil
        defer { isClearingRevoked = false }
        do {
            let count = try await AlexandriaClient(config: config).clearRevokedRunKeys()
            actionStatus = "Removed \(count) revoked run key row\(count == 1 ? "" : "s")."
            await refresh()
        } catch is CancellationError {
            return
        } catch {
            actionStatus = "Could not clear revoked keys: \(error.localizedDescription)"
        }
    }
}

private enum CredentialsLoadError: LocalizedError {
    case timedOut
    var errorDescription: String? {
        "The Alex daemon did not respond in time. It may be starting up or unreachable."
    }
}

/// Clipboard access stays in the app target.
enum CredentialClipboard {
    static func copy(_ value: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(value, forType: .string)
    }
}
