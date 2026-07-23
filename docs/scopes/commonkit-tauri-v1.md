# CommonKit v1 — Rust and Tauri 2 implementation scope

Status: Ready for technical planning  
Tracking: UNS-1274  
Repository: `unsoldgroup/commonkit`

## Executive summary

CommonKit v1 is a portable, layered developer environment that moves shared capabilities and policies across macOS, Linux, Windows, projects, remote targets, and coding agents. GitHub stores reviewable desired state. A self-contained Rust runtime composes layers, enforces organization security policy, plans and applies changes, verifies parity, manages snapshots, schedules drift checks, and provides MCP relay behavior. A Tauri 2 desktop application gives users one cross-platform tray and management UI; the same runtime remains fully operable through a headless CLI, local service API, and MCP tools.

Version 1 is complete only when all 12 flows in `CONTEXT.md` are implemented. The current Node CommonKit engine and TypeScript `mcp-local-relay` package are reference implementations whose tested behavior must be preserved or deliberately superseded during migration.

## ADR 0005/0006 provider and SkillOpt amendment

This focused amendment supersedes agent packaging, broad native client compilation, and home-file source-management portions of this scope where they conflict with ADR 0005 or ADR 0006. It does not change the product's composition, policy floor, target, reconciliation, credential, relay, snapshot, service, desktop, platform, or release responsibilities.

- Version-pinned desired-state providers compute normalized resources in isolated staging and never mutate managed live targets.
- APM 0.25.0 is the preferred agent-context provider for the initial Claude and Codex path. Native Claude/Codex behavior remains a migration fallback until parity is proven.
- Chezmoi 2.70.4 is the preferred home-configuration provider for semantics that pass the isolated-destination parity suite. CommonKit does not reimplement chezmoi and rejects unsupported destination-dependent, executable, networked, or secret-resolving features.
- CommonKit alone validates ownership and policy, creates content-addressed plans, performs mutations through operation adapters, and owns confirmations, receipts, verification, recovery, and rollback.
- Provider outputs and backups are stored as integrity-checked content-addressed artifacts. Plans bind composed loadout, provider inputs, ownership map, and artifact-set digests. Recovery never re-runs providers.
- CommonKit will not build a package marketplace, agent dependency resolver, package archive/SBOM/scanner, broad native client compiler, or dotfile template/source engine.

SkillOpt is a separate, safety-gated capability workflow above the agent-context provider boundary:

```text
pinned external SkillOpt
  -> isolated train/validation optimization
  -> independent held-out and policy harness
  -> immutable candidate plus evidence
  -> fresh Git/policy-bound human approval
  -> APM compilation
  -> CommonKit named-canary plan/apply/verify
  -> authenticated durable rollback
```

SkillOpt computes candidates; it never mutates active skills, promotes source, compiles agent packages, schedules itself, or mutates a target. Canonical skill source is `.agents/skills/<name>/SKILL.md`; `.claude/skills` is a delivery symlink. Dataset approval, candidate adoption, and merge remain explicit human actions. A Claude `SessionEnd` hook may record freshness only; it must not harvest, evaluate, spend budget, or adopt. Codex has no equivalent hook in v1.

The WIP reference implementation is branch `al-unsoldgroup/skillopt`, commit `fc7de87`, tracked by `USG-48`. It is explicitly not merge-ready. Its stricter partial fixes must be preserved, but integration proceeds by classified, reviewed changes rather than merging the branch wholesale. The six release blockers are: canonical-source enforcement, independent held-out evidence, corpus binding, real cross-platform provider isolation, fresh Git/policy revalidation, and authenticated verified canary rollback. See `docs/plans/skillopt-v1-integration.md`.

The active implementation sequence is recorded in `docs/plans/provider-adapter-implementation.md`. Durable fresh-process recovery is the first gate; APM/chezmoi production integration cannot proceed until a new adapter process can reconstruct every applied operation from durable plans, artifacts, backups, and receipts. SkillOpt cannot graduate until APM provider convergence and durable authenticated canary rollback are proven.

## Problem

