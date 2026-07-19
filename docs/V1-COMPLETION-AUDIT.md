# CommonKit v1 completion audit

Audit basis: committed implementation on `uns-1274-commonkit-v1-rust-tauri` through `efe9a9d` (`efe9a9d9f4359952a7b0b21ed3e9e1f50e33a270`). “Met locally” means the production Rust path and its focused contracts pass on the audit host; it does not stand in for production signing, notarization, published updater, or hosted multi-OS evidence. GitHub reported no workflow run for this exact commit at the time of the audit.

## Twelve-flow matrix

| # | Flow | Implementation status | Current evidence | Remaining gate |
| --- | --- | --- | --- | --- |
| 1 | Initialize a target | Met locally | `commonkit init create|connect` authenticates with GitHub CLI, creates or clones the kit, writes all composition layers and target registration, creates private runtime roots, imports and validates pinned native/APM/chezmoi inputs, materializes an initial provider plan, and writes production `headless.json`. Desktop onboarding exposes the same bounded provider choices without shell or arbitrary-path authority. The fresh onboarding suite passes. | Installed first-run evidence on the declared platform matrix. |
| 2 | Preview drift | Met locally | Production Git-backed provider materialization, composition/explanation, live observation, durable plan retrieval, deterministic diff, stale-input rejection, and local/SSH planning are wired. Service, CLI, provider-pipeline, and SSH suites pass. | Hosted platform and declared remote support-matrix evidence. |
| 3 | Apply safely | Met locally | Content-bound plans, confirmation, deterministic adapters, idempotency, receipts, stale-target rejection, verification, rollback, typed-SSH dispatch, and fresh-process recovery pass focused tests. | Installed reconciliation through signed artifacts on all three OSes. |
| 4 | Verify parity | Met locally | Service, CLI, MCP, and desktop all expose production verification. Provider integrity and target parity are checked through the same domain. | Installed three-OS interface-parity evidence. |
| 5 | Provision credential references | Met locally | Credential readiness/apply/verify are exposed through service, CLI, and desktop. Production env/file and BWS resolution is tested without leaking values; native platform credential adapters have focused contracts. | Hosted execution of native platform stores and installed products. |
| 6 | Recover or roll back | Met locally | Reconciliation and snapshot restore/promote use authenticated durable plans and receipts, encrypted preimages, lifecycle coordination, tamper rejection, startup discovery, and fresh-process recovery. CLI and desktop operator surfaces are present. | Installed daemon/CLI recovery on macOS, Linux, and Windows artifacts. |
| 7 | Multiple targets and five-layer composition | Met locally | Five-layer composition, monotonic organization policy, provenance, durable local/SSH target inventory, consent-bound multi-target selection, target-scoped plan/apply/verify, selected-target scheduler iteration, per-target roots, controller-side provider materialization, portable artifact staging, and semantic typed-SSH plan/apply/recovery pass. Fresh tests apply and verify two configured production targets across restart. | Recorded remote support-matrix runs remain hosted release evidence. |
| 8 | Scheduled read-only drift checks | Met locally | `commonkitd` constructs `DriftScheduler`; persistence, enable/disable/status, overlap suppression, read-only verification, degraded reporting, and CLI/desktop mutation surfaces pass focused tests. | Installed unattended execution evidence. |
| 9 | Optional MCP relay state | Met locally | Rust relay lifecycle, transactional reconciliation, stable endpoint, HTTP upstreams, migration, failure behavior, and redaction are implemented. The shared Node/Rust black-box compatibility suite passed against the current `commonkitd` binary. | Installed lifecycle on the release matrix; legacy retirement remains a later decision. |
| 10 | Coding-agent adapters and SkillOpt | Met locally | Pinned APM 0.25.0, chezmoi 2.70.4 safe isolation, native fallback, provider artifacts, ownership validation, production Git-provider materialization, local/SSH planning, independent SkillOpt harness isolation, and authenticated canary recovery pass focused suites. | Hosted provider matrices and installed provider execution remain release evidence. |
| 11 | Redacted diagnostics | Met locally | Schema-bound diagnostics are exposed by service, CLI, MCP, and desktop; focused redaction and secret-scanning contracts pass. | Installed malicious-input export evidence on all supported platforms. |
| 12 | OSS onboarding, schemas, threat model, CI, packaging, and releases | Partial | README documents the Rust v1 flow. Schemas, threat model, support/migration/release docs, three-OS CI, provider gates, packaging, signatures, SBOM, updater fixtures, and installed unsigned lifecycle automation exist. The installed lifecycle now exercises installed CLI/daemon artifacts through onboarding, plan, apply, verify, encrypted snapshot/restore, fresh-process recovery, relay compatibility, and portable SSH-helper dispatch on the three-OS CI matrix. | Production signing/notarization identities, two published signed versions, and a successful hosted signed install → reconcile → verify → snapshot/restore → recover → update → uninstall matrix remain external release gates. |

