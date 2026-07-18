import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import crypto from 'node:crypto'

import { buildPlan, isForbiddenPath } from './plan.mjs'
import { assertNoEmbeddedSecrets, extractMarketplaceSections, mergeClaudeSettings, renderCodexConfig, renderPortableJson } from './transform.mjs'
import { local, readRemote, shellQuote, ssh, writeRemoteAtomic } from './remote.mjs'

export const RSYNC_EXCLUDES = [
  'auth.json', 'hosts.yml', '.env', '.env.*', '*.token', '*.sock',
  '*.db-wal', '*.db-shm', 'sessions/', 'history*', 'logs/', 'tmp/',
  'node_modules/', '.venv/', '__pycache__/',
  'orchestration.db*', 'orca-devices.json', 'orca-e2ee-keypair.json',
  'credentials.enc', '*credentials*.json', 'client_secret*.json',
  'token_cache.json', '.encryption_key', '*.pem', '*.key',
  'state_*.sqlite*', 'logs_*.sqlite*', 'memories_*.sqlite*', 'goals_*.sqlite*',
  'credentials/', 'credential/', 'secrets/', 'secret/', 'tokens/', 'token/',
  'Cookies', 'Local Storage/', 'Singleton*', '*secret*.json',
]

function digest(content) {
  return crypto.createHash('sha256').update(content).digest('hex')
}

function optional(kind) {
  return kind.endsWith('-optional')
}

function baseKind(kind) {
  return kind.replace(/-optional$/, '')
}

export function findForbiddenDescendant(root) {
  const ignoredTrees = new Set(['node_modules', '.venv', '__pycache__'])
  const stack = [root]
  while (stack.length) {
    const directory = stack.pop()
    for (const entry of fs.readdirSync(directory, { withFileTypes: true })) {
      const candidate = path.join(directory, entry.name)
      if (entry.isDirectory() && ignoredTrees.has(entry.name)) continue
      if (isForbiddenPath(candidate)) return candidate
      if (entry.isDirectory() && !entry.isSymbolicLink()) stack.push(candidate)
    }
  }
  return null
}

export class SyncEngine {
  constructor(config, { home = os.homedir(), output = console } = {}) {
    this.config = config
    this.home = home
    this.output = output
    this.plan = buildPlan({ home, remoteHome: config.remoteHome })
    this.runId = new Date().toISOString().replaceAll(/[:.]/g, '-')
    this.backupRoot = `${config.remoteHome}/.local/state/commonkit/backups/${this.runId}`
    this.validateConfig()
  }

  validateConfig() {
    if (!/^[A-Za-z0-9][A-Za-z0-9._-]*(?:@[A-Za-z0-9][A-Za-z0-9._-]*)?$/.test(this.config.host)) throw new Error('host must be a safe SSH host or alias')
    if (this.config.remoteHome === '/' || !/^\/[A-Za-z0-9._/-]+$/.test(this.config.remoteHome)) throw new Error('remoteHome must be a safe non-root absolute path')
    if (this.config.orcaInstallRoot && !/^\/[A-Za-z0-9._/-]+$/.test(this.config.orcaInstallRoot)) throw new Error('orcaInstallRoot must be a safe absolute path')
    if (!/^[A-Za-z0-9_.@-]+\.service$/.test(this.config.service)) throw new Error('Invalid systemd service name')
    if (!['exact'].includes(this.config.pluginPolicy)) throw new Error('pluginPolicy must be exact')
    if (this.config.contextMode?.enabled) {
      const remote = this.config.contextMode.remoteContent
      if (typeof remote !== 'string' || !remote.startsWith(`${this.config.remoteHome}/`) || !/^\/[A-Za-z0-9._/-]+$/.test(remote)) throw new Error('contextMode.remoteContent must be a safe path below remoteHome')
      if (typeof this.config.contextMode.localContent !== 'string' || !this.config.contextMode.localContent.startsWith('/')) throw new Error('contextMode.localContent must be absolute')
    }
    for (const [name, argv] of Object.entries(this.config.bootstrap ?? {})) {
      if (!/^[A-Za-z0-9_.-]+$/.test(name) || !Array.isArray(argv) || argv.length === 0 || argv.some((arg) => typeof arg !== 'string' || arg.includes('\0'))) {
        throw new Error(`Invalid structured bootstrap command for ${name}`)
      }
    }
    for (const name of this.config.serviceSecrets ?? []) {
      if (!/^[A-Z][A-Z0-9_]*$/.test(name)) throw new Error(`Invalid service secret name: ${name}`)
    }
    for (const [name, key] of Object.entries(this.config.bwsSecrets ?? {})) {
      if (!/^[A-Z][A-Z0-9_]*$/.test(name) || typeof key !== 'string' || !/^[A-Za-z0-9_./-]+$/.test(key)) {
        throw new Error(`Invalid BWS mapping for ${name}`)
      }
    }
  }

