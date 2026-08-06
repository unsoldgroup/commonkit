import { createServer } from 'node:http';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import assert from 'node:assert/strict';
import { writeConfig } from '../src/config.js';
import { serve } from '../src/service.js';

test('serves status bar admin endpoints without secrets', async (t) => {
  const port = await freePort().catch((err: unknown) => {
    if (err && typeof err === 'object' && 'code' in err && err.code === 'EPERM') {
      t.skip('local listen is blocked by the current sandbox');
      return undefined;
    }
    throw err;
  });
  if (!port) return;

  const dir = await mkdtemp(join(tmpdir(), 'mcp-local-relay-'));
  const configPath = join(dir, 'config.json');
  await writeConfig({ admin: { host: '127.0.0.1', port, mcpPath: '/mcp' }, servers: [] }, configPath);

  const service = await serve({ configPath });
  try {
    const statusResponse = await fetch(`http://127.0.0.1:${port}/status`);
    assert.equal(statusResponse.status, 200);
    const status = await statusResponse.json() as {
      ok: boolean;
      name: string;
      uptimeMs: number;
      sessions: number;
      servers: unknown[];
      updates?: { enabled?: boolean; checkIntervalMs?: number };
    };
    assert.equal(status.ok, true);
    assert.equal(status.name, 'mcp-local-relay');
    assert.equal(typeof status.uptimeMs, 'number');
    assert.equal(status.sessions, 0);
    assert.deepEqual(status.servers, []);
    assert.equal(status.updates?.enabled, false);
    assert.equal(status.updates?.checkIntervalMs, 86_400_000);

    const configResponse = await fetch(`http://127.0.0.1:${port}/client-config`);
    assert.equal(configResponse.status, 200);
    const clientConfig = JSON.stringify(await configResponse.json());
    assert.match(clientConfig, new RegExp(`http://127\\.0\\.0\\.1:${port}/mcp`));
    assert.doesNotMatch(clientConfig, /TOKEN|SECRET|PASSWORD|Authorization/i);
  } finally {
    service.manager.stop();
    await new Promise<void>((resolve) => service.httpServer.close(() => resolve()));
    await rm(dir, { recursive: true, force: true });
  }
});

async function freePort() {
  const server = createServer();
  await new Promise<void>((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', () => {
      server.off('error', reject);
      resolve();
    });
  });
  const address = server.address();
  assert.ok(address && typeof address === 'object');
  await new Promise<void>((resolve) => server.close(() => resolve()));
  return address.port;
}
