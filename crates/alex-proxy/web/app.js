/* 1. State, escaping, formatting, API, and toast helpers. */
const TURN_PAGE_SIZE = 20;
const TRACE_PAGE_SIZE = 25;
const TRACE_COLUMN_STORAGE_KEY = "alex.web.trace-columns";
const TRACE_COLUMN_DEFAULTS = Object.freeze({
  left: 420,
  right: 350,
  visible: ["turns", "cost", "duration", "status", "time"],
  showSubagents: true,
});
function readTraceColumnPreferences() {
  try {
    const value = JSON.parse(localStorage.getItem(TRACE_COLUMN_STORAGE_KEY) || "{}");
    const bounded = (candidate, minimum, maximum, fallback) => Number.isFinite(Number(candidate)) ? Math.min(maximum, Math.max(minimum, Number(candidate))) : fallback;
    return {
      left: bounded(value.left, 200, 560, TRACE_COLUMN_DEFAULTS.left),
      right: bounded(value.right, 280, 720, TRACE_COLUMN_DEFAULTS.right),
      visible: Array.isArray(value.visible) ? value.visible.filter((column) => TRACE_COLUMN_DEFAULTS.visible.includes(column)) : [...TRACE_COLUMN_DEFAULTS.visible],
      showSubagents: value.showSubagents !== false,
    };
  } catch {
    return { ...TRACE_COLUMN_DEFAULTS, visible: [...TRACE_COLUMN_DEFAULTS.visible] };
  }
}
const ONBOARDING_STEP_TITLES = [
  "Web access", "Meet Alex", "Pick a provider", "Connect and test",
  "Credentials for compatible apps", "Never lose a login", "Keep your agents running",
  "Beyond single provider",
];
const REFRESH_STORAGE_KEY = "alex.web.refresh-seconds";
const ALEX_RELEASES_URL = "https://github.com/madhavajay/alex/releases";
const PROVIDERS = [
  ["claude", "Anthropic", "oauth"],
  ["codex", "OpenAI", "oauth"],
  ["gemini", "Gemini", "oauth"],
  ["grok", "xAI", "oauth"],
  ["kimi", "Kimi", "oauth"],
  ["amp", "Amp", "import"],
  ["openrouter", "OpenRouter", "form"],
  ["exo", "Exo", "form"],
  ["cliproxyapi", "CLIProxyAPI", "form"],
];
const PROVIDER_LOGOS = Object.freeze({
  claude: "claude-code.png",
  anthropic: "claude-code.png",
  codex: "codex.png",
  openai: "codex.png",
  gemini: "gemini-cli.png",
  google: "gemini-cli.png",
  kimi: "kimi-code.png",
  moonshot: "kimi-code.png",
  grok: "grok-build.png",
  xai: "grok-build.png",
  amp: "amp-code.svg",
  openrouter: "openrouter.png",
  exo: "exo.png",
});
const HARNESS_LOGOS = Object.freeze({
  pi: "pi.svg",
  omp: "oh-my-pi.png",
  "oh-my-pi": "oh-my-pi.png",
  claude: "claude-code.png",
  "claude-code": "claude-code.png",
  codex: "codex.png",
  gemini: "gemini-cli.png",
  kimi: "kimi-code.png",
  cursor: "cursor-cli.png",
  "cursor-cli": "cursor-cli.png",
  amp: "amp-code.svg",
  droid: "droid-cli.svg",
  grok: "grok-build.png",
  opencode: "opencode.png",
  "mini-swe-agent": "mini-swe-agent.png",
  qwen: "qwen-code.png",
  "qwen-code": "qwen-code.png",
  goose: "goose.jpg",
  opensage: "opensage-adk.png",
  "opensage-adk": "opensage-adk.png",
  "pydantic-ai": "pydantic-ai-harness.png",
  jcode: "jcode.png",
  hermes: "hermes.png",
});
const HARNESS_ORDER = Object.freeze([
  "pi", "claude", "codex", "grok", "amp", "gemini", "opencode",
  "kimi", "cursor", "droid", "qwen", "goose", "omp", "mini-swe-agent",
  "opensage", "pydantic-ai", "stirrup", "jcode", "hermes",
]);
const LOGIN_PROVIDERS = {
  claude: ["Claude Code", "Anthropic", "A", "claude"],
  anthropic: ["Claude Code", "Anthropic", "A", "claude"],
  codex: ["Codex", "OpenAI", "O", "codex"],
  openai: ["Codex", "OpenAI", "O", "codex"],
  gemini: ["Gemini", "Google", "G", "gemini"],
  google: ["Gemini", "Google", "G", "gemini"],
  grok: ["Grok", "xAI", "X", "grok"],
  xai: ["Grok", "xAI", "X", "grok"],
  kimi: ["Kimi", "Moonshot AI", "K", "kimi"],
  moonshot: ["Kimi", "Moonshot AI", "K", "kimi"],
};
const VIEW_COPY = {
  onboarding: ["Onboarding", "Set up Alex in a few small steps"],
  dashboard: ["Dashboard", "Daemon, providers, tools, and recent activity"],
  traces: ["Trace Browser", "Body-safe request inspection"],
  general: ["General", "Daemon, storage, and web access"],
  updates: ["Updates", "Versions, release channel, and safe installation"],
  providers: ["Providers", "Accounts, usage, quotas, and routing"],
  harnesses: ["Harnesses", "Connect and configure coding tools"],
  credentials: ["Credentials", "Scoped access and outbound credential status"],
  dario: ["Dario", "Claude subscription runtime and prompt caches"],
  middleware: ["Middleware", "Rules, protection, activity, and leases"],
  notifications: ["Notifications", "Telegram alerts and daemon messages"],
};
const CHART_COLORS = ["#0a84ff", "#30d158", "#ff9f0a", "#bf5af2", "#64d2ff", "#ff453a"];
const traceColumnPreferences = readTraceColumnPreferences();
const state = {
  adminKey: null,
  sessionAuthenticated: false,
  auth: null,
  onboardingStep: 0,
  currentView: "onboarding",
  refreshTimer: null,
  refreshSeconds: 60,
  health: null,
  accounts: [],
  providers: [],
  harnesses: [],
  providerTab: null,
  harnessTab: null,
  providerAnalytics: null,
  providerRoutings: [],
  analytics: null,
  limits: null,
  dario: null,
  update: null,
  updateCheckedAt: null,
  updateInstalling: false,
  updateOutcome: null,
  middleware: null,
  fixtures: [],
  cliproxyapi: null,
  exo: null,
  openrouter: null,
  traceCursor: null,
  traceFilters: {},
  traceRows: [],
  traceSessions: [],
  traceSort: { key: "time", direction: "desc" },
  traceVisibleColumns: new Set(traceColumnPreferences.visible),
  traceShowSubagents: traceColumnPreferences.showSubagents,
  traceExpandedSessions: new Set(),
  traceColumnWidths: { left: traceColumnPreferences.left, right: traceColumnPreferences.right },
  selectedTraceId: null,
  selectedSessionId: null,
  traceDetailRequest: null,
  sessionMenu: null,
  sessionFixturesLoaded: false,
  loginPoll: null,
  networkFailures: 0,
  sessionRecovery: false,
  importCandidates: [],
  selectedProvider: null,
  selectedProviderAccount: null,
  selectedImports: new Set(),
  selectedHarness: null,
  harnessPlan: [],
  harnessPlanStatus: "idle",
  harnessStatus: "idle",
  harnessSummary: null,
  harnessTraceStartedMs: null,
  harnessTrace: null,
  harnessTraceStatus: "idle",
  harnessTracePoll: null,
  manualHarnessResult: null,
};

const $ = (selector, root = document) => root.querySelector(selector);
const $$ = (selector, root = document) => [...root.querySelectorAll(selector)];
const escapeHtml = (value) => String(value ?? "").replace(/[&<>"']/g, (character) => ({
  "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;",
})[character]);
const stableHueClass = (value) => {
  let hash = 0;
  for (const character of String(value || "?")) hash = ((hash << 5) - hash + character.charCodeAt(0)) | 0;
  return `brand-hue-${Math.abs(hash) % 8}`;
};
const logoTile = (logos, name, label, className) => {
  const key = String(name || "").toLowerCase();
  const file = logos[key];
  if (file) return `<span class="${escapeHtml(className)} brand-logo-tile" aria-hidden="true"><img src="/ui/assets/${escapeHtml(file)}" alt=""></span>`;
  const monogram = key === "cliproxyapi" ? "⚙" : String(label || name || "?").trim().charAt(0).toUpperCase() || "?";
  return `<span class="${escapeHtml(className)} brand-monogram-tile ${stableHueClass(key)}" aria-hidden="true">${escapeHtml(monogram)}</span>`;
};
const providerLogoTile = (name, label, className = "provider-monogram") => logoTile(PROVIDER_LOGOS, name, label, className);
const harnessLogoTile = (name, label, className = "harness-logo") => logoTile(HARNESS_LOGOS, name, label, className);
const providerDisplayName = (name) => ({
  anthropic: "Anthropic", claude: "Anthropic", openai: "OpenAI", codex: "OpenAI",
  google: "Google", gemini: "Google", xai: "xAI", grok: "xAI",
  moonshot: "Moonshot AI", kimi: "Moonshot AI", openrouter: "OpenRouter",
  exo: "Exo", amp: "Amp",
})[String(name || "").toLowerCase()] || String(name || "Unrouted");
const display = (value) => value === null || value === undefined || value === "" ? "—" : String(value);
const parseList = (value) => {
  if (Array.isArray(value)) return value;
  if (typeof value !== "string") return [];
  try { return JSON.parse(value); } catch { return []; }
};
const finite = (value, fallback = 0) => Number.isFinite(Number(value)) ? Number(value) : fallback;
const clamp = (value, minimum, maximum) => Math.min(maximum, Math.max(minimum, finite(value)));
const formatNumber = (value) => new Intl.NumberFormat(undefined, { maximumFractionDigits: 1 }).format(finite(value));
const formatInteger = (value) => new Intl.NumberFormat().format(Math.round(finite(value)));
const formatMoney = (value) => `$${finite(value).toFixed(finite(value) < 1 ? 4 : 2)}`;
const formatTime = (value) => value ? new Date(finite(value)).toLocaleString() : "—";
const formatAge = (seconds) => {
  const value = Math.max(0, finite(seconds));
  if (value < 60) return `${Math.round(value)}s`;
  if (value < 3600) return `${Math.round(value / 60)}m`;
  if (value < 86400) return `${Math.round(value / 3600)}h`;
  return `${Math.round(value / 86400)}d`;
};
const safeUrl = (value) => {
  try {
    const parsed = new URL(String(value), location.origin);
    return ["http:", "https:"].includes(parsed.protocol) ? parsed.href : "";
  } catch { return ""; }
};
const errorMessage = (payload, response) => {
  const error = payload?.error;
  return (typeof error === "string" ? error : error?.message) || payload?.message || response.statusText || `HTTP ${response.status}`;
};

async function api(path, options = {}) {
  const { suppressSessionRecovery = false, ...fetchOptions } = options;
  const headers = new Headers(options.headers || {});
  if (state.adminKey && !state.sessionAuthenticated) headers.set("x-api-key", state.adminKey);
  if (options.body && !headers.has("content-type")) headers.set("content-type", "application/json");
  let response;
  try {
    response = await fetch(path, { credentials: "same-origin", ...fetchOptions, headers });
    state.networkFailures = 0;
  } catch {
    state.networkFailures += 1;
    if (state.sessionAuthenticated && state.networkFailures >= 2 && !suppressSessionRecovery && !state.updateInstalling) endExpiredSession();
    throw new Error(state.updateInstalling || suppressSessionRecovery
      ? "The daemon is restarting and is temporarily unreachable."
      : (state.networkFailures >= 2 ? sessionEndedMessage() : "The daemon is temporarily unreachable. Retrying may help."));
  }
  const payload = response.status === 204 ? null : await response.json().catch(() => null);
  if (response.status === 401 && state.sessionAuthenticated && !suppressSessionRecovery && !state.updateInstalling) endExpiredSession();
  if (!response.ok) {
    const error = new Error(errorMessage(payload, response));
    error.status = response.status;
    error.payload = payload;
    throw error;
  }
  return payload;
}

async function apiText(path, options = {}) {
  const { suppressSessionRecovery = false, ...fetchOptions } = options;
  const headers = new Headers(options.headers || {});
  if (state.adminKey && !state.sessionAuthenticated) headers.set("x-api-key", state.adminKey);
  let response;
  try {
    response = await fetch(path, { credentials: "same-origin", ...fetchOptions, headers });
    state.networkFailures = 0;
  } catch {
    state.networkFailures += 1;
    if (state.sessionAuthenticated && state.networkFailures >= 2 && !suppressSessionRecovery && !state.updateInstalling) endExpiredSession();
    throw new Error(state.updateInstalling || suppressSessionRecovery
      ? "The daemon is restarting and is temporarily unreachable."
      : (state.networkFailures >= 2 ? sessionEndedMessage() : "The daemon is temporarily unreachable. Retrying may help."));
  }
  const text = await response.text();
  if (response.status === 401 && state.sessionAuthenticated && !suppressSessionRecovery && !state.updateInstalling) endExpiredSession();
  if (!response.ok) {
    let payload = null;
    try { payload = JSON.parse(text); } catch { /* plain-text response */ }
    throw new Error(errorMessage(payload, response));
  }
  return text;
}

function sessionEndedMessage() {
  return "Session ended — the daemon restarted or your session expired; sign in again.";
}

function endExpiredSession() {
  if (state.sessionRecovery) return;
  state.sessionRecovery = true;
  if (state.currentView && location.hash !== `#${state.currentView}`) history.replaceState(null, "", `#${state.currentView}`);
  state.sessionAuthenticated = false;
  state.adminKey = null;
  clearInterval(state.refreshTimer);
  clearTimeout(state.loginPoll);
  stopHarnessTracePolling();
  showAuthState("password-login", sessionEndedMessage());
}

let toastTimer = null;
function toast(message, kind = "success") {
  const node = $("#toast");
  node.textContent = String(message);
  node.className = `toast ${kind}`;
  node.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => { node.hidden = true; }, 4500);
}

function inlineMessage(node, message, kind = "danger") {
  node.textContent = String(message);
  node.className = `inline-message ${kind}`;
  node.hidden = !message;
}

function renderError(node, error, label = "Could not load this section") {
  node.innerHTML = `<div class="inline-message danger"><strong>${escapeHtml(label)}</strong><p>${escapeHtml(error.message)}</p></div>`;
}

function facts(items) {
  return `<dl class="facts">${items.map(([label, value]) => `<div><dt>${escapeHtml(label)}</dt><dd>${escapeHtml(display(value))}</dd></div>`).join("")}</dl>`;
}

function statCards(items) {
  return items.map(([label, value, note]) => `<div class="stat-card"><span>${escapeHtml(label)}</span><strong>${escapeHtml(display(value))}</strong>${note ? `<small>${escapeHtml(note)}</small>` : ""}</div>`).join("");
}

function bindActions(root, selector, handler) {
  $$(selector, root).forEach((node) => node.addEventListener("click", handler));
}

/* 2. Auth, bootstrap, password, and onboarding state. */
function showAuthState(id, message = "") {
  ["auth-loading", "password-login", "key-bootstrap"].forEach((name) => { $(`#${name}`).hidden = name !== id; });
  $("#auth-screen").hidden = false;
  $("#app-shell").hidden = true;
  inlineMessage($("#auth-error"), message);
  $(`#${id} input`)?.focus();
}

async function bootstrap() {
  showAuthState("auth-loading");
  try {
    const auth = await api("/web/auth/status");
    state.auth = auth;
    state.sessionAuthenticated = Boolean(auth.authenticated);
    if (auth.authenticated) {
      enterApplication();
      return;
    }
    if (auth.password_configured) {
      showAuthState("password-login");
      return;
    }
    const response = await fetch("/connect", { credentials: "same-origin" });
    if (response.status === 403) {
      showAuthState("key-bootstrap");
      return;
    }
    const connect = await response.json().catch(() => null);
    if (!response.ok || !connect?.api_key) throw new Error(errorMessage(connect, response));
    state.adminKey = String(connect.api_key);
    state.auth.authenticated = true;
    enterApplication();
  } catch (error) {
    showAuthState("auth-loading", error.message);
  }
}

async function submitPasswordLogin(event) {
  event.preventDefault();
  const form = event.currentTarget;
  const button = $("button", form);
  button.disabled = true;
  inlineMessage($("#auth-error"), "");
  try {
    await api("/web/auth/login", { method: "POST", body: JSON.stringify({ password: new FormData(form).get("password") }) });
    form.reset();
    state.sessionAuthenticated = true;
    state.adminKey = null;
    state.auth = await api("/web/auth/status");
    enterApplication();
  } catch (error) {
    inlineMessage($("#auth-error"), error.message);
  } finally { button.disabled = false; }
}

async function submitAdminKey(event) {
  event.preventDefault();
  const form = event.currentTarget;
  const button = $("button", form);
  button.disabled = true;
  inlineMessage($("#auth-error"), "");
  state.adminKey = String(new FormData(form).get("key") || "").trim();
  try {
    await api("/admin/accounts");
    form.reset();
    state.auth.authenticated = true;
    enterApplication();
  } catch (error) {
    state.adminKey = null;
    inlineMessage($("#auth-error"), error.message);
  } finally { button.disabled = false; }
}

function enterApplication() {
  state.sessionRecovery = false;
  state.networkFailures = 0;
  $("#auth-screen").hidden = true;
  $("#app-shell").hidden = false;
  configureRefreshInterval();
  updatePasswordStep();
  const sharedHarnessFilter = new URLSearchParams(location.search).get("harness");
  if (sharedHarnessFilter) {
    state.traceFilters.harness = sharedHarnessFilter;
    $("#trace-filters").elements.harness.value = sharedHarnessFilter;
  }
  const requested = location.hash.slice(1);
  const requestedView = requested.split("/", 1)[0];
  selectView(state.auth?.onboarding_completed ? (VIEW_COPY[requestedView] ? requested : "dashboard") : "onboarding", false);
  refreshSharedData().catch((error) => toast(error.message, "danger"));
}

function updatePasswordStep() {
  const configured = Boolean(state.auth?.password_configured);
  $("#password-configured").hidden = !configured;
  $("#password-config-form").hidden = configured;
}

async function submitWebPassword(event, onboarding = false) {
  event.preventDefault();
  const form = event.currentTarget;
  const data = new FormData(form);
  const password = String(data.get("password") || "");
  if (password !== String(data.get("confirmation") || "")) {
    toast("The password confirmation does not match.", "danger");
    return;
  }
  const button = $("button[type='submit'], button:not([type])", form);
  button.disabled = true;
  try {
    await api("/admin/web/password", { method: "POST", body: JSON.stringify({ password }) });
    state.sessionAuthenticated = true;
    state.adminKey = null;
    state.auth.password_configured = true;
    form.reset();
    updatePasswordStep();
    toast(onboarding ? "Dashboard password created." : "Password replaced; other sessions were signed out.");
  } catch (error) { toast(error.message, "danger"); }
  finally { button.disabled = false; }
}

function showOnboardingStep(step) {
  const previous = state.onboardingStep;
  state.onboardingStep = clamp(step, 0, ONBOARDING_STEP_TITLES.length - 1);
  $$('[data-onboarding-step]').forEach((node) => { node.hidden = Number(node.dataset.onboardingStep) !== state.onboardingStep; });
  $("#onboarding-title").textContent = ONBOARDING_STEP_TITLES[state.onboardingStep];
  $("#onboarding-count").textContent = `${state.onboardingStep + 1} of ${ONBOARDING_STEP_TITLES.length}`;
  $("#onboarding-back").disabled = state.onboardingStep === 0;
  $("#onboarding-next").textContent = state.onboardingStep === ONBOARDING_STEP_TITLES.length - 1 ? "Get started" : "Next";
  $("#onboarding-dots").innerHTML = ONBOARDING_STEP_TITLES.map((_, index) => `<i class="${index === state.onboardingStep ? "active" : ""}"></i>`).join("");
  inlineMessage($("#onboarding-error"), "");
  if (previous === 3 && state.onboardingStep !== 3) stopHarnessTracePolling();
  if (state.onboardingStep === 2) {
    renderOnboardingProviders();
    loadOnboardingImports();
  }
  if (state.onboardingStep === 3) {
    renderOnboardingHarnessStages();
    if (state.harnessStatus === "success" && !state.harnessTrace) beginHarnessTracePolling(false);
  }
  $("#onboarding-view").closest(".content")?.scrollTo({ top: 0, behavior: "smooth" });
}

function renderOnboardingData() {
  if (state.onboardingStep === 2) renderOnboardingProviders();
  if (state.onboardingStep === 3) renderOnboardingHarnessStages();
}

async function finishOnboarding() {
  const button = $("#onboarding-next");
  button.disabled = true;
  inlineMessage($("#onboarding-error"), "");
  try {
    await api("/admin/web/onboarding", { method: "POST", body: JSON.stringify({ completed: true }) });
    state.auth.onboarding_completed = true;
    toast("Onboarding complete.");
    selectView("dashboard", true);
  } catch (error) { inlineMessage($("#onboarding-error"), error.message); }
  finally { button.disabled = false; }
}

function nextOnboardingStep() {
  if (state.onboardingStep === ONBOARDING_STEP_TITLES.length - 1) finishOnboarding();
  else showOnboardingStep(state.onboardingStep + 1);
}

function skipOnboardingStep() {
  clearTimeout(state.loginPoll);
  if (state.onboardingStep === 3) stopHarnessTracePolling();
  nextOnboardingStep();
}

async function restartOnboarding() {
  try {
    await api("/admin/web/onboarding", { method: "POST", body: JSON.stringify({ completed: false }) });
    state.auth.onboarding_completed = false;
    resetOnboardingFlow();
    showOnboardingStep(0);
    selectView("onboarding", true);
  } catch (error) { toast(error.message, "danger"); }
}

function resetOnboardingFlow() {
  stopHarnessTracePolling();
  state.onboardingStep = 0;
  state.selectedProvider = null;
  state.selectedProviderAccount = null;
  state.selectedHarness = null;
  state.harnessPlan = [];
  state.harnessPlanStatus = "idle";
  state.harnessStatus = "idle";
  state.harnessSummary = null;
  state.harnessTraceStartedMs = null;
  state.harnessTrace = null;
  state.harnessTraceStatus = "idle";
  state.manualHarnessResult = null;
}

function onboardingProviderDefinition(id) {
  return PROVIDERS.find(([provider]) => provider === id);
}

function providerAccounts(id) {
  const canonical = providerCanonical(id);
  return state.accounts.filter((account) => providerCanonical(account.provider) === canonical);
}

function providerCandidates(id) {
  const canonical = providerCanonical(id);
  return state.importCandidates.filter((candidate) => providerCanonical(candidate.provider) === canonical);
}

