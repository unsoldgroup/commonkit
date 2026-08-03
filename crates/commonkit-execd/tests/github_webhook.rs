//! A GitHub delivery is untrusted input that may contribute exactly one thing:
//! the revision to run the repository's own declared tasks against.
use axum::body::Body;
use axum::http::{Request, StatusCode};
use commonkit_contracts::ExecutionManifest;
use commonkit_execd::webhook::WebhookConfig;
use commonkit_execd::{ApiState, router};
use commonkit_execution::Scheduler;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::PathBuf;
use tower::ServiceExt;

const SECRET: &str = "webhook-secret";
const SHA: &str = "1111111111111111111111111111111111111111";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExecutionContextFile {
    tasks: BTreeMap<String, ExecutionManifest>,
}

fn tasks() -> BTreeMap<String, ExecutionManifest> {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../commonkit.execution-context.json");
    let file: ExecutionContextFile = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    file.tasks
}

fn state() -> ApiState {
    ApiState::new(Scheduler::open_memory().unwrap(), Vec::new(), false)
        .with_github_webhook(WebhookConfig::new(SECRET, tasks()))
}

fn hmac(key: &[u8], message: &[u8]) -> String {
    let mut inner_pad = [0x36_u8; 64];
    let mut outer_pad = [0x5c_u8; 64];
    for (index, byte) in key.iter().enumerate() {
        inner_pad[index] ^= byte;
        outer_pad[index] ^= byte;
    }
    let mut inner = Sha256::new();
    inner.update(inner_pad);
    inner.update(message);
    let digest = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(outer_pad);
    outer.update(digest);
    format!("{:x}", outer.finalize())
}

fn delivery(event: &str, body: &str, signature: Option<String>) -> Request<Body> {
    let mut request = Request::builder()
        .method("POST")
        .uri("/execution/v1/github/webhook")
        .header("x-github-event", event);
    let signature =
        signature.unwrap_or_else(|| format!("sha256={}", hmac(SECRET.as_bytes(), body.as_bytes())));
    request = request.header("x-hub-signature-256", signature);
    request.body(Body::from(body.to_owned())).unwrap()
}

fn push_body(sha: &str) -> String {
    format!(
        r#"{{"after":"{sha}","deleted":false,"repository":{{"full_name":"unsoldgroup/commonkit"}}}}"#
    )
}

async fn json(response: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn a_push_submits_every_declared_task_at_the_pushed_revision() {
    let response = router(state())
        .oneshot(delivery("push", &push_body(SHA), None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let body = json(response).await;
    assert_eq!(body["revision"], SHA);
    let submitted = body["submitted"].as_array().unwrap();
    assert_eq!(submitted.len(), tasks().len());
    assert!(submitted.iter().all(|entry| entry["created"] == true));
}

/// GitHub redelivers; a redelivery must not run the suite twice.
#[tokio::test]
async fn redelivery_of_the_same_revision_is_idempotent() {
    let state = state();
    let first = router(state.clone())
        .oneshot(delivery("push", &push_body(SHA), None))
        .await
        .unwrap();
    let first_ids: Vec<_> = json(first).await["submitted"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["jobId"].clone())
        .collect();
    let second = router(state)
        .oneshot(delivery("push", &push_body(SHA), None))
        .await
        .unwrap();
    let body = json(second).await;
    let submitted = body["submitted"].as_array().unwrap();
    assert!(submitted.iter().all(|entry| entry["created"] == false));
    let second_ids: Vec<_> = submitted
        .iter()
        .map(|entry| entry["jobId"].clone())
        .collect();
    assert_eq!(first_ids, second_ids);
}

#[tokio::test]
async fn an_unsigned_or_wrongly_signed_delivery_is_rejected_and_audited() {
    let state = state();
    let forged = router(state.clone())
        .oneshot(delivery(
            "push",
            &push_body(SHA),
            Some(format!("sha256={}", hmac(b"wrong-secret", b"anything"))),
        ))
        .await
        .unwrap();
    assert_eq!(forged.status(), StatusCode::UNAUTHORIZED);
    let entries = state.audit_entries().unwrap();
    assert_eq!(entries.len(), 1);
    assert!(!entries[0].allowed);
    assert!(!serde_json::to_string(&entries).unwrap().contains(SECRET));
}

/// The signature covers the body, so an attacker cannot keep a valid signature and
/// swap in a revision of their choosing.
#[tokio::test]
async fn a_signature_from_a_different_body_is_rejected() {
    let response = router(state())
        .oneshot(delivery(
            "push",
            &push_body("2222222222222222222222222222222222222222"),
            Some(format!(
                "sha256={}",
                hmac(SECRET.as_bytes(), push_body(SHA).as_bytes())
            )),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_pull_request_synchronize_submits_at_the_head_revision() {
    let body = format!(
        r#"{{"action":"synchronize","pull_request":{{"head":{{"sha":"{SHA}"}}}},"repository":{{"full_name":"unsoldgroup/commonkit"}}}}"#
    );
    let response = router(state())
        .oneshot(delivery("pull_request", &body, None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    assert_eq!(json(response).await["revision"], SHA);
}

/// A ping, a branch deletion, and a closed pull request are all healthy deliveries
/// that must be accepted without submitting anything.
#[tokio::test]
async fn deliveries_that_ask_for_nothing_are_accepted_and_run_nothing() {
    let deleted = r#"{"after":"0000000000000000000000000000000000000000","deleted":true,"repository":{"full_name":"unsoldgroup/commonkit"}}"#.to_owned();
    for (event, body) in [
        ("ping", r#"{"zen":"Anything added dilutes everything else."}"#.to_owned()),
        ("push", deleted),
        (
            "pull_request",
            r#"{"action":"closed","pull_request":{"head":{"sha":"1111111111111111111111111111111111111111"}},"repository":{"full_name":"unsoldgroup/commonkit"}}"#.to_owned(),
        ),
    ] {
        let response = router(state())
            .oneshot(delivery(event, &body, None))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{event}");
        assert!(json(response).await["submitted"].as_array().unwrap().is_empty());
    }
}

/// A delivery for a repository nothing is declared for must not run this
/// repository's tasks.
#[tokio::test]
async fn a_delivery_for_another_repository_submits_nothing() {
    let body = format!(
        r#"{{"after":"{SHA}","deleted":false,"repository":{{"full_name":"attacker/commonkit"}}}}"#
    );
    let response = router(state())
        .oneshot(delivery("push", &body, None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    assert!(
        json(response).await["submitted"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn the_route_is_absent_when_no_webhook_is_configured() {
    let state = ApiState::new(Scheduler::open_memory().unwrap(), Vec::new(), false);
    let response = router(state)
        .oneshot(delivery("push", &push_body(SHA), None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
