import { useState } from "react";
import { Settings, Cpu, Zap, ChevronRight, Terminal, X, FileText } from "lucide-react";

// Real icon assets
import claudeIcon from "@/imports/claude-code.png";
import codexIcon from "@/imports/codex.png";
import cursorIcon from "@/imports/cursor-cli.png";
import geminiIcon from "@/imports/gemini-cli.png";
import ampIcon from "@/imports/amp-code.svg";
import droidIcon from "@/imports/droid-cli-1.svg";
import piIcon from "@/imports/pi.svg";
import gooseIcon from "@/imports/goose.jpg";
import hermesIcon from "@/imports/hermes.png";

type NavSection = "general" | "providers" | "harnesses";

// --- Icon primitives ---

/** Renders a real asset (png/svg) as a harness icon */
function AssetIcon({ src, bg = "#1c1c1e", pad = 0, size = 32 }: { src: string; bg?: string; pad?: number; size?: number }) {
  return (
    <div
      className="shrink-0 rounded-[8px] overflow-hidden flex items-center justify-center"
      style={{ width: size, height: size, background: bg, padding: pad }}
    >
      <img src={src} alt="" className="w-full h-full object-contain" />
    </div>
  );
}

/** Custom SVG icon for harnesses without a provided asset */
function SvgIcon({ size = 32, bg, children }: { size?: number; bg: string; children: React.ReactNode }) {
  return (
    <div
      className="shrink-0 rounded-[8px] flex items-center justify-center"
      style={{ width: size, height: size, background: bg }}
    >
      {children}
    </div>
  );
}

function IconPi({ size = 32 }: { size?: number }) {
  return (
    <SvgIcon size={size} bg="#5B5BD6">
      <svg width={size * 0.6} height={size * 0.6} viewBox="0 0 18 18" fill="none">
        <text x="9" y="14" textAnchor="middle" fill="white" fontSize="14" fontFamily="Georgia, serif" fontWeight="bold">π</text>
      </svg>
    </SvgIcon>
  );
}

function IconGrok({ size = 32 }: { size?: number }) {
  return (
    <SvgIcon size={size} bg="#1D1D1D">
      <svg width={size * 0.55} height={size * 0.55} viewBox="0 0 18 18" fill="none">
        <path d="M3 3L9 9M9 9L15 3M9 9L3 15M9 9L15 15" stroke="white" strokeWidth="2" strokeLinecap="round" />
      </svg>
    </SvgIcon>
  );
}

function IconGoose({ size = 32 }: { size?: number }) {
  return (
    <SvgIcon size={size} bg="#FF9F0A">
      <svg width={size * 0.6} height={size * 0.6} viewBox="0 0 20 20" fill="none">
        <ellipse cx="10" cy="13" rx="5" ry="4" fill="white" />
        <ellipse cx="10" cy="8" rx="3.5" ry="3.5" fill="white" />
        <path d="M13 7.5L17 6L14.5 9.5" fill="#FF9F0A" />
        <circle cx="11.2" cy="7.2" r="0.9" fill="#1c1c1e" />
      </svg>
    </SvgIcon>
  );
}

function IconHermes({ size = 32 }: { size?: number }) {
  return (
    <SvgIcon size={size} bg="#FFD60A">
      <svg width={size * 0.55} height={size * 0.55} viewBox="0 0 18 18" fill="none">
        <line x1="9" y1="3" x2="9" y2="15" stroke="#1c1c1e" strokeWidth="1.6" strokeLinecap="round" />
        <path d="M5.5 5.5C5.5 5.5 7 8 9 6.5C11 5 12.5 7.5 12.5 7.5" stroke="#1c1c1e" strokeWidth="1.4" strokeLinecap="round" fill="none" />
        <path d="M5.5 10C5.5 10 7 12.5 9 11C11 9.5 12.5 12 12.5 12" stroke="#1c1c1e" strokeWidth="1.4" strokeLinecap="round" fill="none" />
        <path d="M7 3C7 3 6.5 2 9 1.5C11.5 2 11 3 11 3" stroke="#1c1c1e" strokeWidth="1.2" strokeLinecap="round" fill="none" />
      </svg>
    </SvgIcon>
  );
}

