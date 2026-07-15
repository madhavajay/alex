import Foundation
import Testing
@testable import AlexandriaBarCore

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
