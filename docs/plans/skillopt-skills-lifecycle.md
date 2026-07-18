# Skill optimization and continuous improvement plan

Status: Proposed
Tracking: USG-48
Depends on: UNS-1274
Optional integrations: USG-46, USG-47

## Outcome

CommonKit manages skills as reviewable desired state and can continuously produce evidence-backed improvement candidates without allowing an optimizer, model, scheduled job, or target agent to mutate active skills directly.

Microsoft SkillOpt is the initial optimization engine. CommonKit does not reimplement its rollout, reflection, edit, or validation algorithms. SkillOpt computes candidates in an isolated workspace; CommonKit owns source selection, policy, durable artifacts, review, promotion, distribution, verification, and rollback.

SkillOpt is installed outside the Rust runtime in a dedicated, version-pinned `uv` tool environment or container. CommonKit treats its CLI, JSON report, and staged proposal files as an external protocol. No SkillOpt Python source or dependencies are vendored into CommonKit.

```text
curated evaluations + approved, redacted session evidence
                         |
                 SkillOpt provider
       rollout -> reflect -> edit -> validate
                         |
             immutable candidate bundle
                         |
       CommonKit policy and regression gates
                         |
                  human approval
                         |
             reviewed source repository
                         |
               APM provider compile
                         |
        CommonKit plan -> apply -> verify
```

## Architectural fit

This work targets the Rust architecture on `uns-1274-commonkit-v1-rust-tauri`, not the legacy Node synchronizer.

- `commonkit-core` owns skill lifecycle state, policy decisions, promotion plans, and stable failure codes.
- `commonkit-contracts` owns canonical candidate, evaluation, evidence, and promotion receipts.
- `commonkit-adapters` owns an isolated SkillOpt provider that computes but never mutates a live target or source checkout.
- `commonkit-reconcile` owns promotion and deployment transactions using content-addressed plans and artifacts.
- `commonkit-service` owns local lifecycle APIs, schedules, events, and restart recovery.
- `commonkit-cli` exposes the headless workflow.
- `commonkit-mcp` exposes read and proposal operations; promotion remains confirmation-bound.
- USG-46's execution service may run expensive or remote evaluation tasks after the local vertical slice works.
- USG-47 may render candidate diffs, scores, provenance, approvals, and receipts. It does not own lifecycle state.

SkillOpt is not an agent-context packaging replacement. APM remains the preferred provider for compiling reviewed agent context for Claude and Codex. CommonKit promotes a candidate into the Git-owned skill source; APM then compiles that accepted source through the existing desired-state provider pipeline.

## Product boundary

### In scope for v1

1. Inventory Git-owned skills and identify their APM package/source provenance.
2. Define explicit evaluation suites for a selected `SKILL.md`.
3. Import manually reviewed, redacted failure evidence.
4. Run SkillOpt in an isolated, bounded workspace pinned to an exact version.
5. Persist baseline and candidate scores, trajectories, diffs, and provenance as immutable artifacts.
6. Reject candidates that fail validation, policy, portability, or security gates.
7. Review and explicitly approve one candidate.
8. Produce a source patch and promotion plan bound to the exact source revision and candidate digest.
9. Commit accepted source through the normal Git review workflow.
10. Compile with APM, reconcile through CommonKit, verify a canary loadout, and roll back through receipts.
11. Schedule read-only discovery and candidate generation; never schedule unattended adoption by default.

### Deferred

- Automatic extraction from every local agent transcript.
- Automatic promotion or Git commits.
- Optimizing scripts, binaries, MCP definitions, hooks, or multiple skills in one run.
- Organization-wide learning from unreviewed user sessions.
- Synthetic dreaming, multi-skill credit assignment, and unconstrained optimizer arguments.
- Replacing APM's package, lock, audit, or compilation responsibilities.
- A CommonKit-specific optimizer algorithm.

## User flows

### 1. Inventory and readiness

The user lists skills with source, active/library state, target clients, current digest, evaluation coverage, latest accepted score, and last optimization status. CommonKit reports why a skill is not optimizable, such as no suite, generated-only source, unsupported content, or ambiguous provenance.

### 2. Establish a baseline

