// Shared design system — types, tokens, utilities, and UI primitives.
// Import from here rather than redefining across panel files.

import { useState } from "react";
import {
  CheckCircle2, XCircle, AlertCircle, Circle,
  Terminal, Globe, Link2, Settings, Wrench,
  FileText, Search, Hash, Code2,
  ChevronDown, ChevronRight,
  Copy, Check,
} from "lucide-react";

// ─── Design tokens ────────────────────────────────────────────────────────────

export const C = {
  bg:           "#1c1c1e",
  surface:      "#242426",
  card:         "#2c2c2e",
  raised:       "#3a3a3c",
  border:       "rgba(255,255,255,0.07)",
  borderMed:    "rgba(255,255,255,0.11)",
  text:         "#e5e5ea",
  textSub:      "#aeaeb2",
  textMuted:    "#8e8e93",
  textDim:      "#636366",
  textGhost:    "#3a3a3c",
  accent:       "#0a84ff",
  accentDim:    "rgba(10,132,255,0.15)",
  success:      "#30d158",
  error:        "#ff453a",
  warning:      "#ffd60a",
  purple:       "#bf5af2",
  teal:         "#5ac8fa",
  PANEL_H:      48,   // px — consistent header height across all panels
  ROW_H:        30,   // px — default list row height
  FILTER_H:     40,   // px — filter bar height
} as const;

export const MODEL_COLORS: Record<string, { bg: string; text: string; border: string }> = {
  "claude-opus-4-8":   { bg: "rgba(191,90,242,0.12)", text: "#bf5af2", border: "rgba(191,90,242,0.25)" },
  "claude-sonnet-4-6": { bg: "rgba(10,132,255,0.12)", text: "#409cff", border: "rgba(10,132,255,0.25)" },
  "claude-haiku-4-5":  { bg: "rgba(48,209,88,0.10)",  text: "#30d158", border: "rgba(48,209,88,0.22)" },
  "gpt-4o":            { bg: "rgba(90,200,250,0.10)",  text: "#5ac8fa", border: "rgba(90,200,250,0.22)" },
};

// ─── Types ────────────────────────────────────────────────────────────────────

export type Status      = "success" | "error" | "running" | "pending";
export type Role        = "user" | "assistant";
export type HarnessType = "claude-code" | "api" | "langchain" | "custom";
export type Provider    = "anthropic" | "openai" | "google";

export interface ToolCall {
  id: string;
  name: string;
  input: Record<string, unknown>;
  output?: string;
  status: Status;
  duration?: number;
}

export interface SubagentCall {
  id: string;
  traceId: string;
  model: string;
  prompt: string;
  status: Status;
  duration?: number;
  turnCount?: number;
}

export interface Message {
  id: string;
  role: Role;
  content: string;
  timestamp: string;
  toolCalls?: ToolCall[];
  subagent?: SubagentCall;
  tokenCount?: number;
  model?: string;
}

export interface Session {
  id: string;
  shortId: string;
  status: Status;
  harness: HarnessType;
  model: string;
  provider: Provider;
  turns: number;
  cost: number;
  duration: number;
  account: string;
  timestamp: string;
  children?: Session[];
  isSubagent?: boolean;
}

export interface TraceEvent {
  messageId: string;
  role: Role;
  model?: string;
  timestamp: string;
  tokenCount?: number;
  requestId: string;
  endpoint: string;
  method: string;
  requestHeaders: Record<string, string>;
  requestBody: unknown;
  responseHeaders: Record<string, string>;
  responseBody: unknown;
  httpStatus: number;
  duration: number;
}

// ─── Utilities ────────────────────────────────────────────────────────────────

export const truncate = (s: string, n: number) =>
  s.length > n ? s.slice(0, n) + "…" : s;

export const formatJson = (obj: unknown) => JSON.stringify(obj, null, 2);

export const formatDur = (ms: number) =>
  ms === 0 ? "—" : ms < 1000 ? `${ms}ms` : `${(ms / 1000).toFixed(1)}s`;

export const formatCost = (usd: number) =>
  usd === 0 ? "—" : `$${usd.toFixed(2)}`;

