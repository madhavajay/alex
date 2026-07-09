import AppKit
import SwiftUI
import Observation
import AlexandriaBarCore

enum HarnessActionKind: Equatable {
    case connect
    case disconnect
    case refresh

    /// Canonical user-facing label (flyout, tab, modal titles, progress).
    var label: String {
        switch self {
        case .connect: "Install Config & Models"
        case .disconnect: "Remove Config & Models"
        case .refresh: "Update Config & Models"
        }
    }

    var windowTitle: String { label }
}

/// Visual state for each plan line across approve → response.
enum PlanLineMark: Equatable {
    case none
    case spinning
    case ok
    case error
}

enum HarnessActionPhase: Equatable {
    case loading
    /// Awaiting Approve/Cancel.
    case plan([HarnessPlanStep])
    /// Request in flight; plan lines stay visible with spinners.
    case executing([HarnessPlanStep])
    case successConfig([HarnessPlanStep], HarnessConfigWriteResponse)
    case successDisconnect([HarnessPlanStep], HarnessDisconnectResponse)
    /// Optional plan from a failed approve; empty when plan load itself failed.
    case failure([HarnessPlanStep], String)

    var isBusy: Bool {
        switch self {
        case .loading, .executing: true
        default: false
        }
    }

    var planSteps: [HarnessPlanStep] {
        switch self {
        case .plan(let s), .executing(let s), .successConfig(let s, _), .successDisconnect(let s, _),
            .failure(let s, _):
            return s
        case .loading:
            return []
        }
    }

    var lineMark: PlanLineMark {
        switch self {
        case .plan: .none
        case .executing: .spinning
        case .successConfig, .successDisconnect: .ok
        case .failure: .error
        case .loading: .none
        }
    }
}

/// Shared result / plan modal used by refresh (one-shot) and connect/disconnect (approve).
/// Scrollable body + pinned footer (Approve/Cancel or Done) so long results are never clipped.
struct HarnessActionResultView: View {
    let kind: HarnessActionKind
    let harnessDisplayName: String
    let phase: HarnessActionPhase
    var onApprove: (() -> Void)? = nil
    var onCancel: (() -> Void)? = nil
    let onClose: () -> Void

    private let listCap = 12

    static let minSize = CGSize(width: 420, height: 360)
    static let idealSize = CGSize(width: 480, height: 420)

    var body: some View {
        VStack(spacing: 0) {
            ScrollView {
                VStack(alignment: .leading, spacing: 14) {
                    Text(title)
                        .font(.headline)
                    if !harnessDisplayName.isEmpty {
                        Text(harnessDisplayName)
                            .font(.system(size: 11))
                            .foregroundStyle(.secondary)
                    }
                    content
                }
                .frame(maxWidth: .infinity, alignment: .topLeading)
                .padding(18)
            }
            Divider()
            footer
                .padding(.horizontal, 18)
                .padding(.vertical, 12)
                .frame(maxWidth: .infinity)
                .background(.bar)
        }
        .frame(
            minWidth: Self.minSize.width,
            idealWidth: Self.idealSize.width,
            minHeight: Self.minSize.height,
            idealHeight: Self.idealSize.height,
            alignment: .topLeading
        )
        // Sheet presentation is non-resizable by default; promote the host window.
        .background(HarnessActionWindowSizer())
    }

    private var title: String {
        switch phase {
        case .loading, .plan, .executing:
            return kind.label
        case .successConfig, .successDisconnect:
            return kind.label
        case .failure:
            return "\(kind.label) Failed"
        }
    }

