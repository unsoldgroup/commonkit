//! Single-target Linux worker loop. Workspace preparation and secret resolution stay outside durable state.
use crate::ApiState;
use crate::github::{StatusReporter, StatusState};
use crate::workspace::{self, WorkspaceError};
use commonkit_contracts::{ExecutionTarget, JobState, StableId};
use commonkit_execution::supervisor::{ProcessSupervisor, SupervisorError};
use commonkit_execution::{ExecutionError, LocalObjectStore};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

/// Everything the worker loop reuses across iterations.
pub struct WorkerContext<'a> {
    pub workspace_root: &'a Path,
    pub objects: &'a LocalObjectStore,
    pub resolved_secrets: &'a BTreeMap<String, String>,
    pub supervisor: &'a ProcessSupervisor,
    /// Retain the prepared worktree of a failed job so it can be inspected.
    pub keep_failed_workspaces: bool,
    /// Reports declared CI tasks to GitHub. Absent when no token is configured.
    pub status: Option<&'a StatusReporter>,
}

pub async fn run_once(
    state: &ApiState,
    target: &ExecutionTarget,
    worker_id: StableId,
    context: &WorkerContext<'_>,
) -> Result<Option<String>, WorkerError> {
    let WorkerContext {
        workspace_root,
        objects,
        resolved_secrets,
        supervisor,
        keep_failed_workspaces,
        status,
    } = *context;
    let now = now_ms();
    state
        .scheduler
        .lock()
        .unwrap()
        .register_target(&worker_id, target, now)?;
    let Some(mut lease) = state
        .scheduler
        .lock()
        .unwrap()
        .acquire(worker_id, target, now, 30_000)?
    else {
        return Ok(None);
    };
    let snapshot = state.scheduler.lock().unwrap().snapshot(&lease.job_id)?;
    if state
        .policy
        .as_deref()
        .is_some_and(|policy| policy.validate(&snapshot.job.manifest).is_err())
    {
        let _ = state.scheduler.lock().unwrap().audit(
            now_ms(),
            target.id.as_str(),
            "execute",
            &lease.job_id,
            false,
            "runtime_policy_denied",
        );
        state
            .scheduler
            .lock()
            .unwrap()
            .complete(&lease, JobState::Failed, None, now_ms())?;
        return Err(WorkerError::RuntimePolicyDenied);
    }
    let preparing = state.scheduler.lock().unwrap().transition(
        &lease.attempt_id,
        snapshot.attempt.revision,
        JobState::Preparing,
        now_ms(),
    )?;
    // Preparation fetches from the manifest repository, so it must stay after the
    // policy check above: a denied repository is never contacted.
    let workspace =
        match workspace::prepare(&snapshot.job.manifest, workspace_root, &lease.job_id) {
            Ok(path) => path,
            Err(error) => {
                fail_enforcement(
                    state,
                    &lease,
                    target,
                    "workspace_preparation_failed",
                    status.map(|status| (status, &snapshot.job.manifest)),
                )
                .await?;
                return Err(error.into());
            }
        };
    let mut guard = WorkspaceGuard {
        root: workspace_root,
        repository: snapshot.job.manifest.repository.clone(),
        job_id: lease.job_id.clone(),
        keep_on_failure: keep_failed_workspaces,
        succeeded: false,
    };
    if let Some(status) = status {
        status
            .report(&snapshot.job.manifest, StatusState::Pending)
            .await;
    }
    let running = state.scheduler.lock().unwrap().transition(
        &lease.attempt_id,
        preparing.revision,
        JobState::Running,
        now_ms(),
    )?;
    let (task_environment, secret_values) =
        match task_environment(&snapshot.job.manifest.secret_refs, resolved_secrets) {
            Ok(values) => values,
            Err(error) => {
                fail_enforcement(
                    state,
                    &lease,
                    target,
                    "execution_secret_denied",
                    status.map(|status| (status, &snapshot.job.manifest)),
                )
                .await?;
                return Err(error);
            }
        };
    let mut process = match supervisor.spawn_with_environment(
        &snapshot.job.manifest,
        &workspace,
        &task_environment,
    ) {
        Ok(process) => process,
        Err(error) => {
            fail_enforcement(
                state,
                &lease,
                target,
                error.audit_code(),
                status.map(|status| (status, &snapshot.job.manifest)),
            )
            .await?;
            return Err(error.into());
        }
    };
    let _ = running;
    let outcome = loop {
        match process.try_wait() {
            Ok(Some(status)) => break process.finish(status, &secret_values)?,
            Ok(None) => {}
            Err(error @ (SupervisorError::TimedOut | SupervisorError::DiskLimitExceeded)) => {
                fail_enforcement(
                state,
                &lease,
                target,
                error.audit_code(),
                status.map(|status| (status, &snapshot.job.manifest)),
            )
            .await?;
                return Err(error.into());
            }
            Err(error) => {
                fail_enforcement(
                state,
                &lease,
                target,
                error.audit_code(),
                status.map(|status| (status, &snapshot.job.manifest)),
            )
            .await?;
                return Err(error.into());
            }
        }
        let current = state
            .scheduler
            .lock()
            .unwrap()
            .snapshot(&lease.job_id)?
            .attempt;
        if current.state == JobState::Canceled {
            break process.cancel(
                Duration::from_millis(
                    snapshot
                        .job
                        .manifest
                        .cancel_grace_seconds
                        .saturating_mul(1000),
                ),
                &secret_values,
            )?;
        }
        let now = now_ms();
        if lease.expires_at_unix_ms.saturating_sub(now) < 10_000 {
            lease = state.scheduler.lock().unwrap().renew(&lease, now, 30_000)?;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    for (name, bytes) in [
        ("diagnostics/stdout.log", outcome.stdout.as_bytes()),
        ("diagnostics/stderr.log", outcome.stderr.as_bytes()),
    ] {
        if bytes.is_empty() {
            continue;
        }
        let digest = objects.put(bytes, snapshot.job.manifest.artifacts.max_bytes)?;
        state.scheduler.lock().unwrap().commit_artifact(
            &lease,
            name,
            digest,
            bytes.len() as u64,
            "text/plain",
            now_ms(),
        )?;
    }
    let state_now = state
        .scheduler
        .lock()
        .unwrap()
        .snapshot(&lease.job_id)?
        .attempt
        .state;
    let terminal = if state_now == JobState::Canceled {
        JobState::Canceled
    } else if outcome.status.success() {
        JobState::Succeeded
    } else {
        JobState::Failed
    };
    state
        .scheduler
        .lock()
        .unwrap()
        .complete(&lease, terminal, outcome.status.code(), now_ms())?;
    guard.succeeded = terminal == JobState::Succeeded;
    if let Some(status) = status {
        status
            .report(
                &snapshot.job.manifest,
                match terminal {
                    JobState::Succeeded => StatusState::Success,
                    JobState::Canceled => StatusState::Error,
                    _ => StatusState::Failure,
                },
            )
            .await;
    }
    Ok(Some(lease.job_id))
}

/// Removes a prepared worktree on every exit path. Failed workspaces are retained
/// when the operator asked for them so a failure can be inspected on the target.
struct WorkspaceGuard<'a> {
    root: &'a Path,
    repository: String,
    job_id: String,
    keep_on_failure: bool,
    succeeded: bool,
}

impl Drop for WorkspaceGuard<'_> {
    fn drop(&mut self) {
        if self.keep_on_failure && !self.succeeded {
            return;
        }
        let _ = workspace::remove(self.root, &self.repository, &self.job_id);
    }
}

