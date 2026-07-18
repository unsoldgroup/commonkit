import test from 'node:test';
import assert from 'node:assert/strict';
import { normalizeConfig } from '../src/config.js';
import { resolveUpgradeCommand } from '../src/cli.js';

test('normalizes minimal config', () => {
  const config = normalizeConfig({
    servers: [
      {
        id: 'posthog',
        mode: 'posthog-cli',
        remote: { type: 'streamable_http', url: 'https://mcp.posthog.com/mcp' },
      },
    ],
  });
  assert.equal(config.admin?.host, '127.0.0.1');
  assert.equal(config.admin?.port, 3764);
  assert.equal(config.updates?.autoUpgrade, false);
  assert.equal(config.updates?.checkIntervalMs, 86_400_000);
  assert.equal(config.servers[0].enabled, true);
  assert.equal(config.servers[0].mode, 'posthog-cli');
});

test('normalizes app update config', () => {
  const config = normalizeConfig({
    updates: {
      autoUpgrade: true,
      checkIntervalMs: 60_000,
      packageManager: 'pnpm',
      registryUrl: 'https://registry.npmjs.org',
    },
    servers: [],
  });
  assert.equal(config.updates?.autoUpgrade, true);
  assert.equal(config.updates?.checkIntervalMs, 60_000);
  assert.equal(config.updates?.packageManager, 'pnpm');
  assert.equal(config.updates?.registryUrl, 'https://registry.npmjs.org');
});

test('normalizes automatic refresh interval', () => {
  const config = normalizeConfig({
    servers: [
      {
        id: 'docs',
        remote: { type: 'streamable_http', url: 'https://example.com/mcp' },
        cache: { toolsTtlMs: 60_000 },
      },
      {
        id: 'manual',
        remote: { type: 'streamable_http', url: 'https://example.com/mcp' },
        cache: { autoRefreshMs: 0 },
      },
    ],
  });
  assert.equal(config.servers[0].cache?.toolsTtlMs, 60_000);
  assert.equal(config.servers[0].cache?.autoRefreshMs, 60_000);
  assert.equal(config.servers[1].cache?.autoRefreshMs, 0);
});

test('rejects invalid server id', () => {
  assert.throws(() =>
    normalizeConfig({
      servers: [
        {
          id: '../bad',
          remote: { type: 'streamable_http', url: 'https://example.com/mcp' },
        },
      ],
    }),
  );
});

test('normalizes local menu config', () => {
  const config = normalizeConfig({
    servers: [
      {
        id: 'mail',
        remote: { type: 'streamable_http', url: 'https://example.com/mcp' },
        menu: {
          statusUrl: 'http://127.0.0.1:3765/status',
          ttlMs: 5000,
          actions: [
            {
              id: 'sync_now',
              label: 'Sync Now',
              method: 'POST',
              url: 'http://localhost:3765/sync',
              confirm: true,
            },
            {
              id: 'open_index',
              label: 'Open Index',
              url: 'file:///tmp/index',
            },
          ],
        },
      },
    ],
  });
  assert.equal(config.servers[0].menu?.statusUrl, 'http://127.0.0.1:3765/status');
  assert.equal(config.servers[0].menu?.ttlMs, 5000);
  assert.equal(config.servers[0].menu?.actions?.[0].method, 'POST');
  assert.equal(config.servers[0].menu?.actions?.[0].confirm, true);
});

test('rejects non-local menu admin urls', () => {
  assert.throws(() =>
    normalizeConfig({
      servers: [
        {
          id: 'bad',
          remote: { type: 'streamable_http', url: 'https://example.com/mcp' },
          menu: {
            statusUrl: 'https://example.com/status',
          },
        },
      ],
    }),
  );
  assert.throws(() =>
    normalizeConfig({
      servers: [
        {
          id: 'bad',
          remote: { type: 'streamable_http', url: 'https://example.com/mcp' },
          menu: {
            actions: [{ id: 'call', label: 'Call', method: 'POST', url: 'http://example.com/call' }],
          },
        },
      ],
    }),
  );
});

test('resolves upgrade command from package manager', () => {
  assert.deepEqual(resolveUpgradeCommand(undefined, 'pnpm/10.28.2 npm/? node/v24'), {
    command: 'pnpm',
    args: ['add', '-g', 'mcp-local-relay@latest'],
  });
  assert.deepEqual(resolveUpgradeCommand('npm'), {
    command: 'npm',
    args: ['install', '-g', 'mcp-local-relay@latest'],
  });
  assert.deepEqual(resolveUpgradeCommand(undefined, undefined, '/Users/al/Library/pnpm/mcp-local-relayctl'), {
    command: 'pnpm',
    args: ['add', '-g', 'mcp-local-relay@latest'],
  });
});
