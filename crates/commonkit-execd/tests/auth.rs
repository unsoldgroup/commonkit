use axum::body::Body;
use axum::http::{Request, StatusCode};
use commonkit_execd::{ApiState, Capability, router};
use commonkit_execution::Scheduler;
use std::collections::BTreeSet;
use tower::ServiceExt;

#[tokio::test]
async fn unauthorized_operations_are_denied_and_audited_without_tokens() {
    let state = ApiState::new(
        Scheduler::open_memory().unwrap(),
        vec![(
            "read-token".into(),
            "reader".into(),
            BTreeSet::from([Capability::Read]),
        )],
        false,
    );
    let response = router(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/execution/v1/jobs/missing/cancel")
                .header("authorization", "Bearer read-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let entries = state.audit_entries().unwrap();
    assert_eq!(entries.len(), 1);
    assert!(!entries[0].allowed);
    assert_eq!(entries[0].identity, "reader");
    let serialized = serde_json::to_string(&entries).unwrap();
    assert!(!serialized.contains("read-token"));
}

#[tokio::test]
async fn remote_api_requires_tls_termination_signal() {
    let state = ApiState::new(
        Scheduler::open_memory().unwrap(),
        vec![(
            "token".into(),
            "reader".into(),
            BTreeSet::from([Capability::Read]),
        )],
        true,
    );
    let plain = router(state.clone())
        .oneshot(
            Request::builder()
                .uri("/execution/v1/jobs/missing")
                .header("authorization", "Bearer token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(plain.status(), StatusCode::FORBIDDEN);
    let tls = router(state)
        .oneshot(
            Request::builder()
                .uri("/execution/v1/jobs/missing")
                .header("authorization", "Bearer token")
                .header("x-forwarded-proto", "https")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(tls.status(), StatusCode::NOT_FOUND);
}
