#!/usr/bin/env bash
# Update an already-installed linear-graph hub to the tip of its tracked branch.
# Run ON the VPS. install-vps.sh does first-time setup; this is every deploy after.
#
# The checkout must be on a branch with an upstream. A clone made with a
# single-branch refspec cannot see the branch carrying this app, which is what
# forced hand-copied files before EXP-2820:
#   git config remote.origin.fetch "+refs/heads/*:refs/remotes/origin/*"
set -euo pipefail

readonly REPO_DIR="${LINEAR_GRAPH_REPO_DIR:-$HOME/commonkit}"
readonly SERVICE="linear-graph.service"

cd "$REPO_DIR"

upstream="$(git rev-parse --abbrev-ref --symbolic-full-name '@{upstream}' 2>/dev/null || true)"
if [[ -z "$upstream" ]]; then
  echo "error: $REPO_DIR is not on a branch with an upstream; see install-vps.sh" >&2
  exit 1
fi

# Anything uncommitted here is a hand-edit on a deploy target: refuse rather than
# silently discard it. .bak files and other untracked cruft do not block a deploy.
if ! git diff --quiet || ! git diff --cached --quiet; then
  echo "error: $REPO_DIR has uncommitted changes to tracked files; commit or discard them first" >&2
  git status --short >&2
  exit 1
fi

before="$(git rev-parse HEAD)"
git fetch origin --quiet
after="$(git rev-parse "$upstream")"

if [[ "$before" == "$after" ]]; then
  echo "already at $after ($upstream); restarting anyway to pick up env changes"
else
  echo "updating $before -> $after ($upstream)"
  git reset --hard "$after" --quiet
fi

# The hub runs TypeScript directly under Bun, so only protocol and web need a build.
if [[ "$before" != "$after" ]] && git diff --name-only "$before" "$after" | grep -qE '^apps/linear-graph/(protocol|web)/'; then
  echo "protocol or web changed; rebuilding"
  pnpm --dir "$REPO_DIR" --filter @commonkit/linear-graph-protocol build
  pnpm --dir "$REPO_DIR" --filter @commonkit/linear-graph-web build
fi

systemctl --user restart "$SERVICE"

pid="$(systemctl --user show "$SERVICE" -p MainPID --value)"
port="$(tr '\0' '\n' < "/proc/${pid}/environ" | sed -n 's/^LINEAR_GRAPH_PORT=//p')"
port="${port:-8790}"
for _ in $(seq 1 15); do
  if curl -fsS "http://127.0.0.1:${port}/health" >/dev/null 2>&1; then
    echo "healthy on ${port} at $(git rev-parse --short HEAD)"
    exit 0
  fi
  sleep 2
done

echo "error: ${SERVICE} did not answer /health on ${port} after the restart" >&2
systemctl --user status "$SERVICE" --no-pager -n 20 >&2
exit 1
