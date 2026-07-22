/* 1. State, escaping, formatting, API, and toast helpers. */
const TURN_PAGE_SIZE = 20;
const TRACE_PAGE_SIZE = 25;
const REFRESH_STORAGE_KEY = "alex.web.refresh-seconds";
const PROVIDERS = [
  ["claude", "Anthropic", "oauth"],
  ["codex", "OpenAI", "oauth"],
  ["gemini", "Google Gemini", "oauth"],
  ["grok", "xAI", "oauth"],
  ["kimi", "Moonshot Kimi", "oauth"],
  ["amp", "Amp", "import"],
  ["cliproxyapi", "CLIProxyAPI", "form"],
];
const VIEW_COPY = {
  onboarding: ["Onboarding", "Set up Alex in a few small steps"],
  dashboard: ["Dashboard", "Daemon, providers, tools, and recent activity"],
  traces: ["Trace Browser", "Body-safe request inspection"],
  general: ["General", "Daemon, updates, storage, and web access"],
  providers: ["Providers", "Accounts, usage, quotas, and routing"],
  harnesses: ["Harnesses", "Connect and configure coding tools"],
  credentials: ["Credentials", "Scoped access and outbound credential status"],
  dario: ["Dario", "Claude subscription runtime and prompt caches"],
  middleware: ["Middleware", "Rules, protection, activity, and leases"],
  notifications: ["Notifications", "Telegram alerts and daemon messages"],
};
const CHART_COLORS = ["#0a84ff", "#30d158", "#ff9f0a", "#bf5af2", "#64d2ff", "#ff453a"];
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
  harnesses: [],
  analytics: null,
  limits: null,
  dario: null,
  update: null,
  middleware: null,
  fixtures: [],
  cliproxyapi: null,
  exo: null,
  openrouter: null,
  traceCursor: null,
  traceFilters: {},
  loginPoll: null,
};

const $ = (selector, root = document) => root.querySelector(selector);
const $$ = (selector, root = document) => [...root.querySelectorAll(selector)];
const escapeHtml = (value) => String(value ?? "").replace(/[&<>"']/g, (character) => ({
  "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;",
})[character]);
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
  const headers = new Headers(options.headers || {});
  if (state.adminKey && !state.sessionAuthenticated) headers.set("x-api-key", state.adminKey);
  if (options.body && !headers.has("content-type")) headers.set("content-type", "application/json");
  const response = await fetch(path, { credentials: "same-origin", ...options, headers });
  const payload = response.status === 204 ? null : await response.json().catch(() => null);
  if (!response.ok) throw new Error(errorMessage(payload, response));
  return payload;
}

async function apiText(path, options = {}) {
  const headers = new Headers(options.headers || {});
  if (state.adminKey && !state.sessionAuthenticated) headers.set("x-api-key", state.adminKey);
  const response = await fetch(path, { credentials: "same-origin", ...options, headers });
  const text = await response.text();
  if (!response.ok) {
    let payload = null;
    try { payload = JSON.parse(text); } catch { /* plain-text response */ }
    throw new Error(errorMessage(payload, response));
  }
  return text;
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
  $("#auth-screen").hidden = true;
  $("#app-shell").hidden = false;
  configureRefreshInterval();
  updatePasswordStep();
  const requested = location.hash.slice(1);
  selectView(state.auth?.onboarding_completed ? (VIEW_COPY[requested] ? requested : "dashboard") : "onboarding", false);
  refreshSharedData().catch((error) => toast(error.message, "danger"));
}

function updatePasswordStep() {
  const configured = Boolean(state.auth?.password_configured);
  $("#password-configured").hidden = !configured;
  $("#password-config-form").hidden = configured;
  const next = $('[data-next-step="1"]', $('[data-onboarding-step="0"]'));
  if (next) next.disabled = !configured;
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
  state.onboardingStep = clamp(step, 0, 3);
  $$('[data-onboarding-step]').forEach((node) => { node.hidden = Number(node.dataset.onboardingStep) !== state.onboardingStep; });
  $$('[data-step-button]').forEach((node) => {
    const nodeStep = Number(node.dataset.stepButton);
    node.classList.toggle("active", nodeStep === state.onboardingStep);
    node.classList.toggle("complete", nodeStep < state.onboardingStep);
    node.disabled = nodeStep > state.onboardingStep || (!state.auth?.password_configured && nodeStep > 0);
  });
  if (state.onboardingStep >= 1) renderOnboardingData();
}

