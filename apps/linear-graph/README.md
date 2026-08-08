# Linear work graph

The graph is a private, read-only focus dashboard for Linear issues. It groups issues into
stable topical zones, renders parent and relationship edges, and adds only high-confidence
Codex suggestions as dashed edges. The UI has Focus and Universe views, filters, an accessible
issue list, editable focus brief, and a primary-topic correction control.

## Local development

```sh
pnpm install
pnpm --filter @commonkit/linear-graph-protocol test
pnpm --filter @commonkit/linear-graph-hub typecheck
pnpm --filter @commonkit/linear-graph-web build
LINEAR_API_TOKEN=... CODEX_API_KEY=... LINEAR_GRAPH_ACTION_TOKEN=... \
  pnpm --filter @commonkit/linear-graph-hub start
```

The hub listens on `127.0.0.1:8790` by default. `POST /api/analysis-runs` is protected by the
action token; Linear and Codex credentials are never exposed to the browser. The scheduled run
is daily at 06:00 Europe/Madrid by default (override with `LINEAR_GRAPH_TIMEZONE`), with a startup
run when no completed snapshot exists.

## VPS deployment

On the VPS, install Bun, pnpm, Caddy, and acme.sh first. Then run the installer with secrets
provided from the existing secrets manager (do not put them in the repository):

```sh
LINEAR_API_TOKEN='…' LINEAR_GRAPH_ACTION_TOKEN='…' \
CF_Token='…' LINEAR_GRAPH_TAILSCALE_IP='100.71.109.66' \
  bash apps/linear-graph/deploy/install-vps.sh
```

The installer writes a mode-600 environment file, a user systemd service, and a Caddy site
restricted to Tailscale source addresses. Create a DNS-only `graph.unsold.cloud` A record for that address,
then verify from a tailnet device:

```sh
curl -fsS https://graph.unsold.cloud/health
systemctl --user status linear-graph.service
```

Codex uses `CODEX_API_KEY` when supplied, or the existing headless Codex OAuth session on the
VPS when that variable is omitted. `LINEAR_GRAPH_REPO_MAP` is optional JSON such as
`{"teams":{"UNS":"unsold"},"projects":{"project-id":"repo-name"}}`; it provides bounded
repository context without sending repository source code to Codex. Codex runs as a read-only,
non-interactive child process with a strict output schema and no tools or network access.
