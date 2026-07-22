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
import { verifyDeploymentOnce, waitForDeployment } from "./verify-live.mjs";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const siteRoot = path.resolve(scriptDir, "..");
const repoRoot = path.resolve(siteRoot, "..");

function cssBlock(source, marker) {
  const markerIndex = source.indexOf(marker);
  assert.notEqual(markerIndex, -1, `missing CSS block: ${marker}`);
  const openingBrace = source.indexOf("{", markerIndex);
  let depth = 0;
  for (let index = openingBrace; index < source.length; index += 1) {
    if (source[index] === "{") depth += 1;
    if (source[index] === "}") depth -= 1;
    if (depth === 0) return source.slice(openingBrace + 1, index);
  }
  assert.fail(`unterminated CSS block: ${marker}`);
}

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
  assert.equal(scenario.next_turn.anthropic_attempts, 1);
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
  assert.match(html, /alex\.fable-5-to-gpt-5\.6-sol/);
  assert.match(html, /aria-live="polite"/);
  assert.doesNotMatch(html, /<!-- BUILD:/);
});

test("no-JavaScript rendering keeps content and hides only inert controls", async () => {
  await build();
  const [html, css] = await Promise.all([
    readFile(path.join(siteRoot, "dist/index.html"), "utf8"),
    readFile(path.join(siteRoot, "dist/styles.css"), "utf8")
  ]);

  assert.doesNotMatch(html, /<script[^>]*>[^<]*js-ready/);
  assert.match(html, /<noscript>[\s\S]*Every routing step, trace summary, and outbound action remains available/);
  assert.equal((html.match(/data-demo-step/g) ?? []).length, 16);
  assert.equal((html.match(/class="trace-panel"/g) ?? []).length, 3);
  assert.equal((html.match(/data-demo-action=/g) ?? []).length, 3);
  assert.match(css, /\.demo-controls\s*\{[^}]*display:\s*none/);
  assert.match(css, /\.js-ready \.demo-controls\s*\{[^}]*display:\s*flex/);

  const opacityRules = [...css.matchAll(/([^{}]+)\{([^{}]*opacity:[^{}]*)\}/g)]
    .filter(([, selector]) => selector.includes(".demo-step"));
  assert(opacityRules.length > 0);
  for (const [rule, selector] of opacityRules) {
    assert(selector.includes(".js-ready"), `static fallback must not be dimmed by ${rule.trim()}`);
  }
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

test("reduced-motion and mobile layouts disable movement and collapse wide grids", async () => {
  const css = await readFile(path.join(siteRoot, "src/styles.css"), "utf8");
  const reducedMotion = cssBlock(css, "@media (prefers-reduced-motion: reduce)");
  assert.match(reducedMotion, /scroll-behavior:\s*auto/);
  assert.match(reducedMotion, /transition:\s*none\s*!important/);
  assert.match(reducedMotion, /animation:\s*none\s*!important/);
  assert.match(reducedMotion, /\.js-ready \.demo-step\.is-active\s*\{[^}]*transform:\s*none/);

  const mobile = cssBlock(css, "@media (max-width: 760px)");
  assert.match(mobile, /\.demo-head, \.demo-grid, \.interest\s*\{[^}]*grid-template-columns:\s*1fr/);
  assert.match(mobile, /\.trace-panel\s*\{[^}]*border-left:\s*0[^}]*border-top:/);
  assert.match(mobile, /\.demo-controls\s*\{[^}]*width:\s*100%/);
  assert.match(mobile, /\.install-command\s*\{[^}]*flex-direction:\s*column/);
  assert.match(mobile, /\.demo-next\s*\{[^}]*flex-direction:\s*column/);
  assert.match(css, /\.trace-row strong\s*\{[^}]*overflow-wrap:\s*anywhere/);
});

test("each walkthrough has isolated controls and completion analytics", async () => {
  const source = await readFile(path.join(siteRoot, "src/app.js"), "utf8");
  assert.match(source, /querySelectorAll\("\[data-demo\]"\)\.forEach\(setupDemo\)/);
  assert.match(source, /demo\.querySelectorAll\("\[data-demo-step\]"\)/);
  assert.match(source, /captureEvent\("demo_completed"/);
  assert.match(source, /captureEvent\("demo_action_clicked"/);
});

test("live deployment verifier checks every manifest hash and campaign query", async () => {
  await build();
  const manifestPath = path.join(siteRoot, "dist/build-manifest.json");
  const manifest = JSON.parse(await readFile(manifestPath, "utf8"));
  const requests = [];
  const distFetch = async (input) => {
    const url = new URL(input);
    requests.push(url);
    const relativePath = decodeURIComponent(url.pathname.replace(/^\/alex\//, ""));
    try {
      return new Response(await readFile(path.join(siteRoot, "dist", relativePath)), { status: 200 });
    } catch {
      return new Response("missing", { status: 404 });
    }
  };

  const exact = await verifyDeploymentOnce(
    "https://madhavajay.github.io/alex/?utm_source=test&utm_campaign=v1",
    manifestPath,
    distFetch
  );
  assert.deepEqual(exact, { ok: true, checked: Object.keys(manifest).length, mismatches: [] });
  assert.equal(requests.length, Object.keys(manifest).length);
  for (const request of requests) {
    assert.equal(request.searchParams.get("utm_source"), "test");
    assert.equal(request.searchParams.get("utm_campaign"), "v1");
    assert.match(request.searchParams.get("alex_deploy"), /^[a-f0-9]{12}$/);
  }

  let calls = 0;
  const staleThenCurrent = async (input) => {
    calls += 1;
    if (calls <= Object.keys(manifest).length) return new Response("stale", { status: 200 });
    return distFetch(input);
  };
  const recovered = await waitForDeployment("https://madhavajay.github.io/alex/", manifestPath, {
    timeoutMs: 5_000,
    intervalMs: 0,
    fetchImpl: staleThenCurrent
  });
  assert.equal(recovered.ok, true);
  assert.equal(recovered.attempts, 2);
});

test("Pages workflow builds the locked site, preserves appcasts, and verifies the live artifact", async () => {
  const [workflow, appcastWorkflow] = await Promise.all([
    readFile(path.join(repoRoot, ".github/workflows/pages.yml"), "utf8"),
    readFile(path.join(repoRoot, ".github/workflows/dmg-appcast.yml"), "utf8")
  ]);
  assert.match(workflow, /branches:\s*\[main\]/);
  assert.match(workflow, /npm ci --ignore-scripts/);
  assert.match(workflow, /run:\s*npm test/);
  assert.match(workflow, /run:\s*npm run build/);
  assert.match(workflow, /name:\s*alex-public-site-\$\{\{ github\.sha \}\}/);
  assert.match(workflow, /ref:\s*gh-pages/);
  assert.match(workflow, /--exclude='appcast\.xml'/);
  assert.match(workflow, /--exclude='appcast-beta\.xml'/);
  assert.match(workflow, /node source\/site\/scripts\/verify-live\.mjs "\$SITE_URL" built\/build-manifest\.json 240/);
  assert.match(workflow, /SITE_URL:\s*https:\/\/madhavajay\.github\.io\/alex\/\?utm_source=github&utm_campaign=v1-deploy/);
  assert.match(workflow, /group:\s*gh-pages-deploy/);
  assert.match(appcastWorkflow, /group:\s*gh-pages-deploy/);
});
