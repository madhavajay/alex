import { useState, useRef, useEffect } from "react";
import {
  ChevronDown, ChevronRight, GitBranch, Zap, Bot, User,
  MoreHorizontal, ArrowUpRight, Layers, Clock, Activity,
} from "lucide-react";
import {
  C, MODEL_COLORS,
  truncate, formatJson, formatDur, formatCost, toolIcon,
  StatusDot, StatusIcon, ModelBadge, HarnessIcon, ProviderBadge,
  TabBar, SearchInput, CopyButton,
  PanelHeader, FilterRow, CollapsibleSection, JsonBlock, KVTable,
} from "./components/shared";
import type { Status, Role, HarnessType, Provider, ToolCall, SubagentCall, Message, Session, TraceEvent } from "./components/shared";

// ─── Mock data ────────────────────────────────────────────────────────────────

const SESSIONS: Session[] = [
  {
    id: "A3F8B2D1-7c4e-4f9b-a821-2d9c1e0f5a8b", shortId: "A3F8B2D1",
    status: "success", harness: "claude-code", model: "claude-opus-4-8", provider: "anthropic",
    turns: 7, cost: 0.84, duration: 42100, account: "prod-api", timestamp: "14:23",
    children: [{
      id: "B7F2A9C1-admin-routes", shortId: "B7F2A9C1",
      status: "success", harness: "claude-code", model: "claude-sonnet-4-6", provider: "anthropic",
      turns: 6, cost: 0.18, duration: 8420, account: "prod-api", timestamp: "14:23", isSubagent: true,
    }],
  },
  {
    id: "C4D2E8F0-3b1a-4d7c-8e9f-0a2b4c6d8e0f", shortId: "C4D2E8F0",
    status: "error", harness: "api", model: "claude-sonnet-4-6", provider: "anthropic",
    turns: 3, cost: 0.12, duration: 8900, account: "staging", timestamp: "14:21",
  },
  {
    id: "D5E3F1A2-4c2b-5e8d-9f0a-1b3c5d7e9f1a", shortId: "D5E3F1A2",
    status: "running", harness: "langchain", model: "gpt-4o", provider: "openai",
    turns: 2, cost: 0.03, duration: 12300, account: "dev-oai", timestamp: "14:24",
  },
  {
    id: "E6F4A2B3-5d3c-6f9e-0a1b-2c4d6e8f0a2b", shortId: "E6F4A2B3",
    status: "success", harness: "claude-code", model: "claude-haiku-4-5", provider: "anthropic",
    turns: 12, cost: 0.09, duration: 28400, account: "prod-api", timestamp: "14:19",
    children: [{
      id: "F7A5B3C4-6e4d-7a0f-1b2c-3d5e7f9a1b3c", shortId: "F7A5B3C4",
      status: "success", harness: "claude-code", model: "claude-haiku-4-5", provider: "anthropic",
      turns: 4, cost: 0.02, duration: 5100, account: "prod-api", timestamp: "14:19", isSubagent: true,
    }],
  },
  {
    id: "A1B2C3D4-7f5e-8b1a-2c3d-4e6f8a0b2c4d", shortId: "A1B2C3D4",
    status: "success", harness: "custom", model: "claude-opus-4-8", provider: "anthropic",
    turns: 5, cost: 0.61, duration: 31200, account: "prod-api", timestamp: "14:17",
  },
  {
    id: "B2C3D4E5-8a6f-9c2b-3d4e-5f7a9b1c3d5e", shortId: "B2C3D4E5",
    status: "error", harness: "api", model: "claude-sonnet-4-6", provider: "anthropic",
    turns: 1, cost: 0, duration: 1200, account: "staging", timestamp: "14:15",
  },
  {
    id: "C3D4E5F6-9b7a-0c3d-4e5f-6a8b0c2d4e6f", shortId: "C3D4E5F6",
    status: "success", harness: "claude-code", model: "claude-sonnet-4-6", provider: "anthropic",
    turns: 9, cost: 0.27, duration: 19800, account: "prod-api", timestamp: "14:12",
  },
  {
    id: "D4E5F6A7-0c8b-1d4e-5f6a-7b9c1d3e5f7a", shortId: "D4E5F6A7",
    status: "pending", harness: "api", model: "claude-haiku-4-5", provider: "anthropic",
    turns: 0, cost: 0, duration: 0, account: "dev", timestamp: "14:24",
  },
];

