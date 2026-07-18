import { randomUUID } from 'node:crypto';
import { mkdir, readFile, rename, writeFile } from 'node:fs/promises';
import { dirname } from 'node:path';
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StreamableHTTPClientTransport } from '@modelcontextprotocol/sdk/client/streamableHttp.js';
import { Server } from '@modelcontextprotocol/sdk/server/index.js';
import {
  CallToolRequestSchema,
  ErrorCode,
  ListToolsRequestSchema,
  McpError,
} from '@modelcontextprotocol/sdk/types.js';
import { loadEnvFile } from './env-file.js';
import { normalizeConfig, normalizeServer, toolsCachePath, writeConfig } from './config.js';
import { RelayLogger, errorFields } from './logger.js';
import type {
  RelayConfig,
  RelayActionInputField,
  RelayMenuAction,
  RelayMenuState,
  RelayMenuStatus,
  RelayServerConfig,
  RelayServerStatus,
  RelayStatus,
  RelayUpdateStatus,
} from './types.js';

interface ToolDefinition {
  name: string;
  description?: string;
  inputSchema?: unknown;
  [key: string]: unknown;
}

interface RelayState {
  config: RelayServerConfig;
  client?: Client;
  connecting?: Promise<Client>;
  autoRefreshTimer?: ReturnType<typeof setTimeout>;
  nextAutoRefreshAt: number;
  cachedTools: ToolDefinition[];
  cachedAt: number;
  lastRefreshAttemptAt: number;
  lastRefreshError: string;
  menuStatus?: RelayMenuStatus;
  menuCachedAt: number;
  menuLastError: string;
}

export class RelayManager {
  private readonly states = new Map<string, RelayState>();
  private config: RelayConfig;
  private notifyListChanged: () => Promise<void> = async () => {};
  readonly startedAt = Date.now();

  constructor(
    config: RelayConfig,
    private readonly configPath: string,
    private readonly logger = new RelayLogger(),
  ) {
    this.config = normalizeConfig(config);
    for (const server of this.config.servers) {
      this.states.set(server.id, {
        config: server,
        nextAutoRefreshAt: 0,
        cachedTools: [],
        cachedAt: 0,
        lastRefreshAttemptAt: 0,
        lastRefreshError: '',
        menuCachedAt: 0,
        menuLastError: '',
      });
    }
  }

  setToolListChangedNotifier(fn: () => Promise<void>) {
    this.notifyListChanged = fn;
  }

  async start() {
    await Promise.all([...this.states.values()].map((state) => this.loadCache(state)));
    for (const state of this.states.values()) {
      if (state.config.enabled) void this.autoRefreshState(state, true);
    }
    this.logger.info('relay_started', {
      configPath: this.configPath,
      servers: this.config.servers.length,
      enabledServers: this.config.servers.filter((server) => server.enabled !== false).length,
    });
  }

  stop() {
    for (const state of this.states.values()) {
      this.clearAutoRefresh(state);
      this.resetClient(state);
    }
  }

  buildMcpServer() {
    const server = new Server(
      { name: 'mcp-local-relay', version: '0.1.0' },
      {
        capabilities: { tools: { listChanged: true } },
        instructions:
          'Persistent local MCP relay. Use relay_* tools to hot-add, refresh, enable, or inspect upstream MCP servers.',
      },
    );
    server.setRequestHandler(ListToolsRequestSchema, async () => ({ tools: this.listTools() }));
    server.setRequestHandler(CallToolRequestSchema, async (request) => {
      return await this.callTool(request.params.name, request.params.arguments || {});
    });
    return server;
  }

  status(sessions = 0, updates?: RelayUpdateStatus): RelayStatus {
    const servers = [...this.states.values()].map((state): RelayServerStatus => ({
      id: state.config.id,
      name: state.config.name || state.config.id,
      description: state.config.description || '',
      category: state.config.category || '',
      enabled: state.config.enabled !== false,
      mode: state.config.mode || 'generic-cached',
      remoteUrl: state.config.remote.url,
      connected: Boolean(state.client),
      cachedTools: state.cachedTools.length,
      cachedAt: state.cachedAt,
      autoRefreshMs: state.config.cache?.autoRefreshMs || 0,
      nextAutoRefreshAt: state.nextAutoRefreshAt,
      lastRefreshAttemptAt: state.lastRefreshAttemptAt,
      lastRefreshError: state.lastRefreshError,
    }));
    return {
      ok: servers.every((server) => !server.enabled || !server.lastRefreshError),
      name: 'mcp-local-relay',
      uptimeMs: Date.now() - this.startedAt,
      sessions,
      servers,
      updates,
      events: this.logger.recent(50),
    };
  }

