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
const failurePath = path.join(repoRoot, "crates/alex-proxy/tests/fixtures/middleware/anthropic-fable-refusal-200.json");
const builtinPath = path.join(repoRoot, "crates/alex-middleware/src/builtins.rs");
const rulePath = path.join(siteRoot, "data/fable-to-sol-rule.json");
const useAnywherePath = path.join(siteRoot, "data/use-anywhere-vector.json");
const askAnotherPath = path.join(siteRoot, "data/ask-another-model-vector.json");
const corePath = path.join(repoRoot, "crates/alex-core/src/lib.rs");
const cliPath = path.join(repoRoot, "crates/alex/src/main.rs");
const harnessConnectPath = path.join(repoRoot, "crates/alex/src/harness_connect.rs");
const resumePath = path.join(repoRoot, "crates/alex/src/resume.rs");
const storePath = path.join(repoRoot, "crates/alex-store/src/lib.rs");
const proxyPath = path.join(repoRoot, "crates/alex-proxy/src/lib.rs");

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
  assert(vector.expected_decision.scope.scope === rule.then.reroute.scope, "fallback scope must match rule");
  assert(rule.then.reroute.providers.includes(vector.expected_attempts[1].provider), "successful provider must be allowed by rule");

  const requiredRustLiterals = [
    rule.id,
    rule.name,
    rule.when.models[0],
    rule.when.providers[0],
    ...rule.when.error_kinds,
    rule.then.reroute.model,
    rule.then.reroute.providers[0],
    rule.then.reroute.effort,
    rule.then.reroute.reason
  ];
  for (const literal of requiredRustLiterals) {
    assert(builtinSource.includes(literal), `Rust built-in is missing ${literal}`);
  }

  const requiredRustStructure = [
    "HookPoint::AttemptResult",
    "Capability::RouteOverride",
    "Capability::SessionPin",
    "ProviderModeV1::Only",
    "RouteScopeKindV1::Session",
    "max_attempts: Some(3)"
  ];
  for (const fragment of requiredRustStructure) {
    assert(builtinSource.includes(fragment), `Rust built-in is missing ${fragment}`);
  }

  assert(rule.hook === "attempt_result", "rule hook must be attempt_result");
  assert(rule.capabilities.join(",") === "route.override,session.pin", "rule capabilities must match the built-in");
  assert(rule.when.error_kinds.join(",") === "upstream_refusal", "the rule must match normalized refusals");
  assert(rule.when.stable_session === true, "session fallback requires a stable session");
  assert(rule.then.reroute.provider_mode === "only", "provider mode must remain only");
  assert(rule.then.reroute.scope === "session", "reroute must remain session-scoped");
  assert(rule.then.reroute.ttl_seconds === 86400, "session route must last 24 hours");
  assert(rule.then.reroute.required_capabilities.portable_history === true, "session history must be portable");
  assert(rule.then.reroute.effort === "high", "replacement effort must remain high");
  assert(rule.then.reroute.max_attempts === 3, "attempt guard must match the built-in");
}

function validateSourceCheckedScenario(vector, sourceFiles) {
  assert(vector.schema_version === 1, `${vector.id} schema must be version 1`);
  assert(Array.isArray(vector.steps) && vector.steps.length >= 5, `${vector.id} needs a complete walkthrough`);
  assert(Array.isArray(vector.source_contracts) && vector.source_contracts.length > 0, `${vector.id} needs source contracts`);
  const combinedSource = sourceFiles.map((entry) => entry.content).join("\n");
  for (const contract of vector.source_contracts) {
    assert(combinedSource.includes(contract), `${vector.id} source contract is missing ${contract}`);
  }
  return {
    ...vector,
    source_evidence: Object.fromEntries(sourceFiles.map((entry) => [
      path.relative(repoRoot, entry.path),
      sha256(entry.content)
    ]))
  };
}

