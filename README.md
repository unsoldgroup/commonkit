# CommonKit

**Your best development setup, everywhere.**

CommonKit gives every coding agent your proven instructions, skills, hooks,
tools, and policies. Define the setup once. Preview changes before they land.
Apply them across local machines, remote hosts, and projects.

```text
use → learn → review → improve once → benefit everywhere
```

CommonKit is for AI-native developers and small engineering teams tired of
rebuilding agent setups one machine, project, and tool at a time.

## Why CommonKit

### Start capable

Carry your working agent setup to every target. New environments start with
the capabilities and guardrails you already trust.

### Stay consistent

Treat shared developer configuration as versioned desired state. CommonKit
finds drift, shows the plan, applies approved changes, and verifies the result.

### Improve once. Benefit everywhere.

CommonKit's native skill-optimization lifecycle turns evaluations and approved,
redacted usage evidence into reviewable improvement candidates. SkillOpt runs
in isolation. It cannot rewrite an active skill. A human approves the change
before CommonKit promotes and distributes it.

> Skill optimization is under active development and is not part of the
> current TypeScript CLI release.

### Change safely

Dry runs, explicit approval, secret scanning, staged writes, backups, and
structural configuration merges make changes inspectable and recoverable.

### Provision secrets safely

Portable configuration contains references, not secret values. Password-manager
providers provision credentials independently on each target. Bitwarden Secrets
Manager is the first native provider.

## How it works

1. Define a **Loadout**: the capabilities and overrides a target should have.
2. Run `commonkit doctor` to check prerequisites.
3. Run `commonkit diff` or a dry run to inspect drift.
4. Apply the approved plan.
5. Verify the target matches the desired state.

```sh
commonkit doctor
commonkit diff
commonkit apply --dry-run
commonkit apply --yes
commonkit plugins --yes
commonkit verify
```

CommonKit is intentionally one-way and declarative. Portable configuration is
versioned. Credentials are provisioned independently on each target, while
runtime-specific state stays outside the portable manifest.

## What it manages today

- global Claude and Codex instructions;
- Claude agents and active or library skills;
- Claude hooks, permissions, plugins, and marketplace declarations;
- Codex configuration, hooks, rules, review checklists, and templates;
- the target user's Codex home and Orca's isolated Codex runtime home;
- Orca agent hooks and native Linear connectivity;
- context-mode content databases, excluding live session state;
- root-only service environment references already available on the target;
- generated Claude and Codex agent and skill symlinks;
- optional `mcp-local-relay` state.

Repository-level `AGENTS.md`, `CLAUDE.md`, manifests, `CONTEXT.md`, and ADRs
remain Git-owned. CommonKit verifies them after clone or pull instead of
overlaying them.

## What CommonKit is—and is not

CommonKit is the desired-state control plane for a complete agentic development
setup. It composes specialized providers instead of replacing them:

- APM resolves and compiles versioned agent context.
- Chezmoi can materialize compatible home configuration.
- SkillOpt produces skill-improvement candidates.
- Password managers provide secrets; Bitwarden Secrets Manager is native first.
- `mcp-local-relay` keeps MCP upstreams warm as an independent data plane.

The alternative is fragmented manual setup—not any one of these tools.

## Safety model

CommonKit excludes known authentication files, `.env` files, histories,
sessions, memories, device identities, E2EE keys, sockets, private keys, and
live orchestration databases from portable state. Generated configuration is
scanned for secret-like keys and inline credentials before it is written.

Single-file changes use a staged file and atomic rename. Tree and context
updates use per-file rsync backups. Every apply creates a timestamped backup
under `~/.local/state/commonkit/backups`. Recovery from these backups is manual
today; apply is not yet a transactional rollback.

Before reconciling plugin removals, CommonKit saves the remote Claude and Codex
plugin inventories. These inventories act as reinstall manifests for manual
recovery.

`apply --dry-run` uses the same semantic comparison as `diff`. It reports
changed, missing, synchronized, or unreachable resources without running
bootstrap, plugin, service-environment, or restart actions.

## Get started

Requirements: Node.js 24+, SSH, rsync, and pnpm.

```sh
pnpm install
cp commonkit.example.json commonkit.json
```

Edit `commonkit.json` for the target, then run:

```sh
commonkit doctor
commonkit apply --dry-run
```

For another configuration file, pass `--config <file>`. Secret values never
belong in the manifest.

Run the workspace checks with:

```sh
pnpm test
pnpm typecheck
```

## Open-source customization

Change the SSH host, remote home, service name, required commands, secret
variable names, and provider configuration in your local manifest.

The current plugin policy is `exact`: the local installed set and enabled state
become desired state. Apply reconciles managed remote additions and removals but
does not mutate local plugins. Claude does not expose arbitrary version pinning,
so a remote update may resolve to the marketplace's current version. Verification
fails closed when that state differs from local.

CommonKit is a pnpm workspace and owns the independently publishable
[`mcp-local-relay`](packages/mcp-local-relay) package.
