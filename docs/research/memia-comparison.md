# CommonKit compared with MEMIA (`@kairros/memia`)

**Status**: research note. Not a product commitment, not a positioning change.
**Reviewed package**: `@kairros/memia@0.2.1`, published 2026-08-16, `UNLICENSED`,
no declared repository or homepage. Evidence below comes from reading the
published npm tarball (31 files, ~4,000 lines of JavaScript), not from
marketing copy.

## Short answer

MEMIA is not a competitor to CommonKit. The two products overlap on exactly one
narrow surface — both write files that coding agents read (`CLAUDE.md`,
`AGENTS.md`, `.cursor/rules/`, `.mcp.json`) from a single source of truth — and
they disagree about almost everything else, because they are solving different
problems.

- **CommonKit** answers: *how do I get my proven development setup onto every
  machine and every agent, safely and reversibly?* It is a desired-state
  configuration runtime.
- **MEMIA** answers: *how do I stop losing facts, commitments and decisions
  between conversations, tools and people?* It is a local working-memory and
  commitment tracker.

CommonKit moves **capabilities** (instructions, skills, hooks, tools, policies).
MEMIA accumulates **facts** (operations, projects, commitments, resources,
assets). Where CommonKit already has a facts-shaped feature — the About Me
profile — MEMIA is a more developed version of that one feature and nothing
else CommonKit does.

## What MEMIA actually is

MEMIA (from Kairros) is a French/English, local-first CLI. `memia init` creates
an "OPERA workspace" on disk: five folders of JSON entity files
(`operations`, `projects`, `engagements`, `resources`, `assets`) plus an
append-only journal. Everything else in the package reads from or writes to
that workspace.

Its five headline capabilities:

1. **Commitment detection with confidence thresholds.** `memia scan
   export.json` ingests a local mail/message export and scores each candidate
   commitment. Above 80 it becomes a tracked engagement; 40–80 becomes a
   proposal you confirm; below 40 it is journaled and dropped. It explicitly
   refuses to infer a due date it cannot justify.
2. **An agent harness.** 218 prewritten agent definitions (12 core, 6 "board",
   200 specialists) that run through the Codex or Claude Code CLI *already
   installed on your machine*. Sandbox flags are always explicit and the
   package never passes `danger-full-access` or `bypassPermissions`. Read-only
   agents get `Read,Grep,Glob`; writers additionally get `Edit,Write`; all of
   them get `WebFetch,WebSearch,Bash` explicitly disallowed.
3. **Multi-tool projections.** The overlap with CommonKit — see below.
4. **A read-only MCP server** (`memia mcp serve`) exposing the workspace.
5. **An egress gate ("sas").** Every outbound send is denied by default and
   journaled. The package deliberately ships *no* send adapter at all, so
   approving a request produces a ticket that nothing can currently redeem.

Inbound connectors (Gmail, Outlook, WhatsApp via a local QR, Discord) are
consent-based and receive-only.

## The one real overlap: projections

`memia projections write` takes a canonical instruction block and writes it to
four targets: `AGENTS.md` (full text), `CLAUDE.md` (a short pointer plus the
invariants), `.cursor/rules/memia.mdc`, and `.mcp.json` (additively merged, so
other MCP servers survive). `memia projections check` then re-reads those files
and compares the invariant block byte-for-byte against a recomputed SHA-256.

This is recognisably the same instinct as CommonKit's
`sync → diff → apply → verify`, and the comparison is worth taking seriously
because MEMIA got the *shape* right:

| Concern | MEMIA | CommonKit |
| --- | --- | --- |
| Single source of truth | OPERA `resources/instruction-canonique` | Git-owned layered kit |
| Materialization | `projections write` | provider → artifact store → adapters |
| Drift detection | `projections check`, hash equality | `verify`, receipts, observed state |
| Scope of content | one hand-written invariants block | instructions, skills, hooks, plugins, MCP declarations, services, credential references |
| Targets | files in one directory | local, SSH, and per-target overrides |
| Preview before change | none — `write` writes | plan is mandatory; `apply` needs `--confirmed` |
| Undo | none | `rollback <run-id>`, content-addressed preimages |
| Conflict handling | overwrites its own files unconditionally | ownership boundaries; stale plan rejected on any input change |