function renderOnboardingProviders() {
  const grid = $("#onboarding-providers");
  if (!grid) return;
  grid.innerHTML = PROVIDERS.map(([id, label]) => {
    const connected = providerAccounts(id).length;
    const detected = providerCandidates(id).length;
    const selected = state.selectedProvider === id;
    const status = selected ? "Selected" : connected ? `${connected} connected` : detected ? "Detected login" : "Connect";
    return `<button class="onboarding-choice ${selected ? "selected" : ""}" data-onboarding-provider="${escapeHtml(id)}">${providerLogoTile(id, label)}<span><strong>${escapeHtml(label)}</strong><small>${escapeHtml(status)}</small></span>${selected ? '<b aria-hidden="true">✓</b>' : ""}</button>`;
  }).join("");
  bindActions(grid, "[data-onboarding-provider]", (event) => {
    clearTimeout(state.loginPoll);
    state.selectedProvider = event.currentTarget.dataset.onboardingProvider;
    state.selectedProviderAccount = null;
    loginFlowNodes().forEach((node) => { node.hidden = true; node.replaceChildren(); });
    renderOnboardingProviders();
    renderOnboardingProviderDetail();
  });
  renderOnboardingProviderDetail();
}

function renderOnboardingProviderDetail(message = "", kind = "success") {
  const node = $("#onboarding-provider-detail");
  const id = state.selectedProvider;
  if (!node || !id) { if (node) node.replaceChildren(); return; }
  const definition = onboardingProviderDefinition(id);
  const label = definition?.[1] || id;
  const mode = definition?.[2] || "oauth";
  const accounts = providerAccounts(id);
  const candidates = providerCandidates(id);
  const accountRows = accounts.map((account) => `<button class="provider-account-choice ${state.selectedProviderAccount === account.id ? "selected" : ""}" data-use-provider-account="${escapeHtml(account.id)}"><span class="status-dot ${escapeHtml(account.health || account.status || "unknown")}"></span><span><strong>${escapeHtml(accountTitle(account))}</strong><small>${escapeHtml(account.kind === "oauth" ? "Connected subscription" : account.kind)}</small></span><b>${state.selectedProviderAccount === account.id ? "✓" : "Use"}</b></button>`).join("");
  const candidateRows = candidates.map((candidate) => `<label class="import-choice"><input class="square-check" type="checkbox" value="${escapeHtml(candidate.source)}" ${state.selectedImports.has(candidate.source) ? "checked" : ""}><span><strong>${escapeHtml(candidate.label)}</strong><small>${escapeHtml(`${candidate.kind.replaceAll("_", " ")} · ${candidate.source_path}`)}</small></span></label>`).join("");
  const freshAction = mode === "oauth" ? `Add another ${label} account` : mode === "import" ? "Import another detected credential" : `Configure another ${label} account`;
  node.innerHTML = `<div class="onboarding-provider-card"><h3>${accounts.length ? "Choose an existing account or add a new one" : candidates.length ? "Existing login detected" : `Connect ${escapeHtml(label)}`}</h3>${accountRows}${candidates.length ? `<div class="detected-heading"><strong>Detected outside Alex</strong><small>These remain owned by their original apps. Alex imports only the credentials you leave checked.</small></div>${candidateRows}<button data-import-checked>Import checked credential</button>` : ""}<button data-add-provider-account>${escapeHtml(freshAction)}</button>${message ? `<p class="inline-message ${escapeHtml(kind)}">${escapeHtml(message)}</p>` : ""}</div>`;
  bindActions(node, "[data-use-provider-account]", (event) => {
    state.selectedProviderAccount = event.currentTarget.dataset.useProviderAccount;
    renderOnboardingProviderDetail(`${label} account selected.`, "success");
    renderOnboardingProviders();
  });
  $$(".square-check", node).forEach((input) => input.addEventListener("change", () => {
    if (input.checked) state.selectedImports.add(input.value); else state.selectedImports.delete(input.value);
  }));
  $("[data-import-checked]", node)?.addEventListener("click", importOnboardingCredentials);
  $("[data-add-provider-account]", node)?.addEventListener("click", () => {
    if (mode === "oauth") startLogin(id);
    else if (mode === "import") loadOnboardingImports(true);
    else { selectView(`providers/${id}`, true); $(`#${id}-form`)?.scrollIntoView({ behavior: "smooth" }); }
  });
}

async function loadOnboardingImports(force = false) {
  if (state.importCandidates.length && !force) return;
  try {
    const data = await api("/admin/auth/import-candidates");
    state.importCandidates = data.candidates || [];
    state.importCandidates.forEach((candidate) => state.selectedImports.add(candidate.source));
    renderOnboardingProviders();
  } catch (error) {
    renderOnboardingProviderDetail(`Could not scan external credentials: ${error.message}`, "warning");
  }
}

async function importOnboardingCredentials(event) {
  const button = event.currentTarget;
  const selected = providerCandidates(state.selectedProvider).filter((candidate) => state.selectedImports.has(candidate.source));
  if (!selected.length) { renderOnboardingProviderDetail("Select a detected credential to import, or connect a new account.", "danger"); return; }
  button.disabled = true;
  try {
    let imported = [];
    let note = "";
    for (const candidate of selected) {
      const result = await api("/admin/auth/import", { method: "POST", body: JSON.stringify({ source: candidate.source }) });
      imported = imported.concat(...(result.outcomes || []).map((outcome) => outcome.imported || []));
      note ||= (result.outcomes || []).find((outcome) => outcome.note)?.note || "";
    }
    await loadAccounts();
    const account = imported.map((id) => state.accounts.find((item) => item.id === id)).find(Boolean) || providerAccounts(state.selectedProvider).at(-1);
    state.selectedProviderAccount = account?.id || null;
    renderOnboardingProviders();
    renderOnboardingProviderDetail(account ? `${accountTitle(account)} imported and selected.` : (note || "Import completed."), account ? "success" : "warning");
  } catch (error) { renderOnboardingProviderDetail(error.message, "danger"); }
  finally { button.disabled = false; }
}

function harnessDisplayName(name) {
  return ({ pi: "Pi", codex: "Codex", claude: "Claude Code", grok: "Grok", amp: "Amp", kimi: "Kimi" })[name] || String(name || "Harness").replace(/(^|-)([a-z])/g, (_, separator, letter) => `${separator}${letter.toUpperCase()}`);
}

function onboardingModel() {
  return ({ claude: "alex/claude-haiku-4-5", anthropic: "alex/claude-haiku-4-5", codex: "alex/gpt-5.6-sol", openai: "alex/gpt-5.6-sol", gemini: "alex/gemini-2.5-flash", grok: "alex/grok-code-fast-1", xai: "alex/grok-code-fast-1", kimi: "alex/kimi/k3" })[state.selectedProvider] || "alex/gpt-5.6-sol";
}

function harnessTestCommand(name) {
  const model = onboardingModel();
  return ({
    claude: `claude --settings ~/.claude/alex-settings.json -p "test" --model ${model}`,
    kimi: `kimi -m ${model} -p "test"`,
    codex: `codex --profile alex exec --skip-git-repo-check -m ${model} "test"`,
    pi: `pi --model ${model} -p "test"`,
    amp: 'alex wrap amp -- -x "test"',
  })[name] || `${name} -m ${model} -p "test"`;
}

function operationMarkup(status, message) {
  if (status === "idle") return "";
  const icon = status === "working" ? '<span class="spinner"></span>' : status === "success" ? "✓" : "×";
  return `<div class="onboarding-operation ${escapeHtml(status)}"><b>${icon}</b><span>${escapeHtml(message)}</span></div>`;
}

function stageHeader(number, title, completed, summary = "", action = "") {
  return `<div class="stage-head"><span class="stage-number ${completed ? "complete" : ""}">${completed ? "✓" : number}</span><strong>${escapeHtml(title)}</strong>${completed ? `<small>${escapeHtml(summary)}</small>${action ? `<button data-stage-action="${escapeHtml(action)}">Change harness</button>` : ""}` : ""}</div>`;
}

function renderHarnessPlan() {
  if (state.harnessPlanStatus === "working") return operationMarkup("working", "Previewing changes…");
  if (state.harnessPlanStatus === "failure") return operationMarkup("failure", state.harnessPlanMessage || "Could not preview changes.");
  if (state.harnessPlanStatus !== "success") return "";
  const about = state.harnessPlan.find((item) => item.action === "about")?.detail;
  const changes = state.harnessPlan.filter((item) => item.action !== "about");
  return `<div class="files-changed"><span>FILES CHANGED</span>${about ? `<p class="about-change"><b>ABOUT</b>${escapeHtml(about)}</p>` : ""}${changes.length ? changes.map((item) => `<div class="file-change"><b class="${escapeHtml(item.action)}">${escapeHtml(String(item.action || "change").toUpperCase())}</b><code>${escapeHtml(item.path)}</code><small>${escapeHtml(item.detail)}</small></div>`).join("") : '<p class="muted">No file changes are needed; Connect will refresh the harness model list.</p>'}<button class="primary" data-connect-onboarding-harness>Connect</button>${operationMarkup(state.harnessStatus, state.harnessStatus === "working" ? `Connecting ${harnessDisplayName(state.selectedHarness)}…` : state.harnessStatus === "failure" ? (state.harnessStatusMessage || "Connection failed.") : "")}</div>`;
}

function traceStatusMessage() {
  if (state.harnessTraceStatus === "working") return "Waiting for a new traced request…";
  if (state.harnessTraceStatus === "checking") return "Checking for a new matching request…";
  if (state.harnessTraceStatus === "failure") return state.harnessTraceMessage || "The matching request returned an error.";
  return "Run the command above, then Alex will match the new trace to this harness.";
}

function renderTraceSummary(trace) {
  const input = finite(trace.input_tokens);
  const output = finite(trace.output_tokens);
  const cost = trace.total_cost_usd ?? trace.cost_usd;
  const status = trace.error || finite(trace.status) >= 400 ? `Error · ${trace.status || "unknown"}` : trace.status || "Complete";
  const timestamp = trace.ts_response_ms || trace.ts_request_ms;
  const age = Math.max(0, Math.round((Date.now() - finite(timestamp)) / 1000));
  return `<dl class="trace-summary"><div><dt>Model</dt><dd>${escapeHtml(trace.model || trace.served_model || "Unknown")}</dd></div><div><dt>Tokens</dt><dd>${escapeHtml(`${formatInteger(input)} in · ${formatInteger(output)} out`)}</dd></div><div><dt>Cost</dt><dd>${escapeHtml(cost === undefined || cost === null ? "Not recorded" : formatMoney(cost))}</dd></div><div><dt>Status</dt><dd>${escapeHtml(status)}</dd></div><div><dt>Time</dt><dd>${escapeHtml(age < 10 ? "now" : `${formatAge(age)} ago`)}</dd></div></dl>`;
}

function renderOnboardingHarnessStages() {
  const node = $("#onboarding-harness-stages");
  if (!node) return;
  const connectable = state.harnesses.filter((harness) => harness.supports_connect);
  const installed = connectable.filter((harness) => harness.installed);
  const stageOneComplete = state.harnessStatus === "success";
  const stageTwoComplete = Boolean(state.harnessTrace);
  const cards = installed.map((harness) => `<button class="harness-choice ${state.selectedHarness === harness.name ? "selected" : ""}" data-onboarding-harness="${escapeHtml(harness.name)}">${harnessLogoTile(harness.name, harnessDisplayName(harness.name))}<span><strong>${escapeHtml(harnessDisplayName(harness.name))}</strong><small>${state.selectedHarness === harness.name && state.harnessPlanStatus === "success" ? "Plan loaded" : "Preview plan"}</small></span>${state.selectedHarness === harness.name ? "<b>✓</b>" : ""}</button>`).join("");
  const manualOptions = connectable.map((harness) => `<option value="${escapeHtml(harness.name)}" ${state.selectedHarness === harness.name ? "selected" : ""}>${escapeHtml(harnessDisplayName(harness.name))}</option>`).join("");
  const manualResult = state.manualHarnessResult ? `<p class="inline-message ${escapeHtml(state.manualHarnessResult.kind)}">${escapeHtml(state.manualHarnessResult.message)}</p>` : "";
  const stageOneBody = stageOneComplete ? "" : `<div class="harness-choice-grid">${cards || '<p class="muted">No installed, connectable harnesses were detected. Add one manually or skip this page and continue.</p>'}</div>${renderHarnessPlan()}<details class="manual-harness" ${state.manualHarnessResult ? "open" : ""}><summary>Add harness manually</summary><p>Choose the harness and enter its binary path. Alex saves the existing override, refreshes detection, and reports the detected version.</p><form id="manual-harness-form" class="manual-harness-form"><select name="harness" required>${manualOptions}</select><input name="binary" required placeholder="/absolute/path/to/binary" spellcheck="false"><button>Check binary</button></form><div id="manual-harness-result">${manualResult}</div></details>`;
  const command = state.selectedHarness ? harnessTestCommand(state.selectedHarness) : "";
  const stageTwoBody = stageOneComplete && !stageTwoComplete ? `<p>Use the connected profile and send one real request through Alex.</p><div class="copy-code"><code>${escapeHtml(command)}</code><button data-copy-harness-command>Copy</button></div>${operationMarkup(state.harnessTraceStatus === "failure" ? "failure" : "working", traceStatusMessage())}<button data-check-harness-trace>Check for Request</button>` : "";
  const traceLink = safeUrl(state.selectedHarness ? `/ui/?harness=${encodeURIComponent(state.selectedHarness)}#traces` : "/ui/#traces");
  const stageThreeBody = stageTwoComplete ? `${renderTraceSummary(state.harnessTrace)}<a class="primary button-link" href="${escapeHtml(traceLink)}" target="_blank" rel="noopener">Open Trace Browser</a><p class="micro">Opens in a new tab filtered with <code>harness:${escapeHtml(state.selectedHarness)}</code>.</p>` : '<p class="locked-copy">Complete the previous stage to unlock this one.</p>';
  node.innerHTML = `<section class="onboarding-stage ${stageOneComplete ? "collapsed" : ""}">${stageHeader(1, "Pick your harness", stageOneComplete, state.harnessSummary ? `${state.harnessSummary.models_total || 0} models ready ✓` : "Connected", "change")}${stageOneBody}</section><section class="onboarding-stage ${!stageOneComplete ? "locked" : ""} ${stageTwoComplete ? "collapsed" : ""}">${stageHeader(2, "Send a test request", stageTwoComplete, stageTwoComplete ? `${state.harnessTrace.model || "alex model"} · ${finite(state.harnessTrace.input_tokens) + finite(state.harnessTrace.output_tokens)} tokens` : "")}${!stageOneComplete ? '<p class="locked-copy">Complete the previous stage to unlock this one.</p>' : stageTwoBody}</section><section class="onboarding-stage ${!stageTwoComplete ? "locked" : ""}">${stageHeader(3, "See your trace", false)}${stageThreeBody}</section>`;
  bindActions(node, "[data-onboarding-harness]", (event) => selectOnboardingHarness(event.currentTarget.dataset.onboardingHarness));
  $("[data-connect-onboarding-harness]", node)?.addEventListener("click", connectOnboardingHarness);
  $("[data-stage-action='change']", node)?.addEventListener("click", changeOnboardingHarness);
  $("#manual-harness-form", node)?.addEventListener("submit", checkManualHarness);
  $("[data-copy-harness-command]", node)?.addEventListener("click", async (event) => {
    await copyLoginText(command, $("code", event.currentTarget.parentElement));
    event.currentTarget.textContent = "Copied";
  });
  $("[data-check-harness-trace]", node)?.addEventListener("click", () => pollForHarnessTrace(true));
}

async function selectOnboardingHarness(name) {
  stopHarnessTracePolling();
  state.selectedHarness = name;
  state.harnessPlan = [];
  state.harnessPlanStatus = "working";
  state.harnessStatus = "idle";
  state.harnessSummary = null;
  state.harnessTrace = null;
  state.harnessTraceStatus = "idle";
  state.manualHarnessResult = null;
  renderOnboardingHarnessStages();
  try {
    const preview = await api(`/admin/harnesses/${encodeURIComponent(name)}/connect?dry_run=true`, { method: "POST" });
    if (state.selectedHarness !== name) return;
    state.harnessPlan = preview.plan || [];
    state.harnessPlanStatus = "success";
  } catch (error) {
    if (state.selectedHarness !== name) return;
    state.harnessPlanStatus = "failure";
    state.harnessPlanMessage = error.message;
  }
  renderOnboardingHarnessStages();
}

async function connectOnboardingHarness(event) {
  const name = state.selectedHarness;
  if (!name) return;
  event.currentTarget.disabled = true;
  state.harnessStatus = "working";
  renderOnboardingHarnessStages();
  try {
    state.harnessSummary = await api(`/admin/harnesses/${encodeURIComponent(name)}/connect`, { method: "POST" });
    if (state.selectedHarness !== name) return;
    state.harnessStatus = "success";
    await loadHarnessesData(true);
    beginHarnessTracePolling(true);
  } catch (error) {
    if (state.selectedHarness !== name) return;
    state.harnessStatus = "failure";
    state.harnessStatusMessage = error.message;
  }
  renderOnboardingHarnessStages();
}

function changeOnboardingHarness() {
  stopHarnessTracePolling();
  state.selectedHarness = null;
  state.harnessPlan = [];
  state.harnessPlanStatus = "idle";
  state.harnessStatus = "idle";
  state.harnessSummary = null;
  state.harnessTrace = null;
  state.harnessTraceStatus = "idle";
  renderOnboardingHarnessStages();
}

async function checkManualHarness(event) {
  event.preventDefault();
  const form = event.currentTarget;
  const values = new FormData(form);
  const name = String(values.get("harness") || "");
  const binary = String(values.get("binary") || "").trim();
  const output = $("#manual-harness-result");
  const button = $("button", form);
  button.disabled = true;
  inlineMessage(output, "Checking the binary and refreshing detection…", "neutral");
  try {
    await api(`/admin/harnesses/${encodeURIComponent(name)}/override`, { method: "PUT", body: JSON.stringify({ binary, config_dir: null }) });
    const harnesses = await loadHarnessesData(true);
    const harness = harnesses.find((item) => item.name === name);
    if (!harness?.installed || !harness.binary) throw new Error("Binary not found or invalid for this harness.");
    const foundMessage = `✓ Found ${harnessDisplayName(name)}${harness.version ? ` · ${harness.version}` : ""}`;
    state.selectedHarness = name;
    await selectOnboardingHarness(name);
    state.manualHarnessResult = { kind: "success", message: foundMessage };
    renderOnboardingHarnessStages();
  } catch (error) {
    state.manualHarnessResult = { kind: "danger", message: `× ${error.message}` };
    inlineMessage(output, state.manualHarnessResult.message, state.manualHarnessResult.kind);
  }
  finally { button.disabled = false; }
}

function beginHarnessTracePolling(resetStart = true) {
  stopHarnessTracePolling();
  if (resetStart || !state.harnessTraceStartedMs) state.harnessTraceStartedMs = Date.now();
  state.harnessTraceStatus = "working";
  renderOnboardingHarnessStages();
  pollForHarnessTrace(false);
  state.harnessTracePoll = setInterval(() => pollForHarnessTrace(false), 2000);
}

function stopHarnessTracePolling() {
  clearInterval(state.harnessTracePoll);
  state.harnessTracePoll = null;
}

async function pollForHarnessTrace(manual = false) {
  if (!state.selectedHarness || !state.harnessTraceStartedMs || state.harnessTrace) return;
  if (manual) state.harnessTraceStatus = "checking";
  renderOnboardingHarnessStages();
  try {
    const params = new URLSearchParams({ limit: "100", harness: state.selectedHarness, since: new Date(state.harnessTraceStartedMs).toISOString() });
    const data = await api(`/traces/summaries?${params}`);
    const trace = (data.traces || []).filter((item) => finite(item.ts_request_ms) >= state.harnessTraceStartedMs).sort((left, right) => finite(right.ts_request_ms) - finite(left.ts_request_ms))[0];
    if (!trace) {
      state.harnessTraceStatus = "working";
      state.harnessTraceMessage = manual ? "No new matching request yet — run the command, then check again." : "";
      renderOnboardingHarnessStages();
      return;
    }
    if (finite(trace.status) >= 400 || trace.error) {
      state.harnessTraceStatus = "failure";
      state.harnessTraceMessage = `Your request reached Alex but the provider rejected it: ${trace.error || `HTTP ${trace.status}`}`;
      stopHarnessTracePolling();
      renderOnboardingHarnessStages();
      return;
    }
    const metadata = await api(`/traces/${encodeURIComponent(trace.id)}/metadata`).catch(() => null);
    state.harnessTrace = { ...trace, ...(metadata?.trace || metadata || {}) };
    state.harnessTraceStatus = "success";
    stopHarnessTracePolling();
    renderOnboardingHarnessStages();
  } catch (error) {
    if (state.sessionRecovery) return;
    state.harnessTraceStatus = manual ? "failure" : "working";
    state.harnessTraceMessage = manual ? error.message : "";
    renderOnboardingHarnessStages();
  }
}

async function logout() {
  try { await api("/web/auth/logout", { method: "POST" }); } catch { /* clear local UI regardless */ }
  state.sessionAuthenticated = false;
  state.adminKey = null;
  clearInterval(state.refreshTimer);
  state.auth = await api("/web/auth/status").catch(() => ({ password_configured: true }));
  showAuthState(state.auth.password_configured ? "password-login" : "key-bootstrap");
}

/* 3. Navigation and refresh scheduling. */
const VIEW_LOADERS = {
  dashboard: loadDashboard,
  general: loadGeneral,
  updates: loadUpdates,
  providers: loadProviders,
  harnesses: () => loadHarnesses(false),
  credentials: loadCredentials,
  dario: loadDario,
  middleware: loadMiddleware,
  notifications: loadNotifications,
  traces: () => loadTraces(false),
  onboarding: async () => { await refreshSharedData(); renderOnboardingData(); },
};

function viewRoute(requested) {
  const [view, encodedSection] = String(requested || "").split("/", 2);
  let section = null;
  if (encodedSection) {
    try { section = decodeURIComponent(encodedSection); } catch { section = null; }
  }
  return { view, section };
}

