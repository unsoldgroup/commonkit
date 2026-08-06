use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::http::{Request, header};
use commonkit_contracts::StableId;
use commonkit_service::{
    ApplyStatus, CompositionDomain, ControlPlane, ControlToken, DomainFailure, EventHub,
    ExecutionResult, HeadlessDomainRegistry, PlanExecutor, ServiceStatus, router_with_control,
};
use serde_json::{Value, json};
use tokio::sync::RwLock;
use tower::ServiceExt;

struct UnusedExecutor;

impl PlanExecutor for UnusedExecutor {
    fn execute(&self, _: &commonkit_contracts::Plan, _: &StableId) -> ExecutionResult {
        ExecutionResult {
            status: ApplyStatus::Failed,
            failure_code: None,
        }
    }
}

struct CompositionFixture;

impl CompositionDomain for CompositionFixture {
    fn compose(&self) -> Result<Value, DomainFailure> {
        Ok(
            json!({"spec":{"contextBudget":{"maxTotalTokens":80}},"specDigest":format!("sha256:{}", "1".repeat(64))}),
        )
    }

    fn explain(&self, pointer: &str) -> Result<Value, DomainFailure> {
        assert_eq!(pointer, "/contextBudget/maxTotalTokens");
        Ok(json!({"pointer":pointer,"winner":{"layerId":"personal"}}))
    }
}

async fn call(
    app: axum::Router,
    token: &ControlToken,
    method: &str,
    path: &str,
    body: Value,
) -> Value {
    let response = app
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
async fn configured_composition_domain_serves_composed_state_and_provenance() {
    let token = ControlToken::generate();
    let control = ControlPlane::new(Arc::new(UnusedExecutor));
    control.set_headless_domains(HeadlessDomainRegistry {
        composition: Some(Arc::new(CompositionFixture)),
        ..HeadlessDomainRegistry::default()
    });
    let app = router_with_control(
        token.clone(),
        Arc::new(RwLock::new(ServiceStatus::default())),
        "127.0.0.1:3764",
        EventHub::new(8),
        control,
    );

    let composed = call(app.clone(), &token, "GET", "/control/v1/compose", json!({})).await;
    assert_eq!(composed["spec"]["contextBudget"]["maxTotalTokens"], 80);
    let explained = call(
        app,
        &token,
        "POST",
        "/control/v1/explain",
        json!({"pointer":"/contextBudget/maxTotalTokens"}),
    )
    .await;
    assert_eq!(explained["winner"]["layerId"], "personal");
}