The honest read: MEMIA's projection layer is roughly what CommonKit's
materialization would look like if you deleted the plan, the receipt, the
artifact store, the ownership model, and multi-target support. That is not a
criticism of MEMIA — it writes four files it owns entirely, in one directory,
so it does not need any of that. It is a useful demonstration that the
*minimum viable* version of "one source, many agent formats" is about 270 lines
of JavaScript, and that CommonKit's weight only pays for itself once you have
multiple targets, content you did not author, and changes you might regret.

## Where the two genuinely differ

**Architecture.** MEMIA is ~4,000 lines of dependency-light Node (two runtime
dependencies, both for WhatsApp). CommonKit is 16 Rust crates, a daemon, a
target helper, a Tauri desktop app, and a pnpm workspace. MEMIA is a tool.
CommonKit is a runtime.

**What travels.** MEMIA's workspace is emphatically local — "aucune donnée ne
quitte le poste" (no data leaves the machine). There is no sync, no remote
target, no team story. CommonKit's entire premise is that context travels
across machines, colleagues, and an organization kit, with encryption for the
parts that must not.

**Safety model.** Both fail closed, but at different doors. MEMIA gates
*outbound communication* (nothing leaves the machine without approval).
CommonKit gates *mutation* (nothing changes a target without a reviewed plan
and an explicit `--confirmed`, and anything applied can be rolled back).

**Treatment of agents.** MEMIA ships opinions — 218 named agent roles with
fixed missions and write permissions. CommonKit ships no opinions about which
agents you should have; it carries the ones you already proved useful.

**Provenance.** MEMIA's strongest idea, and the one with no CommonKit
equivalent: every entity must carry a `{type, ref}` source, every detection
score carries its evidence, and a fact without an origin is rejected outright.

## What is worth borrowing

Three ideas, in order of how cheaply CommonKit could adopt them.

1. **Byte-exact invariant verification with an embedded hash.** MEMIA embeds
   `<!-- memia:invariants:v2:fr:sha256=… -->` in every projected file, so drift
   detection needs no database lookup — the file states what it should hash to.
   CommonKit verifies through receipts and observed state, which is stronger,
   but the embedded marker makes a hand-edited file self-evidently wrong to a
   human reading it. Cheap, and good for the "do not edit by hand" contract.
2. **Mandatory provenance on stored claims.** The About Me profile
   (`crates/commonkit-about-me`) stores approved claims about the kit owner.
   MEMIA's rule — no entity without a source, no inferred value that cannot be
   justified — maps onto it directly and would make the profile auditable.
   Related: ADR 0008 (`import-by-budget-distill-by-retention`).
3. **Confidence thresholds with a silence band.** MEMIA's 80/40 split means
   low-confidence detections are journaled and never surfaced, and the middle
   band always asks rather than guesses. CommonKit's About Me suggestion flow
   is binary (suggest or do not). A calibrated three-way split, with user
   decisions feeding `engagements calibrage`, is a better shape for anything
   that proposes memories to a human.

## What is worth avoiding

- **Unconditional writes.** `projeter()` calls `writeFileSync` on `AGENTS.md`
  and `CLAUDE.md` with no read-back, no diff, and no check for local edits. A
  repository whose `CLAUDE.md` you maintain by hand loses it silently. This is
  precisely the failure CommonKit's plan step exists to prevent, and it is the
  clearest argument for CommonKit's extra machinery.
- **An egress gate that gates nothing.** Denying by default when no send
  adapter exists is honest but unproven; the first real adapter is where that
  design gets tested.
- **`UNLICENSED` with no public repository.** Unauditable, and unusable as a
  dependency.

## Positioning consequence

None. CommonKit's `CONTEXT.md` positioning stands unchanged: MEMIA does not
compete for the "your best development setup, everywhere" promise, and
CommonKit does not compete for "stop losing commitments between tools." The two
could coexist on one machine — MEMIA's workspace would simply be another kind
of context that CommonKit could carry.

The one adjacent claim to watch is MEMIA's "reprise" invariant: *work started
on one surface continues on another*. That is a continuity promise phrased very
close to CommonKit's portability promise, and if MEMIA ever adds a second
machine, the two roadmaps start converging.
