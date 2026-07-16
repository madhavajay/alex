import Foundation

/// A reusable response fixture exposed by the daemon's local admin API.
/// Kept in Core so both the trace browser and preferences pane share the
/// same portable JSON representation.
public struct ErrorSimulationFixture: Codable, Sendable, Equatable, Identifiable {
    public let name: String
    public let provider: String?
    public let status: Int?
    public let errorKind: String?
    public let direction: String?
    public let createdMs: Int64?
    public let sourceTraceId: String?

    public var id: String { return name }

    enum CodingKeys: String, CodingKey {
        case name, provider, status, direction
        case errorKind = "error_kind"
        case createdMs = "created_ms"
        case sourceTraceId = "source_trace_id"
    }
}

/// The daemon's persisted provider fail-over policy.
public struct ProtectionPolicy: Codable, Sendable, Equatable {
    public var enabled: Bool
    public var rerouteOnAuth: Bool
    public var retries: Int
    public var autoReturn: Bool
    public var equivalencies: [String: [String: String]]

    public init(
        enabled: Bool,
        rerouteOnAuth: Bool,
        retries: Int,
        autoReturn: Bool,
        equivalencies: [String: [String: String]]
    ) {
        self.enabled = enabled
        self.rerouteOnAuth = rerouteOnAuth
        self.retries = retries
        self.autoReturn = autoReturn
        self.equivalencies = equivalencies
    }

    enum CodingKeys: String, CodingKey {
        case enabled, retries, equivalencies
        case rerouteOnAuth = "reroute_on_auth"
        case autoReturn = "auto_return"
    }
}