  transformedContent(operation) {
    const source = fs.readFileSync(operation.source, 'utf8')
    const paths = { localHome: this.home, remoteHome: this.config.remoteHome }
    if (operation.kind === 'claude-settings-merge') {
      const remote = JSON.parse(readRemote(this.config.host, operation.target) ?? '{}')
      const merged = mergeClaudeSettings(JSON.parse(source), remote, paths)
      assertNoEmbeddedSecrets(merged, { label: 'Claude settings' })
      return `${JSON.stringify(merged, null, 2)}\n`
    }
    if (operation.kind === 'codex-config-merge') {
      const rendered = renderCodexConfig(source, paths)
      assertNoEmbeddedSecrets(rendered, { label: 'Codex config' })
      return rendered
    }
    if (baseKind(operation.kind) === 'json-template') {
      const codexHome = operation.target.includes('/codex-runtime-home/home/')
        ? `${this.config.remoteHome}/.config/orca/codex-runtime-home/home`
        : `${this.config.remoteHome}/.codex`
      const rendered = renderPortableJson(source, { ...paths, codexHome })
      assertNoEmbeddedSecrets(JSON.parse(rendered), { label: 'hook JSON' })
      return rendered
    }
    return source
  }

  inspect() {
    return this.plan.map((operation) => {
      if (!fs.existsSync(operation.source)) {
        return { ...operation, status: optional(operation.kind) ? 'optional-missing' : 'missing' }
      }
      if (baseKind(operation.kind) === 'tree') return { ...operation, status: this.treeStatus(operation) }
      const desired = this.transformedContent(operation)
      let current = readRemote(this.config.host, operation.target)
      if (current !== null && operation.kind === 'codex-config-merge') {
        current = renderCodexConfig(current, { localHome: this.config.remoteHome, remoteHome: this.config.remoteHome })
      }
      return { ...operation, status: current !== null && digest(current) === digest(desired) ? 'synced' : 'changed' }
    })
  }

  treeStatus(operation) {
    const args = ['-ain', '--delete']
    for (const pattern of RSYNC_EXCLUDES) args.push('--exclude', pattern)
    args.push('-e', 'ssh -o BatchMode=yes', `${operation.source}/`, `${this.config.host}:${operation.target}/`)
    const result = local('rsync', args)
    if (result.status !== 0) return 'unreachable'
    return result.stdout.trim() === '' ? 'synced' : 'changed'
  }

