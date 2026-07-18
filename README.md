# CommonKit

CommonKit carries shared developer capabilities across local machines, remote
hosts, projects, and coding agents. Its current implementation safely makes a
headless development host reproduce a trusted local Claude Code/Codex setup.
It is intentionally one-way and declarative: portable configuration is
versioned, while credentials and runtime identity remain independently
provisioned on each target.

This repository is a pnpm workspace. It also owns the independently
publishable [`mcp-local-relay`](packages/mcp-local-relay) package, which keeps
MCP upstreams warm and exposes them through one persistent local endpoint.
CommonKit remains the desired-state control plane; the relay is its MCP data
plane.

Requires Node.js 24+, SSH, and rsync. Copy `commonkit.example.json` to
`commonkit.json` and select your target before running it.

```sh
commonkit doctor
commonkit diff
commonkit apply --dry-run
commonkit apply --yes
commonkit plugins --yes
commonkit verify
```

Run all workspace checks with:

```sh
pnpm install
pnpm test
pnpm typecheck
```

## What it synchronizes

- global Claude and Codex instructions;
- Claude agents and active/library skills;
- Claude hooks, permissions, plugins, and marketplace declarations through a
  structural settings merge;
- Codex configuration, hooks, rules, review checklist, and templates;
- both the target user's Codex home and Orca's isolated Codex runtime home;
- Orca agent hooks;
- context-mode content databases, excluding live WAL/SHM and session state;
- root-only service environment references already available on the VPS;
- native Orca Linear connectivity using the provisioned service token;
- generated Claude/Codex agent and skill symlinks.

Repo-level `AGENTS.md`, `CLAUDE.md`, manifests, `CONTEXT.md`, and ADRs remain
Git-owned and should be verified after clone/pull rather than overlaid.

## Safety model

Authentication files, `.env` files, histories, sessions, memories, device
identities, E2EE keys, sockets, private keys, and Orca's live orchestration
database are denied by the manifest and recursive transfer filters. Generated
configuration is scanned for secret-like keys and inline credentials before it
is written. Native Orca Linear OAuth is runtime-specific and is verified but
never exported. `linearis` authentication is checked separately.

Single-file writes use a staged file and atomic rename. Tree and context
updates use rsync's per-file backup support. Every apply creates a timestamped
backup below `~/.local/state/commonkit/backups`; recovery from those backups
is currently a manual operation, not a transactional rollback. VPS-only Claude
settings are preserved through a structural merge. Codex hook trust state is
removed so it can be regenerated for installed VPS paths.

Before reconciling plugin removals, apply saves the remote Claude and both
Codex plugin inventories under that run's `plugin-state/` backup directory.
Those inventories are reinstall manifests for manual recovery.

`apply --dry-run` performs the same semantic comparison as `diff` and reports
which files or trees are synced, changed, missing, or unreachable. It does not
run bootstrap, plugin, service-environment, or restart actions.

## Open-source customization

Copy `commonkit.example.json` to `commonkit.json`, then change the SSH host,
remote home, service name,
required commands, and secret variable names, then pass `--config <file>`.
Secret values never belong in the manifest.

The plugin policy is `exact`: the local installed set and enabled state are the
desired manifest. Apply reconciles managed remote additions and removals but
does not mutate local plugins. Claude does not expose arbitrary version
pinning, so a remote update may still resolve to the marketplace's current
version; verification fails closed if that differs from local state.
