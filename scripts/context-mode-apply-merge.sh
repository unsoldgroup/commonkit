#!/usr/bin/env bash
# Union-merge the context-mode stores into the canonical store (USG-96).
#
#   scripts/context-mode-apply-merge.sh            # dry run, changes nothing
#   scripts/context-mode-apply-merge.sh --apply    # write the merged store
#
# Agent sessions may stay running. Every database is copied with VACUUM INTO,
# which snapshots through a read transaction and captures WAL content, so a
# live writer no longer forces a torn read. Sessions already running keep
# writing to the old store until they restart.
#
# Writes only to the destination. Both source stores are left untouched, so
# undoing this is `rm -rf` on the destination.
set -euo pipefail
export NODE_NO_WARNINGS=1

CLAUDE_STORE="${CLAUDE_STORE:-$HOME/.claude/context-mode}"
CODEX_STORE="${CODEX_STORE:-$HOME/.codex/context-mode}"
DEST="${DEST:-$HOME/.local/share/context-mode}"

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
merge="$repo/scripts/context-mode-merge.mjs"

apply_flag=""
case "${1:-}" in
  --apply) apply_flag="--apply" ;;
  "") ;;
  *) printf 'usage: %s [--apply]\n' "$(basename "$0")" >&2; exit 1 ;;
esac

[ -f "$merge" ] || { echo "$merge not found. Run this from a commonkit checkout on main." >&2; exit 1; }
for store in "$CLAUDE_STORE" "$CODEX_STORE"; do
  [ -d "$store" ] || { echo "source store not found: $store" >&2; exit 1; }
done

node "$merge" --source "$CLAUDE_STORE" --source "$CODEX_STORE" --out "$DEST" ${apply_flag:+"$apply_flag"}

if [ -n "$apply_flag" ]; then
  cat <<EOF

Merged store:  $DEST
Manifest:      $DEST/merge-manifest.json
Sources are unchanged:
  $CLAUDE_STORE
  $CODEX_STORE

To undo: rm -rf "$DEST"
EOF
fi
