# Cloudflare integration

Cloudflare integration is optional. The CommonKit CLI, daemon, reconciliation,
normal Claude and Codex permission prompts, and local MCP relay work without a
Cloudflare account.

Cloudflare provides two separate edge services for teams that want them:

1. an MCP server portal for cloud MCP consolidation; and
2. Cloudflare Tunnel and Access for the optional Session Board.

CommonKit remains the source of truth for desired state, policy, credential
references, and receipts. Cloudflare is a replaceable capability data plane and
access boundary. Never copy Cloudflare, GitHub OAuth, or upstream MCP secrets
into a kit repository.

## Responsibility boundary

| Component | Owns | Does not own |
| --- | --- | --- |
| CommonKit | Portable MCP declarations, tool grants, credential references, target policy, plans, and receipts | Cloudflare sessions, upstream OAuth tokens, or secret values |
| Cloudflare MCP server portal | One remote endpoint, Access policy, upstream HTTP routing, tool exposure, and portal logs | Canonical CommonKit declarations or local/stdio tools |
| Local MCP relay | Target-local stdio, private, offline, and portal-incompatible capabilities | Organization identity or the canonical capability registry |
| Cloudflare Access | Authentication and edge admission | CommonKit authorization or Session Board decisions |
| GitHub | Identity provider for Session Board users | Board authorization data or CommonKit target state |

## MCP server portal: preferred cloud path

Use a [Cloudflare MCP server portal](https://developers.cloudflare.com/cloudflare-one/access-controls/ai-controls/mcp-portals/)
as the preferred shared endpoint for eligible remote HTTP MCP servers. A portal
can expose a curated tool set from several upstream servers through one URL,
apply Access policies, keep per-user upstream OAuth, and record remote calls.
Its context-optimization and Code Mode settings can reduce the initial tool
schema cost when a portal contains many tools.

Use the local relay instead when an upstream:

- runs over stdio or depends on a local process;
- must work offline or keep data and credentials on one Target;
- has no remote HTTP endpoint;
- rejects proxy clients; or
- must remain available when Cloudflare or the network is unavailable.

The local relay is the less-preferred aggregation path for ordinary remote HTTP
servers. It remains the required compatibility path for capabilities a portal
cannot carry.

### Portal setup

1. Add an identity provider to Cloudflare Zero Trust.
2. Register each eligible remote HTTP MCP server under **Access controls > AI
   controls**. Give every server its own Access policy.
3. Create a portal on a Cloudflare-managed hostname and add only the servers
   needed by that audience. If the portal is created by API or Terraform, also
   create the required proxied CNAME to `gateway.agents.cloudflare.com`.
4. Default to per-user upstream OAuth. Use admin credentials only when shared
   authority is intentional and documented.
5. Hide all tools by default and allowlist the small set each portal needs.
6. Connect Claude, Codex, or another remote-MCP client to the portal `/mcp`
   endpoint. Keep the local relay configured for non-portal capabilities.
7. Verify login, tool discovery, one harmless tool call, revocation, and the
   behavior when the portal is unavailable before making it a team default.

For unattended agents, an Access service token needs a matching **Service
Auth** policy at both the portal and every linked MCP server. A server mapped
for per-user OAuth is intentionally unavailable to that service-token session.

### Portal limits and controls

- Portals accept remote Streamable HTTP and SSE upstreams, not stdio-only
  servers. Gateway routing supports Streamable HTTP only.
- A portal currently supports up to 40 MCP servers. Split portals by audience
  and responsibility rather than treating that limit as a target.
- Device authentication without a browser redirect is not supported. Human
  sessions use the configured Access identity provider.
- An upstream may block proxy clients. Test every server before removing its
  direct or local fallback.
- Admin OAuth credentials can expire without notification. Monitor server
  status and test reauthentication.
- Access protects the portal hostname, not a directly reachable upstream URL.
  Secure each upstream separately when bypassing the portal would be unsafe.
- Independent MFA, purpose justification, and temporary authentication have
  portal-specific policy limits. Do not assume an upstream policy adds a second
  challenge after the user has entered the portal.
- Code Mode executes generated JavaScript in an isolated Dynamic Worker. Make
  it opt-in until the portal's tool allowlist and evaluation suite prove it.
- Optional Gateway routing adds HTTP logs and DLP inspection but also adds a
  failure boundary and changes egress behavior.

The CommonKit Cloudflare adapter is designed but not shipped. Today an operator
creates the portal and records its endpoint and credential references in the
kit. A future adapter should plan portal, DNS, server, tool-allowlist, and Access
policy changes through the normal `sync -> diff -> apply -> verify` contract.
Until that adapter is verified, CommonKit must not claim it provisioned or
audited live Cloudflare state.

## Session Board: optional, GitHub-protected

The Session Board is not part of CommonKit onboarding and never gates a CLI or
daemon release. When a team chooses to deploy it, protect every browser route
with Cloudflare Access and use GitHub as the Access identity provider. Do not
offer One-time PIN as a fallback login method for the Board.

1. Create a GitHub OAuth app for the Cloudflare Access team domain. Use
   `https://<team>.cloudflareaccess.com/cdn-cgi/access/callback` as its callback.
2. Add GitHub under **Zero Trust > Integrations > Identity providers**, then
   test the connection.
3. Keep the Board hub on a private origin. Publish it through Cloudflare Tunnel
   and a proxied hostname; do not expose the hub port directly.
4. Create a self-hosted Access application for the Board hostname. Allow only
   explicit GitHub users or members of the intended GitHub organization/team.
   Never use **Include everyone** or **all valid emails**.
5. Set `SESSION_BOARD_ACCESS_TEAM_DOMAIN` and
   `SESSION_BOARD_ACCESS_AUD` on the hub. The hub verifies the signed
   `Cf-Access-Jwt-Assertion` against the team JWKS instead of trusting the
   header alone.
6. Give machine reporters a separate path-specific Access application for
   `/reporter` with **Service Auth** when supported. If the current reporter is
   used without those headers, use a **Bypass** policy only on that
   path-specific application and keep the Board's independent, per-Target
   reporter token. Never bypass the hostname or browser routes.
7. Test an allowed GitHub user, a denied user, an expired Access session, a
   reporter reconnect, and the normal terminal fallback before relying on the
   deployment.

Every approval remains a human action bound to the owning Principal. Access
decides who may reach the Board; it does not approve agent actions. If the
Board, Tunnel, Access, or reporter is unavailable, CommonKit returns control to
the agent client's normal terminal prompt.

See [Session Board deployment](session-board.md) for the origin and reporter
runbook, [Cloudflare's GitHub identity-provider guide](https://developers.cloudflare.com/cloudflare-one/integrations/identity-providers/github/),
and [Cloudflare Access policy guidance](https://developers.cloudflare.com/cloudflare-one/access-controls/policies/).
