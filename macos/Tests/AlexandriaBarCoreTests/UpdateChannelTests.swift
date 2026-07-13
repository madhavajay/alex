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
}
