import Foundation

/// The durable state of the legacy-body to LAR migration job.
///
/// Unknown values are preserved so a newer daemon does not make an older menu
/// app fail to decode the rest of the migration counters.
public enum LARMigrationJobState: Sendable, Equatable, Codable {
    case idle
    case pending
    case discovering
    case running
    case paused
    case verifying
    case completed
    case failed
    case unknown(String)

    public init(from decoder: Decoder) throws {
        let value = try decoder.singleValueContainer().decode(String.self)
        switch value {
        case "idle", "not_started": self = .idle
        case "pending": self = .pending
        case "discovering": self = .discovering
        case "running": self = .running
        case "paused": self = .paused
        case "verifying": self = .verifying
        case "completed", "complete": self = .completed
        case "failed": self = .failed
        default: self = .unknown(value)
        }
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        try container.encode(wireValue)
    }

    public var wireValue: String {
        switch self {
        case .idle: "idle"
        case .pending: "pending"
        case .discovering: "discovering"
        case .running: "running"
        case .paused: "paused"
        case .verifying: "verifying"
        case .completed: "completed"
        case .failed: "failed"
        case .unknown(let value): value
        }
    }

    public var displayName: String {
        switch self {
        case .idle: "Not started"
        case .pending: "Queued"
        case .discovering: "Discovering"
        case .running: "Migrating"
        case .paused: "Paused"
        case .verifying: "Verifying"
        case .completed: "Complete"
        case .failed: "Needs attention"
        case .unknown(let value): value.replacingOccurrences(of: "_", with: " ").capitalized
        }
    }
}

/// A compact, forward-compatible view of the persisted LAR migration job.
public struct LARMigrationStatus: Sendable, Equatable, Codable {
    public let jobID: String?
    public let state: LARMigrationJobState
    public let discovered: UInt64
    public let pending: UInt64
    public let migrated: UInt64
    public let skipped: UInt64
    public let failed: UInt64
    public let bytesRead: UInt64
    public let uniqueBytes: UInt64
    public let deduplicatedBytes: UInt64
    public let lastError: String?
    public let paused: Bool
    public let running: Bool

    public init(
        jobID: String? = nil,
        state: LARMigrationJobState,
        discovered: UInt64 = 0,
        pending: UInt64 = 0,
        migrated: UInt64 = 0,
        skipped: UInt64 = 0,
        failed: UInt64 = 0,
        bytesRead: UInt64 = 0,
        uniqueBytes: UInt64 = 0,
        deduplicatedBytes: UInt64 = 0,
        lastError: String? = nil,
        paused: Bool? = nil,
        running: Bool? = nil
    ) {
        self.jobID = jobID
        self.state = state
        self.discovered = discovered
        self.pending = pending
        self.migrated = migrated
        self.skipped = skipped
        self.failed = failed
        self.bytesRead = bytesRead
        self.uniqueBytes = uniqueBytes
        self.deduplicatedBytes = deduplicatedBytes
        self.lastError = lastError
        self.paused = paused ?? (state == .paused)
        self.running = running ?? state.impliesRunning
    }

    fileprivate enum CodingKeys: String, CodingKey {
        case jobID = "job_id"
        case state
        case discovered
        case discoveredCount = "discovered_count"
        case pending
        case pendingCount = "pending_count"
        case migrated
        case migratedCount = "migrated_count"
        case skipped
        case skippedCount = "skipped_count"
        case failed
        case failedCount = "failed_count"
        case bytesRead = "bytes_read"
        case uniqueBytes = "unique_bytes"
        case deduplicatedBytes = "deduplicated_bytes"
        case dedupBytes = "dedup_bytes"
        case lastError = "last_error"
        case paused
        case running
    }

    public init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        let state = try values.decodeIfPresent(LARMigrationJobState.self, forKey: .state) ?? .idle

        self.init(
            jobID: try values.decodeIfPresent(String.self, forKey: .jobID),
            state: state,
            discovered: try values.decodeCount(.discovered, alias: .discoveredCount),
            pending: try values.decodeCount(.pending, alias: .pendingCount),
            migrated: try values.decodeCount(.migrated, alias: .migratedCount),
            skipped: try values.decodeCount(.skipped, alias: .skippedCount),
            failed: try values.decodeCount(.failed, alias: .failedCount),
            bytesRead: try values.decodeIfPresent(UInt64.self, forKey: .bytesRead) ?? 0,
            uniqueBytes: try values.decodeIfPresent(UInt64.self, forKey: .uniqueBytes) ?? 0,
            deduplicatedBytes: try values.decodeIfPresent(UInt64.self, forKey: .deduplicatedBytes)
                ?? values.decodeIfPresent(UInt64.self, forKey: .dedupBytes)
                ?? 0,
            lastError: try values.decodeIfPresent(String.self, forKey: .lastError),
            paused: try values.decodeIfPresent(Bool.self, forKey: .paused),
            running: try values.decodeIfPresent(Bool.self, forKey: .running))
    }

    public func encode(to encoder: Encoder) throws {
        var values = encoder.container(keyedBy: CodingKeys.self)
        try values.encodeIfPresent(jobID, forKey: .jobID)
        try values.encode(state, forKey: .state)
        try values.encode(discovered, forKey: .discovered)
        try values.encode(pending, forKey: .pending)
        try values.encode(migrated, forKey: .migrated)
        try values.encode(skipped, forKey: .skipped)
        try values.encode(failed, forKey: .failed)
        try values.encode(bytesRead, forKey: .bytesRead)
        try values.encode(uniqueBytes, forKey: .uniqueBytes)
        try values.encode(deduplicatedBytes, forKey: .deduplicatedBytes)
        try values.encodeIfPresent(lastError, forKey: .lastError)
        try values.encode(paused, forKey: .paused)
        try values.encode(running, forKey: .running)
    }

    /// Completed or intentionally skipped items. Failures remain visible and
    /// are not counted as successful migration progress.
    public var processed: UInt64 {
        let (sum, overflow) = migrated.addingReportingOverflow(skipped)
        return overflow ? .max : sum
    }

    public var completionFraction: Double? {
        guard discovered > 0 else { return nil }
        return min(Double(processed) / Double(discovered), 1)
    }

    public var deduplicationFraction: Double? {
        guard bytesRead > 0 else { return nil }
        return min(Double(deduplicatedBytes) / Double(bytesRead), 1)
    }
}

private extension LARMigrationJobState {
    var impliesRunning: Bool {
        switch self {
        case .discovering, .running, .verifying: true
        default: false
        }
    }
}

private extension KeyedDecodingContainer where Key == LARMigrationStatus.CodingKeys {
    func decodeCount(_ key: Key, alias: Key) throws -> UInt64 {
        try decodeIfPresent(UInt64.self, forKey: key)
            ?? decodeIfPresent(UInt64.self, forKey: alias)
            ?? 0
    }
}
