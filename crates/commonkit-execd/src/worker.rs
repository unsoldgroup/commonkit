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
    let (task_environment, secret_values) =
        match task_environment(&snapshot.job.manifest.secret_refs, resolved_secrets) {
            Ok(values) => values,
            Err(error) => {
                fail_enforcement(state, &lease, target, "execution_secret_denied")?;
                return Err(error);
            }
        };
    let mut process = match supervisor.spawn_with_environment(
        &snapshot.job.manifest,
        workspace,
        &task_environment,
    ) {
        Ok(process) => process,
        Err(error) => {
            fail_enforcement(state, &lease, target, error.audit_code())?;
            return Err(error.into());
        }
    };
    let _ = running;
    let outcome = loop {
        match process.try_wait() {
            Ok(Some(status)) => break process.finish(status, &secret_values)?,
            Ok(None) => {}
            Err(error @ (SupervisorError::TimedOut | SupervisorError::DiskLimitExceeded)) => {
                fail_enforcement(state, &lease, target, error.audit_code())?;
                return Err(error.into());
            }
            Err(error) => {
                fail_enforcement(state, &lease, target, error.audit_code())?;
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
    Ok(Some(lease.job_id))
}

fn task_environment(
    references: &[String],
    resolved: &BTreeMap<String, String>,
) -> Result<(BTreeMap<String, String>, Vec<String>), WorkerError> {
    let mut environment = BTreeMap::new();
    let mut secret_values = Vec::new();
    for reference in references {
        let name = reference
            .strip_prefix("env://")
            .filter(|name| valid_environment_name(name) && !reserved_environment_name(name))
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

fn valid_environment_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|first| first == '_' || first.is_ascii_alphabetic())
        && chars.all(|character| character == '_' || character.is_ascii_alphanumeric())
}

fn reserved_environment_name(name: &str) -> bool {
    matches!(name, "PATH" | "LANG" | "LC_ALL" | "HOME" | "TMPDIR")
        || name.starts_with("COMMONKIT_EXECD_")
}

fn fail_enforcement(
    state: &ApiState,
    lease: &commonkit_contracts::Lease,
    target: &ExecutionTarget,
    code: &'static str,
) -> Result<(), ExecutionError> {
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
}
