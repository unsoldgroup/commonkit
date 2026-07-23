#!/usr/bin/env bash
set -euo pipefail

readonly REPO_DIR="${SESSION_BOARD_REPO_DIR:-$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../../../.." && pwd)}"
readonly TEMPLATE="$REPO_DIR/apps/session-board/deploy/templates/cloud.unsold.session-board-reporter.plist"
readonly CONFIG_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/commonkit"
readonly CONFIG_FILE="$CONFIG_DIR/session-board-reporter.json"
readonly LAUNCH_AGENTS_DIR="$HOME/Library/LaunchAgents"
readonly PLIST="$LAUNCH_AGENTS_DIR/cloud.unsold.session-board-reporter.plist"
readonly LOG_DIR="$HOME/Library/Logs/CommonKit"
DOMAIN_TARGET="gui/$(id -u)"
readonly DOMAIN_TARGET
readonly SERVICE_TARGET="$DOMAIN_TARGET/cloud.unsold.session-board-reporter"

require_command() {
  command -v "$1" >/dev/null 2>&1 || {
    printf 'Required command not found: %s\n' "$1" >&2
    exit 1
  }
}

xml_escape() {
  local value=$1
  value=${value//&/&amp;}
  value=${value//</&lt;}
  value=${value//>/&gt;}
  value=${value//\"/&quot;}
  value=${value//\'/&apos;}
  printf '%s' "$value"
}

require_command bun
require_command pnpm
require_command launchctl
require_command plutil

: "${SESSION_BOARD_MACHINE_TOKEN:?Set SESSION_BOARD_MACHINE_TOKEN}"
: "${SESSION_BOARD_MACHINE_ID:?Set SESSION_BOARD_MACHINE_ID to this Target id}"
: "${SESSION_BOARD_MACHINE_NAME:?Set SESSION_BOARD_MACHINE_NAME to the board label}"
readonly HUB_URL="${SESSION_BOARD_HUB_URL:-https://board.unsold.cloud}"
readonly HOOK_PORT="${SESSION_BOARD_HOOK_PORT:-47821}"
BUN_BIN="$(command -v bun)"
readonly BUN_BIN

mkdir -p "$CONFIG_DIR" "$LAUNCH_AGENTS_DIR" "$LOG_DIR"
umask 077
export HUB_URL HOOK_PORT CONFIG_FILE
# shellcheck disable=SC2016
bun -e '
import { writeFile } from "node:fs/promises";
await writeFile(process.env.CONFIG_FILE, `${JSON.stringify({
  hubUrl: process.env.HUB_URL,
  machineToken: process.env.SESSION_BOARD_MACHINE_TOKEN,
  machineId: process.env.SESSION_BOARD_MACHINE_ID,
  machineName: process.env.SESSION_BOARD_MACHINE_NAME,
  hookPort: Number(process.env.HOOK_PORT),
}, null, 2)}\n`, { mode: 0o600 });
'
chmod 600 "$CONFIG_FILE"

sed -e "s|@@BUN_BIN@@|$(xml_escape "$BUN_BIN")|g" \
  -e "s|@@REPO_DIR@@|$(xml_escape "$REPO_DIR")|g" \
  -e "s|@@CONFIG_FILE@@|$(xml_escape "$CONFIG_FILE")|g" \
  -e "s|@@LOG_DIR@@|$(xml_escape "$LOG_DIR")|g" \
  "$TEMPLATE" >"$PLIST"
chmod 600 "$PLIST"
plutil -lint "$PLIST"

pnpm --dir "$REPO_DIR" install --frozen-lockfile
launchctl bootout "$SERVICE_TARGET" >/dev/null 2>&1 || true
launchctl bootstrap "$DOMAIN_TARGET" "$PLIST"
launchctl enable "$SERVICE_TARGET"
launchctl kickstart -k "$SERVICE_TARGET"

printf 'Session Board reporter installed for Target %s.\n' "$SESSION_BOARD_MACHINE_ID"
printf 'This installer does not modify Claude or Codex global configuration.\n'
