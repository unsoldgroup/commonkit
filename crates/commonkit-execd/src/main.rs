use clap::Parser;
use commonkit_contracts::{ExecutionManifest, ExecutionTarget, StableId};
use commonkit_execd::github::StatusReporter;
use commonkit_execd::worker::{WorkerContext, run_once};
use commonkit_execd::{ApiState, Capability, ExecutionPolicy, router};
use commonkit_execution::supervisor::{ProcessSupervisor, SupervisorMode};
use commonkit_execution::{LocalObjectStore, Scheduler};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::path::PathBuf;

/// The secret reference the commit-status poster authenticates with.
const GITHUB_STATUS_TOKEN: &str = "env://GITHUB_STATUS_TOKEN";

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExecutionContextFile {
    tasks: BTreeMap<String, ExecutionManifest>,
}

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
    /// Repository-declared task manifests. Enables GitHub commit statuses when the
    /// secret file also resolves `env://GITHUB_STATUS_TOKEN`.
    #[arg(long)]
    tasks: Option<PathBuf>,
    #[arg(long, default_value = "https://api.github.com")]
    github_api: String,
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
    let declared = match &args.tasks {
        Some(path) => {
            let declared: ExecutionContextFile =
                serde_json::from_slice(&tokio::fs::read(path).await?)?;
            for manifest in declared.tasks.values() {
                manifest.validate()?;
            }
            Some(declared.tasks)
        }
        None => None,
    };
    let status = match (&declared, secrets.get(GITHUB_STATUS_TOKEN)) {
        (Some(tasks), Some(token)) => Some(StatusReporter::new(&args.github_api, token, tasks)?),
        (Some(_), None) => {
            anyhow::bail!("--tasks requires {GITHUB_STATUS_TOKEN} in the secret file");
        }
        _ => None,
    };
    // Declared tasks are what a delivery may submit; without them a webhook secret
    // would authenticate a request that could not run anything.
    let state = match (
        std::env::var("COMMONKIT_EXECD_WEBHOOK_SECRET").ok(),
        declared,
    ) {
        (Some(secret), Some(tasks)) => {
            state.with_github_webhook(commonkit_execd::webhook::WebhookConfig::new(secret, tasks))
        }
        (Some(_), None) => {
            anyhow::bail!("COMMONKIT_EXECD_WEBHOOK_SECRET requires --tasks");
        }
        _ => state,
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
                status: status.as_ref(),
            };
            loop {
                if let Err(error) =
                    run_once(&worker_state, &target, worker_id.clone(), &context).await
                {
                    // The top-level message names the stage; only the source chain
                    // says what actually went wrong, and an operator diagnosing a
                    // remote failure has nothing else to read.
                    let mut line = format!("worker iteration failed: {error}");
                    let mut source = std::error::Error::source(&error);
                    while let Some(cause) = source {
                        line.push_str(&format!(": {cause}"));
                        source = cause.source();
                    }
                    eprintln!("{line}");
                }
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
        });
    }
    let listener = tokio::net::TcpListener::bind(args.listen).await?;
    axum::serve(listener, router(state)).await?;
    Ok(())
}
