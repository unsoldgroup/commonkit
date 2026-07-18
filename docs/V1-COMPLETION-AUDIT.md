# CommonKit v1 completion audit

Audit basis: current `main` implementation and committed tests. Active, uncommitted SkillOpt CLI/MCP/service surface work is excluded. A component contract test is evidence for that component only; it is not evidence that the end-user flow is wired through every required interface or platform.

## First six flows

| Flow | Status | Authoritative evidence | Release gap |
| --- | --- | --- | --- |
| 1. Initialize a target | Partial | `commonkit init` reports the intended private runtime roots; `commonkit-core/tests/target_inventory.rs` validates typed inventories; `commonkit-adapters/tests/git_sync.rs` validates constrained fetch/fast-forward behavior. | Initialization does not create the reported roots, authenticate with GitHub, select/create a kit repository, compose a loadout, register a target, or write production `headless.json`. No clean-machine onboarding test exists. |
| 2. Preview drift | Partial | `commonkit-adapters/tests/provider_planning.rs` and `commonkit-service/tests/production_domains.rs` prove deterministic semantic plans bound to live observed state; stale preimages are rejected. | `commonkit diff` returns `plan_required`; the daemon `/compose` and `/explain` routes remain intentionally unavailable. There is no complete fetch → compose → explain → diff headless flow. |
| 3. Apply safely | Core complete, flow partial | `commonkit-service/tests/control.rs`, `local_executor.rs`, and `production_domains.rs` cover authenticated confirmation, idempotency, durable plans, real adapters, and stale-target cancellation. | A production plan can be applied, but target initialization does not create the configuration needed to reach it, and desktop plan review/apply remains a placeholder. |
| 4. Verify parity | Core complete, flow partial | `commonkit-adapters/tests/files.rs`, `filesystem_resources.rs`, provider tests, and `commonkit-service/tests/production_domains.rs` verify file/resource and provider integrity. | CLI verification reaches the daemon, but no cross-interface contract proves CLI, MCP, and desktop expose the same verification result; desktop verification is not wired. |
| 5. Provision credential references independently | Partial | `commonkit-adapters/tests/credentials.rs` covers env/file/BWS and platform stores; `commonkit-service/tests/production_domains.rs` proves secret-free apply/verify destinations. | Production headless registry currently resolves env/file only. The CLI has no `credentials plan|apply|verify` command group, and the desktop credential view is a placeholder. |
| 6. Recover or roll back | Core complete, flow partial | `commonkit-reconcile/tests/reconcile_engine.rs`, `durable_receipts.rs`, `commonkit-adapters/tests/files.rs`, and `commonkit-service/tests/production_domains.rs` cover reverse rollback, restart recovery, authenticated receipts, artifact tampering, and successful-run rollback. | Filesystem recovery is strong, but no release-matrix test exercises the full installed daemon/CLI rollback flow on macOS, Linux, and Windows. Snapshot restore keeps a previous file during the swap but does not yet expose a durable restore receipt/restart-recovery workflow. |

## All twelve v1 flows

| # | Flow | Audit status | Evidence and remaining gap |
| --- | --- | --- | --- |
| 1 | Initialize a target | Partial | See first-six matrix. GitHub authentication/repository provisioning remains an explicitly unresolved product decision in `CONTEXT.md`. |
| 2 | Preview drift | Partial | Deterministic planning exists; user-facing diff and daemon composition are incomplete. |
| 3 | Apply safely | Partial | Reconciliation core is production-capable; clean-machine and desktop wiring are incomplete. |
| 4 | Verify parity | Partial | Adapter/service verification exists; interface and installed-platform parity is unproven. |
| 5 | Provision credential references | Partial | Adapter/service paths exist; production external-manager wiring and CLI/desktop flows are incomplete. |
| 6 | Recover or roll back | Partial | Durable filesystem recovery is demonstrated; installed-platform and snapshot recovery gates remain. |
| 7 | Multiple targets and five-layer composition | Partial | `commonkit-config/tests/{layer_set,merge,provenance}.rs`, `commonkit-core/tests/{layer_contract,policy_floor,target_inventory}.rs`, and CLI compose/explain tests cover composition. Remote SSH boundaries have contract tests. No production registry discovers/composes layer files itself, remote staging remains deferred, and daemon compose/explain are unavailable. |
| 8 | Scheduled read-only drift checks | Partial | `commonkit-service/tests/scheduler.rs` proves persistence, overlap suppression, and read-only checker behavior. The production daemon does not construct/run a `DriftScheduler`, so persisted schedules do not execute unattended checks. |
| 9 | Optional MCP relay state | Substantially implemented | Relay runtime/config/lifecycle/provider-convergence/transaction tests and service relay routes cover the Rust data/control plane; legacy normalization parity is tested. Installed lifecycle and full dual-runtime black-box parity remain release gates. |
| 10 | Coding-agent adapters | Substantially implemented | APM 0.25.0 contract/real-release tests, chezmoi 2.70.4 isolation tests, native fallback, provider ownership, and artifact tests exist. Remote provider staging and the full platform release matrix are not complete; SkillOpt is separately excluded from this audit. |
| 11 | Redacted diagnostics | Implemented contract, partial release flow | Contract redaction and `commonkit-service/tests/diagnostics.rs` cover schema-bound secret-free exports; CLI and MCP expose diagnostics. Desktop export is not wired and installed-platform validation remains. |
| 12 | OSS onboarding, schemas, threat model, CI | Partial | Schemas, `docs/THREAT-MODEL.md`, migration/release/support docs, three-OS CI, provider gates, and release workflows exist. README/desktop onboarding does not deliver the clean-machine product flow, production signing/notarization has not run, and lifecycle automation upgrades with installers rather than the in-app updater. |

## Acceptance checklist audit

| Criterion | Result |
| --- | --- |
| All 12 flow contract suites | Not met; no single suite covers the twelve end-user flows, and several are only component-level. |
| macOS/Linux/Windows installation, reconciliation, snapshot, uninstall | Not met; workflows exist, but production signed two-version execution is not recorded and snapshot/reconciliation are not exercised through installed applications on all three OSes. |
| Declared remote target combinations | Not met; typed SSH command/filesystem tests exist, while remote provider staging is explicitly deferred. |
| Organization policy weakening rejected with provenance | Met at core/config contract level (`policy_floor`, merge/provenance tests). |
| Deterministic, redacted, input/observed-bound plans | Met for provider/filesystem and relay plans; live observed-state production coverage exists. |
| Idempotent apply and failure recovery | Met for the reconciliation core and local filesystem adapter. |
| No secret/live SQLite in Git or diagnostics | Secret/redaction/path contracts exist; no end-to-end malicious-repository release gate proves the complete claim. |
| Snapshot restore integrity and rollback safety | Partial; encryption/integrity and atomic previous-file restoration exist, but durable restore receipts/restart recovery and service lifecycle coordination are missing. |
| Rust relay black-box parity/migration | Partial; configuration normalization and migration fixtures exist, not the full behavior matrix named by the scope. |
| Desktop/headless same domain state | Not met; desktop status is limited and management panels are placeholders. |
| Signed installers and updates verified | Not met in production; automation and local signature fixtures exist without production identities/notarization evidence. |
| Threat model, schemas, onboarding, support, migration docs | Partial; documents exist, but onboarding is descriptive rather than a working clean-machine flow. |

## Release conclusion

Do not mark CommonKit v1 complete or release-ready. The reconciliation/provider/relay foundations are credible and well tested, but the product-level initialization, scheduled execution, complete headless surface, desktop management, snapshot recovery lifecycle, remote staging, and production release evidence are still open gates.
