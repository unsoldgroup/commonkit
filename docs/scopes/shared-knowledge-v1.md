# Shared knowledge v1 — design brief

Tracking: USG-141, a sub-issue of epic USG-110 (CommonKit multiplayer).

This brief was first written against context-mode. That premise was wrong and
is corrected below, because the correction removes most of the work and points
at a different system. Measured 2026-08-06 on the macOS target.

## Premise correction: context-mode is not the knowledge base

context-mode is a **context-window filter**, not a durable knowledge store. The
evidence:

- **What is indexed is tool output, not knowledge.** Source labels in the
  largest content store read `execute:shell`, `batch:git status short`,
  `batch:repo root files`, `batch:package scripts`. These are captures of
  command output from `ctx_batch_execute`, titled by the agent that ran them.
- **It is overwhelmingly stale.** Of ~278 indexed sources across 7 content
  databases, 262 were indexed on 2026-06-26/27, 2026-06-29 or 2026-07-08. Two
  are from today. One database is empty.
- **It is session-shaped, not subject-shaped.** The store key is a hash of the
  absolute project path, and 723 MB of the 1.0 GB store is `sessions/` across
  871 databases holding `session_events` and `tool_calls` — a transcript and
  resume log.
- **Its own retention says so.** Sessions are pruned after 7 days and content
  after 14. Nothing designed to be durable knowledge prunes itself on that
  clock.

Sharing this would distribute six-week-old `git status` output. **context-mode
is out of scope for knowledge sharing.** It stays what it is: a per-target
token filter, already covered by `context-mode-single-store-v1.md`.

One live defect found while measuring, worth fixing regardless: two content
databases in `~/.claude/context-mode` postdate the 2026-08-05 11:18 merge
(11:36 and 2026-08-06 01:53), so a writer is still resolving to the legacy path
instead of the declared store. Under v1's own rule that is drift, not success.

## The actual subject: engram

engram is the durable, typed, already-shareable knowledge system.

**Measured contents** — `~/.engram/engram.db`, 21 MB, 16 projects:

| Type | Count |
|---|---|
| architecture | 403 |
| decision | 250 |
| session_summary | 188 |
| discovery | 70 |
| bugfix | 58 |
| manual | 55 |
| project | 47 |
| reference | 32 |
| bug | 31 |
| feedback | 8 |

This is the material with team value: why a thing was built the way it was,
what broke and why, what was decided and rejected.

**Sharing already partly exists, by two separate mechanisms.**

*Git transport, working today.* This repository tracks `.engram/` (`chunks/`,
`manifest.json`) — committed, not ignored. `engram sync` exports project-scoped
chunks; `engram sync --import` applies them. Knowledge rides in the repository
it is about and any clone gets it, which is the same shape ADR 0014 gives
everything else: git owns content. Per-project, no infrastructure, and it
cannot share anything across repositories.

*Cloud transport, unconfigured.* `engram cloud serve` runs a Postgres-backed
server with a dashboard and an audit log. Clients point at it with `engram
cloud config`, enrol a project, and run `engram sync --cloud --project X`.
`engram cloud status` currently reports **not configured**. This is the only
path that gives organization-level sharing across repositories and people.

Its server-side controls matter to the design, because they are the
confidentiality levers that already exist:

| Variable | Role |
|---|---|
| `ENGRAM_CLOUD_ALLOWED_PROJECTS` | required allowlist; `*` allows everything |
| `ENGRAM_CLOUD_TOKEN` + `ENGRAM_JWT_SECRET` | authenticated mode |
| `ENGRAM_CLOUD_ADMIN` | separate admin dashboard token |
| `ENGRAM_CLOUD_HOST` | bind host, defaults to `127.0.0.1` |

A deployment that sets the allowlist to `*` answers this brief's open question
by default, and answers it badly: every `session_summary` would be published to
everyone enrolled. The allowlist is per **project**, not per scope, so it
cannot by itself separate personal from team material.

## What is therefore actually missing

The transport exists. The unsolved parts are the ones ADR 0016 raises, and they
are about **confidentiality and ownership**, not plumbing.

