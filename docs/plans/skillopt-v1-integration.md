# SkillOpt v1 integration plan

Status: Active scope; implementation checkpoint is not merge-ready  
Tracking: USG-48, with CommonKit delivery coordinated under UNS-1274  
Reference: branch `al-unsoldgroup/skillopt`, commit `fc7de87`  
Handoff: `.claude/handoffs/2026-07-18-162116-commonkit-v1-skillopt-integration.md`

## Boundary

SkillOpt is an exact-version external optimizer that computes proposed skill improvements from explicitly approved data. It is not a desired-target mutator and does not replace APM. CommonKit owns isolation, evidence contracts, policy gates, immutable candidates, approval, orchestration, receipts, canary verification, and rollback. APM compiles approved Git-owned agent context. CommonKit adapters alone mutate targets.

```text
SkillOpt candidate computation
  -> independent CommonKit harness and policy decision
  -> human approval into canonical Git source
  -> APM materialization
  -> CommonKit plan and named-canary reconciliation
```

Canonical source is `.agents/skills/<name>/SKILL.md`. `.claude/skills` is a relative delivery symlink. Top-level `skills/` is not accepted as canonical.

## Checkpoint classification

### Retain

- Exact SkillOpt 0.2.0 external-provider lock, source revision, wheel digest, compatibility probe, and explicit upgrade/activation workflow.
- Fixed invocation allowlist, scrubbed environment, bounded output, timeout, strict JSON, source-mutation detection, and adopted-path rejection.
- Redacted evidence, immutable candidates, confirmation-bound promotion, receipts, events, schedules with cost/deduplication limits, and service/CLI/MCP contracts.
- APM compiler protocol, named-canary model, lineage types, schema, threat model, runbook, and broad test fixtures.
- The restriction that CommonKit never exposes SkillOpt adoption, auto-adoption, or provider-owned scheduling.

### Adapt before integration

- Complete `harness_executable` and `harness_corpus` wiring through CLI, provider manager upgrades, fixtures, schemas, and tests.
- Replace aggregate-score-derived held-out results with strict independent `HarnessEvaluationReport` evidence.
- Complete `ReviewedTasksBinding` across skill, campaign, suite, train/validation digests, and exact visible case IDs.
- Formalize the digest-bound policy source currently proposed as `policies/skill-optimization.policy.json`.
- Complete canonical `.agents/skills` enforcement in inventory, evidence, callers, and tests.
- Replace the partial macOS-only sandbox launcher with a verified supported-platform isolation contract that fails closed where unavailable.
- Rebuild canary rollback around persisted authenticated deployment receipts and observed restoration verification.

### Defer

- Automatic adoption, merge, or dataset approval.
- Unbounded provider/harness arguments, hidden model calls, implicit spend, or arbitrary provider scheduling.
- Platforms without a proven isolation implementation; they remain explicitly unsupported rather than running weakly isolated.

### Remove or reject

- Fabricated held-out evidence or policy success.
- Legacy skill roots and unbound `reviewed: true` assertions.
- Direct provider access to held-out data.
- Private-directory-only isolation presented as a security boundary.
- Unpersisted caller receipts used as rollback authority.

## Blocking security gates

1. Enforce canonical source in every inventory, evidence, lifecycle, and promotion path.
2. Ensure only the independent harness reads held-out data and binds its report to suite, skill, baseline, candidate, policy, harness/scorer, required cases, and cost.
3. Require provider-visible cases to equal the bound train plus validation set and remain disjoint from held-out cases.
4. Prove OS/container isolation denies network and undeclared filesystem/process access; fail closed when unavailable.
5. Revalidate Git `HEAD` and the canonical policy digest immediately before plan and promotion, with zero mutation on staleness.
6. Persist and authenticate the canary deployment receipt, roll back through durable reconciliation, observe restored target state, and persist a verified rollback receipt.

No gate may be weakened to regain compilation or test success.

## Implementation sequence

1. Import the checkpoint by reviewed commits or patches, preserving concurrent CommonKit work; never merge `fc7de87` wholesale.
2. Restore compilation by completing the stricter harness, corpus-binding, and canonical-root contracts first.
3. Finish independent held-out/policy harness tests and provider-visible dataset separation.
4. Implement and test the supported-platform isolation matrix.
5. Formalize policy location and stale-Git/stale-policy zero-mutation tests.
6. Implement durable authenticated named-canary deployment and verified rollback using CommonKit reconciliation artifacts and receipts.
7. Converge promotion with APM materialization and the normalized provider/ownership/plan pipeline.
8. Reconcile CLI, service, MCP, schedules, schema, threat model, and operator runbook.
9. Run focused and full verification, then fresh cold adversarial reviews. Keep USG-48 In Progress until both pass.

## Completion criteria

- The pinned provider receives only declared, bound train/validation inputs inside real isolation.
- The independent harness alone evaluates held-out cases and package/skill policy.
- CommonKit stores a non-fabricated immutable candidate with redacted, digest-bound evidence.
- Human confirmation promotes the exact candidate only against fresh Git and policy state.
- APM compiles the promoted source without bypassing CommonKit target policy.
- CommonKit plans, applies, and verifies a named canary through normal adapters.
- A persisted authenticated rollback restores and verifies exact prior state after restart.
- No provider/harness path can directly adopt, schedule, merge, or mutate an active target.
- Full workspace tests, clippy, schema checks, and two cold reviews pass from the final tree.

## Verification

```sh
cargo fmt --all -- --check
CARGO_INCREMENTAL=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p commonkit-skills --offline
CARGO_INCREMENTAL=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p commonkit-reconcile --test skill_deployment --offline
CARGO_INCREMENTAL=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p commonkit-service --test skills --offline
CARGO_INCREMENTAL=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p commonkit-mcp --test tools --offline
CARGO_INCREMENTAL=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test --workspace --offline
CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 cargo clippy --workspace --all-targets --offline -- -D warnings
git diff --check
```

The loopback service test may require an explicitly approved unsandboxed run. Earlier green results predate the trust-boundary fixes and are not completion evidence.
