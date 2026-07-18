import { useMemo, useState } from "react";
import {
  LineChart,
  Line,
  XAxis,
  YAxis,
  CartesianGrid,
  Tooltip,
  ResponsiveContainer,
} from "recharts";

// ─── Data ────────────────────────────────────────────────────────────────────

const PROVIDERS = [
  { id: "amp", name: "Amp", count: 1, color: "#ff9f0a" },
  { id: "claude", name: "Claude", count: 1, color: "#bf5af2" },
  { id: "codex", name: "Codex", count: 2, color: "#0a84ff" },
  { id: "gemini", name: "Gemini", count: 0, color: "#34c759" },
  { id: "grok", name: "Grok", count: 1, color: "#ff453a" },
  { id: "openrouter", name: "OpenRouter", count: 1, color: "#ff6961" },
];

const ACCOUNTS_BY_PROVIDER: Record<string, Account[]> = {
  codex: [
    {
      id: "acc-1",
      provider: "Codex",
      name: "example.com",
      email: "user@example.com",
      identifier: "openai-south-acct-69f8bb482efe488",
      status: "Active",
      plan: "7d",
      tokensRemaining: 94,
      tokensLabel: "94% remaining",
      resetIn: "resets in 8 days",
      creditRemaining: 100,
      creditLabel: "100% remaining",
      last24h: { requests: 334, tokens: "33.0M", errors: 5 },
      color: "#0a84ff",
    },
    {
      id: "acc-2",
      provider: "Codex",
      name: "work@example.com",
      email: "work@example.com",
      identifier: "openai-south-acct-8ce6b4257f4dae35",
      status: "Active",
      plan: "7d",
      tokensRemaining: 71,
      tokensLabel: "71% remaining",
      resetIn: "resets in 3 days",
      creditRemaining: 88,
      creditLabel: "88% remaining",
      last24h: { requests: 198, tokens: "18.4M", errors: 1 },
      color: "#34c759",
    },
  ],
  claude: [
    {
      id: "acc-3",
      provider: "Claude",
      name: "personal",
      email: "user@example.com",
      identifier: "anthropic-acct-a1b2c3d4e5f6",
      status: "Active",
      plan: "30d",
      tokensRemaining: 62,
      tokensLabel: "62% remaining",
      resetIn: "resets in 12 days",
      creditRemaining: 55,
      creditLabel: "55% remaining",
      last24h: { requests: 87, tokens: "9.2M", errors: 0 },
      color: "#bf5af2",
    },
  ],
  amp: [
    {
      id: "acc-4",
      provider: "Amp",
      name: "work",
      email: "work@company.io",
      identifier: "amp-acct-z9y8x7w6v5u4",
      status: "Active",
      plan: "30d",
      tokensRemaining: 78,
      tokensLabel: "78% remaining",
      resetIn: "resets in 18 days",
      creditRemaining: 90,
      creditLabel: "90% remaining",
      last24h: { requests: 42, tokens: "4.1M", errors: 0 },
      color: "#ff9f0a",
    },
  ],
};

type Account = {
  id: string;
  provider: string;
  name: string;
  email: string;
  identifier: string;
  status: string;
  plan: string;
  tokensRemaining: number;
  tokensLabel: string;
  resetIn: string;
  creditRemaining: number;
  creditLabel: string;
  last24h: { requests: number; tokens: string; errors: number };
  color: string;
};

type RoutingConfig = {
  selectionMode: "round-robin" | "first-available" | "least-used";
  providerReserve: number;
  allowFailover: boolean;
  accountSettings: Record<string, { enabled: boolean; reserve: number }>;
};

type TimeRange = "24h" | "7d" | "30d" | "all";
const TIME_RANGES: { value: TimeRange; label: string }[] = [
  { value: "24h", label: "24h" },
  { value: "7d", label: "7d" },
  { value: "30d", label: "30d" },
  { value: "all", label: "All" },
];

