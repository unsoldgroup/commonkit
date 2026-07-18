# Provider/adapter implementation plan

Status: Approved and active  
Tracking: UNS-1274  
Decisions: ADR 0005 and ADR 0006  
Detailed APM contract: `docs/plans/apm-resteer-impact.md`

## Architecture

```text
APM 0.25.0       ─┐
chezmoi 2.70.4   ─┼─> normalized desired resources
native providers ─┘              |
                    ownership + policy validation
                                   |
                    content-addressed CommonKit plan
                                   |
               filesystem/service/relay/etc. adapters
                                   |
                   receipts + verification + rollback
```

Providers compute. Adapters mutate. Recovery consumes only durable plans, artifact manifests, artifacts, backup records, and receipts; it never re-runs a provider, downloads content, evaluates templates, or resolves credentials.

SkillOpt is adjacent to, not inside, this desired-target provider fan-in. It computes immutable skill candidates under an independent evidence/policy harness. After human promotion into Git, APM materializes the approved source through this pipeline. See `docs/plans/skillopt-v1-integration.md`.

## Cold inventory

Branch: `uns-1274-commonkit-v1-rust-tauri`

### Retain

- Layered composition, provenance, organization policy floor, canonical operation/plan identities, dependency ordering, reconciliation state machine, receipt hash chain, reverse rollback, and interrupted-run recovery.
- Capability-rooted filesystem access, preimage enforcement, service/CLI/MCP control, relay, schedules, diagnostics, Git sync, credentials, snapshots, desktop, and platform boundaries.
- Native Node Claude/Codex/file behavior until provider parity is demonstrated.
- Paused relay env/cache work in the current worktree; it is non-overlapping and green.

### Adapt

- Replace byte-only provider-facing `FileIntent` with canonical normalized resources while preserving a native compatibility constructor.
- Persist operation mutation payload references instead of holding them only in `FileAdapter.files`.
- Add a content-addressed `PlanStore`, `ArtifactStore`, materialized-set manifests, and run/operation-bound backup records.
- Bind provider inputs, ownership, artifacts, and composed loadout into plans and receipts.
- Add a provider interface with no live mutation method.
- Add directories, safe symlinks, removals, explicit type changes, and portable permissions.
- Make service plan registration durable rather than memory-only.

### Defer or stop

- Defer physical crate splits, an async adapter rewrite, unsupported chezmoi types, extra APM clients, and unrelated remote-executor refactors.
- Stop adding a CommonKit package marketplace/resolver/archive/scanner/SBOM, broad native agent compiler, or Rust dotfile engine.
- Never create live-target `chezmoi apply` or `apm install` operations.
- Retire legacy direct package mutation only after provider parity; do not delete it in this resteer.

## Proven restart-recovery gap

The RED public-interface test `a_fresh_process_recovers_an_applied_file_without_reconstructing_the_provider` uses the real `FileAdapter`, `ReceiptStore`, `ReceiptJournal`, and `Reconciler`:

1. Process A plans, prepares, and applies `before -> after`.
2. The receipt durably records the operation as applied.
3. A fresh adapter opens the same target and state roots.
4. `recover_run` receives only the durable plan and receipt.
5. Expected: exact `before` bytes and terminal `RolledBack`.

Current evidence: it returns `RollbackFailed` because the mutation payload existed only in process A's in-memory map. This test stays red until the artifact boundary is implemented.

## Durable contracts

### Provider

```rust
trait DesiredStateProvider {
    fn id(&self) -> &StableId;
    fn inspect_inputs(&self, context: &ProviderContext)
        -> Result<ProviderInputs, ProviderFailure>;
    fn materialize(
        &self,
        context: &ProviderContext,
        workspace: &ProviderWorkspace,
        artifacts: &ArtifactStore,
    ) -> Result<MaterializedState, ProviderFailure>;
}
```

`ProviderContext` contains serializable target identity, platform facts, composed policy, target roots, and read-only observed facts. It exposes no writable live destination. CLI providers receive a private staging capability, fixed command allowlist, scrubbed environment, bounded output, timeout, and cancellation.

