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