  listTools(): ToolDefinition[] {
    const tools = [...managementTools];
    for (const state of this.states.values()) {
      if (state.config.enabled === false) continue;
      void this.ensureToolsFresh(state);
      for (const tool of state.cachedTools) {
        tools.push(this.localizeTool(state.config.id, tool));
      }
    }
    return tools;
  }

  async callTool(name: string, args: unknown) {
    if (name.startsWith('relay_')) return await this.callManagementTool(name, args);
    const parsed = parseLocalToolName(name);
    if (!parsed) {
      throw new McpError(ErrorCode.InvalidParams, `Unknown relay tool: ${name}`);
    }
    const state = this.requireState(parsed.serverId);
    if (state.config.enabled === false) {
      throw new McpError(ErrorCode.InvalidParams, `Relay server disabled: ${parsed.serverId}`);
    }
    await this.ensureToolsFresh(state, true);
    const exists = state.cachedTools.some((tool) => tool.name === parsed.toolName);
    if (!exists) throw new McpError(ErrorCode.InvalidParams, `Unknown upstream tool: ${parsed.toolName}`);
    const client = await this.getClient(state);
    try {
      return await client.callTool(
        { name: parsed.toolName, arguments: args as Record<string, unknown> },
        undefined,
        { timeout: 120000 },
      );
    } catch (err) {
      this.logger.error('upstream_tool_call_failed', {
        serverId: state.config.id,
        toolName: parsed.toolName,
        ...errorFields(err),
      });
      this.resetClient(state);
      throw err;
    }
  }

  async menuStatuses() {
    return await Promise.all([...this.states.values()].map((state) => this.menuStatusForState(state)));
  }

  async menuStatus(id: string) {
    return await this.menuStatusForState(this.requireState(id));
  }

  async callMenuAction(id: string, actionId: string, input: unknown) {
    const state = this.requireState(id);
    const body = normalizeActionRequest(input);
    const menu = await this.menuStatusForState(state, true);
    const action = menu.actions.find((item) => item.id === actionId);
    if (!action) {
      throw new McpError(ErrorCode.InvalidParams, `Unknown menu action: ${id}/${actionId}`);
    }
    if (action.confirm && body.confirm !== true) {
      throw new McpError(ErrorCode.InvalidParams, `Menu action requires { "confirm": true }: ${id}/${actionId}`);
    }
    if (action.tool) {
      return await this.callUpstreamActionTool(state, action, body.args);
    }
    if (action.method && action.url) {
      return await callLocalHttpAction(action, body.args);
    }
    if (action.url) {
      return { ok: true, action: 'open_url', url: action.url };
    }
    throw new McpError(ErrorCode.InvalidParams, `Menu action is not executable: ${id}/${actionId}`);
  }

  async addServer(input: unknown) {
    const server = normalizeServer(input as RelayServerConfig);
    if (this.states.has(server.id)) {
      throw new McpError(ErrorCode.InvalidParams, `Relay server already exists: ${server.id}`);
    }
    const state: RelayState = {
      config: server,
      nextAutoRefreshAt: 0,
      cachedTools: [],
      cachedAt: 0,
      lastRefreshAttemptAt: 0,
      lastRefreshError: '',
      menuCachedAt: 0,
      menuLastError: '',
    };
    await this.refreshTools(state);
    this.config.servers.push(server);
    await writeConfig(this.config, this.configPath);
    this.states.set(server.id, state);
    this.scheduleAutoRefresh(state);
    await this.notifyListChanged();
    return { content: textJson({ ok: true, id: server.id, cachedTools: state.cachedTools.length }) };
  }

  async updateServer(input: unknown) {
    const server = normalizeServer(input as RelayServerConfig);
    const existing = this.requireState(server.id);
    this.resetClient(existing);
    this.clearAutoRefresh(existing);
    existing.config = server;
    existing.cachedTools = [];
    existing.cachedAt = 0;
    existing.menuStatus = undefined;
    existing.menuCachedAt = 0;
    existing.menuLastError = '';
    await this.refreshTools(existing);
    this.config.servers = this.config.servers.map((item) => (item.id === server.id ? server : item));
    await writeConfig(this.config, this.configPath);
    this.scheduleAutoRefresh(existing);
    await this.notifyListChanged();
    return { content: textJson({ ok: true, id: server.id, cachedTools: existing.cachedTools.length }) };
  }

