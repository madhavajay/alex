import { useState, useRef } from "react";
import { ChevronRight, Shuffle, Clock } from "lucide-react";
import { ImageWithFallback } from "@/app/components/figma/ImageWithFallback";
import claudeIcon from "@/imports/claude-code.png";
import codexIcon from "@/imports/codex.png";
import grokIcon from "@/imports/grok-build.png";
import ampSvg from "@/imports/amp-code.svg";
import piSvg from "@/imports/pi-1.svg";

// ─── Custom icon set ──────────────────────────────────────────────────────────

function IconPing() {
  return (
    <svg width="13" height="13" viewBox="0 0 13 13" fill="none">
      <circle cx="6.5" cy="6.5" r="2" fill="currentColor" opacity=".9" />
      <path d="M6.5 2.5C4.3 2.5 2.5 4.3 2.5 6.5" stroke="currentColor" strokeWidth="1.1" strokeLinecap="round" opacity=".5" />
      <path d="M6.5 0.5C3.2 0.5 0.5 3.2 0.5 6.5" stroke="currentColor" strokeWidth="1.1" strokeLinecap="round" opacity=".25" />
      <path d="M6.5 4.5C5.4 4.5 4.5 5.4 4.5 6.5" stroke="currentColor" strokeWidth="1.1" strokeLinecap="round" opacity=".75" />
    </svg>
  );
}

function IconRefresh() {
  return (
    <svg width="13" height="13" viewBox="0 0 13 13" fill="none">
      <path d="M11 6.5C11 9 9 11 6.5 11C4 11 2 9 2 6.5C2 4 4 2 6.5 2C8 2 9.3 2.7 10.1 3.8" stroke="currentColor" strokeWidth="1.2" strokeLinecap="round" />
      <path d="M10 1.5L10.2 4L7.8 4" stroke="currentColor" strokeWidth="1.2" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  );
}

function IconKey() {
  return (
    <svg width="13" height="13" viewBox="0 0 13 13" fill="none">
      <circle cx="4.5" cy="5.5" r="2.5" stroke="currentColor" strokeWidth="1.1" />
      <path d="M7 5.5H12M10 5.5V7.5M11.5 5.5V7" stroke="currentColor" strokeWidth="1.1" strokeLinecap="round" />
    </svg>
  );
}

function IconTrace() {
  return (
    <svg width="13" height="13" viewBox="0 0 13 13" fill="none">
      <circle cx="6.5" cy="6.5" r="3" stroke="currentColor" strokeWidth="1.1" />
      <circle cx="6.5" cy="6.5" r="1" fill="currentColor" />
      <path d="M6.5 1V2.5M6.5 10.5V12M1 6.5H2.5M10.5 6.5H12" stroke="currentColor" strokeWidth="1.1" strokeLinecap="round" opacity=".5" />
    </svg>
  );
}

function IconTerminal() {
  return (
    <svg width="13" height="13" viewBox="0 0 13 13" fill="none">
      <rect x="1" y="2" width="11" height="9" rx="1.5" stroke="currentColor" strokeWidth="1.1" />
      <path d="M3.5 5.5L5.5 7L3.5 8.5" stroke="currentColor" strokeWidth="1.1" strokeLinecap="round" strokeLinejoin="round" />
      <path d="M6.5 8.5H9" stroke="currentColor" strokeWidth="1.1" strokeLinecap="round" />
    </svg>
  );
}

function IconFile() {
  return (
    <svg width="13" height="13" viewBox="0 0 13 13" fill="none">
      <path d="M3 1.5H8L11 4.5V11.5H3V1.5Z" stroke="currentColor" strokeWidth="1.1" strokeLinejoin="round" />
      <path d="M8 1.5V4.5H11" stroke="currentColor" strokeWidth="1.1" strokeLinejoin="round" />
      <path d="M5 6.5H9M5 8.5H8" stroke="currentColor" strokeWidth="1.1" strokeLinecap="round" opacity=".6" />
    </svg>
  );
}

function IconBug() {
  return (
    <svg width="13" height="13" viewBox="0 0 13 13" fill="none">
      <ellipse cx="6.5" cy="7.5" rx="2.5" ry="3" stroke="currentColor" strokeWidth="1.1" />
      <path d="M4.5 5C4 3.8 4.5 3 5.5 3H7.5C8.5 3 9 3.8 8.5 5" stroke="currentColor" strokeWidth="1.1" strokeLinecap="round" />
      <path d="M4 7.5H2M9 7.5H11M4 9.5L2.5 10.5M9 9.5L10.5 10.5M4 5.5L2.5 4.5M9 5.5L10.5 4.5" stroke="currentColor" strokeWidth="1" strokeLinecap="round" opacity=".6" />
    </svg>
  );
}