const MESSAGES: Message[] = [
  {
    id: "m1", role: "user", timestamp: "14:23:01", tokenCount: 34,
    content: "Can you help me understand the current state of the authentication module and refactor it to use the new JWT middleware?",
  },
  {
    id: "m2", role: "assistant", timestamp: "14:23:02", model: "claude-opus-4-8", tokenCount: 892,
    content: "I'll start by reading the authentication module to understand its current structure, then look at the new JWT middleware implementation.",
    toolCalls: [
      { id: "tc1", name: "Read",  input: { file_path: "/src/auth/middleware.ts" }, output: `import { Request, Response, NextFunction } from 'express';\nimport jwt from 'jsonwebtoken';\n\nexport const legacyAuth = (req, res, next) => {\n  const token = req.headers['x-auth-token'];\n  if (!token) return res.status(401).json({ error: 'No token' });\n  // ...verification logic\n};`, status: "success", duration: 42 },
      { id: "tc2", name: "Glob",  input: { pattern: "src/auth/**/*.ts" }, output: `src/auth/middleware.ts\nsrc/auth/utils.ts\nsrc/auth/types.ts\nsrc/auth/session.ts`, status: "success", duration: 18 },
      { id: "tc3", name: "Read",  input: { file_path: "/src/auth/utils.ts" }, output: `export function verifyToken(token: string) {\n  return jwt.verify(token, process.env.JWT_SECRET!);\n}`, status: "success", duration: 38 },
    ],
  },
  {
    id: "m3", role: "assistant", timestamp: "14:23:05", model: "claude-opus-4-8", tokenCount: 1204,
    content: "I can see the issue — the legacy auth uses a custom `x-auth-token` header and inline verification. The new JWT middleware uses `Authorization: Bearer` and has proper error handling. Let me check if there are any callers before refactoring.",
    toolCalls: [
      { id: "tc4", name: "Grep", input: { pattern: "legacyAuth", glob: "**/*.ts" }, output: `src/routes/user.ts:14\nsrc/routes/admin.ts:8\nsrc/routes/api.ts:22\nsrc/routes/api.ts:47`, status: "success", duration: 156 },
    ],
  },
  {
    id: "m4", role: "assistant", timestamp: "14:23:08", model: "claude-opus-4-8", tokenCount: 1589,
    content: "Found 4 call sites across 3 files. I'll delegate the analysis of the admin routes to a subagent while I handle the user and API routes directly.",
    subagent: {
      id: "sa1", traceId: "B7F2A9C1",
      model: "claude-sonnet-4-6",
      prompt: "Analyze src/routes/admin.ts and update legacyAuth to use jwtMiddleware. Ensure all protected routes use the new middleware and return proper 401 error bodies.",
      status: "success", duration: 8420, turnCount: 6,
    },
  },
  {
    id: "m5", role: "assistant", timestamp: "14:23:09", model: "claude-opus-4-8", tokenCount: 1820,
    content: "While the subagent handles admin routes, I'll update the user and API routes.",
    toolCalls: [
      { id: "tc5", name: "Edit", input: { file_path: "src/routes/user.ts", old_string: "legacyAuth", new_string: "jwtMiddleware" }, output: "File updated successfully.", status: "success", duration: 61 },
      { id: "tc6", name: "Edit", input: { file_path: "src/routes/api.ts",  old_string: "legacyAuth", new_string: "jwtMiddleware({ required: true })" }, output: "File updated successfully.", status: "success", duration: 55 },
      { id: "tc7", name: "Bash", input: { command: "npm test -- --testPathPattern=auth" }, output: `PASS src/auth/middleware.test.ts\nPASS src/auth/jwt.test.ts\nTest Suites: 2 passed\nTests:       14 passed`, status: "success", duration: 3241 },
    ],
  },
  {
    id: "m6", role: "user", timestamp: "14:23:45", tokenCount: 28,
    content: "The tests are passing. Can you also update the API documentation to reflect the new Authorization header format?",
  },
  {
    id: "m7", role: "assistant", timestamp: "14:23:46", model: "claude-opus-4-8", tokenCount: 2103,
    content: "Sure — I'll find and update the API docs to document `Authorization: Bearer <token>` instead of the legacy `x-auth-token` header.",
    toolCalls: [
      { id: "tc8",  name: "Glob", input: { pattern: "docs/**/*.md" }, output: "docs/api/auth.md\ndocs/api/overview.md\ndocs/guides/authentication.md", status: "success", duration: 22 },
      { id: "tc9",  name: "Edit", input: { file_path: "docs/api/auth.md", old_string: "x-auth-token: <your-token>", new_string: "Authorization: Bearer <your-token>" }, output: "File updated successfully.", status: "success", duration: 44 },
      { id: "tc10", name: "Edit", input: { file_path: "docs/guides/authentication.md", old_string: "Include the token in the `x-auth-token` header", new_string: "Include the token as Bearer in the `Authorization` header" }, output: undefined, status: "error", duration: 39 },
    ],
  },
];

const TRACE_EVENTS: Record<string, TraceEvent> = {
  m1: {
    messageId: "m1", role: "user", timestamp: "2026-07-15T14:23:01.000Z", tokenCount: 34,
    requestId: "local", endpoint: "local://harness", method: "INPUT",
    requestHeaders: { "x-harness": "claude-code", "x-session-id": "A3F8B2D1" },
    requestBody: { role: "user", content: "Can you help me understand the current state of the authentication module..." },
    responseHeaders: {}, responseBody: {}, httpStatus: 200, duration: 0,
  },
  m2: {
    messageId: "m2", role: "assistant", model: "claude-opus-4-8",
    timestamp: "2026-07-15T14:23:02.441Z", tokenCount: 892,
    requestId: "req_01VcKjXm9aT3nPqLzYuFbHsR",
    endpoint: "https://api.anthropic.com/v1/messages", method: "POST",
    requestHeaders: {
      "content-type": "application/json",
      "anthropic-version": "2023-06-01",
      "x-api-key": "sk-ant-…[redacted]",
      "anthropic-beta": "tools-2024-04-04",
    },
    requestBody: {
      model: "claude-opus-4-8", max_tokens: 8192,
      system: "You are Claude Code, Anthropic's official CLI for Claude…",
      messages: [{ role: "user", content: "Can you help me understand the current state of the authentication module and refactor it to use the new JWT middleware?" }],
      tools: [
        { name: "Read",  description: "Read a file from the filesystem", input_schema: { type: "object", properties: { file_path: { type: "string" } }, required: ["file_path"] } },
        { name: "Glob",  description: "Find files matching a glob pattern", input_schema: { type: "object", properties: { pattern: { type: "string" } }, required: ["pattern"] } },
        { name: "Grep",  description: "Search file contents with regex", input_schema: { type: "object", properties: { pattern: { type: "string" } } } },
        { name: "Edit",  description: "Edit a file", input_schema: { type: "object", properties: { file_path: { type: "string" }, old_string: { type: "string" }, new_string: { type: "string" } } } },
        { name: "Bash",  description: "Run a bash command", input_schema: { type: "object", properties: { command: { type: "string" } } } },
      ],
    },
    responseHeaders: {
      "content-type": "application/json",
      "request-id": "req_01VcKjXm9aT3nPqLzYuFbHsR",
      "anthropic-ratelimit-requests-remaining": "498",
      "x-robots-tag": "none",
    },
    responseBody: {
      id: "msg_01VcKjXm9aT3nPqLzYuFbHsR",
      type: "message", role: "assistant",
      content: [
        { type: "text", text: "I'll start by reading the authentication module to understand its current structure, then look at the new JWT middleware implementation." },
        { type: "tool_use", id: "tc1", name: "Read", input: { file_path: "/src/auth/middleware.ts" } },
      ],
      model: "claude-opus-4-8", stop_reason: "tool_use",
      usage: { input_tokens: 892, output_tokens: 187 },
    },
    httpStatus: 200, duration: 2840,
  },
};

