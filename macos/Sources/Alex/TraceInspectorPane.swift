import AppKit
import SwiftUI
import AlexCore

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
        InfoRow(label: "harness", value: model.harnessName(for: trace))
        InfoRow(label: "client ip", value: trace.clientIp)
        InfoRow(label: "key fingerprint", value: trace.keyFingerprint)
        InfoRow(label: "billing type", value: trace.billingBucket)
        InfoRow(label: "via dario", value: trace.viaDario.map { $0 ? "yes" : "no" })
        InfoRow(label: "dario generation", value: trace.darioGeneration)
        InfoRow(
            label: "subscription account",
            value: model.accountIdentity(trace.accountId))
        InfoRow(label: "internal route", value: model.internalRoute(trace.accountId))
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

/// Captured tool args/result body shown in the inspector column instead of
/// a popup window (mock had none of this — "View captured args"/"View
/// output" used to open an NSAlert). Breadcrumb lets the user step back to
/// the turn's normal inspector view.
struct ToolBodyInspectorView: View {
    let route: TraceBrowserModel.ToolBodyRoute
    @Bindable var model: TraceBrowserModel

    @State private var phase: TraceInspectorView.BodyLoad.Phase = .loading
    @State private var loadedRoute: TraceBrowserModel.ToolBodyRoute?

    var body: some View {
        VStack(spacing: 0) {
            header
            ScrollView {
                VStack(alignment: .leading, spacing: 8) {
                    content
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(12)
            }
        }
        .task(id: route) {
            guard loadedRoute != route else { return }
            phase = .loading
            do {
                let body = try await model.fetchToolBody(id: route.toolId, kind: route.kind)
                phase = .loaded(
                    raw: body.text, diskPath: body.diskPath,
                    isTruncated: body.isTruncated, fullByteCount: body.fullByteCount)
            } catch {
                phase = .failed(error.localizedDescription)
            }
            loadedRoute = route
        }
    }

    private var header: some View {
        PanelHeader(accentLeft: true) {
            VStack(alignment: .leading, spacing: 2) {
                breadcrumb
                Text(route.kind == "args" ? "Captured arguments" : "Captured output")
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(AlexTheme.Colors.foreground)
            }
        } right: {
            PanelIconButton(systemImage: "xmark", help: "Close details") {
                model.closeInspector()
            }
        }
    }

    /// "shortId › Turn N › toolName args|output" with a back arrow to
    /// return to the turn's normal inspector view.
    private var breadcrumb: some View {
        HStack(spacing: 4) {
            Button {
                model.closeInspectorToolBody()
            } label: {
                Image(systemName: "chevron.left")
                    .font(.system(size: 8, weight: .semibold))
            }
            .buttonStyle(.plain)
            .foregroundStyle(AlexTheme.Colors.primary)
            .help("Back to turn")
            if let sessionId = model.selectedSessionId {
                Text(SessionShortId.shorten(sessionId))
                    .font(AlexTheme.Fonts.mono(9.5))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
                    .lineLimit(1)
                    .truncationMode(.middle)
                Image(systemName: "chevron.right")
                    .font(.system(size: 7, weight: .semibold))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
            }
            Text(turnNumber.map { "Turn \($0)" } ?? "Turn —")
                .font(AlexTheme.Fonts.mono(9.5))
                .foregroundStyle(AlexTheme.Colors.textTertiary)
            Image(systemName: "chevron.right")
                .font(.system(size: 7, weight: .semibold))
                .foregroundStyle(AlexTheme.Colors.textTertiary)
            Text("\(route.toolName) \(route.kind == "args" ? "args" : "output")")
                .font(AlexTheme.Fonts.mono(9.5))
                .foregroundStyle(AlexTheme.Colors.textTertiary)
                .lineLimit(1)
        }
    }

    private var turnNumber: Int? {
        guard let index = model.turns.firstIndex(where: { $0.traceId == route.turnId }) else {
            return nil
        }
        return index + 1
    }

    @ViewBuilder
    private var content: some View {
        switch phase {
        case .idle, .loading:
            HStack(spacing: 6) {
                ProgressView().controlSize(.small)
                Text("Loading captured body…")
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
            }
        case let .failed(message):
            Text(message)
                .font(.system(size: 11))
                .foregroundStyle(.red)
                .textSelection(.enabled)
        case let .loaded(raw, diskPath, isTruncated, fullByteCount):
            let capped = BodyPretty.capped(raw)
            VStack(alignment: .leading, spacing: 4) {
                HStack(spacing: 8) {
                    Button("Copy") {
                        NSPasteboard.general.clearContents()
                        NSPasteboard.general.setString(capped.text, forType: .string)
                    }
                    Button("Reveal in Finder") {
                        guard let diskPath else { return }
                        NSWorkspace.shared.activateFileViewerSelecting(
                            [URL(fileURLWithPath: diskPath)])
                    }
                    .disabled(diskPath == nil)
                    if isTruncated {
                        Button("Load full body") {
                            Task {
                                guard let body = try? await model.fetchToolBody(
                                    id: route.toolId, kind: route.kind, loadFull: true)
                                else { return }
                                phase = .loaded(
                                    raw: body.text, diskPath: body.diskPath,
                                    isTruncated: false, fullByteCount: body.fullByteCount)
                            }
                        }
                        .help("Load all \(ByteCountFormatter.string(fromByteCount: Int64(fullByteCount), countStyle: .file))")
                    }
                    if !SSEFrames.isSSE(raw), !BodyPretty.looksLikeJSON(raw), capped.isTruncated {
                        Text("truncated to \(BodyPretty.displayCap / 1000)KB")
                            .font(.system(size: 9))
                            .foregroundStyle(.orange)
                    }
                    Spacer()
                }
                .controlSize(.small)
                if SSEFrames.isSSE(raw) {
                    SSEBodyView(source: raw)
                } else if BodyPretty.looksLikeJSON(raw) {
                    FormattedJSONBody(source: raw)
                } else {
                    InspectorTextPane(text: capped.text, highlightJSON: false)
                        .frame(minHeight: 220, maxHeight: .infinity)
                }
            }
        }
    }
}

struct TraceInspectorView: View {
    let traceId: String
    @Bindable var model: TraceBrowserModel

