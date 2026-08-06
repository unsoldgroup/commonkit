export type RelayMode = 'generic-cached' | 'posthog-cli';

export interface RelayListenConfig {
  host?: string;
  port?: number;
  path?: string;
}

export interface RelayRemoteConfig {
  type: 'streamable_http';
  url: string;
  headers?: Record<string, string>;
}

export interface RelayCacheConfig {
  toolsTtlMs?: number;
  autoRefreshMs?: number;
}

export type RelayMenuState = 'ready' | 'running' | 'paused' | 'attention' | 'error' | 'unknown';
export type RelayMenuActionMethod = 'GET' | 'POST' | 'PUT' | 'PATCH' | 'DELETE';
export type RelayViewStatus = 'success' | 'running' | 'warning' | 'error' | 'paused' | 'neutral';

export interface RelayActionViewColumn {
  id: string;
  label: string;
  kind?: 'status' | 'text';
}

export interface RelayActionViewFooterAction {
  id: string;
  label: string;
  systemImage?: string;
  url: string;
}

export interface RelayActionView {
  type: 'table' | 'list' | 'summary';
  title: string;
  summary?: string;
  refreshSeconds?: number;
  density?: 'compact' | 'normal';
  columns?: RelayActionViewColumn[];
  rows?: Array<Record<string, string | number | boolean | null>>;
  footerActions?: RelayActionViewFooterAction[];
}

export interface RelayActionInputField {
  id: string;
  label: string;
  type?: 'string' | 'number' | 'boolean';
  placeholder?: string;
  default?: string | number | boolean;
  required?: boolean;
  multiline?: boolean;
}

export interface RelayActionInput {
  title?: string;
  submitLabel?: string;
  fields: RelayActionInputField[];
}

export interface RelayMenuAction {
  id: string;
  label: string;
  systemImage?: string;
  confirm?: boolean;
  tool?: string;
  args?: Record<string, unknown>;
  url?: string;
  method?: RelayMenuActionMethod;
  view?: RelayActionView;
  input?: RelayActionInput;
}

export interface RelayMenuConfig {
  statusUrl?: string;
  ttlMs?: number;
  actions?: RelayMenuAction[];
}

export interface RelayServerConfig {
  id: string;
  name?: string;
  description?: string;
  category?: string;
  enabled?: boolean;
  mode?: RelayMode;
  remote: RelayRemoteConfig;
  envFile?: string;
  cache?: RelayCacheConfig;
  menu?: RelayMenuConfig;
}

export interface RelayAdminConfig {
  host?: string;
  port?: number;
  mcpPath?: string;
}

export interface RelayUpdateConfig {
  autoUpgrade?: boolean;
  checkIntervalMs?: number;
  packageManager?: string;
  registryUrl?: string;
}

export interface RelayConfig {
  admin?: RelayAdminConfig;
  updates?: RelayUpdateConfig;
  servers: RelayServerConfig[];
}

export interface CachedToolsFile {
  cachedAt: number;
  tools: unknown[];
}

export interface RelayServerStatus {
  id: string;
  name: string;
  description: string;
  category: string;
  enabled: boolean;
  mode: RelayMode;
  remoteUrl: string;
  connected: boolean;
  cachedTools: number;
  cachedAt: number;
  autoRefreshMs: number;
  nextAutoRefreshAt: number;
  lastRefreshAttemptAt: number;
  lastRefreshError: string;
}

export interface RelayStatus {
  ok: boolean;
  name: string;
  uptimeMs: number;
  sessions: number;
  servers: RelayServerStatus[];
  updates?: RelayUpdateStatus;
  events?: Array<Record<string, unknown>>;
}

export interface RelayUpdateStatus {
  enabled: boolean;
  checkIntervalMs: number;
  currentVersion: string;
  latestVersion: string;
  lastCheckAt: number;
  nextCheckAt: number;
  lastUpdateAt: number;
  lastError: string;
}

export interface RelayMenuStatus {
  id: string;
  title: string;
  summary: string;
  state: RelayMenuState;
  detail: string[];
  actions: RelayMenuAction[];
  cachedAt: number;
  lastError: string;
}