// ─── Helpers ──────────────────────────────────────────────────────────────────

function flattenSessions(sessions: Session[]): Session[] {
  return sessions.flatMap((s) => [s, ...(s.children ?? [])]);
}

// ─── SessionRow ───────────────────────────────────────────────────────────────

const COL = "1fr 26px 50px 44px 72px";

function SessionRow({ session, selectedId, onSelect, isChild = false }: {
  session: Session;
  selectedId: string;
  onSelect: (id: string) => void;
  isChild?: boolean;
}) {
  const [expanded, setExpanded] = useState(true);
  const selected = session.shortId === selectedId;
  const hasChildren = !isChild && (session.children?.length ?? 0) > 0;

  return (
    <>
      <div
        onClick={() => onSelect(session.shortId)}
        className="grid items-center cursor-pointer select-none transition-all duration-100"
        style={{
          gridTemplateColumns: COL,
          height: C.ROW_H,
          paddingRight: 8,
          paddingLeft: isChild ? 0 : 4,
          background: selected ? "rgba(10,132,255,0.07)" : undefined,
          // The blue right-border is the "sending" connection indicator → middle panel
          borderRight: selected ? `2px solid ${C.accent}` : "2px solid transparent",
        }}
        onMouseEnter={(e) => { if (!selected) e.currentTarget.style.background = "rgba(255,255,255,0.025)"; }}
        onMouseLeave={(e) => { if (!selected) e.currentTarget.style.background = "transparent"; }}
      >
        {/* Column 1: tree / expand / icons / ID / model */}
        <div className="flex items-center gap-1.5 min-w-0">
          {isChild ? (
            <div className="flex items-center shrink-0" style={{ width: 24, paddingLeft: 8 }}>
              <div style={{ width: 1, height: 12, background: C.textGhost, marginRight: 3 }} />
              <div style={{ width: 8, height: 1, background: C.textGhost }} />
            </div>
          ) : hasChildren ? (
            <button
              onClick={(e) => { e.stopPropagation(); setExpanded((o) => !o); }}
              className="shrink-0 flex items-center justify-center"
              style={{ width: 16, color: C.textDim }}>
              {expanded ? <ChevronDown size={11} /> : <ChevronRight size={11} />}
            </button>
          ) : (
            <div style={{ width: 16 }} />
          )}
          <StatusDot status={session.status} />
          <HarnessIcon type={session.harness} />
          <ProviderBadge provider={session.provider} />
          <span className="font-mono truncate"
            style={{ fontSize: 11, color: selected ? C.text : C.textSub, letterSpacing: "0.01em" }}>
            {session.shortId}
          </span>
          <ModelBadge model={session.model} />
        </div>
        {/* Column 2: turns */}
        <div className="text-right font-mono" style={{ fontSize: 10.5, color: C.textDim }}>{session.turns}</div>
        {/* Column 3: cost */}
        <div className="text-right font-mono" style={{ fontSize: 10.5, color: C.textDim }}>{formatCost(session.cost)}</div>
        {/* Column 4: duration */}
        <div className="text-right font-mono" style={{ fontSize: 10, color: C.textDim }}>{formatDur(session.duration)}</div>
        {/* Column 5: account */}
        <div className="truncate text-right" style={{ fontSize: 10, color: C.textGhost }}>{session.account}</div>
      </div>

      {/* Children */}
      {hasChildren && expanded && session.children!.map((child) => (
        <SessionRow key={child.id} session={child} selectedId={selectedId} onSelect={onSelect} isChild />
      ))}
    </>
  );
}

// ─── SessionListPanel ─────────────────────────────────────────────────────────

const SESSION_FILTERS = ["All", "Running", "Error", "Done"] as const;
type SessionFilter = typeof SESSION_FILTERS[number];

