use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use commonkit_adapters::{
    ArtifactStore, ContentSensitivity, ManagerBindingV1, OfflineInstallRecipeV1, PackageAdapter,
    PackageMutationBackend, PackageMutationError, PackageObservationV1, PackageResolutionAuthority,
    PackageResolutionV1, PackageSourceRegistry, PackageTargetV1, ResolvedPackage, SourceBindingV1,
};
use commonkit_contracts::{
    Operation, OperationKind, PackageDeclaration, PackageManager, PackageSelector,
    RecoveryCapability, ResourceRef, Risk, SchemaVersion, SecurityPolicy, Sha256Digest, StableId,
};
use commonkit_core::OperationDraft;
use commonkit_reconcile::Adapter;

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
    observed: PackageObservationV1,
    calls: Arc<Mutex<Vec<&'static str>>>,
}

impl PackageMutationBackend for FakeMutationBackend {
    fn manager(&self) -> PackageManager {
        self.manager
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
