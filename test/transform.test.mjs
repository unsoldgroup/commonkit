import assert from 'node:assert/strict'
import test from 'node:test'

import { assertNoEmbeddedSecrets, mergeClaudeSettings, renderCodexConfig, renderPortableJson } from '../src/transform.mjs'

test('merges owned Claude keys while preserving VPS-only settings', () => {
  const local = {
    permissions: { allow: ['Bash(git *)'] },
    hooks: { Stop: [{ hooks: [{ type: 'command', command: '/Users/developer/bin/stop' }] }] },
    enabledPlugins: { 'linear@example': true },
    effortLevel: 'high',
    theme: 'dark',
  }
  const remote = {
    permissions: { allow: ['Bash(*)'] },
    hooks: {},
    enabledPlugins: {},
    theme: 'light',
    remote: { defaultEnvironmentId: 'vps' },
  }

  assert.deepEqual(mergeClaudeSettings(local, remote, { localHome: '/Users/developer', remoteHome: '/root' }), {
    permissions: { allow: ['Bash(git *)'] },
    hooks: { Stop: [{ hooks: [{ type: 'command', command: '/root/bin/stop' }] }] },
    enabledPlugins: { 'linear@example': true },
    effortLevel: 'high',
    theme: 'light',
    remote: { defaultEnvironmentId: 'vps' },
  })
})

test('renders portable Codex config without copying hook trust state', () => {
  const source = `approval_policy = "on-request"\nsandbox_mode = "workspace-write"\nlast_updated = "machine timestamp"\ncommand = "/Users/developer/bin/hook"\n\n[marketplaces.local]\nsource_type = "local"\nsource = "/Users/developer/.tmp/marketplace"\n\n[hooks.state."/Users/developer/hook:permission_request:0:0"]\napproved = true\n\n[mcp_servers.shared]\ncommand = "/Users/developer/bin/server"\n\n[mcp_servers.shared.env]\nAPI_KEY = "literal-secret-value"\nSAFE_MODE = "true"\n`
  const rendered = renderCodexConfig(source, { localHome: '/Users/developer', remoteHome: '/root' })

  assert.match(rendered, /approval_policy = "on-request"/)
  assert.match(rendered, /command = "\/root\/bin\/hook"/)
  assert.match(rendered, /command = "\/root\/bin\/server"/)
  assert.doesNotMatch(rendered, /hooks\.state/)
  assert.doesNotMatch(rendered, /approved = true/)
  assert.doesNotMatch(rendered, /literal-secret-value/)
  assert.doesNotMatch(rendered, /^API_KEY =/m)
  assert.doesNotMatch(rendered, /^last_updated =/m)
  assert.doesNotMatch(rendered, /^\[marketplaces\./m)
  assert.match(rendered, /SAFE_MODE = "true"/)
})

test('renders JSON hook paths for the selected Codex home', () => {
  const source = JSON.stringify({ command: '/Users/developer/.codex/plugins/cache/hook.mjs' })
  const rendered = renderPortableJson(source, {
    localHome: '/Users/developer',
    remoteHome: '/root',
    codexHome: '/root/.config/orca/codex-runtime-home/home',
  })
  assert.equal(JSON.parse(rendered).command, '/root/.config/orca/codex-runtime-home/home/plugins/cache/hook.mjs')
})

test('rejects secret-like JSON keys while allowing environment references', () => {
  assert.throws(
    () => assertNoEmbeddedSecrets({ nested: { accessToken: 'sk-live-literal' } }, { label: 'settings' }),
    /Embedded secret-like key at settings\.nested\.accessToken/,
  )
  assert.doesNotThrow(() => assertNoEmbeddedSecrets({ api_token: '${API_TOKEN}' }))
  assert.doesNotThrow(() => assertNoEmbeddedSecrets({ token_budget: 4096 }))
})

test('rejects authorization headers, URL userinfo, and inline assignments', () => {
  assert.throws(() => assertNoEmbeddedSecrets('Authorization: Bearer abcdefghijklmnop'), /authorization value/)
  assert.throws(() => assertNoEmbeddedSecrets('https://alice:hunter2@example.com/api'), /URL credentials/)
  assert.throws(() => assertNoEmbeddedSecrets('env = { API_KEY = "literal-value" }'), /secret assignment/)
  assert.throws(() => assertNoEmbeddedSecrets('headers = { "X-API-Key" = "literal-value" }'), /secret assignment/)
  assert.doesNotThrow(() => assertNoEmbeddedSecrets('env = { API_KEY = "${API_KEY}" }'))
})
