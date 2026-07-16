import SwiftUI
import AlexandriaBarCore

/// One line of the usage chart. `values` are raw token counts per bucket.
struct UsageChartSeries: Identifiable, Sendable {
    let id: String
    var name: String
    var color: Color
    var values: [Double]
}

/// Multi-series usage line chart in pure SwiftUI (no Charts dependency),
/// matching the Accounts mock (App.tsx:496-539): monotone-smooth 1.5pt lines,
/// horizontal-only grid, mono 9px axis labels with `N×10⁷`-style Y ticks, and
/// a hover cursor with per-series dots and a floating tooltip. Domain and
/// interpolation math live in Core (`UsageChartScale` / `UsageChartMath`).
struct UsageLineChart: View {
    let series: [UsageChartSeries]
    /// One label per bucket; thinned to ~5 via `UsageChartMath.axisLabelIndices`.
    let xLabels: [String]
    var plotHeight: CGFloat = 130
    var showsLegend = true
    @State private var hoverIndex: Int?
    @State private var hoverLocation: CGPoint = .zero
    @State private var curveCache = CurveCache.empty

    private static let yAxisWidth: CGFloat = 46
    private static let xLabelHeight: CGFloat = 18

    private var bucketCount: Int {
        max(series.map(\.values.count).max() ?? 0, xLabels.count)
    }

    /// This changes only when the input snapshot changes. It is used as an
    /// identity for the one-time path build, not as work performed from the
    /// hover-driven body invalidation.
    private var dataKey: Int {
        var hasher = Hasher()
        for line in series {
            hasher.combine(line.id)
            hasher.combine(line.values)
        }
        hasher.combine(xLabels)
        return hasher.finalize()
    }

    var body: some View {
        VStack(alignment: .leading, spacing: AlexTheme.Spacing.md) {
            if showsLegend {
                legend
            }
            HStack(alignment: .top, spacing: 0) {
                yAxis
                plot
            }
        }
    }

    private var legend: some View {
        HStack(spacing: 16) {
            ForEach(series) { line in
                HStack(spacing: 6) {
                    Capsule().fill(line.color).frame(width: 18, height: 2)
                    Text(line.name)
                        .font(AlexTheme.Fonts.mono(10))
                        .foregroundStyle(AlexTheme.Colors.textTertiary)
                        .lineLimit(1)
                }
            }
        }
    }

    private var yAxis: some View {
        ZStack(alignment: .trailing) {
            ForEach(Array(curveCache.scale.ticks.enumerated()), id: \.offset) { _, tick in
                Text(UsageChartScale.tickLabel(tick))
                    .font(AlexTheme.Fonts.mono(9))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
                    .position(
                        x: Self.yAxisWidth / 2 + 8,
                        y: yPosition(for: tick, scale: curveCache.scale))
            }
        }
        .frame(width: Self.yAxisWidth, height: plotHeight + Self.xLabelHeight)
    }

    private var plot: some View {
        GeometryReader { geometry in
            let width = geometry.size.width
            ZStack(alignment: .topLeading) {
                gridLines(width: width)
                linePaths(width: width)
                hoverOverlay(width: width)
                xAxisLabels(width: width)
            }
            .contentShape(Rectangle())
            .onContinuousHover { phase in
                switch phase {
                case .active(let location):
                    hoverLocation = location
                    hoverIndex = nearestIndex(x: location.x, width: width)
                case .ended:
                    hoverIndex = nil
                }
            }
            .onAppear { rebuildCurveCache(width: width) }
            .onChange(of: width) { _, changed in rebuildCurveCache(width: changed) }
            .onChange(of: dataKey) { _, _ in rebuildCurveCache(width: width) }
        }
        .frame(height: plotHeight + Self.xLabelHeight)
    }

    private func gridLines(width: CGFloat) -> some View {
            ForEach(Array(curveCache.scale.ticks.enumerated()), id: \.offset) { _, tick in
            Path { path in
                let y = yPosition(for: tick, scale: curveCache.scale)
                path.move(to: CGPoint(x: 0, y: y))
                path.addLine(to: CGPoint(x: width, y: y))
            }
            .stroke(AlexTheme.Colors.hairline, lineWidth: 1)
        }
    }

