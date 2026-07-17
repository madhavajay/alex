import SwiftUI
import AlexandriaBarCore

/// Preferences → Notifications. Telegram credentials stay only in this
/// short-lived form state and are never included in the channel list response.
struct NotificationsPreferencesSection: View {
    let store: SnapshotStore

    @State private var token = ""
    @State private var chatID = ""
    @State private var minLevel: NotificationLevel = .warn
    @State private var reauthOnly = true
    @State private var instructionsExpanded = true
    @State private var discoveredChats: [NotificationChat] = []
    @State private var connectedBot: String?
    @State private var validationResult: String?
    @State private var discoveryResult: String?
    @State private var actionResult: String?
    @State private var channels: [NotificationChannel] = []
    @State private var isLoading = true
    @State private var isValidating = false
    @State private var isDiscovering = false
    @State private var isTesting = false
    @State private var isSaving = false
    @State private var removingID: String?
    @State private var loadError: String?

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
                        notificationSections
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
        VStack(alignment: .leading, spacing: 1) {
            Text("Notifications")
                .font(AlexTheme.Fonts.panelTitle)
                .foregroundStyle(AlexTheme.Colors.foreground)
            Text("Telegram alerts from the Alexandria daemon")
                .font(.system(size: 12))
                .foregroundStyle(AlexTheme.Colors.textTertiary)
        }
        .padding(.horizontal, 24)
        .padding(.top, 16)
        .padding(.bottom, 12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .overlay(alignment: .bottom) {
            Rectangle()
                .fill(AlexTheme.Colors.overlay(0.06))
                .frame(height: 1)
                .padding(.horizontal, 24)
        }
    }

    private var loadingState: some View {
        HStack(spacing: 8) {
            ProgressView().controlSize(.small)
            Text("Loading notification channels…")
        }
        .font(.system(size: 12))
        .foregroundStyle(AlexTheme.Colors.textSecondary)
        .padding(.vertical, 28)
        .frame(maxWidth: .infinity)
    }

    private func errorState(_ message: String) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Notifications unavailable")
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
    private var notificationSections: some View {
        SectionLabel(text: "Telegram setup")
            .settingsSectionSpacing()

        DisclosureGroup("How to connect a Telegram bot", isExpanded: $instructionsExpanded) {
            VStack(alignment: .leading, spacing: 4) {
                Text("1. In Telegram, message @BotFather → /newbot → copy the token.")
                Text("2. Message your new bot once.")
                Text("3. Paste the token below and Validate.")
            }
            .font(.system(size: 11))
            .foregroundStyle(AlexTheme.Colors.textTertiary)
            .padding(.top, 5)
        }
        .font(.system(size: 12, weight: .medium))
        .padding(.vertical, 10)

        SettingRow(label: "Bot token", hint: "Stored by the daemon only after you save") {
            HStack(spacing: 8) {
                SecureField("123456:ABC…", text: $token)
                    .settingsField(width: 240)
                PillButton(
                    title: isValidating ? "Validating…" : "Validate", variant: .bordered,
                    isEnabled: !token.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && !isValidating,
                    isBusy: isValidating
                ) { Task { await validate() } }
            }
        }
        if let validationResult {
            statusCaption(validationResult, isError: connectedBot == nil)
        }
        RowDivider()

        SettingRow(label: "Chat ID", hint: "Detect finds chats that have already messaged this bot") {
            HStack(spacing: 8) {
                TextField("e.g. 123456789", text: $chatID)
                    .settingsField(width: 180)
                PillButton(
                    title: isDiscovering ? "Detecting…" : "Detect", variant: .bordered,
                    isEnabled: !token.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && !isDiscovering,
                    isBusy: isDiscovering
                ) { Task { await discoverChats() } }
            }
        }
        if discoveredChats.count > 1 {
            Picker("Detected chat", selection: $chatID) {
                ForEach(discoveredChats) { chat in
                    Text("\(chat.chatName) (\(chat.chatID))").tag(chat.chatID)
                }
            }
            .settingsPicker()
            .padding(.vertical, 8)
        }
        if let discoveryResult {
            statusCaption(discoveryResult, isError: discoveredChats.isEmpty)
        }

        SectionLabel(text: "Alert policy")
            .settingsSectionSpacing()
        SettingRow(label: "Alert me when a subscription needs re-auth") {
            Toggle("", isOn: $reauthOnly)
                .settingsSwitch()
        }
        RowDivider()
        SettingRow(label: "Minimum level") {
            Picker("Minimum level", selection: $minLevel) {
                Text("Info").tag(NotificationLevel.info)
                Text("Warn").tag(NotificationLevel.warn)
                Text("Critical").tag(NotificationLevel.critical)
            }
            .settingsPicker()
        }
        SettingCaption(reauthOnly
            ? "Only subscription re-auth alerts will be sent."
            : "All notification categories will be sent.")

        HStack(spacing: 8) {
            PillButton(
                title: isTesting ? "Sending…" : "Send test message", variant: .bordered,
                systemImage: "paperplane", isEnabled: canSubmit && !isTesting, isBusy: isTesting
            ) { Task { await test() } }
            PillButton(
                title: isSaving ? "Saving…" : "Save", variant: .primary,
                isEnabled: canSubmit && !isSaving, isBusy: isSaving
            ) { Task { await save() } }
            if let actionResult {
                Text(actionResult)
                    .font(.system(size: 11))
                    .foregroundStyle(actionResult.hasPrefix("Save failed") || actionResult.hasPrefix("Test failed")
                        ? AlexTheme.Colors.destructive : AlexTheme.Colors.success)
            }
            Spacer()
        }
        .padding(.top, 18)

        SectionLabel(text: "Configured channels")
            .settingsSectionSpacing()
        if channels.isEmpty {
            SettingCaption("No notification channels are configured.")
        } else {
            ForEach(channels, id: \.stableID) { channel in
                configuredChannel(channel)
                if channel.stableID != channels.last?.stableID { RowDivider() }
            }
        }
    }

