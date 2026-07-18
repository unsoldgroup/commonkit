use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use commonkit_service::{
    DriftChecker, DriftResult, DriftScheduler, EventHub, OverallState, ServiceStatus,
};
use tokio::sync::RwLock;

struct ReadOnlyCheck {
    calls: AtomicUsize,
    result: DriftResult,
}

impl DriftChecker for ReadOnlyCheck {
    fn check(&self) -> DriftResult {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.result.clone()
    }
}

#[tokio::test]
async fn records_drift_without_invoking_any_mutation_surface() {
    let checker = Arc::new(ReadOnlyCheck {
        calls: AtomicUsize::new(0),
        result: DriftResult {
            state: OverallState::Drifted,
            code: None,
        },
    });
    let status = Arc::new(RwLock::new(ServiceStatus::default()));
    let events = EventHub::new(4);
    let scheduler = DriftScheduler::new(checker.clone(), status.clone(), events.clone());

    assert_eq!(scheduler.check_now().await.state, OverallState::Drifted);
    assert_eq!(checker.calls.load(Ordering::SeqCst), 1);
    assert_eq!(status.read().await.state, OverallState::Drifted);
    assert!(status.read().await.last_drift_check_unix_ms.is_some());
    assert_eq!(events.replay_after(None).events[0].event, "drift.checked");
}

#[tokio::test]
async fn surfaces_check_failures_as_degraded_status_with_a_stable_code() {
    let checker = Arc::new(ReadOnlyCheck {
        calls: AtomicUsize::new(0),
        result: DriftResult {
            state: OverallState::Degraded,
            code: Some("layer_unavailable".into()),
        },
    });
    let status = Arc::new(RwLock::new(ServiceStatus::default()));
    let scheduler = DriftScheduler::new(checker, status.clone(), EventHub::new(4));
    scheduler.check_now().await;
    assert_eq!(
        status.read().await.last_drift_error_code.as_deref(),
        Some("layer_unavailable")
    );
}
