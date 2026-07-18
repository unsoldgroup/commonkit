# CommonKit

CommonKit is a portable, versioned collection of shared developer capabilities that can be materialized consistently across local machines, remote hosts, projects, and coding agents.

## Product positioning

**Primary audience**:
AI-native developers and small engineering teams using multiple coding agents across local and remote development environments.

**Primary promise**:
Your best development setup, everywhere. CommonKit makes hard-won agent instructions, skills, hooks, tools, and policies portable so every machine and coding agent starts capable, consistent, and ready.

**Supporting benefits**:
Control, safety, and speed substantiate the primary capability promise; they do not compete with it as the lead message.

**Continuous improvement**:
CommonKit natively uses SkillOpt to produce evidence-backed skill improvement candidates from evaluations and approved, redacted usage evidence. Improvements are computed in isolation, reviewed by a human, promoted into Git-owned source, and then reconciled across targets. CommonKit never allows the optimizer to mutate active skills directly.

**Secret provisioning**:
CommonKit keeps secret values out of portable configuration and integrates with password managers to provision them independently on each target. Bitwarden Secrets Manager is the native first provider, not the permanent product boundary.

_Avoid_: Claims that CommonKit synchronizes secrets, guarantees all machine identity remains local, or is exclusively coupled to Bitwarden.

**Brand voice**:
Concise, confident craftsperson. Use plain claims, concrete verbs, short sentences, and technically exact supporting evidence. Be opinionated without hype. Avoid flowery language, cute metaphors, enterprise jargon, and inflated AI-product claims.

## Language

**CommonKit**:
The complete portable collection of shared developer capabilities and policies.
_Avoid_: Toolbox, toolchain, environment sync

**Loadout**:
A selected subset and set of overrides from a CommonKit for a particular target.
_Avoid_: Profile, bundle

**Target**:
A local or remote machine where a loadout is materialized.
_Avoid_: Box, VPS, environment

**Adapter**:
A translator between CommonKit's normalized model and an agent or service's native configuration.
_Avoid_: Plugin, integration

**Agent-context provider**:
A package system that resolves, locks, audits, and compiles agent instructions, skills, prompts, hooks, plugins, and MCP declarations.
_Avoid_: CommonKit package manager

**Desired-state provider**:
A version-pinned, non-mutating resolver that converts provider-owned inputs into normalized desired resources and provenance.
_Avoid_: Adapter, installer

**Operation adapter**:
A CommonKit-controlled mutator that prepares, applies, verifies, and rolls back normalized operations on a target.
_Avoid_: Provider, package manager

**Reconciliation**:
The process of planning, applying, and verifying a target against its selected loadout.
_Avoid_: File copy, deployment

## Relationships

- A **CommonKit** defines one or more **Loadouts**.
- A **CommonKit** composes configuration in this precedence order: public base, organization policy, personal kit, project loadout, then target overrides.
- Organization security policy is a non-overridable floor. Later layers may tighten it but cannot weaken it.
- A **Loadout** is materialized on one or more **Targets**.
- An **Adapter** participates in **Reconciliation** for one agent or service.
- A **Loadout** selects a version-pinned **Agent-context provider** for portable agent content.
- A **Desired-state provider** computes resources in isolated staging; it never mutates a managed live target.
- An **Operation adapter** is the only component allowed to mutate, verify, or roll back a target.
- APM is CommonKit's preferred **Agent-context provider**; CommonKit does not compete with APM's package resolution, distribution, compilation, or package-audit responsibilities.
- Chezmoi is CommonKit's preferred home-configuration provider for the subset of semantics proven equivalent under isolated materialization; unsupported destination-dependent or executable features fail closed.
- SkillOpt is an exact-version external candidate-computation provider, not a target mutator. CommonKit independently evaluates held-out evidence and policy, requires human promotion into canonical Git source, then uses APM and CommonKit reconciliation for compilation and named-canary deployment.
- Native providers remain available for migration, fallback, and capabilities not safely delegated upstream.
- CommonKit stages provider output and applies it through CommonKit **Reconciliation** so target mutation remains plan-bound, receipted, verifiable, and recoverable.
- APM policy governs which agent packages and primitives may be installed; CommonKit policy governs targets, paths, permissions, services, credentials, schedules, relay exposure, and mutation authorization.
- The APM lockfile owns the resolved agent-package graph and content integrity; the CommonKit lockfile references its digest and owns composed loadout, target, and adapter state.
- **Reconciliation** never treats secrets or machine identity as portable CommonKit content.
- **mcp-local-relay** is an independently publishable package in the CommonKit repository. It remains the MCP data plane; CommonKit owns desired-state composition and reconciliation.
- A GitHub repository is the durable store for portable, reviewable CommonKit state. Secrets and mutable database files do not belong in Git.
- A local CommonKit service exposes peer interfaces for the macOS status bar, CLI, and MCP tools. The status bar does not communicate through MCP.
- The version 1 runtime is implemented in Rust and shared by the CLI, local service, MCP server, and Tauri 2 desktop application. The existing TypeScript reconciliation engine and `mcp-local-relay` runtime are migration sources, not permanent sidecars.
- Database adapters create consistent, integrity-checked, encrypted snapshots in S3-compatible object storage. Git records only snapshot descriptors and content hashes.
- Version 1 uses one authoritative writer per database. Cross-machine database portability is snapshot and restore, not binary merging; multi-writer synchronization requires a later application-level export/import model.

## Example dialogue

> **Developer:** "Does my remote Codex session have the same hooks and skills as my Mac?"
> **Domain expert:** "Apply the remote-development **Loadout** from your **CommonKit** to that **Target**, then verify its **Adapters**."

## Flagged ambiguities

- "Toolbox" was the initial metaphor; resolved: the product and domain object are **CommonKit**.
- "Profile" described a selected subset; resolved: use **Loadout** unless later user research favors a more conventional term.
- "Agent package manager" overlapped with CommonKit's initial adapter scope; resolved: package management belongs to the selected **Agent-context provider**, with APM preferred, while CommonKit orchestrates the complete developer environment.

## Open — not yet resolved

- How the APM adapter translates MCP declarations into `mcp-local-relay` upstream desired state and generated client configuration.
- Which GitHub authentication and repository-provisioning flow CommonKit supports in version 1.
- Which S3-compatible object-store provider is the default for encrypted snapshots.

## Version 1 contract

Version 1 is complete only when it supports all of these flows:

1. Initialize a target.
2. Preview drift.
3. Apply safely.
4. Verify parity.
5. Provision credential references independently.
6. Recover or roll back.
7. Manage multiple targets and layered configuration.
8. Run scheduled read-only drift checks.
9. Manage optional `mcp-local-relay` state.
10. Add coding-agent adapters.
11. Export redacted diagnostics.
12. Provide open-source onboarding, schema, threat model, and CI.

Version 1 supports macOS, Linux, and Windows as first-class managed targets. Every platform must support headless CLI/service operation; the desktop status application is an additional interface, not a runtime requirement.
