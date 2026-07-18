use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use commonkit_service::{ControlToken, ServiceStatus, router};
use tokio::sync::RwLock;
use tower::ServiceExt;

#[tokio::test]
async fn requires_the_installation_token_and_rejects_browser_origins() {
    let token = ControlToken::generate();
    let application = router(
        token.clone(),
        Arc::new(RwLock::new(ServiceStatus::default())),
        "127.0.0.1:3764",
    );

    let unauthorized = application
        .clone()
        .oneshot(
            Request::get("/control/v1/status")
                .header(header::HOST, "127.0.0.1:3764")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

    let authorized = application
        .clone()
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
    assert_eq!(authorized.status(), StatusCode::OK);

    let browser = application
        .oneshot(
            Request::get("/control/v1/status")
                .header(header::HOST, "127.0.0.1:3764")
                .header(
                    header::AUTHORIZATION,
                    format!("Bearer {}", token.expose_for_client()),
                )
                .header(header::ORIGIN, "https://attacker.example")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(browser.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn health_is_authenticated_too() {
    let token = ControlToken::generate();
    let application = router(
        token.clone(),
        Arc::new(RwLock::new(ServiceStatus::default())),
        "127.0.0.1:3764",
    );
    let response = application
        .oneshot(
            Request::get("/control/v1/health")
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
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn rejects_dns_rebinding_host_headers() {
    let token = ControlToken::generate();
    let application = router(
        token.clone(),
        Arc::new(RwLock::new(ServiceStatus::default())),
        "127.0.0.1:3764",
    );
    let response = application
        .oneshot(
            Request::get("/control/v1/status")
                .header(header::HOST, "attacker.example")
                .header(
                    header::AUTHORIZATION,
                    format!("Bearer {}", token.expose_for_client()),
                )
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::MISDIRECTED_REQUEST);
}
