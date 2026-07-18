# CommonKit v1 support matrix

This matrix is a release gate, not a claim that unchecked rows are production-ready.

| Capability | macOS | Linux | Windows | Remote macOS/Linux |
| --- | --- | --- | --- | --- |
| Composition, policy, plans, receipts | Implemented | CI contract | CI contract | Transport-neutral |
| Local filesystem reconciliation | Implemented | CI contract | CI contract; ACL/reparse release tests required | Typed SSH boundary |
| APM 0.25.0 Claude + Codex | Real release verified (arm64); x86_64 gated | Contract implemented; release binaries gated | Contract implemented; x86_64 gated, arm64 unavailable upstream | Deferred until remote staging harness |
| chezmoi 2.70.4 safe subset | Isolation contract | Isolation contract | Isolation contract | Deferred until remote staging harness |
| Credentials | env/file/BWS; absolute-path Keychain CLI | env/file/BWS; absolute-path Secret Service CLI | env/file/BWS; native Credential Manager API, Windows CI gated | Target-local only |
| Snapshots | Provider-neutral core | Provider-neutral core | Provider-neutral core | Object-store transport-neutral |
| Desktop | Tauri build gate | AppImage/deb gate | Signed installer gate | Manages through daemon |
| Standalone CLI | Universal signed archive gate | Signed archive gate | Signed archive gate | Same control API |

Windows remote management is out of v1; local Windows is required. Linux v1 packaging targets AppImage and Debian-family `.deb`. GitHub CLI is the initial GitHub authentication path. Snapshot storage remains S3-compatible and provider-neutral; Cloudflare R2 is the recommended deployment profile. BWS is the initial external secret-manager adapter.

No row is release-ready until its installer, reconciliation, recovery, update, and uninstall tests pass on the named platform.

Release packaging and lifecycle workflows are validation-only and deliberately disabled until production signing identities and real two-version fixtures are provisioned. A green ordinary CI run does not imply installer or updater support.