The user selects one instruction-only skill and evaluation suite. CommonKit runs the current source through the pinned target harness, records per-case results and aggregate metrics, and persists a baseline receipt. Repeated runs with the same declared inputs are comparable; model or harness drift is shown explicitly.

### 3. Add evidence

The user imports a failed task or a supported session excerpt. CommonKit previews the exact redacted material that may leave the machine, rejects secret-like or out-of-scope content, and records consent, source, retention, and digest. Evidence is not automatically treated as a held-out evaluation case.

### 4. Generate a candidate

CommonKit creates an immutable optimization manifest and invokes the pinned SkillOpt provider in a private staging workspace. The provider can read only declared skill, training cases, approved evidence, preferences, and credential references. It writes only staged outputs. Cancellation, timeout, output limits, and cost limits are enforced.

### 5. Gate and review

CommonKit evaluates the candidate against held-out cases and shows the skill diff, aggregate and per-case deltas, rejected edits, model/harness provenance, cost, policy findings, and uncertainty. A candidate cannot advance if any mandatory gate fails.

### 6. Promote

Explicit approval creates a promotion plan bound to the candidate digest, source repository revision, original skill digest, suite digest, policy digest, and approval identity. Stale source or policy invalidates the plan. Apply creates a patch in the source worktree; it does not overwrite installed or generated skills.

### 7. Distribute and verify

After normal Git review, APM compiles the accepted source. CommonKit produces its ordinary target reconciliation plan, applies first to a selected canary loadout, verifies exact target state, and records the candidate lineage in the receipt.

### 8. Reject or roll back

Rejected candidates remain immutable audit artifacts subject to retention policy. A post-promotion regression rolls back the CommonKit deployment receipt and reverts the source change through Git; the rejected candidate digest is added to the optimization history so it is not proposed again unchanged.

### 9. Continuous operation

A read-only schedule identifies stale baselines, repeated failures, and eligible skills. Candidate generation may run on a schedule when cost and evidence policies allow. Adoption always remains a separate confirmed operation in v1.

Claude's asynchronous `SessionEnd` hook is treated only as a freshness signal. It appends `<UTC-RFC3339>\t<absolute-project-path>` to `~/.skillopt-sleep/session-end.log`; CommonKit may use matching markers to decide whether discovery work is worthwhile, but the marker reader cannot open transcripts, invoke a model, run evaluations, or modify a skill. Codex has no automatic session-end hook. Harvesting either client's sessions is a separate, explicit repository-scoped job whose output must be reviewed before entering a replay dataset.

Opportunity routing is per skill and corpus: recurring failures for clause extraction can launch only the clause-extraction campaign, never a general debugging skill. Automation may mark activity, harvest candidate examples, evaluate, stage a candidate, and prepare a report or draft PR. Dataset approval, instruction review, adoption, and merge remain manual.

## Desired-state contract

Add an optional skill-optimization capability to a CommonKit layer:

```yaml
capabilities:
  skillOptimization:
    provider: skillopt
    providerVersion: 0.2.0
    adoption: review_required
    sourceRoot: ./skills
    stateRoot: ./.commonkit/skillopt
    defaultHarness: codex_exec
    evidencePolicy: ./policies/skill-evidence.yml
    evaluationPolicy: ./policies/skill-evaluation.yml
    schedules:
      discovery: "0 3 * * *"
      candidateGeneration: disabled
```

Rules:

- Exact provider version is required and participates in every run digest.
- `adoption` is `review_required` in v1 and cannot be weakened by later layers.
- Source and policy paths are portable, relative, and inside the trusted repository.
- Secret values and executable optimizer arguments are forbidden.
- Organization policy can deny evidence sources, backends, models, schedules, or outbound providers.
- A skill must resolve to one canonical Git-owned source. Generated APM output and live target paths are never valid promotion sources.

### External provider compatibility lock

```yaml
skillOpt:
  providerVersion: 0.2.0
  adapterContract: skillopt-sleep-v1
  source: pypi
  packageDigest: sha256:...
  capabilities:
    - reviewed-tasks
    - staged-skill
    - json-report
```

The lock is part of the optimization manifest and candidate receipt. Compatibility requires an exact match of provider version, source/package digest, adapter-contract version, and required capabilities. Unknown versions or output fields fail with `unsupported SkillOpt provider version`; the previous pinned environment remains usable.

