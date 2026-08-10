use std::collections::{BTreeMap, BTreeSet};

use commonkit_adapters::{
    ArtifactEvidence, ArtifactStore, ControlledPackageSourceV1, ExactProviderVersion,
    ManagerBindingV1, MaterializedState, NormalizedResource, OfflineInstallRecipeV1,
    PackageDesiredIntent, PackageFetch, PackageFetchRequestV1, PackageObservationV1,
    PackageResolutionBackend, PackageResolutionCoordinator, PackageResolutionDraftV1,
    PackageResolutionError, PackageResolutionProbeV1, PackageResolutionRequestV1,
    PackageSourceRegistry, PackageTargetV1, ProviderInputs, ResolvedPackage, ResourceProvenance,
    SourceBindingV1,
};
use commonkit_contracts::{
    PackageDeclaration, PackageManager, PackageSelector, SecurityPolicy, Sha256Digest, StableId,
    digest_domain_json,
};
use sha2::{Digest, Sha256};

fn id(value: &str) -> StableId {
    StableId::parse(value).unwrap()
}

fn digest(seed: char) -> Sha256Digest {
    Sha256Digest::parse(format!("sha256:{}", seed.to_string().repeat(64))).unwrap()
}

struct PanicBackend;

impl PackageResolutionBackend for PanicBackend {
    fn manager(&self) -> PackageManager {
        PackageManager::Homebrew
    }

    fn probe(
        &mut self,
        _request: &PackageResolutionRequestV1<'_>,
    ) -> Result<PackageResolutionProbeV1, PackageResolutionError> {
        panic!("policy rejection must happen before backend probe")
    }

    fn resolve_and_fetch(
        &mut self,
        _request: &PackageResolutionRequestV1<'_>,
        _source: &SourceBindingV1,
        _fetch: &mut dyn PackageFetch,
    ) -> Result<PackageResolutionDraftV1, PackageResolutionError> {
        panic!("policy rejection must happen before backend resolution")
    }
}

struct PanicFetch;

impl PackageFetch for PanicFetch {
    fn fetch(
        &mut self,
        _request: &PackageFetchRequestV1,
    ) -> Result<Vec<u8>, PackageResolutionError> {
        panic!("policy rejection must happen before fetch")
    }
}

struct UnavailableBackend;

impl PackageResolutionBackend for UnavailableBackend {
    fn manager(&self) -> PackageManager {
        PackageManager::Homebrew
    }

    fn probe(
        &mut self,
        _request: &PackageResolutionRequestV1<'_>,
    ) -> Result<PackageResolutionProbeV1, PackageResolutionError> {
        Err(PackageResolutionError::ResolverUnavailable {
            manager: PackageManager::Homebrew,
        })
    }

    fn resolve_and_fetch(
        &mut self,
        _request: &PackageResolutionRequestV1<'_>,
        _source: &SourceBindingV1,
        _fetch: &mut dyn PackageFetch,
    ) -> Result<PackageResolutionDraftV1, PackageResolutionError> {
        panic!("unavailable resolver cannot reach fetch")
    }
}

#[derive(Clone)]
struct FixtureBackend {
    draft: PackageResolutionDraftV1,
    fetch_requests: Vec<PackageFetchRequestV1>,
    observation: PackageObservationV1,
    repository_revision: Option<String>,
    signed_metadata: Vec<ArtifactEvidence>,
}

impl PackageResolutionBackend for FixtureBackend {
    fn manager(&self) -> PackageManager {
        PackageManager::Homebrew
    }

    fn probe(
        &mut self,
        _request: &PackageResolutionRequestV1<'_>,
    ) -> Result<PackageResolutionProbeV1, PackageResolutionError> {
        Ok(PackageResolutionProbeV1 {
            before: self.observation.clone(),
            repository_revision: self.repository_revision.clone(),
            signed_metadata: self.signed_metadata.clone(),
        })
    }

    fn resolve_and_fetch(
        &mut self,
        _request: &PackageResolutionRequestV1<'_>,
        _source: &SourceBindingV1,
        fetch: &mut dyn PackageFetch,
    ) -> Result<PackageResolutionDraftV1, PackageResolutionError> {
        for artifact in &self.fetch_requests {
            fetch.fetch(artifact)?;
        }
        Ok(self.draft.clone())
    }
}

