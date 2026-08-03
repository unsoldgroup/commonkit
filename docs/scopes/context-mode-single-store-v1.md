# context-mode single store v1

Tracking: USG-95

context-mode is a project-memory system CommonKit does not own. This scope makes
its storage a managed resource: one store per target, written by one process,
declared as target state, and reconciled by `commonkitd`. It changes nothing
inside context-mode itself. Twelve decisions (USG-96 through USG-107) are framed
here; each sub-issue records its resolution against the invariants below.

## Destination

One context-mode store per **Target**, written by every agent runtime on that
target through a single writer, at a path CommonKit declares rather than one
that resolves by default. The store is machine-local mutable state. It is
inventoried, observed, and snapshotted. It is never portable configuration and
never enters Git.

## Measured state

Measured 2026-08-03 on the macOS target:

- `~/.claude/context-mode` — 369 MB, written by Claude Code through the plugin over stdio.
- `~/.codex/context-mode` — 631 MB, written by Codex and by the shared HTTP server on `127.0.0.1:3766`.

Four facts constrain everything that follows.

`CONTEXT_MODE_DATA_DIR` already exists in the 1.0.169 bundle and resolves `~`.
Relocating a store requires no upstream change. This scope is about what happens
around that lever, not the lever.

Claude has both transports registered simultaneously —
`mcp__plugin_context-mode_context-mode__*` over stdio writing `~/.claude`, and
`mcp__context-mode-shared__*` over HTTP 3766 writing `~/.codex`. Which store
receives a write is an accident of which tool name the model picks.

Store filenames hash `CONTEXT_MODE_PROJECT_DIR`, not the runtime. Both stores
contain `sessions/44dc58e62d613136.db` with different content, 1.7 MB against
361 MB. The name collides; the bytes do not.

Every repository collapses into one home-directory project hash, which is why a
single session database reached 361 MB. The store move does not fix this.

## Invariants

- One authoritative writer per store. CommonKit's v1 contract already states
  this for every database (`CONTEXT.md:189`), and the promotion machinery that
  enforces it exists (`crates/commonkit-snapshots/src/lib.rs:232`, `:1006`).
- The store is mutable state, never portable configuration. It does not enter
  Git, a plan, a receipt, or a diagnostic bundle (`CONTEXT.md:185`).
- Reconciliation never destroys captured memory. A plan rollback restores
  configuration; it does not rewind the store.
- Absence is a finding. A store path that resolves by default rather than by
  declaration is drift, not success.
- CommonKit observes the store. It does not read its contents.

## Canonical store

The canonical store is `~/.local/share/context-mode/`, the path
`docs/HEADLESS-DOMAINS.md:105` already assumes. It sits outside both `~/.claude`
and `~/.codex`.

That placement is deliberate and load-bearing: `~/.claude`, `~/.codex`, and
`~/.agents` are a symlink farm CommonKit cannot adopt today, because absolute
symlink targets are rejected and pre-existing symlinks are not traversed during
inspection (dogfood inventory G2, `crates/commonkit-adapters/src/resources.rs:98-140`).
A store outside both trees routes around G2 rather than waiting on it.

## Merge authority

Resolved: **union, collisions recorded** (USG-96, decided 2026-08-03).

The canonical store is produced by a one-time, offline, schema-aware merge of
both existing stores. Rows are preserved verbatim. No row is dropped.

- Where a session identifier exists in both stores with divergent content, both
  survive under distinct keys. Neither is silently preferred.
- Every collision writes a record naming both source stores, both content
  digests, both sizes, and which key is designated the loser. A merge that
  discards without a record is a defect.
- The merge writes a new store. It never mutates either source. Both sources are
  retained read-only until the merged store passes verification, then archived.
- The merge is idempotent and re-runnable. Running it twice against the same
  sources produces the same store and the same collision records.
- The merge runs while no writer is live. It is a migration step, not a
  reconcile operation, and no adapter performs it.

`scripts/context-mode-merge.mjs` implements this. Measured against the two
stores on 2026-08-03: 833 databases, 832 disjoint by filename and copied as-is,
one — `sessions/44dc58e62d613136.db` — unioned. The two stores hold 1,560 and
1,822 distinct session identifiers and share none, so the union has no losers
and the collision record is an empty-set safety net rather than a live concern.
That is a property of today's data, not a guarantee; the merge still refuses to
run rather than drop a row.

This is the application-level export/import path `CONTEXT.md:189` already
reserves. It is not binary merging, and it does not generalize: the union is a
one-time same-machine migration, not a mechanism for combining stores across
targets. Nothing in this scope permits a cross-target merge.

**Precondition on value, not correctness.** Because store keys hash
`CONTEXT_MODE_PROJECT_DIR` and every repository currently collapses to one
home-directory hash, the union carries that collapsed key into the canonical
store verbatim. The merge is correct regardless. It is only worth what the keys
are worth, and re-keying is separate work tracked outside this scope.

## Writer and transport

