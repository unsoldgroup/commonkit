# context-mode single store v1

Tracking: USG-95

context-mode is a project-memory system CommonKit does not own. This scope makes
its storage a managed resource: one store per target, at a path CommonKit
declares rather than one that resolves by default, inventoried, observed, and
snapshotted. It changes nothing inside context-mode itself. Thirteen decisions
(USG-96 through USG-108) are recorded here, each resolved against the invariants
below.

## Destination

One context-mode store per **Target**, written by the agent runtimes on that
target, at a declared path. The store is machine-local mutable state. It is
never portable configuration and never enters Git.

## Measured state

Measured 2026-08-03, corrected 2026-08-05 after the decisions below were
verified against the running system. Several claims in the original framing were
wrong; they are stated here in corrected form because later sections depend on
them.

- `~/.claude/context-mode` — 369 MB, written by Claude Code through the plugin's
  stdio server. This is the only live writer.
- `~/.codex/context-mode` — 631 MB. Not written by Codex: Codex's context-mode
  plugin is disabled (`~/.codex/config.toml`, `enabled = false` for both the
  plugin and its MCP server). Written historically by the shared HTTP service on
  `127.0.0.1:3766`, which has no registered clients and is kept alive only by
  launchd.

Five facts constrain everything that follows.

**The storage lever is `CONTEXT_MODE_DIR`, not `CONTEXT_MODE_DATA_DIR`.** The
MCP server resolves its store root from `CONTEXT_MODE_DIR`, which must be an
absolute path, is ignored when blank, does not expand `~`, and throws when
relative. It yields `<CONTEXT_MODE_DIR>/sessions` and
`<CONTEXT_MODE_DIR>/content`. `CONTEXT_MODE_DATA_DIR` is read only by the
adapters and hooks, expands `~`, and yields
`<CONTEXT_MODE_DATA_DIR>/context-mode/sessions`. The two therefore take
different values to name the same store, and setting only one splits the server
and the pruning hook across two different stores. Relocating a store still needs
no upstream change.

**Store keys are context-mode's, not CommonKit's.** A store filename is the
first sixteen hex characters of SHA-256 over the case-folded absolute project
directory, applied to both `sessions/<key>.db` and `content/<key>.db`. A
`__<hash>` suffix is an eight-character hash of the current linked git worktree
root, added when it differs from the main worktree. The plugin already keys
projects and worktrees correctly.

**The collapse came from one long-lived shared process, not from a variable.**
The 3766 shim defaults the project directory to `$HOME` when unset, and its
launchd job pins the same value; but a shared HTTP server has no per-session
working directory, so any long-lived shared process collapses every project into
one key regardless of what the variable says. The plugin's per-session stdio
child inherits the session's directory and does not.

**The evidence of that collapse.** The stdio store holds 61 distinct project
keys across 60 database files, largest ~13 MB. The 3766 store holds 2 project
directories across 2 files, one of them 361 MB with 1,815 sessions. The 361 MB
is mostly one large corpus indexing run, not many repositories mashed together.

**Retention already ships.** context-mode caps session events at 1,000 per
session, deletes sessions older than 7 days from its SessionStart hook, and
deletes content databases and indexed sources older than 14 days when the server
first opens a content store. The 361 MB store grew because pruning is driven by
the Claude SessionStart hook, which never ran for the HTTP service. This is an
unpruned-writer gap, not a missing-retention gap.

## Invariants

- One authoritative writing **target** per store. The rule is target-scoped, not
  process-scoped: `CONTEXT.md:468` states it alongside cross-machine
  portability, and the machinery keys authority by target
  (`SnapshotDatabase.target_id`,
  `crates/commonkit-service/src/production_domains.rs:293`;
  `PortableAuthorityRecord`, `crates/commonkit-snapshots/src/lib.rs:244`;
  `AuthorityStore`, `:232`).
