import assert from 'node:assert/strict';
import { spawn, type ChildProcess } from 'node:child_process';
import { createServer } from 'node:http';
import { mkdtemp, readdir, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { basename, join } from 'node:path';
import test from 'node:test';
import { writeConfig } from '../src/config.js';
import { toolsCachePath } from '../src/config.js';
import { serve } from '../src/service.js';

type Probe = {
  initialize: boolean;
  tools: string[];
  call: boolean;
  timeout: boolean;
  missingTool: boolean;
  methodError: boolean;
  healthy: boolean;
  boundedMs: number;
  migrated: boolean;
};

test('relay-compatibility: legacy Node and Rust expose the same core behavior', async (t) => {
  const upstream = await fixtureUpstream();
  const fixtureId = `compat${process.pid}`;
  try {
    const node = await probeNode(t, upstream.url, fixtureId);
    if (!node) return;
    const rustBinary = process.env.COMMONKITD_BIN;
    if (!rustBinary) {
      t.diagnostic('COMMONKITD_BIN is absent; legacy baseline passed and Rust comparison runs in CI');
      return;
    }
    const rust = await probeRust(rustBinary, upstream.url, fixtureId);
    assert.deepEqual(
      { ...rust, boundedMs: rust.boundedMs < 5_000 },
      { ...node, boundedMs: node.boundedMs < 5_000 },
    );
  } finally {
    await upstream.close();
    await rm(toolsCachePath(fixtureId), { force: true });
  }
});

async function probeNode(t: test.TestContext, upstreamUrl: string, fixtureId: string): Promise<Probe | undefined> {
  const port = await freePort().catch((error: NodeJS.ErrnoException) => {
    if (error.code === 'EPERM') t.skip('loopback listen is unavailable');
    else throw error;
  });
  if (!port) return;
  const root = await mkdtemp(join(tmpdir(), 'relay-compat-node-'));
  const config = join(root, 'legacy.json');
  await writeConfig({ admin: { host: '127.0.0.1', port }, servers: [fixtureConfig(upstreamUrl, fixtureId)] }, config);
  const service = await serve({ configPath: config });
  try {
    await waitForNodeTools(port);
    const started = Date.now();
    const initialized = await rpc(`http://127.0.0.1:${port}/mcp`, undefined, 1, 'initialize', {
      protocolVersion: '2025-06-18', capabilities: {}, clientInfo: { name: 'compat', version: '1' },
    });
    const session = initialized.headers.get('mcp-session-id') ?? undefined;
    const tools = await rpc(`http://127.0.0.1:${port}/mcp`, session, 2, 'tools/list', {});
    const call = await rpc(`http://127.0.0.1:${port}/mcp`, session, 3, 'tools/call', { name: `${fixtureId}__echo`, arguments: { value: 'same' } });
    const timeout = await timesOut(`http://127.0.0.1:${port}/mcp`, session, `${fixtureId}__echo`);
    const missing = await rpc(`http://127.0.0.1:${port}/mcp`, session, 4, 'tools/call', { name: 'missing', arguments: {} });
    const method = await rpc(`http://127.0.0.1:${port}/mcp`, session, 5, 'unknown/method', {});
    const health = await fetch(`http://127.0.0.1:${port}/healthz`).then((response) => response.json()) as { ok: boolean };
    return normalize(initialized.body, tools.body, call.body, timeout, missing.body, method.body, health.ok, Date.now() - started, true);
  } finally {
    service.manager.stop();
    await new Promise<void>((resolve) => service.httpServer.close(() => resolve()));
    await rm(root, { recursive: true, force: true });
  }
}

async function waitForNodeTools(port: number) {
  const deadline = Date.now() + 5_000;
  while (Date.now() < deadline) {
    const status = await fetch(`http://127.0.0.1:${port}/status`).then((response) => response.json()) as { servers?: Array<{ cachedTools?: number }> };
    if ((status.servers?.[0]?.cachedTools ?? 0) > 0) return;
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
  throw new Error('legacy relay did not cache fixture tools');
}

async function probeRust(binary: string, upstreamUrl: string, fixtureId: string): Promise<Probe> {
  const root = await mkdtemp(join(tmpdir(), 'relay-compat-rust-'));
  const relayPort = await freePort();
  const env = {
    ...process.env,
    HOME: root,
    USERPROFILE: root,
    XDG_CONFIG_HOME: join(root, 'config'),
    XDG_DATA_HOME: join(root, 'data'),
    XDG_CACHE_HOME: join(root, 'cache'),
    APPDATA: join(root, 'appdata'),
    LOCALAPPDATA: join(root, 'localappdata'),
  };
  const child = spawn(binary, ['--port', '0', '--relay-port', String(relayPort)], {
    env,
    stdio: ['ignore', 'ignore', 'inherit'],
  });
  try {
    const tokenPath = await waitForFile(root, 'control.token');
    const discoveryPath = await waitForFile(root, 'daemon.json');
    const token = (await readFile(tokenPath, 'utf8')).trim();
    const discovery = JSON.parse(await readFile(discoveryPath, 'utf8')) as { port: number };
    const configPath = join(tokenPath.slice(0, -basename(tokenPath).length), 'relay.json');
    await writeFile(configPath, JSON.stringify({ servers: [fixtureConfig(upstreamUrl, fixtureId)] }));
    await control(discovery.port, token, '/control/v1/relay/restart', { confirmed: true });
    const headers = { authorization: `Bearer ${token}` };
    const url = `http://127.0.0.1:${relayPort}/mcp`;
    const started = Date.now();
    const initialized = await rpc(url, undefined, 1, 'initialize', { protocolVersion: '2025-06-18' }, headers);
    const tools = await rpc(url, undefined, 2, 'tools/list', {}, headers);
    const call = await rpc(url, undefined, 3, 'tools/call', { name: `${fixtureId}__echo`, arguments: { value: 'same' } }, headers);
    const timeout = await timesOut(url, undefined, `${fixtureId}__echo`, headers);
    const missing = await rpc(url, undefined, 4, 'tools/call', { name: 'missing', arguments: {} }, headers);
    const method = await rpc(url, undefined, 5, 'unknown/method', {}, headers);
    return normalize(initialized.body, tools.body, call.body, timeout, missing.body, method.body, initialized.response.ok, Date.now() - started, true);
  } finally {
    await stop(child);
    await rm(root, { recursive: true, force: true });
  }
}

function normalize(initialize: any, tools: any, call: any, timeout: boolean, missing: any, method: any, healthy: boolean, boundedMs: number, migrated: boolean): Probe {
  return {
    initialize: Boolean(initialize.result?.protocolVersion),
    tools: (tools.result?.tools ?? []).map((tool: { name: string }) => tool.name).filter((name: string) => !name.startsWith('relay_')),
    call: Boolean(call.result) && !call.error,
    timeout,
    missingTool: Boolean(missing.error),
    methodError: Boolean(method.error),
    healthy,
    boundedMs,
    migrated,
  };
}

function fixtureConfig(url: string, id: string) {
  return { id, remote: { type: 'streamable_http' as const, url } };
}

async function fixtureUpstream() {
  const server = createServer((request, response) => {
    let body = '';
    request.setEncoding('utf8');
    request.on('data', (chunk) => { body += chunk; });
    request.on('end', () => {
      const message = JSON.parse(body) as { id: unknown; method: string; params?: { arguments?: unknown } };
      const result = message.method === 'initialize'
        ? { protocolVersion: '2025-06-18', capabilities: { tools: {} }, serverInfo: { name: 'fixture', version: '1' } }
        : message.method === 'tools/list'
          ? { tools: [{ name: 'echo', description: 'echo', inputSchema: { type: 'object' } }] }
          : { content: [{ type: 'text', text: JSON.stringify(message.params?.arguments ?? {}) }] };
      const send = () => {
        response.writeHead(200, { 'content-type': 'application/json' });
        response.end(JSON.stringify({ jsonrpc: '2.0', id: message.id, result }));
      };
      if ((message.params?.arguments as { slow?: boolean } | undefined)?.slow) setTimeout(send, 500);
      else send();
    });
  });
  await new Promise<void>((resolve, reject) => server.once('error', reject).listen(0, '127.0.0.1', resolve));
  const address = server.address();
  assert.ok(address && typeof address === 'object');
  return {
    url: `http://127.0.0.1:${address.port}/mcp`,
    close: () => new Promise<void>((resolve) => server.close(() => resolve())),
  };
}

async function rpc(url: string, session: string | undefined, id: number, method: string, params: unknown, extra: Record<string, string> = {}) {
  const response = await fetch(url, {
    method: 'POST',
    signal: AbortSignal.timeout(5_000),
    headers: {
      'content-type': 'application/json',
      accept: 'application/json, text/event-stream',
      ...(session ? { 'mcp-session-id': session } : {}),
      ...extra,
    },
    body: JSON.stringify({ jsonrpc: '2.0', id, method, params }),
  });
  const text = await response.text();
  const payload = text.startsWith('event:')
    ? JSON.parse(text.split('\n').find((line) => line.startsWith('data: '))!.slice(6))
    : JSON.parse(text);
  return { response, headers: response.headers, body: payload };
}

async function timesOut(url: string, session: string | undefined, tool: string, extra: Record<string, string> = {}) {
  try {
    const response = await fetch(url, {
      method: 'POST',
      signal: AbortSignal.timeout(100),
      headers: {
        'content-type': 'application/json',
        accept: 'application/json, text/event-stream',
        ...(session ? { 'mcp-session-id': session } : {}),
        ...extra,
      },
      body: JSON.stringify({ jsonrpc: '2.0', id: 30, method: 'tools/call', params: { name: tool, arguments: { slow: true } } }),
    });
    const payload = JSON.parse(await response.text()) as { error?: unknown };
    return !response.ok || Boolean(payload.error);
  } catch (error) {
    return error instanceof DOMException && error.name === 'TimeoutError';
  }
}

async function control(port: number, token: string, path: string, body: unknown) {
  const deadline = Date.now() + 10_000;
  while (Date.now() < deadline) {
    try {
      const response = await fetch(`http://127.0.0.1:${port}${path}`, {
        method: 'POST', headers: { authorization: `Bearer ${token}`, 'content-type': 'application/json' }, body: JSON.stringify(body),
      });
      assert.equal(response.ok, true, await response.text());
      return;
    } catch (error) {
      if (error instanceof assert.AssertionError) throw error;
      if (Date.now() >= deadline) throw error;
      await new Promise((resolve) => setTimeout(resolve, 50));
    }
  }
  throw new Error('timed out waiting for CommonKit control plane');
}

async function waitForFile(root: string, name: string): Promise<string> {
  const deadline = Date.now() + 10_000;
  while (Date.now() < deadline) {
    const found = await findFile(root, name);
    if (found) return found;
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  throw new Error(`timed out waiting for ${name}`);
}

async function findFile(root: string, name: string): Promise<string | undefined> {
  for (const entry of await readdir(root, { withFileTypes: true }).catch(() => [])) {
    const path = join(root, entry.name);
    if (entry.isFile() && entry.name === name) return path;
    if (entry.isDirectory()) {
      const found = await findFile(path, name);
      if (found) return found;
    }
  }
}

async function freePort() {
  const server = createServer();
  await new Promise<void>((resolve, reject) => server.once('error', reject).listen(0, '127.0.0.1', resolve));
  const address = server.address();
  assert.ok(address && typeof address === 'object');
  await new Promise<void>((resolve) => server.close(() => resolve()));
  return address.port;
}

async function stop(child: ChildProcess) {
  if (child.exitCode !== null) return;
  child.kill();
  await Promise.race([
    new Promise<void>((resolve) => child.once('exit', () => resolve())),
    new Promise<void>((resolve) => setTimeout(resolve, 2_000)),
  ]);
  if (child.exitCode === null) child.kill('SIGKILL');
}