    private func statusCaption(_ text: String, isError: Bool) -> some View {
        Text(text)
            .font(.system(size: 11, weight: .medium))
            .foregroundStyle(isError ? AlexTheme.Colors.destructive : AlexTheme.Colors.success)
            .padding(.vertical, 5)
    }

    private func configuredChannel(_ channel: NotificationChannel) -> some View {
        HStack(alignment: .top, spacing: 12) {
            VStack(alignment: .leading, spacing: 3) {
                Text(channelSummary(channel))
                    .font(.system(size: 12, weight: .medium))
                    .foregroundStyle(AlexTheme.Colors.foreground)
                if let host = channel.host, !host.isEmpty {
                    Text(host)
                        .font(AlexTheme.Fonts.metaMono)
                        .foregroundStyle(AlexTheme.Colors.textTertiary)
                }
                Text(channelStatus(channel))
                    .font(.system(size: 11))
                    .foregroundStyle(channel.lastError == nil
                        ? AlexTheme.Colors.textTertiary : AlexTheme.Colors.destructive)
            }
            Spacer(minLength: 12)
            if let id = channel.id {
                Button {
                    Task { await remove(id: id) }
                } label: {
                    if removingID == id {
                        ProgressView().controlSize(.small)
                    } else {
                        Image(systemName: "trash")
                    }
                }
                .buttonStyle(.plain)
                .foregroundStyle(AlexTheme.Colors.textTertiary)
                .disabled(removingID != nil)
                .help("Remove notification channel")
            }
        }
        .padding(.vertical, 11)
    }

