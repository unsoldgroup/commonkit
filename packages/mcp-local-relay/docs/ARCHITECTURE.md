# Architecture

`mcp-local-relay` is one local Streamable HTTP MCP server that aggregates upstream MCP servers.

The client connects once:

```text
Codex / Claude / Cursor -> http://127.0.0.1:3764/mcp
```

The relay then manages upstreams:

```text
mcp-local-relay -> remote Streamable HTTP MCPs
```

Each upstream server has a cached `tools/list` result. Local tool names are prefixed as:

```text
<server-id>__<upstream-tool-name>
```

The relay also exposes `relay_*` management tools. These tools can validate, add, update, remove, enable, disable, and refresh upstream MCP servers at runtime.

## Hot Swap

When upstream configuration changes, the relay:

1. validates the new server if needed
2. writes config atomically
3. refreshes the tool cache
4. emits `notifications/tools/list_changed`

Clients that honor tool-list change notifications can discover new tools without restarting their session.

The hotswap control surface is exposed through MCP tools on the relay itself:

- `relay_validate_server`
- `relay_add_server`
- `relay_update_server`
- `relay_remove_server`
- `relay_enable_server`
- `relay_disable_server`
- `relay_refresh_tools`

`relay_add_server` validates the Streamable HTTP upstream, caches its tools, writes the relay config atomically, and emits `notifications/tools/list_changed` to active sessions.
