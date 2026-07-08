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

final class Collected: @unchecked Sendable {
    private let lock = NSLock()
    private var stdout = ""
    private var stderr = ""

    func append(_ line: String, stderr isErr: Bool) {
        lock.lock()
        defer { lock.unlock() }
        if isErr {
            stderr += line + "\n"
        } else {
            stdout += line + "\n"
        }
    }

    func snapshot() -> (String, String) {
        lock.lock()
        defer { lock.unlock() }
        return (stdout, stderr)
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

    public static func runStreaming(
        args: [String],
        timeout: TimeInterval = 120,
        onLine: @escaping @Sendable (String) -> Void
    ) async -> CommandResult {
        guard let bin = findBinary() else {
            let msg = "alexandria binary not found"
            onLine(msg)
            return CommandResult(exitCode: 127, stdout: "", stderr: msg)
        }
        let proc = Process()
        proc.executableURL = URL(fileURLWithPath: bin)
        proc.arguments = args
        let out = Pipe()
        let err = Pipe()
        proc.standardOutput = out
        proc.standardError = err
        let collected = Collected()

        func attach(_ pipe: Pipe, stderr: Bool) {
            nonisolated(unsafe) var buffer = Data()
            pipe.fileHandleForReading.readabilityHandler = { handle in
                let data = handle.availableData
                if data.isEmpty {
                    handle.readabilityHandler = nil
                    if let line = String(data: buffer, encoding: .utf8), !line.isEmpty {
                        collected.append(line, stderr: stderr)
                        onLine(stripANSI(line))
                    }
                    return
                }
                buffer.append(data)
                while let nl = buffer.firstIndex(of: 0x0A) {
                    let lineData = buffer[buffer.startIndex..<nl]
                    buffer.removeSubrange(buffer.startIndex...nl)
                    guard let line = String(data: lineData, encoding: .utf8) else { continue }
                    collected.append(line, stderr: stderr)
                    onLine(stripANSI(line))
                }
            }
        }
        attach(out, stderr: false)
        attach(err, stderr: true)

        let exited = AsyncStream<Void> { continuation in
            proc.terminationHandler = { _ in
                continuation.yield()
                continuation.finish()
            }
        }
        do {
            try proc.run()
        } catch {
            onLine("failed to launch: \(error.localizedDescription)")
            return CommandResult(exitCode: 126, stdout: "", stderr: error.localizedDescription)
        }
        let timeoutTask = Task.detached {
            try? await Task.sleep(for: .seconds(timeout))
            if proc.isRunning {
                onLine("— timed out after \(Int(timeout))s, terminating —")
                proc.terminate()
            }
        }
        for await _ in exited {}
        timeoutTask.cancel()
        let (stdout, stderr) = collected.snapshot()
        return CommandResult(exitCode: proc.terminationStatus, stdout: stdout, stderr: stderr)
    }

    public static func stripANSI(_ s: String) -> String {
        s.replacingOccurrences(
            of: "\u{1B}\\[[0-9;?]*[A-Za-z]", with: "", options: .regularExpression)
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
