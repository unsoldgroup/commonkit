use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use commonkit_adapters::{
    ArtifactStore, ContentSensitivity, ManagerBindingV1, OfflineInstallRecipeV1, PackageAdapter,
    PackageMutationBackend, PackageMutationError, PackageObservationV1, PackageResolutionAuthority,
    PackageResolutionV1, PackageSourceRegistry, PackageTargetV1, ProcessOfflinePackageBackend,
    ResolvedPackage, SourceBindingV1,
};
use commonkit_contracts::{
    Operation, OperationKind, PackageDeclaration, PackageManager, PackageSelector,
    RecoveryCapability, ResourceRef, Risk, SchemaVersion, SecurityPolicy, Sha256Digest, StableId,
};
use commonkit_core::OperationDraft;
use commonkit_reconcile::{Adapter, RecoveryObservation};

#[test]
fn package_adapter_api_exposes_only_the_typed_offline_backend_seam() {
    fn assert_send<T: Send>() {}

    assert_send::<PackageAdapter>();
    let _typed_backend: Option<Box<dyn PackageMutationBackend>> = None;
}

fn digest(seed: char) -> Sha256Digest {
    Sha256Digest::parse(format!("sha256:{}", seed.to_string().repeat(64))).unwrap()
}

#[derive(Clone)]
struct FakeMutationBackend {
    manager: PackageManager,
    target_supported: bool,
    observed: PackageObservationV1,
    calls: Arc<Mutex<Vec<&'static str>>>,
}

impl PackageMutationBackend for FakeMutationBackend {
    fn manager(&self) -> PackageManager {
        self.manager
    }

    fn supports_target(&self, _resolution: &PackageResolutionV1) -> bool {
        self.target_supported
    }

    fn observe(
        &mut self,
        _resolution: &PackageResolutionV1,
    ) -> Result<PackageObservationV1, PackageMutationError> {
        self.calls.lock().unwrap().push("observe");
        Ok(self.observed.clone())
    }

    fn prepare_offline(
        &mut self,
        _resolution: &PackageResolutionV1,
        _artifacts: &ArtifactStore,
    ) -> Result<(), PackageMutationError> {
        self.calls.lock().unwrap().push("prepare");
        Ok(())
    }

    fn apply_offline(
        &mut self,
        _resolution: &PackageResolutionV1,
        _artifacts: &ArtifactStore,
    ) -> Result<(), PackageMutationError> {
        self.calls.lock().unwrap().push("apply");
        Ok(())
    }

    fn verify_offline(
        &mut self,
        _resolution: &PackageResolutionV1,
        _artifacts: &ArtifactStore,
    ) -> Result<(), PackageMutationError> {
        self.calls.lock().unwrap().push("verify");
        Ok(())
    }
}

fn apt_resolution() -> (PackageResolutionV1, PackageResolutionAuthority) {
    let source = StableId::parse("ubuntu-main").unwrap();
    let target = PackageTargetV1 {
        os: "linux".into(),
        os_version: "24.04".into(),
        distro_id: Some("ubuntu".into()),
        distro_version: Some("24.04".into()),
        codename: Some("noble".into()),
        arch: "amd64".into(),
        libc: Some("glibc".into()),
        manager_prefix: None,
    };
    let manager = ManagerBindingV1 {
        manager: PackageManager::Apt,
        version: "2.7.14".into(),
        executable_digest: digest('a'),
        config_digest: digest('b'),
    };
    let registry = PackageSourceRegistry::builtin().unwrap();
    let policy = SecurityPolicy {
        allowlists: [(
            StableId::parse("package_sources").unwrap(),
            BTreeSet::from(["ubuntu-main".into()]),
        )]
        .into_iter()
        .collect(),
        ..SecurityPolicy::default()
    };
    let authority = PackageResolutionAuthority::new(&target, &manager, &registry, &policy).unwrap();
    let declaration = PackageDeclaration {
        id: StableId::parse("ripgrep").unwrap(),
        version: "14.1.1-1ubuntu1".into(),
        manager: PackageManager::Apt,
        source: source.clone(),
        selector: Some(PackageSelector::AptBinary {
            name: "ripgrep".into(),
            architecture: Some("amd64".into()),
        }),
    };
    let source_binding = SourceBindingV1 {
        source_id: source,
        registry_definition_digest: registry
            .source_definition_digest(&StableId::parse("ubuntu-main").unwrap())
            .unwrap(),
        canonical_repository: "https://archive.ubuntu.com/ubuntu".into(),
        repository_revision: Some(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".into(),
        ),
        signed_metadata: Vec::new(),
    };
    let resolution = PackageResolutionV1 {
        schema_version: SchemaVersion(1),
        declaration: declaration.clone(),
        target,
        manager,
        source: source_binding.clone(),
        before: commonkit_adapters::PackageObservationV1 {
            installed_versions: BTreeSet::new(),
        },
        closure: vec![ResolvedPackage {
            declaration,
            source: source_binding,
        }],
        artifacts: Vec::new(),
        recipe: OfflineInstallRecipeV1::AptArchives {
            artifact_roles: BTreeSet::new(),
        },
    };
    (resolution, authority)
}