Developer capabilities are fragmented across machine-local configuration, repositories, remote hosts, agent runtimes, service managers, credentials, and mutable context databases. Moving to another machine requires manual copying and produces silent drift. Existing synchronization is one-way, host-specific, and hard-coded. The relay has a useful local data plane and status interface, but it is macOS-centric and separate from the desired-state control plane.

CommonKit must provide a safe, explainable, cross-platform way to answer:

- What should this target contain?
- Which layer supplied each value?
- Does the target comply with organization policy?
- What will change before anything is mutated?
- Can the user recover if an apply fails?
- How does the same kit reach a new machine without copying secrets or corrupting mutable databases?

## Users

- Individual developers moving between personal computers and remote hosts.
- Organizations enforcing a security baseline while permitting personal and project customization.
- Coding agents that need a stable, policy-governed capability surface.
- Operators managing headless development targets through SSH or scheduled automation.
- Open-source users installing CommonKit without Unsold-specific infrastructure.

## Success criteria

- A new macOS, Linux, or Windows machine can install CommonKit, authenticate to GitHub, select a loadout, preview, apply, and verify without hand-copying configuration.
- The same composed input produces byte-identical normalized desired state and operation ordering on all supported operating systems.
- No secret value, OAuth state, machine identity, live database, WAL, or session state enters Git, plans, receipts, diagnostics, or logs.
- A later layer cannot weaken organization security policy; violations identify the layer, path, governing rule, and remediation.
- Reapplying an unchanged loadout is a no-op.
- A failed apply restores the previous managed state or produces an explicit recoverable receipt.
- All current CommonKit safety tests and relay behavior tests have Rust equivalents before the Node implementations are retired.
- Installers and updates are signed; native manual validation builds and tests macOS, Linux, and Windows release artifacts.

## Product boundary

### In scope

1. Target initialization.
2. Drift preview and explanation.
3. Safe, confirmed reconciliation.
4. Exact verification.
5. Credential-reference provisioning as a separate action.
6. Transactional recovery and rollback.
7. Multiple targets and five-layer composition.
8. Scheduled read-only drift checks.
9. Integrated MCP relay desired state, lifecycle, and runtime health.
10. Versioned coding-agent adapters.
11. Redacted diagnostic exports.
12. Open-source onboarding, schemas, threat model, manual validation, packaging, and releases.
13. macOS, Linux, and Windows desktop and headless operation.
14. GitHub-backed portable state and encrypted S3-compatible database snapshots.

### Out of scope for v1

- Mobile applications.
- True multi-writer database synchronization or binary SQLite merging.
- Storing secret values in CommonKit or GitHub.
- Silent Git conflict resolution, silent policy weakening, or unattended mutation by default.
- Arbitrary remote URL imports for layers; v1 resolves local files from a Git checkout and pins digests in a lockfile.
- A general-purpose device-management or MDM product.
- Guaranteed support for every Linux distribution or desktop shell; publish a tested support matrix.

## Layered desired-state model

Composition order is fixed:

```text
public base → organization policy → personal kit → project loadout → target overrides
```

Each layer has `schemaVersion`, stable `id`, `kind`, source metadata, and `spec`. Exactly one layer of each kind is allowed in v1; omitted optional layers behave as empty layers.

Merge semantics are schema-defined, never inferred globally:

- Scalars: later value replaces earlier value.
- Maps: recursive merge.
- Ordered argument lists: replace.
- Requirement and denial sets: union.
- Adapters, hooks, plugins, and targets: merge by stable ID.
- Deletion: explicit `$delete` operation, allowed only on schema-approved paths.
- Unknown fields, type changes, duplicate IDs, cycles, and ambiguous ordering fail closed.

Organization security constraints are stored separately from ordinary desired values and are monotonic:

- Denials may only grow.
- Required controls may only become stricter.
- Allowlists may only narrow.
- Minimums and maximums follow typed schema constraints.
- Later layers that attempt weakening are rejected, not silently clamped.

Composition emits a clean normalized document plus a sidecar trace and lockfile. `commonkit explain <json-pointer>` reports the winning value, every contributing layer, merge operation, source revision, and governing policy.

