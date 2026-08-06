import assert from 'node:assert/strict'
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import test from 'node:test'

import { findForbiddenDescendant, RSYNC_DEREFERENCE, RSYNC_EXCLUDES, SyncEngine } from '../src/sync.mjs'

test('detects forbidden paths recursively before rsync', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'commonkit-test-'))
  try {
    fs.mkdirSync(path.join(root, 'skill', 'secrets'), { recursive: true })
    fs.writeFileSync(path.join(root, 'skill', 'secrets', 'token.txt'), 'not-a-real-secret')
    assert.equal(path.basename(findForbiddenDescendant(root)), 'secrets')
    assert.ok(RSYNC_EXCLUDES.includes('secrets/'))
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

test('scans through symlinked directories, which rsync dereferences', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'commonkit-test-'))
  try {
    const outside = path.join(root, 'outside', 'skill', 'secrets')
    fs.mkdirSync(outside, { recursive: true })
    fs.writeFileSync(path.join(outside, 'token.txt'), 'not-a-real-secret')
    const tree = path.join(root, 'tree')
    fs.mkdirSync(tree)
    fs.symlinkSync(path.join(root, 'outside', 'skill'), path.join(tree, 'skill'))
    assert.equal(RSYNC_DEREFERENCE, '--copy-links')
    assert.equal(path.basename(findForbiddenDescendant(tree)), 'secrets')
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

test('terminates on symlink cycles inside a managed tree', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'commonkit-test-'))
  try {
    fs.mkdirSync(path.join(root, 'skill'))
    fs.symlinkSync(root, path.join(root, 'skill', 'loop'))
    assert.equal(findForbiddenDescendant(root), null)
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