fn operation(resolution_digest: Sha256Digest) -> Operation {
    commonkit_core::finalize_operation(OperationDraft {
        adapter_id: StableId::parse("packages").unwrap(),
        kind: OperationKind::Create,
        resource: ResourceRef {
            resource_type: StableId::parse("package").unwrap(),
            resource_id: StableId::parse("ripgrep").unwrap(),
            managed_path: None,
        },
        risk: Risk::Medium,
        requires_confirmation: true,
        recovery_capability: RecoveryCapability::ConvergeForwardOnly,
        depends_on: Vec::new(),
        before_digest: None,
        after_digest: Some(digest('c')),
        payload_digest: resolution_digest,
        provenance: None,
        summary: "install ripgrep".into(),
    })
    .unwrap()
}

#[test]
fn approved_apt_resolution_delegates_only_offline_mutation_calls() {
    let (resolution, authority) = apt_resolution();
    let root = tempfile::tempdir().unwrap();
    let artifacts = ArtifactStore::open(root.path()).unwrap();
    let bytes = serde_json::to_vec(&resolution).unwrap();
    let resolution_ref = artifacts.put(&bytes, ContentSensitivity::Portable).unwrap();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let backend = FakeMutationBackend {
        manager: PackageManager::Apt,
        target_supported: true,
        observed: resolution.before.clone(),
        calls: calls.clone(),
    };
    let mut adapter = PackageAdapter::new(authority, artifacts, Box::new(backend));
    let operation = operation(resolution_ref.digest);

    adapter.preflight(&operation).unwrap();
    adapter.prepare(&operation).unwrap();
    adapter.apply(&operation).unwrap();
    adapter.verify(&operation).unwrap();
    assert_eq!(*calls.lock().unwrap(), vec!["prepare", "apply", "verify"]);
}

#[test]
fn unsupported_manager_fails_before_backend_mutation() {
    let (mut resolution, authority) = apt_resolution();
    resolution.recipe = OfflineInstallRecipeV1::HomebrewBottle {
        artifact_roles: BTreeSet::new(),
    };
    let root = tempfile::tempdir().unwrap();
    let artifacts = ArtifactStore::open(root.path()).unwrap();
    let resolution_ref = artifacts
        .put(
            &serde_json::to_vec(&resolution).unwrap(),
            ContentSensitivity::Portable,
        )
        .unwrap();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let backend = FakeMutationBackend {
        manager: PackageManager::Apt,
        target_supported: true,
        observed: resolution.before.clone(),
        calls: calls.clone(),
    };
    let mut adapter = PackageAdapter::new(authority, artifacts, Box::new(backend));
    let error = adapter
        .preflight(&operation(resolution_ref.digest))
        .unwrap_err();
    assert_eq!(error.code, "package_manager_unsupported");
    assert!(calls.lock().unwrap().is_empty());
}