- The store is mutable state, never portable configuration. It does not enter
  Git, a plan, a receipt, or a diagnostic bundle (`CONTEXT.md:464`).
- Reconciliation never destroys captured memory. A plan rollback restores
  configuration; it does not rewind the store.
- Absence is a finding. A store path that resolves by default rather than by
  declaration is drift, not success.
- CommonKit observes the store's declaration. It does not read its contents and
  does not measure its size.

## Canonical store

The canonical store is `/Users/developer/.local/share/context-mode` on the macOS
target. It sits outside `~/.claude`, `~/.codex`, and `~/.agents`, and no
component of its path is a symlink.

The store move is **not gated on G2** (USG-104). G2 is narrower than it reads:
CommonKit rejects a symlink when it appears in a declared path's own ancestors
or leaf, never as a property of the surrounding tree
(`crates/commonkit-adapters/src/resources.rs:98-140`). A store outside the
agent-config trees routes around G2 rather than waiting on it. That is why this
path was chosen.

## Merge authority

Resolved: **union, collisions recorded** (USG-96, decided 2026-08-03, applied
2026-08-05).

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
- Re-runnable holds only while the destination is a staging directory. Once a
  writer is pointed at it the merge refuses, because rebuilding from the sources
  would discard everything captured since. `--force` overrides for a deliberate
  discard. A `-wal` newer than the manifest is what marks it live: these stores
  are WAL, so a live writer leaves the database file itself untouched.
- Every database is copied with `VACUUM INTO`, not as a file. A file copy takes
  the database without its `-wal`, and everything committed since the last
  checkpoint lives there, so a plain copy silently loses recent rows whenever a
  writer is live or a process exited without closing cleanly. Measured: a file
  copy of a 500-row database with an uncheckpointed WAL yields 392 rows.
- Writers may therefore stay running. Each database is snapshotted through its
  own read transaction, so the result is per-database consistent rather than one
  instant across the store, which is what project memory needs since each
  project's database stands alone.
- The merge is a migration step, not a reconcile operation, and no adapter
  performs it.

`scripts/context-mode-merge.mjs` implements it and
`scripts/context-mode-apply-merge.sh` drives it. Applied 2026-08-05: 858
databases written, 857 disjoint by filename and copied as-is, one —
`sessions/44dc58e62d613136.db` — unioned. Zero session-identifier collisions, so
the union had no losers and the collision record is an empty-set safety net.
That is a property of the data, not a guarantee; the merge still refuses to run
rather than drop a row.

This is the application-level export/import path `CONTEXT.md:468` already
reserves. It is not binary merging, and it does not generalize: the union is a
one-time same-machine migration, never a mechanism for combining stores across
targets.

**Precondition on value, not correctness.** The merged store carries the
collapsed home-directory bucket from the 3766 store verbatim. The merge is
correct regardless. It is only worth what its keys are worth (USG-108).

## Writer and transport

Claude keeps exactly one context-mode transport (USG-98, resolved 2026-08-05).
The remote SSE registration — `https://ctx.unsold.group/sse`, a cloudflared
tunnel to a context-mode server on the VPS writing VPS-local stores — was
removed along with its allow rules, so it is **absent**, not deprioritized. A
transport that merely ranks lower still writes whenever the model picks its tool
name.

The writer is the plugin's stdio server, and the 3766 service is decommissioned
rather than promoted (USG-97, default revised). Promoting it was rejected on
three grounds: it reverses USG-98; a shared HTTP server has no per-session
working directory, so it cannot key projects correctly at all; and it has no
registered clients, while still opening `~/.codex/context-mode` on start.

The stdio server is one process **per session**, so the one-authoritative-writer
rule is granted an explicit process-level exception. It is safe rather than
lucky: every database is opened `journal_mode = WAL`, `synchronous = NORMAL`,
`busy_timeout = 30000`, with three write retries and `locking_mode = EXCLUSIVE`
explicitly prohibited. Same-host multi-process WAL with a busy timeout is the
supported SQLite configuration for concurrent writers; writes serialize, readers
never block, and every concurrent writer is the same runtime at the same version
against the same schema. The invariant holds because it is target-scoped: one
authoritative writing target per store, many same-runtime processes within it.
Availability is bounded — sustained contention can still end in `SQLITE_BUSY`.

