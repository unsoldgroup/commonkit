use std::sync::Arc;

use commonkit_mcp::{CommonKitMcp, DaemonBackend};
use rmcp::{ServiceExt, transport::stdio};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let backend = Arc::new(DaemonBackend::discover()?);
    let service = CommonKitMcp::new(backend).serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
