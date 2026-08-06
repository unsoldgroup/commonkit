const CLAUDE_OWNED_KEYS = [
  'permissions',
  'hooks',
  'enabledPlugins',
  'extraKnownMarketplaces',
  'effortLevel',
  'statusLine',
  'skipDangerousModePermissionPrompt',
  'skipWorkflowUsageWarning',
  'skipAutoPermissionPrompt',
]

const SECRET_ASSIGNMENT = /(?:^|[,{\s])(?:"([^"]+)"|'([^']+)'|([A-Za-z_][A-Za-z0-9_.-]*))\s*=\s*(?:["']([^"']+)["']|([^\s,}#]+))/gim
const AUTH_VALUE = /\b(?:bearer|basic)\s+[A-Za-z0-9+/_.=-]{8,}/i
const URL_USERINFO = /\b[a-z][a-z0-9+.-]*:\/\/[^\s/@:]+:[^\s/@]+@/i

function isReference(value) {
  return /^(?:\$\{?[A-Za-z_][A-Za-z0-9_]*\}?|env:|secret:|bws:|keychain:|vault:)/i.test(value.trim())
}

function isSecretKey(key) {
  const normalized = key.replaceAll(/[^A-Za-z0-9]/g, '').toLowerCase()
  return ['apikey', 'token', 'secret', 'password', 'passwd', 'authorization', 'credential', 'privatekey']
    .some((marker) => normalized === marker || normalized.endsWith(marker))
}

function assertSafeString(value, location) {
  if (AUTH_VALUE.test(value)) throw new Error(`Embedded authorization value at ${location}`)
  if (URL_USERINFO.test(value)) throw new Error(`Embedded URL credentials at ${location}`)
  for (const match of value.matchAll(SECRET_ASSIGNMENT)) {
    const key = match[1] ?? match[2] ?? match[3]
    const assigned = match[4] ?? match[5] ?? ''
    if (isSecretKey(key) && assigned && !isReference(assigned)) throw new Error(`Embedded secret assignment for ${key} at ${location}`)
  }
}

/** Reject credentials embedded in generated configuration before it leaves the local machine. */
export function assertNoEmbeddedSecrets(value, { label = 'generated configuration' } = {}) {
  function visit(item, location) {
    if (typeof item === 'string') {
      assertSafeString(item, location)
      return
    }
    if (Array.isArray(item)) {
      item.forEach((child, index) => visit(child, `${location}[${index}]`))
      return
    }
    if (!item || typeof item !== 'object') return
    for (const [key, child] of Object.entries(item)) {
      const childLocation = `${location}.${key}`
      if (isSecretKey(key) && child !== null && child !== '' && child !== false) {
        if (typeof child !== 'string' || !isReference(child)) throw new Error(`Embedded secret-like key at ${childLocation}`)
      }
      visit(child, childLocation)
    }
  }

  if (typeof value === 'string') assertSafeString(value, label)
  else visit(value, label)
  return value
}

function rewritePaths(value, { localHome, remoteHome }) {
  if (typeof value === 'string') {
    return value
      .replaceAll(localHome, remoteHome)
      .replaceAll(`${remoteHome}/.codex/plugins/cache`, `${remoteHome}/.claude/plugins/cache`)
  }
  if (Array.isArray(value)) return value.map((item) => rewritePaths(item, { localHome, remoteHome }))
  if (value && typeof value === 'object') {
    return Object.fromEntries(
      Object.entries(value).map(([key, item]) => [key, rewritePaths(item, { localHome, remoteHome })]),
    )
  }
  return value
}

export function mergeClaudeSettings(local, remote, paths) {
  const result = structuredClone(remote)
  for (const key of CLAUDE_OWNED_KEYS) {
    if (Object.hasOwn(local, key)) result[key] = rewritePaths(local[key], paths)
  }
  return result
}

export function renderCodexConfig(source, { localHome, remoteHome }) {
  const lines = source.replaceAll(localHome, remoteHome).split('\n')
  const kept = []
  let droppingHookState = false
  let droppingMarketplace = false

  for (const line of lines) {
    if (/^\[hooks\.state\./.test(line)) {
      droppingHookState = true
      continue
    }
    if (/^\[marketplaces\./.test(line)) {
      droppingMarketplace = true
      continue
    }
    if (droppingHookState && /^\[/.test(line)) droppingHookState = false
    if (droppingMarketplace && /^\[/.test(line)) droppingMarketplace = false
    const generatedMetadata = /^\s*last_updated\s*=/i.test(line)
    if (!droppingHookState && !droppingMarketplace && !generatedMetadata && !/^\s*[A-Za-z0-9_]*(?:API_?KEY|TOKEN|SECRET|PASSWORD|AUTHORIZATION|CREDENTIAL)[A-Za-z0-9_]*\s*=/i.test(line)) {
      kept.push(line)
    }
  }

  return `${kept.join('\n').trimEnd()}\n`
}

export function renderPortableJson(source, { localHome, remoteHome, codexHome = `${remoteHome}/.codex` }) {
  const value = JSON.parse(source)
  function rewrite(item) {
    if (typeof item === 'string') return item.replaceAll(localHome, remoteHome).replaceAll(`${remoteHome}/.codex`, codexHome)
    if (Array.isArray(item)) return item.map(rewrite)
    if (item && typeof item === 'object') return Object.fromEntries(Object.entries(item).map(([key, child]) => [key, rewrite(child)]))
    return item
  }
  return `${JSON.stringify(rewrite(value), null, 2)}\n`
}

export function extractMarketplaceSections(source) {
  const sections = source.split(/(?=^\[)/m)
  return sections.filter((section) => /^\[marketplaces\./.test(section)).join('').trim()
}
