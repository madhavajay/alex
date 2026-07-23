import { spawn, type ChildProcess } from 'node:child_process';
import { chmod, mkdir, mkdtemp, rm, stat, writeFile } from 'node:fs/promises';
import { createWriteStream } from 'node:fs';
import { createServer } from 'node:net';
import { tmpdir } from 'node:os';
import path from 'node:path';
import type { TestRuntime } from './runtime';

const root = path.resolve(__dirname, '..');
const runtimePath = path.join(__dirname, '.runtime.json');

async function run(command: string, args: string[]) {
  await new Promise<void>((resolve, reject) => {
    const child = spawn(command, args, { cwd: root, stdio: 'inherit' });
    child.once('error', reject);
    child.once('exit', code => code === 0 ? resolve() : reject(new Error(`${command} exited with ${code}`)));
  });
}

async function executable(file: string) {
  const info = await stat(file).catch(() => null);
  if (!info?.isFile()) throw new Error(`missing test binary: ${file}`);
}

async function ephemeralPort() {
  const server = createServer();
  await new Promise<void>((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', resolve);
  });
  const address = server.address();
  if (!address || typeof address === 'string') throw new Error('could not reserve daemon port');
  await new Promise<void>((resolve, reject) => server.close(error => error ? reject(error) : resolve()));
  return address.port;
}

async function firstJsonLine(child: ChildProcess, timeoutMs: number) {
  return await new Promise<Record<string, string>>((resolve, reject) => {
    let buffer = '';
    const timer = setTimeout(() => reject(new Error('fakeprov handshake timed out')), timeoutMs);
    child.stdout?.setEncoding('utf8');
    child.stdout?.on('data', chunk => {
      buffer += chunk;
      const end = buffer.indexOf('\n');
      if (end < 0) return;
      clearTimeout(timer);
      try {
        resolve(JSON.parse(buffer.slice(0, end)));
      } catch (error) {
        reject(error);
      }
    });
    child.once('exit', code => {
      clearTimeout(timer);
      reject(new Error(`fakeprov exited before handshake with ${code}`));
    });
    child.once('error', reject);
  });
}

async function waitForHealth(baseUrl: string, child: ChildProcess) {
  const deadline = Date.now() + 60_000;
  while (Date.now() < deadline) {
    if (child.exitCode !== null) throw new Error(`daemon exited with ${child.exitCode}`);
    try {
      const response = await fetch(`${baseUrl}/health`, { signal: AbortSignal.timeout(1_000) });
      if (response.ok) return;
    } catch {}
    await new Promise(resolve => setTimeout(resolve, 250));
  }
  throw new Error('daemon did not become healthy within 60 seconds');
}

async function proxyRequest(baseUrl: string, localKey: string, index: number) {
  const anthropic = index % 2 === 0;
  const endpoint = anthropic ? '/v1/messages' : '/v1/chat/completions';
  const body = anthropic
    ? { model: 'claude-sonnet-4-5', max_tokens: 32, messages: [{ role: 'user', content: `web UI trace ${index}` }] }
    : { model: 'gpt-4.1', messages: [{ role: 'user', content: `web UI trace ${index}` }] };
  const response = await fetch(`${baseUrl}${endpoint}`, {
    method: 'POST',
    headers: {
      'content-type': 'application/json',
      'x-api-key': localKey,
      'x-session-id': `webui-seed-${index}`,
      'x-alex-harness': anthropic ? 'webui-playwright-anthropic' : 'webui-playwright-openai'
    },
    body: JSON.stringify(body)
  });
  if (!response.ok) throw new Error(`trace seed ${index} failed with ${response.status}: ${await response.text()}`);
}

async function waitForTraces(baseUrl: string, localKey: string, expected: number) {
  const deadline = Date.now() + 10_000;
  while (Date.now() < deadline) {
    const response = await fetch(`${baseUrl}/admin/traces?limit=50`, { headers: { 'x-api-key': localKey } });
    if (response.ok) {
      const payload = await response.json() as { traces?: unknown[] };
      if ((payload.traces?.length ?? 0) >= expected) return;
    }
    await new Promise(resolve => setTimeout(resolve, 100));
  }
  throw new Error(`expected ${expected} seeded traces`);
}

function stop(child: ChildProcess | undefined) {
  if (!child?.pid || child.exitCode !== null) return;
  try {
    process.kill(-child.pid, 'SIGTERM');
  } catch {
    child.kill('SIGTERM');
  }
}

