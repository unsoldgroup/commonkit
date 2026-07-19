#![cfg(unix)]
mod support;
use commonkit_execution::supervisor::{ProcessSupervisor, SupervisorMode};
use std::time::{Duration, Instant};

#[test]
fn cancellation_kills_the_process_group_and_retains_redacted_diagnostics() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir(directory.path().join("work")).unwrap();
    let mut manifest = support::manifest();
    manifest.repository_revision = support::init_repository(directory.path());
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

#[test]
fn rejects_a_stale_repository_revision_before_spawn() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir(directory.path().join("work")).unwrap();
    support::init_repository(directory.path());
    let manifest = support::manifest();
    let supervisor = ProcessSupervisor::new(
        SupervisorMode::ProcessGroup,
        directory.path().join("diagnostics"),
    )
    .unwrap();
    let error = match supervisor.spawn(&manifest, directory.path()) {
        Err(error) => error,
        Ok(_) => panic!("stale revision was accepted"),
    };
    assert!(matches!(
        error,
        commonkit_execution::supervisor::SupervisorError::RepositoryRevisionMismatch
    ));
}

#[test]
fn rejects_a_workspace_materialized_from_the_wrong_repository() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir(directory.path().join("work")).unwrap();
    let mut manifest = support::manifest();
    manifest.repository_revision = support::init_repository(directory.path());
    manifest.repository = "https://example.invalid/other.git".into();
    let supervisor = ProcessSupervisor::new(
        SupervisorMode::ProcessGroup,
        directory.path().join("diagnostics"),
    )
    .unwrap();
    let error = match supervisor.spawn(&manifest, directory.path()) {
        Err(error) => error,
        Ok(_) => panic!("workspace from wrong repository was accepted"),
    };
    assert!(matches!(
        error,
        commonkit_execution::supervisor::SupervisorError::RepositoryMismatch
    ));
}

#[test]
fn rejects_a_workspace_bundle_changed_after_materialization() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir(directory.path().join("work")).unwrap();
    std::fs::write(directory.path().join("work/input"), "before").unwrap();
    let mut manifest = support::manifest();
    manifest.repository_revision = support::init_repository(directory.path());
    manifest.workspace_bundle_digest = Some(support::digest('d'));
    let supervisor = ProcessSupervisor::new(
        SupervisorMode::ProcessGroup,
        directory.path().join("diagnostics"),
    )
    .unwrap();
    let error = match supervisor.spawn(&manifest, directory.path()) {
        Err(error) => error,
        Ok(_) => panic!("changed bundle was accepted"),
    };
    assert!(matches!(
        error,
        commonkit_execution::supervisor::SupervisorError::WorkspaceBundleMismatch
    ));
}

#[test]
fn timeout_hard_kills_descendants_and_leaves_no_late_output() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir(directory.path().join("work")).unwrap();
    let marker = directory.path().join("late-marker");
    let mut manifest = support::manifest();
    manifest.repository_revision = support::init_repository(directory.path());
    manifest.timeout_seconds = 1;
    manifest.argv = vec![
        "sh".into(),
        "-c".into(),
        format!("(sleep 2; touch {}) & wait", marker.display()),
    ];
    let supervisor = ProcessSupervisor::new(
        SupervisorMode::ProcessGroup,
        directory.path().join("diagnostics"),
    )
    .unwrap();
    let mut process = supervisor.spawn(&manifest, directory.path()).unwrap();
    let error = loop {
        match process.try_wait() {
            Err(error) => break error,
            Ok(Some(_)) => panic!("process unexpectedly completed"),
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
        }
    };
    assert!(matches!(
        error,
        commonkit_execution::supervisor::SupervisorError::TimedOut
    ));
    std::thread::sleep(Duration::from_millis(1200));
    assert!(!marker.exists(), "timed-out descendant survived");
}

