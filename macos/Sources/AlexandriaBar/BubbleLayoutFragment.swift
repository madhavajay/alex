import AppKit
import AlexandriaBarCore

final class BubbleLayoutFragment: NSTextLayoutFragment {
    var kind: TranscriptBubbleKind?
    var turnId = ""
    var leftInset: CGFloat = 8
    var rightInset: CGFloat = 8
    var roundedTop = true
    var roundedBottom = true
    var isRightAligned = false
    var selectedTurnProvider: (() -> String?)?

    private static let cornerRadius: CGFloat = 8
    private static let padX: CGFloat = 10
    private static let padY: CGFloat = 3
    private static let barWidth: CGFloat = 2.5

    private var isSelected: Bool {
        !turnId.isEmpty && selectedTurnProvider?() == turnId
    }

    private var isCard: Bool {
        kind == .tool || kind == .error
    }

    private var textBounds: CGRect {
        var rect: CGRect?
        for line in textLineFragments {
            let bounds = line.typographicBounds
            rect = rect?.union(bounds) ?? bounds
        }
        return rect ?? .zero
    }

    private var bubbleRect: CGRect {
        guard kind != nil else { return .null }
        let bounds = textBounds
        guard bounds.width > 0 || bounds.height > 0 else { return .null }
        let width = layoutFragmentFrame.width
        let top = roundedTop ? bounds.minY - Self.padY : 0
        let bottom = roundedBottom
            ? bounds.maxY + Self.padY
            : max(bounds.maxY + Self.padY, layoutFragmentFrame.height)
        if isCard || kind == .toolResult {
            let x = max(0, leftInset - Self.padX)
            let maxX = width - max(0, rightInset - Self.padX)
            return CGRect(x: x, y: top, width: max(0, maxX - x), height: bottom - top)
        }
        var x = bounds.minX - Self.padX
        var w = bounds.width + Self.padX * 2
        if isRightAligned {
            let rightEdge = width - max(0, rightInset - Self.padX)
            x = min(x, rightEdge - w)
            w = rightEdge - x
        }
        return CGRect(x: x, y: top, width: w, height: bottom - top)
    }

    override var renderingSurfaceBounds: CGRect {
        var bounds = super.renderingSurfaceBounds
        let bubble = bubbleRect
        if !bubble.isNull { bounds = bounds.union(bubble.insetBy(dx: -2, dy: -2)) }
        bounds = bounds.union(
            CGRect(x: 0, y: 0, width: 4, height: layoutFragmentFrame.height))
        return bounds
    }

    override func draw(at point: CGPoint, in context: CGContext) {
        let selected = isSelected
        context.saveGState()
        if selected {
            context.setFillColor(
                NSColor.controlAccentColor.withAlphaComponent(0.85).cgColor)
            context.fill(CGRect(x: 0, y: 0, width: 3, height: layoutFragmentFrame.height))
        }
        if let kind {
            let bubble = bubbleRect
            if !bubble.isNull, bubble.width > 0, bubble.height > 0 {
                let path = Self.roundedPath(
                    bubble, radius: Self.cornerRadius,
                    top: roundedTop, bottom: roundedBottom)
                context.addPath(path)
                context.setFillColor(fillColor(kind, selected: selected).cgColor)
                context.fillPath()
                if let bar = barColor(kind) {
                    context.saveGState()
                    context.addPath(path)
                    context.clip()
                    context.setFillColor(bar.cgColor)
                    context.fill(CGRect(
                        x: bubble.minX, y: bubble.minY,
                        width: Self.barWidth, height: bubble.height))
                    context.restoreGState()
                }
                if selected {
                    context.addPath(path)
                    context.setStrokeColor(NSColor.controlAccentColor.cgColor)
                    context.setLineWidth(1.5)
                    context.strokePath()
                }
            }
        }
        context.restoreGState()
        super.draw(at: point, in: context)
    }

    private func fillColor(_ kind: TranscriptBubbleKind, selected: Bool) -> NSColor {
        let base: NSColor = switch kind {
        case .user, .toolResult: .unemphasizedSelectedContentBackgroundColor
        case .model: .controlAccentColor.withAlphaComponent(0.12)
        case .tool: .systemPurple.withAlphaComponent(0.10)
        case .error: .systemRed.withAlphaComponent(0.10)
        }
        guard selected else { return base }
        return base.blended(
            withFraction: 0.25, of: NSColor.controlAccentColor.withAlphaComponent(0.5))
            ?? base
    }

    private func barColor(_ kind: TranscriptBubbleKind) -> NSColor? {
        switch kind {
        case .tool: .systemPurple
        case .toolResult: .systemTeal
        case .error: .systemRed
        case .user, .model: nil
        }
    }

    static func roundedPath(
        _ rect: CGRect, radius: CGFloat, top: Bool, bottom: Bool
    ) -> CGPath {
        guard top || bottom else { return CGPath(rect: rect, transform: nil) }
        let r = min(radius, min(rect.width, rect.height) / 2)
        let topRadius = top ? r : 0
        let bottomRadius = bottom ? r : 0
        let path = CGMutablePath()
        path.move(to: CGPoint(x: rect.minX + topRadius, y: rect.minY))
        path.addLine(to: CGPoint(x: rect.maxX - topRadius, y: rect.minY))
        path.addArc(
            tangent1End: CGPoint(x: rect.maxX, y: rect.minY),
            tangent2End: CGPoint(x: rect.maxX, y: rect.minY + topRadius), radius: topRadius)
        path.addLine(to: CGPoint(x: rect.maxX, y: rect.maxY - bottomRadius))
        path.addArc(
            tangent1End: CGPoint(x: rect.maxX, y: rect.maxY),
            tangent2End: CGPoint(x: rect.maxX - bottomRadius, y: rect.maxY),
            radius: bottomRadius)
        path.addLine(to: CGPoint(x: rect.minX + bottomRadius, y: rect.maxY))
        path.addArc(
            tangent1End: CGPoint(x: rect.minX, y: rect.maxY),
            tangent2End: CGPoint(x: rect.minX, y: rect.maxY - bottomRadius),
            radius: bottomRadius)
        path.addLine(to: CGPoint(x: rect.minX, y: rect.minY + topRadius))
        path.addArc(
            tangent1End: CGPoint(x: rect.minX, y: rect.minY),
            tangent2End: CGPoint(x: rect.minX + topRadius, y: rect.minY), radius: topRadius)
        path.closeSubpath()
        return path
    }
}
