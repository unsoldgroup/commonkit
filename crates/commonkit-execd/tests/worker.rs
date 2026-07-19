#![cfg(unix)]
use axum::body::{Body, to_bytes};
use axum::http::Request;
use commonkit_contracts::*;
use commonkit_execd::worker::run_once;
use commonkit_execd::{ApiState, Capability, router};
use commonkit_execution::supervisor::{ProcessSupervisor, SupervisorMode};
use commonkit_execution::{LocalObjectStore, Scheduler};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::process::Command;
use tower::ServiceExt;

fn digest(seed: char) -> Sha256Digest {
    Sha256Digest::parse(format!("sha256:{}", seed.to_string().repeat(64))).unwrap()
}
fn manifest() -> ExecutionManifest {
    ExecutionManifest {
        schema_version: SchemaVersion(1),
        repository: "https://example.invalid/repo.git".into(),
        repository_revision: GitRevision::parse("a".repeat(40)).unwrap(),
        workspace_bundle_digest: None,
        argv: vec![
            "sh".into(),
            "-c".into(),
            "printf 'worker-ok:%s' \"$TASK_TOKEN\"".into(),
        ],
        workdir: PortableSourcePath::parse("work").unwrap(),
        secret_refs: vec!["env://TASK_TOKEN".into()],
        timeout_seconds: 30,
        cancel_grace_seconds: 1,
        resources: ResourceRequirements {
            cpu_millis: 100,
            memory_mib: 64,
            disk_mib: 10,
        },
        required_capabilities: BTreeSet::new(),
        loadout_digest: digest('b'),
        execution_profile_digest: digest('c'),
        retry: RetryPolicy {
            max_attempts: 1,
            retryable_exit_codes: BTreeSet::new(),
        },
        checkpoint_enabled: true,
        artifacts: ArtifactPolicy {
            globs: vec!["diagnostics/**".into()],
            retention_seconds: 60,
            max_bytes: 1024,
        },
        network_policy: NetworkPolicy::Deny,
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
        capabilities: BTreeSet::new(),
        ready_secret_refs: BTreeSet::from(["env://TASK_TOKEN".into()]),
        free: ResourceRequirements {
            cpu_millis: 1000,
            memory_mib: 1024,
            disk_mib: 1000,
        },
        queue_depth: 0,
        cost_score: 0,
    }
}

fn init_repository(root: &std::path::Path) -> GitRevision {
    std::fs::write(root.join(".fixture"), "commonkit execution fixture").unwrap();
    for args in [
        vec!["init", "-q"],
        vec!["config", "user.email", "tests@commonkit.invalid"],
        vec!["config", "user.name", "CommonKit Tests"],
        vec![
            "remote",
            "add",
            "origin",
            "https://example.invalid/repo.git",
        ],
        vec!["add", "."],
        vec!["commit", "-qm", "fixture"],
    ] {
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    GitRevision::parse(String::from_utf8(output.stdout).unwrap().trim()).unwrap()
}

#[tokio::test]
async fn submitted_job_runs_after_client_disconnect_and_returns_artifacts() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(directory.path().join("workspace/work")).unwrap();
    let objects = LocalObjectStore::open(directory.path().join("objects")).unwrap();
    let state = ApiState::new(
        Scheduler::open(directory.path().join("jobs.db")).unwrap(),
        vec![(
            "client-token".into(),
            "client".into(),
            BTreeSet::from([Capability::Submit, Capability::Read, Capability::Cancel]),
        )],
        false,
    )
    .with_object_store(objects.clone(), "artifact-signing-key");
    let mut submitted_manifest = manifest();
    submitted_manifest.repository_revision = init_repository(&directory.path().join("workspace"));
    let body = json!({"manifest":submitted_manifest,"idempotencyKey":"offline-client"}).to_string();
    let response = router(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/execution/v1/jobs")
                .header("authorization", "Bearer client-token")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let submitted: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let job_id = submitted["jobId"].as_str().unwrap().to_string();
    let supervisor = ProcessSupervisor::new(
        SupervisorMode::ProcessGroup,
        directory.path().join("diagnostics"),
    )
    .unwrap();
    run_once(
        &state,
        &target(),
        StableId::parse("linux-vps").unwrap(),
        &directory.path().join("workspace"),
        &objects,
        &BTreeMap::from([("env://TASK_TOKEN".into(), "resolved-task-secret".into())]),
        &supervisor,
    )
    .await
    .unwrap();
    let response = router(state.clone())
        .oneshot(
            Request::builder()
                .uri(format!("/execution/v1/jobs/{job_id}"))
                .header("authorization", "Bearer client-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let status: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(status["attempt"]["state"], "succeeded");
    assert_eq!(
        status["receipt"]["artifactIds"].as_array().unwrap().len(),
        1
    );
    let artifact_id = status["receipt"]["artifactIds"][0].as_str().unwrap();
    let response = router(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/execution/v1/artifacts/{artifact_id}/access"))
                .header("authorization", "Bearer client-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let access: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
            .unwrap();
    let response = router(state)
        .oneshot(
            Request::builder()
                .uri(access["url"].as_str().unwrap())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap()
            .as_ref(),
        b"worker-ok:[REDACTED]"
    );
}

#[tokio::test]
async fn workspace_revision_denial_is_failed_and_audited() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(directory.path().join("workspace/work")).unwrap();
    init_repository(&directory.path().join("workspace"));
    let objects = LocalObjectStore::open(directory.path().join("objects")).unwrap();
    let state = ApiState::new(
        Scheduler::open(directory.path().join("jobs.db")).unwrap(),
        vec![(
            "client-token".into(),
            "client".into(),
            BTreeSet::from([Capability::Submit]),
        )],
        false,
    );
    let body = json!({"manifest":manifest(),"idempotencyKey":"stale-workspace"}).to_string();
    let response = router(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/execution/v1/jobs")
                .header("authorization", "Bearer client-token")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(response.status().is_success());
    let supervisor = ProcessSupervisor::new(
        SupervisorMode::ProcessGroup,
        directory.path().join("diagnostics"),
    )
    .unwrap();
    let error = run_once(
        &state,
        &target(),
        StableId::parse("linux-vps").unwrap(),
        &directory.path().join("workspace"),
        &objects,
        &BTreeMap::from([("env://TASK_TOKEN".into(), "resolved-task-secret".into())]),
        &supervisor,
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("supervisor failed"));
    let audit = state.audit_entries().unwrap();
    assert!(
        audit
            .iter()
            .any(|entry| !entry.allowed && entry.reason == "workspace_revision_denied")
    );
}
