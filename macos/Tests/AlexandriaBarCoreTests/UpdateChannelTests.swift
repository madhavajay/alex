import XCTest
@testable import AlexandriaBarCore

final class UpdateChannelTests: XCTestCase {
    func testFromRawValue() {
        XCTAssertEqual(UpdateChannelSetting.from("stable"), .stable)
        XCTAssertEqual(UpdateChannelSetting.from("beta"), .beta)
        XCTAssertEqual(UpdateChannelSetting.from(nil), .stable)
        XCTAssertEqual(UpdateChannelSetting.from("nightly"), .stable)
        XCTAssertEqual(UpdateChannelSetting.from(""), .stable)
    }

    func testStableChannelUsesBakedFeed() {
        XCTAssertNil(
            UpdateChannelSetting.stable.feedURLString(
                stableFeed: "https://madhavajay.github.io/alex/appcast.xml"))
    }

    func testBetaChannelDerivesBetaFeed() {
        XCTAssertEqual(
            UpdateChannelSetting.beta.feedURLString(
                stableFeed: "https://madhavajay.github.io/alex/appcast.xml"),
            "https://madhavajay.github.io/alex/appcast-beta.xml")
    }

    func testBetaChannelRefusesUnrecognizedFeed() {
        XCTAssertNil(
            UpdateChannelSetting.beta.feedURLString(
                stableFeed: "https://example.test/updates/feed.rss"))
    }

    func testScopeDefaultTargetsBothAppAndDaemon() {
        // The default path is one selection = both, so the app picker can never
        // leave the daemon on a different channel.
        XCTAssertTrue(UpdateChannelScope.both.appliesToApp)
        XCTAssertTrue(UpdateChannelScope.both.appliesToDaemon)
    }

    func testScopeOverridesTargetExactlyOneSide() {
        XCTAssertTrue(UpdateChannelScope.app.appliesToApp)
        XCTAssertFalse(UpdateChannelScope.app.appliesToDaemon)
        XCTAssertFalse(UpdateChannelScope.daemon.appliesToApp)
        XCTAssertTrue(UpdateChannelScope.daemon.appliesToDaemon)
    }

    func testDaemonChannelResponseDecodesGetAndPostShapes() throws {
        // GET carries only `channel`.
        let get = try JSONDecoder().decode(
            DaemonChannelResponse.self, from: Data(#"{"channel":"beta"}"#.utf8))
        XCTAssertEqual(get.channel, "beta")
        XCTAssertNil(get.updateAvailable)

        // POST also carries the recomputed availability.
        let post = try JSONDecoder().decode(
            DaemonChannelResponse.self,
            from: Data(#"{"channel":"beta","update_available":true,"latest":"0.2.0-beta.1","update_channel":"beta"}"#.utf8))
        XCTAssertEqual(post.channel, "beta")
        XCTAssertEqual(post.updateAvailable, true)
        XCTAssertEqual(post.latest, "0.2.0-beta.1")
    }
}