The v1 layer `spec` is a closed CommonKit-owned envelope. Its top-level
vocabulary is published in `schemas/layer.schema.json` and includes
provider-backed `capabilities` such as `capabilities.agentContext`. Unknown
top-level fields fail closed. Provider- or adapter-owned payloads remain nested
inside their named capability, adapter, hook, or plugin envelope and are
validated by that provider's version-pinned contract rather than by a second
CommonKit package-policy language.

## Runtime architecture

```text
GitHub kit repository             S3-compatible snapshot store
          │                                   │
          └──────────────┬────────────────────┘
                         │
              commonkit-core (Rust)
 composition · policy · plan · apply · verify · receipts
         │               │                │
 platform adapters   capability adapters  state adapters
         │               │                │
   macOS/Linux/Win   Claude/Codex/Relay   context-mode
                         │
                 commonkitd (Rust)
        local API · scheduler · events · relay runtime
             │             │              │
          CLI (Rust)   MCP server      Tauri 2 app
```

### Workspace layout

```text
crates/
  commonkit-core/          domain types, composition, policy, planning
  commonkit-config/        schema, parsing, lockfile, provenance
  commonkit-reconcile/     transaction engine, receipts, rollback
  commonkit-platform/      platform abstraction contracts
  commonkit-platform-macos/
  commonkit-platform-linux/
  commonkit-platform-windows/
  commonkit-adapters/      capability adapter SDK and registry
  commonkit-relay/         MCP relay runtime and control contract
  commonkit-snapshots/     consistent export, encryption, object storage
  commonkit-service/       local API, scheduler, event stream
  commonkit-mcp/           agent-facing MCP server
  commonkit-cli/           headless CLI

apps/
  desktop/                 Tauri 2 shell and frontend

packages/
  mcp-local-relay/         legacy TypeScript source during migration

schemas/
  commonkit.schema.json
  layer.schema.json
  receipt.schema.json
  diagnostics.schema.json
```

Crates may be consolidated during technical design when boundaries do not justify separate packages. The public contracts matter more than the initial crate count.

## Interfaces

### Core reconciliation contract

Every adapter implements the semantic equivalent of:

```rust
trait Adapter {
    fn inspect(&self, target: &TargetContext) -> Result<ObservedState>;
    fn plan(&self, desired: &DesiredState, observed: &ObservedState) -> Result<Vec<Operation>>;
    fn apply(&self, operations: &[Operation], transaction: &mut Transaction) -> Result<Receipt>;
    fn verify(&self, desired: &DesiredState, target: &TargetContext) -> Result<Verification>;
}
```

Operations are deterministic, serializable, redacted, attributable to an adapter, and classified by risk. Apply never accepts an unvalidated ad hoc operation list; it consumes a signed or content-addressed plan tied to the exact desired and observed digests.

### CLI

```text
commonkit init
commonkit status
commonkit sync --fetch
commonkit compose
commonkit explain <pointer>
commonkit diff [target]
commonkit apply [target] --plan <id> --yes
commonkit verify [target]
commonkit credentials plan|apply|verify
commonkit snapshot create|list|restore|promote
commonkit rollback <receipt>
commonkit schedule enable|disable|status
commonkit diagnostics export
commonkit relay status|restart|reconcile
```

### Local service API

The Tauri app, CLI when operating as a client, and MCP server call one versioned local control API. Prefer a Unix domain socket on macOS/Linux and a named pipe on Windows. If loopback HTTP is retained, require per-installation authentication and reject non-loopback binding by default.

The API exposes status, composition, plan, apply, verification, credentials, snapshots, rollback, schedules, relay health, events, and diagnostics. Mutating calls accept plan IDs and explicit confirmation metadata.

### MCP tools

Initial surface:

- `commonkit_get_status`
- `commonkit_compose`
- `commonkit_explain`
- `commonkit_plan_sync`
- `commonkit_apply_plan`
- `commonkit_verify`
- `commonkit_snapshot_create`
- `commonkit_snapshot_restore`
- `commonkit_rollback`
- `commonkit_export_diagnostics`

Read operations can be broadly available. Mutation tools require explicit user consent, policy validation, and plan-bound authorization. Tool output is size-bounded and redacted.

## Tauri desktop application

The Tauri 2 application replaces the macOS-only SwiftUI relay status app. It is a system-tray application with an optional full management window.

