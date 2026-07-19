//! Single-target Linux worker loop. Workspace preparation and secret resolution stay outside durable state.
use crate::ApiState;
use commonkit_contracts::{ExecutionTarget, JobState, StableId};
use commonkit_execution::supervisor::{ProcessSupervisor, SupervisorError};
use commonkit_execution::{ExecutionError, LocalObjectStore};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

pub async fn run_once(
    state: &ApiState,
    target: &ExecutionTarget,
    worker_id: StableId,
    workspace: &Path,
    objects: &LocalObjectStore,
    resolved_secrets: &BTreeMap<String, String>,
    supervisor: &ProcessSupervisor,
) -> Result<Option<String>, WorkerError> {
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
    let running = state.scheduler.lock().unwrap().transition(
        &lease.attempt_id,
        preparing.revision,
        JobState::Running,
        now_ms(),
    )?;
    let mut process = match supervisor.spawn(&snapshot.job.manifest, workspace) {
        Ok(process) => process,
        Err(error) => {
            state
                .scheduler
                .lock()
                .unwrap()
                .complete(&lease, JobState::Failed, None, now_ms())?;
            return Err(error.into());
        }
    };
    let secret_values: Vec<String> = snapshot
        .job
        .manifest
        .secret_refs
        .iter()
        .filter_map(|reference| resolved_secrets.get(reference).cloned())
        .collect();
    let _ = running;
    let outcome = loop {
        match process.try_wait() {
            Ok(Some(status)) => break process.finish(status, &secret_values)?,
            Ok(None) => {}
            Err(error @ (SupervisorError::TimedOut | SupervisorError::DiskLimitExceeded)) => {
                state.scheduler.lock().unwrap().complete(
                    &lease,
                    JobState::Failed,
                    None,
                    now_ms(),
                )?;
                return Err(error.into());
            }
            Err(error) => return Err(error.into()),
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
    Ok(Some(lease.job_id))
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
}