    private var canSubmit: Bool {
        !token.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            && !chatID.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    private var request: TelegramNotificationChannelRequest {
        TelegramNotificationChannelRequest(
            token: token.trimmingCharacters(in: .whitespacesAndNewlines),
            chatID: chatID.trimmingCharacters(in: .whitespacesAndNewlines),
            minLevel: minLevel,
            categories: reauthOnly ? ["reauth"] : [])
    }

    private func client() -> AlexandriaClient? {
        guard let config = store.config ?? DaemonDiscovery.load() else { return nil }
        return AlexandriaClient(config: config)
    }

    private func load() async {
        isLoading = true
        loadError = nil
        defer { isLoading = false }
        guard let client = client() else {
            loadError = "No Alexandria daemon configuration was found."
            return
        }
        do {
            channels = try await client.notificationSettings().channels
        } catch is CancellationError {
        } catch {
            loadError = error.localizedDescription
        }
    }

    private func validate() async {
        guard let client = client() else {
            validationResult = "No Alexandria daemon configuration was found."
            connectedBot = nil
            return
        }
        isValidating = true
        validationResult = nil
        connectedBot = nil
        defer { isValidating = false }
        do {
            let response = try await client.validateTelegramNotification(token: token)
            if response.ok, let username = response.botUsername, !username.isEmpty {
                connectedBot = username
                validationResult = "✓ Connected to @\(username)"
            } else {
                validationResult = response.error ?? "Telegram validation failed"
            }
        } catch is CancellationError {
        } catch {
            validationResult = error.localizedDescription
        }
    }

    private func discoverChats() async {
        guard let client = client() else {
            discoveryResult = "No Alexandria daemon configuration was found."
            return
        }
        isDiscovering = true
        discoveryResult = nil
        discoveredChats = []
        defer { isDiscovering = false }
        do {
            let response = try await client.discoverTelegramChats(token: token)
            discoveredChats = response.chats
            if let first = response.chats.first {
                chatID = first.chatID
                discoveryResult = response.chats.count == 1
                    ? "Detected \(first.chatName)"
                    : "Detected \(response.chats.count) chats — choose one below."
            } else {
                discoveryResult = response.error ?? "Send your bot a message first, then Detect."
            }
        } catch is CancellationError {
        } catch {
            discoveryResult = error.localizedDescription
        }
    }

    private func test() async {
        guard let client = client() else {
            actionResult = "Test failed: no daemon configuration"
            return
        }
        isTesting = true
        actionResult = nil
        defer { isTesting = false }
        do {
            let response = try await client.testTelegramNotification(request)
            if let failed = response.channels.first(where: { !$0.ok }) {
                actionResult = "Test failed: \(failed.error ?? "Telegram delivery failed")"
            } else if response.channels.isEmpty {
                actionResult = "Test failed: no channel result returned"
            } else {
                actionResult = "Sent — check Telegram"
            }
        } catch is CancellationError {
        } catch {
            actionResult = "Test failed: \(error.localizedDescription)"
        }
    }

    private func save() async {
        guard let client = client() else {
            actionResult = "Save failed: no daemon configuration"
            return
        }
        isSaving = true
        actionResult = nil
        defer { isSaving = false }
        do {
            let response = try await client.saveTelegramNotification(request)
            guard response.ok else {
                actionResult = "Save failed: \(response.error ?? "Telegram validation failed")"
                return
            }
            token = ""
            chatID = ""
            discoveredChats = []
            connectedBot = nil
            validationResult = nil
            actionResult = "Telegram notifications saved"
            await loadChannels(using: client)
        } catch is CancellationError {
        } catch {
            actionResult = "Save failed: \(error.localizedDescription)"
        }
    }

    private func remove(id: String) async {
        guard let client = client() else {
            actionResult = "Remove failed: no daemon configuration"
            return
        }
        removingID = id
        defer { removingID = nil }
        do {
            try await client.removeNotification(id: id)
            actionResult = "Notification channel removed"
            await loadChannels(using: client)
        } catch is CancellationError {
        } catch {
            actionResult = "Remove failed: \(error.localizedDescription)"
        }
    }

    private func loadChannels(using client: AlexandriaClient) async {
        do {
            channels = try await client.notificationSettings().channels
        } catch is CancellationError {
        } catch {
            actionResult = "Saved, but could not reload channels: \(error.localizedDescription)"
        }
    }

    private func channelSummary(_ channel: NotificationChannel) -> String {
        let bot = channel.botUsername.map { "@\($0)" } ?? "Telegram"
        return "\(channel.format.capitalized) · \(bot)"
    }

    private func channelStatus(_ channel: NotificationChannel) -> String {
        if let error = channel.lastError, !error.isEmpty { return "Last error: \(error)" }
        guard let sent = channel.lastSentMs else { return "No messages sent yet" }
        return "Last sent \(Self.relativeDate(sent))"
    }

    private static func relativeDate(_ milliseconds: Int64) -> String {
        let date = Date(timeIntervalSince1970: Double(milliseconds) / 1_000)
        return RelativeDateTimeFormatter().localizedString(for: date, relativeTo: Date())
    }
}

#if DEBUG
#Preview {
    NotificationsPreferencesSection(store: SnapshotStore())
        .frame(width: 780, height: 620)
}
#endif
