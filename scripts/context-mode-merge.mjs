#!/usr/bin/env node
// Union-merge context-mode stores into one canonical store (USG-96).
//
// Contract from docs/scopes/context-mode-single-store-v1.md:
//   - rows preserved verbatim, no row dropped
//   - colliding session ids both survive under distinct keys
//   - every collision writes a record naming both sources, digests, sizes, loser
//   - writes a new store, never mutates a source
//   - idempotent and re-runnable
//   - runs only while no writer is live
//
// Usage:
//   node scripts/context-mode-merge.mjs --source ~/.claude/context-mode \
//        --source ~/.codex/context-mode --out ~/.local/share/context-mode
//   node scripts/context-mode-merge.mjs ... --apply     # write (default is dry run)
//   node scripts/context-mode-merge.mjs --selftest

import { DatabaseSync } from 'node:sqlite'
import { createHash } from 'node:crypto'
import { execFileSync } from 'node:child_process'
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'

const SUBDIRS = ['sessions', 'content']

// Tables merged when two stores hold the same session-database filename, in
// dependency order. `key` is the natural identity; `surrogate` columns are
// AUTOINCREMENT ids that the destination reassigns.
const SESSION_TABLES = [
  { name: 'session_meta', key: ['session_id'], surrogate: [] },
  { name: 'session_resume', key: ['session_id'], surrogate: ['id'] },
  { name: 'tool_calls', key: ['session_id', 'tool'], surrogate: [] },
  { name: 'session_events', key: null, surrogate: ['id'] },
]

function parseArgs(argv) {
  const out = { sources: [], out: null, apply: false, selftest: false }
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i]
    if (a === '--source') out.sources.push(path.resolve(expand(argv[++i])))
    else if (a === '--out') out.out = path.resolve(expand(argv[++i]))
    else if (a === '--apply') out.apply = true
    else if (a === '--selftest') out.selftest = true
    else throw new Error(`unknown argument: ${a}`)
  }
  return out
}

const expand = p => (p.startsWith('~') ? path.join(os.homedir(), p.slice(1)) : p)

const sha256 = file => {
  const h = createHash('sha256')
  h.update(fs.readFileSync(file))
  return h.digest('hex')
}

const listDbs = (root, sub) => {
  const dir = path.join(root, sub)
  if (!fs.existsSync(dir)) return []
  return fs.readdirSync(dir).filter(f => f.endsWith('.db')).sort()
}

// A live writer makes every read inconsistent, so refuse rather than merge a
// torn snapshot. `lsof +D` is the only portable-enough check here; a missing
// lsof is treated as "cannot prove it is safe".
function assertNoLiveWriters(roots) {
  const holders = []
  for (const root of roots) {
    let out = ''
    try {
      out = execFileSync('lsof', ['+D', root], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] })
    } catch (e) {
      // lsof exits non-zero when nothing matches, which is the good case.
      if (e.status === 1 && !e.stdout) continue
      if (e.code === 'ENOENT') throw new Error('lsof not found; cannot prove no writer is live')
      out = e.stdout || ''
    }
    for (const line of out.split('\n').slice(1)) {
      const cols = line.trim().split(/\s+/)
      if (cols.length > 1) holders.push(`${cols[0]}(${cols[1]}) ${cols[cols.length - 1]}`)
    }
  }
  if (holders.length) {
    throw new Error(
      `refusing to merge: ${holders.length} open handle(s) under the source stores.\n  ` +
        [...new Set(holders)].slice(0, 10).join('\n  ') +
        '\nQuit these Claude Code / Codex sessions first. Merging now would read a' +
        '\ntorn snapshot. This cannot quit them for you: one of them may be the' +
        '\nsession you are reading this in.',
    )
  }
}

function openReadable(file) {
  // Copy first: attaching a WAL database creates a -shm beside it, which would
  // mutate a source store.
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'ctxmerge-'))
  const copy = path.join(tmp, path.basename(file))
  fs.copyFileSync(file, copy)
  return { db: new DatabaseSync(copy), cleanup: () => fs.rmSync(tmp, { recursive: true, force: true }) }
}

function columnsOf(db, table) {
  return db.prepare(`PRAGMA table_info(${table})`).all().map(r => r.name)
}

function tableExists(db, table) {
  return !!db.prepare("SELECT 1 FROM sqlite_master WHERE type='table' AND name=?").get(table)
}

