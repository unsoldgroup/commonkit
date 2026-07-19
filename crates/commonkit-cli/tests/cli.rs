use std::fs;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

fn temporary_directory(test: &str) -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!("commonkit-{test}-{}-{nonce}", std::process::id()))
}

fn layer(id: &str, kind: &str, spec: Value) -> Value {
    json!({
        "schemaVersion": 1,
        "id": id,
        "kind": kind,
        "source": {
            "path": format!("layers/{id}.json"),
            "revision": "57a085e7d0b558e71c8d2255b7e60e6c677dee76",
            "contentDigest": format!("sha256:{}", "0".repeat(64))
        },
        "spec": spec
    })
}

#[test]
fn reports_versioned_machine_readable_status() {
    let output = Command::new(env!("CARGO_BIN_EXE_commonkit"))
        .arg("status")
        .output()
        .expect("status");
    assert!(output.status.success());
    let status: Value = serde_json::from_slice(&output.stdout).expect("JSON");
    assert_eq!(status["contractVersion"], "1.0");
    assert_eq!(status["schemaVersion"], 1);
    assert!(status["stateDirectory"].as_str().is_some());
}

#[test]
fn init_exposes_create_connect_and_rejects_malformed_repository_before_gh() {
    let help = Command::new(env!("CARGO_BIN_EXE_commonkit"))
        .args(["init", "--help"])
        .output()
        .expect("init help");
    let help = String::from_utf8(help.stdout).expect("utf8");
    assert!(help.contains("create"));
    assert!(help.contains("connect"));
    let root = temporary_directory("invalid-onboarding");
    let output = Command::new(env!("CARGO_BIN_EXE_commonkit"))
        .args([
            "init",
            "create",
            "--repository",
            "not/a/valid/repository",
            "--kit-directory",
        ])
        .arg(root.join("kit"))
        .args([
            "--loadout",
            "personal",
            "--target",
            "local",
            "--target-root",
        ])
        .arg(root.join("target"))
        .output()
        .expect("invalid init");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("repository"));
}

#[test]
fn composes_layers_and_explains_the_winning_value() {
    let directory = temporary_directory("cli-compose");
    fs::create_dir_all(&directory).expect("directory");
    let base = directory.join("base.json");
    let organization = directory.join("organization.json");
    let personal = directory.join("personal.json");
    fs::write(
        &base,
        serde_json::to_vec(&layer("base", "public_base", json!({"theme": "light"}))).expect("base"),
    )
    .expect("write base");
    fs::write(
        &organization,
        serde_json::to_vec(&layer("organization", "organization_policy", json!({})))
            .expect("organization"),
    )
    .expect("write organization");
    fs::write(
        &personal,
        serde_json::to_vec(&layer("personal", "personal_kit", json!({"theme": "dark"})))
            .expect("personal"),
    )
    .expect("write personal");

    let output = Command::new(env!("CARGO_BIN_EXE_commonkit"))
        .args(["compose", "--layer"])
        .arg(&base)
        .arg("--layer")
        .arg(&organization)
        .arg("--layer")
        .arg(&personal)
        .output()
        .expect("compose");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let composition: Value = serde_json::from_slice(&output.stdout).expect("JSON");
    assert_eq!(composition["spec"]["theme"], "dark");

    let explanation = Command::new(env!("CARGO_BIN_EXE_commonkit"))
        .args(["explain", "/theme", "--layer"])
        .arg(&base)
        .arg("--layer")
        .arg(&organization)
        .arg("--layer")
        .arg(&personal)
        .output()
        .expect("explain");
    assert!(explanation.status.success());
    let explanation: Value = serde_json::from_slice(&explanation.stdout).expect("JSON");
    assert_eq!(explanation["winner"]["layerId"], "personal");

    fs::remove_dir_all(directory).expect("cleanup");
}

#[test]
fn publishes_the_complete_v1_command_surface() {
    let output = Command::new(env!("CARGO_BIN_EXE_commonkit"))
        .arg("--help")
        .output()
        .expect("help");
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).expect("UTF-8");
    for command in [
        "init",
        "status",
        "sync",
        "compose",
        "explain",
        "diff",
        "apply",
        "verify",
        "rollback",
        "schedule",
        "diagnostics",
        "relay",
        "skills",
    ] {
        assert!(help.contains(command), "missing {command} in {help}");
    }
}

