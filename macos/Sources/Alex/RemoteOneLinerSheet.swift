import SwiftUI
import AlexCore

/// Customization sheet shown before copying a remote 1-liner. Defaults are
/// pre-seeded from the harness whose Copy button opened it; every axis the
/// command supports (harness, model, install, interface, auth) is adjustable.
@MainActor
@Observable
final class RemoteOneLinerSheetModel: Identifiable {
    struct InterfaceChoice: Identifiable, Hashable {
        let label: String
        let host: String?

        var id: String { host ?? "loopback" }
    }

    let config: DaemonConfig
    var harness: String
    var model = ""
    var includeInstall = true
    var includeKey = true
    var selectedInterfaceID: String
    var interfaces: [InterfaceChoice]
    var availableModels: [String] = []
    var copying = false
    var copied = false
    var error: String?

    init(harness: String, config: DaemonConfig) {
        self.harness = harness
        self.config = config
        let ranked = NetworkInterfaces.rankedForRemoteAccess(NetworkInterfaces.addresses())
        var choices = ranked.map {
            InterfaceChoice(label: $0.displayName, host: $0.address)
        }
        choices.append(InterfaceChoice(label: "Localhost (this Mac only)", host: nil))

        let configuredHost = config.host.trimmingCharacters(in: .whitespacesAndNewlines)
        let selected: String
        if config.lanEnabled, ranked.contains(where: { $0.address == configuredHost }) {
            selected = configuredHost
        } else if config.lanEnabled, !configuredHost.isEmpty,
                  !["0.0.0.0", "::", "[::]", "*"].contains(configuredHost.lowercased())
        {
            // A configured bind address that isn't currently enumerable —
            // offer it anyway; it is what the daemon actually listens on.
            choices.insert(
                InterfaceChoice(label: "Configured (\(configuredHost))", host: configuredHost),
                at: 0)
            selected = configuredHost
        } else {
            selected = choices.first?.id ?? "loopback"
        }
        interfaces = choices
        selectedInterfaceID = selected
    }

    var selectedInterface: InterfaceChoice? {
        interfaces.first { $0.id == selectedInterfaceID }
    }

    var selectedHostIsLoopback: Bool { selectedInterface?.host == nil }

    /// The daemon must actually listen on the interface the command embeds.
    var interfaceUnreachableWarning: String? {
        guard let host = selectedInterface?.host else {
            return "A 1-liner pointing at localhost only works on this Mac. Remote machines need a LAN or Tailscale address."
        }
        let bound = config.host.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        if ["0.0.0.0", "::", "[::]", "*"].contains(bound) { return nil }
        if !config.lanEnabled {
            return "The daemon currently listens on localhost only, so remote machines can't reach \(host). Change the listen interface in Settings → General first."
        }
        if bound != host.lowercased() {
            return "The daemon listens on \(config.host), not \(host). Remote machines can only reach the bound address."
        }
        return nil
    }

    var baseURL: URL {
        guard let host = selectedInterface?.host else { return config.baseURL }
        return RemoteOneLiner.url(host: host, port: config.port) ?? config.baseURL
    }

    var options: RemoteOneLiner.Options {
        RemoteOneLiner.Options(
            harness: harness,
            model: model.isEmpty ? nil : model,
            includeInstall: includeInstall,
            includeKey: includeKey)
    }

    var preview: String {
        RemoteOneLiner.build(
            options: options,
            baseURL: baseURL,
            key: includeKey ? "alxk-…minted-on-copy" : nil)
    }

    func loadModels() async {
        guard availableModels.isEmpty else { return }
        availableModels = (try? await AlexClient(config: config).modelCatalog()) ?? []
    }

    func copy() {
        guard !copying else { return }
        copying = true
        copied = false
        error = nil
        Task {
            defer { copying = false }
            do {
                try await RemoteOneLinerClipboard.copy(
                    options: options, baseURL: baseURL, config: config)
                copied = true
            } catch {
                NSSound.beep()
                self.error = error.localizedDescription
            }
        }
    }
}