    private func linePaths(width: CGFloat) -> some View {
        ForEach(series) { line in
            (curveCache.paths[line.id] ?? Path())
                .stroke(
                    line.color,
                    style: StrokeStyle(lineWidth: 1.5, lineCap: .round, lineJoin: .round))
        }
    }

    /// Monotone-cubic path through the series points (Hermite → Bézier with
    /// Core-computed tangents, so the curve never overshoots the data).
    private func path(
        for values: [Double], width: CGFloat, scale: UsageChartScale, bucketCount: Int
    ) -> Path {
        var path = Path()
        guard values.count > 1, bucketCount > 1 else {
            if values.count == 1 {
                let point = CGPoint(x: xPosition(0, width: width, bucketCount: bucketCount),
                                    y: yPosition(for: values[0], scale: scale))
                path.addEllipse(in: CGRect(x: point.x - 1.5, y: point.y - 1.5,
                                           width: 3, height: 3))
            }
            return path
        }
        let tangents = UsageChartMath.monotoneTangents(values)
        let points = values.enumerated().map { index, value in
            CGPoint(
                x: xPosition(index, width: width, bucketCount: bucketCount),
                y: yPosition(for: value, scale: scale))
        }
        path.move(to: points[0])
        for index in 0..<(points.count - 1) {
            let dx = points[index + 1].x - points[index].x
            // Tangents are dv/dindex; one index step spans dx pixels and
            // valueSpan/plotHeight per pixel vertically (y is flipped).
            let control1 = CGPoint(
                x: points[index].x + dx / 3,
                y: points[index].y - yDelta(tangents[index], scale: scale) / 3)
            let control2 = CGPoint(
                x: points[index + 1].x - dx / 3,
                y: points[index + 1].y + yDelta(tangents[index + 1], scale: scale) / 3)
            path.addCurve(to: points[index + 1], control1: control1, control2: control2)
        }
        return path
    }

    @ViewBuilder
    private func hoverOverlay(width: CGFloat) -> some View {
        if let index = hoverIndex {
            let x = xPosition(index, width: width, bucketCount: curveCache.bucketCount)
            Path { path in
                path.move(to: CGPoint(x: x, y: 0))
                path.addLine(to: CGPoint(x: x, y: plotHeight))
            }
            .stroke(AlexTheme.Colors.overlay(0.1), lineWidth: 1)
            ForEach(series) { line in
                if index < line.values.count {
                    Circle()
                        .fill(line.color)
                        .frame(width: 6, height: 6)
                        .position(x: x, y: yPosition(for: line.values[index], scale: curveCache.scale))
                }
            }
            tooltip(index: index)
                .position(tooltipPosition(x: x, width: width))
        }
    }

