import AppKit
import SwiftUI
import AlexandriaBarCore

struct SessionInfoCard: View {
    @Bindable var model: TraceBrowserModel

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 4) {
                if let detail = model.firstTraceDetail {
                    facts(detail.trace)
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
    @State private var reqHeadersOpen = false
    @State private var respHeadersOpen = false
    @State private var reqBody = BodyLoad()
    @State private var respBody = BodyLoad()

    struct BodyLoad {
        var open = false
        var phase = Phase.idle

        enum Phase {
            case idle
            case loading
            case loaded(display: String, truncated: Bool, raw: String, diskPath: String?)
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
            detail = nil
            loadError = nil
            reqHeadersOpen = false
            respHeadersOpen = false
            reqBody = BodyLoad()
            respBody = BodyLoad()
            await loadDetail()
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
            Spacer()
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
            section("Extras") {
                InfoRow(label: "reasoning effort", value: extras.reasoningEffort)
                InfoRow(label: "thinking budget", value: extras.thinkingBudget.map { "\($0)" })
                InfoRow(label: "max tokens", value: extras.maxTokens.map { "\($0)" })
                InfoRow(label: "temperature", value: extras.temperature.map { "\($0)" })
                InfoRow(label: "messages", value: extras.messageCount.map { "\($0)" })
                InfoRow(label: "system chars", value: extras.systemChars.map { "\($0)" })
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
        bodyGroup(title: "Request body", kind: .request, load: $reqBody)
        bodyGroup(title: "Response body", kind: .response, load: $respBody)
    }

    @ViewBuilder
    private func overview(_ trace: TraceDetail) -> some View {
        section("Overview") {
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

    @ViewBuilder
    private func section(_ title: String, @ViewBuilder rows: () -> some View) -> some View {
        VStack(alignment: .leading, spacing: 3) {
            Text(title)
                .font(.system(size: 10, weight: .semibold))
                .foregroundStyle(.secondary)
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
                HeaderListView(pairs: pairs, delta: diffAgainstFirst ? firstRequestDelta(pairs) : nil)
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
        title: String, kind: TraceBodyKind, load: Binding<BodyLoad>
    ) -> some View {
        DisclosureGroup(isExpanded: load.open) {
            bodyContent(load.wrappedValue.phase)
        } label: {
            groupLabel(title)
        }
        .onChange(of: load.wrappedValue.open) { _, open in
            guard open, case .idle = load.wrappedValue.phase else { return }
            load.wrappedValue.phase = .loading
            let tid = traceId
            Task {
                let phase = await fetchBody(tid, kind: kind)
                guard tid == traceId else { return }
                load.wrappedValue.phase = phase
            }
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
        case let .loaded(display, truncated, raw, diskPath):
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
                    if truncated {
                        Text("truncated to \(BodyPretty.displayCap / 1000)KB")
                            .font(.system(size: 9))
                            .foregroundStyle(.orange)
                    }
                    Spacer()
                }
                .controlSize(.small)
                InspectorTextPane(text: display)
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
        guard let client = model.detailClient() else { return .failed("daemon unavailable") }
        do {
            let content = try await client.traceBody(id: id, kind: kind)
            let capped = BodyPretty.display(content.text)
            return .loaded(
                display: capped.text, truncated: capped.isTruncated,
                raw: content.text, diskPath: content.diskPath)
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

struct InspectorTextPane: NSViewRepresentable {
    let text: String

    func makeNSView(context: Context) -> NSScrollView {
        let textView = NSTextView(usingTextLayoutManager: true)
        textView.isEditable = false
        textView.isSelectable = true
        textView.isRichText = false
        textView.drawsBackground = false
        textView.font = NSFont.monospacedSystemFont(ofSize: 10, weight: .regular)
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
        guard let textView = scroll.documentView as? NSTextView else { return }
        if textView.string != text {
            textView.string = text
            textView.font = NSFont.monospacedSystemFont(ofSize: 10, weight: .regular)
            textView.scroll(.zero)
        }
    }
}