// Union one incoming session database into `destPath`, which already holds the
// base store's rows. Returns the collision records produced.
function mergeSessionDb({ destPath, incomingPath, baseLabel, incomingLabel, filename }) {
  const collisions = []
  const dest = new DatabaseSync(destPath)
  const { db: src, cleanup } = openReadable(incomingPath)
  try {
    // Session ids already present in the destination lose their identity to the
    // base store, so the incoming copy is kept under a suffixed key instead of
    // being dropped.
    const existing = new Set(
      tableExists(dest, 'session_meta')
        ? dest.prepare('SELECT session_id FROM session_meta').all().map(r => r.session_id)
        : [],
    )
    const remap = new Map()
    if (tableExists(src, 'session_meta')) {
      for (const r of src.prepare('SELECT session_id FROM session_meta ORDER BY session_id').all()) {
        if (!existing.has(r.session_id)) continue
        const renamed = `${r.session_id}#${incomingLabel}`
        remap.set(r.session_id, renamed)
        collisions.push({
          file: filename,
          sessionId: r.session_id,
          keptAs: renamed,
          loser: incomingLabel,
          winner: baseLabel,
        })
      }
    }

    dest.exec('BEGIN')
    for (const spec of SESSION_TABLES) {
      if (!tableExists(src, spec.name) || !tableExists(dest, spec.name)) continue
      const cols = columnsOf(src, spec.name).filter(c => !spec.surrogate.includes(c))
      const order = spec.surrogate.length ? `ORDER BY ${spec.surrogate[0]}` : ''
      const rows = src.prepare(`SELECT ${cols.join(', ')} FROM ${spec.name} ${order}`).all()
      const stmt = dest.prepare(
        `INSERT INTO ${spec.name} (${cols.join(', ')}) VALUES (${cols.map(() => '?').join(', ')})`,
      )
      for (const row of rows) {
        const v = cols.map(c => (c === 'session_id' && remap.has(row[c]) ? remap.get(row[c]) : row[c]))
        stmt.run(...v)
      }
    }
    dest.exec('COMMIT')
  } finally {
    cleanup()
    dest.close()
  }
  return collisions
}

function plan(sources, outRoot) {
  const actions = []
  for (const sub of SUBDIRS) {
    const byName = new Map()
    for (const root of sources) {
      for (const f of listDbs(root, sub)) {
        if (!byName.has(f)) byName.set(f, [])
        byName.get(f).push(root)
      }
    }
    for (const [f, roots] of [...byName].sort(([a], [b]) => (a < b ? -1 : 1))) {
      actions.push({
        sub,
        file: f,
        dest: path.join(outRoot, sub, f),
        base: roots[0],
        incoming: roots.slice(1),
      })
    }
  }
  return actions
}

function run({ sources, out, apply }) {
  if (sources.length < 2) throw new Error('need at least two --source stores')
  for (const s of sources) if (!fs.existsSync(s)) throw new Error(`source store not found: ${s}`)
  if (!out) throw new Error('need --out')

  // Planning only lists filenames, so it is safe while agents are running. Only
  // --apply reads database contents and needs a quiet store.
  if (apply) assertNoLiveWriters(sources)

  const label = root => path.basename(path.dirname(root)) || path.basename(root)
  const actions = plan(sources, out)
  const copied = actions.filter(a => !a.incoming.length)
  const merged = actions.filter(a => a.incoming.length)

  console.log(`sources: ${sources.map(s => `${label(s)}=${s}`).join('  ')}`)
  console.log(`destination: ${out}`)
  console.log(`databases: ${actions.length} total, ${copied.length} copied as-is, ${merged.length} unioned`)
  for (const a of merged) {
    console.log(`  union ${a.sub}/${a.file}: base=${label(a.base)} incoming=${a.incoming.map(label).join(',')}`)
  }
  if (!apply) {
    console.log('\ndry run. re-run with --apply to write.')
    return { collisions: [], actions }
  }

  const manifest = { generatedFrom: [], databases: [], collisions: [] }
  for (const s of sources) manifest.generatedFrom.push({ label: label(s), path: s })

  for (const sub of SUBDIRS) fs.mkdirSync(path.join(out, sub), { recursive: true })

  for (const a of actions) {
    // Rebuilt from the sources every run, so a re-run of the same inputs
    // produces the same destination.
    fs.rmSync(a.dest, { force: true })
    fs.copyFileSync(path.join(a.base, a.sub, a.file), a.dest)
    const record = {
      file: `${a.sub}/${a.file}`,
      base: { label: label(a.base), sha256: sha256(path.join(a.base, a.sub, a.file)), bytes: fs.statSync(path.join(a.base, a.sub, a.file)).size },
      incoming: [],
    }
    for (const inc of a.incoming) {
      const incFile = path.join(inc, a.sub, a.file)
      record.incoming.push({ label: label(inc), sha256: sha256(incFile), bytes: fs.statSync(incFile).size })
      if (a.sub !== 'sessions') {
        throw new Error(
          `two stores hold ${a.sub}/${a.file}; only 'sessions' databases have a defined union. ` +
            'Resolve by hand or extend SESSION_TABLES.',
        )
      }
      manifest.collisions.push(
        ...mergeSessionDb({
          destPath: a.dest,
          incomingPath: incFile,
          baseLabel: label(a.base),
          incomingLabel: label(inc),
          filename: `${a.sub}/${a.file}`,
        }),
      )
    }
    record.merged = { sha256: sha256(a.dest), bytes: fs.statSync(a.dest).size }
    manifest.databases.push(record)
  }

  fs.writeFileSync(path.join(out, 'merge-manifest.json'), JSON.stringify(manifest, null, 2) + '\n')
  console.log(`\nwrote ${actions.length} databases and merge-manifest.json`)
  console.log(`session-id collisions recorded: ${manifest.collisions.length}`)
  return { collisions: manifest.collisions, actions }
}

