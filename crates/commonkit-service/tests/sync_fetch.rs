use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use commonkit_contracts::StableId;
use commonkit_service::{
    ApplyStatus, ControlPlane, ControlToken, DomainFailure, EventHub, ExecutionResult,
    HeadlessDomainRegistry, PlanExecutor, ServiceStatus, SyncDomain, router_with_control,
};
use tokio::sync::RwLock;
use tower::ServiceExt;

struct NeverExecute;

impl PlanExecutor for NeverExecute {
    fn execute(&self, _: &commonkit_contracts::Plan, _: &StableId) -> ExecutionResult {
        ExecutionResult {
            status: ApplyStatus::Failed,
            failure_code: None,
        }
    }
}

struct OrderedSync {
    calls: Mutex<Vec<&'static str>>,
}

struct GatedSync {
    git_state: Option<&'static str>,
    plan_calls: AtomicUsize,
}

impl SyncDomain for GatedSync {
    fn git_sync(&self, fetch: bool) -> Result<serde_json::Value, DomainFailure> {
        assert!(fetch);
        self.git_state
            .map(|state| serde_json::json!({"state":state,"fetched":true}))
            .ok_or(DomainFailure::OperationFailed)
    }

    fn plan(&self, _: serde_json::Value) -> Result<serde_json::Value, DomainFailure> {
        self.plan_calls.fetch_add(1, Ordering::SeqCst);
        Ok(serde_json::json!({"unexpected":"plan"}))
    }

    fn verify(&self, _: serde_json::Value) -> Result<serde_json::Value, DomainFailure> {
        Err(DomainFailure::InvalidRequest)
    }

    fn rollback(&self, _: serde_json::Value) -> Result<serde_json::Value, DomainFailure> {
        Err(DomainFailure::InvalidRequest)
    }
}

impl SyncDomain for OrderedSync {
    fn git_sync(&self, fetch: bool) -> Result<serde_json::Value, DomainFailure> {
        assert!(fetch, "sync --fetch must fetch before planning");
        self.calls.lock().unwrap().push("fetch");
        Ok(serde_json::json!({"state":"clean","fetched":true}))
    }

    fn plan(&self, request: serde_json::Value) -> Result<serde_json::Value, DomainFailure> {
        assert_eq!(self.calls.lock().unwrap().as_slice(), ["fetch"]);
        assert_eq!(request.get("fetch"), None);
        self.calls.lock().unwrap().push("plan");
        Ok(
            serde_json::json!({"planId":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}),
        )
    }

    fn verify(&self, _: serde_json::Value) -> Result<serde_json::Value, DomainFailure> {
        Err(DomainFailure::InvalidRequest)
    }

    fn rollback(&self, _: serde_json::Value) -> Result<serde_json::Value, DomainFailure> {
        Err(DomainFailure::InvalidRequest)
    }
}

#[tokio::test]
async fn authenticated_sync_fetches_and_validates_before_returning_a_plan_without_applying() {
    let domain = Arc::new(OrderedSync {
        calls: Mutex::new(Vec::new()),
    });
    let control = ControlPlane::new(Arc::new(NeverExecute));
    control.set_headless_domains(HeadlessDomainRegistry {
        sync: Some(domain.clone()),
        ..HeadlessDomainRegistry::default()
    });
    let token = ControlToken::generate();
    let app = router_with_control(
        token.clone(),
        Arc::new(RwLock::new(ServiceStatus::default())),
        "127.0.0.1:3764",
        EventHub::new(4),
        control,
    );

    let response = app
        .oneshot(
            Request::post("/control/v1/sync/plan")
                .header(header::HOST, "127.0.0.1:3764")
                .header(
                    header::AUTHORIZATION,
                    format!("Bearer {}", token.expose_for_client()),
                )
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"confirmed":true,"confirmationId":"sync-fetch","fetch":true}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    let status = response.status();
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    assert_eq!(domain.calls.lock().unwrap().as_slice(), ["fetch", "plan"]);
}

#[tokio::test]
async fn sync_fetch_rejects_dirty_git_state_before_provider_planning() {
    let domain = Arc::new(GatedSync {
        git_state: Some("dirty"),
        plan_calls: AtomicUsize::new(0),
    });
    let control = ControlPlane::new(Arc::new(NeverExecute));
    control.set_headless_domains(HeadlessDomainRegistry {
        sync: Some(domain.clone()),
        ..HeadlessDomainRegistry::default()
    });
    let token = ControlToken::generate();
    let app = router_with_control(
        token.clone(),
        Arc::new(RwLock::new(ServiceStatus::default())),
        "127.0.0.1:3764",
        EventHub::new(4),
        control,
    );

    let response = app
        .oneshot(
            Request::post("/control/v1/sync/plan")
                .header(header::HOST, "127.0.0.1:3764")
                .header(
                    header::AUTHORIZATION,
                    format!("Bearer {}", token.expose_for_client()),
                )
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"confirmed":true,"confirmationId":"sync-fetch","fetch":true}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(domain.plan_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn sync_fetch_fails_closed_on_diverged_or_untrusted_git_state() {
    for git_state in [Some("diverged"), None] {
        let domain = Arc::new(GatedSync {
            git_state,
            plan_calls: AtomicUsize::new(0),
        });
        let control = ControlPlane::new(Arc::new(NeverExecute));
        control.set_headless_domains(HeadlessDomainRegistry {
            sync: Some(domain.clone()),
            ..HeadlessDomainRegistry::default()
        });
        let token = ControlToken::generate();
        let app = router_with_control(
            token.clone(),
            Arc::new(RwLock::new(ServiceStatus::default())),
            "127.0.0.1:3764",
            EventHub::new(4),
            control,
        );
        let response = app
            .oneshot(
                Request::post("/control/v1/sync/plan")
                    .header(header::HOST, "127.0.0.1:3764")
                    .header(
                        header::AUTHORIZATION,
                        format!("Bearer {}", token.expose_for_client()),
                    )
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"confirmed":true,"confirmationId":"sync-fetch","fetch":true}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert!(!response.status().is_success());
        assert_eq!(domain.plan_calls.load(Ordering::SeqCst), 0);
    }
}
