use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use commonkit_contracts::*;
use commonkit_mcp::{
    ApplyPlanInput, BackendFuture, CommonKitMcp, ControlBackend, ExecutionBackend,
    ExecutionContext, SubmitTaskInput,
};
use rmcp::handler::server::wrapper::Parameters;
use serde_json::json;

#[derive(Default)]
struct FakeBackend {
    applies: Mutex<Vec<String>>,
}

impl ControlBackend for FakeBackend {
    fn get(&self, path: &'static str) -> BackendFuture<'_> {
        Box::pin(async move { Ok(json!({"path": path, "state": "healthy"})) })
    }

    fn apply(&self, input: ApplyPlanInput) -> BackendFuture<'_> {
        self.applies
            .lock()
            .expect("applies")
            .push(input.plan_id.clone());
        Box::pin(async move { Ok(json!({"planId": input.plan_id, "status": "running"})) })
    }
}
#[derive(Default)]
struct FakeExecution {
    paths: Mutex<Vec<String>>,
}
impl ExecutionBackend for FakeExecution {
    fn request(
        &self,
        _: &'static str,
        path: String,
        _: Option<serde_json::Value>,
    ) -> BackendFuture<'_> {
        self.paths.lock().unwrap().push(path);
        Box::pin(async { Ok(json!({"jobId":"job_1"})) })
    }
}
fn digest(seed: char) -> Sha256Digest {
    Sha256Digest::parse(format!("sha256:{}", seed.to_string().repeat(64))).unwrap()
}
fn manifest() -> ExecutionManifest {
    ExecutionManifest {
        schema_version: SchemaVersion(1),
        repository: "https://example.invalid/repo.git".into(),
        repository_revision: GitRevision::parse("a".repeat(40)).unwrap(),
        workspace_bundle_digest: None,
        argv: vec!["cargo".into(), "test".into()],
        workdir: PortableSourcePath::parse("repo").unwrap(),
        secret_refs: vec![],
        timeout_seconds: 60,
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
        checkpoint_enabled: false,
        artifacts: ArtifactPolicy {
            globs: vec![],
            retention_seconds: 60,
            max_bytes: 1024,
        },
        network_policy: NetworkPolicy::Deny,
        repository_write: false,
        browser: None,
    }
}

#[test]
fn publishes_stable_initial_tool_names() {
    let names = CommonKitMcp::published_tool_names();
    assert_eq!(
        names,
        [
            "commonkit_apply_plan",
            "commonkit_cancel_job",
            "commonkit_export_diagnostics",
            "commonkit_get_artifact_access",
            "commonkit_get_execution_context",
            "commonkit_get_execution_targets",
            "commonkit_get_job",
            "commonkit_get_job_events",
            "commonkit_get_status",
            "commonkit_list_job_artifacts",
            "commonkit_resume_job",
            "commonkit_retry_job",
            "commonkit_submit_task",
        ]
    );
}

#[tokio::test]
async fn read_tools_return_structured_daemon_data() {
    let server = CommonKitMcp::new(Arc::new(FakeBackend::default()));
    let result = server.get_status().await.expect("status");
    assert_eq!(
        result.structured_content.expect("structured")["state"],
        "healthy"
    );
}

#[tokio::test]
async fn apply_refuses_missing_confirmation_before_calling_the_backend() {
    let backend = Arc::new(FakeBackend::default());
    let server = CommonKitMcp::new(backend.clone());
    let denied = server
        .apply_plan(Parameters(ApplyPlanInput {
            plan_id: format!("sha256:{}", "a".repeat(64)),
            confirmed: false,
            confirmation_id: "user-approved".into(),
            idempotency_key: "request-1".into(),
        }))
        .await
        .expect("denied");
    assert_eq!(denied.is_error, Some(true));
    assert!(backend.applies.lock().expect("applies").is_empty());
}

#[tokio::test]
async fn remote_mcp_submits_only_repository_declared_tasks() {
    let execution = Arc::new(FakeExecution::default());
    let server = CommonKitMcp::new(Arc::new(FakeBackend::default())).with_execution(
        execution.clone(),
        ExecutionContext {
            issue_id: "USG-46".into(),
            plan: "Run contracts".into(),
            skills: vec!["durable-objects".into()],
            tasks: BTreeMap::from([("verify".into(), manifest())]),
        },
    );
    let context = server.get_execution_context().await.unwrap();
    assert_eq!(context.structured_content.unwrap()["issueId"], "USG-46");
    let denied = server
        .submit_task(Parameters(SubmitTaskInput {
            task_id: "arbitrary-shell".into(),
            idempotency_key: "1".into(),
        }))
        .await
        .unwrap();
    assert_eq!(denied.is_error, Some(true));
    assert!(execution.paths.lock().unwrap().is_empty());
    let accepted = server
        .submit_task(Parameters(SubmitTaskInput {
            task_id: "verify".into(),
            idempotency_key: "2".into(),
        }))
        .await
        .unwrap();
    assert_eq!(accepted.structured_content.unwrap()["jobId"], "job_1");
}
