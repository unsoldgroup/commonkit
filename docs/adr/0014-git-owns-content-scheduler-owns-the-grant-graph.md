# Git owns shared content, the authoritative scheduler owns the grant graph

Multiplayer CommonKit needs state that two people write, which CONTEXT.md's
rules appear to forbid: a GitHub repository is the durable store for portable
reviewable state, mutable databases never synchronize through Git, and version
1 assigns one authoritative writer per database (ADR 0003). The rules are not
in conflict with multiplayer once shared state is split by what must be live.
Shared **content** — skills, loadouts, policy, styleguides — stays Git-owned
and PR-reviewed exactly as today, because a grant of content is a change to
desired state and **Reconciliation** already plans, applies, verifies, and
rolls it back. Only the **Grant graph** — scope membership and which scope
lends which capability to which — moves into the existing authoritative
service, because only it must be evaluated at submit time and revoked without
waiting for a pull. This adds no database and no hosting: `commonkit-execd`
already carries an authoritative scheduler with per-target worker identity,
tokens, and heartbeats, and ADR 0012's single-SQLite-writer limit holds
unchanged, remaining correct for a handful of targets and still not high
availability. Rejected: a Git-only graph, where revocation lands whenever the
grantee next pulls, so revocation latency is unbounded and an audience floor
computed from a stale checkout is not a floor. Also rejected: a new
multi-writer service at qm parity, which would retire Git as the durable store
for portable state and add identity, authentication, hosting, and cost to buy
liveness for content that does not need it. The cost accepted is that a grant
is not atomic: the graph edge and the content it points at are committed
separately, so CommonKit must treat an edge whose content is absent as
unresolved rather than as an error, and must never treat content reachable in
Git as granted without a live edge.
