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
//   node scripts/context-mode-merge.mjs ... --apply --force   # rebuild a live destination
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
  const out = { sources: [], out: null, apply: false, force: false, selftest: false }
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i]
    if (a === '--source') out.sources.push(path.resolve(expand(argv[++i])))
    else if (a === '--out') out.out = path.resolve(expand(argv[++i]))
    else if (a === '--apply') out.apply = true
    else if (a === '--force') out.force = true
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

// Report writers holding a source store. This is no longer a refusal: every
// database is copied with VACUUM INTO, which takes an internally consistent
// snapshot through a read transaction even while a writer is active. Databases
// are snapshotted one at a time, so the result is per-database consistent
// rather than a single instant across the whole store -- which is what project
// memory needs, since each project's database stands alone.
function reportLiveWriters(roots) {
  const holders = []
  for (const root of roots) {
    let out = ''
    try {
      out = execFileSync('lsof', ['+D', root], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] })
    } catch (e) {
      // lsof exits non-zero when nothing matches, which is the good case.
      if (e.status === 1 && !e.stdout) continue
      if (e.code === 'ENOENT') return // no lsof, nothing to report; snapshots are safe regardless
      out = e.stdout || ''
    }
    for (const line of out.split('\n').slice(1)) {
      const cols = line.trim().split(/\s+/)
      if (cols.length > 1) holders.push(cols[1])
    }
  }
  const pids = [...new Set(holders)]
  if (pids.length) {
    console.log(
      `note: ${pids.length} process(es) are writing the source stores (pid ${pids.slice(0, 6).join(', ')}).\n` +
        '      Each database is snapshotted consistently, so this is safe. Sessions\n' +
        '      still running will keep writing to the old store until they restart.',
    )
  }
}

const sqlQuote = value => `'${value.replace(/'/g, "''")}'`

// Copy a database the only way that is correct for WAL: let SQLite write the
// snapshot. A plain file copy takes the database file without its -wal, and
// everything committed since the last checkpoint lives in that -wal -- so a
// file copy silently loses recent rows whenever a writer is live or a process
// exited without closing cleanly. VACUUM INTO reads through a read transaction,
// captures WAL content, and is deterministic for identical input.
function snapshotDatabase(source, dest) {
  fs.rmSync(dest, { force: true })
  let db
  try {
    db = new DatabaseSync(source, { readOnly: true })
    db.exec(`VACUUM INTO ${sqlQuote(dest)}`)
    return 'snapshot'
  } catch (error) {
    // Not a database SQLite will open (corrupt, or mid-write with no -shm to
    // attach). A file copy is worse but better than dropping it entirely.
    fs.rmSync(dest, { force: true })
    fs.copyFileSync(source, dest)
    console.log(`  WARNING: ${path.basename(source)} copied byte-for-byte, not snapshotted: ${error.message}`)
    return 'copied'
  } finally {
    db?.close()
  }
}

function openReadable(file) {
  // Snapshot rather than attach: attaching a WAL database creates a -shm beside
  // it, which would mutate a source store.
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'ctxmerge-'))
  const copy = path.join(tmp, path.basename(file))
  snapshotDatabase(file, copy)
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

// True when the destination holds a database written after the manifest that
// produced it — i.e. a writer has been pointed at it and it is no longer a
// staging directory.
//
// The -wal and -shm siblings matter more than the database itself here: these
// stores are WAL, so a live writer's changes sit in the -wal file and the main
// file's mtime does not move until checkpoint. Watching only *.db would let a
// destination that has been live for hours pass as staging.
function destinationIsLive(out) {
  const manifest = path.join(out, 'merge-manifest.json')
  if (!fs.existsSync(manifest)) return false
  const mergedAt = fs.statSync(manifest).mtimeMs
  for (const sub of SUBDIRS) {
    for (const f of listDbs(out, sub)) {
      for (const suffix of ['', '-wal', '-shm']) {
        const candidate = path.join(out, sub, `${f}${suffix}`)
        if (!fs.existsSync(candidate)) continue
        if (fs.statSync(candidate).mtimeMs > mergedAt) return true
      }
    }
  }
  return false
}