export default async function globalSetup() {
  let fakeprov: ChildProcess | undefined;
  let daemon: ChildProcess | undefined;
  const tempDir = await mkdtemp(path.join(tmpdir(), 'alex-webui-'));
  try {
    const buildArgs = ['build'];
    if (!process.env.ALEX_BIN) buildArgs.push('-p', 'alex', '--bin', 'alex');
    if (!process.env.FAKEPROV_BIN) buildArgs.push('-p', 'alex-fakeprov', '--bin', 'alex-fakeprov');
    if (buildArgs.length > 1) {
      await run('cargo', buildArgs);
    }
    const alexBin = process.env.ALEX_BIN ? path.resolve(root, process.env.ALEX_BIN) : path.join(root, 'target/debug/alex');
    const fakeprovBin = process.env.FAKEPROV_BIN ? path.resolve(root, process.env.FAKEPROV_BIN) : path.join(root, 'target/debug/alex-fakeprov');
    await Promise.all([executable(alexBin), executable(fakeprovBin)]);

    const fakeLog = createWriteStream(path.join(tempDir, 'fakeprov.log'));
    fakeprov = spawn(fakeprovBin, ['--port', '0'], {
      cwd: root,
      detached: true,
      stdio: ['ignore', 'pipe', 'pipe']
    });
    fakeprov.stderr?.pipe(fakeLog);
    const handshake = await firstJsonLine(fakeprov, 10_000);
    const fakeBaseUrl = handshake.base_url;
    const fakeControlKey = handshake.control_key;
    if (!fakeBaseUrl || !fakeControlKey) throw new Error('fakeprov emitted an invalid handshake');

    const home = path.join(tempDir, 'home');
    const accountsDir = path.join(home, 'accounts');
    await mkdir(accountsDir, { recursive: true });
    const port = await ephemeralPort();
    const localKey = 'alx-webui-test-key';
    const config = [
      'host = "127.0.0.1"',
      `port = ${port}`,
      `data_dir = ${JSON.stringify(home)}`,
      `local_key = "${localKey}"`,
      'heartbeat_minutes = 0',
      'reauth_check_minutes = 0',
      'update_check_hours = 0',
      'anthropic_upstream = "direct"',
      'dario_mode_migrated = true',
      'dario_update_check_minutes = 0',
      ''
    ].join('\n');
    const accounts = [
      { id: 'mock-anthropic', provider: 'anthropic', kind: 'api_key', name: 'mock', api_key: 'mock-anthropic-key', status: 'active' },
      { id: 'mock-openai', provider: 'openai', kind: 'api_key', name: 'mock', api_key: 'mock-openai-key', status: 'active' }
    ];
    const configPath = path.join(home, 'config.toml');
    await writeFile(configPath, config);
    await Promise.all(accounts.map(account => writeFile(path.join(accountsDir, `${account.id}.json`), `${JSON.stringify(account, null, 2)}\n`)));
    await Promise.all([configPath, ...accounts.map(account => path.join(accountsDir, `${account.id}.json`))].map(file => chmod(file, 0o600)));

    const daemonLog = createWriteStream(path.join(tempDir, 'daemon.log'));
    const baseUrl = `http://127.0.0.1:${port}`;
    daemon = spawn(alexBin, ['daemon', '--host', '127.0.0.1', '--port', String(port)], {
      cwd: root,
      detached: true,
      env: {
        ...process.env,
        ALEX_HOME: home,
        ALEX_UPSTREAM_ANTHROPIC_URL: `${fakeBaseUrl}/anthropic`,
        ALEX_UPSTREAM_OPENAI_URL: `${fakeBaseUrl}/openai`,
        ALEX_UPSTREAM_CODEX_URL: `${fakeBaseUrl}/openai`,
        ALEX_UPSTREAM_XAI_URL: `${fakeBaseUrl}/xai`,
        ALEX_UPSTREAM_GEMINI_URL: `${fakeBaseUrl}/gemini`,
        ALEX_UPSTREAM_GEMINI_CODE_ASSIST_URL: `${fakeBaseUrl}/gemini`,
        ALEX_UPSTREAM_OPENROUTER_URL: `${fakeBaseUrl}/openrouter`,
        ALEX_UPSTREAM_KIMI_URL: `${fakeBaseUrl}/kimi`,
        ALEX_UPSTREAM_AMP_URL: `${fakeBaseUrl}/amp`
      },
      stdio: ['ignore', 'ignore', 'pipe']
    });
    daemon.stderr?.pipe(daemonLog);
    await waitForHealth(baseUrl, daemon);

    for (let index = 0; index < 28; index += 1) await proxyRequest(baseUrl, localKey, index);
    await waitForTraces(baseUrl, localKey, 28);

    const runtime: TestRuntime = {
      baseUrl,
      localKey,
      fakeBaseUrl,
      fakeControlKey,
      daemonPid: daemon.pid!,
      fakeprovPid: fakeprov.pid!,
      tempDir
    };
    await writeFile(runtimePath, `${JSON.stringify(runtime, null, 2)}\n`);
  } catch (error) {
    stop(daemon);
    stop(fakeprov);
    await rm(tempDir, { recursive: true, force: true });
    throw error;
  }
}