The adapter exposes only the allowlisted `skillopt-sleep run` path with reviewed tasks, explicit target skill, bounded task/edit counts, a configured backend/model, and `--json`. It never exposes or invokes `adopt`, `schedule`, `unschedule`, or `--auto-adopt`.

Provider management commands are explicit:

```text
commonkit skills provider check
commonkit skills provider upgrade --version <version> --package-digest <sha256>
```

Upgrade creates a disposable environment, verifies package identity, runs synthetic adapter-contract fixtures, proves the source skill was not mutated, validates JSON and staged-file compatibility, replays a fixed behavioral corpus against old and new environments, and emits an upgrade report. Updating the provider lock is a separate confirmed operation.

## Durable contracts

### Skill descriptor

```rust
struct SkillDescriptor {
    id: StableId,
    source_path: PortableSourcePath,
    source_digest: Sha256Digest,
    package: Option<StableId>,
    targets: BTreeSet<AgentClientTarget>,
    lifecycle: SkillLifecycle,
}

enum SkillLifecycle { Active, Library, Experimental, Retired }
```

Only a single instruction document is optimizable in v1. Referenced scripts and assets may be present for execution, but SkillOpt cannot edit them.

### Evaluation suite

```rust
struct SkillEvaluationSuite {
    id: StableId,
    skill_id: StableId,
    schema_version: SchemaVersion,
    train_cases: ContentReference,
    validation_cases: ContentReference,
    held_out_cases: ContentReference,
    rubric: ContentReference,
    harness: HarnessLock,
    metric: MetricDefinition,
}
```

Train, validation, and held-out identities are disjoint and digest-bound. Held-out contents are unavailable to the optimizer. The receipt records exact model, harness, environment, seed, case manifests, and scoring implementation.

### Evidence envelope

```rust
struct EvidenceEnvelope {
    id: StableId,
    skill_id: StableId,
    source_kind: EvidenceSourceKind,
    content: ContentReference,
    redaction_report: ContentReference,
    consent: EvidenceConsent,
    retention: EvidenceRetention,
    created_at: Timestamp,
}
```

Evidence defaults to local-sensitive storage. Portable evidence requires explicit consent and a second scan. Raw transcripts never enter plans, receipts, diagnostics, Git, or portable artifact stores.

### Optimization manifest

```rust
struct SkillOptimizationManifest {
    id: StableId,
    skill: SkillDescriptor,
    suite_digest: Sha256Digest,
    evidence_digests: Vec<Sha256Digest>,
    provider: ProviderLock,
    optimizer: ModelLock,
    target: ModelLock,
    limits: OptimizationLimits,
    policy_digest: Sha256Digest,
    repository_revision: GitRevision,
}
```

The manifest is canonical, immutable, and rejects shell strings, inline credentials, floating model aliases where a provider exposes immutable revisions, arbitrary flags, and undeclared network destinations.

### Candidate bundle

```rust
struct SkillCandidate {
    id: StableId,
    manifest_digest: Sha256Digest,
    parent_skill_digest: Sha256Digest,
    candidate_content: ContentReference,
    patch: ContentReference,
    evaluation: EvaluationReceipt,
    optimization_history: ContentReference,
    policy_result: PolicyResult,
    state: CandidateState,
}

enum CandidateState { Staged, Rejected, Approvable, Approved, Promoted, Superseded }
```

State changes are append-only events. Candidate bytes never change. A revised candidate receives a new identity.

### Promotion binding

A promotion plan binds:

- candidate and parent skill digests;
- exact repository revision and source path;
- suite, harness, provider, model, and policy digests;
- approval identity and timestamp;
- required canary loadout;
- expected source preimage;
- resulting APM provider-input digest.

Any mismatch before source mutation returns a stable stale-promotion error.

## Gates

A candidate is approvable only when all mandatory gates pass:

