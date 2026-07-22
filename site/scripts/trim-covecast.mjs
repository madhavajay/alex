import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { copyFile, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";

const [inputArgument, outputArgument, cutoffArgument] = process.argv.slice(2);

if (!inputArgument || !outputArgument || !cutoffArgument) {
  throw new Error("usage: node trim-covecast.mjs INPUT OUTPUT CUTOFF_SECONDS");
}

const cutoffSeconds = Number(cutoffArgument);
if (!Number.isFinite(cutoffSeconds) || cutoffSeconds <= 0) {
  throw new Error("cutoff seconds must be a positive number");
}

const inputPath = path.resolve(inputArgument);
const outputPath = path.resolve(outputArgument);
const workingDirectory = await mkdtemp(path.join(os.tmpdir(), "alex-covecast-trim-"));

try {
  execFileSync("unzip", ["-q", inputPath, "-d", workingDirectory]);

  const recordingPath = path.join(workingDirectory, "recording.cast");
  const recordingLines = (await readFile(recordingPath, "utf8")).trimEnd().split("\n");
  const header = recordingLines.shift();
  JSON.parse(header);

  const keptEvents = [];
  for (const line of recordingLines) {
    const event = JSON.parse(line);
    if (!Array.isArray(event) || !Number.isFinite(Number(event[0]))) {
      throw new Error("recording.cast contains an invalid event");
    }
    if (Number(event[0]) <= cutoffSeconds) keptEvents.push(line);
  }

  if (keptEvents.length === 0) {
    throw new Error("cutoff removed every recording event");
  }

  const trimmedRecording = `${[header, ...keptEvents].join("\n")}\n`;
  await writeFile(recordingPath, trimmedRecording);

  const bundlePath = path.join(workingDirectory, "bundle.json");
  const bundle = JSON.parse(await readFile(bundlePath, "utf8"));
  const recordingMember = bundle.members?.find((member) => member.name === "recording.cast");
  if (!recordingMember) throw new Error("bundle manifest has no recording.cast member");

  const recordingBytes = Buffer.from(trimmedRecording);
  recordingMember.sha256 = createHash("sha256").update(recordingBytes).digest("hex");
  recordingMember.size_bytes = recordingBytes.byteLength;
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

  const lastEvent = JSON.parse(keptEvents.at(-1));
  console.log(`Trimmed ${recordingLines.length - keptEvents.length} events after ${cutoffSeconds}s; last event is ${lastEvent[0]}s`);
} finally {
  await rm(workingDirectory, { recursive: true, force: true });
}
