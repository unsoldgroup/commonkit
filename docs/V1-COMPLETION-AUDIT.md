# CommonKit v1 completion audit

Audit basis: committed implementation on `uns-1274-commonkit-v1-rust-tauri` through `589c5e7`. Active, uncommitted SkillOpt work is excluded. Component tests establish their named contracts; they do not by themselves establish an installed, cross-platform product flow.

## Twelve-flow matrix

| # | Flow | Status | Current evidence | Actual remaining gate |
| --- | --- | --- | --- | --- |
| 1 | Initialize a target | Partial | `commonkit init create|connect` now checks GitHub CLI authentication, creates or clones a repository, writes the required composition layers and target registration, creates private runtime roots, and writes production `headless.json`. `commonkit-cli/tests/onboarding.rs` covers both modes and production composition/plan loading. | Connected loadouts are represented by an empty native `MaterializedState`; initialization does not run the selected APM, chezmoi, or native provider pipeline. A clean machine therefore cannot yet reproduce a non-empty loadout end to end. Desktop onboarding is absent. |
| 2 | Preview drift | Partial | Production compose/explain domains and durable plan retrieval are wired through daemon and CLI. Plans bind live observed state and reject stale preimages. | `sync` does not fetch Git or materialize providers. Production planning consumes materialized-state files prepared out of band, so fetch → compose → provider materialize → explain → diff is not one production flow. |
| 3 | Apply safely | Partial | Content-bound plans, confirmation, idempotency, deterministic adapters, receipts, stale-target rejection, verification, and rollback are implemented and tested. Desktop can submit a reviewed plan ID. | The clean-machine/provider gap prevents a complete loadout-to-apply flow. Installed-app reconciliation is not exercised on the three-OS release matrix. |
| 4 | Verify parity | Partial | Filesystem, provider, service, CLI, and MCP verification contracts exist. | Desktop has no verification command/view, no shared cross-interface result contract proves parity, and installed-platform parity has not run. |
| 5 | Provision credential references | Partial | Credential readiness/apply/verify service routes and CLI commands exist. Env/file, BWS, macOS Keychain, Linux Secret Service, and Windows Credential Manager adapters have focused tests. | The production headless resolver wires env/file only; BWS and platform stores are not selectable there. Desktop is read-only for credential readiness. |
| 6 | Recover or roll back | Partial | Reconciliation recovery is durable and plan-bound. Snapshot restore and writer promotion now have authenticated durable plans/receipts, encrypted preimages, fresh-process recovery, startup discovery, lifecycle stop/start coordination, tamper rejection, and SQLite integrity tests. | The CLI has no snapshot command group. Installed daemon/CLI recovery and snapshot restore have not run on macOS, Linux, and Windows release artifacts. |
| 7 | Multiple targets and five-layer composition | Partial | Composition order, policy floor, provenance, target inventory, local execution, typed SSH transport, and controller-side portable-artifact staging are tested. Production composition is available through the daemon. | Production `headless.json` selects one local target. The SSH stager is not connected to a real provider → remote plan/apply flow, and no declared remote combination has completed the support matrix. |
| 8 | Scheduled read-only drift checks | Partial | `commonkitd` now constructs and runs `DriftScheduler`; persistence, overlap suppression, read-only verification, and degraded-state reporting are tested. | The general CLI `schedule` command is status-only despite the scoped `enable|disable|status` surface. Desktop exposes schedule state but no mutation. Installed unattended execution is unproven. |
| 9 | Optional MCP relay state | Partial | Rust relay configuration, runtime, lifecycle, transaction, provider convergence, HTTP upstream, service routes, stable endpoint, and legacy normalization/migration fixtures exist. | The required shared black-box suite has not run Node and Rust through tool discovery, calls, notifications, lifecycle, failure, and redaction equivalence. Desktop relay management is read-only; installed lifecycle remains a release gate. |
| 10 | Coding-agent adapters | Partial | Pinned APM 0.25.0 and chezmoi 2.70.4 isolation contracts, native fallback, provider artifacts, ownership validation, transactional apply, and checksum-pinned CI gates exist. Portable provider artifacts can be staged over typed SSH. | Production onboarding/service does not invoke providers; it reads pre-materialized JSON. Windows chezmoi CI verifies the binary version but not the isolation fixture. Real remote provider staging/reconciliation is not exercised. |
| 11 | Redacted diagnostics | Partial | Schema-bound redaction, service diagnostics, CLI export, and MCP export are tested. Desktop reads the diagnostics domain. | Desktop does not perform a diagnostic export, and no installed-platform malicious-input test proves the complete export path. |
| 12 | OSS onboarding, schemas, threat model, CI, packaging, and releases | Partial | Schemas, threat model, support/migration/release docs, three-OS Rust/Node/Tauri CI, provider gates, release packaging, signatures, SBOM, updater fixtures, and an install/update/uninstall workflow exist. | `README.md` remains the legacy Node-era onboarding and contradicts current transactional recovery. Production signing/notarization identities and two published signed versions have not exercised the lifecycle workflow. That workflow checks installation, upgrade, CLI version, and uninstall, but not reconciliation or snapshots through each installed product. |