struct FixtureFetch {
    bytes: BTreeMap<String, Vec<u8>>,
    calls: usize,
}

impl PackageFetch for FixtureFetch {
    fn fetch(
        &mut self,
        request: &PackageFetchRequestV1,
    ) -> Result<Vec<u8>, PackageResolutionError> {
        self.calls += 1;
        self.bytes
            .get(&request.immutable_locator)
            .cloned()
            .ok_or(PackageResolutionError::FetchUnavailable)
    }
}

fn portable_digest(bytes: &[u8]) -> Sha256Digest {
    Sha256Digest::parse(format!("sha256:{:x}", Sha256::digest(bytes))).unwrap()
}

fn allowed_policy() -> SecurityPolicy {
    SecurityPolicy {
        allowlists: BTreeMap::from([(
            id("package_sources"),
            BTreeSet::from(["homebrew-core".into()]),
        )]),
        ..SecurityPolicy::default()
    }
}

fn manager_binding() -> ManagerBindingV1 {
    ManagerBindingV1 {
        manager: PackageManager::Homebrew,
        version: "4.5.0".into(),
        executable_digest: digest('a'),
        config_digest: digest('b'),
    }
}

fn target() -> PackageTargetV1 {
    PackageTargetV1 {
        os: "macos".into(),
        os_version: "15.6".into(),
        distro_id: None,
        distro_version: None,
        codename: None,
        arch: "aarch64".into(),
        libc: None,
        manager_prefix: Some("/opt/homebrew".into()),
    }
}

fn desired() -> PackageDesiredIntent {
    PackageDesiredIntent::new(PackageDeclaration {
        id: id("ripgrep"),
        version: "14.1.1".into(),
        manager: PackageManager::Homebrew,
        source: id("homebrew-core"),
        selector: Some(PackageSelector::HomebrewFormula {
            name: "ripgrep".into(),
        }),
    })
    .unwrap()
}

fn fixture_backend(bytes: &[u8]) -> FixtureBackend {
    let repository_revision = Some("0123456789abcdef0123456789abcdef01234567".into());
    let signed_metadata = vec![ArtifactEvidence {
        authority: id("homebrew"),
        metadata_digest: digest('c'),
        signature_digest: digest('d'),
    }];
    let source_metadata_digest = digest_domain_json(
        "commonkit.package-source-metadata.v1",
        &(&repository_revision, &signed_metadata),
    )
    .unwrap();
    let role = id("bottle");
    let artifact = PackageFetchRequestV1 {
        role: role.clone(),
        immutable_locator:
            "https://github.com/Homebrew/homebrew-core/releases/download/ripgrep-14.1.1/bottle.tar.gz"
                .into(),
        upstream_checksum: portable_digest(bytes),
        size: bytes.len() as u64,
        materialization_key: id("ripgrep-bottle"),
        source_metadata_digest,
    };
    FixtureBackend {
        observation: PackageObservationV1 {
            installed_versions: BTreeSet::new(),
        },
        repository_revision,
        signed_metadata,
        fetch_requests: vec![artifact.clone()],
        draft: PackageResolutionDraftV1 {
            closure: vec![ResolvedPackage {
                declaration: match desired() {
                    PackageDesiredIntent::Package { declaration } => declaration,
                },
            }],
            artifacts: vec![artifact],
            recipe: OfflineInstallRecipeV1::HomebrewBottle {
                artifact_roles: BTreeSet::from([role]),
            },
        },
    }
}

