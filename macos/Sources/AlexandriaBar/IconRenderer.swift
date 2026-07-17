import AppKit

@MainActor
enum IconRenderer {
    static let trayImage: NSImage? = {
        guard let url = Bundle.main.urlForImageResource("icon_nobackground")
            ?? Bundle.main.resourceURL.map({
                $0.appendingPathComponent("icon_nobackground.png")
            }),
            let image = NSImage(contentsOf: url)
        else { return nil }
        image.size = NSSize(width: 18, height: 18)
        image.isTemplate = false
        return image
    }()

    static func statusIcon() -> NSImage {
        if let trayImage {
            return trayImage
        }
        let fallback = NSImage(
            systemSymbolName: "sparkles",
            accessibilityDescription: "Alex") ?? NSImage()
        fallback.isTemplate = true
        return fallback
    }
}
