#![cfg(unix)]
use axum::body::{Body, to_bytes};
use axum::http::Request;
use commonkit_contracts::*;
use commonkit_execd::worker::{WorkerContext, run_once};
use commonkit_execd::{ApiState, Capability, ExecutionPolicy, router};
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

/// Creates the upstream repository that workspace preparation clones from. The
/// manifest's `workdir` must exist in the tree, so the fixture commits `work/`.
fn init_repository(root: &std::path::Path) -> GitRevision {
    std::fs::create_dir_all(root.join("work")).unwrap();
    std::fs::write(root.join("work/.fixture"), "commonkit execution fixture").unwrap();
    for args in [
        vec!["init", "-q"],
        vec!["config", "user.email", "tests@commonkit.invalid"],
        vec!["config", "user.name", "CommonKit Tests"],
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
    git_revision(root, "HEAD")
}

fn git_revision(root: &std::path::Path, reference: &str) -> GitRevision {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", reference])
        .output()
        .unwrap();
    GitRevision::parse(String::from_utf8(output.stdout).unwrap().trim()).unwrap()
}

/// Builds a source repository and a manifest pinned to its head commit.
fn manifest_for(source: &std::path::Path) -> (ExecutionManifest, GitRevision) {
    std::fs::create_dir_all(source).unwrap();
    let revision = init_repository(source);
    let mut manifest = manifest();
    manifest.repository = source.to_str().unwrap().to_owned();
    manifest.repository_revision = revision.clone();
    (manifest, revision)
}

#[tokio::test]
async fn submitted_job_runs_after_client_disconnect_and_returns_artifacts() {
    let directory = tempfile::tempdir().unwrap();
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
    let (submitted_manifest, _) = manifest_for(&directory.path().join("source"));
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
        &WorkerContext {
            workspace_root: &directory.path().join("workspace"),
            objects: &objects,
            resolved_secrets: &BTreeMap::from([(
                "env://TASK_TOKEN".into(),
                "resolved-task-secret".into(),
            )]),
            supervisor: &supervisor,
            keep_failed_workspaces: false,
            status: None,
        },
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

/// Preparation clones from `manifest.repository`, so the runtime policy check has to
/// come first: a repository the policy denies must never be contacted.
#[tokio::test]
async fn policy_denied_repository_is_never_fetched() {
    let directory = tempfile::tempdir().unwrap();
    let objects = LocalObjectStore::open(directory.path().join("objects")).unwrap();
    let (denied, _) = manifest_for(&directory.path().join("source"));
    let permissive = ExecutionPolicy {
        allowed_repositories: BTreeSet::from([denied.repository.clone()]),
        max_cpu_millis: 1_000,
        max_memory_mib: 1_024,
        max_disk_mib: 1_024,
        allow_network: false,
        allow_repository_write: false,
    };
    let state = ApiState::new(
        Scheduler::open(directory.path().join("jobs.db")).unwrap(),
        vec![(
            "client-token".into(),
            "client".into(),
            BTreeSet::from([Capability::Submit]),
        )],
        false,
    )
    .with_policy(permissive);
    let body = json!({"manifest":denied,"idempotencyKey":"denied-repository"}).to_string();
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

    // The policy tightens between submission and execution: the worker re-checks.
    let state = ApiState::new(
        Scheduler::open(directory.path().join("jobs.db")).unwrap(),
        Vec::new(),
        false,
    )
    .with_policy(ExecutionPolicy {
        allowed_repositories: BTreeSet::new(),
        max_cpu_millis: 1_000,
        max_memory_mib: 1_024,
        max_disk_mib: 1_024,
        allow_network: false,
        allow_repository_write: false,
    });
    let supervisor = ProcessSupervisor::new(
        SupervisorMode::ProcessGroup,
        directory.path().join("diagnostics"),
    )
    .unwrap();
    let workspace_root = directory.path().join("workspace");
    let error = run_once(
        &state,
        &target(),
        StableId::parse("linux-vps").unwrap(),
        &WorkerContext {
            workspace_root: &workspace_root,
            objects: &objects,
            resolved_secrets: &BTreeMap::new(),
            supervisor: &supervisor,
            keep_failed_workspaces: false,
            status: None,
        },
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("policy denied"));
    assert!(!workspace_root.exists(), "denied repository was fetched");
}

/// The manifest declares `env://TASK_TOKEN`; this target cannot resolve it.
#[tokio::test]
async fn unresolved_secret_fails_the_attempt_and_is_audited() {
    let directory = tempfile::tempdir().unwrap();
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
    let (submitted, _) = manifest_for(&directory.path().join("source"));
    let body = json!({"manifest":submitted,"idempotencyKey":"unresolved-secret"}).to_string();
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
        &WorkerContext {
            workspace_root: &directory.path().join("workspace"),
            objects: &objects,
            resolved_secrets: &BTreeMap::new(),
            supervisor: &supervisor,
            keep_failed_workspaces: false,
            status: None,
        },
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("secret was not resolved"));
    let audit = state.audit_entries().unwrap();
    assert!(
        audit
            .iter()
            .any(|entry| !entry.allowed && entry.reason == "execution_secret_denied")
    );
}

#[tokio::test]
async fn unknown_revision_fails_preparation_and_is_audited() {
    let directory = tempfile::tempdir().unwrap();
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
    let (mut stale, _) = manifest_for(&directory.path().join("source"));
    stale.repository_revision = GitRevision::parse("a".repeat(40)).unwrap();
    let body = json!({"manifest":stale,"idempotencyKey":"stale-workspace"}).to_string();
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
        &WorkerContext {
            workspace_root: &directory.path().join("workspace"),
            objects: &objects,
            resolved_secrets: &BTreeMap::from([(
                "env://TASK_TOKEN".into(),
                "resolved-task-secret".into(),
            )]),
            supervisor: &supervisor,
            keep_failed_workspaces: false,
            status: None,
        },
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("workspace preparation failed"));
    let audit = state.audit_entries().unwrap();
    assert!(
        audit
            .iter()
            .any(|entry| !entry.allowed && entry.reason == "workspace_preparation_failed")
    );
}
