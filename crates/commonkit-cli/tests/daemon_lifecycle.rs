use std::path::PathBuf;
use std::process::{Command, Stdio};

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
fn windows_task_definition_declares_the_encoding_of_the_written_bytes() {
    let definition = service(
        DaemonBackend::WindowsTask,
        "/Applications/CommonKit é/commonkitd",
    )
    .render_definition();
    let bytes = definition.as_bytes();

    assert!(std::str::from_utf8(bytes).is_ok());
    assert!(definition.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
    assert!(!definition.contains("encoding=\"UTF-16\""));
}

#[test]
fn windows_task_status_probe_is_numeric_and_does_not_parse_localized_output() {
    let probe = commonkit_cli::daemon_lifecycle::windows_task_status_probe();

    assert_eq!(probe.program, "powershell.exe");
    assert!(
        probe
            .arguments
            .iter()
            .any(|argument| argument.contains("[int]$task.State -eq 4"))
    );
    for localized_word in ["running", "en cours", "wird ausgeführt", "ejecutando"] {
        assert!(
            !probe
                .arguments
                .iter()
                .any(|argument| argument.to_ascii_lowercase().contains(localized_word))
        );
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

#[cfg(unix)]
#[test]
fn fallback_refuses_same_executable_with_a_reused_pid_birth_token() {
    let yes = if PathBuf::from("/usr/bin/yes").is_file() {
        PathBuf::from("/usr/bin/yes")
    } else {
        PathBuf::from("/bin/yes")
    };
    let temporary = tempfile::tempdir().unwrap();
    let service = DaemonService::configured(
        DaemonBackend::ProcessFallback,
        temporary.path().to_path_buf(),
        yes.clone(),
    )
    .unwrap();
    service.install().unwrap();
    service.start().unwrap();

    let pid_path = temporary.path().join("commonkitd.pid");
    let original = std::fs::read_to_string(&pid_path).unwrap();
    let original_value: serde_json::Value = serde_json::from_str(&original).unwrap();
    assert!(original_value["startToken"].as_str().is_some());
    assert_eq!(
        original_value["serviceRoot"].as_str(),
        temporary.path().canonicalize().unwrap().to_str()
    );
    let original_pid = original_value["pid"].as_u64().unwrap().to_string();

    let other_root = temporary.path().join("other-instance");
    let other_service = DaemonService::configured(
        DaemonBackend::ProcessFallback,
        other_root.clone(),
        yes.clone(),
    )
    .unwrap();
    other_service.install().unwrap();
    std::fs::write(other_root.join("commonkitd.pid"), &original).unwrap();
    assert!(
        !other_service.status().unwrap().running,
        "a process record copied from another service root must not match"
    );
    other_service.uninstall().unwrap();

    let mut unrelated = Command::new(&yes)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut forged = original_value;
    forged["pid"] = serde_json::json!(unrelated.id());
    std::fs::write(&pid_path, serde_json::to_vec(&forged).unwrap()).unwrap();

    let status = service.status().unwrap();
    assert!(
        !status.running,
        "a same-binary PID reuse must not match the old birth token"
    );
    service.uninstall().unwrap();
    assert!(
        unrelated.try_wait().unwrap().is_none(),
        "CommonKit must not signal the unrelated process"
    );

    let _ = Command::new("kill").args(["-TERM", &original_pid]).status();
    let _ = unrelated.kill();
    let _ = unrelated.wait();
}
