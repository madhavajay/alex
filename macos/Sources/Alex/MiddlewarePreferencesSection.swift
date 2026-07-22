import AppKit
import SwiftUI
import AlexCore

/// Preferences → Middleware. The daemon remains the authority for validation;
/// this pane deliberately fails softly when paired with an older beta daemon.
struct MiddlewarePreferencesSection: View {
    let store: SnapshotStore
    var migratedFromFailover = false
    var onOpenTraceBrowser: (String) -> Void = { _ in }

    @State private var runtime: MiddlewareRuntimeStatus?
    @State private var settings = MiddlewareSettings()
    @State private var isLoading = true
    @State private var isSavingSettings = false
    @State private var isReloading = false
    @State private var loadError: String?
    @State private var actionResult: String?
    @State private var actionIsError = false
    @State private var showingWizard = false
    @State private var wizardDraft = MiddlewareWizardDraft.fableToSolExample
    @State private var wizardEditingID: String?
    @State private var inspectedRule: MiddlewareRuleSpecV1?
    @State private var pendingDelete: MiddlewareRuleSpecV1?
    @State private var activity: [MiddlewareActivityEvent] = []
    @State private var activityLoadError: String?

    private var builtIns: [MiddlewareRuleSpecV1] {
        (runtime?.rules ?? []).filter(\.isBuiltIn)
    }

    private var customRules: [MiddlewareRuleSpecV1] {
        (runtime?.rules ?? []).filter { !$0.isBuiltIn }
    }

