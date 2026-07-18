# CommonKit threat model

## Assets and boundaries

Portable manifests, job metadata, audit records, checkpoints, and artifacts cross the client/scheduler boundary. Secret values, provider credentials, worker identity, and artifact-signing keys stay target-local. Remote MCP is an untrusted client of the authenticated execution API; it is not a lifecycle authority.

## Required controls

- Reject inline secrets, shell command strings, path traversal, symlink escapes, oversized artifacts, stale fences, revision conflicts, and idempotency-key digest conflicts.
- Require HTTPS for non-loopback execution access and distinct revocable client/worker bearer identities. Terminate TLS only at a trusted same-host proxy which overwrites `X-Forwarded-Proto`.
- Run workers unprivileged without a Docker socket. Production uses systemd scopes/cgroups; repository writes and network access follow manifest policy enforced by the target sandbox.
- Redact streaming diagnostics before object commit. Never store bearer tokens or resolved secret values in SQLite, events, receipts, API errors, or artifact metadata.
- Sign artifact access with a target-local key and short expiry. Keep object storage encrypted at rest and replicate it separately from SQLite backups.
- Audit denied authorization decisions using identity name, action, resource, and safe reason only.

## Residual risks

The v1 binary relies on a correctly configured TLS/sandbox proxy and Linux service account. A public hosted-provider gateway, distributable subscription OAuth, multi-worker scheduling, and direct S3 credential handling require separate threat reviews before enablement.
