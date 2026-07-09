import AppKit
import SwiftUI
import AlexandriaBarCore

struct SessionInfoCard: View {
    @Bindable var model: TraceBrowserModel
    @State private var showPrompt = false

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 4) {
                if let detail = model.firstTraceDetail {
                    facts(detail.trace)
                    if let prompt = model.sessionSystemPrompt {
                        Button {
                            showPrompt = true
                        } label: {
                            Label(
                                "System prompt (\(prompt.count) chars)",
                                systemImage: "doc.text")
                                .font(.system(size: 10, weight: .medium))
                        }
                        .controlSize(.small)
                        .padding(.top, 2)
                        .popover(isPresented: $showPrompt) {
                            SystemPromptView(
                                prompt: prompt,
                                modelName: model.selectedSession?.models?.first)
                        }
                    }
                    let headers = model.firstRequestHeaders
                    if !headers.isEmpty {
                        Text("First request headers")
                            .font(.system(size: 10, weight: .semibold))
                            .foregroundStyle(.secondary)
                            .padding(.top, 4)
                        HeaderListView(pairs: headers)
                    }
                } else {
                    HStack(spacing: 6) {
                        ProgressView().controlSize(.small)
                        Text("Loading first request…")
                            .font(.system(size: 10))
                            .foregroundStyle(.secondary)
                    }
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
        }
        .frame(maxHeight: 260)
        .background(.quaternary.opacity(0.2))
        .onAppear { model.ensureFirstTraceDetail() }
    }

    @ViewBuilder
    private func facts(_ trace: TraceDetail) -> some View {
        let session = model.selectedSession
        InfoRow(label: "harness", value: trace.harness ?? session?.harness)
        InfoRow(label: "client ip", value: trace.clientIp)
        InfoRow(label: "key fingerprint", value: trace.keyFingerprint)
        InfoRow(label: "billing", value: trace.billingBucket)
        FormatRow(clientFormat: trace.clientFormat, upstreamFormat: trace.upstreamFormat)
        InfoRow(label: "provider", value: trace.upstreamProvider)
        if let tags = session?.tags, !tags.isEmpty {
            let summary = tags.filter { !$0.value.isEmpty }
                .sorted { $0.key < $1.key }
                .map { "\($0.key)=\($0.value)" }
                .joined(separator: "  ")
            InfoRow(label: "tags", value: summary.isEmpty ? nil : summary)
        }
    }
}

struct TraceInspectorView: View {
    let traceId: String
    @Bindable var model: TraceBrowserModel

    @State private var detail: TraceDetailResponse?
    @State private var loadError: String?
    @State private var reqBody = BodyLoad()
    @State private var respBody = BodyLoad()
    @State private var darioReqBody = BodyLoad()
    @State private var darioRespBody = BodyLoad()
    @State private var copiedAll = false
    @State private var isLoading = false
    @AppStorage("InspectorRawMode") private var rawMode = false
    @AppStorage("InspectorReqHeadersOpen") private var reqHeadersOpen = false
    @AppStorage("InspectorRespHeadersOpen") private var respHeadersOpen = false
    @AppStorage("InspectorReqBodyOpen") private var reqBodyOpen = false
    @AppStorage("InspectorRespBodyOpen") private var respBodyOpen = false
    @AppStorage("InspectorDarioReqBodyOpen") private var darioReqBodyOpen = false
    @AppStorage("InspectorDarioRespBodyOpen") private var darioRespBodyOpen = false

    struct BodyLoad {
        var phase = Phase.idle