1. **Personal versus team observations.** engram already carries a `scope`
   column, and both the CLI and the MCP tools accept `project` or `personal`
   at write time. Measured today: 1,158 `project`, 4 `global`, 0 `personal` —
   so the mechanism exists and is effectively unused. The decision is therefore
   not "what marks one" but **who sets it and when**: the author at write time
   (the current design, and nobody uses it), or a policy on the project that
   classifies automatically. Note ADR 0016 puts team scopes in a shared
   repository and personal kits in private ones because Git access control is
   repository-level — so engram's scope must map onto a repository boundary to
   mean anything for confidentiality, not merely filter a query.

2. **`session_summary` is the risky type.** 188 of them. A session summary is a
   narrative of what someone did, in their words, and is closest to owner
   memory under ADR 0010 rather than to project knowledge. Decide whether it is
   shared, redacted, or personal-only.

3. **Secrets in observations.** Nothing stops an observation quoting a token, a
   customer name, or an internal URL. A shared store needs either write-time
   discipline or a redaction pass. Note the invariant that CommonKit observes
   declarations and does not read contents — a CommonKit-side redactor
   contradicts it as written.

4. **Conflict and precedence.** Two people record contradicting architecture
   observations about the same subject. ADR 0017 says lent content never
   outranks your own, so the local observation wins; but `mem_judge` already
   models supersedes/conflicts relations. Reconcile the two mechanisms rather
   than adding a third.

5. **Revocation.** When a Grant is withdrawn, what happens to observations
   already pulled? Reconciliation never destroys captured memory, which appears
   to make revocation impossible for anything already materialized. This
   tension is real and needs an answer, not a footnote.

6. **CommonKit's role.** Does CommonKit own engram enrolment as a managed
   resource — declaring which projects are enrolled and to which Scope — the
   way it owns the context-mode store declaration? That is the natural fit and
   would make enrolment observable and reconcilable.

## Scope decisions from the owner

**First customer**: both one person's several machines and a team, designed
together (2026-08-06). The confidentiality model must therefore be present from
v1; a device-only design that defers it is rejected.

**Transport**: engram stays **local on every device, and CommonKit syncs the
chunks periodically** (2026-08-06). Rejected: `engram cloud serve`, which would
put a second authority beside CommonKit and duplicate the target, credential
and reconciliation machinery CommonKit already owns. Also rejected: manual git
commit of `.engram/`, which works but is per-repository, unscheduled, and
cannot carry anything across projects.

This choice fits CommonKit better than it first appears, for a specific reason:
**a chunk is a portable artifact, not the live store.** `engram sync` emits
compressed JSONL chunks with a manifest; the SQLite database never moves. So
CommonKit can carry chunks as content-addressed resources without reading them,
which keeps the v1 invariant — CommonKit observes declarations and does not
inspect store contents — intact rather than broken.

It also inherits the right failure mode. Under ADR 0014 an edge whose content
the grantee's target cannot read is an unresolved edge that Reconciliation
reports, degrading to a no-op rather than a failed apply. Chunk transport is
exactly that shape.

The design must now answer, in addition to the decisions below:

- **What CommonKit models a chunk set as.** A managed resource with a declared
  path, a snapshot, or a new artifact kind? Chunks are append-only and
  content-addressed, which none of the existing resource kinds assume.
- **Cadence and trigger.** Scheduled, on session end, on commit, or on
  reconcile? Note engram ships `--watch` with a minimum one-minute interval,
  which may remove the need for CommonKit to schedule anything itself.
- **Direction and authority.** v1 requires one authoritative writing target per
  store. Periodic bidirectional chunk exchange has many writers by
  construction, so either that invariant is replaced or chunks are declared not
  to be the store.
- **Import is not idempotent by assumption.** `engram sync --import` must be
  verified safe to re-run against already-applied chunks before any scheduler
  drives it.
- **Whether the personal scope gates transport.** engram's `scope` is a query
  filter today. If CommonKit selects which chunks to carry, scope can become a
  real boundary — which is the cheapest available answer to decision 2 below.

## Scope of work

Five phases. Each lands independently and is verifiable on its own; none
requires the next to be useful. Phase 0 is a hard prerequisite — the rest rests
on facts it establishes, and two of them are currently unproven.

### Phase 0 — Prerequisites and spikes

Nothing here ships a feature. It closes the unknowns the design depends on.

