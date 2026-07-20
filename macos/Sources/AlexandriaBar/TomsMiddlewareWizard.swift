import SwiftUI
import AlexandriaBarCore

/// A four-step declarative-rule builder. It intentionally exposes the common
/// email-filter-shaped subset and always asks the daemon to validate the final
/// RuleSpec before Save becomes available.
struct TomsMiddlewareWizard: View {
    let store: SnapshotStore
    @Binding var draft: MiddlewareWizardDraft
    let editingRuleID: String?
    let onSaved: () -> Void

    @Environment(\.dismiss) private var dismiss
    @State private var step = 0
    @State private var validation: MiddlewareValidationResponse?
    @State private var validationError: String?
    @State private var isValidating = false
    @State private var isSaving = false
    @State private var saveError: String?

    private let harnesses = ["claude", "codex", "pi", "amp", "gemini", "opencode"]
    private let providers = ProviderInfo.supportedProviders

    var body: some View {
        VStack(spacing: 0) {
            header
            stepBar
            Divider()
            ScrollView {
                Group {
                    switch step {
                    case 0: nameStep
                    case 1: conditionsStep
                    case 2: actionStep
                    default: reviewStep
                    }
                }
                .padding(22)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            Divider()
            footer
        }
        .frame(width: 700, height: 640)
        .onChange(of: draft) { _, _ in
            validation = nil
            validationError = nil
            saveError = nil
        }
    }

    private var header: some View {
        HStack(spacing: 10) {
            Image(systemName: "wand.and.stars")
                .font(.system(size: 20, weight: .medium))
                .foregroundStyle(AlexTheme.Colors.primary)
            VStack(alignment: .leading, spacing: 2) {
                Text("Tom's Middleware Wizard")
                    .font(.system(size: 15, weight: .semibold))
                Text(editingRuleID == nil
                    ? "Build a routing rule without writing code"
                    : "Edit a basic declarative rule")
                    .font(.system(size: 11))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
            }
            Spacer()
            Button { dismiss() } label: { Image(systemName: "xmark") }
                .buttonStyle(.plain)
                .keyboardShortcut(.cancelAction)
                .accessibilityLabel("Close wizard")
        }
        .padding(.horizontal, 20)
        .frame(height: 56)
    }

    private var stepBar: some View {
        HStack(spacing: 0) {
            ForEach(Array(stepTitles.enumerated()), id: \.offset) { index, title in
                HStack(spacing: 6) {
                    ZStack {
                        Circle()
                            .fill(index <= step
                                ? AlexTheme.Colors.primary : AlexTheme.Colors.overlay(0.08))
                            .frame(width: 20, height: 20)
                        if index < step {
                            Image(systemName: "checkmark")
                                .font(.system(size: 9, weight: .bold))
                                .foregroundStyle(.white)
                        } else {
                            Text("\(index + 1)")
                                .font(.system(size: 9, weight: .bold))
                                .foregroundStyle(index <= step ? .white : AlexTheme.Colors.textTertiary)
                        }
                    }
                    Text(title)
                        .font(.system(size: 10, weight: index == step ? .semibold : .regular))
                        .foregroundStyle(index <= step
                            ? AlexTheme.Colors.foreground : AlexTheme.Colors.textTertiary)
                    if index < stepTitles.count - 1 {
                        Rectangle()
                            .fill(AlexTheme.Colors.overlay(index < step ? 0.16 : 0.06))
                            .frame(height: 1)
                            .padding(.horizontal, 8)
                    }
                }
                .frame(maxWidth: .infinity)
                .accessibilityLabel("Step \(index + 1), \(title)\(index == step ? ", current" : "")")
            }
        }
        .padding(.horizontal, 18)
        .padding(.bottom, 12)
    }

    private var nameStep: some View {
        VStack(alignment: .leading, spacing: 16) {
            stepHeading(
                "What should this middleware be called?",
                "Use a name that explains the behavior when it appears in a trace.")
            VStack(alignment: .leading, spacing: 6) {
                Text("Name").font(.system(size: 11, weight: .semibold))
                TextField("Move overloaded Fable chats to Sol", text: $draft.name)
                    .textFieldStyle(.roundedBorder)
                    .accessibilityLabel("Middleware name")
            }
            VStack(alignment: .leading, spacing: 6) {
                Text("Description (optional)").font(.system(size: 11, weight: .semibold))
                TextField("What this rule is for", text: $draft.description)
                    .textFieldStyle(.roundedBorder)
            }
            HStack(spacing: 10) {
                Image(systemName: "sparkles")
                    .foregroundStyle(AlexTheme.Colors.primary)
                VStack(alignment: .leading, spacing: 2) {
                    Text("Want a working example?")
                        .font(.system(size: 11, weight: .semibold))
                    Text("Load the Fable → GPT 5.6 Sol failover described in the middleware beta plan.")
                        .font(.system(size: 10))
                        .foregroundStyle(AlexTheme.Colors.textTertiary)
                }
                Spacer()
                Button("Use example") { draft = .fableToSolExample }
                    .buttonStyle(.bordered)
                    .controlSize(.small)
            }
            .padding(12)
            .alexCard(background: AlexTheme.Colors.primary.opacity(0.06))
            Spacer(minLength: 0)
        }
    }

    private var conditionsStep: some View {
        VStack(alignment: .leading, spacing: 16) {
            stepHeading(
                "When should it run?",
                "Leave a group empty to allow any value. Values inside a group are alternatives.")

            wizardGroup("Run on") {
                HStack {
                    Label("Failed attempt", systemImage: "exclamationmark.arrow.triangle.2.circlepath")
                        .font(.system(size: 11, weight: .medium))
                    Spacer()
                    Text("Basic rerouting beta")
                        .font(.system(size: 10))
                        .foregroundStyle(AlexTheme.Colors.textTertiary)
                }
                Text("Request and final-response patch hooks are reserved for a later wizard release.")
                    .font(.system(size: 10))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
            }

            wizardGroup("Harness") {
                chipWrap(values: harnesses, selected: draft.harnesses) { toggleHarness($0) }
                TextField("Version requirement (optional), e.g. >=2.1", text: $draft.harnessVersion)
                    .textFieldStyle(.roundedBorder)
            }

            HStack(alignment: .top, spacing: 12) {
                wizardGroup("Requested model") {
                    TextField("Any, exact, or wildcard: fable-*", text: $draft.modelPattern)
                        .textFieldStyle(.roundedBorder)
                }
                wizardGroup("Current provider") {
                    chipWrap(values: providers, selected: sourceProviders) { toggleSourceProvider($0) }
                }
            }

            if draft.hook == .attemptResult {
                wizardGroup("Failure") {
                    chipWrap(
                        values: MiddlewareWizardErrorKind.allCases,
                        title: \.rawValue,
                        selected: Array(draft.errorKinds)
                    ) { toggleErrorKind($0) }
                    HStack(spacing: 10) {
                        TextField("HTTP status: 429, 5xx, 500-599", text: $draft.statusText)
                            .textFieldStyle(.roundedBorder)
                        Picker("Combine", selection: $draft.conditionMode) {
                            Text("All conditions").tag(MiddlewareConditionMode.all)
                            Text("Any condition").tag(MiddlewareConditionMode.any)
                        }
                        .frame(width: 145)
                    }
                    VStack(alignment: .leading, spacing: 4) {
                        Text("Error body contains (one phrase per line)")
                            .font(.system(size: 10, weight: .medium))
                        TextEditor(text: $draft.bodyPhrasesText)
                            .font(.system(size: 11, design: .monospaced))
                            .frame(height: 62)
                            .padding(4)
                            .background(RoundedRectangle(cornerRadius: 5)
                                .stroke(AlexTheme.Colors.borderStrong))
                        if !draft.bodyPhrases.isEmpty {
                            Label("Only eligible failed responses are inspected, up to the configured byte cap.", systemImage: "gauge.with.dots.needle.33percent")
                                .font(.system(size: 10))
                                .foregroundStyle(AlexTheme.Colors.warningOrange)
                        }
                    }
                }
            } else {
                Label("Body and error matching are available only for Failed attempt.", systemImage: "info.circle")
                    .font(.system(size: 10))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
            }
        }
    }

    private var actionStep: some View {
        VStack(alignment: .leading, spacing: 16) {
            stepHeading(
                "What should Alex do?",
                "The router performs the action; middleware never receives credentials or network access.")

            wizardGroup("Action") {
                Picker("Action", selection: $draft.action) {
                    ForEach(MiddlewareWizardAction.allCases, id: \.self) { Text($0.rawValue).tag($0) }
                }
                .pickerStyle(.radioGroup)
                .labelsHidden()
            }

            if draft.action != .retrySame {
                wizardGroup(draft.action == .routeExact ? "Target model" : "Equivalence class") {
                    if draft.action == .routeExact {
                        TextField("gpt-5.6-sol", text: $draft.targetModel)
                            .textFieldStyle(.roundedBorder)
                    } else {
                        TextField("claude-fable-5", text: $draft.equivalenceClass)
                            .textFieldStyle(.roundedBorder)
                        Text("Use a source-model key configured by the Model Equivalence Failover policy.")
                            .font(.system(size: 10))
                            .foregroundStyle(AlexTheme.Colors.textTertiary)
                    }
                }

                wizardGroup("Provider choice") {
                    Picker("Provider choice", selection: $draft.providerMode) {
                        Text("Any available").tag(MiddlewareProviderMode.any)
                        Text("Prefer selected").tag(MiddlewareProviderMode.prefer)
                        Text("Only selected").tag(MiddlewareProviderMode.only)
                    }
                    .pickerStyle(.segmented)
                    .labelsHidden()
                    if draft.providerMode != .any {
                        chipWrap(values: providers, selected: draft.targetProviders) { toggleTargetProvider($0) }
                    }
                }

                wizardGroup("Apply") {
                    Picker("Apply", selection: $draft.scope) {
                        Text("This request only").tag(MiddlewareRouteScope.request)
                        Text("Keep for this session").tag(MiddlewareRouteScope.session)
                    }
                    .pickerStyle(.segmented)
                    .labelsHidden()
                    if draft.scope == .session {
                        Toggle("Require a stable, portable session before pinning", isOn: $draft.stableSessionRequired)
                            .toggleStyle(.checkbox)
                            .font(.system(size: 11))
                        HStack {
                            Text("Route lease")
                                .font(.system(size: 11))
                            Stepper(value: $draft.ttlSeconds, in: 300...604_800, step: 3_600) {
                                Text("\(max(1, draft.ttlSeconds / 3_600)) hours")
                                    .font(AlexTheme.Fonts.metaMono)
                            }
                            .frame(width: 150)
                        }
                    }
                }

                wizardGroup("Model-switch notice") {
                    Toggle("Tell the harness after the fallback succeeds", isOn: $draft.includeNotice)
                        .toggleStyle(.checkbox)
                        .font(.system(size: 11))
                    if draft.includeNotice {
                        TextField("We moved this chat to another model.", text: $draft.notice)
                            .textFieldStyle(.roundedBorder)
                        Text("A notice can buffer the exceptional fallback response. Normal successful streaming stays untouched.")
                            .font(.system(size: 10))
                            .foregroundStyle(AlexTheme.Colors.warningOrange)
                    }
                }
            }

            DisclosureGroup("Advanced") {
                HStack {
                    Text("Priority")
                    Stepper(value: $draft.priority, in: 0...10_000) {
                        Text("\(draft.priority)").font(AlexTheme.Fonts.metaMono)
                    }
                    .frame(width: 120)
                    Spacer()
                    Text("Higher priority runs first.")
                        .font(.system(size: 10))
                        .foregroundStyle(AlexTheme.Colors.textTertiary)
                }
                .padding(.top, 6)
            }
            .font(.system(size: 11, weight: .medium))
        }
    }

    private var reviewStep: some View {
        VStack(alignment: .leading, spacing: 14) {
            stepHeading(
                "Review and validate",
                "Alex validates capabilities, route safety, and the active daemon schema before this rule can be saved.")

            VStack(alignment: .leading, spacing: 6) {
                Text("Plain-language summary")
                    .font(.system(size: 11, weight: .semibold))
                Text(draft.summary)
                    .font(.system(size: 12))
                    .foregroundStyle(AlexTheme.Colors.textSecondary)
            }
            .padding(12)
            .frame(maxWidth: .infinity, alignment: .leading)
            .alexCard(background: AlexTheme.Colors.primary.opacity(0.06))

            if !draft.localValidationErrors.isEmpty {
                messageList(
                    title: "Fix before validating",
                    messages: draft.localValidationErrors,
                    color: AlexTheme.Colors.destructive,
                    icon: "xmark.octagon.fill")
            }
            if !draft.warnings.isEmpty {
                messageList(
                    title: "Things to know",
                    messages: draft.warnings,
                    color: AlexTheme.Colors.warningOrange,
                    icon: "exclamationmark.triangle.fill")
            }
            if let validation {
                messageList(
                    title: validation.valid ? "Daemon validation passed" : "Daemon rejected this rule",
                    messages: validation.valid
                        ? (validation.warnings.isEmpty
                            ? ["The rule is ready to save."]
                            : validation.warnings.map(\.displayText))
                        : validation.errors.map(\.displayText),
                    color: validation.valid ? AlexTheme.Colors.success : AlexTheme.Colors.destructive,
                    icon: validation.valid ? "checkmark.seal.fill" : "xmark.octagon.fill")
            } else if let validationError {
                messageList(
                    title: "Could not validate",
                    messages: [validationError],
                    color: AlexTheme.Colors.destructive,
                    icon: "wifi.exclamationmark")
            }

            VStack(alignment: .leading, spacing: 5) {
                Text("Structured RuleSpecV1 preview")
                    .font(.system(size: 11, weight: .semibold))
                ScrollView {
                    Text(rulePreview)
                        .font(.system(size: 10, design: .monospaced))
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(8)
                }
                .frame(height: 150)
                .background(RoundedRectangle(cornerRadius: 7)
                    .fill(AlexTheme.Colors.overlay(0.04)))
                .overlay(RoundedRectangle(cornerRadius: 7)
                    .stroke(AlexTheme.Colors.border))
            }

            if let saveError {
                Text(saveError)
                    .font(.system(size: 10))
                    .foregroundStyle(AlexTheme.Colors.destructive)
            }
        }
    }

    private var footer: some View {
        HStack(spacing: 8) {
            Button("Cancel") { dismiss() }
                .buttonStyle(.bordered)
                .keyboardShortcut(.cancelAction)
            Spacer()
            if step > 0 {
                Button("Back") { step -= 1 }
                    .buttonStyle(.bordered)
            }
            if step < 3 {
                Button("Continue") { step += 1 }
                    .buttonStyle(.borderedProminent)
                    .keyboardShortcut(.defaultAction)
                    .disabled(!canContinue)
            } else {
                Button(isValidating ? "Validating…" : "Validate with daemon") {
                    Task { await validate() }
                }
                .buttonStyle(.bordered)
                .disabled(isValidating || isSaving || !draft.localValidationErrors.isEmpty)
                Button(isSaving ? "Saving…" : (editingRuleID == nil ? "Save Middleware" : "Save Changes")) {
                    Task { await save() }
                }
                .buttonStyle(.borderedProminent)
                .keyboardShortcut(.defaultAction)
                .disabled(isSaving || isValidating || validation?.valid != true)
            }
        }
        .padding(.horizontal, 20)
        .frame(height: 56)
    }

    private var canContinue: Bool {
        switch step {
        case 0: !draft.name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        case 1:
            draft.hook != .attemptResult
                || !draft.errorKinds.isEmpty
                || !draft.statusMatchers.isEmpty
                || !draft.bodyPhrases.isEmpty
                || !draft.modelPattern.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        case 2:
            switch draft.action {
            case .retrySame: true
            case .routeExact: !draft.targetModel.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            case .routeEquivalent: !draft.equivalenceClass.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            }
        default: true
        }
    }

    private var rulePreview: String {
        guard let rule = try? draft.makeRule(id: editingRuleID) else {
            return "Complete the required fields to preview this rule."
        }
        return prettyWizardRuleJSON(rule)
    }

    private var sourceProviders: [String] {
        draft.sourceProvider.split(separator: ",").map {
            String($0).trimmingCharacters(in: .whitespacesAndNewlines)
        }.filter { !$0.isEmpty }
    }

    private var stepTitles: [String] { ["Name", "When", "Action", "Review"] }

    private func stepHeading(_ title: String, _ subtitle: String) -> some View {
        VStack(alignment: .leading, spacing: 3) {
            Text(title).font(.system(size: 16, weight: .semibold))
            Text(subtitle).font(.system(size: 11)).foregroundStyle(AlexTheme.Colors.textTertiary)
        }
    }

    private func wizardGroup<Content: View>(
        _ title: String,
        @ViewBuilder content: () -> Content
    ) -> some View {
        VStack(alignment: .leading, spacing: 7) {
            Text(title).font(.system(size: 11, weight: .semibold))
            content()
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private func chipWrap(
        values: [String],
        selected: [String],
        action: @escaping (String) -> Void
    ) -> some View {
        HStack(spacing: 6) {
            chip("Any", selected: selected.isEmpty) {
                selected.forEach(action)
            }
            ForEach(values, id: \.self) { value in
                chip(value.capitalized, selected: selected.contains(value)) { action(value) }
            }
        }
    }

    private func chipWrap<Value: Hashable>(
        values: [Value],
        title: KeyPath<Value, String>,
        selected: [Value],
        action: @escaping (Value) -> Void
    ) -> some View {
        HStack(spacing: 6) {
            ForEach(values, id: \.self) { value in
                chip(value[keyPath: title], selected: selected.contains(value)) { action(value) }
            }
        }
    }

    private func chip(_ title: String, selected: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(title)
                .font(.system(size: 10, weight: .medium))
                .foregroundStyle(selected ? .white : AlexTheme.Colors.textSecondary)
                .padding(.horizontal, 9)
                .padding(.vertical, 5)
                .background(Capsule().fill(selected
                    ? AlexTheme.Colors.primary : AlexTheme.Colors.overlay(0.06)))
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(selected ? .isSelected : [])
    }

    private func messageList(
        title: String,
        messages: [String],
        color: Color,
        icon: String
    ) -> some View {
        HStack(alignment: .top, spacing: 8) {
            Image(systemName: icon).foregroundStyle(color)
            VStack(alignment: .leading, spacing: 3) {
                Text(title).font(.system(size: 11, weight: .semibold))
                ForEach(messages, id: \.self) { Text("• \($0)").font(.system(size: 10)) }
            }
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 7).fill(color.opacity(0.07)))
    }

    private func toggleHarness(_ value: String) {
        if let index = draft.harnesses.firstIndex(of: value) {
            draft.harnesses.remove(at: index)
        } else {
            draft.harnesses.append(value)
        }
    }

    private func toggleSourceProvider(_ value: String) {
        var selected = sourceProviders
        if let index = selected.firstIndex(of: value) {
            selected.remove(at: index)
        } else {
            selected.append(value)
        }
        draft.sourceProvider = selected.joined(separator: ", ")
    }

    private func toggleTargetProvider(_ value: String) {
        if let index = draft.targetProviders.firstIndex(of: value) {
            draft.targetProviders.remove(at: index)
        } else {
            draft.targetProviders.append(value)
        }
    }

    private func toggleErrorKind(_ value: MiddlewareWizardErrorKind) {
        if value == .any {
            draft.errorKinds = draft.errorKinds == [.any] ? [] : [.any]
            return
        }
        draft.errorKinds.remove(.any)
        if draft.errorKinds.contains(value) {
            draft.errorKinds.remove(value)
        } else {
            draft.errorKinds.insert(value)
        }
    }

    private func client() -> AlexandriaClient? {
        guard let config = store.config ?? DaemonDiscovery.load() else { return nil }
        return AlexandriaClient(config: config)
    }

    private func validate() async {
        validation = nil
        validationError = nil
        guard let rule = try? draft.makeRule(id: editingRuleID) else {
            validationError = draft.localValidationErrors.joined(separator: " ")
            return
        }
        guard let client = client() else {
            validationError = "No Alex daemon configuration was found."
            return
        }
        isValidating = true
        defer { isValidating = false }
        do {
            validation = try await client.validateMiddlewareRule(rule)
        } catch is CancellationError {
        } catch {
            validationError = error.localizedDescription
        }
    }

    private func save() async {
        guard validation?.valid == true else { return }
        guard let builtRule = try? draft.makeRule(id: editingRuleID), let client = client() else {
            saveError = "The rule could not be built or the daemon is unavailable."
            return
        }
        let rule = validation?.canonicalRule ?? builtRule
        isSaving = true
        saveError = nil
        defer { isSaving = false }
        do {
            if editingRuleID == nil {
                _ = try await client.createMiddlewareRule(rule)
            } else {
                _ = try await client.updateMiddlewareRule(rule)
            }
            onSaved()
        } catch is CancellationError {
        } catch {
            saveError = error.localizedDescription
        }
    }
}

private func prettyWizardRuleJSON(_ rule: MiddlewareRuleSpecV1) -> String {
    let encoder = JSONEncoder()
    encoder.outputFormatting = [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes]
    guard let data = try? encoder.encode(rule) else { return "Unable to encode rule." }
    return String(data: data, encoding: .utf8) ?? "Unable to encode rule."
}