        enum Phase {
            case idle
            case loading
            case loaded(raw: String, diskPath: String?)
            case failed(String)
        }
    }

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider()
            ScrollView {
                VStack(alignment: .leading, spacing: 8) {
                    if let detail {
                        content(detail)
                    } else if let loadError {
                        Text(loadError)
                            .font(.system(size: 11))
                            .foregroundStyle(.red)
                            .textSelection(.enabled)
                    } else {
                        HStack(spacing: 6) {
                            ProgressView().controlSize(.small)
                            Text("Loading trace…")
                                .font(.system(size: 11))
                                .foregroundStyle(.secondary)
                        }
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(12)
            }
        }
        .task(id: traceId) {
            loadError = nil
            isLoading = true
            if reqBodyOpen {
                loadBody(.request, into: $reqBody)
            } else {
                reqBody = BodyLoad()
            }
            if respBodyOpen {
                loadBody(.response, into: $respBody)
            } else {
                respBody = BodyLoad()
            }
            darioReqBody = BodyLoad()
            darioRespBody = BodyLoad()
            await loadDetail()
            if let capture = detail?.extras?.darioCapture {
                if darioReqBodyOpen && capture.requestAvailable {
                    loadBody(.darioUpstreamRequest, into: $darioReqBody)
                }
                if darioRespBodyOpen && capture.responseAvailable {
                    loadBody(.darioUpstreamResponse, into: $darioRespBody)
                }
            }
            isLoading = false
        }
    }

    private func loadBody(_ kind: TraceBodyKind, into load: Binding<BodyLoad>) {
        // Keep previously loaded content visible while the next turn's body
        // loads so the inspector scroll position survives turn browsing.
        if case .loaded = load.wrappedValue.phase {
        } else {
            load.wrappedValue.phase = .loading
        }
        let tid = traceId
        Task {
            let phase = await fetchBody(tid, kind: kind)
            guard tid == traceId else { return }
            load.wrappedValue.phase = phase
        }
    }

    private var header: some View {
        HStack(spacing: 8) {
            Text("Turn Details")
                .font(.system(size: 11, weight: .semibold))
            Text(traceId)
                .font(.system(size: 10, design: .monospaced))
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .truncationMode(.middle)
                .textSelection(.enabled)
            if isLoading, detail != nil {
                ProgressView()
                    .controlSize(.small)
                    .scaleEffect(0.55)
            }
            Spacer()
            Button(copiedAll ? "Copied" : "Copy All") {
                copyAll()
            }
            .controlSize(.small)
            .font(.system(size: 10))
            .disabled(detail == nil)
            .help("Copy the whole turn as markdown")
            Button {
                model.stepInspector(-1)
            } label: {
                Image(systemName: "chevron.left")
                    .font(.system(size: 10, weight: .semibold))
                    .foregroundStyle(.secondary)
            }
            .buttonStyle(.plain)
            .disabled(!model.canStepInspector(-1))
            .help("Previous turn")
            Button {
                model.stepInspector(1)
            } label: {
                Image(systemName: "chevron.right")
                    .font(.system(size: 10, weight: .semibold))
                    .foregroundStyle(.secondary)
            }
            .buttonStyle(.plain)
            .disabled(!model.canStepInspector(1))
            .help("Next turn")
            Button {
                model.closeInspector()
            } label: {
                Image(systemName: "xmark.circle.fill")
                    .foregroundStyle(.secondary)
            }
            .buttonStyle(.plain)
            .help("Close details")
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 7)
    }