fn task_environment(
    references: &[String],
    resolved: &BTreeMap<String, String>,
) -> Result<(BTreeMap<String, String>, Vec<String>), WorkerError> {
    let mut environment = BTreeMap::new();
    let mut secret_values = Vec::new();
    for reference in references {
        let name = crate::secrets::environment_name(reference)
            .ok_or_else(|| WorkerError::UnsupportedSecretReference(reference.clone()))?;
        let value = resolved
            .get(reference)
            .cloned()
            .ok_or_else(|| WorkerError::MissingResolvedSecret(reference.clone()))?;
        environment.insert(name.to_owned(), value.clone());
        secret_values.push(value);
    }
    Ok((environment, secret_values))
}

/// Fails the attempt, audits why, and tells GitHub the task could not run.
async fn fail_enforcement(
    state: &ApiState,
    lease: &commonkit_contracts::Lease,
    target: &ExecutionTarget,
    code: &'static str,
    reported: Option<(&StatusReporter, &commonkit_contracts::ExecutionManifest)>,
) -> Result<(), ExecutionError> {
    {
        let mut scheduler = state.scheduler.lock().unwrap();
        scheduler.audit(
            now_ms(),
            target.id.as_str(),
            "execute",
            &lease.job_id,
            false,
            code,
        )?;
        scheduler.complete(lease, JobState::Failed, None, now_ms())?;
    }
    if let Some((status, manifest)) = reported {
        status.report(manifest, StatusState::Error).await;
    }
    Ok(())
}
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
#[derive(Debug, thiserror::Error)]
pub enum WorkerError {
    #[error("execution failed")]
    Execution(#[from] ExecutionError),
    #[error("supervisor failed")]
    Supervisor(#[from] SupervisorError),
    #[error("current execution policy denied the leased manifest")]
    RuntimePolicyDenied,
    #[error("execution secret reference is unsupported: {0}")]
    UnsupportedSecretReference(String),
    #[error("execution secret was not resolved: {0}")]
    MissingResolvedSecret(String),
    #[error("workspace preparation failed")]
    Workspace(#[from] WorkspaceError),
}
