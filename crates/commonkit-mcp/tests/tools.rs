use std::sync::{Arc, Mutex};

use commonkit_mcp::{
    ApplyPlanInput, BackendFuture, CommonKitMcp, ConsentInput, ControlBackend, ReadInput,
};
use rmcp::handler::server::wrapper::Parameters;
use serde_json::json;

#[derive(Default)]
struct FakeBackend {
    applies: Mutex<Vec<String>>,
    posts: Mutex<Vec<String>>,
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

    fn post(&self, path: &'static str, _input: serde_json::Value) -> BackendFuture<'_> {
        self.posts.lock().expect("posts").push(path.into());
        Box::pin(async move { Ok(json!({"path": path})) })
    }
}

#[test]
fn publishes_stable_initial_tool_names() {
    let names = CommonKitMcp::published_tool_names();
    assert_eq!(
        names,
        [
            "commonkit_apply_plan",
            "commonkit_compose",
            "commonkit_explain",
            "commonkit_export_diagnostics",
            "commonkit_get_status",
            "commonkit_plan_sync",
            "commonkit_rollback",
            "commonkit_snapshot_create",
            "commonkit_snapshot_restore",
            "commonkit_verify",
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
    assert!(backend.posts.lock().expect("posts").is_empty());
}

#[tokio::test]
async fn every_mutation_tool_requires_consent_before_backend_execution() {
    let backend = Arc::new(FakeBackend::default());
    let server = CommonKitMcp::new(backend.clone());
    for result in [
        server.plan_sync(Parameters(ConsentInput::denied())).await,
        server
            .snapshot_create(Parameters(ConsentInput::denied()))
            .await,
        server
            .snapshot_restore(Parameters(ConsentInput::denied()))
            .await,
        server.rollback(Parameters(ConsentInput::denied())).await,
    ] {
        let result = result.expect("structured denial");
        assert_eq!(result.is_error, Some(true));
        assert_eq!(
            result.structured_content.expect("structured")["code"],
            "confirmation_required"
        );
    }
    assert!(backend.applies.lock().expect("applies").is_empty());
    assert!(backend.posts.lock().expect("posts").is_empty());
}

#[tokio::test]
async fn read_tools_forward_only_bounded_structured_inputs() {
    let server = CommonKitMcp::new(Arc::new(FakeBackend::default()));
    let result = server
        .compose(Parameters(ReadInput {
            target_id: Some("local".into()),
            pointer: None,
        }))
        .await
        .expect("compose");
    assert_eq!(
        result.structured_content.expect("structured")["path"],
        "/control/v1/compose"
    );
}