`ProviderInputs` contains provider ID, exact version, adapter contract version, source/config/manifest/lock/policy digests, and declared features. `MaterializedState` contains normalized resources, provenance, unsupported side effects, and a deterministic digest.

### Artifacts

```rust
struct ContentReference {
    digest: Sha256Digest,
    bytes: u64,
    sensitivity: ContentSensitivity,
}

trait ArtifactStore {
    fn put(&self, bytes: &[u8], sensitivity: ContentSensitivity)
        -> Result<ContentReference, ArtifactError>;
    fn load(&self, reference: &ContentReference)
        -> Result<Vec<u8>, ArtifactError>;
    fn persist_set(&self, set: &MaterializedState)
        -> Result<MaterializedSetReference, ArtifactError>;
    fn load_set(&self, reference: &MaterializedSetReference)
        -> Result<MaterializedState, ArtifactError>;
}
```

Writes are immutable under a private root. Filename, length, canonical manifest, and bytes are verified on load. Secret plaintext is not a portable artifact; target-local sensitive artifacts must be encrypted/local-only and absent from plans, receipts, diagnostics, and Git.

### Normalized filesystem resources

```rust
enum FilesystemIntent {
    File { path, content: ContentReference, mode, expected_before },
    Directory { path, mode, exact },
    Symlink { path, target: SafeSymlinkTarget, expected_before },
    Remove { path, expected_before },
}
```

The durable form is canonical serde data with no absolute host/staging paths. Resource digests include type and metadata. Removals and exact-directory deletions are at least high risk and explicitly confirmed. Windows ACL/read-only normalization is separate from Unix mode bits.

### Plan bindings

```rust
struct PlanBindings {
    composed_loadout_digest: Sha256Digest,
    provider_inputs_digest: Sha256Digest,
    ownership_map_digest: Sha256Digest,
    artifact_set_digest: Sha256Digest,
}
```

Filesystem operations contain typed mutation-payload references whose digests participate in operation identity. `Plan`, `PlanDraft`, semantic hashing, schemas, service validation, durable `PlanStore`, and receipts carry required bindings. Any provider/version/input, ownership, artifact, observation, target, or policy change invalidates the plan before mutation.

### Backups and recovery

Prepare stores an immutable preimage artifact and per-run/operation record with resource type, metadata, before digest, and artifact reference. Rollback validates the record and bytes. Missing or mismatched operations/artifacts/backups fail recovery explicitly and never guess or re-run providers.

Fresh adapters resolve typed payload references from the artifact store. Receipt bindings must match plan and artifact set before recovery.

## Ownership validation

All provider sets form one canonical map: `normalized target path -> provider -> provenance -> resource type`.

A pure pre-plan pass rejects identical path claims, exact-directory ancestry conflicts, type conflicts, cross-provider removals, claims outside declared roots, protected CommonKit state/control paths, and target-platform normalization/case-fold collisions. There is no provider precedence or last-writer-wins byte merge.

## Chezmoi 2.70.4 isolation spike

Pinned release: `v2.70.4`, commit `6458368`.

Every command uses explicit source, config, destination, cache, persistent-state, working-tree, no-TTY/no-pager/no-color, disabled external refresh, and file mode. CommonKit never invokes `init`, `update`, Git commands, or live apply.

Candidate staging sequence inside a private OS sandbox:

```text
chezmoi <isolated flags> apply --force --exclude=scripts
chezmoi <isolated flags> managed --format=json --path-style=all
chezmoi <isolated flags> dump --format=json
chezmoi <isolated flags> archive --format=tar
chezmoi <isolated flags> verify --exclude=scripts
```

Preflight rejects scripts, externals, command hooks, arbitrary-process templates, host/network inspection, secret/keyring access, undeclared local paths, and unsafe symlinks. Excluding scripts is defense in depth, not silent omission.

Chezmoi defines target state from source, config, and current destination; it has no documented “compute for live A, emit to staging B” flag. Production support is therefore limited to semantics whose staged result matches read-only live-destination computation.

