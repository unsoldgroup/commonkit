use std::collections::{BTreeMap, BTreeSet};

use commonkit_adapters::{
    ArtifactEvidence, ArtifactStore, ControlledPackageSourceV1, ExactProviderVersion,
    ManagerBindingV1, MaterializedState, NormalizedResource, OfflineInstallRecipeV1,
    PackageDesiredIntent, PackageFetch, PackageFetchHopV1, PackageFetchRequestV1,
    PackageFetchResultV1, PackageObservationV1, PackageResolutionAuthority,
    PackageResolutionBackend, PackageResolutionCoordinator, PackageResolutionDraftV1,
    PackageResolutionError, PackageResolutionProbeV1, PackageResolutionRequestV1,
    PackageResolutionV1, PackageSourceRegistry, PackageTargetV1, ProviderInputs, ResolvedPackage,
    ResourceProvenance, SourceBindingV1,
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

    fn resolve(
        &mut self,
        _request: &PackageResolutionRequestV1<'_>,
        _source: &SourceBindingV1,
    ) -> Result<PackageResolutionDraftV1, PackageResolutionError> {
        panic!("policy rejection must happen before backend resolution")
    }
}

struct PanicFetch;

impl PackageFetch for PanicFetch {
    fn fetch_hop(
        &mut self,
        _request: &PackageFetchRequestV1,
        _locator: &str,
    ) -> Result<PackageFetchHopV1, PackageResolutionError> {
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

    fn resolve(
        &mut self,
        _request: &PackageResolutionRequestV1<'_>,
        _source: &SourceBindingV1,
    ) -> Result<PackageResolutionDraftV1, PackageResolutionError> {
        panic!("unavailable resolver cannot reach fetch")
    }
}

struct CountingBackend {
    probes: usize,
}

impl PackageResolutionBackend for CountingBackend {
    fn manager(&self) -> PackageManager {
        PackageManager::Homebrew
    }

    fn probe(
        &mut self,
        _request: &PackageResolutionRequestV1<'_>,
    ) -> Result<PackageResolutionProbeV1, PackageResolutionError> {
        self.probes += 1;
        Err(PackageResolutionError::ResolverUnavailable {
            manager: PackageManager::Homebrew,
        })
    }

