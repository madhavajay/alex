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
        for dir in ProcessInfo.processInfo.environment["PATH"]?.split(separator: ":") ?? [] {
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
}