#[test]
fn apt_recovery_rejects_ambiguous_all_and_native_identity_before_backend_mutation() {
    let (mut resolution, authority) = apt_resolution();
    let mut ambiguous = resolution.closure[0].clone();
    ambiguous.declaration.id = StableId::parse("zz-dep-0123456789abcdef012345678").unwrap();
    ambiguous.declaration.selector = Some(PackageSelector::AptBinary {
        name: "ripgrep".into(),
        architecture: Some("all".into()),
    });
    resolution.closure.push(ambiguous);

    let root = tempfile::tempdir().unwrap();
    let artifacts = ArtifactStore::open(root.path()).unwrap();
    let resolution_ref = artifacts
        .put(
            &serde_json::to_vec(&resolution).unwrap(),
            ContentSensitivity::Portable,
        )
        .unwrap();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let backend = FakeMutationBackend {
        manager: PackageManager::Apt,
        target_supported: true,
        observed: resolution.before.clone(),
        calls: calls.clone(),
    };
    let mut adapter = PackageAdapter::new(authority, artifacts, Box::new(backend));

    let error = adapter
        .preflight(&operation(resolution_ref.digest))
        .expect_err("ambiguous APT identities must fail closed");
    assert_eq!(error.code, "package_apt_identity_invalid");
    assert!(calls.lock().unwrap().is_empty());
}

#[test]
fn apt_recovery_rejects_foreign_architecture_before_backend_mutation() {
    let (mut resolution, authority) = apt_resolution();
    let mut foreign = resolution.closure[0].clone();
    foreign.declaration.id = StableId::parse("zz-dep-0123456789abcdef012345678").unwrap();
    foreign.declaration.selector = Some(PackageSelector::AptBinary {
        name: "ripgrep".into(),
        architecture: Some("arm64".into()),
    });
    resolution.closure.push(foreign);

    let root = tempfile::tempdir().unwrap();
    let artifacts = ArtifactStore::open(root.path()).unwrap();
    let resolution_ref = artifacts
        .put(
            &serde_json::to_vec(&resolution).unwrap(),
            ContentSensitivity::Portable,
        )
        .unwrap();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let backend = FakeMutationBackend {
        manager: PackageManager::Apt,
        target_supported: true,
        observed: resolution.before.clone(),
        calls: calls.clone(),
    };
    let mut adapter = PackageAdapter::new(authority, artifacts, Box::new(backend));

    let error = adapter
        .preflight(&operation(resolution_ref.digest))
        .expect_err("foreign APT architectures must fail closed");
    assert_eq!(error.code, "package_apt_identity_invalid");
    assert!(calls.lock().unwrap().is_empty());
}

#[test]
fn apt_recovery_does_not_treat_wrong_real_identity_as_after() {
    let (resolution, authority) = apt_resolution();
    let root = tempfile::tempdir().unwrap();
    let artifacts = ArtifactStore::open(root.path()).unwrap();
    let resolution_ref = artifacts
        .put(
            &serde_json::to_vec(&resolution).unwrap(),
            ContentSensitivity::Portable,
        )
        .unwrap();
    let backend = FakeMutationBackend {
        manager: PackageManager::Apt,
        target_supported: true,
        observed: PackageObservationV1 {
            installed_versions: BTreeSet::from(["logical-root:amd64=14.1.1-1ubuntu1".into()]),
        },
        calls: Arc::new(Mutex::new(Vec::new())),
    };
    let mut adapter = PackageAdapter::new(authority, artifacts, Box::new(backend));

    assert_eq!(
        adapter.observe_recovery(&operation(resolution_ref.digest)),
        Ok(RecoveryObservation::Other)
    );
}

#[test]
fn apt_recovery_does_not_treat_ambiguous_observed_architectures_as_after() {
    let (resolution, authority) = apt_resolution();
    let root = tempfile::tempdir().unwrap();
    let artifacts = ArtifactStore::open(root.path()).unwrap();
    let resolution_ref = artifacts
        .put(
            &serde_json::to_vec(&resolution).unwrap(),
            ContentSensitivity::Portable,
        )
        .unwrap();
    let backend = FakeMutationBackend {
        manager: PackageManager::Apt,
        target_supported: true,
        observed: PackageObservationV1 {
            installed_versions: BTreeSet::from([
                "ripgrep:all=14.1.1-1ubuntu1".into(),
                "ripgrep:amd64=14.1.1-1ubuntu1".into(),
            ]),
        },
        calls: Arc::new(Mutex::new(Vec::new())),
    };
    let mut adapter = PackageAdapter::new(authority, artifacts, Box::new(backend));

    assert_eq!(
        adapter.observe_recovery(&operation(resolution_ref.digest)),
        Ok(RecoveryObservation::Other)
    );
}