function selectView(requested, updateHash = true) {
  const route = viewRoute(requested);
  let view = VIEW_COPY[route.view] ? route.view : "dashboard";
  // Traces stays reachable mid-onboarding so "See your trace" can open it in a new tab.
  if (!state.auth?.onboarding_completed && view !== "traces") view = "onboarding";
  if (view === "providers") state.providerTab = route.section;
  if (view === "harnesses") state.harnessTab = route.section;
  state.currentView = view;
  $$('[data-panel]').forEach((panel) => { panel.hidden = panel.id !== `${view}-view`; });
  $$('nav [data-view]').forEach((button) => {
    if (button.dataset.view === view) button.setAttribute("aria-current", "page");
    else button.removeAttribute("aria-current");
  });
  const [title, subtitle] = VIEW_COPY[view];
  $("#page-title").textContent = title;
  $("#page-subtitle").textContent = subtitle;
  document.body.classList.remove("nav-open");
  const section = view === "providers" ? state.providerTab : view === "harnesses" ? state.harnessTab : null;
  const nextHash = `#${view}${section ? `/${encodeURIComponent(section)}` : ""}`;
  if (updateHash && location.hash !== nextHash) history.pushState(null, "", nextHash);
  if (view === "onboarding") showOnboardingStep(state.onboardingStep);
  VIEW_LOADERS[view]?.().catch((error) => toast(error.message, "danger"));
}

function selectSectionTab(view, id, scroll = false) {
  if (view === "providers") {
    state.providerTab = id || null;
    renderProvidersView();
  } else {
    state.harnessTab = id || null;
    renderHarnessesView(scroll);
  }
  const nextHash = `#${view}${id ? `/${encodeURIComponent(id)}` : ""}`;
  if (location.hash !== nextHash) history.pushState(null, "", nextHash);
}

async function refreshCurrentView() {
  const loader = VIEW_LOADERS[state.currentView];
  if (!loader) return;
  const button = $("#global-refresh");
  button.disabled = true;
  try { await loader(); toast("Refreshed."); }
  catch (error) { toast(error.message, "danger"); }
  finally { button.disabled = false; }
}

function configureRefreshInterval() {
  const saved = Number(localStorage.getItem(REFRESH_STORAGE_KEY));
  state.refreshSeconds = [30, 60, 300, 900].includes(saved) ? saved : 60;
  $("#refresh-interval").value = String(state.refreshSeconds);
  clearInterval(state.refreshTimer);
  state.refreshTimer = setInterval(() => {
    if (!document.hidden && !$("#app-shell").hidden) refreshCurrentView();
  }, state.refreshSeconds * 1000);
}

function setRefreshInterval(event) {
  state.refreshSeconds = Number(event.currentTarget.value);
  localStorage.setItem(REFRESH_STORAGE_KEY, String(state.refreshSeconds));
  configureRefreshInterval();
  toast("Browser refresh cadence saved.");
}

/* 4. Shared data loaders. */
function loadHealthDisplay(health) {
  $("#daemon-status").textContent = "Online";
  $("#sidebar-dot").className = "status-dot ok";
  $("#sidebar-web-version").textContent = `Web UI v${health.version}`;
  $("#sidebar-uptime").textContent = `Daemon v${health.version} · up ${formatAge(health.uptime_s)}`;
  $("#about-version").textContent = health.version;
}

function renderSidebarUpdate(update) {
  const button = $("#sidebar-update");
  button.hidden = !update?.update_available;
  button.disabled = state.updateInstalling;
  button.title = state.updateInstalling ? "Installing update; daemon restarting" : (update?.update_available ? `Review update to ${update.latest}` : "Daemon is up to date");
}

function renderDashboardUpdate(update) {
  const banner = $("#update-banner");
  if (!banner) return;
  banner.hidden = !update?.update_available && !state.updateInstalling;
  if (banner.hidden) {
    banner.innerHTML = "";
    return;
  }
  banner.innerHTML = state.updateInstalling
    ? '<div><strong>Installing Alex update</strong><span>The daemon is restarting; this browser stays signed in.</span></div><button data-review-dashboard-update>View progress</button>'
    : `<div><strong>Alex ${escapeHtml(update.latest)} is available</strong><span>You are running ${escapeHtml(update.current)}.</span></div><button data-review-dashboard-update>Review update</button>`;
  $('[data-review-dashboard-update]', banner)?.addEventListener("click", () => selectView("updates", true));
}

function updateReleaseUrl(version) {
  return version ? safeUrl(`${ALEX_RELEASES_URL}/tag/v${encodeURIComponent(version)}`) : "";
}

function renderUpdatesPage() {
  const update = state.update;
  const health = state.health;
  if (!$("#updates-view") || !update || !health) return;

  $("#update-versions").innerHTML = facts([
    ["Daemon & web UI", health.version],
  ]);

  const available = Boolean(update.update_available);
  const uncertain = !available && update.confirmed === false;
  const current = update.current || health.version;
  const latest = update.latest || current;
  const title = $("#update-status-title");
  const pill = $("#update-status-pill");
  if (state.updateInstalling) {
    title.textContent = "Installing… daemon restarting";
    pill.textContent = "Installing";
    pill.className = "pill warning";
  } else if (available) {
    title.textContent = `Alex ${latest} is available`;
    pill.textContent = "Available";
    pill.className = "pill warning";
  } else if (uncertain) {
    title.textContent = "Latest release could not be confirmed";
    pill.textContent = "Unconfirmed";
    pill.className = "pill warning";
  } else {
    title.textContent = `You're on the latest ${current}`;
    pill.textContent = "Up to date";
    pill.className = "pill success";
  }

  const checkedAt = state.updateCheckedAt || update.checked_at_ms;
  const statusRows = [
    ["Current version", current],
    ["Available version", update.latest || "No newer release"],
    ["Last checked", checkedAt ? formatTime(checkedAt) : "Not checked yet"],
  ];
  if (update.reason) statusRows.splice(2, 0, ["Reason", update.reason]);
  $("#update-status-detail").innerHTML = facts(statusRows);
  if (update.notes_url) {
    const notesUrl = safeUrl(update.notes_url);
    if (notesUrl) $("#update-status-detail").insertAdjacentHTML("beforeend", `<p class="update-notes"><a href="${escapeHtml(notesUrl)}" target="_blank" rel="noopener">Release notes ↗</a></p>`);
  }

  const releaseLink = $("#update-release-link");
  const releaseUrl = updateReleaseUrl(update.latest);
  releaseLink.hidden = !releaseUrl;
  if (releaseUrl) releaseLink.href = releaseUrl;
  $("#all-releases-link").href = safeUrl(ALEX_RELEASES_URL);
  const install = $("#install-update");
  install.hidden = !available;
  install.disabled = state.updateInstalling;
  install.textContent = state.updateInstalling ? "Installing… daemon restarting" : "Install update…";
  $("#check-updates").disabled = state.updateInstalling;
  $$("select, button", $("#update-channel-form")).forEach((control) => { control.disabled = state.updateInstalling; });

  const outcome = $("#update-outcome");
  if (state.updateOutcome) inlineMessage(outcome, state.updateOutcome.message, state.updateOutcome.kind);
  else inlineMessage(outcome, "");
}

function renderUpdateState() {
  renderSidebarUpdate(state.update);
  renderDashboardUpdate(state.update);
  renderUpdatesPage();
}

async function refreshUpdateState({ markChecked = false, suppressSessionRecovery = false } = {}) {
  const [health, update] = await Promise.all([
    api("/health", { suppressSessionRecovery }),
    api("/admin/update", { suppressSessionRecovery }),
  ]);
  state.health = health;
  state.update = update;
  if (markChecked) state.updateCheckedAt = Date.now();
  loadHealthDisplay(health);
  renderUpdateState();
  return update;
}

async function loadAccounts() {
  const data = await api("/admin/accounts");
  state.accounts = data.accounts || [];
  if (state.onboardingStep === 2) renderOnboardingProviders();
  return state.accounts;
}

async function loadHarnessesData(refresh = false) {
  const data = await api(`/admin/harnesses${refresh ? "?refresh=1" : ""}`);
  state.harnesses = data.harnesses || [];
  return state.harnesses;
}

async function loadProvidersData() {
  const data = await api("/admin/providers");
  state.providers = data.providers || [];
  return state.providers;
}

async function refreshSharedData() {
  const results = await Promise.allSettled([
    refreshUpdateState(), loadAccounts(), loadHarnessesData(false), loadProvidersData(),
    api("/admin/middleware"), api("/admin/analytics?since_minutes=60"),
  ]);
  if (results[4].status === "fulfilled") state.middleware = results[4].value;
  if (results[5].status === "fulfilled") state.analytics = results[5].value;
  renderOnboardingData();
  const failed = results.find((result) => result.status === "rejected");
  if (failed && !state.health) throw failed.reason;
}

/* 5. Dashboard renderers. */
function analyticsTotals(analytics) {
  const totals = analytics?.totals || {};
  return {
    requests: totals.requests ?? totals.request_count ?? 0,
    errors: totals.errors ?? totals.error_count ?? 0,
    input: totals.input_tokens ?? 0,
    output: totals.output_tokens ?? 0,
    cost: totals.cost_usd ?? 0,
  };
}

function quotaPercent(entry) {
  const quota = entry?.quota || {};
  if (quota.remaining_pct !== undefined) return clamp(100 - finite(quota.remaining_pct), 0, 100);
  if (quota.used_pct !== undefined) return clamp(quota.used_pct, 0, 100);
  const windows = entry?.windows || [];
  return windows.length ? Math.max(...windows.map((window) => clamp(window.used_pct, 0, 100))) : 0;
}

function creditBalanceText(entry) {
  const raw = entry?.individual_credits_usd;
  if (raw === null || raw === undefined || !Number.isFinite(Number(raw))) return null;
  return `💰 $${Number(raw).toFixed(2)} credits`;
}

function renderLimits(node, providers) {
  node.innerHTML = providers.length ? providers.map((provider) => {
    const credits = creditBalanceText(provider);
    if (credits) {
      return `<div class="limit-row" data-credit-provider="${escapeHtml(provider.provider)}"><div><strong>${escapeHtml(provider.provider)}</strong><small>${escapeHtml(provider.source || "Credit balance")}</small></div><span data-credit-balance>${escapeHtml(credits)}</span></div>`;
    }
    const used = quotaPercent(provider);
    const label = provider.quota?.state || provider.plan || provider.source || "Observed usage";
    return `<div class="limit-row"><div><strong>${escapeHtml(provider.provider)}</strong><small>${escapeHtml(label)}</small></div><progress max="100" value="${escapeHtml(used)}"></progress><span>${escapeHtml(`${formatNumber(used)}% used`)}</span></div>`;
  }).join("") : '<p class="muted">No provider quota observations yet.</p>';
}

function compactAccounts(accounts) {
  return accounts.length ? accounts.slice(0, 6).map((account) => {
    const health = accountHealth(account);
    return `<div class="account-row"><span class="status-dot ${escapeHtml(health.value)}"></span><div><strong>${escapeHtml(account.email || account.label || account.name)}</strong><small>${escapeHtml(account.provider)} · ${escapeHtml(health.label)}</small></div>${account.needs_reauth ? `<button data-reauth-id="${escapeHtml(account.id)}" data-reauth-provider="${escapeHtml(account.provider)}">Re-auth</button>` : ""}</div>`;
  }).join("") : '<p class="muted">No accounts connected.</p>';
}

function darioEmptyState() {
  return '<div class="dario-empty"><p>Dario routes Anthropic subscriptions into non-Claude harnesses. Connect an Anthropic subscription to enable it.</p><a class="button-link" href="#providers/claude">Connect Anthropic</a></div>';
}

async function loadDashboard() {
  const requests = [
    refreshUpdateState(), loadAccounts(), loadHarnessesData(false),
    api("/admin/analytics?since_minutes=60"), api("/admin/analytics?since_minutes=1440"),
    api("/admin/limits"), api("/admin/dario"),
    api(`/traces/summaries?limit=6`),
  ];
  const [health, accounts, harnesses, hour, day, limits, darioResult, traces] = await Promise.allSettled(requests);
  if (health.status === "rejected") throw health.reason;
  state.analytics = hour.status === "fulfilled" ? hour.value : state.analytics;
  state.limits = limits.status === "fulfilled" ? limits.value : { providers: [] };
  state.dario = darioResult.status === "fulfilled" ? darioResult.value : null;
  const hourTotals = analyticsTotals(hour.value);
  const dayTotals = analyticsTotals(day.value);
  $("#dashboard-lede").textContent = `Alex ${state.health.version} has been online ${formatAge(state.health.uptime_s)} with ${formatInteger(state.health.in_flight)} request${state.health.in_flight === 1 ? "" : "s"} in flight.`;
  $("#dashboard-stats").innerHTML = statCards([
    ["Last hour", `${formatInteger(hourTotals.requests)} requests`, `${formatInteger(hourTotals.errors)} errors`],
    ["Last 24 hours", `${formatInteger(dayTotals.requests)} requests`, `${formatInteger(dayTotals.input + dayTotals.output)} tokens`],
    ["24h cost", formatMoney(dayTotals.cost), "Recorded estimate"],
    ["In flight", formatInteger(state.health.in_flight), `Uptime ${formatAge(state.health.uptime_s)}`],
  ]);
  renderDashboardUpdate(state.update);
  renderLimits($("#dashboard-limits"), state.limits.providers || []);
  $("#dashboard-accounts").innerHTML = compactAccounts(state.accounts);
  bindAccountActions($("#dashboard-accounts"));
  $("#dashboard-harnesses").innerHTML = state.harnesses.filter((item) => item.installed).slice(0, 5).map((item) => `<div class="mini-row"><div>${harnessLogoTile(item.name, item.display_name || item.name)}<strong>${escapeHtml(item.display_name || item.name)}</strong></div><span class="pill ${item.connected ? "success" : "neutral"}">${item.connected ? "Connected" : "Detected"}</span></div>`).join("") || '<p class="muted">No supported harness detected.</p>';
  $("#dashboard-dario").innerHTML = state.dario?.health === "not-applicable" ? darioEmptyState() : state.dario ? facts([["Health", state.dario.health], ["Active generation", state.dario.active_generation_id], ["Generations", state.dario.generations?.length || 0], ["Route enabled", state.dario.route_enabled]]) : '<p class="muted">Dario mode is not enabled.</p>';
  const traceRows = traces.status === "fulfilled" ? traces.value.traces || [] : [];
  $("#dashboard-traces").innerHTML = traceRows.map((trace) => `<button class="trace-mini" data-dashboard-trace="${escapeHtml(trace.id)}"><code>${escapeHtml(trace.model || trace.id)}</code><span>${escapeHtml(trace.provider || "unrouted")} · ${escapeHtml(formatTime(trace.ts_request_ms))}</span><b>${escapeHtml(trace.status ?? "—")}</b></button>`).join("") || '<p class="muted">No recent traces.</p>';
  bindActions($("#dashboard-traces"), "[data-dashboard-trace]", (event) => { selectView("traces", true); openTrace(event.currentTarget.dataset.dashboardTrace); });
}

/* 6. Settings destinations: General, Providers, Harnesses, Credentials, Dario,
 * Middleware, and Notifications. */
async function loadGeneral() {
  const [health, storage] = await Promise.allSettled([
    refreshUpdateState(), api("/admin/storage"),
  ]);
  if (health.status === "rejected") throw health.reason;
  $("#daemon-settings").innerHTML = facts([
    ["Version", state.health.version],
    ["Uptime", formatAge(state.health.uptime_s)],
    ["In-flight requests", state.health.in_flight],
    ["Dario active", state.health.dario],
    ["Daemon paths", storage.value?.data_dir || storage.value?.root || "Not exposed by this daemon"],
  ]);
  const network = $("#network-exposure");
  const loopback = ["localhost", "127.0.0.1", "::1"].includes(location.hostname);
  network.textContent = loopback ? "Loopback browser" : `LAN via ${location.hostname}`;
  network.className = `pill ${loopback ? "neutral" : "warning"}`;
  $("#storage-summary").innerHTML = storage.status === "fulfilled" ? facts(Object.entries(storage.value || {}).map(([key, value]) => [key.replaceAll("_", " "), typeof value === "object" ? JSON.stringify(value) : value])) : `<p class="inline-message danger">${escapeHtml(storage.reason?.message || "Storage status unavailable")}</p>`;
}

async function loadUpdates() {
  const [update, channel] = await Promise.allSettled([
    refreshUpdateState(),
    api("/admin/update/channel"),
  ]);
  if (update.status === "rejected") throw update.reason;
  const selectedChannel = channel.status === "fulfilled" ? channel.value.channel : state.update?.update_channel;
  if (selectedChannel) $("#update-channel-form").elements.channel.value = selectedChannel;
  renderUpdatesPage();
}

async function checkForUpdates() {
  const button = $("#check-updates");
  button.disabled = true;
  state.updateOutcome = null;
  renderUpdatesPage();
  try {
    await refreshUpdateState({ markChecked: true });
    toast(state.update?.update_available ? `Alex ${state.update.latest} is available.` : "Update check completed.");
  } catch (error) {
    state.updateOutcome = { kind: "danger", message: error.message };
    renderUpdatesPage();
  } finally {
    button.disabled = false;
  }
}

async function submitUpdateChannel(event) {
  event.preventDefault();
  const channel = new FormData(event.currentTarget).get("channel");
  try {
    await api("/admin/update/channel", { method: "POST", body: JSON.stringify({ channel }) });
    toast(`Update channel set to ${channel}.`);
    state.updateOutcome = null;
    await refreshUpdateState({ markChecked: true });
  } catch (error) { toast(error.message, "danger"); }
}

function openUpdateConfirmation() {
  if (!state.update?.update_available || state.updateInstalling) return;
  const current = state.update.current || state.health?.version || "current";
  const latest = state.update.latest || "latest";
  $("#update-confirm-title").textContent = `Install ${current} → ${latest}?`;
  $("#update-confirm-copy").textContent = `Install ${current} → ${latest}? The daemon restarts; this browser stays signed in.`;
  $("#update-confirm-dialog").showModal();
}

const wait = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds));

async function waitForUpdatedDaemon(targetVersion, timeoutMs = 60000) {
  const deadline = Date.now() + timeoutMs;
  let lastVersion = state.health?.version;
  let sawUnavailable = false;
  while (Date.now() < deadline) {
    await wait(1000);
    try {
      const health = await api("/health", { suppressSessionRecovery: true });
      lastVersion = health.version;
      if (health.version === targetVersion) return health;
    } catch {
      sawUnavailable = true;
    }
  }
  const restartDetail = sawUnavailable ? "The daemon restarted" : "No restart was observed";
  throw new Error(`${restartDetail}, but version ${targetVersion} did not return within 60 seconds${lastVersion ? ` (last response: ${lastVersion})` : ""}.`);
}

async function installUpdate() {
  $("#update-confirm-dialog").close();
  const targetVersion = state.update?.latest;
  if (!targetVersion || state.updateInstalling) return;
  state.updateInstalling = true;
  state.updateOutcome = null;
  renderUpdateState();
  try {
    const result = await api("/admin/update", { method: "POST" });
    if (!result.applying) {
      await refreshUpdateState({ markChecked: true, suppressSessionRecovery: true });
      state.updateOutcome = { kind: "success", message: `Alex ${state.health.version} is already up to date.` };
      return;
    }
    await waitForUpdatedDaemon(targetVersion);
    await refreshUpdateState({ markChecked: true, suppressSessionRecovery: true });
    if (state.health.version !== targetVersion) throw new Error(`The daemon returned on ${state.health.version}; expected ${targetVersion}.`);
    state.updateOutcome = { kind: "success", message: `Update installed successfully. Alex ${state.health.version} is running.` };
    toast(`Alex ${state.health.version} installed successfully.`);
  } catch (error) {
    await refreshUpdateState({ markChecked: true, suppressSessionRecovery: true }).catch(() => {});
    state.updateOutcome = { kind: "danger", message: `Update failed: ${error.payload?.reason || error.message}` };
  } finally {
    state.updateInstalling = false;
    renderUpdateState();
  }
}

async function pruneStorage(event) {
  event.preventDefault();
  const submitter = event.submitter;
  const form = new FormData(event.currentTarget);
  const apply = submitter?.value === "apply";
  if (apply && !confirm("Delete the selected stored trace bodies? Trace metadata will remain.")) return;
  const output = $("#storage-result");
  inlineMessage(output, apply ? "Pruning stored bodies…" : "Calculating prune preview…", "neutral");
  try {
    const result = await api("/admin/storage/prune", { method: "POST", body: JSON.stringify({ older_than: form.get("older_than"), bodies_only: true, dry_run: !apply }) });
    inlineMessage(output, JSON.stringify(result, null, 2), "success");
    await loadGeneral();
  } catch (error) { inlineMessage(output, error.message); }
}

function providerCanonical(provider) {
  return ({ claude: "anthropic", codex: "openai", grok: "xai" })[provider] || provider;
}

function providerTabId(provider) {
  return ({ anthropic: "claude", openai: "codex", xai: "grok" })[providerCanonical(provider)] || providerCanonical(provider);
}

function providerMatchesTab(provider, tab = state.providerTab) {
  return !tab || providerCanonical(provider) === providerCanonical(tab);
}

function accountHealth(account) {
  const health = account.health || "unknown";
  return { value: health, label: health === "unknown" ? "not checked yet" : health.replaceAll("_", " ") };
}

function renderSectionTabs(node, items, selected, kind) {
  const logo = kind === "providers" ? providerLogoTile : harnessLogoTile;
  node.innerHTML = `<button class="section-tab ${selected ? "" : "selected"}" data-section-tab="" aria-current="${selected ? "false" : "page"}"><span class="section-tab-all" aria-hidden="true">•••</span><strong>All</strong></button>${items.map(({ id, label }) => `<button class="section-tab ${selected === id ? "selected" : ""}" data-section-tab="${escapeHtml(id)}" aria-current="${selected === id ? "page" : "false"}">${logo(id, label, "section-tab-logo")}<strong>${escapeHtml(label)}</strong></button>`).join("")}`;
  bindActions(node, "[data-section-tab]", (event) => selectSectionTab(kind, event.currentTarget.dataset.sectionTab || null, kind === "harnesses"));
}

function renderProviderPicker(node, collapsible = true) {
  const counts = new Map();
  state.accounts.forEach((account) => counts.set(account.provider, (counts.get(account.provider) || 0) + 1));
  const definitions = state.providerTab ? PROVIDERS.filter(([id]) => id === state.providerTab) : PROVIDERS;
  node.innerHTML = definitions.map(([id, label, mode]) => {
    const count = counts.get(providerCanonical(id)) || 0;
    const action = mode === "oauth" ? "Connect subscription" : mode === "import" ? "Review import" : "Configure below";
    return `<button class="provider-picker-choice" data-provider="${escapeHtml(id)}" data-provider-mode="${escapeHtml(mode)}">${providerLogoTile(id, label)}<span class="provider-choice-copy"><strong>${escapeHtml(label)}</strong><span>${escapeHtml(`${count} connected · ${action}`)}</span></span></button>`;
  }).join("");
  node.hidden = state.providerTab ? false : (collapsible ? node.hidden : false);
  bindActions(node, "[data-provider]", (event) => {
    const button = event.currentTarget;
    if (button.dataset.providerMode === "oauth") startLogin(button.dataset.provider);
    else if (button.dataset.providerMode === "import") { selectSectionTab("providers", button.dataset.provider); loadImportCandidates(); }
    else { selectSectionTab("providers", button.dataset.provider); $(`#${button.dataset.provider}-form`)?.scrollIntoView({ behavior: "smooth" }); }
  });
}