function renderOnboardingData() {
  const accounts = state.accounts;
  $("#onboarding-accounts").innerHTML = accounts.length ? accounts.map((account) => `<div class="mini-row"><span class="status-dot ${escapeHtml(account.health || "unknown")}"></span><strong>${escapeHtml(account.email || account.label || account.name)}</strong><small>${escapeHtml(account.provider)}</small></div>`).join("") : '<p class="muted">No providers connected yet.</p>';
  renderProviderPicker($("#onboarding-providers"), false);
  const harnesses = state.harnesses.filter((harness) => harness.installed).slice(0, 6);
  $("#onboarding-harnesses").innerHTML = harnesses.length ? harnesses.map((harness) => `<div class="mini-row"><strong>${escapeHtml(harness.display_name || harness.name)}</strong><span class="pill ${harness.connected ? "success" : "neutral"}">${harness.connected ? "Connected" : "Detected"}</span></div>`).join("") : '<p class="muted">No supported coding harness was detected on this machine.</p>';
  $("#ready-summary").innerHTML = statCards([
    ["Accounts", accounts.length],
    ["Connected harnesses", state.harnesses.filter((harness) => harness.connected).length],
    ["Middleware rules", state.middleware?.rules?.length || 0],
    ["Daemon", state.health?.status || "online"],
  ]);
}

async function finishOnboarding() {
  const button = $("#finish-onboarding");
  button.disabled = true;
  try {
    await api("/admin/web/onboarding", { method: "POST", body: JSON.stringify({ completed: true }) });
    state.auth.onboarding_completed = true;
    toast("Onboarding complete.");
    selectView("dashboard", true);
  } catch (error) { toast(error.message, "danger"); }
  finally { button.disabled = false; }
}

async function restartOnboarding() {
  try {
    await api("/admin/web/onboarding", { method: "POST", body: JSON.stringify({ completed: false }) });
    state.auth.onboarding_completed = false;
    showOnboardingStep(0);
    selectView("onboarding", true);
  } catch (error) { toast(error.message, "danger"); }
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
  providers: loadProviders,
  harnesses: () => loadHarnesses(false),
  credentials: loadCredentials,
  dario: loadDario,
  middleware: loadMiddleware,
  notifications: loadNotifications,
  traces: () => loadTraces(false),
  onboarding: async () => { await refreshSharedData(); renderOnboardingData(); },
};

