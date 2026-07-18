# APM provider resteer: impact and focused contract diff

Status: Proposed for approval  
Tracking: UNS-1274  
Tested provider contract: APM `0.25.0`, tag commit `d73e6ac3645d2b9c5c813095e2e58f020f38f17a`  
Supersedes: Agent packaging and broad native-agent compilation portions of `docs/scopes/commonkit-tauri-v1.md` where they conflict with ADR 0005  
Does not supersede: CommonKit composition, target, policy-floor, reconciliation, credential, relay, snapshot, service, desktop, or platform scope

## Real release verification (2026-07-18)

CommonKit's opt-in compatibility test was exercised with Microsoft's official
`apm-darwin-arm64.tar.gz` release asset. The archive SHA-256 published by the
GitHub release and independently verified before extraction is
`8b46963bf881d1369c1ee30f6099fba54d13379a8860ec190cdcaf50cf4586a9`.
The binary reports `Agent Package Manager (APM) CLI version 0.25.0 (d73e6ac)`.

Observed behavior relevant to the adapter:

- Frozen install reads the committed lockfile from the disposable project.
- Compilation may emit Codex context as root `AGENTS.md`; Claude instructions
  may remain under `.claude/rules/` without a duplicate root `CLAUDE.md`.
- CI audit performs a cache-only install replay. Its temporary directory must
  be outside the project tree; APM refuses an in-tree `TMPDIR`.
- The `.apm/` project tree is a provider input. CommonKit stages it, rejects
  symlinks, and binds its deterministic tree digest into provider inputs.

After checksum verification, run:

```sh
COMMONKIT_APM_025_BIN=/absolute/path/to/apm \
  cargo test -p commonkit-adapters --test apm_provider \
  real_apm_025_release_materializes_only_in_disposable_staging_when_enabled
```

The real-binary gate currently covers macOS arm64. Linux arm64/x86_64, macOS
x86_64, and Windows x86_64 remain release-matrix gates; upstream publishes no
Windows arm64 asset for 0.25.0. Local-path dependency trees outside the
committed project `.apm/` tree remain unsupported until supplied as an
explicit digest-bound source bundle. There is no live-target install fallback.

## Outcome

CommonKit remains the orchestrator and transactional reconciler for complete developer environments. A version-pinned APM CLI is the preferred provider for portable agent context. APM owns package resolution, locking, package policy, compilation formats, package audit, and package provenance. CommonKit owns target policy and every live-target mutation.

Provider execution is a build step, not an apply step:

```text
committed apm.yml + apm.lock.yaml + apm-policy.yml
  -> validate pinned provider and inputs
  -> compile into a private staging root
  -> audit staged output
  -> convert staged/observed differences into CommonKit operations
  -> normal plan + confirmation
  -> CommonKit transaction applies/verifies/rolls back
```

APM must never receive the live managed destination as an install target. APM 0.25.0 supports the required boundary when CommonKit constructs a complete disposable workspace and runs install, compile, and audit with that workspace as the process working directory. CommonKit does not fall back to an in-place APM install.

## Current implementation inventory

Branch: `uns-1274-commonkit-v1-rust-tauri`

Latest committed checkpoint: `c027798 feat: define atomic Rust relay contract [UNS-1274]`

Uncommitted at resteer:

- `CONTEXT.md`: approved agent-context provider terminology and product boundary; retain.
- `docs/adr/0005-use-apm-for-agent-context.md`: approved architecture decision; retain.
- `crates/commonkit-relay/src/lib.rs` and `crates/commonkit-relay/tests/storage.rs`: interrupted secure env-file and relay-cache storage slice; retain but pause. It is uniquely CommonKit runtime work and does not overlap APM.

No Rust agent package manager, marketplace, dependency resolver, archive format, SBOM generator, or broad native client compiler has been implemented. The overlap is concentrated in the legacy Node migration source under `src/` and its tests.

## Retain, adapt, defer, remove