    @ViewBuilder
    private var content: some View {
        switch phase {
        case .loading:
            busyRow(message: loadingMessage)
        case .plan(let steps):
            planBody(steps, mark: .none, heading: "This will:")
        case .executing(let steps):
            VStack(alignment: .leading, spacing: 10) {
                busyRow(message: executingMessage)
                planBody(steps, mark: .spinning, heading: "In progress:")
            }
        case .successConfig(let steps, let result):
            VStack(alignment: .leading, spacing: 12) {
                if !steps.isEmpty {
                    planBody(steps, mark: .ok, heading: "Completed:")
                }
                configSuccessBody(result)
            }
        case .successDisconnect(let steps, let result):
            VStack(alignment: .leading, spacing: 12) {
                if !steps.isEmpty {
                    planBody(steps, mark: .ok, heading: "Completed:")
                }
                disconnectSuccessBody(result)
            }
        case .failure(let steps, let message):
            VStack(alignment: .leading, spacing: 10) {
                if !steps.isEmpty {
                    planBody(steps, mark: .error, heading: "Failed:")
                }
                Text(message)
                    .font(.system(size: 11))
                    .foregroundStyle(.red)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    private var loadingMessage: String {
        switch kind {
        case .connect, .disconnect:
            return "Planning \(kind.label.lowercased())…"
        case .refresh:
            return "\(kind.label)…"
        }
    }

    private var executingMessage: String {
        "\(kind.label)…"
    }

    @ViewBuilder
    private var footer: some View {
        HStack {
            Spacer()
            switch phase {
            case .plan:
                Button("Cancel") { onCancel?() ?? onClose() }
                    .keyboardShortcut(.cancelAction)
                Button("Approve") { onApprove?() }
                    .keyboardShortcut(.defaultAction)
            case .loading, .executing:
                Button("Done") { onClose() }
                    .disabled(true)
            case .successConfig, .successDisconnect, .failure:
                Button("Done") { onClose() }
                    .keyboardShortcut(.defaultAction)
            }
        }
    }

    @ViewBuilder
    private func busyRow(message: String) -> some View {
        HStack(spacing: 10) {
            ProgressView()
                .controlSize(.small)
            Text(message)
                .foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.vertical, 4)
    }

    @ViewBuilder
    private func planBody(_ steps: [HarnessPlanStep], mark: PlanLineMark, heading: String) -> some View {
        if steps.isEmpty {
            Text("Nothing to change.")
                .font(.system(size: 11))
                .foregroundStyle(.secondary)
        } else {
            VStack(alignment: .leading, spacing: 8) {
                Text(heading)
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
                ForEach(steps) { step in
                    HStack(alignment: .top, spacing: 8) {
                        planMarkView(mark)
                            .frame(width: 14, height: 14)
                            .padding(.top, 2)
                        VStack(alignment: .leading, spacing: 2) {
                            HStack(spacing: 6) {
                                Text(step.action.uppercased())
                                    .font(.system(size: 9, weight: .bold))
                                    .foregroundStyle(actionTint(step.action))
                                    .padding(.horizontal, 5)
                                    .padding(.vertical, 1)
                                    .background(actionTint(step.action).opacity(0.12), in: Capsule())
                                Text(step.detail)
                                    .font(.system(size: 11))
                                    .lineLimit(2)
                            }
                            Text(step.path)
                                .font(.system(size: 10, design: .monospaced))
                                .foregroundStyle(.secondary)
                                .lineLimit(1)
                                .truncationMode(.middle)
                                .textSelection(.enabled)
                                .help(step.path)
                        }
                    }
                }
            }
        }
    }

    @ViewBuilder
    private func planMarkView(_ mark: PlanLineMark) -> some View {
        switch mark {
        case .none:
            Image(systemName: "circle")
                .font(.system(size: 11))
                .foregroundStyle(.tertiary)
        case .spinning:
            ProgressView()
                .controlSize(.mini)
        case .ok:
            Image(systemName: "checkmark.circle.fill")
                .font(.system(size: 12))
                .foregroundStyle(.green)
        case .error:
            Image(systemName: "xmark.circle.fill")
                .font(.system(size: 12))
                .foregroundStyle(.red)
        }
    }

    private func actionTint(_ action: String) -> Color {
        switch action.lowercased() {
        case "create": .green
        case "delete": .orange
        default: .accentColor
        }
    }

    @ViewBuilder
    private func configSuccessBody(_ result: HarnessConfigWriteResponse) -> some View {
        HarnessConfigWriteSummaryView(result: result)
    }

    @ViewBuilder
    private func disconnectSuccessBody(_ result: HarnessDisconnectResponse) -> some View {
        HarnessDisconnectSummaryView(result: result)
    }
}

// MARK: - Shared summary blocks (single + multi update)

struct HarnessConfigWriteSummaryView: View {
    let result: HarnessConfigWriteResponse
    private let listCap = 12

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            labeled("Path") {
                Text(result.path)
                    .font(.system(size: 11, design: .monospaced))
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .textSelection(.enabled)
                    .help(result.path)
            }
            labeled("Summary") {
                Text(summaryLine)
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
                    .textSelection(.enabled)
            }
            labeled("Key") {
                Text(keyLabel(result.key))
                    .font(.system(size: 11))
            }
            if !result.added.isEmpty {
                modelList(title: "Added", ids: result.added, tint: .green)
            }
            if !result.removed.isEmpty {
                modelList(title: "Removed", ids: result.removed, tint: .orange)
            }
            if result.added.isEmpty && result.removed.isEmpty {
                Text("Model list unchanged.")
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
            }
        }
    }

    private var summaryLine: String {
        var parts = ["\(result.modelsTotal) models"]
        if !result.added.isEmpty { parts.append("\(result.added.count) added") }
        if !result.removed.isEmpty { parts.append("\(result.removed.count) removed") }
        parts.append("\(result.unchanged) unchanged")
        if !result.baseUrl.isEmpty { parts.append(result.baseUrl) }
        return parts.joined(separator: " · ")
    }

    private func keyLabel(_ key: String) -> String {
        switch key {
        case "reused": return "Reused existing harness key"
        case "minted": return "Minted new harness key"
        case "revoked": return "Revoked harness key(s)"
        case "none": return "No harness key change"
        default: return key
        }
    }

    @ViewBuilder
    private func labeled(_ title: String, @ViewBuilder content: () -> some View) -> some View {
        VStack(alignment: .leading, spacing: 3) {
            Text(title)
                .font(.system(size: 10, weight: .semibold))
                .foregroundStyle(.secondary)
            content()
        }
    }

    @ViewBuilder
    private func modelList(title: String, ids: [String], tint: Color) -> some View {
        let shown = Array(ids.prefix(listCap))
        let overflow = max(0, ids.count - listCap)
        VStack(alignment: .leading, spacing: 3) {
            Text("\(title) (\(ids.count))")
                .font(.system(size: 10, weight: .semibold))
                .foregroundStyle(tint)
            ForEach(shown, id: \.self) { id in
                Text(id)
                    .font(.system(size: 11, design: .monospaced))
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .textSelection(.enabled)
            }
            if overflow > 0 {
                Text("+\(overflow) more")
                    .font(.system(size: 10))
                    .foregroundStyle(.secondary)
            }
        }
    }
}

struct HarnessDisconnectSummaryView: View {
    let result: HarnessDisconnectResponse
    private let listCap = 12

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            labeled("Path") {
                Text(result.path)
                    .font(.system(size: 11, design: .monospaced))
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .textSelection(.enabled)
                    .help(result.path)
            }
            labeled("Summary") {
                let parts = [
                    result.wasConnected ? "Provider removed" : "Already removed",
                    "\(result.revoked) key(s) revoked",
                ]
                Text(parts.joined(separator: " · "))
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
            }
            if !result.removed.isEmpty {
                let shown = Array(result.removed.prefix(listCap))
                let overflow = max(0, result.removed.count - listCap)
                VStack(alignment: .leading, spacing: 3) {
                    Text("Removed (\(result.removed.count))")
                        .font(.system(size: 10, weight: .semibold))
                        .foregroundStyle(.orange)
                    ForEach(shown, id: \.self) { id in
                        Text(id)
                            .font(.system(size: 11, design: .monospaced))
                            .lineLimit(1)
                            .truncationMode(.middle)
                    }
                    if overflow > 0 {
                        Text("+\(overflow) more")
                            .font(.system(size: 10))
                            .foregroundStyle(.secondary)
                    }
                }
            }
        }
    }

    @ViewBuilder
    private func labeled(_ title: String, @ViewBuilder content: () -> some View) -> some View {
        VStack(alignment: .leading, spacing: 3) {
            Text(title)
                .font(.system(size: 10, weight: .semibold))
                .foregroundStyle(.secondary)
            content()
        }
    }
}

