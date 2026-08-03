# Durable execution and browser benchmark offload v1

Tracking: USG-46

`commonkit-execd` is a Rust scheduler and single-Linux-target worker supervisor. It extends the UNS-1274 Rust workspace and remains separate from loopback-only `commonkitd` reconciliation.

## Contract

- `ExecutionManifest` is versioned, canonically hashed, pinned to an exact repository revision, and rejects shell strings and inline secrets. Arguments are an array and secret values remain target-local.
- A `Job` is immutable. Retries create revisioned `Attempt` records.
- State transition and append-only event writes share one SQLite transaction. Events have per-job monotonic sequences and unique IDs; consumers deduplicate at-least-once delivery by ID.
- Worker leases carry monotonically increasing fencing tokens. Expired or stale workers cannot checkpoint, publish artifacts, or complete.
- Immutable object bytes are written before artifact metadata. Metadata uniqueness makes shard checkpoints and aggregate publication exactly once.
- Local worker execution uses a systemd user scope with cgroup CPU/memory limits in production. Process-group mode exists for unsupported/test environments. Cancellation signals the complete process group, waits the manifest grace period, then kills it while retaining redacted diagnostics.
- Remote listeners require trusted TLS termination. Client and worker bearer identities have distinct capabilities; tokens are hashed in memory and never persisted. Authorization denials are audited without token values.
- Artifact downloads use signed five-minute URLs. Paths, sizes, and metadata are validated before commit.

## Remote MCP

The MCP server injects a repository-owned `ExecutionContext`: issue ID, current plan, skill names, and a map of task IDs to immutable manifests. Remote agents can list this context, submit declared tasks, reconnect to jobs/events, cancel, and retrieve results. They cannot submit arbitrary argv.

## Browser benchmarks

Functional profiles pin Chromium revision, OS, fonts digest, locale, timezone, viewport, DPR, and headless mode. Each shard is a checkpoint boundary. Completed shards survive retry; aggregate artifacts use a unique job/name commit. Environment digest mismatches are rejected as incomparable. Performance profiles additionally require an exact target machine-class capability and reserved 2x browser memory headroom.

## Operations

Run behind an authenticated TLS proxy on the VPS:

```sh
COMMONKIT_EXECD_CLIENT_TOKEN='from-secret-provider' \
COMMONKIT_EXECD_WORKER_TOKEN='from-secret-provider' \
COMMONKIT_EXECD_ARTIFACT_SIGNING_KEY='from-secret-provider' \
COMMONKIT_EXECD_OBJECT_ENCRYPTION_KEY='from-secret-provider' \
commonkit-execd --listen 127.0.0.1:7341 --database /var/lib/commonkit-execd/jobs.db \
  --target /etc/commonkit/execution-target.json --worker-id linux-vps \
  --policy /etc/commonkit/execution-policy.json \
  --secrets /etc/commonkit/secrets.json \
  --tasks /etc/commonkit/execution-context.json \
  --workspace-root /var/lib/commonkit-execd/workspaces \
  --object-root /var/lib/commonkit-execd/objects
```

Each repository is cached under `--workspace-root` as a bare mirror and each job
runs in its own detached worktree at the manifest revision, removed when the job
reaches a terminal state unless `--keep-failed-workspaces` is set. Preparation
runs after the runtime policy check, so a denied repository is never contacted.

`--secrets` is an operator-rendered JSON object of `env://NAME` to value, refused
unless it is owner-only. Its keys become the target's `ready_secret_refs`, so
placement reflects what the target can actually resolve. Without it no manifest
may declare `secretRefs`.

`--tasks` points at the repository-declared task manifests and turns on GitHub
commit statuses, which require `env://GITHUB_STATUS_TOKEN` in the secret file. A
job is matched back to its declared task by repository, workdir, and argv, so no
job-to-task mapping is persisted. Statuses are posted as `commonkit/<taskId>`,
namespaced so they cannot collide with another producer's contexts. Posting is
best effort and retried only on transport failures, 429, and 5xx; GitHub being
unreachable never changes a job's outcome. CommonKit does not use GitHub Actions.

Setting `COMMONKIT_EXECD_WEBHOOK_SECRET` alongside `--tasks` exposes
`POST /execution/v1/github/webhook`, which submits every declared task of the
matching repository at the revision a `push` or `pull_request` delivery reports.
Deliveries authenticate by `X-Hub-Signature-256` over the raw body and by nothing
else; an unsigned or mis-signed delivery is refused and audited. The payload
contributes only that revision — the command, target, ceilings, and secrets all
come from the committed declaration — and `<taskId>:<sha>` idempotency means a
redelivery returns the original job rather than running the suite twice. A
delivery that names no runnable revision (a ping, a branch deletion, a closed
pull request) is accepted and submits nothing, so GitHub keeps the hook healthy.

## Deploying a Linux execution target

`scripts/deploy-execd.sh` builds, installs, and starts the daemon as a systemd
user service, refusing to deploy to a target missing any command
`commonkit.ci-loadout.json` requires. `scripts/provision-execd-host.sh` does the
four things the daemon cannot do for itself: a git credential store, so a private
repository's mirror can be fetched without a token ever appearing in a manifest;
the public DNS record; the TLS certificate and reverse-proxy vhost; and a
firewall allowlist opening 443 to GitHub's published webhook source ranges only,
because an execution daemon has no reason to present its TLS stack to the whole
internet when one caller needs it. `scripts/smoke-execd.sh` submits one declared
task at one revision and waits, which is what the webhook does, so a green smoke
run is evidence the webhook path runs the same thing.

The toolchain a job uses must live where the sandbox can see it. The sandbox
binds `/usr`, `/bin`, `/lib`, `/lib64`, the workspace, and a scratch directory —
so a toolchain installed under a user's home is invisible to every job, and a
Rust installation belongs under `/usr/local` with links to the real toolchain
binaries rather than to `rustup`'s shims, which need environment the sandbox
clears.

Back up with the scheduler's online SQLite backup API and copy the resulting database plus encrypted object-store replica. Startup opens WAL mode and `recover_expired` turns abandoned attempts into interrupted attempts and queues policy-permitted retries.

## Explicit v1 limits

One authoritative Linux worker is supported. HA, Postgres, live process migration, arbitrary hosted shell, and cross-machine performance comparison are excluded. Claude and Codex engine references remain opaque behind the Orca implementor contract.
