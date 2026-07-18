import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { dirname } from 'node:path';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { defaultConfigPath, defaultNodePath, launchAgentLabel, launchAgentPath, stateDir } from './paths.js';

const execFileAsync = promisify(execFile);

export function renderLaunchAgent(options?: { nodePath?: string; configPath?: string; cliPath?: string }) {
  const nodePath = options?.nodePath || defaultNodePath();
  const configPath = options?.configPath || defaultConfigPath();
  const cliPath = options?.cliPath || process.argv[1];
  const outLog = `${stateDir()}/relay.out.log`;
  const errLog = `${stateDir()}/relay.err.log`;
  return `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>${launchAgentLabel}</string>
  <key>ProgramArguments</key>
  <array>
    <string>${escapeXml(nodePath)}</string>
    <string>${escapeXml(cliPath)}</string>
    <string>serve</string>
    <string>--config</string>
    <string>${escapeXml(configPath)}</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>StandardOutPath</key>
  <string>${escapeXml(outLog)}</string>
  <key>StandardErrorPath</key>
  <string>${escapeXml(errLog)}</string>
</dict>
</plist>
`;
}

export async function installLaunchAgent(options?: { nodePath?: string; configPath?: string; cliPath?: string }) {
  await mkdir(dirname(launchAgentPath()), { recursive: true });
  await mkdir(stateDir(), { recursive: true });
  await writeFile(launchAgentPath(), renderLaunchAgent(options));
}

export async function launchctl(action: 'bootstrap' | 'bootout' | 'kickstart' | 'print') {
  const uid = process.getuid?.() || Number(process.env.UID);
  const domain = `gui/${uid}`;
  if (action === 'bootstrap') {
    return await execFileAsync('launchctl', ['bootstrap', domain, launchAgentPath()]);
  }
  if (action === 'bootout') {
    return await execFileAsync('launchctl', ['bootout', domain, launchAgentPath()]);
  }
  if (action === 'kickstart') {
    return await execFileAsync('launchctl', ['kickstart', '-k', `${domain}/${launchAgentLabel}`]);
  }
  return await execFileAsync('launchctl', ['print', `${domain}/${launchAgentLabel}`]);
}

export async function readLogs(lines = 80) {
  const err = await readFile(`${stateDir()}/relay.err.log`, 'utf8').catch(() => '');
  const out = await readFile(`${stateDir()}/relay.out.log`, 'utf8').catch(() => '');
  return {
    out: out.split('\n').slice(-lines).join('\n'),
    err: err.split('\n').slice(-lines).join('\n'),
  };
}

function escapeXml(value: string) {
  return value
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
    .replaceAll('"', '&quot;')
    .replaceAll("'", '&apos;');
}
