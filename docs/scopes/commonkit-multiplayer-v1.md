# CommonKit multiplayer — flow map (working draft)

Status: **all twelve flows resolved**. Terms are in `CONTEXT.md`; the
hard decisions are ADRs 0014–0021. What remains is ticketing.

Destination: a ticketed backlog that adds a coworker axis to CommonKit —
scope-owned capabilities, grants between people, shadowing and promotion,
shared-scope memory, shared workspaces, and audience-floor policy resolution.

Prior art: `yc-software/qm` (MIT). Its scope model, resolution service, ACL
grants, audience floor, and skill lifecycle are the reference; its runtime
substrate (Postgres, hosted sandboxes, Slack) is not automatically adopted.

## The collision that gates everything

qm's multiplayer state is live Postgres. CommonKit's durable store is a Git
repository, with "one authoritative writer per database" and "secrets and
mutable database files do not belong in Git" as standing rules. Multiplayer
needs state that two people write. Flow 1 picks that authority; every later
flow inherits the answer.

## Flows

| # | Pain point | Resolution | Status |
|---|---|---|---|
| 1 | Two people need to write shared kit state; Git is single-writer by review and the local DB is single-writer by design | Git owns content; the authoritative scheduler owns the **Grant graph**, because only membership and grants must be live and revocable without a pull (ADR 0014) | resolved |
| 2 | CommonKit has no coworker axis: layering is a precedence chain, not a membership graph | **Scope** is an ownership axis orthogonal to the precedence chain; ADR 0001's structural org floor survives, where qm's is prose (ADR 0015) | resolved |
| 3 | Al distills a skill and wants Bea to have it without promoting it org-wide | team and org scopes share one repo, personal kits stay private repos; a grant is a mount at a handle path, removed on revoke (ADR 0016) | resolved |
| 4 | A team skill and an org skill share a name | lent content composes beneath the grantee's own, so accepting a grant is always safe; a team lends, only policy binds (ADR 0017) | resolved |
| 5 | A granted skill costs the recipient router text against a budget they did not choose | the grantee's Loadout pays; a grant over budget stays unresolved and is reported, never silently admitted | resolved |
| 6 | A Job submitted in a shared scope must not exceed the least-privileged participant | two-principal intersection, submitter and target owner, recorded in the receipt as **Isolation level** is (ADR 0018) | resolved |
| 7 | Bea submits a Declared task to an Execution Target Al owns | placement needs a grant edge; a credential-requiring **Declared task** does not place off-owner at all (ADR 0019) | resolved |
| 8 | About Me Profile is single-owner, encrypted, one writer; multiplayer wants team knowledge | About Me remains private; opaque Engram chunks may cross principals only with project-scope attestation and prospective revocation (ADR 0022) | resolved |
| 9 | CommonKit has no principal concept at all | a **Principal** is a GitHub login brokered by the installed `gh`, resolved identically on every surface (ADR 0021) | resolved |
| 10 | The Session Board shows every machine but decides nothing (ADR 0008) | team-wide visibility, verbs bound to the session's owner, so no one writes an "Always" rule into another's project | resolved |
| 11 | Secrets are non-portable and provisioned per-target; a shared scope has no credential story | refused for v1: no shared credentials; follows from ADR 0019 | resolved |
| 12 | A coworker joins the kit | add to the graph, extend repo read, first reconcile materializes team and org content plus resolved grants | resolved |

## Not yet specified

- Revocation latency. The graph is live, but a grantee's target only stops
  materializing lent content at its next **Reconciliation**. Whether that
  needs a wake path is unresolved.
- Two people editing the same team-scope skill. It is a pull request into
  the shared repository, so Git resolves it — but nothing yet says what a
  **Grant** pointing at a half-merged branch does.
- Contractors and external guests: a fourth **Scope** kind, or an
  entitlement on an existing one.
- **Styleguide** under grant. A **Loadout** selects at most one, so a grant
  carrying a second is a conflict ADR 0017's shadowing rule may or may not
  cover — single-valued capabilities may need their own rule.
- Whether an unresolved **Grant** is reported once, every reconcile, or
  escalates.

## Process practices adopted from qm, independent of this feature

- Zero comments in the repository; intent through names, structure, tests.
- Never self-review in the authoring context; dispatch an independent
  reviewer, and let the reviewer set the depth.
- Durable by default: nothing the system reads back later lives in RAM.
