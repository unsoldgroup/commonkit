use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use commonkit_adapters::{
    AptRepositoryConfigurationV1, AptResolutionBackend, AptResolutionCommandError,
    AptResolutionCommandRunner, AptResolutionSnapshotV1, AptResolutionSystemRequestV1,
    AptResolvedArchiveV1, AptResolvedPackageV1, AptSourceAuthorityV1, AptTransactionRisksV1,
    ArtifactEvidence, ArtifactStore, ManagerBindingV1, OfflineInstallRecipeV1, PackageFetch,
    PackageFetchHopV1, PackageFetchRequestV1, PackageFetchResultV1, PackageObservationV1,
    PackageResolutionCoordinator, PackageResolutionError, PackageSourceRegistry, PackageTargetV1,
    ProcessAptResolutionCommandRunner, ResolvedPackageIntent,
};
use commonkit_contracts::{
    PackageDeclaration, PackageManager, PackageSelector, SecurityPolicy, Sha256Digest, StableId,
};
use sha2::{Digest, Sha256};

fn id(value: &str) -> StableId {
    StableId::parse(value).unwrap()
}

fn digest(byte: u8) -> Sha256Digest {
    Sha256Digest::parse(format!(
        "sha256:{}",
        char::from(byte).to_string().repeat(64)
    ))
    .unwrap()
}

fn content_digest(bytes: &[u8]) -> Sha256Digest {
    Sha256Digest::parse(format!("sha256:{:x}", Sha256::digest(bytes))).unwrap()
}

fn target() -> PackageTargetV1 {
    PackageTargetV1 {
        os: "linux".into(),
        os_version: "24.04".into(),
        distro_id: Some("ubuntu".into()),
        distro_version: Some("24.04".into()),
        codename: Some("noble".into()),
        arch: "amd64".into(),
        libc: Some("glibc".into()),
        manager_prefix: None,
    }
}

fn manager() -> ManagerBindingV1 {
    ManagerBindingV1 {
        manager: PackageManager::Apt,
        version: "apt:2.7.14;dpkg:1.22.6".into(),
        executable_digest: digest(b'a'),
        config_digest: digest(b'b'),
    }
}

fn desired() -> commonkit_adapters::PackageDesiredIntent {
    commonkit_adapters::PackageDesiredIntent::new(apt_declaration(
        "curl",
        "8.5.0-2ubuntu10.6",
        "amd64",
    ))
    .unwrap()
}

fn apt_declaration(name: &str, version: &str, architecture: &str) -> PackageDeclaration {
    PackageDeclaration {
        id: id(name),
        version: version.into(),
        manager: PackageManager::Apt,
        source: id("ubuntu-main"),
        selector: Some(PackageSelector::AptBinary {
            name: name.into(),
            architecture: Some(architecture.into()),
        }),
    }
}

fn policy() -> SecurityPolicy {
    SecurityPolicy {
        allowlists: BTreeMap::from([(
            id("package_sources"),
            BTreeSet::from(["ubuntu-main".into()]),
        )]),
        ..SecurityPolicy::default()
    }
}

fn repository() -> AptRepositoryConfigurationV1 {
    AptRepositoryConfigurationV1 {
        source_id: id("ubuntu-main"),
        suite: "noble".into(),
        components: BTreeSet::from(["main".into()]),
        signed_by: PathBuf::from("/usr/share/keyrings/ubuntu-archive-keyring.gpg"),
        signing_authority: id("ubuntu-archive-keyring"),
    }
}

fn source_authority() -> AptSourceAuthorityV1 {
    AptSourceAuthorityV1 {
        suite: "noble".into(),
        components: BTreeSet::from(["main".into()]),
        signing_authority: id("ubuntu-archive-keyring"),
        signing_key_digest: digest(b'd'),
    }
}

fn apt_registry() -> PackageSourceRegistry {
    PackageSourceRegistry::builtin()
        .unwrap()
        .with_apt_source_authority(&id("ubuntu-main"), source_authority())
        .unwrap()
}

#[derive(Clone)]
struct FixtureRunner {
    snapshot: AptResolutionSnapshotV1,
    calls: usize,
}

struct PanicRunner;

