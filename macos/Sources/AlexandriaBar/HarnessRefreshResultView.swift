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
    var toolCapture: Binding<Bool>? = nil
    var captureWarning: String? = nil
    var onApprove: (() -> Void)? = nil
    var onCancel: (() -> Void)? = nil
    let onClose: () -> Void

    static let minSize = CGSize(width: 420, height: 360)
    static let idealSize = CGSize(width: 480, height: 420)

    var body: some View {
        VStack(spacing: 0) {
            PanelHeader {
                VStack(alignment: .leading, spacing: 2) {
                    Text(title)
                        .font(.system(size: 13, weight: .semibold))
                        .foregroundStyle(AlexTheme.Colors.foreground)
                        .lineLimit(1)
                    if !harnessDisplayName.isEmpty {
                        Text(harnessDisplayName)
                            .font(.system(size: 11))
                            .foregroundStyle(AlexTheme.Colors.textTertiary)
                            .lineLimit(1)
                    }
                }
            } right: {
                phaseChip
            }
            ScrollView {
                VStack(alignment: .leading, spacing: 14) {
                    content
                }
                .frame(maxWidth: .infinity, alignment: .topLeading)
                .padding(.horizontal, 16)
                .padding(.vertical, 14)
            }
            footer
        }
        .background(AlexTheme.Colors.background)
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

    private var phaseChip: StatusChip {
        switch phase {
        case .loading:
            StatusChip(status: .pending, text: "planning")
        case .plan:
            StatusChip(status: .pending, text: "awaiting approval")
        case .executing:
            StatusChip(status: .running, text: "running")
        case .successConfig, .successDisconnect:
            StatusChip(status: .success, text: "done")
        case .failure:
            StatusChip(status: .error, text: "failed")
        }
    }

    @ViewBuilder
    private var content: some View {
        switch phase {
        case .loading:
            busyRow(message: loadingMessage)
        case .plan(let steps):
            planBody(steps, mark: .none, heading: "This will")
        case .executing(let steps):
            VStack(alignment: .leading, spacing: 10) {
                busyRow(message: executingMessage)
                planBody(steps, mark: .spinning, heading: "In progress")
            }
        case .successConfig(let steps, let result):
            VStack(alignment: .leading, spacing: 12) {
                if !steps.isEmpty {
                    planBody(steps, mark: .ok, heading: "Completed")
                }
                configSuccessBody(result)
                if let captureWarning {
                    Text(captureWarning)
                        .font(.system(size: 11))
                        .foregroundStyle(AlexTheme.Colors.warningOrange)
                        .textSelection(.enabled)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
        case .successDisconnect(let steps, let result):
            VStack(alignment: .leading, spacing: 12) {
                if !steps.isEmpty {
                    planBody(steps, mark: .ok, heading: "Completed")
                }
                disconnectSuccessBody(result)
            }
        case .failure(let steps, let message):
            VStack(alignment: .leading, spacing: 10) {
                if !steps.isEmpty {
                    planBody(steps, mark: .error, heading: "Failed")
                }
                Text(message)
                    .font(.system(size: 11))
                    .foregroundStyle(AlexTheme.Colors.destructive)
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

    /// True once the action has finished (success or failure) and the capture choice is locked in.
    private var isSettled: Bool {
        switch phase {
        case .successConfig, .successDisconnect, .failure: true
        case .loading, .plan, .executing: false
        }
    }

    @ViewBuilder
    private var footer: some View {
        HStack(spacing: AlexTheme.Spacing.md) {
            if let toolCapture {
                Toggle(isOn: toolCapture) {
                    Text("Capture tool calls")
                        .font(.system(size: 11))
                        .foregroundStyle(AlexTheme.Colors.textSecondary)
                }
                .toggleStyle(.switch)
                .controlSize(.mini)
                .disabled(isSettled)
                .help("Opt in to storing this harness's tool arguments and results locally. Secrets are redacted before storage.")
            }
            Spacer()
            switch phase {
            case .plan:
                PillButton(
                    title: "Cancel", variant: .standard,
                    horizontalPadding: 12, verticalPadding: 5, cornerRadius: 6,
                    keyboardShortcut: .cancelAction
                ) {
                    onCancel?() ?? onClose()
                }
                PillButton(
                    title: "Approve", variant: .solidAccent,
                    keyboardShortcut: .defaultAction
                ) {
                    onApprove?()
                }
            case .loading, .executing:
                PillButton(title: "Done", variant: .solidAccent, isEnabled: false) {}
            case .successConfig, .successDisconnect, .failure:
                PillButton(
                    title: "Done", variant: .solidAccent,
                    keyboardShortcut: .defaultAction
                ) {
                    onClose()
                }
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 12)
        .frame(maxWidth: .infinity)
        .overlay(alignment: .top) {
            Rectangle().fill(AlexTheme.Colors.cardBorder).frame(height: 1)
        }
    }

    @ViewBuilder
    private func busyRow(message: String) -> some View {
        HStack(spacing: 10) {
            ProgressView()
                .controlSize(.small)
            Text(message)
                .font(.system(size: 12))
                .foregroundStyle(AlexTheme.Colors.textSecondary)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.vertical, 4)
    }

    @ViewBuilder
    private func planBody(_ steps: [HarnessPlanStep], mark: PlanLineMark, heading: String) -> some View {
        if steps.isEmpty {
            EmptyStateView(message: "Nothing to change")
        } else {
            VStack(alignment: .leading, spacing: 8) {
                SectionLabel(text: heading, style: .prominent)
                ForEach(steps) { step in
                    if step.action == "about" {
                        HStack(alignment: .top, spacing: 8) {
                            Image(systemName: "info.circle.fill")
                                .foregroundStyle(AlexTheme.Colors.primary)
                                .padding(.top, 1)
                            Text(step.detail)
                                .font(.system(size: 11))
                                .foregroundStyle(AlexTheme.Colors.foreground)
                                .fixedSize(horizontal: false, vertical: true)
                                .textSelection(.enabled)
                        }
                        .padding(10)
                        .background(
                            AlexTheme.Colors.primary.opacity(0.08),
                            in: RoundedRectangle(cornerRadius: AlexTheme.Radius.md))
                    } else {
                        HStack(alignment: .top, spacing: 8) {
                            planMarkView(mark)
                                .frame(width: 14, height: 14)
                                .padding(.top, 2)
                            VStack(alignment: .leading, spacing: 2) {
                                HStack(spacing: 6) {
                                    StatusChip(
                                        tint: actionTint(step.action),
                                        text: step.action.uppercased(),
                                        style: .mini)
                                    Text(step.detail)
                                        .font(.system(size: 11))
                                        .foregroundStyle(AlexTheme.Colors.foreground)
                                        .lineLimit(2)
                                }
                                Text(step.path)
                                    .font(AlexTheme.Fonts.metaMicro)
                                    .foregroundStyle(AlexTheme.Colors.textTertiary)
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
    }

    @ViewBuilder
    private func planMarkView(_ mark: PlanLineMark) -> some View {
        switch mark {
        case .none:
            Image(systemName: "circle")
                .font(.system(size: 11))
                .foregroundStyle(AlexTheme.Colors.textFaint)
        case .spinning:
            ProgressView()
                .controlSize(.mini)
        case .ok:
            Image(systemName: "checkmark.circle.fill")
                .font(.system(size: 12))
                .foregroundStyle(AlexTheme.Colors.success)
        case .error:
            Image(systemName: "xmark.circle.fill")
                .font(.system(size: 12))
                .foregroundStyle(AlexTheme.Colors.destructive)
        }
    }

    private func actionTint(_ action: String) -> Color {
        switch action.lowercased() {
        case "create": AlexTheme.Colors.success
        case "delete": AlexTheme.Colors.warningOrange
        default: AlexTheme.Colors.primary
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

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            if let description = result.description, !description.isEmpty {
                HStack(alignment: .top, spacing: 8) {
                    Image(systemName: "info.circle.fill")
                        .foregroundStyle(AlexTheme.Colors.primary)
                        .padding(.top, 1)
                    Text(description)
                        .font(.system(size: 11))
                        .foregroundStyle(AlexTheme.Colors.foreground)
                        .fixedSize(horizontal: false, vertical: true)
                        .textSelection(.enabled)
                }
                .padding(10)
                .background(
                    AlexTheme.Colors.primary.opacity(0.08),
                    in: RoundedRectangle(cornerRadius: AlexTheme.Radius.md))
            }
            HStack(spacing: AlexTheme.Spacing.sm) {
                if !result.added.isEmpty {
                    StatusChip(
                        tint: AlexTheme.Colors.success,
                        text: "+\(result.added.count) added")
                }
                if !result.removed.isEmpty {
                    StatusChip(
                        tint: AlexTheme.Colors.warningOrange,
                        text: "−\(result.removed.count) removed")
                }
                Text(summaryLine)
                    .font(.system(size: 11))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
                    .textSelection(.enabled)
            }
            CollapsibleSection(title: "Config Summary", defaultOpen: true) {
                VStack(alignment: .leading, spacing: 8) {
                    labeled("Path") {
                        Text(result.path)
                            .font(AlexTheme.Fonts.metaLabel)
                            .foregroundStyle(AlexTheme.Colors.textSecondary)
                            .lineLimit(1)
                            .truncationMode(.middle)
                            .textSelection(.enabled)
                            .help(result.path)
                    }
                    labeled("Key") {
                        Text(keyLabel(result.key))
                            .font(.system(size: 11))
                            .foregroundStyle(AlexTheme.Colors.textSecondary)
                    }
                    if !result.added.isEmpty {
                        modelList(
                            title: "Added", ids: result.added,
                            tint: AlexTheme.Colors.success)
                    }
                    if !result.removed.isEmpty {
                        modelList(
                            title: "Removed", ids: result.removed,
                            tint: AlexTheme.Colors.warningOrange)
                    }
                    if result.added.isEmpty && result.removed.isEmpty {
                        Text(result.modelsTotal == 0 ? "No model catalog changes." : "Model list unchanged.")
                            .font(.system(size: 11))
                            .foregroundStyle(AlexTheme.Colors.textTertiary)
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 12)
                .padding(.bottom, 10)
            }
        }
    }

    private var summaryLine: String {
        var parts = result.modelsTotal == 0 ? ["Lifecycle integration"] : ["\(result.modelsTotal) models"]
        if result.modelsTotal > 0 { parts.append("\(result.unchanged) unchanged") }
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
            SectionLabel(text: title)
            content()
        }
    }

    @ViewBuilder
    private func modelList(title: String, ids: [String], tint: Color) -> some View {
        VStack(alignment: .leading, spacing: 3) {
            SectionLabel(text: "\(title) (\(ids.count))")
            ForEach(ids, id: \.self) { id in
                HStack(alignment: .firstTextBaseline, spacing: AlexTheme.Spacing.sm) {
                    StatusDot(tint: tint, size: 5)
                        .padding(.top, 1)
                    Text(id)
                        .font(AlexTheme.Fonts.metaLabel)
                        .foregroundStyle(AlexTheme.Colors.textSecondary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                        .textSelection(.enabled)
                }
            }
        }
    }
}

struct HarnessDisconnectSummaryView: View {
    let result: HarnessDisconnectResponse

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(summaryLine)
                .font(.system(size: 11))
                .foregroundStyle(AlexTheme.Colors.textTertiary)
                .textSelection(.enabled)
            CollapsibleSection(title: "Removal Summary", defaultOpen: true) {
                VStack(alignment: .leading, spacing: 8) {
                    labeled("Path") {
                        Text(result.path)
                            .font(AlexTheme.Fonts.metaLabel)
                            .foregroundStyle(AlexTheme.Colors.textSecondary)
                            .lineLimit(1)
                            .truncationMode(.middle)
                            .textSelection(.enabled)
                            .help(result.path)
                    }
                    if !result.removed.isEmpty {
                        VStack(alignment: .leading, spacing: 3) {
                            SectionLabel(text: "Removed (\(result.removed.count))")
                            ForEach(result.removed, id: \.self) { id in
                                HStack(alignment: .firstTextBaseline, spacing: AlexTheme.Spacing.sm) {
                                    StatusDot(tint: AlexTheme.Colors.warningOrange, size: 5)
                                        .padding(.top, 1)
                                    Text(id)
                                        .font(AlexTheme.Fonts.metaLabel)
                                        .foregroundStyle(AlexTheme.Colors.textSecondary)
                                        .lineLimit(1)
                                        .truncationMode(.middle)
                                }
                            }
                        }
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 12)
                .padding(.bottom, 10)
            }
        }
    }

    private var summaryLine: String {
        [
            result.wasConnected ? "Harness integration removed" : "Already removed",
            "\(result.revoked) key(s) revoked",
        ].joined(separator: " · ")
    }

    @ViewBuilder
    private func labeled(_ title: String, @ViewBuilder content: () -> some View) -> some View {
        VStack(alignment: .leading, spacing: 3) {
            SectionLabel(text: title)
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
            PanelHeader {
                Text("Update All Harnesses")
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(AlexTheme.Colors.foreground)
                if !items.isEmpty {
                    PanelCountBadge(count: items.count)
                }
            } right: {
                overallChip
            }
            ScrollView {
                VStack(alignment: .leading, spacing: 12) {
                    if items.isEmpty {
                        if finished {
                            EmptyStateView(
                                message: "No connected harnesses support config updates",
                                style: .card)
                        } else {
                            HStack(spacing: 10) {
                                ProgressView().controlSize(.small)
                                Text("Looking for connected harnesses…")
                                    .font(.system(size: 12))
                                    .foregroundStyle(AlexTheme.Colors.textSecondary)
                            }
                        }
                    } else {
                        ForEach(items) { item in
                            multiSection(item)
                        }
                    }
                    if finished, !items.isEmpty {
                        Text(totalsLine)
                            .font(.system(size: 12, weight: .semibold))
                            .foregroundStyle(AlexTheme.Colors.foreground)
                            .padding(.top, 2)
                    }
                }
                .frame(maxWidth: .infinity, alignment: .topLeading)
                .padding(.horizontal, 16)
                .padding(.vertical, 14)
            }
            HStack {
                Spacer()
                PillButton(
                    title: "Done", variant: .solidAccent, isEnabled: finished,
                    keyboardShortcut: .defaultAction
                ) {
                    onClose()
                }
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 12)
            .frame(maxWidth: .infinity)
            .overlay(alignment: .top) {
                Rectangle().fill(AlexTheme.Colors.cardBorder).frame(height: 1)
            }
        }
        .background(AlexTheme.Colors.background)
        .frame(
            minWidth: HarnessActionResultView.minSize.width,
            idealWidth: HarnessActionResultView.idealSize.width,
            minHeight: HarnessActionResultView.minSize.height,
            idealHeight: HarnessActionResultView.idealSize.height,
            alignment: .topLeading
        )
        .background(HarnessActionWindowSizer())
    }

    private var overallChip: StatusChip {
        if !finished {
            return StatusChip(status: .running, text: "updating")
        }
        let failed = items.reduce(0) {
            if case .failure = $1.status { return $0 + 1 }
            return $0
        }
        return failed > 0
            ? StatusChip(status: .error, text: "\(failed) failed")
            : StatusChip(status: .success, text: "done")
    }

    @ViewBuilder
    private func multiSection(_ item: MultiHarnessRefreshItem) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: AlexTheme.Spacing.md) {
                HarnessIconView(harness: item.name, tags: nil, size: 17, showsFallback: true)
                Text(item.displayName)
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(AlexTheme.Colors.foreground)
                Spacer()
                itemChip(item.status)
            }
            switch item.status {
            case .pending:
                Text("Waiting…")
                    .font(.system(size: 11))
                    .foregroundStyle(AlexTheme.Colors.textTertiary)
            case .running:
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text("\(HarnessActionKind.refresh.label)…")
                        .font(.system(size: 11))
                        .foregroundStyle(AlexTheme.Colors.textSecondary)
                }
            case .success(let result):
                HarnessConfigWriteSummaryView(result: result)
            case .failure(let message):
                Text(message)
                    .font(.system(size: 11))
                    .foregroundStyle(AlexTheme.Colors.destructive)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .alexCard(radius: AlexTheme.Radius.lg)
    }

    private func itemChip(_ status: MultiHarnessItemStatus) -> StatusChip {
        switch status {
        case .pending: StatusChip(status: .pending)
        case .running: StatusChip(status: .running)
        case .success: StatusChip(status: .success, text: "updated")
        case .failure: StatusChip(status: .error, text: "failed")
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
    var captureToolCalls: Bool
    private(set) var captureWarning: String?

    var displayName: String { HarnessCatalog.displayName(harness.name) }

    var showsToolCapture: Bool {
        kind != .disconnect && HarnessCatalog.toolCaptureHarnesses.contains(harness.name)
    }

    init(store: SnapshotStore, harness: Harness, kind: HarnessActionKind) {
        self.store = store
        self.harness = harness
        self.kind = kind
        self.captureToolCalls = (harness.connected ? harness.toolCaptureEnabled : nil) ?? true
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
                    await self?.applyToolCaptureIfNeeded(client)
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
                    await self?.applyToolCaptureIfNeeded(client)
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

    /// Applies the sheet's "Capture tool calls" choice after a successful connect/refresh.
    /// Failures never fail the action itself; they surface as a secondary warning.
    private func applyToolCaptureIfNeeded(_ client: AlexandriaClient) async {
        guard showsToolCapture, captureToolCalls != (harness.toolCaptureEnabled ?? false) else { return }
        do {
            try await client.setHarnessToolCapture(harness.name, enabled: captureToolCalls)
        } catch {
            captureWarning =
                "Tool capture setting was not applied: \(error.localizedDescription)"
        }
    }
}
