use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::Command;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use commonkit_service::{ControlToken, DaemonDiscovery};
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

fn isolate_app_paths(command: &mut Command, root: &std::path::Path) {
    command
        .env("HOME", root)
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("APPDATA", root.join("appdata"))
        .env("LOCALAPPDATA", root.join("local-appdata"));
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
        .env("PATH", "")
        .output()
        .expect("status");
    assert!(output.status.success());
    let status: Value = serde_json::from_slice(&output.stdout).expect("JSON");
    assert_eq!(status["contractVersion"], "1.0");
    assert_eq!(status["schemaVersion"], 1);
    assert!(status["stateDirectory"].as_str().is_some());
}

#[test]
fn sync_help_exposes_explicit_fetch_before_plan() {
    let output = Command::new(env!("CARGO_BIN_EXE_commonkit"))
        .args(["sync", "--help"])
        .output()
        .expect("sync help");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--fetch"), "{stdout}");
    assert!(!stdout.contains("--confirmed"), "{stdout}");
}

#[test]
fn engram_status_reports_a_declared_project_chunk_set() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("chunks")).unwrap();
    fs::write(
        root.path().join("manifest.json"),
        r#"{"version":1,"chunks":[]}"#,
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_commonkit"))
        .args([
            "engram",
            "status",
            "--project-id",
            "github.com/unsoldgroup/commonkit",
            "--root",
            root.path().to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("principal_unavailable"));
}

#[test]
fn engram_commands_do_not_accept_arbitrary_owner_labels() {
    let output = Command::new(env!("CARGO_BIN_EXE_commonkit"))
        .args(["engram", "status", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    assert!(!help.contains("--owner-id"));
}

#[test]
fn engram_watch_requires_confirmation_and_has_a_one_minute_default() {
    let output = Command::new(env!("CARGO_BIN_EXE_commonkit"))
        .args(["engram", "watch", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    assert!(help.contains("--interval-seconds <INTERVAL_SECONDS>"));
    assert!(help.contains("[default: 60]"));
    assert!(help.contains("--confirmed"));
}

#[test]
fn sync_fetch_sends_one_fetch_bound_plan_request_and_never_applies() {
    let root = tempfile::tempdir().unwrap();
    let mut status = Command::new(env!("CARGO_BIN_EXE_commonkit"));
    isolate_app_paths(&mut status, root.path());
    let status: Value = serde_json::from_slice(&status.arg("status").output().unwrap().stdout)
        .expect("status JSON");
    let config = std::path::PathBuf::from(status["configDirectory"].as_str().unwrap());
    let state = std::path::PathBuf::from(status["stateDirectory"].as_str().unwrap());
    std::fs::create_dir_all(&config).unwrap();
    std::fs::create_dir_all(&state).unwrap();
    let token = ControlToken::load_or_create(&config.join("control.token")).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    DaemonDiscovery::new(port, None, &token)
        .unwrap()
        .persist(&state.join("daemon.json"))
        .unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0_u8; 8192];
        let length = stream.read(&mut request).unwrap();
        let request = String::from_utf8_lossy(&request[..length]);
        assert!(request.starts_with("POST /control/v1/sync/plan "));
        assert!(request.contains(r#""fetch":true"#), "{request}");
        assert!(
            request.contains(r#""confirmed":true"#),
            "the explicit --fetch option is the operator's consent to update Git tracking refs: {request}"
        );
        assert!(!request.contains("/apply"));
        let body = r#"{"planId":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#;
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
    });

    let mut command = Command::new(env!("CARGO_BIN_EXE_commonkit"));
    isolate_app_paths(&mut command, root.path());
    let output = command.args(["sync", "--fetch"]).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    server.join().unwrap();
}

#[test]
fn skill_optimization_requires_an_explicit_production_backend() {
    let help = Command::new(env!("CARGO_BIN_EXE_commonkit"))
        .args(["skills", "optimize", "--help"])
        .output()
        .expect("skill optimize help");
    assert!(help.status.success());
    let help = String::from_utf8(help.stdout).expect("utf8");
    assert!(help.contains("--backend <BACKEND>"));
    assert!(!help.contains("[default: mock]"));

    let output = Command::new(env!("CARGO_BIN_EXE_commonkit"))
        .args([
            "skills",
            "optimize",
            "--repository",
            "repo",
            "--state",
            "state",
            "--manifest",
            "manifest.json",
            "--suite",
            "suite.json",
            "--environment",
            "environment",
            "--tasks",
            "tasks.json",
            "--harness",
            "harness",
            "--corpus",
            "corpus.json",
            "--confirmed",
        ])
        .output()
        .expect("skill optimize parse");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--backend"));

    let output = Command::new(env!("CARGO_BIN_EXE_commonkit"))
        .args([
            "skills",
            "optimize",
            "--repository",
            "repo",
            "--state",
            "state",
            "--manifest",
            "manifest.json",
            "--suite",
            "suite.json",
            "--environment",
            "environment",
            "--tasks",
            "tasks.json",
            "--harness",
            "harness",
            "--corpus",
            "corpus.json",
            "--backend",
            "mock",
            "--confirmed",
        ])
        .output()
        .expect("mock backend authorization");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("development-only"));
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
    let create_help = Command::new(env!("CARGO_BIN_EXE_commonkit"))
        .args(["init", "create", "--help"])
        .output()
        .expect("init create help");
    let create_help = String::from_utf8(create_help.stdout).expect("utf8");
    assert!(create_help.contains("--project-loadout"));
    assert!(create_help.contains("--target-override"));
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
        serde_json::to_vec(&layer(
            "base",
            "public_base",
            json!({"contextBudget": {"maxTotalTokens": 100}}),
        ))
        .expect("base"),
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
        serde_json::to_vec(&layer(
            "personal",
            "personal_kit",
            json!({"contextBudget": {"maxTotalTokens": 80}}),
        ))
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
    assert_eq!(composition["spec"]["contextBudget"]["maxTotalTokens"], 80);

    let explanation = Command::new(env!("CARGO_BIN_EXE_commonkit"))
        .args(["explain", "/contextBudget/maxTotalTokens", "--layer"])
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
fn compose_rejects_a_personal_policy_that_widens_the_organization_allowlist() {
    let directory = temporary_directory("cli-policy-floor");
    fs::create_dir_all(&directory).expect("directory");
    let policy = |allowed: &[&str]| {
        json!({
            "deniedPaths": ["**/.env"],
            "requiredControls": {"secret_scan": true},
            "allowlists": {"git_hosts": allowed},
            "minimums": {"backup_count": 1},
            "maximums": {"snapshot_age_hours": 24}
        })
    };
    let documents = [
        ("base", "public_base", json!({})),
        (
            "organization",
            "organization_policy",
            json!({"securityPolicy": policy(&["github.com"])}),
        ),
        (
            "personal",
            "personal_kit",
            json!({"securityPolicy": policy(&["github.com", "evil.example"])}),
        ),
    ];
    let paths = documents.map(|(id, kind, spec)| {
        let path = directory.join(format!("{id}.json"));
        fs::write(&path, serde_json::to_vec(&layer(id, kind, spec)).unwrap()).unwrap();
        path
    });
    let mut command = Command::new(env!("CARGO_BIN_EXE_commonkit"));
    command.arg("compose");
    for path in &paths {
        command.arg("--layer").arg(path);
    }

    let output = command.output().expect("compose");

    assert!(!output.status.success());
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
    let directory = temporary_directory("cli-no-daemon");
    let mut command = Command::new(env!("CARGO_BIN_EXE_commonkit"));
    isolate_app_paths(&mut command, &directory);
    let output = command.arg("verify").output().expect("verify");
    assert!(!output.status.success());
    let error = String::from_utf8(output.stderr).expect("UTF-8");
    assert!(error.contains("daemon_unavailable"));
    assert!(error.contains("CommonKit daemon"));
}

#[test]
fn mutations_require_an_explicit_confirmation_flag() {
    for arguments in [
        vec![
            "apply",
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ],
        vec!["rollback", "run-reviewed"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_commonkit"))
            .args(arguments)
            .output()
            .expect("mutation");
        assert!(!output.status.success());
        assert!(
            String::from_utf8(output.stderr)
                .unwrap()
                .contains("confirmation_required")
        );
    }
}

#[test]
fn read_only_sync_preview_does_not_require_confirmation() {
    let directory = temporary_directory("cli-sync-preview-no-daemon");
    let mut command = Command::new(env!("CARGO_BIN_EXE_commonkit"));
    isolate_app_paths(&mut command, &directory);
    let output = command.arg("sync").output().expect("sync preview");

    assert!(!output.status.success());
    let error = String::from_utf8(output.stderr).expect("UTF-8");
    assert!(error.contains("daemon_unavailable"), "{error}");
    assert!(!error.contains("confirmation_required"), "{error}");
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
    let mut command = Command::new(env!("CARGO_BIN_EXE_commonkit"));
    isolate_app_paths(&mut command, &directory);
    let enabled = command
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
    let mut command = Command::new(env!("CARGO_BIN_EXE_commonkit"));
    isolate_app_paths(&mut command, &directory);
    let output = command
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
