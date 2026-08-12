use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::{Mutex, MutexGuard, OnceLock};

use commonkit_adapters::{
    FileMode, NodeRuntimeHost, NormalizedManagedPath, PackageArtifactV1, PackageDesiredIntent,
    PackageMutationArtifact, PackageMutationPhase, PackageResolutionV1, PackageSourceRegistry,
    PackageTargetV1, ProcessNodeRuntimeHost, ResolvedPackage, SafeSymlinkTarget, SourceBindingV1,
    SshFilesystemRequest, SshFilesystemResponse, SymlinkTargetKind, TargetHelper,
    TargetNodeResolutionConfig, TargetPackageResolutionConfig, TargetResource,
    package_resolution_request_digest,
};
use commonkit_contracts::{
    PackageDeclaration, PackageManager, PackageSelector, SecurityPolicy, digest_domain_json,
};
use commonkit_core::{RootAccess, Sha256Digest, StableId, TargetRoot};
use sha2::{Digest, Sha256};

fn temp(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("commonkit-helper-{name}-{}", std::process::id()))
}

#[cfg(unix)]
struct NvmEnvironmentGuard {
    previous: Vec<(OsString, OsString)>,
    _lock: MutexGuard<'static, ()>,
}

#[cfg(unix)]
impl NvmEnvironmentGuard {
    fn original_value(&self, name: &OsStr) -> Option<OsString> {
        self.previous
            .iter()
            .find(|(previous_name, _)| previous_name == name)
            .map(|(_, value)| value.clone())
    }
}

#[cfg(unix)]
impl Drop for NvmEnvironmentGuard {
    fn drop(&mut self) {
        for name in std::env::vars_os()
            .filter_map(|(name, _)| is_prohibited_nvm_environment_name(&name).then_some(name))
        {
            // Environment mutation is synchronized for this test process.
            unsafe { std::env::remove_var(name) };
        }
        for (name, value) in &self.previous {
            // Environment mutation is synchronized for this test process.
            unsafe { std::env::set_var(name, value) };
        }
    }
}

#[cfg(unix)]
fn is_prohibited_nvm_environment_name(name: &OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return true;
    };
    let canonical = name.to_ascii_uppercase();
    canonical.starts_with("NPM_CONFIG_")
        || matches!(
            canonical.as_str(),
            "PREFIX"
                | "NVM_NODEJS_ORG_MIRROR"
                | "NVM_IOJS_ORG_MIRROR"
                | "NVM_REINSTALL_PACKAGES_FROM"
                | "NVM_INSTALL_LATEST_NPM"
        )
}

#[cfg(unix)]
fn clean_nvm_environment() -> NvmEnvironmentGuard {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let lock = LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
    let previous = std::env::vars_os()
        .filter(|(name, _)| is_prohibited_nvm_environment_name(name))
        .collect::<Vec<_>>();
    for (name, _) in &previous {
        // Environment mutation is synchronized for this test process.
        unsafe { std::env::remove_var(name) };
    }
    NvmEnvironmentGuard {
        previous,
        _lock: lock,
    }
}

#[cfg(unix)]
#[test]
fn nvm_environment_guard_removes_created_prohibited_variants() {
    use std::os::unix::ffi::OsStringExt;

    let non_utf8_name = OsString::from_vec(b"NPM_CONFIG_\xff".to_vec());
    {
        let guard = clean_nvm_environment();
        let prefix_before = guard.original_value(OsStr::new("PREFIX"));
        let lowercase_before = guard.original_value(OsStr::new("npm_config_registry"));
        let non_utf8_before = guard.original_value(&non_utf8_name);
        // Environment mutation is synchronized by the guard.
        unsafe {
            std::env::set_var("PREFIX", "created-by-guard-regression");
            std::env::set_var("npm_config_registry", "created-by-guard-regression");
            std::env::set_var(&non_utf8_name, "created-by-guard-regression");
        }
        drop(guard);
        let verify = clean_nvm_environment();
        assert_eq!(verify.original_value(OsStr::new("PREFIX")), prefix_before);
        assert_eq!(
            verify.original_value(OsStr::new("npm_config_registry")),
            lowercase_before
        );
        assert_eq!(verify.original_value(&non_utf8_name), non_utf8_before);
        drop(verify);
    }
}