| Deliverable | Acceptance |
|---|---|
| The context-mode drift finding is closed | Every writer resolves to the declared store, verified from a clean session on both Claude Code and Codex; the legacy directories receive no new writes over a full working day |
| `engram sync --import` idempotency is proven | A chunk applied twice produces no duplicate observations and no error, demonstrated on a copy of the live database, with the result recorded on the issue |
| Chunk format is characterised | Written note covering chunk naming, manifest structure, whether chunks are append-only in practice, and what an out-of-order import does |
| Project identity is decided | A rule that resolves the same subject on two machines, given engram keys projects by repository basename and two repos here already share the name `commonkit` |

**If import is not idempotent, stop.** A scheduler driving a non-idempotent
import corrupts memory quietly, and the phase plan below changes shape.

### Phase 1 — Chunks as a CommonKit resource

Model an engram chunk set as something CommonKit can declare, inventory,
verify and reconcile, without reading chunk contents.

Acceptance: `commonkit status` reports the chunk set for a declared project;
a missing or extra chunk is reported as drift; nothing in the path reads or
decompresses a chunk payload; the existing invariant that CommonKit does not
inspect store contents is demonstrably intact.

### Phase 2 — Transport between two targets

Carry chunks between two targets belonging to one owner. This is the tracer
bullet: the smallest thing that proves the whole idea.

Acceptance: an observation saved on device A is searchable on device B after a
sync, with no manual `engram` command run on either side; a target that cannot
reach the other degrades to a Reconciliation-reported no-op per ADR 0014, not a
failed apply; the receipt records what moved.

Cadence is decided here, informed by Phase 0. `engram --watch` may make
CommonKit-side scheduling unnecessary.

### Phase 3 — Scope becomes a boundary

Make engram's `scope` field gate transport, so `personal` observations do not
travel. Today scope is a query filter and 0 of 1,162 observations use
`personal`, so this phase includes deciding who sets it and when.

Acceptance: an observation written with `scope: personal` is never carried to
another target; the classification rule is documented; existing observations
are migrated or consciously left as `project` with that choice recorded.

### Phase 4 — The team boundary

Extend from one owner's devices to several people, mapping onto ADR 0016: team
scopes in a shared repository, personal kits private. This is where revocation
and the `session_summary` question must be answered rather than deferred.

Acceptance: a Grant confers access to a team scope's chunks; withdrawing it
produces a defined, documented outcome for already-materialized chunks;
`session_summary` has a resolved disposition — shared, redacted, or
personal-only.

### Sequencing note

Phases 0 to 2 deliver cross-device sync for one person, which is immediately
useful and carries no confidentiality risk, since every target is the owner's.
Phase 3 is the gate before anything reaches a second person. Phase 4 should not
begin until Phase 3 holds, because a team boundary built over a decorative
scope field is a boundary in name only.

## Explicitly out of scope

No context-mode changes beyond closing the drift finding above. No upstream
engram fork before the design says one is needed. No real-time collaborative
editing. No cross-organization sharing.

## Start gate

- The context-mode drift finding is closed and the legacy directories stop
  receiving writes.
- A Linear issue exists under USG-110 enumerating the six decisions above.
- The design agent has read ADRs 0010, 0011, 0014, 0015, 0016, 0017,
  `CONTEXT.md`, and `context-mode-single-store-v1.md` — the last as an example
  of the output shape expected, not as subject matter.

## Open question for the owner

Is `session_summary` shared or personal? It is 16 % of observations and carries
most of the confidentiality risk. Everything else in the type table is
project-factual and comparatively safe to share.
---

# Shared knowledge v1 — implemented disposition

Tracking: USG-141, under USG-110.

## Destination

Engram remains a target-local SQLite store. CommonKit carries only Engram's
portable compressed JSONL chunks and manifest. A chunk set is an opaque,
append-only, content-addressed managed resource identified by a declared
**Engram project identity**. CommonKit inventories manifest metadata, hashes
compressed bytes, transports missing chunks, invokes Engram export and import,
and records a redacted movement receipt. It never decompresses or interprets a
chunk payload.

