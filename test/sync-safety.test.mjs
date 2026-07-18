import assert from 'node:assert/strict'
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import test from 'node:test'

import { findForbiddenDescendant, RSYNC_EXCLUDES, SyncEngine } from '../src/sync.mjs'

test('detects forbidden paths recursively before rsync', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'commonkit-test-'))
  try {
    fs.mkdirSync(path.join(root, 'skill', 'secrets'), { recursive: true })
    fs.writeFileSync(path.join(root, 'skill', 'secrets', 'token.txt'), 'not-a-real-secret')
    assert.match(findForbiddenDescendant(root), /\/secrets$/)
    assert.ok(RSYNC_EXCLUDES.includes('secrets/'))
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

test('rejects unsafe hosts, root targets, and context paths', () => {
  const base = { host: 'vps', remoteHome: '/srv/agent', service: 'orca.service', pluginPolicy: 'exact' }
  assert.throws(() => new SyncEngine({ ...base, host: '-oProxyCommand=bad' }), /host/)
  assert.throws(() => new SyncEngine({ ...base, remoteHome: '/' }), /non-root/)
  assert.throws(() => new SyncEngine({ ...base, contextMode: { enabled: true, localContent: '/tmp/db', remoteContent: '/tmp/db' } }), /below remoteHome/)
})
