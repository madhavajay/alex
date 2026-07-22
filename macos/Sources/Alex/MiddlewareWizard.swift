import SwiftUI
import AlexCore

/// A four-step declarative-rule builder. Every matcher is a regular
/// expression, and the Code tab exposes the exact RuleSpecV1 JSON so it can
/// be edited directly or pasted into another tool. The daemon validates the
/// final RuleSpec before Save becomes available.
struct MiddlewareWizard: View {
    let store: SnapshotStore
    @Binding var draft: MiddlewareWizardDraft
    @Binding var editingRuleID: String?
    let onSaved: () -> Void
    let onOpenTraceBrowser: (String) -> Void
    let onClose: () -> Void

    @State private var step = 0
    @State private var showingCode = false
    @State private var codeText = ""
    @State private var codeRule: MiddlewareRuleSpecV1?
    @State private var codeError: String?
    @State private var validation: MiddlewareValidationResponse?
    @State private var validationError: String?
    @State private var isValidating = false
    @State private var isSaving = false
    @State private var saveError: String?
    @State private var traceMatches: MiddlewareTraceMatchResponse?
    @State private var traceMatchError: String?
    @State private var isLoadingTraceMatches = false
    @State private var traceMatchEndpointAvailable = true
    @State private var traceMatchTask: Task<Void, Never>?
    @State private var expandedTraceIDs: Set<String> = []
    @State private var selectedCandidateID: String?
    @State private var matchLabSort = MatchLabSort.newest