  apply({ dryRun = false } = {}) {
    if (dryRun) return this.inspect()
    this.preflight()
    const results = []
    this.bootstrapToolchain()
    for (const operation of this.plan) {
      if (!fs.existsSync(operation.source)) {
        if (!optional(operation.kind)) throw new Error(`Required source is missing: ${operation.source}`)
        results.push({ ...operation, status: 'skipped' })
        this.output.log?.(`skipped          ${operation.domain}:${operation.target}`)
        continue
      }
      if (baseKind(operation.kind) === 'tree') this.syncTree(operation)
      else {
        const mode = operation.target.endsWith('.sh') ? '0755' : '0644'
        this.backupFile(operation.target)
        writeRemoteAtomic(this.config.host, operation.target, this.transformedContent(operation), mode)
      }
      results.push({ ...operation, status: 'applied' })
      this.output.log?.(`applied          ${operation.domain}:${operation.target}`)
    }
    {
      this.output.log?.('applying         plugin installations')
      this.syncPlugins()
      this.output.log?.('applied          plugin installations')
      this.output.log?.('applying         generated links')
      this.regenerateLinks()
      this.output.log?.('applied          generated links')
      if (this.config.contextMode?.enabled) this.syncContextContent()
      this.output.log?.('applied          context-mode content')
      this.output.log?.('applying         Orca service environment')
      this.configureServiceEnvironment()
      this.output.log?.('applied          Orca service environment')
      if (this.config.orcaInstallRoot && (this.config.serviceSecrets ?? []).includes('LINEAR_API_TOKEN')) {
        this.output.log?.('applying         native Orca Linear connection')
        this.connectNativeLinear()
        this.output.log?.('applied          native Orca Linear connection')
      }
    }
    return results
  }

  preflight() {
    const connection = ssh(this.config.host, 'true')
    if (connection.status !== 0) throw new Error(`SSH preflight failed: ${connection.stderr.trim()}`)
    const bootstrapCommands = new Set(Object.keys(this.config.bootstrap ?? {}))
    const required = (this.config.requiredCommands ?? []).filter((command) => !bootstrapCommands.has(command))
    const commandProbe = ssh(this.config.host, `for c in ${required.map(shellQuote).join(' ')}; do command -v "$c" >/dev/null 2>&1 || { echo "$c"; exit 1; }; done`)
    if (commandProbe.status !== 0) throw new Error(`Remote prerequisite is missing: ${commandProbe.stdout.trim() || commandProbe.stderr.trim()}`)
    for (const operation of this.plan) {
      if (!fs.existsSync(operation.source)) {
        if (!optional(operation.kind)) throw new Error(`Required source is missing: ${operation.source}`)
        continue
      }
      if (baseKind(operation.kind) !== 'tree') continue
      const forbidden = findForbiddenDescendant(operation.source)
      if (forbidden) throw new Error(`Forbidden path exists inside a managed tree: ${forbidden}`)
    }
    const targets = this.plan.map(({ target }) => target)
    if (this.config.contextMode?.enabled) targets.push(this.config.contextMode.remoteContent)
    const pathProbe = ssh(this.config.host, `set -e; home=$(realpath -m ${shellQuote(this.config.remoteHome)}); for target in ${targets.map(shellQuote).join(' ')}; do resolved=$(realpath -m "$target"); case "$resolved" in "$home"/*) ;; *) echo "$target -> $resolved"; exit 1;; esac; done`)
    if (pathProbe.status !== 0) throw new Error(`Remote target escapes remoteHome: ${pathProbe.stdout.trim() || pathProbe.stderr.trim()}`)
  }