| Classification | Current area | Decision |
|---|---|---|
| Retain | `commonkit-contracts`, `commonkit-config`, and `commonkit-core` composition, provenance, canonical digests, policy floor, deterministic plans | These are provider-neutral CommonKit responsibilities. |
| Retain | `commonkit-reconcile` plan validation, durable receipts, recovery, verification, rollback | APM output must enter this machinery. |
| Retain | `commonkit-adapters::FileAdapter` capability-rooted writes, backups, verification, rollback | It is the initial live-target writer for provider-produced files. |
| Retain | `commonkit-service`, CLI, CommonKit MCP controls, relay, platform paths, scheduling, diagnostics | These remain CommonKit control/runtime planes. |
| Retain paused | Interrupted relay storage changes | Complete after the APM provider boundary is stable. |
| Adapt | Normalized desired-state schema | Add an `agentContext` provider selection and content-addressed provider inputs. |
| Adapt | `CommonKitLock` | Reference provider/version/input digests and selected targets; do not copy the APM package graph. |
| Adapt | Plan binding and apply preflight | Add an explicit provider-input aggregate digest and re-hash inputs immediately before mutation. |
| Adapt | Receipt | Record redacted provider provenance and audit result digest. |
| Adapt | Adapter layer | Add a provider interface that stages/audits and emits deterministic intents; keep mutation in reconciliation adapters. |
| Adapt | Relay desired-state planning | Translate documented resolved APM MCP declarations into CommonKit relay desired state, then give APM the stable local client endpoint. |
| Preserve for migration | Legacy native Claude/Codex file, plugin, hook, and verification behavior under `src/` | Expose later as `native` fallback; do not delete until the APM path proves parity. |
| Defer | New native compilers for Copilot, Cursor, Gemini, OpenCode, Windsurf, Kiro, or future clients | APM owns this expansion path. |
| Stop/remove if introduced | CommonKit marketplace, transitive package resolver, package archives, package scanner, package SBOM, replacement manifest/lockfile | None exists in the Rust implementation today. |

## Focused desired-state diff

The composed shape gains one provider-backed capability. Paths remain relative portable source paths and are resolved only inside the trusted Git checkout.

```yaml
capabilities:
  agentContext:
    provider: apm
    version: 0.25.0
    manifest: ./apm.yml
    lockfile: ./apm.lock.yaml
    policy: ./apm-policy.yml
    targets:
      - claude
      - codex
```

Minimum Rust contract:

```rust
enum AgentContextProviderKind {
    Apm,
    Native,
}

struct AgentContextCapability {
    provider: AgentContextProviderKind,
    provider_version: ProviderVersion,
    manifest: PortableSourcePath,
    lockfile: PortableSourcePath,
    package_policy: Option<PortableSourcePath>,
    targets: BTreeSet<AgentClientTarget>,
}
```

Rules:

- `provider = apm` requires an exact supported version, manifest, lockfile, and at least one target.
- `provider = native` is migration-only and uses the current native adapter configuration rather than APM files.
- Unknown providers, targets, fields, or non-relative paths fail closed.
- Layer composition may select/override ordinary provider values, but organization target policy remains a non-overridable floor.
- Package restrictions are supplied to APM through `apm-policy.yml`; CommonKit must not invent a parallel package-policy schema.

## Focused lockfile diff

Add provider input references to `CommonKitLock`:

```yaml
inputs:
  agentContext:
    provider: apm
    providerVersion: 0.25.0
    manifestDigest: sha256:...
    lockDigest: sha256:...
    packagePolicyDigest: sha256:...
    targets:
      - claude
      - codex
```

The entry contains no resolved dependency nodes, package files, source revisions, secret values, executable path, machine identity, or provider stdout/stderr. Those remain owned by APM or local runtime state.

The aggregate provider-input digest is domain-separated and includes:

- provider kind and exact version;
- manifest digest;
- APM lockfile digest;
- package-policy digest;
- ordered selected targets;
- relevant CommonKit target-policy digest;
- provider adapter contract version.

This aggregate is part of normalized desired state, so `Plan.desired_digest` also changes when a provider input changes. Add the aggregate as `Plan.inputs_digest` as a separately auditable preflight binding; mirror it into the receipt and include it in plan/receipt semantic hashes. Apply preflight must independently re-hash the three files and validate the executable version before the first operation; a mismatch rejects the plan as stale.

## Provider API diff

Add a provider boundary in `commonkit-adapters` without coupling Rust to APM Python modules:

