import AppKit

@MainActor
enum TerminalLauncher {
    enum TerminalApp: String, CaseIterable {
        case ghostty
        case iterm
        case terminal

        var bundleId: String {
            switch self {
            case .ghostty: "com.mitchellh.ghostty"
            case .iterm: "com.googlecode.iterm2"
            case .terminal: "com.apple.Terminal"
            }
        }

        var displayName: String {
            switch self {
            case .ghostty: "Ghostty"
            case .iterm: "iTerm2"
            case .terminal: "Terminal"
            }
        }

        var appURL: URL? {
            if let url = NSWorkspace.shared.urlForApplication(withBundleIdentifier: bundleId) {
                return url
            }
            let fallback = URL(fileURLWithPath: "/Applications/\(displayName == "iTerm2" ? "iTerm" : displayName).app")
            return FileManager.default.fileExists(atPath: fallback.path) ? fallback : nil
        }

        var isInstalled: Bool { appURL != nil }
    }

    static var installedApps: [TerminalApp] {
        TerminalApp.allCases.filter(\.isInstalled)
    }

    static var resolved: TerminalApp {
        let setting = UserDefaults.standard.string(forKey: "terminalApp") ?? "auto"
        if setting != "auto", let app = TerminalApp(rawValue: setting), app.isInstalled {
            return app
        }
        // Auto: prefer Ghostty when it is installed, then any other installed
        // terminal, then Terminal.app. (Launching into a running Ghostty is handled
        // by launchGhostty, which falls back to Terminal when Ghostty can't take a
        // command.)
        if TerminalApp.ghostty.isInstalled {
            return .ghostty
        }
        return installedApps.first ?? .terminal
    }

    private static var loginShell: String {
        ProcessInfo.processInfo.environment["SHELL"] ?? "/bin/zsh"
    }

    static func launch(command: String) {
        switch resolved {
        case .ghostty: launchGhostty(command)
        case .iterm: launchITerm(command)
        case .terminal: launchTerminal(command)
        }
    }

    static var ghosttyIsRunning: Bool {
        !NSRunningApplication.runningApplications(
            withBundleIdentifier: TerminalApp.ghostty.bundleId
        ).isEmpty
    }

    private static func launchGhostty(_ command: String) {
        guard let url = TerminalApp.ghostty.appURL else { return launchTerminal(command) }
        // Ghostty has no macOS IPC to open a command window in a running
        // instance, and `open -n` spawns a second instance that restores
        // every saved window. Only launch Ghostty cold; otherwise use Terminal.
        guard !ghosttyIsRunning else { return launchTerminal(command) }
        let proc = Process()
        proc.executableURL = URL(fileURLWithPath: "/usr/bin/open")
        proc.arguments = ["-a", url.path, "--args", "-e", loginShell, "-lc", holdOpen(command)]
        try? proc.run()
    }

    private static func launchITerm(_ command: String) {
        let full = "\(loginShell) -lc \(shellQuote(holdOpen(command)))"
        runAppleScript("""
        tell application "iTerm"
            activate
            create window with default profile command "\(escapeAppleScript(full))"
        end tell
        """)
    }

    private static func launchTerminal(_ command: String) {
        runAppleScript("""
        tell application "Terminal"
            activate
            do script "\(escapeAppleScript(command))"
        end tell
        """)
    }

    private static func holdOpen(_ command: String) -> String {
        "\(command); echo; read -s -k \"?— finished, press any key to close —\"" + " 2>/dev/null || read -p \"— finished, press enter to close —\""
    }

    private static func shellQuote(_ s: String) -> String {
        "'" + s.replacingOccurrences(of: "'", with: "'\\''") + "'"
    }

    private static func escapeAppleScript(_ s: String) -> String {
        s.replacingOccurrences(of: "\\", with: "\\\\")
            .replacingOccurrences(of: "\"", with: "\\\"")
    }

    private static func runAppleScript(_ source: String) {
        let proc = Process()
        proc.executableURL = URL(fileURLWithPath: "/usr/bin/osascript")
        proc.arguments = ["-e", source]
        try? proc.run()
    }
}
