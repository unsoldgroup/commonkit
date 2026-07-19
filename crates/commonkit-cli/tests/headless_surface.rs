use std::process::Command;

fn command(arguments: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_commonkit"))
        .args(arguments)
        .output()
        .expect("commonkit command")
}

#[test]
fn daemon_composition_and_durable_diff_have_actionable_absent_daemon_errors() {
    for arguments in [
        vec!["compose"],
        vec!["explain", "/theme"],
        vec![
            "diff",
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ],
    ] {
        let output = command(&arguments);
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("daemon_unavailable"));
    }
}

#[test]
fn credential_apply_requires_consent_before_contacting_daemon() {
    let denied = command(&["credentials", "apply", "api-token"]);
    assert!(!denied.status.success());
    assert!(String::from_utf8_lossy(&denied.stderr).contains("confirmation_required"));

    for arguments in [
        vec!["credentials", "readiness", "env://COMMONKIT_TOKEN"],
        vec!["credentials", "verify", "api-token"],
    ] {
        let output = command(&arguments);
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("daemon_unavailable"));
    }
}

#[test]
fn snapshot_and_drift_schedule_mutations_require_explicit_consent() {
    for arguments in [
        vec!["snapshots", "create", "context-mode"],
        vec!["snapshots", "restore", "snapshot-1"],
        vec!["snapshots", "promote", "context-mode", "workstation-b"],
        vec!["schedule", "enable", "--interval-seconds", "60"],
        vec!["schedule", "disable"],
    ] {
        let output = command(&arguments);
        assert!(!output.status.success(), "{arguments:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("confirmation_required"),
            "{arguments:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    for arguments in [
        vec!["snapshots", "list"],
        vec!["snapshots", "create", "context-mode", "--confirmed"],
        vec![
            "schedule",
            "enable",
            "--interval-seconds",
            "60",
            "--confirmed",
        ],
    ] {
        let output = command(&arguments);
        assert!(!output.status.success(), "daemon is intentionally absent");
        assert!(String::from_utf8_lossy(&output.stderr).contains("daemon_unavailable"));
    }
}

#[test]
fn skill_canary_mutations_require_operator_confirmation_before_daemon_contact() {
    for arguments in [
        vec![
            "skills",
            "canary-apply",
            "--deployment",
            "missing.json",
            "--run-id",
            "run-1",
        ],
        vec![
            "skills",
            "canary-rollback",
            "--run-id",
            "run-1",
            "--deployment-receipt-id",
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ],
    ] {
        let output = command(&arguments);
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("confirmation_required"));
    }
}