export const toolIcon = (name: string): React.ReactNode => {
  const m: Record<string, React.ReactNode> = {
    Read: <FileText size={11} />, Glob: <Search size={11} />,
    Grep: <Hash size={11} />,    Edit: <Code2 size={11} />,
    Write: <Code2 size={11} />,  Bash: <Terminal size={11} />,
  };
  return m[name] ?? <Wrench size={11} />;
};

// ─── StatusDot ────────────────────────────────────────────────────────────────

export function StatusDot({ status }: { status: Status }) {
  const colors: Record<Status, string> = {
    success: C.success, error: C.error, running: C.warning, pending: C.textDim,
  };
  return <span className="w-1.5 h-1.5 rounded-full inline-block shrink-0"
    style={{ background: colors[status] }} />;
}

// ─── StatusIcon ───────────────────────────────────────────────────────────────

export function StatusIcon({ status }: { status: Status }) {
  const m: Record<Status, { icon: React.ReactNode; color: string }> = {
    success: { icon: <CheckCircle2 size={11} />, color: C.success },
    error:   { icon: <XCircle size={11} />,      color: C.error },
    running: { icon: <AlertCircle size={11} />,  color: C.warning },
    pending: { icon: <Circle size={11} />,       color: C.textDim },
  };
  const { icon, color } = m[status];
  return <span style={{ color }}>{icon}</span>;
}

// ─── ModelBadge ───────────────────────────────────────────────────────────────

export function ModelBadge({ model }: { model: string }) {
  const s = MODEL_COLORS[model] ?? { bg: "rgba(255,255,255,0.08)", text: C.textMuted, border: C.border };
  const label = model
    .replace("claude-", "")
    .replace(/-(\d+)-(\d+)$/, " $1.$2");
  return (
    <span className="shrink-0 whitespace-nowrap font-mono font-medium"
      style={{ fontSize: 9.5, padding: "2px 6px", borderRadius: 5, background: s.bg, color: s.text, border: `1px solid ${s.border}` }}>
      {label}
    </span>
  );
}

// ─── HarnessIcon ──────────────────────────────────────────────────────────────

export function HarnessIcon({ type }: { type: HarnessType }) {
  const m: Record<HarnessType, { icon: React.ReactNode; bg: string; title: string }> = {
    "claude-code": { icon: <Terminal size={9} />,  bg: "rgba(10,132,255,0.18)",  title: "Claude Code" },
    "api":         { icon: <Globe size={9} />,     bg: "rgba(191,90,242,0.15)",  title: "API" },
    "langchain":   { icon: <Link2 size={9} />,     bg: "rgba(90,200,250,0.15)",  title: "LangChain" },
    "custom":      { icon: <Settings size={9} />,  bg: "rgba(255,255,255,0.08)", title: "Custom" },
  };
  const { icon, bg, title } = m[type];
  return (
    <span title={title} className="shrink-0 flex items-center justify-center rounded"
      style={{ width: 17, height: 17, background: bg, color: C.textSub }}>
      {icon}
    </span>
  );
}

// ─── ProviderBadge ────────────────────────────────────────────────────────────

export function ProviderBadge({ provider }: { provider: Provider }) {
  const m: Record<Provider, { label: string; bg: string; text: string }> = {
    anthropic: { label: "A", bg: "rgba(255,107,0,0.18)",  text: "#ff9040" },
    openai:    { label: "O", bg: "rgba(16,185,129,0.18)", text: "#10b981" },
    google:    { label: "G", bg: "rgba(66,133,244,0.18)", text: "#4285f4" },
  };
  const { label, bg, text } = m[provider];
  return (
    <span title={provider} className="shrink-0 flex items-center justify-center rounded font-bold"
      style={{ width: 17, height: 17, background: bg, color: text, fontSize: 9 }}>
      {label}
    </span>
  );
}

// ─── TabBar ───────────────────────────────────────────────────────────────────

