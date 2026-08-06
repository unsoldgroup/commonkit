use commonkit_contracts::*;
use commonkit_execution::*;
use std::collections::BTreeSet;

fn digest(seed: char) -> Sha256Digest {
    Sha256Digest::parse(format!("sha256:{}", seed.to_string().repeat(64))).unwrap()
}
fn manifest() -> ExecutionManifest {
    ExecutionManifest {
        schema_version: SchemaVersion(1),
        repository: "https://example.invalid/repo.git".into(),
        repository_revision: GitRevision::parse("a".repeat(40)).unwrap(),
        workspace_bundle_digest: None,
        argv: vec!["node".into(), "bench.mjs".into()],
        workdir: PortableSourcePath::parse("bench").unwrap(),
        secret_refs: vec!["bws:project/key".into()],
        timeout_seconds: 600,
        cancel_grace_seconds: 5,
        resources: ResourceRequirements {
            cpu_millis: 1000,
            memory_mib: 512,
            disk_mib: 100,
        },
        required_capabilities: BTreeSet::from([StableId::parse("browser").unwrap()]),
        loadout_digest: digest('b'),
        execution_profile_digest: digest('c'),
        retry: RetryPolicy {
            max_attempts: 2,
            retryable_exit_codes: BTreeSet::from([1]),
        },
        checkpoint_enabled: true,
        artifacts: ArtifactPolicy {
            globs: vec!["results/**".into()],
            retention_seconds: 86400,
            max_bytes: 1024,
        },
        network_policy: NetworkPolicy::Restricted,
        repository_write: false,
        browser: None,
    }
}
fn target() -> ExecutionTarget {
    ExecutionTarget {
        id: StableId::parse("linux-vps").unwrap(),
        platform: "linux".into(),
        healthy: true,
        draining: false,
        loadout_digest: digest('b'),
        execution_profile_digest: digest('c'),
        capabilities: BTreeSet::from([StableId::parse("browser").unwrap()]),
        ready_secret_refs: BTreeSet::from(["bws:project/key".into()]),
        free: ResourceRequirements {
            cpu_millis: 4000,
            memory_mib: 4096,
            disk_mib: 10000,
        },
        queue_depth: 0,
        cost_score: 10,
    }
}

#[test]
fn submission_is_durable_and_idempotent() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("execution.db");
    let first = {
        let mut scheduler = Scheduler::open(&database).unwrap();
        scheduler.submit(&manifest(), "request-1", 1).unwrap()
    };
    let job_id = match &first {
        SubmitOutcome::Created(id) => id.clone(),
        _ => panic!(),
    };
    drop(first);
    let mut scheduler = Scheduler::open(&database).unwrap();
    assert_eq!(
        scheduler.submit(&manifest(), "request-1", 2).unwrap(),
        SubmitOutcome::Existing(job_id.clone())
    );
    assert_eq!(
        scheduler.snapshot(&job_id).unwrap().attempt.state,
        JobState::Queued
    );
    let mut changed = manifest();
    changed.argv.push("different".into());
    assert!(matches!(
        scheduler.submit(&changed, "request-1", 2),
        Err(ExecutionError::IdempotencyConflict)
    ));
}

#[test]
fn stale_workers_cannot_checkpoint_or_complete_after_recovery() {
    let mut scheduler = Scheduler::open_memory().unwrap();
    let job_id = match scheduler.submit(&manifest(), "request-2", 1).unwrap() {
        SubmitOutcome::Created(id) => id,
        _ => panic!(),
    };
    let lease = scheduler
        .acquire(StableId::parse("worker-a").unwrap(), &target(), 10, 100)
        .unwrap()
        .unwrap();
    scheduler.recover_expired(111).unwrap();
    assert!(matches!(
        scheduler.checkpoint(&lease, "shard-1", digest('d'), 112),
        Err(ExecutionError::StaleFence)
    ));
    assert!(matches!(
        scheduler.complete(&lease, JobState::Succeeded, Some(0), 112),
        Err(ExecutionError::StaleFence)
    ));
    assert_eq!(scheduler.snapshot(&job_id).unwrap().attempt.number, 2);
}

#[test]
fn browser_shard_and_aggregate_commits_are_exactly_once() {
    let mut scheduler = Scheduler::open_memory().unwrap();
    let job_id = match scheduler.submit(&manifest(), "request-3", 1).unwrap() {
        SubmitOutcome::Created(id) => id,
        _ => panic!(),
    };
    let lease = scheduler
        .acquire(StableId::parse("worker-a").unwrap(), &target(), 10, 1000)
        .unwrap()
        .unwrap();
    let first = scheduler
        .checkpoint(&lease, "shard-1", digest('d'), 11)
        .unwrap();
    let duplicate = scheduler
        .checkpoint(&lease, "shard-1", digest('e'), 12)
        .unwrap();
    assert_eq!(first.id, duplicate.id);
    let aggregate = scheduler
        .commit_artifact(
            &lease,
            "aggregate.json",
            digest('f'),
            20,
            "application/json",
            13,
        )
        .unwrap();
    let retry = scheduler
        .commit_artifact(
            &lease,
            "aggregate.json",
            digest('1'),
            20,
            "application/json",
            14,
        )
        .unwrap();
    assert_eq!(aggregate.id, retry.id);
    assert_eq!(
        scheduler.snapshot(&job_id).unwrap().attempt.state,
        JobState::Assigned
    );
}