The first customer is both one principal's devices and a team. Owner-device
transport may carry the current mixed-scope chunks because every target belongs
to the same principal. Cross-principal transport fails closed until the
scope-separated export gate below holds.

## Decisions

1. **Resource kind.** An Engram chunk set is a distinct managed artifact set,
   not a file tree or database snapshot. The SQLite store never moves. Chunk
   IDs and compressed-byte digests are immutable; a repeated ID with different
   bytes is a collision and reconciliation stops.
2. **Personal versus team.** Engram scope gates cross-principal transport.
   `personal` observations never cross a principal boundary. CommonKit does not
   classify or redact observations because doing so would require reading
   payloads. Engram must emit a project-only chunk set with machine-verifiable
   scope attestation before team sync can start.
3. **Session summaries.** `session_summary` is personal-only by default.
   Existing summaries are not automatically published or rewritten. A person
   may deliberately distill selected facts into typed project observations;
   only those new observations are eligible for team transport.
4. **Conflict and precedence.** Engram owns observation identity,
   `supersedes`, and `conflicts` relations. CommonKit neither merges nor ranks
   observations. ADR 0017 applies to whether a granted chunk set is admitted;
   after import, Engram's relation model resolves knowledge conflicts. No third
   precedence mechanism is introduced.
5. **Revocation.** Withdrawing a Grant stops future chunk delivery and removes
   unimported transported chunks from CommonKit-controlled staging. It does not
   erase observations already imported into another principal's store. Imported
   knowledge is captured memory, so team enrolment must disclose this
   prospective-revocation limit before the first transfer.
6. **CommonKit ownership.** CommonKit owns Engram enrolment as target state:
   project identity, chunk root, peer target, owner principal, cadence, and any
   team Grant. Engram owns export, import, payload schema, observation scope,
   and the live SQLite store.

## Phase 0 evidence

Measured on macOS on 2026-08-06 with Engram 1.16.1.

- Applying the repository's 12 chunks twice to a fresh database produced 27
  observations after both runs, zero duplicate non-null sync IDs, and the
  second import reported all 12 chunks already imported.
- Reversing the manifest order imported all 12 chunks successfully with zero
  duplicate sync IDs. Import is order-independent for the measured set.
- `manifest.json` is version 1 and lists chunk ID, creator, creation time, and
  session/memory/prompt counts. Each chunk is `<8 lowercase hex>.jsonl.gz` and
  contains one JSONL envelope. CommonKit does not depend on or inspect that
  envelope.
- Chunk IDs are append-only in the measured repository. Reconciliation treats
  same-ID/different-digest as corruption rather than replacement.
- Project identity is an explicit portable ID, normally the normalized
  canonical Git remote identity such as `github.com/unsoldgroup/commonkit`.
  Repository basename is never identity; this distinguishes unrelated repos
  named `commonkit` and resolves the same subject on different machines.
- A controlled export containing one `project` discovery and one `personal`
  session summary placed both observations in the same chunk. Engram 1.16.1
  therefore does not yet provide the cross-principal confidentiality boundary.
- Upstream commit `509e6762fdd9417ff7a39d30f426a9566220eaf0` confirms
  the cause: local `sync --project` calls `ExportProject`, whose observation
  query filters project identity but has no scope predicate; `ChunkEntry` also
  carries no project or scope attestation.

## Resource behavior

`commonkit engram status --project-id ID --owner-id PRINCIPAL --root PATH` inventories one declared
set. Missing and extra chunks are drift. The command reads manifest metadata
and hashes compressed files; it never decompresses them.

`commonkit engram sync --project-id ID --owner-id PRINCIPAL --left PATH --right PATH --confirmed`
runs Engram export on both project roots, exchanges missing chunks in both
directions, writes a union manifest atomically, and runs idempotent import on
both roots. A missing peer reports an unresolved no-op. A project identity
mismatch, digest collision, unsafe path, or personal transport request fails
closed. Receipts contain chunk IDs, compressed-byte digests and sizes, and
source/destination roots, never observation content.

The same inventory and transfer logic accepts CommonKit target capability
roots. SSH targets use the closed target-helper protocol: typed directory list,
file read/write, and fixed Engram export/import operations. No arbitrary remote
command or chunk decompression is exposed. A remote cycle exports on both
targets, exchanges and verifies compressed chunks, writes the union manifest,
then imports on both targets.

