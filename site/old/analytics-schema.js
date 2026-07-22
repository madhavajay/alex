export const ANALYTICS_SCHEMA = Object.freeze({
  page_view: [
    "path",
    "referrer_host",
    "utm_source",
    "utm_medium",
    "utm_campaign",
    "utm_content",
    "utm_term"
  ],
  demo_started: ["demo_id", "entry_point"],
  demo_completed: ["demo_id", "steps_count"],
  demo_action_clicked: ["demo_id", "action"],
  rule_revealed: ["demo_id", "rule_id"],
  install_copied: ["surface"],
  download_clicked: ["platform", "surface"],
  provider_selected: ["provider", "surface"],
  route_interest_selected: ["provider", "harness"],
  cliproxyapi_docs_opened: ["surface"]
});

export const CAMPAIGN_KEYS = Object.freeze([
  "utm_source",
  "utm_medium",
  "utm_campaign",
  "utm_content",
  "utm_term"
]);

const SAFE_VALUE = /^[\w .:/+-]{0,120}$/u;

export function sanitizeProperties(eventName, properties = {}) {
  const allowed = ANALYTICS_SCHEMA[eventName];
  if (!allowed) return null;

  const clean = {};
  for (const key of allowed) {
    const value = properties[key];
    if (typeof value === "number" && Number.isFinite(value)) {
      clean[key] = value;
    } else if (typeof value === "string") {
      const trimmed = value.trim().slice(0, 120);
      if (SAFE_VALUE.test(trimmed)) clean[key] = trimmed;
    }
  }
  return clean;
}

export function campaignProperties(search = "") {
  const input = new URLSearchParams(search);
  const result = {};
  for (const key of CAMPAIGN_KEYS) {
    const value = input.get(key);
    if (value) result[key] = value;
  }
  return sanitizeProperties("page_view", result) ?? {};
}

export function withCampaignParameters(href, base, search = "") {
  const url = new URL(href, base);
  if (url.protocol !== "https:" && url.protocol !== "http:") return href;

  const incoming = new URLSearchParams(search);
  for (const key of CAMPAIGN_KEYS) {
    const value = incoming.get(key);
    if (value && !url.searchParams.has(key)) url.searchParams.set(key, value);
  }
  return url.href;
}
