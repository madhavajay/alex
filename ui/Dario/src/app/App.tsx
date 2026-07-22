import { useState } from "react";
import svgPaths from "@/imports/DarioProxyStatus/svg-42ls8ze1sd";

// --- SVG Icons ---
function SearchIcon() {
  return (
    <div className="relative shrink-0 size-[11px]">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 11 11">
        <g clipPath="url(#search-clip)">
          <path d={svgPaths.pa33b280} stroke="#636366" strokeLinecap="round" />
        </g>
        <defs>
          <clipPath id="search-clip"><rect fill="white" height="11" width="11" /></clipPath>
        </defs>
      </svg>
    </div>
  );
}

function ChevronRightIcon({ color = "#636366" }: { color?: string }) {
  return (
    <div className="relative shrink-0 size-[11px]">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 11 11">
        <path d={svgPaths.p1a78e480} stroke={color} strokeLinecap="round" strokeWidth="2" />
      </svg>
    </div>
  );
}

function TerminalIcon({ color = "#AEAEB2" }: { color?: string }) {
  return (
    <div className="relative shrink-0 size-[9px]">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 9 9">
        <g clipPath="url(#term-clip)">
          <path d={svgPaths.p3128be40} stroke={color} strokeLinecap="round" strokeWidth="2" />
        </g>
        <defs>
          <clipPath id="term-clip"><rect fill="white" height="9" width="9" /></clipPath>
        </defs>
      </svg>
    </div>
  );
}

function CpuIcon() {
  return (
    <div className="relative shrink-0 size-[14px]">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 14 14">
        <g clipPath="url(#cpu-clip)">
          <path d={svgPaths.p188e8800} stroke="#0A84FF" strokeLinecap="round" strokeWidth="2" />
        </g>
        <defs>
          <clipPath id="cpu-clip"><rect fill="white" height="14" width="14" /></clipPath>
        </defs>
      </svg>
    </div>
  );
}

function DotIcon({ color }: { color: string }) {
  return (
    <div className="relative shrink-0 size-[5px]">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 5 5">
        <circle cx="2.5" cy="2.5" fill={color} r="2.5" />
      </svg>
    </div>
  );
}

// --- Session Data ---
type FilterType = "All" | "Running" | "Error" | "Done";

interface Session {
  id: string;
  model: string;
  modelColor: "blue" | "purple";
  turns: number;
  cost: string;
  duration: string;
  account: string;
  status: "running" | "error" | "done";
  children?: Session[];
}

const SESSIONS: Session[] = [
  {
    id: "s1",
    model: "opus 4.8",
    modelColor: "purple",
    turns: 7,
    cost: "$0.84",
    duration: "42.1s",
    account: "prod-api",
    status: "running",
    children: [
      { id: "s1a", model: "sonnet 4.6", modelColor: "blue", turns: 6, cost: "$0.18", duration: "8.4s", account: "prod-api", status: "running" },
    ],
  },
  {
    id: "s2",
    model: "sonnet 4.6",
    modelColor: "blue",
    turns: 3,
    cost: "$0.12",
    duration: "8.9s",
    account: "staging",
    status: "error",
  },
];

function ModelBadge({ model, color }: { model: string; color: "blue" | "purple" }) {
  const styles = color === "purple"
    ? { bg: "bg-[rgba(191,90,242,0.12)]", border: "border-[rgba(191,90,242,0.25)]", text: "text-[#bf5af2]" }
    : { bg: "bg-[rgba(10,132,255,0.12)]", border: "border-[rgba(10,132,255,0.25)]", text: "text-[#0a84ff]" };

  return (
    <div className={`${styles.bg} relative flex items-start px-[6px] py-[2px] rounded-[5px] shrink-0`}>
      <div aria-hidden className={`absolute border ${styles.border} border-solid inset-0 pointer-events-none rounded-[5px]`} />
      <span className={`font-['JetBrains_Mono',monospace] font-medium ${styles.text} text-[9.5px] whitespace-nowrap`}>{model}</span>
    </div>
  );
}