## Focused verification run

- Fresh CLI onboarding, including imported APM/chezmoi inputs and secret/symlink rejection: 13 tests passed.
- Fresh target inventory, consent, per-target execution, selected-target scheduling, and restart behavior: 11 tests passed.
- Fresh production typed-SSH reconciliation: 1 end-to-end test passed.
- Fresh provider-to-relay convergence, durable input binding, and ownership collision rejection: 3 tests passed.
- Fresh desktop management, onboarding, security, and update behavior: 18 tests passed.
- Fresh release-readiness contracts, including installed three-OS lifecycle wiring: 7 tests passed.
- Earlier production service domains, credentials, scheduler, and typed-SSH focused verification: 10 tests passed.
- Earlier provider pipeline, remote staging, and SSH filesystem recovery: 5 tests passed.
- Earlier durable snapshot restore/promotion: 9 tests passed.
- Shared relay compatibility: passed with both legacy Node and the current Rust daemon.
- Skill deployment/canary recovery: 7 tests passed.
- Fresh SkillOpt crate verification: 14 tests plus doc tests passed, including provider isolation, stale Git/policy rejection, durable candidate promotion, evidence handling, scheduling, and upgrade isolation.

## Priority findings

Finding count: **P0: 0; P1: 2**.

### P0

None found in the audited committed paths.

### P1

1. Supply production signing/notarization/updater identities, publish two signed versions, and run the signed release lifecycle on macOS, Linux, and Windows.
2. Record the declared remote-target and hosted provider support matrices. Local production contracts cover these paths, but no workflow run exists for current HEAD and local execution cannot substitute for the hosted evidence.

## Local implementation versus external release gates

Locally implemented and evidenced: onboarding with bounded provider import, composition/policy, Git-backed provider materialization, deterministic plan/diff, durable multi-target selection and per-target local/typed-SSH apply, verification, credentials, receipts/recovery, snapshots, scheduler, relay parity, diagnostics, operator CLI, desktop mutations, SkillOpt lifecycle contracts, packaging automation, fail-closed updater fixtures, and the executable installed-lifecycle harness.

External or hosted evidence still required:

1. Production Apple signing/notarization, Windows signing, and Tauri updater identities.
2. Two published signed versions and a successful install → reconcile → verify → snapshot/restore → recover → update → uninstall run on macOS, Linux, and Windows. The workflow is implemented but has no run for current HEAD.
3. Recorded declared remote-target support-matrix runs.
4. Hosted provider isolation gates for every supported OS/architecture combination available upstream.

## Conclusion

Do not mark CommonKit v1 fully release-ready yet. Eleven of the twelve flows are implemented and pass local production-path contracts. Flow 12 remains partial solely because production signing, two-version publication, and hosted platform/provider/remote-target evidence have not yet been supplied. No P0 implementation finding was identified in this final pass.
