use std::path::PathBuf;

use commonkit_cli::daemon_lifecycle::{DaemonBackend, DaemonService};

fn service(backend: DaemonBackend, executable: &str) -> DaemonService {
    DaemonService::configured(
        backend,
        PathBuf::from("/tmp/commonkit-service-test"),
        PathBuf::from(executable),
    )
    .unwrap()
}

#[test]
fn platform_definitions_use_fixed_executable_fields_without_shells() {
    let executable = "/Applications/CommonKit & Tools/commonkitd";
    let launchd = service(DaemonBackend::Launchd, executable).render_definition();
    let systemd = service(DaemonBackend::SystemdUser, executable).render_definition();
    let windows = service(DaemonBackend::WindowsTask, executable).render_definition();

    assert!(launchd.contains("<key>ProgramArguments</key>"));
    assert!(launchd.contains("CommonKit &amp; Tools/commonkitd"));
    assert!(systemd.contains("ExecStart=\"/Applications/CommonKit & Tools/commonkitd\""));
    assert!(windows.contains("<Command>/Applications/CommonKit &amp; Tools/commonkitd</Command>"));
    for definition in [&launchd, &systemd, &windows] {
        assert!(!definition.contains("sh -c"));
        assert!(!definition.contains("cmd.exe"));
    }
}

#[test]
fn fallback_definition_contains_only_a_json_encoded_absolute_executable() {
    let definition =
        service(DaemonBackend::ProcessFallback, "/opt/commonkit/commonkitd").render_definition();
    let value: serde_json::Value = serde_json::from_str(&definition).unwrap();
    assert_eq!(
        value,
        serde_json::json!({"executable":"/opt/commonkit/commonkitd"})
    );
}

#[cfg(unix)]
#[test]
fn service_install_rejects_a_symlinked_definition() {
    use std::os::unix::fs::symlink;

    let temporary = tempfile::tempdir().unwrap();
    let executable = temporary.path().join("commonkitd");
    std::fs::write(&executable, b"binary").unwrap();
    let service = DaemonService::configured(
        DaemonBackend::ProcessFallback,
        temporary.path().to_path_buf(),
        executable,
    )
    .unwrap();
    let victim = temporary.path().join("victim");
    std::fs::write(&victim, b"do not replace").unwrap();
    symlink(&victim, service.definition_path()).unwrap();

    assert!(service.install().is_err());
    assert_eq!(std::fs::read(&victim).unwrap(), b"do not replace");
}