  async removeServer(input: unknown) {
    const id = readId(input);
    const state = this.requireState(id);
    this.clearAutoRefresh(state);
    this.resetClient(state);
    this.states.delete(id);
    this.config.servers = this.config.servers.filter((server) => server.id !== id);
    await writeConfig(this.config, this.configPath);
    await this.notifyListChanged();
    return { content: textJson({ ok: true, id }) };
  }

  async setEnabled(input: unknown, enabled: boolean) {
    const id = readId(input);
    const state = this.requireState(id);
    state.config.enabled = enabled;
    this.config.servers = this.config.servers.map((server) =>
      server.id === id ? { ...server, enabled } : server,
    );
    if (enabled) await this.ensureToolsFresh(state, true);
    if (enabled) this.scheduleAutoRefresh(state);
    else {
      this.clearAutoRefresh(state);
      this.resetClient(state);
    }
    await writeConfig(this.config, this.configPath);
    await this.notifyListChanged();
    return { content: textJson({ ok: true, id, enabled }) };
  }

  async refreshServer(input: unknown) {
    const id = readId(input);
    const state = this.requireState(id);
    await this.refreshTools(state);
    await this.notifyListChanged();
    return { content: textJson({ ok: true, id, cachedTools: state.cachedTools.length }) };
  }

  async validateServer(input: unknown) {
    const server = normalizeServer(input as RelayServerConfig);
    const state: RelayState = {
      config: server,
      nextAutoRefreshAt: 0,
      cachedTools: [],
      cachedAt: 0,
      lastRefreshAttemptAt: 0,
      lastRefreshError: '',
      menuCachedAt: 0,
      menuLastError: '',
    };
    await this.refreshTools(state);
    this.resetClient(state);
    return { content: textJson({ ok: true, id: server.id, tools: state.cachedTools.length }) };
  }

  private async callManagementTool(name: string, args: unknown) {
    switch (name) {
      case 'relay_list_servers':
      case 'relay_get_status':
        return { content: textJson(this.status()) };
      case 'relay_add_server':
        return await this.addServer(args);
      case 'relay_update_server':
        return await this.updateServer(args);
      case 'relay_remove_server':
        return await this.removeServer(args);
      case 'relay_enable_server':
        return await this.setEnabled(args, true);
      case 'relay_disable_server':
        return await this.setEnabled(args, false);
      case 'relay_refresh_tools':
        return await this.refreshServer(args);
      case 'relay_validate_server':
        return await this.validateServer(args);
      case 'relay_get_client_config':
        return {
          content: textJson({
            codex: {
              mcp_servers: {
                'mcp-local-relay': {
                  url: `http://${this.config.admin?.host || '127.0.0.1'}:${this.config.admin?.port || 3764}${this.config.admin?.mcpPath || '/mcp'}`,
                },
              },
            },
            claude: {
              mcpServers: {
                'mcp-local-relay': {
                  type: 'http',
                  url: `http://${this.config.admin?.host || '127.0.0.1'}:${this.config.admin?.port || 3764}${this.config.admin?.mcpPath || '/mcp'}`,
                },
              },
            },
          }),
        };
      default:
        throw new McpError(ErrorCode.InvalidParams, `Unknown management tool: ${name}`);
    }
  }

  private async getClient(state: RelayState) {
    if (state.client) return state.client;
    if (state.connecting) return await state.connecting;
    state.connecting = this.connectClient(state);
    try {
      return await state.connecting;
    } finally {
      state.connecting = undefined;
    }
  }