export function TabBar<T extends string>({ tabs, active, onChange }: {
  tabs: readonly T[];
  active: T;
  onChange: (t: T) => void;
}) {
  return (
    <div className="flex items-center gap-0.5 p-0.5 rounded-lg"
      style={{ background: "rgba(255,255,255,0.05)", border: `1px solid ${C.border}` }}>
      {tabs.map((t) => (
        <button key={t} onClick={() => onChange(t)}
          className="font-medium rounded-md transition-all duration-150"
          style={{
            fontSize: 10.5, padding: "4px 10px",
            background: active === t ? "rgba(255,255,255,0.10)" : "transparent",
            color: active === t ? C.text : C.textDim,
          }}>
          {t}
        </button>
      ))}
    </div>
  );
}

// ─── SearchInput ──────────────────────────────────────────────────────────────

export function SearchInput({ value, onChange, placeholder = "Search…" }: {
  value: string;
  onChange: (v: string) => void;
  placeholder?: string;
}) {
  return (
    <div className="flex items-center gap-2 flex-1 px-2.5 rounded-lg"
      style={{ height: 28, background: "rgba(255,255,255,0.06)", border: `1px solid ${C.border}` }}>
      <Search size={11} style={{ color: C.textDim, flexShrink: 0 }} />
      <input value={value} onChange={(e) => onChange(e.target.value)}
        placeholder={placeholder}
        className="bg-transparent outline-none flex-1 font-mono"
        style={{ fontSize: 11, color: C.text }} />
    </div>
  );
}

// ─── CopyButton ───────────────────────────────────────────────────────────────

export function CopyButton({ value, label }: { value: string; label?: string }) {
  const [copied, setCopied] = useState(false);
  const copy = () => {
    navigator.clipboard?.writeText(value).catch(() => {});
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };
  return (
    <button onClick={copy}
      className="flex items-center gap-1.5 rounded-lg transition-all"
      style={{ padding: "4px 8px", color: copied ? C.success : C.textDim, background: "transparent" }}
      onMouseEnter={(e) => (e.currentTarget.style.background = "rgba(255,255,255,0.06)")}
      onMouseLeave={(e) => (e.currentTarget.style.background = "transparent")}>
      {copied ? <Check size={11} /> : <Copy size={11} />}
      {label && <span style={{ fontSize: 10.5, fontWeight: 500 }}>{copied ? "Copied" : label}</span>}
    </button>
  );
}

// ─── PanelHeader ──────────────────────────────────────────────────────────────
// accentLeft = true draws the blue left-border "receiving" connection indicator.

export function PanelHeader({ left, right, accentLeft = false }: {
  left: React.ReactNode;
  right?: React.ReactNode;
  accentLeft?: boolean;
}) {
  return (
    <div className="flex items-center shrink-0 gap-2"
      style={{
        height: C.PANEL_H,
        borderBottom: `1px solid ${C.border}`,
        borderLeft: accentLeft ? `2px solid ${C.accent}` : undefined,
        paddingLeft: accentLeft ? 10 : 12,
        paddingRight: 8,
      }}>
      <div className="flex-1 min-w-0 flex items-center gap-2">{left}</div>
      {right && <div className="flex items-center gap-0.5 shrink-0">{right}</div>}
    </div>
  );
}

// ─── FilterRow ────────────────────────────────────────────────────────────────

export function FilterRow({ children }: { children: React.ReactNode }) {
  return (
    <div className="flex items-center gap-2 shrink-0 px-3"
      style={{ height: C.FILTER_H, borderBottom: `1px solid ${C.border}`, background: `${C.bg}cc` }}>
      {children}
    </div>
  );
}

// ─── CollapsibleSection ───────────────────────────────────────────────────────

