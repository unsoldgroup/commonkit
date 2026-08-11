use std::fs;
use std::io::Write;
use std::process::{Command, Stdio};

use commonkit_adapters::{
    FileMode, NormalizedManagedPath, PackageDesiredIntent, SafeSymlinkTarget, SshFilesystemRequest,
    SshFilesystemResponse, SymlinkTargetKind, TargetHelper, TargetResource,
    package_resolution_request_digest,
};
use commonkit_contracts::{
    PackageDeclaration, PackageManager, PackageSelector, SecurityPolicy, digest_domain_json,
};
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

#[cfg(unix)]
#[test]
fn helper_subprocess_routes_relative_directories_and_symlinks_without_following_ancestors() {
    use std::os::unix::fs::symlink;

    let home = temp("relative-home");
    let target = temp("relative-target");
    let outside = temp("relative-outside");
    let state = temp("relative-state");
    fs::create_dir_all(home.join(".config/commonkit")).unwrap();
    fs::create_dir_all(&target).unwrap();
    fs::create_dir_all(&outside).unwrap();
    fs::write(
        home.join(".config/commonkit/target-helper.json"),
        serde_json::to_vec(&serde_json::json!({
            "stateRoot": state, "roots": [{"id":"home","path":target,"access":"read_write"}]
        }))
        .unwrap(),
    )
    .unwrap();
    let root_id = StableId::parse("home").unwrap();
    let source = NormalizedManagedPath::parse(".agents/skills/tool").unwrap();
    let link = NormalizedManagedPath::parse(".codex/skills/tool").unwrap();
    let relative = SafeSymlinkTarget::parse(&link, "../../.agents/skills/tool").unwrap();

    for request in [
        SshFilesystemRequest::WriteDirectory {
            root_id: root_id.clone(),
            path: source.clone(),
            mode: Some(FileMode::parse(0o700).unwrap()),
        },
        SshFilesystemRequest::WriteSymlink {
            root_id: root_id.clone(),
            path: link.clone(),
            target: relative,
            target_kind: SymlinkTargetKind::Directory,
        },
    ] {
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
    }
    let inspected = invoke(
        &home,
        &SshFilesystemRequest::InspectResource {
            root_id: root_id.clone(),
            path: link,
        },
    );
    assert_eq!(
        serde_json::from_slice::<SshFilesystemResponse>(&inspected.stdout).unwrap(),
        SshFilesystemResponse::Resource {
            resource: TargetResource::Symlink {
                target: "../../.agents/skills/tool".into(),
                target_kind: SymlinkTargetKind::File,
            }
        }
    );

    symlink(&outside, target.join("escape")).unwrap();
    let escaped = invoke(
        &home,
        &SshFilesystemRequest::WriteDirectory {
            root_id,
            path: NormalizedManagedPath::parse("escape/blocked").unwrap(),
            mode: None,
        },
    );
    assert!(!escaped.status.success());
    assert!(!outside.join("blocked").exists());

    fs::remove_dir_all(home).unwrap();
    fs::remove_dir_all(target).unwrap();
    fs::remove_dir_all(outside).unwrap();
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
fn chunk_transfer_rejects_gaps_and_tampering_but_accepts_safe_retry() {
    let home = temp("chunk-home");
    let target = temp("chunk-target");
    let state = temp("chunk-state");
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
    let bytes = vec![b'a'; 1_048_577];
    let first_chunk = vec![b'a'; 1_048_576];
    let second_chunk = vec![b'a'];
    let digest = Sha256Digest::parse(format!("sha256:{:x}", Sha256::digest(&bytes))).unwrap();
    let transfer = StableId::parse("transfer-one").unwrap();
    let malformed = invoke(
        &home,
        &SshFilesystemRequest::StageArtifactChunk {
            run_id: transfer.clone(),
            transfer_id: transfer.clone(),
            digest: digest.clone(),
            byte_count: bytes.len() as u64,
            chunk_size: commonkit_adapters::ARTIFACT_CHUNK_SIZE,
            sequence: 0,
            offset: bytes.len() as u64 + 1,
            total_chunks: 2,
            content: first_chunk.clone(),
        },
    );
    assert!(!malformed.status.success());
    let gap = invoke(
        &home,
        &SshFilesystemRequest::StageArtifactChunk {
            run_id: transfer.clone(),
            transfer_id: transfer.clone(),
            digest: digest.clone(),
            byte_count: bytes.len() as u64,
            chunk_size: commonkit_adapters::ARTIFACT_CHUNK_SIZE,
            sequence: 1,
            offset: commonkit_adapters::ARTIFACT_CHUNK_SIZE as u64,
            total_chunks: 2,
            content: second_chunk.clone(),
        },
    );
    assert!(!gap.status.success());
    let first = SshFilesystemRequest::StageArtifactChunk {
        run_id: transfer.clone(),
        transfer_id: transfer.clone(),
        digest: digest.clone(),
        byte_count: bytes.len() as u64,
        chunk_size: commonkit_adapters::ARTIFACT_CHUNK_SIZE,
        sequence: 0,
        offset: 0,
        total_chunks: 2,
        content: first_chunk.clone(),
    };
    assert!(invoke(&home, &first).status.success());
    assert!(invoke(&home, &first).status.success());
    let tampered = SshFilesystemRequest::StageArtifactChunk {
        run_id: transfer.clone(),
        transfer_id: transfer.clone(),
        digest: digest.clone(),
        byte_count: bytes.len() as u64,
        chunk_size: commonkit_adapters::ARTIFACT_CHUNK_SIZE,
        sequence: 0,
        offset: 0,
        total_chunks: 2,
        content: vec![b'W'; 1_048_576],
    };
    assert!(!invoke(&home, &tampered).status.success());
    let second = SshFilesystemRequest::StageArtifactChunk {
        run_id: transfer.clone(),
        transfer_id: transfer.clone(),
        digest: digest.clone(),
        byte_count: bytes.len() as u64,
        chunk_size: commonkit_adapters::ARTIFACT_CHUNK_SIZE,
        sequence: 1,
        offset: commonkit_adapters::ARTIFACT_CHUNK_SIZE as u64,
        total_chunks: 2,
        content: second_chunk,
    };
    assert!(invoke(&home, &second).status.success());
    let verified = invoke(
        &home,
        &SshFilesystemRequest::VerifyArtifact {
            run_id: transfer,
            digest: digest.clone(),
        },
    );
    assert!(verified.status.success());
    fs::remove_dir_all(home).unwrap();
    fs::remove_dir_all(target).unwrap();
    fs::remove_dir_all(state).unwrap();
}

#[cfg(unix)]
#[test]
fn sequence_zero_restarts_after_orphaned_part_or_metadata() {
    let home = temp("chunk-restart-home");
    let target = temp("chunk-restart-target");
    let state = temp("chunk-restart-state");
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
    let bytes = vec![b'r'; 1_048_577];
    let first_chunk = vec![b'r'; 1_048_576];
    let digest = Sha256Digest::parse(format!("sha256:{:x}", Sha256::digest(&bytes))).unwrap();
    let transfer = StableId::parse("transfer-restart").unwrap();
    let first = SshFilesystemRequest::StageArtifactChunk {
        run_id: transfer.clone(),
        transfer_id: transfer.clone(),
        digest: digest.clone(),
        byte_count: bytes.len() as u64,
        chunk_size: commonkit_adapters::ARTIFACT_CHUNK_SIZE,
        sequence: 0,
        offset: 0,
        total_chunks: 2,
        content: first_chunk,
    };
    assert!(invoke(&home, &first).status.success());
    let staging = state.join("artifact-staging");
    let stem = format!(
        "{}-{}",
        transfer,
        digest.as_str().trim_start_matches("sha256:")
    );
    let part_path = staging.join(format!("{stem}.part"));
    let meta_path = staging.join(format!("{stem}.meta"));
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(25 * 60 * 60);
    fs::File::open(&part_path)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(old))
        .unwrap();
    // The part looks stale, but the authoritative metadata was refreshed by
    // the accepted chunk. Restart cleanup must retain the active transfer.
    fs::File::open(&meta_path)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(std::time::SystemTime::now()))
        .unwrap();
    let retained = invoke(
        &home,
        &SshFilesystemRequest::ReadFile {
            root_id: StableId::parse("home").unwrap(),
            path: NormalizedManagedPath::parse("cleanup-probe").unwrap(),
        },
    );
    assert!(retained.status.success());
    // Simulate a crash after the part write and before metadata creation.
    fs::remove_file(&meta_path).unwrap();
    assert!(invoke(&home, &first).status.success());
    // Simulate the opposite crash window: metadata exists but the part was
    // not durable. A fresh sequence-0 request must safely recreate both.
    fs::remove_file(&part_path).unwrap();
    assert!(invoke(&home, &first).status.success());
    let final_chunk = SshFilesystemRequest::StageArtifactChunk {
        run_id: transfer.clone(),
        transfer_id: transfer.clone(),
        digest: digest.clone(),
        byte_count: bytes.len() as u64,
        chunk_size: commonkit_adapters::ARTIFACT_CHUNK_SIZE,
        sequence: 1,
        offset: commonkit_adapters::ARTIFACT_CHUNK_SIZE as u64,
        total_chunks: 2,
        content: vec![b'r'],
    };
    assert!(invoke(&home, &final_chunk).status.success());
    let verified = invoke(
        &home,
        &SshFilesystemRequest::VerifyArtifact {
            run_id: transfer,
            digest,
        },
    );
    assert!(verified.status.success());
    fs::remove_dir_all(home).unwrap();
    fs::remove_dir_all(target).unwrap();
    fs::remove_dir_all(state).unwrap();
}