    @State private var detail: TraceDetailResponse?
    @State private var loadError: String?
    @State private var reqBody = BodyLoad()
    @State private var previousReqBody = BodyLoad()
    @State private var respBody = BodyLoad()
    @State private var darioReqBody = BodyLoad()
    @State private var darioRespBody = BodyLoad()
    @State private var copiedAll = false
    @State private var isLoading = false
    @State private var fullRequestJSON = false
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
            case loaded(
                raw: String, diskPath: String?, isTruncated: Bool, fullByteCount: Int)
            case failed(String)
        }
    }

    var body: some View {
        VStack(spacing: 0) {
            header
            if let trace = detail?.trace {
                quickStats(trace)
            }
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
            previousReqBody = BodyLoad()
            if reqBodyOpen {
                reqBody.phase = .loading
                loadBody(.request, into: $reqBody)
                if !fullRequestJSON {
                    loadPreviousRequestBody()
                }
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

    private func loadBody(
        _ kind: TraceBodyKind, into load: Binding<BodyLoad>, loadFull: Bool = false
    ) {
        // Keep previously loaded content visible while the next turn's body
        // loads so the inspector scroll position survives turn browsing.
        if case .loaded = load.wrappedValue.phase {
        } else {
            load.wrappedValue.phase = .loading
        }
        let tid = traceId
        Task {
            let phase = await fetchBody(tid, kind: kind, loadFull: loadFull)
            guard tid == traceId else { return }
            load.wrappedValue.phase = phase
        }
    }

    private func loadPreviousRequestBody() {
        let currentTraceId = traceId
        guard let previousTraceId = model.previousTraceId(before: currentTraceId) else {
            previousReqBody = BodyLoad()
            return
        }
        previousReqBody.phase = .loading
        Task {
            let phase = await fetchBody(previousTraceId, kind: .request)
            guard currentTraceId == traceId else { return }
            previousReqBody.phase = phase
        }
    }

    private var header: some View {
        PanelHeader(accentLeft: true) {
            VStack(alignment: .leading, spacing: 2) {
                breadcrumb
                HStack(spacing: AlexTheme.Spacing.md) {
                    Text(detail?.trace.method != nil ? "API Request" : "Turn Details")
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(AlexTheme.Colors.foreground)
                    if let status = detail?.trace.status {
                        httpStatusChip(status, errorKind: detail?.trace.errorKind)
                    }
                    if isLoading, detail != nil {
                        ProgressView()
                            .controlSize(.small)
                            .scaleEffect(0.55)
                    }
                }
            }
        } right: {
            Button(copiedAll ? "Copied" : "Copy All") {
                copyAll()
            }
            .controlSize(.small)
            .font(.system(size: 10))
            .disabled(detail == nil)
            .help("Copy the whole turn as markdown")
            PanelIconButton(systemImage: "chevron.left", help: "Previous turn") {
                model.stepInspector(-1)
            }
            .disabled(!model.canStepInspector(-1))
            PanelIconButton(systemImage: "chevron.right", help: "Next turn") {
                model.stepInspector(1)
            }
            .disabled(!model.canStepInspector(1))
            PanelIconButton(systemImage: "xmark", help: "Close details") {
                model.closeInspector()
            }
        }
    }

    /// Breadcrumb "shortId › Turn N" (mock TB App.tsx:759-768). The mock's
    /// trailing role segment has no per-turn source here — the inspector
    /// targets whole turns, not individual messages.
    private var breadcrumb: some View {
        HStack(spacing: 4) {
            if let sessionId = model.selectedSessionId {
                Text(SessionShortId.shorten(sessionId))
                    .font(AlexTheme.Fonts.mono(9.5))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
                    .lineLimit(1)
                    .truncationMode(.middle)
                Image(systemName: "chevron.right")
                    .font(.system(size: 7, weight: .semibold))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
            }
            Text(turnNumber.map { "Turn \($0)" } ?? "Turn —")
                .font(AlexTheme.Fonts.mono(9.5))
                .foregroundStyle(AlexTheme.Colors.textTertiary)
        }
    }

    private var turnNumber: Int? {
        guard let index = model.turns.firstIndex(where: { $0.traceId == traceId }) else {
            return nil
        }
        return index + 1
    }

    /// HTTP status chip: mono 9.5, 2×6 padding, radius 4, green/red tint pair
    /// (mock TB App.tsx:773-780).
    private func httpStatusChip(_ status: Int, errorKind: String?) -> some View {
        let clientClosed = TraceClassification.isClientDisconnect(errorKind: errorKind)
        let ok = (200..<300).contains(status)
        let tint = clientClosed
            ? AlexTheme.Colors.textSecondary
            : (ok ? AlexTheme.Colors.success : AlexTheme.Colors.destructive)
        return Text(clientClosed ? "client closed" : "\(status)")
            .font(AlexTheme.Fonts.mono(9.5))
            .foregroundStyle(tint)
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(RoundedRectangle(cornerRadius: 4).fill(tint.opacity(0.1)))
            .fixedSize()
    }

    /// 3-up Method | Duration | Tokens strip (mock TB App.tsx:788-801).
    private func quickStats(_ trace: TraceDetail) -> some View {
        let duration = trace.tsRequestMs.flatMap { requestMs in
            TurnHeader.duration(requestMs: requestMs, responseMs: trace.tsResponseMs)
        } ?? trace.latencyMs.map { "\($0)ms" }
        let tokens: String? = trace.inputTokens == nil && trace.outputTokens == nil
            ? nil
            : TraceNumberFormat.tokens((trace.inputTokens ?? 0) + (trace.outputTokens ?? 0))
        return StatTilesRow(
            items: [
                StatTileData(label: "Method", value: trace.method ?? "—"),
                StatTileData(label: "Duration", value: duration ?? "—"),
                StatTileData(label: "Tokens", value: tokens ?? "—"),
            ],
            style: .bordered)
    }

    /// Endpoint block: uppercase label, endpoint, request id
    /// (mock TB App.tsx:806-816).
    @ViewBuilder
    private func endpointBlock(_ trace: TraceDetail) -> some View {
        let methodPath = [trace.method, trace.path].compactMap(\.self)
            .joined(separator: " ")
        if !methodPath.isEmpty {
            VStack(alignment: .leading, spacing: 4) {
                Text("ENDPOINT")
                    .font(.system(size: 10, weight: .medium))
                    .tracking(0.7)
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
                Text(methodPath)
                    .font(AlexTheme.Fonts.metaMono)
                    .foregroundStyle(AlexTheme.Colors.textSecondary)
                    .textSelection(.enabled)
                    .lineLimit(2)
                // The daemon doesn't record the actual upstream base URL on
                // the trace (no such field in TraceDetail — only method,
                // path, and upstream_provider), so this shows the truthful
                // "<provider> · <path>" rather than guessing a host from the
                // provider name. See report's needs-backend note.
                if let provider = trace.upstreamProvider, let path = trace.path {
                    Text("\(provider) · \(path)")
                        .font(AlexTheme.Fonts.mono(9.5))
                        .foregroundStyle(AlexTheme.Colors.textTertiary)
                        .textSelection(.enabled)
                        .lineLimit(1)
                        .truncationMode(.middle)
                        .help(
                            "The full upstream URL wasn't captured by the daemon — "
                                + "only the provider name and request path are available.")
                }
                Text(trace.id)
                    .font(AlexTheme.Fonts.mono(10))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
                    .textSelection(.enabled)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
        }
    }

    @ViewBuilder
    private func content(_ response: TraceDetailResponse) -> some View {
        let trace = response.trace
        endpointBlock(trace)
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
        requestBodyGroup()
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
        let clientClosed = TraceClassification.isClientDisconnect(errorKind: trace.errorKind)
        section(
            "Overview",
            copyText: TurnExport.overviewLines(trace).joined(separator: "\n")
        ) {
            if let status = trace.status {
                InfoRow(
                    label: "status", value: "\(status)",
                    color: clientClosed
                        ? AlexTheme.Colors.textSecondary
                        : (status >= 400 ? .red : .green))
            }
            if let requestMs = trace.tsRequestMs {
                InfoRow(label: "time", value: TraceFormat.time(requestMs))
                InfoRow(
                    label: "duration",
                    value: TurnHeader.duration(
                        requestMs: requestMs, responseMs: trace.tsResponseMs)
                        ?? trace.latencyMs.map { "\($0)ms" })
            }
            InfoRow(label: "model", value: modelLine(trace))
            InfoRow(label: "harness", value: model.harnessName(for: trace))
            InfoRow(label: "provider", value: trace.upstreamProvider)
            FormatRow(clientFormat: trace.clientFormat, upstreamFormat: trace.upstreamFormat)
            InfoRow(label: "billing type", value: trace.billingBucket)
            InfoRow(
                label: "subscription account",
                value: model.accountIdentity(trace.accountId))
            InfoRow(label: "internal route", value: model.internalRoute(trace.accountId))
            InfoRow(label: "session", value: trace.sessionId)
            InfoRow(label: "run", value: trace.runId)
            InfoRow(label: "client ip", value: trace.clientIp)
            InfoRow(label: "key fingerprint", value: trace.keyFingerprint)
            InfoRow(label: "tokens", value: tokensLine(trace))
            if let cost = trace.costUsd, cost > 0 {
                InfoRow(label: "cost", value: TraceNumberFormat.cost(cost))
            }
            if clientClosed {
                InfoRow(
                    label: "event", value: "client closed",
                    color: AlexTheme.Colors.textSecondary)
            } else if let error = TraceErrorDisplay.line(
                kind: trace.errorKind, code: trace.errorCode, message: trace.error)
            {
                let label = trace.errorClass.map { "error [\($0)]" } ?? "error"
                InfoRow(label: label, value: error, color: .red)
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
                Text(title.uppercased())
                    .font(.system(size: 10, weight: .medium))
                    .tracking(0.7)
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
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
    private func requestBodyGroup() -> some View {
        DisclosureGroup(isExpanded: $reqBodyOpen) {
            requestBodyContent(reqBody.phase)
        } label: {
            groupLabel("Request body")
        }
        .onChange(of: reqBodyOpen) { _, open in
            guard open else { return }
            if case .idle = reqBody.phase {
                loadBody(.request, into: $reqBody)
            }
            if !fullRequestJSON, model.previousTraceId(before: traceId) != nil,
                case .idle = previousReqBody.phase
            {
                loadPreviousRequestBody()
            }
        }
        .onChange(of: fullRequestJSON) { _, showFull in
            guard !showFull, reqBodyOpen,
                model.previousTraceId(before: traceId) != nil,
                case .idle = previousReqBody.phase
            else { return }
            loadPreviousRequestBody()
        }
    }

    @ViewBuilder
    private func bodyGroup(
        title: String, kind: TraceBodyKind, load: Binding<BodyLoad>, isExpanded: Binding<Bool>
    ) -> some View {
        DisclosureGroup(isExpanded: isExpanded) {
            bodyContent(load.wrappedValue.phase, kind: kind, load: load)
        } label: {
            groupLabel(title)
        }
        .onChange(of: isExpanded.wrappedValue) { _, open in
            guard open, case .idle = load.wrappedValue.phase else { return }
            loadBody(kind, into: load)
        }
    }

    @ViewBuilder
    private func requestBodyContent(_ phase: BodyLoad.Phase) -> some View {
        switch phase {
        case .idle, .loading:
            bodyLoadingView("Loading request body…")
        case let .failed(message):
            bodyErrorView(message)
        case let .loaded(raw, diskPath, isTruncated, fullByteCount):
            if fullRequestJSON {
                bodyViewer(
                    source: raw,
                    displayed: BodyPretty.capped(raw).text,
                    diskPath: diskPath, highlightJSON: !rawMode && BodyPretty.looksLikeJSON(raw),
                    note: nil, showsFullJSONToggle: true,
                    sourceTruncated: isTruncated, fullByteCount: fullByteCount,
                    loadFull: { loadBody(.request, into: $reqBody, loadFull: true) })
            } else if model.previousTraceId(before: traceId) == nil {
                let presentation = RequestJSONDiff.presentation(previous: nil, current: raw)
                requestDiffViewer(presentation, source: raw, diskPath: diskPath)
            } else {
                switch previousReqBody.phase {
                case .idle, .loading:
                    bodyLoadingView("Loading previous request for comparison…")
                case let .failed(message):
                    bodyViewer(
                        source: raw, displayed: BodyPretty.capped(raw).text,
                        diskPath: diskPath, highlightJSON: BodyPretty.looksLikeJSON(raw),
                        note: "Previous request unavailable (\(message)); showing the full current body.",
                        showsFullJSONToggle: true, sourceTruncated: isTruncated,
                        fullByteCount: fullByteCount,
                        loadFull: { loadBody(.request, into: $reqBody, loadFull: true) })
                case let .loaded(previous, _, _, _):
                    let presentation = RequestJSONDiff.presentation(
                        previous: previous, current: raw)
                    requestDiffViewer(presentation, source: raw, diskPath: diskPath)
                }
            }
        }
    }

    @ViewBuilder
    private func requestDiffViewer(
        _ presentation: RequestJSONDiffPresentation, source: String, diskPath: String?
    ) -> some View {
        bodyViewer(
            source: source, displayed: presentation.text, diskPath: diskPath,
            highlightJSON: BodyPretty.isJSON(presentation.text), note: presentation.note,
            showsFullJSONToggle: true)
    }

    @ViewBuilder
    private func bodyContent(
        _ phase: BodyLoad.Phase, kind: TraceBodyKind, load: Binding<BodyLoad>
    ) -> some View {
        switch phase {
        case .idle, .loading:
            bodyLoadingView("Loading body…")
        case let .failed(message):
            bodyErrorView(message)
        case let .loaded(raw, diskPath, isTruncated, fullByteCount):
            bodyViewer(
                source: raw,
                displayed: BodyPretty.capped(raw).text,
                diskPath: diskPath, highlightJSON: !rawMode && BodyPretty.looksLikeJSON(raw),
                note: nil, showsFullJSONToggle: false, sourceTruncated: isTruncated,
                fullByteCount: fullByteCount,
                loadFull: { loadBody(kind, into: load, loadFull: true) })
        }
    }

    private func bodyLoadingView(_ label: String) -> some View {
        HStack(spacing: 6) {
            ProgressView().controlSize(.small)
            Text(label)
                .font(.system(size: 10))
                .foregroundStyle(.secondary)
        }
        .padding(.vertical, 4)
    }

    private func bodyErrorView(_ message: String) -> some View {
        Text(message)
            .font(.system(size: 10))
            .foregroundStyle(.red)
            .textSelection(.enabled)
    }

    @ViewBuilder
    private func bodyViewer(
        source: String, displayed: String, diskPath: String?, highlightJSON: Bool,
        note: String?, showsFullJSONToggle: Bool, sourceTruncated: Bool = false,
        fullByteCount: Int = 0, loadFull: (() -> Void)? = nil
    ) -> some View {
        let capped = BodyPretty.capped(displayed)
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 8) {
                Button("Copy") {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(
                        showsFullJSONToggle && !fullRequestJSON ? displayed : source,
                        forType: .string)
                }
                Button("Reveal in Finder") {
                    guard let diskPath else { return }
                    NSWorkspace.shared.activateFileViewerSelecting(
                        [URL(fileURLWithPath: diskPath)])
                }
                .disabled(diskPath == nil)
                if sourceTruncated, let loadFull {
                    Button("Load full body", action: loadFull)
                        .help("Load all \(ByteCountFormatter.string(fromByteCount: Int64(fullByteCount), countStyle: .file))")
                }
                if showsFullJSONToggle {
                    Toggle("Full JSON", isOn: $fullRequestJSON)
                        .toggleStyle(.checkbox)
                        .font(.system(size: 10))
                        .help("Show the complete request instead of changes from the previous request")
                }
                if !showsFullJSONToggle || fullRequestJSON {
                    Toggle("Raw", isOn: $rawMode)
                        .toggleStyle(.checkbox)
                        .font(.system(size: 10))
                        .help("Show as-fetched text without pretty-printing or highlighting")
                }
                // The formatted SSE/JSON views below have their own
                // truncation affordances (a "… (truncated)" token, or a
                // "showing the first N events" note); this generic banner
                // only applies to the plain capped-text fallback path.
                if !usesEnhancedFormatting(source: source), capped.isTruncated {
                    Text("truncated to \(BodyPretty.displayCap / 1000)KB")
                        .font(.system(size: 9))
                        .foregroundStyle(.orange)
                }
                Spacer()
            }
            .controlSize(.small)
            if let note {
                Text(note)
                    .font(.system(size: 9))
                    .foregroundStyle(.secondary)
            }
            if !rawMode, SSEFrames.isSSE(source) {
                SSEBodyView(source: source)
            } else if !rawMode, BodyPretty.isJSON(source) {
                FormattedJSONBody(source: source)
            } else {
                InspectorTextPane(text: capped.text, highlightJSON: highlightJSON)
                    .frame(height: 220)
            }
        }
    }

    /// Whether `bodyViewer` renders one of the new formatted views (SSE
    /// frames or tree-aware JSON) instead of the plain capped-text fallback,
    /// mirroring the branch above.
    private func usesEnhancedFormatting(source: String) -> Bool {
        guard !rawMode else { return false }
        return SSEFrames.isSSE(source) || BodyPretty.looksLikeJSON(source)
    }

    private func groupLabel(_ text: String) -> some View {
        Text(text)
            .font(.system(size: 11, weight: .medium))
            .foregroundStyle(AlexTheme.Colors.mutedForeground)
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

    private func fetchBody(
        _ id: String, kind: TraceBodyKind, loadFull: Bool = false
    ) async -> BodyLoad.Phase {
        do {
            let content = try await model.fetchTraceBody(
                id: id, kind: kind, loadFull: loadFull)
            return .loaded(
                raw: content.text, diskPath: content.diskPath,
                isTruncated: content.isTruncated, fullByteCount: content.fullByteCount)
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
                    .font(AlexTheme.Fonts.mono(10))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
                    .frame(width: 96, alignment: .leading)
                Text(value)
                    .font(AlexTheme.Fonts.mono(10))
                    .foregroundStyle(color ?? AlexTheme.Colors.textSecondary)
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
                    .font(AlexTheme.Fonts.mono(10))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
                    .frame(width: 96, alignment: .leading)
                Text("\(client) → \(upstream)\(translated ? "  (translated)" : "")")
                    .font(AlexTheme.Fonts.mono(10))
                    .foregroundStyle(
                        translated
                            ? AlexTheme.Colors.warningOrange : AlexTheme.Colors.textSecondary)
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
                    .foregroundStyle(AlexTheme.Colors.warningOrange)
            }
            // KV table styling (shared.tsx:405-420): dim right-aligned key
            // column, brighter truncating value.
            ForEach(pairs, id: \.name) { pair in
                HStack(alignment: .firstTextBaseline, spacing: 4) {
                    Circle()
                        .fill(
                            marked(pair.name)
                                ? AlexTheme.Colors.warningOrange : Color.clear)
                        .frame(width: 5, height: 5)
                    Text(pair.name)
                        .font(AlexTheme.Fonts.metaMono)
                        .foregroundStyle(AlexTheme.Colors.textTertiary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                        .frame(width: 130, alignment: .trailing)
                        .help(pair.name)
                    Text(pair.value)
                        .font(AlexTheme.Fonts.metaMono)
                        .foregroundStyle(AlexTheme.Colors.textSecondary)
                        .textSelection(.enabled)
                        .lineLimit(3)
                }
                .padding(.vertical, 1)
                .help(markHelp(pair.name) ?? "")
            }
            if let delta, !delta.removed.isEmpty {
                Text("missing vs first request: \(delta.removed.sorted().joined(separator: ", "))")
                    .font(.system(size: 9, design: .monospaced))
                    .foregroundStyle(AlexTheme.Colors.warningOrange)
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

/// Formatted (non-Raw) JSON body view: string values that are themselves
/// valid JSON render as indented, annotated sub-blocks, and literal newlines
/// inside long strings render as real line breaks — see `JsonFormatted` in
/// Core for the tree walk. The walk (plus the linear-highlighter fallback
/// for oversized bodies) runs off the main actor in `.task(id:)`, so a
/// multi-MB body never blocks the UI; Raw mode bypasses this view entirely
/// and shows the exact original text.
struct FormattedJSONBody: View {
    let source: String

    @State private var built: NSAttributedString?
    @State private var builtKey: String?

    var body: some View {
        Group {
            if let built {
                InspectorTextPane(text: "", precomputed: built)
            } else {
                HStack(spacing: 6) {
                    ProgressView().controlSize(.small)
                    Text("Formatting…")
                        .font(.system(size: 10))
                        .foregroundStyle(.secondary)
                }
                .padding(.vertical, 4)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        .frame(height: 220)
        .task(id: source) {
            guard builtKey != source else { return }
            let attributed = await FormattedJSONBodyBuilder.build(source)
            guard !Task.isCancelled else { return }
            built = attributed.value
            builtKey = source
        }
    }
}

/// The formatting work itself, split out of `FormattedJSONBody` (a `View`,
/// and so implicitly `@MainActor`) so it can genuinely run off the main
/// actor via `Task.detached`.
private enum FormattedJSONBodyBuilder {
    /// Above this size, skip the tree-walk enhancement (embedded-JSON
    /// sub-blocks, literal-newline rendering) and fall back to the existing
    /// linear `JsonHighlight` colorer — still computed off the main thread
    /// here, just without the extra per-string-value work a huge document
    /// would make expensive.
    static let enhancedSizeCap = 600_000

    static func build(_ source: String) async -> AttributedStringBox {
        await Task.detached(priority: .userInitiated) { () -> AttributedStringBox in
            let start = ContinuousClock.now
            defer {
                let elapsed = start.duration(to: .now)
                BarLog.timing(
                    .ui, label: "formatted json build bytes=\(source.utf8.count)",
                    milliseconds: Double(elapsed.components.seconds) * 1000
                        + Double(elapsed.components.attoseconds) / 1e15)
            }
            let font = NSFont.monospacedSystemFont(ofSize: 10, weight: .regular)
            if source.utf8.count <= enhancedSizeCap,
                let tokens = JsonFormatted.tokens(source, maxChars: BodyPretty.displayCap)
            {
                return AttributedStringBox(attributedString(tokens: tokens, font: font))
            }
            let displayed = BodyPretty.display(source).text
            let capped = BodyPretty.capped(displayed).text
            return AttributedStringBox(
                JsonHighlight.attributed(capped, font: font, colors: InspectorTextPane.jsonColors))
        }.value
    }

    private static func attributedString(tokens: [JsonFormatted.Token], font: NSFont) -> NSAttributedString {
        let out = NSMutableAttributedString()
        let colors = InspectorTextPane.jsonColors
        for token in tokens {
            let color: NSColor
            switch token.kind {
            case .key: color = colors.key
            case .string: color = colors.string
            case .number: color = colors.number
            case .boolean, .null: color = colors.keyword
            case .punctuation, .whitespace: color = colors.punctuation
            case .annotation: color = .tertiaryLabelColor
            }
            out.append(NSAttributedString(
                string: token.text, attributes: [.font: font, .foregroundColor: color]))
        }
        return out
    }
}

/// `NSAttributedString` isn't `Sendable` (Apple explicitly withholds the
/// conformance); this box carries a freshly-built, not-yet-shared one across
/// the `Task.detached` boundary, matching the existing
/// `TranscriptDocument`/`BuiltDocument` pattern used for render output.
private struct AttributedStringBox: @unchecked Sendable {
    let value: NSAttributedString

    init(_ value: NSAttributedString) {
        self.value = value
    }
}

/// Formatted SSE ("event: X\ndata: {...}") body view: splits the stream into
/// frames (`SSEFrames` in Core), shows a dim "event: <name>" header per
/// frame, and pretty-prints/highlights each frame's data via the existing
/// `JsonBlock`/`JsonSyntax` machinery. Frames are capped (see `SSEFrames`)
/// and paged in client-side with a "Show more" affordance rather than
/// rendering the whole stream at once. Parsing runs off the main actor.
struct SSEBodyView: View {
    let source: String

    @State private var frames: [SSEFrames.Frame] = []
    @State private var truncated = false
    @State private var parsedKey: String?
    @State private var visibleCount = 20

    private static let pageSize = 20

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 6) {
                if parsedKey == nil {
                    HStack(spacing: 6) {
                        ProgressView().controlSize(.small)
                        Text("Parsing events…")
                            .font(.system(size: 10))
                            .foregroundStyle(.secondary)
                    }
                } else if frames.isEmpty {
                    Text("No events parsed")
                        .font(.system(size: 10))
                        .foregroundStyle(.secondary)
                } else {
                    ForEach(Array(frames.prefix(visibleCount).enumerated()), id: \.offset) {
                        _, frame in
                        frameView(frame)
                    }
                    if visibleCount < frames.count {
                        Button("Show more (\(frames.count - visibleCount) more loaded)") {
                            visibleCount = min(frames.count, visibleCount + Self.pageSize)
                        }
                        .buttonStyle(.link)
                        .font(.system(size: 10))
                    } else if truncated {
                        Text(
                            "Showing the first \(frames.count) events — switch to Raw mode to see the full stream."
                        )
                        .font(.system(size: 9))
                        .foregroundStyle(.orange)
                    }
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(8)
        }
        .frame(height: 220)
        .background(
            RoundedRectangle(cornerRadius: AlexTheme.Radius.md)
                .fill(AlexTheme.Colors.surfaceFaint))
        .task(id: source) {
            guard parsedKey != source else { return }
            let result = await Task.detached(priority: .userInitiated) {
                SSEFrames.parse(source)
            }.value
            guard !Task.isCancelled else { return }
            frames = result.frames
            truncated = result.truncated
            parsedKey = source
            visibleCount = min(Self.pageSize, result.frames.count)
        }
    }

    @ViewBuilder
    private func frameView(_ frame: SSEFrames.Frame) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text("event: \(frame.event ?? "message")")
                .font(AlexTheme.Fonts.mono(9.5))
                .foregroundStyle(AlexTheme.Colors.textTertiary)
            if !frame.data.isEmpty, BodyPretty.isJSON(frame.data) {
                JsonBlock(content: BodyPretty.display(frame.data).text, maxHeight: 160)
            } else if !frame.data.isEmpty {
                Text(frame.data)
                    .font(AlexTheme.Fonts.metaMono)
                    .foregroundStyle(AlexTheme.Colors.textSecondary)
                    .textSelection(.enabled)
            }
        }
        .padding(6)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(
            RoundedRectangle(cornerRadius: AlexTheme.Radius.sm)
                .fill(AlexTheme.Colors.overlay(0.02)))
    }
}

struct InspectorTextPane: NSViewRepresentable {
    let text: String
    var highlightJSON = false
    var fontSize: CGFloat = 10
    /// When set, rendered as-is instead of computing highlighting from
    /// `text`/`highlightJSON` — lets a caller hand in an attributed string
    /// it already built off the main actor (see `FormattedJSONBody`, which
    /// runs `JsonFormatted` in a background task and hands the result here).
    var precomputed: NSAttributedString?

    private var font: NSFont {
        NSFont.monospacedSystemFont(ofSize: fontSize, weight: .regular)
    }

    /// JSON syntax palette aligned with `AlexTheme.Colors.Json`
    /// (shared.tsx:380-388); appearance-dynamic NSColors for the NSTextView.
    /// `nonisolated`: read from `FormattedJSONBodyBuilder`'s off-main
    /// formatting task; `JsonHighlight.Colors` is `@unchecked Sendable`.
    nonisolated static let jsonColors = JsonHighlight.Colors(
        key: dynamicColor(light: 0x33708E, dark: 0x79B8D4),
        string: dynamicColor(light: 0x4A7A3E, dark: 0x87BD78),
        number: dynamicColor(light: 0x9C5A28, dark: 0xD49668),
        keyword: dynamicColor(light: 0x7C4FA8, dark: 0xB48ADE),
        punctuation: dynamicColor(light: 0xB8B8C2, dark: 0x3E3E4A))

    private nonisolated static func dynamicColor(light: UInt32, dark: UInt32) -> NSColor {
        NSColor(name: nil) { appearance in
            let hex = appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
                ? dark : light
            return NSColor(
                srgbRed: CGFloat((hex >> 16) & 0xFF) / 255,
                green: CGFloat((hex >> 8) & 0xFF) / 255,
                blue: CGFloat(hex & 0xFF) / 255,
                alpha: 1)
        }
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
        if let precomputed {
            let key = "precomputed|\(precomputed.length)|\(precomputed.string.hashValue)"
            guard context.coordinator.lastKey != key else { return }
            context.coordinator.lastKey = key
            storage.setAttributedString(precomputed)
            textView.scroll(.zero)
            return
        }
        let key = "\(highlightJSON)|\(fontSize)|\(text.count)|\(text.hashValue)"
        guard context.coordinator.lastKey != key else { return }
        context.coordinator.lastKey = key
        if highlightJSON {
            storage.setAttributedString(
                JsonHighlight.attributed(text, font: font, colors: Self.jsonColors))
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
