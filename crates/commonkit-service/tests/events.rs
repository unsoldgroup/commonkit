use axum::{
    body::{Body, to_bytes},
    http::{Request, header},
};
use commonkit_service::{ControlToken, EventHub, ServiceStatus, router_with_events};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::RwLock;
use tower::ServiceExt;

#[test]
fn replays_events_after_a_cursor_and_signals_expired_history() {
    let events = EventHub::new(2);
    assert_eq!(
        events
            .publish("status.changed", json!({"state": "healthy"}))
            .id,
        1
    );
    assert_eq!(
        events.publish("operation.updated", json!({"id": "one"})).id,
        2
    );

    let replay = events.replay_after(Some(1));
    assert!(!replay.expired);
    assert_eq!(replay.events.len(), 1);
    assert_eq!(replay.events[0].id, 2);

    events.publish("operation.updated", json!({"id": "two"}));
    let expired = events.replay_after(Some(0));
    assert!(expired.expired);
    assert_eq!(
        expired
            .events
            .iter()
            .map(|event| event.id)
            .collect::<Vec<_>>(),
        [2, 3]
    );
}

#[tokio::test]
async fn authenticated_snapshot_exposes_the_shared_event_cursor_without_holding_a_stream() {
    let token = ControlToken::generate();
    let events = EventHub::new(4);
    events.publish("operation.updated", json!({"state":"running"}));
    let app = router_with_events(
        token.clone(),
        Arc::new(RwLock::new(ServiceStatus::default())),
        "127.0.0.1:3764",
        events,
    );
    let response = app
        .oneshot(
            Request::get("/control/v1/events/snapshot")
                .header(header::HOST, "127.0.0.1:3764")
                .header(
                    header::AUTHORIZATION,
                    format!("Bearer {}", token.expose_for_client()),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["lastEventId"], 1);
    assert_eq!(value["events"][0]["event"], "operation.updated");
}

#[test]
fn a_fresh_subscriber_receives_the_bounded_snapshot() {
    let events = EventHub::new(2);
    events.publish("one", json!({}));
    events.publish("two", json!({}));
    events.publish("three", json!({}));
    assert_eq!(
        events
            .replay_after(None)
            .events
            .iter()
            .map(|event| event.event.as_str())
            .collect::<Vec<_>>(),
        ["two", "three"]
    );
}
