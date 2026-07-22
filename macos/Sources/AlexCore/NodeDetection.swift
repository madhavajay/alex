import Foundation

public enum NodeDetection {
    public static func findNode() -> String? {
        let fm = FileManager.default
        let home = fm.homeDirectoryForCurrentUser.path
        var candidates = [
            "/opt/homebrew/bin/node",
            "/usr/local/bin/node",
            "\(home)/.volta/bin/node",
            "\(home)/.local/bin/node",
            "/usr/bin/node",
        ]
        let pathDirs = ProcessInfo.processInfo.environment["PATH"]?
            .split(separator: ":").map(String.init) ?? []
        for dir in Self.probeSafeDirectories(pathDirs, home: home) {
            candidates.append("\(dir)/node")
        }
        let nvmVersions = "\(home)/.nvm/versions/node"
        if let versions = try? fm.contentsOfDirectory(atPath: nvmVersions) {
            for version in versions.sorted().reversed() {
                candidates.append("\(nvmVersions)/\(version)/bin/node")
            }
        }
        let fnmVersions = "\(home)/.fnm/node-versions"
        if let versions = try? fm.contentsOfDirectory(atPath: fnmVersions) {
            for version in versions.sorted().reversed() {
                candidates.append("\(fnmVersions)/\(version)/installation/bin/node")
            }
        }
        return candidates.first { fm.isExecutableFile(atPath: $0) }
    }

    /// Filters PATH entries down to ones safe to stat without a TCC prompt.
    /// Probing a network volume or a protected user folder makes macOS ask
    /// "Alex wants to access files on a network volume / Desktop / …" at every
    /// launch after an update — for a probe that never finds anything a normal
    /// node install wouldn't put in the well-known locations above.
    public static func probeSafeDirectories(_ dirs: [String], home: String) -> [String] {
        let protected = ["Desktop", "Documents", "Downloads"].map { "\(home)/\($0)" }
        return dirs.filter { dir in
            if dir.hasPrefix("/Volumes/") || dir == "/Volumes" { return false }
            return !protected.contains { dir == $0 || dir.hasPrefix("\($0)/") }
        }
    }
}