function HarnessBox({ color = "blue" }: { color?: "blue" | "purple" }) {
  const bg = color === "purple" ? "bg-[rgba(191,90,242,0.15)]" : "bg-[rgba(10,132,255,0.18)]";
  const termColor = color === "purple" ? "#bf5af2" : "#AEAEB2";
  return (
    <div className={`${bg} flex items-center justify-center rounded-[3px] shrink-0 size-[17px]`}>
      <TerminalIcon color={termColor} />
    </div>
  );
}

function ProviderBox() {
  return (
    <div className="bg-[rgba(255,144,64,0.18)] flex items-center justify-center rounded-[3px] shrink-0 size-[17px]">
      <span className="font-['Inter',sans-serif] font-bold text-[#ff9040] text-[9px]">A</span>
    </div>
  );
}

function SessionRowItem({
  session,
  isActive,
  isChild,
  onClick,
}: {
  session: Session;
  isActive: boolean;
  isChild?: boolean;
  onClick: () => void;
}) {
  const statusDot = session.status === "error" ? "#FF453A" : "#30D158";
  const harnessColor = session.model.includes("opus") ? "purple" : "blue";

  return (
    <div
      onClick={onClick}
      className={`h-[30px] relative shrink-0 w-full cursor-pointer transition-colors ${
        isActive ? "bg-[rgba(10,132,255,0.07)]" : "hover:bg-[rgba(255,255,255,0.03)]"
      }`}
    >
      {isActive && (
        <div aria-hidden className="absolute border-[#0a84ff] border-r-2 border-solid inset-0 pointer-events-none" />
      )}
      <div className="flex items-center h-full pr-[8px]">
        {/* session col */}
        <div className="flex items-center gap-[5px] flex-1 min-w-0 pl-[4px]">
          {isChild ? (
            <div className="flex items-center pl-[8px] shrink-0 w-[24px]">
              <div className="bg-[#636366] h-[12px] w-px shrink-0" />
              <div className="bg-[#636366] h-px w-[8px] shrink-0" />
            </div>
          ) : (
            <ChevronRightIcon color={isActive ? "#0a84ff" : "#636366"} />
          )}
          <DotIcon color={statusDot} />
          <HarnessBox color={harnessColor} />
          <ProviderBox />
          <ModelBadge model={session.model} color={session.modelColor} />
        </div>
        {/* metrics */}
        <div className="font-['JetBrains_Mono',monospace] font-normal text-[#636366] text-[10.5px] whitespace-nowrap w-[28px] text-right shrink-0">{session.turns}</div>
        <div className="font-['JetBrains_Mono',monospace] font-normal text-[#636366] text-[10.5px] whitespace-nowrap w-[40px] text-right shrink-0">{session.cost}</div>
        <div className="font-['JetBrains_Mono',monospace] font-normal text-[#636366] text-[10px] whitespace-nowrap w-[36px] text-right shrink-0">{session.duration}</div>
        <div className="font-['Inter',sans-serif] font-normal text-[#636366] text-[10px] whitespace-nowrap w-[44px] text-right shrink-0 overflow-hidden text-ellipsis">{session.account}</div>
      </div>
    </div>
  );
}

