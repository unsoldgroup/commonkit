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
- USG-141 carries the six decisions above and their resolutions.
- The design agent has read ADRs 0010, 0011, 0014, 0015, 0016, 0017,
  `CONTEXT.md`, and `context-mode-single-store-v1.md` — the last as an example
  of the output shape expected, not as subject matter.

## Open question for the owner

Is `session_summary` shared or personal? It is 16 % of observations and carries
most of the confidentiality risk. Everything else in the type table is
project-factual and comparatively safe to share.

## Implemented owner-device disposition

Engram remains a target-local SQLite store. CommonKit carries only its opaque,
compressed JSONL chunks and manifest, identified by an explicit portable project
identity. It inventories metadata, hashes compressed bytes, exchanges missing
chunks, invokes Engram export/import, and records redacted receipts; it never
decompresses or interprets payloads.

Owner-device transport accepts the current mixed-scope export because both
targets belong to one principal. Personal transport is rejected explicitly.
Cross-principal transport is fail-closed until Engram emits
`engram.scope-export.v1`: project identity, project scope, exporter version,
exact manifest digest, and every chunk's ID, compressed size, and digest. A
matching active Grant is required; withdrawal is prospective and retains
already-imported observations.

The implemented resource is append-only and content-addressed. Same-ID,
different-byte chunks are collisions; missing peers are unresolved no-ops.
Local and SSH targets use typed directory/read/write operations and fixed
Engram export/import requests; no arbitrary remote command or payload decoding
is exposed. Every cycle exports both targets, exchanges and verifies chunks,
atomically updates manifests, then imports both sides. Receipts record only
movement IDs, compressed-byte digests/sizes, confirmation identity, and any
unresolved outcome.

`session_summary` remains personal-only by default. Team sharing is not claimed
until Engram supplies scope-separated attestation and the receiving-side gate
verifies it. This is the honest v1 boundary: owner-device sync is implemented;
team sync remains intentionally unavailable.
