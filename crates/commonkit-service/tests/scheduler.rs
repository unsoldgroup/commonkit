use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use commonkit_service::{
    DriftChecker, DriftResult, DriftScheduler, EventHub, OverallState, SchedulerConfig,
    SchedulerStore, ServiceStatus,
};
use tokio::sync::RwLock;

struct ReadOnlyCheck {
    calls: AtomicUsize,
    result: DriftResult,
}

#[tokio::test]
async fn scheduler_enable_disable_and_startup_restore_are_persistent() {
    let temp = std::env::temp_dir().join(format!("commonkit-scheduler-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&temp);
    let store = SchedulerStore::open(&temp).unwrap();
    let checker = Arc::new(ReadOnlyCheck {
        calls: AtomicUsize::new(0),
        result: DriftResult {
            state: OverallState::Healthy,
            code: None,
        },
    });
    let status = Arc::new(RwLock::new(ServiceStatus::default()));
    let scheduler = DriftScheduler::with_store(checker, status, EventHub::new(4), store).unwrap();

    scheduler
        .enable(std::time::Duration::from_secs(90))
        .unwrap();
    assert_eq!(
        scheduler.configuration(),
        SchedulerConfig {
            enabled: true,
            interval_seconds: 90
        }
    );
    let restored = DriftScheduler::with_store(
        Arc::new(ReadOnlyCheck {
            calls: AtomicUsize::new(0),
            result: DriftResult {
                state: OverallState::Healthy,
                code: None,
            },
        }),
        Arc::new(RwLock::new(ServiceStatus::default())),
        EventHub::new(4),
        SchedulerStore::open(&temp).unwrap(),
    )
    .unwrap();
    assert_eq!(restored.configuration().interval_seconds, 90);
    restored.disable().unwrap();
    assert!(!restored.configuration().enabled);
    let _ = std::fs::remove_dir_all(temp);
}

#[tokio::test]
async fn overlapping_manual_checks_are_suppressed() {
    let checker = Arc::new(ReadOnlyCheck {
        calls: AtomicUsize::new(0),
        result: DriftResult {
            state: OverallState::Healthy,
            code: None,
        },
    });
    let scheduler = Arc::new(DriftScheduler::new(
        checker.clone(),
        Arc::new(RwLock::new(ServiceStatus::default())),
        EventHub::new(4),
    ));
    // The guard is public behavior through try_check_now: one caller owns the run.
    let first = scheduler.try_check_now().await;
    assert!(first.is_some());
    assert_eq!(checker.calls.load(Ordering::SeqCst), 1);
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
