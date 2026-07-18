import { mkdir, readFile, rename, stat, writeFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { defaultConfigPath, stateDir } from './paths.js';
import type {
  RelayConfig,
  RelayMenuAction,
  RelayMenuActionMethod,
  RelayMenuConfig,
  RelayMode,
  RelayServerConfig,
  RelayUpdateConfig,
} from './types.js';

export const defaultAdmin = {
  host: '127.0.0.1',
  port: 3764,
  mcpPath: '/mcp',
};

export function normalizeMode(mode: unknown): RelayMode {
  if (mode === 'posthog-cli') return 'posthog-cli';
  return 'generic-cached';
}

export function normalizeConfig(raw: unknown): RelayConfig {
  if (!raw || typeof raw !== 'object') {
    throw new Error('Config must be a JSON object');
  }
  const input = raw as Partial<RelayConfig>;
  const servers = Array.isArray(input.servers) ? input.servers.map(normalizeServer) : [];
  return {
    admin: {
      host: input.admin?.host || defaultAdmin.host,
      port: Number(input.admin?.port || defaultAdmin.port),
      mcpPath: input.admin?.mcpPath || defaultAdmin.mcpPath,
    },
    updates: normalizeUpdates(input.updates),
    servers,
  };
}

function normalizeUpdates(input: unknown): RelayUpdateConfig {
  if (input === undefined) {
    return {
      autoUpgrade: false,
      checkIntervalMs: 24 * 60 * 60 * 1000,
    };
  }
  if (!input || typeof input !== 'object') throw new Error('updates config must be an object');
  const updates = input as RelayUpdateConfig;
  return {
    autoUpgrade: updates.autoUpgrade === true,
    checkIntervalMs: normalizePositiveMs(updates.checkIntervalMs, 24 * 60 * 60 * 1000),
    packageManager: normalizePackageManager(updates.packageManager),
    registryUrl: typeof updates.registryUrl === 'string' && updates.registryUrl ? updates.registryUrl : undefined,
  };
}

function normalizePackageManager(value: unknown) {
  return value === 'pnpm' || value === 'npm' || value === 'yarn' || value === 'bun' ? value : undefined;
}

export function normalizeServer(input: RelayServerConfig): RelayServerConfig {
  if (!input || typeof input !== 'object') throw new Error('Server config must be an object');
  if (!input.id || !/^[a-zA-Z0-9_-]+$/.test(input.id)) {
    throw new Error('Server id is required and must contain only letters, numbers, _ or -');
  }
  if (!input.remote || input.remote.type !== 'streamable_http') {
    throw new Error(`Server ${input.id} must use remote.type "streamable_http"`);
  }
  if (!input.remote.url) throw new Error(`Server ${input.id} remote.url is required`);
  new URL(input.remote.url);
  return {
    id: input.id,
    name: input.name || input.id,
    description: input.description || '',
    category: input.category || '',
    enabled: input.enabled !== false,
    mode: normalizeMode(input.mode),
    remote: {
      type: 'streamable_http',
      url: input.remote.url,
      headers: input.remote.headers || {},
    },
    envFile: input.envFile,
    cache: {
      toolsTtlMs: Number(input.cache?.toolsTtlMs || 15 * 60 * 1000),
      autoRefreshMs: normalizeAutoRefreshMs(input.cache?.autoRefreshMs, input.cache?.toolsTtlMs),
    },
    menu: normalizeMenu(input.menu),
  };
}

function normalizeAutoRefreshMs(value: unknown, toolsTtlMs: unknown) {
  if (value === false || value === 0) return 0;
  const fallback = Number(toolsTtlMs || 15 * 60 * 1000);
  return normalizePositiveMs(value, fallback);
}

function normalizePositiveMs(value: unknown, fallback: number) {
  const parsed = Number(value || fallback);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : fallback;
}

function normalizeMenu(input: unknown): RelayMenuConfig | undefined {
  if (input === undefined) return undefined;
  if (!input || typeof input !== 'object') throw new Error('menu config must be an object');
  const menu = input as RelayMenuConfig;
  if (menu.statusUrl) assertLocalHttpUrl(menu.statusUrl, 'menu.statusUrl');
  const actions = Array.isArray(menu.actions) ? menu.actions.map(normalizeMenuAction) : undefined;
  return {
    statusUrl: menu.statusUrl,
    ttlMs: Number(menu.ttlMs || 15_000),
    actions,
  };
}

function normalizeMenuAction(input: RelayMenuAction): RelayMenuAction {
  if (!input || typeof input !== 'object') throw new Error('menu.actions entries must be objects');
  if (!input.id || !/^[a-zA-Z0-9_-]+$/.test(input.id)) {
    throw new Error('menu action id is required and must contain only letters, numbers, _ or -');
  }
  if (!input.label || typeof input.label !== 'string') {
    throw new Error(`menu action ${input.id} label is required`);
  }
  const action: RelayMenuAction = {
    id: input.id,
    label: input.label,
    systemImage: input.systemImage,
    confirm: input.confirm === true,
  };
  if (input.tool) action.tool = input.tool;
  if (input.args && typeof input.args === 'object' && !Array.isArray(input.args)) {
    action.args = input.args;
  }
  if (input.view && typeof input.view === 'object' && !Array.isArray(input.view)) {
    action.view = input.view;
  }
  if (input.input && typeof input.input === 'object' && !Array.isArray(input.input)) {
    action.input = input.input;
  }
  if (input.url) {
    if (input.method) assertLocalHttpUrl(input.url, `menu action ${input.id} url`);
    else assertLocalDisplayUrl(input.url, `menu action ${input.id} url`);
    action.url = input.url;
  }
  if (input.method) {
    action.method = normalizeMethod(input.method);
    if (!action.url) throw new Error(`menu action ${input.id} method requires url`);
  }
  if (action.tool && action.url) {
    throw new Error(`menu action ${input.id} must use only one of tool or url`);
  }
  if (!action.tool && !action.url && !action.view) {
    throw new Error(`menu action ${input.id} requires tool, url, or view`);
  }
  return action;
}

function normalizeMethod(method: unknown): RelayMenuActionMethod {
  if (method === 'GET' || method === 'POST' || method === 'PUT' || method === 'PATCH' || method === 'DELETE') {
    return method;
  }
  throw new Error('menu action method must be GET, POST, PUT, PATCH, or DELETE');
}

function assertLocalDisplayUrl(raw: string, label: string) {
  const url = new URL(raw);
  if (url.protocol === 'file:') return;
  assertLocalHttpUrl(raw, label);
}

function assertLocalHttpUrl(raw: string, label: string) {
  const url = new URL(raw);
  if (url.protocol !== 'http:') throw new Error(`${label} must use http://127.0.0.1, http://localhost, or file:`);
  if (url.hostname !== '127.0.0.1' && url.hostname !== 'localhost') {
    throw new Error(`${label} must target 127.0.0.1 or localhost`);
  }
}

export async function readConfig(path = defaultConfigPath()): Promise<RelayConfig> {
  const raw = await readFile(path, 'utf8');
  return normalizeConfig(JSON.parse(raw));
}

export async function writeConfig(config: RelayConfig, path = defaultConfigPath()) {
  const normalized = normalizeConfig(config);
  await mkdir(dirname(path), { recursive: true });
  const tmp = `${path}.${process.pid}.tmp`;
  await writeFile(tmp, `${JSON.stringify(normalized, null, 2)}\n`);
  await rename(tmp, path);
}

export async function ensureDefaultConfig(path = defaultConfigPath()) {
  try {
    await stat(path);
  } catch {
    await writeConfig(
      {
        admin: defaultAdmin,
        servers: [],
      },
      path,
    );
  }
}

export function toolsCachePath(serverId: string) {
  return join(stateDir(), `${serverId}.tools.json`);
}
