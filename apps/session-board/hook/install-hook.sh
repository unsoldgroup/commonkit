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
settings.hooks.PreToolUse ??= [];
if (!Array.isArray(settings.hooks.PreToolUse)) throw new Error("settings.hooks.PreToolUse must be an array");
const exists = settings.hooks.PreToolUse.some((entry) =>
  Array.isArray(entry?.hooks) && entry.hooks.some((hook) => hook?.type === "command" && hook?.command === command));
if (!exists) settings.hooks.PreToolUse.push({
  matcher: "Bash|Edit|Write|NotebookEdit|WebFetch|WebSearch|Task",
  hooks: [{ type: "command", command, timeout: 60 }],
});
await mkdir(dirname(path), { recursive: true });
const temporary = `${path}.session-board.tmp`;
await writeFile(temporary, `${JSON.stringify(settings, null, 2)}\n`, { mode: 0o600 });
await rename(temporary, path);
' 

echo "Installed Session Board PreToolUse hook in $SETTINGS_PATH"