// One runnable check: two stores sharing a filename, one shared session id with
// different content. Asserts nothing is dropped and the collision is recorded.
function selftest() {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'ctxmerge-selftest-'))
  const SCHEMA = `
    CREATE TABLE session_events (id INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT NOT NULL,
      type TEXT NOT NULL, data TEXT NOT NULL);
    CREATE TABLE session_meta (session_id TEXT PRIMARY KEY, project_dir TEXT NOT NULL);
    CREATE TABLE session_resume (id INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT NOT NULL UNIQUE,
      snapshot TEXT NOT NULL);
    CREATE TABLE tool_calls (session_id TEXT NOT NULL, tool TEXT NOT NULL, calls INTEGER NOT NULL,
      PRIMARY KEY (session_id, tool));`

  const make = (store, rows) => {
    const dir = path.join(tmp, store, 'sessions')
    fs.mkdirSync(dir, { recursive: true })
    fs.mkdirSync(path.join(tmp, store, 'content'), { recursive: true })
    const db = new DatabaseSync(path.join(dir, 'shared.db'))
    db.exec(SCHEMA)
    for (const [sid, payload] of rows) {
      db.prepare('INSERT INTO session_meta VALUES (?, ?)').run(sid, `/p/${sid}`)
      db.prepare('INSERT INTO session_events (session_id, type, data) VALUES (?, ?, ?)').run(sid, 'e', payload)
      db.prepare('INSERT INTO session_resume (session_id, snapshot) VALUES (?, ?)').run(sid, payload)
      db.prepare('INSERT INTO tool_calls VALUES (?, ?, ?)').run(sid, 'Read', 1)
    }
    db.close()
    return path.join(tmp, store)
  }

  // `dup` exists in both with different data; `a1`/`b1` are unique to one side.
  const A = make('alpha/store', [['a1', 'from-a'], ['dup', 'a-version']])
  const B = make('beta/store', [['b1', 'from-b'], ['dup', 'b-version']])
  const out = path.join(tmp, 'out')

  const { collisions } = run({ sources: [A, B], out, apply: true })

  const db = new DatabaseSync(path.join(out, 'sessions/shared.db'))
  const metas = db.prepare('SELECT session_id FROM session_meta ORDER BY session_id').all().map(r => r.session_id)
  const events = db.prepare('SELECT session_id, data FROM session_events ORDER BY id').all()
  const resumes = db.prepare('SELECT session_id FROM session_resume').all().length
  const calls = db.prepare('SELECT COUNT(*) c FROM tool_calls').get().c
  db.close()

  const assert = (cond, msg) => { if (!cond) { console.error(`FAIL: ${msg}`); process.exitCode = 1 } }

  assert(metas.length === 4, `expected 4 sessions, got ${metas.length} (${metas})`)
  assert(metas.includes('dup') && metas.includes('store#store') === false, 'base dup kept under original id')
  assert(metas.some(m => m.startsWith('dup#')), `losing dup kept under a distinct key (${metas})`)
  assert(events.length === 4, `no event dropped, got ${events.length}`)
  assert(events.some(e => e.data === 'a-version') && events.some(e => e.data === 'b-version'),
    'both versions of the colliding session survive')
  assert(resumes === 4, `no resume dropped, got ${resumes}`)
  assert(calls === 4, `no tool_call dropped, got ${calls}`)
  assert(collisions.length === 1, `exactly one collision recorded, got ${collisions.length}`)
  assert(collisions[0]?.sessionId === 'dup', 'collision names the session id')
  assert(!!collisions[0]?.loser && !!collisions[0]?.winner, 'collision names winner and loser')

  // Idempotence: same inputs, same destination digests.
  const first = JSON.parse(fs.readFileSync(path.join(out, 'merge-manifest.json'), 'utf8'))
  run({ sources: [A, B], out, apply: true })
  const second = JSON.parse(fs.readFileSync(path.join(out, 'merge-manifest.json'), 'utf8'))
  assert(
    JSON.stringify(first.databases.map(d => d.merged.sha256)) ===
      JSON.stringify(second.databases.map(d => d.merged.sha256)),
    're-run produces identical databases',
  )

  fs.rmSync(tmp, { recursive: true, force: true })
  console.log(process.exitCode ? '\nselftest FAILED' : '\nselftest passed')
}

try {
  const args = parseArgs(process.argv.slice(2))
  if (args.selftest) selftest()
  else run(args)
} catch (e) {
  console.error(e.message)
  process.exit(1)
}