// Map harness id → icon renderer
function HarnessIconFor({ id, size = 32 }: { id: string; size?: number }) {
  switch (id) {
    case "claude":
      return <AssetIcon src={claudeIcon} bg="#F0EBE3" size={size} />;
    case "opencode":
      return <AssetIcon src={codexIcon} bg="#FFFFFF" size={size} />;
    case "cursor":
      return <AssetIcon src={cursorIcon} bg="#0D0D0D" size={size} />;
    case "gemini":
      return <AssetIcon src={geminiIcon} bg="#FFFFFF" size={size} />;
    case "amp":
      return <AssetIcon src={ampIcon} bg="#1A1A1A" pad={6} size={size} />;
    case "droid":
      return <AssetIcon src={droidIcon} bg="#020202" size={size} />;
    case "goose":
      return <AssetIcon src={gooseIcon} bg="#000000" size={size} />;
    case "hermes":
      return <AssetIcon src={hermesIcon} bg="#000000" size={size} />;
    case "pi":
      return <AssetIcon src={piIcon} bg="#09090b" size={size} />;
    case "grok":
      return <IconGrok size={size} />;
    case "goose":
      return <IconGoose size={size} />;
    case "hermes":
      return <IconHermes size={size} />;
    default:
      return (
        <SvgIcon size={size} bg="#2c2c2e">
          <Cpu size={size * 0.45} className="text-[#636366]" />
        </SvgIcon>
      );
  }
}

// --- Data ---

interface Harness {
  id: string;
  name: string;
  version: string;
  path: string;
  captureTools: boolean;
  hasUpdate: boolean;
}

const INITIAL_HARNESSES: Harness[] = [
  { id: "pi",       name: "Pi",       version: "0.80.7",     path: "/users/madhavejay/.pi/agent",         captureTools: true,  hasUpdate: true  },
  { id: "claude",   name: "Claude",   version: "2.1.210",    path: "/users/madhavejay/.claude",           captureTools: true,  hasUpdate: true  },
  { id: "grok",     name: "Grok",     version: "0.144.4",    path: "/users/madhavejay/.grok",             captureTools: true,  hasUpdate: false },
  { id: "amp",      name: "Amp",      version: "0.178402",   path: "/users/madhavejay/.config/amp",       captureTools: true,  hasUpdate: true  },
  { id: "gemini",   name: "Gemini",   version: "0.36.3",     path: "/users/madhavejay/.config/gemini",    captureTools: false, hasUpdate: false },
  { id: "opencode", name: "OpenCode", version: "2026.07.09", path: "/users/madhavejay/.config/opencode",  captureTools: false, hasUpdate: false },
  { id: "cursor",   name: "Cursor",   version: "2026.07.09", path: "/users/madhavejay/.cursor",           captureTools: false, hasUpdate: false },
  { id: "droid",    name: "Droid",    version: "0.167.0",    path: "/users/madhavejay/.factory",          captureTools: false, hasUpdate: false },
  { id: "goose",    name: "Goose",    version: "1.41.0",     path: "/users/madhavejay/.config/goose",     captureTools: false, hasUpdate: false },
  { id: "hermes",   name: "Hermes",   version: "0.9.1",      path: "/users/madhavejay/.config/hermes",    captureTools: false, hasUpdate: false },
];

// --- Primitives ---

function Toggle({ checked, onChange }: { checked: boolean; onChange: () => void }) {
  return (
    <button
      role="switch"
      aria-checked={checked}
      onClick={onChange}
      className={`relative inline-flex h-[18px] w-[32px] shrink-0 cursor-default items-center rounded-full transition-colors duration-200 focus:outline-none ${
        checked ? "bg-[#0a84ff]" : "bg-[#3a3a3c]"
      }`}
    >
      <span
        className={`inline-block h-[14px] w-[14px] transform rounded-full bg-white shadow-sm transition-transform duration-200 ${
          checked ? "translate-x-[16px]" : "translate-x-[2px]"
        }`}
      />
    </button>
  );
}

function Badge({ children, color = "blue" }: { children: React.ReactNode; color?: "blue" | "green" | "orange" }) {
  const colors = {
    blue:   "bg-[rgba(10,132,255,0.15)]  text-[#0a84ff]",
    green:  "bg-[rgba(48,209,88,0.15)]   text-[#30d158]",
    orange: "bg-[rgba(255,159,10,0.15)]  text-[#ff9f0a]",
  };
  return (
    <span className={`text-[10px] font-semibold px-[6px] py-[2px] rounded-[4px] ${colors[color]}`}>
      {children}
    </span>
  );
}