    var body: some View {
        VStack(spacing: 0) {
            paneHeader
            ScrollView {
                VStack(alignment: .leading, spacing: 18) {
                    if migratedFromFailover { migrationBanner }
                    if isLoading {
                        loadingState
                    } else {
                        if let loadError { unavailableCard(loadError) }
                        runtimeSection
                        builtInSection
                        rulesSection
                        activitySection
                        scriptsSection
                        leasesSection
                        safetySection
                    }
                }
                .padding(24)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        .task { await load() }
        .sheet(isPresented: $showingWizard) {
            MiddlewareWizard(
                store: store,
                draft: $wizardDraft,
                editingRuleID: $wizardEditingID,
                onSaved: {
                    showingWizard = false
                    Task { await load(showSpinner: false) }
                })
        }
        .sheet(item: $inspectedRule) { rule in
            MiddlewareRuleInspector(rule: rule)
        }
        .alert("Delete middleware?", isPresented: Binding(
            get: { pendingDelete != nil },
            set: { if !$0 { pendingDelete = nil } }
        ), presenting: pendingDelete) { rule in
            Button("Delete", role: .destructive) {
                Task { await delete(rule) }
            }
            Button("Cancel", role: .cancel) {}
        } message: { rule in
            Text("“\(rule.name)” will stop running immediately.")
        }
    }

    private var paneHeader: some View {
        HStack(spacing: 10) {
            Image(systemName: "arrow.triangle.branch")
                .font(.system(size: 20, weight: .medium))
                .foregroundStyle(AlexTheme.Colors.primary)
            VStack(alignment: .leading, spacing: 1) {
                Text("Middleware")
                    .font(AlexTheme.Fonts.panelTitle)
                    .foregroundStyle(AlexTheme.Colors.foreground)
                Text("Match requests and failures, then route or patch")
                    .font(.system(size: 12))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
            }
            Spacer()
            Button {
                beginCreate()
            } label: {
                Label("Add Middleware", systemImage: "plus")
            }
            .buttonStyle(.borderedProminent)
            .controlSize(.small)
            .accessibilityHint("Opens the Middleware Wizard")
        }
        .padding(.horizontal, 24)
        .padding(.vertical, 14)
        .overlay(alignment: .bottom) {
            Rectangle().fill(AlexTheme.Colors.overlay(0.06)).frame(height: 1)
                .padding(.horizontal, 24)
        }
    }

    private var loadingState: some View {
        HStack(spacing: 8) {
            ProgressView().controlSize(.small)
            Text("Loading middleware…")
        }
        .font(.system(size: 12))
        .foregroundStyle(AlexTheme.Colors.textSecondary)
        .padding(.vertical, 28)
        .frame(maxWidth: .infinity)
    }

    private var migrationBanner: some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: "arrow.right.circle.fill")
                .foregroundStyle(AlexTheme.Colors.primary)
            VStack(alignment: .leading, spacing: 3) {
                Text("Failover has moved to Middleware")
                    .font(.system(size: 12, weight: .semibold))
                Text("Your existing settings appear below as built-in policies. They use the same rule engine as custom middleware.")
                    .font(.system(size: 11))
                    .foregroundStyle(AlexTheme.Colors.textSecondary)
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: AlexTheme.Radius.lg)
            .fill(AlexTheme.Colors.primary.opacity(0.09)))
    }

    private func unavailableCard(_ message: String) -> some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: "exclamationmark.triangle.fill")
                .foregroundStyle(AlexTheme.Colors.warningOrange)
            VStack(alignment: .leading, spacing: 5) {
                Text("Middleware API unavailable")
                    .font(.system(size: 12, weight: .semibold))
                Text(message)
                    .font(AlexTheme.Fonts.metaMono)
                    .foregroundStyle(AlexTheme.Colors.textSecondary)
                    .textSelection(.enabled)
                Text("This is expected when the app is connected to a daemon from before the middleware beta.")
                    .font(.system(size: 11))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
                Button("Try again") { Task { await load() } }
                    .buttonStyle(.bordered)
                    .controlSize(.small)
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .alexCard()
    }

    private var runtimeSection: some View {
        VStack(alignment: .leading, spacing: 8) {
            SectionLabel(text: "Runtime", style: .prominent)
            VStack(spacing: 0) {
                HStack(spacing: 16) {
                    VStack(alignment: .leading, spacing: 3) {
                        HStack(spacing: 6) {
                            Circle()
                                .fill(settings.enabled && loadError == nil
                                    ? AlexTheme.Colors.success : AlexTheme.Colors.textFaint)
                                .frame(width: 7, height: 7)
                            Text(settings.enabled ? "Middleware enabled" : "Middleware paused")
                                .font(.system(size: 13, weight: .semibold))
                        }
                        Text(runtimeSummary)
                            .font(.system(size: 11))
                            .foregroundStyle(AlexTheme.Colors.textTertiary)
                    }
                    Spacer()
                    Toggle("", isOn: $settings.enabled)
                        .labelsHidden()
                        .settingsSwitch()
                        .disabled(runtime == nil)
                        .accessibilityLabel("Enable middleware")
                }
                .padding(12)

                RowDivider()

                HStack(spacing: 8) {
                    Button(isSavingSettings ? "Saving…" : "Save") {
                        Task { await saveSettings() }
                    }
                    .buttonStyle(.borderedProminent)
                    .controlSize(.small)
                    .disabled(runtime == nil || isSavingSettings)
                    Button("Revert") { applyRuntimeSettings() }
                        .buttonStyle(.bordered)
                        .controlSize(.small)
                        .disabled(runtime == nil || isSavingSettings)
                    Button(isReloading ? "Reloading…" : "Reload") {
                        Task { await reload() }
                    }
                    .buttonStyle(.bordered)
                    .controlSize(.small)
                    .disabled(runtime == nil || isReloading)
                    Button("Open Folder") { openMiddlewareFolder() }
                        .buttonStyle(.bordered)
                        .controlSize(.small)
                    Spacer()
                    if let actionResult {
                        Text(actionResult)
                            .font(.system(size: 11))
                            .foregroundStyle(actionIsError
                                ? AlexTheme.Colors.destructive : AlexTheme.Colors.success)
                            .lineLimit(2)
                    }
                }
                .padding(12)
            }
            .alexCard()

            if let errors = runtime?.errors, !errors.isEmpty {
                Label("\(errors.count) load or runtime error\(errors.count == 1 ? "" : "s")", systemImage: "exclamationmark.octagon.fill")
                    .font(.system(size: 11, weight: .medium))
                    .foregroundStyle(AlexTheme.Colors.destructive)
            }
        }
    }

    private var builtInSection: some View {
        VStack(alignment: .leading, spacing: 8) {
            SectionLabel(text: "Default middleware", style: .prominent)
            Text("Alex ships one simple, inspectable Fable 5 fallback.")
                .font(.system(size: 11))
                .foregroundStyle(AlexTheme.Colors.textTertiary)

            ForEach(BuiltInMiddlewarePolicy.allCases) { policy in
                let rules = builtIns.filter { policy.matches($0.id) }
                policyCard(policy, rules: rules)
            }
        }
    }

    private func policyCard(
        _ policy: BuiltInMiddlewarePolicy,
        rules: [MiddlewareRuleSpecV1]
    ) -> some View {
        let rule = rules.first
        return HStack(alignment: .center, spacing: 10) {
            Image(systemName: policy.icon)
                .font(.system(size: 14, weight: .medium))
                .foregroundStyle(AlexTheme.Colors.primary)
                .frame(width: 26, height: 26)
                .background(Circle().fill(AlexTheme.Colors.primary.opacity(0.10)))
            VStack(alignment: .leading, spacing: 2) {
                Text(rules.count == 1 ? (rule?.name ?? policy.title) : policy.title)
                    .font(.system(size: 12, weight: .semibold))
                Text(rules.count == 1 ? (rule?.description ?? policy.summary) : policy.summary)
                    .font(.system(size: 10))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
                    .lineLimit(2)
            }
            Spacer(minLength: 8)
            Button("Edit") { if let rule { edit(rule) } }
                .buttonStyle(.borderless)
                .controlSize(.small)
                .disabled(rule.map(isWizardEditable) != true)
            Toggle("", isOn: Binding(
                get: { rules.isEmpty ? policy.defaultEnabled : rules.allSatisfy(\.enabled) },
                set: { enabled in Task { await setEnabled(rules, enabled) } }
            ))
            .labelsHidden()
            .settingsSwitch()
            .disabled(!policy.canToggle(rules))
            .accessibilityLabel("Enable \(policy.title)")
        }
        .padding(11)
        .alexCard(background: AlexTheme.Colors.overlay(0.03))
    }

    private var rulesSection: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                SectionLabel(text: "Basic rules", style: .prominent)
                Spacer()
                Text("\(customRules.count)")
                    .font(AlexTheme.Fonts.metaMono)
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
            }
            if customRules.isEmpty {
                VStack(spacing: 7) {
                    Image(systemName: "arrow.triangle.branch")
                        .font(.system(size: 20))
                        .foregroundStyle(AlexTheme.Colors.textFaint)
                    Text("No custom middleware yet")
                        .font(.system(size: 12, weight: .semibold))
                    Text("The Middleware Wizard can build a safe rerouting rule without code.")
                        .font(.system(size: 11))
                        .foregroundStyle(AlexTheme.Colors.textTertiary)
                    Button("Add Middleware") { beginCreate() }
                        .buttonStyle(.bordered)
                        .controlSize(.small)
                }
                .padding(18)
                .frame(maxWidth: .infinity)
                .alexCard()
            } else {
                ForEach(customRules) { rule in
                    ruleRow(rule)
                }
            }
        }
    }

    private func ruleRow(_ rule: MiddlewareRuleSpecV1) -> some View {
        HStack(alignment: .center, spacing: 10) {
            Toggle("", isOn: Binding(
                get: { rule.enabled },
                set: { enabled in Task { await setEnabled(rule, enabled) } }
            ))
            .labelsHidden()
            .settingsSwitch()
            .accessibilityLabel("Enable \(rule.name)")
            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 7) {
                    Text(rule.name).font(.system(size: 12, weight: .semibold))
                    Text("priority \(rule.priority)")
                        .font(AlexTheme.Fonts.metaMono)
                        .foregroundStyle(AlexTheme.Colors.textTertiary)
                }
                Text(ruleSummary(rule))
                    .font(.system(size: 10))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
                    .lineLimit(2)
                Text("\(rule.hitCount ?? 0) matches\(lastMatchedText(rule))")
                    .font(.system(size: 9))
                    .foregroundStyle(AlexTheme.Colors.textFaint)
            }
            Spacer()
            Menu {
                if isWizardEditable(rule) {
                    Button("Edit in Middleware Wizard") { edit(rule) }
                }
                Button("Inspect generated rule") { inspectedRule = rule }
                if isWizardEditable(rule) {
                    Button("Duplicate") { duplicateInWizard(rule) }
                }
                Divider()
                Button("Delete", role: .destructive) { pendingDelete = rule }
            } label: {
                Image(systemName: "ellipsis.circle")
                    .foregroundStyle(AlexTheme.Colors.textSecondary)
            }
            .menuStyle(.borderlessButton)
            .frame(width: 24)
            .accessibilityLabel("Actions for \(rule.name)")
        }
        .padding(11)
        .alexCard(background: AlexTheme.Colors.overlay(0.03))
    }

    private var activitySection: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                SectionLabel(text: "Recent middleware activity", style: .prominent)
                Spacer()
                Button("Refresh") { Task { await load(showSpinner: false) } }
                    .buttonStyle(.borderless)
                    .controlSize(.small)
            }
            Text("Run a real request in Claude, Pi, or another connected harness. Alex shows whether the resulting trace matched a rule and whether the action executed.")
                .font(.system(size: 11))
                .foregroundStyle(AlexTheme.Colors.textTertiary)

            if activity.isEmpty {
                Text(activityLoadError.map { "Activity unavailable: \($0)" }
                    ?? "No recent Fable or middleware events in the last 24 hours.")
                    .font(.system(size: 11))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
                    .padding(12)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .alexCard(background: AlexTheme.Colors.overlay(0.03))
            } else {
                ForEach(activity) { event in
                    activityRow(event)
                }
            }
        }
    }

    private func activityRow(_ event: MiddlewareActivityEvent) -> some View {
        let matches = event.matchedDecisions
        let executed = matches.contains { $0.executed == true }
        let source = event.attempts.first?.model ?? event.requestedModel ?? "unknown model"
        let target = event.finalModel ?? event.attempts.last?.model ?? source
        let refusal = event.attempts.first { $0.errorKind == "upstream_refusal" }
        let outcome: String = if let match = matches.first {
            "Matched \(match.ruleName ?? match.ruleId) · \(executed ? "action executed" : "action not executed")"
        } else if let refusal {
            "Refusal observed\(refusal.errorCode.map { " (\($0))" } ?? "") · no rule matched"
        } else {
            "No rule matched"
        }

        return HStack(alignment: .top, spacing: 10) {
            Image(systemName: executed
                ? "arrow.triangle.branch" : (matches.isEmpty ? "circle.dashed" : "exclamationmark.triangle"))
                .foregroundStyle(executed
                    ? AlexTheme.Colors.success : (matches.isEmpty
                        ? AlexTheme.Colors.textTertiary : AlexTheme.Colors.warningOrange))
                .frame(width: 20)
            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 6) {
                    Text(event.harness?.capitalized ?? "Harness")
                        .font(.system(size: 11, weight: .semibold))
                    Text(source == target ? source : "\(source) → \(target)")
                        .font(AlexTheme.Fonts.metaMono)
                        .lineLimit(1)
                }
                Text(outcome)
                    .font(.system(size: 10, weight: matches.isEmpty ? .regular : .medium))
                    .foregroundStyle(executed
                        ? AlexTheme.Colors.success : AlexTheme.Colors.textSecondary)
                HStack(spacing: 6) {
                    if let ts = event.tsMs { Text(formattedDate(ts)) }
                    if let status = event.status { Text("HTTP \(status)") }
                    Text(String(event.id.prefix(8)))
                }
                .font(AlexTheme.Fonts.metaMicro)
                .foregroundStyle(AlexTheme.Colors.textFaint)
            }
            Spacer()
            Button("Open Trace") {
                if let session = event.sessionId {
                    onOpenTraceBrowser("session:\(session)")
                } else {
                    onOpenTraceBrowser(event.id)
                }
            }
            .buttonStyle(.borderless)
            .controlSize(.small)
        }
        .padding(11)
        .alexCard(background: AlexTheme.Colors.overlay(0.03))
    }

    private var scriptsSection: some View {
        VStack(alignment: .leading, spacing: 8) {
            SectionLabel(text: "Advanced scripts", style: .prominent)
            let scripts = runtime?.scripts ?? []
            if scripts.isEmpty {
                Text("Rhai is intentionally deferred for this beta while the declarative rule ABI and performance are validated. Script fields and limits are reserved for the next release.")
                    .font(.system(size: 11))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
                    .padding(12)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .alexCard(background: AlexTheme.Colors.overlay(0.03))
            } else {
                ForEach(scripts) { script in
                    HStack(spacing: 10) {
                        Image(systemName: script.status == "loaded" ? "checkmark.circle.fill" : "xmark.octagon.fill")
                            .foregroundStyle(script.status == "loaded"
                                ? AlexTheme.Colors.success : AlexTheme.Colors.destructive)
                        VStack(alignment: .leading, spacing: 2) {
                            Text(script.script).font(AlexTheme.Fonts.metaMono)
                            Text(script.hooks.map(\.rawValue).joined(separator: " · "))
                                .font(.system(size: 10))
                                .foregroundStyle(AlexTheme.Colors.textTertiary)
                            if let error = script.error {
                                Text(error).font(.system(size: 10)).foregroundStyle(AlexTheme.Colors.destructive)
                            }
                        }
                        Spacer()
                        Text(script.status.capitalized)
                            .font(.system(size: 10, weight: .semibold))
                            .foregroundStyle(AlexTheme.Colors.textSecondary)
                    }
                    .padding(11)
                    .alexCard(background: AlexTheme.Colors.overlay(0.03))
                }
            }
        }
    }

    private var leasesSection: some View {
        VStack(alignment: .leading, spacing: 8) {
            SectionLabel(text: "Active session routes", style: .prominent)
            let leases = runtime?.leases ?? []
            if leases.isEmpty {
                Text("No chats are pinned to a fallback route.")
                    .font(.system(size: 11))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
            } else {
                ForEach(leases) { lease in
                    HStack(spacing: 10) {
                        VStack(alignment: .leading, spacing: 2) {
                            Text("\(lease.originalModel) → \(lease.target.displayModel)")
                                .font(.system(size: 12, weight: .medium))
                            Text("\(lease.harness ?? "unknown harness") · \(lease.sessionId) · \(lease.target.displayProviders) · expires \(formattedDate(lease.expiresMs))")
                                .font(AlexTheme.Fonts.metaMono)
                                .foregroundStyle(AlexTheme.Colors.textTertiary)
                                .lineLimit(1)
                        }
                        Spacer()
                        Button("Clear") { Task { await clear(lease) } }
                            .buttonStyle(.bordered)
                            .controlSize(.small)
                    }
                    .padding(10)
                    .alexCard(background: AlexTheme.Colors.overlay(0.03))
                }
            }
        }
    }

    private var safetySection: some View {
        VStack(alignment: .leading, spacing: 8) {
            SectionLabel(text: "Limits and safety", style: .prominent)
            VStack(spacing: 0) {
                safetyRow("Error-body inspection", "\(settings.errorBodyLimitBytes / 1024) KiB")
                RowDivider()
                safetyRow("Maximum upstream attempts", "\(settings.maxAttempts)")
                RowDivider()
                safetyRow("Reserved Rhai timeout", "\(settings.defaultScriptTimeoutMs) ms")
                RowDivider()
                safetyRow("Reserved Rhai operation budget", "\(settings.defaultScriptMaxOperations)")
            }
            .alexCard(background: AlexTheme.Colors.overlay(0.03))
            Text("Middleware contexts exclude credentials and other secret headers. Normal successful streaming responses are not buffered.")
                .font(.system(size: 10))
                .foregroundStyle(AlexTheme.Colors.textTertiary)
        }
    }

    private func safetyRow(_ label: String, _ value: String) -> some View {
        HStack {
            Text(label).font(.system(size: 11))
            Spacer()
            Text(value).font(AlexTheme.Fonts.metaMono).foregroundStyle(AlexTheme.Colors.textSecondary)
        }
        .padding(.horizontal, 11)
        .padding(.vertical, 8)
    }

    private var runtimeSummary: String {
        guard let runtime else { return "Waiting for a compatible daemon" }
        let generation = runtime.generation.map { "generation \($0)" } ?? "no active generation"
        let reloaded = runtime.lastReloadMs.map { " · reloaded \(formattedDate($0))" } ?? ""
        return "\(generation) · \(runtime.rules.count) rules · \(runtime.scripts.count) scripts\(reloaded)"
    }

    private func client() -> AlexClient? {
        guard let config = store.config ?? DaemonDiscovery.load() else { return nil }
        return AlexClient(config: config)
    }

    private func load(showSpinner: Bool = true) async {
        if showSpinner { isLoading = true }
        loadError = nil
        defer { isLoading = false }
        guard let client = client() else {
            loadError = "No Alex daemon configuration was found."
            return
        }
        do {
            let value = try await client.middlewareStatus()
            runtime = value
            settings = value.settings
            do {
                activity = try await client.middlewareActivity()
                activityLoadError = nil
            } catch is CancellationError {
            } catch {
                activityLoadError = error.localizedDescription
            }
        } catch is CancellationError {
        } catch {
            runtime = nil
            loadError = error.localizedDescription
        }
    }

    private func saveSettings() async {
        guard let client = client() else { return report("No daemon configuration", error: true) }
        isSavingSettings = true
        defer { isSavingSettings = false }
        do {
            _ = try await client.updateMiddlewareSettings(settings)
            await load(showSpinner: false)
            report("Settings saved")
        } catch is CancellationError {
        } catch { report("Save failed: \(error.localizedDescription)", error: true) }
    }

    private func reload() async {
        guard let client = client() else { return report("No daemon configuration", error: true) }
        isReloading = true
        defer { isReloading = false }
        do {
            _ = try await client.reloadMiddleware()
            await load(showSpinner: false)
            report("Middleware reloaded")
        } catch is CancellationError {
        } catch { report("Reload failed: \(error.localizedDescription)", error: true) }
    }

    private func applyRuntimeSettings() {
        if let runtime { settings = runtime.settings }
    }

    private func setEnabled(_ rule: MiddlewareRuleSpecV1, _ enabled: Bool) async {
        guard let client = client() else { return report("No daemon configuration", error: true) }
        var updated = rule
        updated.enabled = enabled
        // Runtime-only fields are accepted by tolerant daemons, but omitting
        // them keeps write payloads stable across the beta.
        updated.builtIn = nil
        updated.hitCount = nil
        updated.lastMatchedMs = nil
        updated.validationErrors = nil
        do {
            _ = try await client.updateMiddlewareRule(updated)
            await load(showSpinner: false)
            report(enabled ? "\(rule.name) enabled" : "\(rule.name) disabled")
        } catch is CancellationError {
        } catch { report("Update failed: \(error.localizedDescription)", error: true) }
    }

    private func setEnabled(_ rules: [MiddlewareRuleSpecV1], _ enabled: Bool) async {
        guard !rules.isEmpty else { return }
        guard let client = client() else { return report("No daemon configuration", error: true) }
        do {
            for rule in rules {
                var updated = rule
                updated.enabled = enabled
                updated.builtIn = nil
                updated.hitCount = nil
                updated.lastMatchedMs = nil
                updated.validationErrors = nil
                _ = try await client.updateMiddlewareRule(updated)
            }
            await load(showSpinner: false)
            report(enabled ? "Built-in policy enabled" : "Built-in policy disabled")
        } catch is CancellationError {
        } catch {
            await load(showSpinner: false)
            report("Update failed: \(error.localizedDescription)", error: true)
        }
    }

    private func delete(_ rule: MiddlewareRuleSpecV1) async {
        pendingDelete = nil
        guard let client = client() else { return report("No daemon configuration", error: true) }
        do {
            try await client.deleteMiddlewareRule(id: rule.id)
            await load(showSpinner: false)
            report("\(rule.name) deleted")
        } catch is CancellationError {
        } catch { report("Delete failed: \(error.localizedDescription)", error: true) }
    }

    private func clear(_ lease: MiddlewareRouteLease) async {
        guard let client = client() else { return report("No daemon configuration", error: true) }
        do {
            try await client.clearMiddlewareLease(id: lease.id)
            await load(showSpinner: false)
            report("Session route cleared")
        } catch is CancellationError {
        } catch { report("Clear failed: \(error.localizedDescription)", error: true) }
    }

    private func beginCreate() {
        wizardDraft = .fableToSolExample
        wizardEditingID = nil
        showingWizard = true
    }

    private func edit(_ rule: MiddlewareRuleSpecV1) {
        wizardDraft = MiddlewareWizardDraft(rule: rule)
        wizardEditingID = rule.id
        showingWizard = true
    }

    private func duplicateInWizard(_ rule: MiddlewareRuleSpecV1) {
        var draft = MiddlewareWizardDraft(rule: rule)
        draft.name += " Copy"
        wizardDraft = draft
        wizardEditingID = nil
        showingWizard = true
    }

    private func report(_ message: String, error: Bool = false) {
        actionResult = message
        actionIsError = error
    }

    private func openMiddlewareFolder() {
        let home = FileManager.default.homeDirectoryForCurrentUser
        let directory = home.appendingPathComponent(".alex/middleware", isDirectory: true)
        let existing = FileManager.default.fileExists(atPath: directory.path)
            ? directory : home.appendingPathComponent(".alex", isDirectory: true)
        NSWorkspace.shared.open(existing)
    }

    private func formattedDate(_ milliseconds: Int64) -> String {
        Date(timeIntervalSince1970: Double(milliseconds) / 1000)
            .formatted(date: .abbreviated, time: .shortened)
    }

    private func lastMatchedText(_ rule: MiddlewareRuleSpecV1) -> String {
        guard let value = rule.lastMatchedMs else { return "" }
        return " · last \(formattedDate(value))"
    }

    private func ruleSummary(_ rule: MiddlewareRuleSpecV1) -> String {
        guard isWizardEditable(rule) else {
            return "Advanced \(rule.hook.rawValue.replacingOccurrences(of: "_", with: " ")) rule — inspect its structured definition."
        }
        return MiddlewareWizardDraft(rule: rule).summary
    }

    private func isWizardEditable(_ rule: MiddlewareRuleSpecV1) -> Bool {
        guard rule.hook == .attemptResult else { return false }
        if rule.then.retrySameRoute != nil { return rule.then.reroute == nil }
        guard let reroute = rule.then.reroute else { return false }
        return reroute.providerMode != .exclude
    }
}