// --- Sidebar ---
function Sidebar({
  activeId,
  onSelect,
  filter,
  onFilter,
  search,
  onSearch,
}: {
  activeId: string;
  onSelect: (id: string) => void;
  filter: FilterType;
  onFilter: (f: FilterType) => void;
  search: string;
  onSearch: (s: string) => void;
}) {
  const filters: FilterType[] = ["All", "Running", "Error", "Done"];

  const visible = SESSIONS.filter((s) => {
    if (filter !== "All" && s.status !== filter.toLowerCase()) return false;
    if (search && !s.model.toLowerCase().includes(search.toLowerCase()) && !s.account.toLowerCase().includes(search.toLowerCase())) return false;
    return true;
  });

  const total = SESSIONS.reduce((n, s) => n + 1 + (s.children?.length ?? 0), 0);

  return (
    <div className="content-stretch flex flex-col h-full relative shrink-0 w-[340px]">
      <div aria-hidden className="absolute border-[rgba(255,255,255,0.07)] border-r border-solid inset-0 pointer-events-none" />

      {/* Panel header */}
      <div className="h-[48px] relative shrink-0 w-full">
        <div aria-hidden className="absolute border-[rgba(255,255,255,0.07)] border-b border-solid inset-0 pointer-events-none" />
        <div className="flex items-center h-full pl-[12px] pr-[8px] gap-[6.5px]">
          <span className="font-['Inter',sans-serif] font-semibold text-[#e5e5ea] text-[13px] whitespace-nowrap">Sessions</span>
          <div className="bg-[rgba(255,255,255,0.06)] px-[6px] py-[2px] rounded-[4px]">
            <span className="font-['JetBrains_Mono',monospace] font-normal text-[#8e8e93] text-[10px]">{total}</span>
          </div>
        </div>
      </div>

      {/* Search */}
      <div className="bg-[#1c1c1e] h-[40px] relative shrink-0 w-full">
        <div aria-hidden className="absolute border-[rgba(255,255,255,0.07)] border-b border-solid inset-0 pointer-events-none" />
        <div className="flex items-center h-full px-[10px]">
          <div className="bg-[rgba(255,255,255,0.06)] flex-1 h-[28px] relative rounded-[8px]">
            <div aria-hidden className="absolute border border-[rgba(255,255,255,0.07)] border-solid inset-0 pointer-events-none rounded-[8px]" />
            <div className="flex items-center h-full px-[8px] gap-[6.5px]">
              <SearchIcon />
              <input
                className="flex-1 min-w-0 bg-transparent font-['JetBrains_Mono',monospace] font-normal text-[11px] text-[#e5e5ea] placeholder:text-[rgba(229,229,234,0.5)] outline-none"
                placeholder="Search sessions..."
                value={search}
                onChange={(e) => onSearch(e.target.value)}
              />
            </div>
          </div>
        </div>
      </div>

      {/* Column header */}
      <div className="h-[24px] relative shrink-0 w-full">
        <div aria-hidden className="absolute border-[rgba(255,255,255,0.07)] border-b border-solid inset-0 pointer-events-none" />
        <div className="flex items-center h-full pl-[20px] pr-[8px]">
          <span className="font-['Inter',sans-serif] font-medium text-[#636366] text-[10px] flex-1">Session</span>
          <span className="font-['Inter',sans-serif] font-normal text-[#636366] text-[10px] text-right w-[28px]">T</span>
          <span className="font-['Inter',sans-serif] font-normal text-[#636366] text-[10px] text-right w-[40px]">Cost</span>
          <span className="font-['Inter',sans-serif] font-normal text-[#636366] text-[10px] text-right w-[36px]">Dur</span>
          <span className="font-['Inter',sans-serif] font-normal text-[#636366] text-[10px] text-right w-[44px]">Account</span>
        </div>
      </div>

      {/* Filter tabs */}
      <div className="h-[32px] relative shrink-0 w-full">
        <div aria-hidden className="absolute border-[rgba(255,255,255,0.07)] border-b border-solid inset-0 pointer-events-none" />
        <div className="flex items-center h-full px-[10px] gap-[5px]">
          {filters.map((f) => (
            <button
              key={f}
              onClick={() => onFilter(f)}
              className={`px-[10px] py-[4px] rounded-[6px] font-['Inter',sans-serif] font-medium text-[10px] whitespace-nowrap transition-colors cursor-pointer ${
                filter === f
                  ? "bg-[rgba(255,255,255,0.1)] text-[#e5e5ea]"
                  : "text-[#636366] hover:text-[#8e8e93]"
              }`}
            >
              {f}
            </button>
          ))}
        </div>
      </div>

      {/* Session list */}
      <div className="flex-1 min-h-0 overflow-y-auto overflow-x-hidden">
        {visible.map((session) => (
          <div key={session.id}>
            <SessionRowItem
              session={session}
              isActive={activeId === session.id}
              onClick={() => onSelect(session.id)}
            />
            {session.children?.map((child) => (
              <SessionRowItem
                key={child.id}
                session={child}
                isActive={activeId === child.id}
                isChild
                onClick={() => onSelect(child.id)}
              />
            ))}
          </div>
        ))}
      </div>

      {/* Footer */}
      <div className="h-[28px] relative shrink-0 w-full">
        <div aria-hidden className="absolute border-[rgba(255,255,255,0.07)] border-solid border-t inset-0 pointer-events-none" />
        <div className="flex items-center h-full px-[12px]">
          <span className="font-['JetBrains_Mono',monospace] font-normal text-[#636366] text-[10px] whitespace-nowrap">
            {visible.length + visible.reduce((n, s) => n + (s.children?.length ?? 0), 0)} of {total} sessions
          </span>
        </div>
      </div>
    </div>
  );
}

