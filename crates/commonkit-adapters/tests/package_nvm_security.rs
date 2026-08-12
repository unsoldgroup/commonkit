use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use commonkit_adapters::{
    ArtifactEvidence, ManagerBindingV1, NodeOfflineInstallRecipeV1, NodeRuntimeHost,
    OfflineInstallRecipeV1, PackageMutationBackend, PackageObservationV1, PackageResolutionV1,
    PackageTargetV1, ProcessNodeRuntimeHost, ProcessOfflinePackageBackend, ResolvedPackage,
    SourceBindingV1, TargetNodeResolutionConfig, TargetPackageResolutionConfig,
};
use commonkit_contracts::SecurityPolicy;
use commonkit_contracts::{
    PackageDeclaration, PackageManager, PackageSelector, SchemaVersion, Sha256Digest, StableId,
};
use sha2::{Digest, Sha256};

fn digest(bytes: &[u8]) -> Sha256Digest {
    Sha256Digest::parse(format!("sha256:{:x}", Sha256::digest(bytes))).unwrap()
}

fn resolution(target_root: &Path, script_digest: Sha256Digest) -> PackageResolutionV1 {
    let source_id = StableId::parse("nodejs").unwrap();
    let declaration = PackageDeclaration {
        id: StableId::parse("node").unwrap(),
        version: "20.0.0".into(),
        manager: PackageManager::Nvm,
        source: source_id.clone(),
        selector: Some(PackageSelector::NodeRuntime {}),
    };
    let source = SourceBindingV1 {
        source_id,
        registry_definition_digest: digest(b"registry"),
        canonical_repository: "https://nodejs.org/dist".into(),
        repository_revision: None,
        signed_metadata: Vec::new(),
    };
    PackageResolutionV1 {
        schema_version: SchemaVersion(1),
        declaration: declaration.clone(),
        target: PackageTargetV1 {
            os: "linux".into(),
            os_version: "24.04".into(),
            distro_id: Some("ubuntu".into()),
            distro_version: Some("24.04".into()),
            codename: Some("noble".into()),
            arch: "x86_64".into(),
            libc: Some("glibc".into()),
            manager_prefix: Some(target_root.join(".nvm").to_string_lossy().into_owned()),
        },
        manager: ManagerBindingV1 {
            manager: PackageManager::Nvm,
            version: "0.40.6".into(),
            executable_digest: script_digest.clone(),
            config_digest: digest(b"config"),
        },
        source: source.clone(),
        before: PackageObservationV1 {
            installed_versions: BTreeSet::new(),
        },
        closure: vec![ResolvedPackage {
            declaration,
            source,
        }],
        artifacts: Vec::new(),
        recipe: OfflineInstallRecipeV1::NodeArchive {
            artifact_roles: BTreeSet::new(),
            install: Some(NodeOfflineInstallRecipeV1 {
                node_version: "20.0.0".into(),
                archive_file_name: "node-v20.0.0-linux-x64.tar.xz".into(),
                cache_relative_path: ".cache/node.tar.xz".into(),
                nvm_version: "0.40.6".into(),
                nvm_script_digest: script_digest,
                shell_executable_digest: digest(b"shell"),
                offline: true,
                no_source_fallback: true,
                per_version_lock: true,
                install_latest_npm: false,
                migrate_packages: false,
            }),
        },
    }
}

fn live_binding_fixture(
    root: &Path,
) -> (
    PackageResolutionV1,
    TargetPackageResolutionConfig,
    std::path::PathBuf,
) {
    let nvm = root.join(".nvm");
    fs::create_dir_all(&nvm).unwrap();
    let script = include_bytes!("fixtures/node/nvm-v0.40.6.sh").to_vec();
    fs::write(nvm.join("nvm.sh"), &script).unwrap();
    let shell = Path::new("/bin/bash").to_path_buf();
    let keyring = root.join("node-release-keyring.kbx");
    let gpgv = root.join("gpgv");
    fs::write(&keyring, b"fixture keyring").unwrap();
    fs::copy("/bin/bash", &gpgv).unwrap();
    #[cfg(unix)]
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
    let config = TargetPackageResolutionConfig {
        target: target.clone(),
        manager: snapshot.manager.clone(),
        policy: SecurityPolicy::default(),
        apt: None,
        node: Some(TargetNodeResolutionConfig {
            nvm_dir: nvm.clone(),
            shell_executable: shell,
            release_keyring: keyring,
            gpgv_executable: gpgv,
            gpgv_executable_digest: snapshot.gpgv_executable_digest,
            release_keyring_digest: Some(snapshot.release_keyring_digest),
        }),
        target_identity_digest: digest(b"target"),
    };
    let mut resolution = resolution(root, snapshot.nvm_script_digest.clone());
    resolution.schema_version = SchemaVersion(2);
    resolution.target = target;
    resolution.manager = snapshot.manager;
    resolution.recipe = match resolution.recipe {
        OfflineInstallRecipeV1::NodeArchive {
            artifact_roles,
            install: Some(mut install),
        } => {
            install.nvm_version = resolution.manager.version.clone();
            install.nvm_script_digest = snapshot.nvm_script_digest.clone();
            install.shell_executable_digest = snapshot.shell_executable_digest;
            OfflineInstallRecipeV1::NodeArchive {
                artifact_roles,
                install: Some(install),
            }
        }
        recipe => recipe,
    };
    (resolution, config, nvm.join("nvm.sh"))
}

