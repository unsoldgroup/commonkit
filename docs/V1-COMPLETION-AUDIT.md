# CommonKit v1 completion audit

Audit date: 2026-07-21
Audit basis: the current `main` policy checkpoint. “Met locally” means the Rust production path, desktop contracts, and installed unsigned lifecycle pass on the macOS arm64 audit host; it does not substitute for production signing/notarization, a published updater, or manually recorded native multi-OS evidence. CommonKit does not use GitHub Actions, and GitHub-hosted status is not a completion or release gate.

## Core eight-flow status

The eight original CommonKit runtime goals are implemented and locally evidenced: target initialization; drift preview; safe reconciliation; parity verification; independent credential-reference provisioning; recovery and rollback; multiple targets with five-layer composition; and scheduled read-only drift checks. These are necessary but not sufficient for v1 release readiness. The complete v1 contract also includes relay management, coding-agent providers and SkillOpt, redacted diagnostics, cross-platform desktop/headless delivery, Git-backed portable state, encrypted snapshots, and the release lifecycle.

## Twelve-flow matrix

| # | Flow | Implementation status | Current evidence | Remaining gate |
| --- | --- | --- | --- | --- |
| 1 | Initialize a target | Met locally | `commonkit init create|connect` creates or clones a kit, writes the composition layers and target registration, establishes private runtime roots, validates pinned native/APM/chezmoi inputs, materializes an initial provider plan, and writes `headless.json`. Publication is protected by a private fsynced transaction journal: restart recovery finalizes an observed pushed revision, rolls back an unpushed transaction, and fails closed on ambiguous remote advancement or tampered artifacts. Desktop onboarding uses the same bounded provider choices. The unsigned installed lifecycle passed on macOS arm64. | Installed first-run evidence on the remaining declared platform matrix. |
| 2 | Preview drift | Met locally | Git-backed provider materialization, composition/explanation, live observation, durable plan retrieval, deterministic diff, stale-input rejection, and local/typed-SSH planning are implemented. Provider-pipeline, service, CLI, and SSH contracts are green on the audited commit. | Native platform and declared remote support-matrix evidence. |
| 3 | Apply safely | Met locally | Plans are content-bound and revalidated before apply. Deterministic adapters provide confirmation, idempotency, receipts, verification, reverse rollback, typed-SSH dispatch, capability-bound filesystem access, and fresh-process reconstruction from durable artifacts. The macOS installed lifecycle completed reconcile and verify. | Signed-artifact execution on all three OSes. |
| 4 | Verify parity | Met locally | Service, CLI, CommonKit control surface, and desktop expose the production verification domain; provider integrity and observed target parity share the same bound plan state. | Installed three-OS interface-parity evidence. |
| 5 | Provision credential references | Met locally | Credential plan/apply/verify are exposed through service, CLI, and desktop. The reviewed plan ID binds destination, reference, and observed state; apply resolves all references before mutation. Env/file and BWS contracts avoid persisting secret plaintext. File and recovery metadata are durably fsynced, overlapping destinations are rejected, and sync failures fail closed. The installed lifecycle exercises the redacted CLI/daemon plan → apply → verify path. | Manually recorded execution against supported native credential stores. |
| 6 | Recover or roll back | Met locally | Reconciliation, onboarding, credentials, and snapshot restore/promotion use durable plans/journals and receipts, content-addressed artifacts and preimages, reverse-order rollback, tamper rejection, process-restart discovery, Git compare-and-swap publication, and durable authority anchors. Credential SIGKILL recovery restores exact prior bytes—including empty files—without re-resolving secrets; onboarding restart recovery is bound to observed remote HEAD. | Passing installed daemon/CLI recovery on Linux and Windows artifacts. |
| 7 | Multiple targets and five-layer composition | Met locally | `public base → organization policy → personal kit → project loadout → target override` composition, monotonic non-overridable organization policy, provenance, local/SSH inventory, consent-bound selection, target-scoped execution, per-target roots, portable artifact staging, and typed-SSH recovery are implemented. Create/connect validate the complete selected composition before registration or provider side effects. | Recorded remote-target support-matrix runs. |
| 8 | Scheduled read-only drift checks | Met locally | `commonkitd` constructs `DriftScheduler`; persistence, live interval reconfiguration, enable/disable/status, real concurrent overlap suppression, read-only verification, degraded reporting, and CLI/desktop controls have focused coverage. The installed lifecycle proves enable → observed drift tick → disable without daemon restart. | Installed unattended execution evidence on Linux and Windows. |
| 9 | Optional MCP relay state | Met locally | The Rust relay implements persistent lifecycle, transactional reconciliation, loopback authentication, stable endpoints, HTTP upstreams, failure handling, migration, credential isolation, and redaction. Shared Node/Rust black-box compatibility has passed locally. | Passing manually invoked installed lifecycle on the release matrix; legacy retirement remains a later decision. |
| 10 | Coding-agent providers and SkillOpt | Met locally, native matrix pending | APM 0.25.0, chezmoi 2.70.4 safe isolation, native fallback, provider artifacts, ownership validation, production Git-provider materialization, independent SkillOpt harness isolation, fresh Git/policy checks, and authenticated canary recovery have focused coverage. The official APM binary has been exercised only on macOS arm64. | Native provider/isolation matrices and installed provider execution across every declared OS/architecture. |
| 11 | Redacted diagnostics | Met locally | Schema-bound diagnostics are exposed by service, CLI, control surface, and desktop; focused redaction, secret scanning, and bounded-output contracts pass locally. | Installed malicious-input export evidence on supported platforms. |
| 12 | OSS onboarding, schemas, threat model, manual validation, packaging, and releases | Partial | README, schemas, threat model, support/migration/release docs, platform-neutral release scripts, provider gates, packaging, SBOM, updater fixtures, and installed-lifecycle automation exist. The unsigned macOS lifecycle passed. An ad-hoc-signed arm64 bundle was installed at `/Applications/CommonKit.app`, launched as an accessory/status-bar process, started or attached to its bundled daemon, and reported relay health. Tauri updater handoff is durable on Windows and fails closed without signing inputs. | Production signing/notarization identities, two published signed versions, and the manually invoked signed install → reconcile → verify → snapshot/restore → recover → update → uninstall matrix remain required. |

