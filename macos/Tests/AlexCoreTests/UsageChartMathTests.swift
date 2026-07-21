import Foundation
import Testing
@testable import AlexCore

@Suite struct UsageChartMathTests {
    @Test func scaleMatchesMockDomain() {
        // Mock chart peaks around 5.2×10⁷ tokens → domain [0,6e7], ticks 0/2/4/6.
        let scale = UsageChartScale(maxValue: 5.2e7)
        #expect(scale.upper == 6e7)
        #expect(scale.ticks == [0, 2e7, 4e7, 6e7])
    }

    @Test func scaleHandlesDegenerateInput() {
        #expect(UsageChartScale(maxValue: 0).ticks == [0, 1, 2, 3])
        #expect(UsageChartScale(maxValue: -5).ticks == [0, 1, 2, 3])
        #expect(UsageChartScale(maxValue: .nan).upper == 3)
    }

    @Test func scaleCoversMaxValue() {
        for max in [1.0, 7.0, 99.0, 1234.0, 3.1e5, 9.9e8] {
            let scale = UsageChartScale(maxValue: max)
            #expect(scale.upper >= max)
            #expect(scale.ticks.count == 4)
            #expect(scale.ticks[0] == 0)
        }
    }

    @Test func niceStepPicksSmallestNiceValue() {
        #expect(UsageChartScale.niceStep(1.734e7) == 2e7)
        #expect(UsageChartScale.niceStep(2.1e7) == 2.5e7)
        #expect(UsageChartScale.niceStep(2e7) == 2e7)
        #expect(UsageChartScale.niceStep(6e6) == 1e7)
        #expect(UsageChartScale.niceStep(3.0) == 5.0)
    }

    @Test func tickLabels() {
        #expect(UsageChartScale.tickLabel(0) == "0")
        #expect(UsageChartScale.tickLabel(2e7) == "2×10⁷")
        #expect(UsageChartScale.tickLabel(2.5e7) == "2.5×10⁷")
        #expect(UsageChartScale.tickLabel(6e7) == "6×10⁷")
        #expect(UsageChartScale.tickLabel(2000) == "2000")
    }

    @Test func axisLabelIndicesShowRoughlyFiveLabels() {
        #expect(UsageChartMath.axisLabelIndices(count: 0) == [])
        #expect(UsageChartMath.axisLabelIndices(count: 3) == [0, 1, 2])
        #expect(UsageChartMath.axisLabelIndices(count: 24) == [0, 4, 8, 12, 16, 20])
        #expect(UsageChartMath.axisLabelIndices(count: 30) == [0, 6, 12, 18, 24])
    }

