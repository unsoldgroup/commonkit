import { randomUUID } from 'node:crypto';
import { createServer } from 'node:http';
import { mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import assert from 'node:assert/strict';
import { Server } from '@modelcontextprotocol/sdk/server/index.js';
import { StreamableHTTPServerTransport } from '@modelcontextprotocol/sdk/server/streamableHttp.js';
import {
  CallToolRequestSchema,
  isInitializeRequest,
  ListToolsRequestSchema,
} from '@modelcontextprotocol/sdk/types.js';
import { RelayManager } from '../src/relay-manager.js';
import { serve } from '../src/service.js';

test('hot-adds upstream server and calls localized tool', async (t) => {
  const upstream = await startMockMcpServer().catch((err: unknown) => {
    if (err && typeof err === 'object' && 'code' in err && err.code === 'EPERM') {
      t.skip('local listen is blocked by the current sandbox');
      return undefined;
    }
    throw err;
  });
  if (!upstream) return;
  const dir = await mkdtemp(join(tmpdir(), 'mcp-local-relay-'));
  const manager = new RelayManager({ servers: [] }, join(dir, 'config.json'));

  try {
    await manager.addServer({
      id: 'echo',
      remote: { type: 'streamable_http', url: upstream.url },
    });

    const names = manager.listTools().map((tool) => tool.name);
    assert.ok(names.includes('relay_add_server'));
    assert.ok(names.includes('echo__echo'));

    const result = await manager.callTool('echo__echo', { message: 'hello' });
    assert.deepEqual(result, {
      content: [{ type: 'text', text: 'hello' }],
    });
  } finally {
    manager.stop();
    await upstream.close();
    await rm(dir, { recursive: true, force: true });
  }
});

test('automatically refreshes upstream tool cache on an interval', async (t) => {
  const upstream = await startMockMcpServer().catch((err: unknown) => {
    if (err && typeof err === 'object' && 'code' in err && err.code === 'EPERM') {
      t.skip('local listen is blocked by the current sandbox');
      return undefined;
    }
    throw err;
  });
  if (!upstream) return;
  const dir = await mkdtemp(join(tmpdir(), 'mcp-local-relay-'));
  const manager = new RelayManager(
    {
      servers: [
        {
          id: 'auto-refresh',
          remote: { type: 'streamable_http', url: upstream.url },
          cache: { toolsTtlMs: 25, autoRefreshMs: 25 },
        },
      ],
    },
    join(dir, 'config.json'),
  );

  try {
    await manager.start();
    await waitUntil(() => upstream.listToolsCount() >= 2);
    const status = manager.status();
    assert.equal(status.servers[0].connected, true);
    assert.equal(status.servers[0].autoRefreshMs, 25);
    assert.equal(typeof status.servers[0].nextAutoRefreshAt, 'number');
  } finally {
    manager.stop();
    await upstream.close();
    await rm(dir, { recursive: true, force: true });
  }
});

async function waitUntil(fn: () => boolean, timeoutMs = 1000) {
  const started = Date.now();
  while (!fn()) {
    if (Date.now() - started > timeoutMs) throw new Error('timed out waiting for condition');
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
}

test('discovers menu status and executes upstream menu action', async (t) => {
  const upstream = await startMockMcpServer({
    menuStatus: {
      title: 'Mail Index',
      summary: 'Ready',
      state: 'ready',
      detail: ['Last sync 4m ago'],
      actions: [
        {
          id: 'sync_now',
          label: 'Sync Now',
          tool: 'sync_now',
          input: {
            title: 'Sync',
            fields: [
              { id: 'force', label: 'Force', type: 'boolean' },
              { id: 'notes', label: 'Notes', type: 'string', multiline: true },
            ],
          },
        },
        {
          id: 'ledger',
          label: 'Ledger',
          view: {
            type: 'table',
            title: 'Ledger',
            columns: [{ id: 'status', label: '', kind: 'status' }],
            rows: [{ status: 'success', plan: 'Plan A' }],
          },
        },
      ],
    },
  }).catch((err: unknown) => {
    if (err && typeof err === 'object' && 'code' in err && err.code === 'EPERM') {
      t.skip('local listen is blocked by the current sandbox');
      return undefined;
    }
    throw err;
  });
  if (!upstream) return;
  const dir = await mkdtemp(join(tmpdir(), 'mcp-local-relay-'));
  const manager = new RelayManager({ servers: [] }, join(dir, 'config.json'));

  try {
    await manager.addServer({
      id: 'mail',
      remote: { type: 'streamable_http', url: upstream.url },
    });

    const menu = await manager.menuStatus('mail');
    assert.equal(menu.title, 'Mail Index');
    assert.equal(menu.summary, 'Ready');
    assert.equal(menu.actions[0].id, 'sync_now');
    assert.equal(menu.actions[0].input?.fields[0].id, 'force');
    assert.equal(menu.actions[0].input?.fields[1].multiline, true);
    assert.equal(menu.actions[1].view?.type, 'table');
    assert.equal(menu.actions[1].view?.rows?.[0]?.status, 'success');

    const result = await manager.callMenuAction('mail', 'sync_now', { args: { force: true } });
    assert.deepEqual(result, {
      content: [{ type: 'text', text: 'synced:true' }],
    });
  } finally {
    manager.stop();
    await upstream.close();
    await rm(dir, { recursive: true, force: true });
  }
});

test('falls back when menu metadata is malformed and enforces confirmation', async (t) => {
  const upstream = await startMockMcpServer({ menuStatusText: '{bad json' }).catch((err: unknown) => {
    if (err && typeof err === 'object' && 'code' in err && err.code === 'EPERM') {
      t.skip('local listen is blocked by the current sandbox');
      return undefined;
    }
    throw err;
  });
  if (!upstream) return;
  const dir = await mkdtemp(join(tmpdir(), 'mcp-local-relay-'));
  const manager = new RelayManager({ servers: [] }, join(dir, 'config.json'));

  try {
    await manager.addServer({
      id: 'ctx',
      remote: { type: 'streamable_http', url: upstream.url },
      menu: {
        actions: [{ id: 'purge', label: 'Purge', tool: 'sync_now', confirm: true }],
      },
    });

    const menu = await manager.menuStatus('ctx');
    assert.equal(menu.title, 'ctx');
    assert.equal(menu.state, 'error');
    assert.match(menu.lastError, /JSON/);
    assert.equal(menu.actions[0].id, 'purge');
    await assert.rejects(() => manager.callMenuAction('ctx', 'purge', {}), /confirm/);
  } finally {
    manager.stop();
    await upstream.close();
    await rm(dir, { recursive: true, force: true });
  }
});

test('serves menu status and menu actions over admin endpoints', async (t) => {
  const upstream = await startMockMcpServer({
    menuStatus: {
      title: 'Context Mode',
      summary: 'Indexed',
      state: 'ready',
      detail: ['12 chunks'],
      actions: [{ id: 'sync_now', label: 'Sync Now', tool: 'sync_now' }],
    },
  }).catch((err: unknown) => {
    if (err && typeof err === 'object' && 'code' in err && err.code === 'EPERM') {
      t.skip('local listen is blocked by the current sandbox');
      return undefined;
    }
    throw err;
  });
  if (!upstream) return;
  const dir = await mkdtemp(join(tmpdir(), 'mcp-local-relay-'));
  const port = await getFreePort();
  const configPath = join(dir, 'config.json');
  await writeFile(
    configPath,
    JSON.stringify({
      admin: { host: '127.0.0.1', port, mcpPath: '/mcp' },
      servers: [{ id: 'ctx', remote: { type: 'streamable_http', url: upstream.url } }],
    }),
  );
  const service = await serve({ configPath });

  try {
    const menuResponse = await fetch(`http://127.0.0.1:${port}/servers/ctx/menu`);
    assert.equal(menuResponse.status, 200);
    const menu = (await menuResponse.json()) as { title?: string; actions?: Array<{ id: string }> };
    assert.equal(menu.title, 'Context Mode');
    assert.equal(menu.actions?.[0].id, 'sync_now');

    const actionResponse = await fetch(`http://127.0.0.1:${port}/servers/ctx/menu/actions/sync_now`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ args: { force: true } }),
    });
    assert.equal(actionResponse.status, 200);
    const result = (await actionResponse.json()) as { content?: Array<{ text?: string }> };
    assert.equal(result.content?.[0].text, 'synced:true');
  } finally {
    service.manager.stop();
    await new Promise<void>((resolve) => service.httpServer.close(() => resolve()));
    await upstream.close();
    await rm(dir, { recursive: true, force: true });
  }
});