  private async connectClient(state: RelayState) {
    const env = await loadEnvFile(state.config.envFile);
    const headers = this.headersFor(state.config, env);
    this.logger.info('upstream_connect_start', {
      serverId: state.config.id,
      url: state.config.remote.url,
      mode: state.config.mode || 'generic-cached',
      hasEnvFile: Boolean(state.config.envFile),
      headerNames: Object.keys(headers),
    });
    const client = new Client({ name: `mcp-local-relay-${state.config.id}`, version: '0.1.0' }, {
      capabilities: {},
    });
    const transport = new StreamableHTTPClientTransport(new URL(state.config.remote.url), {
      requestInit: { headers },
      reconnectionOptions: {
        initialReconnectionDelay: 1000,
        maxReconnectionDelay: 10000,
        reconnectionDelayGrowFactor: 1.5,
        maxRetries: 3,
      },
    });
    transport.onerror = (err) => {
      const message = err instanceof Error ? err.message : String(err);
      if (!state.client && /AbortError|operation was aborted/i.test(message)) return;
      state.lastRefreshError = message;
      this.logger.warn('upstream_transport_error', {
        serverId: state.config.id,
        ...errorFields(err),
      });
    };
    try {
      await client.connect(transport, { timeout: 20000 });
      state.client = client;
      this.logger.info('upstream_connect_ok', { serverId: state.config.id });
      return client;
    } catch (err) {
      this.logger.error('upstream_connect_failed', {
        serverId: state.config.id,
        url: state.config.remote.url,
        ...errorFields(err),
      });
      throw err;
    }
  }

  private headersFor(config: RelayServerConfig, env: Record<string, string>) {
    const headers: Record<string, string> = { ...(config.remote.headers || {}) };
    if (config.mode === 'posthog-cli') {
      headers['x-posthog-mcp-consumer'] ||= 'mcp-local-relay';
      headers['x-posthog-mcp-mode'] ||= 'cli';
      const token = env.POSTHOG_MCP_API_TOKEN || env.POSTHOG_PERSONAL_API_KEY || env.POSTHOG_API_KEY;
      if (token && !headers.Authorization) headers.Authorization = `Bearer ${token}`;
    }
    return headers;
  }

  private async ensureToolsFresh(state: RelayState, force = false) {
    const ttl = state.config.cache?.toolsTtlMs || 15 * 60 * 1000;
    if (!force && state.cachedTools.length > 0 && Date.now() - state.cachedAt < ttl) {
      return state.cachedTools;
    }
    try {
      return await this.refreshTools(state);
    } catch (err) {
      state.lastRefreshError = err instanceof Error ? err.message : String(err);
      this.logger.warn('upstream_tools_refresh_failed_using_cache', {
        serverId: state.config.id,
        cachedTools: state.cachedTools.length,
        cachedAt: state.cachedAt,
        ...errorFields(err),
      });
      return state.cachedTools;
    }
  }

  private scheduleAutoRefresh(state: RelayState) {
    this.clearAutoRefresh(state);
    const interval = state.config.cache?.autoRefreshMs || 0;
    if (state.config.enabled === false || interval <= 0) {
      state.nextAutoRefreshAt = 0;
      return;
    }
    state.nextAutoRefreshAt = Date.now() + interval;
    state.autoRefreshTimer = setTimeout(() => {
      void this.autoRefreshState(state);
    }, interval);
    state.autoRefreshTimer.unref?.();
    this.logger.info('upstream_auto_refresh_scheduled', {
      serverId: state.config.id,
      autoRefreshMs: interval,
      nextAutoRefreshAt: state.nextAutoRefreshAt,
    });
  }

  private clearAutoRefresh(state: RelayState) {
    if (state.autoRefreshTimer) clearTimeout(state.autoRefreshTimer);
    state.autoRefreshTimer = undefined;
    state.nextAutoRefreshAt = 0;
  }

  private async autoRefreshState(state: RelayState, immediate = false) {
    if (this.states.get(state.config.id) !== state || state.config.enabled === false) return;
    const before = JSON.stringify(state.cachedTools);
    try {
      await this.ensureToolsFresh(state, true);
      if (before !== JSON.stringify(state.cachedTools)) {
        await this.notifyListChanged();
        this.logger.info('upstream_auto_refresh_tools_changed', {
          serverId: state.config.id,
          tools: state.cachedTools.length,
        });
      }
    } finally {
      if (!immediate || this.states.get(state.config.id) === state) this.scheduleAutoRefresh(state);
    }
  }

  private async refreshTools(state: RelayState) {
    state.lastRefreshAttemptAt = Date.now();
    this.logger.info('upstream_tools_refresh_start', { serverId: state.config.id });
    try {
      const client = await this.getClient(state);
      const result = await client.listTools(undefined, { timeout: 20000 });
      state.cachedTools = normalizeTools(result);
      state.cachedAt = Date.now();
      state.lastRefreshError = '';
      await this.saveCache(state);
      this.logger.info('upstream_tools_refresh_ok', {
        serverId: state.config.id,
        tools: state.cachedTools.length,
        cachedAt: state.cachedAt,
      });
      return state.cachedTools;
    } catch (err) {
      this.logger.error('upstream_tools_refresh_failed', {
        serverId: state.config.id,
        ...errorFields(err),
      });
      throw err;
    }
  }