    @Test func axisLabelFormats() {
        let timeZone = TimeZone(identifier: "UTC")!
        let locale = Locale(identifier: "en_US_POSIX")
        // 2026-07-12 13:00:00 UTC
        let bucketMs: Int64 = 1_783_861_200_000
        #expect(
            UsageChartMath.axisLabel(
                bucketMs: bucketMs, hourly: true, timeZone: timeZone, locale: locale)
                == "13:00")
        #expect(
            UsageChartMath.axisLabel(
                bucketMs: bucketMs, hourly: false, timeZone: timeZone, locale: locale)
                == "Jul 12")
    }

    @Test func axisLabelUsesBucketSizeForHourlyAndDailyFormats() {
        let timeZone = TimeZone(identifier: "UTC")!
        let locale = Locale(identifier: "en_US_POSIX")
        // 2026-07-12 13:00:00 UTC
        let bucketMs: Int64 = 1_783_861_200_000
        #expect(
            UsageChartMath.axisLabel(
                bucketMs: bucketMs,
                bucketSizeMs: 60 * 60 * 1_000,
                timeZone: timeZone,
                locale: locale)
                == "13:00")
        #expect(
            UsageChartMath.axisLabel(
                bucketMs: bucketMs,
                bucketSizeMs: 6 * 60 * 60 * 1_000,
                timeZone: timeZone,
                locale: locale)
                == "Jul 12")
        #expect(
            UsageChartMath.axisLabel(
                bucketMs: bucketMs,
                bucketSizeMs: 24 * 60 * 60 * 1_000,
                timeZone: timeZone,
                locale: locale)
                == "Jul 12")
    }

    /// A large-positive offset from UTC pushes a UTC-aligned bucket's
    /// *clock time* across midnight, but the *date* label must not follow
    /// — the daemon groups buckets by UTC, so rendering the date in the
    /// viewer's local zone would split one UTC-aligned run of buckets
    /// across two calendar-day labels. Captured against a real 6-hour
    /// bucket from a `/admin/accounts/analytics?bucket_minutes=360`
    /// response (the last bucket of UTC day Jul 14): in AEST (UTC+10) its
    /// clock time is 2026-07-15 04:00, one calendar day later, which is
    /// exactly the rollover that made the 7d chart's dates look wrong.
    @Test func axisLabelDailyGranularityIgnoresLocalTimeZone() {
        let aest = TimeZone(identifier: "Australia/Brisbane")!
        let locale = Locale(identifier: "en_US_POSIX")
        // 2026-07-14 18:00:00 UTC — last 6h bucket of the UTC day Jul 14.
        // Rendered in AEST (UTC+10) clock time this is 2026-07-15 04:00,
        // which is why local-time day labels rolled this into "Jul 15".
        let bucketMs: Int64 = 1_784_052_000_000
        #expect(
            UsageChartMath.axisLabel(
                bucketMs: bucketMs,
                bucketSizeMs: 6 * 60 * 60 * 1_000,
                timeZone: aest,
                locale: locale)
                == "Jul 14")
        // Same bucket, daily (30d) granularity: still Jul 14 in UTC.
        #expect(
            UsageChartMath.axisLabel(
                bucketMs: bucketMs,
                bucketSizeMs: 24 * 60 * 60 * 1_000,
                timeZone: aest,
                locale: locale)
                == "Jul 14")
        // Hourly buckets are unaffected — they still show local wall time.
        #expect(
            UsageChartMath.axisLabel(
                bucketMs: bucketMs,
                bucketSizeMs: 60 * 60 * 1_000,
                timeZone: aest,
                locale: locale)
                == "04:00")
    }

    /// Real `/admin/accounts/analytics?since_minutes=10080&bucket_minutes=360`
    /// response: `since_ms` is a raw "now minus 7d" cutoff, not itself
    /// bucket-aligned, yet the daemon's earliest returned bucket was
    /// 2026-07-08T06:00:00Z — the bucket *containing* since_ms
    /// (2026-07-08T11:45:14Z), i.e. floor-aligned to the 6-hour grid.
    @Test func canonicalBucketsMatchesRealDaemonAlignment7d() {
        let sinceMs: Int64 = 1_783_511_114_641 // 2026-07-08T11:45:14.641Z
        let bucketMs: Int64 = 21_600_000 // 6h
        let nowMs: Int64 = 1_784_115_776_000 // 2026-07-15T11:42:56Z (curl capture time)
        let buckets = UsageChartMath.canonicalBuckets(
            sinceMs: sinceMs, bucketMs: bucketMs, nowMs: nowMs)
        #expect(buckets.first == 1_783_490_400_000) // 2026-07-08T06:00:00Z, observed daemon minimum
        #expect(buckets.last == 1_784_095_200_000) // floor(nowMs to 6h) = 2026-07-15T06:00:00Z
        // Fully contiguous — no gaps — regardless of which accounts have data.
        for index in 1..<buckets.count {
            #expect(buckets[index] - buckets[index - 1] == bucketMs)
        }
    }

    /// Same shape for the 30d/1440m (daily) response: `since_ms` floors to
    /// 2026-06-15T00:00:00Z even though the raw cutoff was 11:45:14Z.
    @Test func canonicalBucketsMatchesRealDaemonAlignment30d() {
        let sinceMs: Int64 = 1_781_523_914_717 // 2026-06-15T11:45:14.717Z
        let bucketMs: Int64 = 86_400_000 // 1 day
        let nowMs: Int64 = 1_784_115_776_000
        let buckets = UsageChartMath.canonicalBuckets(
            sinceMs: sinceMs, bucketMs: bucketMs, nowMs: nowMs)
        #expect(buckets.first == 1_781_481_600_000) // 2026-06-15T00:00:00Z
        #expect(buckets.last == 1_784_073_600_000) // 2026-07-15T00:00:00Z
        #expect(buckets.count == 31)
    }

    @Test func canonicalBucketsHandlesDegenerateInput() {
        #expect(UsageChartMath.canonicalBuckets(sinceMs: 100, bucketMs: 0, nowMs: 200) == [])
        #expect(UsageChartMath.canonicalBuckets(sinceMs: 200, bucketMs: 10, nowMs: 100) == [])
    }

    /// Real observed history: tracing/account activity starts
    /// 2026-07-07T00:00:00Z (the earliest non-empty bucket in a
    /// 30d/1440m fetch captured 2026-07-15T11:42:56Z) — about 9 days of
    /// data. 7d and 30d both reveal data a shorter tab wouldn't show;
    /// "All" (same 43_200-minute span as 30d today) does not.
    @Test func enabledRangesMatchesRealNineDayHistory() {
        let spans = [1_440, 10_080, 43_200, 43_200] // 24h, 7d, 30d, All
        let earliestActivityMs: Int64 = 1_783_382_400_000 // 2026-07-07T00:00:00Z
        let nowMs: Int64 = 1_784_115_776_000 // 2026-07-15T11:42:56Z
        let enabled = UsageChartMath.enabledRanges(
            spansMinutes: spans, earliestActivityMs: earliestActivityMs, nowMs: nowMs)
        #expect(enabled == [true, true, true, false])
    }

    @Test func enabledRangesDisablesLongerTabsForFreshHistory() {
        let spans = [1_440, 10_080, 43_200, 43_200]
        let nowMs: Int64 = 1_784_115_776_000
        // Tracing started 3 hours ago — nothing beyond 24h has anything new.
        let earliestActivityMs = nowMs - 3 * 60 * 60 * 1_000
        let enabled = UsageChartMath.enabledRanges(
            spansMinutes: spans, earliestActivityMs: earliestActivityMs, nowMs: nowMs)
        #expect(enabled == [true, false, false, false])
    }

    @Test func enabledRangesFailsOpenWhenUnknown() {
        let spans = [1_440, 10_080, 43_200, 43_200]
        #expect(
            UsageChartMath.enabledRanges(spansMinutes: spans, earliestActivityMs: nil, nowMs: 1_000)
                == [true, true, true, true])
    }

    @Test func millionsLabel() {
        #expect(UsageChartMath.millionsLabel(33_000_000) == "33.0M tokens")
        #expect(UsageChartMath.millionsLabel(9_240_000) == "9.2M tokens")
        #expect(UsageChartMath.millionsLabel(0) == "0.0M tokens")
    }

    @Test func monotoneTangentsFlatDataIsZero() {
        #expect(UsageChartMath.monotoneTangents([5, 5, 5, 5]) == [0, 0, 0, 0])
        #expect(UsageChartMath.monotoneTangents([1]) == [0])
        #expect(UsageChartMath.monotoneTangents([]) == [])
    }

    @Test func monotoneTangentsZeroAtLocalExtremum() {
        let tangents = UsageChartMath.monotoneTangents([0, 10, 0])
        #expect(tangents[1] == 0)
    }

    @Test func monotoneTangentsPreserveDirection() {
        let tangents = UsageChartMath.monotoneTangents([0, 1, 4, 9, 16])
        #expect(tangents.allSatisfy { $0 >= 0 })
        // Fritsch–Carlson bound: |m/delta| <= 3 keeps interpolation monotone.
        let values = [0.0, 1, 4, 9, 16]
        for index in 0..<(values.count - 1) {
            let delta = values[index + 1] - values[index]
            #expect(abs(tangents[index] / delta) <= 3 + 1e-9)
            #expect(abs(tangents[index + 1] / delta) <= 3 + 1e-9)
        }
    }
}
