# USG-51 — Delegate CI jobs to a capability-labelled Execution Target

Parent scope: USG-46 (`docs/scopes/durable-execution-v1.md`). Status: plan only, no code landed.

## What already exists

The durable execution core is built and tested. Nothing in this ticket requires
new scheduler, lease, artifact, or authorization work.

| Capability | Where | State |
|---|---|---|
| Manifest contract, canonical hash, argv-array-only | `commonkit-contracts`, `schemas/execution-manifest.schema.json` | Done |
| Capability-labelled placement (`manifest.requiredCapabilities ⊆ target.capabilities`) | `commonkit-execution/src/lib.rs:695` | Done |
| Placement explanations (`capability_missing`, resource, secret readiness) | `commonkit-execution/src/lib.rs` | Done |
| Job/attempt/event durability, leases with fencing tokens | `commonkit-execution` | Done |
| Worker loop, systemd-scope supervision, cancellation, diagnostics | `commonkit-execd/src/worker.rs`, `commonkit-execution/src/supervisor.rs` | Done |
| Distinct client/worker bearer identities, TLS-proxy guard, audit on denial | `commonkit-execd/src/lib.rs` | Done |
| Execution policy (repo allowlist, resource ceilings, network, repo-write) | `commonkit-execd/src/lib.rs` | Done |
| MCP front: `get_execution_context`, `submit_task`, `get_job`, events, artifacts | `commonkit-mcp/src/lib.rs:752+` | Done |
| Repository-declared task manifests loaded from `COMMONKIT_EXECUTION_CONTEXT` | `commonkit-mcp/src/main.rs` | Done |

**A CI capability label needs no new code.** It is a `StableId` in
`ExecutionTarget.capabilities` plus the same string in each manifest's
`requiredCapabilities`. The scheduler already enforces the subset check.

## Real gaps

The ticket's four-item gap list is right about deployment but understates two
code gaps and misses the trigger.

### G1 — Workspace preparation does not exist (blocker)

`ProcessSupervisor::spawn_with_environment` calls `verify_workspace`
(`supervisor.rs:322`), which requires the workspace root to *already* be:

- a git repo whose `HEAD` equals `manifest.repositoryRevision` exactly,
- whose `origin` URL equals `manifest.repository`,
- and clean.

Nothing in `execd` ever clones, fetches, or checks out. `main.rs` passes a
single process-wide `--workspace-root` to every job, so even with a manual
checkout only one repo at one revision could ever run. CI is inherently
many-repos-many-revisions. This is the largest piece of work in the ticket.

### G2 — Secret resolution is stubbed out

`worker::run_once` takes `resolved_secrets: &BTreeMap<String, String>` and
`main.rs:88` passes `&BTreeMap::new()`. Any manifest with a non-empty
`secretRefs` fails with `execution_secret_denied`. The GitHub token needed for
commit-status posting is exactly such a secret. `ExecutionTarget.ready_secret_refs`
is likewise never populated, so secret-readiness placement is dead.

### G3 — Nothing posts GitHub commit status

There is no GitHub code anywhere in the repo. The confirmed decision (commit
Status API, secret-provider token, context string must not collide with
`Workers Builds`) has no implementation.

### G4 — No CI ExecutionContext for this repository

`COMMONKIT_EXECUTION_CONTEXT` expects a JSON file of task-ID → manifest. None is
committed. The four checks in `CLAUDE.md` must become four immutable manifests.

### G5 — No trigger

The ticket assumes results post back to GitHub but never says what submits the
job on a push. Nothing listens for GitHub events. Without this, CI is
agent-initiated only.

### G6 — Not deployed

VPS `srv1833518` (8 cores, 32 GB, 297 GB free) has no `commonkit-execd` binary,
no `/etc/commonkit`, no unit. Caddy 2.6.2 is already running and terminates TLS
for `board.unsold.cloud`, so the TLS proxy is a vhost, not new infrastructure.
No `actions-runner` on this host — the interim runner from EXP-2302 lives
elsewhere.

## Proposed sub-issues

Ordered; each is independently landable and verifiable.

### 1. Per-job workspace preparation (G1)

- Add `commonkit-execd::workspace::prepare(manifest, root) -> PathBuf`.
- Bare mirror cache at `<root>/mirrors/<sha256(repository)>.git`; `git fetch --prune`
  then `git worktree add --detach <root>/jobs/<jobId> <revision>` — cheap per job,
  no full re-clone per commit.
- Pass the per-job path to `spawn_with_environment` instead of the shared root.
- Refuse `repository` values outside the execution policy allowlist *before*
  fetching (the policy check already runs first — assert ordering in a test).
- Remove the worktree on terminal state; leave it on failure when
  `--keep-failed-workspaces` is set.
- Tests: revision mismatch rejected, dirty worktree rejected, two concurrent jobs
  on different revisions of the same repo, disallowed repository never fetched.

### 2. Secret resolution provider (G2)

- `--secrets <path>`: JSON `{"env://NAME": "value"}` read at startup, file mode
  enforced `0600`, never logged, values already redacted from diagnostics by
  `finish(status, &secret_values)`.
