import test from 'node:test';
import assert from 'node:assert/strict';
import { renderLaunchAgent } from '../src/launchd.js';

test('renders LaunchAgent plist without secrets', () => {
  const plist = renderLaunchAgent({
    nodePath: '/usr/local/bin/node',
    cliPath: '/usr/local/bin/mcp-local-relay',
    configPath: '/Users/al/.config/mcp-local-relay/config.json',
  });
  assert.match(plist, /com\.unsoldgroup\.mcp-local-relay/);
  assert.match(plist, /RunAtLoad/);
  assert.doesNotMatch(plist, /TOKEN|SECRET|PASSWORD/);
});
