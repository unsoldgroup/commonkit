# Separate portable configuration from mutable state snapshots

CommonKit uses a GitHub repository as the durable store for versioned configuration, policy, capabilities, and snapshot descriptors. A local CommonKit service powers the CLI, macOS status bar, and MCP tools as peer interfaces. Mutable SQLite databases are never synchronized through Git: adapters create consistent, integrity-checked, encrypted snapshots in S3-compatible object storage, and version 1 assigns one authoritative writer per database to avoid unsafe binary merges and ambiguous conflict resolution.