function generateChartData(range: TimeRange) {
  const now = Date.now();
  const configs: Record<TimeRange, { points: number; stepMs: number; fmt: (d: Date) => string }> = {
    "24h": { points: 28, stepMs: 3600 * 1000, fmt: (d) => d.toLocaleTimeString([], { hour: "numeric", minute: "2-digit", hour12: false }) },
    "7d":  { points: 28, stepMs: 6 * 3600 * 1000, fmt: (d) => d.toLocaleDateString([], { month: "short", day: "numeric" }) },
    "30d": { points: 30, stepMs: 24 * 3600 * 1000, fmt: (d) => d.toLocaleDateString([], { month: "short", day: "numeric" }) },
    "all": { points: 24, stepMs: 7 * 24 * 3600 * 1000, fmt: (d) => d.toLocaleDateString([], { month: "short", day: "numeric" }) },
  };
  const { points, stepMs, fmt } = configs[range];
  const spikeA = Math.floor(points * 0.58);
  const spikeB = Math.floor(points * 0.65);
  return Array.from({ length: points }, (_, i) => {
    const t = now - (points - 1 - i) * stepMs;
    const spike = i === spikeA ? 5.2 : i === spikeA + 1 ? 3.8 : 0;
    const spike2 = i === spikeB ? 4.1 : i === spikeB + 1 ? 2.3 : 0;
    return {
      label: fmt(new Date(t)),
      acc1: Math.max(0, 0.08 + spike + Math.random() * 0.35),
      acc2: Math.max(0, 0.04 + spike2 + Math.random() * 0.28),
    };
  });
}

// ─── Subcomponents ───────────────────────────────────────────────────────────

const CustomTooltip = ({ active, payload, label }: any) => {
  if (!active || !payload?.length) return null;
  return (
    <div
      className="rounded-lg px-3 py-2 text-[11px] font-['Inter',sans-serif]"
      style={{
        background: "rgba(44,44,46,0.96)",
        border: "1px solid rgba(255,255,255,0.10)",
        boxShadow: "0 4px 20px rgba(0,0,0,0.5)",
      }}
    >
      <p className="text-[#636366] mb-1">{label}</p>
      {payload.map((p: any) => (
        <p key={p.dataKey} style={{ color: p.color }}>
          {p.name}: {(p.value * 10).toFixed(1)}M tokens
        </p>
      ))}
    </div>
  );
};

function UsageBar({ pct, color }: { pct: number; color: string }) {
  return (
    <div className="h-[3px] w-full rounded-full overflow-hidden" style={{ background: "rgba(255,255,255,0.08)" }}>
      <div className="h-full rounded-full" style={{ width: `${pct}%`, background: color }} />
    </div>
  );
}

function StatusBadge() {
  return (
    <span className="flex items-center gap-[5px]">
      <span className="inline-block size-[7px] rounded-full" style={{ background: "#34c759", boxShadow: "0 0 5px #34c75988" }} />
      <span className="text-[11px] font-medium text-[#34c759]">Active</span>
    </span>
  );
}

function Toggle({ on, onChange }: { on: boolean; onChange: (v: boolean) => void }) {
  return (
    <button
      onClick={() => onChange(!on)}
      className="inline-flex items-center flex-shrink-0 rounded-full transition-colors duration-200 cursor-pointer"
      style={{
        width: 36,
        height: 20,
        padding: 2,
        background: on ? "#0a84ff" : "rgba(255,255,255,0.18)",
        justifyContent: on ? "flex-end" : "flex-start",
        border: "none",
        outline: "none",
      }}
    >
      <span
        className="rounded-full bg-white transition-transform duration-200"
        style={{ width: 16, height: 16, display: "block", flexShrink: 0 }}
      />
    </button>
  );
}

