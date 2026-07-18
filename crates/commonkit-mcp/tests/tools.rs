use std::sync::{Arc, Mutex};

use commonkit_mcp::{ApplyPlanInput, BackendFuture, CommonKitMcp, ControlBackend};
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

#[test]
fn publishes_stable_initial_tool_names() {
    let names = CommonKitMcp::published_tool_names();
    assert_eq!(
        names,
        [
            "commonkit_apply_plan",
            "commonkit_export_diagnostics",
            "commonkit_get_status",
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