function accountTitle(account) {
  return account.email || account.label || account.description || account.name || account.id;
}

function quotaMarkup(limits) {
  if (!limits || typeof limits !== "object") return '<span class="muted">No quota observation</span>';
  const credits = creditBalanceText(limits);
  if (credits) return `<div class="quota-line credit-balance" data-credit-balance><span>Credit balance</span><strong>${escapeHtml(credits)}</strong></div>`;
  const percent = quotaPercent(limits);
  return `<div class="quota-line"><progress max="100" value="${escapeHtml(percent)}"></progress><span>${escapeHtml(`${formatNumber(percent)}% used`)}</span></div>`;
}

function providerLimitsForAccount(account) {
  const limits = state.limits?.providers || [];
  return limits.find((entry) => entry.provider === account.provider && entry.account_id === account.id)
    || limits.find((entry) => entry.provider === account.provider && !entry.account_id)
    || null;
}

function renderAccountCard(account) {
  const health = accountHealth(account);
  const needsReauth = account.kind === "oauth" && (account.needs_reauth || health.value === "auth_failed" || account.status !== "active");
  return `<article class="account-card" data-account-card="${escapeHtml(account.id)}">
    <div class="account-head">${providerLogoTile(account.provider, account.provider, "account-provider-logo")}<span class="status-dot ${escapeHtml(health.value)}"></span><div><span class="section-kicker">${escapeHtml(account.provider)}</span><h3>${escapeHtml(accountTitle(account))}</h3><small>${escapeHtml(health.label)} · ${escapeHtml(account.kind)}${account.paused ? " · paused" : ""}</small></div></div>
    ${quotaMarkup(account.limits || providerLimitsForAccount(account))}
    ${facts([["Status", account.status], ["Expires", account.expires_in_s === null || account.expires_in_s === undefined ? "—" : formatAge(account.expires_in_s)], ["Routing", account.routing?.eligible ? `Priority ${finite(account.routing.priority) + 1}` : "Ineligible"], ["Reserve", account.routing?.reserve_pct === undefined ? "—" : `${account.routing.reserve_pct}%`]])}
    <div class="card-actions"><button data-account-pause="${escapeHtml(account.id)}" data-paused="${account.paused}">${account.paused ? "Resume" : "Pause"}</button>${needsReauth ? `<button data-reauth-id="${escapeHtml(account.id)}" data-reauth-provider="${escapeHtml(account.provider)}">Re-authenticate</button>` : ""}<button class="danger-button" data-account-remove="${escapeHtml(account.id)}">Remove</button></div>
  </article>`;
}

function bindAccountActions(root) {
  bindActions(root, "[data-reauth-id]", (event) => startReauth(event.currentTarget.dataset.reauthProvider, event.currentTarget.dataset.reauthId));
  bindActions(root, "[data-account-pause]", async (event) => {
    const button = event.currentTarget;
    button.disabled = true;
    try {
      await api(`/admin/accounts/${encodeURIComponent(button.dataset.accountPause)}`, { method: "PUT", body: JSON.stringify({ paused: button.dataset.paused !== "true" }) });
      toast(button.dataset.paused === "true" ? "Account resumed." : "Account paused.");
      await loadProviders();
    } catch (error) { toast(error.message, "danger"); button.disabled = false; }
  });
  bindActions(root, "[data-account-remove]", async (event) => {
    const id = event.currentTarget.dataset.accountRemove;
    if (!confirm("Remove this credential from Alex? Historical trace metadata will remain.")) return;
    try { await api(`/admin/accounts/${encodeURIComponent(id)}`, { method: "DELETE" }); toast("Credential removed."); await loadProviders(); }
    catch (error) { toast(error.message, "danger"); }
  });
}

function renderUsageAnalytics(data) {
  const accounts = data.by_account || [];
  const totalTokens = accounts.reduce((sum, account) => sum + finite(account.input_tokens) + finite(account.output_tokens), 0);
  $("#analytics-total").textContent = `${formatInteger(totalTokens)} tokens · ${formatInteger(accounts.reduce((sum, account) => sum + finite(account.requests), 0))} requests`;
  $("#usage-share").innerHTML = accounts.length ? accounts.map((account, index) => {
    const tokens = finite(account.input_tokens) + finite(account.output_tokens);
    const share = totalTokens ? tokens / totalTokens * 100 : 0;
    return `<div><i class="chart-color-${index % CHART_COLORS.length}"></i><span>${escapeHtml(account.account_id)}</span><strong>${escapeHtml(`${formatNumber(share)}%`)}</strong></div>`;
  }).join("") : '<p class="muted">No account traffic in the selected period.</p>';
  renderUsageChart(data);
}

function providerAnalytics(data, tab) {
  if (!tab) return data;
  const accountIds = new Set(state.accounts.filter((account) => providerMatchesTab(account.provider, tab)).map((account) => account.id));
  const belongs = (entry) => accountIds.has(entry.account_id);
  return {
    ...data,
    by_account: (data.by_account || []).filter(belongs),
    series: (data.series || []).filter(belongs),
    plot_series: (data.plot_series || []).filter(belongs),
  };
}

function renderUsageChart(data) {
  const series = data.plot_series || [];
  const count = Math.max(1, finite(data.bucket_count, data.x_labels?.length || 1));
  const maximum = Math.max(1, ...series.flatMap((item) => item.values || []).map(finite));
  const paths = series.map((item, index) => {
    const values = item.values || [];
    const points = values.map((value, point) => `${count === 1 ? 0 : point / (count - 1) * 100},${100 - finite(value) / maximum * 90}`).join(" ");
    return `<polyline class="chart-line chart-color-${index % CHART_COLORS.length}" points="${escapeHtml(points)}" vector-effect="non-scaling-stroke"></polyline>`;
  }).join("");
  const labels = (data.x_labels || []).filter((_, index, all) => index === 0 || index === all.length - 1 || index === Math.floor(all.length / 2));
  $("#usage-chart").innerHTML = series.length ? `<svg viewBox="0 0 100 100" preserveAspectRatio="none" role="img" aria-label="Account token usage over time">${paths}</svg><div class="chart-labels">${labels.map((label) => `<span>${escapeHtml(label)}</span>`).join("")}</div>` : '<div class="empty-state"><p>Usage history will appear after routed requests.</p></div>';
}

function routingEditor(snapshot) {
  return `<form class="routing-form" data-routing-provider="${escapeHtml(snapshot.provider)}">
    <div class="section-label first">${escapeHtml(snapshot.provider)} ROUTING</div>
    <div class="form-grid four"><label>Strategy<select name="strategy">${[["reset_first", "Reset first"], ["highest_quota", "Highest quota"], ["priority", "Priority"], ["round_robin", "Round robin"]].map(([value, label]) => `<option value="${value}" ${snapshot.strategy === value ? "selected" : ""}>${label}</option>`).join("")}</select></label><label>Provider reserve %<input name="reserve_pct" type="number" min="0" max="100" value="${escapeHtml(snapshot.reserve_pct)}"></label><label class="toggle-line"><input name="allow_mid_thread_failover" type="checkbox" ${snapshot.allow_mid_thread_failover ? "checked" : ""}>Mid-thread failover</label></div>
    <div class="routing-accounts">${(snapshot.accounts || []).map((account) => `<div class="routing-row" data-routing-account="${escapeHtml(account.account_id)}"><strong>${escapeHtml(account.account_id)}</strong><label><input name="eligible" type="checkbox" ${account.eligible ? "checked" : ""}> Eligible</label><label>Priority <input name="priority" type="number" min="1" value="${escapeHtml(finite(account.priority) + 1)}"></label><label>Reserve % <input name="account_reserve_pct" type="number" min="0" max="100" value="${escapeHtml(account.reserve_pct ?? snapshot.reserve_pct)}"></label><span class="pill ${account.reserve_blocked ? "warning" : "neutral"}">${account.reserve_blocked ? "Reserve blocked" : "Available"}</span></div>`).join("")}</div>
    <button class="primary">Save ${escapeHtml(snapshot.provider)} routing</button>
  </form>`;
}

async function saveRouting(event) {
  event.preventDefault();
  const form = event.currentTarget;
  const accounts = $$('[data-routing-account]', form).map((row) => ({
    account_id: row.dataset.routingAccount,
    eligible: $('[name="eligible"]', row).checked,
    priority: Math.max(0, finite($('[name="priority"]', row).value, 1) - 1),
    reserve_pct: finite($('[name="account_reserve_pct"]', row).value),
  }));
  const payload = {
    strategy: form.elements.strategy.value,
    reserve_pct: finite(form.elements.reserve_pct.value),
    allow_mid_thread_failover: form.elements.allow_mid_thread_failover.checked,
    accounts,
  };
  try {
    await api(`/admin/routing/${encodeURIComponent(form.dataset.routingProvider)}`, { method: "PUT", body: JSON.stringify(payload) });
    toast("Routing policy saved.");
    await loadProviders();
  } catch (error) { toast(error.message, "danger"); }
}

async function loadProviders() {
  const [accounts, analytics, limits] = await Promise.all([
    loadAccounts(),
    api("/admin/accounts/analytics?since_minutes=1440&bucket_minutes=60"),
    api("/admin/limits"),
  ]);
  const providers = [...new Set(accounts.map((account) => account.provider))];
  const routings = await Promise.all(providers.map((provider) => api(`/admin/routing/${encodeURIComponent(provider)}`).catch(() => null)));
  state.providerAnalytics = analytics;
  state.limits = limits;
  state.providerRoutings = routings.filter(Boolean);
  renderProvidersView();
  await Promise.allSettled([loadOpenRouterModels(), loadExo(), loadCLIProxyAPI(), loadImportCandidates()]);
}

function renderProvidersView() {
  const validTab = PROVIDERS.some(([id]) => id === state.providerTab) ? state.providerTab : null;
  if (state.providerTab && !validTab && state.currentView === "providers") history.replaceState(null, "", "#providers");
  state.providerTab = validTab;
  renderSectionTabs($("#provider-tabs"), PROVIDERS.map(([id, label]) => ({ id, label })), validTab, "providers");
  const accounts = state.accounts.filter((account) => providerMatchesTab(account.provider, validTab));
  const routings = state.providerRoutings.filter((snapshot) => providerMatchesTab(snapshot.provider, validTab));
  renderUsageAnalytics(providerAnalytics(state.providerAnalytics || {}, validTab));
  renderProviderPicker($("#provider-picker"), true);
  const accountList = $("#provider-accounts");
  const empty = validTab ? `No ${PROVIDERS.find(([id]) => id === validTab)?.[1] || validTab} accounts connected yet.` : "No provider accounts connected. Choose Add provider to begin.";
  accountList.innerHTML = accounts.length ? `${accounts.map(renderAccountCard).join("")}${routings.map(routingEditor).join("")}` : `<article class="settings-card provider-empty-state"><p class="muted">${escapeHtml(empty)}</p></article>`;
  bindAccountActions(accountList);
  $$('.routing-form', accountList).forEach((form) => form.addEventListener("submit", saveRouting));
  $$('[data-provider-setup]', $("#provider-setup")).forEach((section) => { section.hidden = Boolean(validTab) && section.dataset.providerSetup !== validTab; });
  $("#provider-setup").hidden = Boolean(validTab) && !$$('[data-provider-setup]', $("#provider-setup")).some((section) => !section.hidden);
  renderImportCandidates();
}

let credentialPingRun = 0;

function credentialPingResult(account, results) {
  const exact = results.find((result) => result.account_id === account.id);
  if (exact) return exact;
  const providerResults = results.filter((result) => providerCanonical(result.provider) === providerCanonical(account.provider));
  const providerAccounts = activeCredentialAccounts().filter((candidate) => providerCanonical(candidate.provider) === providerCanonical(account.provider));
  return providerResults.length === 1 && providerAccounts.length === 1 ? providerResults[0] : null;
}

function activeCredentialAccounts() {
  return state.accounts.filter((account) => account.status === "active" && !account.paused);
}

function renderCredentialPingRows(results = null, requestError = "") {
  const rows = $("#credential-ping-rows");
  const accounts = activeCredentialAccounts();
  if (!accounts.length) {
    rows.innerHTML = '<div class="credential-ping-empty">No provider credentials are connected.</div>';
    return { passed: 0, total: 0 };
  }
  let passed = 0;
  rows.innerHTML = accounts.map((account) => {
    if (!results && !requestError) {
      return `<div class="credential-ping-row pending">${providerLogoTile(account.provider, account.provider, "credential-ping-logo")}<div><strong>${escapeHtml(account.email || account.id)}</strong><small>${escapeHtml(account.provider)}</small></div><span class="spinner" aria-label="Checking"></span></div>`;
    }
    const result = requestError ? null : credentialPingResult(account, results || []);
    const ok = Boolean(result?.ok);
    if (ok) passed += 1;
    const message = requestError || result?.message || "No ping result was returned for this credential.";
    return `<div class="credential-ping-row ${ok ? "passed" : "failed"}">${providerLogoTile(account.provider, account.provider, "credential-ping-logo")}<div><strong>${escapeHtml(account.email || account.id)}</strong><small>${escapeHtml(account.provider)}${result ? ` · ${escapeHtml(`${result.latency_ms} ms`)}` : ""}</small>${ok ? "" : `<p>${escapeHtml(message)}</p>`}</div><span class="credential-ping-mark" aria-label="${ok ? "Passed" : "Failed"}">${ok ? "✓" : "×"}</span></div>`;
  }).join("");
  return { passed, total: accounts.length };
}

async function runCredentialPingChecks() {
  const run = ++credentialPingRun;
  const rerun = $("#rerun-credential-ping");
  const summary = $("#credential-ping-summary");
  rerun.disabled = true;
  summary.className = "credential-ping-summary checking";
  const accountCount = activeCredentialAccounts().length;
  summary.textContent = `Checking ${accountCount} credential${accountCount === 1 ? "" : "s"}…`;
  renderCredentialPingRows();
  try {
    const data = await api("/admin/accounts/test", { method: "POST" });
    if (run !== credentialPingRun) return;
    const outcome = renderCredentialPingRows(data.results || []);
    summary.className = `credential-ping-summary ${outcome.passed === outcome.total ? "success" : "warning"}`;
    summary.textContent = `${outcome.passed}/${outcome.total} credentials passed`;
    await loadAccounts();
  } catch (error) {
    if (run !== credentialPingRun) return;
    const outcome = renderCredentialPingRows([], error.message);
    summary.className = "credential-ping-summary warning";
    summary.textContent = `${outcome.passed}/${outcome.total} credentials passed`;
  } finally {
    if (run === credentialPingRun) rerun.disabled = false;
  }
}

function openCredentialPingChecks() {
  const dialog = $("#credential-ping-dialog");
  if (!dialog.open) dialog.showModal();
  runCredentialPingChecks();
}

async function saveGeminiKey(event) {
  event.preventDefault();
  const form = event.currentTarget;
  const key = new FormData(form).get("key");
  try {
    await api("/admin/auth/gemini-key", { method: "POST", body: JSON.stringify({ key }) });
    form.reset();
    toast("Gemini API key saved.");
    await loadProviders();
  } catch (error) { toast(error.message, "danger"); }
}

async function saveOpenRouter(event) {
  event.preventDefault();
  const form = event.currentTarget;
  const values = Object.fromEntries(new FormData(form));
  try {
    await api("/admin/auth/openrouter-key", { method: "POST", body: JSON.stringify(values) });
    form.elements.key.value = "";
    toast("OpenRouter key saved.");
    await Promise.all([loadAccounts(), loadOpenRouterModels()]);
  } catch (error) { toast(error.message, "danger"); }
}

async function loadOpenRouterModels() {
  const data = await api("/admin/openrouter/exposed");
  state.openrouter = data;
  const selected = new Set(data.exposed || []);
  const available = [...new Set([...(data.available || []), ...(data.exposed || [])])];
  const node = $("#openrouter-models");
  node.innerHTML = `<div class="section-label">EXPOSED MODELS</div><p class="setting-caption">Only selected models are advertised to connected harnesses.</p><div class="model-picker">${available.map((model) => `<label><input type="checkbox" value="${escapeHtml(model)}" ${selected.has(model) ? "checked" : ""}> <span>${escapeHtml(model)}</span></label>`).join("") || '<p class="muted">Save an OpenRouter key to load its model catalog.</p>'}</div><button data-save-openrouter-models ${available.length ? "" : "disabled"}>Save exposed models</button>`;
  $('[data-save-openrouter-models]', node)?.addEventListener("click", saveOpenRouterModels);
}

async function saveOpenRouterModels() {
  const exposed = $$('#openrouter-models input:checked').map((input) => input.value);
  try {
    await api("/admin/openrouter/exposed", { method: "POST", body: JSON.stringify({ exposed }) });
    toast("OpenRouter model curation saved.");
    await loadOpenRouterModels();
  } catch (error) { toast(error.message, "danger"); }
}

async function loadExo() {
  const [config, status] = await Promise.all([api("/admin/exo"), api("/admin/exo/status")]);
  state.exo = config;
  $("#exo-form").elements.url.value = config.url;
  const node = $("#exo-result");
  let models = [];
  let modelError = "";
  if (status.running) {
    try { models = (await api("/admin/exo/models")).models || []; }
    catch (error) { modelError = error.message; }
  }
  node.innerHTML = `<div class="inline-message ${status.running ? "success" : "warning"}">${status.running ? `Exo is running with ${escapeHtml(status.model_count)} models.` : escapeHtml(status.error || "Exo is not reachable.")}</div>${modelError ? `<p class="inline-message danger">${escapeHtml(modelError)}</p>` : ""}${models.length ? `<div class="model-picker">${models.map((model) => `<label><input type="checkbox" value="${escapeHtml(model.id)}" ${model.enabled ? "checked" : ""}> <span>${escapeHtml(model.name || model.id)}${model.running === true ? " · running" : ""}</span></label>`).join("")}</div><button data-save-exo-models>Save exposed models</button>` : ""}`;
  $('[data-save-exo-models]', node)?.addEventListener("click", saveExoModels);
}

async function probeExo(event) {
  event.preventDefault();
  const url = new FormData(event.currentTarget).get("url");
  try {
    await api("/admin/exo", { method: "PUT", body: JSON.stringify({ url, enabled_models: state.exo?.enabled_models || [] }) });
    toast("Exo endpoint saved and probed.");
    await loadExo();
  } catch (error) { renderError($("#exo-result"), error, "Could not save or probe Exo"); }
}

async function saveExoModels() {
  const enabled_models = $$('#exo-result input:checked').map((input) => input.value);
  try {
    await api("/admin/exo", { method: "PUT", body: JSON.stringify({ url: state.exo.url, enabled_models }) });
    toast("Exo model selection saved.");
    await loadExo();
  } catch (error) { toast(error.message, "danger"); }
}

async function loadImportCandidates() {
  const node = $("#import-candidates");
  try {
    const data = await api("/admin/auth/import-candidates");
    state.importCandidates = data.candidates || [];
    renderImportCandidates();
  } catch (error) { renderError(node, error, "Credential scan failed"); }
}

function renderImportCandidates() {
  const node = $("#import-candidates");
  if (!node) return;
  const candidates = state.importCandidates.filter((candidate) => providerMatchesTab(candidate.provider));
  node.innerHTML = candidates.map((candidate) => `<div class="mini-row"><div><strong>${escapeHtml(candidate.label)}</strong><small>${escapeHtml(candidate.provider)} · ${escapeHtml(candidate.kind)} · ${escapeHtml(candidate.source_path)}</small></div><button data-import-source="${escapeHtml(candidate.source)}">Review & import</button></div>`).join("") || '<p class="muted">No importable CLI credentials detected.</p>';
  bindActions(node, "[data-import-source]", importCredential);
}

async function importCredential(event) {
  const source = event.currentTarget.dataset.importSource;
  if (!confirm(`Import credentials from ${source}? Alex will copy only the selected provider credential.`)) return;
  try {
    const result = await api("/admin/auth/import", { method: "POST", body: JSON.stringify({ source }) });
    const outcome = result.outcomes?.[0];
    toast(outcome?.imported?.length ? `Imported ${outcome.imported.length} credential(s).` : (outcome?.note || "Import completed."));
    await loadProviders();
  } catch (error) { toast(error.message, "danger"); }
}

function renderCLIProxyAPI(result, message = "") {
  state.cliproxyapi = result;
  const panel = $("#cliproxyapi-result");
  panel.hidden = false;
  panel.innerHTML = `<div class="inline-message ${result.connected ? "success" : "neutral"}"><strong>${escapeHtml(message || (result.connected ? "CLIProxyAPI connected" : "CLIProxyAPI is not configured"))}</strong>${result.url ? `<p>${escapeHtml(result.url)}</p>` : ""}</div>${result.models?.length ? `<p>${escapeHtml(result.models.length)} model(s): ${escapeHtml(result.models.slice(0, 8).join(", "))}</p><button data-test-cliproxyapi>Send routed test</button>` : ""}`;
  $('[data-test-cliproxyapi]', panel)?.addEventListener("click", testCLIProxyAPI);
}

async function loadCLIProxyAPI() {
  const result = await api("/admin/cliproxyapi");
  if (result.connected) renderCLIProxyAPI(result);
  return result;
}

async function saveCLIProxyAPI(event) {
  event.preventDefault();
  const form = event.currentTarget;
  const values = new FormData(form);
  const panel = $("#cliproxyapi-result");
  panel.hidden = false;
  panel.textContent = "Probing CLIProxyAPI…";
  try {
    const result = await api("/admin/auth/cliproxyapi", { method: "POST", body: JSON.stringify({ url: values.get("url"), credential: values.get("credential") }) });
    form.elements.credential.value = "";
    renderCLIProxyAPI({ ...result, connected: true }, "CLIProxyAPI connected and ready");
    await loadAccounts();
  } catch (error) { renderError(panel, error, "CLIProxyAPI connection failed"); }
}

async function testCLIProxyAPI() {
  if (!state.cliproxyapi?.models?.length) return;
  const panel = $("#cliproxyapi-result");
  panel.insertAdjacentHTML("beforeend", '<p data-cliproxyapi-test class="muted">Sending a routed test request…</p>');
  const output = $('[data-cliproxyapi-test]', panel);
  try {
    const result = await api("/admin/accounts/test", { method: "POST" });
    const probe = (result.results || []).find((item) => item.provider === "cliproxyapi");
    if (!probe) throw new Error("CLIProxyAPI is not active in the credential test set");
    output.textContent = `${probe.ok ? "Test completed" : "Test failed"}: ${probe.message} (${probe.latency_ms} ms)`;
  } catch (error) { output.textContent = `Test failed: ${error.message}`; }
}