    @ViewBuilder
    private func content(_ response: TraceDetailResponse) -> some View {
        let trace = response.trace
        overview(trace)
        if let extras = response.extras, extras.hasAny {
            section(
                "Extras",
                copyText: TurnExport.extrasLines(extras).joined(separator: "\n")
            ) {
                InfoRow(label: "reasoning effort", value: extras.reasoningEffort)
                InfoRow(label: "thinking budget", value: extras.thinkingBudget.map { "\($0)" })
                InfoRow(label: "max tokens", value: extras.maxTokens.map { "\($0)" })
                InfoRow(label: "temperature", value: extras.temperature.map { "\($0)" })
                InfoRow(label: "messages", value: extras.messageCount.map { "\($0)" })
                InfoRow(label: "system chars", value: extras.systemChars.map { "\($0)" })
                if let capture = extras.darioCapture {
                    InfoRow(
                        label: "Dario request",
                        value: capture.requestAvailable ? (capture.requestPath ?? "captured") : nil)
                    InfoRow(
                        label: "Dario response",
                        value: capture.responseAvailable ? (capture.responsePath ?? "captured") : nil)
                    if let prompt = capture.promptCache {
                        InfoRow(label: "prompt model", value: prompt.model)
                        InfoRow(label: "prompt cache", value: promptCacheLine(prompt))
                        InfoRow(label: "prompt path", value: prompt.path)
                        InfoRow(label: "prompt error", value: prompt.error, color: .red)
                    }
                }
            }
        }
        Divider()
        headersGroup(
            title: "Request headers", json: trace.reqHeadersJson,
            isExpanded: $reqHeadersOpen, diffAgainstFirst: true)
        headersGroup(
            title: "Response headers", json: trace.respHeadersJson,
            isExpanded: $respHeadersOpen, diffAgainstFirst: false)
        Divider()
        bodyGroup(
            title: "Request body", kind: .request, load: $reqBody, isExpanded: $reqBodyOpen)
        bodyGroup(
            title: "Response body", kind: .response, load: $respBody, isExpanded: $respBodyOpen)
        if let capture = response.extras?.darioCapture {
            if capture.requestAvailable {
                bodyGroup(
                    title: "Dario → Anthropic", kind: .darioUpstreamRequest,
                    load: $darioReqBody, isExpanded: $darioReqBodyOpen)
            }
            if capture.responseAvailable {
                bodyGroup(
                    title: "Anthropic → Dario", kind: .darioUpstreamResponse,
                    load: $darioRespBody, isExpanded: $darioRespBodyOpen)
            }
        }
    }

    private func copyAll() {
        guard let response = detail else { return }
        let tid = traceId
        Task {
            let reqContent = try? await model.fetchTraceBody(id: tid, kind: .request)
            let respContent = try? await model.fetchTraceBody(id: tid, kind: .response)
            let markdown = TurnExport.markdown(
                detail: response.trace, extras: response.extras,
                reqHeaders: TraceHeaders.sortedPairs(response.trace.reqHeadersJson),
                respHeaders: TraceHeaders.sortedPairs(response.trace.respHeadersJson),
                reqBody: reqContent?.text, respBody: respContent?.text)
            guard tid == traceId else { return }
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(markdown, forType: .string)
            copiedAll = true
            try? await Task.sleep(for: .seconds(1.5))
            copiedAll = false
        }
    }

    @ViewBuilder
    private func overview(_ trace: TraceDetail) -> some View {
        section(
            "Overview",
            copyText: TurnExport.overviewLines(trace).joined(separator: "\n")
        ) {
            if let status = trace.status {
                InfoRow(
                    label: "status", value: "\(status)",
                    color: status >= 400 ? .red : .green)
            }
            let methodPath = [trace.method, trace.path].compactMap(\.self)
                .joined(separator: " ")
            InfoRow(label: "endpoint", value: methodPath.isEmpty ? nil : methodPath)
            if let requestMs = trace.tsRequestMs {
                InfoRow(label: "time", value: TraceFormat.time(requestMs))
                InfoRow(
                    label: "duration",
                    value: TurnHeader.duration(
                        requestMs: requestMs, responseMs: trace.tsResponseMs)
                        ?? trace.latencyMs.map { "\($0)ms" })
            }
            InfoRow(label: "model", value: modelLine(trace))
            InfoRow(label: "provider", value: trace.upstreamProvider)
            FormatRow(clientFormat: trace.clientFormat, upstreamFormat: trace.upstreamFormat)
            InfoRow(label: "billing", value: trace.billingBucket)
            InfoRow(label: "account", value: trace.accountId)
            InfoRow(label: "session", value: trace.sessionId)
            InfoRow(label: "run", value: trace.runId)
            InfoRow(label: "client ip", value: trace.clientIp)
            InfoRow(label: "key fingerprint", value: trace.keyFingerprint)
            InfoRow(label: "tokens", value: tokensLine(trace))
            if let cost = trace.costUsd, cost > 0 {
                InfoRow(label: "cost", value: TraceNumberFormat.cost(cost))
            }
            if let error = trace.error, !error.isEmpty {
                InfoRow(label: "error", value: error, color: .red)
            }
        }
    }

