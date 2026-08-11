use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use commonkit_service::{ControlToken, OverallState, ServiceStatus, router};
use serde_json::Value;
use tokio::sync::RwLock;
use tower::ServiceExt;

async fn get_panel(status: ServiceStatus) -> (StatusCode, Value) {
    let token = ControlToken::generate();
    let application = router(
        token.clone(),
        Arc::new(RwLock::new(status)),
        "127.0.0.1:3764",
    );
    let response = application
        .oneshot(
            Request::get("/control/v1/panel")
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
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let body = serde_json::from_slice(&bytes).expect("json");
    (status, body)
}

async fn get_status(status: ServiceStatus) -> Value {
    let token = ControlToken::generate();
    let application = router(
        token.clone(),
        Arc::new(RwLock::new(status)),
        "127.0.0.1:3764",
    );
    let response = application
        .oneshot(
            Request::get("/control/v1/status")
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
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    serde_json::from_slice(&bytes).expect("json")
}

#[tokio::test]
async fn panel_distinguishes_unchecked_empty_and_unavailable_channels() {
    let (status, body) = get_panel(ServiceStatus {
        state: OverallState::Healthy,
        last_drift_check_unix_ms: None,
        ..ServiceStatus::default()
    })
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["contractVersion"], "commonkit.panel/v1");
    assert_eq!(body["channels"]["drift"]["state"], "unchecked");
    assert_eq!(body["channels"]["drift"]["reason"], "never_checked");
    assert_eq!(body["channels"]["changes"]["state"], "empty");
    assert_eq!(body["channels"]["credentials"]["state"], "empty");
    assert_eq!(body["channels"]["agentSessions"]["state"], "unavailable");
    assert_eq!(
        body["channels"]["agentSessions"]["reason"],
        "session_source_unconfigured"
    );
    assert_eq!(body["channels"]["skills"]["state"], "unavailable");
}

#[tokio::test]
async fn status_exposes_immutable_runtime_identity() {
    let body = get_status(ServiceStatus::default()).await;

    assert_eq!(body["panelContractVersion"], "commonkit.panel/v1");
    assert!(
        body["buildRevision"]
            .as_str()
            .is_some_and(|value| !value.is_empty())
    );
    assert!(
        body["daemonBinarySha256"]
            .as_str()
            .is_some_and(|value| value.starts_with("sha256:") && value.len() == 71)
    );
}