### Exact fixture matrix

1. Regular/empty/executable/private/read-only/combined-mode files.
2. Ordinary/empty/private/read-only directories.
3. Exact directories with unmanaged children and nested ancestry.
4. Safe relative symlinks plus absolute/traversal/templated/empty rejection.
5. `remove_` and `.chezmoiremove` across resource types and absent targets.
6. Fixed OS/architecture/hostname/config templates.
7. Plain/negated/nested/conditional `.chezmoiignore`.
8. `create_` against absent and differing observed targets.
9. `modify_` and modify-template rejection.
10. Built-in age encryption with explicit identity and no prompt/external command.
11. `.chezmoi.destDir` and `.chezmoi.targetFile` mismatch/rejection.
12. `lstat`, `stat`, `glob`, `findExecutable`, and observed-host rejection.
13. Every script phase and `.chezmoiscripts` rejection.
14. URL/Git/filter externals rejection before process/network activity.
15. Process/environment/network template-function rejection.
16. Generic/provider secret-function rejection before evaluation.
17. Two clean runs yielding identical canonical resources and digest.

`create_`, exact removals, and dump/archive translation graduate only with parity evidence. `modify_`, destination-path variables, host-observing functions, scripts, externals, process/network functions, and secret functions are denied in v1. No mismatch permits live apply.

If CommonKit redistributes chezmoi, its complete MIT notice must ship in `THIRD_PARTY_NOTICES` and every platform artifact.

## APM convergence

APM 0.25.0 uses the same inputs, materialized sets, ownership, bindings, artifacts, and adapters. Frozen install, compile, and CI audit occur in a disposable workspace. See the detailed APM plan for fixed commands, policy bypass denial, MCP limitations, and its vertical acceptance slices.

## Implementation order

1. Keep the real fresh-process recovery test red.
2. Implement immutable artifacts and materialized-set contracts.
3. Bind typed artifact payload references into operation identity.
4. Implement immutable backup records and fresh adapter reconstruction; turn recovery green.
5. Add durable plan storage and daemon restart recovery.
6. Add normalized file/directory/symlink/remove resources and exact rollback.
7. Add provider inputs/provenance, PlanBindings, receipt binding, and stale-plan rejection.
8. Add ownership validation and protected-root/platform collision rules.
9. Execute the chezmoi fixture spike and classify every case.
10. Implement only the proven chezmoi subset.
11. Implement APM for local Claude and Codex on the same pipeline.
12. Complete the SkillOpt candidate lifecycle gates, then feed only human-promoted Git source into APM and named-canary reconciliation.
13. Converge relay declarations/client config and preserve native fallbacks.
14. Resume Git sync, credentials, snapshots, desktop, packaging, and platforms.
15. Run adversarial review, failure injection, cross-platform, migration, threat-model, and release audits.

## Completion evidence

- Existing core/control/relay suites remain green.
- Fresh-process recovery restores exact bytes, types, modes, symlinks, and removals without provider execution.
- Artifacts, plans, receipts, and backups are integrity-checked.
- Provider/input/ownership/artifact changes reject stale plans.
- Provider collisions fail before plan creation.
- Chezmoi and APM cannot mutate live targets during materialization.
- Unsupported chezmoi semantics fail with source-linked remediation.
- Repeated supported materialization is deterministic.
- Plans contain semantic operations, never provider apply commands.
- Native behavior remains selectable.
- No secret plaintext enters portable state or observability.
- All v1 flows and platform artifacts pass the final release matrix.

## Official references

- [chezmoi v2.70.4](https://github.com/twpayne/chezmoi/releases/tag/v2.70.4)
- [chezmoi concepts](https://www.chezmoi.io/reference/concepts/)
- [chezmoi global flags](https://www.chezmoi.io/reference/command-line-flags/global/)
- [chezmoi target types](https://www.chezmoi.io/reference/target-types/)
- [chezmoi template functions](https://www.chezmoi.io/reference/templates/functions/)
- [APM v0.25.0](https://github.com/microsoft/apm/releases/tag/v0.25.0)
