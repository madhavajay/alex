import { useState } from "react";
import { ExternalLink, Check, Loader2 } from "lucide-react";
import { ImageWithFallback } from "@/app/components/figma/ImageWithFallback";
import claudeCodeLogo from "@/imports/claude-code.png";

const CODE = "7CPP–4JXRJ";
const O = "#D4693A"; // Anthropic orange — single source of truth
const O15 = "rgba(212,105,58,0.15)";
const O28 = "rgba(212,105,58,0.28)";

type CopiedState = "link" | "code" | null;

export default function App() {
  const [copied, setCopied] = useState<CopiedState>(null);

  const handleCopy = (type: "link" | "code", text: string) => {
    navigator.clipboard.writeText(text).catch(() => {});
    setCopied(type);
    setTimeout(() => setCopied(null), 1800);
  };

  return (
    <div
      className="size-full flex items-center justify-center"
      style={{ background: "#111113" }}
    >
      <div
        className="flex flex-col rounded-[12px] overflow-hidden w-[480px] shadow-2xl"
        style={{
          background: "#1c1c1e",
          border: "1px solid rgba(255,255,255,0.1)",
          boxShadow:
            "0 32px 80px rgba(0,0,0,0.7), 0 0 0 0.5px rgba(255,255,255,0.06)",
        }}
      >
        {/* Title bar */}
        <div
          className="flex items-center gap-[8px] px-[16px] h-[48px] shrink-0 relative"
          style={{ borderBottom: "1px solid rgba(255,255,255,0.07)" }}
        >
          <button className="size-[12px] rounded-full bg-[#ff5f57] hover:brightness-110 transition-all" />
          <button className="size-[12px] rounded-full bg-[#febc2e] hover:brightness-110 transition-all" />
          <button className="size-[12px] rounded-full bg-[#28c840] hover:brightness-110 transition-all" />
          <span
            className="absolute left-1/2 -translate-x-1/2 text-[13px] font-semibold whitespace-nowrap"
            style={{ color: "#e5e5ea", fontFamily: "'Inter', sans-serif", letterSpacing: "-0.01em" }}
          >
            Add Claude Code Account
          </span>
        </div>

        {/* Body */}
        <div className="flex flex-col gap-[20px] px-[24px] pt-[24px] pb-[20px]">

          {/* Service identity row */}
          <div className="flex items-center gap-[14px]">
            <div
              className="flex items-center justify-center size-[44px] rounded-[10px] shrink-0 overflow-hidden"
              style={{ background: O15, border: `1px solid ${O28}` }}
            >
              <ImageWithFallback
                src={claudeCodeLogo}
                alt="Anthropic logo"
                className="size-[26px] object-contain"
                style={{ filter: "invert(1) sepia(1) saturate(3) hue-rotate(340deg) brightness(1.1)" }}
              />
            </div>
            <div className="flex flex-col gap-[2px]">
              <span
                className="text-[15px] font-semibold leading-none"
                style={{ color: "#e5e5ea", fontFamily: "'Inter', sans-serif", letterSpacing: "-0.015em" }}
              >
                Claude Code
              </span>
              <span
                className="text-[12px] leading-none"
                style={{ color: "#636366", fontFamily: "'Inter', sans-serif" }}
              >
                by Anthropic
              </span>
            </div>
            <div
              className="ml-auto flex items-center gap-[5px] px-[8px] py-[3px] rounded-full"
              style={{ background: O15, border: `1px solid ${O28}` }}
            >
              <div className="size-[5px] rounded-full" style={{ background: O }} />
              <span className="text-[10px] font-medium" style={{ color: O, fontFamily: "'Inter', sans-serif" }}>
                OAuth Device Flow
              </span>
            </div>
          </div>

          {/* Divider */}
          <div style={{ height: "1px", background: "rgba(255,255,255,0.06)" }} />

          {/* Step 1 */}
          <div className="flex flex-col gap-[10px]">
            <div className="flex items-center gap-[10px]">
              <StepBadge n={1} />
              <span className="text-[13px] font-medium" style={{ color: "#e5e5ea", fontFamily: "'Inter', sans-serif" }}>
                Open the authorization page.
              </span>
            </div>
            <div className="flex gap-[8px]">
              <ActionButton
                icon={<ExternalLink size={12} strokeWidth={2} />}
                label="Open in Browser"
                onClick={() => {}}
              />
              <ActionButton
                icon={copied === "link" ? <Check size={12} strokeWidth={2.5} /> : <CopyIcon size={12} />}
                label={copied === "link" ? "Copied!" : "Copy Link"}
                onClick={() => handleCopy("link", "https://claude.ai/auth/device")}
              />
            </div>
          </div>

          {/* Step 2 */}
          <div className="flex flex-col gap-[10px]">
            <div className="flex items-center gap-[10px]">
              <StepBadge n={2} />
              <span className="text-[13px] font-medium" style={{ color: "#e5e5ea", fontFamily: "'Inter', sans-serif" }}>
                Enter this code when Claude asks for it:
              </span>
            </div>
            <div className="flex items-center gap-[10px]">
              <div
                className="flex-1 flex items-center justify-center h-[44px] rounded-[8px]"
                style={{ background: "rgba(255,255,255,0.04)", border: "1px solid rgba(255,255,255,0.08)" }}
              >
                <span
                  className="select-all text-[22px] font-semibold"
                  style={{ fontFamily: "Menlo, 'Courier New', monospace", color: "#e5e5ea", letterSpacing: "0.12em" }}
                >
                  {CODE}
                </span>
              </div>
              <button
                onClick={() => handleCopy("code", CODE)}
                className="flex items-center justify-center h-[44px] w-[44px] rounded-[8px] transition-all hover:brightness-110 active:scale-95 shrink-0"
                style={{
                  background: copied === "code" ? "rgba(40,200,64,0.15)" : O15,
                  border: copied === "code" ? "1px solid rgba(40,200,64,0.3)" : `1px solid ${O28}`,
                  color: copied === "code" ? "#28c840" : O,
                }}
                title="Copy code"
              >
                {copied === "code" ? <Check size={15} strokeWidth={2.5} /> : <CopyIcon size={15} />}
              </button>
            </div>
          </div>

          {/* Waiting status */}
          <div
            className="flex items-center gap-[10px] px-[14px] py-[12px] rounded-[8px]"
            style={{ background: "rgba(255,255,255,0.035)", border: "1px solid rgba(255,255,255,0.06)" }}
          >
            <Loader2 size={13} strokeWidth={2} className="animate-spin shrink-0" style={{ color: "#636366" }} />
            <span className="text-[12px]" style={{ color: "#8e8e93", fontFamily: "'Inter', sans-serif" }}>
              Waiting for authorization — keep this window open.
            </span>
          </div>
        </div>

        {/* Footer */}
        <div className="flex items-center justify-end px-[16px] pb-[16px]">
          <button
            className="px-[16px] h-[28px] rounded-[6px] text-[12px] font-medium transition-all hover:bg-white/10 active:scale-95"
            style={{
              background: "rgba(255,255,255,0.06)",
              border: "1px solid rgba(255,255,255,0.1)",
              color: "#e5e5ea",
              fontFamily: "'Inter', sans-serif",
            }}
          >
            Cancel
          </button>
        </div>
      </div>
    </div>
  );
}