```rust
trait AgentContextProvider {
    fn kind(&self) -> AgentContextProviderKind;
    fn validate(
        &self,
        request: &ProviderRequest,
    ) -> Result<ValidatedProviderInputs, ProviderError>;
    fn stage(
        &self,
        inputs: &ValidatedProviderInputs,
        staging: &CapabilityRoot,
    ) -> Result<StagedAgentContext, ProviderError>;
    fn audit(
        &self,
        staged: &StagedAgentContext,
    ) -> Result<ProviderAudit, ProviderError>;
    fn inspect(
        &self,
        target: &TargetContext,
        ownership: &OwnershipRecord,
    ) -> Result<ObservedAgentContext, ProviderError>;
    fn plan(
        &self,
        staged: &StagedAgentContext,
        observed: &ObservedAgentContext,
        target_policy: &TargetPolicy,
    ) -> Result<Vec<FileIntent>, ProviderError>;
    fn verify(
        &self,
        expected: &StagedAgentContext,
        target: &TargetContext,
    ) -> Result<ProviderVerification, ProviderError>;
}
```

The provider has no `apply`, `install`, `remove`, `restart`, or arbitrary-command method. `plan` emits deterministic CommonKit intents. The existing reconciliation adapter turns those intents into content-addressed `Operation`s and exclusively owns writes, lifecycle changes, backups, and rollback.

Before the spike is complete, `FileAdapter` must also support deterministic delete intents, explicit file modes, a durable managed-file ownership manifest, and recovery from persisted operation payloads. Its current in-memory registration is adequate for the existing file tracer but cannot reconstruct a staged provider deployment after process restart.

`ApmCliProvider` invokes only a versioned allowlist of commands with fixed flags. It uses an explicit executable path, a scrubbed environment, bounded stdout/stderr, timeouts, no shell, and a capability-rooted working/staging directory. Provider output is secret-scanned before it can become an operation or diagnostic.

For APM 0.25.0, the fixed command sequence is:

```text
apm --version
apm install --frozen --target claude,codex
apm compile --target claude,codex
apm audit --ci --policy <absolute-staged-apm-policy.yml> --no-fail-fast --format json
```

All commands except `--version` run with `cwd` set to the disposable staging workspace. CommonKit projects only allowlisted committed inputs into that workspace. Out-of-root local dependencies and symlinks fail closed until a projection contract is implemented.

`apm --version` must contain exact semver `0.25.0` after the literal ` version ` marker. A release build may append a short commit identifier; prerelease, `unknown`, missing, or mismatched versions fail with remediation.

`install --frozen` is lockfile-only but is not a complete content-integrity or offline guarantee. A cold machine may still require package material. The CI audit is mandatory and exit code `0` is the only passing result.

The runner forbids arbitrary provider arguments and scrubs APM bypass environment variables. In particular it never permits `--no-policy`, `--force`, `--trust-transitive-mcp`, `--allow-insecure`, `--allow-insecure-host`, `--no-audit`, `--audit off`, `--no-drift`, `--strip`, `--skip-verify`, `APM_POLICY_DISABLE`, or `APM_ALLOW_PROTOCOL_FALLBACK`.

## Plan and receipt changes

### Plan binding

Add `inputsDigest` to `Plan`, `PlanDraft`, plan semantic hashing, service validation, and `RunReceipt`. The planner must prove:

```text
desiredDigest = digest(composed desired state including ProviderInputLock)
inputsDigest  = digest(provider/version/input files/targets/adapter version)
policyDigest  = digest(CommonKit target policy + referenced APM package-policy decision)
```

Before mutation, `prepare` revalidates:

- exact provider executable version;
- manifest, APM lockfile, and package-policy digests;
- target set and provider adapter version;
- staged-output digest and audit result;
- live observed digest used by the plan.

Any mismatch yields a stable stale-plan/provider-input error before a target file changes.

### Receipt provenance

Add an optional, ordered `providerProvenance` collection to `RunReceipt`:

```rust
struct ProviderProvenance {
    capability_id: StableId,
    provider: AgentContextProviderKind,
    provider_version: ProviderVersion,
    manifest_digest: Sha256Digest,
    lock_digest: Sha256Digest,
    package_policy_digest: Option<Sha256Digest>,
    audit_result_digest: Sha256Digest,
    staged_output_digest: Sha256Digest,
    managed_files_digest: Sha256Digest,
}
```

Provider provenance participates in the receipt hash chain and central secret scanning. It does not contain package names, source URLs, machine paths, command output, or secret readiness details.

## Policy boundary

The pre-mutation gate is ordered:

1. Validate CommonKit organization floor and composed target policy.
2. Validate provider/version and address provider files by digest.
3. Invoke the pinned APM package-policy validation/audit against staging.
4. Translate staged output to deterministic intents.
5. Validate every intent against CommonKit target policy.
6. Build and present the CommonKit plan.
7. Revalidate all digests and policy immediately before apply.