function SessionListPanel({ sessions, selectedId, onSelect }: {
  sessions: Session[];
  selectedId: string;
  onSelect: (id: string) => void;
}) {
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<SessionFilter>("All");

  const visible = sessions.filter((s) => {
    if (filter === "Running" && s.status !== "running") return false;
    if (filter === "Error"   && s.status !== "error")   return false;
    if (filter === "Done"    && s.status !== "success")  return false;
    if (query && !s.shortId.toLowerCase().includes(query.toLowerCase()) && !s.account.toLowerCase().includes(query.toLowerCase())) return false;
    return true;
  });

  const total = sessions.length;

  return (
    <>
      <PanelHeader
        left={
          <>
            <span style={{ fontSize: 12, fontWeight: 600, color: C.text }}>Sessions</span>
            <span className="font-mono" style={{ fontSize: 10, color: C.textDim,
              background: "rgba(255,255,255,0.06)", padding: "2px 6px", borderRadius: 4 }}>
              {total}
            </span>
          </>
        }
      />
      <FilterRow>
        <SearchInput value={query} onChange={setQuery} placeholder="Search sessions…" />
      </FilterRow>

      {/* Column header */}
      <div className="grid shrink-0"
        style={{
          gridTemplateColumns: COL,
          height: 24,
          paddingLeft: 20, paddingRight: 8,
          borderBottom: `1px solid ${C.border}`,
        }}>
        <span style={{ fontSize: 10, color: C.textDim, fontWeight: 500 }}>Session</span>
        <span className="text-right" style={{ fontSize: 10, color: C.textDim }}>T</span>
        <span className="text-right" style={{ fontSize: 10, color: C.textDim }}>Cost</span>
        <span className="text-right" style={{ fontSize: 10, color: C.textDim }}>Dur</span>
        <span className="text-right" style={{ fontSize: 10, color: C.textDim }}>Account</span>
      </div>

      {/* Status filter pills — below column headers */}
      <div className="flex items-center gap-1.5 px-3 shrink-0"
        style={{ height: 32, borderBottom: `1px solid ${C.border}` }}>
        {SESSION_FILTERS.map((f) => (
          <button key={f} onClick={() => setFilter(f)}
            className="transition-all duration-100"
            style={{
              fontSize: 10, fontWeight: 500, padding: "2px 8px", borderRadius: 6,
              background: filter === f ? "rgba(255,255,255,0.10)" : "transparent",
              color: filter === f ? C.text : C.textDim,
            }}>
            {f}
          </button>
        ))}
      </div>

      {/* Session rows */}
      <div className="flex-1 overflow-y-auto" style={{ scrollbarWidth: "none" }}>
        {visible.map((s) => (
          <SessionRow key={s.id} session={s} selectedId={selectedId} onSelect={onSelect} />
        ))}
        {visible.length === 0 && (
          <div className="flex items-center justify-center" style={{ height: 80, color: C.textDim, fontSize: 11 }}>
            No sessions match
          </div>
        )}
      </div>

      {/* Status bar */}
      <div className="flex items-center gap-2 px-3 shrink-0"
        style={{ height: 28, borderTop: `1px solid ${C.border}` }}>
        <span className="font-mono" style={{ fontSize: 10, color: C.textDim }}>
          {visible.length} of {total} sessions
        </span>
      </div>
    </>
  );
}

// ─── ToolCallCard ─────────────────────────────────────────────────────────────

function ToolCallCard({ tool }: { tool: ToolCall }) {
  const [open, setOpen] = useState(false);
  const [tab, setTab] = useState<"input" | "output">("input");

  return (
    <div className="rounded-lg overflow-hidden"
      style={{ background: "rgba(255,255,255,0.04)", border: `1px solid ${C.border}` }}>
      <button onClick={(e) => { e.stopPropagation(); setOpen((o) => !o); }}
        className="w-full flex items-center gap-2 px-3 py-2 text-left hover:bg-white/5 transition-colors">
        <span style={{ color: C.textMuted, flexShrink: 0 }}>{toolIcon(tool.name)}</span>
        <span className="font-mono font-medium" style={{ fontSize: 11, color: C.text }}>{tool.name}</span>
        <span className="font-mono truncate flex-1" style={{ fontSize: 11, color: C.textDim }}>
          {Object.values(tool.input)[0] !== undefined ? truncate(String(Object.values(tool.input)[0]), 44) : ""}
        </span>
        <div className="flex items-center gap-2 ml-auto shrink-0">
          {tool.duration !== undefined && (
            <span className="font-mono" style={{ fontSize: 10, color: C.textDim }}>{tool.duration}ms</span>
          )}
          <StatusIcon status={tool.status} />
          <span style={{ color: C.textDim }}>{open ? <ChevronDown size={12} /> : <ChevronRight size={12} />}</span>
        </div>
      </button>

      {open && (
        <div style={{ borderTop: `1px solid rgba(255,255,255,0.05)` }}>
          <div className="flex gap-0.5 px-3 pt-2">
            {(["input", "output"] as const).map((t) => (
              <button key={t} onClick={(e) => { e.stopPropagation(); setTab(t); }}
                className="font-medium rounded-md transition-colors"
                style={{
                  fontSize: 10, padding: "3px 8px",
                  background: tab === t ? "rgba(255,255,255,0.10)" : "transparent",
                  color: tab === t ? C.text : C.textDim,
                }}>
                {t.charAt(0).toUpperCase() + t.slice(1)}
              </button>
            ))}
          </div>
          <JsonBlock content={tab === "input" ? tool.input : (tool.output ?? "(no output)")} maxHeight={180} />
        </div>
      )}
    </div>
  );
}

// ─── SubagentCard ─────────────────────────────────────────────────────────────

