# MCP fronts a separate durable execution service

Remote Claude and Codex environments discover CommonKit context and submit repository-declared task IDs through MCP, but `commonkit-execd` owns jobs, attempts, leases, events, checkpoints, and artifacts in SQLite. MCP request and session lifetimes cannot provide restart recovery, monotonic fencing, or offline reconnect guarantees, so MCP is the capability/context plane rather than the persistence mechanism.
