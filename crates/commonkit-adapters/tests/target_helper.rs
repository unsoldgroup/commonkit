use std::fs;
use std::io::Write;
use std::process::{Command, Stdio};

use commonkit_adapters::{NormalizedManagedPath, SshFilesystemRequest, SshFilesystemResponse};
use commonkit_core::{Sha256Digest, StableId};
use sha2::{Digest, Sha256};

fn temp(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("commonkit-helper-{name}-{}", std::process::id()))
}

fn invoke(home: &std::path::Path, request: &SshFilesystemRequest) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_commonkit-target-helper"))
        .arg("--stdio-v1")
        .env("HOME", home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(request).unwrap())
        .unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn helper_subprocess_applies_only_typed_requests_inside_configured_roots() {
    let home = temp("home");
    let target = temp("target");
    let state = temp("state");
    fs::create_dir_all(home.join(".config/commonkit")).unwrap();
    fs::create_dir_all(&target).unwrap();
    let config = serde_json::json!({
        "stateRoot": state,
        "roots": [{"id":"home","path":target,"access":"read_write"}]
    });
    fs::write(
        home.join(".config/commonkit/target-helper.json"),
        serde_json::to_vec(&config).unwrap(),
    )
    .unwrap();
    let request = SshFilesystemRequest::WriteFile {
        root_id: StableId::parse("home").unwrap(),
        path: NormalizedManagedPath::parse(".config/tool/settings.json").unwrap(),
        content: b"{}\n".to_vec(),
    };
    let output = invoke(&home, &request);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<SshFilesystemResponse>(&output.stdout).unwrap(),
        SshFilesystemResponse::Applied
    );
    assert_eq!(
        fs::read(target.join(".config/tool/settings.json")).unwrap(),
        b"{}\n"
    );
    fs::remove_dir_all(home).unwrap();
    fs::remove_dir_all(target).unwrap();
    fs::remove_dir_all(state).unwrap();
}

#[test]
fn artifacts_and_recovery_receipts_survive_helper_process_restarts() {
    let home = temp("artifact-home");
    let target = temp("artifact-target");
    let state = temp("artifact-state");
    fs::create_dir_all(home.join(".config/commonkit")).unwrap();
    fs::create_dir_all(&target).unwrap();
    fs::write(
        home.join(".config/commonkit/target-helper.json"),
        serde_json::to_vec(&serde_json::json!({
            "stateRoot": state, "roots": [{"id":"home","path":target,"access":"read_write"}]
        }))
        .unwrap(),
    )
    .unwrap();
    let bytes = b"durable receipt";
    let digest = Sha256Digest::parse(format!("sha256:{:x}", Sha256::digest(bytes))).unwrap();
    let run_id = StableId::parse("run-2").unwrap();
    let staged = invoke(
        &home,
        &SshFilesystemRequest::StageArtifact {
            run_id: run_id.clone(),
            digest: digest.clone(),
            content: bytes.to_vec(),
        },
    );
    assert!(staged.status.success());
    assert_eq!(
        serde_json::from_slice::<SshFilesystemResponse>(&staged.stdout).unwrap(),
        SshFilesystemResponse::ArtifactStaged {
            digest: digest.clone()
        }
    );
    let verified = invoke(
        &home,
        &SshFilesystemRequest::VerifyArtifact {
            run_id: run_id.clone(),
            digest: digest.clone(),
        },
    );
    assert_eq!(
        serde_json::from_slice::<SshFilesystemResponse>(&verified.stdout).unwrap(),
        SshFilesystemResponse::ArtifactVerified {
            digest: digest.clone()
        }
    );
    let bound = invoke(
        &home,
        &SshFilesystemRequest::BindRecoveryReceipt {
            run_id: run_id.clone(),
            receipt_digest: digest.clone(),
        },
    );
    assert_eq!(
        serde_json::from_slice::<SshFilesystemResponse>(&bound.stdout).unwrap(),
        SshFilesystemResponse::RecoveryReceiptBound {
            receipt_digest: digest.clone()
        }
    );
    let recovery = invoke(
        &home,
        &SshFilesystemRequest::RecoverRun {
            run_id,
            receipt_digest: digest.clone(),
        },
    );
    assert_eq!(
        serde_json::from_slice::<SshFilesystemResponse>(&recovery.stdout).unwrap(),
        SshFilesystemResponse::RecoveryReady {
            receipt_digest: digest
        }
    );
    fs::remove_dir_all(home).unwrap();
    fs::remove_dir_all(target).unwrap();
    fs::remove_dir_all(state).unwrap();
}

#[test]
fn helper_rejects_any_command_surface_other_than_stdio_protocol() {
    let output = Command::new(env!("CARGO_BIN_EXE_commonkit-target-helper"))
        .arg("sh")
        .arg("-c")
        .arg("touch /tmp/nope")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "commonkit target helper rejected the request\n"
    );
}
