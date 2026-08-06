import { spawnSync } from 'node:child_process'

function run(command, args, options = {}) {
  const result = spawnSync(command, args, { encoding: 'utf8', maxBuffer: 64 * 1024 * 1024, ...options })
  if (result.error) throw result.error
  return { status: result.status, stdout: result.stdout ?? '', stderr: result.stderr ?? '' }
}

export function local(command, args = [], options = {}) {
  return run(command, args, options)
}

export function ssh(host, script, options = {}) {
  return run('ssh', ['-o', 'BatchMode=yes', host, `bash -lc ${shellQuote(script)}`], options)
}

export function readRemote(host, target) {
  const result = ssh(host, `cat -- ${shellQuote(target)}`)
  if (result.status !== 0) return null
  return result.stdout
}

export function writeRemoteAtomic(host, target, content, mode = '0600') {
  const directory = target.slice(0, target.lastIndexOf('/')) || '/'
  const script = `set -euo pipefail; install -d -m 0700 ${shellQuote(directory)}; tmp=$(mktemp ${shellQuote(`${directory}/.commonkit.XXXXXX`)}); cat > "$tmp"; chmod ${mode} "$tmp"; mv -f "$tmp" ${shellQuote(target)}`
  const result = ssh(host, script, { input: content })
  if (result.status !== 0) throw new Error(`Unable to write ${target}: ${result.stderr.trim()}`)
}

export function shellQuote(value) {
  return `'${String(value).replaceAll("'", `'"'"'`)}'`
}