function SubagentCard({ subagent, onFollow }: {
  subagent: SubagentCall;
  onFollow: (id: string) => void;
}) {
  const statusMeta: Record<string, { bg: string; text: string }> = {
    success: { bg: "rgba(48,209,88,0.1)",  text: C.success },
    error:   { bg: "rgba(255,69,58,0.1)",  text: C.error },
    running: { bg: "rgba(255,214,10,0.1)", text: C.warning },
  };
  const sm = statusMeta[subagent.status] ?? statusMeta.running;

  return (
    <div className="rounded-xl overflow-hidden"
      style={{
        background: "linear-gradient(135deg, rgba(10,132,255,0.08) 0%, rgba(10,132,255,0.03) 100%)",
        border: "1px solid rgba(10,132,255,0.22)",
      }}>
      <div className="flex items-center gap-2.5 px-3 py-2.5">
        <div className="flex items-center justify-center rounded-lg shrink-0"
          style={{ width: 28, height: 28, background: "rgba(10,132,255,0.15)" }}>
          <GitBranch size={13} style={{ color: C.accent }} />
        </div>
        <div className="flex-1 min-w-0">
          <div className="flex items-center gap-2">
            <span style={{ fontSize: 10, fontWeight: 700, color: C.accent, letterSpacing: "0.08em", textTransform: "uppercase" }}>
              Subagent
            </span>
            <span className="font-mono" style={{ fontSize: 9, padding: "2px 5px", borderRadius: 4, background: sm.bg, color: sm.text }}>
              {subagent.status}
            </span>
          </div>
          <p className="font-mono truncate mt-0.5" style={{ fontSize: 10.5, color: C.textSub }}>
            {truncate(subagent.prompt, 68)}
          </p>
        </div>
      </div>

      <div className="flex items-center gap-4 px-3 py-2"
        style={{ borderTop: "1px solid rgba(10,132,255,0.10)" }}>
        <div className="flex items-center gap-1.5" style={{ color: C.textDim }}>
          <ModelBadge model={subagent.model} />
        </div>
        {subagent.turnCount !== undefined && (
          <div className="flex items-center gap-1" style={{ color: C.textDim, fontSize: 10 }}>
            <Layers size={10} />
            <span className="font-mono">{subagent.turnCount} turns</span>
          </div>
        )}
        {subagent.duration !== undefined && (
          <div className="flex items-center gap-1" style={{ color: C.textDim, fontSize: 10 }}>
            <Clock size={10} />
            <span className="font-mono">{formatDur(subagent.duration)}</span>
          </div>
        )}
        <button
          onClick={(e) => { e.stopPropagation(); onFollow(subagent.traceId); }}
          className="ml-auto flex items-center gap-1.5 rounded-lg transition-colors"
          style={{ fontSize: 10, fontWeight: 500, color: C.accent, padding: "3px 8px" }}
          onMouseEnter={(e) => (e.currentTarget.style.background = C.accentDim)}
          onMouseLeave={(e) => (e.currentTarget.style.background = "transparent")}>
          Follow trace
          <ArrowUpRight size={11} />
        </button>
      </div>
    </div>
  );
}

// ─── MessageBubble ────────────────────────────────────────────────────────────

function MessageBubble({ message, selected, onClick, onFollowSubagent, isThreaded, turnIndex }: {
  message: Message;
  selected: boolean;
  onClick: () => void;
  onFollowSubagent: (id: string) => void;
  isThreaded?: boolean;   // true → draw a connector line below avatar to next sibling
  turnIndex?: number;     // shown as a dim turn counter in the gutter
}) {
  const isUser = message.role === "user";

  const bubbleBg = isUser
    ? selected ? "rgba(52,52,54,0.95)" : "rgba(44,44,46,0.80)"
    : selected ? "rgba(10,84,160,0.28)"  : "rgba(10,84,160,0.16)";

  const bubbleBorder = isUser
    ? selected ? "rgba(255,255,255,0.14)" : "rgba(255,255,255,0.07)"
    : selected ? "rgba(10,132,255,0.45)"  : "rgba(10,132,255,0.20)";

  const connectorColor = isUser
    ? "rgba(255,255,255,0.07)"
    : "rgba(10,132,255,0.18)";

  return (
    <div
      onClick={onClick}
      className={`flex gap-3 px-4 py-1 cursor-pointer transition-all duration-100 ${isUser ? "flex-row" : "flex-row-reverse"}`}
      style={{
        background: selected ? "rgba(255,255,255,0.025)" : undefined,
        borderRight: selected ? `2px solid ${C.accent}` : "2px solid transparent",
      }}
      onMouseEnter={(e) => { if (!selected) e.currentTarget.style.background = "rgba(255,255,255,0.015)"; }}
      onMouseLeave={(e) => { if (!selected) e.currentTarget.style.background = "transparent"; }}
    >
      {/* Avatar column — stretches full height so connector line fills it */}
      <div className="shrink-0 mt-1 flex flex-col items-center" style={{ alignSelf: "stretch" }}>
        <div className="flex items-center justify-center rounded-full shrink-0"
          style={{
            width: 24, height: 24,
            background: isUser
              ? "rgba(58,58,60,0.8)"
              : "linear-gradient(135deg, rgba(10,132,255,0.4), rgba(94,92,230,0.4))",
            border: `1px solid ${isUser ? "rgba(255,255,255,0.10)" : "rgba(10,132,255,0.4)"}`,
          }}>
          {isUser
            ? <User size={11} style={{ color: C.textMuted }} />
            : <Bot  size={11} style={{ color: "#409cff" }} />}
        </div>
        {/* Thread connector — only when the next message continues the same role */}
        {isThreaded && (
          <div className="flex-1 w-px mt-1 mb-0" style={{ background: connectorColor, minHeight: 12 }} />
        )}
      </div>

      {/* Content column */}
      <div className={`flex-1 min-w-0 pb-3 flex flex-col ${isUser ? "items-start" : "items-end"}`}>
        {/* Header row */}
        <div className={`flex items-center gap-2 mb-1.5 w-full ${isUser ? "flex-row" : "flex-row-reverse"}`}>
          <span style={{ fontSize: 11, fontWeight: 600, color: isUser ? C.textMuted : "#409cff" }}>
            {isUser ? "User / Harness" : "Model"}
          </span>
          {message.model && (
            <span className="font-mono" style={{ fontSize: 10, color: C.textDim }}>{message.model}</span>
          )}
          <span className="flex-1" />
          {/* Turn number */}
          {turnIndex !== undefined && (
            <span className="font-mono" style={{ fontSize: 9.5, color: C.textGhost }}>
              #{turnIndex + 1}
            </span>
          )}
          {message.tokenCount !== undefined && (
            <span className="font-mono" style={{ fontSize: 10, color: C.textDim }}>
              {message.tokenCount.toLocaleString()} tok
            </span>
          )}
          <span className="font-mono" style={{ fontSize: 10, color: C.textGhost }}>{message.timestamp}</span>
        </div>

        {/* Bubble */}
        <div className={`rounded-2xl max-w-[88%] ${isUser ? "rounded-tl-sm" : "rounded-tr-sm"}`}
          style={{
            padding: "10px 14px", fontSize: 12, lineHeight: 1.65,
            background: bubbleBg, border: `1px solid ${bubbleBorder}`,
            color: isUser ? "#c7c7cc" : "#dde8ff",
          }}>
          {message.content}
        </div>

        {/* Tool calls */}
        {message.toolCalls && message.toolCalls.length > 0 && (
          <div className="mt-2 flex flex-col gap-1.5 w-full max-w-[92%]">
            <div className={`flex items-center gap-1.5 mb-0.5 ${isUser ? "" : "flex-row-reverse"}`}>
              <div className="w-px h-3" style={{ background: "rgba(255,255,255,0.10)" }} />
              <span className="font-mono" style={{ fontSize: 10, color: C.textDim }}>
                {message.toolCalls.length} tool call{message.toolCalls.length !== 1 ? "s" : ""}
              </span>
            </div>
            {message.toolCalls.map((tc) => <ToolCallCard key={tc.id} tool={tc} />)}
          </div>
        )}

        {/* Subagent */}
        {message.subagent && (
          <div className="mt-2 w-full max-w-[92%]">
            <SubagentCard subagent={message.subagent} onFollow={onFollowSubagent} />
          </div>
        )}
      </div>
    </div>
  );
}

