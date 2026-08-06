# Build the CommonKit runtime in Rust with Tauri 2

CommonKit version 1 uses a shared Rust core for composition, policy enforcement, reconciliation, scheduling, snapshots, and relay behavior. The CLI, headless local service, MCP server, and Tauri 2 desktop application all invoke that core; the existing Node implementations are behavioral migration sources rather than permanent sidecars. This adds an up-front rewrite but gives macOS, Linux, and Windows one self-contained runtime, installer, security boundary, and update path.