The shared HTTP server on `127.0.0.1:3766` becomes the sole writer (USG-97).
Both agent-runtime plugins become clients of it. No exception is granted to the
one-authoritative-writer rule, and no multi-process SQLite locking mode is
relied on to make three writers safe.

Claude keeps exactly one context-mode transport (USG-98). The stdio plugin
registration becomes **absent**, not deprioritized. A registered transport that
ranks lower still writes whenever the model picks its tool name, so removal is
the only state that holds.

## Declaration and adoption

`CONTEXT_MODE_DATA_DIR` is declared as target state (USG-99). It is set on the
3766 service's environment through the existing `ServiceSpec { environment }`
field, not through a dotfile. Once the writer is collapsed to that one service,
no other process needs the variable, which keeps the declaration clear of the
shell-environment gap (dogfood inventory G7). A store path that is unset, so
that the per-runtime default wins by omission, is reported as drift.

The store needs no new adapter (USG-100). It is a `files` concern (the directory)
plus a `service` concern (the environment), and both adapters ship today. G5 —
inert layer fields — is closed **for this case only**; the other twelve fields
named in the dogfood inventory remain inert and remain dangerous.

Two unrelated things are named `databases`, and this scope uses only the second.
The layer spec field (`schemas/layer.schema.json:66`,
`crates/commonkit-contracts/src/lib.rs:802`) is accepted, merged, and consumed by
nothing. The runtime snapshot configuration
(`crates/commonkit-service/src/production_domains.rs:290-303`) is wired end to
end and already carries a context-mode row.

## The store as a resource

The store is inventoried at the **target override** layer (USG-101). It is
machine-local mutable state; the personal kit is Git-backed, and the store must
never reach Git.

Stores are per-target by definition (USG-103). The VPS runs its own
`context-mode-server` against its own store, and that store stays its own.
Snapshot and restore move a store between machines as an operator act; they do
not synchronize project memory, and no reconcile carries memory across targets.

## Retention and observation

context-mode prunes its own store. CommonKit observes size and does not delete
(USG-102).

Store size is a verify observation with a warning threshold — not a failure, and
never a reconcile-time deletion. Starting values, tunable: warn above 1 GB total,
sessions older than 90 days eligible for pruning. Scheduled pruning is deferred;
there is no scheduler adapter (dogfood inventory G9), and reconcile is the wrong
place for it because reconcile does not destroy captured memory.

Unbounded and unobserved is the failure this replaces. One session database
reached 361 MB before anyone looked.

## Snapshot and rollback

The store participates in mutable-state snapshots and is excluded from plan
rollback (USG-106).

Snapshot is operator-initiated and explicit: consistent SQLite export, integrity
check, client-side encryption, upload, and a content-addressed descriptor in
portable state — the path the existing `snapshots.databases` row already
describes. Rollback is plan-scoped and leaves the store untouched. Reverting a
bad reconcile restores configuration and never rewinds memory captured after the
plan applied.

## First reconcile

A fresh target has no store, and that is correct (USG-107). First reconcile
creates the directory, declares the path, and starts the writer. The store is
empty, nothing is seeded, and an empty store is a verify **pass** rather than a
warning. A fresh machine with no project memory is not in drift.

## Terminology

USG-105 lands the terms in `CONTEXT.md`. This scope proposes one glossary entry
and one boundary statement.

**Project memory store**: A target-local, per-target database holding indexed
project content and session memory for one agent-context tool, written by a
single authoritative writer and never carried in portable state.
_Avoid_: Cache, index, knowledge base

The boundary: an **About Me Profile** is owner memory, separately encrypted, with
its own writer (ADR 0010). A project memory store is project memory, is not
encrypted by CommonKit, and holds no owner claims. Project memory is never
**Scope**-shareable — it is target-local mutable state and its contents are
unredacted.

## Explicit v1 limits

No upstream context-mode change; `CONTEXT_MODE_DATA_DIR` is used as it ships. No
project re-keying. No cross-target project-memory sync. No new adapter for the
`databases` layer field, and G5 stays open for its other twelve fields. No
scheduled pruning. No content-level inspection, redaction, search, or export of
store contents by CommonKit. No merge of stores belonging to different targets.

## Implementation start gate

- USG-96 is resolved and its merge invariants are recorded on the issue.
- The union merge tool exists, is re-runnable, and has produced a merged store
  plus collision records from copies of both live stores.
- Both source stores are archived read-only and their digests recorded before
  any writer is repointed.
- Claude's stdio context-mode registration is removed, not merely reordered.

## Defaults still requiring confirmation

Flow 1 is resolved. These carry recommended defaults and remain open until their
sub-issue records a resolution.

1. Retention thresholds — 1 GB warning, 90-day session eligibility (USG-102).
2. Whether closing G5 for this case is recorded as a permanent boundary or a
   deferral until a `databases` adapter exists (USG-100).
3. Whether the archived source stores are retained indefinitely or expire
   (USG-96).
