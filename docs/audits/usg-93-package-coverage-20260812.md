# USG-93 package behavior coverage audit

Date: 2026-08-12
Target: `origin/main` (`08a2d2b272e8fd28413cdab10e3cfbde13325db1`)
Sources: `recovery/package-local-20260812` (`283d1df`),
`archive/main-local-36068de-20260812` (`36068de`), and their relevant
USG-128/129/130 package predecessors.

## Decision

No production behavior is missing from `origin/main`. The target contains the
archived planner, consent, v3 receipt, mutation, recovery, local execution,
SSH execution, APT authority/evidence, and NVM authority/evidence seams, plus
later fail-closed hardening. No test-first port is therefore required and no
production code is changed by this audit.

The archived `98a216c` mutation commit is an evolved equivalent rather than an
exact patch match: `origin/main` carries the corresponding implementation as
`dab0848` and then adds concrete offline local/SSH backends and authority
checks. The archived `283d1df` consent/service/CLI change is an exact stable
patch match to `origin/main`'s `3ee70cc`. `36068de` has no package-file delta
relative to `283d1df`; its changes are restore-rehearsal and symlink-plan
artifacts outside the package seams.

## Semantic coverage matrix

| Behavior | Archived source behavior | Target evidence on `origin/main` | Public tests | Result |
| --- | --- | --- | --- | --- |
| Planning and provider boundary | `005b9a5`, `8a31e44`, `4c7b3a7`, `f293e77`: normalized package resources route by typed manager/id; providers can declare only exact package intent; the controller resolves and persists artifacts before planning. | `900351a`, `83ae1e0`, `2bc177d`, `e911add`; `PackageResolutionCoordinator`, `ResolvedPackageIntent`, `PackageResourcePlanner`, and `build_resolved_provider_plan_with_router`. | `provider_contract`: `provider_json_cannot_submit_package_resolution_or_artifacts`; `provider_planning`: `resolved_planning_rejects_removed_package_source_policy_before_registration`, `package_artifact_omission_and_tamper_fail_before_registration`; `package_resolution`: `disallowed_package_source_fails_before_backend_or_fetch`. | Covered; target adds artifact/source/target binding checks. |
| Consent and apply entry points | `ddc5f67`, `283d1df`: package operations require exact reviewed consent; service and CLI carry JSON consent and confirmation IDs through apply. | `25c0c69`, `3ee70cc`, then current `b2aeaca` boundary hardening; `Reconciler::validate_package_consent`, `execute_with_package_consent`, production local/SSH executors, and CLI `--package-consent`. | `package_authorization`: `package_plan_requires_exact_consent_before_receipt_or_mutation`; `service/control`: `package_apply_requires_consent_before_reserving_idempotency`, `package_apply_carries_consent_to_the_executor`; CLI headless consent tests. | Covered; no consent can reach receipt creation, target observation, or mutation. |
| Receipt v3 and evidence | `23c462e`: versioned package authorization/evidence is bound into the receipt chain; legacy versions are not actionable. | `2ca2e6e`, current receipt validation and evidence repair commits through `08a2d2b`; `PackageReceiptAuthorization`, `PackageReceiptEvidence`, v3 state/transition validation, and plan/target bindings. | `package_receipt`: `package_authorization_and_evidence_are_bound_into_v3_receipt_chain`; `package_authorization`: `package_plan_v2_receipt_cannot_be_acted_upon`, `legacy_v2_package_receipt_is_rejected_before_adapter_preflight`, recovery evidence repair tests; `receipt_state`: forward-only state tests. | Covered and stricter than archive. |
| Mutation contract | `58ab302`, `98a216c`: typed `PackageMutationBackend` receives only persisted resolution/artifacts; adapter plans a confirmed, forward-only operation and delegates prepare/apply/verify. | `cc34c16`, evolved `dab0848`, and current `PackageMutationBackendRegistry`, `ProcessOfflinePackageBackend`, `SshOfflinePackageBackend`, artifact staging, runtime authority, and exact final-observation validation. | `package_adapter_foundation`: `package_adapter_api_exposes_only_the_typed_offline_backend_seam`, `approved_apt_resolution_delegates_only_offline_mutation_calls`, source/evidence rejection tests; `target_helper`: `package_mutation_revalidates_target_authority_and_exact_artifacts`. | Covered; target is a strict superset. |
| Forward recovery and restart | `0321e49`, `c5fd21f`, `8d2f6c9`, `98a216c`: package recovery is forward-only, observes before/after, and uses durable resolution/artifacts without provider or network re-execution. | `ed50a79`, `085bcb6`, `caeb6ac`, evolved `dab0848`, plus current recovery barriers, receipt snapshots, target observation gates, and failed-evidence repair. | `package_authorization`: `interrupted_package_recovery_accepts_v2_plan_bound_to_v3_receipt`, `package_recovery_requires_after_observation_before_repairing_evidence`, both failed-evidence repair tests; `target_helper`: `artifacts_and_recovery_receipts_survive_helper_process_restarts`. | Covered and hardened. |
| Local execution | `98a216c`: local package adapter seam and package manager observation/mutation path. | `ProcessOfflinePackageBackend` binds target platform, manager authority, retained roots, exact artifacts, and offline recipes; local production executor registers it. | `package_nvm_security`: source/config/shell drift, scrubbed environment, malformed observations, root swaps, and retained-root tests; `package_adapter_foundation` APT recovery tests; `native_package_resolution`: offline reopen/no-target-mutation test. | Covered; target adds fail-closed root and evidence checks. |
| SSH execution | `98a216c`, `283d1df`: package operations are available through typed SSH execution with consent and durable recovery. | `SshOfflinePackageBackend`, `ProductionSshPlanExecutor`, target identity/platform/privilege binding, chunked artifact staging, and receipt recovery. | `package_adapter_foundation`: SSH malformed/foreign observation and chunk staging tests; `production_ssh_e2e`: authenticated SSH apply/recovery; `target_helper`: helper restart and probe tests. | Covered; target adds target identity, privilege, and artifact-transfer barriers. |
| APT authority and evidence | `4c7b3a7` through `abc363b`: controlled Debian/Ubuntu resolver, signed metadata, exact closure/archive evidence, architecture authority, and canonical live-state observations. | `2bc177d` through `41ea574`, then `origin/main` hardening: private APT root, `_apt` sandbox, executable/config/key digests, signed `InRelease`, exact closure, SHA-256 archives, dpkg status/evidence, and source anchors. | `apt_resolution`: fixed private command plan, source/key scope, unsafe snapshot/foreign archive rejection, deterministic closure, authenticated exact closure; `native_package_resolution`: closed command-shape tests; `package_adapter_foundation`: forged metadata and recovery identity tests; `target_helper`: trusted metadata anchor test. | Covered and stricter than archive. |
| NVM/Node authority and evidence | `3cc955a`, `3f259b7`, `e329545`, `376af98`: controlled nvm/Node resolver, version dispatch parsing, pinned script release authority, signed release evidence, and offline recipe. | `0c5dc35`, `e5dd90f`, `35b27ed`, `d1b3471`, then current marker/canonicalization hardening through `08a2d2b`; exact nvm script, shell/keyring/config digests, release signatures, target tuple, and strict installed/current/system observations. | `node_resolution`: closed environment/signature commands, unknown release, dispatch, script authority, and v2 schema tests; `package_nvm_security`: forged evidence, script/config drift, malformed/padded/current/empty observations, manager-prefix and root-binding tests. | Covered and stricter than archive. |