function IconGitHub() {
  return (
    <svg width="13" height="13" viewBox="0 0 13 13" fill="none">
      <path fillRule="evenodd" clipRule="evenodd" d="M6.5 0.5C3.185 0.5 0.5 3.185 0.5 6.5C0.5 9.14 2.18 11.385 4.54 12.17C4.84 12.225 4.95 12.04 4.95 11.88C4.95 11.735 4.945 11.335 4.945 10.87C3.5 11.135 3.11 10.49 2.99 10.15C2.92 9.975 2.63 9.44 2.375 9.295C2.165 9.18 1.865 8.915 2.37 8.91C2.845 8.905 3.185 9.335 3.3 9.515C3.845 10.43 4.73 10.17 4.975 10.01C5.03 9.62 5.185 9.355 5.355 9.205C4.095 9.055 2.775 8.57 2.775 6.415C2.775 5.8 2.995 5.29 3.315 4.895C3.255 4.745 3.055 4.165 3.375 3.385C3.375 3.385 3.88 3.23 4.95 3.96C5.395 3.825 5.87 3.76 6.345 3.76C6.82 3.76 7.295 3.825 7.74 3.96C8.81 3.225 9.315 3.385 9.315 3.385C9.635 4.165 9.435 4.745 9.375 4.895C9.695 5.29 9.915 5.795 9.915 6.415C9.915 8.575 8.59 9.055 7.33 9.205C7.545 9.39 7.73 9.745 7.73 10.295C7.73 11.08 7.725 11.71 7.725 11.88C7.725 12.04 7.835 12.23 8.135 12.17C9.313 11.773 10.328 11.01 11.042 9.99C11.756 8.97 12.134 7.749 12.13 6.5C12.13 3.185 9.445 0.5 6.13 0.5H6.5Z" fill="currentColor"/>
    </svg>
  );
}

function IconSettings() {
  return (
    <svg width="13" height="13" viewBox="0 0 13 13" fill="none">
      <circle cx="6.5" cy="6.5" r="1.8" stroke="currentColor" strokeWidth="1.1" />
      <path d="M6.5 1v1.2M6.5 10.8V12M1 6.5h1.2M10.8 6.5H12M2.4 2.4l.85.85M9.75 9.75l.85.85M2.4 10.6l.85-.85M9.75 3.25l.85-.85" stroke="currentColor" strokeWidth="1.1" strokeLinecap="round" opacity=".7" />
    </svg>
  );
}

function IconDownload() {
  return (
    <svg width="13" height="13" viewBox="0 0 13 13" fill="none">
      <path d="M6.5 1.5V8.5M4 6.5L6.5 9L9 6.5" stroke="currentColor" strokeWidth="1.2" strokeLinecap="round" strokeLinejoin="round" />
      <path d="M2 10H11" stroke="currentColor" strokeWidth="1.2" strokeLinecap="round" opacity=".6" />
    </svg>
  );
}

function IconHarness() {
  return (
    <svg width="13" height="13" viewBox="0 0 13 13" fill="none">
      <rect x="1.5" y="3" width="4" height="7" rx="1" stroke="currentColor" strokeWidth="1.1" />
      <rect x="7.5" y="3" width="4" height="7" rx="1" stroke="currentColor" strokeWidth="1.1" />
      <path d="M5.5 6.5H7.5" stroke="currentColor" strokeWidth="1.1" strokeLinecap="round" />
      <path d="M3.5 1.5V3M9.5 1.5V3" stroke="currentColor" strokeWidth="1.1" strokeLinecap="round" opacity=".5" />
    </svg>
  );
}

// ─── Brand provider icons ─────────────────────────────────────────────────────

type PingStatus = "ok" | "slow" | "error" | "pending" | "unknown";

const healthBadgeColor: Record<PingStatus, string> = {
  ok:      "#34c759",
  slow:    "#ff9500",
  error:   "#ff3b30",
  pending: "#636366",
  unknown: "transparent",
};

function ProviderIcon({ name, size = 14, health }: { name: string; size?: number; health?: PingStatus }) {
  const cls = `rounded-[3px] shrink-0 object-contain`;
  const badgeSize = Math.max(4, Math.round(size * 0.38));

  let img: React.ReactNode;
  if (name === "Claude")
    img = <ImageWithFallback src={claudeIcon} alt="Claude" style={{ width: size, height: size }} className={`${cls} bg-[#f5f4ef]`} />;
  else if (name === "Codex")
    img = <ImageWithFallback src={codexIcon} alt="Codex" style={{ width: size, height: size }} className={`${cls} bg-white`} />;
  else if (name === "Grok")
    img = <ImageWithFallback src={grokIcon} alt="Grok" style={{ width: size, height: size }} className={cls} />;
  else if (name === "Amp")
    img = <ImageWithFallback src={ampSvg} alt="Amp" style={{ width: size, height: size }} className={cls} />;
  else if (name === "Pi")
    img = <ImageWithFallback src={piSvg} alt="Pi" style={{ width: size, height: size }} className={`${cls} rounded-[3px]`} />;
  else
    img = (
      <div style={{ width: size, height: size }} className="rounded-[3px] bg-[#af52de] flex items-center justify-center shrink-0">
        <span className="text-white font-bold" style={{ fontSize: size * 0.5 }}>OR</span>
      </div>
    );

  if (!health || health === "unknown") return <div className="relative shrink-0">{img}</div>;

  return (
    <div className="relative shrink-0" style={{ width: size, height: size }}>
      {img}
      <span
        className="absolute rounded-full ring-[1.5px] ring-[#1c1c1e]"
        style={{
          width: badgeSize,
          height: badgeSize,
          background: healthBadgeColor[health],
          bottom: -Math.round(badgeSize * 0.35),
          right: -Math.round(badgeSize * 0.35),
          opacity: health === "pending" ? 0.5 : 1,
        }}
      />
    </div>
  );
}

