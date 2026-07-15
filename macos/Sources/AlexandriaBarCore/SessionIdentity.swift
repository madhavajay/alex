import Foundation

/// Chooses the compact identity shown beside a Trace Browser session.
///
/// A session can contain requests to more than one provider, but the primary
/// session cell intentionally shows one harness plus one provider. For a
/// mixed-provider session, the last listed provider is the compact identity.
public enum SessionIdentity {
    public static func primaryProvider(
        providers: [String], harness: String?, tags: [String: String]?
    ) -> String? {
        if let provider = providers.reversed().first(where: { !$0.isEmpty }) {
            return provider.lowercased()
        }

        // Amp is itself both the harness and subscription model provider. Keep
        // older wrapped sessions (recorded before provider capture) legible.
        if ["amp", "amp-code"].contains(HarnessIcon.canonicalKey(harness: harness, tags: tags)) {
            return "amp"
        }
        return nil
    }

    /// Every lineage child shows the same primary chip regardless of what
    /// the harness called it; the specific type comes from `agentTypeTag`.
    public static let subagentLabel = "sub-agent"

    /// Harnesses use `default` when a child has no more specific agent role.
    /// Surface the specific type as a secondary tag, hiding that wire value.
    public static func agentTypeTag(agentType: String?) -> String? {
        guard
            let value = agentType?.trimmingCharacters(in: .whitespacesAndNewlines),
            !value.isEmpty,
            value.caseInsensitiveCompare("default") != .orderedSame
        else { return nil }
        return value
    }
}
