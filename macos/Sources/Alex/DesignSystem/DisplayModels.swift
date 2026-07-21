import SwiftUI

/// Plain value inputs for the design-system components. These deliberately do
/// not reference the networking models so components stay decoupled; richer
/// fields (status, durations, exit codes) are optional and render when present.
enum DisplayStatus: String, Equatable, Sendable {
    case success
    case error
    case running
    case pending

    var tint: Color {
        switch self {
        case .success: AlexTheme.Colors.success
        case .error: AlexTheme.Colors.destructive
        case .running: AlexTheme.Colors.warning
        case .pending: AlexTheme.Colors.textTertiary  // #636366 (shared.tsx:143)
        }
    }

    var systemImage: String {
        switch self {
        case .success: "checkmark.circle.fill"
        case .error: "xmark.circle.fill"
        case .running: "ellipsis.circle.fill"
        case .pending: "circle"
        }
    }
}

struct ToolCallDisplay: Identifiable, Equatable, Sendable {
    let id: String
    let name: String
    var argumentPreview: String?
    var input = ""
    var output: String?
    var status: DisplayStatus?
    var statusText: String?
    var durationText: String?
    var exitStatus: Int?
    var hasArgsBody = false
    var hasResultBody = false

    var iconSystemName: String {
        switch name.lowercased() {
        case "read", "notebookread": "doc.text"
        case "glob", "search", "websearch": "magnifyingglass"
        case "grep": "number"
        case "edit", "write", "multiedit", "notebookedit":
            "chevron.left.forwardslash.chevron.right"
        case "bash", "shell", "exec", "terminal": "terminal"
        case "task", "agent": "arrow.triangle.branch"
        case "webfetch", "fetch": "globe"
        default: "wrench.adjustable"
        }
    }
}

struct SubagentDisplay: Identifiable, Equatable, Sendable {
    let id: String
    let traceId: String
    var model: String?
    var prompt = ""
    var status: DisplayStatus = .running
    var durationText: String?
    var turnCount: Int?
}

struct MessageDisplay: Identifiable, Equatable, Sendable {
    enum Role: Equatable, Sendable {
        case user
        case assistant
        /// A tool result fed back to the model by the harness — structurally
        /// the "user" slot of the turn, but authored by the harness, not the
        /// user (see `TurnHeader.harnessResultLabel`).
        case harness
    }

    let id: String
    /// The trace this message belongs to; used for selection and deep links.
    let turnId: String
    let role: Role
    var roleLabel: String
    var content = ""
    var isMonospaced = false
    var model: String?
    /// Extra header metadata (reasoning effort, billing bucket, …).
    var detail: String?
    var timestamp: String?
    var tokenText: String?
    var toolCalls: [ToolCallDisplay] = []
    var subagent: SubagentDisplay?
    var error: String?
    var event: String?
}