/// Ensures sheet and standalone presentations are resizable with a floor size.
private struct HarnessActionWindowSizer: NSViewRepresentable {
    func makeNSView(context: Context) -> NSView {
        let view = NSView()
        DispatchQueue.main.async { Self.apply(to: view.window) }
        return view
    }

    func updateNSView(_ nsView: NSView, context: Context) {
        DispatchQueue.main.async { Self.apply(to: nsView.window) }
    }

    private static func apply(to window: NSWindow?) {
        guard let window else { return }
        if !window.styleMask.contains(.resizable) {
            window.styleMask.insert(.resizable)
        }
        let min = HarnessActionResultView.minSize
        if window.minSize.width < min.width || window.minSize.height < min.height {
            window.minSize = NSSize(width: min.width, height: min.height)
        }
    }
}

@MainActor
final class HarnessActionWindowController {
    private var window: NSWindow?
    private var model: HarnessActionWindowModel?
    private var multiModel: MultiHarnessRefreshModel?

    func show(store: SnapshotStore, harness: Harness, kind: HarnessActionKind) {
        window?.close()
        multiModel = nil
        let displayName = HarnessCatalog.displayName(harness.name)
        let model = HarnessActionWindowModel(
            store: store,
            harnessName: harness.name,
            displayName: displayName,
            kind: kind
        )
        self.model = model
        present(
            title: kind.windowTitle,
            root: AnyView(
                HarnessActionWindowRoot(
                    model: model,
                    onClose: { [weak self] in self?.closeWindow() }
                ))
        )
        model.start()
    }