function buildScenario(vector, failure, rule, hashes) {
  const refusalEvent = failure.body
    .split("\n")
    .filter((line) => line.startsWith("data:"))
    .map((line) => JSON.parse(line.slice(5).trim()))
    .find((event) => event?.delta?.stop_reason === "refusal");
  assert(refusalEvent, "failure fixture must contain a structured refusal event");
  return {
    schema_version: 1,
    id: "fable-to-sol-refusal",
    title: "Fable 5 refuses. The session keeps moving.",
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
        label: "Fable returns a refusal signal",
        detail: `HTTP ${failure.status} · refusal · ${refusalEvent.delta.stop_details.category}`,
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
        label: "The trace explains the session fallback",
        detail: "Later requests in this session stay on Sol for 24 hours",
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
  const [
    vectorRaw,
    failureRaw,
    ruleRaw,
    builtinSource,
    useAnywhereRaw,
    askAnotherRaw,
    coreSource,
    cliSource,
    harnessConnectSource,
    resumeSource,
    storeSource,
    proxySource
  ] = await Promise.all([
    readFile(vectorPath, "utf8"),
    readFile(failurePath, "utf8"),
    readFile(rulePath, "utf8"),
    readFile(builtinPath, "utf8"),
    readFile(useAnywherePath, "utf8"),
    readFile(askAnotherPath, "utf8"),
    readFile(corePath, "utf8"),
    readFile(cliPath, "utf8"),
    readFile(harnessConnectPath, "utf8"),
    readFile(resumePath, "utf8"),
    readFile(storePath, "utf8"),
    readFile(proxyPath, "utf8")
  ]);
  const vector = JSON.parse(vectorRaw);
  const failure = JSON.parse(failureRaw);
  const rule = JSON.parse(ruleRaw);
  const useAnywhere = JSON.parse(useAnywhereRaw);
  const askAnother = JSON.parse(askAnotherRaw);
  validateSources(vector, failure, rule, builtinSource);

  const scenario = buildScenario(vector, failure, rule, {
    vector: sha256(vectorRaw),
    failure_fixture: sha256(failureRaw),
    rule: sha256(ruleRaw),
    rule_builtin: sha256(builtinSource)
  });
  const useAnywhereScenario = validateSourceCheckedScenario(useAnywhere, [
    { path: corePath, content: coreSource },
    { path: cliPath, content: cliSource },
    { path: harnessConnectPath, content: harnessConnectSource },
    { path: resumePath, content: resumeSource },
    { path: storePath, content: storeSource },
    { path: proxyPath, content: proxySource }
  ]);
  const askAnotherScenario = validateSourceCheckedScenario(askAnother, [
    { path: cliPath, content: cliSource },
    { path: resumePath, content: resumeSource },
    { path: storePath, content: storeSource },
    { path: proxyPath, content: proxySource }
  ]);
  const scenarios = [useAnywhereScenario, scenario, askAnotherScenario];

  await rm(outputRoot, { recursive: true, force: true });
  await mkdir(path.join(outputRoot, "assets"), { recursive: true });
  await cp(sourceRoot, outputRoot, { recursive: true });

  const templatePath = path.join(outputRoot, "index.html");
  let html = await readFile(templatePath, "utf8");
  html = html
    .replace("<!-- BUILD:USE_ANYWHERE_STEPS -->", scenarioMarkup(useAnywhereScenario.steps))
    .replace("<!-- BUILD:SCENARIO_STEPS -->", scenarioMarkup(scenario.steps))
    .replace("<!-- BUILD:ASK_ANOTHER_STEPS -->", scenarioMarkup(askAnotherScenario.steps))
    .replace("<!-- BUILD:RULE_SOURCE -->", escapeHtml(JSON.stringify(rule, null, 2)));
  assert(!html.includes("<!-- BUILD:"), "all build placeholders must be replaced");
  await writeFile(templatePath, html);
  await writeFile(path.join(outputRoot, "assets/scenario.json"), `${JSON.stringify(scenario, null, 2)}\n`);
  await writeFile(path.join(outputRoot, "assets/scenarios.json"), `${JSON.stringify(scenarios, null, 2)}\n`);

  const manifest = {};
  for (const relative of await filesRecursively(outputRoot)) {
    const content = await readFile(path.join(outputRoot, relative));
    manifest[relative] = sha256(content);
  }
  await writeFile(path.join(outputRoot, "build-manifest.json"), `${JSON.stringify(manifest, null, 2)}\n`);
  return { outputRoot, scenario, scenarios, manifest };
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const result = await build();
  console.log(`built ${Object.keys(result.manifest).length} files in ${path.relative(repoRoot, result.outputRoot)}`);
}
