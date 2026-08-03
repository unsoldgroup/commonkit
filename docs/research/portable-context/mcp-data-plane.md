# MCP data-plane architecture

## Recommendation

Use a hybrid architecture with three explicit planes:

1. **Context plane — CommonKit-owned.** Selects organization documentation,
   project context, and user profile material; applies authorization,
   precedence, relevance, context budgets, rendering, and provenance receipts.
2. **Capability control plane — CommonKit-owned.** Resolves MCP servers, tools,
   grants, credentials references, and desired state from layered CommonKit
   inputs.
3. **Capability data plane — replaceable adapters.** A persistent device relay
   handles local/private capabilities. An optional organization portal handles
   remote organization-governed MCPs.

Cloudflare calls its current hosted product **MCP server portals**. “Agents
Gateway” is an obsolete name. Cloudflare Gateway is a separate Zero Trust/DLP
layer through which portal traffic can optionally be routed.

## Comparison

| Concern | Persistent device relay | Cloudflare MCP server portal |
|---|---|---|
| Best use | Local stdio processes, offline resources, local credentials, immediate discovery | Organization remote servers, Access identity, centralized policy and audit |
| Client endpoint | One loopback endpoint per target | One organization HTTP endpoint per portal |
| Stdio upstreams | Native | Unsupported unless separately hosted behind HTTP |
| Offline operation | Available for local upstreams | Unavailable |
| Credentials | Can remain in OS keychain/device secret provider | OAuth, admin credentials, headers, or service tokens in Cloudflare's control boundary |
| Discovery | Immediate; exact cache behavior is CommonKit-controlled | Managed synchronization; documentation reports background discovery rather than immediate local updates |
| Context optimization | Cached discovery and CommonKit selection | Portal modes include tool minimization, search-and-execute, and Code Mode |
| Organization policy | CommonKit must implement and distribute enforcement | Cloudflare Access policies and per-portal tool/prompt selection |
| Observability | Must be instrumented; can correlate exact CommonKit receipts | Managed per-call portal logs; Logpush export is Enterprise-only |
| Failure domain | Per device; local tools can survive network failure | Shared portal, Access, network, and upstream chain |
| Scale cost | Lifecycle and drift multiply by target | Central remote aggregation, with documented account limits |
| Private user profile | Strong fit; keep canonical content local-first | Do not route private canonical profile content through the portal by default |

## Why neither option is sufficient alone

The local relay solves a target problem: it keeps local and remote upstreams
warm, bridges stdio, protects device-local credentials, and gives every local
client a stable endpoint. By itself it does not provide centralized
organization identity, audit, revocation, or uniform remote enforcement.

The Cloudflare portal solves an organization edge problem: it aggregates remote
MCP servers behind Access, selects exposed tools, and centralizes remote-call
telemetry. It does not replace local processes, offline operation, CommonKit's
profile/document selection, or CommonKit's explanation and receipt model.

## Proposed request path

```text
organization + project + user layers
              |
     CommonKit capability resolver
       | desired state + grants
       +--------------------------+
       |                          |
device relay adapter       hosted portal adapter
stdio/local/private        remote/org-governed
       |                          |
       +----------- MCP calls ----+
                    |
       correlated CommonKit receipt
```

The client should normally connect to the device relay. The relay can route an
organization-governed remote capability through the configured portal while
keeping local capabilities local. Direct portal connection may remain an
explicit thin-client mode.

## Constraints to verify in a spike

- Portal support for each required upstream authentication pattern.
- Tool-name collision and namespacing behavior.
- Freshness after an upstream changes its tool list.
- Service-token behavior for unattended agents.
- Redaction and retention of portal and optional Gateway logs.
- Failover behavior when the portal is unavailable.
- Whether the current default limit of 40 servers per portal requires sharding.
- Terraform/API coverage sufficient for plan-bound CommonKit reconciliation.

## Primary sources

- [Cloudflare MCP server portals](https://developers.cloudflare.com/cloudflare-one/access-controls/ai-controls/mcp-portals/)
- [Cloudflare account limits](https://developers.cloudflare.com/cloudflare-one/account-limits/)
- [Cloudflare portal Logpush](https://developers.cloudflare.com/changelog/post/2026-02-27-mcp-portal-logpush/)
- [Cloudflare MCP server](https://github.com/cloudflare/mcp)
- [Official MCP SDK organization](https://github.com/modelcontextprotocol)
- [Docker MCP Gateway](https://github.com/docker/mcp-gateway)
- [Microsoft MCP Gateway](https://github.com/microsoft/mcp-gateway)
- [IBM ContextForge](https://github.com/IBM/mcp-context-forge)
- [MCP Inspector](https://github.com/modelcontextprotocol/inspector)
- [Automerge](https://github.com/automerge/automerge)