  private async menuStatusForState(state: RelayState, force = false): Promise<RelayMenuStatus> {
    const ttl = state.config.menu?.ttlMs || 15_000;
    if (!force && state.menuStatus && Date.now() - state.menuCachedAt < ttl) {
      return state.menuStatus;
    }
    try {
      const discovered = await this.discoverMenuStatus(state);
      const status = normalizeMenuStatus(state, discovered, '');
      state.menuStatus = status;
      state.menuCachedAt = Date.now();
      state.menuLastError = '';
      return status;
    } catch (err) {
      state.menuLastError = err instanceof Error ? err.message : String(err);
      this.logger.warn('upstream_menu_status_failed', {
        serverId: state.config.id,
        ...errorFields(err),
      });
      const fallback = fallbackMenuStatus(state, state.menuLastError);
      state.menuStatus = fallback;
      state.menuCachedAt = Date.now();
      return fallback;
    }
  }

  private async discoverMenuStatus(state: RelayState) {
    if (state.config.enabled === false) return undefined;
    if (state.config.menu?.statusUrl) return await fetchMenuStatusUrl(state.config.menu.statusUrl);
    await this.ensureToolsFresh(state);
    const statusTool = findMenuStatusTool(state);
    if (!statusTool) return undefined;
    const client = await this.getClient(state);
    const result = await client.callTool({ name: statusTool, arguments: {} }, undefined, { timeout: 20_000 });
    return parseToolJson(result);
  }

  private async callUpstreamActionTool(state: RelayState, action: RelayMenuAction, requestArgs: Record<string, unknown>) {
    if (state.config.enabled === false) {
      throw new McpError(ErrorCode.InvalidParams, `Relay server disabled: ${state.config.id}`);
    }
    await this.ensureToolsFresh(state, true);
    const toolName = action.tool!;
    const exists = state.cachedTools.some((tool) => tool.name === toolName);
    if (!exists) throw new McpError(ErrorCode.InvalidParams, `Unknown upstream tool: ${toolName}`);
    const client = await this.getClient(state);
    try {
      return await client.callTool(
        { name: toolName, arguments: { ...(action.args || {}), ...requestArgs } },
        undefined,
        { timeout: 120000 },
      );
    } catch (err) {
      this.logger.error('upstream_menu_action_failed', {
        serverId: state.config.id,
        actionId: action.id,
        toolName,
        ...errorFields(err),
      });
      this.resetClient(state);
      throw err;
    }
  }

  private async loadCache(state: RelayState) {
    try {
      const parsed = JSON.parse(await readFile(toolsCachePath(state.config.id), 'utf8'));
      state.cachedTools = normalizeTools(parsed);
      state.cachedAt = Number(parsed.cachedAt || 0);
      this.logger.info('tools_cache_loaded', {
        serverId: state.config.id,
        tools: state.cachedTools.length,
        cachedAt: state.cachedAt,
      });
    } catch {
      state.cachedTools = [];
      state.cachedAt = 0;
    }
  }

  private async saveCache(state: RelayState) {
    const path = toolsCachePath(state.config.id);
    await mkdir(dirname(path), { recursive: true });
    const tmp = `${path}.${process.pid}.${randomUUID()}.tmp`;
    await writeFile(tmp, JSON.stringify({ cachedAt: state.cachedAt, tools: state.cachedTools }, null, 2));
    await rename(tmp, path);
  }

  private resetClient(state: RelayState) {
    const client = state.client;
    state.client = undefined;
    state.connecting = undefined;
    state.menuStatus = undefined;
    state.menuCachedAt = 0;
    if (client) void client.close().catch(() => {});
    if (client) this.logger.info('upstream_client_reset', { serverId: state.config.id });
  }

  private requireState(id: string) {
    const state = this.states.get(id);
    if (!state) throw new McpError(ErrorCode.InvalidParams, `Unknown relay server: ${id}`);
    return state;
  }

  private localizeTool(serverId: string, tool: ToolDefinition): ToolDefinition {
    return {
      ...tool,
      name: localToolName(serverId, tool.name),
      description: `[${serverId}] ${tool.description || tool.name}`,
    };
  }
}

