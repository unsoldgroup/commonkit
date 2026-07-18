# CommonKit threat model

Status: implementation baseline

## Assets and trust boundaries

CommonKit protects organization policy, composed desired state, provider inputs, target credentials, mutable databases, relay credentials, plans, artifacts, backups, receipts, and update metadata. Trust boundaries exist at Git repositories, provider executables, local control clients, remote targets, MCP upstreams, credential stores, snapshot object stores, and desktop webview/native IPC.

## Required invariants

- Organization policy is a non-overridable floor; later layers may only tighten it.
- Providers compute in isolated workspaces and never receive a live-target mutation capability.
- Plans bind target identity, desired and observed state, policy, provider inputs, ownership, and artifacts.
- Only adapters mutate targets, after confirmation, with durable preimages and recoverable receipts.
- Recovery verifies plans, artifacts, backups, and receipt chains and never reruns providers or secret resolution.
- Portable state, plans, receipts, logs, events, diagnostics, Git, and UI never contain secret plaintext.
- Managed paths are relative, canonical, root-contained, case-normalized for the target, and reject symlink/reparse escapes.
- Git fetch never implies apply; dirty, diverged, untrusted, or policy-invalid state stops.
- Relay endpoints bind loopback by default, require authenticated control, and accept credential references only.
- Snapshots are consistent, integrity-checked, encrypted before upload, and restored through staging with rollback.
- Desktop capabilities are least-privilege and expose no unrestricted shell to the webview.

## Principal threats and controls

| Threat | Required control |
| --- | --- |
| Malicious provider/package | Exact version and input digests, preflight policy, isolated staging, normalized output scan |
| Stale-plan substitution | Content-addressed plans and immediate binding/preimage revalidation |
| Path escape or type confusion | Capability roots, lexical validation, no-follow inspection, ownership/type collision checks |
| Crash during mutation | Apply-started checkpoint, immutable backups, reverse rollback, startup recovery |
| Secret exfiltration | Reference-only portable state, scrubbed provider environments, central redaction/scanning |
| Malicious Git update | Trusted remote/revision validation, clean fast-forward rules, organization-floor validation |
| Local API abuse | Private endpoint, per-installation authentication, capability authorization, explicit consent |
| Relay upstream compromise | Stable local endpoint, strict changed-upstream validation, isolated credentials, bounded/redacted errors |
| Snapshot tampering | Client-side authenticated encryption, descriptor/content hashes, writer state machine |
| Supply-chain/update attack | Signed artifacts and updater metadata, checksums, pinned dependencies, SBOM |

## Release gate

The public release audit must exercise malicious provider output, traversal and symlink/reparse attacks, stale plans, every crash boundary, secret canaries across all outputs, Git trust-state transitions, relay authentication, corrupted snapshots, desktop capability abuse, and update-signature failure on every supported platform.