No flow is fully met under the scope's product-level, cross-platform acceptance rule. Several underlying cores are complete; their remaining status is driven by missing production wiring or release evidence, not missing reconciliation primitives.

## Stale claims closed since the prior audit

- GitHub-backed create/connect onboarding, required layer creation, target registration, private roots, and `headless.json` generation now exist.
- Daemon composition and explanation, durable CLI diff, credential CLI commands, and production scheduler execution are wired.
- Snapshot restore and authoritative-writer promotion now support authenticated durable recovery across restart and lifecycle coordination.
- Portable provider artifacts can be integrity-checked and staged through the typed SSH boundary.
- Desktop management reads real plan, credential, snapshot, relay, schedule, and diagnostic domain state, and plan apply is daemon-authorized.
- Three-OS CI, signed release assembly, updater consent fixtures, and a published-release lifecycle workflow exist.

## Acceptance checklist

| Criterion | Result |
| --- | --- |
| All 12 flow contract suites | Not met: component suites exist, but no twelve-flow product suite covers provider materialization, all interfaces, and installed platforms. |
| macOS/Linux/Windows installation, reconciliation, snapshot, update, uninstall | Not met: automation covers install/update/version/uninstall only; production signed runs and installed reconciliation/snapshot coverage are absent. |
| Declared remote target combinations | Not met: typed SSH staging passes; production provider/reconciliation integration and remote support-matrix runs are absent. |
| Organization policy weakening rejected with provenance | Met at composition/core contract level. |
| Deterministic, redacted, input/observed-bound plans | Met for the implemented local provider/filesystem and relay planning paths. |
| Idempotent apply and failure recovery | Met for reconciliation and local filesystem operations; installed-platform evidence remains part of the release gate. |
| No secret or live SQLite in Git/diagnostics | Partial: focused scanners, redaction, and snapshot separation pass; a complete malicious-repository installed-flow gate is absent. |
| Snapshot restore integrity and rollback safety | Met at core/service contract level, including restart recovery and tamper rejection; installed-platform proof remains. |
| Rust relay black-box parity/migration | Not met: migration/configuration parity exists, not the required dual-runtime behavior suite. |
| Desktop/headless same domain state | Not met: desktop reads most domain state and applies plans, but verification, onboarding, and management mutations are missing. |
| Signed installers and updates verified | Not met in production: fail-closed automation and local updater fixtures exist without production identities, notarization, or a published two-version run. |
| Threat model, schemas, onboarding, support, migration docs | Partial: required reference documents exist; public onboarding remains stale. |

## Remaining implementation versus external release gates

Implementation work:

1. Connect Git fetch and pinned provider materialization to onboarding/service planning, then use the same pipeline for local and SSH targets.
2. Add production multi-target selection and exercise remote provider staging through semantic CommonKit plans and adapters.
3. Complete the scoped CLI surface for snapshots and drift-schedule enable/disable.
4. Wire BWS and platform credential resolvers into the production credential domain.
5. Complete desktop onboarding, verification, credential/snapshot/relay/schedule mutations, and diagnostics export.
6. Run one shared Node/Rust relay black-box contract suite across the full required behavior matrix.
7. Replace the legacy README with the Rust v1 installation and clean-machine workflow.
8. Extend installed-release tests to reconcile, verify, snapshot, restore, and recover—not only install and report a version.

External release evidence:

1. Supply production macOS signing/notarization and Windows signing identities.
2. Publish two signed versions and pass the lifecycle workflow on macOS, Linux, and Windows.
3. Record the declared remote-target support-matrix runs.

## Conclusion

Do not mark CommonKit v1 complete or release-ready. The differentiated reconciliation, policy, provider isolation, relay, snapshot, and service foundations are credible. Completion is now concentrated in provider-to-production wiring, full operator surfaces, cross-runtime relay parity, current onboarding documentation, and executable release evidence.