private enum BuiltInMiddlewarePolicy: String, CaseIterable, Identifiable {
    case fableToSol = "alex.fable-5-to-gpt-5.6-sol"

    var id: String { rawValue }
    var title: String { "Fable 5 → GPT-5.6 Sol" }
    var summary: String {
        "When Anthropic Fable 5 refuses, switch the stable session to high-effort GPT-5.6 Sol for 24 hours."
    }
    var icon: String { "arrow.right.circle" }
    var defaultEnabled: Bool { true }

    func matches(_ ruleID: String) -> Bool { ruleID == id }
    func canToggle(_ rules: [MiddlewareRuleSpecV1]) -> Bool { !rules.isEmpty }
}

private struct MiddlewareRuleInspector: View {
    @Environment(\.dismiss) private var dismiss
    let rule: MiddlewareRuleSpecV1

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                VStack(alignment: .leading, spacing: 2) {
                    Text(rule.name).font(.headline)
                    Text(rule.id).font(.system(size: 10, design: .monospaced)).foregroundStyle(.secondary)
                }
                Spacer()
                Button("Done") { dismiss() }.keyboardShortcut(.cancelAction)
            }
            Text(MiddlewareWizardDraft(rule: rule).summary)
                .font(.system(size: 12))
            ScrollView {
                Text(prettyRuleJSON(rule))
                    .font(.system(size: 11, design: .monospaced))
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(10)
            }
            .background(RoundedRectangle(cornerRadius: 8).fill(Color.primary.opacity(0.04)))
        }
        .padding(20)
        .frame(width: 620, height: 480)
    }
}

private func prettyRuleJSON(_ rule: MiddlewareRuleSpecV1) -> String {
    let encoder = JSONEncoder()
    encoder.outputFormatting = [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes]
    guard let data = try? encoder.encode(rule) else { return "Unable to encode rule." }
    return String(data: data, encoding: .utf8) ?? "Unable to encode rule."
}