  syncPlugins() {
    let claudeList = local('claude', ['plugin', 'list', '--json'])
    if (claudeList.status !== 0) throw new Error(`Unable to list local Claude plugins: ${claudeList.stderr.trim()}`)
    const desiredClaude = JSON.parse(claudeList.stdout)
    const remoteClaudeList = ssh(this.config.host, 'claude plugin list --json')
    if (remoteClaudeList.status !== 0) throw new Error('Unable to list remote Claude plugins')
    writeRemoteAtomic(this.config.host, `${this.backupRoot}/plugin-state/claude.json`, remoteClaudeList.stdout, '0600')
    const remoteClaude = JSON.parse(remoteClaudeList.stdout)
    const remoteClaudeById = new Map(remoteClaude.map((plugin) => [plugin.id ?? plugin.pluginId ?? plugin.name, plugin]))
    const desiredClaudeIds = new Set(desiredClaude.map((plugin) => plugin.id ?? plugin.pluginId ?? plugin.name).filter(Boolean))
    for (const plugin of remoteClaude) {
      const id = plugin.id ?? plugin.pluginId ?? plugin.name
      if (id && !desiredClaudeIds.has(id)) {
        const removed = ssh(this.config.host, `claude plugin uninstall ${shellQuote(id)} >/dev/null`)
        if (removed.status !== 0) throw new Error(`Unable to remove remote Claude plugin ${id}`)
      }
    }
    for (const plugin of desiredClaude) {
      const id = plugin.id ?? plugin.pluginId ?? plugin.name
      if (!id) continue
      const updated = ssh(this.config.host, `claude plugin update ${shellQuote(id)} >/dev/null 2>&1 || claude plugin install ${shellQuote(id)} >/dev/null`)
      if (updated.status !== 0) throw new Error(`Unable to install Claude plugin ${id}`)
      const existing = remoteClaudeById.get(id)
      if (!existing || Boolean(existing.enabled) !== (plugin.enabled !== false)) {
        const toggle = plugin.enabled === false ? 'disable' : 'enable'
        const toggled = ssh(this.config.host, `claude plugin ${toggle} ${shellQuote(id)} >/dev/null`)
        if (toggled.status !== 0) throw new Error(`Unable to ${toggle} Claude plugin ${id}: ${toggled.stderr.trim() || toggled.stdout.trim()}`)
      }
    }

    const marketplaceList = local('codex', ['plugin', 'marketplace', 'list', '--json'])
    const pluginList = local('codex', ['plugin', 'list', '--json'])
    if (marketplaceList.status !== 0 || pluginList.status !== 0) throw new Error('Unable to inspect local Codex plugin state')
    const marketplaces = JSON.parse(marketplaceList.stdout).marketplaces
      .filter((entry) => entry.marketplaceSource?.sourceType === 'git')
    const gitNames = new Set(marketplaces.map((entry) => entry.name))
    const plugins = JSON.parse(pluginList.stdout).installed
      .filter((entry) => entry.installed && gitNames.has(entry.marketplaceName))
      .map((entry) => entry.pluginId)
    const homes = [
      `${this.config.remoteHome}/.codex`,
      `${this.config.remoteHome}/.config/orca/codex-runtime-home/home`,
    ]
    for (const home of homes) {
      for (const marketplace of marketplaces) {
        const result = ssh(this.config.host, `CODEX_HOME=${shellQuote(home)} codex plugin marketplace add ${shellQuote(marketplace.marketplaceSource.source)} --json >/dev/null 2>&1 || { CODEX_HOME=${shellQuote(home)} codex plugin marketplace remove ${shellQuote(marketplace.name)} --json >/dev/null 2>&1 || true; CODEX_HOME=${shellQuote(home)} codex plugin marketplace add ${shellQuote(marketplace.marketplaceSource.source)} --json >/dev/null; }`)
        if (result.status !== 0) throw new Error(`Unable to install Codex marketplace ${marketplace.name}: ${result.stderr.trim()}`)
      }
      for (const plugin of plugins) {
        const result = ssh(this.config.host, `CODEX_HOME=${shellQuote(home)} codex plugin add ${shellQuote(plugin)} --json >/dev/null`)
        if (result.status !== 0) throw new Error(`Unable to install Codex plugin ${plugin}: ${result.stderr.trim()}`)
      }
      const remotePluginList = ssh(this.config.host, `CODEX_HOME=${shellQuote(home)} codex plugin list --json`)
      if (remotePluginList.status !== 0) throw new Error(`Unable to inspect Codex plugins in ${home}`)
      const inventoryName = home.endsWith('/.codex') ? 'codex-shell.json' : 'codex-orca-runtime.json'
      writeRemoteAtomic(this.config.host, `${this.backupRoot}/plugin-state/${inventoryName}`, remotePluginList.stdout, '0600')
      const desiredIds = new Set(plugins)
      for (const installed of JSON.parse(remotePluginList.stdout).installed ?? []) {
        if (installed.marketplaceSource?.sourceType === 'git' && installed.installed && !desiredIds.has(installed.pluginId)) {
          const removed = ssh(this.config.host, `CODEX_HOME=${shellQuote(home)} codex plugin remove ${shellQuote(installed.pluginId)} --json >/dev/null`)
          if (removed.status !== 0) throw new Error(`Unable to remove Codex plugin ${installed.pluginId}`)
        }
      }
      const operation = this.plan.find((item) => item.kind === 'codex-config-merge' && item.target === `${home}/config.toml`)
      const installedConfig = readRemote(this.config.host, operation.target) ?? ''
      const marketplaceConfig = extractMarketplaceSections(installedConfig)
      const desired = this.transformedContent(operation)
      this.backupFile(operation.target)
      // The local config is the authority for installed plugin enablement. Rewriting it
      // after add/remove makes disabled plugins converge even though Codex has no
      // dedicated plugin enable/disable subcommand.
      writeRemoteAtomic(this.config.host, operation.target, `${desired.trimEnd()}\n\n${marketplaceConfig}\n`, '0644')
    }
  }