#[test]
fn disallowed_package_source_fails_before_backend_or_fetch() {
    let root = tempfile::tempdir().unwrap();
    let store = ArtifactStore::open(root.path().join("artifacts")).unwrap();
    let policy = SecurityPolicy {
        allowlists: BTreeMap::from([(
            id("package_sources"),
            BTreeSet::from(["company-controlled".into()]),
        )]),
        ..SecurityPolicy::default()
    };
    let registry = PackageSourceRegistry::builtin().unwrap();
    let manager = ManagerBindingV1 {
        manager: PackageManager::Homebrew,
        version: "4.5.0".into(),
        executable_digest: digest('a'),
        config_digest: digest('b'),
    };
    let target = PackageTargetV1 {
        os: "macos".into(),
        os_version: "15.6".into(),
        distro_id: None,
        distro_version: None,
        codename: None,
        arch: "aarch64".into(),
        libc: None,
        manager_prefix: Some("/opt/homebrew".into()),
    };
    let desired = PackageDesiredIntent::new(PackageDeclaration {
        id: id("ripgrep"),
        version: "14.1.1".into(),
        manager: PackageManager::Homebrew,
        source: id("homebrew-core"),
        selector: Some(PackageSelector::HomebrewFormula {
            name: "ripgrep".into(),
        }),
    })
    .unwrap();
    let mut backend = PanicBackend;
    let mut fetch = PanicFetch;
    let mut coordinator =
        PackageResolutionCoordinator::new(&policy, &registry, manager, &mut backend, &mut fetch);

    let error = coordinator
        .resolve(&desired, &target, &store)
        .expect_err("source policy must fail closed");

    assert!(matches!(
        error,
        PackageResolutionError::PackageSourceNotAllowed { .. }
    ));
}

#[test]
fn target_manager_and_source_mismatch_fail_before_backend_or_fetch() {
    let root = tempfile::tempdir().unwrap();
    let store = ArtifactStore::open(root.path().join("artifacts")).unwrap();
    let registry = PackageSourceRegistry::builtin().unwrap();

    let mut invalid_target = target();
    invalid_target.arch.clear();
    let mut backend = PanicBackend;
    let mut fetch = PanicFetch;
    assert!(matches!(
        PackageResolutionCoordinator::new(
            &allowed_policy(),
            &registry,
            manager_binding(),
            &mut backend,
            &mut fetch,
        )
        .resolve(&desired(), &invalid_target, &store),
        Err(PackageResolutionError::InvalidTarget)
    ));

    let mut wrong_manager = manager_binding();
    wrong_manager.manager = PackageManager::Apt;
    let mut backend = PanicBackend;
    let mut fetch = PanicFetch;
    assert!(matches!(
        PackageResolutionCoordinator::new(
            &allowed_policy(),
            &registry,
            wrong_manager,
            &mut backend,
            &mut fetch,
        )
        .resolve(&desired(), &target(), &store),
        Err(PackageResolutionError::ManagerMismatch)
    ));

    let wrong_source =
        PackageSourceRegistry::from_controlled_sources(vec![ControlledPackageSourceV1 {
            source_id: id("homebrew-core"),
            manager: PackageManager::Apt,
            canonical_repository: "https://archive.example.invalid/apt".into(),
        }])
        .unwrap();
    let mut backend = PanicBackend;
    let mut fetch = PanicFetch;
    assert!(matches!(
        PackageResolutionCoordinator::new(
            &allowed_policy(),
            &wrong_source,
            manager_binding(),
            &mut backend,
            &mut fetch,
        )
        .resolve(&desired(), &target(), &store),
        Err(PackageResolutionError::SourceManagerMismatch)
    ));
}

#[test]
fn identical_resolution_inputs_persist_identical_resolution_and_artifact_references() {
    let root = tempfile::tempdir().unwrap();
    let store = ArtifactStore::open(root.path().join("artifacts")).unwrap();
    let registry = PackageSourceRegistry::builtin().unwrap();
    let bytes = b"immutable package archive";

    let resolve_once = |backend: &mut FixtureBackend, fetch: &mut FixtureFetch| {
        PackageResolutionCoordinator::new(
            &allowed_policy(),
            &registry,
            manager_binding(),
            backend,
            fetch,
        )
        .resolve(&desired(), &target(), &store)
        .unwrap()
    };
    let mut first_backend = fixture_backend(bytes);
    let mut first_fetch = FixtureFetch {
        bytes: BTreeMap::from([(
            first_backend.draft.artifacts[0].immutable_locator.clone(),
            bytes.to_vec(),
        )]),
        calls: 0,
    };
    let first = resolve_once(&mut first_backend, &mut first_fetch);
    let mut second_backend = fixture_backend(bytes);
    let mut second_fetch = FixtureFetch {
        bytes: first_fetch.bytes.clone(),
        calls: 0,
    };
    let second = resolve_once(&mut second_backend, &mut second_fetch);

    assert_eq!(first, second);
    assert_eq!(first_fetch.calls, 1);
    assert_eq!(second_fetch.calls, 1);
    store.load(&first.resolution).unwrap();
    for artifact in &first.artifacts {
        assert_eq!(store.load(artifact).unwrap(), bytes);
    }
}

