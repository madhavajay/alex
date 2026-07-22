import SwiftUI
import AlexCore

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
    // True once a channel is saved: the daemon redacts the token from its API,
    // so on reload we show a masked "saved" indicator instead of a blank field
    // (which read as data loss). Replace reveals the SecureField for a new token.
    @State private var hasSavedToken = false
    @State private var validationResult: String?
    @State private var discoveryResult: String?
    @State private var actionResult: String?
    @State private var channels: [NotificationChannel] = []
    @State private var messages: [NotificationLogEntry] = []
    @State private var isLoading = true
    @State private var isLoadingMessages = false
    @State private var isValidating = false
    @State private var isDiscovering = false
    @State private var isTesting = false
    @State private var isSaving = false
    @State private var removingID: String?
    @State private var updatingCommandIDs: Set<String> = []
    @State private var commandResults: [String: String] = [:]
    @State private var commandErrors: Set<String> = []
    @State private var loadError: String?
    @State private var messagesError: String?

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
            Text("Telegram alerts from the Alex daemon")
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

        SettingRow(
            label: "Bot token",
            hint: hasSavedToken
                ? "A token is saved (hidden). Replace to change it."
                : "Stored by the daemon only after you save"
        ) {
            HStack(spacing: 8) {
                if hasSavedToken {
                    Text(verbatim: "•••••••••••••••")
                        .font(AlexTheme.Fonts.metaMono)
                        .foregroundStyle(AlexTheme.Colors.textSecondary)
                        .settingsField(width: 180)
                    if let bot = connectedBot, !bot.isEmpty {
                        Text(verbatim: "✓ @\(bot)")
                            .font(.system(size: 11, weight: .medium))
                            .foregroundStyle(AlexTheme.Colors.success)
                            .lineLimit(1)
                            .truncationMode(.middle)
                    }
                    PillButton(title: "Replace", variant: .bordered, isEnabled: true) {
                        hasSavedToken = false
                        token = ""
                        connectedBot = nil
                    }
                    .fixedSize()
                } else {
                    SecureField("123456:ABC…", text: $token)
                        .settingsField(width: 180)
                    PillButton(
                        title: isValidating ? "Validating…" : "Validate", variant: .bordered,
                        isEnabled: !token.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && !isValidating,
                        isBusy: isValidating
                    ) { Task { await validate() } }
                    .fixedSize()
                }
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
                systemImage: "paperplane", isEnabled: !isTesting && !isSaving,
                isBusy: isTesting
            ) { Task { await test() } }
            PillButton(
                title: isSaving ? "Saving…" : "Save", variant: .primary,
                isEnabled: canSubmit && !isSaving && !isTesting, isBusy: isSaving
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

        recentMessagesPanel
    }

    private func statusCaption(_ text: String, isError: Bool) -> some View {
        Text(text)
            .font(.system(size: 11, weight: .medium))
            .foregroundStyle(isError ? AlexTheme.Colors.destructive : AlexTheme.Colors.success)
            .padding(.vertical, 5)
    }

    private func configuredChannel(_ channel: NotificationChannel) -> some View {
        VStack(alignment: .leading, spacing: 8) {
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
                    .disabled(removingID != nil || !updatingCommandIDs.isEmpty)
                    .help("Remove notification channel")
                }
            }

            if channel.format.lowercased() == "telegram" {
                HStack(spacing: 8) {
                    Text("Allow commands")
                        .font(.system(size: 12, weight: .medium))
                        .foregroundStyle(AlexTheme.Colors.foreground)
                    Toggle("", isOn: commandBinding(for: channel))
                        .settingsSwitch()
                        .disabled(channel.id == nil || updatingCommandIDs.contains(channel.stableID))
                    if updatingCommandIDs.contains(channel.stableID) {
                        ProgressView().controlSize(.small)
                    }
                }
                Text(channel.id == nil
                    ? "Commands unavailable because this channel has no saved ID."
                    : "Enables /status and code#state paste-back from this Telegram chat.")
                    .font(.system(size: 11))
                    .foregroundStyle(channel.id == nil
                        ? AlexTheme.Colors.destructive : AlexTheme.Colors.textTertiary)
                if let result = commandResults[channel.stableID] {
                    statusCaption(result, isError: commandErrors.contains(channel.stableID))
                        .padding(.vertical, -5)
                }
            }
        }
        .padding(.vertical, 11)
    }

    private var recentMessagesPanel: some View {
        VStack(alignment: .leading, spacing: 0) {
            SectionLabel(text: "Recent messages") {
                PillButton(
                    title: isLoadingMessages ? "Refreshing…" : "Refresh",
                    variant: .bordered, systemImage: "arrow.clockwise",
                    isEnabled: !isLoadingMessages, isBusy: isLoadingMessages
                ) { Task { await refreshMessages() } }
            }
            .settingsSectionSpacing()

            VStack(alignment: .leading, spacing: 0) {
                HStack {
                    Text("Activity")
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(AlexTheme.Colors.foreground)
                    Spacer()
                    Text("Last \(messages.count)")
                        .font(AlexTheme.Fonts.metaMono)
                        .foregroundStyle(AlexTheme.Colors.textTertiary)
                }
                .padding(.horizontal, 12)
                .padding(.vertical, 9)

                RowDivider()

                if isLoadingMessages && messages.isEmpty {
                    HStack(spacing: 8) {
                        ProgressView().controlSize(.small)
                        Text("Loading recent notification activity…")
                    }
                    .font(.system(size: 11))
                    .foregroundStyle(AlexTheme.Colors.textSecondary)
                    .padding(12)
                } else if let messagesError {
                    Text("Could not load activity: \(messagesError)")
                        .font(.system(size: 11))
                        .foregroundStyle(AlexTheme.Colors.destructive)
                        .textSelection(.enabled)
                        .padding(12)
                } else if messages.isEmpty {
                    Text("No inbound or outbound messages recorded yet.")
                        .font(.system(size: 11))
                        .foregroundStyle(AlexTheme.Colors.textTertiary)
                        .padding(12)
                } else {
                    ScrollView {
                        LazyVStack(alignment: .leading, spacing: 0) {
                            ForEach(messages.indices, id: \.self) { index in
                                activityRow(messages[index])
                                if index != messages.indices.last { RowDivider() }
                            }
                        }
                    }
                    .frame(maxHeight: 240)
                }
            }
            .alexCard(radius: AlexTheme.Radius.lg)
        }
    }

    private func activityRow(_ message: NotificationLogEntry) -> some View {
        HStack(alignment: .top, spacing: 9) {
            Image(systemName: message.direction == "in" ? "arrow.down.left" : "arrow.up.right")
                .font(.system(size: 10, weight: .semibold))
                .foregroundStyle(message.direction == "in"
                    ? AlexTheme.Colors.primary : AlexTheme.Colors.textSecondary)
                .frame(width: 13, height: 16)
            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 6) {
                    Text(message.direction.uppercased())
                        .font(AlexTheme.Fonts.chipMono)
                    Text(message.category)
                        .font(AlexTheme.Fonts.metaMono)
                    Image(systemName: message.ok ? "checkmark.circle.fill" : "exclamationmark.circle.fill")
                        .font(.system(size: 9))
                        .foregroundStyle(message.ok
                            ? AlexTheme.Colors.success : AlexTheme.Colors.destructive)
                    Spacer(minLength: 8)
                    Text(Self.activityTime(message.tsMs))
                        .font(AlexTheme.Fonts.metaMicro)
                        .foregroundStyle(AlexTheme.Colors.textTertiary)
                }
                Text(message.summary)
                    .font(.system(size: 11))
                    .foregroundStyle(AlexTheme.Colors.textSecondary)
                    .lineLimit(2)
                    .textSelection(.enabled)
                if let error = message.error, !error.isEmpty {
                    Text(error)
                        .font(AlexTheme.Fonts.metaMono)
                        .foregroundStyle(AlexTheme.Colors.destructive)
                        .lineLimit(2)
                        .textSelection(.enabled)
                }
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
    }

    private var canSubmit: Bool {
        !token.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            && !chatID.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    private var savedChannelID: String? {
        channels.first(where: { $0.format.lowercased() == "telegram" && $0.id != nil })?.id
    }

    private var testTarget: NotificationTestTarget? {
        NotificationTestTarget.resolve(
            token: token, chatID: chatID, savedChannelID: savedChannelID)
    }

    private var request: TelegramNotificationChannelRequest {
        TelegramNotificationChannelRequest(
            token: token.trimmingCharacters(in: .whitespacesAndNewlines),
            chatID: chatID.trimmingCharacters(in: .whitespacesAndNewlines),
            minLevel: minLevel,
            categories: reauthOnly ? ["reauth"] : [])
    }

    private func client() -> AlexClient? {
        guard let config = store.config ?? DaemonDiscovery.load() else { return nil }
        return AlexClient(config: config)
    }

    private func load() async {
        isLoading = true
        loadError = nil
        defer { isLoading = false }
        guard let client = client() else {
            loadError = "No Alex daemon configuration was found."
            return
        }
        do {
            applyChannels(try await client.notificationSettings().channels)
            await loadMessages(using: client)
        } catch is CancellationError {
        } catch {
            loadError = error.localizedDescription
        }
    }

    private func validate() async {
        guard let client = client() else {
            validationResult = "No Alex daemon configuration was found."
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
            discoveryResult = "No Alex daemon configuration was found."
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
        guard let testTarget else {
            actionResult = "Test failed: enter a bot token and chat ID, or save a channel first"
            return
        }
        isTesting = true
        actionResult = nil
        defer { isTesting = false }
        do {
            let response: NotificationTestResponse
            switch testTarget {
            case .inline:
                response = try await client.testTelegramNotification(request)
            case let .savedChannel(channelID):
                response = try await client.testTelegramNotification(channelId: channelID)
            }
            if let failed = response.channels.first(where: { !$0.ok }) {
                actionResult = "Test failed: \(failed.error ?? "Telegram delivery failed")"
            } else if response.channels.isEmpty {
                actionResult = "Test failed: no channel result returned"
            } else {
                actionResult = "Sent — check Telegram"
            }
            await loadMessages(using: client)
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
            commandResults[id] = nil
            commandErrors.remove(id)
            await loadChannels(using: client)
        } catch is CancellationError {
        } catch {
            actionResult = "Remove failed: \(error.localizedDescription)"
        }
    }

    private func loadChannels(using client: AlexClient) async {
        do {
            applyChannels(try await client.notificationSettings().channels)
        } catch is CancellationError {
        } catch {
            actionResult = "Saved, but could not reload channels: \(error.localizedDescription)"
        }
    }

    private func applyChannels(_ loadedChannels: [NotificationChannel]) {
        channels = loadedChannels
        // Populate the form from an existing Telegram channel so it reads as
        // saved while its token remains redacted and server-side.
        if let saved = loadedChannels.first(where: { $0.format.lowercased() == "telegram" }) {
            hasSavedToken = true
            connectedBot = saved.botUsername
            if let cid = saved.chatID, !cid.isEmpty { chatID = cid }
        } else if token.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            hasSavedToken = false
            connectedBot = nil
        }
    }

    private func commandBinding(for channel: NotificationChannel) -> Binding<Bool> {
        Binding(
            get: {
                channels.first(where: { $0.stableID == channel.stableID })?.allowCommands
                    ?? channel.allowCommands
            },
            set: { allowCommands in
                guard let id = channel.id else {
                    commandResults[channel.stableID] = "Commands unavailable: channel has no saved ID"
                    commandErrors.insert(channel.stableID)
                    return
                }
                Task { await setCommands(channelID: id, allowCommands: allowCommands) }
            })
    }

    private func setCommands(channelID: String, allowCommands: Bool) async {
        guard let client = client() else {
            commandResults[channelID] = "Update failed: no daemon configuration"
            commandErrors.insert(channelID)
            return
        }
        updatingCommandIDs.insert(channelID)
        commandResults[channelID] = nil
        commandErrors.remove(channelID)
        defer { updatingCommandIDs.remove(channelID) }
        do {
            let response = try await client.setChannelCommands(
                channelId: channelID, allowCommands: allowCommands)
            guard response.ok, let updated = response.channel else {
                commandResults[channelID] = "Update failed: \(response.error ?? "no channel returned")"
                commandErrors.insert(channelID)
                return
            }
            if let index = channels.firstIndex(where: { $0.stableID == channelID }) {
                channels[index] = updated
            } else {
                await loadChannels(using: client)
            }
            commandResults[channelID] = allowCommands ? "Commands enabled" : "Commands disabled"
        } catch is CancellationError {
        } catch {
            commandResults[channelID] = "Update failed: \(error.localizedDescription)"
            commandErrors.insert(channelID)
        }
    }

    private func refreshMessages() async {
        guard let client = client() else {
            messagesError = "No Alex daemon configuration was found."
            return
        }
        await loadMessages(using: client)
    }

    private func loadMessages(using client: AlexClient) async {
        isLoadingMessages = true
        messagesError = nil
        defer { isLoadingMessages = false }
        do {
            messages = Array(try await client.notificationsLog(limit: 50).messages.reversed())
        } catch is CancellationError {
        } catch {
            messagesError = error.localizedDescription
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

    private static func activityTime(_ milliseconds: Int64) -> String {
        Date(timeIntervalSince1970: Double(milliseconds) / 1_000)
            .formatted(date: .omitted, time: .shortened)
    }
}

#if DEBUG
#Preview {
    NotificationsPreferencesSection(store: SnapshotStore())
        .frame(width: 780, height: 620)
}
#endif