function AccountCard({ account, routingConfig, onRoutingChange }: {
  account: Account;
  routingConfig: RoutingConfig;
  onRoutingChange: (id: string, field: "enabled" | "reserve", val: any) => void;
}) {
  const [paused, setPaused] = useState(false);
  const acctSetting = routingConfig.accountSettings[account.id] ?? { enabled: true, reserve: 10 };

  return (
    <div
      className="rounded-xl p-4 flex flex-col gap-3"
      style={{ background: "rgba(255,255,255,0.04)", border: "1px solid rgba(255,255,255,0.07)" }}
    >
      <div className="flex items-start justify-between gap-3">
        <div className="flex flex-col gap-[3px]">
          <div className="flex items-center gap-2">
            <span className="text-[13px] font-semibold text-[#e5e5ea] font-['Inter',sans-serif]">{account.provider}</span>
            <StatusBadge />
          </div>
          <span className="text-[11px] font-['Menlo',monospace] text-[#636366]">{account.identifier}</span>
          <span className="text-[11px] text-[#aeaeb2] font-['Inter',sans-serif]">Email: {account.email}</span>
        </div>
      </div>

      <div className="grid grid-cols-3 gap-2 py-3 rounded-lg px-3" style={{ background: "rgba(255,255,255,0.04)" }}>
        <div className="flex flex-col gap-[2px]">
          <span className="text-[10px] text-[#636366] font-['Inter',sans-serif] uppercase tracking-wide">Requests 24h</span>
          <span className="text-[13px] font-semibold text-[#e5e5ea] font-['Inter',sans-serif]">{account.last24h.requests.toLocaleString()}</span>
        </div>
        <div className="flex flex-col gap-[2px]">
          <span className="text-[10px] text-[#636366] font-['Inter',sans-serif] uppercase tracking-wide">Tokens 24h</span>
          <span className="text-[13px] font-semibold text-[#e5e5ea] font-['Inter',sans-serif]">{account.last24h.tokens}</span>
        </div>
        <div className="flex flex-col gap-[2px]">
          <span className="text-[10px] text-[#636366] font-['Inter',sans-serif] uppercase tracking-wide">Errors</span>
          <span className="text-[13px] font-semibold font-['Inter',sans-serif]" style={{ color: account.last24h.errors > 0 ? "#ff453a" : "#34c759" }}>
            {account.last24h.errors}
          </span>
        </div>
      </div>

      <div className="flex flex-col gap-2">
        <div className="flex flex-col gap-[6px]">
          <div className="flex items-center justify-between">
            <span className="text-[11px] text-[#636366] font-['Inter',sans-serif]">{account.plan} · Tokens</span>
            <span className="text-[11px] text-[#aeaeb2] font-['Inter',sans-serif]">
              {account.tokensLabel} <span className="text-[#636366]">· {account.resetIn}</span>
            </span>
          </div>
          <UsageBar pct={account.tokensRemaining} color={account.color} />
        </div>
        <div className="flex flex-col gap-[6px]">
          <div className="flex items-center justify-between">
            <span className="text-[11px] text-[#636366] font-['Inter',sans-serif]">{account.plan} · Credits</span>
            <span className="text-[11px] text-[#aeaeb2] font-['Inter',sans-serif]">{account.creditLabel}</span>
          </div>
          <UsageBar pct={account.creditRemaining} color={account.color} />
        </div>
      </div>

      {/* Per-account routing inline */}
      <div className="flex items-center justify-between pt-1 pb-[2px]">
        <div className="flex items-center gap-2">
          <Toggle on={acctSetting.enabled} onChange={(v) => onRoutingChange(account.id, "enabled", v)} />
          <span className="text-[11px] text-[#aeaeb2] font-['Inter',sans-serif]">Use for requests</span>
        </div>
        <span
          className="text-[10px] font-medium px-2 py-[3px] rounded-md font-['Inter',sans-serif]"
          style={{ background: "rgba(255,255,255,0.07)", color: "#636366" }}
        >
          Keep unused: {acctSetting.reserve}%
        </span>
      </div>

      <div className="flex items-center gap-2 pt-1">
        <button
          onClick={() => setPaused((p) => !p)}
          className="text-[11px] font-medium font-['Inter',sans-serif] px-3 py-[5px] rounded-[7px] transition-all"
          style={{
            background: "rgba(255,255,255,0.08)",
            color: paused ? "#34c759" : "#e5e5ea",
            border: "1px solid rgba(255,255,255,0.07)",
          }}
        >
          {paused ? "Resume account" : "Pause account"}
        </button>
        <button
          className="text-[11px] font-medium font-['Inter',sans-serif] px-3 py-[5px] rounded-[7px]"
          style={{ background: "rgba(255,255,255,0.08)", color: "#e5e5ea", border: "1px solid rgba(255,255,255,0.07)" }}
        >
          Re-authenticate
        </button>
        <button
          className="text-[11px] font-medium font-['Inter',sans-serif] px-3 py-[5px] rounded-[7px] ml-auto"
          style={{ background: "rgba(255,69,58,0.12)", color: "#ff453a", border: "1px solid rgba(255,69,58,0.18)" }}
        >
          Remove
        </button>
      </div>
    </div>
  );
}

