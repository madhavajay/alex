import AppKit
import SwiftUI
import AlexandriaBarCore

@MainActor
enum HarnessIconLoader {
    private static var cache: [String: NSImage] = [:]
    private static var misses: Set<String> = []

    // Bundle.module traps when the SwiftPM resource bundle can't be resolved,
    // which took the whole app down from an icon lookup (0.1.19 Trace Browser
    // crash). Resolve it by hand and treat a missing bundle as "no icon".
    private static let resourceBundle: Bundle? = {
        let name = "AlexandriaBar_AlexandriaBar.bundle"
        let candidates = [
            Bundle.main.resourceURL,
            Bundle.main.bundleURL,
            Bundle.main.executableURL?.deletingLastPathComponent(),
        ]
        for base in candidates {
            if let url = base?.appendingPathComponent(name), let bundle = Bundle(url: url) {
                return bundle
            }
        }
        BarLog.warn(.ui, "harness icon resource bundle missing; icons disabled")
        return nil
    }()

    static func image(harness: String?, tags: [String: String]?) -> NSImage? {
        guard let file = HarnessIcon.assetName(harness: harness, tags: tags) else { return nil }
        if let cached = cache[file] { return cached }
        guard !misses.contains(file) else { return nil }
        let name = (file as NSString).deletingPathExtension
        let ext = (file as NSString).pathExtension
        guard
            let url = resourceBundle?.url(
                forResource: name, withExtension: ext, subdirectory: "logos"),
            let image = NSImage(contentsOf: url), image.isValid
        else {
            misses.insert(file)
            BarLog.warn(.ui, "harness icon unavailable: \(file)")
            return nil
        }
        cache[file] = image
        return image
    }
}

@MainActor
enum ProviderChipRenderer {
    private static var cache: [String: NSImage] = [:]

    static func image(for provider: String) -> NSImage {
        if let cached = cache[provider] { return cached }
        let image = draw(provider)
        cache[provider] = image
        return image
    }

    private static func draw(_ provider: String) -> NSImage {
        let fill = color(provider)
        let initial = ModelProvider.initial(for: provider)
        return NSImage(size: NSSize(width: 13, height: 13), flipped: false) { rect in
            fill.setFill()
            NSBezierPath(ovalIn: rect).fill()
            if provider == "xai" || provider == "amp" {
                NSColor.white.withAlphaComponent(0.85).setStroke()
                let ring = NSBezierPath(ovalIn: rect.insetBy(dx: 0.5, dy: 0.5))
                ring.lineWidth = 1
                ring.stroke()
            }
            let text = NSAttributedString(
                string: initial,
                attributes: [
                    .font: NSFont.systemFont(ofSize: 7.5, weight: .bold),
                    .foregroundColor: NSColor.white,
                ])
            let size = text.size()
            text.draw(at: NSPoint(
                x: rect.midX - size.width / 2, y: rect.midY - size.height / 2))
            return true
        }
    }

    private static func color(_ provider: String) -> NSColor {
        switch provider {
        case "anthropic": NSColor(red: 0xD9 / 255, green: 0x77 / 255, blue: 0x57 / 255, alpha: 1)
        case "openai": NSColor(red: 0x10 / 255, green: 0xA3 / 255, blue: 0x7F / 255, alpha: 1)
        case "xai": .black
        case "gemini": NSColor(red: 0x42 / 255, green: 0x85 / 255, blue: 0xF4 / 255, alpha: 1)
        case "cursor": NSColor(red: 0x8E / 255, green: 0x5C / 255, blue: 0xFF / 255, alpha: 1)
        case "amp": .black
        case "openrouter": NSColor(red: 0x65 / 255, green: 0x61 / 255, blue: 0xFF / 255, alpha: 1)
        default: .gray
        }
    }
}