### Tray

- Overall state: healthy, drifted, blocked, applying, degraded, or offline.
- Git ahead/behind/diverged state.
- Active loadout and target.
- Organization-policy violations.
- Relay and upstream health.
- Last drift check and snapshot.
- Quick actions: fetch, plan, verify, snapshot, open dashboard.

### Management window

- First-run onboarding and GitHub repository selection/creation.
- Layer and target inventory.
- Human-readable plan with provenance and risk.
- Apply confirmation and progress.
- Credential-reference readiness without displaying values.
- Snapshot history, authoritative-writer status, restore, and promotion.
- Relay upstream inventory and health.
- Adapter diagnostics and redacted export.
- Scheduling and update settings.

### Desktop lifecycle

- Single-instance behavior.
- User-controlled autostart.
- Signed auto-updates with visible release notes and rollback guidance.
- Platform-scoped Tauri capabilities with least privilege.
- No unrestricted shell execution from the webview.
- Secrets are obtained through OS credential facilities or external providers and never passed to the frontend.

## MCP relay migration

The Rust relay replaces the TypeScript runtime only after behavioral parity.

### Preserve

- Streamable HTTP downstream endpoint.
- Persistent upstream MCP connections.
- Cached tool discovery and background refresh.
- Hot add, update, enable, disable, remove, and refresh.
- Tool-name localization and list-changed notifications.
- Provider modes such as PostHog CLI mode.
- Health, status, client-config, and generic menu/action models.
- Environment-file references without exposing values.

### Change deliberately

- Replace the incomplete unauthenticated admin surface with the authenticated CommonKit local control API.
- Add atomic bulk validate/reconcile instead of mutating one server before remote validation completes.
- Track `managedBy: commonkit`; preserve unmanaged entries unless explicit prune is planned and approved.
- Separate relay package-version lifecycle from upstream desired state.
- Disable relay self-upgrade when CommonKit manages its version.
- Move the generic tray UI into the Tauri application.
- Prohibit literal secret headers in portable desired state; use provider-neutral secret references.

### Migration gate

Run the existing TypeScript relay and Rust relay against the same black-box contract suite. Retire the TypeScript runtime only when configuration normalization, tool discovery, tool calls, notifications, lifecycle state, failure behavior, and redaction are equivalent or an approved migration explicitly changes the contract.

## GitHub synchronization

Git stores declarative CommonKit content only. `commonkit sync` performs fetch, validation, plan, and explicit apply. It may fast-forward automatically only when the worktree is clean, the source is trusted, the organization floor passes, and the user has enabled that policy. Diverged, dirty, conflicted, or unreviewed changes stop with remediation guidance.

The lockfile pins layer paths, Git commit IDs, schema versions, and content digests. Organization repositories should support a PR-based push workflow. Initial authentication may use the installed GitHub CLI; the desktop onboarding architecture must leave room for a GitHub App/device flow without storing tokens in the kit.

## Credentials

Portable state contains typed references such as `env://`, `file://`, `keychain://`, or provider-specific opaque IDs. Core composition and planning never resolve secret values. A separate credential materializer runs only during `credentials apply`, writes target-local files or OS credential entries with restrictive permissions, and returns redacted receipts.

Provider support is adapter-based. The initial implementation should include environment/file references and one external secret-manager adapter, with provider choice finalized before implementation of flow 5.

## Database snapshots

Each database/corpus declares one authoritative writer. The adapter:

1. Requests a consistent application export or SQLite backup.
2. Excludes rebuildable indexes, caches, WAL/SHM, sessions, and transient state where possible.
3. Runs integrity validation.
4. Adds schema/export version, source identity, parent digest, timestamp, and hashes.
5. Compresses and encrypts client-side.
6. Uploads to S3-compatible object storage.
7. Writes only the descriptor and content hash to Git.

Restore downloads into staging, verifies, decrypts, migrates, rebuilds derived indexes, validates again, stops the owning service, atomically swaps, and preserves the previous database for rollback. Promotion of another machine to authoritative writer is explicit and refuses when unsnapshotted authoritative changes exist.

## Cross-platform execution

