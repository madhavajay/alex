import SwiftUI
import AlexandriaBarCore

/// Preferences → General → Storage. Migration errors are deliberately scoped
/// to the archive operation: an offline archive must not be presented as a
/// generic daemon outage when the rest of Alex is still healthy.
struct LARMigrationPreferencesSection: View {
    let store: SnapshotStore

    @State private var status: LARMigrationStatus?
    @State private var availability: Availability = .loading
    @State private var action: Action?
    @State private var actionMessage: String?

    var body: some View {
        Group {
            SectionLabel(text: "Storage")
                .settingsSectionSpacing()

            switch availability {
            case .loading:
                SettingRow(
                    label: "LAR archive migration",
                    hint: "Loading legacy trace conversion status"
                ) {
                    ProgressView().controlSize(.small)
                }
            case .unsupported:
                SettingRow(
                    label: "LAR archive migration",
                    hint: "Migration controls are not available in this version of Alex. Existing traces are unchanged."
                ) {
                    StatusChip(tint: AlexTheme.Colors.textFaint, text: "Unavailable")
                }
            case .unavailable:
                SettingRow(
                    label: "LAR archive migration",
                    hint: "Storage migration status could not be loaded. Existing traces remain available through their current storage."
                ) {
                    PillButton(title: "Retry", variant: .bordered) {
                        Task { await loadStatus() }
                    }
                }
            case .available:
                if let status {
                    migrationStatus(status)
                }
            }
        }
        .task(id: configurationID) {
            await loadStatus()
        }
    }

    private var configurationID: String {
        guard let config = store.config else { return "unconfigured" }
        return "\(config.host):\(config.port)"
    }

    @ViewBuilder
    private func migrationStatus(_ status: LARMigrationStatus) -> some View {
        SettingRow(label: "LAR archive migration", hint: progressSummary(status)) {
            HStack(spacing: 7) {
                if status.running {
                    ProgressView().controlSize(.small)
                }
                StatusChip(
                    tint: statusTint(status),
                    text: status.state.displayName)
            }
        }

        if let progress = status.completionFraction, status.state != .completed {
            ProgressView(value: progress)
                .progressViewStyle(.linear)
                .tint(AlexTheme.Colors.primary)
                .padding(.vertical, 3)
        }

        SettingCaption(storageSummary(status))

        if let lastError = status.lastError, !lastError.isEmpty {
            Label(lastError, systemImage: "exclamationmark.triangle.fill")
                .font(.system(size: 11))
                .foregroundStyle(AlexTheme.Colors.warningOrange)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.vertical, 3)
        }

        HStack(spacing: 8) {
            if status.running {
                controlButton("Pause", operation: .pause)
            } else if status.state != .completed {
                controlButton("Resume", operation: .resume)
            }
            controlButton(
                "Verify", operation: .verify,
                enabled: !status.running && status.migrated > 0)
            PillButton(
                title: "Refresh", variant: .standard,
                isEnabled: action == nil
            ) {
                Task { await loadStatus() }
            }
        }
        .padding(.vertical, 4)

        if let actionMessage {
            SettingCaption(actionMessage)
        }
    }

    private func controlButton(
        _ title: String, operation: Action, enabled: Bool = true
    ) -> some View {
        PillButton(
            title: action == operation ? "\(title)…" : title,
            variant: .bordered,
            isEnabled: enabled && action == nil,
            isBusy: action == operation
        ) {
            perform(operation)
        }
    }

    private func perform(_ operation: Action) {
        guard let config = store.config else {
            actionMessage = "Storage migration controls are not available right now."
            return
        }
        action = operation
        actionMessage = nil
        Task {
            do {
                let client = AlexandriaClient(config: config)
                let supported = switch operation {
                case .pause: try await client.larMigrationPause()
                case .resume: try await client.larMigrationResume()
                case .verify: try await client.larMigrationVerify()
                }
                guard supported else {
                    availability = .unsupported
                    status = nil
                    action = nil
                    return
                }
                actionMessage = operation.successMessage
                action = nil
                await loadStatus()
            } catch {
                actionMessage = operation.failureMessage
                action = nil
            }
        }
    }

    @MainActor
    private func loadStatus() async {
        guard let config = store.config else {
            status = nil
            availability = .unavailable
            return
        }
        if status == nil { availability = .loading }
        do {
            if let fetched = try await AlexandriaClient(config: config).larMigrationStatus() {
                status = fetched
                availability = .available
            } else {
                status = nil
                availability = .unsupported
            }
        } catch is CancellationError {
            return
        } catch {
            status = nil
            availability = .unavailable
        }
    }

    private func progressSummary(_ status: LARMigrationStatus) -> String {
        var parts = [
            "\(status.migrated.formatted()) migrated",
            "\(status.pending.formatted()) pending",
            "\(status.skipped.formatted()) skipped",
        ]
        if status.failed > 0 {
            parts.append("\(status.failed.formatted()) failed")
        }
        if status.discovered == 0 && status.state == .idle {
            return "No legacy trace migration is currently queued"
        }
        return parts.joined(separator: " · ")
    }

    private func storageSummary(_ status: LARMigrationStatus) -> String {
        let read = ByteCountFormatter.string(
            fromByteCount: clampedBytes(status.bytesRead), countStyle: .file)
        let unique = ByteCountFormatter.string(
            fromByteCount: clampedBytes(status.uniqueBytes), countStyle: .file)
        var summary = "Read \(read) · unique LAR data \(unique)"
        if let fraction = status.deduplicationFraction {
            summary += " · \(fraction.formatted(.percent.precision(.fractionLength(0)))) deduplicated"
        }
        return summary
    }

    private func clampedBytes(_ value: UInt64) -> Int64 {
        Int64(clamping: value)
    }

    private func statusTint(_ status: LARMigrationStatus) -> Color {
        if status.failed > 0 || status.state == .failed {
            return AlexTheme.Colors.warningOrange
        }
        return switch status.state {
        case .completed: AlexTheme.Colors.success
        case .running, .discovering, .verifying: AlexTheme.Colors.primary
        case .paused: AlexTheme.Colors.warningOrange
        default: AlexTheme.Colors.textTertiary
        }
    }

    private enum Availability {
        case loading
        case available
        case unsupported
        case unavailable
    }

    private enum Action: Equatable {
        case pause
        case resume
        case verify

        var successMessage: String {
            switch self {
            case .pause: "Migration pause requested."
            case .resume: "Migration resume requested."
            case .verify: "Migration verification requested."
            }
        }

        var failureMessage: String {
            switch self {
            case .pause: "Could not pause the storage migration. Its current state is unchanged."
            case .resume: "Could not resume the storage migration. Its current state is unchanged."
            case .verify: "Could not start storage verification. Existing archive data is unchanged."
            }
        }
    }
}