function ProviderDot({ color }: { color: string }) {
  return <span className="inline-block size-[8px] rounded-full flex-shrink-0" style={{ background: color }} />;
}

const SELECTION_MODES = [
  { value: "round-robin", label: "Round robin", description: "Alternate new sessions across eligible accounts, skipping accounts that have reached the reserve." },
  { value: "first-available", label: "First available", description: "Always use the first account that has tokens remaining, falling back in order." },
  { value: "least-used", label: "Least used", description: "Route to whichever account has consumed the fewest tokens in the current period." },
] as const;

// ─── Main ─────────────────────────────────────────────────────────────────────

const TABS = ["General", "Providers", "Harnesses"];

export default function App() {
  const [activeTab, setActiveTab] = useState("Providers");
  const [selectedProvider, setSelectedProvider] = useState("codex");
  const [routingConfig, setRoutingConfig] = useState<RoutingConfig>({
    selectionMode: "round-robin",
    providerReserve: 10,
    allowFailover: true,
    accountSettings: {
      "acc-1": { enabled: true, reserve: 10 },
      "acc-2": { enabled: true, reserve: 10 },
    },
  });
  const [savedConfig, setSavedConfig] = useState<RoutingConfig>(routingConfig);
  const [timeRange, setTimeRange] = useState<TimeRange>("24h");

  const provider = PROVIDERS.find((p) => p.id === selectedProvider)!;
  const accounts = ACCOUNTS_BY_PROVIDER[selectedProvider] ?? [];
  // Regenerate only when the range changes — the data is Math.random-based,
  // so calling it every render redraws the chart on any state change.
  const chartData = useMemo(() => generateChartData(timeRange), [timeRange]);
  const isDirty = JSON.stringify(routingConfig) !== JSON.stringify(savedConfig);

  function updateRouting(patch: Partial<RoutingConfig>) {
    setRoutingConfig((c) => ({ ...c, ...patch }));
  }

  function updateAccountRouting(id: string, field: "enabled" | "reserve", val: any) {
    setRoutingConfig((c) => ({
      ...c,
      accountSettings: {
        ...c.accountSettings,
        [id]: { ...(c.accountSettings[id] ?? { enabled: true, reserve: 10 }), [field]: val },
      },
    }));
  }

  function saveRouting() {
    setSavedConfig(routingConfig);
  }

  function cancelRouting() {
    setRoutingConfig(savedConfig);
  }

  const selectionModeInfo = SELECTION_MODES.find((m) => m.value === routingConfig.selectionMode)!;

  return (
    <div className="min-h-screen w-full flex items-center justify-center p-6" style={{ background: "#141414" }}>
      <div
        className="w-full max-w-[820px] rounded-2xl overflow-hidden flex flex-col"
        style={{
          background: "#1c1c1e",
          border: "1px solid rgba(255,255,255,0.09)",
          boxShadow: "0 24px 80px rgba(0,0,0,0.7)",
          maxHeight: "90vh",
        }}
      >
        {/* Title bar */}
        <div className="relative flex items-center justify-center px-4 py-[11px] flex-shrink-0" style={{ borderBottom: "1px solid rgba(255,255,255,0.07)" }}>
          <div className="absolute left-4 flex items-center gap-[7px]">
            <span className="inline-block size-3 rounded-full bg-[#ff5f57]" />
            <span className="inline-block size-3 rounded-full bg-[#febc2e]" />
            <span className="inline-block size-3 rounded-full bg-[#28c840]" />
          </div>
          <span className="text-[13px] font-semibold text-[#e5e5ea] font-['Inter',sans-serif]">Alex Settings</span>
        </div>

        {/* Tab bar */}
        <div className="flex items-center justify-center gap-1 px-4 py-2 flex-shrink-0" style={{ borderBottom: "1px solid rgba(255,255,255,0.07)" }}>
          {TABS.map((tab) => (
            <button
              key={tab}
              onClick={() => setActiveTab(tab)}
              className="text-[12px] font-medium font-['Inter',sans-serif] px-4 py-[5px] rounded-[7px] transition-all"
              style={activeTab === tab ? { background: "#0a84ff", color: "#ffffff" } : { background: "transparent", color: "#636366" }}
            >
              {tab}
            </button>
          ))}
        </div>

        {/* Body: sidebar + main */}
        <div className="flex min-h-0 flex-1">

          {/* ── Left sidebar ── */}
          <div
            className="w-[180px] flex-shrink-0 flex flex-col overflow-hidden"
            style={{ borderRight: "1px solid rgba(255,255,255,0.07)" }}
          >
            <div className="flex items-center justify-between px-3 py-3 flex-shrink-0" style={{ borderBottom: "1px solid rgba(255,255,255,0.05)" }}>
              <span className="text-[12px] font-semibold text-[#e5e5ea] font-['Inter',sans-serif]">Providers</span>
              <button
                className="text-[11px] font-medium font-['Inter',sans-serif] px-2 py-[3px] rounded-md transition-all"
                style={{ background: "rgba(255,255,255,0.07)", color: "#636366" }}
                title="Settings"
              >
                ⚙
              </button>
            </div>

            <div className="flex-1 overflow-y-auto py-1">
              {PROVIDERS.map((p) => (
                <button
                  key={p.id}
                  onClick={() => setSelectedProvider(p.id)}
                  className="w-full flex items-center justify-between px-3 py-[7px] text-left transition-colors"
                  style={{
                    background: selectedProvider === p.id ? "rgba(255,255,255,0.08)" : "transparent",
                    borderLeft: selectedProvider === p.id ? `2px solid ${p.color}` : "2px solid transparent",
                  }}
                >
                  <div className="flex items-center gap-2">
                    <ProviderDot color={p.color} />
                    <span
                      className="text-[12px] font-['Inter',sans-serif]"
                      style={{ color: selectedProvider === p.id ? "#e5e5ea" : "#aeaeb2", fontWeight: selectedProvider === p.id ? 600 : 400 }}
                    >
                      {p.name}
                    </span>
                  </div>
                  {p.count > 0 && (
                    <span
                      className="text-[10px] font-medium font-['Menlo',monospace] px-[6px] py-[1px] rounded-[4px]"
                      style={{
                        background: selectedProvider === p.id ? p.color : "rgba(255,255,255,0.08)",
                        color: selectedProvider === p.id ? "#fff" : "#636366",
                      }}
                    >
                      {p.count}
                    </span>
                  )}
                </button>
              ))}
            </div>

            {/* Add provider */}
            <div className="p-3 flex-shrink-0" style={{ borderTop: "1px solid rgba(255,255,255,0.07)" }}>
              <button
                className="w-full py-[7px] rounded-lg text-[11px] font-medium font-['Inter',sans-serif] transition-all"
                style={{
                  background: "rgba(10,132,255,0.10)",
                  color: "#0a84ff",
                  border: "1px dashed rgba(10,132,255,0.3)",
                }}
              >
                + Add provider
              </button>
            </div>
          </div>

          {/* ── Right panel ── */}
          <div className="flex-1 flex flex-col overflow-hidden">
            <div className="flex-1 overflow-y-auto">
            <div className="flex flex-col gap-5 p-5">

              {/* Usage header */}
              <div className="flex items-center justify-between">
                <h2 className="text-[13px] font-semibold text-[#e5e5ea] font-['Inter',sans-serif]">
                  Usage
                </h2>
                <div className="flex items-center gap-2">
                  <ProviderDot color={provider.color} />
                  <span className="text-[12px] font-semibold text-[#aeaeb2] font-['Inter',sans-serif]">{provider.name}</span>
                </div>
              </div>

              {/* Chart */}
              {accounts.length > 0 && (
                <div
                  className="rounded-xl px-4 pt-4 pb-2"
                  style={{ background: "rgba(255,255,255,0.03)", border: "1px solid rgba(255,255,255,0.07)" }}
                >
                  <div className="flex items-center justify-between mb-3">
                    <span className="text-[11px] text-[#636366] font-['Inter',sans-serif]">Tokens routed over time</span>
                    {/* Time range switcher */}
                    <div className="flex items-center gap-[2px] p-[2px] rounded-[7px]" style={{ background: "rgba(255,255,255,0.06)" }}>
                      {TIME_RANGES.map((r) => (
                        <button
                          key={r.value}
                          onClick={() => setTimeRange(r.value)}
                          className="text-[10px] font-medium font-['Inter',sans-serif] px-[8px] py-[3px] rounded-[5px] transition-all"
                          style={timeRange === r.value
                            ? { background: "rgba(255,255,255,0.15)", color: "#e5e5ea" }
                            : { background: "transparent", color: "#636366" }
                          }
                        >
                          {r.label}
                        </button>
                      ))}
                    </div>
                  </div>
                  {/* Legend */}
                  <div className="flex items-center gap-4 mb-2">
                    {accounts.map((a) => (
                      <span key={a.id} className="flex items-center gap-[6px] text-[10px] font-['Menlo',monospace] text-[#636366]">
                        <span className="inline-block h-[2px] w-[18px] rounded-full" style={{ background: a.color }} />
                        {a.email}
                      </span>
                    ))}
                  </div>
                  <ResponsiveContainer width="100%" height={130}>
                    <LineChart data={chartData} margin={{ top: 4, right: 4, left: 0, bottom: 0 }}>
                      <CartesianGrid strokeDasharray="0" vertical={false} stroke="rgba(255,255,255,0.05)" />
                      <XAxis dataKey="label" tick={{ fill: "#636366", fontSize: 9, fontFamily: "Menlo, monospace" }} axisLine={false} tickLine={false} interval={Math.floor(chartData.length / 5)} dy={6} />
                      <YAxis tickFormatter={(v) => v === 0 ? "0" : `${v.toFixed(0)}×10⁷`} tick={{ fill: "#636366", fontSize: 9, fontFamily: "Menlo, monospace" }} axisLine={false} tickLine={false} width={46} domain={[0, 6]} ticks={[0, 2, 4, 6]} />
                      <Tooltip content={<CustomTooltip />} cursor={{ stroke: "rgba(255,255,255,0.1)", strokeWidth: 1 }} />
                      {accounts[0] && <Line type="monotone" dataKey="acc1" name={accounts[0].email} stroke={accounts[0].color} strokeWidth={1.5} dot={false} activeDot={{ r: 3, fill: accounts[0].color, strokeWidth: 0 }} />}
                      {accounts[1] && <Line type="monotone" dataKey="acc2" name={accounts[1].email} stroke={accounts[1].color} strokeWidth={1.5} dot={false} activeDot={{ r: 3, fill: accounts[1].color, strokeWidth: 0 }} />}
                    </LineChart>
                  </ResponsiveContainer>
                </div>
              )}

              {/* ── Accounts ── */}
              <div>
                <div className="flex items-center justify-between mb-3" style={{ borderBottom: "1px solid rgba(255,255,255,0.07)", paddingBottom: "10px" }}>
                  <span className="text-[13px] font-semibold text-[#e5e5ea] font-['Inter',sans-serif]">{provider.name}</span>
                  <span className="text-[10px] text-[#636366] font-['Inter',sans-serif] max-w-[260px] text-right leading-snug">
                    Accounts are separate credentials. Pause and routing eligibility are controlled independently.
                  </span>
                </div>

                <div className="flex flex-col gap-3">
                  {accounts.length === 0 ? (
                    <div className="rounded-xl p-6 text-center" style={{ background: "rgba(255,255,255,0.03)", border: "1px dashed rgba(255,255,255,0.1)" }}>
                      <p className="text-[12px] text-[#636366] font-['Inter',sans-serif]">No accounts connected for {provider.name}</p>
                    </div>
                  ) : (
                    accounts.map((account) => (
                      <AccountCard key={account.id} account={account} routingConfig={routingConfig} onRoutingChange={updateAccountRouting} />
                    ))
                  )}
                </div>

                <button
                  className="w-full py-[9px] mt-3 rounded-xl text-[12px] font-medium font-['Inter',sans-serif] transition-all"
                  style={{ background: "rgba(10,132,255,0.10)", color: "#0a84ff", border: "1px dashed rgba(10,132,255,0.28)" }}
                >
                  + Add account
                </button>
              </div>

              {/* ── Routing rules ── */}
              {accounts.length > 1 && (
                <div>
                  <div className="flex items-center justify-between mb-1" style={{ borderBottom: "1px solid rgba(255,255,255,0.07)", paddingBottom: "10px" }}>
                    <span className="text-[13px] font-semibold text-[#e5e5ea] font-['Inter',sans-serif]">{provider.name} routing</span>
                  </div>
                  <p className="text-[11px] text-[#636366] font-['Inter',sans-serif] mb-4 leading-relaxed">
                    Choose how connected accounts may receive requests. Pausing an account disables it more broadly and always overrides this setting.
                  </p>

                  <div className="flex flex-col gap-px overflow-hidden rounded-xl" style={{ border: "1px solid rgba(255,255,255,0.07)" }}>

                    {/* Selection mode */}
                    <div className="px-4 py-3" style={{ background: "rgba(255,255,255,0.03)" }}>
                      <div className="flex items-center justify-between mb-[6px]">
                        <span className="text-[12px] font-medium text-[#e5e5ea] font-['Inter',sans-serif]">Selection mode</span>
                        <select
                          value={routingConfig.selectionMode}
                          onChange={(e) => updateRouting({ selectionMode: e.target.value as any })}
                          className="text-[11px] font-medium font-['Inter',sans-serif] px-2 py-[4px] rounded-[7px] outline-none appearance-none cursor-pointer"
                          style={{ background: "rgba(255,255,255,0.08)", color: "#e5e5ea", border: "1px solid rgba(255,255,255,0.1)" }}
                        >
                          {SELECTION_MODES.map((m) => (
                            <option key={m.value} value={m.value} style={{ background: "#2c2c2e" }}>{m.label}</option>
                          ))}
                        </select>
                      </div>
                      <p className="text-[11px] text-[#636366] font-['Inter',sans-serif] leading-relaxed">{selectionModeInfo.description}</p>
                    </div>

                    <div style={{ height: "1px", background: "rgba(255,255,255,0.07)" }} />

                    {/* Provider-wide reserve */}
                    <div className="px-4 py-3" style={{ background: "rgba(255,255,255,0.03)" }}>
                      <div className="flex items-center justify-between mb-[6px]">
                        <span className="text-[12px] font-medium text-[#e5e5ea] font-['Inter',sans-serif]">Provider-wide reserve</span>
                        <div className="flex items-center gap-2">
                          <input
                            type="range"
                            min={0}
                            max={50}
                            value={routingConfig.providerReserve}
                            onChange={(e) => updateRouting({ providerReserve: Number(e.target.value) })}
                            className="w-[80px] accent-[#0a84ff] cursor-pointer"
                          />
                          <span
                            className="text-[11px] font-medium font-['Menlo',monospace] px-2 py-[3px] rounded-md min-w-[42px] text-center"
                            style={{ background: "rgba(255,255,255,0.08)", color: "#aeaeb2" }}
                          >
                            {routingConfig.providerReserve}%
                          </span>
                        </div>
                      </div>
                      <p className="text-[11px] text-[#636366] font-['Inter',sans-serif] leading-relaxed">
                        Accounts below this token threshold are skipped for new sessions. Changing this updates accounts still using the default; set a per-account reserve below to override individually.
                      </p>
                    </div>

                    <div style={{ height: "1px", background: "rgba(255,255,255,0.07)" }} />

                    {/* Allow mid-thread failover */}
                    <div className="px-4 py-3" style={{ background: "rgba(255,255,255,0.03)" }}>
                      <div className="flex items-center justify-between mb-[6px]">
                        <span className="text-[12px] font-medium text-[#e5e5ea] font-['Inter',sans-serif]">Allow mid-thread account failover</span>
                        <Toggle on={routingConfig.allowFailover} onChange={(v) => updateRouting({ allowFailover: v })} />
                      </div>
                      <p className="text-[11px] text-[#636366] font-['Inter',sans-serif] leading-relaxed">
                        If the assigned account hits an auth, rate-limit, or server failure, Alex may move that thread to another eligible account. This keeps work moving but can reduce prompt-cache reuse.
                      </p>
                    </div>

                    <div style={{ height: "1px", background: "rgba(255,255,255,0.07)" }} />

                    {/* Per-account routing rows */}
                    {accounts.map((account, i) => {
                      const setting = routingConfig.accountSettings[account.id] ?? { enabled: true, reserve: 10 };
                      return (
                        <div key={account.id}>
                          {i > 0 && <div style={{ height: "1px", background: "rgba(255,255,255,0.05)" }} />}
                          <div className="px-4 py-3" style={{ background: "rgba(255,255,255,0.02)" }}>
                            <div className="flex items-center justify-between gap-3 mb-[6px]">
                              <div className="flex items-center gap-2 min-w-0">
                                <Toggle on={setting.enabled} onChange={(v) => updateAccountRouting(account.id, "enabled", v)} />
                                <span className="text-[11px] font-medium text-[#aeaeb2] font-['Inter',sans-serif] truncate">{account.email}</span>
                                <span
                                  className="text-[10px] font-medium px-2 py-[2px] rounded-md flex-shrink-0 font-['Inter',sans-serif]"
                                  style={{
                                    background: setting.enabled ? "rgba(10,132,255,0.15)" : "rgba(255,255,255,0.06)",
                                    color: setting.enabled ? "#0a84ff" : "#636366",
                                  }}
                                >
                                  {setting.enabled ? "active" : "skipped"}
                                </span>
                              </div>
                              <div className="flex items-center gap-2 flex-shrink-0">
                                <span className="text-[10px] text-[#636366] font-['Inter',sans-serif]">Reserve</span>
                                <input
                                  type="range"
                                  min={0}
                                  max={50}
                                  value={setting.reserve}
                                  onChange={(e) => updateAccountRouting(account.id, "reserve", Number(e.target.value))}
                                  className="w-[60px] accent-[#0a84ff] cursor-pointer"
                                />
                                <span
                                  className="text-[11px] font-medium font-['Menlo',monospace] px-2 py-[2px] rounded-md min-w-[36px] text-center"
                                  style={{ background: "rgba(255,255,255,0.07)", color: "#aeaeb2" }}
                                >
                                  {setting.reserve}%
                                </span>
                              </div>
                            </div>
                            <p className="text-[10px] text-[#636366] font-['Menlo',monospace]">{account.identifier}</p>
                          </div>
                        </div>
                      );
                    })}
                  </div>

                </div>
              )}

            </div>
            </div>

            {/* Save / Cancel — pinned outside scroll */}
            <div
              className="flex items-center justify-end gap-2 px-5 py-3 flex-shrink-0"
              style={{ borderTop: "1px solid rgba(255,255,255,0.07)" }}
            >
              <button
                onClick={cancelRouting}
                className="text-[12px] font-medium font-['Inter',sans-serif] px-4 py-[7px] rounded-[8px] transition-all"
                style={{
                  background: "rgba(255,255,255,0.06)",
                  color: isDirty ? "#aeaeb2" : "#3a3a3c",
                  cursor: isDirty ? "pointer" : "default",
                  pointerEvents: isDirty ? "auto" : "none",
                }}
              >
                Cancel
              </button>
              <button
                onClick={saveRouting}
                className="text-[12px] font-medium font-['Inter',sans-serif] px-4 py-[7px] rounded-[8px] transition-all"
                style={{
                  background: isDirty ? "#0a84ff" : "rgba(255,255,255,0.06)",
                  color: isDirty ? "#ffffff" : "#3a3a3c",
                  cursor: isDirty ? "pointer" : "default",
                }}
              >
                {isDirty ? "Save routing" : "Saved"}
              </button>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