  backupFile(target) {
    const backup = `${this.backupRoot}${target}`
    const directory = backup.slice(0, backup.lastIndexOf('/'))
    const result = ssh(this.config.host, `[ ! -e ${shellQuote(target)} ] || [ -e ${shellQuote(backup)} ] || { install -d -m 0700 ${shellQuote(directory)}; cp -a ${shellQuote(target)} ${shellQuote(backup)}; }`)
    if (result.status !== 0) throw new Error(`Unable to back up ${target}: ${result.stderr.trim()}`)
  }

  bootstrapToolchain() {
    for (const [command, argv] of Object.entries(this.config.bootstrap ?? {})) {
      const installer = argv.map(shellQuote).join(' ')
      const result = ssh(this.config.host, `command -v ${shellQuote(command)} >/dev/null 2>&1 || ${installer}`)
      if (result.status !== 0) throw new Error(`Unable to install ${command}: ${result.stderr.trim()}`)
    }
  }

  syncTree(operation) {
    const mkdir = ssh(this.config.host, `install -d -m 0755 ${shellQuote(operation.target)}`)
    if (mkdir.status !== 0) throw new Error(mkdir.stderr.trim())
    const backupDirectory = `${this.backupRoot}${operation.target}`
    const args = ['-a', '--delete-delay', '--backup', `--backup-dir=${backupDirectory}`]
    for (const pattern of RSYNC_EXCLUDES) args.push('--exclude', pattern)
    args.push('-e', 'ssh -o BatchMode=yes', `${operation.source}/`, `${this.config.host}:${operation.target}/`)
    const result = local('rsync', args)
    if (result.status !== 0) throw new Error(`rsync failed for ${operation.source}: ${result.stderr.trim()}`)
  }

  regenerateLinks() {
    const home = this.config.remoteHome
    const claudeAgents = `${home}/.claude/agents`
    const claudeSkills = `${home}/.claude/skills`
    const codexSkills = `${home}/.codex/skills`
    const orcaSkills = `${home}/.config/orca/codex-runtime-home/home/skills`
    const script = `set -euo pipefail
install -d -m 0755 ${shellQuote(claudeAgents)} ${shellQuote(claudeSkills)} ${shellQuote(codexSkills)} ${shellQuote(orcaSkills)}
find ${shellQuote(claudeAgents)} -maxdepth 1 -type l -delete
for f in ${shellQuote(`${home}/.agents/claude-agents`)}/*.md; do [ -e "$f" ] || continue; ln -sfn "../../.agents/claude-agents/$(basename "$f")" ${shellQuote(claudeAgents)}/"$(basename "$f")"; done
find ${shellQuote(claudeSkills)} ${shellQuote(codexSkills)} -mindepth 1 -maxdepth 1 -type l -delete
for d in ${shellQuote(`${home}/.agents/skills`)}/*; do [ -d "$d" ] || continue; n=$(basename "$d"); ln -sfn "../../.agents/skills/$n" ${shellQuote(claudeSkills)}/"$n"; ln -sfn "../../.agents/skills/$n" ${shellQuote(codexSkills)}/"$n"; ln -sfn "$d" ${shellQuote(orcaSkills)}/"$n"; done`
    const result = ssh(this.config.host, script)
    if (result.status !== 0) throw new Error(`Unable to regenerate links: ${result.stderr.trim()}`)
  }