// --- Main Dashboard ---
function Header({ onRestart, onCheckUpdate }: { onRestart: () => void; onCheckUpdate: () => void }) {
  return (
    <div className="h-[48px] relative shrink-0 w-full">
      <div aria-hidden className="absolute border-[rgba(255,255,255,0.07)] border-b border-solid inset-0 pointer-events-none" />
      <div className="flex items-center justify-between h-full px-[20px]">
        {/* Left */}
        <div className="flex items-center gap-[12px]">
          <div className="bg-[rgba(10,132,255,0.15)] flex items-center justify-center rounded-[8px] size-[30px] relative">
            <div aria-hidden className="absolute border border-[rgba(10,132,255,0.22)] border-solid inset-0 pointer-events-none rounded-[8px]" />
            <CpuIcon />
          </div>
          <div className="flex flex-col items-start leading-[normal]">
            <span className="font-['Inter',sans-serif] font-semibold text-[#e5e5ea] text-[14px] whitespace-nowrap">Alex - Dario</span>
            <span className="font-['Inter',sans-serif] font-normal text-[#636366] text-[10px] whitespace-nowrap">Dario 5.1.1 - active gen-5.1.1-50932</span>
          </div>
        </div>
        {/* Right */}
        <div className="flex items-center gap-[8px]">
          {[{ label: "Restart", fn: onRestart }, { label: "Check Update", fn: onCheckUpdate }].map(({ label, fn }) => (
            <button
              key={label}
              onClick={fn}
              className="bg-[rgba(255,255,255,0.06)] relative flex items-center justify-center px-[12px] py-[6px] rounded-[6px] cursor-pointer hover:bg-[rgba(255,255,255,0.1)] transition-colors"
            >
              <div aria-hidden className="absolute border border-[rgba(255,255,255,0.07)] border-solid inset-0 pointer-events-none rounded-[6px]" />
              <span className="font-['Inter',sans-serif] font-semibold text-[#e5e5ea] text-[11px] whitespace-nowrap">{label}</span>
            </button>
          ))}
        </div>
      </div>
    </div>
  );
}

