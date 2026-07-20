import {
  ANALYTICS_SCHEMA,
  campaignProperties,
  sanitizeProperties,
  withCampaignParameters
} from "./analytics-schema.js";

const endpoint = "https://plausible.io/api/event";
const domain = "madhavajay.github.io";
const privacyEnabled = navigator.globalPrivacyControl === true
  || navigator.doNotTrack === "1"
  || window.doNotTrack === "1";

window.alexAnalytics = Object.freeze({ schema: ANALYTICS_SCHEMA });

export function captureEvent(eventName, properties = {}) {
  const clean = sanitizeProperties(eventName, properties);
  if (!clean) return false;

  document.dispatchEvent(new CustomEvent("alex:analytics", {
    detail: Object.freeze({ name: eventName, properties: clean })
  }));

  if (privacyEnabled || location.protocol === "file:") return true;

  const pageUrl = `${location.origin}${location.pathname}`;
  void fetch(endpoint, {
    method: "POST",
    headers: { "Content-Type": "text/plain" },
    body: JSON.stringify({
      name: eventName,
      url: pageUrl,
      domain,
      props: clean
    }),
    keepalive: true,
    credentials: "omit",
    referrerPolicy: "no-referrer"
  }).catch(() => {});
  return true;
}

export function preserveCampaignParameters(link, search = location.search) {
  link.href = withCampaignParameters(link.href, location.href, search);
}

const attribution = campaignProperties(location.search);
let referrerHost = "";
try {
  referrerHost = document.referrer ? new URL(document.referrer).hostname : "";
} catch {}

captureEvent("page_view", {
  path: location.pathname,
  referrer_host: referrerHost,
  ...attribution
});