impl AptResolutionCommandRunner for PanicRunner {
    fn resolve(
        &mut self,
        _request: &AptResolutionSystemRequestV1,
    ) -> Result<AptResolutionSnapshotV1, AptResolutionCommandError> {
        panic!("invalid APT authority must fail before the runner")
    }
}

impl AptResolutionCommandRunner for FixtureRunner {
    fn resolve(
        &mut self,
        _request: &AptResolutionSystemRequestV1,
    ) -> Result<AptResolutionSnapshotV1, AptResolutionCommandError> {
        self.calls += 1;
        Ok(self.snapshot.clone())
    }
}

struct FixtureFetch {
    bytes: BTreeMap<String, Vec<u8>>,
    calls: usize,
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

fn archive(package_name: &str, version: &str, locator: &str, bytes: &[u8]) -> AptResolvedArchiveV1 {
    AptResolvedArchiveV1 {
        package_name: package_name.into(),
        version: version.into(),
        architecture: "amd64".into(),
        immutable_locator: locator.into(),
        upstream_checksum: content_digest(bytes),
        size: bytes.len() as u64,
    }
}

fn archive_for_architecture(
    package_name: &str,
    version: &str,
    architecture: &str,
    locator: &str,
    bytes: &[u8],
) -> AptResolvedArchiveV1 {
    AptResolvedArchiveV1 {
        architecture: architecture.into(),
        ..archive(package_name, version, locator, bytes)
    }
}

fn resolved_package(name: &str, version: &str, architecture: &str) -> AptResolvedPackageV1 {
    AptResolvedPackageV1 {
        name: name.into(),
        version: version.into(),
        architecture: architecture.into(),
    }
}

fn persisted(
    intent: &ResolvedPackageIntent,
    store: &ArtifactStore,
) -> commonkit_adapters::PackageResolutionV1 {
    intent.load_persisted(store).unwrap()
}

fn apt_snapshot(curl_bytes: &[u8], libc_bytes: &[u8]) -> AptResolutionSnapshotV1 {
    AptResolutionSnapshotV1 {
        target: target(),
        manager: manager(),
        before: PackageObservationV1 {
            installed_versions: BTreeSet::new(),
        },
        repository_revision: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
            .into(),
        signed_metadata: vec![ArtifactEvidence {
            authority: id("ubuntu-archive-keyring"),
            metadata_digest: digest(b'c'),
            signature_digest: digest(b'd'),
        }],
        closure: vec![
            AptResolvedPackageV1 {
                name: "libc6".into(),
                version: "2.39-0ubuntu8.4".into(),
                architecture: "amd64".into(),
            },
            AptResolvedPackageV1 {
                name: "curl".into(),
                version: "8.5.0-2ubuntu10.6".into(),
                architecture: "amd64".into(),
            },
        ],
        archives: vec![
            archive(
                "curl",
                "8.5.0-2ubuntu10.6",
                "https://archive.ubuntu.com/ubuntu/pool/main/c/curl/curl_8.5.0-2ubuntu10.6_amd64.deb",
                curl_bytes,
            ),
            archive(
                "libc6",
                "2.39-0ubuntu8.4",
                "https://archive.ubuntu.com/ubuntu/pool/main/g/glibc/libc6_2.39-0ubuntu8.4_amd64.deb",
                libc_bytes,
            ),
        ],
        risks: AptTransactionRisksV1::default(),
    }
}

#[test]
fn process_apt_runner_has_a_fixed_private_read_only_command_plan() {
    let registry = apt_registry();
    let declaration = match desired() {
        commonkit_adapters::PackageDesiredIntent::Package { declaration } => declaration,
    };
    let request = AptResolutionSystemRequestV1 {
        declaration,
        target: target(),
        manager: manager(),
        source_id: id("ubuntu-main"),
        canonical_repository: "https://archive.ubuntu.com/ubuntu".into(),
        registry_definition_digest: registry
            .source_definition_digest(&id("ubuntu-main"))
            .unwrap(),
        source_authority: source_authority(),
        repository: repository(),
    };

    let commands = ProcessAptResolutionCommandRunner::command_snapshot(&request).unwrap();

    assert_eq!(
        commands
            .iter()
            .map(|command| command.executable.as_str())
            .collect::<Vec<_>>(),
        vec![
            "/usr/bin/apt-get",
            "/usr/bin/dpkg",
            "/usr/bin/dpkg",
            "/usr/bin/apt-config",
            "/usr/bin/dpkg-query",
            "/usr/bin/apt-mark",
            "/usr/bin/apt-get",
            "/usr/bin/apt-cache",
            "/usr/bin/apt-get",
            "/usr/bin/apt-get",
        ]
    );
    let rendered = commands
        .iter()
        .map(|command| format!("{} {}", command.executable, command.args.join(" ")))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(rendered.contains("Dir::State::lists=<private>/lists"));
    assert_eq!(
        rendered
            .matches("Dir::State::status=<private>/status")
            .count(),
        3,
        "dependency inspection, simulation and archive enumeration must use one private solver state"
    );
    assert!(rendered.contains("Dir::Cache::archives=<private>/archives"));
    assert!(rendered.contains("Acquire::AllowInsecureRepositories=false"));
    assert!(rendered.contains("Acquire::AllowWeakRepositories=false"));
    assert!(rendered.contains("Acquire::AllowDowngradeToInsecureRepositories=false"));
    assert!(rendered.contains("APT::Get::AllowUnauthenticated=false"));
    assert!(rendered.contains("Acquire::Check-Valid-Until=true"));
    assert!(rendered.contains("Acquire::Check-Date=true"));
    assert!(rendered.contains("Acquire::By-Hash=force"));
    assert!(rendered.contains("Acquire::https::AllowRedirect=false"));
    assert!(rendered.contains("--simulate --no-remove"));
    assert!(rendered.contains("--quiet=2 --print-uris --download-only --no-remove"));
    assert!(!rendered.contains("--allow-unauthenticated"));
    assert!(commands.iter().all(|command| {
        command.environment.get("APT_CONFIG").map(String::as_str) == Some("<private>/apt.conf")
    }));
    assert_eq!(
        commands
            .iter()
            .enumerate()
            .filter_map(|(index, command)| command.network.then_some(index))
            .collect::<Vec<_>>(),
        vec![6]
    );
}

#[test]
fn live_safety_simulation_pins_the_archived_closure_without_selecting_it() {
    let registry = apt_registry();
    let declaration = match desired() {
        commonkit_adapters::PackageDesiredIntent::Package { declaration } => declaration,
    };
    let request = AptResolutionSystemRequestV1 {
        declaration,
        target: target(),
        manager: manager(),
        source_id: id("ubuntu-main"),
        canonical_repository: "https://archive.ubuntu.com/ubuntu".into(),
        registry_definition_digest: registry
            .source_definition_digest(&id("ubuntu-main"))
            .unwrap(),
        source_authority: source_authority(),
        repository: repository(),
    };
    let closure = vec![
        AptResolvedPackageV1 {
            name: "curl".into(),
            version: "8.5.0-2ubuntu10.6".into(),
            architecture: "amd64".into(),
        },
        AptResolvedPackageV1 {
            name: "libc6".into(),
            version: "2.39-0ubuntu8.4".into(),
            architecture: "amd64".into(),
        },
    ];

    let command =
        ProcessAptResolutionCommandRunner::live_safety_command_snapshot(&request, &closure)
            .unwrap();
    let rendered = format!("{} {}", command.executable, command.args.join(" "));

    assert!(rendered.contains("Dir::State::status=<private>/live-status"));
    assert!(rendered.contains("--simulate --no-remove"));
    assert!(rendered.contains("curl:amd64=8.5.0-2ubuntu10.6"));
    assert!(rendered.contains("libc6:amd64=2.39-0ubuntu8.4"));
    assert!(!rendered.contains("--print-uris"));
    assert!(!command.network);
}

#[test]
fn apt_source_scope_and_key_are_part_of_registry_authority() {
    let base = PackageSourceRegistry::builtin().unwrap();
    let source_id = id("ubuntu-main");
    let first = base
        .clone()
        .with_apt_source_authority(&source_id, source_authority())
        .unwrap();
    let mut changed_scope = source_authority();
    changed_scope.components.insert("universe".into());
    let widened = base
        .clone()
        .with_apt_source_authority(&source_id, changed_scope)
        .unwrap();
    let mut changed_key = source_authority();
    changed_key.signing_key_digest = digest(b'e');
    let rekeyed = base
        .clone()
        .with_apt_source_authority(&source_id, changed_key)
        .unwrap();
    let mut changed_suite = source_authority();
    changed_suite.suite = "noble-updates".into();
    let resuited = base
        .clone()
        .with_apt_source_authority(&source_id, changed_suite)
        .unwrap();
    let mut changed_signer = source_authority();
    changed_signer.signing_authority = id("different-archive-keyring");
    let resigned = base
        .with_apt_source_authority(&source_id, changed_signer)
        .unwrap();

    assert_ne!(
        first.source_definition_digest(&source_id).unwrap(),
        widened.source_definition_digest(&source_id).unwrap()
    );
    assert_ne!(
        first.source_definition_digest(&source_id).unwrap(),
        rekeyed.source_definition_digest(&source_id).unwrap()
    );
    assert_ne!(
        first.source_definition_digest(&source_id).unwrap(),
        resuited.source_definition_digest(&source_id).unwrap()
    );
    assert_ne!(
        first.source_definition_digest(&source_id).unwrap(),
        resigned.source_definition_digest(&source_id).unwrap()
    );
}

#[test]
fn apt_backend_rejects_missing_registry_source_scope_before_runner_or_fetch() {
    let registry = PackageSourceRegistry::builtin().unwrap();
    let root = tempfile::tempdir().unwrap();
    let store = ArtifactStore::open(root.path().join("artifacts")).unwrap();
    let mut backend = AptResolutionBackend::new(repository(), PanicRunner);
    let mut fetch = FixtureFetch {
        bytes: BTreeMap::new(),
        calls: 0,
    };

    assert!(matches!(
        PackageResolutionCoordinator::new(
            &policy(),
            &registry,
            manager(),
            &mut backend,
            &mut fetch,
        )
        .resolve(&desired(), &target(), &store),
        Err(PackageResolutionError::InvalidAptRequest)
    ));
    assert_eq!(fetch.calls, 0);

    let mut mismatched = source_authority();
    mismatched.components.insert("universe".into());
    let registry = PackageSourceRegistry::builtin()
        .unwrap()
        .with_apt_source_authority(&id("ubuntu-main"), mismatched)
        .unwrap();
    let mut backend = AptResolutionBackend::new(repository(), PanicRunner);
    assert!(matches!(
        PackageResolutionCoordinator::new(
            &policy(),
            &registry,
            manager(),
            &mut backend,
            &mut fetch,
        )
        .resolve(&desired(), &target(), &store),
        Err(PackageResolutionError::InvalidAptRequest)
    ));
    assert_eq!(fetch.calls, 0);
}

#[cfg(not(target_os = "linux"))]
#[test]
fn process_apt_runner_fails_closed_off_linux() {
    let registry = apt_registry();
    let declaration = match desired() {
        commonkit_adapters::PackageDesiredIntent::Package { declaration } => declaration,
    };
    let request = AptResolutionSystemRequestV1 {
        declaration,
        target: target(),
        manager: manager(),
        source_id: id("ubuntu-main"),
        canonical_repository: "https://archive.ubuntu.com/ubuntu".into(),
        registry_definition_digest: registry
            .source_definition_digest(&id("ubuntu-main"))
            .unwrap(),
        source_authority: source_authority(),
        repository: repository(),
    };
    let mut runner = ProcessAptResolutionCommandRunner;

    assert!(matches!(
        runner.resolve(&request),
        Err(AptResolutionCommandError::Unavailable(_))
    ));
}

#[cfg(not(target_os = "linux"))]
#[test]
fn apt_manager_authority_probe_fails_closed_off_linux() {
    assert!(matches!(
        ProcessAptResolutionCommandRunner::probe_manager_binding(
            &target(),
            "https://archive.ubuntu.com/ubuntu",
            &repository(),
        ),
        Err(AptResolutionCommandError::Unavailable(_))
    ));
    assert!(matches!(
        ProcessAptResolutionCommandRunner::probe_source_authority(
            &target(),
            "https://archive.ubuntu.com/ubuntu",
            &repository(),
        ),
        Err(AptResolutionCommandError::Unavailable(_))
    ));
}

#[test]
fn apt_backend_requires_an_explicit_target_architecture_before_runner_or_fetch() {
    let mut declaration = match desired() {
        commonkit_adapters::PackageDesiredIntent::Package { declaration } => declaration,
    };
    declaration.selector = Some(PackageSelector::AptBinary {
        name: "curl".into(),
        architecture: None,
    });
    let desired = commonkit_adapters::PackageDesiredIntent::new(declaration).unwrap();
    let registry = apt_registry();
    let root = tempfile::tempdir().unwrap();
    let store = ArtifactStore::open(root.path().join("artifacts")).unwrap();
    let mut backend = AptResolutionBackend::new(repository(), PanicRunner);
    let mut fetch = FixtureFetch {
        bytes: BTreeMap::new(),
        calls: 0,
    };

    assert!(matches!(
        PackageResolutionCoordinator::new(
            &policy(),
            &registry,
            manager(),
            &mut backend,
            &mut fetch,
        )
        .resolve(&desired, &target(), &store),
        Err(PackageResolutionError::InvalidAptRequest)
    ));
    assert_eq!(fetch.calls, 0);
}

#[test]
fn apt_backend_rejects_source_line_unsafe_architecture_before_runner_or_fetch() {
    let mut target = target();
    target.arch = "amd64/foreign".into();
    let mut declaration = match desired() {
        commonkit_adapters::PackageDesiredIntent::Package { declaration } => declaration,
    };
    declaration.selector = Some(PackageSelector::AptBinary {
        name: "curl".into(),
        architecture: Some(target.arch.clone()),
    });
    let desired = commonkit_adapters::PackageDesiredIntent::new(declaration).unwrap();
    let registry = apt_registry();
    let root = tempfile::tempdir().unwrap();
    let store = ArtifactStore::open(root.path().join("artifacts")).unwrap();
    let mut backend = AptResolutionBackend::new(repository(), PanicRunner);
    let mut fetch = FixtureFetch {
        bytes: BTreeMap::new(),
        calls: 0,
    };

    assert!(matches!(
        PackageResolutionCoordinator::new(
            &policy(),
            &registry,
            manager(),
            &mut backend,
            &mut fetch,
        )
        .resolve(&desired, &target, &store),
        Err(PackageResolutionError::InvalidAptRequest)
    ));
    assert_eq!(fetch.calls, 0);
}

#[test]
fn apt_backend_rejects_unauthenticated_or_unsafe_snapshots_before_fetch() {
    let curl_bytes = b"exact curl deb";
    let libc_bytes = b"exact libc deb";
    for snapshot in {
        let mut unauthenticated = apt_snapshot(curl_bytes, libc_bytes);
        unauthenticated.signed_metadata.clear();
        let mut unsafe_transaction = apt_snapshot(curl_bytes, libc_bytes);
        unsafe_transaction.risks.removals.insert("old-lib".into());
        let mut rekeyed = apt_snapshot(curl_bytes, libc_bytes);
        rekeyed.signed_metadata[0].signature_digest = digest(b'e');
        [unauthenticated, unsafe_transaction, rekeyed]
    } {
        let registry = apt_registry();
        let root = tempfile::tempdir().unwrap();
        let store = ArtifactStore::open(root.path().join("artifacts")).unwrap();
        let mut backend =
            AptResolutionBackend::new(repository(), FixtureRunner { snapshot, calls: 0 });
        let mut fetch = FixtureFetch {
            bytes: BTreeMap::new(),
            calls: 0,
        };

        assert!(
            PackageResolutionCoordinator::new(
                &policy(),
                &registry,
                manager(),
                &mut backend,
                &mut fetch,
            )
            .resolve(&desired(), &target(), &store)
            .is_err()
        );
        assert_eq!(fetch.calls, 0);
    }
}

#[test]
fn apt_archive_outside_the_controlled_repository_is_rejected_before_fetch() {
    let curl_bytes = b"exact curl deb";
    let libc_bytes = b"exact libc deb";
    let mut snapshot = apt_snapshot(curl_bytes, libc_bytes);
    snapshot.archives[0].immutable_locator =
        "https://mirror.attacker.invalid/curl_8.5.0-2ubuntu10.6_amd64.deb".into();
    let registry = apt_registry();
    let root = tempfile::tempdir().unwrap();
    let store = ArtifactStore::open(root.path().join("artifacts")).unwrap();
    let mut backend = AptResolutionBackend::new(repository(), FixtureRunner { snapshot, calls: 0 });
    let mut fetch = FixtureFetch {
        bytes: BTreeMap::new(),
        calls: 0,
    };

    let error = PackageResolutionCoordinator::new(
        &policy(),
        &registry,
        manager(),
        &mut backend,
        &mut fetch,
    )
    .resolve(&desired(), &target(), &store)
    .unwrap_err();
    assert!(
        matches!(error, PackageResolutionError::UnapprovedArtifactLocation),
        "unexpected error: {error:?}"
    );
    assert_eq!(fetch.calls, 0);
}

#[test]
fn apt_resolution_is_deterministic_across_solver_output_order() {
    let curl_bytes = b"exact curl deb";
    let libc_bytes = b"exact libc deb";
    let registry = apt_registry();
    let root = tempfile::tempdir().unwrap();
    let store = ArtifactStore::open(root.path().join("artifacts")).unwrap();
    let resolve = |snapshot: AptResolutionSnapshotV1| {
        let bytes = BTreeMap::from([
            (
                "https://archive.ubuntu.com/ubuntu/pool/main/c/curl/curl_8.5.0-2ubuntu10.6_amd64.deb".into(),
                curl_bytes.to_vec(),
            ),
            (
                "https://archive.ubuntu.com/ubuntu/pool/main/g/glibc/libc6_2.39-0ubuntu8.4_amd64.deb".into(),
                libc_bytes.to_vec(),
            ),
        ]);
        let mut backend =
            AptResolutionBackend::new(repository(), FixtureRunner { snapshot, calls: 0 });
        let mut fetch = FixtureFetch { bytes, calls: 0 };
        PackageResolutionCoordinator::new(&policy(), &registry, manager(), &mut backend, &mut fetch)
            .resolve(&desired(), &target(), &store)
            .unwrap()
    };

    let first = resolve(apt_snapshot(curl_bytes, libc_bytes));
    let mut reordered = apt_snapshot(curl_bytes, libc_bytes);
    reordered.archives.reverse();
    reordered.closure.reverse();
    let second = resolve(reordered);

    assert_eq!(first, second);
}

#[test]
fn apt_backend_resolves_authenticated_exact_closure_and_fetches_every_archive() {
    let curl_bytes = b"exact curl deb";
    let libc_bytes = b"exact libc deb";
    let curl_url =
        "https://archive.ubuntu.com/ubuntu/pool/main/c/curl/curl_8.5.0-2ubuntu10.6_amd64.deb";
    let libc_url =
        "https://archive.ubuntu.com/ubuntu/pool/main/g/glibc/libc6_2.39-0ubuntu8.4_amd64.deb";
    let snapshot = apt_snapshot(curl_bytes, libc_bytes);
    let mut backend = AptResolutionBackend::new(repository(), FixtureRunner { snapshot, calls: 0 });
    let mut fetch = FixtureFetch {
        bytes: BTreeMap::from([
            (curl_url.into(), curl_bytes.to_vec()),
            (libc_url.into(), libc_bytes.to_vec()),
        ]),
        calls: 0,
    };
    let registry = apt_registry();
    let root = tempfile::tempdir().unwrap();
    let store = ArtifactStore::open(root.path().join("artifacts")).unwrap();

    let intent = PackageResolutionCoordinator::new(
        &policy(),
        &registry,
        manager(),
        &mut backend,
        &mut fetch,
    )
    .resolve(&desired(), &target(), &store)
    .unwrap();
    let resolution = persisted(&intent, &store);
    let mut rekeyed_authority = source_authority();
    rekeyed_authority.signing_key_digest = digest(b'e');
    let rekeyed_registry = PackageSourceRegistry::builtin()
        .unwrap()
        .with_apt_source_authority(&id("ubuntu-main"), rekeyed_authority)
        .unwrap();

    assert_eq!(resolution.closure.len(), 2);
    assert_eq!(resolution.artifacts.len(), 2);
    assert!(matches!(
        resolution.recipe,
        OfflineInstallRecipeV1::AptArchives { .. }
    ));
    assert_eq!(fetch.calls, 2);
    assert!(matches!(
        intent.load_and_validate(&target(), &manager(), &rekeyed_registry, &store),
        Err(PackageResolutionError::SourceBindingMismatch)
    ));
}

#[test]
fn architecture_all_root_persists_actual_architecture_and_target_authority() {
    let package_bytes = b"exact debian archive keyring deb";
    let package_url = "https://archive.ubuntu.com/ubuntu/pool/main/d/debian-archive-keyring/debian-archive-keyring_2023.4ubuntu1_all.deb";
    let declaration = apt_declaration("debian-archive-keyring", "2023.4ubuntu1", "amd64");
    let desired = commonkit_adapters::PackageDesiredIntent::new(declaration.clone()).unwrap();
    let all_archive = archive_for_architecture(
        "debian-archive-keyring",
        "2023.4ubuntu1",
        "all",
        package_url,
        package_bytes,
    );
    let mut snapshot = apt_snapshot(package_bytes, b"unused");
    snapshot.closure = vec![resolved_package(
        "debian-archive-keyring",
        "2023.4ubuntu1",
        "all",
    )];
    snapshot.archives = vec![all_archive];
    let mut backend = AptResolutionBackend::new(repository(), FixtureRunner { snapshot, calls: 0 });
    let mut fetch = FixtureFetch {
        bytes: BTreeMap::from([(package_url.into(), package_bytes.to_vec())]),
        calls: 0,
    };
    let registry = apt_registry();
    let root = tempfile::tempdir().unwrap();
    let store = ArtifactStore::open(root.path().join("artifacts")).unwrap();

    let intent = PackageResolutionCoordinator::new(
        &policy(),
        &registry,
        manager(),
        &mut backend,
        &mut fetch,
    )
    .resolve(&desired, &target(), &store)
    .unwrap();
    let resolution = persisted(&intent, &store);

    assert_eq!(resolution.declaration, declaration);
    assert_eq!(resolution.target.arch, "amd64");
    assert_eq!(
        resolution.closure[0].declaration,
        apt_declaration("debian-archive-keyring", "2023.4ubuntu1", "all")
    );
    assert_eq!(resolution.artifacts.len(), 1);
    assert_eq!(
        resolution.artifacts[0].role,
        id("apt-archive-f7f93d44c2b54c00ebfc14ac")
    );
    assert_eq!(fetch.calls, 1);
}

#[test]
fn foreign_architecture_does_not_satisfy_the_requested_root() {
    let package_bytes = b"foreign archive";
    let package_url = "https://archive.ubuntu.com/ubuntu/pool/main/d/debian-archive-keyring/debian-archive-keyring_2023.4ubuntu1_arm64.deb";
    let declaration = apt_declaration("debian-archive-keyring", "2023.4ubuntu1", "amd64");
    let desired = commonkit_adapters::PackageDesiredIntent::new(declaration).unwrap();
    let foreign_archive = archive_for_architecture(
        "debian-archive-keyring",
        "2023.4ubuntu1",
        "arm64",
        package_url,
        package_bytes,
    );
    let mut snapshot = apt_snapshot(package_bytes, b"unused");
    snapshot.closure = vec![resolved_package(
        "debian-archive-keyring",
        "2023.4ubuntu1",
        "arm64",
    )];
    snapshot.archives = vec![foreign_archive];
    let mut backend = AptResolutionBackend::new(repository(), FixtureRunner { snapshot, calls: 0 });
    let mut fetch = FixtureFetch {
        bytes: BTreeMap::from([(package_url.into(), package_bytes.to_vec())]),
        calls: 0,
    };
    let registry = apt_registry();
    let root = tempfile::tempdir().unwrap();
    let store = ArtifactStore::open(root.path().join("artifacts")).unwrap();

    let error = PackageResolutionCoordinator::new(
        &policy(),
        &registry,
        manager(),
        &mut backend,
        &mut fetch,
    )
    .resolve(&desired, &target(), &store)
    .unwrap_err();

    assert!(matches!(
        error,
        PackageResolutionError::IncompleteAptClosure
    ));
    assert_eq!(fetch.calls, 0);
}

#[test]
fn foreign_dependency_architecture_is_rejected_before_fetch() {
    let curl_bytes = b"exact curl deb";
    let libc_bytes = b"foreign libc deb";
    let curl_url =
        "https://archive.ubuntu.com/ubuntu/pool/main/c/curl/curl_8.5.0-2ubuntu10.6_amd64.deb";
    let foreign_url =
        "https://archive.ubuntu.com/ubuntu/pool/main/g/glibc/libc6_2.39-0ubuntu8.4_arm64.deb";
    let mut snapshot = apt_snapshot(curl_bytes, libc_bytes);
    snapshot.closure[0].architecture = "arm64".into();
    snapshot.archives[1].architecture = "arm64".into();
    snapshot.archives[1].immutable_locator = foreign_url.into();
    let mut backend = AptResolutionBackend::new(repository(), FixtureRunner { snapshot, calls: 0 });
    let mut fetch = FixtureFetch {
        bytes: BTreeMap::from([
            (curl_url.into(), curl_bytes.to_vec()),
            (foreign_url.into(), libc_bytes.to_vec()),
        ]),
        calls: 0,
    };
    let registry = apt_registry();
    let root = tempfile::tempdir().unwrap();
    let store = ArtifactStore::open(root.path().join("artifacts")).unwrap();

    let error = PackageResolutionCoordinator::new(
        &policy(),
        &registry,
        manager(),
        &mut backend,
        &mut fetch,
    )
    .resolve(&desired(), &target(), &store)
    .unwrap_err();

    assert!(matches!(
        error,
        PackageResolutionError::IncompleteAptClosure
    ));
    assert_eq!(fetch.calls, 0);
}

#[test]
fn native_and_all_architectures_for_one_package_version_are_ambiguous() {
    let native_bytes = b"native archive";
    let all_bytes = b"architecture all archive";
    let native_url = "https://archive.ubuntu.com/ubuntu/pool/main/d/debian-archive-keyring/debian-archive-keyring_2023.4ubuntu1_amd64.deb";
    let all_url = "https://archive.ubuntu.com/ubuntu/pool/main/d/debian-archive-keyring/debian-archive-keyring_2023.4ubuntu1_all.deb";
    let declaration = apt_declaration("debian-archive-keyring", "2023.4ubuntu1", "amd64");
    let desired = commonkit_adapters::PackageDesiredIntent::new(declaration).unwrap();
    let all_archive = archive_for_architecture(
        "debian-archive-keyring",
        "2023.4ubuntu1",
        "all",
        all_url,
        all_bytes,
    );
    let mut snapshot = apt_snapshot(native_bytes, all_bytes);
    snapshot.closure = vec![
        resolved_package("debian-archive-keyring", "2023.4ubuntu1", "amd64"),
        resolved_package("debian-archive-keyring", "2023.4ubuntu1", "all"),
    ];
    snapshot.archives = vec![
        archive_for_architecture(
            "debian-archive-keyring",
            "2023.4ubuntu1",
            "amd64",
            native_url,
            native_bytes,
        ),
        all_archive,
    ];
    let mut backend = AptResolutionBackend::new(repository(), FixtureRunner { snapshot, calls: 0 });
    let mut fetch = FixtureFetch {
        bytes: BTreeMap::from([
            (native_url.into(), native_bytes.to_vec()),
            (all_url.into(), all_bytes.to_vec()),
        ]),
        calls: 0,
    };
    let registry = apt_registry();
    let root = tempfile::tempdir().unwrap();
    let store = ArtifactStore::open(root.path().join("artifacts")).unwrap();

    let error = PackageResolutionCoordinator::new(
        &policy(),
        &registry,
        manager(),
        &mut backend,
        &mut fetch,
    )
    .resolve(&desired, &target(), &store)
    .unwrap_err();

    assert!(matches!(
        error,
        PackageResolutionError::IncompleteAptClosure
    ));
    assert_eq!(fetch.calls, 0);
}