    /// One-click refresh-config for every connected, connect-capable harness.
    func showUpdateAll(store: SnapshotStore) {
        window?.close()
        model = nil
        let multi = MultiHarnessRefreshModel(store: store)
        multiModel = multi
        present(
            title: "Update All Harnesses",
            root: AnyView(
                MultiHarnessRefreshRoot(
                    model: multi,
                    onClose: { [weak self] in self?.closeWindow() }
                ))
        )
        multi.start()
    }

    private func present(title: String, root: AnyView) {
        let host = NSHostingController(rootView: root)
        let win = NSWindow(contentViewController: host)
        win.title = title
        win.styleMask = [.titled, .closable, .resizable, .miniaturizable]
        win.isReleasedWhenClosed = false
        win.minSize = NSSize(
            width: HarnessActionResultView.minSize.width,
            height: HarnessActionResultView.minSize.height)
        win.setContentSize(NSSize(
            width: HarnessActionResultView.idealSize.width,
            height: HarnessActionResultView.idealSize.height))
        win.center()
        window = win
        DockIconManager.shared.track(win)
        NSApp.activate(ignoringOtherApps: true)
        win.makeKeyAndOrderFront(nil)
    }

    private func closeWindow() {
        window?.close()
        window = nil
        model = nil
        multiModel = nil
    }
}

@MainActor
@Observable
final class HarnessActionWindowModel {
    let store: SnapshotStore
    let harnessName: String
    let displayName: String
    let kind: HarnessActionKind
    private(set) var phase: HarnessActionPhase = .loading

    init(store: SnapshotStore, harnessName: String, displayName: String, kind: HarnessActionKind) {
        self.store = store
        self.harnessName = harnessName
        self.displayName = displayName
        self.kind = kind
    }

    func start() {
        phase = .loading
        guard let config = store.config else {
            phase = .failure([], "Daemon configuration unavailable")
            return
        }
        let client = AlexandriaClient(config: config)
        let name = harnessName
        let kind = kind
        Task { [weak self] in
            do {
                switch kind {
                case .refresh:
                    self?.phase = .executing([])
                    let result = try await client.refreshHarnessConfig(name)
                    await self?.store.refresh()
                    self?.phase = .successConfig([], result)
                case .connect:
                    let plan = try await client.connectHarnessPlan(name)
                    self?.phase = .plan(plan.plan)
                case .disconnect:
                    let plan = try await client.disconnectHarnessPlan(name)
                    self?.phase = .plan(plan.plan)
                }
            } catch {
                self?.phase = .failure([], error.localizedDescription)
            }
        }
    }

    func approve() {
        guard case .plan(let steps) = phase else { return }
        guard let config = store.config else {
            phase = .failure(steps, "Daemon configuration unavailable")
            return
        }
        phase = .executing(steps)
        let client = AlexandriaClient(config: config)
        let name = harnessName
        let kind = kind
        Task { [weak self] in
            do {
                switch kind {
                case .connect:
                    let result = try await client.connectHarness(name)
                    await self?.store.refresh()
                    self?.phase = .successConfig(steps, result)
                case .disconnect:
                    let result = try await client.disconnectHarness(name)
                    await self?.store.refresh()
                    self?.phase = .successDisconnect(steps, result)
                case .refresh:
                    break
                }
            } catch {
                self?.phase = .failure(steps, error.localizedDescription)
            }
        }
    }
}

