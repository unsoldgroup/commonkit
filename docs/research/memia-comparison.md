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

## Proposed tickets

Five candidates were drafted, each verified against CommonKit's current code
rather than assumed. Three were filed on team `Unsoldantarctica`; the drafting
of candidates 3 and 5 is kept below with the reasoning for folding one in and
declining the other.

| # | Candidate | Outcome |
| --- | --- | --- |
| 1 | `CLAUDE.md` as a pointer to `AGENTS.md` | Filed — [UNS-1659](https://linear.app/unsoldantarctica/issue/UNS-1659), High |
| 2 | Provenance header and content hash | Filed — [UNS-1660](https://linear.app/unsoldantarctica/issue/UNS-1660), Medium |
| 3 | Offline drift check | Folded into UNS-1660 |
| 4 | Source reference on About Me claims | Filed — [UNS-1661](https://linear.app/unsoldantarctica/issue/UNS-1661), Medium |
| 5 | Calibrated confidence bands | Not filed — see below |

### 1. Project `CLAUDE.md` as a pointer to `AGENTS.md` to cut always-on cost

**Filed as UNS-1659. Priority**: High.

**Goal**

Stop paying for the same agent instructions twice on every turn.

**Why now.** `crates/commonkit-adapters/src/apm.rs:1055` materializes
`AGENTS.md` and `CLAUDE.md` from the same composed source, and
`crates/commonkit-adapters/src/budget.rs:256` already classifies both as
`ALWAYS_ON_FILES` — "injected regardless of task". CommonKit therefore already
*measures* the duplication it creates. MEMIA avoids it: `AGENTS.md` carries the
full canonical instruction, and `CLAUDE.md` is a short pointer to it plus only
the invariant block, repeated verbatim.

**Scope**

* Add a loadout-level projection mode that emits one authoritative file and
  renders the others as pointers carrying only the content that must be
  repeated verbatim.
* Decide per agent which file is authoritative; do not hard-code `AGENTS.md`.
* Make the mode opt-in per loadout — an agent that cannot follow a pointer must
  still be able to receive full text.
* Report the saved always-on tokens through the existing loadout-cost command
  so the benefit is measured, not claimed.

**Done when**

* A loadout can be projected in pointer mode and the resulting agent session
  behaves equivalently to full-text mode on a recorded comparison.
* The loadout-cost report shows the before/after always-on delta.
* Full-text mode remains the default until the comparison is recorded.

### 2. Stamp every materialized file with a provenance header and content hash

**Filed as UNS-1660, with candidate 3 folded in. Priority**: Medium.

**Goal**

Make a hand-edited managed file self-evidently wrong to the person reading it,
without a daemon, a receipt, or a network call.

**Why now.** `grep -rniE "do not edit|generated by|managed by commonkit"` over
`crates/` returns nothing. CommonKit knows a file is managed; the file itself
does not say so. MEMIA embeds
`<!-- memia:invariants:v2:fr:sha256=… -->` in every projection, so the file
states what it should hash to and names the command that regenerates it.

**Scope**

* Define one header format for managed text files: that the file is
  materialized, the source layer or loadout it came from, the command that
  regenerates it, and a content hash of the managed region.
* Emit it from the file adapters that write managed text
  (`crates/commonkit-adapters/src/files.rs`, `apm.rs`).
* Respect formats that cannot carry comments; do not corrupt JSON or TOML
  targets.
* Ensure the header is part of the hashed region's identity, so editing the
  header is itself detectable.

**Done when**

* Every managed text file CommonKit writes carries the header.
* A hand-edited managed file is detectable from its own bytes alone.
* Non-comment formats are handled explicitly and documented.

### 3. Offline `commonkit check` for materialized-file drift

**Folded into UNS-1660, not filed separately.** The header without a reader is
half the value, and the reader cannot exist without the header, so the two are
one unit of work rather than a ticket and its blocker.

**Goal**

Answer "have my managed files been touched?" in under a second, with no daemon
running and no network.

**Why now.** `Verify` (`crates/commonkit-cli/src/main.rs:96`) checks managed
state and provider integrity through the daemon — correct, thorough, and too
heavy to run casually. MEMIA's `projections check` re-reads the files, compares
a hash, and exits. Depends on ticket 2 for the embedded hash.

**Scope**

* Add a read-only command that reads managed files, recomputes the hash from
  ticket 2, and reports per-file `present` / `identical`.
* No daemon dependency, no mutation, no provider invocation.
* Non-zero exit on drift, so it can run as a shell hook or a pre-commit check.
* Point its output at `verify` for the authoritative answer — this command
  is a smoke test, not a replacement.

**Done when**

* The command runs correctly with the daemon stopped.
* Drift, absence, and agreement are distinguished in the output.
* Documentation states plainly that `verify` remains authoritative.

### 4. Require a source reference, not just a source kind, on About Me claims

**Filed as UNS-1661. Priority**: Medium.

**Goal**

Make every stored claim about the kit owner traceable to where it came from.

**Why now.** `crates/commonkit-about-me/src/lib.rs:524` stores
`source_kind TEXT NOT NULL` — *what type* of source, not *which* source. A
claim can therefore be audited only to its category. MEMIA's rule is stricter:
every entity carries `{type, ref}`, and a fact without an origin is rejected at
write time. This is the strongest idea in the package.

**Scope**

* Extend the claim schema with a source reference alongside the existing kind,
  and reject writes that omit it.
* Migrate existing claims, marking pre-migration ones explicitly unsourced
  rather than inventing a reference for them.
* Surface the reference in `about-me summary` and `about-me search`.
* Keep references free of secret material and of anything that would leak into
  a portable artifact.

**Done when**

* A claim cannot be written without a source reference.
* Existing claims are migrated and visibly marked unsourced.
* The reference appears wherever a claim is displayed.
* Threat-model review confirms references carry nothing sensitive.

### 5. Calibrated confidence bands for About Me suggestions

**Not filed.** This is the one candidate that copies MEMIA's design without
evidence that CommonKit has the problem it solves. MEMIA needs bands because it
ingests whole mail exports unattended and would otherwise flood the user;
CommonKit's About Me suggestions arise from a direct statement in conversation,
already human-in-the-loop, at a volume nobody has complained about. Building
scoring, evidence capture and a calibration loop for an unobserved problem is
speculative work on a flow that currently behaves.

Revisit if suggestion fatigue is actually reported, or if About Me ever grows
an unattended ingestion path — that is the condition that makes the idea
necessary rather than merely elegant. The draft is kept below for that day.

**Goal**

Stop surfacing weak inferences to the user, and stop guessing on medium ones.

**Why now.** `grep -rniE "confidence|threshold|score"` over
`crates/commonkit-about-me/src` returns nothing — the suggestion flow is
binary, so every candidate either interrupts the user or is silently dropped.
MEMIA splits at 80/40: above 80 it records with evidence attached, 40–80 it
asks, below 40 it journals and stays silent. User decisions then feed a
calibration command.

**Scope**

* Score suggestion candidates and attach the evidence that produced the score.
* Three bands: record with evidence, ask the user, journal silently.
* Never infer a value the score cannot justify — prefer asking to guessing.
* Feed accept/correct/reject decisions back into calibration, and expose the
  current calibration for inspection.
* Depends on ticket 4: a scored claim needs a source reference to be auditable.

**Done when**

* Candidates are scored and banded, with evidence recorded for each.
* Low-confidence candidates never reach the user, but are recoverable from the
  journal.
* Calibration is inspectable and shifts measurably after recorded decisions.

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
