import Foundation

/// Pure presentation logic for the OpenRouter two-list transfer picker.
///
/// The pane shows two columns: LEFT = all known/available catalog models the
/// user can still add, RIGHT = the curated exposed set that gets injected into
/// connected harnesses. Keeping the derivation here (out of the SwiftUI view)
/// makes it unit-testable on Linux without AppKit.
public enum OpenRouterCuration {
    /// Case-insensitive, deterministic model-id ordering. Mirrors the daemon's
    /// `sort_model_ids` so every picker — daemon `/v1/models` and this pane —
    /// reads the same alphabetical order.
    public static func sorted(_ ids: [String]) -> [String] {
        ids.sorted { lhs, rhs in
            let l = lhs.lowercased()
            let r = rhs.lowercased()
            if l != r { return l < r }
            return lhs < rhs
        }
    }

    /// The LEFT column: catalog models not yet exposed, optionally filtered by a
    /// case-insensitive substring search, alphabetically sorted. De-duplicated
    /// so a catalog with repeats never doubles a row.
    public static func available(
        catalog: [String], exposed: [String], search: String = ""
    ) -> [String] {
        let exposedSet = Set(exposed)
        let needle = search.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        var seen = Set<String>()
        let filtered = catalog.filter { id in
            guard !exposedSet.contains(id), seen.insert(id).inserted else { return false }
            return needle.isEmpty || id.lowercased().contains(needle)
        }
        return sorted(filtered)
    }

    /// The RIGHT column: the curated exposed set, alphabetically sorted and
    /// de-duplicated.
    public static func exposedSorted(_ exposed: [String]) -> [String] {
        sorted(Array(Set(exposed)))
    }

    /// Move a model onto the exposed list (Add →).
    public static func adding(_ id: String, to exposed: [String]) -> [String] {
        exposedSorted(exposed + [id])
    }

    /// Take a model off the exposed list (Remove ←).
    public static func removing(_ id: String, from exposed: [String]) -> [String] {
        exposedSorted(exposed.filter { $0 != id })
    }
}