    private func modelLine(_ trace: TraceDetail) -> String? {
        switch (trace.requestedModel, trace.routedModel) {
        case let (.some(requested), .some(routed)) where requested != routed:
            return "\(requested) → \(routed)"
        case let (requested, routed):
            return requested ?? routed
        }
    }

    private func tokensLine(_ trace: TraceDetail) -> String? {
        guard trace.inputTokens != nil || trace.outputTokens != nil else { return nil }
        var parts = ["in \(TraceNumberFormat.tokens(trace.inputTokens))"]
        if let cached = trace.cachedInputTokens, cached > 0 {
            parts.append("cached \(TraceNumberFormat.tokens(cached))")
        }
        parts.append("out \(TraceNumberFormat.tokens(trace.outputTokens))")
        if let reasoning = trace.reasoningTokens, reasoning > 0 {
            parts.append("reasoning \(TraceNumberFormat.tokens(reasoning))")
        }
        return parts.joined(separator: " · ")
    }

    private func promptCacheLine(_ prompt: DarioPromptCacheUse) -> String? {
        let parts = [
            prompt.status,
            prompt.applied.map { $0 ? "applied" : "not applied" },
            prompt.systemPromptChars.map { "\($0) chars" },
            prompt.claudeVersion,
        ].compactMap(\.self)
        return parts.isEmpty ? nil : parts.joined(separator: " · ")
    }

    @ViewBuilder
    private func section(
        _ title: String, copyText: String? = nil, @ViewBuilder rows: () -> some View
    ) -> some View {
        VStack(alignment: .leading, spacing: 3) {
            HStack(spacing: 6) {
                Text(title)
                    .font(.system(size: 10, weight: .semibold))
                    .foregroundStyle(.secondary)
                if let copyText, !copyText.isEmpty {
                    CopyIconButton(text: copyText, help: "Copy \(title.lowercased())")
                }
            }
            rows()
        }
    }

    @ViewBuilder
    private func headersGroup(
        title: String, json: String?, isExpanded: Binding<Bool>, diffAgainstFirst: Bool
    ) -> some View {
        let pairs = TraceHeaders.sortedPairs(json)
        DisclosureGroup(isExpanded: isExpanded) {
            if pairs.isEmpty {
                Text("none recorded")
                    .font(.system(size: 10))
                    .foregroundStyle(.tertiary)
            } else {
                VStack(alignment: .leading, spacing: 4) {
                    CopyIconButton(
                        text: TurnExport.headerLines(pairs).joined(separator: "\n"),
                        help: "Copy \(title.lowercased())", showsLabel: true)
                    HeaderListView(
                        pairs: pairs,
                        delta: diffAgainstFirst ? firstRequestDelta(pairs) : nil)
                }
            }
        } label: {
            groupLabel("\(title) (\(pairs.count))")
        }
    }

    private func firstRequestDelta(_ pairs: [HeaderPair]) -> HeaderDelta? {
        guard traceId != model.firstTurnTraceId else { return nil }
        let first = model.firstRequestHeaders
        guard !first.isEmpty else { return nil }
        let delta = HeaderDiff.delta(first: first, other: pairs)
        return delta.isEmpty ? nil : delta
    }

    @ViewBuilder
    private func bodyGroup(
        title: String, kind: TraceBodyKind, load: Binding<BodyLoad>, isExpanded: Binding<Bool>
    ) -> some View {
        DisclosureGroup(isExpanded: isExpanded) {
            bodyContent(load.wrappedValue.phase)
        } label: {
            groupLabel(title)
        }
        .onChange(of: isExpanded.wrappedValue) { _, open in
            guard open, case .idle = load.wrappedValue.phase else { return }
            loadBody(kind, into: load)
        }
    }