// ─── TraceViewPanel ───────────────────────────────────────────────────────────

const MSG_FILTERS = ["All", "User", "Model", "Tools", "Agents"] as const;
type MsgFilter = typeof MSG_FILTERS[number];

function TraceViewPanel({ session, messages, selectedMsgId, onSelectMsg, onFollowSubagent, followingTrace, onDismissFollow }: {
  session: Session | undefined;
  messages: Message[];
  selectedMsgId: string | null;
  onSelectMsg: (id: string) => void;
  onFollowSubagent: (id: string) => void;
  followingTrace: string | null;
  onDismissFollow: () => void;
}) {
  const [filter, setFilter] = useState<MsgFilter>("All");
  const [query, setQuery] = useState("");

  const filtered = messages.filter((m) => {
    if (filter === "User"   && m.role !== "user")          return false;
    if (filter === "Model"  && m.role !== "assistant")     return false;
    if (filter === "Tools"  && !m.toolCalls?.length)       return false;
    if (filter === "Agents" && !m.subagent)                return false;
    if (query && !m.content.toLowerCase().includes(query.toLowerCase())) return false;
    return true;
  });

  const toolCount = messages.reduce((a, m) => a + (m.toolCalls?.length ?? 0), 0);
  const agentCount = messages.filter((m) => m.subagent).length;

  return (
    <>
      {/* Header — accentLeft = true because it's "receiving" the selection from the left panel */}
      <PanelHeader
        accentLeft
        left={
          <>
            <div className="flex items-center justify-center rounded-lg"
              style={{ width: 30, height: 30, background: C.accentDim, border: `1px solid rgba(10,132,255,0.22)`, flexShrink: 0 }}>
              <Zap size={13} style={{ color: C.accent }} />
            </div>
            <div className="min-w-0">
              <div className="font-mono truncate" style={{ fontSize: 11, fontWeight: 600, color: C.text }}>
                {session?.shortId ?? "—"}
              </div>
              <div style={{ fontSize: 10, color: C.textDim }}>
                {messages.length} turns · {toolCount} tools · {agentCount} subagent{agentCount !== 1 ? "s" : ""}
              </div>
            </div>
            {session && <ModelBadge model={session.model} />}
          </>
        }
        right={
          <>
            {session && <CopyButton value={session.id} label="Copy ID" />}
            <button className="flex items-center justify-center rounded-lg transition-colors"
              style={{ width: 28, height: 28, color: C.textDim }}
              onMouseEnter={(e) => (e.currentTarget.style.background = "rgba(255,255,255,0.08)")}
              onMouseLeave={(e) => (e.currentTarget.style.background = "transparent")}>
              <MoreHorizontal size={14} />
            </button>
          </>
        }
      />

      <FilterRow>
        <SearchInput value={query} onChange={setQuery} placeholder="Filter messages…" />
        <TabBar tabs={MSG_FILTERS} active={filter} onChange={setFilter} />
      </FilterRow>

      {/* Follow banner */}
      {followingTrace && (
        <div className="mx-4 mt-3 mb-1 flex items-center gap-3 rounded-xl"
          style={{
            padding: "8px 14px",
            background: "linear-gradient(135deg, rgba(10,132,255,0.12), rgba(94,92,230,0.08))",
            border: "1px solid rgba(10,132,255,0.3)",
          }}>
          <GitBranch size={13} style={{ color: C.accent, flexShrink: 0 }} />
          <span style={{ fontSize: 11, color: C.textSub }}>
            Following subagent{" "}
            <span className="font-mono" style={{ color: "#409cff" }}>{followingTrace}</span>
          </span>
          <button onClick={onDismissFollow}
            className="ml-auto rounded transition-colors"
            style={{ fontSize: 10, fontWeight: 500, color: C.textDim, padding: "2px 8px" }}
            onMouseEnter={(e) => (e.currentTarget.style.background = "rgba(255,255,255,0.08)")}
            onMouseLeave={(e) => (e.currentTarget.style.background = "transparent")}>
            Dismiss
          </button>
        </div>
      )}

      {/* Messages */}
      <div className="flex-1 overflow-y-auto pt-2 pb-4" style={{ scrollbarWidth: "none" }}>
        {filtered.map((msg, i) => {
          const prevRole = i > 0 ? filtered[i - 1].role : null;
          const nextRole = i < filtered.length - 1 ? filtered[i + 1].role : null;
          const roleChanged = prevRole !== null && prevRole !== msg.role;
          const isThreaded = nextRole === msg.role;
          const globalIndex = messages.findIndex((m) => m.id === msg.id);
          return (
            <div key={msg.id}>
              {/* Role-change divider */}
              {roleChanged && (
                <div className="flex items-center gap-3 mx-4 my-2.5">
                  <div className="flex-1 h-px" style={{ background: "rgba(255,255,255,0.05)" }} />
                  <span className="font-mono" style={{
                    fontSize: 9.5, color: msg.role === "user" ? C.textDim : "rgba(10,132,255,0.5)",
                    fontWeight: 600, letterSpacing: "0.07em", textTransform: "uppercase",
                  }}>
                    {msg.role === "user" ? "User" : "Model"}
                  </span>
                  <div className="flex-1 h-px" style={{ background: "rgba(255,255,255,0.05)" }} />
                </div>
              )}
              <MessageBubble
                message={msg}
                selected={selectedMsgId === msg.id}
                onClick={() => onSelectMsg(msg.id)}
                onFollowSubagent={onFollowSubagent}
                isThreaded={isThreaded}
                turnIndex={globalIndex}
              />
            </div>
          );
        })}
      </div>

      {/* Footer */}
      <div className="flex items-center gap-3 px-4 shrink-0"
        style={{ height: 28, borderTop: `1px solid ${C.border}` }}>
        <div className="w-1.5 h-1.5 rounded-full" style={{ background: C.success }} />
        <span className="font-mono" style={{ fontSize: 10, color: C.textDim }}>
          {filtered.length} of {messages.length} messages
        </span>
        <span className="font-mono ml-auto" style={{ fontSize: 10, color: C.textDim }}>
          {messages.reduce((a, m) => a + (m.tokenCount ?? 0), 0).toLocaleString()} tokens total
        </span>
      </div>
    </>
  );
}

