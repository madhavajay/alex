import { readFile, rm } from 'node:fs/promises';
import path from 'node:path';
import type { TestRuntime } from './runtime';

const runtimePath = path.join(__dirname, '.runtime.json');

function signal(pid: number, value: NodeJS.Signals) {
  try {
    process.kill(-pid, value);
  } catch {
    try {
      process.kill(pid, value);
    } catch {}
  }
}

async function waitForExit(pid: number) {
  const deadline = Date.now() + 3_000;
  while (Date.now() < deadline) {
    try {
      process.kill(pid, 0);
    } catch {
      return;
    }
    await new Promise(resolve => setTimeout(resolve, 50));
  }
  signal(pid, 'SIGKILL');
}

export default async function globalTeardown() {
  const source = await readFile(runtimePath, 'utf8').catch(() => null);
  if (!source) return;
  const runtime = JSON.parse(source) as TestRuntime;
  signal(runtime.daemonPid, 'SIGTERM');
  signal(runtime.fakeprovPid, 'SIGTERM');
  await Promise.all([waitForExit(runtime.daemonPid), waitForExit(runtime.fakeprovPid)]);
  await rm(runtimePath, { force: true });
  if (path.basename(runtime.tempDir).startsWith('alex-webui-')) {
    await rm(runtime.tempDir, { recursive: true, force: true });
  }
}