    @ViewBuilder
    private func bodyContent(_ phase: BodyLoad.Phase) -> some View {
        switch phase {
        case .idle, .loading:
            HStack(spacing: 6) {
                ProgressView().controlSize(.small)
                Text("Loading body…")
                    .font(.system(size: 10))
                    .foregroundStyle(.secondary)
            }
            .padding(.vertical, 4)
        case let .failed(message):
            Text(message)
                .font(.system(size: 10))
                .foregroundStyle(.red)
                .textSelection(.enabled)
        case let .loaded(raw, diskPath):
            let capped = rawMode ? BodyPretty.capped(raw) : BodyPretty.display(raw)
            let highlight = !rawMode && BodyPretty.isJSON(raw)
            VStack(alignment: .leading, spacing: 4) {
                HStack(spacing: 8) {
                    Button("Copy") {
                        NSPasteboard.general.clearContents()
                        NSPasteboard.general.setString(raw, forType: .string)
                    }
                    Button("Reveal in Finder") {
                        guard let diskPath else { return }
                        NSWorkspace.shared.activateFileViewerSelecting(
                            [URL(fileURLWithPath: diskPath)])
                    }
                    .disabled(diskPath == nil)
                    Toggle("Raw", isOn: $rawMode)
                        .toggleStyle(.checkbox)
                        .font(.system(size: 10))
                        .help("Show as-fetched text without pretty-printing or highlighting")
                    if capped.isTruncated {
                        Text("truncated to \(BodyPretty.displayCap / 1000)KB")
                            .font(.system(size: 9))
                            .foregroundStyle(.orange)
                    }
                    Spacer()
                }
                .controlSize(.small)
                InspectorTextPane(text: capped.text, highlightJSON: highlight)
                    .frame(height: 220)
            }
        }
    }

    private func groupLabel(_ text: String) -> some View {
        Text(text)
            .font(.system(size: 11, weight: .medium))
    }

    private func loadDetail() async {
        guard let client = model.detailClient() else {
            loadError = "daemon unavailable"
            return
        }
        do {
            let fetched = try await client.traceDetail(id: traceId)
            guard fetched.trace.id == traceId else { return }
            detail = fetched
        } catch {
            if !(error is CancellationError) {
                loadError = error.localizedDescription
            }
        }
    }

    private func fetchBody(_ id: String, kind: TraceBodyKind) async -> BodyLoad.Phase {
        do {
            let content = try await model.fetchTraceBody(id: id, kind: kind)
            return .loaded(raw: content.text, diskPath: content.diskPath)
        } catch {
            return .failed(error.localizedDescription)
        }
    }
}

struct InfoRow: View {
    let label: String
    let value: String?
    var color: Color?

    var body: some View {
        if let value, !value.isEmpty {
            HStack(alignment: .firstTextBaseline, spacing: 6) {
                Text(label)
                    .font(.system(size: 10))
                    .foregroundStyle(.secondary)
                    .frame(width: 96, alignment: .leading)
                Text(value)
                    .font(.system(size: 10, design: .monospaced))
                    .foregroundStyle(color ?? Color.primary)
                    .textSelection(.enabled)
                    .lineLimit(4)
            }
        }
    }
}

struct FormatRow: View {
    let clientFormat: String?
    let upstreamFormat: String?

    var body: some View {
        if clientFormat != nil || upstreamFormat != nil {
            let client = clientFormat ?? "?"
            let upstream = upstreamFormat ?? "?"
            let translated = clientFormat != nil && upstreamFormat != nil
                && clientFormat != upstreamFormat
            HStack(alignment: .firstTextBaseline, spacing: 6) {
                Text("format")
                    .font(.system(size: 10))
                    .foregroundStyle(.secondary)
                    .frame(width: 96, alignment: .leading)
                Text("\(client) → \(upstream)\(translated ? "  (translated)" : "")")
                    .font(.system(size: 10, design: .monospaced))
                    .foregroundStyle(translated ? Color.orange : Color.primary)
                    .textSelection(.enabled)
            }
        }
    }
}