export function CollapsibleSection({ title, badge, defaultOpen = false, children }: {
  title: string;
  badge?: number | string;
  defaultOpen?: boolean;
  children: React.ReactNode;
}) {
  const [open, setOpen] = useState(defaultOpen);
  return (
    <div style={{ borderTop: `1px solid ${C.border}` }}>
      <button onClick={() => setOpen((o) => !o)}
        className="w-full flex items-center gap-2 text-left transition-colors"
        style={{ padding: "8px 12px", color: C.textMuted }}
        onMouseEnter={(e) => (e.currentTarget.style.background = "rgba(255,255,255,0.03)")}
        onMouseLeave={(e) => (e.currentTarget.style.background = "transparent")}>
        <span style={{ color: C.textDim }}>
          {open ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
        </span>
        <span style={{ fontSize: 11, fontWeight: 500 }}>{title}</span>
        {badge !== undefined && (
          <span className="font-mono" style={{ fontSize: 10, color: C.textDim }}>({badge})</span>
        )}
      </button>
      {open && children}
    </div>
  );
}

// ─── JsonBlock with syntax highlighting ──────────────────────────────────────

type TokType = "key" | "string" | "number" | "boolean" | "null" | "punct" | "ws";
interface Tok { type: TokType; text: string; }

function tokenizeJson(src: string): Tok[] {
  const out: Tok[] = [];
  let i = 0;
  while (i < src.length) {
    const ch = src[i];
    // whitespace
    if (/[ \t\n\r]/.test(ch)) {
      let t = ""; while (i < src.length && /[ \t\n\r]/.test(src[i])) t += src[i++];
      out.push({ type: "ws", text: t }); continue;
    }
    // string
    if (ch === '"') {
      let t = '"'; i++;
      while (i < src.length) {
        if (src[i] === "\\") { t += src[i] + (src[i + 1] ?? ""); i += 2; }
        else if (src[i] === '"') { t += '"'; i++; break; }
        else t += src[i++];
      }
      let j = i; while (j < src.length && src[j] === " ") j++;
      out.push({ type: src[j] === ":" ? "key" : "string", text: t }); continue;
    }
    // number
    if (ch === "-" || (ch >= "0" && ch <= "9")) {
      let t = ""; while (i < src.length && /[-\d.eE+]/.test(src[i])) t += src[i++];
      out.push({ type: "number", text: t }); continue;
    }
    // keywords
    if (src.startsWith("true",  i)) { out.push({ type: "boolean", text: "true"  }); i += 4; continue; }
    if (src.startsWith("false", i)) { out.push({ type: "boolean", text: "false" }); i += 5; continue; }
    if (src.startsWith("null",  i)) { out.push({ type: "null",    text: "null"  }); i += 4; continue; }
    // punctuation
    out.push({ type: "punct", text: src[i++] });
  }
  return out;
}

const JSON_COLORS: Record<TokType, string> = {
  key:     "#79b8d4",  // steel blue  — property names
  string:  "#87bd78",  // sage green  — string values
  number:  "#d49668",  // terracotta  — numbers
  boolean: "#b48ade",  // soft purple — true/false
  null:    "#7a7a9a",  // gray-purple — null
  punct:   "#3e3e4a",  // very dim    — brackets & commas
  ws:      "inherit",
};

export function JsonBlock({ content, maxHeight = 200 }: { content: unknown; maxHeight?: number }) {
  const src = typeof content === "string" ? content : formatJson(content);
  const tokens = tokenizeJson(src);
  return (
    <pre className="overflow-auto font-mono"
      style={{ fontSize: 10.5, lineHeight: 1.65, maxHeight, padding: "8px 12px", scrollbarWidth: "none" }}>
      {tokens.map((tok, idx) => (
        <span key={idx} style={{ color: JSON_COLORS[tok.type] }}>{tok.text}</span>
      ))}
    </pre>
  );
}

// ─── KVTable ──────────────────────────────────────────────────────────────────

export function KVTable({ pairs }: { pairs: [string, string][] }) {
  return (
    <div style={{ padding: "4px 12px 8px" }}>
      {pairs.map(([k, v]) => (
        <div key={k} className="flex gap-3 items-baseline" style={{ padding: "2px 0" }}>
          <span className="font-mono shrink-0 text-right" style={{ fontSize: 10.5, color: C.textDim, width: 130 }}>
            {k}
          </span>
          <span className="font-mono truncate" style={{ fontSize: 10.5, color: C.textSub }}>
            {v}
          </span>
        </div>
      ))}
    </div>
  );
}
