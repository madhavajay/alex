import Foundation

/// Y-axis domain for the usage line chart (ui/Accounts App.tsx:533: domain
/// [0,6]×10⁷, ticks [0,2,4,6]). Always four ticks including zero; the step is
/// rounded up to a "nice" value so tick labels stay short.
public struct UsageChartScale: Equatable, Sendable {
    public let upper: Double
    public let ticks: [Double]

    public init(maxValue: Double) {
        guard maxValue > 0, maxValue.isFinite else {
            upper = 3
            ticks = [0, 1, 2, 3]
            return
        }
        let step = Self.niceStep(maxValue / 3)
        upper = step * 3
        ticks = [0, step, step * 2, step * 3]
    }

    /// Smallest of {1, 2, 2.5, 5, 10}×10ᵏ that is ≥ raw.
    static func niceStep(_ raw: Double) -> Double {
        let magnitude = pow(10, floor(log10(raw)))
        for multiple in [1.0, 2.0, 2.5, 5.0] {
            let step = multiple * magnitude
            if step >= raw * (1 - 1e-9) { return step }
        }
        return 10 * magnitude
    }

    /// Mock Y-axis label form: 0 → "0", 2e7 → "2×10⁷"; values under 10⁴ render
    /// plainly ("2000").
    public static func tickLabel(_ value: Double) -> String {
        guard value > 0 else { return "0" }
        let exponent = Int(floor(log10(value) + 1e-9))
        guard exponent >= 4 else { return trimmed(value) }
        let coefficient = value / pow(10, Double(exponent))
        return trimmed(coefficient) + "×10" + superscript(exponent)
    }

    static func trimmed(_ value: Double) -> String {
        String(format: "%g", value)
    }

    static func superscript(_ value: Int) -> String {
        let digits: [Character] = ["⁰", "¹", "²", "³", "⁴", "⁵", "⁶", "⁷", "⁸", "⁹"]
        return String(value).map { character -> String in
            if let digit = character.wholeNumberValue, (0...9).contains(digit) {
                return String(digits[digit])
            }
            return character == "-" ? "⁻" : String(character)
        }.joined()
    }
}

public enum UsageChartMath {
    /// Show ~5 X-axis labels: every max(1, count/5)-th bucket, starting at 0
    /// (mock interval logic, Accounts App.tsx:532).
    public static func axisLabelIndices(count: Int) -> [Int] {
        guard count > 0 else { return [] }
        let step = max(1, count / 5)
        return Array(stride(from: 0, to: count, by: step))
    }

    /// Hourly buckets → "13:00"; daily and longer → "Jul 12".
    public static func axisLabel(
        bucketMs: Int64,
        hourly: Bool,
        timeZone: TimeZone = .current,
        locale: Locale = .current
    ) -> String {
        let date = Date(timeIntervalSince1970: Double(bucketMs) / 1_000)
        let formatter = DateFormatter()
        formatter.locale = locale
        formatter.timeZone = timeZone
        if hourly {
            formatter.dateFormat = "HH:mm"
        } else {
            formatter.setLocalizedDateFormatFromTemplate("MMM d")
        }
        return formatter.string(from: date)
    }

    /// Chooses an hourly or calendar-day label from the analytics bucket
    /// width. One-hour buckets show times in `timeZone` (the viewer's local
    /// wall clock — expected for an intraday view). Wider buckets show
    /// dates, but the daemon aligns those bucket boundaries to absolute UTC
    /// (e.g. 6-hour buckets fall on 00:00/06:00/12:00/18:00 UTC, daily
    /// buckets on UTC midnight — see `canonicalBuckets`), so the *date*
    /// portion is always derived in UTC regardless of `timeZone`. Deriving
    /// it from the viewer's local zone instead would, for any offset that
    /// doesn't evenly divide the bucket width, split a single UTC-aligned
    /// bucket run across two calendar-day labels (e.g. UTC+10 rolls the
    /// 18:00 UTC bucket of a 6-hour series into the next local day),
    /// producing a visibly wrong/duplicated date sequence on the 7d/30d
    /// charts.
    public static func axisLabel(
        bucketMs: Int64,
        bucketSizeMs: Int64,
        timeZone: TimeZone = .current,
        locale: Locale = .current
    ) -> String {
        let hourly = bucketSizeMs <= 60 * 60 * 1_000
        return axisLabel(
            bucketMs: bucketMs,
            hourly: hourly,
            timeZone: hourly ? timeZone : TimeZone(identifier: "UTC")!,
            locale: locale)
    }