#[cfg(unix)]
#[test]
fn nvm_environment_guard_serializes_concurrent_snapshots() {
    let workers = (0..2)
        .map(|worker| {
            std::thread::spawn(move || {
                let guard = clean_nvm_environment();
                let prefix_before = guard.original_value(OsStr::new("PREFIX"));
                // Environment mutation is synchronized by the guard.
                unsafe {
                    std::env::set_var("PREFIX", format!("created-by-worker-{worker}"));
                }
                drop(guard);

                let verify = clean_nvm_environment();
                assert_eq!(verify.original_value(OsStr::new("PREFIX")), prefix_before);
                drop(verify);
            })
        })
        .collect::<Vec<_>>();
    for worker in workers {
        worker.join().unwrap();
    }
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
fn package_mutation_revalidates_target_authority_and_exact_artifacts() {
    let _nvm_environment = clean_nvm_environment();
    let root = temp("package-authority-root");
    let state = temp("package-authority-state");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&state).unwrap();
    let root = fs::canonicalize(root).unwrap();
    let nvm = root.join(".nvm");
    fs::create_dir_all(&nvm).unwrap();
    fs::write(
        nvm.join("nvm.sh"),
        include_bytes!("fixtures/node/nvm-v0.40.6.sh"),
    )
    .unwrap();
    fs::create_dir_all(nvm.join("versions/node/v20.0.0")).unwrap();
    let shell = std::path::PathBuf::from("/bin/bash");
    let keyring = root.join("node-release-keyring.kbx");
    let gpgv = root.join("gpgv");
    fs::write(&keyring, b"fixture node release keyring").unwrap();
    fs::copy(&shell, &gpgv).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&gpgv, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let target = PackageTargetV1 {
        os: "linux".into(),
        os_version: "24.04".into(),
        distro_id: Some("ubuntu".into()),
        distro_version: Some("24.04".into()),
        codename: Some("noble".into()),
        arch: "x86_64".into(),
        libc: Some("glibc".into()),
        manager_prefix: Some(nvm.to_string_lossy().into_owned()),
    };
    let mut host = ProcessNodeRuntimeHost::new_with_gpgv(
        nvm.clone(),
        shell.clone(),
        keyring.clone(),
        gpgv.clone(),
    );
    let snapshot = host.probe(&target).unwrap();
    let manager = snapshot.manager.clone();
    let source_id = StableId::parse("nodejs-nvm").unwrap();
    let policy = SecurityPolicy {
        allowlists: [(
            StableId::parse("package_sources").unwrap(),
            [source_id.as_str().into()].into_iter().collect(),
        )]
        .into_iter()
        .collect(),
        ..SecurityPolicy::default()
    };
    let config = TargetPackageResolutionConfig {
        target: target.clone(),
        manager: manager.clone(),
        policy: policy.clone(),
        apt: None,
        node: Some(TargetNodeResolutionConfig {
            nvm_dir: nvm.clone(),
            shell_executable: shell,
            release_keyring: keyring.clone(),
            gpgv_executable: gpgv,
            gpgv_executable_digest: snapshot.gpgv_executable_digest.clone(),
        }),
        target_identity_digest: Sha256Digest::parse(format!("sha256:{}", "c".repeat(64))).unwrap(),
    };
    let registry = PackageSourceRegistry::builtin()
        .unwrap()
        .with_commonkit_node_release_authority(&source_id)
        .unwrap();
    let source = SourceBindingV1 {
        source_id: source_id.clone(),
        registry_definition_digest: registry.source_definition_digest(&source_id).unwrap(),
        canonical_repository: "https://nodejs.org/dist".into(),
        repository_revision: Some("0123456789abcdef".repeat(4)),
        signed_metadata: Vec::new(),
    };
    let declaration = PackageDeclaration {
        id: StableId::parse("node").unwrap(),
        version: "20.0.0".into(),
        manager: PackageManager::Nvm,
        source: source_id,
        selector: Some(PackageSelector::NodeRuntime {}),
    };
    let resolution = PackageResolutionV1 {
        schema_version: commonkit_contracts::SchemaVersion(2),
        declaration: declaration.clone(),
        target,
        manager: manager.clone(),
        source: source.clone(),
        before: snapshot.before.clone(),
        closure: vec![ResolvedPackage {
            declaration,
            source,
        }],
        artifacts: Vec::new(),
        recipe: commonkit_adapters::OfflineInstallRecipeV1::NodeArchive {
            artifact_roles: BTreeSet::new(),
            install: Some(commonkit_adapters::NodeOfflineInstallRecipeV1 {
                node_version: "20.0.0".into(),
                archive_file_name: "node-v20.0.0-linux-x64.tar.xz".into(),
                cache_relative_path:
                    ".cache/bin/node-v20.0.0-linux-x64/node-v20.0.0-linux-x64.tar.xz".into(),
                nvm_version: manager.version.clone(),
                nvm_script_digest: snapshot.nvm_script_digest.clone(),
                shell_executable_digest: snapshot.shell_executable_digest.clone(),
                offline: true,
                no_source_fallback: true,
                per_version_lock: true,
                install_latest_npm: false,
                migrate_packages: false,
            }),
        },
    };
    let direct_authority = commonkit_adapters::PackageResolutionAuthority::new(
        &resolution.target,
        &resolution.manager,
        &registry,
        &policy,
    )
    .unwrap();
    direct_authority.validate_resolution(&resolution).unwrap();
    let target_identity_digest = config.target_identity_digest.clone();
    let helper = TargetHelper::open_with_package_resolution(
        vec![TargetRoot {
            id: StableId::parse("home").unwrap(),
            path: root.to_string_lossy().into_owned(),
            access: RootAccess::ReadWrite,
        }],
        &state,
        Some(config),
    )
    .unwrap();
    let request = |resolution: PackageResolutionV1, artifacts, target_identity_digest| {
        SshFilesystemRequest::PackageMutation {
            root_id: StableId::parse("home").unwrap(),
            phase: PackageMutationPhase::Observe,
            resolution,
            artifacts,
            target_identity_digest,
        }
    };
    let accepted = helper.dispatch(request(
        resolution.clone(),
        Vec::new(),
        Some(target_identity_digest.clone()),
    ));
    assert!(
        matches!(
        accepted,
        Ok(SshFilesystemResponse::PackageObserved {
            ref installed_versions,
        }) if *installed_versions == snapshot.before.installed_versions
        ),
        "valid package observation failed: {accepted:?}"
    );

    let mut observed_resolution = resolution.clone();
    let observed_reference = commonkit_adapters::ContentReference {
        digest: Sha256Digest::parse(format!("sha256:{}", "e".repeat(64))).unwrap(),
        bytes: 1,
        sensitivity: commonkit_adapters::ContentSensitivity::Portable,
    };
    let artifact_role = StableId::parse("node-archive").unwrap();
    observed_resolution.artifacts = vec![PackageArtifactV1 {
        role: artifact_role.clone(),
        content: observed_reference.clone(),
        upstream_checksum: observed_reference.digest.clone(),
        size: observed_reference.bytes,
        materialization_key: StableId::parse("node-archive").unwrap(),
        source_metadata_digest: observed_resolution.source.metadata_digest().unwrap(),
    }];
    observed_resolution.recipe = commonkit_adapters::OfflineInstallRecipeV1::NodeArchive {
        artifact_roles: [artifact_role].into_iter().collect(),
        install: Some(commonkit_adapters::NodeOfflineInstallRecipeV1 {
            node_version: "20.0.0".into(),
            archive_file_name: "node-v20.0.0-linux-x64.tar.xz".into(),
            cache_relative_path: ".cache/bin/node-v20.0.0-linux-x64/node-v20.0.0-linux-x64.tar.xz"
                .into(),
            nvm_version: manager.version.clone(),
            nvm_script_digest: snapshot.nvm_script_digest.clone(),
            shell_executable_digest: snapshot.shell_executable_digest.clone(),
            offline: true,
            no_source_fallback: true,
            per_version_lock: true,
            install_latest_npm: false,
            migrate_packages: false,
        }),
    };
    let observed = helper.dispatch(request(
        observed_resolution,
        Vec::new(),
        Some(target_identity_digest.clone()),
    ));
    assert!(
        matches!(
        observed,
        Ok(SshFilesystemResponse::PackageObserved {
            ref installed_versions,
        }) if *installed_versions == snapshot.before.installed_versions
        ),
        "observe with persisted artifacts failed: {observed:?}"
    );

    let mut altered = resolution.clone();
    altered.source.registry_definition_digest =
        Sha256Digest::parse(format!("sha256:{}", "d".repeat(64))).unwrap();
    assert!(matches!(
        helper.dispatch(request(
            altered,
            Vec::new(),
            Some(target_identity_digest.clone()),
        )),
        Err(commonkit_adapters::TargetFilesystemError::PackageResolutionRejected)
    ));

    assert!(matches!(
        helper.dispatch(request(
            resolution.clone(),
            Vec::new(),
            Some(Sha256Digest::parse(format!("sha256:{}", "f".repeat(64))).unwrap()),
        )),
        Err(commonkit_adapters::TargetFilesystemError::PackageResolutionRejected)
    ));

    let forged_artifact = PackageMutationArtifact {
        reference: commonkit_adapters::ContentReference {
            digest: Sha256Digest::parse(format!("sha256:{}", "e".repeat(64))).unwrap(),
            bytes: 1,
            sensitivity: commonkit_adapters::ContentSensitivity::Portable,
        },
    };
    assert!(matches!(
        helper.dispatch(request(
            resolution,
            vec![forged_artifact],
            Some(target_identity_digest),
        )),
        Err(commonkit_adapters::TargetFilesystemError::RemoteArtifact)
    ));

    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(state);
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
    assert!(
        !output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
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

#[cfg(unix)]
#[test]
fn helper_rejects_a_symlinked_root_ancestor_before_writefile() {
    use std::os::unix::fs::symlink;

    let home = temp("alias-home");
    let state = temp("alias-state");
    fs::create_dir_all(home.join(".config/commonkit")).unwrap();
    fs::create_dir_all(&state).unwrap();
    let alias = home.join("managed");
    symlink(home.join(".config"), &alias).unwrap();
    fs::write(
        home.join(".config/commonkit/target-helper.json"),
        serde_json::to_vec(&serde_json::json!({
            "stateRoot": state,
            "roots": [{"id":"home","path":alias.join("commonkit"),"access":"read_write"}]
        }))
        .unwrap(),
    )
    .unwrap();

    let output = invoke(
        &home,
        &SshFilesystemRequest::WriteFile {
            root_id: StableId::parse("home").unwrap(),
            path: NormalizedManagedPath::parse("escaped.txt").unwrap(),
            content: b"must not write".to_vec(),
        },
    );
    assert!(
        !output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!home.join(".config/commonkit/escaped.txt").exists());

    let _ = fs::remove_dir_all(home);
    let _ = fs::remove_dir_all(state);
}

#[cfg(unix)]
#[test]
fn helper_rejects_group_world_writable_runtime_root_and_state() {
    use std::os::unix::fs::PermissionsExt;

    for (name, unsafe_root, unsafe_state) in [("root", true, false), ("state", false, true)] {
        let home = temp(&format!("unsafe-{name}-home"));
        let root = temp(&format!("unsafe-{name}-root"));
        let state = temp(&format!("unsafe-{name}-state"));
        fs::create_dir_all(home.join(".config/commonkit")).unwrap();
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&state).unwrap();
        if unsafe_root {
            fs::set_permissions(&root, fs::Permissions::from_mode(0o777)).unwrap();
        }
        if unsafe_state {
            fs::set_permissions(&state, fs::Permissions::from_mode(0o777)).unwrap();
        }
        fs::write(
            home.join(".config/commonkit/target-helper.json"),
            serde_json::to_vec(&serde_json::json!({
                "stateRoot": state,
                "roots": [{"id":"home","path":root,"access":"read_write"}]
            }))
            .unwrap(),
        )
        .unwrap();

        let output = invoke(
            &home,
            &SshFilesystemRequest::WriteFile {
                root_id: StableId::parse("home").unwrap(),
                path: NormalizedManagedPath::parse("blocked.txt").unwrap(),
                content: b"must not write".to_vec(),
            },
        );
        assert!(!output.status.success(), "unsafe {name} was accepted");
        assert!(!root.join("blocked.txt").exists());

        let _ = fs::remove_dir_all(home);
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(state);
    }
}

#[test]
fn helper_accepts_the_platform_temp_alias_when_it_resolves_to_private_storage() {
    let home = {
        let base = fs::canonicalize(std::env::temp_dir()).unwrap();
        base.strip_prefix("/private/var")
            .map(|relative| std::path::Path::new("/var").join(relative))
            .unwrap_or_else(|_| temp("alias-platform-home"))
            .join(format!(
                "commonkit-helper-alias-platform-home-{}",
                std::process::id()
            ))
    };
    let root = std::path::PathBuf::from(format!(
        "/tmp/commonkit-helper-alias-root-{}",
        std::process::id()
    ));
    let state = temp("alias-platform-state");
    fs::create_dir_all(home.join(".config/commonkit")).unwrap();
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&state).unwrap();
    fs::write(
        home.join(".config/commonkit/target-helper.json"),
        serde_json::to_vec(&serde_json::json!({
            "stateRoot": state,
            "roots": [{"id":"home","path":root,"access":"read_write"}]
        }))
        .unwrap(),
    )
    .unwrap();

    let output = invoke(
        &home,
        &SshFilesystemRequest::WriteFile {
            root_id: StableId::parse("home").unwrap(),
            path: NormalizedManagedPath::parse("alias.txt").unwrap(),
            content: b"platform alias".to_vec(),
        },
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read(root.join("alias.txt")).unwrap(), b"platform alias");

    let _ = fs::remove_dir_all(home);
    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(state);
}

#[test]
fn helper_keeps_writing_to_the_bound_root_after_path_swap() {
    let root = temp("swap-root");
    let replacement = temp("swap-replacement");
    let state = temp("swap-state");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&state).unwrap();
    let helper = TargetHelper::open(
        vec![TargetRoot {
            id: StableId::parse("home").unwrap(),
            path: root.to_string_lossy().into_owned(),
            access: RootAccess::ReadWrite,
        }],
        &state,
    )
    .unwrap();
    fs::rename(&root, &replacement).unwrap();
    fs::create_dir_all(&root).unwrap();

    helper
        .dispatch(SshFilesystemRequest::WriteFile {
            root_id: StableId::parse("home").unwrap(),
            path: NormalizedManagedPath::parse("bound.txt").unwrap(),
            content: b"bound handle".to_vec(),
        })
        .unwrap();
    assert_eq!(
        fs::read(replacement.join("bound.txt")).unwrap(),
        b"bound handle"
    );
    assert!(!root.join("bound.txt").exists());

    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(replacement);
    let _ = fs::remove_dir_all(state);
}
