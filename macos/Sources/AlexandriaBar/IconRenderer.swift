import AppKit
import CoreText
import AlexandriaBarCore

@MainActor
enum IconRenderer {
    static let glyph = "\u{13080}"

    static let glyphAvailable: Bool = {
        let base = CTFontCreateWithName("Helvetica" as CFString, 16, nil)
        let chars = Array(glyph.utf16)
        let cascade = CTFontCreateForString(base, glyph as CFString, CFRange(location: 0, length: chars.count))
        var glyphs = [CGGlyph](repeating: 0, count: chars.count)
        return CTFontGetGlyphsForCharacters(cascade, chars, &glyphs, chars.count)
    }()

    private static var cache: [String: NSImage] = [:]

    static func statusIcon(severity: StoreAlert.Severity?, daemonUp: Bool) -> NSImage {
        let color: NSColor?
        switch (daemonUp, severity) {
        case (false, _): color = .systemRed
        case (_, .critical): color = .systemRed
        case (_, .warning): color = .systemOrange
        default: color = nil
        }
        let key = color.map { $0.description } ?? "template"
        if let cached = cache[key] { return cached }
        let image = render(color: color)
        cache[key] = image
        return image
    }

    private static func render(color: NSColor?) -> NSImage {
        guard glyphAvailable else {
            let fallback = NSImage(
                systemSymbolName: "building.columns",
                accessibilityDescription: "Alexandria")
            if let color, let tinted = fallback?.withSymbolConfiguration(.init(paletteColors: [color])) {
                tinted.isTemplate = false
                return tinted
            }
            fallback?.isTemplate = true
            return fallback ?? NSImage()
        }

        let size = NSSize(width: 20, height: 17)
        let image = NSImage(size: size, flipped: false) { rect in
            let attrs: [NSAttributedString.Key: Any] = [
                .font: NSFont.systemFont(ofSize: 15),
                .foregroundColor: color ?? NSColor.black,
            ]
            let text = NSAttributedString(string: glyph, attributes: attrs)
            let bounds = text.boundingRect(with: rect.size, options: [.usesLineFragmentOrigin])
            text.draw(at: NSPoint(
                x: (rect.width - bounds.width) / 2 - bounds.origin.x,
                y: (rect.height - bounds.height) / 2 - bounds.origin.y))
            return true
        }
        image.isTemplate = (color == nil)
        image.accessibilityDescription = "Alexandria"
        return image
    }
}
