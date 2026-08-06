#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
SETTINGS_PATH=${CLAUDE_SETTINGS_PATH:-"$HOME/.claude/settings.json"}
HOOK_COMMAND="bun run \"$SCRIPT_DIR/session-board-hook.ts\""
export SETTINGS_PATH HOOK_COMMAND

bun -e '
import { mkdir, readFile, rename, writeFile } from "node:fs/promises";
import { dirname } from "node:path";
const path = process.env.SETTINGS_PATH;
const command = process.env.HOOK_COMMAND;
let settings = {};
try { settings = JSON.parse(await readFile(path, "utf8")); }
catch (error) { if (error?.code !== "ENOENT") throw error; }
settings.hooks ??= {};
if (!settings.hooks || typeof settings.hooks !== "object" || Array.isArray(settings.hooks)) throw new Error("settings.hooks must be an object");
const isOurs = (hook) => hook?.type === "command" && typeof hook?.command === "string" && hook.command.includes("session-board-hook.ts");
// Migrate: session-board used to register under PreToolUse, which also fires for auto-approved calls.
if (Array.isArray(settings.hooks.PreToolUse)) {
  settings.hooks.PreToolUse = settings.hooks.PreToolUse.filter((entry) => !(Array.isArray(entry?.hooks) && entry.hooks.some(isOurs)));
  if (settings.hooks.PreToolUse.length === 0) delete settings.hooks.PreToolUse;
}
settings.hooks.PermissionRequest ??= [];
if (!Array.isArray(settings.hooks.PermissionRequest)) throw new Error("settings.hooks.PermissionRequest must be an array");
settings.hooks.PermissionRequest = settings.hooks.PermissionRequest.filter((entry) => !(Array.isArray(entry?.hooks) && entry.hooks.some(isOurs)));
settings.hooks.PermissionRequest.push({
  matcher: "Bash|Edit|Write|NotebookEdit|WebFetch|WebSearch|Task",
  hooks: [{ type: "command", command, timeout: 60 }],
});
await mkdir(dirname(path), { recursive: true });
const temporary = `${path}.session-board.tmp`;
await writeFile(temporary, `${JSON.stringify(settings, null, 2)}\n`, { mode: 0o600 });
await rename(temporary, path);
' 

echo "Installed Session Board PreToolUse hook in $SETTINGS_PATH"
