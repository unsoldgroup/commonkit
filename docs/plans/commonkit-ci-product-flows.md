# CommonKit CI — user flows

Status of this document: **step 3 of the product loop**. The flow list must be
agreed as a whole before any single flow is deepened. Nothing here is a
commitment yet.

## Positioning decision (settled)

CI on capability-labelled Execution Targets is a **shipped CommonKit v1
capability**, pointed at the user's own repositories and targets. Its first
customer is CommonKit's own support matrix.

Why that framing and not "a CI runner": `docs/SUPPORT-MATRIX.md` gates every
platform row on *manually recorded, commit-bound native validation evidence*,
and `AGENTS.md` forbids substituting one platform's evidence for another's.
Meanwhile `commonkit_execution::placement` already refuses a job whose
`loadoutDigest` or `executionProfileDigest` differs from the target's. So a
finished CI run is not merely "tests passed" — it is *this commit passed on this
platform under this exact loadout*, which is the evidence the matrix is missing.
That claim, not breadth of features, is the product.

## Flows

Resolution status describes the code as it stands on
`al/usg-51-delegate-ci-jobs-to-a-capability-labelled-execution-target`.

### 1. Declare what CI means for a repository — `in progress`

Pain: the checks that must pass live in a README, a CLAUDE.md, or somebody's
head, and drift from what anyone actually runs.

1. Author one immutable manifest per check, argv as an array, no shell string.
2. Validate them locally before committing.
3. Commit them as repository-owned state.

Resolved: five manifests exist in `commonkit.execution-context.json` and a test
asserts each is valid, shell-free, policy-admitted, and placeable.
Open: they were authored by hand. There is no `commonkit ci` command to scaffold,
validate, or diff a declaration, so a user's first act is copying JSON.

### 2. Label a target as CI-capable — `resolved`

Pain: "which of my machines can run this, and will it run it the same way mine
does?"

1. Declare the capability the target advertises.
2. Bind it to the exact loadout and execution profile it materializes.
3. Reconcile, and have the target advertise readiness and free capacity.

Resolved: `ci-linux-x64` is declared, the target file advertises it, placement
enforces the capability subset.

Resolved in this session:

- **The two digests were always the same fact under two names.** Composition
  already computes a real `composedLoadoutDigest`
  (`digest_domain_json("commonkit.onboarding.loadout.v1", …)` over composed layer
  bytes) and carries it through plans, receipts, and skill canary. The execution
  side's `loadoutDigest` was simply never bound to it. An Execution Target's
  loadout digest is now defined as the composed loadout digest of that target's
  last successful reconciliation, not an independently authored value.
- **`execd` resolves it, rather than being told it.** At startup it reads the
  digest from local CommonKit state and refuses to advertise readiness if it
  cannot. The target file stops carrying hand-written digests, which removes the
  class of error where a target claims a loadout it does not have. Labelling a
  target as CI-capable becomes a loadout statement reconciled onto the machine,
  consistent with providers computing desired state and adapters mutating.
- **The execution profile pins platform, not toolchain** (ADR 0010). It records
  operating system, architecture, and machine class — what a repository cannot
  state about itself. Tool versions stay pinned by repository-owned,
  commit-bound files. Recording them on the target would duplicate a fact the
  commit already carries and turn a routine `rustc` bump into a fleet-wide
  placement outage.

Consequence worth stating plainly: loadout equality proves the CommonKit-managed
environment matched, not that the compiler did. A repository that pins nothing
gets a receipt honest about its platform and silent about its tools. That is a
repository problem, and the evidence model should not paper over it.

### 3. Trigger CI when code changes — `todo`

Pain: nothing runs on its own; a commit lands unchecked.

1. A push or pull-request event arrives.
2. It maps to the repository's declared task IDs at that head SHA.
3. Jobs are submitted idempotently, so redelivery does not double-run.

Open: entirely. Planned as `POST /execution/v1/github/webhook` with
`X-Hub-Signature-256` validation and `<taskId>:<sha>` idempotency (USG-56).

### 4. Run a check on demand — `in progress`

Pain: "run the Linux checks against this commit, now" — from a terminal, or from
an agent that cannot be trusted with arbitrary argv.

1. Pick a declared task.
2. Submit it for a chosen revision.
3. Reconnect to it later from anywhere.

Resolved: MCP exposes `commonkit_submit_task` restricted to declared tasks, plus
job, event, cancel, retry, resume, and artifact tools.
Open: no CLI path. A human at a terminal has no first-class way in.

### 5. Watch a run and control it mid-flight — `in progress`

Pain: it has been eight minutes and there is no way to tell running from wedged.

1. Follow state transitions and events as they happen.
2. Cancel, retry, or resume from a checkpoint.

Resolved: the durable API covers all of it, with per-job monotonic event
sequences and at-least-once delivery consumers deduplicate by ID.
Open: no surface renders it. The Session Board is the obvious home — it already
aggregates per-machine agent state on an always-on hub — but a CI run is not a
pending human decision, so the board's card model does not fit as-is.

### 6. See the result where the work already is — `resolved`

Pain: a result nobody sees changes no behaviour.

1. Terminal state maps to a commit status.
2. It appears on the commit and pull request.

Resolved: contexts are posted as `commonkit/<taskId>`, namespaced away from
`Workers Builds`. A job maps back to its declared task by repository, workdir,
and argv, so the mapping is derived rather than persisted. Posting is best effort
and cannot change a job's outcome. Deliberately omitted: `target_url`, because
signed artifact URLs expire in five minutes and the job endpoint needs a bearer
token a GitHub viewer does not have.