    private let harnesses = ["claude", "codex", "pi", "amp", "gemini", "opencode"]
    private let providers = ProviderInfo.supportedProviders
    private let efforts = ["low", "medium", "high", "xhigh", "max"]

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider()
            HStack(spacing: 0) {
                matchLabSidebar
                    .frame(width: 360)
                Divider()
                if showingCode {
                    codeView
                } else {
                    VStack(spacing: 0) {
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
                    }
                }
            }
            Divider()
            footer
        }
        .frame(
            minWidth: 900, idealWidth: 1_100, maxWidth: .infinity,
            minHeight: 560, idealHeight: 680, maxHeight: .infinity)
        .onChange(of: draft) { _, _ in
            invalidateValidation()
            scheduleTraceMatch()
        }
        .onChange(of: codeText) { _, _ in
            guard showingCode else { return }
            invalidateValidation()
            parseCode()
            scheduleTraceMatch()
        }
        .onExitCommand { onClose() }
        .onChange(of: draft.includeNotice) { _, includeNotice in
            if includeNotice,
               draft.notice.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            {
                draft.notice = MiddlewareWizardDraft.defaultNoticeTemplate
            }
        }
        .onAppear { scheduleTraceMatch() }
        .onDisappear { traceMatchTask?.cancel() }
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
                    : "Edit a declarative rule")
                    .font(.system(size: 11))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
            }
            Spacer()
            Picker("View", selection: viewModeBinding) {
                Text("Wizard").tag(false)
                Text("Code").tag(true)
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .frame(width: 150)
            .accessibilityLabel("Switch between wizard and code view")
            Button { onClose() } label: { Image(systemName: "xmark") }
                .buttonStyle(.plain)
                .keyboardShortcut(.cancelAction)
                .accessibilityLabel("Close wizard")
        }
        .padding(.horizontal, 20)
        .frame(height: 56)
    }

    /// Switching to Code renders the draft as JSON; switching back projects a
    /// valid edited rule into the draft, and refuses to leave when the code
    /// no longer parses so edits are never silently dropped.
    private var viewModeBinding: Binding<Bool> {
        Binding(
            get: { showingCode },
            set: { wantsCode in
                if wantsCode {
                    codeText = currentRuleJSON()
                    parseCode()
                    showingCode = true
                } else {
                    parseCode()
                    if let codeRule {
                        draft = MiddlewareWizardDraft(rule: codeRule)
                    } else if !codeText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                        return
                    }
                    showingCode = false
                }
            })
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
                "Every matcher is a regular expression. Leave a field empty to allow any value.")

            wizardGroup("Harness") {
                chipWrap(values: harnesses, selectedPattern: draft.harnessNameRegex) {
                    toggleRegexChip(&draft.harnessNameRegex, value: $0)
                }
                regexField(
                    "Harness name regex, e.g. ^(claude|pi)$",
                    text: $draft.harnessNameRegex)
                regexField(
                    "Harness version regex (optional), e.g. ^2\\.1\\.",
                    text: $draft.harnessVersionRegex)
            }

            wizardGroup("Model") {
                regexField("Model regex, e.g. ^claude-fable-5$", text: $draft.modelRegex)
                Text("Matches the originally requested or currently selected model ID.")
                    .font(.system(size: 10))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
            }

            wizardGroup("Provider") {
                chipWrap(
                    values: providers,
                    titles: providers.map(ProviderInfo.displayName),
                    selectedPattern: draft.providerRegex
                ) { toggleRegexChip(&draft.providerRegex, value: $0) }
                regexField("Provider regex, e.g. ^(anthropic|openai)$", text: $draft.providerRegex)
            }

            wizardGroup("Effort / thinking (optional)") {
                effortPicker(
                    selection: $draft.sourceEffort,
                    emptyLabel: "Any incoming effort")
            }

            wizardGroup("Response") {
                regexField("HTTP status regex, e.g. ^200$ or ^(429|5\\d\\d)$", text: $draft.statusRegex)
                VStack(alignment: .leading, spacing: 4) {
                    Text("Response headers (one key-regex => value-regex per line)")
                        .font(.system(size: 10, weight: .medium))
                    TextEditor(text: $draft.responseHeaderRegexText)
                        .font(.system(size: 11, design: .monospaced))
                        .frame(height: 48)
                        .padding(4)
                        .background(RoundedRectangle(cornerRadius: 5)
                            .stroke(AlexTheme.Colors.borderStrong))
                }
                VStack(alignment: .leading, spacing: 4) {
                    Text("Body regex")
                        .font(.system(size: 10, weight: .medium))
                    TextEditor(text: $draft.bodyRegex)
                        .font(.system(size: 11, design: .monospaced))
                        .frame(height: 62)
                        .padding(4)
                        .background(RoundedRectangle(cornerRadius: 5)
                            .stroke(AlexTheme.Colors.borderStrong))
                    if !draft.bodyRegex.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                        Label("Only eligible failed responses are inspected, up to the configured byte cap.",
                              systemImage: "gauge.with.dots.needle.33percent")
                            .font(.system(size: 10))
                            .foregroundStyle(AlexTheme.Colors.warningOrange)
                    }
                }
            }

            if !regexProblems.isEmpty {
                messageList(
                    title: "Regex problems",
                    messages: regexProblems,
                    color: AlexTheme.Colors.destructive,
                    icon: "xmark.octagon.fill")
            }
        }
    }

    private var actionStep: some View {
        VStack(alignment: .leading, spacing: 16) {
            stepHeading(
                "What should Alex do?",
                "A match reroutes to the target model and keeps the route for the session.")

            wizardGroup("Target model") {
                TextField("Enter a target model, e.g. gpt-5.6-sol", text: $draft.targetModel)
                    .textFieldStyle(.roundedBorder)
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
                    providerPicker(selected: draft.targetProviders) { provider in
                        toggleTargetProvider(provider)
                    }
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

            wizardGroup("Session route") {
                Text("After a successful fallback, later requests with the same stable session go directly to the replacement model until the route lease expires.")
                    .font(.system(size: 10))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
                    .fixedSize(horizontal: false, vertical: true)
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

            wizardGroup("Model-switch notice") {
                Toggle("Tell the harness after the fallback succeeds", isOn: $draft.includeNotice)
                    .toggleStyle(.checkbox)
                    .font(.system(size: 11))
                if draft.includeNotice {
                    TextField(MiddlewareWizardDraft.defaultNoticeTemplate, text: $draft.notice)
                        .textFieldStyle(.roundedBorder)
                    Text("Templates: {from_model}, {to_model}, {from_provider}, {to_provider}.")
                        .font(.system(size: 10))
                        .foregroundStyle(AlexTheme.Colors.textTertiary)
                    Text("A notice can buffer the exceptional fallback response. Normal successful streaming stays untouched.")
                        .font(.system(size: 10))
                        .foregroundStyle(AlexTheme.Colors.warningOrange)
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
            validationResults

            VStack(alignment: .leading, spacing: 5) {
                HStack {
                    Text("Structured RuleSpecV1 preview")
                        .font(.system(size: 11, weight: .semibold))
                    Spacer()
                    Button {
                        copyToPasteboard(rulePreview)
                    } label: {
                        Label("Copy", systemImage: "doc.on.doc")
                            .font(.system(size: 10))
                    }
                    .buttonStyle(.bordered)
                    .controlSize(.small)
                }
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

    private var codeView: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                VStack(alignment: .leading, spacing: 2) {
                    Text("RuleSpecV1 JSON")
                        .font(.system(size: 11, weight: .semibold))
                    Text("Edit directly, or copy it into another LLM and paste the result back.")
                        .font(.system(size: 10))
                        .foregroundStyle(AlexTheme.Colors.textTertiary)
                }
                Spacer()
                Button {
                    copyToPasteboard(codeText)
                } label: {
                    Label("Copy", systemImage: "doc.on.doc").font(.system(size: 10))
                }
                .buttonStyle(.bordered)
                .controlSize(.small)
                Button {
                    if let pasted = NSPasteboard.general.string(forType: .string) {
                        codeText = JSONTextFormatting.prettyPrinted(pasted) ?? pasted
                    }
                } label: {
                    Label("Paste", systemImage: "doc.on.clipboard").font(.system(size: 10))
                }
                .buttonStyle(.bordered)
                .controlSize(.small)
                Button {
                    formatCode()
                } label: {
                    Label("Format", systemImage: "text.alignleft").font(.system(size: 10))
                }
                .buttonStyle(.bordered)
                .controlSize(.small)
                .disabled(codeRule == nil)
                Button {
                    codeText = currentRuleJSON()
                } label: {
                    Label("Reset", systemImage: "arrow.counterclockwise").font(.system(size: 10))
                }
                .buttonStyle(.bordered)
                .controlSize(.small)
            }

            JSONCodeEditor(text: $codeText, onFormatRequest: formatCode)
                .padding(1)
                .background(RoundedRectangle(cornerRadius: 7)
                    .fill(AlexTheme.Colors.overlay(0.04)))
                .overlay(RoundedRectangle(cornerRadius: 7)
                    .stroke(codeError == nil ? AlexTheme.Colors.border : AlexTheme.Colors.destructive))
                .frame(maxHeight: .infinity)
                .accessibilityLabel("Rule JSON editor")

            if let codeError {
                messageList(
                    title: "This JSON cannot be saved",
                    messages: [codeError],
                    color: AlexTheme.Colors.destructive,
                    icon: "xmark.octagon.fill")
            } else {
                validationResults
            }

            if let saveError {
                Text(saveError)
                    .font(.system(size: 10))
                    .foregroundStyle(AlexTheme.Colors.destructive)
            }
        }
        .padding(16)
    }

    @ViewBuilder
    private var validationResults: some View {
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
    }

    @ViewBuilder
    private var matchLabSidebar: some View {
        VStack(spacing: 0) {
            HStack(spacing: 7) {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Live match lab")
                        .font(.system(size: 12, weight: .semibold))
                    Text("Recent attempts update as you edit")
                        .font(AlexTheme.Fonts.metaMicro)
                        .foregroundStyle(AlexTheme.Colors.textTertiary)
                }
                Spacer()
                if isLoadingTraceMatches {
                    ProgressView().controlSize(.mini)
                } else if let traceMatches {
                    Text("\(traceMatches.matchCount)/\(traceMatches.scanned)")
                        .font(AlexTheme.Fonts.metaMono)
                        .foregroundStyle(AlexTheme.Colors.primary)
                        .help("Matched attempts / recent attempts checked")
                }
                Menu {
                    Picker("Sort", selection: $matchLabSort) {
                        ForEach(MatchLabSort.allCases) { option in
                            Text(option.rawValue).tag(option)
                        }
                    }
                } label: {
                    Image(systemName: "arrow.up.arrow.down")
                        .font(.system(size: 10, weight: .medium))
                }
                .menuStyle(.borderlessButton)
                .menuIndicator(.hidden)
                .fixedSize()
                .help("Sort sessions: \(matchLabSort.rawValue)")
            }
            .padding(12)

            Divider()

            matchLabRuleFilterBar

            Divider()

            if let traceMatchError {
                matchLabMessage(traceMatchError, icon: "wifi.exclamationmark")
            } else if traceMatchRule() == nil {
                matchLabMessage(
                    showingCode ? (codeError ?? "Enter valid RuleSpecV1 JSON to test it.")
                        : "Fix invalid regular expressions to test recent attempts.",
                    icon: "xmark.octagon")
            } else if let candidates = traceMatches?.recentCandidates, !candidates.isEmpty {
                List {
                    ForEach(groupedSessionCandidates, id: \.sessionId) { session in
                        DisclosureGroup(
                            isExpanded: matchLabExpansionBinding("session:\(session.sessionId)"))
                        {
                            ForEach(Array(session.turns.enumerated()), id: \.element.traceId) { index, turn in
                                DisclosureGroup(
                                    isExpanded: matchLabExpansionBinding("turn:\(turn.traceId)"))
                                {
                                    ForEach(turn.attempts) { attempt in
                                        matchAttemptRow(attempt)
                                            .listRowInsets(EdgeInsets(
                                                top: 2, leading: 28, bottom: 2, trailing: 7))
                                    }
                                } label: {
                                    matchTurnHeader(turn, number: index + 1)
                                        .contextMenu { matchContextMenu(turn.attempts[0]) }
                                }
                                .listRowInsets(EdgeInsets(
                                    top: 2, leading: 14, bottom: 2, trailing: 7))
                            }
                        } label: {
                            matchSessionHeader(session)
                        }
                    }
                }
                .listStyle(.sidebar)
            } else if traceMatches != nil {
                matchLabMessage("No recent attempts are available.", icon: "clock.arrow.circlepath")
            } else {
                Spacer()
                ProgressView().controlSize(.small)
                Text("Loading recent attempts…")
                    .font(AlexTheme.Fonts.metaMicro)
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
                Spacer()
            }

            if let selectedCandidate {
                Divider()
                matchAttemptDetail(selectedCandidate)
                    .frame(maxHeight: 190)
            }
        }
        .background(AlexTheme.Colors.overlay(0.018))
    }

    private enum MatchLabRuleFilter: String, CaseIterable, Identifiable {
        case harness = "Harness"
        case model = "Model"
        case provider = "Provider"

        var id: Self { self }
    }

    private var matchLabRuleFilterBar: some View {
        HStack(spacing: 6) {
            ForEach(MatchLabRuleFilter.allCases) { filter in
                matchLabRuleFilterMenu(filter)
            }
        }
        .padding(.horizontal, 9)
        .padding(.vertical, 7)
    }

    private func matchLabRuleFilterMenu(_ filter: MatchLabRuleFilter) -> some View {
        let pattern = matchLabRuleFilterPattern(filter)
        let exactValue = patternSelections(pattern).count == 1
            ? patternSelections(pattern).first : nil
        return Menu {
            Button {
                setMatchLabRuleFilter(filter, value: nil)
            } label: {
                matchLabRuleFilterItem("Any", checked: pattern.isEmpty)
            }
            Divider()
            ForEach(matchLabRuleFilterValues(filter), id: \.self) { value in
                Button {
                    setMatchLabRuleFilter(filter, value: value)
                } label: {
                    matchLabRuleFilterItem(value, checked: exactValue == value)
                }
            }
            if !pattern.isEmpty, exactValue == nil {
                Divider()
                Text("Regex: \(pattern)")
            }
        } label: {
            HStack(spacing: 3) {
                Text("\(filter.rawValue): \(matchLabRuleFilterLabel(pattern))")
                    .lineLimit(1)
                    .truncationMode(.middle)
                Image(systemName: "chevron.down")
                    .font(.system(size: 6, weight: .semibold))
            }
            .font(.system(size: 9, weight: pattern.isEmpty ? .regular : .semibold))
            .foregroundStyle(pattern.isEmpty
                ? AnyShapeStyle(AlexTheme.Colors.textSecondary)
                : AnyShapeStyle(AlexTheme.Colors.primary))
            .padding(.horizontal, 7)
            .padding(.vertical, 4)
            .background(Capsule().fill(pattern.isEmpty
                ? AlexTheme.Colors.overlay(0.05)
                : AlexTheme.Colors.primary.opacity(0.13)))
            .frame(maxWidth: 110)
        }
        .menuStyle(.button)
        .buttonStyle(.plain)
        .menuIndicator(.hidden)
        .disabled(showingCode)
        .help(showingCode
            ? "Switch to Wizard view to change this rule filter"
            : "Filter recent attempts by \(filter.rawValue.lowercased())")
    }

    @ViewBuilder
    private func matchLabRuleFilterItem(_ text: String, checked: Bool) -> some View {
        if checked {
            Label(text, systemImage: "checkmark")
        } else {
            Text(text)
        }
    }

    private func matchLabRuleFilterPattern(_ filter: MatchLabRuleFilter) -> String {
        if showingCode, let rule = codeRule {
            switch filter {
            case .harness:
                return rule.when.harnessNameRegex?.first
                    ?? MiddlewareWizardDraft.exactAlternation(rule.when.harnessNames ?? [])
            case .model:
                return rule.when.modelRegex?.first
                    ?? MiddlewareWizardDraft.exactAlternation(rule.when.models ?? [])
            case .provider:
                return rule.when.providerRegex?.first
                    ?? MiddlewareWizardDraft.exactAlternation(rule.when.providers ?? [])
            }
        }
        switch filter {
        case .harness: return draft.harnessNameRegex
        case .model: return draft.modelRegex
        case .provider: return draft.providerRegex
        }
    }

    private func matchLabRuleFilterValues(_ filter: MatchLabRuleFilter) -> [String] {
        let candidates = traceMatches?.recentCandidates ?? []
        let values: [String]
        switch filter {
        case .harness:
            values = harnesses + candidates.compactMap(\.harnessName)
        case .model:
            values = candidates.map(\.model)
        case .provider:
            values = providers + candidates.map(\.provider)
        }
        return Array(Set(values.filter { !$0.isEmpty })).sorted {
            $0.localizedCaseInsensitiveCompare($1) == .orderedAscending
        }
    }

    private func setMatchLabRuleFilter(_ filter: MatchLabRuleFilter, value: String?) {
        guard !showingCode else { return }
        let pattern = value.map { "^\(NSRegularExpression.escapedPattern(for: $0))$" } ?? ""
        switch filter {
        case .harness: draft.harnessNameRegex = pattern
        case .model: draft.modelRegex = pattern
        case .provider: draft.providerRegex = pattern
        }
    }

    private func matchLabRuleFilterLabel(_ pattern: String) -> String {
        guard !pattern.isEmpty else { return "Any" }
        let selections = patternSelections(pattern)
        return selections.count == 1 ? selections.first! : pattern
    }

    private enum MatchLabSort: String, CaseIterable, Identifiable {
        case newest = "Newest"
        case matches = "Most matches"
        case harness = "Harness"
        case model = "Model"
        case turns = "Turn count"

        var id: Self { self }
    }

    private struct SessionCandidateGroup {
        let sessionId: String
        let latestTimestampMs: Int64
        let turns: [TraceCandidateGroup]
    }

    private struct TraceCandidateGroup {
        let traceId: String
        let timestampMs: Int64
        let attempts: [MiddlewareTraceMatch]
    }

    private var groupedSessionCandidates: [SessionCandidateGroup] {
        let candidates = traceMatches?.recentCandidates ?? []
        let attemptsByTrace = Dictionary(grouping: candidates, by: \.traceId)
        let turns = attemptsByTrace.compactMap { traceId, attempts -> TraceCandidateGroup? in
            guard let first = attempts.first else { return nil }
            return TraceCandidateGroup(
                traceId: traceId,
                timestampMs: first.timestampMs,
                attempts: attempts.sorted { ($0.attemptNumber ?? 0) < ($1.attemptNumber ?? 0) })
        }
        let turnsBySession = Dictionary(grouping: turns) { turn in
            turn.attempts[0].sessionId ?? "trace:\(turn.traceId)"
        }
        let sessions = turnsBySession.map { sessionId, turns in
            SessionCandidateGroup(
                sessionId: sessionId,
                latestTimestampMs: turns.map(\.timestampMs).max() ?? 0,
                turns: turns.sorted { $0.timestampMs < $1.timestampMs })
        }
        return sessions.sorted(by: matchLabSessionPrecedes)
    }

    private func matchLabSessionPrecedes(
        _ lhs: SessionCandidateGroup, _ rhs: SessionCandidateGroup
    ) -> Bool {
        switch matchLabSort {
        case .newest:
            return lhs.latestTimestampMs > rhs.latestTimestampMs
        case .matches:
            let left = lhs.turns.flatMap(\.attempts).filter { $0.matched == true }.count
            let right = rhs.turns.flatMap(\.attempts).filter { $0.matched == true }.count
            return left == right ? lhs.latestTimestampMs > rhs.latestTimestampMs : left > right
        case .harness:
            let left = lhs.turns.first?.attempts.first?.harnessName ?? ""
            let right = rhs.turns.first?.attempts.first?.harnessName ?? ""
            return left.localizedCaseInsensitiveCompare(right) == .orderedAscending
        case .model:
            let left = lhs.turns.first?.attempts.first?.model ?? ""
            let right = rhs.turns.first?.attempts.first?.model ?? ""
            return left.localizedCaseInsensitiveCompare(right) == .orderedAscending
        case .turns:
            return lhs.turns.count == rhs.turns.count
                ? lhs.latestTimestampMs > rhs.latestTimestampMs
                : lhs.turns.count > rhs.turns.count
        }
    }

    private var selectedCandidate: MiddlewareTraceMatch? {
        guard let selectedCandidateID else { return nil }
        return traceMatches?.recentCandidates.first { $0.id == selectedCandidateID }
    }

    private func matchLabExpansionBinding(_ key: String) -> Binding<Bool> {
        Binding(
            get: { expandedTraceIDs.contains(key) },
            set: { expanded in
                if expanded { expandedTraceIDs.insert(key) }
                else { expandedTraceIDs.remove(key) }
            })
    }

    private func matchSessionHeader(_ session: SessionCandidateGroup) -> some View {
        let attempts = session.turns.flatMap(\.attempts)
        let matchCount = attempts.filter { $0.matched == true }.count
        let first = attempts[0]
        return HStack(spacing: 6) {
            StatusDot(status: matchCount > 0 ? .success : .pending, size: 6)
            HarnessIconView(
                harness: first.harnessName, tags: nil, size: 17, showsFallback: true)
            ProviderBadgeView(provider: first.provider, size: 17, style: .tinted)
            Text(shortMatchLabID(session.sessionId))
                .font(AlexTheme.Fonts.mono(10.5))
                .foregroundStyle(AlexTheme.Colors.textSecondary)
                .lineLimit(1)
                .help(session.sessionId)
            ModelBadge(model: first.model)
            Spacer(minLength: 2)
            Text("\(session.turns.count)T")
                .font(AlexTheme.Fonts.mono(9.5))
                .foregroundStyle(AlexTheme.Colors.textTertiary)
            if matchCount > 0 {
                Text("\(matchCount)✓")
                    .font(AlexTheme.Fonts.mono(9.5, weight: .semibold))
                    .foregroundStyle(AlexTheme.Colors.success)
            }
        }
        .frame(height: AlexTheme.Metrics.listRowHeight)
    }

    private func matchTurnHeader(_ turn: TraceCandidateGroup, number: Int) -> some View {
        let matchCount = turn.attempts.filter { $0.matched == true }.count
        let first = turn.attempts[0]
        return HStack(spacing: 6) {
            StatusDot(status: matchCount > 0 ? .success : .pending, size: 6)
            Text("T\(number)")
                .font(AlexTheme.Fonts.mono(9.5, weight: .medium))
                .foregroundStyle(AlexTheme.Colors.textSecondary)
            ProviderBadgeView(provider: first.provider, size: 14, style: .tinted)
            ModelBadge(model: first.model)
            Spacer(minLength: 2)
            Text(traceMatchTime(turn.timestampMs))
                .font(AlexTheme.Fonts.metaMicro)
                .foregroundStyle(AlexTheme.Colors.textFaint)
            if turn.attempts.count > 1 {
                Text("\(turn.attempts.count)A")
                    .font(AlexTheme.Fonts.mono(9))
                    .foregroundStyle(AlexTheme.Colors.textFaint)
            }
        }
        .frame(height: 25)
    }

    private func shortMatchLabID(_ id: String) -> String {
        guard id.count > 13 else { return id }
        return "\(id.prefix(6))…\(id.suffix(5))"
    }

    private func matchAttemptRow(_ attempt: MiddlewareTraceMatch) -> some View {
        Button {
            selectedCandidateID = attempt.id
        } label: {
            HStack(spacing: 6) {
                Image(systemName: attempt.matched == true ? "checkmark.circle.fill" : "circle")
                    .foregroundStyle(attempt.matched == true
                        ? AlexTheme.Colors.success : AlexTheme.Colors.textFaint)
                Text("#\(attempt.attemptNumber ?? 1)")
                    .foregroundStyle(AlexTheme.Colors.textFaint)
                VStack(alignment: .leading, spacing: 1) {
                    Text(attempt.model).lineLimit(1)
                    Text(ProviderInfo.displayName(attempt.provider))
                        .foregroundStyle(AlexTheme.Colors.textTertiary)
                }
                Spacer()
                Text("HTTP \(attempt.status)")
                    .foregroundStyle(attempt.status >= 400
                        ? AlexTheme.Colors.warningOrange : AlexTheme.Colors.textSecondary)
            }
            .font(AlexTheme.Fonts.metaMicro)
            .padding(.vertical, 4)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .contextMenu { matchContextMenu(attempt) }
    }

    @ViewBuilder
    private func matchContextMenu(_ attempt: MiddlewareTraceMatch) -> some View {
        Button("Match harness ‘\(attempt.harnessName ?? "unknown")’") {
            if let harness = attempt.harnessName { draft.harnessNameRegex = exactRegex(harness) }
        }
        .disabled(attempt.harnessName == nil || showingCode)
        Button("Match model ‘\(attempt.model)’") { draft.modelRegex = exactRegex(attempt.model) }
            .disabled(showingCode)
        Button("Match provider ‘\(attempt.provider)’") { draft.providerRegex = exactRegex(attempt.provider) }
            .disabled(showingCode)
        Button("Match HTTP \(attempt.status)") { draft.statusRegex = exactRegex("\(attempt.status)") }
            .disabled(showingCode)
        Divider()
        Button("Open full trace") { onOpenTraceBrowser(attempt.traceId) }
    }

    private func matchAttemptDetail(_ attempt: MiddlewareTraceMatch) -> some View {
        VStack(alignment: .leading, spacing: 5) {
            HStack {
                Text("Response detail")
                    .font(.system(size: 10, weight: .semibold))
                Spacer()
                Button("Open full response") { onOpenTraceBrowser(attempt.traceId) }
                    .buttonStyle(.link)
                    .font(AlexTheme.Fonts.metaMicro)
            }
            if let headers = attempt.responseHeaders, !headers.isEmpty {
                Text(headers.sorted { $0.key < $1.key }
                    .map { "\($0.key): \($0.value.joined(separator: ", "))" }
                    .joined(separator: "\n"))
                    .font(.system(size: 9, design: .monospaced))
                    .lineLimit(3)
                    .textSelection(.enabled)
            }
            if let body = attempt.bodyPreview, !body.isEmpty {
                ScrollView([.vertical, .horizontal]) {
                    Text(body)
                        .font(.system(size: 9, design: .monospaced))
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                if attempt.bodyTruncated == true {
                    Text("Preview truncated — open the trace for the full body.")
                        .font(AlexTheme.Fonts.metaMicro)
                        .foregroundStyle(AlexTheme.Colors.warningOrange)
                }
            } else {
                Text("No response body was captured for this attempt.")
                    .font(AlexTheme.Fonts.metaMicro)
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
            }
        }
        .padding(10)
    }

    private func matchLabMessage(_ message: String, icon: String) -> some View {
        VStack(spacing: 8) {
            Spacer()
            Image(systemName: icon).foregroundStyle(AlexTheme.Colors.textFaint)
            Text(message)
                .font(.system(size: 10))
                .foregroundStyle(AlexTheme.Colors.textTertiary)
                .multilineTextAlignment(.center)
            Spacer()
        }
        .padding(18)
    }

    private func exactRegex(_ value: String) -> String {
        "^\(NSRegularExpression.escapedPattern(for: value))$"
    }

    private var footer: some View {
        HStack(spacing: 8) {
            Button("Cancel") { onClose() }
                .buttonStyle(.bordered)
                .keyboardShortcut(.cancelAction)
            Spacer()
            if showingCode {
                Button(isValidating ? "Validating…" : "Validate with daemon") {
                    Task { await validate() }
                }
                .buttonStyle(.bordered)
                .disabled(isValidating || isSaving || codeRule == nil)
                Button(isSaving ? "Saving…" : (editingRuleID == nil ? "Save Middleware" : "Save Changes")) {
                    Task { await save() }
                }
                .buttonStyle(.borderedProminent)
                .keyboardShortcut(.defaultAction)
                .disabled(isSaving || isValidating || codeRule == nil || validation?.valid != true)
            } else {
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
        }
        .padding(.horizontal, 20)
        .frame(height: 56)
    }

    private var canContinue: Bool {
        switch step {
        case 0: !draft.name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        case 1: regexProblems.isEmpty
        case 2: !draft.targetModel.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        default: true
        }
    }

    private var continueRequirement: String? {
        guard !canContinue else { return nil }
        switch step {
        case 0: return "Enter a middleware name."
        case 1: return "Fix the invalid regex."
        case 2: return "Enter a target model."
        default: return nil
        }
    }

    private var regexProblems: [String] {
        draft.localValidationErrors.filter { $0.contains("regex") || $0.contains("Header matcher") }
    }

    private var rulePreview: String {
        guard let rule = try? draft.makeRule(id: editingRuleID) else {
            return "Complete the required fields to preview this rule."
        }
        return prettyWizardRuleJSON(rule)
    }

    private var stepTitles: [String] { ["Name", "When", "Action", "Review"] }

    private func currentRuleJSON() -> String {
        guard let rule = try? draft.makeRule(id: editingRuleID) else {
            // Incomplete drafts still round-trip: build a permissive copy so
            // the JSON always reflects what the wizard currently holds.
            var permissive = draft
            if permissive.name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                permissive.name = "untitled-rule"
            }
            if permissive.targetModel.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                permissive.targetModel = "gpt-5.6-sol"
            }
            if permissive.providerMode != .any && permissive.targetProviders.isEmpty {
                permissive.providerMode = .any
            }
            guard let rule = try? permissive.makeRule(id: editingRuleID) else { return "{}" }
            return prettyWizardRuleJSON(rule)
        }
        return prettyWizardRuleJSON(rule)
    }

    private func formatCode() {
        guard let formatted = JSONTextFormatting.prettyPrinted(codeText), formatted != codeText else {
            return
        }
        codeText = formatted
    }

    private func parseCode() {
        let text = codeText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else {
            codeRule = nil
            codeError = "Paste or write a RuleSpecV1 JSON object."
            return
        }
        do {
            let rule = try JSONDecoder().decode(MiddlewareRuleSpecV1.self, from: Data(text.utf8))
            codeRule = rule
            codeError = nil
        } catch let error as DecodingError {
            codeRule = nil
            codeError = decodingErrorText(error)
        } catch {
            codeRule = nil
            codeError = error.localizedDescription
        }
    }

    private func decodingErrorText(_ error: DecodingError) -> String {
        switch error {
        case let .dataCorrupted(context):
            return "Invalid JSON: \(context.debugDescription)"
        case let .keyNotFound(key, _):
            return "Missing required field \"\(key.stringValue)\"."
        case let .typeMismatch(_, context), let .valueNotFound(_, context):
            let path = context.codingPath.map(\.stringValue).joined(separator: ".")
            return "\(context.debugDescription) at \(path.isEmpty ? "root" : path)"
        @unknown default:
            return "The JSON does not match the RuleSpecV1 schema."
        }
    }

    private func invalidateValidation() {
        validation = nil
        validationError = nil
        saveError = nil
    }

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

    private func regexField(_ placeholder: String, text: Binding<String>) -> some View {
        TextField(placeholder, text: text)
            .textFieldStyle(.roundedBorder)
            .font(.system(size: 11, design: .monospaced))
    }

    private func chipWrap(
        values: [String],
        titles: [String]? = nil,
        selectedPattern: String,
        action: @escaping (String) -> Void
    ) -> some View {
        HStack(spacing: 6) {
            ForEach(Array(values.enumerated()), id: \.element) { index, value in
                chip(
                    titles?[index] ?? value.capitalized,
                    selected: patternSelections(selectedPattern).contains(value)
                ) { action(value) }
            }
        }
    }

    /// Chips stay in sync with the regex text field: tapping a chip rebuilds
    /// an anchored alternation, while hand-written patterns simply leave all
    /// chips unselected.
    private func patternSelections(_ pattern: String) -> Set<String> {
        let trimmed = pattern.trimmingCharacters(in: .whitespacesAndNewlines)
        guard trimmed.hasPrefix("^"), trimmed.hasSuffix("$") else { return [] }
        var inner = String(trimmed.dropFirst().dropLast())
        if inner.hasPrefix("("), inner.hasSuffix(")") {
            inner = String(inner.dropFirst().dropLast())
        }
        let parts = inner.components(separatedBy: "|")
        let plain = parts.allSatisfy { part in
            !part.isEmpty && part.rangeOfCharacter(
                from: CharacterSet(charactersIn: "\\^$.[]{}()*+?")) == nil
        }
        return plain ? Set(parts) : []
    }

    private func toggleRegexChip(_ pattern: inout String, value: String) {
        var selections = patternSelections(pattern)
        if selections.contains(value) {
            selections.remove(value)
        } else if selections.isEmpty && !pattern.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            // A hand-written pattern is replaced rather than merged.
            selections = [value]
        } else {
            selections.insert(value)
        }
        pattern = MiddlewareWizardDraft.exactAlternation(selections.sorted())
    }

    private func toggleTargetProvider(_ provider: String) {
        if let index = draft.targetProviders.firstIndex(of: provider) {
            draft.targetProviders.remove(at: index)
        } else {
            draft.targetProviders.append(provider)
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
        action: @escaping (String) -> Void
    ) -> some View {
        LazyVGrid(
            columns: [GridItem(.adaptive(minimum: 145), spacing: 8)],
            spacing: 8
        ) {
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

    private func copyToPasteboard(_ text: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
    }

    private func client() -> AlexClient? {
        guard let config = store.config ?? DaemonDiscovery.load() else { return nil }
        return AlexClient(config: config)
    }

    private func traceMatchRule() -> MiddlewareRuleSpecV1? {
        if showingCode { return codeRule }
        guard regexProblems.isEmpty else { return nil }
        var matchDraft = draft
        if matchDraft.name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            matchDraft.name = "Unsaved middleware preview"
        }
        // Match testing evaluates no action. Supply a harmless valid action so
        // the full RuleSpecV1 can pass through the daemon's existing compiler
        // even while the user is still on the When step.
        if matchDraft.targetModel.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            matchDraft.targetModel = "middleware-match-preview"
        }
        if matchDraft.providerMode != .any && matchDraft.targetProviders.isEmpty {
            matchDraft.providerMode = .any
        }
        return try? matchDraft.makeRule(id: editingRuleID)
    }

    private func scheduleTraceMatch() {
        traceMatchTask?.cancel()
        isLoadingTraceMatches = false
        traceMatchError = nil
        guard traceMatchEndpointAvailable else { return }
        guard let rule = traceMatchRule() else {
            traceMatches = nil
            return
        }
        traceMatchTask = Task { @MainActor in
            do {
                try await Task.sleep(for: .milliseconds(400))
                try Task.checkCancellation()
                guard let client = client() else {
                    traceMatchError = "No Alex daemon configuration was found."
                    return
                }
                isLoadingTraceMatches = true
                defer { isLoadingTraceMatches = false }
                traceMatches = try await client.matchingMiddlewareTraces(for: rule)
                traceMatchError = nil
            } catch is CancellationError {
            } catch AlexClient.ClientError.http(404, _) {
                // Older daemons know the saved-rule dry-run shape only. The
                // unsaved ID intentionally yields 404, so this optional panel
                // disappears without affecting the rest of the wizard.
                traceMatchEndpointAvailable = false
                traceMatches = nil
                traceMatchError = nil
            } catch {
                traceMatches = nil
                traceMatchError = "Recent trace matching is temporarily unavailable."
            }
        }
    }

    private func traceMatchTime(_ timestampMs: Int64) -> String {
        Date(timeIntervalSince1970: Double(timestampMs) / 1_000)
            .formatted(date: .omitted, time: .shortened)
    }

    private func ruleForSubmission() -> MiddlewareRuleSpecV1? {
        if showingCode {
            return codeRule
        }
        return try? draft.makeRule(id: editingRuleID)
    }

    private func validate() async {
        validation = nil
        validationError = nil
        guard let rule = ruleForSubmission() else {
            validationError = showingCode
                ? (codeError ?? "The JSON does not match the RuleSpecV1 schema.")
                : draft.localValidationErrors.joined(separator: " ")
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
        guard let builtRule = ruleForSubmission(), let client = client() else {
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
