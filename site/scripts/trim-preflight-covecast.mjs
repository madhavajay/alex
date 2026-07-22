import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { copyFile, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";

const [inputArgument, outputArgument] = process.argv.slice(2);
if (!inputArgument || !outputArgument) {
  throw new Error("usage: node trim-preflight-covecast.mjs INPUT OUTPUT");
}

const inputPath = path.resolve(inputArgument);
const outputPath = path.resolve(outputArgument);
const workingDirectory = await mkdtemp(path.join(os.tmpdir(), "alex-covecast-preflight-"));

function updateMember(bundle, name, bytes) {
  const member = bundle.members?.find((candidate) => candidate.name === name);
  if (!member) throw new Error(`bundle manifest has no ${name} member`);
  member.sha256 = createHash("sha256").update(bytes).digest("hex");
  member.size_bytes = bytes.byteLength;
}

try {
  execFileSync("unzip", ["-q", inputPath, "-d", workingDirectory]);

  const tracePath = path.join(workingDirectory, "trace.json");
  const trace = JSON.parse(await readFile(tracePath, "utf8"));
  if (!Array.isArray(trace.turns)) throw new Error("trace.json has no turns array");

  const originalTurnCount = trace.turns.length;
  trace.turns = trace.turns.filter((turn) => turn.phase !== "preflight");
  const removedTurnCount = originalTurnCount - trace.turns.length;
  if (removedTurnCount !== 2) {
    throw new Error(`expected to remove two preflight turns, removed ${removedTurnCount}`);
  }

  const traceBytes = Buffer.from(`${JSON.stringify(trace, null, 2)}\n`);
  await writeFile(tracePath, traceBytes);

  const traceCount = new Set(
    trace.turns.map((turn) => turn.trace_id).filter((traceId) => traceId != null)
  ).size;

  const summaryPath = path.join(workingDirectory, "summary.json");
  const summary = JSON.parse(await readFile(summaryPath, "utf8"));
  summary.trace_count = traceCount;
  const summaryBytes = Buffer.from(`${JSON.stringify(summary, null, 2)}\n`);
  await writeFile(summaryPath, summaryBytes);

  const bundlePath = path.join(workingDirectory, "bundle.json");
  const bundle = JSON.parse(await readFile(bundlePath, "utf8"));
  bundle.usage.trace_count = traceCount;
  updateMember(bundle, "trace.json", traceBytes);
  updateMember(bundle, "summary.json", summaryBytes);
  await writeFile(bundlePath, `${JSON.stringify(bundle, null, 2)}\n`);

  const temporaryOutput = path.join(workingDirectory, "trimmed.covecast");
  execFileSync("zip", [
    "-9", "-X", "-q", temporaryOutput,
    "bundle.json",
    "recording.cast",
    "recording-manifest.json",
    "trace.json",
    "summary.json"
  ], { cwd: workingDirectory });

  await copyFile(temporaryOutput, outputPath);
  console.log(`Removed ${removedTurnCount} preflight turns; ${trace.turnCount ?? trace.turns.length} turns and ${traceCount} traces remain`);
} finally {
  await rm(workingDirectory, { recursive: true, force: true });
}
