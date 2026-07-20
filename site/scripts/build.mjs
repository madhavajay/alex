import { createHash } from "node:crypto";
import { cp, mkdir, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import path from "node:path";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const siteRoot = path.resolve(scriptDir, "..");
const repoRoot = path.resolve(siteRoot, "..");
const sourceRoot = path.join(siteRoot, "src");
const outputRoot = path.join(siteRoot, "dist");

const vectorPath = path.join(repoRoot, "crates/alex-proxy/tests/fixtures/middleware/fable-to-sol-vector.json");
const failurePath = path.join(repoRoot, "crates/alex-proxy/tests/fixtures/middleware/anthropic-fable-unavailable-529.json");
const builtinPath = path.join(repoRoot, "crates/alex-middleware/src/builtins.rs");
const rulePath = path.join(siteRoot, "data/fable-to-sol-rule.json");

function assert(condition, message) {
  if (!condition) throw new Error(`site build invariant failed: ${message}`);
}

function escapeHtml(value) {
  return String(value)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

function sha256(value) {
  return createHash("sha256").update(value).digest("hex");
}

function validateSources(vector, failure, rule, builtinSource) {
  assert(vector.failure_fixture === failure.name, "vector must reference the failure fixture");
  assert(vector.request.model === rule.when.models[0], "requested model must match rule");
  assert(vector.expected_attempts[0].status === failure.status, "first status must come from failure fixture");
  assert(vector.expected_attempts[0].provider === failure.provider, "first provider must come from failure fixture");
  assert(vector.expected_decision.target.model === rule.then.reroute.model, "decision model must match rule");
  assert(vector.expected_decision.scope.ttl_seconds === rule.then.reroute.ttl_seconds, "session TTL must match rule");
  assert(rule.then.reroute.providers.includes(vector.expected_attempts[1].provider), "successful provider must be allowed by rule");

  const requiredRustLiterals = [
    rule.id,
    rule.name,
    rule.when.models[0],
    rule.when.models[1],
    rule.when.providers[0],
    String(rule.when.status[0]),
    rule.when.status[1],
    ...rule.when.body_contains_any,
    rule.then.reroute.model,
    rule.then.reroute.providers[0],
    String(rule.then.reroute.ttl_seconds),
    rule.then.reroute.notice,
    rule.then.reroute.reason
  ];
  for (const literal of requiredRustLiterals) {
    const rustSpelling = literal === "86400" ? "86_400" : literal;
    assert(builtinSource.includes(rustSpelling), `Rust built-in is missing ${literal}`);
  }

  const requiredRustStructure = [
    "HookPoint::AttemptResult",
    "Capability::AttemptReadErrorBody",
    "Capability::RouteOverride",
    "Capability::SessionPin",
    "Capability::ResponsePrependText",
    "ErrorClass::Capacity",
    "ErrorClass::Server",
    "ProviderModeV1::Only",
    "RouteScopeKindV1::Session",
    "max_attempts: Some(3)",
    "portable_history: true"
  ];
  for (const fragment of requiredRustStructure) {
    assert(builtinSource.includes(fragment), `Rust built-in is missing ${fragment}`);
  }

  assert(rule.hook === "attempt_result", "rule hook must be attempt_result");
  assert(rule.capabilities.join(",") === [
    "attempt.read_error_body",
    "route.override",
    "session.pin",
    "response.prepend_text"
  ].join(","), "rule capabilities must match the built-in");
  assert(rule.when.error_classes.join(",") === "capacity,server", "error classes must match the built-in");
  assert(rule.then.reroute.provider_mode === "only", "provider mode must remain only");
  assert(rule.then.reroute.scope === "session", "reroute must remain session-scoped");
  assert(rule.then.reroute.max_attempts === 3, "attempt guard must match the built-in");
  assert(rule.then.reroute.required_capabilities.portable_history === true, "portable history must remain required");
}

function buildScenario(vector, failure, rule, hashes) {
  const errorBody = JSON.parse(failure.body);
  return {
    schema_version: 1,
    id: "fable-to-sol-overload",
    title: "Fable 5 is full. The session keeps moving.",
    description: vector.description,
    source: {
      vector: path.relative(repoRoot, vectorPath),
      failure_fixture: path.relative(repoRoot, failurePath),
      rule_builtin: path.relative(repoRoot, builtinPath),
      sha256: hashes
    },
    steps: [
      {
        id: "request",
        label: "Request enters Alex",
        detail: `${vector.request.harness} asks for ${vector.request.model}`,
        kind: "request"
      },
      {
        id: "fable-attempt",
        label: "Alex routes to Fable 5",
        detail: `${vector.expected_attempts[0].provider} receives the first attempt`,
        kind: "route"
      },
      {
        id: "capacity-signal",
        label: "Fable returns a verified capacity signal",
        detail: `HTTP ${failure.status} · ${errorBody.error.type}`,
        kind: "signal"
      },
      {
        id: "rule-match",
        label: "The middleware rule matches",
        detail: rule.then.reroute.reason,
        kind: "decision"
      },
      {
        id: "sol-attempt",
        label: "Alex retries with Sol 5.6",
        detail: `${vector.expected_attempts[1].provider} serves ${vector.expected_attempts[1].model}`,
        kind: "route"
      },
      {
        id: "trace",
        label: "The session is pinned and the trace explains why",
        detail: `${rule.then.reroute.ttl_seconds / 3600} hour pin · next turn skips Anthropic`,
        kind: "success"
      }
    ],
    request: vector.request,
    expected_decision: vector.expected_decision,
    expected_attempts: vector.expected_attempts,
    next_turn: vector.next_turn,
    failure: {
      name: failure.name,
      provider: failure.provider,
      status: failure.status,
      error_kind: failure.error_kind
    },
    rule
  };
}

function scenarioMarkup(steps) {
  return steps.map((step, index) => `
            <li class="demo-step ${escapeHtml(step.kind)}" data-demo-step data-label="${escapeHtml(step.label)}">
              <span class="step-index" aria-hidden="true">${index + 1}</span>
              <span class="step-copy"><strong>${escapeHtml(step.label)}</strong><span>${escapeHtml(step.detail)}</span></span>
            </li>`).join("");
}

async function filesRecursively(directory, prefix = "") {
  const result = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const relative = path.join(prefix, entry.name);
    if (entry.isDirectory()) result.push(...await filesRecursively(path.join(directory, entry.name), relative));
    else result.push(relative);
  }
  return result.sort();
}

export async function build() {
  const [vectorRaw, failureRaw, ruleRaw, builtinSource] = await Promise.all([
    readFile(vectorPath, "utf8"),
    readFile(failurePath, "utf8"),
    readFile(rulePath, "utf8"),
    readFile(builtinPath, "utf8")
  ]);
  const vector = JSON.parse(vectorRaw);
  const failure = JSON.parse(failureRaw);
  const rule = JSON.parse(ruleRaw);
  validateSources(vector, failure, rule, builtinSource);

  const scenario = buildScenario(vector, failure, rule, {
    vector: sha256(vectorRaw),
    failure_fixture: sha256(failureRaw),
    rule: sha256(ruleRaw),
    rule_builtin: sha256(builtinSource)
  });

  await rm(outputRoot, { recursive: true, force: true });
  await mkdir(path.join(outputRoot, "assets"), { recursive: true });
  await cp(sourceRoot, outputRoot, { recursive: true });

  const templatePath = path.join(outputRoot, "index.html");
  let html = await readFile(templatePath, "utf8");
  html = html
    .replace("<!-- BUILD:SCENARIO_STEPS -->", scenarioMarkup(scenario.steps))
    .replace("<!-- BUILD:RULE_SOURCE -->", escapeHtml(JSON.stringify(rule, null, 2)));
  assert(!html.includes("<!-- BUILD:"), "all build placeholders must be replaced");
  await writeFile(templatePath, html);
  await writeFile(path.join(outputRoot, "assets/scenario.json"), `${JSON.stringify(scenario, null, 2)}\n`);

  const manifest = {};
  for (const relative of await filesRecursively(outputRoot)) {
    const content = await readFile(path.join(outputRoot, relative));
    manifest[relative] = sha256(content);
  }
  await writeFile(path.join(outputRoot, "build-manifest.json"), `${JSON.stringify(manifest, null, 2)}\n`);
  return { outputRoot, scenario, manifest };
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const result = await build();
  console.log(`built ${Object.keys(result.manifest).length} files in ${path.relative(repoRoot, result.outputRoot)}`);
}
