use commonkit_service::EventHub;
use serde_json::json;

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
