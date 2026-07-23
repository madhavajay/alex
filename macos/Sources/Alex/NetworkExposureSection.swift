import AppKit
import SwiftUI
import AlexCore

struct NetworkExposureSection: View {
    let store: SnapshotStore

    @State private var networkExposure = "loopback"
    @State private var selectedInterfaceAddress = ""
    @State private var networkInterfaces: [NetworkInterfaceAddress] = []
    @State private var savingNetworkExposure = false
    @State private var networkExposureStatus: String?

    var body: some View {
        Group {
            SectionLabel(text: "Network exposure")
                .settingsSectionSpacing()
            SettingRow(label: "Listen on") {
                Picker("", selection: $networkExposure) {
                    Text("Loopback only (recommended)").tag("loopback")
                    Text("A specific interface").tag("interface")
                    Text("All interfaces").tag("all")
                }
                .settingsPicker()
                .onChange(of: networkExposure) { saveNetworkExposure() }
            }

            if networkExposure == "interface" {
                if networkInterfaces.isEmpty {
                    SettingCaption("No non-loopback interface addresses are available.")
                } else {
                    RowDivider()
                    SettingRow(
                        label: "Interface",
                        hint: "A LAN address can change under DHCP. If the saved address is unavailable at startup, Alex reports it loudly and falls back to loopback."
                    ) {
                        Picker("", selection: $selectedInterfaceAddress) {
                            ForEach(networkInterfaces) { interface in
                                Text(interface.displayName).tag(interface.address)
                            }
                        }
                        .settingsPicker()
                        .onChange(of: selectedInterfaceAddress) { saveNetworkExposure() }
                    }
                }
            }

            if networkExposure != "loopback" {
                VStack(alignment: .leading, spacing: 5) {
                    Label("Remote admin access enabled", systemImage: "exclamationmark.triangle.fill")
                        .fontWeight(.bold)
                        .foregroundStyle(.red)
                    (Text("This exposes Alex's admin API — your credential vault, key minting, and data reset — to that network. Anyone who can reach this port ")
                        + Text("and has your local key").bold()
                        + Text(" can control Alex and delete your data. Rotate your local key when enabling this."))
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(.red)
                }
                .padding(10)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(.red.opacity(0.12), in: RoundedRectangle(cornerRadius: AlexTheme.Radius.md))
                .padding(.vertical, 8)
            }

            if savingNetworkExposure {
                ProgressView("Saving network exposure…")
                    .controlSize(.small)
                    .padding(.vertical, 4)
            }
            if let networkExposureStatus {
                SettingCaption(networkExposureStatus)
            }
            if networkExposure != "loopback" {
                PillButton(
                    title: "Restart daemon service to apply", variant: .bordered,
                    isEnabled: !savingNetworkExposure
                ) {
                    restartDaemonService()
                }
                .help("Network exposure is not live until alex service restart completes.")
                .padding(.vertical, 6)
            }
        }
        .onAppear { loadNetworkExposure() }
        .onChange(of: store.config?.host) { loadNetworkExposure() }
    }

    private func loadNetworkExposure() {
        networkInterfaces = NetworkInterfaces.addresses()
        guard let host = store.config?.host else { return }
        switch host {
        case "127.0.0.1", "localhost", "::1", "[::1]", "":
            networkExposure = "loopback"
        case "0.0.0.0", "::", "*":
            networkExposure = "all"
        default:
            networkExposure = "interface"
            selectedInterfaceAddress = host
        }
        if selectedInterfaceAddress.isEmpty {
            selectedInterfaceAddress = networkInterfaces.first?.address ?? ""
        }
    }

    private func saveNetworkExposure() {
        let target: String
        switch networkExposure {
        case "all": target = "all"
        case "interface":
            guard !selectedInterfaceAddress.isEmpty else { return }
            target = selectedInterfaceAddress
        default: target = "loopback"
        }
        savingNetworkExposure = true
        networkExposureStatus = nil
        Task {
            let result = await DaemonController.run(args: ["service", "bind", target])
            savingNetworkExposure = false
            if result.ok {
                networkExposureStatus = "Saved. Restart the daemon service to apply this network exposure."
                await store.refresh()
            } else {
                NSSound.beep()
                networkExposureStatus = result.combined.isEmpty
                    ? "Could not save network exposure."
                    : result.combined
            }
        }
    }

    private func restartDaemonService() {
        savingNetworkExposure = true
        networkExposureStatus = "Restarting daemon service…"
        Task {
            let result = await DaemonController.run(args: ["service", "restart"])
            savingNetworkExposure = false
            if result.ok {
                networkExposureStatus = "Daemon service restarted with the saved network exposure."
                await store.refresh()
            } else {
                NSSound.beep()
                networkExposureStatus = result.combined.isEmpty
                    ? "Could not restart the daemon service."
                    : result.combined
            }
        }
    }
}