## Stable patch-ID ledger

Patch IDs below are computed with `git patch-id --stable` over the package,
reconcile, service, CLI, contract, and schema paths. The target commit is the
same patch where shown; a different target patch ID is intentionally marked as
an evolved implementation and was reviewed semantically rather than treated
as absent.

| Archived commit | Stable patch ID | Target equivalent | Coverage |
| --- | --- | --- | --- |
| `0321e49`, `c5fd21f`, `8d2f6c9` | `37dd1bda`, `cdd55bbe`, `94571437` | `ed50a79`, `085bcb6`, `caeb6ac` | recovery barrier/contracts/artifacts |
| `005b9a5`, `8a31e44` | `3fe87bbb`, `33b3f6b8` | `900351a`, `83ae1e0` | normalized planning/routing |
| `4c7b3a7`, `f293e77`, `8373291` | `199cd446`, `5e118014`, `cdfb528b` | `2bc177d`, `e911add`, `c8c7495` | resolution foundation/authority |
| `76a140d`, `db0c1a8`, `2e01a3b` | `499fd7be`, `62059c75`, `948d063e` | `2168bb3`, `d6a0ab2`, `0a70fc0` | schema/closure/source compatibility |
| `8511cc8`, `99338c2`, `550a783`, `9bfe6ba`, `e59a31d`, `dcb28e7` | `f2501e7d`, `ab4a3d82`, `d2e62571`, `de5359b5`, `c30e51d7`, `fab6cff6` | `527b3ef`, `dc668c9`, `7c27f29`, `2081daf`, `374aacc`, `a8fe754` | controlled APT command/resolution safety |
| `3cc955a`, `3f259b7`, `e283c2e`, `10778ab` | `d864ce7a`, `c83be9b6`, `b68611e1`, `db31552e` | `0c5dc35`, `e5dd90f`, `76d09ac`, `6d7e9b6` | Node/NVM resolver and native evidence |
| `88c7b04`, `538d81b`, `f4d68cc`, `dbee117`, `68c3f02`, `f47352f`, `abc363b` | `1f26b854`, `2aefe259`, `a49eef34`, `fd586679`, `c3f7835e`, `62acb4d6`, `95338e84` | `0e02a8e`, `d97d901`, `a5295c9`, `0ceb628`, `6ad1a3e`, `74d152b`, `41ea574` | signed metadata/live APT evidence/architecture |
| `e329545`, `376af98` | `bc488d81`, `5bf3915a` | `35b27ed`, `d1b3471` | NVM dispatch/script authority |
| `ddc5f67`, `23c462e`, `58ab302` | `ac46cf0e`, `cf1f218c`, `eec16761` | `25c0c69`, `2ca2e6e`, `cc34c16` | consent/receipt/adapter contract |
| `98a216c` | `ff9b5542` | `dab0848` (`a3c83ee5`, evolved) | mutation core, concretized by current backends |
| `283d1df` | `e3ac7ecd` | `3ee70cc` (exact) | service/CLI consent apply |
| `36068de` | no package patch | n/a | restore/symlink artifacts only; no package delta |

