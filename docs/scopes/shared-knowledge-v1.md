# Shared knowledge v1 — design brief

Tracking: unfiled. Belongs under epic USG-110 (CommonKit multiplayer).

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

## Scope decision from the owner

First customer: **both one person's several machines and a team, designed
together** (decided 2026-08-06). So the confidentiality model must be present
from v1; a device-only design that defers it is rejected.

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