function harnessSupportNote(harness) {
  if (!harness.supports_connect) return "Detection only on this platform; Alex cannot configure this harness yet.";
  if (!harness.installed) return "Not detected. Set a binary or config-directory override if installed elsewhere.";
  if (harness.daemon_reachable === false) return "Detected, but this harness cannot currently reach the daemon.";
  return "";
}

function renderHarnessCard(harness) {
  const override = harness.override || {};
  const support = harnessSupportNote(harness);
  const captureSupported = ["pi", "claude", "codex", "amp"].includes(harness.name);
  return `<article class="harness-card" data-harness-card="${escapeHtml(harness.name)}">
    <div class="card-heading"><div class="harness-card-identity">${harnessLogoTile(harness.name, harness.display_name || harness.name)}<div><span class="section-kicker">${harness.installed ? "DETECTED" : "NOT DETECTED"}</span><h3>${escapeHtml(harness.display_name || harness.name)}</h3><p>${escapeHtml(harness.version || harness.binary || "Version unavailable")}</p></div></div><span class="pill ${harness.connected ? "success" : harness.installed ? "neutral" : "warning"}">${harness.connected ? "Connected" : harness.installed ? "Available" : "Unavailable"}</span></div>
    ${support ? `<p class="inline-message warning">${escapeHtml(support)}</p>` : ""}
    ${facts([["Binary", harness.binary], ["Config directory", harness.config_dir], ["Models", harness.models_total ?? harness.models?.length], ["Last checked", harness.checked_ms ? formatTime(harness.checked_ms) : "Current scan"]])}
    <div class="card-actions">${harness.supports_connect && harness.installed ? `<button class="primary" data-harness-mutate="${escapeHtml(harness.connected ? "disconnect" : "connect")}">${harness.connected ? "Disconnect" : "Connect"}</button><button data-harness-refresh ${harness.connected ? "" : "disabled"}>Refresh models/config</button>` : ""}</div>
    <form class="form-grid harness-override" data-harness-override><label>Binary override<input name="binary" value="${escapeHtml(override.binary || "")}" placeholder="Auto-detect"></label><label>Config directory<input name="config_dir" value="${escapeHtml(override.config_dir || "")}" placeholder="Default"></label><button>Save override</button><button type="button" data-clear-override>Clear</button></form>
    <label class="toggle-line"><input data-tool-capture type="checkbox" ${harness.tool_capture_enabled ? "checked" : ""} ${captureSupported && harness.connected ? "" : "disabled"}>Capture tool calls ${captureSupported ? "" : "(not yet supported)"}</label>
  </article>`;
}

async function loadHarnesses(refresh = false) {
  await loadHarnessesData(refresh);
  renderHarnessesView();
  renderOnboardingData();
}

function orderedHarnesses(harnesses) {
  return [...harnesses].sort((left, right) => {
    const leftIndex = HARNESS_ORDER.indexOf(left.name);
    const rightIndex = HARNESS_ORDER.indexOf(right.name);
    const leftOrder = leftIndex === -1 ? HARNESS_ORDER.length : leftIndex;
    const rightOrder = rightIndex === -1 ? HARNESS_ORDER.length : rightIndex;
    return leftOrder - rightOrder || String(left.display_name || left.name).localeCompare(String(right.display_name || right.name));
  });
}

function renderHarnessesView(scroll = false) {
  const harnesses = orderedHarnesses(state.harnesses);
  const detected = harnesses.filter((item) => item.installed);
  const validTab = detected.some((item) => item.name === state.harnessTab) ? state.harnessTab : null;
  if (state.harnessTab && !validTab && state.currentView === "harnesses") history.replaceState(null, "", "#harnesses");
  state.harnessTab = validTab;
  renderSectionTabs($("#harness-tabs"), detected.map((item) => ({ id: item.name, label: item.display_name || item.name })), validTab, "harnesses");
  $("#harness-summary").innerHTML = statCards([
    ["Detected", harnesses.filter((item) => item.installed).length],
    ["Connected", harnesses.filter((item) => item.connected).length],
    ["Configurable", harnesses.filter((item) => item.supports_connect).length],
    ["Tool capture", harnesses.filter((item) => item.tool_capture_enabled).length],
  ]);
  const list = $("#harness-list");
  const visible = validTab ? harnesses.filter((item) => item.name === validTab) : harnesses;
  list.innerHTML = visible.map(renderHarnessCard).join("") || '<article class="settings-card provider-empty-state"><p class="muted">No detected harnesses to show.</p></article>';
  $$('.harness-card', list).forEach(bindHarnessCard);
  $("#harness-advanced").hidden = Boolean(validTab);
  if (scroll && validTab) requestAnimationFrame(() => $(`[data-harness-card="${CSS.escape(validTab)}"]`, list)?.scrollIntoView({ behavior: "smooth", block: "start" }));
}

function bindHarnessCard(card) {
  const name = card.dataset.harnessCard;
  $('[data-harness-mutate]', card)?.addEventListener("click", (event) => previewHarnessMutation(name, event.currentTarget.dataset.harnessMutate));
  $('[data-harness-refresh]', card)?.addEventListener("click", () => mutateHarness(name, "refresh-config"));
  $('[data-harness-override]', card)?.addEventListener("submit", (event) => saveHarnessOverride(event, name));
  $('[data-clear-override]', card)?.addEventListener("click", () => clearHarnessOverride(name));
  $('[data-tool-capture]', card)?.addEventListener("change", (event) => setHarnessCapture(name, event.currentTarget.checked));
}

async function previewHarnessMutation(name, action) {
  try {
    const preview = await api(`/admin/harnesses/${encodeURIComponent(name)}/${action}?dry_run=true`, { method: "POST" });
    const changes = (preview.plan || []).map((item) => `• ${item.detail || item.action || JSON.stringify(item)}`).join("\n") || "No file changes were reported.";
    if (!confirm(`${action === "connect" ? "Connect" : "Disconnect"} ${name}?\n\nPlanned changes:\n${changes}`)) return;
    await mutateHarness(name, action);
  } catch (error) { toast(error.message, "danger"); }
}

async function mutateHarness(name, action) {
  try {
    await api(`/admin/harnesses/${encodeURIComponent(name)}/${action}`, { method: "POST" });
    toast(`${name} ${action === "connect" ? "connected" : action === "disconnect" ? "disconnected" : "configuration refreshed"}.`);
    await loadHarnesses(true);
  } catch (error) { toast(error.message, "danger"); }
}

async function saveHarnessOverride(event, name) {
  event.preventDefault();
  const data = new FormData(event.currentTarget);
  const binary = String(data.get("binary") || "").trim() || null;
  const config_dir = String(data.get("config_dir") || "").trim() || null;
  try {
    await api(`/admin/harnesses/${encodeURIComponent(name)}/override`, { method: "PUT", body: JSON.stringify({ binary, config_dir }) });
    toast(`${name} override saved.`);
    await loadHarnesses(true);
  } catch (error) { toast(error.message, "danger"); }
}

async function clearHarnessOverride(name) {
  try {
    await api(`/admin/harnesses/${encodeURIComponent(name)}/override`, { method: "PUT", body: JSON.stringify({ binary: null, config_dir: null }) });
    toast(`${name} override cleared.`);
    await loadHarnesses(true);
  } catch (error) { toast(error.message, "danger"); }
}

async function setHarnessCapture(name, enabled) {
  try {
    await api(`/admin/harnesses/${encodeURIComponent(name)}/tool-capture`, { method: "PUT", body: JSON.stringify({ enabled }) });
    toast(`Tool capture ${enabled ? "enabled" : "disabled"} for ${name}.`);
    await loadHarnesses(false);
  } catch (error) { toast(error.message, "danger"); await loadHarnesses(false); }
}

function renderOneTimeKey(node, result, apiType = null, model = null) {
  const base = location.origin;
  let snippet = result.exports || "";
  if (apiType === "anthropic") snippet = `export ANTHROPIC_BASE_URL=${base}\nexport ANTHROPIC_API_KEY=${result.key}${model ? `\n# model: ${model}` : ""}`;
  if (apiType === "openai") snippet = `export OPENAI_BASE_URL=${base}/v1\nexport OPENAI_API_KEY=${result.key}${model ? `\n# model: ${model}` : ""}`;
  node.hidden = false;
  node.innerHTML = `<strong>Copy this key now — it will not be shown again.</strong><pre>${escapeHtml(result.key)}</pre>${snippet ? `<pre>${escapeHtml(snippet)}</pre>` : ""}<button data-copy-secret>Copy connection details</button>`;
  $('[data-copy-secret]', node)?.addEventListener("click", async () => {
    try { await navigator.clipboard.writeText(snippet || result.key); toast("Copied to clipboard."); }
    catch { toast("Clipboard access was denied. Select and copy the text manually.", "warning"); }
  });
}

async function mintRunKey(event, connectHelper = false) {
  event.preventDefault();
  const form = event.currentTarget;
  const values = new FormData(form);
  const model = String(values.get("model") || "").trim();
  const payload = {
    kind: "run",
    label: String(values.get("label") || "").trim() || null,
    ttl_seconds: Number(values.get("ttl_seconds") || 86400),
    tags: model ? { model } : {},
  };
  try {
    const result = await api("/admin/run-keys", { method: "POST", body: JSON.stringify(payload) });
    renderOneTimeKey(connectHelper ? $("#connect-snippet") : $("#minted-key"), result, connectHelper ? values.get("api") : null, model);
    toast("Scoped model-only key minted.");
    await loadCredentials();
  } catch (error) { toast(error.message, "danger"); }
}

function renderRunKeys(keys) {
  const node = $("#run-key-list");
  node.innerHTML = `<div class="key-toolbar"><button data-revoke-all-keys ${keys.some((key) => !key.revoked && key.kind !== "harness") ? "" : "disabled"}>Revoke all model keys</button><button data-clear-revoked ${keys.some((key) => key.revoked) ? "" : "disabled"}>Clear revoked records</button></div>${keys.map((key) => `<div class="key-row"><div><strong>${escapeHtml(key.label || key.kind)}</strong><code>${escapeHtml(key.key_fingerprint || key.id)}</code><small>${escapeHtml(key.kind)} · created ${escapeHtml(formatTime(key.created_ms))} · ${escapeHtml(key.use_count || 0)} uses${key.expires_ms ? ` · expires ${escapeHtml(formatTime(key.expires_ms))}` : ""}</small></div><span class="pill ${key.revoked ? "warning" : "success"}">${key.revoked ? "Revoked" : "Active"}</span>${key.revoked ? "" : `<button data-revoke-key="${escapeHtml(key.id)}">Revoke</button>`}</div>`).join("") || '<p class="muted">No scoped keys have been minted.</p>'}`;
  bindActions(node, "[data-revoke-key]", async (event) => {
    if (!confirm("Revoke this scoped key now? Connected clients using it will stop working.")) return;
    try { await api(`/admin/run-keys/${encodeURIComponent(event.currentTarget.dataset.revokeKey)}`, { method: "DELETE" }); toast("Key revoked."); await loadCredentials(); }
    catch (error) { toast(error.message, "danger"); }
  });
  $('[data-revoke-all-keys]', node)?.addEventListener("click", async () => {
    if (!confirm("Revoke every active model/run key? Harness keys will remain connected.")) return;
    try { await api("/admin/run-keys/revoke-all", { method: "POST" }); toast("All model/run keys revoked."); await loadCredentials(); }
    catch (error) { toast(error.message, "danger"); }
  });
  $('[data-clear-revoked]', node)?.addEventListener("click", async () => {
    try { await api("/admin/run-keys/revoked", { method: "DELETE" }); toast("Revoked key records cleared."); await loadCredentials(); }
    catch (error) { toast(error.message, "danger"); }
  });
}

function renderCredentialInventory(credentials) {
  const outbound = credentials.outbound || [];
  $("#credential-inventory").innerHTML = `<div class="credential-card"><strong>Admin key</strong><span class="pill ${credentials.inbound?.admin_key?.present ? "success" : "warning"}">${credentials.inbound?.admin_key?.present ? "Present" : "Missing"}</span><small>Control-plane access; never displayed here</small></div>${outbound.map((item) => `<div class="credential-card"><strong>${escapeHtml(item.provider || item.name || item.credential_id || item.kind)}</strong><span class="pill ${item.present && item.active ? "success" : "warning"}">${escapeHtml(item.present ? (item.active ? "Active" : "Inactive") : "Missing")}</span><small>${escapeHtml(item.identity || item.source || item.kind || "Outbound credential")}</small></div>`).join("") || '<p class="muted">No outbound provider credentials configured.</p>'}`;
}

async function loadCredentials() {
  const [credentials, keys] = await Promise.all([api("/admin/credentials"), api("/admin/run-keys?all=1")]);
  renderRunKeys(keys.run_keys || credentials.inbound?.run_keys || []);
  renderCredentialInventory(credentials);
}

function darioCacheRows(caches) {
  return caches.map((cache) => `<div class="cache-row"><div><strong>${escapeHtml(cache.key || cache.id)}</strong><small>${escapeHtml(cache.model || cache.provider || "Prompt cache")} · last used ${escapeHtml(cache.last_used_at || formatTime(cache.last_used_ms))}</small></div><span>${escapeHtml(cache.size_bytes === undefined ? "" : `${formatInteger(cache.size_bytes)} bytes`)}</span><button class="danger-button" data-delete-cache="${escapeHtml(cache.key || cache.id)}">Delete</button></div>`).join("") || '<p class="muted">No prompt caches recorded.</p>';
}

async function loadDario() {
  const [status, caches] = await Promise.all([api("/admin/dario"), api("/admin/dario/prompt-caches")]);
  state.dario = status;
  const notApplicable = status.health === "not-applicable";
  $("#dario-status").className = notApplicable ? "dario-page-empty settings-card" : "stat-grid";
  $("#dario-details").hidden = notApplicable;
  $("#ping-dario").hidden = notApplicable;
  if (notApplicable) {
    $("#dario-status").innerHTML = darioEmptyState();
    return;
  }
  $("#dario-status").innerHTML = statCards([
    ["Health", status.health],
    ["Generation", status.active_generation_id],
    ["Route", status.route_enabled ? "Enabled" : "Disabled"],
    ["Credentials", status.anthropic_credentials_present ? "Present" : "Missing"],
  ]);
  $("#dario-routing").innerHTML = facts([["Health reason", status.health_reason], ["Should be healthy", status.should_be_healthy], ["Active generation", status.active_generation_id], ["Issue", status.issue?.message]]);
  $("#dario-generations").innerHTML = (status.generations || []).map((generation) => `<tr><td>${escapeHtml(generation.id)}</td><td>${escapeHtml(generation.version)}</td><td>${escapeHtml(generation.phase || generation.status)}</td><td>${escapeHtml(generation.port)}</td><td>${escapeHtml(generation.pid)}</td><td>${escapeHtml(generation.busy ?? generation.in_flight)}</td><td>${escapeHtml(generation.age_s === undefined ? formatTime(generation.started_ms) : formatAge(generation.age_s))}</td></tr>`).join("") || '<tr><td colspan="7">No active generations.</td></tr>';
  const cacheList = caches.prompt_caches || status.prompt_caches || [];
  $("#dario-caches").innerHTML = darioCacheRows(cacheList);
  bindActions($("#dario-caches"), "[data-delete-cache]", deleteDarioCache);
  $("#dario-runtime").textContent = JSON.stringify({ health: status.health, generation_health: status.generation_health, active_generation_id: status.active_generation_id, route_enabled: status.route_enabled, issue: status.issue, generations: status.generations || [] }, null, 2);
}

async function pingDario() {
  const button = $("#ping-dario");
  button.disabled = true;
  try { const result = await api("/admin/dario/ping", { method: "POST" }); toast(`Dario ping: ${result.health}.`); await loadDario(); }
  catch (error) { toast(error.message, "danger"); }
  finally { button.disabled = false; }
}

async function deleteDarioCache(event) {
  const key = event.currentTarget.dataset.deleteCache;
  if (!confirm(`Delete prompt cache ${key}?`)) return;
  try { await api(`/admin/dario/prompt-caches/${encodeURIComponent(key)}`, { method: "DELETE" }); toast("Prompt cache deleted."); await loadDario(); }
  catch (error) { toast(error.message, "danger"); }
}

function writableRule(rule) {
  const { api_version, built_in, hit_count, last_matched_ms, ...payload } = rule;
  return payload;
}

function populatedEntries(value) {
  return Object.entries(value || {}).filter(([, item]) => Array.isArray(item) ? item.length : item !== null && item !== undefined && item !== false && item !== "");
}

function ruleExplanation(rule) {
  const conditions = populatedEntries(rule.when).map(([name, value]) => `${name.replaceAll("_", " ")}: ${Array.isArray(value) ? value.join(", ") : JSON.stringify(value)}`);
  if (rule.expression) conditions.push("advanced expression");
  const actions = populatedEntries(rule.then || rule.action).map(([name, value]) => {
    if (name === "continue") return "continue";
    if (name === "return_original") return "return the original response";
    if (name === "retry_same_route") return `retry same route (${value.reason || "next eligible account"})`;
    if (name === "reroute") return `reroute${value.model ? ` to ${value.model}` : value.equivalent_class ? ` to equivalent ${value.equivalent_class}` : ""}`;
    return name.replaceAll("_", " ");
  });
  return `${conditions.length ? `When ${conditions.join("; ")}` : "For every matching hook"} → ${actions.join(", ") || "no action"}.`;
}

function fixtureOptions(fixtures) {
  return fixtures.map((fixture) => `<option value="${escapeHtml(fixture.name)}">${escapeHtml(fixture.name)} · ${escapeHtml(fixture.provider)} ${escapeHtml(fixture.status)}</option>`).join("");
}

function renderMiddlewareRule(rule) {
  return `<article class="rule-card" data-rule-card="${escapeHtml(rule.id)}"><div class="card-heading"><div><h3>${escapeHtml(rule.name)}</h3><code>${escapeHtml(rule.id)}</code></div><button data-rule-toggle="${escapeHtml(rule.id)}" aria-pressed="${rule.enabled}">${rule.enabled ? "Disable" : "Enable"}</button></div><p>${escapeHtml(rule.description || "No description provided.")}</p><p class="muted">${escapeHtml(ruleExplanation(rule))}</p><p class="micro">Priority ${escapeHtml(rule.priority)} · ${escapeHtml(rule.hook)} · ${escapeHtml(rule.hit_count || 0)} matches${rule.last_matched_ms ? ` · last ${escapeHtml(formatTime(rule.last_matched_ms))}` : ""}</p><details><summary>Readable rule source</summary><pre>${escapeHtml(JSON.stringify(writableRule(rule), null, 2))}</pre></details><form class="form-grid" data-rule-test="${escapeHtml(rule.id)}"><label>Saved error fixture<select name="fixture" required>${fixtureOptions(state.fixtures)}</select></label><button ${state.fixtures.length ? "" : "disabled"}>Run dry test</button></form><div data-rule-result="${escapeHtml(rule.id)}"></div></article>`;
}

async function loadMiddleware() {
  const [middleware, fixtureData, activity, protection] = await Promise.all([
    api("/admin/middleware"), api("/admin/fixtures"), api("/admin/middleware/activity?limit=8"), api("/admin/protection"),
  ]);
  state.middleware = middleware;
  state.fixtures = fixtureData.fixtures || [];
  const settings = middleware.settings || {};
  const form = $("#middleware-settings-form");
  ["enabled"].forEach((name) => { form.elements[name].checked = Boolean(settings[name]); });
  ["fail_mode", "max_attempts", "error_body_limit_bytes", "default_script_timeout_ms", "default_script_max_operations"].forEach((name) => { form.elements[name].value = settings[name] ?? ""; });
  const protectionForm = $("#protection-form");
  protectionForm.elements.enabled.checked = Boolean(protection.enabled);
  protectionForm.elements.reroute_on_auth.checked = Boolean(protection.reroute_on_auth);
  protectionForm.elements.retries.value = protection.retries;
  protectionForm.elements.auto_return.checked = Boolean(protection.auto_return);
  protectionForm.elements.equivalencies.value = JSON.stringify(protection.equivalencies || {}, null, 2);
  const enabled = (middleware.rules || []).filter((rule) => rule.enabled).length;
  $("#middleware-summary").innerHTML = statCards([["Generation", middleware.generation], ["Enabled", `${enabled}/${middleware.rules?.length || 0}`], ["Fixtures", state.fixtures.length], ["Active leases", middleware.leases?.length || 0]]);
  const errors = middleware.errors || [];
  const errorPanel = $("#middleware-errors");
  errorPanel.hidden = !errors.length;
  errorPanel.innerHTML = errors.length ? `<strong>Runtime errors</strong><ul>${errors.map((error) => `<li>${escapeHtml(error)}</li>`).join("")}</ul>` : "";
  const rules = $("#middleware-rules");
  rules.innerHTML = (middleware.rules || []).map(renderMiddlewareRule).join("") || '<article class="settings-card"><p class="muted">No middleware rules installed.</p></article>';
  bindActions(rules, "[data-rule-toggle]", (event) => setRuleEnabled(event.currentTarget.dataset.ruleToggle, event.currentTarget.getAttribute("aria-pressed") !== "true"));
  $$('[data-rule-test]', rules).forEach((ruleForm) => ruleForm.addEventListener("submit", (event) => dryRunRule(event, ruleForm.dataset.ruleTest)));
  $("#middleware-activity").innerHTML = (activity.events || []).map((item) => `<button class="mini-row" data-activity-trace="${escapeHtml(item.id)}"><div><strong>${escapeHtml(item.requested_model || item.id)}</strong><small>${escapeHtml(item.harness || "unknown")} · ${escapeHtml(item.status)} · ${escapeHtml(formatTime(item.ts_ms))}</small></div></button>`).join("") || '<p class="muted">No recent middleware activity.</p>';
  bindActions($("#middleware-activity"), "[data-activity-trace]", (event) => { selectView("traces", true); openTrace(event.currentTarget.dataset.activityTrace); });
  $("#middleware-leases").innerHTML = (middleware.leases || []).map((lease) => `<div class="mini-row"><div><strong>${escapeHtml(lease.harness)} · ${escapeHtml(lease.session_id)}</strong><small>${escapeHtml(lease.original_model)} → ${escapeHtml(lease.target?.model || lease.target?.provider || JSON.stringify(lease.target))} · expires ${escapeHtml(formatTime(lease.expires_ms))}</small></div><button data-clear-lease="${escapeHtml(lease.id)}">Clear</button></div>`).join("") || '<p class="muted">No active session routes.</p>';
  bindActions($("#middleware-leases"), "[data-clear-lease]", clearMiddlewareLease);
}

