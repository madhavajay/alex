const tabs = [...document.querySelectorAll("[data-demo-tab]")];
const panels = [...document.querySelectorAll("[data-demo-panel]")];

function pairTraceTurns(turns) {
  const pairs = [];
  const pending = new Map();
  let latest = null;

  for (const turn of turns) {
    const traceId = turn.trace_id == null ? null : String(turn.trace_id);
    const isResponse = String(turn.direction ?? "").toLowerCase().includes("response");

    if (!isResponse) {
      const pair = { request: turn, response: null };
      pairs.push(pair);
      if (traceId) pending.set(traceId, pair);
      latest = pair;
      continue;
    }

    const pair = (traceId ? pending.get(traceId) : null) ?? (latest && !latest.response ? latest : null);
    if (pair && !pair.response) {
      pair.response = turn;
      if (traceId) pending.delete(traceId);
      if (pair === latest) latest = null;
    } else {
      pairs.push({ request: null, response: turn });
    }
  }

  return pairs;
}

function extractAnthropicRefusal(response) {
  if (!response?.body) return null;

  for (const line of String(response.body).split(/\r?\n/)) {
    if (!line.startsWith("data:")) continue;

    try {
      const frame = JSON.parse(line.slice(5));
      const delta = frame.delta ?? frame.message?.delta ?? {};
      const stopReason = delta.stop_reason ?? frame.message?.stop_reason;
      const details = delta.stop_details ?? frame.message?.stop_details;

      if (stopReason === "refusal" || details?.type === "refusal") {
        return {
          stopReason: stopReason ?? "refusal",
          type: details?.type ?? "refusal",
          category: details?.category ?? "unknown"
        };
      }
    } catch {
      // Non-JSON SSE frames are not refusal metadata.
    }
  }

  return null;
}

function buildCoveEventCard({ tone, label, title, description, route, meta, onClick }) {
  const card = document.createElement("button");
  card.type = "button";
  card.className = `cove-event-card ${tone}`;
  card.dataset.alexEvent = tone;
  card.addEventListener("click", onClick);

  const heading = document.createElement("span");
  heading.className = "cove-event-heading";
  const icon = document.createElement("span");
  icon.className = "cove-event-icon";
  icon.setAttribute("aria-hidden", "true");
  icon.textContent = tone === "routing" ? "↗" : "!";
  const labelElement = document.createElement("strong");
  labelElement.textContent = label;
  heading.append(icon, labelElement);

  const titleElement = document.createElement("span");
  titleElement.className = "cove-event-title";
  titleElement.textContent = title;

  const descriptionElement = document.createElement("span");
  descriptionElement.className = "cove-event-description";
  descriptionElement.textContent = description;

  card.append(heading, titleElement, descriptionElement);

  if (route) {
    const routeElement = document.createElement("code");
    routeElement.className = "cove-event-route";
    routeElement.textContent = route;
    card.append(routeElement);
  }

  const metaElement = document.createElement("span");
  metaElement.className = "cove-event-meta";
  metaElement.textContent = meta;
  card.append(metaElement);
  return card;
}

function openRawPair(player, pairIndex) {
  player.querySelector(`.cb-turn[data-turn="${pairIndex}"] .cb-inspect`)?.click();
}

function decorateCoveEventStream(player, bundle) {
  const pairs = pairTraceTurns(bundle.trace?.turns ?? []);
  const source = player.getAttribute("src") ?? "";
  const mainPairIndex = pairs.findIndex((pair) => String(pair.request?.body ?? "").startsWith("<system-reminder>"));
  const pairIndex = mainPairIndex >= 0 ? mainPairIndex : Math.max(0, pairs.length - 1);
  const pair = pairs[pairIndex];
  const turn = player.querySelector(`.cb-turn[data-turn="${pairIndex}"]`);
  if (!turn || turn.querySelector("[data-alex-event]")) return;

  if (source.includes("refusal-failover")) {
    const response = turn.querySelector(".cb-response");
    if (!response) return;

    response.before(buildCoveEventCard({
      tone: "routing",
      label: "Alex routing",
      title: "Fable 5 → GPT-5.6 Sol matched",
      description: "Alex selected openai/gpt-5.6-sol using the configured fallback account.",
      route: "anthropic/claude-fable-5 → openai/gpt-5.6-sol",
      meta: "Model refusal · upstream_refusal · bio · Middleware: Fable 5 → GPT-5.6 Sol · reroute",
      onClick: () => openRawPair(player, pairIndex)
    }));
    return;
  }

  const refusal = extractAnthropicRefusal(pair?.response);
  const responseBubble = turn.querySelector(".cb-response .cb-bubble");
  if (!refusal || !responseBubble) return;

  responseBubble.replaceChildren(buildCoveEventCard({
    tone: "refusal",
    label: "Upstream refusal",
    title: "Fable 5 refused this request",
    description: "Anthropic returned HTTP 200 with a structured refusal signal.",
    meta: `HTTP ${pair.response.status ?? 200} · stop_reason: ${refusal.stopReason} · type: ${refusal.type} · category: ${refusal.category}`,
    onClick: () => openRawPair(player, pairIndex)
  }));
}

document.addEventListener("cove-bundle-ready", (event) => {
  if (event.target instanceof HTMLElement && event.target.matches("cove-player")) {
    decorateCoveEventStream(event.target, event.detail.bundle);
  }
});

function selectDemo(name) {
  for (const tab of tabs) {
    const selected = tab.dataset.demoTab === name;
    tab.setAttribute("aria-selected", String(selected));
    tab.tabIndex = selected ? 0 : -1;
  }

  for (const panel of panels) {
    panel.hidden = panel.dataset.demoPanel !== name;
  }
}

for (const [index, tab] of tabs.entries()) {
  tab.addEventListener("click", () => selectDemo(tab.dataset.demoTab));
  tab.addEventListener("keydown", (event) => {
    if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
    event.preventDefault();
    const offset = event.key === "ArrowRight" ? 1 : -1;
    const nextTab = tabs[(index + offset + tabs.length) % tabs.length];
    selectDemo(nextTab.dataset.demoTab);
    nextTab.focus();
  });
}

for (const button of document.querySelectorAll("[data-copy]")) {
  button.addEventListener("click", async () => {
    const label = button.querySelector("[data-copy-label]");
    const original = label.textContent;

    try {
      await navigator.clipboard.writeText(button.dataset.copy);
      label.textContent = "Copied";
    } catch {
      label.textContent = "Select & copy";
    }

    window.setTimeout(() => {
      label.textContent = original;
    }, 1800);
  });
}

selectDemo("plain");