function SmallButton({
  children,
  variant = "default",
  onClick,
}: {
  children: React.ReactNode;
  variant?: "default" | "primary" | "danger";
  onClick?: () => void;
}) {
  const variants = {
    default: "bg-[rgba(255,255,255,0.08)] text-[#e5e5ea] hover:bg-[rgba(255,255,255,0.13)]",
    primary: "bg-[rgba(10,132,255,0.18)]  text-[#0a84ff] hover:bg-[rgba(10,132,255,0.28)]",
    danger:  "bg-[rgba(255,69,58,0.1)]    text-[#ff453a] hover:bg-[rgba(255,69,58,0.2)]",
  };
  return (
    <button
      onClick={onClick}
      className={`text-[11px] font-medium px-[8px] py-[3px] rounded-[5px] cursor-default transition-colors duration-100 whitespace-nowrap ${variants[variant]}`}
    >
      {children}
    </button>
  );
}

// --- Sidebar ---

function SidebarItem({
  label, icon, active, onClick,
}: {
  label: string; icon: React.ReactNode; active: boolean; onClick: () => void;
}) {
  return (
    <button
      onClick={onClick}
      className={`w-full flex items-center gap-[9px] px-[10px] py-[7px] rounded-[8px] cursor-default text-left transition-colors duration-100 group ${
        active
          ? "bg-[rgba(255,255,255,0.1)] text-[#e5e5ea]"
          : "text-[#636366] hover:text-[#e5e5ea] hover:bg-[rgba(255,255,255,0.05)]"
      }`}
    >
      <span className={`transition-colors duration-100 shrink-0 ${active ? "text-[#0a84ff]" : "text-[#48484a] group-hover:text-[#636366]"}`}>
        {icon}
      </span>
      <span className="text-[13px] font-medium flex-1">{label}</span>
      {active && <ChevronRight size={12} className="opacity-30 shrink-0" />}
    </button>
  );
}

// --- File changes data ---

interface FileLine {
  type: "added" | "removed" | "context";
  content: string;
}

interface FileChange {
  path: string;
  added: number;
  removed: number;
  lines: FileLine[];
}