    /// The full x-axis timeline for a usage chart: every bucket-aligned
    /// timestamp from the requested range through the most recent
    /// (possibly still-forming) bucket, regardless of which buckets
    /// actually have data.
    ///
    /// The daemon aligns bucket boundaries to absolute epoch multiples of
    /// `bucketMs` (confirmed against `/admin/accounts/analytics`: hourly
    /// buckets fall on :00, 6-hour buckets on 00:00/06:00/12:00/18:00 UTC,
    /// daily buckets on UTC midnight) and floors the requested `sinceMs`
    /// to that grid — its earliest returned bucket is
    /// `(sinceMs / bucketMs) * bucketMs`, not `sinceMs` itself.
    ///
    /// Callers previously derived the x-axis from whichever bucket
    /// timestamps happened to appear in the (often per-provider-filtered)
    /// series data. Because most accounts are only active in a handful of
    /// buckets over a 7d/30d window, that collapsed long real gaps between
    /// sparse data points into adjacent chart columns — producing a
    /// non-uniform timeline whose date labels didn't reflect the actual
    /// elapsed time between them. This walks the same bucket grid the
    /// daemon uses so the x-axis is complete and evenly spaced, and callers
    /// zero-fill any bucket with no matching data point.
    public static func canonicalBuckets(sinceMs: Int64, bucketMs: Int64, nowMs: Int64) -> [Int64] {
        guard bucketMs > 0, nowMs >= sinceMs else { return [] }
        let first = (sinceMs / bucketMs) * bucketMs
        let last = (nowMs / bucketMs) * bucketMs
        guard last >= first else { return [] }
        return Array(stride(from: first, through: last, by: Int(bucketMs)))
    }

    /// Which chart-range tabs (e.g. 24h/7d/30d/All, `spansMinutes` sorted
    /// ascending by lookback window) have enough history to show something
    /// the *previous, shorter* tab wouldn't already show. The shortest
    /// range is always enabled; each following range is enabled iff the
    /// earliest available activity predates the previous range's cutoff —
    /// i.e. there's real data outside what the previous tab already
    /// covers. `earliestActivityMs` of `nil` (not loaded yet, or the
    /// lookup failed) fails open: every range stays enabled rather than
    /// flashing a disabled state or getting stuck disabled on a transient
    /// error.
    public static func enabledRanges(
        spansMinutes: [Int], earliestActivityMs: Int64?, nowMs: Int64
    ) -> [Bool] {
        guard let earliestActivityMs else {
            return spansMinutes.map { _ in true }
        }
        return spansMinutes.indices.map { index in
            guard index > 0 else { return true }
            let previousCutoffMs = nowMs - Int64(spansMinutes[index - 1]) * 60_000
            return earliestActivityMs <= previousCutoffMs
        }
    }

    /// Tooltip series value: 33_000_000 → "33.0M tokens" (Accounts mock).
    public static func millionsLabel(_ tokens: Double) -> String {
        String(format: "%.1fM tokens", tokens / 1_000_000)
    }

    /// Fritsch–Carlson monotone-cubic tangents in per-index units, so the
    /// chart's smooth interpolation matches the mock's `type: monotone`
    /// without overshooting between points.
    public static func monotoneTangents(_ values: [Double]) -> [Double] {
        let count = values.count
        guard count > 1 else { return Array(repeating: 0, count: count) }
        var delta = [Double](repeating: 0, count: count - 1)
        for index in 0..<(count - 1) {
            delta[index] = values[index + 1] - values[index]
        }
        var tangents = [Double](repeating: 0, count: count)
        tangents[0] = delta[0]
        tangents[count - 1] = delta[count - 2]
        for index in 1..<(count - 1) {
            tangents[index] =
                delta[index - 1] * delta[index] <= 0
                ? 0
                : (delta[index - 1] + delta[index]) / 2
        }
        for index in 0..<(count - 1) {
            if delta[index] == 0 {
                tangents[index] = 0
                tangents[index + 1] = 0
                continue
            }
            let alpha = tangents[index] / delta[index]
            let beta = tangents[index + 1] / delta[index]
            let magnitude = alpha * alpha + beta * beta
            if magnitude > 9 {
                let scale = 3 / magnitude.squareRoot()
                tangents[index] = scale * alpha * delta[index]
                tangents[index + 1] = scale * beta * delta[index]
            }
        }
        return tangents
    }
}
