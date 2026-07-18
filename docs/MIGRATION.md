# Migration to CommonKit v1

## Agent context

Existing native Claude and Codex declarations remain supported during migration. New portable agent context should move to a pinned `apm.yml` and `apm.lock.yaml` with provider version 0.25.0. CommonKit stages, audits, plans, and applies APM output; do not run provider-native installs against a CommonKit-managed target.

## Home configuration

Chezmoi sources may migrate ordinary files, directories, modes, safe relative symlinks, and isolation-safe templates. Scripts, hooks, externals, destination-dependent behavior, `create_`, `modify_`, exact/removal semantics, provider secret access, and executable/network template functions must move to explicit CommonKit services, credentials, or native resources until parity is proven.

## MCP relay

Portable MCP declarations describe upstream intent. CommonKit owns the persistent relay at its stable loopback endpoint and generates client configuration pointing to it. Existing unmanaged relay entries are preserved; pruning CommonKit-owned entries requires an approved plan. Literal credential headers must become references.

## Portable state and databases

Git contains declarative layers, provider manifests/locks, policy, and snapshot descriptors only. Credential values, provider staging, artifacts, receipts, backups, SQLite databases, WAL/SHM files, and snapshot ciphertext remain outside the kit repository. Move mutable corpora through encrypted snapshots and explicit authoritative-writer promotion.

## Safe rollout

1. Import configuration and compose without applying.
2. Resolve ownership collisions and unsupported provider diagnostics.
3. Review the content-addressed plan and provider provenance.
4. Apply to one non-authoritative target and verify repeat apply is a no-op.
5. Exercise rollback and restart recovery.
6. Migrate relay and mutable snapshots only after filesystem parity.
7. Retain native fallback until APM/chezmoi output parity is demonstrated for the loadout.