  syncContextContent() {
    const source = this.config.contextMode.localContent.replace('${HOME}', this.home)
    if (!fs.existsSync(source)) return
    const target = this.config.contextMode.remoteContent
    const snapshot = fs.mkdtempSync(path.join(os.tmpdir(), 'commonkit-context-'))
    try {
      for (const entry of fs.readdirSync(source, { withFileTypes: true })) {
        if (!entry.isFile() || !entry.name.endsWith('.db')) continue
        const input = path.join(source, entry.name)
        const output = path.join(snapshot, entry.name)
        let valid = false
        for (let attempt = 0; attempt < 3 && !valid; attempt += 1) {
          local('sqlite3', [input, 'PRAGMA wal_checkpoint(PASSIVE);'])
          fs.copyFileSync(input, output)
          const integrity = local('sqlite3', [output, 'PRAGMA integrity_check;'])
          valid = integrity.status === 0 && integrity.stdout.trim() === 'ok'
        }
        if (!valid) throw new Error(`Unable to create an integral SQLite snapshot for ${entry.name}`)
      }
      const result = ssh(this.config.host, `install -d -m 0700 ${shellQuote(target)}`)
      if (result.status !== 0) throw new Error(result.stderr.trim())
      const names = fs.readdirSync(snapshot).filter((name) => name.endsWith('.db'))
      const staleBackup = `${this.backupRoot}${target}`
      const reconcile = ssh(this.config.host, `set -e; install -d -m 0700 ${shellQuote(staleBackup)}; cd ${shellQuote(target)}; for f in *.db; do [ -e "$f" ] || continue; keep=false; for name in ${names.map(shellQuote).join(' ')}; do [ "$f" != "$name" ] || keep=true; done; $keep || mv "$f" ${shellQuote(staleBackup)}/; done`)
      if (reconcile.status !== 0) throw new Error(`Unable to back up stale context databases: ${reconcile.stderr.trim()}`)
      const args = ['-a', '--checksum', '--backup', `--backup-dir=${this.backupRoot}${target}`, '-e', 'ssh -o BatchMode=yes', `${snapshot}/`, `${this.config.host}:${target}/`]
      const synced = local('rsync', args)
      if (synced.status !== 0) throw new Error(`Context-mode sync failed: ${synced.stderr.trim()}`)
    } finally {
      fs.rmSync(snapshot, { recursive: true, force: true })
    }
  }