#[cfg(not(unix))]
#[test]
fn helper_rejects_chunk_protocol_on_non_unix_before_dispatch() {
    let home = temp("chunk-platform-home");
    let target = temp("chunk-platform-target");
    let state = temp("chunk-platform-state");
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
    let request = SshFilesystemRequest::StageArtifactChunk {
        run_id: StableId::parse("platform-run").unwrap(),
        transfer_id: StableId::parse("platform-transfer").unwrap(),
        digest: Sha256Digest::parse(format!("sha256:{:x}", Sha256::digest(b""))).unwrap(),
        byte_count: 0,
        chunk_size: commonkit_adapters::ARTIFACT_CHUNK_SIZE,
        sequence: 0,
        offset: 0,
        total_chunks: 1,
        content: vec![],
    };
    assert!(!invoke(&home, &request).status.success());
    fs::remove_dir_all(home).unwrap();
    fs::remove_dir_all(target).unwrap();
    fs::remove_dir_all(state).unwrap();
}

#[cfg(unix)]
#[test]
fn helper_rejects_staging_symlinks_during_cleanup() {
    use std::os::unix::fs::symlink;

    let home = temp("chunk-symlink-home");
    let target = temp("chunk-symlink-target");
    let outside = temp("chunk-symlink-outside");
    let state = temp("chunk-symlink-state");
    fs::create_dir_all(home.join(".config/commonkit")).unwrap();
    fs::create_dir_all(&target).unwrap();
    fs::create_dir_all(&outside).unwrap();
    fs::write(
        home.join(".config/commonkit/target-helper.json"),
        serde_json::to_vec(&serde_json::json!({
            "stateRoot": state, "roots": [{"id":"home","path":target,"access":"read_write"}]
        }))
        .unwrap(),
    )
    .unwrap();
    let content = vec![b's'; commonkit_adapters::ARTIFACT_CHUNK_SIZE as usize + 1];
    let digest = Sha256Digest::parse(format!("sha256:{:x}", Sha256::digest(&content))).unwrap();
    let transfer = StableId::parse("transfer-symlink").unwrap();
    let request = SshFilesystemRequest::StageArtifactChunk {
        run_id: transfer.clone(),
        transfer_id: transfer.clone(),
        digest: digest.clone(),
        byte_count: content.len() as u64,
        chunk_size: commonkit_adapters::ARTIFACT_CHUNK_SIZE,
        sequence: 0,
        offset: 0,
        total_chunks: 2,
        content: content[..commonkit_adapters::ARTIFACT_CHUNK_SIZE as usize].to_vec(),
    };
    assert!(invoke(&home, &request).status.success());
    let staging = state.join("artifact-staging");
    let stem = format!(
        "{}-{}",
        transfer,
        digest.as_str().trim_start_matches("sha256:")
    );
    fs::remove_file(staging.join(format!("{stem}.part"))).unwrap();
    symlink(&outside, staging.join(format!("{stem}.part"))).unwrap();
    let probe = invoke(
        &home,
        &SshFilesystemRequest::ReadFile {
            root_id: StableId::parse("home").unwrap(),
            path: NormalizedManagedPath::parse("cleanup-probe").unwrap(),
        },
    );
    assert!(!probe.status.success());
    fs::remove_dir_all(home).unwrap();
    fs::remove_dir_all(target).unwrap();
    fs::remove_dir_all(outside).unwrap();
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

#[test]
fn helper_without_target_resolver_capability_returns_typed_rejection() {
    let home = temp("resolution-absent-home");
    let target = temp("resolution-absent-target");
    let state = temp("resolution-absent-state");
    fs::create_dir_all(home.join(".config/commonkit")).unwrap();
    fs::create_dir_all(&target).unwrap();
    fs::write(
        home.join(".config/commonkit/target-helper.json"),
        serde_json::to_vec(&serde_json::json!({
            "stateRoot": state,
            "roots": [{"id":"home","path":target,"access":"read_write"}]
        }))
        .unwrap(),
    )
    .unwrap();
    let target_facts = commonkit_adapters::PackageTargetV1 {
        os: "linux".into(),
        os_version: "24.04".into(),
        distro_id: Some("ubuntu".into()),
        distro_version: Some("24.04".into()),
        codename: Some("noble".into()),
        arch: "amd64".into(),
        libc: Some("glibc".into()),
        manager_prefix: None,
    };
    let desired = PackageDesiredIntent::new(PackageDeclaration {
        id: StableId::parse("curl").unwrap(),
        version: "1.0.0".into(),
        manager: PackageManager::Apt,
        source: StableId::parse("ubuntu-main").unwrap(),
        selector: Some(PackageSelector::AptBinary {
            name: "curl".into(),
            architecture: Some("amd64".into()),
        }),
    })
    .unwrap();
    let identity = digest_domain_json("fixture", &"target").unwrap();
    let request_nonce = digest_domain_json("fixture", &"nonce").unwrap();
    let request_digest = package_resolution_request_digest(
        &StableId::parse("home").unwrap(),
        &desired,
        &target_facts,
        PackageManager::Apt,
        &SecurityPolicy::default(),
        &None,
        &identity,
        &request_nonce,
    )
    .unwrap();
    let output = invoke(
        &home,
        &SshFilesystemRequest::PackageResolution {
            root_id: StableId::parse("home").unwrap(),
            request_id: StableId::parse("curl").unwrap(),
            request_nonce: request_nonce.clone(),
            desired,
            target: target_facts,
            manager_kind: PackageManager::Apt,
            policy: SecurityPolicy::default(),
            apt: None,
            target_identity_digest: identity.clone(),
            request_digest: request_digest.clone(),
        },
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<SshFilesystemResponse>(&output.stdout).unwrap(),
        SshFilesystemResponse::PackageResolutionRejected {
            request_id: StableId::parse("curl").unwrap(),
            request_nonce,
            request_digest,
            target_identity_digest: identity,
        }
    );
    fs::remove_dir_all(home).unwrap();
    fs::remove_dir_all(target).unwrap();
    fs::remove_dir_all(state).unwrap();
}

#[cfg(unix)]
#[test]
fn helper_rejects_group_or_world_writable_configuration() {
    use std::os::unix::fs::PermissionsExt;
    let home = temp("config-mode-home");
    let target = temp("config-mode-target");
    let state = temp("config-mode-state");
    fs::create_dir_all(home.join(".config/commonkit")).unwrap();
    fs::create_dir_all(&target).unwrap();
    let config_path = home.join(".config/commonkit/target-helper.json");
    fs::write(
        &config_path,
        serde_json::to_vec(&serde_json::json!({
            "stateRoot": state,
            "roots": [{"id":"home","path":target,"access":"read_write"}]
        }))
        .unwrap(),
    )
    .unwrap();
    fs::set_permissions(&config_path, fs::Permissions::from_mode(0o666)).unwrap();
    let output = invoke(
        &home,
        &SshFilesystemRequest::ReadFile {
            root_id: StableId::parse("home").unwrap(),
            path: NormalizedManagedPath::parse("probe").unwrap(),
        },
    );
    assert!(!output.status.success());
    fs::remove_dir_all(home).unwrap();
    fs::remove_dir_all(target).unwrap();
    let _ = fs::remove_dir_all(state);
}

#[test]
fn package_resolution_request_digest_is_challenge_bound() {
    let target = commonkit_adapters::PackageTargetV1 {
        os: "linux".into(),
        os_version: "24.04".into(),
        distro_id: Some("ubuntu".into()),
        distro_version: Some("24.04".into()),
        codename: Some("noble".into()),
        arch: "amd64".into(),
        libc: Some("glibc".into()),
        manager_prefix: None,
    };
    let desired = PackageDesiredIntent::new(PackageDeclaration {
        id: StableId::parse("curl").unwrap(),
        version: "1.0.0".into(),
        manager: PackageManager::Apt,
        source: StableId::parse("ubuntu-main").unwrap(),
        selector: None,
    })
    .unwrap();
    let identity = digest_domain_json("fixture", &"target").unwrap();
    let first = digest_domain_json("fixture", &"nonce-a").unwrap();
    let second = digest_domain_json("fixture", &"nonce-b").unwrap();
    let digest = |nonce| {
        package_resolution_request_digest(
            &StableId::parse("home").unwrap(),
            &desired,
            &target,
            PackageManager::Apt,
            &SecurityPolicy::default(),
            &None,
            &identity,
            nonce,
        )
        .unwrap()
    };
    assert_ne!(digest(&first), digest(&second));
}

#[cfg(all(unix, target_os = "linux"))]
#[test]
fn installed_probe_writes_a_real_nvm_package_capability_atomically() {
    use std::os::unix::fs::PermissionsExt;

    let home = temp("probe-home");
    let root = temp("probe-root");
    let state = temp("probe-state");
    let nvm = home.join(".nvm");
    let config = home.join(".config/commonkit/target-helper.json");
    fs::create_dir_all(&nvm).unwrap();
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::create_dir_all(&root).unwrap();
    fs::write(
        nvm.join("nvm.sh"),
        include_bytes!("fixtures/node/nvm-v0.40.6.sh"),
    )
    .unwrap();
    let shell = home.join("bash");
    let keyring = home.join("node-release-keyring.kbx");
    let gpgv = home.join("gpgv");
    fs::write(&shell, b"shell fixture").unwrap();
    fs::write(&keyring, b"keyring fixture").unwrap();
    fs::write(&gpgv, b"gpgv fixture").unwrap();
    fs::set_permissions(&shell, fs::Permissions::from_mode(0o700)).unwrap();
    fs::set_permissions(&gpgv, fs::Permissions::from_mode(0o700)).unwrap();
    let identity = digest_domain_json("fixture", "probe-target").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_commonkit-target-helper"))
        .args([
            "--probe-package-resolution",
            "--config",
            config.to_str().unwrap(),
            "--manager",
            "nvm",
            "--target-identity-digest",
            identity.as_str(),
            "--state-root",
            state.to_str().unwrap(),
            "--root-id",
            "home",
            "--root-path",
            root.to_str().unwrap(),
            "--nvm-dir",
            nvm.to_str().unwrap(),
            "--shell-executable",
            shell.to_str().unwrap(),
            "--release-keyring",
            keyring.to_str().unwrap(),
            "--gpgv-executable",
            gpgv.to_str().unwrap(),
        ])
        .env("HOME", &home)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata = fs::symlink_metadata(&config).unwrap();
    assert!(!metadata.file_type().is_symlink());
    assert_eq!(metadata.permissions().mode() & 0o022, 0);
    let document: serde_json::Value = serde_json::from_slice(&fs::read(&config).unwrap()).unwrap();
    assert_eq!(
        document["packageResolution"]["manager"]["version"],
        "0.40.6"
    );
    assert_eq!(
        document["packageResolution"]["targetIdentityDigest"],
        identity.as_str()
    );
    assert_eq!(
        document["packageResolution"]["node"]["gpgvExecutable"],
        gpgv.to_str().unwrap()
    );
    assert!(
        document["packageResolution"]["node"]["gpgvExecutableDigest"]
            .as_str()
            .is_some_and(|digest| digest.starts_with("sha256:"))
    );
    let _ = fs::remove_dir_all(home);
    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(state);
}

#[cfg(unix)]
#[test]
fn installed_probe_rejects_a_symlinked_config_path() {
    use std::os::unix::fs::symlink;
    let home = temp("probe-symlink-home");
    let outside = temp("probe-symlink-outside");
    fs::create_dir_all(home.join(".config/commonkit")).unwrap();
    fs::write(&outside, b"{}").unwrap();
    symlink(&outside, home.join(".config/commonkit/target-helper.json")).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_commonkit-target-helper"))
        .args(["--probe-package-resolution", "--manager", "nvm"])
        .env("HOME", &home)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let _ = fs::remove_dir_all(home);
    let _ = fs::remove_file(outside);
}

#[test]
fn helper_rejects_a_writable_root_overlapping_control_state() {
    let root = temp("protected-root");
    fs::create_dir_all(&root).unwrap();
    let state = root.join("state");
    fs::create_dir_all(&state).unwrap();
    let result = TargetHelper::validate_configuration(
        &[commonkit_core::TargetRoot {
            id: StableId::parse("home").unwrap(),
            path: root.to_string_lossy().into_owned(),
            access: commonkit_core::RootAccess::ReadWrite,
        }],
        &state,
        std::slice::from_ref(&state),
    );
    assert!(result.is_err());
    assert!(!state.join("artifacts").exists());
    let _ = fs::remove_dir_all(root);
}
