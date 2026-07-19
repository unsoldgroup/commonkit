# CommonKit v1 support matrix

This matrix is a release gate, not a claim that unchecked rows are production-ready.

| Capability | macOS | Linux | Windows | Remote macOS/Linux |
| --- | --- | --- | --- | --- |
| Composition, policy, plans, receipts | Implemented | CI contract | CI contract | Transport-neutral |
| Local filesystem reconciliation | Implemented | CI contract | CI contract; ACL/reparse release tests required | Typed SSH boundary |
| APM 0.25.0 Claude + Codex | Fails closed; native fallback only until a privileged helper or VM boundary exists | Contract implemented; release binaries gated | Contract implemented; x86_64 gated, arm64 unavailable upstream | Controller-isolated materialization for SSH targets; verified portable artifacts staged through the typed helper |
| chezmoi 2.70.4 safe subset | Fails closed; native fallback only until a privileged helper or VM boundary exists | Isolation contract | Isolation contract | Controller-isolated materialization only when controller and target platform/architecture match; otherwise fail closed and use native resources |
| Credentials | env/file/BWS; absolute-path Keychain CLI | env/file/BWS; absolute-path Secret Service CLI | env/file/BWS; native Credential Manager API, Windows CI gated | Target-local only |
| Snapshots | Provider-neutral core | Provider-neutral core | Provider-neutral core | Object-store transport-neutral |
| Desktop | Tray health, onboarding, target/loadout management, relay, snapshots, schedules, diagnostics, and updater implemented; signed/notarized lifecycle gated | Same management contract; AppImage/`.deb` installed lifecycle gated | Same management contract; MSI/NSIS installed lifecycle gated | Manages declared targets through the local authenticated daemon; no desktop is installed on headless targets |
| Standalone CLI | Universal signed archive gate | Signed archive gate | Signed archive gate | Same control API |
| Unattended daemon | launchd per-user service implemented; signed installed lifecycle gated | systemd user service implemented with explicit foreground fallback; installed lifecycle and isolated SSH smoke wired | Per-user Scheduled Task implemented with locale-independent numeric status; signed runner gated | Each VPS/remote machine runs its own target-resident daemon and loopback relay |
| Target helper | Packaged and exercised through typed stdin | Packaged; isolated real-`sshd` CI smoke wired | Packaged for local tooling; remote Windows out of v1 | Required on SSH targets; fixed `commonkit-target-helper --stdio-v1` command |
| SkillOpt sleep optimization | macOS sandbox implementation and held-out lifecycle contracts | Fails closed: proven process-isolation backend unavailable | Fails closed: proven process-isolation backend unavailable | Not executed remotely in v1 |

Windows remote management is out of v1; local Windows is required. Linux v1 packaging targets AppImage and Debian-family `.deb`. GitHub CLI is the initial GitHub authentication path. Snapshot storage remains S3-compatible and provider-neutral; Cloudflare R2 is the recommended deployment profile. BWS is the initial external secret-manager adapter. SkillOpt is optional in v1 and is enabled only where its isolation contract has been proven; unsupported platforms reject it rather than weakening isolation.

Pinned APM and chezmoi commands execute inside a proven operating-system boundary:
Linux bubblewrap or a no-network Windows AppContainer. macOS rejects external
provider execution because its unprivileged process boundaries cannot guarantee
termination of deliberately detached descendants; the native provider remains
available until a privileged disposable-user helper or VM boundary is shipped. The
provider receives read access only to operating-system runtime files, its
executable, and declared inputs, and write access only to its disposable
CommonKit workspace. Missing sandbox
capability, an undeclared path, or a network attempt fails closed before target
mutation. Hosted provider jobs preflight the platform boundary before exercising
the checksum-pinned releases.

No row is release-ready until its installer, reconciliation, recovery, update, and uninstall tests pass on the named platform.

Release packaging and lifecycle workflows are executable and fail closed without production signing identities. They have not been demonstrated with production notarization/signing and a published two-version updater path. A green ordinary CI run does not imply installer or updater support.
