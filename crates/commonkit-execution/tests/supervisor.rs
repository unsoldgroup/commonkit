#![cfg(unix)]
mod support;
use commonkit_execution::supervisor::{ProcessSupervisor, SupervisorMode};
use std::time::{Duration, Instant};

#[test]
fn cancellation_kills_the_process_group_and_retains_redacted_diagnostics() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir(directory.path().join("work")).unwrap();
    let mut manifest = support::manifest();
    manifest.argv = vec![
        "sh".into(),
        "-c".into(),
        "echo super-secret; sleep 30 & wait".into(),
    ];
    let supervisor = ProcessSupervisor::new(
        SupervisorMode::ProcessGroup,
        directory.path().join("diagnostics"),
    )
    .unwrap();
    let process = supervisor.spawn(&manifest, directory.path()).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    let started = Instant::now();
    let outcome = process
        .cancel(Duration::from_millis(100), &["super-secret".into()])
        .unwrap();
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(!outcome.status.success());
    assert!(outcome.stdout.contains("[REDACTED]"));
    assert!(!outcome.stdout.contains("super-secret"));
}
