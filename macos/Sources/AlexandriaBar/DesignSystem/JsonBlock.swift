import SwiftUI
import AlexandriaBarCore

/// Syntax-highlighted JSON block (shared.tsx:339-401): mono 10.5px, generous
/// line height, padding 8×12, scrolls beyond `maxHeight`. Tokenization lives
/// in Core (`JsonSyntax`); colors come from `AlexTheme.Colors.Json`.
struct JsonBlock: View {
    let content: String
    var maxHeight: CGFloat = 200
    @State private var highlighted = AttributedString()

    var body: some View {
        ScrollView([.vertical, .horizontal]) {
            Text(highlighted)
                .font(AlexTheme.Fonts.metaMono)
                .lineSpacing(10.5 * 0.65)
                .textSelection(.enabled)
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        .frame(maxHeight: maxHeight)
        .task(id: content) {
            let source = content
            let tokens = await Task.detached(priority: .userInitiated) {
                let start = ContinuousClock.now
                defer {
                    let elapsed = start.duration(to: .now)
                    BarLog.timing(
                        .ui, label: "json tokenize bytes=\(source.utf8.count)",
                        milliseconds: Double(elapsed.components.seconds) * 1000
                            + Double(elapsed.components.attoseconds) / 1e15)
                }
                return JsonSyntax.tokenize(source)
            }.value
            guard !Task.isCancelled else { return }
            highlighted = Self.highlighted(tokens)
        }
    }

    private static func highlighted(_ tokens: [JsonSyntax.Token]) -> AttributedString {
        var result = AttributedString()
        for token in tokens {
            var piece = AttributedString(token.text)
            piece.foregroundColor = Self.color(for: token.type)
            result += piece
        }
        return result
    }

    static func color(for type: JsonSyntax.TokenType) -> Color {
        switch type {
        case .key: AlexTheme.Colors.Json.key
        case .string: AlexTheme.Colors.Json.string
        case .number: AlexTheme.Colors.Json.number
        case .boolean: AlexTheme.Colors.Json.boolean
        case .null: AlexTheme.Colors.Json.null
        case .punctuation: AlexTheme.Colors.Json.punctuation
        case .whitespace: AlexTheme.Colors.foreground
        }
    }
}

#if DEBUG
#Preview("JsonBlock") {
    JsonBlock(
        content: """
        {
          "model": "claude-opus-4-8",
          "max_tokens": 4096,
          "stream": true,
          "stop_sequences": null,
          "messages": [
            { "role": "user", "content": "Refactor the auth module" }
          ]
        }
        """,
        maxHeight: 240)
    .frame(width: 360)
    .background(AlexTheme.Colors.background)
}
#endif
