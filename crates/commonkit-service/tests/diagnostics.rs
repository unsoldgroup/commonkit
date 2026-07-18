use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use commonkit_contracts::DiagnosticBundle;
use commonkit_service::{ControlToken, ServiceStatus, router};
use tokio::sync::RwLock;
use tower::ServiceExt;

#[tokio::test]
async fn exports_a_schema_bound_bundle_without_the_control_capability() {
    let token = ControlToken::generate();
    let application = router(
        token.clone(),
        Arc::new(RwLock::new(ServiceStatus::default())),
        "127.0.0.1:3764",
    );
    let response = application
        .oneshot(
            Request::get("/control/v1/diagnostics")
                .header(header::HOST, "127.0.0.1:3764")
                .header(
                    header::AUTHORIZATION,
                    format!("Bearer {}", token.expose_for_client()),
                )
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 128 * 1024)
        .await
        .expect("body");
    let bundle: DiagnosticBundle = serde_json::from_slice(&body).expect("bundle");
    assert_eq!(bundle.contract_version, "1.0");
    assert!(
        bundle
            .components
            .iter()
            .any(|component| component.id.as_str() == "daemon")
    );
    assert!(!String::from_utf8_lossy(&body).contains(token.expose_for_client()));
}