Platform adapters own path conventions, permissions, process/service management, autostart, atomic replacement, user identity, and OS credential integration.

- macOS: launchd, Keychain, POSIX permissions, signed/notarized application.
- Linux: systemd user service when available, documented fallback for unsupported init systems, Secret Service when available, AppImage and `.deb` initially.
- Windows: named pipes, Windows services or per-user startup tasks, DPAPI/Credential Manager, ACLs, MSI or NSIS installer.

Remote execution is a transport abstraction. SSH is the initial remote transport for macOS/Linux targets. Windows supports local management in v1; WinRM or SSH-based remote Windows management must be explicitly selected during technical planning and tested before claiming remote Windows support.

## Safety and threat model requirements

- Organization policy cannot be redirected or weakened by later layers.
- Layer and plan digests prevent time-of-check/time-of-use substitution.
- Apply is transactional per adapter and coordinated through a run receipt.
- Target paths are contained under declared roots; symlink escapes and traversal fail closed.
- Plans, logs, events, diagnostics, and UI state pass a central redaction layer.
- Local control calls are authenticated and authorized by capability.
- MCP mutations require explicit consent and cannot bypass desktop/CLI policy.
- Git pulls do not auto-apply untrusted or diverged content.
- Updates and installers are signed; release metadata is integrity checked.
- Snapshot encryption occurs before upload; keys remain outside Git and object storage.
- Diagnostics have a machine-readable schema and an automated secret scanner.

Produce `docs/THREAT-MODEL.md` before the first public release.

## Delivery plan

### Phase 0 — Contracts and parity harness

- Finalize schemas, error codes, redaction contract, adapter contract, receipts, and local API protocol.
- Capture black-box fixtures from the current Node engines.
- Build cross-language parity tests before replacing behavior.

Exit: schemas validate fixtures; parity suite fails against unimplemented Rust interfaces for expected reasons.

### Phase 1 — Rust core and composition

- Implement layer loading, schema-defined merging, policy floor, provenance, lockfile, target inventory, deterministic planning primitives, and explain.
- Port forbidden-path and embedded-secret invariants.

Exit: composition and safety property tests pass identically on all three OS runners.

### Phase 2 — Transactional reconciliation

- Implement inspect, plan, apply, verify, receipts, backups, rollback, and local/SSH executors.
- Implement durable content-addressed provider artifacts, normalized filesystem resources, ownership validation, and fresh-process adapter reconstruction.
- Integrate APM and chezmoi as non-mutating desired-state providers after their isolation contracts pass; preserve current native Claude, Codex, and file behavior as migration fallback.

Exit: flows 2–4 and 6 pass end-to-end; repeat apply is a no-op; injected failures recover.

### Phase 3 — Service, CLI, MCP, and scheduling

- Build `commonkitd`, local authenticated transport, event stream, complete CLI, MCP tools, scheduler, and redacted diagnostics.

Exit: every capability is operable headlessly; mutation paths use plan-bound confirmation.

### Phase 3a — SkillOpt candidate lifecycle

- Integrate pinned external SkillOpt behind a strict, isolated provider boundary.
- Bind reviewed train/validation inputs to skill, campaign, suite, and exact case IDs.
- Evaluate held-out cases and policy only in an independent CommonKit-owned harness.
- Store immutable redacted evidence and candidates; require fresh Git/policy-bound human promotion.
- Compile promoted source through APM, reconcile a named canary through CommonKit, and verify durable authenticated rollback.

Exit: all SkillOpt security gates in `docs/plans/skillopt-v1-integration.md` pass on the supported platform isolation matrix; neither provider nor harness can mutate source or targets.

### Phase 4 — Rust relay parity

- Implement relay runtime, atomic desired-state reconciliation, lifecycle boundary, caches, notifications, provider modes, and health.
- Run dual-runtime contract tests and migrate configuration.

Exit: relay parity gate passes; TypeScript relay is deprecated but retained for one migration release.

### Phase 5 — GitHub, credentials, and snapshots

- Implement onboarding, Git sync and lockfile, credential materializer, encrypted object-store snapshots, restore, and writer promotion.