#[test]
fn missing_extra_duplicate_and_corrupt_artifacts_fail_closed() {
    #[derive(Clone, Copy)]
    enum Case {
        Missing,
        Extra,
        Duplicate,
        Conflicting,
        Corrupt,
    }

    for case in [
        Case::Missing,
        Case::Extra,
        Case::Duplicate,
        Case::Conflicting,
        Case::Corrupt,
    ] {
        let root = tempfile::tempdir().unwrap();
        let store = ArtifactStore::open(root.path().join("artifacts")).unwrap();
        let registry = PackageSourceRegistry::builtin().unwrap();
        let bytes = b"immutable package archive";
        let mut backend = fixture_backend(bytes);
        let locator = backend.fetch_requests[0].immutable_locator.clone();
        let expected = match case {
            Case::Missing => {
                backend.fetch_requests.clear();
                "artifact set"
            }
            Case::Extra => {
                backend.draft.artifacts.clear();
                "artifact set"
            }
            Case::Duplicate => {
                backend
                    .fetch_requests
                    .push(backend.fetch_requests[0].clone());
                backend
                    .draft
                    .artifacts
                    .push(backend.draft.artifacts[0].clone());
                "duplicate"
            }
            Case::Conflicting => {
                let mut conflict = backend.fetch_requests[0].clone();
                conflict.role = id("signature");
                backend.fetch_requests.push(conflict.clone());
                backend.draft.artifacts.push(conflict);
                backend.draft.recipe = OfflineInstallRecipeV1::HomebrewBottle {
                    artifact_roles: BTreeSet::from([id("bottle"), id("signature")]),
                };
                "duplicate"
            }
            Case::Corrupt => "failed digest",
        };
        let returned = if matches!(case, Case::Corrupt) {
            b"corrupt".to_vec()
        } else {
            bytes.to_vec()
        };
        let mut fetch = FixtureFetch {
            bytes: BTreeMap::from([(locator, returned)]),
            calls: 0,
        };
        let error = PackageResolutionCoordinator::new(
            &allowed_policy(),
            &registry,
            manager_binding(),
            &mut backend,
            &mut fetch,
        )
        .resolve(&desired(), &target(), &store)
        .expect_err("invalid artifact set");

        assert!(error.to_string().contains(expected), "{error}");
    }
}

#[test]
fn persisted_resolution_rejects_target_manager_source_and_artifact_list_drift() {
    let root = tempfile::tempdir().unwrap();
    let store = ArtifactStore::open(root.path().join("artifacts")).unwrap();
    let registry = PackageSourceRegistry::builtin().unwrap();
    let bytes = b"immutable package archive";
    let mut backend = fixture_backend(bytes);
    let mut fetch = FixtureFetch {
        bytes: BTreeMap::from([(
            backend.fetch_requests[0].immutable_locator.clone(),
            bytes.to_vec(),
        )]),
        calls: 0,
    };
    let resolved = PackageResolutionCoordinator::new(
        &allowed_policy(),
        &registry,
        manager_binding(),
        &mut backend,
        &mut fetch,
    )
    .resolve(&desired(), &target(), &store)
    .unwrap();

    resolved
        .load_and_validate(&target(), &manager_binding(), &registry, &store)
        .unwrap();

    let mut changed_target = target();
    changed_target.arch = "x86_64".into();
    assert!(matches!(
        resolved.load_and_validate(&changed_target, &manager_binding(), &registry, &store),
        Err(PackageResolutionError::TargetBindingMismatch)
    ));

    let mut changed_manager = manager_binding();
    changed_manager.config_digest = digest('f');
    assert!(matches!(
        resolved.load_and_validate(&target(), &changed_manager, &registry, &store),
        Err(PackageResolutionError::ManagerBindingMismatch)
    ));

    let changed_registry =
        PackageSourceRegistry::from_controlled_sources(vec![ControlledPackageSourceV1 {
            source_id: id("homebrew-core"),
            manager: PackageManager::Homebrew,
            canonical_repository: "https://example.invalid/homebrew-core".into(),
        }])
        .unwrap();
    assert!(matches!(
        resolved.load_and_validate(&target(), &manager_binding(), &changed_registry, &store,),
        Err(PackageResolutionError::SourceBindingMismatch)
    ));

    let mut missing_artifact = resolved.clone();
    missing_artifact.artifacts.clear();
    assert!(matches!(
        missing_artifact.load_and_validate(&target(), &manager_binding(), &registry, &store),
        Err(PackageResolutionError::ArtifactSetMismatch)
    ));
}

