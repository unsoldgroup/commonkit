#!/usr/bin/env node
import path from 'node:path'
import { fileURLToPath } from 'node:url'

import { SyncEngine } from './sync.mjs'
import { doctor, loadConfig, verify } from './verify.mjs'

const here = path.dirname(fileURLToPath(import.meta.url))
const defaultConfig = path.resolve(process.cwd(), 'commonkit.json')
const args = process.argv.slice(2)
const command = args.find((arg) => !arg.startsWith('-')) ?? 'help'
const configIndex = args.indexOf('--config')
const configFile = configIndex >= 0 ? path.resolve(args[configIndex + 1]) : defaultConfig

function printRows(rows) {
  for (const row of rows) {
    const mark = row.ok === undefined ? row.status : row.ok ? 'ok' : 'FAIL'
    const name = row.name ?? `${row.domain}:${row.target}`
    const detail = row.detail ? ` (${row.detail})` : ''
    console.log(`${mark.padEnd(16)} ${name}${detail}`)
  }
}

function help() {
  console.log(`commonkit — carry shared developer capabilities across targets

Usage:
  commonkit doctor [--config file]
  commonkit diff [--config file]
  commonkit apply --dry-run [--config file]
  commonkit apply --yes [--config file]
  commonkit plugins --yes [--config file]
  commonkit verify [--config file]

Safety:
  apply requires --yes; credentials, sessions, live databases, device IDs,
  cookies, sockets, and OAuth state are never copied.`)
}

if (command === 'help' || args.includes('--help') || args.includes('-h')) {
  help()
  process.exit(0)
}

const config = loadConfig(configFile)
const engine = new SyncEngine(config)

if (command === 'doctor') printRows(doctor(config))
else if (command === 'diff') printRows(engine.inspect())
else if (command === 'apply') {
  const dryRun = args.includes('--dry-run')
  if (!dryRun && !args.includes('--yes')) throw new Error('Refusing to apply without --yes. Use --dry-run to preview.')
  printRows(engine.apply({ dryRun }))
} else if (command === 'verify') {
  const rows = verify(config, engine)
  printRows(rows)
  if (rows.some((row) => !row.ok)) process.exitCode = 1
} else if (command === 'plugins') {
  if (!args.includes('--yes')) throw new Error('Refusing to change plugins without --yes.')
  engine.syncPlugins()
  console.log('ok               Plugin state converged')
} else {
  help()
  process.exitCode = 1
}