## Verification plan

Run on the named target platform with the repository's `rtest` wrapper where
applicable:

```text
cargo test -p commonkit-adapters --locked
cargo test -p commonkit-reconcile --locked
cargo test -p commonkit-service --locked
cargo test -p commonkit-cli --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo fmt --all -- --check
git diff --check
```

## Verification results

All requested checks passed with exit code 0 in this dispatched worktree:

- `cargo test -p commonkit-adapters -p commonkit-reconcile -p commonkit-service -p commonkit-cli --locked`
  (all executed tests passed; only the repository's explicit platform/provider
  opt-in cases were ignored).
- `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`
- `cargo fmt --all -- --check`
- `git diff --check`

The tests were run against the Orca worktree's archive-derived checkout, not by
mutating or switching the worktree to `origin/main`. Target-side coverage was
verified from the `origin/main` source and test inventory, including the
post-archive evolved implementation and hardening commits listed above. No
live target, network package source, Linear issue, GitHub workflow, or remote
branch was touched.

## Self-review

- Correctness: every requested semantic category has an explicit row, source
  commits, target evidence, and public tests; exact patch matches are separated
  from the evolved `98a216c` implementation.
- Safety: the only file changed is this audit document; no code, schema,
  receipt, fixture, secret, or target state was modified.
- Maintainability: the matrix uses stable commit IDs and named seams/tests so
  a later reviewer can re-run or challenge each equivalence claim.
- Residual risk: target-side test execution was inventory-based because this
  dispatched worktree is intentionally not switched to or merged with
  `origin/main`; no behavior gap was found that warrants a port.