async function saveMiddlewareSettings(event) {
  event.preventDefault();
  const form = event.currentTarget;
  const payload = {
    enabled: form.elements.enabled.checked,
    fail_mode: form.elements.fail_mode.value,
    max_attempts: Number(form.elements.max_attempts.value),
    error_body_limit_bytes: Number(form.elements.error_body_limit_bytes.value),
    default_script_timeout_ms: Number(form.elements.default_script_timeout_ms.value),
    default_script_max_operations: Number(form.elements.default_script_max_operations.value),
  };
  try { await api("/admin/middleware/settings", { method: "PUT", body: JSON.stringify(payload) }); toast("Middleware runtime saved."); await loadMiddleware(); }
  catch (error) { toast(error.message, "danger"); }
}

async function saveProtection(event) {
  event.preventDefault();
  const form = event.currentTarget;
  let equivalencies;
  try { equivalencies = JSON.parse(form.elements.equivalencies.value || "{}"); }
  catch { toast("Model equivalencies must be valid JSON.", "danger"); return; }
  const payload = { enabled: form.elements.enabled.checked, reroute_on_auth: form.elements.reroute_on_auth.checked, retries: Number(form.elements.retries.value), auto_return: form.elements.auto_return.checked, equivalencies };
  try { await api("/admin/protection", { method: "PUT", body: JSON.stringify(payload) }); toast("Failover protection saved."); await loadMiddleware(); }
  catch (error) { toast(error.message, "danger"); }
}

async function reloadMiddleware() {
  try { await api("/admin/middleware/reload", { method: "POST" }); toast("Middleware reloaded from disk."); await loadMiddleware(); }
  catch (error) { toast(error.message, "danger"); }
}

async function setRuleEnabled(id, enabled) {
  const rule = state.middleware?.rules?.find((candidate) => candidate.id === id);
  if (!rule) return;
  try { await api(`/admin/middleware/rules/${encodeURIComponent(id)}`, { method: "PUT", body: JSON.stringify({ ...writableRule(rule), enabled }) }); toast(`Rule ${enabled ? "enabled" : "disabled"}.`); await loadMiddleware(); }
  catch (error) { toast(error.message, "danger"); }
}

async function dryRunRule(event, id) {
  event.preventDefault();
  const output = $$('[data-rule-result]').find((node) => node.dataset.ruleResult === id);
  output.textContent = "Evaluating fixture without changing live routing…";
  try {
    const fixture_name = new FormData(event.currentTarget).get("fixture");
    const result = await api("/admin/middleware/test", { method: "POST", body: JSON.stringify({ middleware_id: id, fixture_name }) });
    const decision = result.decision?.decision || "continue";
    const matched = (result.records || []).filter((record) => record.state === "matched").map((record) => record.rule_id).join(", ") || "none";
    output.innerHTML = `<div class="inline-message success"><strong>Decision: ${escapeHtml(decision.replaceAll("_", " "))}</strong><p>Matched: ${escapeHtml(matched)} · body inspection ${result.body_inspection_required ? "required" : "not required"}</p></div><details><summary>Full dry-run result</summary><pre>${escapeHtml(JSON.stringify(result, null, 2))}</pre></details>`;
  } catch (error) { inlineMessage(output, error.message); }
}

async function clearMiddlewareLease(event) {
  try { await api(`/admin/middleware/leases/${encodeURIComponent(event.currentTarget.dataset.clearLease)}`, { method: "DELETE" }); toast("Session route cleared."); await loadMiddleware(); }
  catch (error) { toast(error.message, "danger"); }
}

function telegramPayload(form) {
  const values = new FormData(form);
  return {
    format: "telegram",
    token: String(values.get("token") || "").trim(),
    chat_id: String(values.get("chat_id") || "").trim(),
    name: String(values.get("name") || "").trim(),
    min_level: values.get("min_level"),
    categories: form.elements.reauth_only.checked ? ["reauth"] : [],
    allow_commands: form.elements.allow_commands.checked,
  };
}

function notificationResult(message, kind = "success") {
  inlineMessage($("#notification-action"), message, kind);
}

async function notificationAction(action) {
  const form = $("#notification-form");
  const payload = telegramPayload(form);
  if (!payload.token) { notificationResult("Enter the Telegram bot token first.", "danger"); return; }
  const path = action === "validate" ? "/admin/notifications/validate" : action === "discover" ? "/admin/notifications/discover-chat" : "/admin/notifications/test";
  const body = action === "test" ? payload : { format: "telegram", token: payload.token };
  try {
    const result = await api(path, { method: "POST", body: JSON.stringify(body) });
    if (action === "discover") {
      const chats = result.chats || [];
      if (chats.length === 1) form.elements.chat_id.value = chats[0].chat_id;
      notificationResult(chats.length ? `Found ${chats.length} chat(s): ${chats.map((chat) => `${chat.chat_name} (${chat.chat_id})`).join(", ")}` : "No chats found. Message the bot once, then try again.", chats.length ? "success" : "warning");
    } else if (action === "validate") notificationResult(result.ok ? `Validated @${result.bot_username}.` : (result.error || "Token validation failed."), result.ok ? "success" : "danger");
    else notificationResult(`Test completed: ${JSON.stringify(result.channels || [])}`, "success");
  } catch (error) { notificationResult(error.message, "danger"); }
}

async function saveNotification(event) {
  event.preventDefault();
  const payload = telegramPayload(event.currentTarget);
  try {
    const result = await api("/admin/notifications", { method: "POST", body: JSON.stringify(payload) });
    event.currentTarget.elements.token.value = "";
    notificationResult(result.warning || "Telegram channel saved.", result.warning ? "warning" : "success");
    await loadNotifications();
  } catch (error) { notificationResult(error.message, "danger"); }
}

async function loadNotifications() {
  const [settings, log] = await Promise.all([api("/admin/notifications"), api("/admin/notifications/log?limit=30")]);
  const channels = settings.channels || [];
  const channelNode = $("#notification-channels");
  channelNode.innerHTML = channels.map((channel) => `<div class="channel-row"><div><strong>${escapeHtml(channel.name || channel.bot_username || channel.id)}</strong><small>${escapeHtml(channel.format)} · ${escapeHtml(channel.chat_id || "No chat")} · ${escapeHtml(channel.min_level || "info")}</small></div><label class="toggle-line"><input data-channel-commands="${escapeHtml(channel.id)}" type="checkbox" ${channel.allow_commands ? "checked" : ""}>Commands</label><button data-test-channel="${escapeHtml(channel.id)}">Test</button><button class="danger-button" data-delete-channel="${escapeHtml(channel.id)}">Delete</button></div>`).join("") || '<p class="muted">No notification channels configured.</p>';
  bindActions(channelNode, "[data-test-channel]", testSavedChannel);
  bindActions(channelNode, "[data-delete-channel]", deleteNotificationChannel);
  $$('[data-channel-commands]', channelNode).forEach((input) => input.addEventListener("change", toggleNotificationCommands));
  $("#notification-log").innerHTML = (log.messages || []).map((message) => `<div class="mini-row"><div><strong>${escapeHtml(message.category || message.direction || "notification")}</strong><small>${escapeHtml(formatTime(message.ts_ms))} · ${escapeHtml(message.channel_id || "all channels")}</small><p>${escapeHtml(message.message || message.text || message.error || "")}</p></div><span class="pill ${message.ok === false ? "warning" : "neutral"}">${message.ok === false ? "Failed" : "Sent"}</span></div>`).join("") || '<p class="muted">No recent notification messages.</p>';
}

async function testSavedChannel(event) {
  try { await api("/admin/notifications/test", { method: "POST", body: JSON.stringify({ channel_id: event.currentTarget.dataset.testChannel }) }); toast("Notification test sent."); await loadNotifications(); }
  catch (error) { toast(error.message, "danger"); }
}

async function toggleNotificationCommands(event) {
  try { await api("/admin/notifications/commands", { method: "POST", body: JSON.stringify({ channel_id: event.currentTarget.dataset.channelCommands, allow_commands: event.currentTarget.checked }) }); toast("Telegram command policy saved."); }
  catch (error) { toast(error.message, "danger"); await loadNotifications(); }
}

async function deleteNotificationChannel(event) {
  if (!confirm("Delete this notification channel?")) return;
  try { await api(`/admin/notifications/${encodeURIComponent(event.currentTarget.dataset.deleteChannel)}`, { method: "DELETE" }); toast("Notification channel deleted."); await loadNotifications(); }
  catch (error) { toast(error.message, "danger"); }
}

/* 7. Provider authentication flows. */
function loginFlowNodes() {
  return [$("#provider-login-flow"), $("#login-flow")].filter(Boolean);
}

function setLoginFlow(html) {
  loginFlowNodes().forEach((node) => { node.hidden = false; node.innerHTML = html; });
}

function loginProvider(provider) {
  const raw = String(provider || "Provider");
  const known = LOGIN_PROVIDERS[raw.toLowerCase()];
  return known ? { name: known[0], vendor: known[1], monogram: known[2], accent: known[3], key: raw.toLowerCase() } : { name: raw, vendor: "Provider", monogram: raw.trim().charAt(0).toUpperCase() || "P", accent: "default", key: raw.toLowerCase() };
}

function selectLoginText(node) {
  try {
    const selection = window.getSelection();
    const range = document.createRange();
    range.selectNodeContents(node);
    selection.removeAllRanges();
    selection.addRange(range);
  } catch { /* Selection is only a last-resort clipboard fallback. */ }
}

async function copyLoginText(text, selectionNode = null) {
  try {
    if (navigator.clipboard?.writeText) {
      try { await navigator.clipboard.writeText(text); return; }
      catch { /* Plain HTTP and older browsers may deny the Clipboard API. */ }
    }
    const helper = document.createElement("textarea");
    helper.value = text;
    helper.setAttribute("readonly", "");
    helper.className = "clipboard-helper";
    document.body.append(helper);
    helper.select();
    let copied = false;
    try { copied = document.execCommand("copy"); } catch { /* Fall through to selecting visible text. */ }
    helper.remove();
    if (!copied && selectionNode) selectLoginText(selectionNode);
  } catch { if (selectionNode) selectLoginText(selectionNode); }
}

function showLoginCopied(button, label = null) {
  const icon = $("[data-login-copy-icon]", button);
  const originalIcon = icon?.textContent;
  const originalLabel = label?.textContent;
  button.classList.add("copied");
  if (icon) icon.textContent = "✓";
  if (label) label.textContent = "Copied!";
  clearTimeout(button.loginCopiedTimer);
  button.loginCopiedTimer = setTimeout(() => {
    if (!button.isConnected) return;
    button.classList.remove("copied");
    if (icon) icon.textContent = originalIcon;
    if (label) label.textContent = originalLabel;
  }, 1800);
}

function bindLoginCard(node, session, reauthProvider) {
  $("[data-login-copy-link]", node)?.addEventListener("click", (event) => {
    const button = event.currentTarget;
    showLoginCopied(button, $("[data-login-copy-label]", button));
    copyLoginText(button.dataset.loginCopyLink);
  });
  $("[data-login-copy-code]", node)?.addEventListener("click", (event) => {
    const button = event.currentTarget;
    showLoginCopied(button);
    copyLoginText(button.dataset.loginCopyCode, $("[data-login-code]", node));
  });
  $("[data-login-complete]", node)?.addEventListener("submit", (event) => reauthProvider ? completeReauth(event, reauthProvider) : completeLogin(event, session.login_id));
}

async function startLogin(provider) {
  clearTimeout(state.loginPoll);
  setLoginFlow("Starting secure provider login…");
  try {
    const session = await api("/admin/auth/login/start", { method: "POST", body: JSON.stringify({ provider, auto_identity: true }) });
    renderLogin(session);
    if (session.state === "pending") pollLogin(session.login_id);
  } catch (error) { setLoginFlow(`<strong>Could not start login</strong><p class="inline-message danger">${escapeHtml(error.message)}</p>`); }
}

async function startReauth(provider, accountId) {
  clearTimeout(state.loginPoll);
  selectView(`providers/${providerTabId(provider)}`, true);
  setLoginFlow("Starting secure re-authentication…");
  $("#provider-login-flow").scrollIntoView({ behavior: "smooth" });
  try {
    const session = await api("/admin/auth/reauth/start", { method: "POST", body: JSON.stringify({ provider, account_id: accountId, notify: false }) });
    renderLogin(session, provider);
    if (session.state === "pending") pollLogin(session.login_id, provider);
  } catch (error) { setLoginFlow(`<strong>Could not start re-authentication</strong><p class="inline-message danger">${escapeHtml(error.message)}</p>`); }
}

function renderLogin(session, reauthProvider = null) {
  const target = safeUrl(session.verification_uri_complete || session.authorize_url || session.verification_uri);
  const provider = loginProvider(session.provider || reauthProvider);
  const pending = session.state === "pending";
  const paste = session.mode === "paste";
  const openStep = target ? `<section class="provider-auth-step"><div class="provider-auth-step-title"><span class="provider-auth-step-badge">1</span><span>Open the authorization page.</span></div><div class="provider-auth-actions"><a class="provider-auth-action" href="${escapeHtml(target)}" target="_blank" rel="noopener"><span aria-hidden="true">↗</span>Open in Browser</a><button type="button" class="provider-auth-action" data-login-copy-link="${escapeHtml(target)}"><span data-login-copy-icon aria-hidden="true">⧉</span><span data-login-copy-label>Copy Link</span></button></div></section>` : "";
  const codeStep = session.user_code ? `<section class="provider-auth-step"><div class="provider-auth-step-title"><span class="provider-auth-step-badge">2</span><span>Enter this code when ${escapeHtml(provider.name)} asks for it:</span></div><div class="provider-auth-code-row"><div class="provider-auth-code" data-login-code tabindex="0">${escapeHtml(session.user_code)}</div><button type="button" class="provider-auth-code-copy" data-login-copy-code="${escapeHtml(session.user_code)}" aria-label="Copy authorization code"><span data-login-copy-icon aria-hidden="true">⧉</span></button></div></section>` : "";
  const pasteStep = paste && pending ? `<section class="provider-auth-step"><div class="provider-auth-step-title"><span class="provider-auth-step-badge">2</span><span>Paste the authorization code or callback URL.</span></div><form data-login-complete class="provider-auth-paste"><label><span>Authorization code or callback URL</span><input name="input" required autocomplete="off"></label><button type="submit">Complete login</button></form></section>` : "";
  const status = session.error ? `<div class="provider-auth-status danger"><span class="provider-auth-status-icon" aria-hidden="true">!</span><span>${escapeHtml(session.error)}</span></div>` : session.state === "done" ? `<div class="provider-auth-status success"><span class="provider-auth-status-icon" aria-hidden="true">✓</span><span>${escapeHtml(session.success_message || "Authorization complete — account connected.")}</span></div>` : pending ? '<div class="provider-auth-status waiting"><span class="spinner" aria-hidden="true"></span><span>Waiting for authorization — keep this window open.</span></div>' : `<div class="provider-auth-status danger"><span class="provider-auth-status-icon" aria-hidden="true">!</span><span>Authorization ${escapeHtml(session.state || "failed")}.</span></div>`;
  const html = `<div class="provider-auth-card" data-login-accent="${escapeHtml(provider.accent)}"><div class="provider-auth-body"><header class="provider-auth-identity">${providerLogoTile(provider.key, provider.monogram, "provider-auth-logo")}<div class="provider-auth-name"><strong>${escapeHtml(provider.name)}</strong><span>by ${escapeHtml(provider.vendor)}</span></div><div class="provider-auth-pill"><i></i><span>${paste ? "Paste code" : "OAuth Device Flow"}</span></div></header><div class="provider-auth-divider"></div>${openStep}${codeStep}${pasteStep}${status}</div></div>`;
  setLoginFlow(html);
  loginFlowNodes().forEach((node) => bindLoginCard(node, session, reauthProvider));
}

async function completeLogin(event, id) {
  event.preventDefault();
  const input = new FormData(event.currentTarget).get("input");
  try {
    const session = await api("/admin/auth/login/complete", { method: "POST", body: JSON.stringify({ login_id: id, input }) });
    renderLogin(session);
    if (session.state === "done") { await loadAccounts(); await loadProviders(); }
  } catch (error) { toast(error.message, "danger"); }
}

async function completeReauth(event, provider) {
  event.preventDefault();
  const input = new FormData(event.currentTarget).get("input");
  try {
    const result = await api("/admin/auth/reauth/submit", { method: "POST", body: JSON.stringify({ provider, input }) });
    if (result.ok === false) throw new Error(result.error || "Re-authentication did not complete");
    renderLogin({ provider, mode: "paste", state: "done", success_message: "Credential updated. Run ping checks to verify it." }, provider);
    await loadProviders();
  } catch (error) { toast(error.message, "danger"); }
}

function pollLogin(id, reauthProvider = null, attempts = 0) {
  clearTimeout(state.loginPoll);
  if (attempts >= 180) { setLoginFlow('<p class="inline-message warning">Authorization timed out. Start again when ready.</p>'); return; }
  state.loginPoll = setTimeout(async () => {
    try {
      const session = await api(`/admin/auth/login/${encodeURIComponent(id)}`);
      renderLogin(session, reauthProvider);
      if (session.state === "pending") pollLogin(id, reauthProvider, attempts + 1);
      else await loadProviders();
    } catch (error) { setLoginFlow(`<p class="inline-message danger">${escapeHtml(error.message)}</p>`); }
  }, 2000);
}

/* 8. Middleware dry-run flow is implemented with its settings group above. */

/* 9. Trace summaries, metadata, lazy bodies, and paged transcripts. */
function traceQuery(append) {
  const params = new URLSearchParams({ limit: String(TRACE_PAGE_SIZE), ...state.traceFilters });
  if (append && state.traceCursor) {
    params.set("before_ms", state.traceCursor.before_ms);
    params.set("before_id", state.traceCursor.before_id);
  }
  return params;
}

function traceShortId(value) {
  const text = String(value || "");
  return text.length > 10 ? text.slice(0, 8) : text || "—";
}

function traceMetaValue(value) {
  if (value === null || value === undefined || value === "") return "—";
  if (typeof value === "object") return JSON.stringify(value);
  return String(value);
}

function formatDurationMs(value) {
  if (value === null || value === undefined || !Number.isFinite(Number(value))) return "—";
  const milliseconds = Number(value);
  return milliseconds < 1000 ? `${Math.round(milliseconds)}ms` : `${(milliseconds / 1000).toFixed(1)}s`;
}

function traceRelativeTime(value) {
  if (!value) return "—";
  const seconds = Math.max(0, (Date.now() - finite(value)) / 1000);
  return seconds < 10 ? "now" : `${formatAge(seconds)} ago`;
}

function traceStatusKind(status, error = null) {
  const code = Number(status);
  if (error || code >= 500) return "error";
  if (code >= 400) return "client-error";
  if (code >= 200 && code < 300) return "success";
  return "neutral";
}

function traceStatusChip(status, error = null) {
  const label = status === null || status === undefined ? (error ? "Error" : "Pending") : String(status);
  return `<span class="trace-status-chip ${traceStatusKind(status, error)}"><i></i>${escapeHtml(label)}</span>`;
}

function traceModelChip(model) {
  return `<span class="trace-model-chip">${escapeHtml(model || "unknown model")}</span>`;
}

function traceMetricChip(label, value, tone = "") {
  return `<span class="trace-metric-chip ${escapeHtml(tone)}"><small>${escapeHtml(label)}</small><b>${escapeHtml(traceMetaValue(value))}</b></span>`;
}

function traceTokenCount(value) {
  return value === null || value === undefined ? "— tok" : `${formatInteger(value)} tok`;
}

function setTraceSelectOptions(name, values, placeholder) {
  const select = $(`#trace-filters [name="${name}"]`);
  const current = select.value || state.traceFilters[name] || "";
  const options = [...new Set(values.filter(Boolean).map(String))].sort((left, right) => left.localeCompare(right));
  if (current && !options.includes(current)) options.unshift(current);
  select.innerHTML = `<option value="">${escapeHtml(placeholder)}</option>${options.map((value) => `<option value="${escapeHtml(value)}">${escapeHtml(value)}</option>`).join("")}`;
  select.value = current;
}

function populateTraceFilterOptions() {
  const providerValues = [
    ...state.providers.map((entry) => entry.provider),
    ...state.accounts.map((entry) => entry.provider),
    ...state.traceRows.map((entry) => entry.provider),
  ];
  const harnessValues = [
    ...state.harnesses.map((entry) => entry.name),
    ...state.traceRows.map((entry) => entry.harness),
  ];
  setTraceSelectOptions("provider", providerValues, "Any provider");
  setTraceSelectOptions("harness", harnessValues, "Any harness");
}

function traceSessionRows() {
  const aggregates = new Map(state.traceSessions.map((session) => [session.session_id, session]));
  const rows = new Map();
  state.traceRows.forEach((trace) => {
    const id = trace.session_id || `trace:${trace.id}`;
    if (!rows.has(id)) rows.set(id, {
      id,
      sessionId: trace.session_id || null,
      traces: [],
      aggregate: trace.session_id ? aggregates.get(trace.session_id) || null : null,
      parentId: null,
      children: [],
      nestedBy: null,
    });
    rows.get(id).traces.push(trace);
  });
  rows.forEach((row) => {
    row.latest = row.traces.slice().sort((left, right) => finite(right.ts_request_ms) - finite(left.ts_request_ms))[0];
    row.runId = row.aggregate?.run_id || row.latest.run_id || null;
    const verifiedParent = row.aggregate?.parent_session_id;
    if (verifiedParent && rows.has(verifiedParent)) {
      row.parentId = verifiedParent;
      row.nestedBy = "lineage";
    }
  });

  const byRun = new Map();
  rows.forEach((row) => {
    if (!row.sessionId || !row.runId) return;
    if (!byRun.has(row.runId)) byRun.set(row.runId, []);
    byRun.get(row.runId).push(row);
  });
  byRun.forEach((runRows) => {
    if (runRows.length < 2) return;
    const possibleRoots = runRows.filter((row) => !row.parentId);
    if (possibleRoots.length < 2) return;
    possibleRoots.sort((left, right) => sessionSortValue(left, "time", true) - sessionSortValue(right, "time", true) || left.id.localeCompare(right.id));
    const root = possibleRoots[0];
    possibleRoots.slice(1).forEach((row) => {
      row.parentId = root.id;
      row.nestedBy = "run";
    });
  });

  rows.forEach((row) => {
    if (row.parentId && rows.has(row.parentId) && row.parentId !== row.id) rows.get(row.parentId).children.push(row);
  });
  return [...rows.values()].filter((row) => !row.parentId || !rows.has(row.parentId));
}

function sessionSortValue(row, key, ascendingTime = false) {
  const aggregate = row.aggregate || {};
  if (key === "session") return row.sessionId || row.latest.id;
  if (key === "turns") return aggregate.trace_count ?? row.traces.length;
  if (key === "cost") return aggregate.total_cost_usd ?? -1;
  if (key === "duration") return aggregate.duration_ms ?? row.latest.latency_ms ?? -1;
  if (key === "status") return aggregate.status_label || traceStatusKind(row.latest.status, row.latest.error);
  const time = aggregate.last_ts_ms ?? row.latest.ts_request_ms ?? 0;
  return ascendingTime ? finite(aggregate.first_ts_ms ?? time) : finite(time);
}

