import Testing
@testable import AlexandriaBarCore

@Suite struct NodeDetectionTests {
    @Test func probeSkipsNetworkVolumesAndProtectedFolders() {
        let home = "/Users/tester"
        let dirs = [
            "/opt/homebrew/bin",
            "/Volumes/NAS/bin",
            "/Volumes",
            "\(home)/Desktop/tools",
            "\(home)/Documents",
            "\(home)/Downloads/node/bin",
            "\(home)/.local/bin",
            "/usr/bin",
            "\(home)/DesktopNot",
        ]
        #expect(NodeDetection.probeSafeDirectories(dirs, home: home) == [
            "/opt/homebrew/bin",
            "\(home)/.local/bin",
            "/usr/bin",
            "\(home)/DesktopNot",
        ])
    }
}