function GenerationSection() {
  return (
    <div className="relative shrink-0 w-full">
      <div className="flex flex-col gap-[8px] items-start p-[16px]">
        <span className="font-['Inter',sans-serif] font-semibold text-[#8e8e93] text-[11px] uppercase tracking-wider">GENERATION</span>
        <div className="relative rounded-[8px] w-full">
          <div className="flex flex-col items-start overflow-clip rounded-[inherit]">
            {/* Gen header */}
            <div className="bg-[#141414] h-[28px] w-full">
              <div className="flex items-center h-full px-[16px] gap-0">
                {[
                  { label: "generation", w: "w-[140px]" },
                  { label: "version", w: "w-[60px]" },
                  { label: "phase", w: "w-[80px]" },
                  { label: "port", w: "w-[60px]" },
                  { label: "pid", w: "w-[60px]" },
                  { label: "busy", w: "w-[40px]" },
                  { label: "probe", w: "w-[60px]" },
                  { label: "age", w: "w-[60px] text-right" },
                ].map(({ label, w }) => (
                  <div key={label} className={`${w} font-['Inter',sans-serif] font-semibold text-[#636366] text-[10.5px] flex-shrink-0`}>
                    {label}
                  </div>
                ))}
              </div>
            </div>
            {/* Gen row */}
            <div className="bg-[rgba(10,132,255,0.07)] h-[36px] w-full relative">
              <div aria-hidden className="absolute border-[rgba(255,255,255,0.07)] border-solid border-t inset-0 pointer-events-none" />
              <div className="flex items-center h-full px-[16px]">
                <div className="w-[140px] shrink-0">
                  <span className="font-['JetBrains_Mono',monospace] font-bold text-[#0a84ff] text-[11px]">gen-5.1.1-50932</span>
                </div>
                <div className="w-[60px] shrink-0">
                  <span className="font-['JetBrains_Mono',monospace] font-normal text-[#e5e5ea] text-[11px]">5.1.1</span>
                </div>
                <div className="w-[80px] shrink-0">
                  <span className="font-['JetBrains_Mono',monospace] font-bold text-[#30d158] text-[11px]">ready</span>
                </div>
                <div className="w-[60px] shrink-0">
                  <span className="font-['JetBrains_Mono',monospace] font-normal text-[#e5e5ea] text-[11px]">50932</span>
                </div>
                <div className="w-[60px] shrink-0">
                  <span className="font-['JetBrains_Mono',monospace] font-normal text-[#e5e5ea] text-[11px]">68113</span>
                </div>
                <div className="w-[40px] shrink-0">
                  <span className="font-['JetBrains_Mono',monospace] font-normal text-[#e5e5ea] text-[11px]">0</span>
                </div>
                <div className="w-[60px] shrink-0">
                  <span className="font-['JetBrains_Mono',monospace] font-normal text-[#636366] text-[11px]">-</span>
                </div>
                <div className="w-[60px] shrink-0 text-right">
                  <span className="font-['JetBrains_Mono',monospace] font-normal text-[#e5e5ea] text-[11px]">59s</span>
                </div>
              </div>
            </div>
          </div>
          <div aria-hidden className="absolute border border-[rgba(255,255,255,0.07)] border-solid inset-0 pointer-events-none rounded-[8px]" />
        </div>
      </div>
    </div>
  );
}

interface CacheEntry {
  model: string;
  path: string;
  status: string;
  statusColor: string;
  chars: number;
  version: string;
  lastUsed: string;
}

const CACHE_DATA: CacheEntry[] = [
  {
    model: "claude-haiku-4-5",
    path: "/Users/mochav/dev/.alex/dario-prompt-cache/claude-haiku-4-5-f73a3fface2eb.json",
    status: "hit",
    statusColor: "#30d158",
    chars: 26941,
    version: "2.1.207",
    lastUsed: "-",
  },
  {
    model: "claude-opus-4-8",
    path: "/Users/mochav/dev/.alex/dario-prompt-cache/claude-opus-4-8-efaeac877ff7.json",
    status: "hit",
    statusColor: "#30d158",
    chars: 5440,
    version: "2.1.207",
    lastUsed: "-",
  },
];

