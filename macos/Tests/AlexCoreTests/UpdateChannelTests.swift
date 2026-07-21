import XCTest
@testable import AlexCore

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

    // MARK: - B2: prerelease build → beta channel default

    func testIsPrereleaseRecognizesPreReleaseBuilds() {
        XCTAssertFalse(UpdateChannelSetting.isPrerelease(version: "0.1.28"))
        XCTAssertFalse(UpdateChannelSetting.isPrerelease(version: "v0.1.28"))
        XCTAssertTrue(UpdateChannelSetting.isPrerelease(version: "0.1.28-beta.3"))
        XCTAssertTrue(UpdateChannelSetting.isPrerelease(version: "0.1.28-rc.1"))
        XCTAssertTrue(UpdateChannelSetting.isPrerelease(version: "0.1.28-alpha.2"))
        // A non-version (e.g. "dev") is not a recognized pre-release.
        XCTAssertFalse(UpdateChannelSetting.isPrerelease(version: "dev"))
    }

    func testDefaultChannelDerivedFromBuildVersion() {
        // A stable build defaults to stable; a pre-release build defaults to
        // beta so refresh checks the beta appcast (B2).
        XCTAssertEqual(
            UpdateChannelSetting.defaultChannel(forRunningVersion: "0.1.28"), .stable)
        XCTAssertEqual(
            UpdateChannelSetting.defaultChannel(forRunningVersion: "0.1.28-beta.3"), .beta)
        XCTAssertEqual(
            UpdateChannelSetting.defaultChannel(forRunningVersion: "0.1.29-rc.1"), .beta)
    }

    /// Channel-resolution matrix from the app's perspective (spec rows 1-7):
    /// which appcast the app checks given its build version and stored choice.
    func testResolvedChannelMatrix() {
        // Rows 1 & 6: stable build, explicit/absent stable → stable.
        XCTAssertEqual(
            UpdateChannelSetting.resolved(rawStored: "stable", runningVersion: "0.1.27"), .stable)
        XCTAssertEqual(
            UpdateChannelSetting.resolved(rawStored: nil, runningVersion: "0.1.28"), .stable)

        // Row 2: stable build, user picked beta → beta.
        XCTAssertEqual(
            UpdateChannelSetting.resolved(rawStored: "beta", runningVersion: "0.1.27"), .beta)

        // Rows 3, 5, 7: beta build, explicit beta → beta.
        XCTAssertEqual(
            UpdateChannelSetting.resolved(rawStored: "beta", runningVersion: "0.1.28-beta.2"), .beta)

        // Row 4 — THE B2 bug: a beta build with NO explicit choice must resolve
        // to beta, not the stable default. This is what made the app compare
        // against the older latest stable and report "up to date".
        XCTAssertEqual(
            UpdateChannelSetting.resolved(rawStored: nil, runningVersion: "0.1.28-beta.2"), .beta)
        // An unrecognized stored value is treated as "no explicit choice".
        XCTAssertEqual(
            UpdateChannelSetting.resolved(rawStored: "nightly", runningVersion: "0.1.28-beta.2"),
            .beta)

        // A user who explicitly picked stable on a beta build is honored.
        XCTAssertEqual(
            UpdateChannelSetting.resolved(rawStored: "stable", runningVersion: "0.1.28-beta.2"),
            .stable)
    }

    /// End-to-end B2: a beta build with no explicit choice ends up pointed at
    /// the beta appcast, so a refresh really does check for newer betas.
    func testBetaBuildWithoutChoiceChecksBetaAppcast() {
        let channel = UpdateChannelSetting.resolved(
            rawStored: nil, runningVersion: "0.1.28-beta.2")
        XCTAssertEqual(
            channel.feedURLString(stableFeed: "https://madhavajay.github.io/alex/appcast.xml"),
            "https://madhavajay.github.io/alex/appcast-beta.xml")
    }

    // MARK: - B4: comparator matches the daemon's Rust ordering

    func testComparatorOrdersRealShippedVersions() {
        // A beta is older than its final release.
        XCTAssertEqual(AlexVersion.compare("0.1.28-beta.3", "0.1.28"), .orderedAscending)
        XCTAssertEqual(AlexVersion.compare("0.1.28", "0.1.28-beta.3"), .orderedDescending)
        // A higher beta number is newer — and it is NOT a string compare, so
        // beta.10 must beat beta.9 (the exact bug that bit the installer).
        XCTAssertEqual(AlexVersion.compare("0.1.28-beta.2", "0.1.28-beta.3"), .orderedAscending)
        XCTAssertEqual(AlexVersion.compare("0.1.28-beta.10", "0.1.28-beta.9"), .orderedDescending)
        // The next version's beta is newer than the current stable.
        XCTAssertEqual(AlexVersion.compare("0.1.27", "0.1.28-beta.1"), .orderedAscending)
        // Stage ordering: alpha < beta < rc < stable.
        XCTAssertEqual(AlexVersion.compare("0.1.28-alpha.1", "0.1.28-beta.1"), .orderedAscending)
        XCTAssertEqual(AlexVersion.compare("0.1.28-beta.1", "0.1.28-rc.1"), .orderedAscending)
        XCTAssertEqual(AlexVersion.compare("0.1.28-rc.1", "0.1.28"), .orderedAscending)
    }

    func testComparatorTreatsEquivalentFormsAsSame() {
        XCTAssertEqual(AlexVersion.compare("0.1.28", "0.1.28"), .orderedSame)
        XCTAssertEqual(AlexVersion.compare("v0.1.28", "0.1.28"), .orderedSame)
        XCTAssertEqual(AlexVersion.compare("0.1.28+build.9", "0.1.28"), .orderedSame)
        XCTAssertEqual(AlexVersion.compare("0.1.0", "0.1"), .orderedSame)
        XCTAssertEqual(AlexVersion.compare("0.1.28-beta.3-dirty", "0.1.28-beta.3"), .orderedSame)
    }

    func testComparatorParsesRobustlyLikeTheDaemon() {
        // Number-less -beta and rc suffixes parse (they must not be lost).
        XCTAssertNotNil(AlexVersion.parse("0.1.28-beta"))
        XCTAssertNotNil(AlexVersion.parse("0.1.29-rc.1"))
        XCTAssertEqual(AlexVersion.parse("0.1.28-beta")?.preNum, 0)
        XCTAssertTrue(AlexVersion.parse("0.1.28+build.9")?.isStable ?? false)
        // Genuinely unparseable core → nil (the app never treats this as
        // "same"/up-to-date; compare falls back to a numeric string order).
        XCTAssertNil(AlexVersion.parse("garbage"))
        XCTAssertNil(AlexVersion.parse(""))
    }
}