Every export/import command includes `--project` with the declared portable
project identity; repository basename is never allowed to choose identity.
Remote project paths reject symlink components, and chunk, manifest, and
attestation writes use same-directory atomic replacement.

A target's normal `sync` declaration may include:

```json
{
  "engram": {
    "projectId": "github.com/unsoldgroup/commonkit",
    "ownerId": "github:astemarie",
    "projectRoot": "repo",
    "root": "repo/.engram",
    "scope": "project",
    "executable": "/usr/local/bin/engram",
    "autoReconcile": {
      "peerTargetId": "workstation-b",
      "intervalSeconds": 60
    }
  }
}
```

`executable` is required for a local target and must resolve to an absolute,
regular, non-symlink file. An SSH target instead uses the remote helper's
separately configured `engramExecutable`; neither path is caller-controlled at
request time. Exactly one target may declare `autoReconcile` in v1. Its peer
must be a different configured target and the interval is at least 60 seconds.

`commonkit engram managed-status` inventories the running daemon's declared
resource. `commonkit engram reconcile --peer-target-id TARGET --confirmed`
runs one reviewed cycle. Each manual or scheduled cycle writes a private
receipt below the daemon receipt root's `engram/` directory. A receipt records
the run and confirmation identities, opaque chunk movements, import outcome,
and any unresolved no-op; it never records observation content.

The ordinary daemon verification report includes this resource's state,
missing and extra IDs, and compressed-byte digests. Drift participates in the
selected-target health check.

Cadence defaults to one minute. The authenticated CommonKit daemon owns the
long-running exchange when `autoReconcile` is declared; Engram's watch facility
is not sufficient because it does not transport chunks between targets. A
cycle is export, exchange, import. A process-local lock serializes manual and
scheduled cycles, and unchanged cycles are idempotent. Configuration reload of
the schedule takes effect when the supervised daemon restarts.

Cross-principal authorization is declared at the top level as
`engramGrants`. A Grant binds a stable ID, the portable project identity,
grantor, grantee, and `active` or `withdrawn` state. Absence of a matching Grant
fails before export. Withdrawal records a prospective no-op and does not erase
previously imported observations.

## Team start gate

Cross-principal reconciliation is disabled until all of these are true:

- Engram can export separate `project` and `personal` chunk sets without
  CommonKit reading payloads.
- The export includes an attestation binding project identity, scope, manifest
  digest, and Engram version.
- CommonKit verifies `scope: project` before transport.
- The Grant graph authorizes the project chunk set for the grantee.
- The grantee acknowledges that revocation is prospective for imported memory.
- Existing `session_summary` observations have been explicitly reclassified as
  personal or consciously retained with an owner-recorded exception.

CommonKit implements the receiving half of this gate. An active, project- and
principal-matched Grant is required. Both exports must contain
`scope-attestation.json` with schema `engram.scope-export.v1`, exporter version,
portable project identity, `scope: project`, the exact manifest digest, and
each chunk's ID, compressed size, and digest. CommonKit verifies the complete
set without opening payloads and merges attestations alongside append-only
chunks. A withdrawn Grant produces a reported no-op and retains already
materialized observations. Engram 1.16.1 does not emit this attestation, so the
team path currently fails closed at the intended compatibility gate.

## Context-mode drift gate

The retired Claude `context-mode-shared` registration was removed on
2026-08-06. Both canonical environment variables resolve to
`~/.local/share/context-mode`. Clean Claude Code sessions started at 20:52 and
21:50 were observed holding only canonical-store database files; the last
legacy database/session write was 12:22. The installed plugin's install-healing
hook was corrected to honor `CONTEXT_MODE_DIR`; a direct invocation at 22:31
wrote its diagnostic log to the canonical store while the legacy log remained
unchanged. The literal no-write observation window therefore starts at 22:31.
Completion still requires one full working day in which neither
`~/.claude/context-mode` nor `~/.codex/context-mode` receives a new write.

## Explicit limits

No Engram cloud server. No live SQLite transport. No CommonKit-side payload
inspection, classification, or redaction. No cross-organization sharing. No
real-time collaborative editing. No team transfer before the scope-attestation
gate holds.
