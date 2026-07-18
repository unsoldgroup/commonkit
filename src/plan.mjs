import path from 'node:path'

const FORBIDDEN = [
  /(^|\/)auth\.json$/,
  /(^|\/)hosts\.yml$/,
  /(^|\/)credentials\.enc$/i,
  /(^|\/).*credentials.*\.json$/i,
  /(^|\/)client_secret.*\.json$/i,
  /(^|\/)token_cache\.json$/i,
  /(^|\/)\.encryption_key$/i,
  /(^|\/)\.env(?:\.|$)/,
  /(^|\/)(?:credentials?|tokens?|secrets?)(?:\/|$|\.(?:json|ya?ml|txt|enc)$)/i,
  /(?:^|\/)orchestration\.db(?:-(?:wal|shm))?$/,
  /(?:^|\/)(?:state|logs|memories|goals)_\d+\.sqlite(?:-(?:wal|shm))?$/,
  /(?:^|\/)(?:state|logs|memories|goals)(?:_[0-9]+)?\.(?:db|sqlite|sqlite3)(?:-(?:wal|shm))?$/,
  /(?:^|\/)context-mode\/sessions\//,
  /(?:^|\/)orca-(?:devices|e2ee-keypair)\.json$/,
  /(?:^|\/)(?:Cookies|Local Storage|Singleton[^/]*)$/,
  /\.(?:sock|token)$/,
  /\.(?:pem|key)$/i,
]

export function isForbiddenPath(candidate) {
  return FORBIDDEN.some((pattern) => pattern.test(candidate))
}

export function buildPlan({ home, remoteHome }) {
  const local = (...parts) => path.posix.join(home, ...parts)
  const remote = (...parts) => path.posix.join(remoteHome, ...parts)
  const orcaCodex = remote('.config/orca/codex-runtime-home/home')

  const operations = [
    { domain: 'instructions', kind: 'template', source: local('.claude/CLAUDE.md'), target: remote('.claude/CLAUDE.md') },
    { domain: 'instructions', kind: 'template', source: local('.codex/AGENTS.md'), target: remote('.codex/AGENTS.md') },
    { domain: 'instructions', kind: 'template', source: local('.codex/AGENTS.md'), target: path.posix.join(orcaCodex, 'AGENTS.md') },
    { domain: 'instructions', kind: 'template-optional', source: local('.claude/RTK.md'), target: remote('.claude/RTK.md') },
    { domain: 'agents', kind: 'tree', source: local('.agents/claude-agents'), target: remote('.agents/claude-agents') },
    { domain: 'agents', kind: 'tree-optional', source: local('.agents/agents'), target: remote('.agents/agents') },
    { domain: 'skills', kind: 'tree', source: local('.agents/skills'), target: remote('.agents/skills') },
    { domain: 'skills', kind: 'tree-optional', source: local('.agents/skills-library'), target: remote('.agents/skills-library') },
    { domain: 'skills', kind: 'file-optional', source: local('.agents/.skill-lock.json'), target: remote('.agents/.skill-lock.json') },
    { domain: 'claude', kind: 'claude-settings-merge', source: local('.claude/settings.json'), target: remote('.claude/settings.json') },
    { domain: 'claude', kind: 'tree-optional', source: local('.claude/hooks'), target: remote('.claude/hooks') },
    { domain: 'claude', kind: 'file-optional', source: local('.claude/statusline.sh'), target: remote('.claude/statusline.sh') },
    { domain: 'codex', kind: 'codex-config-merge', source: local('.codex/config.toml'), target: remote('.codex/config.toml') },
    { domain: 'codex', kind: 'codex-config-merge', source: local('.codex/config.toml'), target: path.posix.join(orcaCodex, 'config.toml') },
    { domain: 'codex', kind: 'tree-optional', source: local('.codex/hooks'), target: remote('.codex/hooks') },
    { domain: 'codex', kind: 'tree-optional', source: local('.codex/hooks'), target: path.posix.join(orcaCodex, 'hooks') },
    { domain: 'codex', kind: 'json-template-optional', source: local('.codex/hooks.json'), target: remote('.codex/hooks.json') },
    { domain: 'codex', kind: 'json-template-optional', source: local('.codex/hooks.json'), target: path.posix.join(orcaCodex, 'hooks.json') },
    { domain: 'codex', kind: 'tree-optional', source: local('.codex/rules'), target: remote('.codex/rules') },
    { domain: 'codex', kind: 'tree-optional', source: local('.codex/rules'), target: path.posix.join(orcaCodex, 'rules') },
    { domain: 'codex', kind: 'file-optional', source: local('.codex/code_review.md'), target: remote('.codex/code_review.md') },
    { domain: 'codex', kind: 'tree-optional', source: local('.codex/templates'), target: remote('.codex/templates') },
    { domain: 'orca', kind: 'tree-optional', source: local('.orca/agent-hooks'), target: remote('.orca/agent-hooks') },
  ]

  return operations.filter(({ source, target }) => !isForbiddenPath(source) && !isForbiddenPath(target))
}