struct HarnessIconView: View {
    let harness: String?
    let tags: [String: String]?
    var size: CGFloat = 16
    /// Draw the logo on a brand tile (Settings 32px variant,
    /// Create Settings App.tsx:90-123). `backgroundPadding` insets the asset.
    var background: Color?
    var backgroundPadding: CGFloat = 0
    var cornerRadius: CGFloat?
    /// When no logo asset exists, draw the mock's 17×17 tinted chip with a
    /// gear glyph instead of collapsing to nothing (shared.tsx:178-192).
    var showsFallback = false

    var body: some View {
        if let image = HarnessIconLoader.image(harness: harness, tags: tags) {
            let logo = Image(nsImage: image)
                .resizable()
                .interpolation(.high)
                .aspectRatio(contentMode: .fit)
            if let background {
                logo
                    .padding(backgroundPadding)
                    .frame(width: size, height: size)
                    .background(
                        RoundedRectangle(cornerRadius: radius).fill(background))
                    .clipShape(RoundedRectangle(cornerRadius: radius))
            } else {
                logo
                    .frame(width: size, height: size)
                    .clipShape(RoundedRectangle(cornerRadius: radius))
            }
        } else if showsFallback {
            RoundedRectangle(cornerRadius: radius)
                .fill(background ?? AlexTheme.Colors.overlay(0.08))
                .overlay(
                    Image(systemName: "gearshape")
                        .font(.system(size: max(6, size * 0.53)))
                        .foregroundStyle(AlexTheme.Colors.textSecondary))
                .frame(width: size, height: size)
                .help(harness ?? "unknown harness")
        }
    }

    private var radius: CGFloat {
        cornerRadius ?? size * 0.2
    }
}

/// Brand icon with a bottom-right health dot (menu App.tsx:130-170):
/// badge = max(4, 0.38×size), nudged past the corner by 0.35×badge, ringed
/// with the panel background; pending renders at 50% opacity.
struct IconWithHealthBadge<Icon: View>: View {
    var size: CGFloat
    var tint: Color
    var pending = false
    /// Ring separating the badge from the icon; match it to the surface the
    /// icon sits on (defaults to the panel background).
    var ringColor: Color?
    private let icon: Icon

    init(
        size: CGFloat, tint: Color, pending: Bool = false,
        ringColor: Color? = nil, @ViewBuilder icon: () -> Icon
    ) {
        self.size = size
        self.tint = tint
        self.pending = pending
        self.ringColor = ringColor
        self.icon = icon()
    }

    var body: some View {
        let badge = max(4, (size * 0.38).rounded())
        icon
            .frame(width: size, height: size)
            .overlay(alignment: .bottomTrailing) {
                Circle()
                    .fill(tint)
                    .overlay(
                        Circle().strokeBorder(
                            ringColor ?? AlexTheme.Colors.background, lineWidth: 1.5))
                    .frame(width: badge + 3, height: badge + 3)
                    .offset(x: 0.35 * badge, y: 0.35 * badge)
                    .opacity(pending ? 0.5 : 1)
            }
    }
}

struct ProviderBadgeView: View {
    /// `.solid` is the original 13px filled circle with a white initial
    /// (menu bar); `.tinted` is the mock's 17×17 rounded square with a
    /// colored letter on a translucent brand tint (shared.tsx:196-209).
    enum Style {
        case solid
        case tinted
    }

    let provider: String
    var size: CGFloat = 10
    var style: Style = .solid

    var body: some View {
        switch style {
        case .solid: solidBadge
        case .tinted: tintedBadge
        }
    }

    private var solidBadge: some View {
        Circle()
            .fill(color)
            .overlay(
                Circle().strokeBorder(
                    provider == "xai" || provider == "amp"
                        ? Color.white.opacity(0.85) : Color.clear,
                    lineWidth: 1))
            .overlay(
                Text(ModelProvider.initial(for: provider))
                    .font(.system(size: max(6, size * 0.48), weight: .bold))
                    .foregroundStyle(.white))
            .frame(width: size, height: size)
            .help(name)
    }