function PromptCacheSection({ onClear }: { onClear: (model: string) => void }) {
  return (
    <div className="relative shrink-0 w-full">
      <div className="flex flex-col gap-[8px] items-start pb-[16px] px-[16px]">
        <span className="font-['Inter',sans-serif] font-semibold text-[#8e8e93] text-[11px] uppercase tracking-wider">PROMPT CACHE</span>
        <div className="relative rounded-[8px] w-full">
          <div className="flex flex-col items-start overflow-clip rounded-[inherit]">
            {/* Cache header */}
            <div className="bg-[#141414] h-[28px] w-full">
              <div className="flex items-center h-full px-[16px]">
                <div className="w-[260px] shrink-0 font-['Inter',sans-serif] font-semibold text-[#636366] text-[10.5px]">cache</div>
                <div className="w-[60px] shrink-0 font-['Inter',sans-serif] font-semibold text-[#636366] text-[10.5px]">status</div>
                <div className="w-[60px] shrink-0 font-['Inter',sans-serif] font-semibold text-[#636366] text-[10.5px]">chars</div>
                <div className="w-[70px] shrink-0 font-['Inter',sans-serif] font-semibold text-[#636366] text-[10.5px]">version</div>
                <div className="w-[80px] shrink-0 font-['Inter',sans-serif] font-semibold text-[#636366] text-[10.5px]">last used</div>
                <div className="w-[120px] shrink-0 font-['Inter',sans-serif] font-semibold text-[#636366] text-[10.5px] text-right">action</div>
              </div>
            </div>
            {/* Cache rows */}
            {CACHE_DATA.map((entry) => (
              <div key={entry.model} className="h-[48px] w-full relative">
                <div aria-hidden className="absolute border-[rgba(255,255,255,0.07)] border-solid border-t inset-0 pointer-events-none" />
                <div className="flex items-center h-full px-[16px]">
                  <div className="w-[260px] shrink-0 flex flex-col gap-[2px]">
                    <span className="font-['JetBrains_Mono',monospace] font-bold text-[#e5e5ea] text-[11px]">{entry.model}</span>
                    <span className="font-['JetBrains_Mono',monospace] font-normal text-[#636366] text-[9.5px] overflow-hidden text-ellipsis whitespace-nowrap">{entry.path}</span>
                  </div>
                  <div className="w-[60px] shrink-0">
                    <span className="font-['JetBrains_Mono',monospace] font-normal text-[11px]" style={{ color: entry.statusColor }}>{entry.status}</span>
                  </div>
                  <div className="w-[60px] shrink-0">
                    <span className="font-['JetBrains_Mono',monospace] font-normal text-[#e5e5ea] text-[11px]">{entry.chars.toLocaleString()}</span>
                  </div>
                  <div className="w-[70px] shrink-0">
                    <span className="font-['JetBrains_Mono',monospace] font-normal text-[#e5e5ea] text-[11px]">{entry.version}</span>
                  </div>
                  <div className="w-[80px] shrink-0">
                    <span className="font-['JetBrains_Mono',monospace] font-normal text-[#636366] text-[11px]">{entry.lastUsed}</span>
                  </div>
                  <div className="w-[120px] shrink-0">
                    <button
                      onClick={() => onClear(entry.model)}
                      className="bg-[rgba(255,255,255,0.06)] relative flex items-center px-[8px] py-[3px] rounded-[4px] cursor-pointer hover:bg-[rgba(255,255,255,0.1)] transition-colors"
                    >
                      <div aria-hidden className="absolute border border-[rgba(255,255,255,0.07)] border-solid inset-0 pointer-events-none rounded-[4px]" />
                      <span className="font-['Inter',sans-serif] font-medium text-[#e5e5ea] text-[9.5px]">Clear</span>
                    </button>
                  </div>
                </div>
              </div>
            ))}
          </div>
          <div aria-hidden className="absolute border border-[rgba(255,255,255,0.07)] border-solid inset-0 pointer-events-none rounded-[8px]" />
        </div>
      </div>
    </div>
  );
}

type LogTab = "stdout" | "stderr";

const STDOUT_LINES: { text: string; color: string }[] = [
  { text: "Device identity: detected", color: "#e5e5ea" },
  { text: "dario | template: live capture, DE v2.1.210 (2h old)", color: "#8e8e93" },
  { text: "dario | * DE compat: installed DE v2.1.210 is newer than dario's last tested version (v2.1.209): usually fine, but untested", color: "#8e8e93" },
  { text: "dario | * TLS Fingerprint: Node v23.6.1 - Bun v1.3.14 on PATH but auto-relaunch bypassed (DARIO_NO_RUN)", color: "#8e8e93" },
  { text: "dario | + unset DARIO_NO_RUN to auto-relaunch under Bun on the next invocation.", color: "#8e8e93" },
  { text: "dario | (silence with DARIO_QUIET_TLS=1, or use --strict-tls to hard-fail)", color: "#8e8e93" },
  { text: "dario - http://localhost:50932", color: "#0a84ff" },
  { text: "Your Claude subscription is now an API.", color: "#30d158" },
  { text: "Usage:", color: "#aeaeb2" },
  { text: "  ANTHROPIC_BASE_URL=http://localhost:50932", color: "#aeaeb2" },
  { text: "  ANTHROPIC_API_KEY=dario", color: "#aeaeb2" },
  { text: "OAuth: healthy (expires in 3h 28m)", color: "#30d158" },
  { text: "Model: passthrough (client decides)", color: "#e5e5ea" },
  { text: "dario | 1 account (a pool of one) - add more with 'dario accounts add <alias>' to load-balance", color: "#8e8e93" },
];

