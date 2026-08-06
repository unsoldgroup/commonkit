import { readFile, stat } from 'node:fs/promises';

export async function loadEnvFile(path?: string): Promise<Record<string, string>> {
  if (!path) return {};
  const info = await stat(path).catch(() => undefined);
  if (!info) return {};
  if ((info.mode & 0o077) !== 0) {
    throw new Error(`Env file must not be readable by group/other: ${path}`);
  }
  const content = await readFile(path, 'utf8');
  const env: Record<string, string> = {};
  for (const rawLine of content.split(/\r?\n/)) {
    const line = rawLine.trim();
    if (!line || line.startsWith('#')) continue;
    const eq = line.indexOf('=');
    if (eq < 1) continue;
    const key = line.slice(0, eq).trim();
    let value = line.slice(eq + 1).trim();
    if (
      (value.startsWith('"') && value.endsWith('"')) ||
      (value.startsWith("'") && value.endsWith("'"))
    ) {
      value = value.slice(1, -1);
    }
    env[key] = value;
  }
  return env;
}