#[test]
fn placement_hard_filters_before_deterministic_scoring() {
    let mut bad = target();
    bad.id = StableId::parse("bad-target").unwrap();
    bad.free.memory_mib = 100;
    let result = placement(&manifest(), &[bad, target()]);
    assert_eq!(result.selected, Some(StableId::parse("linux-vps").unwrap()));
    assert!(
        !result
            .explanations
            .iter()
            .find(|e| e.target_id.as_str() == "bad-target")
            .unwrap()
            .eligible
    );
}

#[test]
fn events_are_monotonic_and_implementor_correlation_is_idempotent() {
    let mut scheduler = Scheduler::open_memory().unwrap();
    let job_id = match scheduler.submit(&manifest(), "request-4", 1).unwrap() {
        SubmitOutcome::Created(id) => id,
        _ => panic!(),
    };
    let lease = scheduler
        .acquire(StableId::parse("worker-a").unwrap(), &target(), 10, 1000)
        .unwrap()
        .unwrap();
    scheduler
        .correlate_engine_run(&lease.attempt_id, lease.fencing_token, "opaque-run", 11)
        .unwrap();
    scheduler
        .correlate_engine_run(&lease.attempt_id, lease.fencing_token, "opaque-run", 12)
        .unwrap();
    assert!(matches!(
        scheduler.correlate_engine_run(&lease.attempt_id, lease.fencing_token, "other-run", 13),
        Err(ExecutionError::DuplicateImplementorRun)
    ));
    let events = scheduler.events(&job_id, 0, 100).unwrap();
    assert!(
        events
            .windows(2)
            .all(|pair| pair[0].sequence < pair[1].sequence)
    );
}

#[test]
fn backup_restore_and_cursor_expiry_are_explicit() {
    let directory = tempfile::tempdir().unwrap();
    let backup = directory.path().join("backup.db");
    let mut scheduler = Scheduler::open_memory().unwrap();
    let job_id = match scheduler.submit(&manifest(), "backup-job", 1).unwrap() {
        SubmitOutcome::Created(id) => id,
        _ => panic!(),
    };
    let lease = scheduler
        .acquire(StableId::parse("worker-a").unwrap(), &target(), 10, 1000)
        .unwrap()
        .unwrap();
    scheduler
        .correlate_engine_run(&lease.attempt_id, lease.fencing_token, "opaque", 11)
        .unwrap();
    scheduler.backup(&backup).unwrap();
    scheduler.prune_events_before(&job_id, 3).unwrap();
    assert!(scheduler.events(&job_id, 0, 10).is_ok());
    assert!(matches!(
        scheduler.events(&job_id, 1, 10),
        Err(ExecutionError::CursorExpired { minimum: 3 })
    ));
    let mut restored = Scheduler::open_memory().unwrap();
    restored.restore(&backup).unwrap();
    assert_eq!(restored.snapshot(&job_id).unwrap().job.id, job_id);
}

#[test]
fn object_artifacts_are_encrypted_before_storage() {
    let directory = tempfile::tempdir().unwrap();
    let store = LocalObjectStore::open_encrypted(directory.path(), [7; 32]).unwrap();
    let digest = store.put(b"sensitive artifact", 1024).unwrap();
    assert_eq!(store.get(&digest).unwrap(), b"sensitive artifact");
    let raw = std::fs::read(
        directory
            .path()
            .join(digest.as_str().trim_start_matches("sha256:")),
    )
    .unwrap();
    assert!(!raw.windows(9).any(|window| window == b"sensitive"));
}

#[test]
fn manual_retry_resume_and_target_heartbeat_are_durable() {
    let mut scheduler = Scheduler::open_memory().unwrap();
    let job_id = match scheduler.submit(&manifest(), "followup", 1).unwrap() {
        SubmitOutcome::Created(id) => id,
        _ => panic!(),
    };
    let worker = StableId::parse("linux-vps").unwrap();
    scheduler.register_target(&worker, &target(), 2).unwrap();
    assert!(scheduler.targets(10, 100).unwrap()[0].healthy);
    assert!(!scheduler.targets(200, 100).unwrap()[0].healthy);
    let lease = scheduler
        .acquire(worker, &target(), 10, 1000)
        .unwrap()
        .unwrap();
    scheduler
        .checkpoint(&lease, "stage-1", digest('d'), 11)
        .unwrap();
    scheduler
        .complete(&lease, JobState::Failed, Some(1), 12)
        .unwrap();
    let resumed = scheduler.resume(&job_id, "stage-1", 13).unwrap();
    assert_eq!(resumed.number, 2);
    assert!(matches!(
        scheduler.retry(&job_id, 14),
        Err(ExecutionError::AttemptActive)
    ));
}