## Verification evidence for the audited worktree

- `cargo test --workspace --all-features --locked`: passed, including provider isolation with temporary loopback access enabled for the test harness.
- `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`: passed.
- `pnpm test`: passed (25 CommonKit contracts and 15 relay contracts).
- `pnpm --dir apps/desktop test`: 43 tests passed.
- `pnpm --dir apps/desktop typecheck`: passed.
- The APM real-binary and one chezmoi real-binary test remain explicitly opt-in because they require pinned external release binaries; their native platform matrix is not satisfied by the ordinary workspace run.
- `scripts/installed-lifecycle.sh <isolated-scratch> target/debug`: passed on macOS arm64, including daemon install/restart, onboarding reload, provider plan/apply/verify, credential plan/apply/verify/redaction, scheduler enable/tick/disable, snapshot create/restore, fresh-process recovery, typed remote-helper execution, and uninstall.
- A local arm64 Tauri application bundle built successfully, passed strict ad-hoc code-signature verification after local signing, installed at `/Applications/CommonKit.app`, and remained running as a macOS accessory/status-bar process. Its application icon uses the same Ck geometry as the monochrome tray icon.
- The isolated Hetzner VPS is reachable, but acceptance stopped before authentication because every presented host key differs from `known_hosts`: ED25519 `SHA256:aANF/QC9DUw1kvu7FCFGqeKibvH8ogAiRbxkBnS3Lzw`, RSA `SHA256:kZZPSOaMcuolC3/uYaDq17uMPP6SOTRasAThKXgGlzU`, and ECDSA `SHA256:oKc1G2YCUdiTI1sDrL9IP+0cCLurV9Foh07Z5LVUjPY`. At least one fingerprint must be independently confirmed before `known_hosts` is changed. No VPS state was changed.

Two findings from an earlier audit are now resolved: the statement that the production daemon does not construct/run a `DriftScheduler` is no longer true, and the statement that management panels are placeholders is no longer true. Their focused runtime and desktop contracts now pass, but installed-platform evidence is still outstanding.

## Priority findings

Finding count: **P0: 0; P1: 3**.

### P0

None identified in the audited committed paths.

### P1

1. Independently confirm the VPS ED25519 host-key fingerprint, then complete the isolated acceptance run; the current attempt stopped before authentication and made no changes.
2. Supply production Apple signing/notarization, Windows signing, and Tauri updater identities; publish two signed versions; then run the signed release lifecycle on macOS, Linux, and Windows.
3. Record the declared remote-target and native provider/isolation support matrices. Local contracts and one macOS-arm64 APM run do not prove the advertised platform matrix.

## Local implementation versus external release gates

Locally implemented and evidenced on the committed audit basis: bounded onboarding; five-layer composition and policy; provider-backed desired state; deterministic, content-addressed planning; local and typed-SSH target execution; capability-safe filesystem mutation; verification; credential references; receipts and restart recovery; durable encrypted snapshots and authoritative-writer promotion; drift scheduling; relay parity; diagnostics; CLI and desktop management; SkillOpt safety contracts; packaging automation; and fail-closed updater fixtures.

Implementation evidence still required before external release qualification:

1. Independently confirm the changed VPS host key, then complete the isolated VPS acceptance run using temporary directories and ports. The local macOS desktop/menu-bar install and unsigned installed lifecycle are now evidenced.

External or native-platform evidence still required:

1. Production Apple signing/notarization, Windows signing, and Tauri updater identities.
2. Two published signed versions and a successful install → reconcile → verify → snapshot/restore → recover → update → uninstall run on macOS, Linux, and Windows.
3. Recorded declared remote-target support-matrix runs.
4. Manually recorded native provider/isolation gates for every supported OS/architecture combination available from the pinned providers.

## Conclusion

Do not mark CommonKit v1 release-ready. The first eight runtime flows and flows 9–11 are locally implemented and covered by production-path contracts, and the expanded unsigned installed lifecycle plus real Mac status-bar install pass. Flow 12 remains partial because VPS acceptance is blocked pending independent host-key confirmation and external signing, publication, updater, native-platform, provider, and remote-target evidence is still outstanding. No P0 finding was identified in this audit.
