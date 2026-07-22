use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use commonkit_contracts::{Plan, StableId};
use commonkit_service::{
    ApplyStatus, ControlPlane, ControlToken, DriftChecker, DriftResult, DriftScheduler, EventHub,
    ExecutionResult, OverallState, PlanExecutor, SchedulerConfig, SchedulerStore, ServiceStatus,
    router_with_control,
};
use tokio::sync::RwLock;
use tower::ServiceExt;

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
async fn control_api_configuration_change_wakes_the_production_wired_scheduler() {
    let temp = tempfile::tempdir().unwrap();
    let store = SchedulerStore::open(temp.path()).unwrap();
    let checker = Arc::new(ReadOnlyCheck {
        calls: AtomicUsize::new(0),
        result: DriftResult {
            state: OverallState::Healthy,
            code: None,
        },
    });
    let scheduler = Arc::new(
        DriftScheduler::with_store(
            checker.clone(),
            Arc::new(RwLock::new(ServiceStatus::default())),
            EventHub::new(4),
            store.clone(),
        )
        .unwrap(),
    );
    scheduler
        .enable(std::time::Duration::from_secs(60))
        .unwrap();

    let control = ControlPlane::new(Arc::new(UnusedExecutor));
    control.set_scheduler_store(Arc::new(store));
    let token = ControlToken::generate();
    let app = router_with_control(
        token.clone(),
        Arc::new(RwLock::new(ServiceStatus::default())),
        "127.0.0.1:3764",
        EventHub::new(4),
        control,
    );

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let running = scheduler.clone();
    let task = tokio::spawn(async move {
        running
            .run_until(std::time::Duration::from_secs(60), async move {
                let _ = shutdown_rx.await;
            })
            .await;
    });
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while checker.calls.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("enabled scheduler should perform its startup read-only check");
    let calls_before_interval_change = checker.calls.load(Ordering::SeqCst);

    let response = app
        .clone()
        .oneshot(
            Request::post("/control/v1/schedule")
                .header(header::HOST, "127.0.0.1:3764")
                .header(
                    header::AUTHORIZATION,
                    format!("Bearer {}", token.expose_for_client()),
                )
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"enabled":true,"intervalSeconds":1,"confirmed":true,"confirmationId":"scheduler-change"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while checker.calls.load(Ordering::SeqCst) == calls_before_interval_change {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("running scheduler should adopt the persisted interval change");

    let calls_after_enabled_tick = checker.calls.load(Ordering::SeqCst);
    let response = app
        .oneshot(
            Request::post("/control/v1/schedule")
                .header(header::HOST, "127.0.0.1:3764")
                .header(
                    header::AUTHORIZATION,
                    format!("Bearer {}", token.expose_for_client()),
                )
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"enabled":false,"confirmed":true,"confirmationId":"scheduler-disable"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    tokio::time::sleep(std::time::Duration::from_millis(1_200)).await;
    assert_eq!(
        checker.calls.load(Ordering::SeqCst),
        calls_after_enabled_tick,
        "persisted disable should stop the running scheduler"
    );
    shutdown_tx.send(()).unwrap();
    task.await.unwrap();
}

#[test]
fn cloned_scheduler_stores_can_save_concurrently() {
    let temp = tempfile::tempdir().unwrap();
    let store = SchedulerStore::open(temp.path()).unwrap();
    let stores = (0..16).map(|_| store.clone()).collect::<Vec<_>>();
    let barrier = Arc::new(std::sync::Barrier::new(stores.len()));
    let saves = stores
        .into_iter()
        .enumerate()
        .map(|(index, store)| {
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let scheduler = DriftScheduler::with_store(
                    Arc::new(ReadOnlyCheck {
                        calls: AtomicUsize::new(0),
                        result: DriftResult {
                            state: OverallState::Healthy,
                            code: None,
                        },
                    }),
                    Arc::new(RwLock::new(ServiceStatus::default())),
                    EventHub::new(1),
                    store,
                )
                .unwrap();
                barrier.wait();
                scheduler.enable(std::time::Duration::from_secs(index as u64 + 1))
            })
        })
        .collect::<Vec<_>>();

    for save in saves {
        save.join().unwrap().unwrap();
    }
    SchedulerStore::open(temp.path()).unwrap();
}

struct UnusedExecutor;

impl PlanExecutor for UnusedExecutor {
    fn execute(&self, _: &Plan, _: &StableId) -> ExecutionResult {
        ExecutionResult {
            status: ApplyStatus::Failed,
            failure_code: None,
        }
    }
}

struct BlockingCheck {
    calls: AtomicUsize,
    entered: std::sync::mpsc::Sender<()>,
    release: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
}

impl DriftChecker for BlockingCheck {
    fn check(&self) -> DriftResult {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.entered.send(()).unwrap();
        self.release
            .lock()
            .unwrap()
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("test should release the in-flight check");
        DriftResult {
            state: OverallState::Healthy,
            code: None,
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn overlapping_manual_checks_are_suppressed() {
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let checker = Arc::new(BlockingCheck {
        calls: AtomicUsize::new(0),
        entered: entered_tx,
        release: std::sync::Mutex::new(release_rx),
    });
    let scheduler = Arc::new(DriftScheduler::new(
        checker.clone(),
        Arc::new(RwLock::new(ServiceStatus::default())),
        EventHub::new(4),
    ));

    let first_scheduler = scheduler.clone();
    let first = tokio::spawn(async move { first_scheduler.try_check_now().await });
    entered_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("first check should be in flight before the overlap attempt");

    assert!(scheduler.try_check_now().await.is_none());
    assert_eq!(checker.calls.load(Ordering::SeqCst), 1);
    release_tx.send(()).unwrap();
    assert!(first.await.unwrap().is_some());
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