1. Candidate parses as a valid skill and preserves protected frontmatter/regions.
2. No secret-like, credential, machine-identity, or private path content is introduced.
3. Validation improves by the configured minimum and held-out score does not regress beyond tolerance.
4. No required case regresses.
5. Token size, allowed links, executable content, and portability remain within policy.
6. Candidate differs from all previously rejected candidate digests.
7. SkillOpt provider, optimizer model, target model, harness, and scorer match their locks.
8. Evaluation artifacts are complete and integrity-checked.
9. The source preimage and repository revision remain current.
10. A human supplies explicit approval metadata.

The held-out gate is evidence, not a security boundary or proof of general improvement. Policy and human review remain independent.

## CLI and service surface

```text
commonkit skills list
commonkit skills show <skill>
commonkit skills baseline <skill> --suite <suite>
commonkit skills evidence import <file> --skill <skill>
commonkit skills optimize <skill> --suite <suite>
commonkit skills candidates [--skill <skill>]
commonkit skills candidate show <candidate>
commonkit skills candidate reject <candidate> --reason <text>
commonkit skills promote <candidate> --plan <id> --confirm
commonkit skills verify <skill> [--target <target>]
commonkit skills schedule status|enable|disable
```

The local service mirrors these as versioned resources and emits lifecycle events. MCP v1 may list skills, start a bounded optimization, inspect candidates, and create a promotion plan. MCP cannot bypass confirmation or directly write source/target files.

## Repository layout

```text
.agents/
  skills/
    <skill>/
      SKILL.md
.claude/
  skills/
    <skill> -> ../../.agents/skills/<skill>
eval/
  skillopt/
    <campaign>/
      suite.yml
      train/
      validation/
      test/
      rubric.md
policies/
  skill-evidence.yml
  skill-evaluation.yml
.commonkit/
  skillopt/
    baselines.lock.json
    rejected.lock.json
```

Sensitive cases, raw evidence, trajectories, model responses, and candidate artifacts live in the private CommonKit artifact/state root, not the repository. Git contains portable cases only when explicitly approved.

## Implementation sequence

### Phase 0 — dependency and threat-model spike

- Pin and inspect SkillOpt 0.2.0, its license, CLI behavior, output layout, credential use, network behavior, resume semantics, Claude/Codex harnesses, and Sleep data handling.
- Execute it only against synthetic transcripts and a disposable skill.
- Decide whether v1 wraps the research engine, SkillOpt-Sleep, or both behind separate provider modes. Recommended: explicit evaluation uses the research engine; session mining remains a disabled-by-default Sleep evidence adapter.
- Add SkillOpt-specific data-flow and prompt-injection threats to the CommonKit threat model.

Exit: fixed supported commands, environment allowlist, output contract, and denied features are documented and tested.

### Phase 1 — contracts and local deterministic vertical slice

- Add canonical Rust contracts and generated JSON schemas for descriptors, suites, manifests, candidates, evaluation receipts, and promotion bindings.
- Implement suite loading, split-isolation validation, deterministic fixture scoring, and artifact persistence.
- Use a fake optimizer to prove baseline -> candidate -> gate -> approval -> stale-safe source patch without invoking a model.

Exit: a fresh daemon can recover and inspect every candidate and receipt; identical inputs yield identical identities; stale promotion changes no source bytes.

### Phase 2 — isolated SkillOpt provider

- Add an exact-version provider runner with private staging, scrubbed environment, structured argv, timeout, cancellation, output limits, network/provider policy, and secret scanning.
- Translate SkillOpt output into CommonKit candidate bundles without exposing SkillOpt runtime paths in durable contracts.
- Keep provider failures and unsupported output non-mutating and diagnosable.

Exit: a synthetic optimization produces an immutable candidate; provider restart or failure cannot corrupt active source or target state.

### Phase 3 — real evaluation and review loop

- Add Codex and Claude Code harness locks, per-case receipts, repeated-run variance reporting, cost accounting, and required-case gates.
- Implement redacted evidence import with exact outbound preview and consent.
- Add CLI/service/MCP read surfaces and confirmation-bound promotion planning.

Exit: one real instruction-only skill can improve on a curated suite, reject a harmful edit, and produce a reviewable source patch.

### Phase 4 — APM and reconciliation integration

- Resolve promoted source to its APM package and invalidate the APM provider input digest.
- Run ordinary APM compile/audit, CommonKit ownership validation, plan creation, apply, verification, and rollback.
- Require a named canary loadout before broader target application.
- Link deployment receipts to candidate and promotion receipts.

