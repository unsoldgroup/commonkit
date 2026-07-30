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

**Job**:
An immutable, content-addressed request for durable execution that outlives its submitting client.
_Avoid_: Task process, remote command

**Attempt**:
A revisioned execution of a **Job** with its own lease, fencing token, and terminal receipt.
_Avoid_: Job retry

**Execution Profile**:
Execution-only resource, capability, isolation, and comparability requirements for a **Job**, pinning operating system, architecture, and machine class. It does not pin the toolchain (ADR 0010).
_Avoid_: Loadout

**Validation evidence**:
A terminal **Attempt** receipt binding an exact commit, platform, Loadout digest, and **Execution Profile** digest, recorded as a support-matrix row's proof that a platform is supported.
_Avoid_: Test run, CI result, green build

**Declared task**:
A repository-owned, immutable **ExecutionManifest** named by a stable task ID, the only thing a remote agent or webhook may submit.
_Avoid_: CI job, pipeline step, workflow

**Investigation**:
A **Declared task** that runs a coding agent on the **Execution Target** that produced a failed **Attempt**, inside its retained workspace, and emits findings as an artifact (ADR 0011).
_Avoid_: Auto-fix, self-healing CI, agent remediation

**Implementor**:
The adapter contract through which CommonKit drives a coding-agent engine as a **Job** — start, events, send, cancel, resume, result — keeping engine references opaque.
_Avoid_: Claude adapter, Codex integration

**Execution Target**:
A managed **Target** advertising durable-execution readiness, capacity, and an exact Loadout digest.
_Avoid_: Worker box

**Context budget**:
The per-**Loadout** accounting of agent context a target pays for on every turn, split into always-on text and router text.
_Avoid_: Token limit, context window

**Always-on text**:
Agent instruction content injected on every turn regardless of task, such as compiled `AGENTS.md`, `CLAUDE.md`, and hook prose. Reducible only by lossy rewriting.
_Avoid_: System prompt, preamble

**Router text**:
The single description line each skill contributes so an agent can decide whether to load it. Reducible only by admitting fewer skills.
_Avoid_: Skill metadata, frontmatter

**Distillation**:
A lossy rewrite of an imported source into the minimal form that preserves its behavior, producing a distilled artifact and a **Retention map**.
_Avoid_: Summarization, compression

**Retention map**:
The provenance record binding each digested section of an imported upstream source to whether distillation kept or dropped it, and where kept content landed.
_Avoid_: Source map, diff

**Material change**:
An upstream revision that touches or removes a section the **Retention map** marks kept, or adds a section matching no known digest or heading. Only a material change warrants re-distillation.
_Avoid_: Upstream drift, breaking change

## Relationships

- A **CommonKit** defines one or more **Loadouts**.
- A **CommonKit** composes configuration in this precedence order: public base, organization policy, personal kit, project loadout, then target overrides.
- Organization security policy is a non-overridable floor. Later layers may tighten it but cannot weaken it.
- A **Loadout** is materialized on one or more **Targets**.
- A **Job** has one or more ordered **Attempts**, but at most one active leased **Attempt**.
- An **Execution Target** references a managed **Target** and exact Loadout and **Execution Profile** digests.
- An **Execution Target**'s Loadout digest is the composed loadout digest of that **Target**'s last successful **Reconciliation**, not an independently authored value. A **Job** whose digests differ does not place, so a drifted target fails closed instead of producing incomparable results.
- A repository declares its own checks as **Declared tasks**. CommonKit submits them; it never accepts arbitrary argv, and a shell string is not a task.
- **Validation evidence** is commit-bound. It proves the platform and the CommonKit-managed environment; the toolchain is proven by the repository's own commit-pinned files (ADR 0010).
- An **Investigation** inherits the **Loadout** of the target it runs on, so it reasons with the same skills and hooks the developer's own agent would. Its output is artifacts only; an **Execution Target** produces evidence and analysis but never authors repository history (ADR 0011).
- The CLI is the primary surface for durable execution. The Session Board mirrors runs read-only for the glanceable case; it consumes the execution API as a client and never fronts it.
- MCP exposes context and durable execution controls; `commonkit-execd` owns lifecycle persistence independently of MCP sessions.
- An **Adapter** participates in **Reconciliation** for one agent or service.
- A **Loadout** selects a version-pinned **Agent-context provider** for portable agent content.
- A **Desired-state provider** computes resources in isolated staging; it never mutates a managed live target.
- An **Operation adapter** is the only component allowed to mutate, verify, or roll back a target.
- APM is CommonKit's preferred **Agent-context provider**; CommonKit does not compete with APM's package resolution, distribution, compilation, or package-audit responsibilities.
- Chezmoi is CommonKit's preferred home-configuration provider for the subset of semantics proven equivalent under isolated materialization; unsupported destination-dependent or executable features fail closed.
- A **Loadout** has exactly one **Context budget**; admitting an import that exceeds it forces an explicit eviction choice rather than silent growth.
- **Distillation** never mutates a live target. It computes a distilled artifact and **Retention map** in isolated staging, a human promotes them into canonical Git source, and CommonKit **Reconciliation** applies the result.
- An imported skill retains its upstream reference and revision as provenance. The upstream reference is decoupled from the distilled form; re-pulling upstream is a three-way merge against the distilled fork, not a replacement.
- Scheduled read-only upstream checks classify revisions against the **Retention map**. Non-material changes advance the recorded upstream revision silently; only a **Material change** notifies a human and queues re-distillation.
- Imported skills are admitted by human review against the **Context budget**, not by automated evaluation. An admitted skill may afterward enter the SkillOpt evaluation and promotion path unchanged.
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
- APM MCP translation was initially open; resolved: CommonKit reads only APM's documented `dependencies.mcp` manifest shape and isolated staged `.mcp.json`, requires the two representations to agree, normalizes declarations with manifest/lock provenance, rejects ambiguous ownership or unsupported transports, and sends the resulting relay desired state and stable loopback client configuration through CommonKit's confirmation-bound plan. It does not import APM internals or invoke provider-native live apply.
- GitHub authentication was initially open; resolved for v1: the installed GitHub CLI is the credential broker and repository-provisioning client. OAuth credentials remain outside CommonKit and the kit. A first-party GitHub App/device flow may replace this boundary later without changing portable state.

## Open — not yet resolved

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

## Session Board

The Session Board is CommonKit's glanceable approval surface: a hub on the always-on VPS (`board.unsold.cloud`, tailnet-only) fed by per-machine reporters, showing every Claude/Codex/Orca session and the decisions they are waiting on.

**Glossary**
- **Pending action**: a decision a human owes a session — a Claude permission prompt, a Codex numbered option prompt, or an Orca gate.
- **Approval card**: the board's rendering of one pending action. A card answers who ([project] + agent), what (verb + relative path or command), why (**intent** — the agent's last transcript message before asking), and shows the exact payload (diff, content, or command).
- **Verbs**: Allow (once), Always (persist a project-local allowlist rule — ADR 0009), Deny with an optional steer message. Codex cards keep their numbered options.
- **Intent**: transcript-derived context on a card; read from the session's JSONL transcript at prompt time, never stored beyond the action.
- **Stale**: an action whose 55s window lapsed; the terminal prompt took over (fail-open, ADR 0008) and the card must show that outcome rather than live buttons.
- **Decision log**: the recent history of human decisions, kept by the hub for glanceable audit (7-day retention).
- **Push opt-in**: per-device web-push subscription so a new pending action reaches the iPad when the board is closed; notification delivery never affects the decision path.
