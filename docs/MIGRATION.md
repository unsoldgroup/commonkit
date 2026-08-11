# Migration to CommonKit v1

## Agent context

Existing native Claude and Codex declarations remain supported during migration. New portable agent context should move to a pinned `apm.yml` and `apm.lock.yaml` with provider version 0.25.0. CommonKit stages, audits, plans, and applies APM output; do not run provider-native installs against a CommonKit-managed target.

## Home configuration

Chezmoi sources may migrate ordinary files, directories, modes, safe relative symlinks, and isolation-safe templates. Scripts, hooks, externals, destination-dependent behavior, `create_`, `modify_`, exact/removal semantics, provider secret access, and executable/network template functions must move to explicit CommonKit services, credentials, or native resources until parity is proven.

## MCP relay

Portable MCP declarations describe upstream intent. CommonKit owns the persistent relay at its stable loopback endpoint and generates client configuration pointing to it. Existing unmanaged relay entries are preserved; pruning CommonKit-owned entries requires an approved plan. Literal credential headers must become references.

## Portable state and databases

Git contains declarative layers, provider manifests/locks, policy, and snapshot descriptors only. Credential values, provider staging, artifacts, receipts, backups, SQLite databases, WAL/SHM files, and snapshot ciphertext remain outside the kit repository. Move mutable corpora through encrypted snapshots and explicit authoritative-writer promotion.

Snapshot configuration now requires `portableState`, an absolute directory inside the checked-out Git kit. Existing installations should create that directory, add it to `snapshots.portableState`, make a new snapshot, and commit/push both the generated `snapshots/<digest>.json` descriptor and `authority/<database>.json`. The authority record is authenticated with the separately provisioned snapshot key and binds the current writer, accepted history head, generation, previous authority revision, and accepted descriptor. It contains no key or database plaintext. Every receiving machine must independently provision the same snapshot-key reference and object-store credentials.

Snapshot commits have a two-level compare-and-swap contract. CommonKit performs a local CAS against the exact authenticated authority revision read before create or promotion. Git sync must then push the commit only when the fetched repository commit is still the remote parent (ordinary non-force, fast-forward-only push); rejection requires fetching the new authority and replanning, never merging two authority records or force-pushing. This repository revision check is what prevents independently checked-out machines from both publishing locally valid generations. Deleting or rolling back the authority record or its accepted descriptor fails closed. Existing kits with descriptors but no authority record require an explicit migration from a verified head rather than automatic reinitialization. Promotion no longer accepts caller-supplied writer/candidate digests: add `observedPaths` entries for every locally inspectable candidate, or leave promotion unavailable until a target inspection transport is configured.

Authority consumers must keep the authenticated monotonic anchor outside `portableState` in protected target-local state. A fresh or lagging clone must call the repository-bound read contract with both its checked-out commit and an independently fetched trusted remote branch head; they must match. Authority mutations must use the published CAS contract, which builds the change in an isolated staged tree and asks the Git publisher to update the expected remote parent before installing it locally. The publisher must reject non-fast-forward updates. These two checks are complementary: the local anchor detects whole-tree replay on a machine that has seen newer state, while the trusted remote-head comparison protects a fresh machine.

Git-backed authority also requires the protected append-only remote ref
`refs/commonkit-authority/<database>`. Its commit ancestry is the generation chain. Configure the
Git server to reject deletion and non-fast-forward updates to `refs/commonkit-authority/*`; on
GitHub this requires a repository ruleset for that namespace with bypass disabled. For a new kit, set
`snapshots.gitAuthority.bootstrap: true` only for the first initialization; CommonKit may use
it only when the portable authority record is absent, and atomically publishes the genesis
record and anchor. Existing kits must not use bootstrap as recovery. If their anchor namespace
is missing, startup fails closed and an operator must investigate remote ref deletion or
rollback rather than recreating trust from the checked-out branch.

Writer promotions persist `prepared`, `cas_confirmed`, or `aborted` state outside the portable tree. Startup commits a prepared promotion only when authenticated portable authority names its candidate; if portable authority still names the previous writer, startup records an abort. An authenticated publish intent makes a crash after immutable history but before the mutable pointer repairable without treating an ordinary stale-pointer replay as a valid transition.

## Safe rollout

1. Import configuration and compose without applying.
2. Resolve ownership collisions and unsupported provider diagnostics.
3. Review the content-addressed plan and provider provenance.
4. Apply to one non-authoritative target and verify repeat apply is a no-op.
5. Exercise rollback and restart recovery.
6. Migrate relay and mutable snapshots only after filesystem parity.
7. Retain native fallback until APM/chezmoi output parity is demonstrated for the loadout.

The installed restore and migration rehearsal is documented in
[`docs/runbooks/restore-rehearsal.md`](runbooks/restore-rehearsal.md). It uses
isolated target roots and never treats Linux evidence as acceptance of the
macOS link inventory or native lifecycle.

## Release and update migration

Choose an explicit update channel and review its signed release notes before migration. Do not switch an existing installation to desktop auto-update until updater metadata signature verification and rollback guidance have been exercised on that platform. Standalone CLI users update through the matching signed release artifact; installers must preserve portable kit data by default while removing application-managed services and startup entries on uninstall.