function findMenuStatusTool(state: RelayState) {
  const names = new Set(state.cachedTools.map((tool) => tool.name));
  if (names.has('relay_menu_status')) return 'relay_menu_status';
  const serverSpecific = `${state.config.id}_menu_status`;
  if (names.has(serverSpecific)) return serverSpecific;
  return undefined;
}

async function fetchMenuStatusUrl(url: string) {
  assertLocalHttpActionUrl(url);
  const response = await fetch(url, { method: 'GET' });
  const text = await response.text();
  if (!response.ok) throw new Error(`menu status endpoint failed: ${response.status} ${text}`);
  return text ? JSON.parse(text) : {};
}

function parseToolJson(result: unknown) {
  const content = (result as { content?: Array<{ type?: string; text?: string }> })?.content;
  const text = Array.isArray(content)
    ? content.find((item) => item?.type === 'text' && typeof item.text === 'string')?.text
    : undefined;
  if (!text) throw new Error('menu status tool did not return text JSON');
  return JSON.parse(text);
}

function normalizeMenuStatus(state: RelayState, input: unknown, lastError: string): RelayMenuStatus {
  const fallback = fallbackMenuStatus(state, lastError);
  const raw = input && typeof input === 'object' ? (input as Partial<RelayMenuStatus>) : {};
  return {
    id: state.config.id,
    title: stringOr(raw.title, fallback.title),
    summary: stringOr(raw.summary, fallback.summary),
    state: normalizeMenuState(raw.state, fallback.state),
    detail: normalizeDetail(raw.detail, fallback.detail),
    actions: normalizeMenuActions([...(Array.isArray(raw.actions) ? raw.actions : []), ...(state.config.menu?.actions || [])]),
    cachedAt: Date.now(),
    lastError,
  };
}

function fallbackMenuStatus(state: RelayState, lastError: string): RelayMenuStatus {
  const enabled = state.config.enabled !== false;
  const stateName: RelayMenuState = !enabled
    ? 'paused'
    : state.lastRefreshError || lastError
      ? 'error'
      : state.client
        ? 'ready'
        : 'unknown';
  const detail = [
    `${state.cachedTools.length} cached tools`,
    state.cachedAt ? `Tools cached at ${new Date(state.cachedAt).toISOString()}` : 'Tools not cached yet',
  ];
  if (state.lastRefreshError) detail.push(`Last refresh error: ${state.lastRefreshError}`);
  if (lastError) detail.push(`Menu error: ${lastError}`);
  return {
    id: state.config.id,
    title: state.config.name || state.config.id,
    summary: enabled
      ? `${state.client ? 'Connected' : 'Not connected'} - ${state.cachedTools.length} tools`
      : 'Disabled',
    state: stateName,
    detail,
    actions: normalizeMenuActions(state.config.menu?.actions || []),
    cachedAt: Date.now(),
    lastError,
  };
}

function normalizeMenuState(value: unknown, fallback: RelayMenuState): RelayMenuState {
  return value === 'ready' ||
    value === 'running' ||
    value === 'paused' ||
    value === 'attention' ||
    value === 'error' ||
    value === 'unknown'
    ? value
    : fallback;
}

function normalizeMenuActions(actions: unknown[]): RelayMenuAction[] {
  return actions.flatMap((input) => {
    const action = input as RelayMenuAction;
    if (!action || typeof action !== 'object') return [];
    if (!action.id || !/^[a-zA-Z0-9_-]+$/.test(action.id)) return [];
    if (!action.label || typeof action.label !== 'string') return [];
    if (action.url) {
      try {
        if (action.method) {
          if (!isMenuActionMethod(action.method)) return [];
          assertLocalHttpActionUrl(action.url);
        } else {
          assertDisplayActionUrl(action.url);
        }
      } catch {
        return [];
      }
    }
    const view = normalizeActionView(action.view);
    const inputSpec = normalizeActionInput(action.input);
    if (action.tool && action.url) return [];
    if (!action.tool && !action.url && !view) return [];
    return [
      {
        id: action.id,
        label: action.label,
        systemImage: typeof action.systemImage === 'string' ? action.systemImage : undefined,
        confirm: action.confirm === true,
        tool: typeof action.tool === 'string' ? action.tool : undefined,
        args: action.args && typeof action.args === 'object' && !Array.isArray(action.args) ? action.args : undefined,
        url: typeof action.url === 'string' ? action.url : undefined,
        method: action.method,
        view,
        input: inputSpec,
      },
    ];
  });
}

