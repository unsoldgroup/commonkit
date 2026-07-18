use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use commonkit_service::{ControlToken, ServiceStatus, router};
use serde_json::Value;
use tokio::sync::RwLock;
use tower::ServiceExt;

#[tokio::test]
async fn unavailable_v1_capabilities_return_stable_actionable_errors() {
    let token = ControlToken::generate();
    let application = router(
        token.clone(),
        Arc::new(RwLock::new(ServiceStatus::default())),
        "127.0.0.1:3764",
    );
    for (method, path) in [
        ("GET", "/control/v1/compose"),
        ("POST", "/control/v1/explain"),
    ] {
        let response = application
            .clone()
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
                    .body(Body::from("{}"))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED, "{path}");
        let body = to_bytes(response.into_body(), 8192).await.expect("body");
        let value: Value = serde_json::from_slice(&body).expect("JSON");
        assert_eq!(value["error"]["code"], "capability_unavailable");
        assert!(
            value["error"]["message"]
                .as_str()
                .unwrap()
                .contains("not implemented")
        );
        assert_eq!(value["error"]["retryable"], false);
    }
}
