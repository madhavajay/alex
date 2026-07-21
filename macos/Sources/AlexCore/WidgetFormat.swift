import Foundation

/// Pure presentation logic for the shared design-system model badge
/// (ui/Trace Browser shared.tsx:39-44, 163-174). Lives in Core so the label
/// derivation and family classification stay unit-testable without SwiftUI.
public enum ModelBadgeFormat {
    public enum Family: String, Equatable, Sendable, CaseIterable {
        case opus
        case sonnet
        case haiku
        case gpt
        case other
    }

    public static func family(of model: String) -> Family {
        let lowered = model.lowercased()
        if lowered.hasPrefix("claude-opus") { return .opus }
        if lowered.hasPrefix("claude-sonnet") { return .sonnet }
        if lowered.hasPrefix("claude-haiku") { return .haiku }
        if lowered.hasPrefix("gpt-") { return .gpt }
        return .other
    }

    /// "claude-opus-4-8" → "opus 4.8"; "gpt-4o" → "gpt-4o"
    /// (strip `claude-` prefix, trailing `-4-8` → ` 4.8`; shared.tsx:165-167).
    public static func label(for model: String) -> String {
        var label = model
        if label.hasPrefix("claude-") {
            label.removeFirst("claude-".count)
        }
        if let match = label.firstMatch(of: /-(\d+)-(\d+)$/) {
            label.replaceSubrange(match.range, with: " \(match.1).\(match.2)")
        }
        return label
    }
}