  configureServiceEnvironment() {
    const names = this.config.serviceSecrets ?? []
    const quotedNames = names.map(shellQuote).join(' ')
    const service = shellQuote(this.config.service)
    const home = this.config.remoteHome
    const configDir = `${home}/.config/orca`
    const envfile = `${configDir}/orca-service.env`
    const dropinDir = `/etc/systemd/system/${this.config.service}.d`
    const dropin = `${dropinDir}/20-commonkit.conf`
    const envBackup = `${this.backupRoot}${envfile}`
    const dropinBackup = `${this.backupRoot}${dropin}`
    const bwsMappings = Object.entries(this.config.bwsSecrets ?? {})
      .map(([name, key]) => `${shellQuote(name)}:${shellQuote(key)}`)
      .join(' ')
    const script = `set -euo pipefail
umask 077
install -d -m 0700 ${shellQuote(configDir)} ${shellQuote(dropinDir)}
envfile=${shellQuote(envfile)}
next=$(mktemp ${shellQuote(`${configDir}/.orca-service.env.XXXXXX`)})
[ ! -f "$envfile" ] || cp "$envfile" "$next"
for name in ${quotedNames}; do
  value=$(printenv "$name" || true)
  if [ -n "$value" ]; then
    [[ "$value" =~ ^[A-Za-z0-9_./:+@=-]+$ ]] || { echo "unsafe value for $name" >&2; exit 1; }
    grep -v "^$name=" "$next" > "\${next}.filtered" || true; mv "\${next}.filtered" "$next"; printf '%s=%s\\n' "$name" "$value" >> "$next"
  fi
done
for mapping in ${bwsMappings || "''"}; do
  [ -n "$mapping" ] || continue
  name="\${mapping%%:*}"
  key="\${mapping#*:}"
  value=$(bws secret list | jq -r --arg key "$key" '.[] | select(.key == $key) | .value' | head -n 1)
  if [ -n "$value" ]; then
    [[ "$value" =~ ^[A-Za-z0-9_./:+@=-]+$ ]] || { echo "unsafe BWS value for $name" >&2; exit 1; }
    grep -v "^\${name}=" "$next" > "\${next}.filtered" || true; mv "\${next}.filtered" "$next"; printf '%s=%s\\n' "$name" "$value" >> "$next"
  fi
done
for name in ${quotedNames}; do
  grep -Eq "^$name=[A-Za-z0-9_./:+@=-]+$" "$next" || { echo "missing or invalid required service secret: $name" >&2; exit 1; }
done
install -d -m 0700 ${shellQuote(envBackup.slice(0, envBackup.lastIndexOf('/')))} ${shellQuote(dropinBackup.slice(0, dropinBackup.lastIndexOf('/')))}
[ ! -f "$envfile" ] || [ -f ${shellQuote(envBackup)} ] || cp -a "$envfile" ${shellQuote(envBackup)}
[ ! -f ${shellQuote(dropin)} ] || [ -f ${shellQuote(dropinBackup)} ] || cp -a ${shellQuote(dropin)} ${shellQuote(dropinBackup)}
chmod 0600 "$next"; mv -f "$next" "$envfile"
dropin_next=$(mktemp ${shellQuote(`${dropinDir}/.commonkit.XXXXXX`)})
printf '%s\\n' '[Service]' ${shellQuote(`EnvironmentFile=${envfile}`)} > "$dropin_next"; chmod 0644 "$dropin_next"; mv -f "$dropin_next" ${shellQuote(dropin)}
systemctl daemon-reload
systemctl restart ${service}`
    const result = ssh(this.config.host, script)
    if (result.status !== 0) throw new Error(`Unable to configure Orca service environment: ${result.stderr.trim()}`)
  }

  connectNativeLinear() {
    const root = this.config.orcaInstallRoot
    const runtimeClient = `${root}/resources/app.asar.unpacked/out/cli/runtime-client.js`
    const electron = `${root}/orca-ide`
    const envfile = `${this.config.remoteHome}/.config/orca/orca-service.env`
    const source = `const {execFileSync}=require('node:child_process'); const {RuntimeClient}=require(${JSON.stringify(runtimeClient)}); (async()=>{const out=execFileSync('journalctl',['-u',${JSON.stringify(this.config.service)},'-g','orca_server_ready','-n','1','-o','cat','--no-pager'],{encoding:'utf8'}); const line=out.split('\\n').find(x=>x.trim().startsWith('{')); if(!line) throw new Error('No current Orca pairing offer'); const pairing=JSON.parse(line).pairing.url; const c=new RuntimeClient(undefined,undefined,pairing,undefined); await c.call('linear.connect',{apiKey:process.env.LINEAR_API_TOKEN},{timeoutMs:30000}); c.close?.(); console.log('connected')})().catch(e=>{console.error(e.message);process.exit(1)})`
    const encoded = Buffer.from(source).toString('base64')
    const script = `set -euo pipefail; f=$(mktemp /tmp/commonkit-linear.XXXXXX.js); trap 'rm -f "$f"' EXIT; printf %s ${shellQuote(encoded)} | base64 -d > "$f"; systemd-run --quiet --wait --pipe --collect -p ${shellQuote(`EnvironmentFile=${envfile}`)} -E ELECTRON_RUN_AS_NODE=1 ${shellQuote(electron)} "$f"`
    const result = ssh(this.config.host, script)
    if (result.status !== 0 || !result.stdout.includes('connected')) throw new Error(`Unable to connect native Orca Linear: ${result.stderr.trim() || result.stdout.trim()}`)
  }
}