#[test]
fn apt_recovery_does_not_treat_malformed_before_identity_as_before() {
    let (mut resolution, authority) = apt_resolution();
    resolution.before = PackageObservationV1 {
        installed_versions: BTreeSet::from(["ripgrep=14.1.1-1ubuntu1".into()]),
    };
    let root = tempfile::tempdir().unwrap();
    let artifacts = ArtifactStore::open(root.path()).unwrap();
    let resolution_ref = artifacts
        .put(
            &serde_json::to_vec(&resolution).unwrap(),
            ContentSensitivity::Portable,
        )
        .unwrap();
    let backend = FakeMutationBackend {
        manager: PackageManager::Apt,
        target_supported: true,
        observed: resolution.before.clone(),
        calls: Arc::new(Mutex::new(Vec::new())),
    };
    let mut adapter = PackageAdapter::new(authority, artifacts, Box::new(backend));

    assert_eq!(
        adapter.observe_recovery(&operation(resolution_ref.digest)),
        Ok(RecoveryObservation::Other)
    );
}

#[test]
fn apt_recovery_does_not_treat_duplicate_version_identities_as_before() {
    let (mut resolution, authority) = apt_resolution();
    resolution.before = PackageObservationV1 {
        installed_versions: BTreeSet::from([
            "ripgrep:amd64=14.1.1-1ubuntu1".into(),
            "ripgrep:amd64=14.1.1-1ubuntu2".into(),
        ]),
    };
    let root = tempfile::tempdir().unwrap();
    let artifacts = ArtifactStore::open(root.path()).unwrap();
    let resolution_ref = artifacts
        .put(
            &serde_json::to_vec(&resolution).unwrap(),
            ContentSensitivity::Portable,
        )
        .unwrap();
    let backend = FakeMutationBackend {
        manager: PackageManager::Apt,
        target_supported: true,
        observed: resolution.before.clone(),
        calls: Arc::new(Mutex::new(Vec::new())),
    };
    let mut adapter = PackageAdapter::new(authority, artifacts, Box::new(backend));

    assert_eq!(
        adapter.observe_recovery(&operation(resolution_ref.digest)),
        Ok(RecoveryObservation::Other)
    );
}

#[test]
fn apt_recovery_does_not_treat_foreign_before_identity_as_before() {
    let (mut resolution, authority) = apt_resolution();
    resolution.before = PackageObservationV1 {
        installed_versions: BTreeSet::from(["ripgrep:arm64=14.1.1-1ubuntu1".into()]),
    };
    let root = tempfile::tempdir().unwrap();
    let artifacts = ArtifactStore::open(root.path()).unwrap();
    let resolution_ref = artifacts
        .put(
            &serde_json::to_vec(&resolution).unwrap(),
            ContentSensitivity::Portable,
        )
        .unwrap();
    let backend = FakeMutationBackend {
        manager: PackageManager::Apt,
        target_supported: true,
        observed: resolution.before.clone(),
        calls: Arc::new(Mutex::new(Vec::new())),
    };
    let mut adapter = PackageAdapter::new(authority, artifacts, Box::new(backend));

    assert_eq!(
        adapter.observe_recovery(&operation(resolution_ref.digest)),
        Ok(RecoveryObservation::Other)
    );
}

#[test]
fn apt_recovery_does_not_treat_all_native_ambiguity_as_before() {
    let (mut resolution, authority) = apt_resolution();
    resolution.before = PackageObservationV1 {
        installed_versions: BTreeSet::from([
            "ripgrep:all=14.1.1-1ubuntu1".into(),
            "ripgrep:amd64=14.1.1-1ubuntu1".into(),
        ]),
    };
    let root = tempfile::tempdir().unwrap();
    let artifacts = ArtifactStore::open(root.path()).unwrap();
    let resolution_ref = artifacts
        .put(
            &serde_json::to_vec(&resolution).unwrap(),
            ContentSensitivity::Portable,
        )
        .unwrap();
    let backend = FakeMutationBackend {
        manager: PackageManager::Apt,
        target_supported: true,
        observed: resolution.before.clone(),
        calls: Arc::new(Mutex::new(Vec::new())),
    };
    let mut adapter = PackageAdapter::new(authority, artifacts, Box::new(backend));

    assert_eq!(
        adapter.observe_recovery(&operation(resolution_ref.digest)),
        Ok(RecoveryObservation::Other)
    );
}

