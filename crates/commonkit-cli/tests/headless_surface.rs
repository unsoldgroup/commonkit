use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn command(arguments: &[&str]) -> std::process::Output {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "commonkit-headless-no-daemon-{}-{nonce}",
        std::process::id()
    ));
    Command::new(env!("CARGO_BIN_EXE_commonkit"))
        .env("HOME", &root)
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("APPDATA", root.join("appdata"))
        .env("LOCALAPPDATA", root.join("local-appdata"))
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
    let denied = command(&[
        "credentials",
        "apply",
        "--plan-id",
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    ]);
    assert!(!denied.status.success());
    assert!(String::from_utf8_lossy(&denied.stderr).contains("confirmation_required"));

    for arguments in [
        vec!["credentials", "plan", "api-token"],
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

#[test]
fn context_cli_enforces_bounds_before_contacting_the_daemon() {
    let invalid = command(&[
        "context",
        "search",
        "anything",
        "--session-id",
        "session-1",
        "--limit",
        "101",
    ]);
    assert!(!invalid.status.success());
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("limit 1..=100"));

    let valid = command(&[
        "context",
        "search",
        "anything",
        "--session-id",
        "session-1",
        "--limit",
        "10",
    ]);
    assert!(!valid.status.success());
    assert!(String::from_utf8_lossy(&valid.stderr).contains("daemon_unavailable"));
}
