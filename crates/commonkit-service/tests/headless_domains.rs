use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::http::{Request, header};
use commonkit_contracts::StableId;
use commonkit_service::{
    ApplyStatus, CompositionDomain, ControlPlane, ControlToken, CredentialDomain, DomainFailure,
    EventHub, ExecutionResult, HeadlessDomainRegistry, PlanExecutor, ServiceStatus, SnapshotDomain,
    SyncDomain, router, router_with_control,
};
use serde_json::Value;
use tokio::sync::RwLock;
use tower::ServiceExt;

async fn call(
    application: axum::Router,
    token: &ControlToken,
    method: &str,
    path: &str,
    body: Value,
) -> Value {
    let response = application
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header(header::HOST, "127.0.0.1:3764")
                .header(
                    header::AUTHORIZATION,
                    format!("Bearer {}", token.expose_for_client()),
                )
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(response.status().is_success(), "{}", response.status());
    serde_json::from_slice(&to_bytes(response.into_body(), 8192).await.unwrap()).unwrap()
}

#[tokio::test]
async fn credentials_readiness_and_schedule_are_real_authenticated_domains() {
    let token = ControlToken::generate();
    let app = router(
        token.clone(),
        Arc::new(RwLock::new(ServiceStatus::default())),
        "127.0.0.1:3764",
    );
    let credential = call(
        app.clone(),
        &token,
        "POST",
        "/control/v1/credentials/readiness",
        serde_json::json!({"references":["env://COMMONKIT_TEST_MISSING"]}),
    )
    .await;
    assert_eq!(credential["credentials"][0]["readiness"], "missing");
    assert!(credential.to_string().contains("COMMONKIT_TEST_MISSING"));
    let enabled = call(
        app.clone(),
        &token,
        "POST",
        "/control/v1/schedule",
        serde_json::json!({"enabled":true,"intervalSeconds":60,"confirmed":true,"confirmationId":"schedule-test"}),
    )
    .await;
    assert_eq!(
        enabled,
        serde_json::json!({"enabled":true,"intervalSeconds":60})
    );
}

struct UnusedExecutor;
impl PlanExecutor for UnusedExecutor {
    fn execute(
        &self,
        _plan: &commonkit_contracts::Plan,
        _confirmation_id: &StableId,
    ) -> ExecutionResult {
        ExecutionResult {
            status: ApplyStatus::Failed,
            failure_code: None,
        }
    }
}

struct EchoDomains;
impl CompositionDomain for EchoDomains {
    fn compose(&self) -> Result<Value, DomainFailure> {
        Ok(serde_json::json!({"spec":{}}))
    }
    fn explain(&self, _: &str) -> Result<Value, DomainFailure> {
        Err(DomainFailure::InvalidRequest)
    }
    fn policy_summary(&self) -> Result<Value, DomainFailure> {
        Ok(serde_json::json!({"state":"valid","violations":[]}))
    }
}
impl SyncDomain for EchoDomains {
    fn plan(&self, _: Value) -> Result<Value, DomainFailure> {
        Ok(serde_json::json!({"domain":"sync","action":"plan"}))
    }
    fn verify(&self, _: Value) -> Result<Value, DomainFailure> {
        Ok(serde_json::json!({"domain":"sync","action":"verify"}))
    }
    fn rollback(&self, _: Value) -> Result<Value, DomainFailure> {
        Ok(serde_json::json!({"domain":"sync","action":"rollback"}))
    }
}
impl CredentialDomain for EchoDomains {
    fn plan(&self, _: Value) -> Result<Value, DomainFailure> {
        Ok(serde_json::json!({"domain":"credentials","action":"plan"}))
    }
    fn apply(&self, _: Value) -> Result<Value, DomainFailure> {
        Ok(serde_json::json!({"domain":"credentials","action":"apply"}))
    }
    fn verify(&self, _: Value) -> Result<Value, DomainFailure> {
        Ok(serde_json::json!({"domain":"credentials","action":"verify"}))
    }
}
impl SnapshotDomain for EchoDomains {
    fn create(&self, _: Value) -> Result<Value, DomainFailure> {
        Ok(serde_json::json!({"domain":"snapshots","action":"create"}))
    }
    fn list(&self) -> Result<Value, DomainFailure> {
        Ok(serde_json::json!({"domain":"snapshots","action":"list"}))
    }
    fn restore(&self, _: Value) -> Result<Value, DomainFailure> {
        Ok(serde_json::json!({"domain":"snapshots","action":"restore"}))
    }
    fn promote(&self, _: Value) -> Result<Value, DomainFailure> {
        Ok(serde_json::json!({"domain":"snapshots","action":"promote"}))
    }
}

#[tokio::test]
async fn configured_domains_receive_authenticated_consent_checked_requests() {
    let token = ControlToken::generate();
    let control = ControlPlane::new(Arc::new(UnusedExecutor));
    let domains = Arc::new(EchoDomains);
    control.set_headless_domains(HeadlessDomainRegistry {
        about_me: None,
        composition: Some(domains.clone()),
        sync: Some(domains.clone()),
        credentials: Some(domains.clone()),
        snapshots: Some(domains),
    });
    let app = router_with_control(
        token.clone(),
        Arc::new(RwLock::new(ServiceStatus::default())),
        "127.0.0.1:3764",
        EventHub::new(8),
        control,
    );
    let consent = serde_json::json!({"confirmed":true,"confirmationId":"test-consent"});
    assert_eq!(
        call(
            app.clone(),
            &token,
            "POST",
            "/control/v1/credentials/plan",
            serde_json::json!({"destinationIds":["api-token"]}),
        )
        .await["action"],
        "plan"
    );
    for (method, path, expected) in [
        ("POST", "/control/v1/sync/plan", "sync"),
        ("POST", "/control/v1/rollback", "sync"),
        ("POST", "/control/v1/credentials/apply", "credentials"),
        ("POST", "/control/v1/snapshots", "snapshots"),
        ("POST", "/control/v1/snapshots/restore", "snapshots"),
        ("POST", "/control/v1/snapshots/promote", "snapshots"),
    ] {
        assert_eq!(
            call(app.clone(), &token, method, path, consent.clone()).await["domain"],
            expected
        );
    }
    assert_eq!(
        call(
            app.clone(),
            &token,
            "POST",
            "/control/v1/verify",
            serde_json::json!({})
        )
        .await["action"],
        "verify"
    );
    assert_eq!(
        call(
            app.clone(),
            &token,
            "GET",
            "/control/v1/policy/summary",
            serde_json::json!({})
        )
        .await,
        serde_json::json!({"state":"valid","violations":[]})
    );
    assert_eq!(
        call(
            app,
            &token,
            "GET",
            "/control/v1/snapshots",
            serde_json::json!({})
        )
        .await["action"],
        "list"
    );
}