// ─── Types ────────────────────────────────────────────────────────────────────

type ProviderColor = "green" | "orange" | "blue" | "purple" | "red";
type BondMode = "round-robin" | "expires-first";

interface Quota { label: string; pct: number; timeLeft: string; }

interface SingleProvider {
  type: "single";
  id: string;
  name: string;
  model: string;
  color: ProviderColor;
  creditBalance?: string;
  quotas: Quota[];
  agent?: { name: string; version: string; status: "ready" | "error" | "idle" };
}

interface BondedProvider {
  type: "bonded";
  name: string;
  model: string;
  color: ProviderColor;
  mode: BondMode;
  slots: { email: string; quotas: Quota[] }[];
}

type ProviderEntry = SingleProvider | BondedProvider;

interface Harness { name: string; version: string; status: "ready" | "error" | "idle"; }

// ─── Data ─────────────────────────────────────────────────────────────────────

const providerEntries: ProviderEntry[] = [
  { type: "single", id: "amp", name: "Amp", model: "amp", color: "green", creditBalance: "$43.46", quotas: [] },
  {
    type: "single", id: "claude", name: "Claude", model: "claude", color: "orange",
    agent: { name: "Dario", version: "v5.1.1", status: "ready" },
    quotas: [
      { label: "95%", pct: 95, timeLeft: "3h 30m" },
      { label: "80%", pct: 80, timeLeft: "6d 14h" },
    ],
  },
  {
    type: "bonded", name: "Codex", model: "codex", color: "blue", mode: "round-robin",
    slots: [
      { email: "me@madhavajay.com", quotas: [{ label: "98%", pct: 98, timeLeft: "6d 14h" }, { label: "100%", pct: 100, timeLeft: "" }] },
      { email: "madhava@openmined.org", quotas: [{ label: "94%", pct: 94, timeLeft: "6d 14h" }, { label: "100%", pct: 100, timeLeft: "" }] },
    ],
  },
  { type: "single", id: "grok", name: "Grok", model: "grok", color: "purple", quotas: [{ label: "100%", pct: 100, timeLeft: "" }] },
];

const harnesses: Harness[] = [
  { name: "Pi", version: "v2.0.0", status: "ready" },
  { name: "Codex", version: "v1.4.2", status: "ready" },
  { name: "Grok", version: "v0.9.1", status: "error" },
  { name: "Amp", version: "v1.2.0", status: "ready" },
];

const activeHarness = harnesses[0];
const UI_UPDATE: { version: string } | null = { version: "v0.1.27" };
const DAEMON_UPDATE: { version: string } | null = { version: "v0.1.26-beta.18" };

// ─── Color maps ───────────────────────────────────────────────────────────────

const barColor: Record<ProviderColor, string> = {
  green: "bg-[#34c759]", orange: "bg-[#ff9500]", blue: "bg-[#007aff]",
  purple: "bg-[#af52de]", red: "bg-[#ff3b30]",
};

const statusDot: Record<string, string> = {
  ready: "bg-[#34c759]", idle: "bg-[#ff9500]", error: "bg-[#ff3b30]",
};

// ─── Primitives ───────────────────────────────────────────────────────────────

function QuotaBar({ quota, color }: { quota: Quota; color: ProviderColor }) {
  const isLow = quota.pct < 20;
  return (
    <div className="flex items-center gap-[6px] w-full">
      <div className="flex-1 h-[3px] rounded-full bg-white/[0.08] overflow-hidden min-w-0">
        <div className={`h-full rounded-full ${isLow ? "bg-[#ff3b30]" : barColor[color]}`} style={{ width: `${quota.pct}%` }} />
      </div>
      <span className="text-[10px] text-[#636366] font-['Cousine',monospace] w-[28px] text-right shrink-0">{quota.label}</span>
      <span className="text-[10px] text-[#48484a] font-['Cousine',monospace] w-[44px] text-right shrink-0">{quota.timeLeft}</span>
    </div>
  );
}

function SectionLabel({ children }: { children: React.ReactNode }) {
  return (
    <div className="px-3 pt-[9px] pb-[3px]">
      <span className="text-[10px] font-semibold text-[#636366] uppercase tracking-wider font-['Inter',sans-serif]">{children}</span>
    </div>
  );
}