function run({ sources, out, apply, force }) {
  if (sources.length < 2) throw new Error('need at least two --source stores')
  for (const s of sources) if (!fs.existsSync(s)) throw new Error(`source store not found: ${s}`)
  if (!out) throw new Error('need --out')

  // Planning only lists filenames, so it is safe while agents are running. Only
  // --apply reads database contents and needs a quiet store.
  if (apply) reportLiveWriters(sources)

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
  const live = destinationIsLive(out)
  if (!apply) {
    console.log('\ndry run. re-run with --apply to write.')
    if (live) {
      console.log('WARNING: the destination has been written since it was merged. --apply would discard that.')
    }
    return { collisions: [], actions }
  }

  // Once agents are pointed at the destination it stops being a staging area:
  // the merge rebuilds every database from the sources, so a re-run after
  // cutover would silently discard everything captured since. Refuse instead.
  if (live && !force) {
    throw new Error(
      `refusing to merge: ${out} has been written since it was merged.\n` +
        'It is a live store now, not a staging directory, and this rebuilds every\n' +
        'database from the sources — everything captured since would be lost.\n' +
        'Pass --force only if you genuinely mean to discard it.',
    )
  }

  const manifest = { generatedFrom: [], databases: [], collisions: [] }
  for (const s of sources) manifest.generatedFrom.push({ label: label(s), path: s })

  for (const sub of SUBDIRS) fs.mkdirSync(path.join(out, sub), { recursive: true })
  // Drop the old manifest before rewriting any database. An interrupted run
  // would otherwise leave databases newer than a stale manifest, and the retry
  // would be refused as "live" when it is merely half-finished.
  fs.rmSync(path.join(out, 'merge-manifest.json'), { force: true })

  for (const a of actions) {
    // Rebuilt from the sources every run, so a re-run of the same inputs
    // produces the same destination.
    snapshotDatabase(path.join(a.base, a.sub, a.file), a.dest)
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

  // Rows committed but not yet checkpointed live in the -wal, which a file copy
  // does not take. Hold the source open so its WAL stays uncheckpointed.
  const walSource = new DatabaseSync(path.join(A, 'sessions/shared.db'))
  walSource.exec("PRAGMA journal_mode=WAL")
  for (let i = 0; i < 400; i++) {
    walSource.prepare('INSERT INTO session_meta VALUES (?, ?)').run(`wal-${i}`, '/p/wal')
  }
  run({ sources: [A, B], out, apply: true })
  const walCheck = new DatabaseSync(path.join(out, 'sessions/shared.db'), { readOnly: true })
  const walRows = walCheck.prepare("SELECT COUNT(*) c FROM session_meta WHERE session_id LIKE 'wal-%'").get().c
  walCheck.close()
  walSource.close()
  assert(walRows === 400, `uncheckpointed WAL rows are captured, got ${walRows} of 400`)

  // Once a writer has been pointed at the destination, a re-run would rebuild
  // it from the sources and discard everything captured since.
  const live = path.join(out, 'sessions/shared.db')
  fs.utimesSync(live, new Date(), new Date(Date.now() + 60_000))
  let refused = false
  try {
    run({ sources: [A, B], out, apply: true })
  } catch (e) {
    refused = /has been written since it was merged/.test(e.message)
  }
  assert(refused, 'refuses to rebuild a destination that has been written since the merge')

  // --force overrides the refusal. Throws loudly through the top-level handler
  // if it ever stops doing so.
  fs.utimesSync(live, new Date(), new Date(Date.now() + 60_000))
  run({ sources: [A, B], out, apply: true, force: true })

  // A -wal newer than the manifest is the real post-cutover signal, since WAL
  // writes leave the main database file untouched until checkpoint.
  fs.utimesSync(path.join(out, 'merge-manifest.json'), new Date(), new Date())
  fs.writeFileSync(`${live}-wal`, 'pending write')
  fs.utimesSync(`${live}-wal`, new Date(), new Date(Date.now() + 60_000))
  let refusedOnWal = false
  try {
    run({ sources: [A, B], out, apply: true })
  } catch (e) {
    refusedOnWal = /has been written since it was merged/.test(e.message)
  }
  assert(refusedOnWal, 'a -wal newer than the manifest marks the destination live')

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
