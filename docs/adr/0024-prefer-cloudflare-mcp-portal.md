# Prefer a Cloudflare MCP server portal for eligible cloud capabilities

For organization-governed remote HTTP MCP servers, clients should connect to a
Cloudflare MCP server portal by default. The portal provides one endpoint,
Access identity and policy, explicit tool exposure, context optimization, and
central remote-call logs. The device relay remains the fallback for stdio,
local, private, offline, and portal-incompatible capabilities; it is no longer
the preferred aggregator for ordinary remote HTTP servers. CommonKit continues
to own canonical capability declarations, grants, credential references,
plans, and receipts. Neither the portal nor the relay becomes a second registry.

This supersedes ADR 0010's relay-first routing decision. It does not claim the
Cloudflare adapter is already implemented: until plan-bound portal and Access
reconciliation ships and passes compatibility tests, operators provision the
portal separately and the local relay remains the operational fallback.

## Consequences

- A default cloud loadout may expose one portal endpoint plus a local relay
  endpoint, not every remote MCP server separately.
- Portal tool mappings use deny-by-default allowlists and per-user OAuth unless
  shared authority is an explicit policy choice.
- Every upstream needs a compatibility, revocation, and failure-mode test.
- Stdio-only and device-private servers never move to the portal merely for
  uniformity.
- CommonKit needs a Cloudflare adapter that plans and verifies portals, DNS,
  Access policies, upstream mappings, and tool allowlists without storing
  Cloudflare or upstream secret values.
