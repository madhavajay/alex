import SwiftUI
import AlexandriaBarCore

/// Preferences → Dario. This intentionally makes its own lightweight status
/// request so opening Settings never has to wait for the menu's polling cycle.
struct DarioPreferencesSection: View {
    let store: SnapshotStore
    let onOpenDario: () -> Void

    @State private var status: DarioAdminStatus?
    @State private var isLoading = true
    @State private var loadError: String?
    @State private var repairInFlight = false
    @State private var actionResult: String?

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: AlexTheme.Spacing.lg) {
                header

                if isLoading && status == nil {
                    loadingState
                } else if let loadError, status == nil {
                    errorState(loadError)
                } else if let status {
                    statusContent(status)
                } else {
                    disabledState
                }
            }
            .padding(20)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .background(AlexTheme.Colors.background)
        .task { await refresh() }
    }

    private var header: some View {
        HStack(alignment: .center) {
            VStack(alignment: .leading, spacing: 2) {
                Text("Dario")
                    .font(AlexTheme.Fonts.panelTitle)
                    .foregroundStyle(AlexTheme.Colors.foreground)
                Text("Runtime, routing, and generation health")
                    .font(.system(size: 12))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
            }
            Spacer()
            PillButton(
                title: "Refresh", variant: .bordered, systemImage: "arrow.clockwise",
                isEnabled: !isLoading && !repairInFlight
            ) {
                Task { await refresh() }
            }
        }
    }

    private var loadingState: some View {
        HStack(spacing: 8) {
            ProgressView().controlSize(.small)
            Text("Loading Dario status…")
        }
        .font(.system(size: 12))
        .foregroundStyle(AlexTheme.Colors.textSecondary)
        .padding(.vertical, 24)
        .frame(maxWidth: .infinity)
    }

    private func errorState(_ message: String) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 7) {
                StatusDot(tint: AlexTheme.Colors.destructive, size: 7, glow: true)
                Text("Daemon down")
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(AlexTheme.Colors.destructive)
            }
            Text(message)
                .font(AlexTheme.Fonts.metaMono)
                .foregroundStyle(AlexTheme.Colors.textSecondary)
                .textSelection(.enabled)
            PillButton(title: "Try again", variant: .bordered) {
                Task { await refresh() }
            }
        }
        .padding(14)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: AlexTheme.Radius.md)
            .fill(AlexTheme.Colors.destructive.opacity(0.08)))
    }

    private var disabledState: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Dario is not available")
                .font(.system(size: 12, weight: .semibold))
                .foregroundStyle(AlexTheme.Colors.textSecondary)
            Text("The connected daemon does not expose Dario status.")
                .font(.system(size: 12))
                .foregroundStyle(AlexTheme.Colors.textTertiary)
        }
        .padding(14)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: AlexTheme.Radius.md)
            .fill(AlexTheme.Colors.surfaceFaint))
    }

    @ViewBuilder
    private func statusContent(_ status: DarioAdminStatus) -> some View {
        darioCard {
            SectionLabel(text: "Status", style: .prominent)
            HStack(spacing: 7) {
                StatusDot(tint: health.tint.color, size: 7, glow: true)
                Text(health.label)
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(health.tint.color)
                Spacer()
                Text(routingLine(status))
                    .font(.system(size: 11))
                    .foregroundStyle(AlexTheme.Colors.textSecondary)
                    .multilineTextAlignment(.trailing)
            }
        }

        if let issue = status.issue {
            darioCard {
                HStack(alignment: .top, spacing: 10) {
                    Text("⚠")
                        .font(.system(size: 14))
                    VStack(alignment: .leading, spacing: 3) {
                        Text(issue.message)
                            .font(.system(size: 12, weight: .medium))
                            .foregroundStyle(AlexTheme.Colors.destructive)
                        Text(issue.code)
                            .font(AlexTheme.Fonts.metaMono)
                            .foregroundStyle(AlexTheme.Colors.textTertiary)
                    }
                    Spacer(minLength: 8)
                    if issue.fixable {
                        PillButton(
                            title: repairInFlight
                                ? "Opening…"
                                : (issue.code == "reauth" ? "Reauth Dario" : "Fix"),
                            variant: .primary,
                            isEnabled: !repairInFlight,
                            isBusy: repairInFlight
                        ) {
                            if issue.code == "reauth" {
                                reauthDario()
                            } else {
                                Task { await repair() }
                            }
                        }
                    }
                }
            }
        }

        darioCard {
            SectionLabel(text: "Runtime", style: .prominent)
            DarioPreferenceRow(
                label: "Node",
                value: status.resolvedNodeBin,
                detail: status.runtimeVersion
            )
            DarioPreferenceRow(label: "Claude", value: status.resolvedClaudeBin)
        }

        darioCard {
            SectionLabel(text: "Active generation", style: .prominent)
            if let generation = activeGeneration(in: status) {
                DarioPreferenceRow(label: "ID", value: generation.id)
                DarioPreferenceRow(label: "Version", value: generation.version)
                DarioPreferenceRow(label: "Phase", value: generation.phase)
                DarioPreferenceRow(
                    label: "Last probe",
                    value: probeText(generation.lastProbe),
                    valueTint: generation.lastProbe?.ok == false
                        ? AlexTheme.Colors.destructive : nil
                )
            } else {
                Text("No active generation")
                    .font(.system(size: 12))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
            }
        }

        darioCard {
            SectionLabel(text: "Prompt caches", style: .prominent)
            let models = Array(Set((status.promptCaches ?? []).compactMap(\.model))).sorted()
            if models.isEmpty {
                Text("none yet")
                    .font(.system(size: 12))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
            } else {
                ForEach(models, id: \.self) { model in
                    Text(model)
                        .font(AlexTheme.Fonts.metaMono)
                        .foregroundStyle(AlexTheme.Colors.textSecondary)
                }
            }
        }

        HStack(spacing: 8) {
            PillButton(title: "Open Dario viewer", variant: .solidAccent, systemImage: "rectangle.on.rectangle") {
                onOpenDario()
            }
            if let actionResult {
                Text(actionResult)
                    .font(.system(size: 11))
                    .foregroundStyle(actionResult.hasPrefix("Fix failed")
                        ? AlexTheme.Colors.destructive : AlexTheme.Colors.success)
            }
            Spacer()
        }

        if let loadError {
            Text("Last refresh failed: \(loadError)")
                .font(.system(size: 10))
                .foregroundStyle(AlexTheme.Colors.warningOrange)
        }
    }

    private var health: DarioHealthEvaluation { DarioHealth.evaluate(status) }

    private func routingLine(_ status: DarioAdminStatus) -> String {
        let mode = store.config?.anthropicUpstream ?? "direct"
        let enabled = status.routeEnabled ?? (mode == "dario")
        let effective = enabled ? "dario" : "direct"
        let reason: String
        switch mode {
        case "auto": reason = enabled ? "auto route enabled" : "auto route inactive"
        case "dario": reason = enabled ? "explicitly enabled" : "route disabled by daemon"
        default: reason = enabled ? "enabled by daemon" : "direct mode selected"
        }
        return "routing: \(effective) — \(mode): \(reason)"
    }

    private func activeGeneration(in status: DarioAdminStatus) -> DarioGenerationDetail? {
        guard let id = status.activeGenerationId else { return nil }
        return status.generations.first { $0.id == id }
    }

    private func probeText(_ probe: DarioProbeDetail?) -> String {
        guard let probe else { return "not probed" }
        if probe.ok {
            return probe.latencyMs.map { "healthy · \($0)ms" } ?? "healthy"
        }
        return probe.error ?? "failed"
    }

    private func darioCard<Content: View>(@ViewBuilder content: () -> Content) -> some View {
        VStack(alignment: .leading, spacing: 9, content: content)
            .padding(14)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(RoundedRectangle(cornerRadius: AlexTheme.Radius.md)
                .fill(AlexTheme.Colors.surfaceFaint))
            .overlay(RoundedRectangle(cornerRadius: AlexTheme.Radius.md)
                .strokeBorder(AlexTheme.Colors.cardBorder))
    }

    private func refresh() async {
        isLoading = true
        loadError = nil
        defer { isLoading = false }
        guard let config = store.config ?? DaemonDiscovery.load() else {
            loadError = "No Alexandria daemon configuration was found."
            return
        }
        do {
            status = try await AlexandriaClient(config: config).darioDetail()
        } catch is CancellationError {
            return
        } catch {
            loadError = error.localizedDescription
        }
    }

    private func repair() async {
        guard let config = store.config ?? DaemonDiscovery.load() else {
            actionResult = "Fix failed: no daemon configuration"
            return
        }
        repairInFlight = true
        defer { repairInFlight = false }
        do {
            try await AlexandriaClient(config: config).darioRepair()
            actionResult = "Fix requested; refreshing status…"
            await store.refresh()
            await refresh()
            if status?.issue == nil { actionResult = "Fix completed" }
        } catch is CancellationError {
            return
        } catch {
            actionResult = "Fix failed: \(error.localizedDescription)"
        }
    }

    private func reauthDario() {
        let binary = DaemonController.findBinary() ?? "alexandria"
        repairInFlight = true
        TerminalLauncher.launchDarioReauth(daemonBinary: binary)
        actionResult = "Reauth opened in Terminal; repair runs after login."
        repairInFlight = false
    }
}

private struct DarioPreferenceRow: View {
    let label: String
    let value: String?
    var detail: String? = nil
    var valueTint: Color? = nil

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 12) {
            Text("\(label):")
                .font(.system(size: 11, weight: .medium))
                .foregroundStyle(AlexTheme.Colors.textTertiary)
                .frame(width: 74, alignment: .leading)
            if let value {
                Text(value)
                    .font(AlexTheme.Fonts.metaMono)
                    .foregroundStyle(valueTint ?? AlexTheme.Colors.textSecondary)
                    .textSelection(.enabled)
                if let detail {
                    Text(detail)
                        .font(AlexTheme.Fonts.metaMono)
                        .foregroundStyle(AlexTheme.Colors.textTertiary)
                }
            } else {
                Text("(not found)")
                    .font(AlexTheme.Fonts.metaMono)
                    .foregroundStyle(AlexTheme.Colors.destructive)
            }
            Spacer(minLength: 0)
        }
    }
}
