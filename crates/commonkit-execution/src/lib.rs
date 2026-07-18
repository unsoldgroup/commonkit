//! Durable execution domain, SQLite scheduler, placement, and artifact storage.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use chacha20poly1305::{ChaCha20Poly1305, KeyInit, Nonce, aead::Aead};
use commonkit_contracts::{
    Artifact, Attempt, Checkpoint, ExecutionManifest, ExecutionReceipt, ExecutionTarget, Job,
    JobEvent, JobState, Lease, PlacementExplanation, ResourceRequirements, SchemaVersion,
    Sha256Digest, StableId,
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

pub mod browser;
pub mod implementor;
pub mod supervisor;

pub const EVENT_RETENTION_FLOOR: u64 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobSnapshot {
    pub job: Job,
    pub attempt: Attempt,
    pub receipt: Option<ExecutionReceipt>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubmitOutcome {
    Created(String),
    Existing(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditEntry {
    pub sequence: u64,
    pub at_unix_ms: u64,
    pub identity: String,
    pub action: String,
    pub resource: String,
    pub allowed: bool,
    pub reason: String,
}

pub struct Scheduler {
    connection: Connection,
}

impl Scheduler {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, ExecutionError> {
        let connection = Connection::open(path)?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        connection.execute_batch(SCHEMA)?;
        Ok(Self { connection })
    }

    pub fn open_memory() -> Result<Self, ExecutionError> {
        Self::open(":memory:")
    }

    pub fn backup(&self, path: impl AsRef<Path>) -> Result<(), ExecutionError> {
        self.connection
            .backup(rusqlite::DatabaseName::Main, path, None)?;
        Ok(())
    }

    pub fn restore(&mut self, path: impl AsRef<Path>) -> Result<(), ExecutionError> {
        self.connection.restore(
            rusqlite::DatabaseName::Main,
            path,
            None::<fn(rusqlite::backup::Progress)>,
        )?;
        Ok(())
    }

    pub fn submit(
        &mut self,
        manifest: &ExecutionManifest,
        idempotency_key: &str,
        now: u64,
    ) -> Result<SubmitOutcome, ExecutionError> {
        manifest.validate()?;
        if idempotency_key.is_empty() || idempotency_key.len() > 200 {
            return Err(ExecutionError::InvalidIdempotencyKey);
        }
        let manifest_digest = manifest.digest()?;
        let manifest_json = serde_json::to_string(manifest)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some((job_id, existing_digest)) = transaction
            .query_row(
                "SELECT job_id, manifest_digest FROM idempotency_keys WHERE idempotency_key=?1",
                [idempotency_key],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?
        {
            if existing_digest == manifest_digest.as_str() {
                transaction.commit()?;
                return Ok(SubmitOutcome::Existing(job_id));
            }
            return Err(ExecutionError::IdempotencyConflict);
        }
        let job_id = format!("job_{}", Uuid::new_v4().simple());
        let attempt_id = format!("attempt_{}", Uuid::new_v4().simple());
        transaction.execute(
            "INSERT INTO jobs(id,manifest_digest,manifest_json,created_at_ms) VALUES(?1,?2,?3,?4)",
            params![job_id, manifest_digest.as_str(), manifest_json, now],
        )?;
        transaction.execute(
            "INSERT INTO attempts(id,job_id,number,revision,state) VALUES(?1,?2,1,0,'queued')",
            params![attempt_id, job_id],
        )?;
        transaction.execute(
            "INSERT INTO idempotency_keys(idempotency_key,manifest_digest,job_id) VALUES(?1,?2,?3)",
            params![idempotency_key, manifest_digest.as_str(), job_id],
        )?;
        append_event(
            &transaction,
            &job_id,
            Some(&attempt_id),
            "job_queued",
            now,
            json!({"manifestDigest": manifest_digest}),
        )?;
        transaction.commit()?;
        Ok(SubmitOutcome::Created(job_id))
    }

    pub fn snapshot(&self, job_id: &str) -> Result<JobSnapshot, ExecutionError> {
        let job = self
            .connection
            .query_row(
                "SELECT id,manifest_digest,manifest_json,created_at_ms FROM jobs WHERE id=?1",
                [job_id],
                row_job,
            )
            .optional()?
            .ok_or(ExecutionError::NotFound)?;
        let attempt = self.connection.query_row("SELECT id,job_id,number,revision,state,target_id,engine_run_ref,fencing_token FROM attempts WHERE job_id=?1 ORDER BY number DESC LIMIT 1", [job_id], row_attempt)?;
        let receipt = self
            .connection
            .query_row(
                "SELECT receipt_json FROM receipts WHERE attempt_id=?1",
                [&attempt.id],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map(|value| serde_json::from_str(&value))
            .transpose()?;
        Ok(JobSnapshot {
            job,
            attempt,
            receipt,
        })
    }

    pub fn events(
        &self,
        job_id: &str,
        after: u64,
        limit: u32,
    ) -> Result<Vec<JobEvent>, ExecutionError> {
        let minimum = self.connection.query_row(
            "SELECT COALESCE(MIN(sequence),1) FROM events WHERE job_id=?1",
            [job_id],
            |row| row.get::<_, u64>(0),
        )?;
        if after > 0 && after + 1 < minimum {
            return Err(ExecutionError::CursorExpired { minimum });
        }
        let mut statement = self.connection.prepare("SELECT id,job_id,sequence,attempt_id,kind,at_ms,data_json FROM events WHERE job_id=?1 AND sequence>?2 ORDER BY sequence LIMIT ?3")?;
        let rows = statement.query_map(params![job_id, after, limit.clamp(1, 1000)], |row| {
            Ok(JobEvent {
                id: row.get(0)?,
                job_id: row.get(1)?,
                sequence: row.get(2)?,
                attempt_id: row.get(3)?,
                kind: row.get(4)?,
                at_unix_ms: row.get(5)?,
                data: serde_json::from_str::<Value>(&row.get::<_, String>(6)?)
                    .unwrap_or(Value::Null),
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn prune_events_before(
        &mut self,
        job_id: &str,
        minimum_sequence: u64,
    ) -> Result<usize, ExecutionError> {
        Ok(self.connection.execute(
            "DELETE FROM events WHERE job_id=?1 AND sequence<?2",
            params![job_id, minimum_sequence],
        )?)
    }

    pub fn audit(
        &mut self,
        at: u64,
        identity: &str,
        action: &str,
        resource: &str,
        allowed: bool,
        reason: &str,
    ) -> Result<(), ExecutionError> {
        for value in [identity, action, resource, reason] {
            commonkit_contracts::assert_no_embedded_secrets(&Value::String(value.into()))?;
        }
        self.connection.execute("INSERT INTO audit(at_ms,identity,action,resource,allowed,reason) VALUES(?1,?2,?3,?4,?5,?6)", params![at,identity,action,resource,allowed,reason])?;
        Ok(())
    }

    pub fn audit_entries(&self) -> Result<Vec<AuditEntry>, ExecutionError> {
        let mut statement = self.connection.prepare(
            "SELECT id,at_ms,identity,action,resource,allowed,reason FROM audit ORDER BY id",
        )?;
        Ok(statement
            .query_map([], |row| {
                Ok(AuditEntry {
                    sequence: row.get(0)?,
                    at_unix_ms: row.get(1)?,
                    identity: row.get(2)?,
                    action: row.get(3)?,
                    resource: row.get(4)?,
                    allowed: row.get(5)?,
                    reason: row.get(6)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?)
    }

    pub fn acquire(
        &mut self,
        worker_id: StableId,
        target: &ExecutionTarget,
        now: u64,
        ttl_ms: u64,
    ) -> Result<Option<Lease>, ExecutionError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let candidate = {
            let mut statement = transaction.prepare("SELECT a.id,a.job_id,j.manifest_json FROM attempts a JOIN jobs j ON j.id=a.job_id WHERE a.state='queued' ORDER BY j.created_at_ms LIMIT 100")?;
            let mut rows = statement.query([])?;
            let mut found = None;
            while let Some(row) = rows.next()? {
                let manifest: ExecutionManifest = serde_json::from_str(&row.get::<_, String>(2)?)?;
                if placement(&manifest, std::slice::from_ref(target))
                    .selected
                    .is_some()
                {
                    found = Some((row.get::<_, String>(0)?, row.get::<_, String>(1)?));
                    break;
                }
            }
            found
        };
        let Some((attempt_id, job_id)) = candidate else {
            transaction.commit()?;
            return Ok(None);
        };
        let token: u64 = transaction.query_row(
            "SELECT COALESCE(MAX(fencing_token),0)+1 FROM attempts WHERE job_id=?1",
            [&job_id],
            |row| row.get(0),
        )?;
        let changed = transaction.execute("UPDATE attempts SET state='assigned',revision=revision+1,target_id=?1,fencing_token=?2 WHERE id=?3 AND state='queued'", params![target.id.as_str(), token, attempt_id])?;
        if changed != 1 {
            return Err(ExecutionError::RevisionConflict);
        }
        transaction.execute("INSERT OR REPLACE INTO leases(job_id,attempt_id,worker_id,fencing_token,expires_at_ms) VALUES(?1,?2,?3,?4,?5)", params![job_id, attempt_id, worker_id.as_str(), token, now.saturating_add(ttl_ms)])?;
        append_event(
            &transaction,
            &job_id,
            Some(&attempt_id),
            "attempt_assigned",
            now,
            json!({"targetId": target.id, "fencingToken": token}),
        )?;
        transaction.commit()?;
        Ok(Some(Lease {
            job_id,
            attempt_id,
            worker_id,
            fencing_token: token,
            expires_at_unix_ms: now.saturating_add(ttl_ms),
        }))
    }

    pub fn renew(&mut self, lease: &Lease, now: u64, ttl_ms: u64) -> Result<Lease, ExecutionError> {
        let expires = now.saturating_add(ttl_ms);
        let changed = self.connection.execute("UPDATE leases SET expires_at_ms=?1 WHERE job_id=?2 AND attempt_id=?3 AND worker_id=?4 AND fencing_token=?5 AND expires_at_ms>=?6", params![expires, lease.job_id, lease.attempt_id, lease.worker_id.as_str(), lease.fencing_token, now])?;
        if changed != 1 {
            return Err(ExecutionError::StaleFence);
        }
        Ok(Lease {
            expires_at_unix_ms: expires,
            ..lease.clone()
        })
    }

    pub fn transition(
        &mut self,
        attempt_id: &str,
        expected_revision: u64,
        state: JobState,
        now: u64,
    ) -> Result<Attempt, ExecutionError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = transaction.query_row("SELECT id,job_id,number,revision,state,target_id,engine_run_ref,fencing_token FROM attempts WHERE id=?1", [attempt_id], row_attempt).optional()?.ok_or(ExecutionError::NotFound)?;
        if current.revision != expected_revision {
            return Err(ExecutionError::RevisionConflict);
        }
        if !valid_transition(current.state, state) {
            return Err(ExecutionError::InvalidTransition {
                from: current.state,
                to: state,
            });
        }
        transaction.execute(
            "UPDATE attempts SET state=?1,revision=revision+1 WHERE id=?2 AND revision=?3",
            params![state_name(state), attempt_id, expected_revision],
        )?;
        append_event(
            &transaction,
            &current.job_id,
            Some(attempt_id),
            "attempt_state_changed",
            now,
            json!({"from": current.state, "to": state, "revision": expected_revision + 1}),
        )?;
        transaction.commit()?;
        self.connection.query_row("SELECT id,job_id,number,revision,state,target_id,engine_run_ref,fencing_token FROM attempts WHERE id=?1", [attempt_id], row_attempt).map_err(Into::into)
    }

    pub fn correlate_engine_run(
        &mut self,
        attempt_id: &str,
        fencing_token: u64,
        engine_run_ref: &str,
        now: u64,
    ) -> Result<(), ExecutionError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (job_id, existing): (String, Option<String>) = transaction
            .query_row(
                "SELECT job_id,engine_run_ref FROM attempts WHERE id=?1 AND fencing_token=?2",
                params![attempt_id, fencing_token],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?
            .ok_or(ExecutionError::StaleFence)?;
        if let Some(existing) = existing {
            if existing != engine_run_ref {
                return Err(ExecutionError::DuplicateImplementorRun);
            }
            transaction.commit()?;
            return Ok(());
        }
        transaction.execute("UPDATE attempts SET engine_run_ref=?1,revision=revision+1 WHERE id=?2 AND fencing_token=?3", params![engine_run_ref, attempt_id, fencing_token])?;
        append_event(
            &transaction,
            &job_id,
            Some(attempt_id),
            "implementor_correlated",
            now,
            json!({"engineRunRef": engine_run_ref}),
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn checkpoint(
        &mut self,
        lease: &Lease,
        stage: &str,
        digest: Sha256Digest,
        now: u64,
    ) -> Result<Checkpoint, ExecutionError> {
        self.assert_fence(lease, now)?;
        let id = format!("checkpoint_{}", Uuid::new_v4().simple());
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute("INSERT OR IGNORE INTO checkpoints(id,job_id,attempt_id,stage,fencing_token,object_digest,committed_at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7)", params![id, lease.job_id, lease.attempt_id, stage, lease.fencing_token, digest.as_str(), now])?;
        let checkpoint = transaction.query_row("SELECT id,job_id,attempt_id,stage,fencing_token,object_digest,committed_at_ms FROM checkpoints WHERE job_id=?1 AND stage=?2", params![lease.job_id, stage], |row| Ok(Checkpoint { id: row.get(0)?, job_id: row.get(1)?, attempt_id: row.get(2)?, stage: row.get(3)?, fencing_token: row.get(4)?, object_digest: parse_digest(row.get(5)?)?, committed_at_unix_ms: row.get(6)? }))?;
        append_event(
            &transaction,
            &lease.job_id,
            Some(&lease.attempt_id),
            "checkpoint_committed",
            now,
            json!({"checkpointId": checkpoint.id, "stage": stage}),
        )?;
        transaction.commit()?;
        Ok(checkpoint)
    }

    pub fn commit_artifact(
        &mut self,
        lease: &Lease,
        name: &str,
        digest: Sha256Digest,
        size: u64,
        media_type: &str,
        now: u64,
    ) -> Result<Artifact, ExecutionError> {
        self.assert_fence(lease, now)?;
        safe_artifact_name(name)?;
        let id = format!("artifact_{}", Uuid::new_v4().simple());
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute("INSERT OR IGNORE INTO artifacts(id,job_id,attempt_id,name,object_digest,size_bytes,media_type,committed_at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)", params![id, lease.job_id, lease.attempt_id, name, digest.as_str(), size, media_type, now])?;
        let artifact = transaction.query_row("SELECT id,job_id,attempt_id,name,object_digest,size_bytes,media_type,committed_at_ms FROM artifacts WHERE job_id=?1 AND name=?2", params![lease.job_id, name], |row| Ok(Artifact { id: row.get(0)?, job_id: row.get(1)?, attempt_id: row.get(2)?, name: row.get(3)?, object_digest: parse_digest(row.get(4)?)?, size_bytes: row.get(5)?, media_type: row.get(6)?, committed_at_unix_ms: row.get(7)? }))?;
        append_event(
            &transaction,
            &lease.job_id,
            Some(&lease.attempt_id),
            "artifact_committed",
            now,
            json!({"artifactId": artifact.id, "name": name}),
        )?;
        transaction.commit()?;
        Ok(artifact)
    }

    pub fn artifacts(&self, job_id: &str) -> Result<Vec<Artifact>, ExecutionError> {
        let mut statement = self.connection.prepare("SELECT id,job_id,attempt_id,name,object_digest,size_bytes,media_type,committed_at_ms FROM artifacts WHERE job_id=?1 ORDER BY committed_at_ms")?;
        Ok(statement
            .query_map([job_id], row_artifact)?
            .collect::<Result<Vec<_>, _>>()?)
    }

    pub fn artifact(&self, artifact_id: &str) -> Result<Artifact, ExecutionError> {
        self.connection.query_row("SELECT id,job_id,attempt_id,name,object_digest,size_bytes,media_type,committed_at_ms FROM artifacts WHERE id=?1", [artifact_id], row_artifact).optional()?.ok_or(ExecutionError::NotFound)
    }

    pub fn complete(
        &mut self,
        lease: &Lease,
        state: JobState,
        exit_code: Option<i32>,
        now: u64,
    ) -> Result<ExecutionReceipt, ExecutionError> {
        if !state.terminal() {
            return Err(ExecutionError::InvalidTerminalState);
        }
        self.assert_fence(lease, now)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = transaction.execute("UPDATE attempts SET state=?1,revision=revision+1 WHERE id=?2 AND fencing_token=?3 AND state NOT IN ('succeeded','failed','canceled','interrupted')", params![state_name(state), lease.attempt_id, lease.fencing_token])?;
        if changed != 1 {
            let existing: String = transaction
                .query_row(
                    "SELECT state FROM attempts WHERE id=?1 AND fencing_token=?2",
                    params![lease.attempt_id, lease.fencing_token],
                    |row| row.get(0),
                )
                .optional()?
                .ok_or(ExecutionError::StaleFence)?;
            if existing != state_name(state) {
                return Err(ExecutionError::StaleFence);
            }
        }
        let artifact_ids = collect_strings(
            &transaction,
            "SELECT id FROM artifacts WHERE attempt_id=?1 ORDER BY committed_at_ms",
            &lease.attempt_id,
        )?;
        let checkpoint_ids = collect_strings(
            &transaction,
            "SELECT id FROM checkpoints WHERE attempt_id=?1 ORDER BY committed_at_ms",
            &lease.attempt_id,
        )?;
        let receipt = ExecutionReceipt {
            schema_version: SchemaVersion(1),
            job_id: lease.job_id.clone(),
            attempt_id: lease.attempt_id.clone(),
            state,
            exit_code,
            started_at_unix_ms: None,
            finished_at_unix_ms: Some(now),
            artifact_ids,
            checkpoint_ids,
        };
        let receipt_json = serde_json::to_string(&receipt)?;
        transaction.execute("INSERT INTO receipts(attempt_id,receipt_json) VALUES(?1,?2) ON CONFLICT(attempt_id) DO NOTHING", params![lease.attempt_id, receipt_json])?;
        transaction.execute(
            "DELETE FROM leases WHERE attempt_id=?1 AND fencing_token=?2",
            params![lease.attempt_id, lease.fencing_token],
        )?;
        append_event(
            &transaction,
            &lease.job_id,
            Some(&lease.attempt_id),
            "attempt_completed",
            now,
            json!({"state": state, "exitCode": exit_code}),
        )?;
        transaction.commit()?;
        Ok(receipt)
    }

    pub fn cancel(&mut self, job_id: &str, now: u64) -> Result<Attempt, ExecutionError> {
        let attempt = self.snapshot(job_id)?.attempt;
        if attempt.state.terminal() {
            return Ok(attempt);
        }
        self.transition(&attempt.id, attempt.revision, JobState::Canceled, now)
    }

    pub fn recover_expired(&mut self, now: u64) -> Result<Vec<String>, ExecutionError> {
        let expired: Vec<(String, String)> = {
            let mut statement = self
                .connection
                .prepare("SELECT attempt_id,job_id FROM leases WHERE expires_at_ms<?1")?;
            statement
                .query_map([now], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<Result<Vec<_>, _>>()?
        };
        let mut recovered = Vec::new();
        for (attempt_id, job_id) in expired {
            let transaction = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            transaction.execute("UPDATE attempts SET state='interrupted',revision=revision+1 WHERE id=?1 AND state NOT IN ('succeeded','failed','canceled','interrupted')", [&attempt_id])?;
            transaction.execute("DELETE FROM leases WHERE attempt_id=?1", [&attempt_id])?;
            let (number, manifest_json): (u32,String) = transaction.query_row("SELECT a.number,j.manifest_json FROM attempts a JOIN jobs j ON j.id=a.job_id WHERE a.id=?1", [&attempt_id], |row| Ok((row.get(0)?, row.get(1)?)))?;
            let manifest: ExecutionManifest = serde_json::from_str(&manifest_json)?;
            append_event(
                &transaction,
                &job_id,
                Some(&attempt_id),
                "attempt_interrupted",
                now,
                json!({"reason":"lease_expired"}),
            )?;
            if number < manifest.retry.max_attempts.max(1) {
                let next = format!("attempt_{}", Uuid::new_v4().simple());
                transaction.execute("INSERT INTO attempts(id,job_id,number,revision,state) VALUES(?1,?2,?3,0,'queued')", params![next, job_id, number + 1])?;
                append_event(
                    &transaction,
                    &job_id,
                    Some(&next),
                    "attempt_retry_queued",
                    now,
                    json!({"previousAttemptId": attempt_id}),
                )?;
            }
            transaction.commit()?;
            recovered.push(job_id);
        }
        Ok(recovered)
    }

    pub fn retry(&mut self, job_id: &str, now: u64) -> Result<Attempt, ExecutionError> {
        self.queue_followup(job_id, None, "manual_retry", now)
    }
    pub fn resume(
        &mut self,
        job_id: &str,
        checkpoint_stage: &str,
        now: u64,
    ) -> Result<Attempt, ExecutionError> {
        let exists = self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM checkpoints WHERE job_id=?1 AND stage=?2)",
            params![job_id, checkpoint_stage],
            |row| row.get::<_, bool>(0),
        )?;
        if !exists {
            return Err(ExecutionError::CheckpointNotFound);
        }
        self.queue_followup(job_id, Some(checkpoint_stage), "resume", now)
    }
    fn queue_followup(
        &mut self,
        job_id: &str,
        checkpoint: Option<&str>,
        kind: &str,
        now: u64,
    ) -> Result<Attempt, ExecutionError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (number,state,manifest_json):(u32,String,String)=transaction.query_row("SELECT a.number,a.state,j.manifest_json FROM attempts a JOIN jobs j ON j.id=a.job_id WHERE a.job_id=?1 ORDER BY a.number DESC LIMIT 1",[job_id],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional()?.ok_or(ExecutionError::NotFound)?;
        if !parse_state(&state)?.terminal() {
            return Err(ExecutionError::AttemptActive);
        }
        let manifest: ExecutionManifest = serde_json::from_str(&manifest_json)?;
        if number >= manifest.retry.max_attempts.max(1) {
            return Err(ExecutionError::RetryExhausted);
        }
        let attempt_id = format!("attempt_{}", Uuid::new_v4().simple());
        transaction.execute(
            "INSERT INTO attempts(id,job_id,number,revision,state) VALUES(?1,?2,?3,0,'queued')",
            params![attempt_id, job_id, number + 1],
        )?;
        append_event(
            &transaction,
            job_id,
            Some(&attempt_id),
            "attempt_followup_queued",
            now,
            json!({"kind":kind,"checkpoint":checkpoint}),
        )?;
        transaction.commit()?;
        self.connection.query_row("SELECT id,job_id,number,revision,state,target_id,engine_run_ref,fencing_token FROM attempts WHERE id=?1",[attempt_id],row_attempt).map_err(Into::into)
    }

    pub fn register_target(
        &mut self,
        worker_id: &StableId,
        target: &ExecutionTarget,
        now: u64,
    ) -> Result<(), ExecutionError> {
        if worker_id != &target.id {
            return Err(ExecutionError::TargetIdentityMismatch);
        }
        let value = serde_json::to_string(target)?;
        self.connection.execute("INSERT INTO targets(id,worker_id,target_json,last_heartbeat_ms) VALUES(?1,?2,?3,?4) ON CONFLICT(id) DO UPDATE SET worker_id=excluded.worker_id,target_json=excluded.target_json,last_heartbeat_ms=excluded.last_heartbeat_ms",params![target.id.as_str(),worker_id.as_str(),value,now])?;
        Ok(())
    }
    pub fn targets(
        &self,
        now: u64,
        stale_after_ms: u64,
    ) -> Result<Vec<ExecutionTarget>, ExecutionError> {
        let mut statement = self
            .connection
            .prepare("SELECT target_json,last_heartbeat_ms FROM targets ORDER BY id")?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, u64>(1)?))
        })?;
        let mut targets = Vec::new();
        for row in rows {
            let (value, heartbeat) = row?;
            let mut target: ExecutionTarget = serde_json::from_str(&value)?;
            if now.saturating_sub(heartbeat) > stale_after_ms {
                target.healthy = false
            }
            targets.push(target)
        }
        Ok(targets)
    }

    fn assert_fence(&self, lease: &Lease, now: u64) -> Result<(), ExecutionError> {
        let valid = self.connection.query_row("SELECT EXISTS(SELECT 1 FROM leases WHERE job_id=?1 AND attempt_id=?2 AND worker_id=?3 AND fencing_token=?4 AND expires_at_ms>=?5)", params![lease.job_id, lease.attempt_id, lease.worker_id.as_str(), lease.fencing_token, now], |row| row.get::<_, bool>(0))?;
        if valid {
            Ok(())
        } else {
            Err(ExecutionError::StaleFence)
        }
    }
}

pub struct PlacementResult {
    pub selected: Option<StableId>,
    pub explanations: Vec<PlacementExplanation>,
}

pub fn placement(manifest: &ExecutionManifest, targets: &[ExecutionTarget]) -> PlacementResult {
    let mut explanations = Vec::new();
    for target in targets {
        let mut reasons = Vec::new();
        if !target.healthy {
            reasons.push("target_unhealthy".into());
        }
        if target.draining {
            reasons.push("target_draining".into());
        }
        if target.loadout_digest != manifest.loadout_digest {
            reasons.push("loadout_digest_mismatch".into());
        }
        if target.execution_profile_digest != manifest.execution_profile_digest {
            reasons.push("execution_profile_digest_mismatch".into());
        }
        if !manifest
            .required_capabilities
            .is_subset(&target.capabilities)
        {
            reasons.push("capability_missing".into());
        }
        if !manifest
            .secret_refs
            .iter()
            .all(|reference| target.ready_secret_refs.contains(reference))
        {
            reasons.push("secret_not_ready".into());
        }
        if !fits(&manifest.resources, &target.free) {
            reasons.push("insufficient_reserved_headroom".into());
        }
        if manifest.browser.is_some()
            && target.free.memory_mib < manifest.resources.memory_mib.saturating_mul(2)
        {
            reasons.push("browser_memory_overcommit".into());
        }
        if let Some(class) = manifest
            .browser
            .as_ref()
            .and_then(|browser| browser.performance_class.as_ref())
        {
            if !target.capabilities.contains(class) {
                reasons.push("performance_class_mismatch".into());
            }
        }
        let score = reasons.is_empty().then(|| {
            target.free.memory_mib as i64 + target.free.disk_mib as i64
                - i64::from(target.queue_depth) * 100
                - i64::from(target.cost_score)
        });
        explanations.push(PlacementExplanation {
            target_id: target.id.clone(),
            eligible: reasons.is_empty(),
            reasons,
            score,
        });
    }
    explanations.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.target_id.cmp(&b.target_id))
    });
    let selected = explanations
        .iter()
        .find(|entry| entry.eligible)
        .map(|entry| entry.target_id.clone());
    PlacementResult {
        selected,
        explanations,
    }
}

#[derive(Clone)]
pub struct LocalObjectStore {
    root: PathBuf,
    encryption_key: Option<[u8; 32]>,
}
impl LocalObjectStore {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, ExecutionError> {
        fs::create_dir_all(root.as_ref())?;
        Ok(Self {
            root: root.as_ref().to_path_buf(),
            encryption_key: None,
        })
    }
    pub fn open_encrypted(root: impl AsRef<Path>, key: [u8; 32]) -> Result<Self, ExecutionError> {
        fs::create_dir_all(root.as_ref())?;
        Ok(Self {
            root: root.as_ref().to_path_buf(),
            encryption_key: Some(key),
        })
    }
    pub fn put(&self, bytes: &[u8], max_bytes: u64) -> Result<Sha256Digest, ExecutionError> {
        if bytes.len() as u64 > max_bytes {
            return Err(ExecutionError::ArtifactTooLarge);
        }
        let digest = Sha256::digest(bytes);
        let digest = Sha256Digest::parse(format!("sha256:{digest:x}"))?;
        let target = self
            .root
            .join(digest.as_str().trim_start_matches("sha256:"));
        if !target.exists() {
            let temporary = self.root.join(format!(".{}.tmp", Uuid::new_v4()));
            let stored = if let Some(key) = self.encryption_key {
                let nonce: [u8; 12] = rand::random();
                let cipher = ChaCha20Poly1305::new((&key).into());
                let encrypted = cipher
                    .encrypt(Nonce::from_slice(&nonce), bytes)
                    .map_err(|_| ExecutionError::ObjectEncryption)?;
                [nonce.to_vec(), encrypted].concat()
            } else {
                bytes.to_vec()
            };
            fs::write(&temporary, stored)?;
            fs::rename(&temporary, &target)?;
        }
        Ok(digest)
    }
    pub fn get(&self, digest: &Sha256Digest) -> Result<Vec<u8>, ExecutionError> {
        let stored = fs::read(
            self.root
                .join(digest.as_str().trim_start_matches("sha256:")),
        )?;
        if let Some(key) = self.encryption_key {
            if stored.len() < 12 {
                return Err(ExecutionError::ObjectEncryption);
            }
            let (nonce, ciphertext) = stored.split_at(12);
            ChaCha20Poly1305::new((&key).into())
                .decrypt(Nonce::from_slice(nonce), ciphertext)
                .map_err(|_| ExecutionError::ObjectEncryption)
        } else {
            Ok(stored)
        }
    }
    pub fn gc(&self, referenced: &BTreeSet<Sha256Digest>) -> Result<usize, ExecutionError> {
        let mut removed = 0;
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.')
                || referenced
                    .iter()
                    .any(|digest| digest.as_str().ends_with(&name))
            {
                continue;
            }
            if entry.file_type()?.is_file() {
                fs::remove_file(entry.path())?;
                removed += 1;
            }
        }
        Ok(removed)
    }
}

fn append_event(
    transaction: &rusqlite::Transaction<'_>,
    job_id: &str,
    attempt_id: Option<&str>,
    kind: &str,
    now: u64,
    data: Value,
) -> Result<(), ExecutionError> {
    commonkit_contracts::assert_no_embedded_secrets(&data)?;
    let sequence: u64 = transaction.query_row(
        "SELECT COALESCE(MAX(sequence),0)+1 FROM events WHERE job_id=?1",
        [job_id],
        |row| row.get(0),
    )?;
    transaction.execute("INSERT INTO events(id,job_id,sequence,attempt_id,kind,at_ms,data_json) VALUES(?1,?2,?3,?4,?5,?6,?7)", params![format!("event_{}", Uuid::new_v4().simple()), job_id, sequence, attempt_id, kind, now, serde_json::to_string(&data)?])?;
    Ok(())
}
fn row_job(row: &rusqlite::Row<'_>) -> rusqlite::Result<Job> {
    let manifest: String = row.get(2)?;
    Ok(Job {
        id: row.get(0)?,
        manifest_digest: parse_digest(row.get(1)?)?,
        manifest: serde_json::from_str(&manifest).map_err(json_sql_error)?,
        created_at_unix_ms: row.get(3)?,
    })
}
fn row_attempt(row: &rusqlite::Row<'_>) -> rusqlite::Result<Attempt> {
    let state: String = row.get(4)?;
    Ok(Attempt {
        id: row.get(0)?,
        job_id: row.get(1)?,
        number: row.get(2)?,
        revision: row.get(3)?,
        state: parse_state(&state)?,
        target_id: row
            .get::<_, Option<String>>(5)?
            .map(StableId::parse)
            .transpose()
            .map_err(contract_sql_error)?,
        engine_run_ref: row.get(6)?,
        fencing_token: row.get(7)?,
    })
}
fn row_artifact(row: &rusqlite::Row<'_>) -> rusqlite::Result<Artifact> {
    Ok(Artifact {
        id: row.get(0)?,
        job_id: row.get(1)?,
        attempt_id: row.get(2)?,
        name: row.get(3)?,
        object_digest: parse_digest(row.get(4)?)?,
        size_bytes: row.get(5)?,
        media_type: row.get(6)?,
        committed_at_unix_ms: row.get(7)?,
    })
}
fn parse_digest(value: String) -> rusqlite::Result<Sha256Digest> {
    Sha256Digest::parse(value).map_err(contract_sql_error)
}
fn contract_sql_error(error: commonkit_contracts::ContractError) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
}
fn json_sql_error(error: serde_json::Error) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
}
fn parse_state(value: &str) -> rusqlite::Result<JobState> {
    match value {
        "queued" => Ok(JobState::Queued),
        "assigned" => Ok(JobState::Assigned),
        "preparing" => Ok(JobState::Preparing),
        "running" => Ok(JobState::Running),
        "checkpointing" => Ok(JobState::Checkpointing),
        "succeeded" => Ok(JobState::Succeeded),
        "failed" => Ok(JobState::Failed),
        "canceled" => Ok(JobState::Canceled),
        "interrupted" => Ok(JobState::Interrupted),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}
fn state_name(state: JobState) -> &'static str {
    match state {
        JobState::Queued => "queued",
        JobState::Assigned => "assigned",
        JobState::Preparing => "preparing",
        JobState::Running => "running",
        JobState::Checkpointing => "checkpointing",
        JobState::Succeeded => "succeeded",
        JobState::Failed => "failed",
        JobState::Canceled => "canceled",
        JobState::Interrupted => "interrupted",
    }
}
fn valid_transition(from: JobState, to: JobState) -> bool {
    matches!(
        (from, to),
        (JobState::Queued, JobState::Assigned)
            | (JobState::Assigned, JobState::Preparing)
            | (JobState::Preparing, JobState::Running)
            | (JobState::Running, JobState::Checkpointing)
            | (JobState::Checkpointing, JobState::Running)
            | (
                JobState::Running,
                JobState::Succeeded | JobState::Failed | JobState::Canceled | JobState::Interrupted
            )
            | (
                JobState::Preparing,
                JobState::Failed | JobState::Canceled | JobState::Interrupted
            )
            | (
                JobState::Assigned,
                JobState::Canceled | JobState::Interrupted
            )
            | (JobState::Queued, JobState::Canceled)
    )
}
fn fits(required: &ResourceRequirements, free: &ResourceRequirements) -> bool {
    required.cpu_millis <= free.cpu_millis
        && required.memory_mib <= free.memory_mib
        && required.disk_mib <= free.disk_mib
}
fn safe_artifact_name(name: &str) -> Result<(), ExecutionError> {
    if name.is_empty()
        || name.starts_with('/')
        || name.split('/').any(|part| part == "..")
        || name.contains('\0')
    {
        Err(ExecutionError::UnsafeArtifactPath)
    } else {
        Ok(())
    }
}
fn collect_strings(
    tx: &rusqlite::Transaction<'_>,
    sql: &str,
    id: &str,
) -> Result<Vec<String>, ExecutionError> {
    let mut statement = tx.prepare(sql)?;
    Ok(statement
        .query_map([id], |row| row.get(0))?
        .collect::<Result<Vec<_>, _>>()?)
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS jobs(id TEXT PRIMARY KEY,manifest_digest TEXT NOT NULL,manifest_json TEXT NOT NULL,created_at_ms INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS idempotency_keys(idempotency_key TEXT PRIMARY KEY,manifest_digest TEXT NOT NULL,job_id TEXT NOT NULL REFERENCES jobs(id));
CREATE TABLE IF NOT EXISTS attempts(id TEXT PRIMARY KEY,job_id TEXT NOT NULL REFERENCES jobs(id),number INTEGER NOT NULL,revision INTEGER NOT NULL,state TEXT NOT NULL,target_id TEXT,engine_run_ref TEXT,fencing_token INTEGER,UNIQUE(job_id,number));
CREATE TABLE IF NOT EXISTS events(id TEXT PRIMARY KEY,job_id TEXT NOT NULL REFERENCES jobs(id),sequence INTEGER NOT NULL,attempt_id TEXT,kind TEXT NOT NULL,at_ms INTEGER NOT NULL,data_json TEXT NOT NULL,UNIQUE(job_id,sequence));
CREATE TABLE IF NOT EXISTS leases(job_id TEXT PRIMARY KEY,attempt_id TEXT UNIQUE NOT NULL,worker_id TEXT NOT NULL,fencing_token INTEGER NOT NULL,expires_at_ms INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS checkpoints(id TEXT PRIMARY KEY,job_id TEXT NOT NULL,attempt_id TEXT NOT NULL,stage TEXT NOT NULL,fencing_token INTEGER NOT NULL,object_digest TEXT NOT NULL,committed_at_ms INTEGER NOT NULL,UNIQUE(job_id,stage));
CREATE TABLE IF NOT EXISTS artifacts(id TEXT PRIMARY KEY,job_id TEXT NOT NULL,attempt_id TEXT NOT NULL,name TEXT NOT NULL,object_digest TEXT NOT NULL,size_bytes INTEGER NOT NULL,media_type TEXT NOT NULL,committed_at_ms INTEGER NOT NULL,UNIQUE(job_id,name));
CREATE TABLE IF NOT EXISTS receipts(attempt_id TEXT PRIMARY KEY,receipt_json TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS audit(id INTEGER PRIMARY KEY AUTOINCREMENT,at_ms INTEGER NOT NULL,identity TEXT NOT NULL,action TEXT NOT NULL,resource TEXT NOT NULL,allowed INTEGER NOT NULL,reason TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS targets(id TEXT PRIMARY KEY,worker_id TEXT NOT NULL,target_json TEXT NOT NULL,last_heartbeat_ms INTEGER NOT NULL);
PRAGMA user_version=1;
"#;

#[derive(Debug, Error)]
pub enum ExecutionError {
    #[error("SQLite error")]
    Sql(#[from] rusqlite::Error),
    #[error("contract error")]
    Contract(#[from] commonkit_contracts::ContractError),
    #[error("JSON error")]
    Json(#[from] serde_json::Error),
    #[error("I/O error")]
    Io(#[from] std::io::Error),
    #[error("job not found")]
    NotFound,
    #[error("idempotency key conflicts with a different manifest")]
    IdempotencyConflict,
    #[error("invalid idempotency key")]
    InvalidIdempotencyKey,
    #[error("stale or expired fencing token")]
    StaleFence,
    #[error("revision conflict")]
    RevisionConflict,
    #[error("invalid transition {from:?} -> {to:?}")]
    InvalidTransition { from: JobState, to: JobState },
    #[error("invalid terminal state")]
    InvalidTerminalState,
    #[error("duplicate implementor run correlation")]
    DuplicateImplementorRun,
    #[error("event cursor expired; minimum is {minimum}")]
    CursorExpired { minimum: u64 },
    #[error("unsafe artifact path")]
    UnsafeArtifactPath,
    #[error("artifact exceeds configured size limit")]
    ArtifactTooLarge,
    #[error("object encryption or authentication failed")]
    ObjectEncryption,
    #[error("attempt is still active")]
    AttemptActive,
    #[error("retry policy is exhausted")]
    RetryExhausted,
    #[error("checkpoint was not found")]
    CheckpointNotFound,
    #[error("target identity does not match worker identity")]
    TargetIdentityMismatch,
}