#[test]
fn disk_limit_prevents_workspace_allocation() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir(directory.path().join("work")).unwrap();
    let mut manifest = support::manifest();
    manifest.repository_revision = support::init_repository(directory.path());
    manifest.resources.disk_mib = 0;
    manifest.argv = vec![
        "sh".into(),
        "-c".into(),
        "! (printf x > work/output) 2>/dev/null".into(),
    ];
    let supervisor = ProcessSupervisor::new(
        SupervisorMode::ProcessGroup,
        directory.path().join("diagnostics"),
    )
    .unwrap();
    let outcome = supervisor
        .spawn(&manifest, directory.path())
        .unwrap()
        .wait(&[])
        .unwrap();
    assert!(
        outcome.status.success(),
        "disk allocation unexpectedly succeeded"
    );
    let allocated = std::fs::metadata(directory.path().join("work/output"))
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    assert_eq!(allocated, 0);
}

#[cfg(target_os = "linux")]
#[test]
fn deny_network_runs_without_a_network_interface() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir(directory.path().join("work")).unwrap();
    let mut manifest = support::manifest();
    manifest.repository_revision = support::init_repository(directory.path());
    manifest.argv = vec![
        "python3".into(), "-c".into(),
        "import socket,sys\ns=socket.socket();s.settimeout(.2)\ntry:s.connect(('1.1.1.1',53));sys.exit(1)\nexcept OSError:sys.exit(0)".into(),
    ];
    let supervisor = ProcessSupervisor::new(
        SupervisorMode::LinuxSandbox,
        directory.path().join("diagnostics"),
    )
    .unwrap();
    let outcome = supervisor
        .spawn(&manifest, directory.path())
        .unwrap()
        .wait(&[])
        .unwrap();
    assert!(
        outcome.status.success(),
        "network namespace was not isolated: {}",
        outcome.stderr
    );
}

#[cfg(target_os = "linux")]
#[test]
fn restricted_network_fails_closed_without_destination_rules() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir(directory.path().join("work")).unwrap();
    let mut manifest = support::manifest();
    manifest.repository_revision = support::init_repository(directory.path());
    manifest.network_policy = commonkit_contracts::NetworkPolicy::Restricted;
    let supervisor = ProcessSupervisor::new(
        SupervisorMode::LinuxSandbox,
        directory.path().join("diagnostics"),
    )
    .unwrap();
    let error = match supervisor.spawn(&manifest, directory.path()) {
        Err(error) => error,
        Ok(_) => panic!("restricted policy ran without destination enforcement"),
    };
    assert!(matches!(
        error,
        commonkit_execution::supervisor::SupervisorError::UnsupportedNetworkPolicy
    ));
}

#[cfg(target_os = "linux")]
#[test]
fn read_only_repository_cannot_be_modified() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir(directory.path().join("work")).unwrap();
    let mut manifest = support::manifest();
    manifest.repository_revision = support::init_repository(directory.path());
    manifest.argv = vec![
        "sh".into(),
        "-c".into(),
        "! printf changed > work/forbidden".into(),
    ];
    let supervisor = ProcessSupervisor::new(
        SupervisorMode::LinuxSandbox,
        directory.path().join("diagnostics"),
    )
    .unwrap();
    let outcome = supervisor
        .spawn(&manifest, directory.path())
        .unwrap()
        .wait(&[])
        .unwrap();
    assert!(
        outcome.status.success(),
        "repository write unexpectedly succeeded: {}",
        outcome.stderr
    );
    assert!(!directory.path().join("work/forbidden").exists());
}

#[cfg(not(target_os = "linux"))]
#[test]
fn production_isolation_fails_closed_off_linux() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir(directory.path().join("work")).unwrap();
    let mut manifest = support::manifest();
    manifest.repository_revision = support::init_repository(directory.path());
    let supervisor = ProcessSupervisor::new(
        SupervisorMode::SystemdScope,
        directory.path().join("diagnostics"),
    )
    .unwrap();
    let error = match supervisor.spawn(&manifest, directory.path()) {
        Err(error) => error,
        Ok(_) => panic!("production execution ran without supported isolation"),
    };
    assert!(matches!(
        error,
        commonkit_execution::supervisor::SupervisorError::UnsupportedIsolation
    ));
}
