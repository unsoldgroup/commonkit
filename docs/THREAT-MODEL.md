# CommonKit threat model

Status: implementation baseline

## Assets and trust boundaries

CommonKit protects organization policy, composed desired state, provider inputs,
target credentials, the owner's About Me claims and summaries, mutable databases,
relay credentials, plans, artifacts, backups, receipts, update metadata, durable
job metadata, checkpoints, and audit records. Trust boundaries exist at Git repositories, provider executables,
local control clients, the scheduler/client API, workers and remote targets,
MCP upstreams, credential stores, snapshot/object stores, and desktop
webview/native IPC. Remote MCP is an untrusted client of the authenticated
execution API, not a lifecycle authority.

## Required invariants

- Organization policy is a non-overridable floor; later layers may only tighten it.
- Providers compute in isolated workspaces and never receive a live-target mutation capability.
- External providers fail closed when the required isolation boundary is unavailable. On macOS, the v1 external-provider paths remain disabled and use supported native resources instead.
- Plans bind target identity, desired and observed state, policy, provider inputs, ownership, and artifacts.
- Only adapters mutate targets, after confirmation, with recoverable receipts, and with durable preimages wherever the resource type admits them. Package operations admit no preimage: they are additive and converge-forward, and their receipts record the absence of a preimage and the reason explicitly.
- Package installation executes arbitrary upstream code on the live target, outside CommonKit's isolation boundary, by design. It is gated by an organization-policy allowlist of permitted sources and by consent distinct from ordinary apply. The provider sandbox constrains what *computes desired state*, because that output is consumed as truth; package installation *executes declared operator intent* that was confirmed before apply. Constraining the first and disclaiming the second is deliberate, and is why macOS may refuse external provider execution while still running a package installer on the same host.
- Recovery verifies plans, artifacts, backups, and receipt chains and never reruns providers or secret resolution.
- Portable state, plans, receipts, logs, events, diagnostics, Git, and UI never contain secret plaintext.
- About Me text is encrypted at rest, is never committed to Git, and is returned
  to an agent only through the configured Loadout/project view.
- Agents may propose memories but cannot publish them. Direct statements require
  user review, and contradictions require an explicit user choice before the
  previous claim is superseded.
- Managed paths are relative, canonical, root-contained, case-normalized for the target, and reject symlink/reparse escapes.
- Git fetch never implies apply; dirty, diverged, untrusted, or policy-invalid state stops.
- Relay endpoints bind loopback by default, require authenticated control, and accept credential references only.
- Snapshots are consistent, integrity-checked, encrypted before upload, and restored through staging with rollback.
- Desktop capabilities are least-privilege and expose no unrestricted shell to the webview.
- Durable execution accepts structured, repository-declared manifests rather than shell command strings and binds idempotency keys to request digests.
- Non-loopback execution access requires HTTPS and distinct, revocable client and worker identities. TLS may terminate only at a trusted same-host proxy that overwrites `X-Forwarded-Proto`.
- Workers run unprivileged without a Docker socket. Production Linux workers use systemd scopes/cgroups; target sandbox policy governs repository writes and network access.
- Streaming diagnostics are redacted before object commit. Artifact access uses a target-local signing key and short expiry; object storage is encrypted at rest and replicated separately from SQLite backups.
- Authorization denials record only identity name, action, resource, and a safe reason.

## Principal threats and controls

| Threat | Required control |
| --- | --- |
| Malicious provider/package | Exact version and input digests, preflight policy, isolated staging, normalized output scan |
| Stale-plan substitution | Content-addressed plans and immediate binding/preimage revalidation |
| Path escape or type confusion | Capability roots, lexical validation, no-follow inspection, ownership/type collision checks |
| Crash during mutation | Exact-rollback operations run first and reverse-roll back before the durable forward-only barrier; after that barrier, startup recovery re-observes and converges only from the bound plan payload |
| Secret exfiltration | Reference-only portable state, scrubbed provider environments, central redaction/scanning |
| Malicious Git update | Trusted remote/revision validation, clean fast-forward rules, organization-floor validation |
| Local API abuse | Private endpoint, per-installation authentication, capability authorization, explicit consent |
| Relay upstream compromise | Stable local endpoint, strict changed-upstream validation, isolated credentials, bounded/redacted errors |
| Snapshot tampering | Client-side authenticated encryption, descriptor/content hashes, writer state machine |
| Supply-chain/update attack | Signed artifacts and updater metadata, checksums, pinned dependencies, SBOM |
| Durable job replay or lease race | Revision checks, digest-bound idempotency, monotonic fencing, authenticated client/worker roles |
| Remote execution boundary bypass | HTTPS enforcement, trusted proxy contract, unprivileged sandboxed workers, manifest-bound filesystem/network policy |
| Artifact or diagnostic disclosure | Pre-commit redaction, bounded artifacts, short-lived signed access, encrypted and separately replicated object storage |
| Personal-profile disclosure | SQLCipher at rest, separately provisioned key reference, view-scoped queries, bounded results, and content-free access records |
| False or unwanted memory | Draft review before first publish, evidence-bearing suggestions, rejection suppression, revision checks, and explicit contradiction resolution |
| Cross-project inference | Server-selected Loadout/project scope; callers cannot request an arbitrary profile view |
| Malicious package source or tap | Organization-policy allowlist of permitted sources, monotonic across layers; the declared source recorded in the receipt |
| Rotated or substituted package artifact | Resolved artifact digest recorded in the receipt chain and detected on subsequent verification, rather than blocked at install |
| Unintended package execution | Consent gate distinct from ordinary apply; converge-forward only, so no silent removal or downgrade |

## Residual risks

The v1 durable executor relies on a correctly configured TLS/sandbox proxy and
Linux service account. A public hosted-provider gateway, distributable
subscription OAuth, multi-worker scheduling, and direct S3 credential handling
require separate threat reviews before enablement.

A compromised but allowlisted package source can execute arbitrary code as the
target user during apply. CommonKit records the resolved artifact digest and
detects the resulting change on subsequent verification, but does not prevent
the execution. Digests are recorded as evidence rather than enforced as a
precondition, because upstreams rotate artifacts as routine maintenance while
the declared version stays put; enforcing them would fail installs for
legitimate reasons often enough to train operators into overriding the check,
which is a worse outcome than detection. This residual risk is the accepted
cost of managing packages at all.

## Release gate

The public release audit must exercise malicious provider output, unavailable
provider isolation (including macOS fail-closed behavior), traversal and
symlink/reparse attacks, stale plans and fences, revision and idempotency
conflicts, every crash boundary, secret canaries across all outputs, Git
trust-state transitions, relay and execution authentication, authorization
denials, corrupted snapshots and artifacts, desktop capability abuse, worker
sandbox escape attempts, update-signature failure, package installation from a
disallowed source, a rotated package artifact digest, and consent-gate bypass
attempts, on every supported platform.
