import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { createHash } from "node:crypto";
import { access, readFile, stat } from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { promisify } from "node:util";
import { fileURLToPath } from "node:url";

const scriptDirectory = path.dirname(fileURLToPath(import.meta.url));
const siteDirectory = path.resolve(scriptDirectory, "..");
const execFileAsync = promisify(execFile);

test("landing page contains the requested sections and links", async () => {
  const html = await readFile(path.join(siteDirectory, "src/index.html"), "utf8");
  const visibleText = html.replace(/<[^>]*>/g, "");
  assert.match(html, /images\/header\.jpg/);
  assert.match(html, /Sick of Fable refusing\?/);
  assert.match(html, /Claude with Fable 5 \+ Alex Middleware/);
  assert.match(html, /class="tab-brand-mark"/);
  assert.match(html, /assets\/images\/alex-icon-small\.png/);
  assert.match(html, /styles\.css\?v=20260722-small-icons/);
  assert.match(html, /class="tab-cta"[^>]*>Get Alex<\/a>/);
  assert.match(html, /How does it work\?/);
  assert.match(html, /configure multiple token providers/);
  assert.match(html, /custom middleware to reroute your session/);
  assert.match(html, /MoA — Mixture of Agents/);
  assert.match(html, /Problems Alex fixes/);
  assert.match(html, /Fable 5 guardrails kill your session/);
  assert.match(visibleText, /Alex can transparently switch/);
  assert.match(html, /transparently <strong>switch<\/strong>/);
  assert.match(html, /or <strong>fork<\/strong>/);
  assert.match(html, /alex auth import/);
  assert.match(html, /alex connect pi/);
  assert.match(html, /\/model alex\/\*/);
  assert.match(html, /github\.com\/madhavajay\/alex/);
  assert.match(html, /stable-label">Stable/);
  assert.match(html, /install-release\.sh \| sh/);
  assert.match(html, /beta-label">Beta/);
  assert.match(html, /install-beta\.sh \| sh/);
  assert.match(html, /class="button github-button"[^>]*target="_blank"[^>]*rel="noopener noreferrer"/);
  assert.match(html, /metrics\.syftbox\.net\/api\/script\.js/);
  assert.match(html, /data-site-id="75d48849af99"/);
  assert.match(html, /assets\/cove-player\.iife\.js/);
  assert.match(html, /<cove-player[\s\S]*src="\.\/assets\/claude-fable-5-refusal\.covecast"/);
  assert.match(html, /<cove-player[\s\S]*src="\.\/assets\/claude-fable-5-refusal-failover\.covecast"/);
  assert.match(html, /playback-speed="1"/);
  assert.doesNotMatch(html, /<cove-player[\s\S]*?\sfill(?:\s|>)/);
});

test("Cove replay and its vendored player assets are self-contained", async () => {
  const assets = path.join(siteDirectory, "src/assets");
  const refusalReplayPath = path.join(assets, "claude-fable-5-refusal.covecast");
  const failoverReplayPath = path.join(assets, "claude-fable-5-refusal-failover.covecast");
  await stat(refusalReplayPath);
  await stat(failoverReplayPath);
  const player = await readFile(path.join(assets, "cove-player.iife.js"), "utf8");
  assert.match(player, /new URL\(a,document\.baseURI\)\.href/);
  assert.match(player, /document\.currentScript\?\.src\|\|document\.baseURI/);
  await stat(path.join(assets, "vendor/asciinema-player.css"));
  await stat(path.join(assets, "vendor/asciinema-player.js"));

  const [{ stdout: recording }, { stdout: bundleJson }] = await Promise.all([
    execFileAsync("unzip", ["-p", failoverReplayPath, "recording.cast"], { encoding: "utf8" }),
    execFileAsync("unzip", ["-p", failoverReplayPath, "bundle.json"], { encoding: "utf8" })
  ]);
  const recordingLines = recording.trimEnd().split("\n");
  recordingLines.shift();
  const eventTimes = recordingLines.map((line) => Number(JSON.parse(line)[0]));
  assert.ok(Math.max(...eventTimes) <= 16, "replay must end by the 16-second cutoff");

  const bundle = JSON.parse(bundleJson);
  const recordingMember = bundle.members.find((member) => member.name === "recording.cast");
  assert.equal(recordingMember.size_bytes, Buffer.byteLength(recording));
  assert.equal(recordingMember.sha256, createHash("sha256").update(recording).digest("hex"));

  const { stdout: refusalRecording } = await execFileAsync(
    "unzip",
    ["-p", refusalReplayPath, "recording.cast"],
    { encoding: "utf8" }
  );
  const refusalOutput = refusalRecording
    .trimEnd()
    .split("\n")
    .slice(1)
    .map((line) => JSON.parse(line)[2] ?? "")
    .join("");
  assert.match(refusalOutput, /\u001b\[38;2;255;193;7m⏺ Fable 5's safeguards flagged this message/);
  assert.match(refusalOutput, /\u001b\[38;2;153;153;153m  ⎿  Tip: You can configure model switch behavior in \/config/);

  for (const replayPath of [refusalReplayPath, failoverReplayPath]) {
    const [{ stdout: traceJson }, { stdout: replayBundleJson }, { stdout: summaryJson }] = await Promise.all([
      execFileAsync("unzip", ["-p", replayPath, "trace.json"], { encoding: "utf8" }),
      execFileAsync("unzip", ["-p", replayPath, "bundle.json"], { encoding: "utf8" }),
      execFileAsync("unzip", ["-p", replayPath, "summary.json"], { encoding: "utf8" })
    ]);
    const replayTrace = JSON.parse(traceJson);
    const replayBundle = JSON.parse(replayBundleJson);
    const replaySummary = JSON.parse(summaryJson);
    assert.equal(replayTrace.turns.some((turn) => turn.phase === "preflight"), false);
    assert.equal(replayTrace.turns.length, 4);
    assert.equal(replayBundle.usage.trace_count, 2);
    assert.equal(replaySummary.trace_count, 2);

    for (const [memberName, memberBody] of [["trace.json", traceJson], ["summary.json", summaryJson]]) {
      const member = replayBundle.members.find((candidate) => candidate.name === memberName);
      assert.equal(member.size_bytes, Buffer.byteLength(memberBody));
      assert.equal(member.sha256, createHash("sha256").update(memberBody).digest("hex"));
    }
  }
});

test("the production build excludes local metadata", async () => {
  await import("./build.mjs");
  await assert.rejects(access(path.join(siteDirectory, "dist/assets/.DS_Store")));
});

test("the old site remains available as a complete static snapshot", async () => {
  const oldHtml = await readFile(path.join(siteDirectory, "old/index.html"), "utf8");
  assert.match(oldHtml, /Three source-checked walkthroughs/);
  await stat(path.join(siteDirectory, "old/styles.css"));
  await stat(path.join(siteDirectory, "old/app.js"));
  await stat(path.join(siteDirectory, "old/assets/scenarios.json"));
});

test("site styles preserve the Alex application design language", async () => {
  const css = await readFile(path.join(siteDirectory, "src/styles.css"), "utf8");
  assert.match(css, /--background: #1c1c1e/);
  assert.match(css, /--card: #28282a/);
  assert.match(css, /--primary: #0a84ff/);
  assert.match(css, /\.player-panel \.cb-timeline\s*{\s*display: none/);
  assert.match(css, /prefers-reduced-motion/);
});

test("Cove event streams surface refusal and Alex routing evidence", async () => {
  const app = await readFile(path.join(siteDirectory, "src/app.js"), "utf8");
  assert.match(app, /stopReason === "refusal"/);
  assert.match(app, /category: details\?\.category/);
  assert.match(app, /Upstream refusal/);
  assert.match(app, /HTTP \$\{pair\.response\.status \?\? 200\}/);
  assert.match(app, /Fable 5 → GPT-5\.6 Sol matched/);
  assert.match(app, /Model refusal · upstream_refusal · bio/);
  assert.match(app, /\.cb-inspect/);
});