    private var tintedBadge: some View {
        let brand = AlexTheme.ProviderBrand.brand(for: provider)
        return RoundedRectangle(cornerRadius: max(3, size * 0.22))
            .fill(brand.chipBackground)
            .overlay(
                Text(ModelProvider.initial(for: provider))
                    .font(.system(size: max(6, size * 0.53), weight: .bold))
                    .foregroundStyle(brand.chipText))
            .frame(width: size, height: size)
            .help(name)
    }

    private var color: Color {
        switch provider {
        case "anthropic": Color(red: 0xD9 / 255, green: 0x77 / 255, blue: 0x57 / 255)
        case "openai": Color(red: 0x10 / 255, green: 0xA3 / 255, blue: 0x7F / 255)
        case "xai": Color.black
        case "gemini": Color(red: 0x42 / 255, green: 0x85 / 255, blue: 0xF4 / 255)
        case "cursor": Color(red: 0x8E / 255, green: 0x5C / 255, blue: 0xFF / 255)
        case "amp": Color.black
        case "openrouter": Color(red: 0x65 / 255, green: 0x61 / 255, blue: 0xFF / 255)
        default: Color.gray
        }
    }

    private var name: String {
        switch provider {
        case "anthropic": "Anthropic"
        case "openai": "OpenAI"
        case "xai": "xAI"
        case "gemini": "Gemini"
        case "cursor": "Cursor"
        case "amp": "Amp"
        case "openrouter": "OpenRouter"
        default: provider.capitalized
        }
    }
}

#if DEBUG
#Preview("Harness & provider badges") {
    VStack(alignment: .leading, spacing: AlexTheme.Spacing.lg) {
        HStack(spacing: AlexTheme.Spacing.md) {
            HarnessIconView(harness: "claude", tags: nil, size: 17, showsFallback: true)
            HarnessIconView(harness: "mystery", tags: nil, size: 17, showsFallback: true)
            HarnessIconView(
                harness: "claude", tags: nil, size: 32,
                background: AlexTheme.Colors.dynamic(light: 0xF0EBE3, dark: 0xF0EBE3),
                backgroundPadding: 4, cornerRadius: 8, showsFallback: true)
        }
        HStack(spacing: AlexTheme.Spacing.md) {
            ProviderBadgeView(provider: "anthropic", size: 17, style: .tinted)
            ProviderBadgeView(provider: "openai", size: 17, style: .tinted)
            ProviderBadgeView(provider: "gemini", size: 17, style: .tinted)
            ProviderBadgeView(provider: "anthropic", size: 13)
        }
        HStack(spacing: AlexTheme.Spacing.md) {
            IconWithHealthBadge(size: 14, tint: AlexTheme.Colors.success) {
                HarnessIconView(harness: "claude", tags: nil, size: 14, showsFallback: true)
            }
            IconWithHealthBadge(
                size: 14, tint: AlexTheme.Colors.textTertiary, pending: true
            ) {
                HarnessIconView(harness: "codex", tags: nil, size: 14, showsFallback: true)
            }
            IconWithHealthBadge(
                size: 14, tint: AlexTheme.Colors.success,
                ringColor: AlexTheme.Colors.card
            ) {
                HarnessIconView(harness: "claude", tags: nil, size: 14, showsFallback: true)
            }
            .padding(4)
            .background(AlexTheme.Colors.card)
        }
    }
    .padding()
    .background(AlexTheme.Colors.background)
}
#endif

/// The session list's deliberately compact identity: harness + model provider.
struct SessionIdentityIconsView: View {
    let harness: String?
    let tags: [String: String]?
    let providers: [String]
    var size: CGFloat = 16

    var body: some View {
        HStack(spacing: 4) {
            HarnessIconView(harness: harness, tags: tags, size: size)
            if let provider = SessionIdentity.primaryProvider(
                providers: providers, harness: harness, tags: tags)
            {
                ProviderBadgeView(provider: provider, size: size)
            }
        }
        .fixedSize()
    }
}
