import AppKit
import AlexCore

enum BubbleStyle {
    static let cornerRadius: CGFloat = 10
    static let padX: CGFloat = 10
    static let padY: CGFloat = 4
    static let barWidth: CGFloat = 2.5

    static func fill(_ kind: TranscriptBubbleKind, selected: Bool) -> NSColor {
        guard !selected else {
            return NSColor.controlAccentColor.withAlphaComponent(0.16)
        }
        return switch kind {
        case .turn: .unemphasizedSelectedContentBackgroundColor.withAlphaComponent(0.28)
        case .user, .toolResult:
            .unemphasizedSelectedContentBackgroundColor.withAlphaComponent(0.55)
        case .model: .controlAccentColor.withAlphaComponent(0.10)
        case .tool: .systemPurple.withAlphaComponent(0.08)
        case .error: .systemRed.withAlphaComponent(0.08)
        }
    }

    static func bar(_ kind: TranscriptBubbleKind) -> NSColor? {
        switch kind {
        case .tool: .systemPurple
        case .toolResult: .systemTeal
        case .error: .systemRed
        case .turn, .user, .model: nil
        }
    }

    static func draw(
        kind: TranscriptBubbleKind, rect: CGRect, selected: Bool, in context: CGContext
    ) {
        guard rect.width > 0, rect.height > 0 else { return }
        context.saveGState()
        if selected {
            context.setFillColor(
                NSColor.controlAccentColor.withAlphaComponent(0.85).cgColor)
            context.fill(CGRect(x: 2, y: rect.minY, width: 3, height: rect.height))
        }
        let radius = min(cornerRadius, min(rect.width, rect.height) / 2)
        let path = CGPath(
            roundedRect: rect, cornerWidth: radius, cornerHeight: radius, transform: nil)
        context.addPath(path)
        context.setFillColor(fill(kind, selected: selected).cgColor)
        context.fillPath()
        if kind == .turn {
            context.addPath(path)
            context.setStrokeColor(NSColor.separatorColor.withAlphaComponent(0.45).cgColor)
            context.setLineWidth(0.75)
            context.strokePath()
        }
        if let barColor = bar(kind) {
            context.addPath(path)
            context.clip()
            context.setFillColor(barColor.cgColor)
            context.fill(CGRect(
                x: rect.minX, y: rect.minY, width: barWidth, height: rect.height))
        }
        context.restoreGState()
    }
}
