#!/usr/bin/env bash
# Stop the context-mode writers, union-merge both stores, restart the writers.
#
#   scripts/context-mode-apply-merge.sh            # dry run, changes nothing
#   scripts/context-mode-apply-merge.sh --apply    # stop writers, merge, restart
#
# macOS only: stopping the shared writer is a launchd operation. The Linux
# target runs its own store under systemd and is out of scope here.
#
# Writes only to the destination. Both source stores are left untouched, so
# undoing this is `rm -rf` on the destination.
#
# Repointing agents at the merged store is USG-99, not this script. Until then
# the merged store is a staged copy nothing reads.
set -euo pipefail
export NODE_NO_WARNINGS=1

CLAUDE_STORE="${CLAUDE_STORE:-$HOME/.claude/context-mode}"
CODEX_STORE="${CODEX_STORE:-$HOME/.codex/context-mode}"
DEST="${DEST:-$HOME/.local/share/context-mode}"

agent_label="group.unsold.context-mode-mcp"
agent_plist="$HOME/Library/LaunchAgents/$agent_label.plist"
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
merge="$repo/scripts/context-mode-merge.mjs"

if [ -t 1 ]; then bold=$'\033[1m'; red=$'\033[31m'; off=$'\033[0m'; else bold=""; red=""; off=""; fi
say() { printf '\n%s==> %s%s\n' "$bold" "$*" "$off"; }
die() { printf '\n%sstopped: %s%s\n' "$red" "$*" "$off" >&2; exit 1; }

apply_flag=""
case "${1:-}" in
  --apply) apply_flag="--apply" ;;
  "") ;;
  *) die "unknown argument: $1
usage: $(basename "$0") [--apply]" ;;
esac

[ "$(uname -s)" = "Darwin" ] || die "macOS only: this stops a launchd agent. See the header."
[ -f "$merge" ] || die "$merge not found. Run this from a commonkit checkout on main."
for store in "$CLAUDE_STORE" "$CODEX_STORE"; do
  [ -d "$store" ] || die "source store not found: $store"
done

agent_loaded() { launchctl list "$agent_label" >/dev/null 2>&1; }

restart_agent=0
restore() {
  if [ "$restart_agent" = 1 ] && ! agent_loaded; then
    say "restarting $agent_label"
    launchctl bootstrap "gui/$(id -u)" "$agent_plist" 2>/dev/null || true
  fi
}
trap restore EXIT INT TERM

# The dry run only lists filenames, so it is safe while agents are running and
# must short-circuit before anything is stopped.
if [ -z "$apply_flag" ]; then
  say "dry run (nothing is stopped, nothing is written)"
  node "$merge" --source "$CLAUDE_STORE" --source "$CODEX_STORE" --out "$DEST"
  printf '\nre-run with --apply to write. That will stop the context-mode writers first.\n'
  exit 0
fi

# Only restart what this script stopped. An agent Al had deliberately stopped
# stays stopped.
if agent_loaded; then
  say "stopping the shared writer ($agent_label)"
  restart_agent=1
  launchctl bootout "gui/$(id -u)/$agent_label" 2>/dev/null || true
  # bootout returns before the process is gone.
  for _ in {1..10}; do agent_loaded || break; sleep 0.5; done
  if agent_loaded; then die "$agent_label would not stop"; fi
else
  say "$agent_label is not loaded; leaving it that way"
fi

# The merge refuses on its own if any agent session still holds a store, and
# names the holders. The trap restarts the writer either way.
say "merging into $DEST"
node "$merge" --source "$CLAUDE_STORE" --source "$CODEX_STORE" --out "$DEST" --apply

say "done"
cat <<EOF
Merged store:  $DEST
Manifest:      $DEST/merge-manifest.json
Sources are unchanged:
  $CLAUDE_STORE
  $CODEX_STORE

Nothing reads the merged store yet. To undo: rm -rf "$DEST"
EOF
