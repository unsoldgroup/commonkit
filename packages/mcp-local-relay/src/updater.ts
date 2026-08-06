import { execFile } from 'node:child_process';
import { readFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { RelayLogger, errorFields } from './logger.js';
import { packageName } from './paths.js';
import type { RelayUpdateConfig, RelayUpdateStatus } from './types.js';

const execFileAsync = promisify(execFile);

export class UpdateManager {
  private timer?: ReturnType<typeof setTimeout>;
  private checking = false;
  private readonly state: RelayUpdateStatus = {
    enabled: false,
    checkIntervalMs: 0,
    currentVersion: '',
    latestVersion: '',
    lastCheckAt: 0,
    nextCheckAt: 0,
    lastUpdateAt: 0,
    lastError: '',
  };

  constructor(
    private readonly config: RelayUpdateConfig,
    private readonly logger = new RelayLogger(),
    private readonly afterUpgrade: () => Promise<void> = async () => {},
  ) {
    this.state.enabled = config.autoUpgrade === true;
    this.state.checkIntervalMs = config.checkIntervalMs || 24 * 60 * 60 * 1000;
  }

  async start() {
    if (!this.state.enabled) return;
    await this.checkOnce();
    this.schedule();
  }

  stop() {
    if (this.timer) clearTimeout(this.timer);
    this.timer = undefined;
    this.state.nextCheckAt = 0;
  }

  status() {
    return { ...this.state };
  }

  async checkOnce() {
    if (this.checking) return this.status();
    this.checking = true;
    this.state.lastCheckAt = Date.now();
    try {
      const currentVersion = await readInstalledPackageVersion();
      const latestVersion = await fetchLatestPackageVersion(this.config.registryUrl);
      this.state.currentVersion = currentVersion;
      this.state.latestVersion = latestVersion;
      this.state.lastError = '';
      this.logger.info('update_check_ok', { currentVersion, latestVersion });
      if (isSemverGreater(latestVersion, currentVersion)) {
        await upgradePackage(this.config.packageManager);
        this.state.lastUpdateAt = Date.now();
        this.logger.info('update_upgrade_ok', { fromVersion: currentVersion, toVersion: latestVersion });
        await this.afterUpgrade();
      }
      return this.status();
    } catch (err) {
      this.state.lastError = err instanceof Error ? err.message : String(err);
      this.logger.error('update_check_failed', errorFields(err));
      return this.status();
    } finally {
      this.checking = false;
    }
  }

  private schedule() {
    this.stop();
    if (!this.state.enabled || this.state.checkIntervalMs <= 0) return;
    this.state.nextCheckAt = Date.now() + this.state.checkIntervalMs;
    this.timer = setTimeout(() => {
      void this.checkOnce().finally(() => this.schedule());
    }, this.state.checkIntervalMs);
    this.timer.unref?.();
    this.logger.info('update_check_scheduled', {
      checkIntervalMs: this.state.checkIntervalMs,
      nextCheckAt: this.state.nextCheckAt,
    });
  }
}

export function resolveUpgradeCommand(
  packageManager?: string,
  userAgent = process.env.npm_config_user_agent,
  cliPath = process.argv[1] || '',
) {
  const manager = packageManager || userAgent?.split('/')[0] || detectPackageManagerFromPath(cliPath) || 'npm';
  if (manager === 'pnpm') return { command: 'pnpm', args: ['add', '-g', `${packageName}@latest`] };
  if (manager === 'yarn') return { command: 'yarn', args: ['global', 'add', `${packageName}@latest`] };
  if (manager === 'bun') return { command: 'bun', args: ['add', '-g', `${packageName}@latest`] };
  return { command: 'npm', args: ['install', '-g', `${packageName}@latest`] };
}

export async function upgradePackage(packageManager?: string) {
  const { command, args } = resolveUpgradeCommand(packageManager);
  const result = await execFileAsync(command, args, { maxBuffer: 10 * 1024 * 1024 });
  if (result.stdout.trim()) process.stdout.write(`${result.stdout.trim()}\n`);
  if (result.stderr.trim()) process.stderr.write(`${result.stderr.trim()}\n`);
}

export async function readInstalledPackageVersion(root = join(dirname(fileURLToPath(import.meta.url)), '..', '..')) {
  const parsed = JSON.parse(await readFile(join(root, 'package.json'), 'utf8')) as { version?: unknown };
  if (typeof parsed.version !== 'string' || !parsed.version) throw new Error('package.json version is missing');
  return parsed.version;
}

export async function fetchLatestPackageVersion(registryUrl = 'https://registry.npmjs.org') {
  const base = registryUrl.replace(/\/+$/, '');
  const response = await fetch(`${base}/${encodeURIComponent(packageName)}/latest`, {
    signal: AbortSignal.timeout(20_000),
  });
  const text = await response.text();
  if (!response.ok) throw new Error(`version check failed: ${response.status} ${text}`);
  const parsed = JSON.parse(text) as { version?: unknown };
  if (typeof parsed.version !== 'string' || !parsed.version) throw new Error('registry response missing version');
  return parsed.version;
}

export function isSemverGreater(candidate: string, current: string) {
  const left = parseSemver(candidate);
  const right = parseSemver(current);
  for (let i = 0; i < 3; i += 1) {
    if (left[i] > right[i]) return true;
    if (left[i] < right[i]) return false;
  }
  return false;
}

function parseSemver(value: string) {
  const [major = 0, minor = 0, patch = 0] = value.split(/[.-]/).map((part) => Number(part));
  return [major || 0, minor || 0, patch || 0];
}

function detectPackageManagerFromPath(cliPath: string) {
  return cliPath.includes('/pnpm/') || cliPath.includes('/.pnpm/') || cliPath.includes('/Library/pnpm/')
    ? 'pnpm'
    : undefined;
}
