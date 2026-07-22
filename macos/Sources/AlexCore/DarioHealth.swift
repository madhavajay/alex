public enum DarioHealthState: Sendable, Equatable {
    case ready
    case warming
    case down
}

public enum DarioHealthTint: Sendable, Equatable {
    case green
    case orange
    case red
}

public struct DarioHealthEvaluation: Sendable, Equatable {
    public let state: DarioHealthState
    public let tint: DarioHealthTint
    public let label: String
}

/// The single health interpretation used everywhere Dario is presented.
public enum DarioHealth {
    public static func evaluate(_ status: DarioStatus?) -> DarioHealthEvaluation {
        guard let status, status.issue == nil else { return down }
        let active = status.generations.first { $0.id == status.activeGenerationId }
            ?? status.generations.first
        guard let active else { return down }
        return evaluate(phase: active.phase, lastProbeOK: active.lastProbe?.ok)
    }

    public static func evaluate(_ status: DarioAdminStatus?) -> DarioHealthEvaluation {
        guard let status, status.issue == nil else { return down }
        let active = status.generations.first { $0.id == status.activeGenerationId }
            ?? status.generations.first
        guard let active else { return down }
        return evaluate(phase: active.phase, lastProbeOK: active.lastProbe?.ok)
    }

    public static func evaluate(
        phase: String, lastProbeOK: Bool?
    ) -> DarioHealthEvaluation {
        if lastProbeOK == false { return down }
        switch phase.lowercased() {
        case "ready":
            return DarioHealthEvaluation(state: .ready, tint: .green, label: "ready")
        case "dead", "failed", "stopped", "unhealthy", "crashed":
            return down
        default:
            return DarioHealthEvaluation(state: .warming, tint: .orange, label: "warming")
        }
    }

    private static let down = DarioHealthEvaluation(
        state: .down, tint: .red, label: "down")
}