#[test]
fn nvm_apply_rejects_forged_source_evidence_before_mutation() {
    let root = tempfile::tempdir().unwrap();
    let (mut resolution, config, _) = live_binding_fixture(root.path());
    resolution.source.signed_metadata = vec![ArtifactEvidence {
        authority: commonkit_contracts::StableId::parse("node-release-key").unwrap(),
        metadata_digest: digest(b"forged-checksums"),
        signature_digest: digest(b"forged-signature"),
    }];
    resolution.closure[0].source = resolution.source.clone();
    let artifacts =
        commonkit_adapters::ArtifactStore::open(tempfile::tempdir().unwrap().path()).unwrap();
    let mut backend = ProcessOfflinePackageBackend::with_package_resolution(root.path(), config);

    assert!(
        backend.prepare_offline(&resolution, &artifacts).is_err(),
        "forged matching source evidence must be rejected before a package phase"
    );
}

#[test]
fn nvm_v2_apply_rejects_missing_source_evidence_before_mutation() {
    let root = tempfile::tempdir().unwrap();
    let (resolution, config, _) = live_binding_fixture(root.path());
    assert_eq!(resolution.schema_version, SchemaVersion(2));
    assert!(resolution.source.signed_metadata.is_empty());
    let artifacts =
        commonkit_adapters::ArtifactStore::open(tempfile::tempdir().unwrap().path()).unwrap();
    let mut backend = ProcessOfflinePackageBackend::with_package_resolution(root.path(), config);

    assert!(backend.prepare_offline(&resolution, &artifacts).is_err());
}

#[test]
fn nvm_v2_prepare_rejects_missing_source_evidence() {
    let root = tempfile::tempdir().unwrap();
    let (resolution, config, _) = live_binding_fixture(root.path());
    let node = config.node.as_ref().unwrap();
    let mut host = ProcessNodeRuntimeHost::new_with_gpgv(
        node.nvm_dir.clone(),
        node.shell_executable.clone(),
        node.release_keyring.clone(),
        node.gpgv_executable.clone(),
    );
    assert_eq!(
        host.probe(&config.target).unwrap().manager,
        resolution.manager
    );
    let mut backend = ProcessOfflinePackageBackend::with_package_resolution(root.path(), config);

    let artifacts =
        commonkit_adapters::ArtifactStore::open(tempfile::tempdir().unwrap().path()).unwrap();
    assert!(backend.prepare_offline(&resolution, &artifacts).is_err());
}

#[test]
fn nvm_observe_rejects_live_nvm_script_drift_before_sourcing() {
    let root = tempfile::tempdir().unwrap();
    let (resolution, config, script) = live_binding_fixture(root.path());
    fs::write(&script, b"nvm() { touch drifted; }\n").unwrap();
    let mut backend = ProcessOfflinePackageBackend::with_package_resolution(root.path(), config);

    let artifacts =
        commonkit_adapters::ArtifactStore::open(tempfile::tempdir().unwrap().path()).unwrap();
    assert!(backend.prepare_offline(&resolution, &artifacts).is_err());
    assert!(!root.path().join("drifted").exists());
}

#[test]
fn nvm_observe_rejects_live_manager_config_and_shell_drift() {
    let root = tempfile::tempdir().unwrap();
    let (resolution, config, _) = live_binding_fixture(root.path());
    fs::write(
        config.node.as_ref().unwrap().release_keyring.as_path(),
        b"changed",
    )
    .unwrap();
    let mut backend = ProcessOfflinePackageBackend::with_package_resolution(root.path(), config);
    let artifacts =
        commonkit_adapters::ArtifactStore::open(tempfile::tempdir().unwrap().path()).unwrap();
    assert!(backend.prepare_offline(&resolution, &artifacts).is_err());

    let root = tempfile::tempdir().unwrap();
    let (resolution, mut config, _) = live_binding_fixture(root.path());
    let shell = root.path().join("shell");
    fs::copy("/bin/sh", &shell).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&shell, fs::Permissions::from_mode(0o700)).unwrap();
    }
    config.node.as_mut().unwrap().shell_executable = shell;
    let mut backend = ProcessOfflinePackageBackend::with_package_resolution(root.path(), config);
    let artifacts =
        commonkit_adapters::ArtifactStore::open(tempfile::tempdir().unwrap().path()).unwrap();
    assert!(backend.prepare_offline(&resolution, &artifacts).is_err());
}

