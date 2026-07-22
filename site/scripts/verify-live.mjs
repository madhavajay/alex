import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import { pathToFileURL } from "node:url";

function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

function assetUrl(baseUrl, relativePath, expectedHash) {
  const base = new URL(baseUrl);
  const campaign = new URLSearchParams(base.search);
  const encodedPath = relativePath.split("/").map(encodeURIComponent).join("/");
  const url = new URL(encodedPath, base);
  for (const [key, value] of campaign) url.searchParams.append(key, value);
  url.searchParams.set("alex_deploy", expectedHash.slice(0, 12));
  return url;
}

export async function verifyDeploymentOnce(
  baseUrl,
  manifestPath,
  fetchImpl = fetch
) {
  const manifest = JSON.parse(await readFile(manifestPath, "utf8"));
  const mismatches = [];

  for (const [relativePath, expectedHash] of Object.entries(manifest)) {
    const url = assetUrl(baseUrl, relativePath, expectedHash);
    try {
      const response = await fetchImpl(url, {
        headers: { Accept: "*/*" },
        redirect: "follow",
        referrerPolicy: "no-referrer",
      });
      if (!response.ok) {
        mismatches.push({
          path: relativePath,
          reason: `HTTP ${response.status}`,
        });
        continue;
      }
      const actualHash = sha256(Buffer.from(await response.arrayBuffer()));
      if (actualHash !== expectedHash) {
        mismatches.push({
          path: relativePath,
          reason: "SHA-256 mismatch",
          expected: expectedHash,
          actual: actualHash,
        });
      }
    } catch (error) {
      mismatches.push({
        path: relativePath,
        reason: error instanceof Error ? error.message : String(error),
      });
    }
  }

  return {
    ok: mismatches.length === 0,
    checked: Object.keys(manifest).length,
    mismatches,
  };
}

export async function waitForDeployment(
  baseUrl,
  manifestPath,
  {
    timeoutMs = 240_000,
    intervalMs = 5_000,
    fetchImpl = fetch,
    onAttempt = () => {},
  } = {}
) {
  const deadline = Date.now() + timeoutMs;
  let attempt = 0;
  let result;

  do {
    attempt += 1;
    result = await verifyDeploymentOnce(baseUrl, manifestPath, fetchImpl);
    onAttempt({ attempt, result });
    if (result.ok) return { ...result, attempts: attempt };
    if (Date.now() >= deadline) break;
    await new Promise((resolve) =>
      setTimeout(
        resolve,
        Math.min(intervalMs, Math.max(0, deadline - Date.now()))
      )
    );
  } while (Date.now() <= deadline);

  return { ...result, attempts: attempt };
}

async function main() {
  const [baseUrl, manifestPath, timeoutSeconds = "240"] = process.argv.slice(2);
  if (!baseUrl || !manifestPath || !/^\d+$/.test(timeoutSeconds)) {
    console.error(
      "usage: node scripts/verify-live.mjs <base-url> <manifest-path> [timeout-seconds]"
    );
    process.exitCode = 2;
    return;
  }

  const result = await waitForDeployment(baseUrl, manifestPath, {
    timeoutMs: Number(timeoutSeconds) * 1_000,
    onAttempt: ({ attempt, result: current }) => {
      if (!current.ok)
        console.log(
          `live verification attempt ${attempt}: ${current.mismatches.length} file(s) not current yet`
        );
    },
  });

  if (!result.ok) {
    console.error(JSON.stringify(result, null, 2));
    process.exitCode = 1;
    return;
  }
  console.log(
    `verified ${result.checked} deployed files after ${result.attempts} attempt(s)`
  );
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(process.argv[1]).href
) {
  await main();
}