function StepBadge({ n }: { n: number }) {
  return (
    <div
      className="flex items-center justify-center size-[22px] rounded-[5px] shrink-0 text-[11px] font-semibold"
      style={{
        background: "rgba(212,105,58,0.15)",
        border: "1px solid rgba(212,105,58,0.28)",
        color: "#D4693A",
        fontFamily: "'Inter', sans-serif",
      }}
    >
      {n}
    </div>
  );
}

function ActionButton({
  icon,
  label,
  onClick,
}: {
  icon: React.ReactNode;
  label: string;
  onClick: () => void;
}) {
  return (
    <button
      onClick={onClick}
      className="flex items-center gap-[5px] h-[30px] px-[10px] rounded-[6px] text-[12px] font-medium transition-all hover:brightness-110 active:scale-95"
      style={{
        background: "rgba(212,105,58,0.12)",
        border: "1px solid rgba(212,105,58,0.25)",
        color: "#D4693A",
        fontFamily: "'Inter', sans-serif",
      }}
    >
      {icon}
      {label}
    </button>
  );
}

function CopyIcon({ size = 13 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
      <rect x="5" y="5" width="9" height="9" rx="1.5" />
      <path d="M11 5V3.5A1.5 1.5 0 0 0 9.5 2h-6A1.5 1.5 0 0 0 2 3.5v6A1.5 1.5 0 0 0 3.5 11H5" />
    </svg>
  );
}