function sortedSessionRows(rows) {
  const { key, direction } = state.traceSort;
  const multiplier = direction === "asc" ? 1 : -1;
  return rows.slice().sort((left, right) => {
    const a = sessionSortValue(left, key);
    const b = sessionSortValue(right, key);
    const compared = typeof a === "string" || typeof b === "string"
      ? String(a).localeCompare(String(b), undefined, { numeric: true, sensitivity: "base" })
      : finite(a) - finite(b);
    return compared * multiplier || left.id.localeCompare(right.id);
  });
}

function sessionStatusMarkup(row) {
  const label = row.aggregate?.status_label;
  if (label) {
    const kind = label === "Error" ? "error" : label === "Done" ? "success" : "neutral";
    return `<span class="trace-status-chip ${kind}"><i></i>${escapeHtml(label)}</span>`;
  }
  return traceStatusChip(row.latest.status, row.latest.error);
}

function sessionRowMarkup(row, nested = false) {
  const aggregate = row.aggregate || {};
  const trace = row.latest;
  const active = row.sessionId ? state.selectedSessionId === row.sessionId : state.selectedTraceId === trace.id;
  const error = aggregate.status_label === "Error" || traceStatusKind(trace.status, trace.error) === "error";
  const children = sortedSessionRows(row.children);
  const expanded = state.traceExpandedSessions.has(row.id) || !state.traceExpandedSessions.has(`closed:${row.id}`);
  const provider = aggregate.providers?.[0] || trace.provider;
  const harness = aggregate.harness || trace.harness;
  const model = aggregate.models?.[0] || trace.model || trace.served_model;
  const rowTarget = row.sessionId ? `data-session-id="${escapeHtml(row.sessionId)}"` : `data-trace-id="${escapeHtml(trace.id)}"`;
  const treeControl = nested
    ? children.length
      ? `<button class="trace-tree-toggle trace-tree-elbow" data-session-toggle="${escapeHtml(row.id)}" aria-label="${expanded ? "Collapse" : "Expand"} subagent sessions">${expanded ? "⌄" : "›"}</button>`
      : '<span class="trace-tree-elbow" aria-hidden="true"></span>'
    : children.length
      ? `<button class="trace-tree-toggle" data-session-toggle="${escapeHtml(row.id)}" aria-label="${expanded ? "Collapse" : "Expand"} subagent sessions">${expanded ? "⌄" : "›"}</button>`
      : '<span class="trace-tree-spacer"></span>';
  const turns = aggregate.trace_count ?? row.traces.length;
  const cost = aggregate.total_cost_usd;
  const duration = aggregate.duration_ms ?? trace.latency_ms;
  const time = aggregate.last_ts_ms ?? trace.ts_request_ms;
  const typeLabel = nested ? (row.nestedBy === "lineage" ? "subagent" : "same run") : harnessDisplayName(harness);
  return `<section class="trace-session-group ${active ? "active" : ""} ${error ? "error" : ""} ${nested ? "nested-session" : ""}">
    <div class="trace-session-row" data-session-grid ${rowTarget} tabindex="0" role="button">
      <span class="trace-session-identity">${treeControl}<span class="trace-row-brands">${providerLogoTile(provider, provider, "trace-mini-brand")}${harnessLogoTile(harness, harness, "trace-mini-brand")}</span><span class="trace-row-copy"><code>${escapeHtml(traceShortId(row.sessionId || trace.id))}</code><small>${escapeHtml(`${providerDisplayName(provider)} · ${typeLabel}`)}</small></span>${model ? traceModelChip(model) : ""}</span>
      <span data-session-column="turns" class="trace-table-number">${escapeHtml(turns)}</span>
      <span data-session-column="cost" class="trace-table-number">${cost === null || cost === undefined ? "—" : escapeHtml(formatMoney(cost))}</span>
      <span data-session-column="duration" class="trace-table-number">${escapeHtml(formatDurationMs(duration))}</span>
      <span data-session-column="status">${sessionStatusMarkup(row)}</span>
      <time data-session-column="time">${escapeHtml(traceRelativeTime(time))}</time>
      ${row.sessionId ? `<button class="trace-row-menu" data-session-menu="${escapeHtml(row.sessionId)}" aria-label="Session actions">⋯</button>` : '<span></span>'}
    </div>
    ${children.length && expanded && state.traceShowSubagents ? `<div class="trace-session-children">${children.map((child) => sessionRowMarkup(child, true)).join("")}</div>` : ""}
  </section>`;
}

function sessionGridTemplate() {
  const widths = { turns: "22px", cost: "38px", duration: "34px", status: "44px", time: "38px" };
  return ["minmax(0,1fr)", ...TRACE_COLUMN_DEFAULTS.visible.filter((column) => state.traceVisibleColumns.has(column)).map((column) => widths[column]), "24px"].join(" ");
}

function applySessionGrid() {
  const template = sessionGridTemplate();
  $$("[data-session-grid]").forEach((node) => { node.style.gridTemplateColumns = template; });
  $$("[data-session-column]").forEach((node) => { node.hidden = !state.traceVisibleColumns.has(node.dataset.sessionColumn); });
}

function renderTraceSortHeaders() {
  $$("[data-trace-sort]").forEach((button) => {
    const active = button.dataset.traceSort === state.traceSort.key;
    button.classList.toggle("active", active);
    $("[data-sort-arrow]", button).textContent = active ? (state.traceSort.direction === "asc" ? "↑" : "↓") : "";
  });
}

function renderTraceColumnPicker() {
  const picker = $("#trace-column-picker");
  picker.innerHTML = `<strong>Visible columns</strong>${TRACE_COLUMN_DEFAULTS.visible.map((column) => `<label><input type="checkbox" data-trace-column="${escapeHtml(column)}" ${state.traceVisibleColumns.has(column) ? "checked" : ""}><span>${escapeHtml({ turns: "Turns", cost: "Cost", duration: "Duration", status: "Status", time: "Time" }[column])}</span></label>`).join("")}`;
  $$("[data-trace-column]", picker).forEach((input) => input.addEventListener("change", () => {
    if (input.checked) state.traceVisibleColumns.add(input.dataset.traceColumn);
    else state.traceVisibleColumns.delete(input.dataset.traceColumn);
    saveTraceColumnPreferences();
    applySessionGrid();
  }));
}

function renderTraceList() {
  const list = $("#trace-list");
  const roots = sortedSessionRows(traceSessionRows());
  list.innerHTML = roots.map((row) => sessionRowMarkup(row)).join("");
  if (!roots.length) list.innerHTML = '<div class="empty-state compact"><p>No matching body-free trace summaries.</p><small>Route a request or change the metadata filters.</small></div>';
  bindActions(list, "[data-trace-id]", (event) => selectTraceSummary(event.currentTarget.dataset.traceId));
  bindActions(list, "[data-session-id]", (event) => selectSessionSummary(event.currentTarget.dataset.sessionId));
  $$("[data-session-toggle]", list).forEach((button) => button.addEventListener("click", (event) => {
    event.stopPropagation();
    const id = button.dataset.sessionToggle;
    const closedKey = `closed:${id}`;
    if (state.traceExpandedSessions.has(closedKey)) state.traceExpandedSessions.delete(closedKey);
    else state.traceExpandedSessions.add(closedKey);
    renderTraceList();
  }));
  $$("[data-session-menu]", list).forEach((button) => button.addEventListener("click", (event) => {
    event.stopPropagation();
    openSessionMenu(button.dataset.sessionMenu, { anchor: button });
  }));
  $$("[data-session-id]", list).forEach((row) => row.addEventListener("contextmenu", (event) => {
    event.preventDefault();
    openSessionMenu(row.dataset.sessionId, { x: event.clientX, y: event.clientY });
  }));
  applySessionGrid();
  renderTraceSortHeaders();
  $("#trace-count").textContent = String(roots.length);
  $("#trace-list-status").textContent = `${roots.length} loaded session${roots.length === 1 ? "" : "s"}`;
  const filterCount = Object.keys(state.traceFilters).length;
  $("#trace-filter-state").textContent = filterCount ? ` · ${filterCount} filter${filterCount === 1 ? "" : "s"} applied` : "";
  $("#trace-filters").classList.toggle("filters-applied", filterCount > 0);
}

async function loadTraces(append = false) {
  const supportData = [];
  if (!state.providers.length) supportData.push(loadProvidersData().catch(() => []));
  if (!state.harnesses.length) supportData.push(loadHarnessesData(false).catch(() => []));
  const [data, sessions] = await Promise.all([
    api(`/traces/summaries?${traceQuery(append)}`),
    api("/traces/sessions?limit=1000").catch(() => ({ sessions: [] })),
    ...supportData,
  ]);
  state.traceCursor = data.next_cursor;
  state.traceSessions = sessions.sessions || [];
  const incoming = data.traces || [];
  state.traceRows = append ? [...state.traceRows, ...incoming.filter((trace) => !state.traceRows.some((current) => current.id === trace.id))] : incoming;
  populateTraceFilterOptions();
  renderTraceList();
  $("#more-traces").hidden = !data.has_more;
}

function middlewareRecords(attempt) {
  const records = parseList(attempt.middleware_decisions);
  if (!records.length) return '<p class="muted">No middleware decisions recorded.</p>';
  return `<ul class="trace-decision-list">${records.map((record) => `<li><span class="trace-decision-mark ${record.state === "matched" ? "matched" : ""}"></span><span><code>${escapeHtml(record.rule_name || record.rule_id || "unknown rule")}</code><small>${escapeHtml(record.explanation || record.action || "Evaluated")}</small></span><span class="trace-decision-state">${escapeHtml(record.suppressed ? "suppressed" : record.state || "unknown")}</span></li>`).join("")}</ul>`;
}

function renderAttempt(attempt, index) {
  const error = attempt.error?.message || attempt.error || attempt.error_kind;
  const provider = attempt.provider || attempt.upstream_provider;
  return `<article class="trace-attempt"><header><span class="attempt-number">${escapeHtml(attempt.attempt_number || attempt.attempt || index + 1)}</span><div><strong>Attempt ${escapeHtml(attempt.attempt_number || attempt.attempt || index + 1)}</strong><small class="trace-provider-label">${providerLogoTile(provider, provider, "trace-detail-brand")}${escapeHtml(providerDisplayName(provider || "unrouted"))}</small></div>${traceStatusChip(attempt.status, error)}</header><div class="trace-chip-row">${traceModelChip(attempt.model || attempt.routed_model)}${attempt.latency_ms === undefined ? "" : traceMetricChip("Latency", formatDurationMs(attempt.latency_ms))}${attempt.account_id ? traceMetricChip("Account", attempt.account_id) : ""}</div>${error ? `<p class="trace-error-copy">${escapeHtml(traceMetaValue(error))}</p>` : ""}<details class="trace-attempt-decisions" open><summary>Middleware decisions <span>${escapeHtml(parseList(attempt.middleware_decisions).length)}</span></summary>${middlewareRecords(attempt)}</details><details class="trace-json-card"><summary>Attempt metadata</summary><pre>${escapeHtml(JSON.stringify(attempt, null, 2))}</pre></details></article>`;
}

function bodyDetails(trace) {
  const bodies = [["request", "Client request", trace.req_body_path], ["upstream-request", "Upstream request", trace.upstream_req_body_path], ["response", "Client response", trace.resp_body_path]];
  if (trace.via_dario) bodies.push(["dario-upstream-request", "Dario upstream request", true], ["dario-upstream-response", "Dario upstream response", true]);
  return bodies.filter(([, , available]) => available).map(([kind, label]) => `<details class="lazy-data trace-body-card" data-body-kind="${escapeHtml(kind)}"><summary><span>${escapeHtml(label)}</span><small>Load one body</small></summary><pre>Open to load only this body.</pre></details>`).join("") || '<p class="muted">No stored bodies are available for this trace.</p>';
}

function traceDetailGrid(items) {
  return `<dl class="trace-detail-grid">${items.map(([label, value, html = false]) => `<div><dt>${escapeHtml(label)}</dt><dd>${html ? value : escapeHtml(traceMetaValue(value))}</dd></div>`).join("")}</dl>`;
}

function traceDetailSection(title, body, badge = "", open = false) {
  return `<details class="trace-detail-section" ${open ? "open" : ""}><summary><span>${escapeHtml(title)}</span>${badge !== "" ? `<small>${escapeHtml(badge)}</small>` : ""}</summary><div class="trace-section-body">${body}</div></details>`;
}

function renderTraceDetail(id, data) {
  const trace = data.trace || data;
  const attempts = parseList(trace.attempts);
  const detail = $("#trace-detail");
  const summary = traceDetailGrid([
    ["Method / path", [trace.method, trace.path].filter(Boolean).join(" ")], ["Error", trace.error || trace.error_kind],
    ["Session", trace.session_id], ["Run", trace.run_id], ["Streamed", trace.streamed], ["Billing bucket", trace.billing_bucket],
  ]);
  const provenance = traceDetailGrid([
    ["Harness", `${harnessLogoTile(trace.harness, trace.harness, "trace-detail-brand")}<span>${escapeHtml(harnessDisplayName(trace.harness))}</span>`, true],
    ["Client format", trace.client_format],
    ["Provider", `${providerLogoTile(trace.upstream_provider, trace.upstream_provider, "trace-detail-brand")}<span>${escapeHtml(providerDisplayName(trace.upstream_provider))}</span>`, true],
    ["Upstream format", trace.upstream_format], ["Requested model", trace.requested_model], ["Routed model", trace.routed_model],
    ["Original model", trace.original_model], ["Served model", trace.served_model], ["Account", trace.account_id],
    ["Original account", trace.original_account_id], ["Served account", trace.served_account_id], ["Via Dario", trace.via_dario],
    ["Dario generation", trace.dario_generation], ["Routing explanation", trace.substitution_reason],
  ]);
  detail.innerHTML = `<header class="trace-detail-head"><div><div class="trace-detail-eyebrow">TRACE <code>${escapeHtml(traceShortId(id))}</code></div><div class="trace-detail-title">${traceStatusChip(trace.status, trace.error)}<time>${escapeHtml(formatTime(trace.ts_request_ms))}</time></div></div><button data-close-detail>Close</button></header>
    <div class="trace-detail-scroll"><div class="trace-chip-row trace-quick-stats">${traceMetricChip("Latency", formatDurationMs(trace.latency_ms))}${traceMetricChip("Input", traceTokenCount(trace.input_tokens))}${traceMetricChip("Output", traceTokenCount(trace.output_tokens))}${trace.cost_usd === null || trace.cost_usd === undefined ? "" : traceMetricChip("Cost", formatMoney(trace.cost_usd), "cost")}</div>
    ${traceDetailSection("Summary", summary, "", true)}${traceDetailSection("Provenance", provenance, "", true)}${traceDetailSection("Attempts", `<div class="attempt-list">${attempts.length ? attempts.map(renderAttempt).join("") : '<p class="muted">No attempt records stored.</p>'}</div>`, attempts.length, true)}${traceDetailSection("Stored bodies", `<div class="lazy-list">${bodyDetails(trace)}</div>`, "explicit load", true)}${trace.session_id ? traceDetailSection("Session link", `<button class="trace-session-link" data-session-link="${escapeHtml(trace.session_id)}">Open chat flow <code>${escapeHtml(traceShortId(trace.session_id))}</code><span>→</span></button>`, "", true) : ""}</div>`;
  $('[data-close-detail]', detail).addEventListener("click", closeTraceDetail);
  $$('[data-body-kind]', detail).forEach((node) => node.addEventListener("toggle", () => { if (node.open && !node.dataset.loaded) loadTraceBody(id, node); }));
  $('[data-session-link]', detail)?.addEventListener("click", () => {
    const summaryTrace = state.traceRows.find((row) => row.id === id) || { ...trace, id, provider: trace.upstream_provider, model: trace.routed_model };
    showTraceConversation(summaryTrace);
    detail.classList.remove("open");
  });
  detail.scrollTop = 0;
}

function closeTraceDetail() {
  const detail = $("#trace-detail");
  detail.classList.remove("open");
  detail.innerHTML = '<div class="empty-state"><span>⌁</span><h2>Select a trace</h2><p>Inspect metadata, attempts, middleware, and explicitly loaded bodies.</p></div>';
}

// This is deliberately metadata-only. It must never fetch trace body bytes or a
// full transcript; explicit disclosure toggles below own those requests.
async function openTrace(id) {
  state.traceDetailRequest = id;
  try {
    const data = await api(`/traces/${encodeURIComponent(id)}/metadata`);
    if (state.traceDetailRequest === id) renderTraceDetail(id, data);
  }
  catch (error) { toast(error.message, "danger"); }
}

async function loadTraceBody(id, node) {
  node.dataset.loaded = "true";
  const output = $("pre", node);
  output.textContent = "Loading one body…";
  try { output.textContent = await apiText(`/traces/${encodeURIComponent(id)}/body/${encodeURIComponent(node.dataset.bodyKind)}`); }
  catch (error) { delete node.dataset.loaded; output.textContent = `Could not load body: ${error.message}`; }
}

function renderExecutedTools(tools) {
  if (!tools?.length) return "";
  return `<div class="executed-tools"><div class="tool-stack-label">${escapeHtml(`${tools.length} executed tool${tools.length === 1 ? "" : "s"}`)}</div>${tools.map((tool) => {
    const args = tool.arguments === null || tool.arguments === undefined ? "" : (typeof tool.arguments === "string" ? tool.arguments : JSON.stringify(tool.arguments, null, 2));
    const duration = tool.ts_end_ms === null || tool.ts_end_ms === undefined ? "" : formatDurationMs(finite(tool.ts_end_ms) - finite(tool.ts_start_ms));
    return `<details class="trace-tool-card"><summary><span class="tool-icon">⌘</span><code>${escapeHtml(tool.tool_name || "tool")}</code><span class="tool-summary">${escapeHtml(args.split("\n")[0].slice(0, 60))}</span>${duration ? `<small>${escapeHtml(duration)}</small>` : ""}${traceStatusChip(tool.is_error ? 500 : 200, tool.is_error ? "tool error" : null)}</summary><div class="trace-tool-detail">${args ? `<strong>Arguments</strong><pre>${escapeHtml(args)}</pre>` : ""}${tool.result ? `<strong>Result</strong><pre>${escapeHtml(tool.result)}</pre>` : ""}</div></details>`;
  }).join("")}</div>`;
}

function renderTurnMessage(role, text, turn) {
  if (!text) return "";
  const user = role === "user";
  const tokenValue = user ? turn.input_tokens : turn.output_tokens;
  return `<section class="turn-message ${user ? "user" : "assistant"}"><div class="turn-avatar">${user ? "U" : "A"}</div><div class="turn-message-copy"><header><strong>${user ? "User / Harness" : "Assistant"}</strong>${user ? (turn.harness ? `<span>${escapeHtml(turn.harness)}</span>` : "") : traceModelChip(turn.model || turn.served_model)}${tokenValue === null || tokenValue === undefined ? "" : `<small>${escapeHtml(`${formatInteger(tokenValue)} tok`)}</small>`}<time>${escapeHtml(formatTime(turn.ts_request_ms))}</time></header><div class="turn-bubble">${escapeHtml(text)}</div></div></section>`;
}

function renderTurn(turn) {
  const assistant = turn.assistant || parseList(turn.assistant_blocks).filter((block) => block.type === "text").map((block) => block.text).join("\n\n");
  return `<article class="turn-expanded">${renderTurnMessage("user", turn.user, turn)}${renderTurnMessage("assistant", assistant || turn.error || "No assistant text was stored.", turn)}${renderExecutedTools(turn.executed_tools)}<footer class="turn-meta-footer"><code>${escapeHtml(traceShortId(turn.trace_id))}</code>${traceStatusChip(turn.status, turn.error)}${turn.cost_usd === null || turn.cost_usd === undefined ? "" : traceMetricChip("Cost", formatMoney(turn.cost_usd))}</footer></article>`;
}

function subagentsForTurn(turn, pageTurns) {
  const currentSession = state.selectedSessionId;
  if (!currentSession) return [];
  const loadedIds = new Set(state.traceRows.map((trace) => trace.session_id).filter(Boolean));
  const turnTrace = state.traceRows.find((trace) => trace.id === (turn.trace_id || turn.id));
  const currentRun = turnTrace?.run_id;
  return state.traceSessions.filter((session) => {
    if (!loadedIds.has(session.session_id) || session.session_id === currentSession) return false;
    const related = session.parent_session_id === currentSession || (currentRun && session.run_id === currentRun);
    if (!related) return false;
    if (session.lineage_turn_id && session.lineage_turn_id === turn.trace_id) return true;
    const started = finite(session.subagent_started_ms ?? session.first_ts_ms);
    const eligible = pageTurns.filter((candidate) => finite(candidate.ts_request_ms) <= started);
    const assigned = eligible.length ? eligible[eligible.length - 1] : pageTurns[0];
    return assigned && assigned.trace_id === turn.trace_id;
  });
}

function renderSubagentCard(session) {
  const status = session.status_label || "Running";
  const model = session.models?.[0];
  const provider = session.providers?.[0];
  return `<article class="trace-subagent-card">
    <div class="trace-subagent-badge">${harnessLogoTile(session.harness, session.harness, "trace-mini-brand")}<span><b>Subagent</b><small>${escapeHtml(status)}</small></span></div>
    <div class="trace-subagent-facts">${model ? traceModelChip(model) : ""}${provider ? `<span>${providerLogoTile(provider, provider, "trace-mini-brand")}${escapeHtml(providerDisplayName(provider))}</span>` : ""}<span>${escapeHtml(`${session.trace_count || 0} turn${session.trace_count === 1 ? "" : "s"}`)}</span></div>
    <button data-follow-session="${escapeHtml(session.session_id)}">Follow</button>
  </article>`;
}

function renderTurnSummary(turn, pageTurns = []) {
  const traceId = turn.trace_id || turn.id;
  const selected = state.selectedTraceId === traceId;
  const subagents = subagentsForTurn(turn, pageTurns);
  return `<details class="turn-summary ${selected ? "selected" : ""}" data-turn-trace="${escapeHtml(traceId)}" ${selected ? "open" : ""}><summary><div class="turn-summary-conversation"><div class="turn-summary-message user"><span class="turn-avatar">U</span><span><strong>User / Harness</strong><small>Body-free turn metadata</small></span><time>${escapeHtml(traceRelativeTime(turn.ts_request_ms))}</time></div><div class="turn-thread-line"></div><div class="turn-summary-message assistant"><span class="turn-avatar">A</span><span><strong>Assistant</strong><small>${providerLogoTile(turn.provider, turn.provider, "trace-mini-brand")}${escapeHtml(providerDisplayName(turn.provider || "unrouted"))}</small></span>${traceModelChip(turn.model || traceId)}${traceStatusChip(turn.status, turn.error)}<small class="turn-token-total">${escapeHtml(`${formatInteger(finite(turn.input_tokens) + finite(turn.output_tokens))} tok · ${traceRelativeTime(turn.ts_request_ms)}`)}</small></div></div>${subagents.length ? `<div class="turn-subagent-list">${subagents.map(renderSubagentCard).join("")}</div>` : ""}</summary><div class="turn-detail muted">Open to load only this turn.</div></details>`;
}

