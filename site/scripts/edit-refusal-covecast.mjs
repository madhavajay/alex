import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { copyFile, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";

const [inputArgument, outputArgument] = process.argv.slice(2);
if (!inputArgument || !outputArgument) {
  throw new Error("usage: node edit-refusal-covecast.mjs INPUT OUTPUT");
}

const inputPath = path.resolve(inputArgument);
const outputPath = path.resolve(outputArgument);
const workingDirectory = await mkdtemp(path.join(os.tmpdir(), "alex-covecast-edit-"));

try {
  execFileSync("unzip", ["-q", inputPath, "-d", workingDirectory]);

  const recordingPath = path.join(workingDirectory, "recording.cast");
  const recordingLines = (await readFile(recordingPath, "utf8")).trimEnd().split("\n");
  let replacements = 0;

  const editedLines = recordingLines.map((line, index) => {
    if (index === 0) return line;
    const event = JSON.parse(line);
    if (!Array.isArray(event) || typeof event[2] !== "string") return line;
    if (!event[2].includes("API Error: Claude Code is unable to respond to this request")) return line;

    const payload = event[2];
    const messageStart = payload.indexOf("\u001b[38;2;255;193;7m●");
    const spinnerStart = payload.indexOf("\u001b[38;2;215;119;87m✢", messageStart);
    if (messageStart < 0 || spinnerStart < 0) {
      throw new Error("refusal frame did not contain the expected color/layout markers");
    }

    const yellow = "\u001b[38;2;255;193;7m";
    const grey = "\u001b[38;2;153;153;153m";
    const reset = "\u001b[39m";
    const nextIndentedLine = "\r\u001b[2C\u001b[1B";
    const nextLine = "\r\u001b[1B";

    const replacement = [
      `${yellow}⏺ Fable 5's safeguards flagged this message. The safeguards are intentionally broad right now and may flag safe and routine coding,`,
      `${nextIndentedLine}cybersecurity, or biology work. These measures let us bring you Mythos-level capabilities sooner, and we're working to refine them.`,
      `${nextIndentedLine}Switched to Opus 4.8. Send feedback with /feedback or learn more${reset}`,
      `${nextLine}${grey}  ⎿  Tip: You can configure model switch behavior in /config${reset}`,
      `${nextLine}\u001b[K${nextLine}`
    ].join("");

    event[2] = `${payload.slice(0, messageStart)}${replacement}${payload.slice(spinnerStart)}`;
    replacements += 1;
    return JSON.stringify(event);
  });

  if (replacements !== 1) {
    throw new Error(`expected to edit one refusal frame, edited ${replacements}`);
  }

  const editedRecording = `${editedLines.join("\n")}\n`;
  await writeFile(recordingPath, editedRecording);

  const bundlePath = path.join(workingDirectory, "bundle.json");
  const bundle = JSON.parse(await readFile(bundlePath, "utf8"));
  const recordingMember = bundle.members?.find((member) => member.name === "recording.cast");
  if (!recordingMember) throw new Error("bundle manifest has no recording.cast member");

  const recordingBytes = Buffer.from(editedRecording);
  recordingMember.sha256 = createHash("sha256").update(recordingBytes).digest("hex");
  recordingMember.size_bytes = recordingBytes.byteLength;
  await writeFile(bundlePath, `${JSON.stringify(bundle, null, 2)}\n`);

  const temporaryOutput = path.join(workingDirectory, "edited.covecast");
  execFileSync("zip", [
    "-9", "-X", "-q", temporaryOutput,
    "bundle.json",
    "recording.cast",
    "recording-manifest.json",
    "trace.json",
    "summary.json"
  ], { cwd: workingDirectory });

  await copyFile(temporaryOutput, outputPath);
  console.log("Updated the refusal notice and configuration tip");
} finally {
  await rm(workingDirectory, { recursive: true, force: true });
}
