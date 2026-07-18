import fs from 'node:fs'
import crypto from 'node:crypto'

import { local, shellQuote, ssh } from './remote.mjs'

function check(name, ok, detail) { return { name, ok, detail } }

function parseJson(result, label) {
  if (result.status !== 0) return { ok: false, error: `${label} command failed` }
  try { return { ok: true, value: JSON.parse(result.stdout) } } catch { return { ok: false, error: `${label} returned invalid JSON` } }
}

function pluginMap(value, select = () => true) {
  const entries = Array.isArray(value) ? value : value?.installed
  if (!Array.isArray(entries)) throw new Error('Plugin response is not a list')
  return new Map(entries.filter(select).map((plugin) => [plugin.id ?? plugin.pluginId ?? plugin.name, {
    version: plugin.version,
    enabled: Boolean(plugin.enabled),
    installed: plugin.installed !== false,
  }]))
}

function comparePluginMaps(expected, actual) {
  const drift = []
  for (const [id, state] of expected) {
    const remote = actual.get(id)
    if (!remote) drift.push(`${id}:missing`)
    else if (JSON.stringify(remote) !== JSON.stringify(state)) drift.push(`${id}:state`)
  }
  for (const id of actual.keys()) if (!expected.has(id)) drift.push(`${id}:extra`)
  return drift
}

function currentPairingCode(config) {
  const journal = ssh(config.host, `journalctl -u ${shellQuote(config.service)} -g orca_server_ready -n 1 -o cat --no-pager`)
  if (journal.status !== 0) throw new Error('Unable to read the Orca server pairing offer')
  const line = journal.stdout.split('\n').find((candidate) => candidate.trim().startsWith('{'))
  if (!line) throw new Error('Orca server did not expose a current pairing offer')
  const pairingCode = JSON.parse(line)?.pairing?.url
  if (typeof pairingCode !== 'string' || !pairingCode.startsWith('orca://pair')) throw new Error('Orca server pairing offer is invalid')
  return pairingCode
}

function serviceRun(config, script) {
  const envFile = `${config.remoteHome}/.config/orca/orca-service.env`
  return ssh(config.host, `systemd-run --quiet --wait --pipe --collect -p ${shellQuote(`EnvironmentFile=${envFile}`)} /bin/bash -lc ${shellQuote(script)}`)
}

export function doctor(config) {
  const results = []
  const connection = ssh(config.host, 'true')
  results.push(check('ssh', connection.status === 0, connection.status === 0 ? config.host : connection.stderr.trim()))
  if (connection.status !== 0) return results
  const commands = config.requiredCommands ?? []
  const commandProbe = ssh(config.host, `for c in ${commands.map(shellQuote).join(' ')}; do command -v "$c" >/dev/null 2>&1 && printf '%s=ok\\n' "$c" || printf '%s=missing\\n' "$c"; done`)
  for (const line of commandProbe.stdout.trim().split('\n').filter(Boolean)) {
    const [name, state] = line.split('=')
    results.push(check(`command:${name}`, state === 'ok', state))
  }
  const node = ssh(config.host, 'node -p "Number(process.versions.node.split(\'.\')[0]) >= 24"')
  results.push(check('node>=24', node.stdout.trim() === 'true', node.stdout.trim()))
  const service = ssh(config.host, `systemctl is-active ${shellQuote(config.service)}`)
  results.push(check('orca-service', service.stdout.trim() === 'active', service.stdout.trim() || service.stderr.trim()))
  return results
}