// ─── EventDetailPanel ─────────────────────────────────────────────────────────

function EventDetailPanel({ event, session, messageIndex }: {
  event: TraceEvent | null | undefined;
  session: Session | undefined;
  messageIndex: number;
}) {
  if (!event) {
    return (
      <>
        <PanelHeader accentLeft left={
          <span style={{ fontSize: 12, fontWeight: 600, color: C.text }}>Event Detail</span>
        } />
        <div className="flex-1 flex flex-col items-center justify-center gap-2"
          style={{ color: C.textDim }}>
          <Activity size={20} style={{ opacity: 0.4 }} />
          <span style={{ fontSize: 11 }}>Select a message to inspect</span>
        </div>
      </>
    );
  }

  const reqHeaderPairs = Object.entries(event.requestHeaders) as [string, string][];
  const resHeaderPairs = Object.entries(event.responseHeaders) as [string, string][];
  const isApiCall = event.method === "POST";
  const httpOk = event.httpStatus >= 200 && event.httpStatus < 300;

  return (
    <>
      {/* Header — accentLeft = true, "receiving" from middle panel */}
      <PanelHeader
        accentLeft
        left={
          <div className="min-w-0">
            {/* Breadcrumb */}
            <div className="flex items-center gap-1 mb-0.5">
              <span className="font-mono" style={{ fontSize: 9.5, color: C.textDim }}>{session?.shortId}</span>
              <ChevronRight size={9} style={{ color: C.textDim, flexShrink: 0 }} />
              <span className="font-mono" style={{ fontSize: 9.5, color: C.textDim }}>Turn {messageIndex + 1}</span>
              <ChevronRight size={9} style={{ color: C.textDim, flexShrink: 0 }} />
              <span className="font-mono" style={{ fontSize: 9.5, color: event.role === "assistant" ? "#409cff" : C.textMuted }}>
                {event.role}
              </span>
            </div>
            <div className="flex items-center gap-2">
              <span style={{ fontSize: 12, fontWeight: 600, color: C.text }}>
                {isApiCall ? "API Request" : "Harness Input"}
              </span>
              {isApiCall && (
                <span className="font-mono"
                  style={{ fontSize: 9.5, padding: "2px 6px", borderRadius: 4,
                    background: httpOk ? "rgba(48,209,88,0.1)" : "rgba(255,69,58,0.1)",
                    color: httpOk ? C.success : C.error }}>
                  {event.httpStatus}
                </span>
              )}
            </div>
          </div>
        }
        right={session && <CopyButton value={event.requestId} label="Copy ID" />}
      />

      {/* Quick stats */}
      <div className="grid grid-cols-3 gap-0 shrink-0"
        style={{ borderBottom: `1px solid ${C.border}` }}>
        {[
          { label: "Method",   value: event.method },
          { label: "Duration", value: formatDur(event.duration) },
          { label: "Tokens",   value: event.tokenCount ? `${event.tokenCount.toLocaleString()}` : "—" },
        ].map(({ label, value }) => (
          <div key={label} className="flex flex-col items-center justify-center"
            style={{ padding: "10px 6px", borderRight: `1px solid ${C.border}` }}>
            <span className="font-mono" style={{ fontSize: 12, fontWeight: 600, color: C.textSub }}>{value}</span>
            <span style={{ fontSize: 9.5, color: C.textDim, marginTop: 2 }}>{label}</span>
          </div>
        ))}
      </div>

      {/* Scrollable detail sections */}
      <div className="flex-1 overflow-y-auto" style={{ scrollbarWidth: "none" }}>
        {/* Endpoint */}
        <div style={{ padding: "10px 12px", borderBottom: `1px solid ${C.border}` }}>
          <div style={{ fontSize: 10, color: C.textDim, marginBottom: 4, fontWeight: 500, textTransform: "uppercase", letterSpacing: "0.07em" }}>
            Endpoint
          </div>
          <div className="font-mono truncate" style={{ fontSize: 10.5, color: C.textSub }}>
            {event.endpoint}
          </div>
          <div className="font-mono" style={{ fontSize: 10, color: C.textDim, marginTop: 2 }}>
            {event.requestId}
          </div>
        </div>

        {/* Request headers */}
        {reqHeaderPairs.length > 0 && (
          <CollapsibleSection title="Request Headers" badge={reqHeaderPairs.length}>
            <KVTable pairs={reqHeaderPairs} />
          </CollapsibleSection>
        )}

        {/* Request body */}
        {event.requestBody && Object.keys(event.requestBody as object).length > 0 && (
          <CollapsibleSection title="Request Body" defaultOpen>
            <JsonBlock content={event.requestBody} maxHeight={240} />
          </CollapsibleSection>
        )}

        {/* Response headers */}
        {resHeaderPairs.length > 0 && (
          <CollapsibleSection title="Response Headers" badge={resHeaderPairs.length}>
            <KVTable pairs={resHeaderPairs} />
          </CollapsibleSection>
        )}

        {/* Response body */}
        {event.responseBody && Object.keys(event.responseBody as object).length > 0 && (
          <CollapsibleSection title="Response Body" defaultOpen>
            <JsonBlock content={event.responseBody} maxHeight={280} />
          </CollapsibleSection>
        )}
      </div>
    </>
  );
}