const HARNESS_CHANGES: Record<string, FileChange[]> = {
  claude: [
    {
      path: "~/.claude/settings.json",
      added: 3, removed: 1,
      lines: [
        { type: "context",  content: '  "model": "claude-sonnet-4-5",' },
        { type: "context",  content: '  "theme": "dark",' },
        { type: "removed",  content: '  "mcp_servers": {}' },
        { type: "added",    content: '  "mcp_servers": {' },
        { type: "added",    content: '    "alexandria": { "command": "alexandria-mcp" }' },
        { type: "added",    content: '  }' },
      ],
    },
    {
      path: "~/.claude/CLAUDE.md",
      added: 5, removed: 0,
      lines: [
        { type: "context",  content: "# Project context" },
        { type: "added",    content: "" },
        { type: "added",    content: "## AlexandriaBar" },
        { type: "added",    content: "Session capture is enabled. Tool calls are" },
        { type: "added",    content: "forwarded to the AlexandriaBar harness for" },
        { type: "added",    content: "indexing and retrieval." },
      ],
    },
  ],
  amp: [
    {
      path: "~/.config/amp/settings.toml",
      added: 4, removed: 2,
      lines: [
        { type: "context",  content: "[agent]" },
        { type: "removed",  content: "model = \"claude-3-5-sonnet\"" },
        { type: "added",    content: "model = \"claude-sonnet-4-5\"" },
        { type: "context",  content: "" },
        { type: "context",  content: "[tools]" },
        { type: "removed",  content: "# no external tools" },
        { type: "added",    content: "external = [\"alexandria-mcp\"]" },
        { type: "added",    content: "capture = true" },
        { type: "added",    content: "capture_endpoint = \"localhost:7842\"" },
      ],
    },
  ],
  cursor: [
    {
      path: "~/.cursor/mcp.json",
      added: 6, removed: 0,
      lines: [
        { type: "context",  content: "{" },
        { type: "context",  content: '  "mcpServers": {' },
        { type: "added",    content: '    "alexandria": {' },
        { type: "added",    content: '      "command": "npx",' },
        { type: "added",    content: '      "args": ["-y", "alexandria-mcp"],' },
        { type: "added",    content: '      "env": { "PORT": "7842" }' },
        { type: "added",    content: "    }" },
        { type: "added",    content: "  }" },
      ],
    },
    {
      path: "~/.cursor/settings.json",
      added: 1, removed: 1,
      lines: [
        { type: "context",  content: '  "cursor.general.enableShadowWorkspace": true,' },
        { type: "removed",  content: '  "cursor.cpp.disabledLanguages": []' },
        { type: "added",    content: '  "cursor.cpp.disabledLanguages": ["plaintext"]' },
      ],
    },
  ],
  gemini: [
    {
      path: "~/.config/gemini/config.json",
      added: 3, removed: 0,
      lines: [
        { type: "context",  content: "{" },
        { type: "context",  content: '  "model": "gemini-2.0-flash",' },
        { type: "added",    content: '  "tools": {' },
        { type: "added",    content: '    "alexandria_capture": true' },
        { type: "added",    content: "  }" },
      ],
    },
  ],
  opencode: [
    {
      path: "~/.config/opencode/config.json",
      added: 4, removed: 1,
      lines: [
        { type: "context",  content: "{" },
        { type: "removed",  content: '  "providers": {}' },
        { type: "added",    content: '  "providers": {' },
        { type: "added",    content: '    "anthropic": { "model": "claude-sonnet-4-5" }' },
        { type: "added",    content: "  }," },
        { type: "added",    content: '  "mcp": ["alexandria-mcp"]' },
      ],
    },
  ],
  pi: [
    {
      path: "~/.pi/agent/config.yaml",
      added: 2, removed: 0,
      lines: [
        { type: "context",  content: "agent:" },
        { type: "context",  content: "  name: pi" },
        { type: "added",    content: "  capture_tools: true" },
        { type: "added",    content: "  mcp_endpoint: localhost:7842" },
      ],
    },
  ],
  grok: [{ path: "~/.grok/config.json", added: 0, removed: 0, lines: [{ type: "context", content: "// No changes — read-only config" }] }],
  droid: [{ path: "~/.factory/droid.toml", added: 0, removed: 0, lines: [{ type: "context", content: "# No changes — not yet configured" }] }],
  goose: [{ path: "~/.config/goose/config.yaml", added: 0, removed: 0, lines: [{ type: "context", content: "# No changes recorded" }] }],
  hermes: [{ path: "~/.config/hermes/hermes.toml", added: 0, removed: 0, lines: [{ type: "context", content: "# No changes recorded" }] }],
};

// --- Changes modal ---