// ─── Provider rows ────────────────────────────────────────────────────────────

function SingleProviderRow({ p, health }: { p: SingleProvider; health?: PingStatus }) {
  return (
    <div className="px-3 py-[7px] flex flex-col gap-[5px]">
      <div className="flex items-center gap-[7px]">
        <ProviderIcon name={p.name} size={14} health={health} />
        <span className="text-[11px] font-semibold text-[#e5e5ea] font-['Inter',sans-serif]">{p.name}</span>
        <span className="text-[10px] text-[#48484a] font-['Cousine',monospace]">{p.model}</span>
      </div>
      {p.creditBalance && (
        <div className="flex items-center gap-2 pl-[21px]">
          <span className="text-[10px] text-[#636366] font-['Inter',sans-serif]">Credit balance</span>
          <span className="text-[10px] font-semibold text-[#34c759] font-['Cousine',monospace]">{p.creditBalance}</span>
        </div>
      )}
      {p.quotas.length > 0 && (
        <div className="pl-[21px] flex flex-col gap-[4px]">
          {p.quotas.map((q, i) => <QuotaBar key={i} quota={q} color={p.color} />)}
        </div>
      )}
      {p.agent && (
        <div className="pl-[21px]">
          <div
            className="flex items-center gap-[6px] px-[7px] py-[4px] rounded-[6px]"
            style={{ background: "rgba(255,149,0,0.07)", border: "1px solid rgba(255,149,0,0.12)" }}
          >
            <span className={`w-[5px] h-[5px] rounded-full shrink-0 ${statusDot[p.agent.status]}`} />
            <span className="text-[10px] font-medium text-[#aeaeb2] font-['Inter',sans-serif]">{p.agent.name}</span>
            <span className="text-[10px] text-[#48484a] font-['Cousine',monospace]">{p.agent.version}</span>
            <span className="text-[9px] text-[#34c759] font-['Inter',sans-serif] ml-auto">{p.agent.status}</span>
          </div>
        </div>
      )}
    </div>
  );
}

const bondModeIcon: Record<BondMode, React.ElementType> = { "round-robin": Shuffle, "expires-first": Clock };
const bondModeLabel: Record<BondMode, string> = { "round-robin": "Round Robin", "expires-first": "Expires First" };
const bondBorderColor: Record<ProviderColor, string> = {
  green: "rgba(52,199,89,0.3)", orange: "rgba(255,149,0,0.3)", blue: "rgba(0,122,255,0.3)",
  purple: "rgba(175,82,222,0.3)", red: "rgba(255,59,48,0.3)",
};

function BondedProviderRow({ p, health }: { p: BondedProvider; health?: PingStatus }) {
  const ModeIcon = bondModeIcon[p.mode];
  return (
    <div className="px-3 py-[7px] flex flex-col gap-[5px]">
      <div className="flex items-center gap-[7px]">
        <ProviderIcon name={p.name} size={14} health={health} />
        <span className="text-[11px] font-semibold text-[#e5e5ea] font-['Inter',sans-serif]">{p.name}</span>
        <span className="text-[10px] text-[#48484a] font-['Cousine',monospace]">{p.model}</span>
        <div className="ml-auto flex items-center gap-[4px] bg-white/[0.07] rounded-[5px] px-[6px] py-[2px]">
          <ModeIcon className="w-[9px] h-[9px] text-[#636366]" />
          <span className="text-[9px] text-[#636366] font-['Inter',sans-serif] font-medium">{bondModeLabel[p.mode]}</span>
        </div>
      </div>
      <div className="pl-[21px] flex flex-col gap-[7px]">
        {p.slots.map((slot, si) => (
          <div key={si} className="flex flex-col gap-[4px] pl-[8px]" style={{ borderLeft: `1.5px solid ${bondBorderColor[p.color]}` }}>
            <div className="flex items-center gap-[5px]">
              <span className="text-[9px] text-[#48484a] font-['Cousine',monospace] w-[10px] shrink-0">{si + 1}</span>
              <span className="text-[10px] text-[#636366] font-['Inter',sans-serif] truncate">{slot.email}</span>
            </div>
            {slot.quotas.map((q, qi) => <QuotaBar key={qi} quota={q} color={p.color} />)}
          </div>
        ))}
      </div>
    </div>
  );
}

// ─── Harness flyout ───────────────────────────────────────────────────────────

