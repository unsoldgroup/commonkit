# Shared knowledge v1

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