async function getFreePort() {
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
  const port = address.port;
  await new Promise<void>((resolve) => server.close(() => resolve()));
  return port;
}

async function startMockMcpServer(options: { menuStatus?: unknown; menuStatusText?: string } = {}) {
  const transports = new Map<string, StreamableHTTPServerTransport>();
  const servers = new Map<string, Server>();
  let listToolsCount = 0;
  const httpServer = createServer((req, res) => {
    void (async () => {
      const url = new URL(req.url || '/', `http://${req.headers.host}`);
      if (url.pathname !== '/mcp') {
        res.writeHead(404);
        res.end();
        return;
      }
      const header = req.headers['mcp-session-id'];
      const sid = Array.isArray(header) ? header[0] : header;
      let transport = sid ? transports.get(sid) : undefined;

      if (req.method === 'GET' || req.method === 'DELETE') {
        if (!transport) {
          res.writeHead(400);
          res.end();
          return;
        }
        await transport.handleRequest(req, res);
        return;
      }

      const chunks = [];
      for await (const chunk of req) chunks.push(Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk));
      const body = JSON.parse(Buffer.concat(chunks).toString('utf8'));

      if (!transport) {
        assert.ok(isInitializeRequest(body));
        const server = new Server({ name: 'mock-upstream', version: '1.0.0' }, {
          capabilities: { tools: {} },
        });
        server.setRequestHandler(ListToolsRequestSchema, async () => {
          listToolsCount += 1;
          return {
            tools: [
              {
                name: 'echo',
                description: 'Echo a message',
                inputSchema: {
                  type: 'object',
                  properties: { message: { type: 'string' } },
                  required: ['message'],
                },
              },
              {
                name: 'sync_now',
                description: 'Sync now',
                inputSchema: {
                  type: 'object',
                  properties: { force: { type: 'boolean' } },
                },
              },
              {
                name: 'relay_menu_status',
                description: 'Menu status',
                inputSchema: { type: 'object', properties: {} },
              },
            ],
          };
        });
        server.setRequestHandler(CallToolRequestSchema, async (request) => {
          if (request.params.name === 'relay_menu_status') {
            return {
              content: [
                {
                  type: 'text',
                  text: options.menuStatusText ?? JSON.stringify(options.menuStatus || {}),
                },
              ],
            };
          }
          if (request.params.name === 'sync_now') {
            return {
              content: [{ type: 'text', text: `synced:${String(request.params.arguments?.force || false)}` }],
            };
          }
          return {
            content: [{ type: 'text', text: String(request.params.arguments?.message || '') }],
          };
        });
        transport = new StreamableHTTPServerTransport({
          sessionIdGenerator: () => randomUUID(),
          onsessioninitialized: (newSessionId) => {
            transports.set(newSessionId, transport!);
            servers.set(newSessionId, server);
          },
        });
        await server.connect(transport);
      }

      await transport.handleRequest(req, res, body);
    })().catch((err) => {
      res.writeHead(500, { 'content-type': 'application/json' });
      res.end(JSON.stringify({ error: err instanceof Error ? err.message : String(err) }));
    });
  });

  await new Promise<void>((resolve, reject) => {
    httpServer.once('error', reject);
    httpServer.listen(0, '127.0.0.1', () => {
      httpServer.off('error', reject);
      resolve();
    });
  });
  const address = httpServer.address();
  assert.ok(address && typeof address === 'object');
  return {
    url: `http://127.0.0.1:${address.port}/mcp`,
    listToolsCount: () => listToolsCount,
    close: async () => {
      for (const transport of transports.values()) await transport.close();
      for (const server of servers.values()) await server.close();
      await new Promise<void>((resolve) => httpServer.close(() => resolve()));
    },
  };
}
