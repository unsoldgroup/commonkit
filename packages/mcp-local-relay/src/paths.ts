import { homedir } from 'node:os';
import { join } from 'node:path';

export const packageName = 'mcp-local-relay';
export const serviceName = 'mcp-local-relay';
export const launchAgentLabel = 'com.unsoldgroup.mcp-local-relay';

export function configDir() {
  return join(homedir(), '.config', serviceName);
}

export function stateDir() {
  return join(homedir(), '.local', 'state', serviceName);
}

export function defaultConfigPath() {
  return join(configDir(), 'config.json');
}

export function defaultStatePath(name: string) {
  return join(stateDir(), name);
}

export function launchAgentPath() {
  return join(homedir(), 'Library', 'LaunchAgents', `${launchAgentLabel}.plist`);
}

export function defaultNodePath() {
  return process.execPath;
}