### 7. Diagnose a failure on a platform you are not sitting at — `in progress`

Pain: it failed on Linux; the developer is on macOS. This is the single most
common CI experience and the one most often left to "read the log".

1. Read redacted diagnostics for the failed attempt.
2. Retrieve the exact workspace the failure happened in.
3. Reproduce, or attach to it.

Resolved: stdout and stderr are committed as artifacts with secret values
redacted; artifact downloads use signed five-minute URLs; the operator can retain
a failed worktree with `--keep-failed-workspaces`.
Open: nothing lets a developer *reach* a retained workspace, and no client
fetches artifacts outside MCP. "Give me a shell in the failure" is unbuilt.

### 8. Prove a platform is supported — `todo`

Pain: `SUPPORT-MATRIX.md` rows sit gated on runs nobody has performed, and the
recorded evidence is commit-bound, so it decays with every commit.

1. Run the platform's qualifying flow set at a specific commit.
2. Get a receipt binding commit, platform, loadout digest, and binary digests.
3. Record it as the matrix's evidence for that row.

Open: entirely, and this is the flow that distinguishes the product. Execution
receipts exist as a contract; nothing connects a receipt to a support-matrix row
or to `scripts/qualify-eight-flows.sh`.

### 9. Add a second and third platform — `todo`

Pain: one Linux VPS cannot produce macOS or Windows evidence, and the policy
forbids substituting one for another.

1. Label a macOS target and a Windows target.
2. Submit the same declared task set to each.
3. Collect per-platform receipts for one commit.

Open: durable execution v1 explicitly supports **one authoritative Linux
worker**. Multi-platform fan-out is the product's reason to exist and is out of
the current engineering scope. This tension needs an explicit decision.

### 10. Survive a loadout change — `in progress`

Pain: a toolchain moves under you and results quietly stop meaning what they did.

1. The target materializes a new loadout; its digest changes.
2. Jobs pinned to the old digest stop placing rather than running wrong.
3. The declaration is updated deliberately and re-run.

Resolved by design: placement refuses on `loadout_digest_mismatch`, which is
fail-closed and correct.
Open: the operator experience is a job that mysteriously never places. There is
no "your declaration is stale, here is the new digest" path.

### 11. Keep a CI job from harming the target — `resolved`

Pain: CI is arbitrary code from a repository, running on a machine you own.

1. Policy pins an allowed-repository list, resource ceilings, network posture,
   and whether repository write is permitted.
2. It is enforced at submission *and* re-enforced at execution.
3. Denials are audited without token values.

Resolved: both checkpoints exist, argv is an array so no shell string can smuggle
execution, work runs in a systemd scope with cgroup limits, and a policy-denied
repository is never even contacted — asserted by test.

### 12. Give CI a secret without putting it in the repository — `in progress`

Pain: tests need a token; the manifest must stay portable and reviewable.

1. The manifest names `env://NAME`, never a value.
2. The target resolves it locally.
3. Placement refuses targets that cannot resolve it.

Resolved: an owner-only secret file, reserved names rejected at startup, keys
becoming the target's `readySecretRefs`, values redacted from diagnostics.
Open: rendering that file from Bitwarden Secrets Manager at deploy time, and
rotating it, are undefined operator steps.

### 13. Check out a private repository — `todo`

Pain: every repository worth running CI on is private.

Open: preparation clones with `GIT_TERMINAL_PROMPT=0`, so a private repository
fails fast rather than hanging, but no credential path exists. It must be a git
credential helper on the target — never a token embedded in
`manifest.repository`, which is portable reviewable state.

### 14. Live with one target and many checks — `in progress`

Pain: five declared checks and one machine means the fifth pull request waits.

1. Jobs queue.
2. Placement accounts for queue depth and free capacity.
3. Someone can tell why their run has not started.

Resolved: the scheduler queues durably and placement scores on queue depth, free
memory, and cost.
Open: the worker loop leases one job at a time, so a repository's five checks run
serially. Placement explanations exist as a contract but are not surfaced, so
"why is mine not running" has no answer a user can read.

### 15. Decommission a CI target — `todo`

Pain: a machine goes away mid-run, or is retired deliberately.

1. Drain it so no new work places on it.
2. Let in-flight attempts finish or be reassigned.
3. Remove it without stranding jobs.

Resolved in part: the target contract has a `draining` flag and placement honours
it; `recover_expired` turns abandoned attempts into interrupted attempts and
queues policy-permitted retries.
Open: no operator action sets `draining`, and with one worker there is nowhere to
reassign to.

## Cross-cutting observations

- **Eight of fifteen flows have a working durable core and no surface.** The
  scarce thing is not scheduler capability; it is a way for a human to see and
  act on any of it. Flows 5, 7, 10, and 14 all fail for the same reason.
- **Two open items are load-bearing for the product claim itself**: real loadout
  digests (flow 2) and multi-platform targets (flow 9). Without the first, the
  evidence claim is unproven. Without the second, it is unprovable for macOS and
  Windows, which is where the matrix is emptiest.
- **The engineering scope and the product scope disagree.** Durable execution v1
  deliberately supports one Linux worker; the product's reason to exist is
  per-platform evidence. That is a decision to take, not a gap to quietly close.