function selectView(requested, updateHash = true) {
  let view = VIEW_COPY[requested] ? requested : "dashboard";
  if (!state.auth?.onboarding_completed) view = "onboarding";
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
  if (updateHash && location.hash !== `#${view}`) history.pushState(null, "", `#${view}`);
  if (view === "onboarding") showOnboardingStep(state.onboardingStep);
  VIEW_LOADERS[view]?.().catch((error) => toast(error.message, "danger"));
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
async function loadHealth() {
  const response = await fetch("/health", { credentials: "same-origin" });
  if (!response.ok) throw new Error(`Daemon health returned HTTP ${response.status}`);
  state.health = await response.json();
  const health = state.health;
  $("#daemon-status").className = "pill success";
  $("#daemon-status").textContent = `Online · ${health.version}`;
  $("#sidebar-dot").className = "status-dot healthy";
  $("#sidebar-status").textContent = "Daemon online";
  $("#sidebar-uptime").textContent = `Up ${formatAge(health.uptime_s)}`;
  $("#sidebar-version").textContent = `v${health.version}`;
  $("#about-version").textContent = health.version;
  return health;
}

async function loadAccounts() {
  const data = await api("/admin/accounts");
  state.accounts = data.accounts || [];
  return state.accounts;
}

async function loadHarnessesData(refresh = false) {
  const data = await api(`/admin/harnesses${refresh ? "?refresh=1" : ""}`);
  state.harnesses = data.harnesses || [];
  return state.harnesses;
}

async function refreshSharedData() {
  const results = await Promise.allSettled([
    loadHealth(), loadAccounts(), loadHarnessesData(false),
    api("/admin/middleware"), api("/admin/analytics?since_minutes=60"),
  ]);
  if (results[3].status === "fulfilled") state.middleware = results[3].value;
  if (results[4].status === "fulfilled") state.analytics = results[4].value;
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

function renderLimits(node, providers) {
  node.innerHTML = providers.length ? providers.map((provider) => {
    const used = quotaPercent(provider);
    const label = provider.quota?.state || provider.plan || provider.source || "Observed usage";
    return `<div class="limit-row"><div><strong>${escapeHtml(provider.provider)}</strong><small>${escapeHtml(label)}</small></div><progress max="100" value="${escapeHtml(used)}"></progress><span>${escapeHtml(`${formatNumber(used)}% used`)}</span></div>`;
  }).join("") : '<p class="muted">No provider quota observations yet.</p>';
}

function compactAccounts(accounts) {
  return accounts.length ? accounts.slice(0, 6).map((account) => `<div class="account-row"><span class="status-dot ${escapeHtml(account.health || "unknown")}"></span><div><strong>${escapeHtml(account.email || account.label || account.name)}</strong><small>${escapeHtml(account.provider)} · ${escapeHtml(account.health || account.status)}</small></div>${account.needs_reauth ? `<button data-reauth-id="${escapeHtml(account.id)}" data-reauth-provider="${escapeHtml(account.provider)}">Re-auth</button>` : ""}</div>`).join("") : '<p class="muted">No accounts connected.</p>';
}

async function loadDashboard() {
  const requests = [
    loadHealth(), loadAccounts(), loadHarnessesData(false),
    api("/admin/analytics?since_minutes=60"), api("/admin/analytics?since_minutes=1440"),
    api("/admin/limits"), api("/admin/dario"), api("/admin/update"),
    api(`/traces/summaries?limit=6`),
  ];
  const [health, accounts, harnesses, hour, day, limits, darioResult, update, traces] = await Promise.allSettled(requests);
  if (health.status === "rejected") throw health.reason;
  state.analytics = hour.status === "fulfilled" ? hour.value : state.analytics;
  state.limits = limits.status === "fulfilled" ? limits.value : { providers: [] };
  state.dario = darioResult.status === "fulfilled" ? darioResult.value : null;
  state.update = update.status === "fulfilled" ? update.value : null;
  const hourTotals = analyticsTotals(hour.value);
  const dayTotals = analyticsTotals(day.value);
  $("#dashboard-lede").textContent = `Alex ${state.health.version} has been online ${formatAge(state.health.uptime_s)} with ${formatInteger(state.health.in_flight)} request${state.health.in_flight === 1 ? "" : "s"} in flight.`;
  $("#dashboard-stats").innerHTML = statCards([
    ["Last hour", `${formatInteger(hourTotals.requests)} requests`, `${formatInteger(hourTotals.errors)} errors`],
    ["Last 24 hours", `${formatInteger(dayTotals.requests)} requests`, `${formatInteger(dayTotals.input + dayTotals.output)} tokens`],
    ["24h cost", formatMoney(dayTotals.cost), "Recorded estimate"],
    ["In flight", formatInteger(state.health.in_flight), `Uptime ${formatAge(state.health.uptime_s)}`],
  ]);
  const banner = $("#update-banner");
  banner.hidden = !state.update?.update_available;
  banner.innerHTML = state.update?.update_available ? `<div><strong>Alex ${escapeHtml(state.update.latest)} is available</strong><span>You are running ${escapeHtml(state.update.current)}.</span></div><button data-apply-dashboard-update>Review update</button>` : "";
  $('[data-apply-dashboard-update]', banner)?.addEventListener("click", () => selectView("general", true));
  renderLimits($("#dashboard-limits"), state.limits.providers || []);
  $("#dashboard-accounts").innerHTML = compactAccounts(state.accounts);
  bindAccountActions($("#dashboard-accounts"));
  $("#dashboard-harnesses").innerHTML = state.harnesses.filter((item) => item.installed).slice(0, 5).map((item) => `<div class="mini-row"><strong>${escapeHtml(item.display_name || item.name)}</strong><span class="pill ${item.connected ? "success" : "neutral"}">${item.connected ? "Connected" : "Detected"}</span></div>`).join("") || '<p class="muted">No supported harness detected.</p>';
  $("#dashboard-dario").innerHTML = state.dario ? facts([["Health", state.dario.health], ["Active generation", state.dario.active_generation_id], ["Generations", state.dario.generations?.length || 0], ["Route enabled", state.dario.route_enabled]]) : '<p class="muted">Dario mode is not enabled.</p>';
  const traceRows = traces.status === "fulfilled" ? traces.value.traces || [] : [];
  $("#dashboard-traces").innerHTML = traceRows.map((trace) => `<button class="trace-mini" data-dashboard-trace="${escapeHtml(trace.id)}"><code>${escapeHtml(trace.model || trace.id)}</code><span>${escapeHtml(trace.provider || "unrouted")} · ${escapeHtml(formatTime(trace.ts_request_ms))}</span><b>${escapeHtml(trace.status ?? "—")}</b></button>`).join("") || '<p class="muted">No recent traces.</p>';
  bindActions($("#dashboard-traces"), "[data-dashboard-trace]", (event) => { selectView("traces", true); openTrace(event.currentTarget.dataset.dashboardTrace); });
}

/* 6. Settings destinations: General, Providers, Harnesses, Credentials, Dario,
 * Middleware, and Notifications. */
async function loadGeneral() {
  const [health, storage, update, channel] = await Promise.allSettled([
    loadHealth(), api("/admin/storage"), api("/admin/update"), api("/admin/update/channel"),
  ]);
  if (health.status === "rejected") throw health.reason;
  state.update = update.status === "fulfilled" ? update.value : null;
  const updateSummary = $("#update-summary");
  updateSummary.textContent = state.update ? `${state.update.current}${state.update.update_available ? ` → ${state.update.latest} available` : " is current"}` : (update.reason?.message || "Update status unavailable");
  if (channel.status === "fulfilled") $("#update-form").elements.channel.value = channel.value.channel;
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

async function submitUpdateChannel(event) {
  event.preventDefault();
  const channel = new FormData(event.currentTarget).get("channel");
  try {
    await api("/admin/update/channel", { method: "POST", body: JSON.stringify({ channel }) });
    toast(`Update channel set to ${channel}.`);
    await loadGeneral();
  } catch (error) { toast(error.message, "danger"); }
}

async function applyUpdate() {
  const button = $("#apply-update");
  button.disabled = true;
  try {
    const result = await api("/admin/update", { method: "POST" });
    toast(result.applying ? "Update started. The daemon may restart." : (result.message || "Update check completed."));
    await loadGeneral();
  } catch (error) { toast(error.message, "danger"); }
  finally { button.disabled = false; }
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

function renderProviderPicker(node, collapsible = true) {
  const counts = new Map();
  state.accounts.forEach((account) => counts.set(account.provider, (counts.get(account.provider) || 0) + 1));
  node.innerHTML = PROVIDERS.map(([id, label, mode]) => {
    const count = counts.get(providerCanonical(id)) || 0;
    const action = mode === "oauth" ? "Connect subscription" : mode === "import" ? "Review import" : "Configure below";
    return `<button data-provider="${escapeHtml(id)}" data-provider-mode="${escapeHtml(mode)}"><strong>${escapeHtml(label)}</strong><span>${escapeHtml(`${count} connected · ${action}`)}</span></button>`;
  }).join("");
  node.hidden = collapsible ? node.hidden : false;
  bindActions(node, "[data-provider]", (event) => {
    const button = event.currentTarget;
    if (button.dataset.providerMode === "oauth") startLogin(button.dataset.provider);
    else if (button.dataset.providerMode === "import") { selectView("providers", true); loadImportCandidates(); }
    else { selectView("providers", true); $(`#${button.dataset.provider}-form`)?.scrollIntoView({ behavior: "smooth" }); }
  });
}

function accountTitle(account) {
  return account.email || account.label || account.description || account.name || account.id;
}

function quotaMarkup(limits) {
  if (!limits || typeof limits !== "object") return '<span class="muted">No quota observation</span>';
  const percent = quotaPercent(limits);
  return `<div class="quota-line"><progress max="100" value="${escapeHtml(percent)}"></progress><span>${escapeHtml(`${formatNumber(percent)}% used`)}</span></div>`;
}

function renderAccountCard(account) {
  const health = account.health || "unknown";
  const needsReauth = account.kind === "oauth" && (account.needs_reauth || health === "auth_failed" || account.status !== "active");
  return `<article class="account-card" data-account-card="${escapeHtml(account.id)}">
    <div class="account-head"><span class="status-dot ${escapeHtml(health)}"></span><div><span class="section-kicker">${escapeHtml(account.provider)}</span><h3>${escapeHtml(accountTitle(account))}</h3><small>${escapeHtml(health)} · ${escapeHtml(account.kind)}${account.paused ? " · paused" : ""}</small></div></div>
    ${quotaMarkup(account.limits)}
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
  const [accounts, analytics] = await Promise.all([loadAccounts(), api("/admin/accounts/analytics?since_minutes=1440&bucket_minutes=60")]);
  renderUsageAnalytics(analytics);
  renderProviderPicker($("#provider-picker"), true);
  const providers = [...new Set(accounts.map((account) => account.provider))];
  const routings = await Promise.all(providers.map((provider) => api(`/admin/routing/${encodeURIComponent(provider)}`).catch(() => null)));
  $("#provider-accounts").innerHTML = accounts.length ? `${accounts.map(renderAccountCard).join("")}${routings.filter(Boolean).map(routingEditor).join("")}` : '<article class="settings-card"><p class="muted">No provider accounts connected. Choose Add provider to begin.</p></article>';
  bindAccountActions($("#provider-accounts"));
  $$('.routing-form', $("#provider-accounts")).forEach((form) => form.addEventListener("submit", saveRouting));
  await Promise.allSettled([loadOpenRouterModels(), loadExo(), loadCLIProxyAPI(), loadImportCandidates()]);
}

async function testCredentials() {
  const button = $("#test-credentials");
  const output = $("#credential-test-result");
  button.disabled = true;
  inlineMessage(output, "Sending a low-cost routed request through each active provider…", "neutral");
  try {
    const data = await api("/admin/accounts/test", { method: "POST" });
    output.hidden = false;
    output.className = `inline-message ${data.healthy === data.total ? "success" : "warning"}`;
    output.innerHTML = `<strong>${escapeHtml(`${data.healthy}/${data.total} credentials passed`)}</strong><ul>${(data.results || []).map((result) => `<li>${result.ok ? "✓" : "×"} ${escapeHtml(result.provider)} · ${escapeHtml(result.latency_ms)} ms · ${escapeHtml(result.message)}</li>`).join("")}</ul>`;
    await loadAccounts();
  } catch (error) { inlineMessage(output, error.message); }
  finally { button.disabled = false; }
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
    node.innerHTML = (data.candidates || []).map((candidate) => `<div class="mini-row"><div><strong>${escapeHtml(candidate.label)}</strong><small>${escapeHtml(candidate.provider)} · ${escapeHtml(candidate.kind)} · ${escapeHtml(candidate.source_path)}</small></div><button data-import-source="${escapeHtml(candidate.source)}">Review & import</button></div>`).join("") || '<p class="muted">No importable CLI credentials detected.</p>';
    bindActions(node, "[data-import-source]", importCredential);
  } catch (error) { renderError(node, error, "Credential scan failed"); }
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
    <div class="card-heading"><div><span class="section-kicker">${harness.installed ? "DETECTED" : "NOT DETECTED"}</span><h3>${escapeHtml(harness.display_name || harness.name)}</h3><p>${escapeHtml(harness.version || harness.binary || "Version unavailable")}</p></div><span class="pill ${harness.connected ? "success" : harness.installed ? "neutral" : "warning"}">${harness.connected ? "Connected" : harness.installed ? "Available" : "Unavailable"}</span></div>
    ${support ? `<p class="inline-message warning">${escapeHtml(support)}</p>` : ""}
    ${facts([["Binary", harness.binary], ["Config directory", harness.config_dir], ["Models", harness.models_total ?? harness.models?.length], ["Last checked", harness.checked_ms ? formatTime(harness.checked_ms) : "Current scan"]])}
    <div class="card-actions">${harness.supports_connect && harness.installed ? `<button class="primary" data-harness-mutate="${escapeHtml(harness.connected ? "disconnect" : "connect")}">${harness.connected ? "Disconnect" : "Connect"}</button><button data-harness-refresh ${harness.connected ? "" : "disabled"}>Refresh models/config</button>` : ""}</div>
    <form class="form-grid harness-override" data-harness-override><label>Binary override<input name="binary" value="${escapeHtml(override.binary || "")}" placeholder="Auto-detect"></label><label>Config directory<input name="config_dir" value="${escapeHtml(override.config_dir || "")}" placeholder="Default"></label><button>Save override</button><button type="button" data-clear-override>Clear</button></form>
    <label class="toggle-line"><input data-tool-capture type="checkbox" ${harness.tool_capture_enabled ? "checked" : ""} ${captureSupported && harness.connected ? "" : "disabled"}>Capture tool calls ${captureSupported ? "" : "(not yet supported)"}</label>
  </article>`;
}

async function loadHarnesses(refresh = false) {
  const harnesses = await loadHarnessesData(refresh);
  $("#harness-summary").innerHTML = statCards([
    ["Detected", harnesses.filter((item) => item.installed).length],
    ["Connected", harnesses.filter((item) => item.connected).length],
    ["Configurable", harnesses.filter((item) => item.supports_connect).length],
    ["Tool capture", harnesses.filter((item) => item.tool_capture_enabled).length],
  ]);
  const list = $("#harness-list");
  list.innerHTML = harnesses.map(renderHarnessCard).join("") || '<p class="muted">Harness detection returned no entries.</p>';
  $$('.harness-card', list).forEach(bindHarnessCard);
  renderOnboardingData();
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
  selectView("providers", true);
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
  const paste = session.mode === "paste" && session.state === "pending" ? '<form data-login-complete class="form-grid"><label>Authorization code or callback URL<input name="input" required autocomplete="off"></label><button>Complete login</button></form>' : "";
  const html = `<div class="login-status"><span class="step-kicker">PROVIDER AUTHORIZATION</span><h3>${escapeHtml(session.provider || reauthProvider || "Provider")} · ${escapeHtml(session.state)}</h3>${session.user_code ? `<p>Enter code <code>${escapeHtml(session.user_code)}</code></p>` : ""}${target ? `<a class="primary button-link" href="${escapeHtml(target)}" target="_blank" rel="noopener">Open authorization page</a>` : ""}${paste}${session.state === "pending" ? '<p><span class="spinner"></span> Waiting for authorization…</p>' : ""}${session.error ? `<p class="inline-message danger">${escapeHtml(session.error)}</p>` : ""}</div>`;
  setLoginFlow(html);
  loginFlowNodes().forEach((node) => $('[data-login-complete]', node)?.addEventListener("submit", (event) => reauthProvider ? completeReauth(event, reauthProvider) : completeLogin(event, session.login_id)));
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
    setLoginFlow(`<strong>${escapeHtml(provider)} login complete</strong><p>Credential updated. Run ping checks to verify it.</p>`);
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

async function loadTraces(append = false) {
  const data = await api(`/traces/summaries?${traceQuery(append)}`);
  state.traceCursor = data.next_cursor;
  const list = $("#trace-list");
  if (!append) list.replaceChildren();
  (data.traces || []).forEach((trace) => {
    const button = document.createElement("button");
    button.className = `trace-row ${finite(trace.status) >= 400 || trace.error ? "error" : ""}`;
    button.innerHTML = `<code>${escapeHtml(trace.model || trace.id)}</code><span>${escapeHtml(trace.provider || "unrouted")} · ${escapeHtml(trace.harness || "unknown harness")}<small>${escapeHtml(formatTime(trace.ts_request_ms))}</small></span><span>${escapeHtml(trace.status ?? "—")}</span>`;
    button.addEventListener("click", () => openTrace(trace.id));
    list.append(button);
  });
  if (!list.children.length) list.innerHTML = '<div class="empty-state"><p>No matching body-free trace summaries. Route a request or change the metadata filters.</p></div>';
  $("#more-traces").hidden = !data.has_more;
}

function middlewareRecords(attempt) {
  const records = parseList(attempt.middleware_decisions);
  if (!records.length) return '<p class="muted">No middleware decisions recorded.</p>';
  return `<ul class="decision-list">${records.map((record) => `<li><code>${escapeHtml(record.rule_name || record.rule_id || "unknown rule")}</code><span class="pill ${record.state === "matched" ? "success" : "neutral"}">${escapeHtml(record.state || "unknown")}</span>${record.action ? `<span>${escapeHtml(record.action)}</span>` : ""}${record.suppressed ? '<span class="danger">suppressed</span>' : ""}${record.explanation ? `<span>${escapeHtml(record.explanation)}</span>` : ""}</li>`).join("")}</ul>`;
}

function renderAttempt(attempt, index) {
  return `<article class="attempt"><h4>Attempt ${escapeHtml(attempt.attempt_number || attempt.attempt || index + 1)}</h4>${facts([["Provider", attempt.provider || attempt.upstream_provider], ["Model", attempt.model || attempt.routed_model], ["Account", attempt.account_id], ["Status", attempt.status], ["Error", attempt.error?.message || attempt.error || attempt.error_kind], ["Latency", attempt.latency_ms === undefined ? null : `${attempt.latency_ms} ms`]])}<h5>Middleware decisions</h5>${middlewareRecords(attempt)}<details><summary>Attempt metadata</summary><pre>${escapeHtml(JSON.stringify(attempt, null, 2))}</pre></details></article>`;
}

function bodyDetails(trace) {
  const bodies = [["request", "Client request", trace.req_body_path], ["upstream-request", "Upstream request", trace.upstream_req_body_path], ["response", "Client response", trace.resp_body_path]];
  if (trace.via_dario) bodies.push(["dario-upstream-request", "Dario upstream request", true], ["dario-upstream-response", "Dario upstream response", true]);
  return bodies.filter(([, , available]) => available).map(([kind, label]) => `<details class="lazy-data" data-body-kind="${kind}"><summary>${escapeHtml(label)}</summary><pre>Open to load only this body.</pre></details>`).join("") || '<p class="muted">No stored bodies are available for this trace.</p>';
}

function renderTraceDetail(id, data) {
  const trace = data.trace || data;
  const attempts = parseList(trace.attempts);
  const detail = $("#trace-detail");
  detail.classList.add("open");
  detail.innerHTML = `<div class="card-heading"><div><h3>Trace ${escapeHtml(id)}</h3><span class="muted">${escapeHtml(formatTime(trace.ts_request_ms))}</span></div><button data-close-detail>Close</button></div><h4>Summary</h4>${facts([["Status", trace.status], ["Latency", trace.latency_ms === null || trace.latency_ms === undefined ? null : `${trace.latency_ms} ms`], ["Input tokens", trace.input_tokens], ["Output tokens", trace.output_tokens], ["Error", trace.error || trace.error_kind], ["Session", trace.session_id], ["Run", trace.run_id]])}<h4>Provenance</h4>${facts([["Harness", trace.harness], ["Client format", trace.client_format], ["Provider", trace.upstream_provider], ["Upstream format", trace.upstream_format], ["Requested model", trace.requested_model], ["Routed model", trace.routed_model], ["Original model", trace.original_model], ["Served model", trace.served_model], ["Account", trace.account_id], ["Original account", trace.original_account_id], ["Served account", trace.served_account_id], ["Via Dario", trace.via_dario], ["Dario generation", trace.dario_generation], ["Routing explanation", trace.substitution_reason]])}<h4>Attempts and middleware</h4><div class="attempt-list">${attempts.length ? attempts.map(renderAttempt).join("") : '<p class="muted">No attempt records stored.</p>'}</div><h4>Stored bodies</h4><div class="lazy-list">${bodyDetails(trace)}</div>${trace.session_id ? `<h4>Session</h4><details class="lazy-data" data-transcript="${escapeHtml(trace.session_id)}"><summary>Conversation turns</summary><div>Open to load one bounded page of turn summaries.</div></details>` : ""}`;
  $('[data-close-detail]', detail).addEventListener("click", () => { detail.classList.remove("open"); detail.innerHTML = '<div class="empty-state"><span>⌁</span><h2>Select a trace</h2><p>Inspect routing, attempts, middleware, bodies, and conversation turns.</p></div>'; });
  $$('[data-body-kind]', detail).forEach((node) => node.addEventListener("toggle", () => { if (node.open && !node.dataset.loaded) loadTraceBody(id, node); }));
  $$('[data-transcript]', detail).forEach((node) => node.addEventListener("toggle", () => { if (node.open && !node.dataset.loaded) loadTranscript(node); }));
  detail.scrollIntoView({ behavior: matchMedia("(prefers-reduced-motion: reduce)").matches ? "auto" : "smooth" });
}

// This is deliberately metadata-only. It must never fetch trace body bytes or a
// full transcript; explicit disclosure toggles below own those requests.
async function openTrace(id) {
  try { renderTraceDetail(id, await api(`/traces/${encodeURIComponent(id)}/metadata`)); }
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
  return `<div class="executed-tools"><h6>Executed tools</h6>${tools.map((tool) => {
    const args = typeof tool.arguments === "string" ? tool.arguments : JSON.stringify(tool.arguments, null, 2);
    return `<details><summary>${escapeHtml(tool.tool_name || "tool")} ${tool.is_error ? '<span class="danger">error</span>' : ""}</summary>${args ? `<strong>Arguments</strong><pre>${escapeHtml(args)}</pre>` : ""}${tool.result ? `<strong>Result</strong><pre>${escapeHtml(tool.result)}</pre>` : ""}</details>`;
  }).join("")}</div>`;
}

function renderTurn(turn) {
  const assistant = turn.assistant || parseList(turn.assistant_blocks).filter((block) => block.type === "text").map((block) => block.text).join("\n\n");
  return `<article class="turn">${turn.user ? `<div><strong>User</strong><pre>${escapeHtml(turn.user)}</pre></div>` : ""}${assistant ? `<div><strong>Assistant</strong><pre>${escapeHtml(assistant)}</pre></div>` : ""}${renderExecutedTools(turn.executed_tools)}${facts([["Trace", turn.trace_id], ["Model", turn.model || turn.served_model], ["Status", turn.status], ["Input tokens", turn.input_tokens], ["Output tokens", turn.output_tokens]])}</article>`;
}

function renderTurnSummary(turn) {
  return `<details class="turn-summary" data-turn-trace="${escapeHtml(turn.trace_id)}"><summary><span><code>${escapeHtml(turn.model || turn.trace_id)}</code> · ${escapeHtml(turn.provider || "unrouted")}</span><span>${escapeHtml(turn.status ?? "—")} · ${escapeHtml(formatTime(turn.ts_request_ms))}</span></summary><div class="turn-detail muted">Open to load only this turn.</div></details>`;
}

function replaceTranscriptPage(target, html) {
  target.replaceChildren();
  target.insertAdjacentHTML("afterbegin", html);
}

async function loadTranscriptTurn(node) {
  node.dataset.loaded = "true";
  const target = $(".turn-detail", node);
  target.textContent = "Loading this turn…";
  try {
    const data = await api(`/traces/${encodeURIComponent(node.dataset.turnTrace)}/turn`);
    target.classList.remove("muted");
    target.innerHTML = renderTurn(data.turn);
  } catch (error) { delete node.dataset.loaded; target.textContent = `Could not load turn: ${error.message}`; }
}

async function loadTranscriptPage(node, cursor) {
  const target = $(".session-turns", node);
  target.textContent = "Loading a bounded page…";
  const params = new URLSearchParams({ limit: String(TURN_PAGE_SIZE) });
  if (cursor) { params.set("after_ms", cursor.after_ms); params.set("after_id", cursor.after_id); }
  try {
    const data = await api(`/traces/sessions/${encodeURIComponent(node.dataset.transcript)}/transcript/page?${params}`);
    const turns = (data.turns || []).map(renderTurnSummary).join("") || '<p class="muted">No turns found.</p>';
    const controls = `<div class="turn-page-controls"><button data-turn-previous ${node._pageIndex ? "" : "disabled"}>Previous page</button><span>Page ${node._pageIndex + 1} · up to ${TURN_PAGE_SIZE} turns</span><button data-turn-next ${data.has_more ? "" : "disabled"}>Next page</button></div>`;
    replaceTranscriptPage(target, `${turns}${controls}`);
    $$('[data-turn-trace]', target).forEach((turn) => turn.addEventListener("toggle", () => { if (turn.open && !turn.dataset.loaded) loadTranscriptTurn(turn); }));
    $('[data-turn-previous]', target).addEventListener("click", () => { if (node._pageIndex > 0) { node._pageIndex -= 1; loadTranscriptPage(node, node._pageStarts[node._pageIndex]); } });
    $('[data-turn-next]', target).addEventListener("click", () => { if (data.next_cursor) { node._pageStarts = node._pageStarts.slice(0, node._pageIndex + 1); node._pageStarts.push(data.next_cursor); node._pageIndex += 1; loadTranscriptPage(node, data.next_cursor); } });
  } catch (error) { target.textContent = `Could not load turns: ${error.message}`; }
}

async function loadTranscript(node) {
  node.dataset.loaded = "true";
  node._pageStarts = [null];
  node._pageIndex = 0;
  const holder = $("div", node);
  holder.className = "session-turns";
  await loadTranscriptPage(node, null);
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
  $("#finish-onboarding").addEventListener("click", finishOnboarding);
  $("#restart-onboarding").addEventListener("click", restartOnboarding);
  $("#logout").addEventListener("click", logout);
  $$('[data-next-step]').forEach((button) => button.addEventListener("click", () => showOnboardingStep(Number(button.dataset.nextStep))));
  $$('[data-step-button]').forEach((button) => button.addEventListener("click", () => { if (!button.disabled) showOnboardingStep(Number(button.dataset.stepButton)); }));
  $$('nav [data-view]').forEach((button) => button.addEventListener("click", () => selectView(button.dataset.view, true)));
  $$('[data-go]').forEach((button) => button.addEventListener("click", () => selectView(button.dataset.go, true)));
  $$('[data-refresh-card]').forEach((button) => button.addEventListener("click", refreshCurrentView));
  $("#mobile-menu").addEventListener("click", () => document.body.classList.toggle("nav-open"));
  $("#global-refresh").addEventListener("click", refreshCurrentView);
  $("#quick-refresh").addEventListener("click", refreshCurrentView);
  $("#refresh-interval").addEventListener("change", setRefreshInterval);
  $("#run-ping").addEventListener("click", testCredentials);
  $("#test-credentials").addEventListener("click", testCredentials);
  $("#add-provider").addEventListener("click", () => { const picker = $("#provider-picker"); picker.hidden = !picker.hidden; });
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
  $("#update-form").addEventListener("submit", submitUpdateChannel);
  $("#apply-update").addEventListener("click", applyUpdate);
  $("#storage-prune-form").addEventListener("submit", pruneStorage);
  $("#refresh-traces").addEventListener("click", () => loadTraces(false));
  $("#more-traces").addEventListener("click", () => loadTraces(true));
  $("#trace-filters").addEventListener("submit", applyTraceFilters);
  $("#trace-filters").addEventListener("reset", () => { state.traceFilters = {}; state.traceCursor = null; setTimeout(() => loadTraces(false), 0); });
  window.addEventListener("hashchange", () => selectView(location.hash.slice(1), false));
  window.addEventListener("popstate", () => selectView(location.hash.slice(1), false));
}

bindStaticEvents();
bootstrap();