#[test]
fn production_backend_rejects_package_phases_without_target_manager_authority() {
    let root = tempfile::tempdir().unwrap();
    let script = b"nvm() { printf '%s\\n' '-> v20.0.0 *'; }\\n";
    fs::create_dir(root.path().join(".nvm")).unwrap();
    fs::write(root.path().join(".nvm/nvm.sh"), script).unwrap();
    let resolution = resolution(root.path(), digest(script));
    let artifacts =
        commonkit_adapters::ArtifactStore::open(tempfile::tempdir().unwrap().path()).unwrap();
    let mut backend =
        ProcessOfflinePackageBackend::new(root.path()).require_target_package_resolution();

    assert!(backend.prepare_offline(&resolution, &artifacts).is_err());
}

#[test]
fn nvm_observe_rejects_script_drift_before_sourcing_or_side_effects() {
    let root = tempfile::tempdir().unwrap();
    let nvm = root.path().join(".nvm");
    fs::create_dir(&nvm).unwrap();
    let marker = root.path().join("sourced");
    let script = format!(
        "touch '{}'\nnvm() {{ printf '%s\\n' '-> v20.0.0 *'; }}\n",
        marker.display()
    );
    let script_bytes = script.as_bytes();
    fs::write(nvm.join("nvm.sh"), script_bytes).unwrap();
    let bound = digest(b"the-already-bound-script");
    let mut backend = ProcessOfflinePackageBackend::new(root.path());

    let result = backend.observe(&resolution(root.path(), bound));

    assert!(result.is_err(), "drifted nvm.sh must fail closed");
    assert!(!marker.exists(), "drift must be rejected before sourcing");
}

#[test]
fn nvm_observe_uses_a_scrubbed_closed_environment() {
    let root = tempfile::tempdir().unwrap();
    let nvm = root.path().join(".nvm");
    fs::create_dir(&nvm).unwrap();
    let marker = root.path().join("inherited-home-used");
    let script = format!(
        "if [ \"$HOME\" != '{}' ]; then touch '{}'; exit 77; fi\nnvm() {{ printf '%s\\n' '-> v20.0.0 *'; }}\n",
        root.path().display(),
        marker.display()
    );
    let script_bytes = script.as_bytes();
    fs::write(nvm.join("nvm.sh"), script_bytes).unwrap();
    let mut backend = ProcessOfflinePackageBackend::new(root.path());

    backend
        .observe(&resolution(root.path(), digest(script_bytes)))
        .unwrap();

    assert!(
        !marker.exists(),
        "nvm must not receive the controller environment"
    );
}

#[test]
fn nvm_observe_rejects_malformed_final_versions() {
    let output = "-> v20.0.0-extra";
    let root = tempfile::tempdir().unwrap();
    let script = format!("nvm() {{ printf '%s\\n' '{}'; }}\n", output);
    let script_bytes = script.as_bytes();
    fs::create_dir(root.path().join(".nvm")).unwrap();
    fs::write(root.path().join(".nvm/nvm.sh"), script_bytes).unwrap();
    let mut backend = ProcessOfflinePackageBackend::new(root.path());

    assert!(
        backend
            .observe(&resolution(root.path(), digest(script_bytes)))
            .is_err(),
        "malformed NVM observations must fail closed: {output}"
    );
}

#[test]
fn nvm_observe_accepts_real_alias_rows_and_rejects_junk_only_output() {
    let outputs = [
        (
            "v22.17.0 *\niojs -> N/A (default)\nnode -> stable (-> v22.17.0) (default)\nstable -> 22.17 (-> v22.17.0)\nunstable -> N/A (default)\n",
            true,
        ),
        (
            "node -> stable (default)\nnot an nvm listing v20.0.0\n",
            false,
        ),
    ];
    for (output, valid) in outputs {
        let root = tempfile::tempdir().unwrap();
        let script = format!("nvm() {{ printf '%s' '{}'; }}\n", output);
        let script_bytes = script.as_bytes();
        fs::create_dir(root.path().join(".nvm")).unwrap();
        fs::write(root.path().join(".nvm/nvm.sh"), script_bytes).unwrap();
        let mut backend = ProcessOfflinePackageBackend::new(root.path());

        let result = backend.observe(&resolution(root.path(), digest(script_bytes)));
        assert_eq!(
            result.is_ok(),
            valid,
            "unexpected NVM output result: {output}"
        );
        if valid {
            assert_eq!(
                result.unwrap().installed_versions,
                BTreeSet::from(["22.17.0".into()])
            );
        }
    }
}

