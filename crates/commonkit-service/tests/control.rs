use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use commonkit_contracts::{OperationKind, PlanBindings, ResourceRef, Risk, Sha256Digest, StableId};
use commonkit_core::{OperationDraft, PlanDraft, build_plan, finalize_operation};
use commonkit_reconcile::PlanStore;
use commonkit_service::{
    ApplyStatus, ControlPlane, ControlToken, EventHub, ExecutionResult, PlanExecutor,
    ServiceStatus, router_with_control,
};
use tokio::sync::RwLock;
use tower::ServiceExt;

fn digest(character: char) -> Sha256Digest {
    Sha256Digest::parse(format!("sha256:{}", character.to_string().repeat(64))).expect("digest")
}

fn bindings() -> PlanBindings {
    PlanBindings {
        composed_loadout_digest: digest('4'),
        provider_inputs_digest: digest('5'),
        ownership_map_digest: digest('6'),
        artifact_set_digest: digest('7'),
    }
}

fn temporary_directory(test: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!("commonkit-{test}-{}-{nonce}", std::process::id()))
}

fn plan() -> commonkit_contracts::Plan {
    let operation = finalize_operation(OperationDraft {
        adapter_id: StableId::parse("files").expect("adapter"),
        kind: OperationKind::Create,
        resource: ResourceRef {
            resource_type: StableId::parse("file").expect("type"),
            resource_id: StableId::parse("config").expect("resource"),
            managed_path: Some("config.json".into()),
        },
        risk: Risk::Low,
        requires_confirmation: true,
        depends_on: vec![],
        before_digest: None,
        after_digest: Some(digest('d')),
        payload_digest: digest('e'),
        summary: "create config".into(),
    })
    .expect("operation");
    build_plan(PlanDraft {
        target_id: StableId::parse("laptop").expect("target"),
        desired_digest: digest('a'),
        observed_digest: digest('b'),
        policy_digest: digest('c'),
        bindings: bindings(),
        operations: vec![operation],
    })
    .expect("plan")
}

struct SuccessfulExecutor {
    calls: AtomicUsize,
}

impl PlanExecutor for SuccessfulExecutor {
    fn execute(
        &self,
        _plan: &commonkit_contracts::Plan,
        _confirmation_id: &StableId,
    ) -> ExecutionResult {
        self.calls.fetch_add(1, Ordering::SeqCst);
        ExecutionResult {
            status: ApplyStatus::Succeeded,
            failure_code: None,
        }
    }
}

#[test]
fn validates_registered_plans_and_executes_each_idempotency_key_once() {
    let executor = Arc::new(SuccessfulExecutor {
        calls: AtomicUsize::new(0),
    });
    let control = ControlPlane::new(executor.clone());
    let plan = control.register_plan(plan()).expect("register");
    let confirmation = StableId::parse("user-approved").expect("confirmation");

    let (first, created) = control
        .apply(&plan.id, &confirmation, "request-001")
        .expect("apply");
    let (second, repeated) = control
        .apply(&plan.id, &confirmation, "request-001")
        .expect("repeat");
    assert!(created);
    assert!(!repeated);
    assert_eq!(first, second);
    assert_eq!(first.status, ApplyStatus::Succeeded);
    assert_eq!(executor.calls.load(Ordering::SeqCst), 1);

    let mut tampered = plan;
    tampered.target_id = StableId::parse("other").expect("target");
    assert!(control.register_plan(tampered).is_err());
}

#[test]
fn reloads_a_registered_plan_after_the_control_plane_restarts() {
    let root = temporary_directory("control-plan-restart");
    let store = Arc::new(PlanStore::open(&root).expect("plan store"));
    let executor = Arc::new(SuccessfulExecutor {
        calls: AtomicUsize::new(0),
    });
    let plan = ControlPlane::with_plan_store(executor.clone(), store.clone())
        .register_plan(plan())
        .expect("register");

    let restarted = ControlPlane::with_plan_store(executor, store);
    assert_eq!(restarted.plan(&plan.id), Some(plan));
    fs::remove_dir_all(root).expect("cleanup");
}

#[tokio::test]
async fn apply_endpoint_requires_explicit_confirmation_and_idempotency() {
    let token = ControlToken::generate();
    let control = ControlPlane::new(Arc::new(SuccessfulExecutor {
        calls: AtomicUsize::new(0),
    }));
    let plan = control.register_plan(plan()).expect("register");
    let application = router_with_control(
        token.clone(),
        Arc::new(RwLock::new(ServiceStatus::default())),
        "127.0.0.1:3764",
        EventHub::new(8),
        control,
    );
    let uri = format!("/control/v1/plans/{}/apply", plan.id);

    let unconfirmed = application
        .clone()
        .oneshot(
            Request::post(&uri)
                .header(header::HOST, "127.0.0.1:3764")
                .header(
                    header::AUTHORIZATION,
                    format!("Bearer {}", token.expose_for_client()),
                )
                .header(header::CONTENT_TYPE, "application/json")
                .header("idempotency-key", "request-002")
                .body(Body::from(
                    r#"{"confirmed":false,"confirmationId":"user-approved"}"#,
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(unconfirmed.status(), StatusCode::CONFLICT);

    let confirmed = application
        .oneshot(
            Request::post(&uri)
                .header(header::HOST, "127.0.0.1:3764")
                .header(
                    header::AUTHORIZATION,
                    format!("Bearer {}", token.expose_for_client()),
                )
                .header(header::CONTENT_TYPE, "application/json")
                .header("idempotency-key", "request-002")
                .body(Body::from(
                    r#"{"confirmed":true,"confirmationId":"user-approved"}"#,
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(confirmed.status(), StatusCode::ACCEPTED);
}
