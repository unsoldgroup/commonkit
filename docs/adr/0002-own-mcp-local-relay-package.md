# Own mcp-local-relay as a CommonKit workspace package

The CommonKit repository owns `mcp-local-relay` as an independently publishable package rather than treating it as an external companion repository. The package retains a distinct MCP data-plane boundary while CommonKit owns layered desired state and reconciliation, allowing coordinated development without conflating runtime relay behavior with policy composition.