// ─── App root ─────────────────────────────────────────────────────────────────

export default function App() {
  const [selectedSessionId, setSelectedSessionId] = useState("A3F8B2D1");
  const [selectedMsgId, setSelectedMsgId] = useState<string | null>("m2");
  const [followingTrace, setFollowingTrace] = useState<string | null>(null);
  const [leftWidth, setLeftWidth] = useState(340);

  // Drag-to-resize state stored in a ref so mousemove handler doesn't stale-close
  const drag = useRef({ active: false, startX: 0, startW: 0 });

  useEffect(() => {
    const onMove = (e: MouseEvent) => {
      if (!drag.current.active) return;
      const w = Math.max(200, Math.min(560, drag.current.startW + (e.clientX - drag.current.startX)));
      setLeftWidth(w);
    };
    const onUp = () => {
      if (!drag.current.active) return;
      drag.current.active = false;
      document.body.style.cursor = "";
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup",   onUp);
    return () => { window.removeEventListener("mousemove", onMove); window.removeEventListener("mouseup", onUp); };
  }, []);

  const startDrag = (e: React.MouseEvent) => {
    drag.current = { active: true, startX: e.clientX, startW: leftWidth };
    document.body.style.cursor = "col-resize";
    e.preventDefault();
  };

  const currentSession = flattenSessions(SESSIONS).find((s) => s.shortId === selectedSessionId);
  const currentMessages = MESSAGES;
  const currentEvent = selectedMsgId ? TRACE_EVENTS[selectedMsgId] : null;
  const msgIndex = currentMessages.findIndex((m) => m.id === selectedMsgId);

  const handleSelectSession = (id: string) => {
    setSelectedSessionId(id);
    setSelectedMsgId(null);
    setFollowingTrace(null);
  };

  const handleFollowSubagent = (traceId: string) => {
    setFollowingTrace(traceId);
    setSelectedSessionId(traceId);
    setSelectedMsgId(null);
  };

  return (
    <div className="size-full flex overflow-hidden"
      style={{ background: C.bg, fontFamily: "'Inter', system-ui, sans-serif", fontSize: 13 }}>

      {/* Left: Session list — resizable */}
      <div className="flex flex-col shrink-0" style={{ width: leftWidth }}>
        <SessionListPanel
          sessions={SESSIONS}
          selectedId={selectedSessionId}
          onSelect={handleSelectSession}
        />
      </div>

      {/* Drag handle */}
      <div
        onMouseDown={startDrag}
        className="shrink-0 relative group"
        style={{ width: 5, cursor: "col-resize", zIndex: 20 }}
      >
        {/* Visible hairline — brightens on hover / active drag */}
        <div className="absolute inset-y-0 left-[2px] w-px transition-colors duration-150"
          style={{ background: C.border }}
          onMouseEnter={(e) => (e.currentTarget.style.background = C.accent)}
          onMouseLeave={(e) => (e.currentTarget.style.background = C.border)}
        />
      </div>

      {/* Middle: Trace view — flex-1 */}
      <div className="flex flex-col flex-1 min-w-0"
        style={{ borderRight: `1px solid ${C.border}` }}>
        <TraceViewPanel
          session={currentSession}
          messages={currentMessages}
          selectedMsgId={selectedMsgId}
          onSelectMsg={setSelectedMsgId}
          onFollowSubagent={handleFollowSubagent}
          followingTrace={followingTrace}
          onDismissFollow={() => setFollowingTrace(null)}
        />
      </div>

      {/* Right: Event detail — 308px */}
      <div className="flex flex-col shrink-0" style={{ width: 308 }}>
        <EventDetailPanel
          event={currentEvent}
          session={currentSession}
          messageIndex={msgIndex}
        />
      </div>

    </div>
  );
}