APM flags or manifest fields cannot disable steps 1, 5, 6, or 7. Provider commands with bypass, force, ignore-policy, or in-place-install semantics are absent from the allowlist.

`commonkit explain` should expose two adjacent decisions for agent context:

- package plane: the APM policy decision and provider input provenance;
- target plane: the CommonKit rule governing each destination, permission, service, credential reference, relay exposure, and mutation authorization.

## MCP boundary spike

The preferred translation is a documented, resolved APM MCP representation:

```text
resolved APM MCP declarations
  -> narrow translator
  -> CommonKit-owned relay desired state
  -> persistent authenticated upstreams
  -> stable local relay endpoint
  -> APM staged client configuration
```

APM 0.25.0 does not expose a documented machine-readable resolved MCP command. The spike may parse only documented manifest fields and the documented lockfile fields `mcp_configs`, `mcp_target_servers`, and `mcp_config_provenance`. It must not import APM Python internals.

Because APM documentation notes that some clients materialize environment placeholders during install, staged native MCP config and lock baselines must be treated as potentially secret-bearing. The translator accepts only a strict allowlist of portable declaration fields and requires secret references. Registry-backed MCP entries that cannot be represented without internal APM state fail with `resolved_mcp_interface_unsupported`; relay reconciliation remains independent until APM provides a supported export.

## Vertical TDD spike

Each item is one red-green tracer bullet through public interfaces; do not write the suite horizontally.

1. Exact APM version is validated and appears in the generated lock input.
2. Manifest, APM lockfile, and package policy are addressed by digest.
3. Claude output is produced wholly under a private staging root.
4. Codex output is produced wholly under the same staging contract.
5. APM audit failure blocks before any live-target operation exists.
6. Staged-versus-observed comparison emits deterministic redacted operations.
7. Changing any provider input makes an existing plan stale at preflight.
8. Transactional apply creates a provider-provenance receipt and repeat apply is a no-op.
9. A managed-file hand edit is detected by inspect/verify.
10. Injected mid-apply failure restores the prior managed state or persists a recoverable receipt.
11. CommonKit target-policy denial cannot be bypassed by APM configuration or flags.
12. Native Claude/Codex migration behavior remains green.

The spike is successful only when all twelve pass for one local target with Claude and Codex. Remote execution, additional APM targets, and relay translation follow only after this boundary is proven.

## Focused scope-spec update after approval

Do not rewrite the v1 scope. Add a short “ADR 0005 provider amendment” that:

- replaces “port current Claude, Codex, plugin, hook, file/tree behavior” as the primary Phase 2 expansion path with the APM spike above;
- retains native Claude/Codex behavior as migration fallback;
- adds `commonkit-apm` or an equivalent module under `commonkit-adapters`;
- adds provider input references to lockfile and receipt contracts;
- places the APM spike before continued relay/snapshot/desktop breadth;
- removes future native client compiler expansion from CommonKit scope;
- keeps every existing non-agent-context phase and acceptance criterion.

## Pinned official references

- [APM v0.25.0 release](https://github.com/microsoft/apm/releases/tag/v0.25.0)
- [Installation](https://github.com/microsoft/apm/blob/v0.25.0/docs/src/content/docs/getting-started/installation.md)
- [`install` command and `--root`/`--frozen`](https://github.com/microsoft/apm/blob/v0.25.0/docs/src/content/docs/reference/cli/install.md)
- [`compile` command](https://github.com/microsoft/apm/blob/v0.25.0/docs/src/content/docs/reference/cli/compile.md)
- [`audit` command and CI exit semantics](https://github.com/microsoft/apm/blob/v0.25.0/docs/src/content/docs/reference/cli/audit.md)
- [Manifest schema](https://github.com/microsoft/apm/blob/v0.25.0/docs/src/content/docs/reference/manifest-schema.md)
- [Lockfile specification](https://github.com/microsoft/apm/blob/v0.25.0/docs/src/content/docs/reference/lockfile-spec.md)
- [Governance and bypass contract](https://github.com/microsoft/apm/blob/v0.25.0/docs/src/content/docs/enterprise/governance-guide.md)
- [MCP CLI boundary](https://github.com/microsoft/apm/blob/v0.25.0/docs/src/content/docs/reference/cli/mcp.md)

Remaining spike gaps are explicit engineering limits: projection of out-of-root local dependencies, registry MCP entries without sufficient documented portable fields, and secret-materializing client output. None permits in-place installation or Python-internal coupling.
