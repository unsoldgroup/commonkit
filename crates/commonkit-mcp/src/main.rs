use std::sync::Arc;

use commonkit_mcp::{CommonKitMcp, DaemonBackend, ExecutionContext, RemoteExecutionBackend};
use rmcp::{ServiceExt, transport::stdio};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let backend = Arc::new(DaemonBackend::discover()?);
    let mut server = CommonKitMcp::new(backend);
    if let Ok(context_path) = std::env::var("COMMONKIT_EXECUTION_CONTEXT") {
        let base = std::env::var("COMMONKIT_EXECD_URL").map_err(|_| {
            anyhow::anyhow!("COMMONKIT_EXECD_URL is required with COMMONKIT_EXECUTION_CONTEXT")
        })?;
        let token = std::env::var("COMMONKIT_EXECD_CLIENT_TOKEN").map_err(|_| {
            anyhow::anyhow!(
                "COMMONKIT_EXECD_CLIENT_TOKEN is required with COMMONKIT_EXECUTION_CONTEXT"
            )
        })?;
        let context: ExecutionContext =
            serde_json::from_slice(&tokio::fs::read(context_path).await?)?;
        for manifest in context.tasks.values() {
            manifest.validate()?;
        }
        server =
            server.with_execution(Arc::new(RemoteExecutionBackend::new(base, token)?), context);
    }
    let service = server.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