function normalizeActionView(view: unknown): RelayMenuAction['view'] {
  if (!view || typeof view !== 'object' || Array.isArray(view)) return undefined;
  const raw = view as NonNullable<RelayMenuAction['view']>;
  if (!raw.type || !raw.title) return undefined;
  const footerActions = Array.isArray(raw.footerActions)
    ? raw.footerActions.flatMap((action) => {
        if (!action?.id || !action.label || !action.url) return [];
        try {
          assertDisplayActionUrl(action.url);
        } catch {
          return [];
        }
        return [{
          id: String(action.id),
          label: String(action.label),
          systemImage: typeof action.systemImage === 'string' ? action.systemImage : undefined,
          url: String(action.url),
        }];
      })
    : undefined;
  return {
    type: raw.type,
    title: raw.title,
    summary: typeof raw.summary === 'string' ? raw.summary : undefined,
    refreshSeconds: typeof raw.refreshSeconds === 'number' ? raw.refreshSeconds : undefined,
    density: raw.density,
    columns: Array.isArray(raw.columns) ? raw.columns : undefined,
    rows: Array.isArray(raw.rows) ? raw.rows.slice(0, 20) : undefined,
    footerActions,
  };
}

function normalizeActionInput(input: unknown): RelayMenuAction['input'] {
  if (!input || typeof input !== 'object' || Array.isArray(input)) return undefined;
  const raw = input as NonNullable<RelayMenuAction['input']>;
  if (!Array.isArray(raw.fields)) return undefined;
  const fields = raw.fields.flatMap((field) => {
    if (!field?.id || !field.label) return [];
    const type: RelayActionInputField['type'] = field.type === 'number' || field.type === 'boolean' ? field.type : 'string';
    return [{
      id: String(field.id),
      label: String(field.label),
      type,
      placeholder: typeof field.placeholder === 'string' ? field.placeholder : undefined,
      default: typeof field.default === 'string' || typeof field.default === 'number' || typeof field.default === 'boolean'
        ? field.default
        : undefined,
      required: field.required === true,
      multiline: field.multiline === true,
    }];
  });
  if (fields.length === 0) return undefined;
  return {
    title: typeof raw.title === 'string' ? raw.title : undefined,
    submitLabel: typeof raw.submitLabel === 'string' ? raw.submitLabel : undefined,
    fields,
  };
}

function isMenuActionMethod(value: unknown): value is RelayMenuAction['method'] {
  return value === 'GET' || value === 'POST' || value === 'PUT' || value === 'PATCH' || value === 'DELETE';
}

function normalizeDetail(value: unknown, fallback: string[]) {
  return Array.isArray(value) ? value.filter((item): item is string => typeof item === 'string') : fallback;
}

function stringOr(value: unknown, fallback: string) {
  return typeof value === 'string' && value.length > 0 ? value : fallback;
}

function normalizeActionRequest(input: unknown) {
  const body = input && typeof input === 'object' ? (input as { confirm?: unknown; args?: unknown }) : {};
  return {
    confirm: body.confirm === true,
    args: body.args && typeof body.args === 'object' && !Array.isArray(body.args) ? (body.args as Record<string, unknown>) : {},
  };
}

async function callLocalHttpAction(action: RelayMenuAction, args: Record<string, unknown>) {
  assertLocalHttpActionUrl(action.url!);
  const method = action.method || 'POST';
  const hasBody = method !== 'GET' && method !== 'DELETE';
  const response = await fetch(action.url!, {
    method,
    headers: hasBody ? { 'content-type': 'application/json' } : undefined,
    body: hasBody ? JSON.stringify(args) : undefined,
  });
  const text = await response.text();
  let body: unknown = text;
  try {
    body = text ? JSON.parse(text) : undefined;
  } catch {
    body = text;
  }
  return { ok: response.ok, status: response.status, body };
}

function assertDisplayActionUrl(raw: string) {
  const url = new URL(raw);
  if (url.protocol === 'file:') return;
  assertLocalHttpActionUrl(raw);
}

function assertLocalHttpActionUrl(raw: string) {
  const url = new URL(raw);
  if (url.protocol !== 'http:' || (url.hostname !== '127.0.0.1' && url.hostname !== 'localhost')) {
    throw new Error('Menu HTTP actions must target http://127.0.0.1 or http://localhost');
  }
}