private struct HarnessActionWindowRoot: View {
    @Bindable var model: HarnessActionWindowModel
    let onClose: () -> Void

    var body: some View {
        HarnessActionResultView(
            kind: model.kind,
            harnessDisplayName: model.displayName,
            phase: model.phase,
            onApprove: { model.approve() },
            onCancel: onClose,
            onClose: onClose
        )
    }
}

// MARK: - Update All Harnesses

enum MultiHarnessItemStatus: Equatable {
    case pending
    case running
    case success(HarnessConfigWriteResponse)
    case failure(String)
}

struct MultiHarnessRefreshItem: Identifiable, Equatable {
    let name: String
    let displayName: String
    var status: MultiHarnessItemStatus

    var id: String { name }
}

@MainActor
@Observable
final class MultiHarnessRefreshModel {
    let store: SnapshotStore
    private(set) var items: [MultiHarnessRefreshItem] = []
    private(set) var finished = false
    private(set) var started = false

    var isBusy: Bool { started && !finished }

    var updatedCount: Int {
        items.reduce(0) {
            if case .success = $1.status { return $0 + 1 }
            return $0
        }
    }

    var failedCount: Int {
        items.reduce(0) {
            if case .failure = $1.status { return $0 + 1 }
            return $0
        }
    }

    var totalsLine: String {
        "\(updatedCount) updated, \(failedCount) failed"
    }

    init(store: SnapshotStore) {
        self.store = store
    }

    func start() {
        started = true
        finished = false
        let targets = HarnessCatalog.refreshTargets(store.harnesses)
        items = targets.map {
            MultiHarnessRefreshItem(
                name: $0.name,
                displayName: HarnessCatalog.displayName($0.name),
                status: .pending)
        }
        guard let config = store.config else {
            if items.isEmpty {
                items = [
                    MultiHarnessRefreshItem(
                        name: "_",
                        displayName: "Harnesses",
                        status: .failure("Daemon configuration unavailable")),
                ]
            } else {
                for i in items.indices {
                    items[i].status = .failure("Daemon configuration unavailable")
                }
            }
            finished = true
            return
        }
        guard !items.isEmpty else {
            finished = true
            return
        }
        let client = AlexandriaClient(config: config)
        let names = items.map(\.name)
        Task { [weak self] in
            for name in names {
                guard let self else { return }
                if let idx = self.items.firstIndex(where: { $0.name == name }) {
                    self.items[idx].status = .running
                }
                do {
                    let result = try await client.refreshHarnessConfig(name)
                    if let idx = self.items.firstIndex(where: { $0.name == name }) {
                        self.items[idx].status = .success(result)
                    }
                } catch {
                    if let idx = self.items.firstIndex(where: { $0.name == name }) {
                        self.items[idx].status = .failure(error.localizedDescription)
                    }
                }
            }
            await self?.store.refresh()
            self?.finished = true
        }
    }
}

struct MultiHarnessRefreshResultView: View {
    let items: [MultiHarnessRefreshItem]
    let finished: Bool
    let totalsLine: String
    let onClose: () -> Void

