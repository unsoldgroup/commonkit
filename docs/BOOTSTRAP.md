# Bootstrap and tool ownership

CommonKit should make an autonomous agent setup reproducible without becoming
the trust root for its own installer, vendor accounts, or secret-manager
authorization. Keep the clean-machine bootstrap small, then let versioned,
opt-in loadouts carry the rest.

## Minimum clean-machine bootstrap

Install these outside CommonKit:

1. **Node.js 24 or newer** and npm, only because the current public CommonKit
   distribution is an npm package.
2. **Git and SSH** for the private kit repository.
3. **GitHub CLI (`gh`)**, authenticated to the account that owns or can read the
   kit repository. CommonKit uses `gh` as its initial identity and repository
   credential broker without copying its token.
4. **Claude Code, Codex CLI, or both**, installed and authenticated through the
   vendor's supported flow.
5. **CommonKit CLI** with
   `npm install --global @alunsoldgroup/commonkit`.

Rust, pnpm, APM, chezmoi, a desktop GUI, Session Board, Cloudflare, and an MCP
portal are not required for a normal CLI install. Rust 1.85+, pnpm 10.28.2, and
the repository toolchain are source-contributor requirements.

Bitwarden Secrets Manager is required only when a selected loadout contains
`bws://` credential references. Install and authorize `bws` outside CommonKit;
CommonKit may verify and use references but never owns the machine account or
access token.

## What CommonKit carries today

| Item | Current ownership |
| --- | --- |
| `commonkit`, `commonkitd`, target helper | Bundled in the platform-specific npm package with SHA-256 verification |
| Instructions, skills, hooks, plugins, MCP declarations | Versioned desired state, normally resolved by a pinned agent-context provider and applied by CommonKit |
| Files, services, credential references, relay state, snapshots | Planned, applied, verified, and receipted by CommonKit adapters |
| APM and chezmoi output | Computed by exact-pinned providers in isolated staging; providers never mutate the live Target |
| Engram state transport | Declared and reconciled through CommonKit's opaque chunk contracts; Engram owns its binary and database behavior |
| MCP client configuration | Generated from portable declarations; eligible cloud servers may point to one portal and local capabilities to the relay |

## What should fold into opt-in loadouts

These tools should not remain a manual checklist. CommonKit should acquire or
declare exact versions, install them only with package consent, configure their
agent assets, run a canary, and record ownership in the plan:

| Candidate | CommonKit responsibility | Provider boundary |
| --- | --- | --- |
| Microsoft APM | Verified provider acquisition/cache, exact pin, readiness, staged invocation | APM owns its package graph and lockfile |
| context-mode | Optional recipe, store location/mode, plugin and hook declarations, snapshot policy | Provider owns filtering, database schema, and retention behavior |
| Engram | Optional pinned install recipe, client assets, readiness, existing sync/reconcile declarations | Provider owns executable, live database, and authentication |
| ast-grep | Optional core coding-tool loadout with exact binary pin and routing skill | External executable remains independently upgradeable |
| RTK | Optional output-compaction loadout with wrapper canaries | Must never be enabled globally without proving diagnostic fidelity |
| Lightpanda | Optional browser-tool loadout and health check | Real Chrome remains separate for rendering and performance evidence |
| Browser and domain MCPs | Curated, disabled-by-default declarations, ownership, health, mutation class, and credential references | Upstream services and OAuth remain provider-owned |
| Probity | Policy template, hook declarations, and canary | Package stays in each repository's `package.json` and lockfile |
| Remote execution | Package `commonkit-execd`, expose primary CLI job verbs, and manage typed capability declarations | Host, network, certificates, and credentials remain Target-specific |
| Session Board | Optional package/loadout only; never part of core onboarding | Deployment, GitHub Access policy, and infrastructure remain explicit operator choices |

An `autonomous-agent` loadout can compose APM context, context-mode, Engram,
ast-grep, RTK, browser profiles, and a minimal MCP set. Every component remains
separately consented and removable. Domain MCPs stay disabled until the project
selects them.

## What must remain external

- Vendor installation, licensing, updates, and login for Claude Code and Codex.
- GitHub authentication and token custody in `gh`.
- Password-manager installation, machine-account authorization, and secret
  values.
- GitHub OAuth app secrets, Cloudflare account authorization, and upstream MCP
  OAuth grants.
- Project-specific test runners, repository dependencies, remote hosts, DNS,
  SSH keys, certificates, and production credentials.
- CommonKit self-update authority. Updates must be explicit, version-pinned,
  verifiable, and recoverable.

## CLI bootstrap roadmap

The next consolidation work should prioritize the CLI:

1. Ship a signed standalone archive and Homebrew path so Node is no longer a
   normal-user prerequisite.
2. Add `commonkit bootstrap plan` to explain prerequisites, proposed package
   acquisitions, consent boundaries, and recovery before mutation.
3. Add typed Homebrew and Corepack/pnpm resources with source authority,
   checksums, package consent, rollback, and native lifecycle evidence.
4. Add verified provider acquisition and `commonkit doctor` checks for APM,
   context-mode, Engram, `gh`, `bws`, agent clients, hooks, portal/relay clients,
   and binaries selected but missing.
5. Publish the opt-in `autonomous-agent` loadout and test it on a disposable
   clean machine through `sync -> diff -> apply -> verify`.

The desktop GUI stays alpha and consumes the same daemon contracts. It does not
define bootstrap readiness or gate these CLI milestones.
