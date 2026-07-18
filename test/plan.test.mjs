import assert from 'node:assert/strict'
import test from 'node:test'

import { buildPlan, isForbiddenPath } from '../src/plan.mjs'

test('plans both the shell and Orca Codex homes', () => {
  const plan = buildPlan({ home: '/Users/al', remoteHome: '/root' })
  const agentsTargets = plan
    .filter((operation) => operation.source === '/Users/al/.codex/AGENTS.md')
    .map((operation) => operation.target)

  assert.deepEqual(agentsTargets, [
    '/root/.codex/AGENTS.md',
    '/root/.config/orca/codex-runtime-home/home/AGENTS.md',
  ])
})

test('never plans credentials or live runtime databases', () => {
  const forbidden = [
    '/root/.codex/auth.json',
    '/root/.config/gh/hosts.yml',
    '/root/.config/orca/orchestration.db',
    '/root/.config/orca/orchestration.db-wal',
    '/root/.claude/context-mode/sessions/abc.db',
    '/root/.config/orca/orca-e2ee-keypair.json',
    '/root/.env',
    '/root/x/credentials.enc',
    '/root/x/client_secret.json',
    '/root/x/token_cache.json',
    '/root/x/.encryption_key',
    '/root/x/id_ed25519.key',
    '/root/x/state_5.sqlite',
    '/root/x/secrets/token.txt',
  ]

  for (const path of forbidden) assert.equal(isForbiddenPath(path), true, path)
  for (const operation of buildPlan({ home: '/Users/al', remoteHome: '/root' })) {
    assert.equal(isForbiddenPath(operation.source), false, operation.source)
    assert.equal(isForbiddenPath(operation.target), false, operation.target)
  }
})