#[test]
fn apt_recovery_does_not_treat_foreign_identity_as_after() {
    let (resolution, authority) = apt_resolution();
    let root = tempfile::tempdir().unwrap();
    let artifacts = ArtifactStore::open(root.path()).unwrap();
    let resolution_ref = artifacts
        .put(
            &serde_json::to_vec(&resolution).unwrap(),
            ContentSensitivity::Portable,
        )
        .unwrap();
    let backend = FakeMutationBackend {
        manager: PackageManager::Apt,
        target_supported: true,
        observed: PackageObservationV1 {
            installed_versions: BTreeSet::from([
                "ripgrep:amd64=14.1.1-1ubuntu1".into(),
                "ripgrep:arm64=14.1.1-1ubuntu1".into(),
            ]),
        },
        calls: Arc::new(Mutex::new(Vec::new())),
    };
    let mut adapter = PackageAdapter::new(authority, artifacts, Box::new(backend));

    assert_eq!(
        adapter.observe_recovery(&operation(resolution_ref.digest)),
        Ok(RecoveryObservation::Other)
    );
}

#[test]
fn process_verify_rejects_missing_desired_state_instead_of_preparing_artifacts() {
    let target_root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(target_root.path().join(".nvm")).unwrap();
    std::fs::write(
        target_root.path().join(".nvm/nvm.sh"),
        "nvm() { return 0; }\n",
    )
    .unwrap();
    let artifacts = ArtifactStore::open(tempfile::tempdir().unwrap().path()).unwrap();
    let resolution = PackageResolutionV1 {
        schema_version: SchemaVersion(1),
        declaration: PackageDeclaration {
            id: StableId::parse("node").unwrap(),
            version: "v22.1.0".into(),
            manager: PackageManager::Nvm,
            source: StableId::parse("nodejs").unwrap(),
            selector: Some(PackageSelector::NodeRuntime {}),
        },
        target: PackageTargetV1 {
            os: "macos".into(),
            os_version: "14".into(),
            distro_id: None,
            distro_version: None,
            codename: None,
            arch: "aarch64".into(),
            libc: None,
            manager_prefix: None,
        },
        manager: ManagerBindingV1 {
            manager: PackageManager::Nvm,
            version: "0.39.7".into(),
            executable_digest: digest('a'),
            config_digest: digest('b'),
        },
        source: SourceBindingV1 {
            source_id: StableId::parse("nodejs").unwrap(),
            registry_definition_digest: digest('c'),
            canonical_repository: "https://nodejs.org/dist".into(),
            repository_revision: None,
            signed_metadata: Vec::new(),
        },
        before: PackageObservationV1 {
            installed_versions: BTreeSet::new(),
        },
        closure: vec![ResolvedPackage {
            declaration: PackageDeclaration {
                id: StableId::parse("node").unwrap(),
                version: "v22.1.0".into(),
                manager: PackageManager::Nvm,
                source: StableId::parse("nodejs").unwrap(),
                selector: Some(PackageSelector::NodeRuntime {}),
            },
            source: SourceBindingV1 {
                source_id: StableId::parse("nodejs").unwrap(),
                registry_definition_digest: digest('c'),
                canonical_repository: "https://nodejs.org/dist".into(),
                repository_revision: None,
                signed_metadata: Vec::new(),
            },
        }],
        artifacts: Vec::new(),
        recipe: OfflineInstallRecipeV1::NodeArchive {
            artifact_roles: BTreeSet::new(),
            install: None,
        },
    };

    let mut backend = ProcessOfflinePackageBackend::new(target_root.path());
    assert_eq!(
        backend.verify_offline(&resolution, &artifacts),
        Err(PackageMutationError::Backend)
    );
}

