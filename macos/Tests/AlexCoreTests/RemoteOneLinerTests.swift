import Foundation
import Testing
@testable import AlexCore

@Suite struct RemoteOneLinerTests {
    @Test func buildsEveryCatalogHarness() {
        for harness in HarnessCatalog.names {
            let command = RemoteOneLiner.build(
                harness: harness,
                baseURL: "http://192.168.1.42:4100/",
                key: "alxk-fresh")
            #expect(command.hasSuffix(
                "alex up \(harness) --url http://192.168.1.42:4100 --key alxk-fresh\""))
        }
    }

    @Test func includesQuickstartInstallerFallback() {
        let command = RemoteOneLiner.build(
            harness: "codex", baseURL: "https://alex.example.net", key: "alxk-123")
        #expect(command.hasPrefix("sh -c \"command -v alex >/dev/null || "))
        #expect(command.contains(
            "curl -fsSL https://raw.githubusercontent.com/madhavajay/alex/main/install-release.sh | sh;"))
    }

    @Test func quotesShellMetacharactersAcrossBothShells() {
        let command = RemoteOneLiner.build(
            harness: "team's harness; echo unsafe",
            baseURL: "https://alex.example/path?next=$HOME&mode=remote",
            key: "alxk-$(touch /tmp/should-not-run)")
        #expect(command.contains("alex up 'team'\\\\''s harness; echo unsafe'"))
        #expect(command.contains("--url 'https://alex.example/path?next=\\$HOME&mode=remote'"))
        #expect(command.contains("--key 'alxk-\\$(touch /tmp/should-not-run)'"))
    }

    @Test func selectsConfiguredOrConcreteLANAddress() {
        let specific = DaemonConfig(
            host: "100.101.102.103", port: 52415, localKey: "local")
        #expect(RemoteOneLiner.daemonBaseURL(config: specific).absoluteString
            == "http://100.101.102.103:52415")

        let all = DaemonConfig(host: "0.0.0.0", port: 4100, localKey: "local")
        #expect(RemoteOneLiner.daemonBaseURL(
            config: all, availableLANHosts: ["192.168.50.4"]).absoluteString
            == "http://192.168.50.4:4100")

        let local = DaemonConfig(host: "localhost", port: 4100, localKey: "local")
        #expect(RemoteOneLiner.daemonBaseURL(
            config: local, availableLANHosts: ["192.168.50.4"]).absoluteString
            == "http://localhost:4100")
    }

    @Test func optionsControlInstallKeyAndModel() {
        let bare = RemoteOneLiner.build(
            options: .init(
                harness: "claude", model: "alex/gpt-5.6-sol",
                includeInstall: false, includeKey: false),
            baseURL: "http://192.168.1.42:4100", key: nil)
        #expect(!bare.contains("curl"))
        #expect(!bare.contains("--key"))
        #expect(bare.contains("--model alex/gpt-5.6-sol"))
        #expect(bare.hasSuffix(
            "alex up claude --url http://192.168.1.42:4100 --model alex/gpt-5.6-sol\""))

        let keyed = RemoteOneLiner.build(
            options: .init(harness: "codex"),
            baseURL: "http://192.168.1.42:4100", key: "alxk-fresh")
        #expect(keyed.contains("command -v alex >/dev/null || curl"))
        #expect(keyed.hasSuffix(
            "alex up codex --url http://192.168.1.42:4100 --key alxk-fresh\""))
    }

    @Test func remoteAccessRankingPrefersPrimaryLANOverVirtualInterfaces() {
        let ranked = NetworkInterfaces.rankedForRemoteAccess([
            .init(name: "bridge100", address: "192.168.64.1"),
            .init(name: "utun3", address: "100.101.102.103"),
            .init(name: "en0", address: "192.168.1.150"),
            .init(name: "en5", address: "192.168.2.9"),
        ])
        #expect(ranked.map(\.address) == [
            "192.168.1.150", "192.168.2.9", "100.101.102.103", "192.168.64.1",
        ])
    }
}
