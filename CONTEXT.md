# CommonKit

CommonKit is a portable, versioned collection of shared developer capabilities that can be materialized consistently across local machines, remote hosts, projects, and coding agents.

## Language

**CommonKit**:
The complete portable collection of shared developer capabilities and policies.
_Avoid_: Toolbox, toolchain, environment sync

**Loadout**:
A selected subset and set of overrides from a CommonKit for a particular target.
_Avoid_: Profile, bundle

**Target**:
A local or remote machine where a loadout is materialized.
_Avoid_: Box, VPS, environment

**Adapter**:
A translator between CommonKit's normalized model and an agent or service's native configuration.
_Avoid_: Plugin, integration

**Reconciliation**:
The process of planning, applying, and verifying a target against its selected loadout.
_Avoid_: File copy, deployment

## Relationships

- A **CommonKit** defines one or more **Loadouts**.
- A **Loadout** is materialized on one or more **Targets**.
- An **Adapter** participates in **Reconciliation** for one agent or service.
- **Reconciliation** never treats secrets or machine identity as portable CommonKit content.

## Example dialogue

> **Developer:** "Does my remote Codex session have the same hooks and skills as my Mac?"
> **Domain expert:** "Apply the remote-development **Loadout** from your **CommonKit** to that **Target**, then verify its **Adapters**."

## Flagged ambiguities

- "Toolbox" was the initial metaphor; resolved: the product and domain object are **CommonKit**.
- "Profile" described a selected subset; resolved: use **Loadout** unless later user research favors a more conventional term.

## Open — not yet resolved

- Whether a CommonKit is primarily personal, team-owned, or composed from layers.
- Whether Git is mandatory or one possible CommonKit backend.
- Which flows belong in the first public release.
- Whether MCP relay lifecycle is an adapter or a separate companion product.