#[test]
fn apt_recovery_matches_real_names_for_logical_root_and_hashed_dependencies() {
    let (mut resolution, authority) = apt_resolution();
    let source = resolution.source.clone();
    let root = PackageDeclaration {
        id: StableId::parse("logical-root").unwrap(),
        version: "8.5.0-2ubuntu10.6".into(),
        manager: PackageManager::Apt,
        source: source.source_id.clone(),
        selector: Some(PackageSelector::AptBinary {
            name: "curl".into(),
            architecture: Some("amd64".into()),
        }),
    };
    resolution.declaration = root.clone();
    resolution.closure = vec![
        ResolvedPackage {
            declaration: PackageDeclaration {
                id: StableId::parse("apt-dep-0123456789abcdef012345678").unwrap(),
                version: "2.39-0ubuntu8.6".into(),
                manager: PackageManager::Apt,
                source: source.source_id.clone(),
                selector: Some(PackageSelector::AptBinary {
                    name: "libc6".into(),
                    architecture: Some("amd64".into()),
                }),
            },
            source: source.clone(),
        },
        ResolvedPackage {
            declaration: root,
            source,
        },
    ];

    let root = tempfile::tempdir().unwrap();
    let artifacts = ArtifactStore::open(root.path()).unwrap();
    let resolution_ref = artifacts
        .put(
            &serde_json::to_vec(&resolution).unwrap(),
            ContentSensitivity::Portable,
        )
        .unwrap();
    let backend = FakeMutationBackend {
        manager: PackageManager::Apt,
        target_supported: true,
        observed: PackageObservationV1 {
            installed_versions: BTreeSet::from([
                "curl:amd64=8.5.0-2ubuntu10.6".into(),
                "libc6:amd64=2.39-0ubuntu8.6".into(),
            ]),
        },
        calls: Arc::new(Mutex::new(Vec::new())),
    };
    let mut adapter = PackageAdapter::new(authority, artifacts, Box::new(backend));

    assert_eq!(
        adapter.observe_recovery(&operation(resolution_ref.digest)),
        Ok(RecoveryObservation::After)
    );
}

#[test]
fn apt_recovery_resolves_all_and_native_architectures_in_the_closure() {
    let (mut resolution, authority) = apt_resolution();
    let source = resolution.source.clone();
    let root = PackageDeclaration {
        id: StableId::parse("logical-all-root").unwrap(),
        version: "2023.4ubuntu1".into(),
        manager: PackageManager::Apt,
        source: source.source_id.clone(),
        selector: Some(PackageSelector::AptBinary {
            name: "debian-archive-keyring".into(),
            architecture: Some("all".into()),
        }),
    };
    resolution.declaration = root.clone();
    resolution.closure = vec![
        ResolvedPackage {
            declaration: PackageDeclaration {
                id: StableId::parse("apt-dep-fedcba9876543210fedcba98").unwrap(),
                version: "2.39-0ubuntu8.6".into(),
                manager: PackageManager::Apt,
                source: source.source_id.clone(),
                selector: Some(PackageSelector::AptBinary {
                    name: "libc6".into(),
                    architecture: None,
                }),
            },
            source: source.clone(),
        },
        ResolvedPackage {
            declaration: root,
            source,
        },
    ];

    let root = tempfile::tempdir().unwrap();
    let artifacts = ArtifactStore::open(root.path()).unwrap();
    let resolution_ref = artifacts
        .put(
            &serde_json::to_vec(&resolution).unwrap(),
            ContentSensitivity::Portable,
        )
        .unwrap();
    let backend = FakeMutationBackend {
        manager: PackageManager::Apt,
        target_supported: true,
        observed: PackageObservationV1 {
            installed_versions: BTreeSet::from([
                "debian-archive-keyring:all=2023.4ubuntu1".into(),
                "libc6:amd64=2.39-0ubuntu8.6".into(),
            ]),
        },
        calls: Arc::new(Mutex::new(Vec::new())),
    };
    let mut adapter = PackageAdapter::new(authority, artifacts, Box::new(backend));

    assert_eq!(
        adapter.observe_recovery(&operation(resolution_ref.digest)),
        Ok(RecoveryObservation::After)
    );
}

#[test]
fn unsupported_target_platform_fails_before_backend_mutation() {
    let (resolution, authority) = apt_resolution();
    let root = tempfile::tempdir().unwrap();
    let artifacts = ArtifactStore::open(root.path()).unwrap();
    let resolution_ref = artifacts
        .put(
            &serde_json::to_vec(&resolution).unwrap(),
            ContentSensitivity::Portable,
        )
        .unwrap();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let backend = FakeMutationBackend {
        manager: PackageManager::Apt,
        target_supported: false,
        observed: resolution.before.clone(),
        calls: calls.clone(),
    };
    let mut adapter = PackageAdapter::new(authority, artifacts, Box::new(backend));
    let error = adapter
        .preflight(&operation(resolution_ref.digest))
        .unwrap_err();
    assert_eq!(error.code, "package_target_unsupported");
    assert!(calls.lock().unwrap().is_empty());
}