Exit: a clean machine can reproduce a loadout and restore an optional context corpus without copying a secret or live database through Git.

### Phase 6 — Tauri desktop

- Build onboarding, tray, management window, plan review, status, relay management, snapshots, schedules, diagnostics, and updates.
- Replace the SwiftUI status application.

Exit: desktop acceptance suite passes on macOS, Linux, and Windows.

### Phase 7 — Release readiness

- Complete threat model, support matrix, migration guide, OSS onboarding, packaging, signing, update channels, manual platform validation, and release tooling.

Exit: all 12 v1 flows pass on the declared platform matrix; release artifacts install, update, and uninstall cleanly.

## Verification strategy

- Unit tests for schemas, merge strategies, policy monotonicity, canonicalization, redaction, and path safety.
- Property tests for layer ordering, policy weakening attempts, idempotency, and deterministic plans.
- Contract tests shared by CLI, local API, MCP, desktop, adapters, and both relay runtimes.
- Failure injection at validation, backup, write, lifecycle restart, verification, receipt persistence, and rollback.
- Golden tests for plans, receipts, diagnostics, and cross-platform path rendering.
- Integration tests with temporary local services and mock MCP upstreams.
- Cross-platform Tauri tests for onboarding, tray, confirmation, offline, update, and uninstall flows.
- Security tests for traversal, symlinks, local API authorization, malicious Git content, secret leakage, update tampering, and snapshot corruption.
- Upgrade tests from current CommonKit configuration and `mcp-local-relay` v0.1.x state.

## Release artifacts

- macOS signed/notarized universal application and headless CLI.
- Windows signed installer and headless CLI.
- Linux AppImage, `.deb`, and standalone CLI; add RPM when the tested support matrix requires it.
- Checksums, signatures, SBOM, release notes, schema compatibility declaration, and migration guide.
- Signed Tauri updater metadata and artifacts.

## Acceptance checklist

- [ ] All 12 v1 flows pass their contract suites.
- [ ] macOS, Linux, and Windows local targets pass installation, reconciliation, snapshot, and uninstall tests.
- [ ] Declared remote-target combinations pass the support matrix.
- [ ] Organization policy weakening is rejected with provenance.
- [ ] Plans are deterministic, redacted, and bound to input/observed digests.
- [ ] Apply is idempotent and failure recovery is demonstrated.
- [ ] No secret or live SQLite file enters Git or diagnostics.
- [ ] Snapshot restore is integrity-checked and rollback-safe.
- [ ] Rust relay passes black-box parity and migration tests.
- [ ] Tauri app and headless interfaces expose the same domain state.
- [ ] Installers and updates are signed and verified.
- [ ] Threat model, schemas, onboarding, support matrix, and migration docs are published.
- [ ] SkillOpt receives only bound train/validation data in real isolation; held-out evidence comes only from the independent harness.
- [ ] SkillOpt promotion is bound to fresh Git and policy digests, and named-canary rollback is persisted, authenticated, and verified.

## Defaults still requiring confirmation

These do not block Phase 0 contract work but must be resolved before their owning phase:

GitHub authentication is resolved for v1: CommonKit uses the installed GitHub
CLI as its credential broker and repository-provisioning client. CommonKit does
not store GitHub OAuth credentials in portable state.

1. Snapshot backend: Cloudflare R2 as the default S3-compatible provider, or provider-neutral setup only.
2. External secret manager included in v1.
3. Relay authority: CommonKit-owned subset with explicit prune is recommended.
4. Relay upstream validation: strict for changed endpoints/auth, deferred option for offline targets.
5. Remote Windows transport: OpenSSH, WinRM, both, or local-only v1.
6. Initial Linux packaging/support matrix beyond AppImage and Debian-family packages.

## Implementation start gate

Before feature implementation begins:

- Convert Phase 0 into Linear sub-issues with acceptance criteria and dependency links.
- Create failing contract fixtures for every current Node behavior that must survive.
- Approve the versioned normalized schema, adapter contract, receipt format, and threat-model skeleton.
- Establish macOS, Linux, and Windows manually invoked validation runners and an artifact signing strategy.
- Make no production mutation path available until redaction, plan binding, transaction receipts, and rollback primitives exist.
