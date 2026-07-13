import Testing
@testable import AlexandriaBarCore

@Suite struct HarnessMenuTests {
    private func harness(
        _ name: String,
        installed: Bool,
        connected: Bool
    ) -> Harness {
        Harness(
            name: name,
            installed: installed,
            binary: installed ? "/tmp/\(name)" : nil,
            version: nil,
            versionWarning: nil,
            configDir: connected ? "/tmp/config" : nil,
            configDirExists: connected,
            connected: connected,
            supportsConnect: name == "pi",
            override: nil,
            daemonReachable: true)
    }

    @Test func connectedPiRemainsVisibleWhenBinaryIsNotDetected() {
        let rows = HarnessCatalog.menuBarRows([
            harness("pi", installed: false, connected: true),
        ])
        #expect(rows.map(\.name) == ["pi"])
        #expect(rows[0].connected)
        #expect(!rows[0].installed)
    }

    @Test func menuIncludesInstalledPiAndExcludesMissingOrOtherHarnesses() {
        let rows = HarnessCatalog.menuBarRows([
            harness("pi", installed: true, connected: false),
            harness("claude", installed: true, connected: true),
        ])
        #expect(rows.map(\.name) == ["pi"])
        #expect(HarnessCatalog.menuBarRows([
            harness("pi", installed: false, connected: false),
        ]).isEmpty)
    }
}