function ChangesModal({ harness, onClose }: { harness: Harness; onClose: () => void }) {
  const changes = HARNESS_CHANGES[harness.id] ?? [];
  const [selectedFile, setSelectedFile] = useState(0);
  const file = changes[selectedFile];

  const totalAdded   = changes.reduce((s, f) => s + f.added,   0);
  const totalRemoved = changes.reduce((s, f) => s + f.removed, 0);

  return (
    <div
      className="absolute inset-0 z-50 flex items-center justify-center"
      style={{ background: "rgba(0,0,0,0.6)", backdropFilter: "blur(4px)" }}
      onClick={(e) => e.target === e.currentTarget && onClose()}
    >
      <div
        className="flex flex-col rounded-[12px] overflow-hidden"
        style={{
          width: 580,
          height: 420,
          background: "#222224",
          boxShadow: "0 24px 64px rgba(0,0,0,0.8), 0 0 0 0.5px rgba(255,255,255,0.1)",
        }}
      >
        {/* Header */}
        <div className="flex items-center gap-[10px] px-[16px] py-[12px] shrink-0" style={{ borderBottom: "1px solid rgba(255,255,255,0.07)" }}>
          <HarnessIconFor id={harness.id} size={24} />
          <div className="flex-1 min-w-0">
            <span className="text-[13px] font-semibold text-[#e5e5ea]">{harness.name}</span>
            <span className="text-[11px] text-[#636366] ml-[8px]">config changes</span>
          </div>
          <div className="flex items-center gap-[8px] mr-[8px]">
            {totalAdded > 0   && <span className="text-[11px] font-['Menlo',monospace] text-[#30d158]">+{totalAdded}</span>}
            {totalRemoved > 0 && <span className="text-[11px] font-['Menlo',monospace] text-[#ff453a]">−{totalRemoved}</span>}
          </div>
          <button onClick={onClose} className="text-[#48484a] hover:text-[#8e8e93] transition-colors cursor-default">
            <X size={15} />
          </button>
        </div>

        <div className="flex flex-1 min-h-0">
          {/* File list sidebar */}
          <div className="w-[180px] shrink-0 flex flex-col py-[6px] overflow-y-auto" style={{ borderRight: "1px solid rgba(255,255,255,0.06)" }}>
            {changes.map((f, i) => (
              <button
                key={i}
                onClick={() => setSelectedFile(i)}
                className={`flex items-start gap-[7px] px-[10px] py-[7px] text-left cursor-default transition-colors duration-100 ${
                  selectedFile === i ? "bg-[rgba(255,255,255,0.07)]" : "hover:bg-[rgba(255,255,255,0.04)]"
                }`}
              >
                <FileText size={12} className="text-[#48484a] mt-[1px] shrink-0" />
                <div className="min-w-0 flex-1">
                  <div className="text-[11px] text-[#e5e5ea] truncate font-['Menlo',monospace]">
                    {f.path.split("/").pop()}
                  </div>
                  <div className="flex gap-[4px] mt-[2px]">
                    {f.added   > 0 && <span className="text-[9px] font-semibold text-[#30d158]">+{f.added}</span>}
                    {f.removed > 0 && <span className="text-[9px] font-semibold text-[#ff453a]">−{f.removed}</span>}
                    {f.added === 0 && f.removed === 0 && <span className="text-[9px] text-[#48484a]">no changes</span>}
                  </div>
                </div>
              </button>
            ))}
          </div>

          {/* Diff view */}
          <div className="flex-1 flex flex-col min-w-0 min-h-0">
            {/* File path bar */}
            <div className="px-[14px] py-[8px] shrink-0" style={{ borderBottom: "1px solid rgba(255,255,255,0.05)" }}>
              <span className="text-[10px] font-['Menlo',monospace] text-[#48484a]">{file?.path}</span>
            </div>

            {/* Diff lines */}
            <div className="flex-1 overflow-y-auto py-[6px]">
              {file?.lines.map((line, i) => (
                <div
                  key={i}
                  className={`flex items-start px-[14px] py-[1px] ${
                    line.type === "added"   ? "bg-[rgba(48,209,88,0.08)]" :
                    line.type === "removed" ? "bg-[rgba(255,69,58,0.08)]" : ""
                  }`}
                >
                  <span className={`w-[14px] shrink-0 text-[11px] font-['Menlo',monospace] select-none ${
                    line.type === "added"   ? "text-[#30d158]" :
                    line.type === "removed" ? "text-[#ff453a]" : "text-[#3a3a3c]"
                  }`}>
                    {line.type === "added" ? "+" : line.type === "removed" ? "−" : " "}
                  </span>
                  <span className={`text-[11px] font-['Menlo',monospace] whitespace-pre leading-[20px] ${
                    line.type === "added"   ? "text-[#30d158]" :
                    line.type === "removed" ? "text-[#ff453a]" : "text-[#8e8e93]"
                  }`}>
                    {line.content || " "}
                  </span>
                </div>
              ))}
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}

// --- Harnesses panel ---

function HarnessRow({ harness, onToggleCapture, onUpdate, onRemove, onView }: {
  harness: Harness;
  onToggleCapture: () => void;
  onUpdate: () => void;
  onRemove: () => void;
  onView: () => void;
}) {
  const [hovered, setHovered] = useState(false);
  const changes = HARNESS_CHANGES[harness.id] ?? [];
  const totalChanged = changes.reduce((s, f) => s + f.added + f.removed, 0);

  return (
    <div
      className={`flex items-center px-[16px] py-[9px] transition-colors duration-100 ${hovered ? "bg-[rgba(255,255,255,0.04)]" : ""}`}
      onMouseEnter={() => setHovered(true)}
      onMouseLeave={() => setHovered(false)}
    >
      {/* Icon — fixed width */}
      <div className="shrink-0 mr-[10px]">
        <HarnessIconFor id={harness.id} size={32} />
      </div>

      {/* Name + path — fixed width, never grows */}
      <div className="flex flex-col min-w-0 w-[160px] shrink-0 gap-[1px]">
        <div className="flex items-center gap-[5px]">
          <span className="text-[13px] font-semibold text-[#e5e5ea] truncate">{harness.name}</span>
          <span className="text-[10px] font-['Menlo',monospace] text-[#48484a] shrink-0">v{harness.version}</span>
          {harness.hasUpdate && (
            <span className="shrink-0 text-[9px] font-semibold px-[4px] py-[1px] rounded-[3px] bg-[rgba(255,159,10,0.15)] text-[#ff9f0a] leading-tight">
              upd
            </span>
          )}
        </div>
        <span className="text-[11px] font-['Menlo',monospace] text-[#3a3a3c] truncate">{harness.path}</span>
      </div>

      {/* Spacer */}
      <div className="flex-1" />

      {/* Right side — three fixed columns, always same width */}
      <div className="flex items-center shrink-0">

        {/* Col 1 — Tools toggle, fixed width */}
        <div className="w-[80px] flex items-center justify-end gap-[6px]">
          <span className={`text-[11px] text-[#636366] transition-opacity duration-150 ${hovered ? "opacity-100" : "opacity-40"}`}>Tools</span>
          <Toggle checked={harness.captureTools} onChange={onToggleCapture} />
        </div>

        {/* Col 2 — Update button (always) or spacer, fixed width */}
        <div className="w-[72px] flex items-center justify-end pl-[8px]">
          {harness.hasUpdate && (
            <SmallButton variant="primary" onClick={onUpdate}>Update</SmallButton>
          )}
        </div>

        {/* Col 3 — view files link + remove on hover, fixed width */}
        <div className="w-[156px] flex items-center justify-end gap-[6px] pl-[6px]">
          <button
            onClick={onView}
            className={`text-[11px] font-medium px-[8px] py-[3px] rounded-[5px] cursor-default transition-all duration-150 ${
              hovered
                ? "bg-[rgba(255,255,255,0.08)] text-[#e5e5ea]"
                : "bg-[rgba(255,255,255,0.04)] text-[#48484a]"
            }`}
          >
            Config
          </button>
          <div className={`transition-opacity duration-150 ${hovered ? "opacity-100" : "opacity-0 pointer-events-none"}`}>
            <SmallButton variant="danger" onClick={onRemove}>Remove</SmallButton>
          </div>
        </div>

      </div>
    </div>
  );
}

function HarnessesPanel() {
  const [harnesses, setHarnesses] = useState(INITIAL_HARNESSES);
  const [viewing, setViewing] = useState<Harness | null>(null);
  const updatable = harnesses.filter(h => h.hasUpdate);

  return (
    <div className="flex flex-col h-full relative">
      <div className="flex items-center justify-between px-[20px] py-[16px] shrink-0">
        <div>
          <h2 className="text-[15px] font-semibold text-[#e5e5ea]">Harnesses</h2>
          <p className="text-[12px] text-[#636366] mt-[1px]">
            {harnesses.length} connected{updatable.length > 0 ? ` · ${updatable.length} with updates` : ""}
          </p>
        </div>
        {updatable.length > 0 && (
          <button
            className="text-[12px] font-medium text-[#0a84ff] hover:text-[#3a9fff] cursor-default transition-colors"
            onClick={() => setHarnesses(hs => hs.map(h => ({ ...h, hasUpdate: false })))}
          >
            Update All
          </button>
        )}
      </div>

      <div className="border-t border-[rgba(255,255,255,0.06)] mx-[20px] shrink-0" />

      <div className="flex-1 overflow-y-auto py-[4px]">
        {harnesses.map((h, i) => (
          <div key={h.id}>
            <HarnessRow
              harness={h}
              onToggleCapture={() => setHarnesses(hs => hs.map(x => x.id === h.id ? { ...x, captureTools: !x.captureTools } : x))}
              onUpdate={() => setHarnesses(hs => hs.map(x => x.id === h.id ? { ...x, hasUpdate: false } : x))}
              onRemove={() => setHarnesses(hs => hs.filter(x => x.id !== h.id))}
              onView={() => setViewing(h)}
            />
            {i < harnesses.length - 1 && (
              <div className="border-t border-[rgba(255,255,255,0.04)] mx-[16px]" />
            )}
          </div>
        ))}
      </div>

      {viewing && <ChangesModal harness={viewing} onClose={() => setViewing(null)} />}
    </div>
  );
}

// --- General panel ---

function SettingRow({ label, hint, children }: { label: string; hint?: string; children: React.ReactNode }) {
  return (
    <div className="flex items-center justify-between py-[11px]">
      <div>
        <div className="text-[13px] text-[#e5e5ea]">{label}</div>
        {hint && <div className="text-[11px] text-[#48484a] mt-[1px]">{hint}</div>}
      </div>
      {children}
    </div>
  );
}

function SectionLabel({ title }: { title: string }) {
  return <p className="text-[10px] font-semibold text-[#48484a] uppercase tracking-[0.07em] pt-[14px] pb-[4px]">{title}</p>;
}

function GeneralPanel() {
  const [notifications, setNotifications] = useState(true);
  const [autoUpdate, setAutoUpdate] = useState(true);
  const [startAtLogin, setStartAtLogin] = useState(false);
  const [showMenuBar, setShowMenuBar] = useState(true);
  const [telemetry, setTelemetry] = useState(false);

  return (
    <div className="flex flex-col h-full">
      <div className="px-[24px] pt-[16px] pb-[12px] shrink-0">
        <h2 className="text-[15px] font-semibold text-[#e5e5ea]">General</h2>
        <p className="text-[12px] text-[#636366] mt-[1px]">App behavior and preferences</p>
      </div>
      <div className="border-t border-[rgba(255,255,255,0.06)] mx-[24px] shrink-0" />
      <div className="flex-1 overflow-y-auto px-[24px] pb-[20px]">
        <SectionLabel title="System" />
        <div className="divide-y divide-[rgba(255,255,255,0.05)]">
          <SettingRow label="Launch at login" hint="Start AlexandriaBar when you log in">
            <Toggle checked={startAtLogin} onChange={() => setStartAtLogin(v => !v)} />
          </SettingRow>
          <SettingRow label="Show in menu bar">
            <Toggle checked={showMenuBar} onChange={() => setShowMenuBar(v => !v)} />
          </SettingRow>
          <SettingRow label="Auto-update" hint="Automatically install updates">
            <Toggle checked={autoUpdate} onChange={() => setAutoUpdate(v => !v)} />
          </SettingRow>
        </div>

        <SectionLabel title="Notifications" />
        <div className="divide-y divide-[rgba(255,255,255,0.05)]">
          <SettingRow label="Enable notifications">
            <Toggle checked={notifications} onChange={() => setNotifications(v => !v)} />
          </SettingRow>
        </div>

        <SectionLabel title="Privacy" />
        <div className="divide-y divide-[rgba(255,255,255,0.05)]">
          <SettingRow label="Usage telemetry" hint="Help improve AlexandriaBar anonymously">
            <Toggle checked={telemetry} onChange={() => setTelemetry(v => !v)} />
          </SettingRow>
        </div>

        <SectionLabel title="About" />
        <div className="divide-y divide-[rgba(255,255,255,0.05)]">
          <SettingRow label="Version">
            <span className="text-[11px] font-['Menlo',monospace] text-[#636366]">v1.4.2</span>
          </SettingRow>
          <SettingRow label="Check for updates">
            <SmallButton>Check now</SmallButton>
          </SettingRow>
        </div>
      </div>
    </div>
  );
}

// --- Providers panel ---

interface Provider {
  id: string;
  name: string;
  configured: boolean;
  model: string;
  iconId: string;
}

const PROVIDERS: Provider[] = [
  { id: "anthropic", name: "Anthropic", configured: true,  model: "claude-sonnet-4-5",   iconId: "claude"   },
  { id: "openai",    name: "OpenAI",    configured: true,  model: "gpt-4o",               iconId: "opencode" },
  { id: "google",    name: "Google",    configured: false, model: "gemini-2.0-flash",     iconId: "gemini"   },
  { id: "xai",       name: "xAI",       configured: true,  model: "grok-2",               iconId: "grok"     },
];

function ProvidersPanel() {
  return (
    <div className="flex flex-col h-full">
      <div className="px-[24px] pt-[16px] pb-[12px] shrink-0">
        <h2 className="text-[15px] font-semibold text-[#e5e5ea]">Providers</h2>
        <p className="text-[12px] text-[#636366] mt-[1px]">API keys and model selection</p>
      </div>
      <div className="border-t border-[rgba(255,255,255,0.06)] mx-[24px] shrink-0" />
      <div className="flex-1 overflow-y-auto py-[10px] px-[20px] flex flex-col gap-[6px]">
        {PROVIDERS.map(p => (
          <div
            key={p.id}
            className="flex items-center gap-[12px] p-[12px] rounded-[10px] bg-[rgba(255,255,255,0.04)] border border-[rgba(255,255,255,0.06)] hover:bg-[rgba(255,255,255,0.06)] transition-colors duration-100 cursor-default"
          >
            <HarnessIconFor id={p.iconId} size={32} />
            <div className="flex-1 min-w-0">
              <div className="flex items-center gap-[6px]">
                <span className="text-[13px] font-semibold text-[#e5e5ea]">{p.name}</span>
                <Badge color={p.configured ? "green" : "orange"}>
                  {p.configured ? "configured" : "not set"}
                </Badge>
              </div>
              <span className="text-[11px] font-['Menlo',monospace] text-[#48484a]">{p.model}</span>
            </div>
            <SmallButton>Configure</SmallButton>
          </div>
        ))}

        <button className="flex items-center justify-center gap-[6px] py-[10px] rounded-[10px] border border-dashed border-[rgba(255,255,255,0.1)] text-[12px] text-[#48484a] hover:text-[#636366] hover:border-[rgba(255,255,255,0.18)] transition-all duration-100 cursor-default mt-[2px]">
          <span className="text-[16px] leading-none mb-[1px]">+</span>
          Add provider
        </button>
      </div>
    </div>
  );
}

// --- Root ---

export default function App() {
  const [activeSection, setActiveSection] = useState<NavSection>("harnesses");

  const navItems: { id: NavSection; label: string; icon: React.ReactNode }[] = [
    { id: "general",   label: "General",   icon: <Settings size={15} /> },
    { id: "providers", label: "Providers", icon: <Zap size={15} /> },
    { id: "harnesses", label: "Harnesses", icon: <Terminal size={15} /> },
  ];

  return (
    <div className="min-h-screen bg-[#111111] flex items-center justify-center p-8">
      <div
        className="flex rounded-[14px] overflow-hidden"
        style={{
          width: 720,
          height: 540,
          background: "#1c1c1e",
          boxShadow: "0 32px 96px rgba(0,0,0,0.85), 0 0 0 0.5px rgba(255,255,255,0.08)",
        }}
      >
        {/* Sidebar */}
        <div
          className="w-[180px] shrink-0 flex flex-col"
          style={{ borderRight: "1px solid rgba(255,255,255,0.07)", background: "rgba(0,0,0,0.25)" }}
        >
          {/* Traffic lights */}
          <div className="h-[52px] flex items-end px-[16px] pb-[12px] shrink-0">
            <div className="flex gap-[6px]">
              <div className="w-[12px] h-[12px] rounded-full bg-[#ff5f57]" />
              <div className="w-[12px] h-[12px] rounded-full bg-[#febc2e]" />
              <div className="w-[12px] h-[12px] rounded-full bg-[#28c840]" />
            </div>
          </div>

          <div className="px-[14px] pb-[12px] shrink-0">
            <p className="text-[10px] font-semibold text-[#48484a] uppercase tracking-[0.07em]">AlexandriaBar</p>
          </div>

          <nav className="flex-1 px-[8px] flex flex-col gap-[2px]">
            {navItems.map(item => (
              <SidebarItem
                key={item.id}
                label={item.label}
                icon={item.icon}
                active={activeSection === item.id}
                onClick={() => setActiveSection(item.id)}
              />
            ))}
          </nav>

          <div className="p-[12px]">
            <div className="text-[10px] text-[#3a3a3c] text-center font-['Menlo',monospace]">v1.4.2</div>
          </div>
        </div>

        {/* Content */}
        <div className="flex-1 flex flex-col min-w-0">
          <div
            className="h-[52px] shrink-0 flex items-center justify-center"
            style={{ borderBottom: "1px solid rgba(255,255,255,0.07)" }}
          >
            <span className="text-[13px] font-semibold text-[#636366]">Settings</span>
          </div>

          <div className="flex-1 overflow-hidden">
            {activeSection === "general"   && <GeneralPanel />}
            {activeSection === "providers" && <ProvidersPanel />}
            {activeSection === "harnesses" && <HarnessesPanel />}
          </div>
        </div>
      </div>
    </div>
  );
}
