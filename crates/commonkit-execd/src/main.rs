use clap::Parser;
use commonkit_contracts::{ExecutionTarget, StableId};
use commonkit_execd::worker::{WorkerContext, run_once};
use commonkit_execd::{ApiState, Capability, ExecutionPolicy, router};
use commonkit_execution::supervisor::{ProcessSupervisor, SupervisorMode};
use commonkit_execution::{LocalObjectStore, Scheduler};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::path::PathBuf;

#[derive(Parser)]
struct Args {
    #[arg(long, default_value = "127.0.0.1:7341")]
    listen: SocketAddr,
    #[arg(long, default_value = "commonkit-execution.db")]
    database: String,
    #[arg(long, default_value = "worker-vps")]
    worker_id: String,
    #[arg(long)]
    behind_tls_proxy: bool,
    #[arg(long)]
    target: Option<PathBuf>,
    #[arg(long, default_value = "workspaces")]
    workspace_root: PathBuf,
    #[arg(long, default_value = "objects")]
    object_root: PathBuf,
    #[arg(long, default_value = "diagnostics")]
    diagnostic_root: PathBuf,
    #[arg(long, default_value = "execution-policy.json")]
    policy: PathBuf,
    /// Target-local secret values keyed by `env://NAME`, rendered by the operator.
    #[arg(long)]
    secrets: Option<PathBuf>,
    /// Retain the prepared worktree of a failed job so it can be inspected on the target.
    #[arg(long)]
    keep_failed_workspaces: bool,
}
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    if !args.listen.ip().is_loopback() && !args.behind_tls_proxy {
        anyhow::bail!("non-loopback execution API requires --behind-tls-proxy");
    }
    let client_token = std::env::var("COMMONKIT_EXECD_CLIENT_TOKEN")
        .map_err(|_| anyhow::anyhow!("COMMONKIT_EXECD_CLIENT_TOKEN is required"))?;
    let worker_token = std::env::var("COMMONKIT_EXECD_WORKER_TOKEN")
        .map_err(|_| anyhow::anyhow!("COMMONKIT_EXECD_WORKER_TOKEN is required"))?;
    let artifact_signing_key = std::env::var("COMMONKIT_EXECD_ARTIFACT_SIGNING_KEY")
        .map_err(|_| anyhow::anyhow!("COMMONKIT_EXECD_ARTIFACT_SIGNING_KEY is required"))?;
    let object_encryption_key = std::env::var("COMMONKIT_EXECD_OBJECT_ENCRYPTION_KEY")
        .map_err(|_| anyhow::anyhow!("COMMONKIT_EXECD_OBJECT_ENCRYPTION_KEY is required"))?;
    let worker_id = StableId::parse(args.worker_id.clone())?;
    let objects = LocalObjectStore::open_encrypted(
        &args.object_root,
        Sha256::digest(object_encryption_key.as_bytes()).into(),
    )?;
    let policy: ExecutionPolicy = serde_json::from_slice(&tokio::fs::read(&args.policy).await?)?;
    let mut scheduler = Scheduler::open(&args.database)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    scheduler.recover_expired(now)?;
    let state = ApiState::new(
        scheduler,
        vec![
            (
                client_token,
                "client".into(),
                BTreeSet::from([Capability::Submit, Capability::Read, Capability::Cancel]),
            ),
            (
                worker_token,
                args.worker_id,
                BTreeSet::from([Capability::Worker]),
            ),
        ],
        !args.listen.ip().is_loopback(),
    )
    .with_object_store(objects.clone(), artifact_signing_key)
    .with_policy(policy);
    let secrets = match &args.secrets {
        Some(path) => commonkit_execd::secrets::load(path)?,
        None => BTreeMap::new(),
    };
    if let Some(target_path) = args.target {
        let mut target: ExecutionTarget =
            serde_json::from_slice(&tokio::fs::read(target_path).await?)?;
        if target.id != worker_id {
            anyhow::bail!("worker ID must match target ID");
        }
        // Placement trusts what this target can actually resolve, not what the
        // target file claims.
        target.ready_secret_refs = secrets.keys().cloned().collect();
        let worker_state = state.clone();
        let workspace = args.workspace_root.clone();
        let keep_failed_workspaces = args.keep_failed_workspaces;
        let supervisor =
            ProcessSupervisor::new(SupervisorMode::SystemdScope, args.diagnostic_root)?;
        tokio::spawn(async move {
            let context = WorkerContext {
                workspace_root: &workspace,
                objects: &objects,
                resolved_secrets: &secrets,
                supervisor: &supervisor,
                keep_failed_workspaces,
            };
            loop {
                if let Err(error) =
                    run_once(&worker_state, &target, worker_id.clone(), &context).await
                {
                    eprintln!("worker iteration failed: {error}");
                }
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
        });
    }
    let listener = tokio::net::TcpListener::bind(args.listen).await?;
    axum::serve(listener, router(state)).await?;
    Ok(())
}