struct HeaderListView: View {
    let pairs: [HeaderPair]
    var delta: HeaderDelta?

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            if let delta, !delta.isEmpty {
                Label("differs from first request", systemImage: "circle.fill")
                    .font(.system(size: 9))
                    .foregroundStyle(.orange)
            }
            ForEach(pairs, id: \.name) { pair in
                HStack(alignment: .firstTextBaseline, spacing: 4) {
                    Circle()
                        .fill(marked(pair.name) ? Color.orange : Color.clear)
                        .frame(width: 5, height: 5)
                    Text("\(pair.name): \(pair.value)")
                        .font(.system(size: 10, design: .monospaced))
                        .textSelection(.enabled)
                        .lineLimit(3)
                }
                .help(markHelp(pair.name) ?? "")
            }
            if let delta, !delta.removed.isEmpty {
                Text("missing vs first request: \(delta.removed.sorted().joined(separator: ", "))")
                    .font(.system(size: 9, design: .monospaced))
                    .foregroundStyle(.orange)
            }
        }
    }

    private func marked(_ name: String) -> Bool {
        guard let delta else { return false }
        return delta.status(for: name) != .same
    }

    private func markHelp(_ name: String) -> String? {
        switch delta?.status(for: name) {
        case .added: "not present in first request"
        case .changed: "value differs from first request"
        default: nil
        }
    }
}

struct CopyIconButton: View {
    let text: String
    var help = "Copy"
    var showsLabel = false
    @State private var copied = false

    var body: some View {
        Button {
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(text, forType: .string)
            copied = true
            Task {
                try? await Task.sleep(for: .seconds(1.5))
                copied = false
            }
        } label: {
            HStack(spacing: 3) {
                Image(systemName: copied ? "checkmark" : "doc.on.doc")
                    .font(.system(size: 9))
                if showsLabel {
                    Text(copied ? "Copied" : "Copy")
                        .font(.system(size: 9))
                }
            }
            .foregroundStyle(copied ? AnyShapeStyle(.green) : AnyShapeStyle(.secondary))
        }
        .buttonStyle(.plain)
        .help(help)
    }
}

struct SystemPromptView: View {
    let prompt: String
    let modelName: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
                Text("System prompt")
                    .font(.system(size: 12, weight: .semibold))
                Text("\(prompt.count) chars\(modelName.map { " · \($0)" } ?? "")")
                    .font(.system(size: 10))
                    .foregroundStyle(.secondary)
                Spacer()
                Button("Copy") {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(prompt, forType: .string)
                }
                .controlSize(.small)
            }
            InspectorTextPane(text: prompt, fontSize: 12)
                .frame(minHeight: 360)
        }
        .padding(12)
        .frame(width: 560, height: 440)
    }
}

struct InspectorTextPane: NSViewRepresentable {
    let text: String
    var highlightJSON = false
    var fontSize: CGFloat = 10

    private var font: NSFont {
        NSFont.monospacedSystemFont(ofSize: fontSize, weight: .regular)
    }

    func makeNSView(context: Context) -> NSScrollView {
        let textView = NSTextView(usingTextLayoutManager: true)
        textView.isEditable = false
        textView.isSelectable = true
        textView.isRichText = false
        textView.drawsBackground = false
        textView.font = font
        textView.textContainerInset = NSSize(width: 6, height: 6)
        textView.isVerticallyResizable = true
        textView.isHorizontallyResizable = false
        textView.autoresizingMask = [.width]
        textView.textContainer?.widthTracksTextView = true
        let scroll = NSScrollView()
        scroll.documentView = textView
        scroll.hasVerticalScroller = true
        scroll.drawsBackground = false
        scroll.borderType = .bezelBorder
        return scroll
    }

    func updateNSView(_ scroll: NSScrollView, context: Context) {
        guard let textView = scroll.documentView as? NSTextView,
            let storage = textView.textStorage
        else { return }
        let key = "\(highlightJSON)|\(fontSize)|\(text.count)|\(text.hashValue)"
        guard context.coordinator.lastKey != key else { return }
        context.coordinator.lastKey = key
        if highlightJSON {
            storage.setAttributedString(JsonHighlight.attributed(text, font: font))
        } else {
            storage.setAttributedString(NSAttributedString(
                string: text,
                attributes: [.font: font, .foregroundColor: NSColor.labelColor]))
        }
        textView.scroll(.zero)
    }

    func makeCoordinator() -> Coordinator { Coordinator() }

    @MainActor
    final class Coordinator {
        var lastKey: String?
    }
}