struct RemoteOneLinerSheet: View {
    @Bindable var model: RemoteOneLinerSheetModel
    let onClose: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            VStack(alignment: .leading, spacing: 2) {
                Text("COPY REMOTE 1-LINER")
                    .font(AlexTheme.Fonts.metaMono)
                    .foregroundStyle(AlexTheme.Colors.primary)
                Text("Bootstrap \(HarnessCatalog.displayName(model.harness)) on another machine")
                    .font(.system(size: 16, weight: .semibold))
                    .foregroundStyle(AlexTheme.Colors.foreground)
            }

            optionRows
            commandPreview
            if let warning = model.interfaceUnreachableWarning {
                Label(warning, systemImage: "exclamationmark.triangle.fill")
                    .font(.system(size: 11))
                    .foregroundStyle(AlexTheme.Colors.warningOrange)
                    .fixedSize(horizontal: false, vertical: true)
            }
            if let error = model.error {
                Text(error)
                    .font(.system(size: 11))
                    .foregroundStyle(AlexTheme.Colors.destructive)
                    .fixedSize(horizontal: false, vertical: true)
            }

            HStack {
                Spacer()
                PillButton(title: "Close", variant: .bordered) { onClose() }
                PillButton(
                    title: model.copied ? "Copied" : "Mint key & copy",
                    variant: .solidAccent,
                    systemImage: model.copied ? "checkmark" : "doc.on.doc",
                    isEnabled: !model.copying,
                    isBusy: model.copying,
                    keyboardShortcut: .defaultAction
                ) { model.copy() }
            }
        }
        .padding(20)
        .frame(width: 560)
        .background(AlexTheme.Colors.background)
        .task { await model.loadModels() }
    }

    @ViewBuilder private var optionRows: some View {
        VStack(alignment: .leading, spacing: 10) {
            row("Harness") {
                Picker("", selection: $model.harness) {
                    ForEach(HarnessCatalog.names, id: \.self) { name in
                        Text(HarnessCatalog.displayName(name)).tag(name)
                    }
                }
                .labelsHidden()
                .frame(maxWidth: 240)
            }
            row("Default model") {
                Picker("", selection: $model.model) {
                    Text("Alex default").tag("")
                    ForEach(model.availableModels, id: \.self) { id in
                        Text(id).tag(id)
                    }
                }
                .labelsHidden()
                .frame(maxWidth: 300)
            }
            row("Interface") {
                Picker("", selection: $model.selectedInterfaceID) {
                    ForEach(model.interfaces) { choice in
                        Text(choice.label).tag(choice.id)
                    }
                }
                .labelsHidden()
                .frame(maxWidth: 300)
            }
            row("Install Alex if missing") {
                Toggle("", isOn: $model.includeInstall)
                    .labelsHidden()
                    .toggleStyle(.switch)
                    .controlSize(.small)
            }
            row("Include scoped auth key") {
                Toggle("", isOn: $model.includeKey)
                    .labelsHidden()
                    .toggleStyle(.switch)
                    .controlSize(.small)
            }
            if !model.includeKey {
                Text("Without a key the remote machine must already hold a scoped run key, or the daemon must allow unauthenticated model calls.")
                    .font(.system(size: 10.5))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .padding(14)
        .background(RoundedRectangle(cornerRadius: AlexTheme.Radius.md)
            .fill(AlexTheme.Colors.card))
        .overlay(RoundedRectangle(cornerRadius: AlexTheme.Radius.md)
            .strokeBorder(AlexTheme.Colors.cardBorder))
    }

    private var commandPreview: some View {
        ScrollView(.horizontal) {
            Text(model.preview)
                .font(AlexTheme.Fonts.mono(10.5))
                .foregroundStyle(AlexTheme.Colors.foreground)
                .textSelection(.enabled)
                .fixedSize(horizontal: true, vertical: false)
                .padding(10)
        }
        .background(RoundedRectangle(cornerRadius: AlexTheme.Radius.sm)
            .fill(AlexTheme.Colors.consoleBackground))
    }

    private func row<Content: View>(
        _ label: String, @ViewBuilder content: () -> Content
    ) -> some View {
        HStack {
            Text(label)
                .font(.system(size: 12, weight: .medium))
                .foregroundStyle(AlexTheme.Colors.foreground)
            Spacer()
            content()
        }
    }
}