    private func tooltip(index: Int) -> some View {
        VStack(alignment: .leading, spacing: 3) {
            if index < xLabels.count {
                Text(xLabels[index])
                    .font(.system(size: 11))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
            }
            ForEach(series) { line in
                if index < line.values.count {
                    Text("\(line.name): \(UsageChartMath.millionsLabel(line.values[index]))")
                        .font(.system(size: 11))
                        .foregroundStyle(line.color)
                }
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .background(
            RoundedRectangle(cornerRadius: AlexTheme.Radius.md)
                .fill(AlexTheme.Colors.dynamicRGBA(
                    light: (255, 255, 255, 0.96), dark: (44, 44, 46, 0.96))))
        .overlay(
            RoundedRectangle(cornerRadius: AlexTheme.Radius.md)
                .strokeBorder(AlexTheme.Colors.overlay(0.10)))
        .shadow(color: .black.opacity(0.5), radius: 10, y: 4)
        .fixedSize()
    }

    private func tooltipPosition(x: CGFloat, width: CGFloat) -> CGPoint {
        // Keep the card inside the plot; flip sides past the midpoint.
        let estimatedHalfWidth: CGFloat = 80
        let flipped = x > width / 2
        let cardX = flipped ? x - estimatedHalfWidth - 12 : x + estimatedHalfWidth + 12
        return CGPoint(
            x: min(max(cardX, estimatedHalfWidth), width - estimatedHalfWidth),
            y: max(28, min(hoverLocation.y - 24, plotHeight - 28)))
    }

    private func xAxisLabels(width: CGFloat) -> some View {
        ForEach(UsageChartMath.axisLabelIndices(count: xLabels.count), id: \.self) { index in
            Text(xLabels[index])
                .font(AlexTheme.Fonts.mono(9))
                .foregroundStyle(AlexTheme.Colors.textTertiary)
                .fixedSize()
                .position(
                    x: xPosition(index, width: width, bucketCount: curveCache.bucketCount),
                    y: plotHeight + 6 + Self.xLabelHeight / 2 - 4)
        }
    }

    private func xPosition(_ index: Int, width: CGFloat, bucketCount: Int) -> CGFloat {
        guard bucketCount > 1 else { return width / 2 }
        return width * CGFloat(index) / CGFloat(bucketCount - 1)
    }

    private func yPosition(for value: Double, scale: UsageChartScale) -> CGFloat {
        guard scale.upper > 0 else { return plotHeight }
        return plotHeight * CGFloat(1 - min(1, max(0, value / scale.upper)))
    }

    /// Pixel rise for a per-index tangent (positive tangent → upward on screen).
    private func yDelta(_ tangent: Double, scale: UsageChartScale) -> CGFloat {
        guard scale.upper > 0 else { return 0 }
        return plotHeight * CGFloat(tangent / scale.upper)
    }

    private func nearestIndex(x: CGFloat, width: CGFloat) -> Int? {
        guard bucketCount > 0 else { return nil }
        guard bucketCount > 1 else { return 0 }
        let fraction = x / max(width, 1)
        let index = Int((fraction * CGFloat(bucketCount - 1)).rounded())
        return min(max(index, 0), bucketCount - 1)
    }

    private func rebuildCurveCache(width: CGFloat) {
        guard width > 0 else { return }
        let lines = series
        let count = bucketCount
        let nextScale = UsageChartScale(maxValue: lines.flatMap(\.values).max() ?? 0)
        BarLog.measure(.ui, label: "chart path build series=\(lines.count) buckets=\(count)") {
            curveCache = CurveCache(
                paths: Dictionary(uniqueKeysWithValues: lines.map { line in
                    (line.id, path(for: line.values, width: width, scale: nextScale, bucketCount: count))
                }),
                scale: nextScale,
                bucketCount: count)
        }
    }

    private struct CurveCache {
        var paths: [String: Path]
        var scale: UsageChartScale
        var bucketCount: Int
        static let empty = CurveCache(paths: [:], scale: UsageChartScale(maxValue: 0), bucketCount: 0)
    }
}

#if DEBUG
#Preview("UsageLineChart") {
    let hours = (0..<24).map { String(format: "%02d:00", $0) }
    let primary = (0..<24).map { hour in
        3.2e7 + 1.8e7 * sin(Double(hour) / 4) + Double(hour) * 2e5
    }
    let secondary = (0..<24).map { hour in
        1.4e7 + 0.9e7 * cos(Double(hour) / 3)
    }
    return VStack(alignment: .leading, spacing: AlexTheme.Spacing.lg) {
        Text("Tokens routed over time")
            .font(.system(size: 11))
            .foregroundStyle(AlexTheme.Colors.textTertiary)
        UsageLineChart(
            series: [
                UsageChartSeries(
                    id: "acc-1", name: "me@madhavajay.com",
                    color: AlexTheme.Colors.chartPalette[0], values: primary),
                UsageChartSeries(
                    id: "acc-2", name: "madhave@openmined.org",
                    color: AlexTheme.Colors.chartPalette[1], values: secondary),
            ],
            xLabels: hours)
    }
    .padding(16)
    .alexCard()
    .padding()
    .frame(width: 560)
    .background(AlexTheme.Colors.background)
}
#endif