function HarnessSection() {
  const [open, setOpen] = useState(false);
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  function enter() {
    if (timerRef.current) clearTimeout(timerRef.current);
    setOpen(true);
  }
  function leave() {
    timerRef.current = setTimeout(() => setOpen(false), 120);
  }

  return (
    <div style={{ borderBottom: "1px solid rgba(255,255,255,0.07)" }}>
      <SectionLabel>Harnesses</SectionLabel>
      <div className="px-1 pb-[4px] relative">
        {/* Active harness row — hover opens flyout */}
        <button
          onMouseEnter={enter}
          onMouseLeave={leave}
          className={`w-full flex items-center gap-[8px] px-3 py-[6px] rounded-[6px] transition-colors ${open ? "bg-[#0057d8]" : "hover:bg-white/[0.06]"}`}
        >
          <IconHarness />
          <span className={`text-[11px] font-medium font-['Inter',sans-serif] ${open ? "text-white" : "text-[#aeaeb2]"}`}>
            {activeHarness.name}
          </span>
          <span className={`text-[10px] font-['Cousine',monospace] ${open ? "text-white/50" : "text-[#48484a]"}`}>
            {activeHarness.version}
          </span>
          {/* Dario is a Claude harness — show subtle badge */}
          <span className={`text-[10px] ml-auto font-['Inter',sans-serif] ${open ? "text-white/60" : "text-[#34c759]"}`}>
            {activeHarness.status}
          </span>
          <ChevronRight className={`w-3 h-3 shrink-0 ${open ? "text-white/60" : "text-[#48484a]"}`} />
        </button>

        {/* Flyout */}
        {open && (
          <div
            onMouseEnter={enter}
            onMouseLeave={leave}
            className="absolute left-full top-0 ml-[3px] w-[210px] rounded-[10px] z-50 overflow-hidden"
            style={{
              background: "rgba(38,38,40,0.98)",
              backdropFilter: "blur(40px)",
              WebkitBackdropFilter: "blur(40px)",
              border: "1px solid rgba(255,255,255,0.10)",
              boxShadow: "0 8px 32px rgba(0,0,0,0.7), 0 0 0 0.5px rgba(255,255,255,0.04)",
            }}
          >
            <div className="py-[4px]">
              {harnesses.map(h => (
                <FlyoutHarnessRow key={h.name} h={h} />
              ))}
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

function FlyoutHarnessRow({ h }: { h: Harness }) {
  return (
    <button className="w-full flex items-center gap-[8px] px-3 py-[5px] hover:bg-[#0057d8] group transition-colors">
      <ProviderIcon name={h.name} size={13} />
      <span className="text-[12px] font-medium text-[#e5e5ea] font-['Inter',sans-serif] flex-1 text-left">{h.name}</span>
      <span className={`w-[5px] h-[5px] rounded-full shrink-0 ${statusDot[h.status]}`} />
      <span className="text-[10px] text-[#48484a] font-['Cousine',monospace] group-hover:text-white/40">{h.version}</span>
      <ChevronRight className="w-3 h-3 text-[#48484a] group-hover:text-white/50 shrink-0" />
    </button>
  );
}

// ─── Action row ───────────────────────────────────────────────────────────────

function ActionRow({
  icon,
  label,
  kbd,
  onClick,
  dimmed,
}: {
  icon: React.ReactNode;
  label: string;
  kbd?: string;
  onClick?: () => void;
  dimmed?: boolean;
}) {
  return (
    <button
      onClick={onClick}
      className="w-full flex items-center gap-[8px] px-3 py-[5px] rounded-[6px] group transition-colors hover:bg-white/[0.06]"
    >
      <span className={`shrink-0 transition-colors ${dimmed ? "text-[#48484a] group-hover:text-[#636366]" : "text-[#636366] group-hover:text-[#aeaeb2]"}`}>
        {icon}
      </span>
      <span className={`text-[11px] font-['Inter',sans-serif] flex-1 text-left ${dimmed ? "text-[#48484a] group-hover:text-[#636366]" : "text-[#aeaeb2]"}`}>
        {label}
      </span>
      {kbd && (
        <kbd className="text-[9px] text-[#48484a] bg-white/[0.06] rounded-[3px] px-[4px] py-[1px] font-['Cousine',monospace] group-hover:text-[#636366]">
          {kbd}
        </kbd>
      )}
    </button>
  );
}

// ─── Ping health section ──────────────────────────────────────────────────────

interface PingTarget {
  label: string;
  host: string;
  ms: number | null;
  status: PingStatus;
}

const initialPings: PingTarget[] = [
  { label: "Amp",    host: "ampcode.com",       ms: 18,   status: "ok" },
  { label: "Claude", host: "api.anthropic.com", ms: 42,   status: "ok" },
  { label: "Codex",  host: "api.openai.com",    ms: 310,  status: "slow" },
  { label: "Grok",   host: "api.x.ai",          ms: null, status: "error" },
];

const pingMs: Record<PingStatus, string> = {
  ok:      "text-[#34c759]",
  slow:    "text-[#ff9500]",
  error:   "text-[#ff3b30]",
  pending: "text-[#48484a]",
  unknown: "text-[#48484a]",
};

function PingSection({
  pings,
  running,
  onRun,
}: {
  pings: PingTarget[];
  running: boolean;
  onRun: () => void;
}) {
  return (
    <div style={{ borderBottom: "1px solid rgba(255,255,255,0.07)" }}>
      <div className="flex items-center justify-between px-3 pt-[9px] pb-[4px]">
        <span className="text-[10px] font-semibold text-[#636366] uppercase tracking-wider font-['Inter',sans-serif]">
          Ping Health
        </span>
        <button
          onClick={onRun}
          disabled={running}
          className="flex items-center gap-[4px] px-[6px] py-[2px] rounded-[5px] transition-colors hover:bg-white/[0.08] disabled:opacity-40"
        >
          <span className={`text-[#636366] ${running ? "animate-spin" : ""}`} style={{ display: "inline-block" }}>
            <IconRefresh />
          </span>
          <span className="text-[10px] text-[#636366] font-['Inter',sans-serif]">
            {running ? "Checking…" : "Run"}
          </span>
        </button>
      </div>
      <div className="px-3 pb-[8px] flex flex-col gap-[4px]">
        {pings.map((t) => (
          <div key={t.host} className="flex items-center gap-[7px]">
            <ProviderIcon name={t.label} size={11} health={t.status} />
            <span className="text-[10px] text-[#636366] font-['Inter',sans-serif] flex-1 truncate">{t.host}</span>
            {t.status === "pending" ? (
              <span className="text-[10px] text-[#48484a] font-['Cousine',monospace] w-[38px] text-right">…</span>
            ) : t.ms !== null ? (
              <span className={`text-[10px] font-['Cousine',monospace] w-[38px] text-right ${pingMs[t.status]}`}>{t.ms}ms</span>
            ) : (
              <span className="text-[10px] text-[#ff3b30] font-['Cousine',monospace] w-[38px] text-right">—</span>
            )}
          </div>
        ))}
      </div>
    </div>
  );
}

// ─── Trace browser section ────────────────────────────────────────────────────

interface Trace {
  id: string;
  label: string;
  provider: string;
  duration: string;
  status: "ok" | "error";
  ago: string;
}

const recentTraces: Trace[] = [
  { id: "tr_a1b2", label: "codegen: refactor auth module",   provider: "Claude", duration: "4.2s",  status: "ok",    ago: "2m" },
  { id: "tr_c3d4", label: "explain: recursion in python",    provider: "Codex",  duration: "1.8s",  status: "ok",    ago: "11m" },
  { id: "tr_e5f6", label: "fix: null pointer in payment svc",provider: "Claude", duration: "9.1s",  status: "error", ago: "34m" },
  { id: "tr_g7h8", label: "codegen: sql migration script",   provider: "Grok",   duration: "3.3s",  status: "ok",    ago: "1h" },
];

function TraceBrowserSection() {
  const [selected, setSelected] = useState<string | null>(null);

  return (
    <div style={{ borderBottom: "1px solid rgba(255,255,255,0.07)" }}>
      {/* Header */}
      <div className="flex items-center justify-between px-3 pt-[9px] pb-[4px]">
        <span className="text-[10px] font-semibold text-[#636366] uppercase tracking-wider font-['Inter',sans-serif]">
          Traces
        </span>
        <button className="flex items-center gap-[4px] px-[6px] py-[2px] rounded-[5px] hover:bg-white/[0.08] transition-colors">
          <IconTrace />
          <span className="text-[10px] text-[#636366] font-['Inter',sans-serif]">Open Browser</span>
        </button>
      </div>

      {/* Trace rows */}
      <div className="pb-[6px]">
        {recentTraces.map((t) => (
          <button
            key={t.id}
            onClick={() => setSelected(s => s === t.id ? null : t.id)}
            className="w-full flex items-center gap-[7px] px-3 py-[5px] hover:bg-white/[0.05] transition-colors group"
          >
            {/* Status dot */}
            <span className={`w-[5px] h-[5px] rounded-full shrink-0 ${t.status === "ok" ? "bg-[#34c759]" : "bg-[#ff3b30]"}`} />

            {/* Label */}
            <span className="text-[11px] text-[#aeaeb2] font-['Inter',sans-serif] flex-1 text-left truncate">
              {t.label}
            </span>

            {/* Provider icon */}
            <ProviderIcon name={t.provider} size={11} />

            {/* Duration */}
            <span className="text-[10px] text-[#48484a] font-['Cousine',monospace] w-[30px] text-right shrink-0">
              {t.duration}
            </span>

            {/* Age */}
            <span className="text-[10px] text-[#48484a] font-['Inter',sans-serif] w-[22px] text-right shrink-0">
              {t.ago}
            </span>
          </button>
        ))}
      </div>
    </div>
  );
}

// ─── Update banner ────────────────────────────────────────────────────────────

function UpdateSection({ ui, daemon }: { ui: { version: string } | null; daemon: { version: string } | null }) {
  const [dismissed, setDismissed] = useState(false);
  if (dismissed || (!ui && !daemon)) return null;
  const both = ui && daemon;

  return (
    <div style={{ borderBottom: "1px solid rgba(255,149,0,0.15)", borderTop: "1px solid rgba(255,149,0,0.15)", background: "rgba(255,149,0,0.06)" }}>
      <div className="flex gap-[12px] px-3 pt-[10px] pb-[10px]">
        {/* Left — title + version rows */}
        <div className="flex-1 flex flex-col gap-[4px]">
          <span className="text-[11px] font-bold text-[#ff9500] uppercase tracking-wider font-['Inter',sans-serif]">
            Update Available
          </span>
          {ui && (
            <div className="flex items-baseline gap-[10px]">
              <span className="text-[11px] text-[#ff9500]/50 font-['Inter',sans-serif] w-[44px] shrink-0">App</span>
              <span className="text-[12px] font-semibold text-[#ff9500] font-['Cousine',monospace]">{ui.version}</span>
            </div>
          )}
          {daemon && (
            <div className="flex items-baseline gap-[10px]">
              <span className="text-[11px] text-[#ff9500]/50 font-['Inter',sans-serif] w-[44px] shrink-0">Daemon</span>
              <span className="text-[12px] font-semibold text-[#ff9500] font-['Cousine',monospace]">{daemon.version}</span>
            </div>
          )}
        </div>

        {/* Right — Later top, Update Both bottom */}
        <div className="flex flex-col items-end justify-between gap-[8px]">
          <button
            onClick={() => setDismissed(true)}
            className="text-[11px] text-[#ff9500]/40 hover:text-[#ff9500]/70 transition-colors font-['Inter',sans-serif] px-[2px]"
          >
            Later
          </button>
          <button className="text-[11px] font-bold text-[#1c1c1e] bg-[#ff9500] hover:bg-[#ffad33] rounded-[7px] px-[11px] py-[4px] transition-colors font-['Inter',sans-serif] whitespace-nowrap">
            {both ? "Update Both" : "Update"}
          </button>
        </div>
      </div>
    </div>
  );
}

// ─── App ──────────────────────────────────────────────────────────────────────

export default function App() {
  const [accountsOpen, setAccountsOpen] = useState(false);
  const [pings, setPings] = useState<PingTarget[]>(initialPings);
  const [pingRunning, setPingRunning] = useState(false);

  function runPingChecks() {
    if (pingRunning) return;
    setPingRunning(true);
    setPings(p => p.map(t => ({ ...t, status: "pending" as PingStatus, ms: null })));
    initialPings.forEach((target, i) => {
      setTimeout(() => {
        setPings(p => p.map((t, j) => j === i ? { ...target } : t));
        if (i === initialPings.length - 1) setPingRunning(false);
      }, 400 + i * 300);
    });
  }

  // Map provider name → ping status for badge overlays
  const healthMap = Object.fromEntries(pings.map(p => [p.label, p.status]));
  function healthOf(name: string): PingStatus {
    return (healthMap[name] as PingStatus) ?? "unknown";
  }

  return (
    <div className="min-h-screen bg-[#0d0d0d] flex items-start justify-end pt-2 pr-4">
      <div
        className="w-[340px] rounded-[14px] overflow-visible flex flex-col"
        style={{
          background: "rgba(28,28,30,0.97)",
          backdropFilter: "blur(40px) saturate(180%)",
          WebkitBackdropFilter: "blur(40px) saturate(180%)",
          border: "1px solid rgba(255,255,255,0.10)",
          boxShadow: "0 20px 60px rgba(0,0,0,0.85), 0 0 0 0.5px rgba(255,255,255,0.05)",
        }}
      >
        {/* ── Header ── */}
        <div className="px-3 py-[10px]" style={{ borderBottom: "1px solid rgba(255,255,255,0.07)" }}>
          <div className="flex items-center justify-between mb-[2px]">
            <span className="text-[12px] font-semibold text-[#e5e5ea] font-['Inter',sans-serif]">Alex UI</span>
            <div className="flex items-center gap-[5px]">
              <span className="w-[5px] h-[5px] rounded-full bg-[#34c759] inline-block" />
              <span className="text-[10px] text-[#34c759] font-['Inter',sans-serif]">daemon up 11m</span>
            </div>
          </div>
          <div className="flex items-center justify-between">
            <span className="text-[10px] text-[#48484a] font-['Cousine',monospace]">v0.1.26-beta.20</span>
            <span className="text-[10px] text-[#48484a] font-['Cousine',monospace]">daemon v0.1.26-beta.18</span>
          </div>
        </div>

        {/* ── Update available ── */}
        <UpdateSection ui={UI_UPDATE} daemon={DAEMON_UPDATE} />

        {/* ── Stats bar ── */}
        <div className="px-3 py-[8px] flex items-center" style={{ borderBottom: "1px solid rgba(255,255,255,0.07)" }}>
          {([
            { value: "39", label: "requests", color: "text-[#e5e5ea]" },
            { value: "$0.0039", label: "last hour", color: "text-[#e5e5ea]" },
            { value: "4", label: "errors", color: "text-[#ff3b30]" },
          ] as const).map((stat, i) => (
            <div key={i} className="flex-1 flex flex-col items-center gap-[1px] relative">
              <span className={`text-[14px] font-semibold font-['Cousine',monospace] ${stat.color}`}>{stat.value}</span>
              <span className="text-[9px] text-[#48484a] font-['Inter',sans-serif] uppercase tracking-wide">{stat.label}</span>
              {i < 2 && <div className="absolute right-0 top-1/2 -translate-y-1/2 w-px h-[20px] bg-white/[0.08]" />}
            </div>
          ))}
        </div>

        {/* ── Providers ── */}
        <div style={{ borderBottom: "1px solid rgba(255,255,255,0.07)" }}>
          <div className="flex items-center justify-between px-3 pt-[9px] pb-[3px]">
            <span className="text-[10px] font-semibold text-[#636366] uppercase tracking-wider font-['Inter',sans-serif]">Providers</span>
            <div className="flex items-center gap-[2px]">
              <button
                className="flex items-center gap-[4px] px-[6px] py-[2px] rounded-[5px] hover:bg-white/[0.08] transition-colors"
                title="Refresh Now ⌘R"
              >
                <span className="text-[#636366]"><IconRefresh /></span>
                <span className="text-[10px] text-[#636366] font-['Inter',sans-serif]">Refresh</span>
              </button>
              <div className="w-px h-[10px] bg-white/[0.08]" />
              <button
                onClick={runPingChecks}
                disabled={pingRunning}
                className="flex items-center gap-[4px] px-[6px] py-[2px] rounded-[5px] hover:bg-white/[0.08] transition-colors disabled:opacity-40"
              >
                <span className={`text-[#636366] ${pingRunning ? "animate-spin" : ""}`} style={{ display: "inline-block" }}>
                  <IconPing />
                </span>
                <span className="text-[10px] text-[#636366] font-['Inter',sans-serif]">{pingRunning ? "…" : "Ping"}</span>
              </button>
            </div>
          </div>
          {providerEntries.map((p, i) => (
            <div key={i}>
              {p.type === "single"
                ? <SingleProviderRow p={p} health={healthOf(p.name)} />
                : <BondedProviderRow p={p} health={healthOf(p.name)} />}
              {i < providerEntries.length - 1 && <div className="mx-3 h-px bg-white/[0.05]" />}
            </div>
          ))}
          <div className="pb-[4px]" />
        </div>

        {/* ── Accounts (collapsible) ── */}
        <div style={{ borderBottom: "1px solid rgba(255,255,255,0.07)" }}>
          <button
            onClick={() => setAccountsOpen(v => !v)}
            className="w-full flex items-center justify-between px-3 pt-[9px] pb-[3px] group"
          >
            <span className="text-[10px] font-semibold text-[#636366] uppercase tracking-wider font-['Inter',sans-serif] group-hover:text-[#aeaeb2] transition-colors">
              Accounts
            </span>
            <ChevronRight className={`w-[10px] h-[10px] text-[#48484a] transition-transform duration-150 ${accountsOpen ? "rotate-90" : ""}`} />
          </button>
          {accountsOpen && (
            <div className="pb-[4px] px-1">
              {([
                { label: "Amp", email: "me@madhavajay.com" },
                { label: "Claude", email: "me@madhavajay.com" },
                { label: "Claude", email: "me@madhavajay.com" },
                { label: "Codex", email: "madhava@openmined.org" },
                { label: "OpenRouter", email: "api key" },
                { label: "Grok", email: "me@madhavajay.com" },
              ] as const).map((acc, i) => (
                <button key={i} className="w-full flex items-center gap-[8px] px-3 py-[5px] hover:bg-white/[0.06] transition-colors rounded-[6px] group">
                  <ProviderIcon name={acc.label} size={13} health={healthOf(acc.label)} />
                  <span className="text-[11px] font-medium text-[#aeaeb2] font-['Inter',sans-serif] w-[52px] shrink-0">{acc.label}</span>
                  <span className="text-[11px] text-[#636366] font-['Inter',sans-serif] flex-1 text-left truncate">{acc.email}</span>
                  <ChevronRight className="w-3 h-3 text-[#48484a] group-hover:text-[#636366] shrink-0" />
                </button>
              ))}
            </div>
          )}
          {!accountsOpen && <div className="pb-[4px]" />}
        </div>

        {/* ── Harnesses (with flyout) ── */}
        <HarnessSection />

        {/* ── Trace Browser ── */}
        <TraceBrowserSection />


        {/* ── Footer ── */}
        <div className="pt-[4px] pb-[6px]">
          <div className="px-1">
            <ActionRow icon={<IconBug />} label="Report a Bug or Feature…" />
            <ActionRow icon={<IconSettings />} label="Settings…" kbd="⌘," />
            <div className="mx-2 my-[2px] h-px bg-white/[0.06]" />
            <ActionRow icon={<IconGitHub />} label="Star GitHub Project" />
          </div>
        </div>

      </div>
    </div>
  );
}
