import AppKit
import SwiftUI
import AlexandriaBarCore

struct PreferencesView: View {
    @AppStorage("refreshSeconds") private var refreshSeconds: Double = 60
    @AppStorage("limitWarnPct") private var limitWarnPct: Double = 90
    @AppStorage("notifyEnabled") private var notifyEnabled = true
    @AppStorage("binaryPath") private var binaryPath = ""
    @AppStorage("terminalApp") private var terminalApp = "auto"
    @AppStorage("menuIconStyle") private var menuIconStyle = "logo"

    var body: some View {
        Form {
            Section("Refresh") {
                Picker("Poll interval", selection: $refreshSeconds) {
                    Text("30 seconds").tag(30.0)
                    Text("1 minute").tag(60.0)
                    Text("5 minutes").tag(300.0)
                    Text("15 minutes").tag(900.0)
                }
            }
            Section("Menu Bar") {
                Picker("Icon", selection: $menuIconStyle) {
                    Text("Alexandria logo").tag("logo")
                    Text("Hieroglyph (𓂀)").tag("glyph")
                }
            }
            Section("Alerts") {
                Toggle("Show notifications", isOn: $notifyEnabled)
                Picker("Warn when a limit window reaches", selection: $limitWarnPct) {
                    Text("75%").tag(75.0)
                    Text("80%").tag(80.0)
                    Text("90%").tag(90.0)
                    Text("95%").tag(95.0)
                }
            }
            Section("Terminal") {
                Picker("Open commands in", selection: $terminalApp) {
                    Text("Auto (\(TerminalLauncher.resolved.displayName))").tag("auto")
                    ForEach(TerminalLauncher.installedApps, id: \.rawValue) { app in
                        Text(app.displayName).tag(app.rawValue)
                    }
                }
                if TerminalLauncher.resolved == .ghostty {
                    Text("Ghostty can't accept commands while already running — Terminal is used instead in that case.")
                        .font(.system(size: 10))
                        .foregroundStyle(.secondary)
                }
            }
            Section("Daemon") {
                TextField("alexandria binary path (blank = auto)", text: $binaryPath)
                    .font(.system(size: 11, design: .monospaced))
                LabeledContent("Config") {
                    Text(DaemonDiscovery.configPath.path)
                        .font(.system(size: 10, design: .monospaced))
                        .foregroundStyle(.secondary)
                        .textSelection(.enabled)
                }
            }
        }
        .formStyle(.grouped)
        .frame(width: 420)
        .fixedSize(horizontal: false, vertical: true)
    }
}

@MainActor
final class PreferencesWindowController {
    private var window: NSWindow?

    func show() {
        if window == nil {
            let host = NSHostingController(rootView: PreferencesView())
            let win = NSWindow(contentViewController: host)
            win.title = "AlexandriaBar Settings"
            win.styleMask = [.titled, .closable]
            win.isReleasedWhenClosed = false
            win.center()
            window = win
        }
        if let window {
            DockIconManager.shared.track(window)
            window.makeKeyAndOrderFront(nil)
        }
    }
}
