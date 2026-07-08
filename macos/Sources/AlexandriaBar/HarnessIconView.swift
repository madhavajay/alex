import AppKit
import SwiftUI
import AlexandriaBarCore

@MainActor
enum HarnessIconLoader {
    private static var cache: [String: NSImage] = [:]
    private static var misses: Set<String> = []

    static func image(harness: String?, tags: [String: String]?) -> NSImage? {
        guard let file = HarnessIcon.assetName(harness: harness, tags: tags) else { return nil }
        if let cached = cache[file] { return cached }
        guard !misses.contains(file) else { return nil }
        let name = (file as NSString).deletingPathExtension
        let ext = (file as NSString).pathExtension
        guard
            let url = Bundle.module.url(
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

struct HarnessIconView: View {
    let harness: String?
    let tags: [String: String]?
    var size: CGFloat = 16

    var body: some View {
        if let image = HarnessIconLoader.image(harness: harness, tags: tags) {
            Image(nsImage: image)
                .resizable()
                .interpolation(.high)
                .aspectRatio(contentMode: .fit)
                .frame(width: size, height: size)
                .clipShape(RoundedRectangle(cornerRadius: size * 0.2))
        }
    }
}

struct ProviderBadgeView: View {
    let provider: String

    var body: some View {
        Circle()
            .fill(color)
            .overlay(
                Circle().strokeBorder(
                    provider == "xai" ? Color.white.opacity(0.85) : Color.clear,
                    lineWidth: 1))
            .overlay(
                Text(ModelProvider.initial(for: provider))
                    .font(.system(size: 6, weight: .bold))
                    .foregroundStyle(.white))
            .frame(width: 10, height: 10)
            .help(name)
    }

    private var color: Color {
        switch provider {
        case "anthropic": Color(red: 0xD9 / 255, green: 0x77 / 255, blue: 0x57 / 255)
        case "openai": Color(red: 0x10 / 255, green: 0xA3 / 255, blue: 0x7F / 255)
        case "xai": Color.black
        case "gemini": Color(red: 0x42 / 255, green: 0x85 / 255, blue: 0xF4 / 255)
        default: Color.gray
        }
    }

    private var name: String {
        switch provider {
        case "anthropic": "Anthropic"
        case "openai": "OpenAI"
        case "xai": "xAI"
        case "gemini": "Gemini"
        default: provider.capitalized
        }
    }
}