#[test]
fn nvm_observe_accepts_padded_current_and_system_rows() {
    let output = "->     v20.19.0 *\n->       system * (-> v20.19.0)\n";
    let root = tempfile::tempdir().unwrap();
    let script = format!("nvm() {{ printf '%s' '{}'; }}\n", output);
    let script_bytes = script.as_bytes();
    fs::create_dir(root.path().join(".nvm")).unwrap();
    fs::write(root.path().join(".nvm/nvm.sh"), script_bytes).unwrap();
    let mut backend = ProcessOfflinePackageBackend::new(root.path());

    let observed = backend
        .observe(&resolution(root.path(), digest(script_bytes)))
        .unwrap();
    assert_eq!(
        observed.installed_versions,
        BTreeSet::from(["20.19.0".into()])
    );
}

#[test]
fn nvm_observe_rejects_unresolved_or_malformed_current_rows() {
    let outputs = [
        "->       system *\n",
        "->       system * (-> N/A)\n",
        "->     v20.19.0 unexpected\n",
    ];
    for output in outputs {
        let root = tempfile::tempdir().unwrap();
        let script = format!("nvm() {{ printf '%s' '{}'; }}\n", output);
        let script_bytes = script.as_bytes();
        fs::create_dir(root.path().join(".nvm")).unwrap();
        fs::write(root.path().join(".nvm/nvm.sh"), script_bytes).unwrap();
        let mut backend = ProcessOfflinePackageBackend::new(root.path());

        assert!(
            backend
                .observe(&resolution(root.path(), digest(script_bytes)))
                .is_err(),
            "malformed or unresolved current row must fail closed: {output}"
        );
    }
}

#[test]
fn nvm_observe_rejects_missing_arrow_spaces_and_repeated_markers() {
    let outputs = [
        "->v20.19.0\n",
        "v20.19.0\n",
        "-> v20.19.0\n",
        "->system * (-> v20.19.0)\n",
        "node -> stable (->v20.19.0)\n",
        "v20.19.0 * *\n",
        "-> v20.19.0 * *\n",
        "-> system * * (-> v20.19.0)\n",
        "node -> stable (-> v20.19.0 * *)\n",
    ];
    for output in outputs {
        let root = tempfile::tempdir().unwrap();
        let script = format!("nvm() {{ printf '%s' '{}'; }}\n", output);
        let script_bytes = script.as_bytes();
        fs::create_dir(root.path().join(".nvm")).unwrap();
        fs::write(root.path().join(".nvm/nvm.sh"), script_bytes).unwrap();
        let mut backend = ProcessOfflinePackageBackend::new(root.path());

        assert!(
            backend
                .observe(&resolution(root.path(), digest(script_bytes)))
                .is_err(),
            "NVM must reject non-canonical observation: {output}"
        );
    }
}

#[test]
fn nvm_observe_accepts_legitimate_no_installed_state() {
    let output = "N/A *\niojs -> N/A (default)\nnode -> stable (-> N/A) (default)\nunstable -> N/A (default)\n";
    let root = tempfile::tempdir().unwrap();
    let script = format!("nvm() {{ printf '%s' '{}'; return 3; }}\n", output);
    let script_bytes = script.as_bytes();
    fs::create_dir(root.path().join(".nvm")).unwrap();
    fs::write(root.path().join(".nvm/nvm.sh"), script_bytes).unwrap();
    let mut backend = ProcessOfflinePackageBackend::new(root.path());

    let observed = backend
        .observe(&resolution(root.path(), digest(script_bytes)))
        .unwrap();
    assert!(observed.installed_versions.is_empty());
}

#[test]
fn nvm_observe_rejects_fake_version_in_malformed_alias_row() {
    let output = "N/A *\njunk -> nonsense v20.0.0 (-> N/A)\n";
    let root = tempfile::tempdir().unwrap();
    let script = format!("nvm() {{ printf '%s' '{}'; return 3; }}\n", output);
    let script_bytes = script.as_bytes();
    fs::create_dir(root.path().join(".nvm")).unwrap();
    fs::write(root.path().join(".nvm/nvm.sh"), script_bytes).unwrap();
    let mut backend = ProcessOfflinePackageBackend::new(root.path());

    assert!(
        backend
            .observe(&resolution(root.path(), digest(script_bytes)))
            .is_err()
    );
}