#[test]
fn headless_commands_contact_the_daemon_and_fail_actionably_when_it_is_absent() {
    let output = Command::new(env!("CARGO_BIN_EXE_commonkit"))
        .arg("verify")
        .output()
        .expect("verify");
    assert!(!output.status.success());
    let error = String::from_utf8(output.stderr).expect("UTF-8");
    assert!(error.contains("daemon_unavailable"));
    assert!(error.contains("CommonKit daemon"));
}

#[test]
fn mutations_require_an_explicit_confirmation_flag() {
    let output = Command::new(env!("CARGO_BIN_EXE_commonkit"))
        .args([
            "apply",
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ])
        .output()
        .expect("apply");
    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("confirmation_required")
    );
}

#[test]
fn skill_schedule_mutation_requires_confirmation_and_daemon_authority() {
    let directory = temporary_directory("cli-skill-schedule");
    fs::create_dir_all(directory.join(".agents/skills/review")).expect("skills");
    fs::write(
        directory.join(".agents/skills/review/SKILL.md"),
        "# Review\n",
    )
    .expect("skill");
    let state = directory.join("state");
    let denied = Command::new(env!("CARGO_BIN_EXE_commonkit"))
        .args(["skills", "schedule", "enable", "--repository"])
        .arg(&directory)
        .arg("--state")
        .arg(&state)
        .args([
            "--kind",
            "candidate_generation",
            "--interval-seconds",
            "86400",
            "--maximum-cost-micros",
            "100",
        ])
        .output()
        .expect("denied");
    assert!(!denied.status.success());
    assert!(String::from_utf8_lossy(&denied.stderr).contains("confirmation_required"));
    let enabled = Command::new(env!("CARGO_BIN_EXE_commonkit"))
        .args(["skills", "schedule", "enable", "--repository"])
        .arg(&directory)
        .arg("--state")
        .arg(&state)
        .args([
            "--kind",
            "candidate_generation",
            "--interval-seconds",
            "86400",
            "--maximum-cost-micros",
            "100",
            "--confirmed",
        ])
        .output()
        .expect("enable");
    assert!(!enabled.status.success());
    assert!(String::from_utf8_lossy(&enabled.stderr).contains("daemon_unavailable"));
    fs::remove_dir_all(directory).expect("cleanup");
}

#[cfg(unix)]
#[test]
fn provider_check_converges_through_daemon_authority() {
    let directory = temporary_directory("cli-provider");
    let environment = directory.join("provider");
    fs::create_dir_all(environment.join("bin")).expect("bin");
    for (name, body) in [
        ("python", "#!/bin/sh\nprintf '0.2.0\\n'\n"),
        ("skillopt-sleep", "#!/bin/sh\nexit 0\n"),
    ] {
        let path = environment.join("bin").join(name);
        fs::write(&path, body).expect("tool");
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("mode");
    }
    let lock = json!({"id":"skillopt","version":"0.2.0","adapterContract":"skillopt-sleep-v1","source":"pypi","packageDigest":format!("sha256:{}", "8".repeat(64)),"capabilities":["json-report","reviewed-tasks","staged-skill"],"sourceRevision":null});
    let lock_path = directory.join("provider-lock.json");
    fs::write(&lock_path, serde_json::to_vec(&lock).expect("lock")).expect("lock");
    let marker = json!({"provider":"skillopt","version":"0.2.0","adapterContract":"skillopt-sleep-v1","source":"pypi","packageDigest":format!("sha256:{}", "8".repeat(64)),"capabilities":["json-report","reviewed-tasks","staged-skill"],"compatible":true});
    fs::write(
        environment.join("commonkit-provider-lock.json"),
        serde_json::to_vec(&marker).expect("marker"),
    )
    .expect("marker");
    let output = Command::new(env!("CARGO_BIN_EXE_commonkit"))
        .args(["skills", "provider", "check", "--environment"])
        .arg(&environment)
        .arg("--lock")
        .arg(&lock_path)
        .output()
        .expect("check");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("daemon_unavailable"));
    fs::remove_dir_all(directory).expect("cleanup");
}
