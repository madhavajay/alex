import SwiftUI
import AlexCore

/// A four-step declarative-rule builder. It intentionally exposes the common
/// email-filter-shaped subset and always asks the daemon to validate the final
/// RuleSpec before Save becomes available.
struct MiddlewareWizard: View {
    let store: SnapshotStore
    @Binding var draft: MiddlewareWizardDraft
    @Binding var editingRuleID: String?
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
    private let efforts = ["low", "medium", "high", "xhigh", "max"]

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
        .onChange(of: draft.includeNotice) { _, includeNotice in
            if includeNotice,
               draft.notice.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            {
                draft.notice = MiddlewareWizardDraft.defaultNoticeTemplate
            }
        }
    }

    private var header: some View {
        HStack(spacing: 10) {
            Image(systemName: "wand.and.stars")
                .font(.system(size: 20, weight: .medium))
                .foregroundStyle(AlexTheme.Colors.primary)
            VStack(alignment: .leading, spacing: 2) {
                Text("Middleware Wizard")
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
                TextField("Describe this fallback", text: $draft.name)
                    .textFieldStyle(.roundedBorder)
                    .accessibilityLabel("Middleware name")
            }
            VStack(alignment: .leading, spacing: 6) {
                Text("Description (optional)").font(.system(size: 11, weight: .semibold))
                TextField("What this rule is for", text: $draft.description)
                    .textFieldStyle(.roundedBorder)
            }
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

            wizardGroup("Current provider") {
                providerPicker(selected: sourceProviders, includesAny: true) { provider in
                    draft.sourceProvider = provider ?? ""
                }
            }

            wizardGroup("Requested model") {
                TextField("Enter a model, e.g. claude-fable-5", text: $draft.modelPattern)
                    .textFieldStyle(.roundedBorder)
                Text("An ID without * is an exact match; use * only when you want a wildcard.")
                    .font(.system(size: 10))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
            }

            wizardGroup("Effort / thinking (optional)") {
                effortPicker(
                    selection: $draft.sourceEffort,
                    emptyLabel: "Any incoming effort")
                Text("When selected, the rule runs only when the request declares this effort level.")
                    .font(.system(size: 10))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
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
                wizardGroup("Provider choice") {
                    Picker("Provider choice", selection: $draft.providerMode) {
                        Text("Any available").tag(MiddlewareProviderMode.any)
                        Text("Prefer selected").tag(MiddlewareProviderMode.prefer)
                        Text("Only selected").tag(MiddlewareProviderMode.only)
                    }
                    .pickerStyle(.segmented)
                    .labelsHidden()
                    if draft.providerMode != .any {
                        providerPicker(selected: draft.targetProviders, includesAny: false) { provider in
                            draft.targetProviders = provider.map { [$0] } ?? []
                        }
                    }
                }

                wizardGroup(draft.action == .routeExact ? "Target model" : "Equivalence class") {
                    if draft.action == .routeExact {
                        TextField("Enter a target model, e.g. gpt-5.6-sol", text: $draft.targetModel)
                            .textFieldStyle(.roundedBorder)
                    } else {
                        TextField("claude-fable-5", text: $draft.equivalenceClass)
                            .textFieldStyle(.roundedBorder)
                        Text("Enter a configured equivalence-class key.")
                            .font(.system(size: 10))
                            .foregroundStyle(AlexTheme.Colors.textTertiary)
                    }
                }

                wizardGroup("Replacement effort / thinking (optional)") {
                    effortPicker(
                        selection: $draft.targetEffort,
                        emptyLabel: "Keep the incoming effort")
                    Text("When selected, Alex applies this effort level to the replacement request.")
                        .font(.system(size: 10))
                        .foregroundStyle(AlexTheme.Colors.textTertiary)
                }

                wizardGroup("Apply") {
                    Picker("Apply", selection: $draft.scope) {
                        Text("This request only").tag(MiddlewareRouteScope.request)
                        Text("Keep for this session").tag(MiddlewareRouteScope.session)
                    }
                    .pickerStyle(.segmented)
                    .labelsHidden()
                    Text(draft.scope == .request
                        ? "Only the failed request is retried on the replacement model. The next turn starts on the requested model again."
                        : "After a successful fallback, later requests with the same stable session go directly to the replacement model until the route lease expires.")
                        .font(.system(size: 10))
                        .foregroundStyle(AlexTheme.Colors.textTertiary)
                        .fixedSize(horizontal: false, vertical: true)
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
                        TextField(MiddlewareWizardDraft.defaultNoticeTemplate, text: $draft.notice)
                            .textFieldStyle(.roundedBorder)
                        Text("Use {from_model} and {to_model} to include the actual model names.")
                            .font(.system(size: 10))
                            .foregroundStyle(AlexTheme.Colors.textTertiary)
                        Text("A notice can buffer the exceptional fallback response. Normal successful streaming stays untouched.")
                            .font(.system(size: 10))
                            .foregroundStyle(AlexTheme.Colors.warningOrange)
                    }
                }
            }

            wizardGroup("Priority") {
                HStack {
                    Stepper(value: $draft.priority, in: 0...10_000) {
                        Text("\(draft.priority)").font(AlexTheme.Fonts.metaMono)
                    }
                    .frame(width: 120)
                    Text("Higher-priority rules run first.")
                        .font(.system(size: 10))
                        .foregroundStyle(AlexTheme.Colors.textTertiary)
                    Spacer()
                }
            }
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
            if let continueRequirement {
                Text(continueRequirement)
                    .font(.system(size: 10))
                    .foregroundStyle(AlexTheme.Colors.warningOrange)
            }
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

    private var continueRequirement: String? {
        guard !canContinue else { return nil }
        switch step {
        case 0: return "Enter a middleware name."
        case 1: return "Choose at least one condition."
        case 2:
            switch draft.action {
            case .retrySame: return nil
            case .routeExact: return "Enter a target model."
            case .routeEquivalent: return "Enter an equivalence class."
            }
        default: return nil
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

    private func effortPicker(
        selection: Binding<String>,
        emptyLabel: String
    ) -> some View {
        Picker("Effort / thinking", selection: selection) {
            Text(emptyLabel).tag("")
            ForEach(efforts, id: \.self) { effort in
                Text(effort == "xhigh" ? "Extra high" : effort.capitalized).tag(effort)
            }
        }
        .labelsHidden()
        .frame(maxWidth: 260, alignment: .leading)
    }

    private func providerPicker(
        selected: [String],
        includesAny: Bool,
        action: @escaping (String?) -> Void
    ) -> some View {
        LazyVGrid(
            columns: [GridItem(.adaptive(minimum: 145), spacing: 8)],
            spacing: 8
        ) {
            if includesAny {
                providerChoice(
                    title: "Any provider",
                    subtitle: selected.isEmpty ? "Selected" : "No filter",
                    icon: nil,
                    selected: selected.isEmpty
                ) { action(nil) }
            }
            ForEach(providers, id: \.self) { provider in
                let isSelected = selected.contains(provider)
                providerChoice(
                    title: ProviderInfo.displayName(provider),
                    subtitle: isSelected ? "Selected" : "Choose",
                    icon: ProviderInfo.loginArg(provider),
                    selected: isSelected
                ) { action(provider) }
            }
        }
    }

    private func providerChoice(
        title: String,
        subtitle: String,
        icon: String?,
        selected: Bool,
        action: @escaping () -> Void
    ) -> some View {
        Button(action: action) {
            HStack(spacing: 8) {
                if let icon {
                    HarnessIconView(harness: icon, tags: nil, size: 22, showsFallback: true)
                } else {
                    Image(systemName: "network")
                        .font(.system(size: 17, weight: .medium))
                        .foregroundStyle(AlexTheme.Colors.primary)
                        .frame(width: 22, height: 22)
                }
                VStack(alignment: .leading, spacing: 1) {
                    Text(title)
                        .font(.system(size: 11, weight: .semibold))
                        .lineLimit(1)
                    Text(subtitle)
                        .font(AlexTheme.Fonts.metaMicro)
                        .foregroundStyle(AlexTheme.Colors.textTertiary)
                }
                Spacer(minLength: 2)
                if selected {
                    Image(systemName: "checkmark.circle.fill")
                        .font(.system(size: 12))
                        .foregroundStyle(AlexTheme.Colors.primary)
                }
            }
            .padding(9)
            .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
            .background(RoundedRectangle(cornerRadius: AlexTheme.Radius.md)
                .fill(selected ? AlexTheme.Colors.primary.opacity(0.10) : AlexTheme.Colors.card))
            .overlay(RoundedRectangle(cornerRadius: AlexTheme.Radius.md)
                .strokeBorder(selected
                    ? AlexTheme.Colors.primary.opacity(0.45) : AlexTheme.Colors.cardBorder))
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(selected ? .isSelected : [])
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

    private func client() -> AlexClient? {
        guard let config = store.config ?? DaemonDiscovery.load() else { return nil }
        return AlexClient(config: config)
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
        var rule = validation?.canonicalRule ?? builtRule
        if let editingRuleID {
            // The sheet's edit identity is authoritative. Validation may
            // canonicalize fields, but saving an edit must never create a new
            // slugged rule beside the original.
            rule.id = editingRuleID
        }
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