#[test]
fn resolver_unavailable_fails_before_a_resolved_resource_can_reach_planning() {
    let root = tempfile::tempdir().unwrap();
    let store = ArtifactStore::open(root.path().join("artifacts")).unwrap();
    let inputs = ProviderInputs::new(
        id("native"),
        ExactProviderVersion::parse("1.0.0").unwrap(),
        "provider.v3".into(),
        BTreeMap::from([("source".into(), digest('8'))]),
        vec!["package".into()],
    )
    .unwrap();
    let state = MaterializedState::finalize(
        inputs.clone(),
        vec![NormalizedResource {
            intent: desired().into(),
            provenance: ResourceProvenance {
                provider_id: inputs.provider_id.clone(),
                provider_version: inputs.provider_version.to_string(),
                input_digest: inputs.input_set_digest.clone(),
                source: "package:ripgrep".into(),
            },
        }],
        vec![],
        vec![],
    )
    .unwrap();
    let registry = PackageSourceRegistry::builtin().unwrap();
    let mut backend = UnavailableBackend;
    let mut fetch = PanicFetch;
    let error = PackageResolutionCoordinator::new(
        &allowed_policy(),
        &registry,
        manager_binding(),
        &mut backend,
        &mut fetch,
    )
    .resolve_state(&state, &target(), &store)
    .expect_err("unavailable resolver");

    assert!(matches!(
        error,
        PackageResolutionError::ResolverUnavailable {
            manager: PackageManager::Homebrew
        }
    ));
}

#[test]
fn fresh_process_reconstructs_resolved_intent_from_artifacts_without_fetch() {
    let root = tempfile::tempdir().unwrap();
    let artifact_root = root.path().join("artifacts");
    let store = ArtifactStore::open(&artifact_root).unwrap();
    let registry = PackageSourceRegistry::builtin().unwrap();
    let bytes = b"immutable package archive";
    let mut backend = fixture_backend(bytes);
    let mut fetch = FixtureFetch {
        bytes: BTreeMap::from([(
            backend.fetch_requests[0].immutable_locator.clone(),
            bytes.to_vec(),
        )]),
        calls: 0,
    };
    let resolved = PackageResolutionCoordinator::new(
        &allowed_policy(),
        &registry,
        manager_binding(),
        &mut backend,
        &mut fetch,
    )
    .resolve(&desired(), &target(), &store)
    .unwrap();
    let durable = serde_json::to_vec(&resolved).unwrap();
    drop(store);
    drop(fetch);
    drop(backend);

    let _network_capability_that_must_not_be_used = PanicFetch;
    let reopened = ArtifactStore::open_existing(&artifact_root).unwrap();
    let reconstructed: commonkit_adapters::ResolvedPackageIntent =
        serde_json::from_slice(&durable).unwrap();
    let resolution = reconstructed
        .load_and_validate(&target(), &manager_binding(), &registry, &reopened)
        .unwrap();

    assert_eq!(
        resolution.declaration,
        match desired() {
            PackageDesiredIntent::Package { declaration } => declaration,
        }
    );
}

#[test]
fn checked_in_package_resolution_schema_matches_the_closed_rust_contract() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("schemas/package-resolution.schema.json");
    let checked_in: serde_json::Value =
        serde_json::from_slice(&std::fs::read(path).expect("package resolution schema")).unwrap();
    assert_eq!(
        checked_in,
        commonkit_adapters::package_resolution_schema().unwrap()
    );
}