    var body: some View {
        VStack(spacing: 0) {
            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    Text("Update All Harnesses")
                        .font(.headline)
                    if items.isEmpty {
                        if finished {
                            Text("No connected harnesses support config updates.")
                                .font(.system(size: 11))
                                .foregroundStyle(.secondary)
                        } else {
                            HStack(spacing: 10) {
                                ProgressView().controlSize(.small)
                                Text("Looking for connected harnesses…")
                                    .foregroundStyle(.secondary)
                            }
                        }
                    } else {
                        ForEach(items) { item in
                            multiSection(item)
                        }
                    }
                    if finished {
                        Text(totalsLine)
                            .font(.system(size: 12, weight: .semibold))
                            .padding(.top, 4)
                    }
                }
                .frame(maxWidth: .infinity, alignment: .topLeading)
                .padding(18)
            }
            Divider()
            HStack {
                Spacer()
                Button("Done") { onClose() }
                    .keyboardShortcut(.defaultAction)
                    .disabled(!finished)
            }
            .padding(.horizontal, 18)
            .padding(.vertical, 12)
            .frame(maxWidth: .infinity)
            .background(.bar)
        }
        .frame(
            minWidth: HarnessActionResultView.minSize.width,
            idealWidth: HarnessActionResultView.idealSize.width,
            minHeight: HarnessActionResultView.minSize.height,
            idealHeight: HarnessActionResultView.idealSize.height,
            alignment: .topLeading
        )
        .background(HarnessActionWindowSizer())
    }

    @ViewBuilder
    private func multiSection(_ item: MultiHarnessRefreshItem) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
                statusIcon(item.status)
                Text(item.displayName)
                    .font(.system(size: 13, weight: .semibold))
            }
            switch item.status {
            case .pending:
                Text("Waiting…")
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
            case .running:
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text("\(HarnessActionKind.refresh.label)…")
                        .font(.system(size: 11))
                        .foregroundStyle(.secondary)
                }
            case .success(let result):
                HarnessConfigWriteSummaryView(result: result)
            case .failure(let message):
                Text(message)
                    .font(.system(size: 11))
                    .foregroundStyle(.red)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .padding(.vertical, 4)
    }

    @ViewBuilder
    private func statusIcon(_ status: MultiHarnessItemStatus) -> some View {
        switch status {
        case .pending:
            Image(systemName: "circle")
                .font(.system(size: 12))
                .foregroundStyle(.tertiary)
        case .running:
            ProgressView()
                .controlSize(.mini)
                .frame(width: 14, height: 14)
        case .success:
            Image(systemName: "checkmark.circle.fill")
                .font(.system(size: 13))
                .foregroundStyle(.green)
        case .failure:
            Image(systemName: "xmark.circle.fill")
                .font(.system(size: 13))
                .foregroundStyle(.red)
        }
    }
}

private struct MultiHarnessRefreshRoot: View {
    @Bindable var model: MultiHarnessRefreshModel
    let onClose: () -> Void

    var body: some View {
        MultiHarnessRefreshResultView(
            items: model.items,
            finished: model.finished,
            totalsLine: model.totalsLine,
            onClose: onClose
        )
    }
}

// MARK: - Sheet-friendly model for Preferences

@MainActor
@Observable
final class MultiHarnessRefreshSheetModel: Identifiable {
    let id = UUID()
    let store: SnapshotStore
    let model: MultiHarnessRefreshModel

    init(store: SnapshotStore) {
        self.store = store
        self.model = MultiHarnessRefreshModel(store: store)
    }

    func start() { model.start() }
}

@MainActor
@Observable
final class HarnessActionSheetModel: Identifiable {
    let id = UUID()
    let store: SnapshotStore
    let harness: Harness
    let kind: HarnessActionKind
    private(set) var phase: HarnessActionPhase = .loading

    var displayName: String { HarnessCatalog.displayName(harness.name) }

    init(store: SnapshotStore, harness: Harness, kind: HarnessActionKind) {
        self.store = store
        self.harness = harness
        self.kind = kind
    }

    func start() {
        phase = .loading
        guard let config = store.config else {
            phase = .failure([], "Daemon configuration unavailable")
            return
        }
        let client = AlexandriaClient(config: config)
        let name = harness.name
        let kind = kind
        Task { [weak self] in
            do {
                switch kind {
                case .refresh:
                    self?.phase = .executing([])
                    let result = try await client.refreshHarnessConfig(name)
                    await self?.store.refresh()
                    self?.phase = .successConfig([], result)
                case .connect:
                    let plan = try await client.connectHarnessPlan(name)
                    self?.phase = .plan(plan.plan)
                case .disconnect:
                    let plan = try await client.disconnectHarnessPlan(name)
                    self?.phase = .plan(plan.plan)
                }
            } catch {
                self?.phase = .failure([], error.localizedDescription)
            }
        }
    }

    func approve() {
        guard case .plan(let steps) = phase else { return }
        guard let config = store.config else {
            phase = .failure(steps, "Daemon configuration unavailable")
            return
        }
        phase = .executing(steps)
        let client = AlexandriaClient(config: config)
        let name = harness.name
        let kind = kind
        Task { [weak self] in
            do {
                switch kind {
                case .connect:
                    let result = try await client.connectHarness(name)
                    await self?.store.refresh()
                    self?.phase = .successConfig(steps, result)
                case .disconnect:
                    let result = try await client.disconnectHarness(name)
                    await self?.store.refresh()
                    self?.phase = .successDisconnect(steps, result)
                case .refresh:
                    break
                }
            } catch {
                self?.phase = .failure(steps, error.localizedDescription)
            }
        }
    }
}
