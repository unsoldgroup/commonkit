#!/usr/bin/env bash
set -euo pipefail

readonly REPO_DIR="${SESSION_BOARD_REPO_DIR:-$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../../.." && pwd)}"
readonly TEMPLATE="$REPO_DIR/apps/session-board/deploy/templates/cloud.unsold.session-board-reporter.plist"
readonly CONFIG_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/commonkit"
readonly CONFIG_FILE="$CONFIG_DIR/session-board-reporter.json"
readonly LAUNCH_AGENTS_DIR="$HOME/Library/LaunchAgents"
readonly PLIST="$LAUNCH_AGENTS_DIR/cloud.unsold.session-board-reporter.plist"
readonly LOG_DIR="$HOME/Library/Logs/CommonKit"
readonly RUNTIME_ROOT="${COMMONKIT_SESSION_REPORTER_RUNTIME_ROOT:-$HOME/Library/Application Support/com.unsoldgroup.CommonKit/runtime/session-board-reporter}"
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
require_command shasum

: "${SESSION_BOARD_MACHINE_TOKEN:?Set SESSION_BOARD_MACHINE_TOKEN}"
: "${SESSION_BOARD_MACHINE_ID:?Set SESSION_BOARD_MACHINE_ID to this Target id}"
: "${SESSION_BOARD_MACHINE_NAME:?Set SESSION_BOARD_MACHINE_NAME to the board label}"
readonly HUB_URL="${SESSION_BOARD_HUB_URL:-https://board.unsold.cloud}"
readonly HOOK_PORT="${SESSION_BOARD_HOOK_PORT:-47821}"
BUN_BIN="$(command -v bun)"
readonly BUN_BIN

mkdir -p "$CONFIG_DIR" "$LAUNCH_AGENTS_DIR" "$LOG_DIR" "$RUNTIME_ROOT"
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

pnpm --dir "$REPO_DIR" install --frozen-lockfile
pnpm --dir "$REPO_DIR" --filter @commonkit/session-board-protocol build
STAGING_DIR="$(mktemp -d "$RUNTIME_ROOT/.staging.XXXXXX")"
readonly STAGING_DIR
cleanup() {
  if [[ -d "$STAGING_DIR" ]]; then
    rm -rf -- "$STAGING_DIR"
  fi
}
trap cleanup EXIT
bun build "$REPO_DIR/apps/session-board/reporter/src/main.ts" \
  --outfile "$STAGING_DIR/session-board-reporter.js" \
  --target bun
RUNTIME_DIGEST="$(shasum -a 256 "$STAGING_DIR/session-board-reporter.js" | awk '{print $1}')"
readonly RUNTIME_DIGEST
RUNTIME_DIR="$RUNTIME_ROOT/$RUNTIME_DIGEST"
readonly RUNTIME_DIR
if [[ ! -d "$RUNTIME_DIR" ]]; then
  mv "$STAGING_DIR" "$RUNTIME_DIR"
fi
CURRENT_LINK="$RUNTIME_ROOT/current"
NEXT_LINK="$RUNTIME_ROOT/.current.$$.next"
readonly CURRENT_LINK NEXT_LINK
ln -s "$RUNTIME_DIR" "$NEXT_LINK"
mv -f "$NEXT_LINK" "$CURRENT_LINK"

sed -e "s|@@BUN_BIN@@|$(xml_escape "$BUN_BIN")|g" \
  -e "s|@@RUNTIME_DIR@@|$(xml_escape "$CURRENT_LINK")|g" \
  -e "s|@@CONFIG_FILE@@|$(xml_escape "$CONFIG_FILE")|g" \
  -e "s|@@LOG_DIR@@|$(xml_escape "$LOG_DIR")|g" \
  "$TEMPLATE" >"$PLIST"
chmod 600 "$PLIST"
plutil -lint "$PLIST"

launchctl bootout "$SERVICE_TARGET" >/dev/null 2>&1 || true
for attempt in 1 2 3; do
  if launchctl bootstrap "$DOMAIN_TARGET" "$PLIST"; then
    break
  fi
  if [[ "$attempt" == 3 ]]; then
    printf 'Unable to bootstrap Session Board reporter after %s attempts.\n' "$attempt" >&2
    exit 1
  fi
  sleep 1
done
launchctl enable "$SERVICE_TARGET"
launchctl kickstart -k "$SERVICE_TARGET"

printf 'Session Board reporter installed for Target %s.\n' "$SESSION_BOARD_MACHINE_ID"
printf 'This installer does not modify Claude or Codex global configuration.\n'
