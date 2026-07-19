# CommonKit v1 completion audit

Audit basis: committed implementation on `uns-1274-commonkit-v1-rust-tauri` through `5c6777e`. The five pre-existing formatting-only worktree changes were excluded from this audit. “Met locally” means the production Rust path and its focused contracts pass on the audit host; it does not stand in for production signing, notarization, published updater, or hosted multi-OS evidence.

## Twelve-flow matrix

| # | Flow | Implementation status | Current evidence | Remaining gate |
| --- | --- | --- | --- | --- |
| 1 | Initialize a target | Partial | `commonkit init create|connect` authenticates with GitHub CLI, creates or clones the kit, writes all composition layers and target registration, creates private runtime roots, materializes native declarations, and writes production `headless.json`. The onboarding suite passes. | First-run onboarding does not select and configure APM or chezmoi from a loadout, and desktop onboarding remains an informational panel. |
| 2 | Preview drift | Met locally | Production Git-backed provider materialization, composition/explanation, live observation, durable plan retrieval, deterministic diff, stale-input rejection, and local/SSH planning are wired. Service, CLI, provider-pipeline, and SSH suites pass. | Hosted platform and declared remote support-matrix evidence. |
| 3 | Apply safely | Met locally | Content-bound plans, confirmation, deterministic adapters, idempotency, receipts, stale-target rejection, verification, rollback, typed-SSH dispatch, and fresh-process recovery pass focused tests. | Installed reconciliation through signed artifacts on all three OSes. |
| 4 | Verify parity | Met locally | Service, CLI, MCP, and desktop all expose production verification. Provider integrity and target parity are checked through the same domain. | Installed three-OS interface-parity evidence. |
| 5 | Provision credential references | Met locally | Credential readiness/apply/verify are exposed through service, CLI, and desktop. Production env/file and BWS resolution is tested without leaking values; native platform credential adapters have focused contracts. | Hosted execution of native platform stores and installed products. |
| 6 | Recover or roll back | Met locally | Reconciliation and snapshot restore/promote use authenticated durable plans and receipts, encrypted preimages, lifecycle coordination, tamper rejection, startup discovery, and fresh-process recovery. CLI and desktop operator surfaces are present. | Installed daemon/CLI recovery on macOS, Linux, and Windows artifacts. |
| 7 | Multiple targets and five-layer composition | Partial | Five-layer composition, monotonic organization policy, provenance, target inventory, local execution, controller-side provider materialization, portable artifact staging, and semantic typed-SSH plan/apply/recovery pass. | Production `headless.json` still represents one active sync target at a time; no multi-target selection/iteration surface or recorded remote support-matrix run exists. |
| 8 | Scheduled read-only drift checks | Met locally | `commonkitd` constructs `DriftScheduler`; persistence, enable/disable/status, overlap suppression, read-only verification, degraded reporting, and CLI/desktop mutation surfaces pass focused tests. | Installed unattended execution evidence. |
| 9 | Optional MCP relay state | Met locally | Rust relay lifecycle, transactional reconciliation, stable endpoint, HTTP upstreams, migration, failure behavior, and redaction are implemented. The shared Node/Rust black-box compatibility suite passed against the current `commonkitd` binary. | Installed lifecycle on the release matrix; legacy retirement remains a later decision. |
| 10 | Coding-agent adapters and SkillOpt | Partial | Pinned APM 0.25.0, chezmoi 2.70.4 safe isolation, native fallback, provider artifacts, ownership validation, production Git-provider materialization, local/SSH planning, and authenticated SkillOpt canary recovery exist. | The full SkillOpt suite produced one `ProviderIsolationBreached` failure and then passed in isolation, making the isolation gate order/environment-sensitive. First-run provider selection and hosted provider matrices also remain. |
| 11 | Redacted diagnostics | Met locally | Schema-bound diagnostics are exposed by service, CLI, MCP, and desktop; focused redaction and secret-scanning contracts pass. | Installed malicious-input export evidence on all supported platforms. |
| 12 | OSS onboarding, schemas, threat model, CI, packaging, and releases | Partial | README now documents the Rust v1 flow. Schemas, threat model, support/migration/release docs, three-OS CI, provider gates, packaging, signatures, SBOM, updater fixtures, and installed unsigned lifecycle automation exist. | Production signing/notarization identities, two published signed versions, lifecycle workflow evidence, and installed reconciliation/snapshot coverage are external release gates. |

## Focused verification run

- Production service domains, credentials, scheduler, and typed-SSH end to end: 10 tests passed.
- CLI onboarding and headless operator surface: 8 tests passed.
- Provider pipeline, remote staging, and SSH filesystem recovery: 5 tests passed.
- Durable snapshot restore/promotion: 9 tests passed.
- Desktop management and update behavior: 13 tests passed.
- Shared relay compatibility: passed with both legacy Node and the current Rust daemon.
- Skill deployment/canary recovery: 7 tests passed.
- Release-readiness contracts: 7 tests passed.
- SkillOpt: the full crate run failed one real provider-isolation test with `ProviderIsolationBreached`; the same test passed immediately when rerun alone. This is a release-blocking flaky safety gate until diagnosed and made deterministic.

## Priority findings

### P0

None found in the audited committed paths.

### P1

1. Make the SkillOpt real isolation test deterministic. A safety boundary that passes only in isolation cannot be used as release evidence.
2. Finish first-run provider selection/configuration so a user can choose an APM or chezmoi-backed loadout without manually authoring `headless.json`.
3. Add a production multi-target selection/iteration model; one configured sync target is not the complete multiple-target flow.
4. Exercise installed reconciliation, verification, snapshot, recovery, update, and uninstall on macOS, Linux, and Windows. The current unsigned CI lifecycle and signed release workflow do not yet provide that complete evidence.

## Local implementation versus external release gates

Locally implemented and evidenced: composition/policy, Git-backed provider materialization, deterministic plan/diff, local and typed-SSH apply, verification, credentials, receipts/recovery, snapshots, scheduler, relay parity, diagnostics, operator CLI, desktop mutations, SkillOpt lifecycle contracts, packaging automation, and fail-closed updater fixtures.

External or hosted evidence still required:

1. Production Apple signing/notarization, Windows signing, and Tauri updater identities.
2. Two published signed versions and a successful install → reconcile → verify → snapshot/restore → recover → update → uninstall run on macOS, Linux, and Windows.
3. Recorded declared remote-target support-matrix runs.
4. Hosted provider isolation gates for every supported OS/architecture combination available upstream.

## Conclusion

Do not mark CommonKit v1 fully release-ready yet. Seven of the twelve flows are implemented and pass local production-path contracts; five remain partial because of first-run provider selection, multi-target product wiring, the flaky SkillOpt isolation gate, or external signed/platform evidence. The remaining work is no longer a reconciliation-core rewrite.