#[test]
fn nvm_observe_rejects_manager_prefix_that_does_not_match_backend_target() {
    let root = tempfile::tempdir().unwrap();
    let nvm = root.path().join(".nvm");
    fs::create_dir(&nvm).unwrap();
    let script = b"nvm() { printf '%s\\n' '-> v20.0.0 *'; }\n";
    fs::write(nvm.join("nvm.sh"), script).unwrap();
    let mut mismatched = resolution(root.path(), digest(script));
    mismatched.target.manager_prefix = Some("/another-target/.nvm".into());
    let mut backend = ProcessOfflinePackageBackend::new(root.path());

    assert!(
        backend.observe(&mismatched).is_err(),
        "NVM must reject a resolution bound to a different target directory"
    );
}

#[cfg(unix)]
#[test]
fn bound_nvm_mutation_survives_a_post_start_root_swap() {
    use std::fs::OpenOptions;
    use std::os::unix::fs::OpenOptionsExt;

    let root = tempfile::tempdir().unwrap();
    let nvm = root.path().join(".nvm");
    fs::create_dir_all(&nvm).unwrap();
    let marker = "observed-on-bound-root";
    let script = format!("nvm() {{ printf '%s\\n' '-> v20.0.0 *'; touch \"$HOME/{marker}\"; }}\n");
    let script = script.as_bytes();
    fs::write(nvm.join("nvm.sh"), script).unwrap();
    let handle = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(root.path())
        .unwrap();
    let mut backend =
        ProcessOfflinePackageBackend::new_with_bound_root(root.path(), handle).unwrap();
    let resolution = resolution(root.path(), digest(script));
    let replacement = tempfile::tempdir().unwrap();
    fs::rename(root.path(), replacement.path().join("original")).unwrap();
    fs::create_dir_all(root.path()).unwrap();

    let observed = backend.observe(&resolution).unwrap();
    assert!(observed.installed_versions.contains("20.0.0"));
    assert!(
        replacement
            .path()
            .join("original/observed-on-bound-root")
            .exists()
    );
    assert!(!root.path().join("observed-on-bound-root").exists());
}

#[cfg(unix)]
#[test]
fn bound_nvm_rejects_a_symlinked_nvm_ancestor() {
    use std::fs::OpenOptions;
    use std::os::unix::fs::OpenOptionsExt;

    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let script = b"nvm() { printf '%s\\n' '-> v20.0.0 *'; touch \"$HOME/escaped\"; }\n";
    fs::write(outside.path().join("nvm.sh"), script).unwrap();
    std::os::unix::fs::symlink(outside.path(), root.path().join(".nvm")).unwrap();
    let handle = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(root.path())
        .unwrap();
    let mut backend =
        ProcessOfflinePackageBackend::new_with_bound_root(root.path(), handle).unwrap();
    let resolution = resolution(root.path(), digest(script));

    assert!(backend.observe(&resolution).is_err());
    assert!(!outside.path().join("escaped").exists());
}

#[cfg(unix)]
#[test]
fn bound_nvm_observe_keeps_the_nvm_directory_bound_after_a_path_swap() {
    use std::fs::OpenOptions;
    use std::os::unix::fs::OpenOptionsExt;

    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let script = format!(
        "mv '{}' '{}-original'\nln -s '{}' '{}'\nif [ ! -f \"$NVM_DIR/nvm.sh\" ]; then exit 71; fi\ntouch \"$NVM_DIR/bound-marker\"\nnvm() {{ printf '%s\\n' '-> v20.0.0 *'; }}\n",
        root.path().join(".nvm").display(),
        root.path().join(".nvm").display(),
        outside.path().display(),
        root.path().join(".nvm").display()
    );
    let script = script.as_bytes();
    fs::create_dir_all(root.path().join(".nvm")).unwrap();
    fs::write(root.path().join(".nvm/nvm.sh"), script).unwrap();
    let handle = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(root.path())
        .unwrap();
    let mut backend =
        ProcessOfflinePackageBackend::new_with_bound_root(root.path(), handle).unwrap();

    let observed = backend
        .observe(&resolution(root.path(), digest(script)))
        .unwrap();

    assert!(observed.installed_versions.contains("20.0.0"));
    assert!(root.path().join(".nvm").is_symlink());
    assert!(root.path().join(".nvm-original/bound-marker").exists());
    assert!(!outside.path().join("bound-marker").exists());
}