    fn resolve(
        &mut self,
        _request: &PackageResolutionRequestV1<'_>,
        _source: &SourceBindingV1,
    ) -> Result<PackageResolutionDraftV1, PackageResolutionError> {
        panic!("whole-state preflight must reject before resolution")
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

struct LaterInvalidClosureBackend {
    fixture: FixtureBackend,
    resolves: usize,
}

impl PackageResolutionBackend for LaterInvalidClosureBackend {
    fn manager(&self) -> PackageManager {
        PackageManager::Homebrew
    }

    fn probe(
        &mut self,
        request: &PackageResolutionRequestV1<'_>,
    ) -> Result<PackageResolutionProbeV1, PackageResolutionError> {
        self.fixture.probe(request)
    }

    fn resolve(
        &mut self,
        request: &PackageResolutionRequestV1<'_>,
        source: &SourceBindingV1,
    ) -> Result<PackageResolutionDraftV1, PackageResolutionError> {
        self.resolves += 1;
        let PackageDesiredIntent::Package { declaration } = request.desired;
        let mut draft = self.fixture.draft.clone();
        draft.closure = vec![ResolvedPackage {
            declaration: declaration.clone(),
            source: source.clone(),
        }];
        if self.resolves == 2 {
            draft.closure.push(ResolvedPackage {
                declaration: PackageDeclaration {
                    id: id("transitive-denied"),
                    version: "1.2.3".into(),
                    manager: PackageManager::Homebrew,
                    source: id("unapproved-source"),
                    selector: Some(PackageSelector::HomebrewFormula {
                        name: "transitive-denied".into(),
                    }),
                },
                source: source.clone(),
            });
        }
        Ok(draft)
    }
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

    fn resolve(
        &mut self,
        _request: &PackageResolutionRequestV1<'_>,
        _source: &SourceBindingV1,
    ) -> Result<PackageResolutionDraftV1, PackageResolutionError> {
        Ok(self.draft.clone())
    }

    fn fetch_artifacts(
        &mut self,
        _request: &PackageResolutionRequestV1<'_>,
        _source: &SourceBindingV1,
        _artifacts: &[PackageFetchRequestV1],
        fetch: &mut dyn PackageFetch,
    ) -> Result<(), PackageResolutionError> {
        for artifact in &self.fetch_requests {
            match fetch.fetch_hop(artifact, &artifact.immutable_locator)? {
                PackageFetchHopV1::Complete(_) => {}
                PackageFetchHopV1::Redirect { .. } => {
                    return Err(PackageResolutionError::UnvalidatedRedirect);
                }
            }
        }
        Ok(())
    }
}

struct FixtureFetch {
    bytes: BTreeMap<String, Vec<u8>>,
    calls: usize,
}

struct RedirectFetch {
    final_locator: String,
    calls: usize,
}

struct ChainedRedirectFetch {
    approved_final: String,
    denied_intermediate: String,
    contacted: Vec<String>,
}

impl PackageFetch for ChainedRedirectFetch {
    fn fetch_hop(
        &mut self,
        _request: &PackageFetchRequestV1,
        locator: &str,
    ) -> Result<PackageFetchHopV1, PackageResolutionError> {
        self.contacted.push(locator.into());
        if self.contacted.len() == 1 {
            Ok(PackageFetchHopV1::Redirect {
                location: self.denied_intermediate.clone(),
            })
        } else {
            Ok(PackageFetchHopV1::Redirect {
                location: self.approved_final.clone(),
            })
        }
    }
}

impl PackageFetch for RedirectFetch {
    fn fetch_hop(
        &mut self,
        _request: &PackageFetchRequestV1,
        _locator: &str,
    ) -> Result<PackageFetchHopV1, PackageResolutionError> {
        self.calls += 1;
        Ok(PackageFetchHopV1::Redirect {
            location: self.final_locator.clone(),
        })
    }
}

impl PackageFetch for FixtureFetch {
    fn fetch_hop(
        &mut self,
        _request: &PackageFetchRequestV1,
        locator: &str,
    ) -> Result<PackageFetchHopV1, PackageResolutionError> {
        self.calls += 1;
        let bytes = self
            .bytes
            .get(locator)
            .cloned()
            .ok_or(PackageResolutionError::FetchUnavailable)?;
        Ok(PackageFetchHopV1::Complete(PackageFetchResultV1 { bytes }))
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
    let source = SourceBindingV1 {
        source_id: id("homebrew-core"),
        registry_definition_digest: PackageSourceRegistry::builtin()
            .unwrap()
            .source_definition_digest(&id("homebrew-core"))
            .unwrap(),
        canonical_repository: "https://github.com/Homebrew/homebrew-core".into(),
        repository_revision: repository_revision.clone(),
        signed_metadata: signed_metadata.clone(),
    };
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
                source,
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
            approved_artifact_roots: BTreeSet::new(),
            apt_source_authority: None,
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
fn malformed_persisted_before_observation_is_rejected_before_authority_acceptance() {
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
    let mut malformed: PackageResolutionV1 =
        serde_json::from_slice(&store.load(&resolved.resolution).unwrap()).unwrap();
    // A persisted observation is a canonical package inventory, never a
    // command transcript (or an arbitrary value that could carry secrets).
    malformed
        .before
        .installed_versions
        .insert("backend output leaked here".into());
    let authority = PackageResolutionAuthority::new(
        &target(),
        &manager_binding(),
        &registry,
        &allowed_policy(),
    )
    .unwrap();
    assert!(matches!(
        authority.validate_resolution(&malformed),
        Err(PackageResolutionError::MalformedObservation)
    ));
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
fn denied_resolution_closure_fails_before_artifact_fetch() {
    let root = tempfile::tempdir().unwrap();
    let store = ArtifactStore::open(root.path().join("artifacts")).unwrap();
    let registry = PackageSourceRegistry::builtin().unwrap();
    let bytes = b"immutable package archive";
    let mut backend = fixture_backend(bytes);
    let denied_source = backend.draft.closure[0].source.clone();
    backend.draft.closure.push(ResolvedPackage {
        declaration: PackageDeclaration {
            id: id("transitive-denied"),
            version: "1.2.3".into(),
            manager: PackageManager::Homebrew,
            source: id("unapproved-source"),
            selector: Some(PackageSelector::HomebrewFormula {
                name: "transitive-denied".into(),
            }),
        },
        source: denied_source,
    });
    let mut fetch = FixtureFetch {
        bytes: BTreeMap::from([(
            backend.fetch_requests[0].immutable_locator.clone(),
            bytes.to_vec(),
        )]),
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
    .expect_err("unapproved closure source must fail closed");

    assert!(matches!(
        error,
        PackageResolutionError::PackageSourceNotAllowed { .. }
            | PackageResolutionError::UnknownSource { .. }
    ));
    assert_eq!(fetch.calls, 0, "closure validation must precede fetch");
}

#[test]
fn artifact_fetch_is_scoped_to_the_controlled_source() {
    let root = tempfile::tempdir().unwrap();
    let store = ArtifactStore::open(root.path().join("artifacts")).unwrap();
    let registry = PackageSourceRegistry::builtin().unwrap();
    let bytes = b"immutable package archive";
    let mut backend = fixture_backend(bytes);
    let denied_locator = "https://evil.invalid/ripgrep-14.1.1.tar.gz".to_owned();
    backend.fetch_requests[0].immutable_locator = denied_locator.clone();
    backend.draft.artifacts[0].immutable_locator = denied_locator.clone();
    let mut fetch = FixtureFetch {
        bytes: BTreeMap::from([(denied_locator, bytes.to_vec())]),
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
    .expect_err("unregistered artifact origin must fail closed");

    assert!(matches!(
        error,
        PackageResolutionError::UnapprovedArtifactLocation
    ));
    assert_eq!(fetch.calls, 0, "source scope must be checked before fetch");
}

#[test]
fn artifact_fetch_redirect_must_remain_within_controlled_source_roots() {
    let root = tempfile::tempdir().unwrap();
    let store = ArtifactStore::open(root.path().join("artifacts")).unwrap();
    let registry = PackageSourceRegistry::builtin().unwrap();
    let bytes = b"immutable package archive";
    let mut backend = fixture_backend(bytes);
    let mut fetch = RedirectFetch {
        final_locator: "https://evil.invalid/redirected.tar.gz".into(),
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
    .expect_err("redirect outside controlled roots must fail closed");

    assert!(matches!(
        error,
        PackageResolutionError::UnapprovedArtifactLocation
    ));
    assert_eq!(fetch.calls, 1);
}

#[test]
fn artifact_fetch_rejects_unapproved_intermediate_redirect_before_contact() {
    let root = tempfile::tempdir().unwrap();
    let store = ArtifactStore::open(root.path().join("artifacts")).unwrap();
    let registry = PackageSourceRegistry::builtin().unwrap();
    let bytes = b"immutable package archive";
    let mut backend = fixture_backend(bytes);
    let initial = backend.fetch_requests[0].immutable_locator.clone();
    let mut fetch = ChainedRedirectFetch {
        approved_final: initial.clone(),
        denied_intermediate: "https://evil.invalid/intermediate.tar.gz".into(),
        contacted: vec![],
    };

    let error = PackageResolutionCoordinator::new(
        &allowed_policy(),
        &registry,
        manager_binding(),
        &mut backend,
        &mut fetch,
    )
    .resolve(&desired(), &target(), &store)
    .expect_err("an unapproved intermediate redirect must fail before following it");

    assert!(matches!(
        error,
        PackageResolutionError::UnapprovedArtifactLocation
    ));
    assert_eq!(fetch.contacted, vec![initial]);
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
            approved_artifact_roots: BTreeSet::new(),
            apt_source_authority: None,
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

    let mut source_substitution = resolved.load_persisted(&store).unwrap();
    source_substitution.declaration.source = id("substituted-source");
    source_substitution.closure[0].declaration.source = id("substituted-source");
    let mut substituted_intent = resolved.clone();
    substituted_intent.declaration.source = id("substituted-source");
    substituted_intent.resolution = store
        .put(
            &serde_json::to_vec(&source_substitution).unwrap(),
            commonkit_adapters::ContentSensitivity::Portable,
        )
        .unwrap();
    assert!(matches!(
        substituted_intent.load_and_validate(&target(), &manager_binding(), &registry, &store),
        Err(PackageResolutionError::SourceBindingMismatch)
    ));
}

#[test]
fn builtin_source_registry_preserves_the_filesystem_and_generic_v1_digest() {
    assert_eq!(
        PackageSourceRegistry::builtin().unwrap().digest().as_str(),
        "sha256:188316c18c44cefdc652d4828a1904ce35621cd19ee7c4e99f8985df5b6dd2fc"
    );
}

#[test]
fn remote_source_enrichment_rebinds_registry_and_authority_digests() {
    let builtin = PackageSourceRegistry::builtin().unwrap();
    let source_id = id("homebrew-core");
    let source = SourceBindingV1 {
        source_id: source_id.clone(),
        registry_definition_digest: digest('a'),
        canonical_repository: "https://github.com/Homebrew/homebrew-core".into(),
        repository_revision: Some("0123456789abcdef0123456789abcdef0123456789".into()),
        signed_metadata: Vec::new(),
    };
    let enriched =
        PackageSourceRegistry::for_remote_resolution(&source, PackageManager::Homebrew).unwrap();
    assert_ne!(builtin.digest(), enriched.digest());

    let policy = SecurityPolicy {
        allowlists: [(
            id("package_sources"),
            BTreeSet::from([source_id.as_str().into()]),
        )]
        .into_iter()
        .collect(),
        ..SecurityPolicy::default()
    };
    let baseline =
        PackageResolutionAuthority::new(&target(), &manager_binding(), &builtin, &policy).unwrap();
    let remote =
        PackageResolutionAuthority::new(&target(), &manager_binding(), &enriched, &policy).unwrap();
    assert_ne!(baseline.digest(), remote.digest());

    let mut changed = source;
    changed.registry_definition_digest = digest('b');
    let changed_registry =
        PackageSourceRegistry::for_remote_resolution(&changed, PackageManager::Homebrew).unwrap();
    assert_ne!(enriched.digest(), changed_registry.digest());
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
fn later_denied_package_preflights_before_any_backend_or_fetch_call() {
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
    let package = |declaration: PackageDeclaration, source: &str| NormalizedResource {
        intent: PackageDesiredIntent::new(declaration).unwrap().into(),
        provenance: ResourceProvenance {
            provider_id: inputs.provider_id.clone(),
            provider_version: inputs.provider_version.to_string(),
            input_digest: inputs.input_set_digest.clone(),
            source: source.into(),
        },
    };
    let denied = PackageDeclaration {
        id: id("zz-denied"),
        version: "1.2.3".into(),
        manager: PackageManager::Homebrew,
        source: id("unapproved-source"),
        selector: Some(PackageSelector::HomebrewFormula {
            name: "zz-denied".into(),
        }),
    };
    let state = MaterializedState::finalize(
        inputs.clone(),
        vec![
            package(
                match desired() {
                    PackageDesiredIntent::Package { declaration } => declaration,
                },
                "package:allowed",
            ),
            package(denied, "package:denied"),
        ],
        vec![],
        vec![],
    )
    .unwrap();
    let registry = PackageSourceRegistry::builtin().unwrap();
    let mut backend = CountingBackend { probes: 0 };
    let mut fetch = PanicFetch;
    let error = PackageResolutionCoordinator::new(
        &allowed_policy(),
        &registry,
        manager_binding(),
        &mut backend,
        &mut fetch,
    )
    .resolve_state(&state, &target(), &store)
    .expect_err("later denied package must reject the whole state");

    assert!(matches!(
        error,
        PackageResolutionError::PackageSourceNotAllowed { .. }
    ));
    assert_eq!(
        backend.probes, 0,
        "preflight must happen before backend use"
    );
}

#[test]
fn later_invalid_closure_draft_preflights_before_any_artifact_fetch() {
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
    let package = |declaration: PackageDeclaration| NormalizedResource {
        provenance: ResourceProvenance {
            provider_id: inputs.provider_id.clone(),
            provider_version: inputs.provider_version.to_string(),
            input_digest: inputs.input_set_digest.clone(),
            source: format!("package:{}", declaration.id),
        },
        intent: PackageDesiredIntent::new(declaration).unwrap().into(),
    };
    let first = match desired() {
        PackageDesiredIntent::Package { declaration } => declaration,
    };
    let second = PackageDeclaration {
        id: id("zz-second"),
        version: "2.3.4".into(),
        manager: PackageManager::Homebrew,
        source: id("homebrew-core"),
        selector: Some(PackageSelector::HomebrewFormula {
            name: "zz-second".into(),
        }),
    };
    let state = MaterializedState::finalize(
        inputs.clone(),
        vec![package(first), package(second)],
        vec![],
        vec![],
    )
    .unwrap();
    let registry = PackageSourceRegistry::builtin().unwrap();
    let bytes = b"immutable package archive";
    let fixture = fixture_backend(bytes);
    let locator = fixture.fetch_requests[0].immutable_locator.clone();
    let mut backend = LaterInvalidClosureBackend {
        fixture,
        resolves: 0,
    };
    let mut fetch = FixtureFetch {
        bytes: BTreeMap::from([(locator, bytes.to_vec())]),
        calls: 0,
    };

    let error = PackageResolutionCoordinator::new(
        &allowed_policy(),
        &registry,
        manager_binding(),
        &mut backend,
        &mut fetch,
    )
    .resolve_state(&state, &target(), &store)
    .expect_err("all closure drafts must validate before any artifact fetch");

    assert!(matches!(
        error,
        PackageResolutionError::PackageSourceNotAllowed { .. }
            | PackageResolutionError::UnknownSource { .. }
    ));
    assert_eq!(
        fetch.calls, 0,
        "a later invalid closure must prevent every fetch"
    );
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
fn reopens_4c7b3a7_v1_resolution_without_closure_source() {
    let root = tempfile::tempdir().unwrap();
    let store = ArtifactStore::open(root.path().join("artifacts")).unwrap();
    let bytes = include_bytes!("fixtures/package-resolution-v1-4c7b3a7.json");
    let fixture: serde_json::Value = serde_json::from_slice(bytes).unwrap();
    let schema = commonkit_adapters::package_resolution_schema().unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    let schema_errors = validator
        .iter_errors(&fixture)
        .map(|error| error.to_string())
        .collect::<Vec<_>>();
    assert!(
        schema_errors.is_empty(),
        "the published v1 schema must accept the 4c7b3a7 artifact before migration: {schema_errors:?}"
    );
    let resolution = store
        .put(bytes, commonkit_adapters::ContentSensitivity::Portable)
        .unwrap();
    let intent = commonkit_adapters::ResolvedPackageIntent {
        declaration: match desired() {
            PackageDesiredIntent::Package { declaration } => declaration,
        },
        resolution,
        artifacts: vec![],
    };

    let reopened = intent
        .load_persisted(&store)
        .expect("the 4c7b3a7 v1 artifact must remain reopenable");

    assert_eq!(reopened.closure[0].source, reopened.source);
}

#[test]
fn schema_valid_hybrid_v1_closure_migrates_each_missing_source_and_rejects_mismatch() {
    let root = tempfile::tempdir().unwrap();
    let store = ArtifactStore::open(root.path().join("artifacts")).unwrap();
    let bytes = include_bytes!("fixtures/package-resolution-v1-hybrid.json");
    let fixture: serde_json::Value = serde_json::from_slice(bytes).unwrap();
    let schema = commonkit_adapters::package_resolution_schema().unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    assert!(
        validator.is_valid(&fixture),
        "the published v1 schema admits per-entry legacy/current closure sources"
    );
    let resolution = store
        .put(bytes, commonkit_adapters::ContentSensitivity::Portable)
        .unwrap();
    let intent = commonkit_adapters::ResolvedPackageIntent {
        declaration: match desired() {
            PackageDesiredIntent::Package { declaration } => declaration,
        },
        resolution,
        artifacts: vec![],
    };

    let reopened = intent
        .load_persisted(&store)
        .expect("a schema-valid hybrid v1 closure must reopen deterministically");
    assert!(
        reopened
            .closure
            .iter()
            .all(|package| package.source == reopened.source)
    );

    let mut mismatched = fixture.clone();
    mismatched["closure"][1]["source"]["canonicalRepository"] =
        "https://example.invalid/substituted".into();
    assert!(validator.is_valid(&mismatched));
    let mismatched_resolution = store
        .put(
            &serde_json::to_vec(&mismatched).unwrap(),
            commonkit_adapters::ContentSensitivity::Portable,
        )
        .unwrap();
    let mismatched_intent = commonkit_adapters::ResolvedPackageIntent {
        resolution: mismatched_resolution,
        ..intent
    };
    assert!(matches!(
        mismatched_intent.load_persisted(&store),
        Err(PackageResolutionError::SourceBindingMismatch)
    ));

    let mut unknown = fixture;
    unknown["closure"][0]["unexpected"] = true.into();
    assert!(
        !validator.is_valid(&unknown),
        "v1 compatibility must continue to deny unknown closure fields"
    );
}

#[test]
fn explicit_null_closure_source_is_rejected_by_schema_and_runtime() {
    let root = tempfile::tempdir().unwrap();
    let store = ArtifactStore::open(root.path().join("artifacts")).unwrap();
    let mut fixture: serde_json::Value =
        serde_json::from_slice(include_bytes!("fixtures/package-resolution-v1-hybrid.json"))
            .unwrap();
    fixture["closure"][0]["source"] = serde_json::Value::Null;
    let schema = commonkit_adapters::package_resolution_schema().unwrap();
    assert!(
        !jsonschema::validator_for(&schema)
            .unwrap()
            .is_valid(&fixture),
        "the v1 schema permits omission but not explicit null"
    );
    let resolution = store
        .put(
            &serde_json::to_vec(&fixture).unwrap(),
            commonkit_adapters::ContentSensitivity::Portable,
        )
        .unwrap();
    let intent = commonkit_adapters::ResolvedPackageIntent {
        declaration: match desired() {
            PackageDesiredIntent::Package { declaration } => declaration,
        },
        resolution,
        artifacts: vec![],
    };

    assert!(matches!(
        intent.load_persisted(&store),
        Err(PackageResolutionError::Json(_))
    ));
}

#[test]
fn resolved_materialized_state_roundtrips_for_offline_reopen() {
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
    let desired_state = MaterializedState::finalize(
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
    .resolve_state(&desired_state, &target(), &store)
    .unwrap();

    let encoded = serde_json::to_vec(&resolved).unwrap();
    let reopened: commonkit_adapters::ResolvedMaterializedState =
        serde_json::from_slice(&encoded).expect("resolved state must reopen without a provider");

    reopened.verify().unwrap();
    assert_eq!(reopened, resolved);
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