- Populate `ExecutionTarget.ready_secret_refs` from the loaded keys so
  placement's secret-readiness check becomes live.
- On the VPS the file is rendered from `bws` at deploy time, not committed.
- Workspace preparation clones over HTTPS with `GIT_TERMINAL_PROMPT=0`, so a private
  repository needs a credential source. Resolve it here rather than in the manifest:
  a git credential helper configured on the target, never a token embedded in
  `manifest.repository`.
- Tests: missing ref fails with `execution_secret_denied`; resolved ref reaches
  the child env; secret value absent from stdout/stderr artifacts.

### 3. CI capability, target, and policy (G4, partial G6)

- Capability label: `ci-linux-x64` (a `StableId`).
- Commit `commonkit.execution-target.example.json` and extend
  `commonkit.execution-policy.example.json` with the CI repository allowlist.
- Commit `commonkit.execution-context.json` declaring four tasks, each with
  `requiredCapabilities: ["ci-linux-x64"]`, `repositoryWrite: false`,
  `networkPolicy: "restricted"`, artifact globs for test output:
  - `ci-cargo-test` → `cargo test --workspace --all-features --locked`
  - `ci-cargo-clippy` → `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`
  - `ci-pnpm-test` → `pnpm test`
  - `ci-desktop-check` → `pnpm --dir apps/desktop test && typecheck` (two manifests; argv is an array, no shell chaining)
- Test: every committed manifest passes `ExecutionManifest::validate` and matches
  the schema.

### 4. GitHub commit-status poster (G3)

- New `commonkit-execd` component that, on terminal job state, POSTs to
  `/repos/{owner}/{repo}/statuses/{sha}` with token from `env://GITHUB_STATUS_TOKEN`.
- Context strings: `commonkit/ci-cargo-test`, etc. Namespaced under `commonkit/`
  so they cannot collide with `Workers Builds`.
- `target_url` points at the signed artifact/log URL for that job.
- Failure to post is retried with backoff and never fails the job.
- Tests against a stub HTTP server: pending on submit, success/failure mapping,
  context string, retry on 5xx, no token leak in logs.

### 5. GitHub webhook trigger (G5)

Decided 2026-07-28 (Al): option A below.

- `POST /execution/v1/github/webhook`, unauthenticated by bearer but validated by
  `X-Hub-Signature-256` HMAC against `env://GITHUB_WEBHOOK_SECRET`, constant-time
  compare, body size capped.
- Handles `push` and `pull_request` (`opened`, `synchronize`, `reopened`); every
  other event is a 204 no-op.
- Maps repository + head SHA to the declared CI task IDs, rewriting each
  manifest's `repositoryRevision` to the head SHA — argv, capabilities, and
  resources stay exactly as declared. Idempotency key is `<taskId>:<sha>`, so
  redelivered webhooks reuse the existing job.
- Repositories outside the execution policy allowlist are rejected before any
  fetch.
- Tests: bad signature rejected without side effects, redelivery is idempotent,
  unknown repository rejected, unhandled event types no-op, argv cannot be
  influenced by webhook payload.

### 6. VPS deployment (G6)

- Build `commonkit-execd` for `x86_64-unknown-linux-gnu`, ship to
  `/usr/local/bin`, systemd **user** unit alongside the existing services.
- `/etc/commonkit/{execution-target.json,execution-policy.json}`,
  state under `/var/lib/commonkit-execd/`.
- Four tokens generated into `bws` project `dora-private` as
  `commonkit/EXECD_CLIENT_TOKEN`, `EXECD_WORKER_TOKEN`,
  `EXECD_ARTIFACT_SIGNING_KEY`, `EXECD_OBJECT_ENCRYPTION_KEY`.
- Caddy vhost `exec.unsold.cloud` → `127.0.0.1:7341`, DNS via Cloudflare
  (authoritative NS is Cloudflare — cert via `acme.sh dns_cf`, same path the
  Session Board used). Public, not tailnet-only, because claude.ai environments
  must reach it; bearer auth is the access control.
- Smoke: `GET /execution/v1/targets` shows the target healthy with
  `ci-linux-x64`; submit `ci-cargo-test` at a known SHA; assert green status
  lands on the commit.

Landing all six makes EXP-2302's `actions/runner` obsolete.

## Decisions

- **Trigger: GitHub webhook receiver in `execd`** (2026-07-28, Al). Rejected:
  agent-initiated-only (leaves commits unchecked, keeps the interim runner alive)
  and polling (latency plus its own state).
- **Status posting: GitHub commit-status API**, token from the secret provider,
  contexts namespaced `commonkit/*` so they cannot collide with `Workers Builds`.
  GitHub Actions must not be used.

## Verification

Per repo policy (`CLAUDE.md`): no `.github/workflows`, no GitHub-hosted jobs as
gates. Local checks for every code change:

```sh
cargo test --workspace --all-features --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
```

Deployment steps are verified on the VPS by the recorded smoke commands above,
with platform, command, and result written to the Linear issue.
