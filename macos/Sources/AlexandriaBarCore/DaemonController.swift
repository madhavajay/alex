import Foundation

public struct CommandResult: Sendable {
    public let exitCode: Int32
    public let stdout: String
    public let stderr: String
    public var ok: Bool { exitCode == 0 }
    public var combined: String {
        [stdout, stderr].filter { !$0.isEmpty }.joined(separator: "\n")
    }
}

public enum DaemonController {
    public static func findBinary() -> String? {
        let override = UserDefaults.standard.string(forKey: "binaryPath") ?? ""
        let home = FileManager.default.homeDirectoryForCurrentUser.path
        let candidates = [
            override,
            "\(home)/.local/bin/alexandria",
            "/usr/local/bin/alexandria",
            "/opt/homebrew/bin/alexandria",
            "\(home)/dev/alexandria/target/release/alexandria",
        ]
        return candidates.first { !$0.isEmpty && FileManager.default.isExecutableFile(atPath: $0) }
    }

    public static func run(args: [String], timeout: TimeInterval = 120) async -> CommandResult {
        guard let bin = findBinary() else {
            return CommandResult(exitCode: 127, stdout: "", stderr: "alexandria binary not found")
        }
        let proc = Process()
        proc.executableURL = URL(fileURLWithPath: bin)
        proc.arguments = args
        let out = Pipe()
        let err = Pipe()
        proc.standardOutput = out
        proc.standardError = err
        let exited = AsyncStream<Void> { continuation in
            proc.terminationHandler = { _ in
                continuation.yield()
                continuation.finish()
            }
        }
        do {
            try proc.run()
        } catch {
            return CommandResult(exitCode: 126, stdout: "", stderr: error.localizedDescription)
        }
        let timeoutTask = Task.detached {
            try? await Task.sleep(for: .seconds(timeout))
            if proc.isRunning { proc.terminate() }
        }
        for await _ in exited {}
        timeoutTask.cancel()
        let stdout = String(data: out.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8) ?? ""
        let stderr = String(data: err.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8) ?? ""
        return CommandResult(exitCode: proc.terminationStatus, stdout: stdout, stderr: stderr)
    }

    public static func startDaemon() async -> CommandResult {
        await run(args: ["daemon", "--background", "--nosplash"], timeout: 30)
    }

    public static func ping(_ target: String) async -> CommandResult {
        await run(args: ["ping", target], timeout: 60)
    }

    public static func importCredentials() async -> CommandResult {
        await run(args: ["auth", "import", "all"], timeout: 30)
    }

}
