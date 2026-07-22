/* @cove/player v1.1.0 — generated from packages/cove-player/src */
"use strict";(()=>{var J=null,z=null;function ae(t){try{J=new URL(".",t)}catch{J=new URL("http://localhost/")}}function $e(){if(J)return J;let t=typeof document>"u"?"http://localhost/":document.baseURI;return new URL(".",t)}function Z(t){return new URL(t,$e()).href}async function se(){if(z)return z;if(typeof window>"u"||typeof document>"u")throw new Error("terminal playback requires a browser document");let t=window,e=Z("vendor/asciinema-player.css");if(![...document.querySelectorAll("link[rel=stylesheet]")].some(n=>n.href===e)){let n=document.createElement("link");n.rel="stylesheet",n.href=e,document.head.append(n)}return t.AsciinemaPlayer?(z=Promise.resolve(),z):(z=new Promise((n,o)=>{let r=document.createElement("script");r.src=Z("vendor/asciinema-player.js"),r.onload=()=>n(),r.onerror=()=>{z=null,o(new Error("could not load terminal player"))},document.head.append(r)}),z)}var Y="1.1.0",ce="cove.trace-bundle/v1";var be=256*1024*1024,Ee=128,Q=22,Le=65557,ue=new TextDecoder("utf-8",{fatal:!0}),R=class extends Error{constructor(n,o,r){super(o,r);this.code=n;this.name="CovecastError"}},y=(t,e)=>(t[e]??0)|(t[e+1]??0)<<8,A=(t,e)=>(y(t,e)|y(t,e+2)<<16)>>>0;function b(t,e){throw new R(t,e)}function le(t){let e;try{e=ue.decode(t)}catch(n){throw new R("invalid-zip","ZIP member name is not valid UTF-8",{cause:n})}return(!e||e.includes("\0")||e.startsWith("/")||e.split("/").includes(".."))&&b("invalid-zip",`unsafe ZIP member name ${JSON.stringify(e)}`),e}async function Ce(t,e){typeof DecompressionStream>"u"&&b("unsupported-compression",`ZIP compression for ${e} is unsupported by this browser`);try{let n=new Blob([new Uint8Array(t).buffer]).stream().pipeThrough(new DecompressionStream("deflate-raw"));return new Uint8Array(await new Response(n).arrayBuffer())}catch(n){throw new R("invalid-zip",`could not decompress ZIP member ${e}`,{cause:n})}}async function Me(t,e=be){t.byteLength>e&&b("bundle-too-large","bundle is too large");let n=new Uint8Array(t);n.byteLength<Q&&b("invalid-zip","ZIP end record not found");let o=Math.max(0,n.length-Le),r=-1;for(let f=n.length-Q;f>=o;f-=1)if(A(n,f)===101010256){r=f;break}r<0&&b("invalid-zip","ZIP end record not found"),r+Q+y(n,r+20)!==n.length&&b("invalid-zip","invalid ZIP end record"),(y(n,r+4)!==0||y(n,r+6)!==0)&&b("unsupported-zip","multi-disk ZIP bundles are unsupported");let s=y(n,r+8),d=y(n,r+10),l=A(n,r+12),i=A(n,r+16);(s!==d||d>Ee)&&b("invalid-zip","invalid ZIP member count"),i+l!==r&&b("invalid-zip","invalid ZIP central directory bounds");let m=new Map,x=new Map,h=0;for(let f=d;f>0;f-=1){(i+46>r||A(n,i)!==33639248)&&b("invalid-zip","invalid ZIP central directory");let u=y(n,i+8),w=y(n,i+10),$=A(n,i+20),E=A(n,i+24),L=y(n,i+28),M=y(n,i+30),O=y(n,i+32),S=A(n,i+42),_=i+46+L+M+O;_>r&&b("invalid-zip","truncated ZIP central directory");let k=le(n.slice(i+46,i+46+L));m.has(k)&&b("invalid-zip",`duplicate ZIP member ${k}`),(u&1)!==0&&b("unsupported-zip",`encrypted ZIP member ${k} is unsupported`),w!==0&&w!==8&&b("unsupported-compression",`ZIP compression for ${k} is unsupported by this browser`),(S+30>i||A(n,S)!==67324752)&&b("invalid-zip",`invalid ZIP member ${k}`);let H=y(n,S+6),V=y(n,S+8),v=y(n,S+26),P=y(n,S+28),T=S+30+v+P;(H!==u||V!==w||T+$>i)&&b("invalid-zip",`invalid ZIP member ${k}`),le(n.slice(S+30,S+30+v))!==k&&b("invalid-zip",`ZIP member name mismatch for ${k}`),h+=E,h>e&&b("bundle-too-large","expanded bundle is too large"),m.set(k,{method:w,body:n.slice(T,T+$),size:E}),i=_}return i!==r&&b("invalid-zip","invalid ZIP central directory size"),{names:new Set(m.keys()),async member(f){let u=x.get(f);if(u)return u;let w=m.get(f);if(!w)return null;let $=w.method===0?w.body:await Ce(w.body,f);return $.byteLength!==w.size&&b("invalid-zip",`invalid size for ZIP member ${f}`),x.set(f,$),$}}}async function _e(t){globalThis.crypto?.subtle||b("verification-unavailable","bundle verification requires a secure browser context");let e=new Uint8Array(t).buffer;return[...new Uint8Array(await globalThis.crypto.subtle.digest("SHA-256",e))].map(o=>o.toString(16).padStart(2,"0")).join("")}function Ae(t){(!t||typeof t!="object")&&b("invalid-manifest","invalid bundle member manifest");let e=t;(typeof e.name!="string"||typeof e.sha256!="string"||!/^[a-f0-9]{64}$/i.test(e.sha256)||!Number.isSafeInteger(e.size_bytes)||Number(e.size_bytes)<0)&&b("invalid-manifest","invalid bundle member manifest")}async function Pe(t,e){(e.schema_version!==ce||!Array.isArray(e.members))&&b("unsupported-manifest","unsupported bundle manifest");let n=new Set(["bundle.json"]),o=new Map;for(let r of e.members){Ae(r),n.has(r.name)&&b("invalid-manifest","invalid bundle member manifest"),n.add(r.name);let s=await t.member(r.name);s||b("missing-member",`bundle member ${r.name} is missing`);let d=await _e(s);(s.byteLength!==r.size_bytes||d!==r.sha256.toLowerCase())&&b("integrity-failed",`bundle member ${r.name} failed integrity verification`),o.set(r.name,s)}return([...t.names].some(r=>!n.has(r))||[...n].some(r=>!t.names.has(r)))&&b("member-mismatch","bundle members do not match the manifest"),o}async function ze(t,e){let n=e.maxBundleBytes??be;if(t instanceof ArrayBuffer)return t.byteLength>n&&b("bundle-too-large","bundle is too large"),t.slice(0);if(t instanceof Blob)return t.size>n&&b("bundle-too-large","bundle is too large"),t.arrayBuffer();let o=e.fetch??globalThis.fetch;o||b("fetch-unavailable","this environment cannot load bundle URLs");let r=await o(t,{signal:e.signal});r.ok||b("fetch-failed",`bundle request failed (${r.status})`);let s=Number(r.headers.get("content-length")||0);Number.isFinite(s)&&s>n&&b("bundle-too-large","bundle is too large");let d=await r.arrayBuffer();return d.byteLength>n&&b("bundle-too-large","bundle is too large"),d}function de(t,e){try{return JSON.parse(ue.decode(t))}catch(n){throw new R("invalid-json",`bundle member ${e} is not valid JSON`,{cause:n})}}async function X(t,e={}){let n=await ze(t,e),o=await Me(n,e.maxBundleBytes),r=await o.member("bundle.json");r||b("missing-manifest","bundle is missing bundle.json");let s=de(r,"bundle.json"),d=await Pe(o,s),l=d.get("trace.json"),i=l?de(l,"trace.json"):null;return i&&!Array.isArray(i.turns)&&b("invalid-trace","trace.json has no turns array"),{manifest:s,members:d,trace:i,recording:d.get("recording.cast")??null}}var pe=`cove-player { display: block; }
cove-player[fill] { height: 100%; }

.cb {
  --bg: #1c1c1e;
  --panel: #242426;
  --card: #2c2c2e;
  --raised: #3a3a3c;
  --line: rgba(255, 255, 255, 0.07);
  --line-med: rgba(255, 255, 255, 0.11);
  --text: #e5e5ea;
  --text-sub: #aeaeb2;
  --muted: #8e8e93;
  --dim: #636366;
  --accent: #0a84ff;
  --accent-soft: rgba(10, 132, 255, 0.15);
  --accent-text: #409cff;
  --model-bubble: rgba(10, 84, 160, 0.16);
  --model-line: rgba(10, 132, 255, 0.22);
  --user-bubble: rgba(44, 44, 46, 0.85);
  --user-line: rgba(255, 255, 255, 0.08);
  --ok: #30d158;
  --err: #ff453a;
  --warn: #ffd60a;
  --purple: #bf5af2;
  --teal: #5ac8fa;
  --term-bg: #121214;
  --mono: ui-monospace, "SF Mono", SFMono-Regular, Menlo, Consolas, monospace;
  background: var(--bg);
  border: 1px solid var(--line-med);
  border-radius: 12px;
  color: var(--text);
  color-scheme: dark;
  display: flex;
  flex-direction: column;
  font: 12.5px/1.55 -apple-system, BlinkMacSystemFont, "Inter", "Segoe UI", system-ui, sans-serif;
  overflow: hidden;
}

.cb[data-theme="light"] {
  --bg: #f5f5f7;
  --panel: #ffffff;
  --card: #ffffff;
  --raised: #e9e9ec;
  --line: rgba(0, 0, 0, 0.08);
  --line-med: rgba(0, 0, 0, 0.12);
  --text: #1c1c1e;
  --text-sub: #48484a;
  --muted: #6e6e73;
  --dim: #98989d;
  --accent-soft: rgba(10, 132, 255, 0.10);
  --accent-text: #0a6ae0;
  --model-bubble: rgba(10, 132, 255, 0.07);
  --model-line: rgba(10, 132, 255, 0.20);
  --user-bubble: #ffffff;
  --user-line: rgba(0, 0, 0, 0.10);
  --term-bg: #1c1c1e;
  color-scheme: light;
}

cove-player[fill] .cb { border-radius: 0; border: 0; height: 100%; }

.cb * { box-sizing: border-box; }
.cb button { font: inherit; }

/* \u2500\u2500 Toolbar \u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500 */
.cb-toolbar {
  align-items: center;
  background: var(--panel);
  border-bottom: 1px solid var(--line);
  display: flex;
  flex-shrink: 0;
  gap: 12px;
  min-height: 48px;
  padding: 8px 14px;
}
.cb-title { align-items: baseline; display: flex; gap: 9px; min-width: 0; }
.cb-title strong { font-size: 13px; font-weight: 650; letter-spacing: -0.01em; white-space: nowrap; }
.cb-title span {
  color: var(--muted);
  font-family: var(--mono);
  font-size: 11px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.cb-actions { display: flex; gap: 4px; margin-left: auto; }
.cb-action {
  align-items: center;
  background: transparent;
  border: 1px solid transparent;
  border-radius: 8px;
  color: var(--muted);
  cursor: pointer;
  display: flex;
  font-size: 13px;
  height: 28px;
  justify-content: center;
  min-width: 28px;
  padding: 0 6px;
  transition: background 0.12s, color 0.12s;
}
.cb-action:hover { background: var(--raised); color: var(--text); }

/* \u2500\u2500 Stage layout \u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500 */
.cb-stage {
  display: grid;
  flex: 1;
  grid-template-columns: minmax(0, 1fr);
  grid-template-rows: minmax(0, 1fr);
  min-height: 0;
  overflow: auto;
}
.cb-shell {
  background: var(--term-bg);
  display: flex;
  flex-direction: column;
  min-height: 180px;
  min-width: 0;
  overflow: hidden;
  position: relative;
}
.cb-terminal { flex: 1; min-height: 0; overflow: auto; }
.cb-terminal .ap-player { border-radius: 0; }

/* \u2500\u2500 Timeline \u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500 */
.cb-timeline {
  background: rgba(255, 255, 255, 0.04);
  border-bottom: 1px solid var(--line);
  flex-shrink: 0;
  height: 14px;
  position: relative;
}
.cb-tick {
  appearance: none;
  background: var(--accent);
  border: 0;
  border-radius: 2px;
  cursor: pointer;
  height: 8px;
  margin-left: -1.5px;
  opacity: 0.55;
  padding: 0;
  position: absolute;
  top: 3px;
  transition: opacity 0.12s;
  width: 3px;
}
.cb-tick:hover, .cb-tick:focus-visible { opacity: 1; outline: 0; box-shadow: 0 0 0 3px var(--accent-soft); }

/* \u2500\u2500 Trace rail \u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500 */
.cb-rail {
  background: var(--bg);
  border-top: 1px solid var(--line);
  display: flex;
  flex-direction: column;
  max-height: 520px;
  min-height: 0;
  min-width: 0;
  overflow: auto;
  overscroll-behavior: contain;
  scrollbar-width: thin;
}
.cb-rail-head {
  align-items: stretch;
  background: color-mix(in srgb, var(--panel) 88%, transparent);
  backdrop-filter: blur(10px);
  border-bottom: 1px solid var(--line);
  display: flex;
  flex-direction: column;
  flex-shrink: 0;
  gap: 8px;
  padding: 10px 14px 9px;
  position: sticky;
  top: 0;
  z-index: 3;
}
.cb-mode {
  background: rgba(255, 255, 255, 0.05);
  border: 1px solid var(--line);
  border-radius: 9px;
  display: flex;
  gap: 2px;
  padding: 2px;
}
.cb[data-theme="light"] .cb-mode { background: rgba(0, 0, 0, 0.04); }
.cb-mode-btn {
  background: none;
  border: 0;
  border-radius: 7px;
  color: var(--muted);
  cursor: pointer;
  flex: 1;
  font-family: var(--mono);
  font-size: 10.5px;
  font-weight: 650;
  padding: 5px 10px;
  text-align: center;
  transition: background 0.12s, color 0.12s;
  white-space: nowrap;
}
.cb-mode-btn:hover { color: var(--text); }
.cb-mode-btn.active { background: var(--accent); color: #fff; }
.cb-stats { display: flex; flex-wrap: wrap; gap: 5px; }
.cb-stat {
  background: var(--raised);
  border-radius: 999px;
  color: var(--text-sub);
  font-family: var(--mono);
  font-size: 10px;
  padding: 2px 8px;
  white-space: nowrap;
}

/* \u2500\u2500 Turns (chat) \u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500 */
.cb-chat { display: flex; flex-direction: column; gap: 2px; padding: 10px 12px 18px; }
.cb-turn { border-left: 3px solid transparent; border-radius: 10px; padding: 4px 6px 6px 8px; transition: background 0.15s, border-color 0.15s, box-shadow 0.15s; }
.cb-turn.current {
  background: color-mix(in srgb, var(--accent) 13%, transparent);
  border-left-color: var(--accent);
  box-shadow: inset 0 0 0 1px var(--model-line);
}
.cb-turn.current > .cb-turn-bar time { color: var(--accent-text); font-weight: 700; }
.cb-turn.cb-preflight { opacity: 0.62; }

.cb-turn-bar { align-items: center; display: flex; gap: 2px; }
.cb-turn-bar .cb-turn-head { flex: 1; min-width: 0; }
.cb-inspect {
  background: none;
  border: 0;
  border-radius: 6px;
  color: var(--dim);
  cursor: pointer;
  flex-shrink: 0;
  font-family: var(--mono);
  font-size: 10px;
  opacity: 0.5;
  padding: 3px 6px;
  transition: opacity 0.12s, background 0.12s, color 0.12s;
}
.cb-turn:hover .cb-inspect, .cb-inspect:focus-visible, .cb-turn.inspected .cb-inspect { opacity: 1; }
.cb-inspect:hover { background: var(--raised); color: var(--text); }
.cb-turn.inspected .cb-inspect { background: var(--accent-soft); color: var(--accent-text); }

.cb-turn-head {
  align-items: center;
  background: none;
  border: 0;
  border-radius: 6px;
  color: inherit;
  cursor: pointer;
  display: flex;
  gap: 7px;
  padding: 3px 4px;
  text-align: left;
  width: 100%;
}
.cb-turn-head:hover:not(:disabled) { background: rgba(255, 255, 255, 0.04); }
.cb[data-theme="light"] .cb-turn-head:hover:not(:disabled) { background: rgba(0, 0, 0, 0.04); }
.cb-turn-head:disabled { cursor: default; }
.cb-turn-head time {
  color: var(--dim);
  flex-shrink: 0;
  font-family: var(--mono);
  font-size: 10px;
  font-variant-numeric: tabular-nums;
  margin-left: auto;
}
.cb-turn-head:hover:not(:disabled) time { color: var(--accent-text); }

.cb-avatar {
  align-items: center;
  background: var(--raised);
  border: 1px solid var(--line-med);
  border-radius: 50%;
  color: var(--text-sub);
  display: flex;
  flex-shrink: 0;
  font-size: 10px;
  height: 20px;
  justify-content: center;
  width: 20px;
}
.cb-model .cb-avatar, .cb-res-head .cb-avatar {
  background: linear-gradient(135deg, rgba(10, 132, 255, 0.35), rgba(94, 92, 230, 0.35));
  border-color: rgba(10, 132, 255, 0.45);
  color: #cfe3ff;
}
.cb[data-theme="light"] .cb-model .cb-avatar, .cb[data-theme="light"] .cb-res-head .cb-avatar { color: #0a6ae0; }

.cb-role { flex-shrink: 0; font-size: 11px; font-weight: 650; }
.cb-turn-head .cb-role { color: var(--text-sub); }
.cb-model > .cb-turn-bar .cb-role, .cb-res-head .cb-role { color: var(--accent-text); }

.cb-res-head {
  align-items: center;
  background: none;
  border: 0;
  border-radius: 6px;
  color: inherit;
  cursor: pointer;
  display: flex;
  gap: 7px;
  margin-top: 4px;
  padding: 3px 4px;
  text-align: left;
  width: 100%;
}
.cb-res-head:hover:not(:disabled) { background: rgba(255, 255, 255, 0.04); }
.cb[data-theme="light"] .cb-res-head:hover:not(:disabled) { background: rgba(0, 0, 0, 0.04); }
.cb-res-head:disabled { cursor: default; }
.cb-res-head time {
  color: var(--dim);
  flex-shrink: 0;
  font-family: var(--mono);
  font-size: 10px;
  font-variant-numeric: tabular-nums;
  margin-left: auto;
}
.cb-res-head:hover:not(:disabled) time { color: var(--accent-text); }

.cb-live {
  animation: cb-pulse 1.1s ease-in-out infinite;
  color: var(--warn);
  display: none;
  font-size: 10px;
  white-space: nowrap;
}
.cb-turn.current.awaiting .cb-live { display: inline; }
.cb-res-body { transition: opacity 0.3s; }
.cb-turn.current.awaiting .cb-res-body { opacity: 0.4; }
@keyframes cb-pulse { 0%, 100% { opacity: 0.35; } 50% { opacity: 1; } }

.cb-badge {
  border: 1px solid;
  border-radius: 5px;
  flex-shrink: 1;
  font-family: var(--mono);
  font-size: 9.5px;
  overflow: hidden;
  padding: 1px 6px;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.cb-md-opus { background: rgba(191, 90, 242, 0.12); border-color: rgba(191, 90, 242, 0.28); color: var(--purple); }
.cb-md-sonnet { background: rgba(10, 132, 255, 0.12); border-color: rgba(10, 132, 255, 0.28); color: var(--accent-text); }
.cb-md-haiku { background: rgba(48, 209, 88, 0.10); border-color: rgba(48, 209, 88, 0.25); color: var(--ok); }
.cb-md-other { background: rgba(90, 200, 250, 0.10); border-color: rgba(90, 200, 250, 0.25); color: var(--teal); }
.cb-md-default { background: var(--raised); border-color: var(--line-med); color: var(--text-sub); }

.cb-meta { color: var(--dim); flex-shrink: 0; font-family: var(--mono); font-size: 10px; white-space: nowrap; }

/* \u2500\u2500 Bubbles \u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500 */
.cb-bubble { margin: 2px 0 2px 27px; max-width: 100%; }
.cb-x > .cb-bubble > .cb-text,
.cb-x > .cb-bubble > .cb-context {
  background: var(--user-bubble);
  border: 1px solid var(--user-line);
  border-radius: 12px 12px 12px 4px;
}
.cb-response > .cb-bubble > .cb-text {
  background: var(--model-bubble);
  border: 1px solid var(--model-line);
  border-radius: 12px 12px 12px 4px;
}
.cb-text {
  color: var(--text);
  font-size: 12px;
  line-height: 1.62;
  max-height: 340px;
  overflow: auto;
  overflow-wrap: anywhere;
  padding: 8px 12px;
  scrollbar-width: thin;
  white-space: pre-wrap;
}
.cb-response .cb-text { color: #dde8ff; }
.cb[data-theme="light"] .cb-response .cb-text { color: #1c3a5e; }
.cb-faint { color: var(--dim); font-style: italic; }
.cb-text code, .cb-context code {
  background: rgba(255, 255, 255, 0.09);
  border-radius: 4px;
  font-family: var(--mono);
  font-size: 11px;
  padding: 1px 4px;
}
.cb[data-theme="light"] .cb-text code { background: rgba(0, 0, 0, 0.06); }

.cb-context { border-radius: 12px 12px 12px 4px; overflow: hidden; }
.cb-context > summary {
  color: var(--muted);
  cursor: pointer;
  font-family: var(--mono);
  font-size: 10.5px;
  list-style: none;
  overflow: hidden;
  padding: 7px 12px;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.cb-context > summary::-webkit-details-marker { display: none; }
.cb-context > summary::before { color: var(--dim); content: "\u25B8 "; }
.cb-context[open] > summary::before { content: "\u25BE "; }
.cb-context > .cb-code { border-radius: 0; margin: 0; }

/* \u2500\u2500 Code blocks \u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500 */
.cb-code {
  background: #161618;
  border: 1px solid var(--line);
  border-radius: 8px;
  color: #d4d4d8;
  font-family: var(--mono);
  font-size: 11px;
  line-height: 1.55;
  margin: 6px 0;
  max-height: 300px;
  overflow: auto;
  overflow-wrap: anywhere;
  padding: 8px 10px;
  position: relative;
  scrollbar-width: thin;
  white-space: pre-wrap;
}
.cb[data-theme="light"] .cb-code { background: #f6f6f8; border-color: var(--line-med); color: #333338; }
.cb-code[data-lang]::before {
  color: var(--dim);
  content: attr(data-lang);
  float: right;
  font-size: 9px;
  letter-spacing: 0.06em;
  margin-left: 8px;
  text-transform: uppercase;
}
.cb-code i { font-style: normal; }
.cb-hk { color: #7dd3fc; }
.cb-hs { color: #86efac; }
.cb-hn { color: #fbbf24; }
.cb-hb { color: #c4b5fd; }
.cb[data-theme="light"] .cb-hk { color: #0369a1; }
.cb[data-theme="light"] .cb-hs { color: #15803d; }
.cb[data-theme="light"] .cb-hn { color: #b45309; }
.cb[data-theme="light"] .cb-hb { color: #7c3aed; }

/* \u2500\u2500 Tool calls \u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500 */
.cb-tools { display: flex; flex-direction: column; gap: 5px; margin-top: 7px; }
.cb-tools-label { color: var(--dim); font-family: var(--mono); font-size: 10px; padding-left: 2px; }
.cb-tool {
  background: var(--card);
  border: 1px solid var(--line);
  border-radius: 9px;
  overflow: hidden;
}
.cb-tool[open] { border-color: var(--line-med); }
.cb-tool > summary {
  align-items: center;
  cursor: pointer;
  display: flex;
  gap: 8px;
  list-style: none;
  min-width: 0;
  padding: 6px 10px;
  transition: background 0.12s;
}
.cb-tool > summary:hover { background: rgba(255, 255, 255, 0.04); }
.cb[data-theme="light"] .cb-tool > summary:hover { background: rgba(0, 0, 0, 0.03); }
.cb-tool > summary::-webkit-details-marker { display: none; }
.cb-tool-glyph {
  align-items: center;
  background: var(--raised);
  border-radius: 6px;
  color: var(--text-sub);
  display: flex;
  flex-shrink: 0;
  font-family: var(--mono);
  font-size: 10px;
  height: 18px;
  justify-content: center;
  width: 20px;
}
.cb-tool > summary b { flex-shrink: 0; font-family: var(--mono); font-size: 11px; font-weight: 600; }
.cb-tool > summary small {
  color: var(--muted);
  flex: 1;
  font-family: var(--mono);
  font-size: 10.5px;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.cb-tool-status { color: var(--dim); flex-shrink: 0; font-family: var(--mono); font-size: 9.5px; }
.cb-tool-status.cb-ok { color: var(--ok); }
.cb-tool-chev { border-right: 1.5px solid var(--dim); border-top: 1.5px solid var(--dim); flex-shrink: 0; height: 5px; transform: rotate(135deg); transition: transform 0.15s; width: 5px; }
.cb-tool[open] > .cb-tool-chev, .cb-tool[open] > summary .cb-tool-chev { transform: rotate(-45deg) translateY(2px); }
.cb-tool-body { border-top: 1px solid var(--line); padding: 4px 10px 9px; }
.cb-tool-body label {
  color: var(--dim);
  display: block;
  font-size: 9.5px;
  font-weight: 650;
  letter-spacing: 0.07em;
  margin: 7px 0 0;
  text-transform: uppercase;
}

/* \u2500\u2500 Thinking \u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500 */
.cb-thinking {
  border-left: 2px solid var(--line-med);
  margin-bottom: 6px;
  padding-left: 10px;
}
.cb-thinking > summary { color: var(--dim); cursor: pointer; font-size: 10.5px; font-style: italic; list-style: none; }
.cb-thinking > summary::-webkit-details-marker { display: none; }
.cb-thinking > div { color: var(--muted); font-size: 11px; padding-top: 4px; white-space: pre-wrap; }

/* \u2500\u2500 Inspector overlay \u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500 */
.cb-inspector {
  background: var(--bg);
  display: flex;
  flex-direction: column;
  inset: 0;
  overflow: auto;
  overscroll-behavior: contain;
  position: absolute;
  scrollbar-width: thin;
  z-index: 40;
}
.cb-inspector[hidden] { display: none; }
.cb-insp-head {
  align-items: center;
  background: color-mix(in srgb, var(--panel) 88%, transparent);
  backdrop-filter: blur(10px);
  border-bottom: 1px solid var(--line);
  display: flex;
  gap: 10px;
  justify-content: space-between;
  padding: 9px 14px;
  position: sticky;
  top: 0;
  z-index: 2;
}
.cb-insp-head strong { display: block; font-size: 12.5px; font-weight: 650; }
.cb-insp-head small { color: var(--dim); display: block; font-family: var(--mono); font-size: 10px; }
.cb-insp-body { padding: 12px 16px 24px; }
.cb-insp-sec { border: 1px solid var(--line); border-radius: 10px; margin-bottom: 12px; overflow: hidden; }
.cb-insp-sec > summary {
  align-items: center;
  background: var(--panel);
  cursor: pointer;
  display: flex;
  gap: 8px;
  list-style: none;
  padding: 8px 12px;
}
.cb-insp-sec > summary::-webkit-details-marker { display: none; }
.cb-insp-sec > summary b { font-size: 11.5px; }
.cb-insp-sec > summary small { color: var(--dim); font-family: var(--mono); font-size: 10px; margin-left: auto; }
.cb-insp-sec[open] > summary { border-bottom: 1px solid var(--line); }
.cb-insp-sec[open] > summary .cb-tool-chev { transform: rotate(-45deg) translateY(2px); }
.cb-insp-sec-body { padding: 4px 12px 12px; }
.cb-insp-body h4 {
  color: var(--dim);
  font-size: 10px;
  font-weight: 650;
  letter-spacing: 0.07em;
  margin: 16px 0 6px;
  text-transform: uppercase;
}
.cb-kv-grid {
  column-gap: 16px;
  display: grid;
  grid-template-columns: max-content minmax(0, 1fr);
  row-gap: 4px;
}
.cb-kv { display: contents; }
.cb-kv span { color: var(--muted); font-size: 11px; white-space: nowrap; }
.cb-kv code { color: var(--text-sub); font-family: var(--mono); font-size: 10.5px; overflow-wrap: anywhere; }
.cb-raw { max-height: none; }

/* \u2500\u2500 Embed panel \u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500 */
.cb-embed-note { color: var(--text-sub); font-size: 12px; line-height: 1.6; margin: 2px 0 10px; }
.cb-embed-note code { background: rgba(255, 255, 255, 0.09); border-radius: 4px; font-family: var(--mono); font-size: 11px; padding: 1px 4px; }
.cb[data-theme="light"] .cb-embed-note code { background: rgba(0, 0, 0, 0.06); }
.cb-snippet { position: relative; }
.cb-snippet .cb-code { font-size: 11.5px; margin: 0; padding: 12px 14px; }
.cb-copy {
  align-items: center;
  background: var(--raised);
  border: 1px solid var(--line-med);
  border-radius: 7px;
  color: var(--text-sub);
  cursor: pointer;
  display: flex;
  font-size: 10.5px;
  font-weight: 600;
  gap: 5px;
  padding: 4px 10px;
  position: absolute;
  right: 8px;
  top: 8px;
  transition: background 0.12s, color 0.12s, border-color 0.12s;
  z-index: 1;
}
.cb-copy:hover { border-color: var(--accent); color: var(--text); }
.cb-copy.copied { background: rgba(48, 209, 88, 0.15); border-color: rgba(48, 209, 88, 0.4); color: var(--ok); }
.cb-copy-glyph { font-size: 12px; }

/* \u2500\u2500 Empty / error states \u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500 */
.cb-empty {
  align-items: center;
  color: var(--muted);
  display: flex;
  flex: 1;
  justify-content: center;
  min-height: 150px;
  padding: 30px;
  text-align: center;
}
.cb-error { color: var(--err); }

/* \u2500\u2500 No-cast (trace only) \u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500 */
.cb-no-cast .cb-stage { display: flex; flex-direction: column; }
.cb-no-cast .cb-shell { background: var(--panel); flex: 0 0 auto; min-height: 0; }
.cb-no-cast .cb-empty { min-height: 0; padding: 12px; }
.cb-no-cast .cb-rail { border-top: 0; flex: 1; max-height: none; }
.cb-no-cast .cb-inspector { border-bottom: 1px solid var(--line); max-height: 460px; position: static; }

/* \u2500\u2500 Fill & fullscreen \u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500 */
cove-player[fill] .cb-rail, .cb:fullscreen .cb-rail { max-height: none; }
cove-player[fill] .cb-stage { align-content: stretch; grid-template-rows: minmax(0, 1fr); overflow: hidden; }
cove-player[fill] .cb-terminal {
  align-items: stretch;
  display: flex;
  flex-direction: column;
  justify-content: flex-start;
  overflow: hidden;
}
cove-player[fill] .cb:not(.cb-no-cast) .cb-rail { height: auto; min-height: 0; }
.cb:fullscreen { border: 0; border-radius: 0; height: 100%; width: 100%; }

/* \u2500\u2500 Wide layout \u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500 */
@media (min-width: 860px) {
  .cb-stage { align-content: start; grid-template-columns: minmax(0, 1fr) minmax(340px, 440px); grid-template-rows: min-content; }
  .cb-shell { grid-column: 1; }
  .cb-terminal { overflow: visible; }
  .cb-rail { border-left: 1px solid var(--line); border-top: 0; grid-column: 2; }
  .cb:not(.cb-no-cast) .cb-rail { height: 0; max-height: none; min-height: 100%; }
  .cb-no-cast .cb-stage { display: flex; }
}

@media (max-width: 859px) {
  .cb-title span { display: none; }
  .cb-rail { max-height: 440px; }
}

@media (prefers-reduced-motion: reduce) {
  .cb * { scroll-behavior: auto !important; transition: none !important; }
}
`;var p=t=>String(t??"").replace(/[&<>"']/g,e=>({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;"})[e]??e);function B(t){let e=Number(t||0);return e>=1e3?new Intl.NumberFormat().format(e):String(e)}function U(t){let e=Number(t||0);return e>=1e6?`${(e/1e6).toFixed(1)}M`:e>=1e4?`${Math.round(e/1e3)}k`:e>=1e3?`${(e/1e3).toFixed(1)}k`:String(e)}function q(t){let e=Math.max(0,Number(t)||0);return`${Math.floor(e/60)}:${String(Math.floor(e%60)).padStart(2,"0")}`}function Ne(t){let e=String(t||"").toLowerCase();return e.includes("opus")?"cb-md-opus":e.includes("sonnet")?"cb-md-sonnet":e.includes("haiku")?"cb-md-haiku":e.includes("gpt")||e.includes("gemini")?"cb-md-other":"cb-md-default"}var Re=[[/bash|shell|command|exec/i,"$_"],[/read|cat|view|notebook/i,"\u2261"],[/edit|write|create|patch/i,"\u270E"],[/grep|glob|search|find|ls/i,"\u2315"],[/web|fetch|http|browser/i,"\u2295"],[/task|agent|spawn/i,"\u2442"],[/todo|plan/i,"\u2630"]];function Be(t){let e=String(t||"");for(let[n,o]of Re)if(n.test(e))return o;return"\u2699"}var qe=6e3;function D(t,e=qe){return t.length<=e?t:`${t.slice(0,e)}
\u2026 ${B(t.length-e)} more characters`}function Ue(t){return p(t).replace(/(&quot;(?:[^&\\]|\\.|&(?!quot;))*?&quot;)(\s*:)?|\b(true|false|null)\b|(-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?)/g,(e,n,o,r,s)=>n?o?`<i class="cb-hk">${n}</i>${o}`:`<i class="cb-hs">${n}</i>`:r?`<i class="cb-hb">${r}</i>`:s?`<i class="cb-hn">${s}</i>`:e)}function G(t){if(t==null)return"";if(typeof t=="string"){let n=t.trim();if(n.startsWith("{")||n.startsWith("["))try{return G(JSON.parse(n))}catch{}return`<pre class="cb-code">${p(D(t))}</pre>`}let e;try{e=JSON.stringify(t,null,2)??String(t)}catch{e=String(t)}return`<pre class="cb-code">${Ue(D(e))}</pre>`}function je(t){return p(t).replace(/`([^`\n]+)`/g,"<code>$1</code>").replace(/\*\*([^*\n]+)\*\*/g,"<b>$1</b>")}function ee(t){return String(t??"").split("```").map((n,o)=>{if(o%2===1){let r=n.indexOf(`
`),s=r>-1?n.slice(0,r).trim():"",d=(r>-1?n.slice(r+1):n).replace(/\n$/,"");return`<pre class="cb-code"${s?` data-lang="${p(s)}"`:""}>${p(D(d))}</pre>`}return je(o===0?n.replace(/\n$/,""):n.replace(/^\n|\n$/g,""))}).join("")}function ge(t){let e=String(t??"");if(!e.includes("data:"))return null;let n=[],o=null,r=!1;for(let i of e.split(/\r?\n/)){if(!i.startsWith("data:"))continue;let m;try{m=JSON.parse(i.slice(5))}catch{continue}let x=m.type;if(x==="content_block_start")r=!0,n[Number(m.index)]={...m.content_block,json:""};else if(x==="content_block_delta"){r=!0;let h=n[Number(m.index)];if(!h)continue;let f=m.delta??{};f.type==="text_delta"?h.text=(h.text||"")+String(f.text??""):f.type==="input_json_delta"?h.json+=String(f.partial_json??""):f.type==="thinking_delta"&&(h.thinking=(h.thinking||"")+String(f.thinking??""))}else x==="message_delta"&&(r=!0,o=String(m.delta?.stop_reason??o??"")||o)}if(!r)return null;let s=n.filter(i=>i?.type==="text").map(i=>i.text||"").join(`

`).trim(),d=n.filter(i=>i?.type==="thinking").map(i=>i.thinking||"").join(`

`).trim(),l=n.filter(i=>i?.type==="tool_use").map(i=>{let m=i.input;if(i.json)try{m=JSON.parse(i.json)}catch{m=i.json}return{id:i.id,name:i.name,arguments:m}});return{text:s,thinking:d,tools:l,stopReason:o}}function Oe(t){if(t==null)return"";if(typeof t=="string")return t;if(typeof t=="object"){let e=Object.values(t)[0];if(e!==void 0)return typeof e=="string"?e:JSON.stringify(e)}return JSON.stringify(t)}function he(t,e){return t.length>e?`${t.slice(0,e)}\u2026`:t}function Ie(t,e){let n=p(t.name||"tool"),o=he(Oe(t.arguments).replace(/\s+/g," ").trim(),64),r=e==null?'<span class="cb-tool-status">pending</span>':'<span class="cb-tool-status cb-ok">\u2713</span>';return`<details class="cb-tool">
    <summary><span class="cb-tool-glyph">${Be(t.name)}</span><b>${n}</b><small>${p(o)}</small>${r}<span class="cb-tool-chev"></span></summary>
    <div class="cb-tool-body"><label>Input</label>${G(t.arguments)||'<pre class="cb-code">(none)</pre>'}${e==null?"":`<label>Result</label>${G(e)}`}</div>
  </details>`}function Fe(t,e){let n=he(t.replace(/\s+/g," ").trim(),110)||e;return`<details class="cb-context"><summary>${p(n)}</summary><pre class="cb-code">${p(D(t))}</pre></details>`}function ve(t){return String(t.direction||"").toLowerCase().includes("response")}function ye(t){let e=[],n=new Map,o=null;for(let r of t){let s=r.trace_id==null?null:String(r.trace_id);if(!ve(r)){let l={request:r,response:null};e.push(l),s&&n.set(s,l),o=l;continue}let d=(s?n.get(s):null)??(o&&!o.response?o:null);d&&!d.response?(d.response=r,s&&n.delete(s),d===o&&(o=null)):e.push({request:null,response:r})}return e}function te(t){return t&&Number.isFinite(t.offset_seconds)?Number(t.offset_seconds):null}function Ze(t,e,n){let{request:o,response:r}=t,s=r??o,d=te(o),l=te(r),i=d??l,m=i!==null&&(s?.in_recording===!1||i<0),x=String((o??r)?.phase||"")==="preflight",h=i===null?"\xB7":m?"pre":q(i),f=i===null?"This call is not linked to the recording":m?"Before the recording started \xB7 seek to start":`Seek to ${h}`,u=`${U(s?.input_tokens)}\u2192${U(s?.output_tokens)} tok`,w=s?.cost_usd==null?"":` \xB7 $${Number(s.cost_usd).toFixed(3)}`,$=o?.tool_calls??[],E=$.filter(v=>v.id!=null&&!n.delivered.has(String(v.id)));for(let v of $)v.id!=null&&n.delivered.add(String(v.id));let L=String(o?.body??"").trim(),M=L.startsWith("<system-reminder>"),O=o?x?"Preflight":E.length?`${E.length} tool result${E.length===1?"":"s"}`:M?"Harness":"User":"Model",S=o?E.length?"\u21C4":"\u276F":"\u2726",_="";o&&!E.length&&L&&(_=M||x?`<div class="cb-bubble">${Fe(L,"context payload")}</div>`:`<div class="cb-bubble"><div class="cb-text">${ee(L)}</div></div>`);let k="";if(r){let v=ge(r.body),P=v?v.text:String(r.body??"").trim(),T=v?.tools??[],W=v?.thinking?`<details class="cb-thinking"><summary>Thinking</summary><div>${ee(v.thinking)}</div></details>`:"",a=T.length?`<div class="cb-tools"><span class="cb-tools-label">${T.length} tool call${T.length===1?"":"s"}</span>${T.map(C=>Ie(C,C.id==null?null:n.results.get(String(C.id)))).join("")}</div>`:"",c=P?`<div class="cb-text">${ee(P)}</div>`:T.length?"":'<div class="cb-text cb-faint">(no text)</div>',g=r.model?`<span class="cb-badge ${Ne(r.model)}">${p(String(r.model))}</span>`:"",N=l===null?"":`<time>${p(m?"pre":q(l))}</time>`;k=`<div class="cb-response">
      <button type="button" class="cb-res-head"${l===null?" disabled":` data-res-offset="${l}"`} title="${l===null?"Not linked to the recording":`Seek to response at ${q(l)}`}">
        <span class="cb-avatar">\u2726</span><span class="cb-role">Model</span>${g}<span class="cb-live">\u25CF responding\u2026</span>${N}
      </button>
      <div class="cb-bubble cb-res-body">${W}${c}${a}</div>
    </div>`}let H=`data-turn="${e}"${i===null?"":` data-offset="${i}"`}${l===null?"":` data-end="${l}"`}`;return`<article class="cb-turn cb-x${`${x?" cb-preflight":""}${o?"":" cb-model"}`}" ${H}>
    <div class="cb-turn-bar"><button type="button" class="cb-turn-head"${i===null?" disabled":""} title="${p(f)}">
      <span class="cb-avatar">${S}</span><span class="cb-role">${p(O)}</span><span class="cb-meta">${p(u+w)}</span><time>${p(h)}</time>
    </button><button type="button" class="cb-inspect" title="View raw headers &amp; body" aria-label="View raw call details">{&hairsp;}</button></div>
    ${_}${k}
  </article>`}function xe(t){let e={results:new Map,delivered:new Set};for(let n of t){let o=[];for(let r of n.request?.tool_calls??[])if(r.id!=null)r.result!=null?e.results.set(String(r.id),r.result):o.push(String(r.id));else if(r.result!=null){let s=o.shift();s&&e.results.set(s,r.result)}}return t.map((n,o)=>Ze(n,o,e)).join("")}function we(t,e,n){let o=e.reduce((m,x)=>{let h=ve(x)?ge(x.body):null;return m+(h?.tools.length??0)},0),r=t.usage??{},s=r.total_input_tokens??r.input_tokens,d=r.total_output_tokens??r.output_tokens,l=Number(r.total_cost_usd??0);return`<header class="cb-rail-head"><div class="cb-mode" role="tablist" aria-label="Left pane content"><button type="button" class="cb-mode-btn active" data-cb-mode="replay" role="tab" aria-selected="true">\u25B6 Terminal</button><button type="button" class="cb-mode-btn" data-cb-mode="trace" role="tab" aria-selected="false">{&hairsp;} HTTP trace</button><button type="button" class="cb-mode-btn" data-cb-mode="embed" role="tab" aria-selected="false">&lt;/&gt; Embed</button></div><div class="cb-stats">${[`${B(n)} calls`,`${B(o)} tools`,`${U(s)} in \xB7 ${U(d)} out`,l?`$${l.toFixed(2)}`:""].filter(Boolean).map(m=>`<span class="cb-stat">${p(m)}</span>`).join("")}</div></header>`}function me(t){return t.filter(([,e])=>e!=null&&e!=="").map(([e,n])=>`<div class="cb-kv"><span>${p(e)}</span><code>${p(String(n))}</code></div>`).join("")}function fe(t,e){let n=t.timestamp_ms?new Date(Number(t.timestamp_ms)).toISOString():null,o=me([["model",t.model],["phase",t.phase],["http status",t.status],["timestamp",n],["offset",t.offset_seconds==null?null:`${Number(t.offset_seconds).toFixed(2)}s`],["in recording",t.in_recording],["tokens",`${B(t.input_tokens)} in \xB7 ${B(t.output_tokens)} out`],["cost",t.cost_usd==null?null:`$${Number(t.cost_usd).toFixed(4)}`]]),r=Object.entries(t.headers??{}),s=t.tool_calls?.length?`<h4>Tool calls \xB7 ${t.tool_calls.length}</h4>${G(t.tool_calls)}`:"",d=String(t.body??""),l=te(t),i=[l==null?"":q(l),`${U(t.input_tokens)}\u2192${U(t.output_tokens)} tok`].filter(Boolean).join(" \xB7 ");return`<details class="cb-insp-sec" open>
    <summary><b>${p(e)}</b><small>${p(i)}</small><span class="cb-tool-chev"></span></summary>
    <div class="cb-insp-sec-body">
      <div class="cb-kv-grid">${o}</div>
      ${r.length?`<h4>Headers \xB7 ${r.length}</h4><div class="cb-kv-grid">${me(r)}</div>`:""}
      ${s}
      <h4>Raw body \xB7 ${B(d.length)} chars</h4><pre class="cb-code cb-raw">${p(D(d,8e4))||"(empty)"}</pre>
    </div>
  </details>`}function ke(t,e){let n=t.request?.trace_id??t.response?.trace_id??"",o=[t.request?fe(t.request,"Request"):"",t.response?fe(t.response,"Response"):""].join("");return`<header class="cb-insp-head"><div><strong>Call ${e+1} \xB7 request / response</strong><small>${p(String(n))}</small></div><button type="button" class="cb-action" data-cb-close-inspector title="Back to replay" aria-label="Close details">\u2715</button></header>
  <div class="cb-insp-body">${o||'<p class="cb-empty">No detail captured for this call.</p>'}</div>`}function ne(t,e){return`<script src="${t}"><\/script>
<cove-player src="${e}" theme="dark"></cove-player>`}function Se(t,e){let n=ne(t,e),o=t.replace(/[^/]*$/,"");return`<header class="cb-insp-head"><div><strong>Embed this replay</strong><small>self-contained \xB7 static assets only</small></div><button type="button" class="cb-action" data-cb-close-inspector title="Back to replay" aria-label="Close embed">\u2715</button></header>
  <div class="cb-insp-body">
    <p class="cb-embed-note">Paste this anywhere HTML runs. The player is a single script plus this recording's <code>.covecast</code> file \u2014 no server, no iframe.</p>
    <div class="cb-snippet">
      <button type="button" class="cb-copy" data-cb-copy aria-label="Copy embed code"><span class="cb-copy-glyph">\u29C9</span> Copy</button>
      <pre class="cb-code" data-cb-snippet>${p(n)}</pre>
    </div>
    <h4>Options</h4>
    <div class="cb-kv-grid">
      <div class="cb-kv"><span>theme</span><code>"dark" \xB7 "light" \xB7 "auto" (default)</code></div>
      <div class="cb-kv"><span>fill</span><code>add the attribute to fill the parent's height (dashboards, fullscreen pages)</code></div>
      <div class="cb-kv"><span>src</span><code>URL or path to any .covecast bundle \u2014 swap in another run's file and it just plays</code></div>
    </div>
    <h4>Files to host together</h4>
    <div class="cb-kv-grid">
      <div class="cb-kv"><span>player</span><code>${p(t)}</code></div>
      <div class="cb-kv"><span>terminal engine</span><code>${p(o)}vendor/asciinema-player.js + .css (loaded automatically from beside the player script)</code></div>
      <div class="cb-kv"><span>recording</span><code>${p(e)}</code></div>
    </div>
  </div>`}function De(t){if(t.querySelector(":scope > style[data-cove-player-style]"))return;let e=document.createElement("style");e.dataset.covePlayerStyle="",e.textContent=pe,t.prepend(e)}function Ve(t){return t instanceof Error?t:new Error(String(t))}var j=class extends HTMLElement{static version=Y;static observedAttributes=["src","theme"];_source=null;player=null;castUrl=null;timer=null;railSizer=null;abort=null;generation=0;get source(){return this._source}set source(e){this._source=e,this.isConnected&&e&&this.load(e)}connectedCallback(){if(this.abort)return;let e=this._source??this.getAttribute("src")??this.dataset.covePlayerSrc;e&&this.load(e)}disconnectedCallback(){this.cleanup()}attributeChangedCallback(e,n,o){!this.isConnected||n===o||(e==="src"&&o?(this._source=null,this.load(o)):e==="theme"&&this.setTheme(o||"auto"))}cleanup(){this.generation+=1,this.abort?.abort(),this.abort=null,this.cleanupPlayback()}cleanupPlayback(){this.timer!==null&&window.clearInterval(this.timer),this.timer=null,this.railSizer?.disconnect(),this.railSizer=null;try{this.player?.dispose?.()}catch{}this.player=null,this.castUrl&&URL.revokeObjectURL(this.castUrl),this.castUrl=null}setTheme(e){let n=this.querySelector(".cb");if(!n)return;let o=e||this.getAttribute("theme")||"auto",r=o==="auto"?window.matchMedia?.("(prefers-color-scheme: light)").matches?"light":"dark":o;n.dataset.theme=r==="light"?"light":"dark";let s=n.querySelector("[data-cb-theme]");s&&(s.textContent=n.dataset.theme==="light"?"\u263E":"\u2600",s.title=n.dataset.theme==="light"?"Use dark theme":"Use light theme",s.setAttribute("aria-label",s.title))}async load(e){let n=e??this._source??this.getAttribute("src")??this.dataset.covePlayerSrc;if(!n)return null;this.abort?.abort(),this.cleanupPlayback();let o=new AbortController;this.abort=o;let r=++this.generation;this.renderLoading();try{let s=await X(n,{signal:o.signal});return r!==this.generation||!this.isConnected||(this.emit("cove-bundle-load",{meta:s.manifest,bundle:s}),await this.renderBundle(s),r!==this.generation||!this.isConnected)?null:(this.querySelector(".cb")?.setAttribute("aria-busy","false"),s)}catch(s){if(o.signal.aborted||r!==this.generation)return null;let d=Ve(s),l=this.querySelector(".cb-shell");return l&&(l.innerHTML=`<div class="cb-empty cb-error">Could not load bundle: ${p(d.message)}</div>`),this.querySelector(".cb")?.setAttribute("aria-busy","false"),this.emit("cove-bundle-error",{error:d}),null}finally{this.abort===o&&(this.abort=null)}}renderLoading(){this.innerHTML='<div class="cb" aria-busy="true"><div class="cb-toolbar"><div class="cb-title"><strong>Cove replay</strong><span>Loading bundle\u2026</span></div><div class="cb-actions"><button type="button" class="cb-action" data-cb-theme aria-label="Toggle theme"></button><button type="button" class="cb-action" data-cb-fullscreen aria-label="Toggle fullscreen" title="Toggle fullscreen">\u26F6</button></div></div><div class="cb-stage"><section class="cb-shell"><div class="cb-empty">Loading replay bundle\u2026</div></section><aside class="cb-rail" aria-label="Trace timeline"></aside></div></div>',De(this),this.setTheme(),this.querySelector("[data-cb-theme]")?.addEventListener("click",()=>{let e=this.querySelector(".cb");this.setTheme(e?.dataset.theme==="light"?"dark":"light")}),this.querySelector("[data-cb-fullscreen]")?.addEventListener("click",()=>{let e=this.querySelector(".cb");e&&(document.fullscreenElement===e?document.exitFullscreen?.():e.requestFullscreen?.())})}async renderBundle(e){let{manifest:n,recording:o}=e,r=e.trace?.turns??[],s=this.requireElement(".cb"),d=this.requireElement(".cb-shell"),l=this.requireElement(".cb-rail"),i=n.run||{},m=this.requireElement(".cb-title strong");m.textContent=String(i.task||i.run_label||i.job||"Cove replay");let x=this.requireElement(".cb-title span");x.textContent=[i.harness,i.model,i.job].filter(Boolean).join(" \xB7 ");let h=ye(r);l.innerHTML=`${we(n,r,h.length)}<div class="cb-chat">${xe(h)||'<p class="cb-empty">No trace turns were captured.</p>'}</div>`;let f=a=>{let c=null;for(let g of this.querySelectorAll(".cb-turn[data-offset]"))Number(g.dataset.offset)<=a&&(c=g);for(let g of this.querySelectorAll(".cb-turn.current"))g!==c&&(g.classList.remove("current","awaiting"),g.removeAttribute("aria-current"));if(c){let g=Number(c.dataset.end);c.classList.toggle("awaiting",Number.isFinite(g)&&a<g),c.classList.contains("current")||(c.classList.add("current"),c.setAttribute("aria-current","step"),c.scrollIntoView({block:"nearest",behavior:"smooth"}))}};l.querySelectorAll(".cb-turn[data-offset] .cb-turn-head").forEach(a=>{a.addEventListener("click",()=>{let c=Number(a.closest(".cb-turn")?.dataset.offset);try{this.player?.seek(Math.max(0,c))}catch{}f(c)})});let u=document.createElement("aside");u.className="cb-inspector",u.hidden=!0,u.setAttribute("aria-label","Raw turn details");let w=[...l.querySelectorAll("[data-cb-mode]")],$=a=>{for(let c of w){let g=c.dataset.cbMode===a;c.classList.toggle("active",g),c.setAttribute("aria-selected",String(g))}},E=-1,L=()=>{u.hidden=!0,u.innerHTML="",l.querySelectorAll(".cb-turn.inspected").forEach(a=>a.classList.remove("inspected")),$("replay")},M=a=>{let c=h[a];c&&(E=a,l.querySelectorAll(".cb-turn.inspected").forEach(g=>g.classList.remove("inspected")),l.querySelector(`.cb-turn[data-turn="${a}"]`)?.classList.add("inspected"),u.innerHTML=ke(c,a),u.hidden=!1,u.scrollTop=0,u.querySelector("[data-cb-close-inspector]")?.addEventListener("click",L),$("trace"))},O=()=>{let a=String(this.getAttribute("src")??this.dataset.covePlayerSrc??"recording.covecast");try{a=new URL(a,document.baseURI).href}catch{}u.innerHTML=Se(Z("cove-player.iife.js"),a),u.hidden=!1,u.scrollTop=0,l.querySelectorAll(".cb-turn.inspected").forEach(g=>g.classList.remove("inspected")),u.querySelector("[data-cb-close-inspector]")?.addEventListener("click",L);let c=u.querySelector("[data-cb-copy]");c?.addEventListener("click",()=>{let g=ne(Z("cove-player.iife.js"),a),N=()=>{c.classList.add("copied"),c.innerHTML="\u2713 Copied",window.setTimeout(()=>{c.classList.remove("copied"),c.innerHTML='<span class="cb-copy-glyph">\u29C9</span> Copy'},1600)};navigator.clipboard?.writeText(g).then(N).catch(()=>{let C=document.createRange(),I=u.querySelector("[data-cb-snippet]");if(!I)return;C.selectNodeContents(I);let F=window.getSelection();F?.removeAllRanges(),F?.addRange(C)})}),$("embed")};for(let a of w)a.addEventListener("click",()=>{if(a.dataset.cbMode==="replay")L();else if(a.dataset.cbMode==="embed")O();else{let c=Number(l.querySelector(".cb-turn.current")?.dataset.turn);M(E>=0?E:Number.isFinite(c)?c:0)}});if(l.querySelectorAll(".cb-inspect").forEach(a=>{a.addEventListener("click",()=>{let c=Number(a.closest(".cb-turn")?.dataset.turn);Number.isFinite(c)&&(c===E&&!u.hidden?L():M(c))})}),l.querySelectorAll(".cb-turn[data-offset] .cb-turn-head").forEach(a=>{a.addEventListener("click",()=>{if(u.hidden)return;let c=Number(a.closest(".cb-turn")?.dataset.turn);Number.isFinite(c)&&M(c)})}),l.querySelectorAll(".cb-res-head[data-res-offset]").forEach(a=>{a.addEventListener("click",()=>{let c=Number(a.dataset.resOffset);try{this.player?.seek(Math.max(0,c))}catch{}if(f(c),!u.hidden){let g=Number(a.closest(".cb-turn")?.dataset.turn);Number.isFinite(g)&&M(g)}})}),!o){s.classList.add("cb-no-cast"),d.innerHTML=`<div class="cb-empty">${e.trace?"Trace-only replay \xB7 this run has no terminal recording.":"This replay has no terminal recording or trace."}</div>`,d.append(u),this.emitReady(e,!1);return}d.innerHTML='<div class="cb-timeline" aria-label="Replay markers"></div><div class="cb-terminal"></div>',d.append(u);let S=this.requireElement(".cb-timeline"),_=(n.markers||[]).filter(a=>Array.isArray(a)&&Number.isFinite(Number(a[0]))),k=Math.max(1,..._.map(a=>Number(a[0])),...r.map(a=>Number(a.offset_seconds)||0)),H=new Set,V=_.filter(a=>{let c=Math.round(Number(a[0])/k*160);return H.has(c)?!1:(H.add(c),!0)});for(let a of V){let c=document.createElement("button");c.type="button",c.className="cb-tick",c.style.left=`${Math.max(0,Math.min(100,Number(a[0])/k*100))}%`,c.title=`${q(a[0])}${a[1]?` \xB7 ${a[1]}`:""}`,c.setAttribute("aria-label",`Seek to ${c.title}`),c.addEventListener("click",()=>this.player?.seek(Math.max(0,Number(a[0])))),S.append(c)}if(this.castUrl=URL.createObjectURL(new Blob([new Uint8Array(o).buffer],{type:"application/x-asciicast"})),await se(),!this.isConnected||!this.castUrl)return;let v=window.AsciinemaPlayer;if(!v)throw new Error("terminal player did not register");let P=this.hasAttribute("video-capture"),T=Number(this.getAttribute("playback-speed")??2),W=Number.isFinite(T)&&T>=.25&&T<=16?T:2;if(this.player=v.create(this.castUrl,this.requireElement(".cb-terminal"),{speed:W,fit:P?"height":this.hasAttribute("fill")?"both":"width",theme:s.dataset.theme==="light"?"asciinema":"dracula",preload:!0,markers:_}),this.emit("cove-bundle-player",{player:this.player,meta:n}),this.hasAttribute("fill")){let a=this.requireElement(".cb-terminal"),c=this.requireElement(".cb-stage"),g=()=>{let C=a.querySelector(".ap-player"),I=C?.getBoundingClientRect().height??0,F=I>60?Math.ceil(I+S.offsetHeight):0;if(l.style.maxHeight=F?`${F}px`:"",P&&C&&window.matchMedia("(min-width: 860px)").matches){let Te=a.clientWidth,oe=C.offsetWidth,ie=oe>0?Math.min(1,Te/oe):1;C.style.transformOrigin="top left",C.style.transform=ie<.999?`scaleX(${ie})`:"",l.style.height=`${c.clientHeight}px`,l.style.maxHeight=`${c.clientHeight}px`}};this.railSizer=new ResizeObserver(g),this.railSizer.observe(a);let N=a.querySelector(".ap-player");N&&this.railSizer.observe(N),g()}this.emitReady(e,!0),this.timer=window.setInterval(()=>{Promise.resolve(this.player?.getCurrentTime?.()).then(a=>f(Number(a)||0)).catch(()=>{})},500)}emitReady(e,n){this.emit("cove-bundle-ready",{meta:e.manifest,bundle:e,hasRecording:n})}emit(e,n){this.dispatchEvent(new CustomEvent(e,{bubbles:!0,detail:n}))}requireElement(e){let n=this.querySelector(e);if(!n)throw new Error(`player view is missing ${e}`);return n}};function re(t="cove-player"){return customElements.get(t)||customElements.define(t,j),j}function K(t=document){t.querySelectorAll("[data-cove-player-src]").forEach(e=>{if(e.tagName==="COVE-PLAYER")return;let n=document.createElement("cove-player");n.setAttribute("src",e.dataset.covePlayerSrc||""),e.replaceChildren(n)})}window.__coveBundlePlayer||(window.__coveBundlePlayer=!0,ae(document.currentScript?.src||document.baseURI),re(),window.CoveBundlePlayer=Object.freeze({version:Y,CovePlayer:j,loadCovecast:X,register:re,hydrate:K}),document.readyState==="loading"?document.addEventListener("DOMContentLoaded",()=>K(),{once:!0}):K());})();
