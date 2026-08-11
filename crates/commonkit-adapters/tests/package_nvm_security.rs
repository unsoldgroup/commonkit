use std::collections::BTreeSet;
use std::fs;

use commonkit_adapters::{
    ManagerBindingV1, NodeOfflineInstallRecipeV1, OfflineInstallRecipeV1, PackageMutationBackend,
    PackageObservationV1, PackageResolutionV1, PackageTargetV1, ProcessOfflinePackageBackend,
    ResolvedPackage, SourceBindingV1,
};
use commonkit_contracts::{
    PackageDeclaration, PackageManager, PackageSelector, SchemaVersion, Sha256Digest, StableId,
};
use sha2::{Digest, Sha256};

fn digest(bytes: &[u8]) -> Sha256Digest {
    Sha256Digest::parse(format!("sha256:{:x}", Sha256::digest(bytes))).unwrap()
}

fn resolution(script_digest: Sha256Digest) -> PackageResolutionV1 {
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
            manager_prefix: Some("/target/.nvm".into()),
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

#[test]
fn nvm_observe_rejects_script_drift_before_sourcing_or_side_effects() {
    let root = tempfile::tempdir().unwrap();
    let nvm = root.path().join(".nvm");
    fs::create_dir(&nvm).unwrap();
    let marker = root.path().join("sourced");
    let script = format!(
        "touch '{}'\nnvm() {{ printf '%s\\n' '-> v20.0.0'; }}\n",
        marker.display()
    );
    let script_bytes = script.as_bytes();
    fs::write(nvm.join("nvm.sh"), script_bytes).unwrap();
    let bound = digest(b"the-already-bound-script");
    let mut backend = ProcessOfflinePackageBackend::new(root.path());

    let result = backend.observe(&resolution(bound));

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
        "if [ \"$HOME\" != '{}' ]; then touch '{}'; exit 77; fi\nnvm() {{ printf '%s\\n' '-> v20.0.0'; }}\n",
        root.path().display(),
        marker.display()
    );
    let script_bytes = script.as_bytes();
    fs::write(nvm.join("nvm.sh"), script_bytes).unwrap();
    let mut backend = ProcessOfflinePackageBackend::new(root.path());

    backend.observe(&resolution(digest(script_bytes))).unwrap();

    assert!(
        !marker.exists(),
        "nvm must not receive the controller environment"
    );
}
