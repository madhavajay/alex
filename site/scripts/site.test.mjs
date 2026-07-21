import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";
import path from "node:path";
import { fileURLToPath } from "node:url";

import {
  ANALYTICS_SCHEMA,
  campaignProperties,
  sanitizeProperties,
  withCampaignParameters
} from "../src/analytics-schema.js";
import { build } from "./build.mjs";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const siteRoot = path.resolve(scriptDir, "..");

test("build is deterministic and generated from the proxy fixture", async () => {
  const first = await build();
  const firstManifest = JSON.stringify(first.manifest);
  const second = await build();
  assert.equal(JSON.stringify(second.manifest), firstManifest);

  const scenario = JSON.parse(await readFile(path.join(siteRoot, "dist/assets/scenario.json"), "utf8"));
  assert.equal(scenario.request.model, "claude-fable-5");
  assert.equal(scenario.failure.status, 529);
  assert.equal(scenario.expected_decision.decision, "reroute");
  assert.equal(scenario.expected_decision.target.model, "gpt-5.6-sol");
  assert.equal(scenario.expected_attempts[1].provider, "openai");
  assert.equal(scenario.next_turn.anthropic_attempts, 0);
  assert.equal(scenario.steps.length, 6);
  for (const hash of Object.values(scenario.source.sha256)) assert.match(hash, /^[a-f0-9]{64}$/);

  const scenarios = JSON.parse(await readFile(path.join(siteRoot, "dist/assets/scenarios.json"), "utf8"));
  assert.deepEqual(scenarios.map(({ id }) => id), [
    "use-it-anywhere",
    "fable-to-sol-overload",
    "ask-another-model"
  ]);
  assert.equal(scenarios[0].trace.via_dario, true);
  assert.equal(scenarios[2].target.forked_from_harness, "claude");
  for (const sourceChecked of [scenarios[0], scenarios[2]]) {
    for (const hash of Object.values(sourceChecked.source_evidence)) assert.match(hash, /^[a-f0-9]{64}$/);
  }
});

test("built HTML has a useful static fallback and accessible controls", async () => {
  await build();
  const html = await readFile(path.join(siteRoot, "dist/index.html"), "utf8");
  assert.equal((html.match(/data-demo="/g) ?? []).length, 3);
  assert.equal((html.match(/data-demo-step/g) ?? []).length, 16);
  assert.equal((html.match(/data-action="start"/g) ?? []).length, 3);
  assert.equal((html.match(/data-demo-action=/g) ?? []).length, 3);
  assert.equal((html.match(/data-harness=/g) ?? []).length, 4);
  assert.match(html, /Play the whole route, pause it/);
  assert.match(html, /Your Claude subscription\. The harness you prefer/);
  assert.match(html, /Keep the first answer\. Ask for another opinion/);
  assert.match(html, /<noscript>/);
  assert.match(html, /Reveal the actual middleware rule/);
  assert.match(html, /example\.fable-overload-to-sol/);
  assert.match(html, /aria-live="polite"/);
  assert.doesNotMatch(html, /<!-- BUILD:/);
});

test("analytics schema exposes the full privacy-safe funnel", () => {
  assert.deepEqual(Object.keys(ANALYTICS_SCHEMA).sort(), [
    "cliproxyapi_docs_opened",
    "demo_action_clicked",
    "demo_completed",
    "demo_started",
    "download_clicked",
    "install_copied",
    "page_view",
    "provider_selected",
    "route_interest_selected",
    "rule_revealed"
  ]);

  const forbidden = ["prompt", "trace", "credential", "body", "email", "url", "referrer"];
  const fields = Object.values(ANALYTICS_SCHEMA).flat();
  for (const name of forbidden) assert(!fields.includes(name), `${name} must not be an analytics property`);

  assert.deepEqual(sanitizeProperties("demo_started", {
    demo_id: "fable-to-sol-overload",
    entry_point: "play_control",
    prompt: "secret",
    trace_id: "trace-123"
  }), {
    demo_id: "fable-to-sol-overload",
    entry_point: "play_control"
  });
  assert.equal(sanitizeProperties("not_declared", { anything: "no" }), null);
  assert.deepEqual(sanitizeProperties("demo_action_clicked", {
    demo_id: "ask-another-model",
    action: "resume_docs",
    url: "https://example.test/private"
  }), {
    demo_id: "ask-another-model",
    action: "resume_docs"
  });
  assert.deepEqual(sanitizeProperties("route_interest_selected", {
    provider: "cliproxyapi",
    harness: "pi",
    credential: "secret"
  }), {
    provider: "cliproxyapi",
    harness: "pi"
  });
});

test("analytics transport honors browser privacy signals", async () => {
  const source = await readFile(path.join(siteRoot, "src/analytics.js"), "utf8");
  assert.match(source, /globalPrivacyControl/);
  assert.match(source, /doNotTrack/);
  assert.match(source, /credentials: "omit"/);
  assert.match(source, /referrerPolicy: "no-referrer"/);
});

test("campaign attribution is allowlisted and outbound links opt in", async () => {
  assert.deepEqual(campaignProperties("?utm_source=docs&utm_campaign=v1&token=secret"), {
    utm_source: "docs",
    utm_campaign: "v1"
  });
  const outbound = new URL(withCampaignParameters(
    "https://github.com/madhavajay/alex/releases/latest?utm_source=kept#downloads",
    "https://madhavajay.github.io/alex/",
    "?utm_source=ignored&utm_campaign=v1&token=secret"
  ));
  assert.equal(outbound.searchParams.get("utm_source"), "kept");
  assert.equal(outbound.searchParams.get("utm_campaign"), "v1");
  assert.equal(outbound.searchParams.has("token"), false);
  assert.equal(outbound.hash, "#downloads");
  const html = await readFile(path.join(siteRoot, "src/index.html"), "utf8");
  const outboundLinks = [...html.matchAll(/<a\s+[^>]*href="https:\/\/[^\"]+"[^>]*>/g)].map((match) => match[0]);
  assert(outboundLinks.length >= 6);
  for (const link of outboundLinks) assert.match(link, /data-campaign-link/);
});

test("motion and mobile fallbacks are explicit", async () => {
  const css = await readFile(path.join(siteRoot, "src/styles.css"), "utf8");
  assert.match(css, /prefers-reduced-motion: reduce/);
  assert.match(css, /@media \(max-width: 760px\)/);
});

test("each walkthrough has isolated controls and completion analytics", async () => {
  const source = await readFile(path.join(siteRoot, "src/app.js"), "utf8");
  assert.match(source, /querySelectorAll\("\[data-demo\]"\)\.forEach\(setupDemo\)/);
  assert.match(source, /demo\.querySelectorAll\("\[data-demo-step\]"\)/);
  assert.match(source, /captureEvent\("demo_completed"/);
  assert.match(source, /captureEvent\("demo_action_clicked"/);
});