function normalizeTools(value: unknown): ToolDefinition[] {
  const input = value as { tools?: ToolDefinition[] };
  return Array.isArray(input?.tools)
    ? input.tools.filter((tool) => tool && typeof tool.name === 'string')
    : [];
}

function localToolName(serverId: string, toolName: string) {
  return `${serverId}__${toolName.replace(/[^a-zA-Z0-9_-]/g, '_')}`;
}

function parseLocalToolName(name: string) {
  const index = name.indexOf('__');
  if (index < 1) return undefined;
  return { serverId: name.slice(0, index), toolName: name.slice(index + 2) };
}

function readId(input: unknown) {
  const obj = input as { id?: unknown };
  if (!obj || typeof obj.id !== 'string') {
    throw new McpError(ErrorCode.InvalidParams, 'Expected { "id": string }');
  }
  return obj.id;
}

function textJson(value: unknown) {
  return [{ type: 'text' as const, text: JSON.stringify(value, null, 2) }];
}

const idOnlySchema = {
  type: 'object',
  properties: { id: { type: 'string' } },
  required: ['id'],
};

const serverSchema = {
  type: 'object',
  properties: {
    id: { type: 'string' },
    name: { type: 'string' },
    description: { type: 'string' },
    category: { type: 'string' },
    enabled: { type: 'boolean' },
    mode: { type: 'string', enum: ['generic-cached', 'posthog-cli'] },
    remote: {
      type: 'object',
      properties: {
        type: { type: 'string', enum: ['streamable_http'] },
        url: { type: 'string' },
        headers: { type: 'object', additionalProperties: { type: 'string' } },
      },
      required: ['type', 'url'],
    },
    envFile: { type: 'string' },
    cache: {
      type: 'object',
      properties: { toolsTtlMs: { type: 'number' } },
    },
    menu: {
      type: 'object',
      properties: {
        statusUrl: { type: 'string' },
        ttlMs: { type: 'number' },
        actions: {
          type: 'array',
          items: {
            type: 'object',
            properties: {
              id: { type: 'string' },
              label: { type: 'string' },
              systemImage: { type: 'string' },
              confirm: { type: 'boolean' },
              tool: { type: 'string' },
              args: { type: 'object' },
              url: { type: 'string' },
              method: { type: 'string', enum: ['GET', 'POST', 'PUT', 'PATCH', 'DELETE'] },
              view: { type: 'object' },
              input: { type: 'object' },
            },
            required: ['id', 'label'],
          },
        },
      },
    },
  },
  required: ['id', 'remote'],
};

const managementTools: ToolDefinition[] = [
  {
    name: 'relay_list_servers',
    description: 'List configured upstream MCP servers and their health.',
    inputSchema: { type: 'object', properties: {} },
  },
  {
    name: 'relay_get_status',
    description: 'Return aggregate relay status including cached tool counts and last refresh errors.',
    inputSchema: { type: 'object', properties: {} },
  },
  {
    name: 'relay_add_server',
    description: 'Hot-add an upstream MCP server, validate it, cache tools, persist config, and emit tools/list_changed.',
    inputSchema: serverSchema,
  },
  {
    name: 'relay_update_server',
    description: 'Replace an existing upstream MCP server config after validation.',
    inputSchema: serverSchema,
  },
  {
    name: 'relay_remove_server',
    description: 'Remove an upstream MCP server and emit tools/list_changed.',
    inputSchema: idOnlySchema,
  },
  {
    name: 'relay_enable_server',
    description: 'Enable an upstream MCP server and refresh its tool cache.',
    inputSchema: idOnlySchema,
  },
  {
    name: 'relay_disable_server',
    description: 'Disable an upstream MCP server and hide its tools.',
    inputSchema: idOnlySchema,
  },
  {
    name: 'relay_refresh_tools',
    description: 'Refresh cached tools for one upstream MCP server.',
    inputSchema: idOnlySchema,
  },
  {
    name: 'relay_validate_server',
    description: 'Validate an upstream MCP server config without persisting it.',
    inputSchema: serverSchema,
  },
  {
    name: 'relay_get_client_config',
    description: 'Print Codex and Claude MCP config snippets for this local relay.',
    inputSchema: { type: 'object', properties: {} },
  },
];