const STDERR_LINES: { text: string; color: string }[] = [
  { text: "[warn] TLS cert self-signed — browser may flag it", color: "#ff9f0a" },
  { text: "[warn] Rate limit approaching: 89% of hourly quota used", color: "#ff9f0a" },
];

function TerminalLogSection({ activeTab, onTab }: { activeTab: LogTab; onTab: (t: LogTab) => void }) {
  const lines = activeTab === "stdout" ? STDOUT_LINES : STDERR_LINES;
  return (
    <div className="flex-1 min-h-0 w-full flex flex-col">
      <div className="flex flex-col gap-[8px] items-start pb-[16px] px-[16px] h-full">
        {/* Tab bar */}
        <div className="flex gap-[8px] items-center h-[28px] w-full shrink-0">
          {(["stdout", "stderr"] as LogTab[]).map((tab) => (
            <button
              key={tab}
              onClick={() => onTab(tab)}
              className={`flex items-center px-[10px] py-[4px] rounded-[6px] cursor-pointer transition-colors font-['Inter',sans-serif] text-[10.5px] whitespace-nowrap ${
                activeTab === tab
                  ? "bg-[#0a84ff] font-semibold text-[#e5e5ea]"
                  : "bg-[rgba(255,255,255,0.06)] font-medium text-[#8e8e93] hover:text-[#e5e5ea]"
              }`}
            >
              {tab}
            </button>
          ))}
          <span className="font-['JetBrains_Mono',monospace] font-normal text-[#636366] text-[10px] whitespace-nowrap">gen-5.1.1-50932</span>
        </div>
        {/* Console */}
        <div className="bg-[#111112] flex-1 min-h-0 relative rounded-[8px] w-full">
          <div className="overflow-y-auto rounded-[inherit] size-full">
            <div className="flex flex-col gap-[4px] items-start font-['JetBrains_Mono',monospace] font-normal p-[12px] text-[10.5px] leading-[15.5px]">
              {lines.map((line, i) => (
                <p key={i} className="w-full" style={{ color: line.color }}>{line.text}</p>
              ))}
            </div>
          </div>
          <div aria-hidden className="absolute border border-[rgba(255,255,255,0.07)] border-solid inset-0 pointer-events-none rounded-[8px]" />
        </div>
      </div>
    </div>
  );
}

function SubtitleHelp() {
  return (
    <div className="bg-[#141414] relative shrink-0 w-full">
      <div aria-hidden className="absolute border-[rgba(255,255,255,0.07)] border-b border-solid inset-0 pointer-events-none" />
      <div className="flex items-start px-[20px] py-[8px]">
        <p className="font-['JetBrains_Mono',monospace] font-normal text-[#636366] text-[10.5px] leading-[normal] flex-1">
          {"Logs path: /generative-health.html logs. Dario-routed traffic shows up in the Trace Browser under account "}
          <span className="text-[#0a84ff]">{"demo:<generation>"}</span>
        </p>
      </div>
    </div>
  );
}

// --- Root ---
export default function App() {
  const [activeSessionId, setActiveSessionId] = useState("s1");
  const [filter, setFilter] = useState<FilterType>("All");
  const [search, setSearch] = useState("");
  const [logTab, setLogTab] = useState<LogTab>("stdout");

  function handleRestart() {
    alert("Restarting Dario proxy...");
  }

  function handleCheckUpdate() {
    alert("Checking for updates...");
  }

  function handleClear(model: string) {
    alert(`Clearing cache for ${model}`);
  }

  return (
    <div className="bg-[#1c1c1e] flex items-start size-full overflow-hidden">
      <Sidebar
        activeId={activeSessionId}
        onSelect={setActiveSessionId}
        filter={filter}
        onFilter={setFilter}
        search={search}
        onSearch={setSearch}
      />
      <div className="flex flex-col flex-1 min-w-0 h-full">
        <Header onRestart={handleRestart} onCheckUpdate={handleCheckUpdate} />
        <SubtitleHelp />
        <div className="flex-1 min-h-0 overflow-y-auto flex flex-col">
          <GenerationSection />
          <PromptCacheSection onClear={handleClear} />
          <TerminalLogSection activeTab={logTab} onTab={setLogTab} />
        </div>
      </div>
    </div>
  );
}
