#!/usr/bin/env bash
set -euo pipefail

APP_DIR="$HOME/Applications/Mcp Local Relay Status Bar.app"
PLIST_PATH="$HOME/Library/LaunchAgents/com.unsoldgroup.mcp-local-relay-status-bar.plist"

launchctl bootout "gui/$(id -u)" "$PLIST_PATH" >/dev/null 2>&1 || true
rm -f "$PLIST_PATH"
rm -rf "$APP_DIR"

printf 'Removed %s\n' "$APP_DIR"
printf 'Removed %s\n' "$PLIST_PATH"
