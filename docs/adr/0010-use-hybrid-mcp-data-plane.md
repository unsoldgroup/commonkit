# Use a hybrid MCP data plane

Superseded by [ADR 0024](0024-prefer-cloudflare-mcp-portal.md). This record
preserves the original local-relay-first decision.

CommonKit owns context selection and capability desired state, while agents connect by default to a persistent device relay for local, stdio, offline, and private capabilities. Organizations may add a hosted MCP portal adapter for remote organization-governed capabilities, but no hosted portal becomes CommonKit's canonical registry; this preserves user and device boundaries while adding centralized identity, policy, scale, and audit where they are useful.