function verifyContext(config) {
  const source = config.contextMode.localContent.replace('${HOME}', process.env.HOME ?? '')
  const target = config.contextMode.remoteContent
  const localNames = fs.existsSync(source) ? fs.readdirSync(source).filter((name) => name.endsWith('.db')).sort() : []
  const localHashes = new Map()
  for (const name of localNames) {
    const file = `${source}/${name}`
    const integrity = local('sqlite3', [file, 'PRAGMA integrity_check;'])
    if (integrity.status !== 0 || integrity.stdout.trim() !== 'ok') return check('context-mode:content', false, `local source is invalid: ${name}`)
    localHashes.set(name, crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex'))
  }
  const script = `set -euo pipefail; cd ${shellQuote(target)}; for f in *.db; do [ -e "$f" ] || continue; hash=$(sha256sum "$f" | cut -d' ' -f1); result=$(sqlite3 "$f" 'PRAGMA integrity_check;' 2>/dev/null || true); [ "$result" = ok ] && state=ok || state=incompatible; printf '%s\\t%s\\t%s\\n' "$f" "$hash" "$state"; done`
  const remote = ssh(config.host, script)
  if (remote.status !== 0) return check('context-mode:content', false, 'remote database probe failed')
  const entries = remote.stdout.trim().split('\n').filter(Boolean).map((line) => line.split('\t'))
  const remoteNames = entries.map(([name]) => name).sort()
  const exact = entries.every(([name, hash]) => localHashes.get(name) === hash)
  const nativeIntegrity = entries.filter(([, , state]) => state === 'ok').length
  const ok = localNames.length > 0 && JSON.stringify(localNames) === JSON.stringify(remoteNames) && exact
  return check('context-mode:content', ok, ok ? `${localNames.length} exact source-valid databases (${nativeIntegrity} validated by VPS SQLite)` : `local=${localNames.length}, remote=${remoteNames.length}, exact=${exact}`)
}

function verifyClaudePlugins(config) {
  const expectedResult = parseJson(local('claude', ['plugin', 'list', '--json']), 'local Claude plugin list')
  const actualResult = parseJson(ssh(config.host, 'claude plugin list --json'), 'remote Claude plugin list')
  if (!expectedResult.ok || !actualResult.ok) return check('plugins:claude', false, expectedResult.error ?? actualResult.error)
  try {
    const expected = pluginMap(expectedResult.value)
    const actual = pluginMap(actualResult.value)
    const drift = comparePluginMaps(expected, actual)
    return check('plugins:claude', drift.length === 0, drift.length === 0 ? `${expected.size} exactly aligned` : drift.slice(0, 5).join(', '))
  } catch (error) { return check('plugins:claude', false, error.message) }
}

function verifyCodexPlugins(config) {
  const homes = [`${config.remoteHome}/.codex`, `${config.remoteHome}/.config/orca/codex-runtime-home/home`]
  const localResult = parseJson(local('codex', ['plugin', 'list', '--json']), 'local Codex plugin list')
  if (!localResult.ok) return homes.map((home) => check(`plugins:codex:${home}`, false, localResult.error))
  let expected
  try { expected = pluginMap(localResult.value, (plugin) => plugin.marketplaceSource?.sourceType === 'git') } catch (error) {
    return homes.map((home) => check(`plugins:codex:${home}`, false, error.message))
  }
  return homes.map((remoteHome) => {
    const parsed = parseJson(ssh(config.host, `CODEX_HOME=${shellQuote(remoteHome)} codex plugin list --json`), `remote Codex plugin list (${remoteHome})`)
    if (!parsed.ok) return check(`plugins:codex:${remoteHome}`, false, parsed.error)
    try {
      const actual = pluginMap(parsed.value, (plugin) => plugin.marketplaceSource?.sourceType === 'git')
      const drift = comparePluginMaps(expected, actual)
      return check(`plugins:codex:${remoteHome}`, drift.length === 0, drift.length === 0 ? `${expected.size} managed plugins exactly aligned` : drift.slice(0, 5).join(', '))
    } catch (error) { return check(`plugins:codex:${remoteHome}`, false, error.message) }
  })
}

export function verify(config, engine) {
  const results = doctor(config)
  if (!results.find((row) => row.name === 'ssh')?.ok) return results
  for (const operation of engine.inspect()) {
    if (operation.status !== 'optional-missing') results.push(check(`${operation.domain}:${operation.target}`, operation.status === 'synced', operation.status))
  }

  const secretNames = config.serviceSecrets ?? []
  const secretScript = `node -e ${shellQuote(`for (const n of ${JSON.stringify(secretNames)}) console.log(n + '=' + (process.env[n] ? 'ok' : 'missing'))`)}`
  const secretProbe = serviceRun(config, secretScript)
  const secretStates = new Map(secretProbe.stdout.trim().split('\n').filter(Boolean).map((line) => line.split('=')))
  for (const name of secretNames) {
    const state = secretStates.get(name) ?? 'probe-failed'
    results.push(check(`service-secret:${name}`, secretProbe.status === 0 && state === 'ok', state))
  }

  const authProbe = serviceRun(config, `gh auth status >/dev/null 2>&1 && echo gh=ok || echo gh=failed
bws project list >/dev/null 2>&1 && echo bws=ok || echo bws=failed
linearis issues list -l 1 >/dev/null 2>&1 && echo linear-cli=ok || echo linear-cli=failed`)
  const authStates = new Map(authProbe.stdout.trim().split('\n').filter(Boolean).map((line) => line.split('=')))
  for (const name of ['gh', 'bws', 'linear-cli']) {
    const state = authStates.get(name) ?? 'probe-failed'
    results.push(check(`auth:${name}`, authProbe.status === 0 && state === 'ok', state))
  }

  if (config.contextMode?.enabled && config.contextMode.remoteContent) results.push(verifyContext(config))
  results.push(verifyClaudePlugins(config), ...verifyCodexPlugins(config))

  const hookFiles = [
    `${config.remoteHome}/.claude/settings.json`,
    `${config.remoteHome}/.codex/hooks.json`,
    `${config.remoteHome}/.config/orca/codex-runtime-home/home/hooks.json`,
  ]
  const hookCheckScript = `const fs=require('fs'); const home=${JSON.stringify(config.remoteHome)}; const files=${JSON.stringify(hookFiles)}; const missing=[]; function walk(x){ if(typeof x==='string'){ let expanded=x.replaceAll('$HOME',home); if(expanded.startsWith('~/')) expanded=home+expanded.slice(1); for(const m of expanded.matchAll(/\\/[^\\s"']+/g)){ const p=m[0].replace(/[);,]+$/,''); if((p.includes('/hooks/')||p.includes('/statusline/'))&&!fs.existsSync(p)) missing.push(p); } } else if(Array.isArray(x)) x.forEach(walk); else if(x&&typeof x==='object') Object.values(x).forEach(walk); } for(const f of files.filter(fs.existsSync)) { try { walk(JSON.parse(fs.readFileSync(f))) } catch { missing.push(f+':invalid-json') } } console.log(JSON.stringify([...new Set(missing)]));`
  const hooks = ssh(config.host, `node -e ${shellQuote(hookCheckScript)}`)
  let missingHooks = ['probe-failed']
  if (hooks.status === 0) { try { missingHooks = JSON.parse(hooks.stdout) } catch { missingHooks = ['invalid-probe-json'] } }
  results.push(check('hooks:targets', missingHooks.length === 0, missingHooks.length === 0 ? 'all exist' : missingHooks.slice(0, 5).join(', ')))

  let pairingCode
  try { pairingCode = currentPairingCode(config) } catch (error) {
    results.push(check('orca:remote-runtime', false, error.message), check('linear:native-orca', false, 'runtime unavailable'))
    return results
  }
  const runtimeJson = parseJson(local('orca', ['status', '--pairing-code', pairingCode, '--json']), 'Orca runtime status')
  results.push(check('orca:remote-runtime', runtimeJson.ok, runtimeJson.ok ? 'connected' : runtimeJson.error))
  const nativeLinear = parseJson(local('orca', ['linear', 'team', 'list', '--pairing-code', pairingCode, '--workspace', 'all', '--json']), 'native Orca Linear')
  const teams = nativeLinear.ok ? (Array.isArray(nativeLinear.value) ? nativeLinear.value : nativeLinear.value?.teams ?? nativeLinear.value?.result?.teams ?? nativeLinear.value?.data) : null
  const linearOk = Array.isArray(teams) && teams.length > 0
  results.push(check('linear:native-orca', linearOk, linearOk ? `${teams.length} teams connected` : nativeLinear.error ?? 'no connected Linear teams'))
  return results
}

export function loadConfig(file) {
  const raw = fs.readFileSync(file, 'utf8').replaceAll('${HOME}', process.env.HOME ?? '')
  return JSON.parse(raw)
}