Exit: an accepted candidate reaches Claude and Codex only through APM and CommonKit reconciliation, and rollback restores the previous installed version exactly.

### Phase 5 — continuous candidate generation

- Add read-only opportunity discovery and opt-in scheduled optimization.
- Add budgets, concurrency limits, retention, deduplication, backoff, and notifications.
- Optionally submit expensive evaluation shards through USG-46's declared-manifest execution service.
- Expose proposal cards through USG-47's declarative UI contract.

Exit: schedules survive restart, never auto-adopt, do not duplicate equivalent candidates, and stop cleanly at budget/policy limits.

## Test strategy

- Canonical JSON/schema snapshots and semantic digest fixtures.
- Split contamination, held-out leakage, and evidence-consent rejection tests.
- Provider command/environment allowlist tests and malicious-output fixtures.
- Secret, path, symlink, prompt-injection, and oversized-artifact tests.
- Deterministic fake optimizer tests before paid/model tests.
- Required-case regression, variance, scorer drift, model drift, and rejected-candidate deduplication tests.
- Crash/failure injection at every candidate and promotion transition.
- Fresh-process recovery from durable manifests, artifacts, events, and receipts without re-running SkillOpt.
- Stale source/revision/policy/provider/harness rejection before mutation.
- Exact rollback of source patch and installed target state.
- Cross-platform contract tests; execution support may initially be macOS/Linux while Windows reports an explicit unsupported capability.
- End-to-end canary: baseline -> candidate -> approve -> Git patch -> APM compile -> CommonKit apply -> verify -> rollback.

## Security and privacy requirements

- SkillOpt and target harnesses execute as untrusted providers with no live-target or source-write capability.
- Raw sessions and prompts are local-sensitive by default and excluded from portable artifacts, Git, receipts, diagnostics, and telemetry.
- Before a real backend call, the user can inspect the exact redacted outbound evidence payload.
- Credential references are resolved only at execution and never serialized into manifests or artifacts.
- Provider/model allowlists and maximum spend are organization policy floors.
- Candidate Markdown is treated as untrusted content until it passes policy and human review.
- Scheduled work cannot promote, commit, apply, or weaken policy.
- Every external source revision, package version, model identity, scorer, and harness is recorded for auditability.

## Rollout

1. Synthetic fixture and fake optimizer only.
2. One non-critical library skill with curated evaluations.
3. One active instruction-only skill on Codex, manually invoked.
4. Same skill on Claude Code to test transfer and harness differences.
5. One canary loadout through APM/CommonKit deployment.
6. Read-only nightly discovery.
7. Opt-in scheduled candidate generation with strict budget.

Automatic adoption remains out of scope until there is sustained evidence across multiple skills and an independently approved policy change.

## Completion criteria

- The skills lifecycle uses the Rust provider/artifact/plan/receipt architecture and introduces no parallel installer.
- SkillOpt cannot mutate active source, generated APM output, or live targets.
- Every candidate is reproducible enough to audit: all declared inputs, versions, policies, scores, costs, and digests are retained.
- Held-out cases are unavailable during optimization and required-case regressions block approval.
- Promotion is explicit, stale-safe, reviewable as a source patch, and linked to deployment receipts.
- APM remains the agent-context compiler and CommonKit remains the only live-target mutator.
- Canary verification and exact rollback are proven end to end.
- Sensitive evidence does not enter Git or portable CommonKit state.
- Scheduled operation creates proposals only and respects budgets, retention, cancellation, and policy.

## Decisions required before Phase 1

1. **Canonical source layout.** Decided: Git-owned `.agents/skills/<name>/SKILL.md`, with `.claude/skills` using the repository's relative-symlink convention and APM metadata resolving from the canonical source. A top-level `skills/` directory is migration-only.
2. **Initial optimization mode.** Recommended: curated evaluation suites first; SkillOpt-Sleep session harvesting remains opt-in evidence collection until privacy and replay quality are proven.
3. **Approval boundary.** Recommended: human approval plus normal Git review is mandatory; scheduled and MCP callers may create candidates and promotion plans but cannot adopt them.