The writer choice carries a retention constraint (USG-102): the authoritative
writer must host context-mode's SessionStart hook, because that hook is what
prunes sessions. A future transport change that drops the hook silently disables
pruning, which is exactly how the 361 MB store grew.

## Declaration and adoption

The store path is declared as target state (USG-99, mechanism revised). Both
variables are declared together, at every writer spawn point, because they name
the same store with different values:

```
CONTEXT_MODE_DIR=/Users/developer/.local/share/context-mode
CONTEXT_MODE_DATA_DIR=/Users/developer/.local/share
```

`ServiceSpec { environment }` is **not** the mechanism. The field exists and is
digested (`crates/commonkit-adapters/src/service_lifecycle.rs:21`, `:25`), but no
production `ServiceBackend` exists: `install_argv` emits a literal
`"<definition>"` placeholder for every backend (`:60`, `:70`), and the
repository's one plist renderer
(`crates/commonkit-cli/src/daemon_lifecycle.rs:135`) is hardcoded to
`commonkitd`'s own label and emits no `EnvironmentVariables` key. `services` is
no longer a layer field at all. The declaration is therefore a `files` resource
on the file that carries the environment into the agent runtime, which is the
same gap G7 already describes.

Omission is drift. The relevant predicate is *absent*, not *empty*: the target
file carries no `env` key at all today.

The store needs no new adapter (USG-100). The question the ticket asked has been
overtaken: the `databases` layer field no longer exists. `V1_LAYER_SPEC_FIELDS`
is now `capabilities`, `contextBudget`, `files`, `packages`, `securityPolicy`
(`crates/commonkit-contracts/src/lib.rs:875-881`), the layer schema no longer
declares `databases`, and G5 is recorded closed repo-wide rather than for this
case. Nothing here reopens it. The outcome is the one the scope wanted and the
reason is stronger: the store is machine-local mutable state, and mutable state
is never layer-declared data, so a `databases` layer adapter would not have
served it even when the field existed.

The store directory is a `files` concern and ships today as
`FilesystemIntent::Directory`
(`crates/commonkit-adapters/src/resources.rs:170-181`).

The surviving `databases` is the runtime snapshot configuration
(`crates/commonkit-service/src/production_domains.rs:293`), which is wired end to
end and is a daemon configuration surface, not a layer field.

## The store as a resource

The store is inventoried at the **target override** layer as a single `files`
Directory resource at the canonical path, mode `0700`, `exact: false` (USG-101).
`exact: false` matters: CommonKit owns the directory's existence and mode, never
its contents. The personal kit is Git-backed (`CONTEXT.md:464`) and `$delete` is
registered nowhere (dogfood inventory G6), so anything landed in the personal
kit could never later be removed from one machine.

Stores are per-target by definition (USG-103). The VPS runs its own
`context-mode-server` against its own store, and that store stays its own. Each
target's store is a distinct `DatabaseId` — `context-mode-local`,
`context-mode-vps` — never one identifier shared across targets. Snapshot and
restore move a store between machines as an operator act; they do not
synchronize project memory, and no reconcile carries memory across targets.

## Retention and observation

context-mode owns retention and CommonKit never deletes (USG-102, thresholds
revised).

The recorded horizons are context-mode's shipped ones — 1,000 events per
session, 7 days for sessions, 14 days for content databases and indexed sources
— not figures CommonKit invents, enforces, or could keep in step. Pruning is
neither reconcile-time nor scheduled; it is driven by the writer's own
SessionStart hook, which is why the writer must host it.