function replaceTranscriptPage(target, html) {
  target.replaceChildren();
  target.insertAdjacentHTML("afterbegin", html);
}

async function loadTranscriptTurn(node) {
  if (node.dataset.loaded || node.dataset.loading) return;
  node.dataset.loading = "true";
  node.dataset.loaded = "true";
  const target = $(".turn-detail", node);
  target.textContent = "Loading this turn…";
  try {
    const data = await api(`/traces/${encodeURIComponent(node.dataset.turnTrace)}/turn`);
    target.classList.remove("muted");
    target.innerHTML = renderTurn(data.turn);
  } catch (error) { delete node.dataset.loaded; target.textContent = `Could not load turn: ${error.message}`; }
  finally { delete node.dataset.loading; }
}

function selectTranscriptTurn(node, options = {}) {
  const traceId = node.dataset.turnTrace;
  const changed = state.selectedTraceId !== traceId;
  state.selectedTraceId = traceId;
  $$("[data-turn-trace]", node.closest(".session-turns") || node.parentElement).forEach((turn) => turn.classList.toggle("selected", turn === node));
  if (changed || options.forceInspector) openTrace(traceId);
  if (options.expand) {
    node.open = true;
    loadTranscriptTurn(node);
  }
  if (options.scroll) node.scrollIntoView({ block: "nearest", behavior: "smooth" });
}

async function loadTranscriptPage(node, cursor) {
  const target = $(".session-turns", node);
  target.textContent = "Loading a bounded page…";
  const params = new URLSearchParams({ limit: String(TURN_PAGE_SIZE) });
  if (cursor) { params.set("after_ms", cursor.after_ms); params.set("after_id", cursor.after_id); }
  try {
    const data = await api(`/traces/sessions/${encodeURIComponent(node.dataset.transcript)}/transcript/page?${params}`);
    const pageTurns = data.turns || [];
    const selectedSummary = node._pageIndex === 0
      ? state.traceRows.find((trace) => trace.id === state.selectedTraceId && trace.session_id === node.dataset.transcript)
      : null;
    if (selectedSummary && !pageTurns.some((turn) => turn.trace_id === selectedSummary.id)) {
      pageTurns.push({
        trace_id: selectedSummary.id,
        ts_request_ms: selectedSummary.ts_request_ms,
        ts_response_ms: selectedSummary.ts_response_ms,
        harness: selectedSummary.harness,
        model: selectedSummary.model,
        provider: selectedSummary.provider,
        status: selectedSummary.status,
        input_tokens: selectedSummary.input_tokens,
        output_tokens: selectedSummary.output_tokens,
        error: selectedSummary.error,
        substituted: selectedSummary.substituted,
        pinned_selected: true,
      });
    }
    const selectedTurn = pageTurns.find((turn) => turn.trace_id === state.selectedTraceId) || pageTurns[pageTurns.length - 1];
    if (selectedTurn) state.selectedTraceId = selectedTurn.trace_id;
    const turns = pageTurns.map((turn) => renderTurnSummary(turn, pageTurns)).join("") || '<p class="muted">No turns found.</p>';
    const pinned = pageTurns.some((turn) => turn.pinned_selected);
    const controls = `<div class="turn-page-controls"><button data-turn-previous ${node._pageIndex ? "" : "disabled"}>← Previous</button><span>Page ${node._pageIndex + 1} · up to ${TURN_PAGE_SIZE} turns${pinned ? " + selected" : ""}</span><button data-turn-next ${data.has_more ? "" : "disabled"}>Next →</button></div>`;
    replaceTranscriptPage(target, `${turns}${controls}`);
    $$('[data-turn-trace]', target).forEach((turn) => {
      turn.addEventListener("click", (event) => {
        if (event.target.closest("[data-follow-session]")) return;
        selectTranscriptTurn(turn);
      });
      turn.addEventListener("toggle", () => { if (turn.open) loadTranscriptTurn(turn); });
    });
    $$("[data-follow-session]", target).forEach((button) => button.addEventListener("click", (event) => {
      event.preventDefault();
      event.stopPropagation();
      selectSessionSummary(button.dataset.followSession);
    }));
    $('[data-turn-previous]', target).addEventListener("click", () => { if (node._pageIndex > 0) { node._pageIndex -= 1; loadTranscriptPage(node, node._pageStarts[node._pageIndex]); } });
    $('[data-turn-next]', target).addEventListener("click", () => { if (data.next_cursor) { node._pageStarts = node._pageStarts.slice(0, node._pageIndex + 1); node._pageStarts.push(data.next_cursor); node._pageIndex += 1; loadTranscriptPage(node, data.next_cursor); } });
    const selectedNode = selectedTurn ? $$("[data-turn-trace]", target).find((turn) => turn.dataset.turnTrace === selectedTurn.trace_id) : null;
    if (selectedNode) selectTranscriptTurn(selectedNode, { expand: true, forceInspector: true, scroll: false });
  } catch (error) { target.textContent = `Could not load turns: ${error.message}`; }
}

async function loadTranscript(node) {
  node.dataset.loaded = "true";
  node._pageStarts = [null];
  node._pageIndex = 0;
  await loadTranscriptPage(node, null);
}

function conversationShell(trace) {
  const session = trace.session_id;
  const title = session ? traceShortId(session) : `Trace ${traceShortId(trace.id)}`;
  const subtitle = session ? "Bounded session transcript" : "Standalone trace";
  return `<header class="trace-panel-head accent"><div class="conversation-identity"><span class="conversation-mark">⌁</span><span><strong>${escapeHtml(title)}</strong><small>${escapeHtml(subtitle)}</small></span>${traceModelChip(trace.model || trace.served_model)}</div><div class="conversation-actions">${session ? `<button class="trace-icon-button" data-session-menu="${escapeHtml(session)}" aria-label="Session actions">⋯</button>` : ""}<button class="mobile-only-action" data-close-conversation aria-label="Close conversation">Close</button></div></header><div class="trace-conversation-scroll">${session ? `<div class="session-transcript" data-transcript="${escapeHtml(session)}"><div class="session-turns">Loading a bounded page…</div></div>` : renderTurnSummary(trace, [trace])}</div>`;
}

function showTraceConversation(trace) {
  const conversation = $("#trace-conversation");
  conversation.classList.add("open");
  conversation.tabIndex = 0;
  conversation.innerHTML = conversationShell(trace);
  $('[data-close-conversation]', conversation).addEventListener("click", () => conversation.classList.remove("open"));
  $('[data-session-menu]', conversation)?.addEventListener("click", (event) => {
    event.stopPropagation();
    openSessionMenu(event.currentTarget.dataset.sessionMenu, { anchor: event.currentTarget });
  });
  const transcript = $('[data-transcript]', conversation);
  if (transcript) loadTranscript(transcript);
  else $$('[data-turn-trace]', conversation).forEach((turn) => {
    turn.addEventListener("click", () => selectTranscriptTurn(turn));
    turn.addEventListener("toggle", () => { if (turn.open) loadTranscriptTurn(turn); });
    selectTranscriptTurn(turn, { expand: true, forceInspector: true });
  });
  conversation.addEventListener("keydown", (event) => {
    if (!["ArrowUp", "ArrowDown"].includes(event.key) || event.target.closest("input,select,textarea")) return;
    const turns = $$("[data-turn-trace]", conversation);
    if (!turns.length) return;
    const selectedIndex = Math.max(0, turns.findIndex((turn) => turn.classList.contains("selected")));
    const nextIndex = clamp(selectedIndex + (event.key === "ArrowDown" ? 1 : -1), 0, turns.length - 1);
    event.preventDefault();
    selectTranscriptTurn(turns[nextIndex], { expand: true, scroll: true });
  });
}

function selectTraceSummary(id) {
  const trace = state.traceRows.find((row) => row.id === id);
  if (!trace) return;
  state.selectedTraceId = trace.id;
  state.selectedSessionId = trace.session_id || null;
  renderTraceList();
  showTraceConversation(trace);
  openTrace(trace.id);
}

function selectSessionSummary(sessionId) {
  const trace = state.traceRows.find((row) => row.session_id === sessionId);
  if (!trace) return;
  state.selectedSessionId = sessionId;
  state.selectedTraceId = trace.id;
  renderTraceList();
  showTraceConversation(trace);
  openTrace(trace.id);
}

function saveTraceColumnPreferences() {
  try {
    localStorage.setItem(TRACE_COLUMN_STORAGE_KEY, JSON.stringify({
      left: state.traceColumnWidths.left,
      right: state.traceColumnWidths.right,
      visible: [...state.traceVisibleColumns],
      showSubagents: state.traceShowSubagents,
    }));
  } catch { /* Storage can be disabled without disabling the browser. */ }
}

function setTraceColumnWidth(side, width, persist = false) {
  const limits = side === "left" ? [200, 560] : [280, 720];
  state.traceColumnWidths[side] = clamp(width, limits[0], limits[1]);
  $("#trace-browser").style.setProperty(`--trace-${side}-width`, `${state.traceColumnWidths[side]}px`);
  if (persist) saveTraceColumnPreferences();
}

function initTraceColumns() {
  setTraceColumnWidth("left", state.traceColumnWidths.left);
  setTraceColumnWidth("right", state.traceColumnWidths.right);
  renderTraceColumnPicker();
  applySessionGrid();
  const showSubagents = $("#trace-show-subagents");
  showSubagents.checked = state.traceShowSubagents;
  showSubagents.addEventListener("change", () => {
    state.traceShowSubagents = showSubagents.checked;
    saveTraceColumnPreferences();
    renderTraceList();
  });

  $("#trace-column-picker-button").addEventListener("click", (event) => {
    event.stopPropagation();
    const picker = $("#trace-column-picker");
    picker.hidden = !picker.hidden;
    event.currentTarget.setAttribute("aria-expanded", String(!picker.hidden));
  });
  $("#trace-column-picker").addEventListener("click", (event) => event.stopPropagation());
  document.addEventListener("click", () => {
    $("#trace-column-picker").hidden = true;
    $("#trace-column-picker-button").setAttribute("aria-expanded", "false");
  });
  $$("[data-trace-sort]").forEach((button) => button.addEventListener("click", () => {
    const key = button.dataset.traceSort;
    state.traceSort = state.traceSort.key === key
      ? { key, direction: state.traceSort.direction === "asc" ? "desc" : "asc" }
      : { key, direction: key === "session" ? "asc" : "desc" };
    renderTraceList();
  }));

  $$("[data-trace-divider]").forEach((divider) => {
    const side = divider.dataset.traceDivider;
    const reset = () => setTraceColumnWidth(side, TRACE_COLUMN_DEFAULTS[side], true);
    divider.addEventListener("dblclick", reset);
    divider.addEventListener("keydown", (event) => {
      if (event.key === "Enter") return reset();
      if (!["ArrowLeft", "ArrowRight"].includes(event.key)) return;
      event.preventDefault();
      const delta = event.key === "ArrowRight" ? 10 : -10;
      setTraceColumnWidth(side, state.traceColumnWidths[side] + (side === "right" ? -delta : delta), true);
    });
    divider.addEventListener("pointerdown", (event) => {
      if (event.button !== 0) return;
      event.preventDefault();
      const startX = event.clientX;
      const startWidth = state.traceColumnWidths[side];
      divider.classList.add("active");
      document.body.classList.add("trace-resizing");
      divider.setPointerCapture(event.pointerId);
      const move = (moveEvent) => {
        const delta = moveEvent.clientX - startX;
        setTraceColumnWidth(side, startWidth + (side === "right" ? -delta : delta));
      };
      const finish = () => {
        divider.classList.remove("active");
        document.body.classList.remove("trace-resizing");
        divider.removeEventListener("pointermove", move);
        divider.removeEventListener("pointerup", finish);
        divider.removeEventListener("pointercancel", finish);
        saveTraceColumnPreferences();
      };
      divider.addEventListener("pointermove", move);
      divider.addEventListener("pointerup", finish);
      divider.addEventListener("pointercancel", finish);
    });
  });
}

function closeSessionMenu() {
  const menu = $("#trace-session-menu");
  if (menu) menu.remove();
  state.sessionMenu = null;
}

function newestSessionTrace(sessionId) {
  return state.traceRows
    .filter((trace) => trace.session_id === sessionId)
    .sort((left, right) => finite(right.ts_request_ms) - finite(left.ts_request_ms))[0] || null;
}

function sessionMenuMarkup(sessionId, fixtures, loading = false, fixtureError = null) {
  const trace = newestSessionTrace(sessionId);
  const exportUrl = safeUrl(`/traces/export.ndjson?${new URLSearchParams({ session: sessionId })}`);
  const fixtureItems = loading
    ? '<span class="trace-menu-note">Loading fixtures…</span>'
    : fixtureError
      ? `<span class="trace-menu-note danger">${escapeHtml(fixtureError)}</span>`
      : fixtures.length
        ? fixtures.map((fixture) => `<button data-inject-fixture="${escapeHtml(fixture.name)}"><span>${escapeHtml(fixture.direction === "upstream_to_client" ? "Send" : "Replay")}: ${escapeHtml(fixture.name)}</span><small>${escapeHtml(fixture.status || "")}</small></button>`).join("")
        : '<span class="trace-menu-note">No fixtures available</span>';
  return `<div class="trace-menu-card" role="menu" aria-label="Session actions">
    <details class="trace-menu-submenu"><summary>Simulate <span>›</span></summary><div>${fixtureItems}</div></details>
    <button data-clear-injections>Clear pending injections</button>
    <hr>
    <button data-copy-session>Copy Session ID</button>
    <button data-copy-reply ${trace ? "" : "disabled"}>Copy Last Reply as Markdown</button>
    <a href="${escapeHtml(exportUrl)}" download>Export Session…</a>
  </div>`;
}

function positionSessionMenu(menu, position) {
  const anchor = position.anchor?.getBoundingClientRect();
  const preferredX = position.x ?? anchor?.left ?? 12;
  const preferredY = position.y ?? anchor?.bottom ?? 12;
  const left = clamp(preferredX, 8, Math.max(8, window.innerWidth - menu.offsetWidth - 8));
  const top = clamp(preferredY, 8, Math.max(8, window.innerHeight - menu.offsetHeight - 8));
  menu.style.left = `${left}px`;
  menu.style.top = `${top}px`;
}

function bindSessionMenu(menu, sessionId, position) {
  positionSessionMenu(menu, position);
  if (menu.dataset.bound) return;
  menu.dataset.bound = "true";
  menu.addEventListener("click", async (event) => {
    if (event.target.closest("a[download]")) {
      closeSessionMenu();
      return;
    }
    const fixtureButton = event.target.closest("[data-inject-fixture]");
    if (fixtureButton) {
      try {
        await api(`/admin/sessions/${encodeURIComponent(sessionId)}/inject`, { method: "POST", body: JSON.stringify({ fixture: fixtureButton.dataset.injectFixture }) });
        toast(`Queued ${fixtureButton.dataset.injectFixture} for ${traceShortId(sessionId)}.`);
        closeSessionMenu();
      } catch (error) { toast(error.message, "danger"); }
      return;
    }
    if (event.target.closest("[data-clear-injections]")) {
      try {
        await api(`/admin/sessions/${encodeURIComponent(sessionId)}/injections`, { method: "DELETE" });
        toast("Pending injections cleared.");
        closeSessionMenu();
      } catch (error) { toast(error.message, "danger"); }
      return;
    }
    if (event.target.closest("[data-copy-session]")) {
      await copyLoginText(sessionId);
      toast("Session ID copied.");
      closeSessionMenu();
      return;
    }
    if (event.target.closest("[data-copy-reply]")) {
      const trace = newestSessionTrace(sessionId);
      if (!trace) return;
      try {
        const reply = await apiText(`/traces/${encodeURIComponent(trace.id)}/reply.md`);
        await copyLoginText(reply);
        toast("Last reply copied as Markdown.");
        closeSessionMenu();
      } catch (error) { toast(error.message, "danger"); }
    }
  });
}

async function openSessionMenu(sessionId, position = {}) {
  closeSessionMenu();
  const menu = document.createElement("div");
  menu.id = "trace-session-menu";
  menu.className = "trace-session-menu";
  menu.innerHTML = sessionMenuMarkup(sessionId, state.fixtures, !state.sessionFixturesLoaded);
  document.body.append(menu);
  state.sessionMenu = { sessionId, position };
  bindSessionMenu(menu, sessionId, position);
  $("[role=menu] button:not([disabled]), [role=menu] summary, [role=menu] a", menu)?.focus();
  if (state.sessionFixturesLoaded) return;
  try {
    const data = await api("/admin/fixtures");
    state.fixtures = data.fixtures || [];
    state.sessionFixturesLoaded = true;
    if (state.sessionMenu?.sessionId !== sessionId) return;
    menu.innerHTML = sessionMenuMarkup(sessionId, state.fixtures);
    bindSessionMenu(menu, sessionId, position);
  } catch (error) {
    if (state.sessionMenu?.sessionId !== sessionId) return;
    menu.innerHTML = sessionMenuMarkup(sessionId, [], false, error.message);
    bindSessionMenu(menu, sessionId, position);
  }
}

function applyTraceFilters(event) {
  event.preventDefault();
  state.traceFilters = {};
  for (const [key, value] of new FormData(event.currentTarget).entries()) {
    if (value && String(value).trim()) state.traceFilters[key] = key === "errors" ? "1" : String(value).trim();
  }
  state.traceCursor = null;
  loadTraces(false).catch((error) => toast(error.message, "danger"));
}

/* 10. Event binding and startup. */
function bindStaticEvents() {
  $("#password-login-form").addEventListener("submit", submitPasswordLogin);
  $("#remote-auth-form").addEventListener("submit", submitAdminKey);
  $("#password-config-form").addEventListener("submit", (event) => submitWebPassword(event, true));
  $("#web-password-form").addEventListener("submit", (event) => submitWebPassword(event, false));
  $("#skip-onboarding").addEventListener("click", finishOnboarding);
  $("#skip-onboarding-step").addEventListener("click", skipOnboardingStep);
  $("#onboarding-back").addEventListener("click", () => showOnboardingStep(state.onboardingStep - 1));
  $("#onboarding-next").addEventListener("click", nextOnboardingStep);
  $("#restart-onboarding").addEventListener("click", restartOnboarding);
  $("#logout").addEventListener("click", logout);
  $$('nav [data-view]').forEach((button) => button.addEventListener("click", () => selectView(button.dataset.view, true)));
  $$('[data-go]').forEach((button) => button.addEventListener("click", () => selectView(button.dataset.go, true)));
  $$('[data-refresh-card]').forEach((button) => button.addEventListener("click", refreshCurrentView));
  $("#mobile-menu").addEventListener("click", () => document.body.classList.toggle("nav-open"));
  $("#global-refresh").addEventListener("click", refreshCurrentView);
  $("#sidebar-update").addEventListener("click", () => selectView("updates", true));
  $("#quick-refresh").addEventListener("click", refreshCurrentView);
  $("#refresh-interval").addEventListener("change", setRefreshInterval);
  $("#run-ping").addEventListener("click", openCredentialPingChecks);
  $("#test-credentials").addEventListener("click", openCredentialPingChecks);
  $("#rerun-credential-ping").addEventListener("click", runCredentialPingChecks);
  $("#close-credential-ping").addEventListener("click", () => $("#credential-ping-dialog").close());
  $("#done-credential-ping").addEventListener("click", () => $("#credential-ping-dialog").close());
  $("#add-provider").addEventListener("click", () => { const picker = $("#provider-picker"); picker.hidden = state.providerTab ? false : !picker.hidden; });
  $("#gemini-key-form").addEventListener("submit", saveGeminiKey);
  $("#openrouter-form").addEventListener("submit", saveOpenRouter);
  $("#exo-form").addEventListener("submit", probeExo);
  $("#cliproxyapi-form").addEventListener("submit", saveCLIProxyAPI);
  $("#refresh-imports").addEventListener("click", loadImportCandidates);
  $("#refresh-harnesses").addEventListener("click", () => loadHarnesses(true));
  $("#refresh-credentials").addEventListener("click", loadCredentials);
  $("#connect-key-form").addEventListener("submit", (event) => mintRunKey(event, true));
  $("#run-key-form").addEventListener("submit", (event) => mintRunKey(event, false));
  $("#refresh-dario").addEventListener("click", loadDario);
  $("#ping-dario").addEventListener("click", pingDario);
  $("#refresh-middleware").addEventListener("click", loadMiddleware);
  $("#reload-middleware").addEventListener("click", reloadMiddleware);
  $("#middleware-settings-form").addEventListener("submit", saveMiddlewareSettings);
  $("#protection-form").addEventListener("submit", saveProtection);
  $("#refresh-notifications").addEventListener("click", loadNotifications);
  $("#notification-form").addEventListener("submit", saveNotification);
  $("#validate-telegram").addEventListener("click", () => notificationAction("validate"));
  $("#discover-telegram").addEventListener("click", () => notificationAction("discover"));
  $("#test-telegram").addEventListener("click", () => notificationAction("test"));
  $("#check-updates").addEventListener("click", checkForUpdates);
  $("#update-channel-form").addEventListener("submit", submitUpdateChannel);
  $("#install-update").addEventListener("click", openUpdateConfirmation);
  $("#cancel-update-install").addEventListener("click", () => $("#update-confirm-dialog").close());
  $("#confirm-update-install").addEventListener("click", installUpdate);
  $("#storage-prune-form").addEventListener("submit", pruneStorage);
  $("#refresh-traces").addEventListener("click", () => loadTraces(false));
  $("#more-traces").addEventListener("click", () => loadTraces(true));
  $("#trace-filters").addEventListener("submit", applyTraceFilters);
  $("#trace-filters").addEventListener("reset", () => {
    state.traceFilters = {};
    state.traceCursor = null;
    setTimeout(() => {
      $("#trace-show-subagents").checked = state.traceShowSubagents;
      loadTraces(false);
    }, 0);
  });
  document.addEventListener("pointerdown", (event) => {
    const menu = $("#trace-session-menu");
    if (menu && !menu.contains(event.target)) closeSessionMenu();
  });
  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape") closeSessionMenu();
  });
  window.addEventListener("hashchange", () => selectView(location.hash.slice(1), false));
  window.addEventListener("popstate", () => selectView(location.hash.slice(1), false));
  initTraceColumns();
}

bindStaticEvents();
bootstrap();