Store size is **not** a verify assertion, permanently in v1 rather than
deferred. `Adapter::verify` returns `Result<(), AdapterFailure>` and has no
warning channel, so a size observation could only be a pass or a hard failure,
and a large store is neither wrong nor a reason to fail a plan.

## Snapshot and rollback

The store is excluded from plan rollback, and the exclusion is **declared**
rather than inherited from filesystem behaviour (USG-106, default revised).

Rollback restores configuration — the declared store path and the environment
that carries it — and never removes the store directory or its contents. The
exclusion cannot rest on current behaviour: `restore_preimage_in` calls
`remove_entry_in` unconditionally before every branch, and `remove_entry_in` is
a non-recursive `remove_dir`, so today a rollback touching a populated store
directory fails `ENOTEMPTY` rather than declining. Failing is safer than
deleting, but it is an accident, not a contract. This scope makes it explicit:
rollback leaves a populated managed directory in place and reports success.

Snapshot is operator-initiated and explicit: consistent SQLite export, integrity
check, client-side encryption, upload, and a content-addressed descriptor in
portable state. A `snapshots.databases` row names one database file, and the
store is a directory of many, so one row is not the whole store.

## First reconcile

A fresh target has no project memory store, and that is correct (USG-107).

First reconcile creates the store directory as a non-exact `files` Directory
resource and publishes the two environment variables. It does **not** start a
writer: the stdio writer is spawned per session by the agent runtime, and
`commonkitd` never owns its lifecycle. The store is empty, nothing is seeded,
and an empty store is a verify **pass** rather than a warning. A fresh machine
with no project memory is not in drift.

Ordering is the real fresh-target hazard, not emptiness. Store paths are derived
from the environment, so a session that starts before the declaration is applied
writes to the default path and silently creates a second store.

## Terminology

USG-105 lands the terms in `CONTEXT.md`. This scope proposes one glossary entry
and one boundary statement.

**Project memory store**: A target-local database holding indexed project
content and session memory for one agent-context tool, at a path CommonKit
declares, observed and snapshotted as mutable state and never carried in
portable state.
_Avoid_: Cache, index, knowledge base

The entry deliberately does not claim a single authoritative writing process;
the writer is per-session and the invariant is target-scoped.

The boundary: an **About Me Profile** is owner memory, separately encrypted,
with its own writer (ADR 0010). A project memory store is project memory, is not
encrypted by CommonKit, and holds no owner claims. Project memory is never
**Scope**-shareable — it is target-local mutable state and its contents are
unredacted.

## Explicit v1 limits

No upstream context-mode change; `CONTEXT_MODE_DIR` and `CONTEXT_MODE_DATA_DIR`
are used as they ship. No project re-keying: the key definition belongs to
context-mode and the collapsed home-directory bucket stays as a legacy bucket
(USG-108). No cross-target project-memory sync and no merge of stores belonging
to different targets. No new adapter for the `databases` layer field; G5 stays
open for its other fields. No scheduled pruning. No CommonKit-side store size
metric, threshold, or alarm. No content-level inspection, redaction, search, or
export of store contents by CommonKit.

## Implementation start gate

- USG-96 is resolved and its merge invariants are recorded on the issue. Done.
- The union merge tool exists, is re-runnable, and has produced a merged store
  plus collision records from copies of both live stores. Done 2026-08-05.
- The 3766 launchd service is unloaded before the source stores are archived,
  because it opens `~/.codex/context-mode` on start.
- Both source stores are archived read-only and their digests recorded before
  any writer is repointed.
- Claude's remote SSE registration is removed, not merely reordered. Done
  2026-08-05 — note this is the inverse of the original gate, which assumed the
  stdio registration would be the one removed.

## Defaults still requiring confirmation

1. Whether the archived source stores are retained indefinitely or expire
   (USG-96).
2. Whether the declaration mechanism stays a `files` resource on an
   agent-runtime settings file, or waits for a production `ServiceBackend` that
   can materialize `ServiceSpec { environment }` (USG-99).
