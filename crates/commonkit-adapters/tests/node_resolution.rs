use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;

use commonkit_adapters::{
    ArtifactStore, COMMONKIT_NVM_SCRIPT_RELEASES, ContentSensitivity, ControlledPackageSourceV1,
    ManagerBindingV1, NodeReleaseSignatureError, NodeReleaseSignatureVerifier,
    NodeResolutionBackend, NodeRuntimeHost, NodeRuntimeHostError, NodeRuntimeHostSnapshotV1,
    NodeSourceAuthorityV1, OfflineInstallRecipeV1, PackageFetch, PackageFetchHopV1,
    PackageFetchRequestV1, PackageFetchResultV1, PackageObservationV1,
    PackageResolutionCoordinator, PackageResolutionError, PackageSourceRegistry, PackageTargetV1,
    ProcessNodeReleaseSignatureVerifier, ProcessNodeRuntimeHost, ResolvedPackageIntent,
    package_resolution_v2_schema, validate_nvm_environment, validate_nvm_environment_os,
};
use commonkit_contracts::{
    PackageDeclaration, PackageManager, PackageSelector, SecurityPolicy, Sha256Digest, StableId,
    digest_domain_json,
};
use sha2::{Digest, Sha256};

fn id(value: &str) -> StableId {
    StableId::parse(value).unwrap()
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
        manager_prefix: Some("/home/al/.nvm".into()),
    }
}

fn desired() -> commonkit_adapters::PackageDesiredIntent {
    commonkit_adapters::PackageDesiredIntent::new(PackageDeclaration {
        id: id("node"),
        version: "22.14.0".into(),
        manager: PackageManager::Nvm,
        source: id("nodejs-nvm"),
        selector: Some(PackageSelector::NodeRuntime {}),
    })
    .unwrap()
}

fn policy() -> SecurityPolicy {
    SecurityPolicy {
        allowlists: BTreeMap::from([(
            id("package_sources"),
            BTreeSet::from(["nodejs-nvm".into()]),
        )]),
        ..SecurityPolicy::default()
    }
}

fn source_authority() -> NodeSourceAuthorityV1 {
    NodeSourceAuthorityV1 {
        release_key_fingerprints: BTreeSet::from([
            "5be8a3f6c8a5c01d106c0ad820b1a390b168d356".into()
        ]),
        nvm_script_releases: COMMONKIT_NVM_SCRIPT_RELEASES
            .iter()
            .map(|(version, digest)| ((*version).into(), Sha256Digest::parse(*digest).unwrap()))
            .collect(),
    }
}

fn registry() -> PackageSourceRegistry {
    PackageSourceRegistry::builtin()
        .unwrap()
        .with_commonkit_node_release_authority(&id("nodejs-nvm"))
        .unwrap()
}

fn host_snapshot() -> NodeRuntimeHostSnapshotV1 {
    let nvm_script_digest = Sha256Digest::parse(COMMONKIT_NVM_SCRIPT_RELEASES[0].1).unwrap();
    let shell_executable_digest = content_digest(b"bash fixture");
    let release_keyring_digest = content_digest(b"release keyring fixture");
    let gpgv_executable_digest = content_digest(b"gpgv fixture");
    let config_digest = digest_domain_json(
        "commonkit.nvm-manager-config.v1",
        &(
            &shell_executable_digest,
            &release_keyring_digest,
            &gpgv_executable_digest,
            "/home/al/.nvm",
        ),
    )
    .unwrap();
    NodeRuntimeHostSnapshotV1 {
        target: target(),
        manager: ManagerBindingV1 {
            manager: PackageManager::Nvm,
            version: "0.40.6".into(),
            executable_digest: nvm_script_digest.clone(),
            config_digest,
        },
        before: PackageObservationV1 {
            installed_versions: BTreeSet::new(),
        },
        nvm_script_digest,
        shell_executable_digest,
        release_keyring_digest,
        gpgv_executable_digest,
        release_keyring: b"release keyring fixture".to_vec(),
    }
}

#[derive(Clone)]
struct FixtureHost {
    snapshot: Result<NodeRuntimeHostSnapshotV1, NodeRuntimeHostError>,
    calls: usize,
}

impl NodeRuntimeHost for FixtureHost {
    fn probe(
        &mut self,
        _target: &PackageTargetV1,
    ) -> Result<NodeRuntimeHostSnapshotV1, NodeRuntimeHostError> {
        self.calls += 1;
        self.snapshot.clone()
    }
}

#[derive(Clone)]
struct FixtureVerifier {
    fingerprint: String,
    signed_payload: Vec<u8>,
    calls: usize,
}

impl NodeReleaseSignatureVerifier for FixtureVerifier {
    fn verify(
        &mut self,
        _armored_signature: &[u8],
        _keyring: &[u8],
    ) -> Result<commonkit_adapters::VerifiedNodeReleaseSignatureV1, NodeReleaseSignatureError> {
        self.calls += 1;
        Ok(commonkit_adapters::VerifiedNodeReleaseSignatureV1 {
            signer_fingerprint: self.fingerprint.clone(),
            signed_payload: self.signed_payload.clone(),
        })
    }
}

struct FixtureFetch {
    bytes: BTreeMap<String, Vec<u8>>,
    contacted: Vec<String>,
}

impl PackageFetch for FixtureFetch {
    fn fetch_hop(
        &mut self,
        _request: &PackageFetchRequestV1,
        locator: &str,
    ) -> Result<PackageFetchHopV1, PackageResolutionError> {
        self.contacted.push(locator.into());
        let bytes = self
            .bytes
            .get(locator)
            .cloned()
            .ok_or(PackageResolutionError::FetchUnavailable)?;
        Ok(PackageFetchHopV1::Complete(PackageFetchResultV1 { bytes }))
    }
}

fn release_fixture(archive: &[u8]) -> (BTreeMap<String, Vec<u8>>, Vec<u8>, String, String, String) {
    let version_root = "https://nodejs.org/dist/v22.14.0";
    let archive_name = "node-v22.14.0-linux-x64.tar.xz";
    let archive_url = format!("{version_root}/{archive_name}");
    let sums_url = format!("{version_root}/SHASUMS256.txt");
    let signature_url = format!("{version_root}/SHASUMS256.txt.asc");
    let sums = format!("{:x}  {archive_name}\n", Sha256::digest(archive)).into_bytes();
    (
        BTreeMap::from([
            (archive_url.clone(), archive.to_vec()),
            (sums_url.clone(), sums.clone()),
            (signature_url.clone(), b"armored signature fixture".to_vec()),
        ]),
        sums,
        archive_url,
        sums_url,
        signature_url,
    )
}

fn manager() -> ManagerBindingV1 {
    host_snapshot().manager
}

fn persisted(
    intent: &ResolvedPackageIntent,
    store: &ArtifactStore,
) -> commonkit_adapters::PackageResolutionV1 {
    intent.load_persisted(store).unwrap()
}

#[test]
fn node_backend_resolves_signed_exact_archive_and_reopens_offline() {
    let archive = b"exact node archive";
    let (bytes, sums, archive_url, sums_url, signature_url) = release_fixture(archive);
    let mut host = FixtureHost {
        snapshot: Ok(host_snapshot()),
        calls: 0,
    };
    let mut verifier = FixtureVerifier {
        fingerprint: source_authority()
            .release_key_fingerprints
            .iter()
            .next()
            .unwrap()
            .clone(),
        signed_payload: sums,
        calls: 0,
    };
    let mut backend = NodeResolutionBackend::new(&mut host, &mut verifier);
    let mut fetch = FixtureFetch {
        bytes,
        contacted: Vec::new(),
    };
    let root = tempfile::tempdir().unwrap();
    let store = ArtifactStore::open(root.path().join("artifacts")).unwrap();
    let registry = registry();

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

    assert_eq!(fetch.contacted, vec![sums_url, signature_url, archive_url]);
    assert_eq!(resolution.artifacts.len(), 3);
    assert!(matches!(
        &resolution.recipe,
        OfflineInstallRecipeV1::NodeArchive {
            install: Some(_),
            ..
        }
    ));
    let OfflineInstallRecipeV1::NodeArchive {
        install: Some(install),
        ..
    } = &resolution.recipe
    else {
        panic!("expected bound nvm recipe")
    };
    assert_eq!(install.node_version, "22.14.0");
    assert_eq!(install.archive_file_name, "node-v22.14.0-linux-x64.tar.xz");
    assert_eq!(
        install.cache_relative_path,
        ".cache/bin/node-v22.14.0-linux-x64/node-v22.14.0-linux-x64.tar.xz"
    );
    assert!(install.offline);
    assert!(install.no_source_fallback);
    assert!(install.per_version_lock);
    assert!(!install.install_latest_npm);
    assert!(!install.migrate_packages);
    drop(backend);
    assert_eq!(host.calls, 1);
    assert_eq!(verifier.calls, 1);
    let reopened = intent
        .load_and_validate(&target(), &manager(), &registry, &store)
        .unwrap();
    assert_eq!(reopened, resolution);
}

#[test]
fn persisted_node_recipe_rejects_a_different_relative_cache_path() {
    let archive = b"exact node archive";
    let (bytes, sums, _, _, _) = release_fixture(archive);
    let mut host = FixtureHost {
        snapshot: Ok(host_snapshot()),
        calls: 0,
    };
    let mut verifier = FixtureVerifier {
        fingerprint: source_authority()
            .release_key_fingerprints
            .iter()
            .next()
            .unwrap()
            .clone(),
        signed_payload: sums,
        calls: 0,
    };
    let mut backend = NodeResolutionBackend::new(&mut host, &mut verifier);
    let mut fetch = FixtureFetch {
        bytes,
        contacted: Vec::new(),
    };
    let root = tempfile::tempdir().unwrap();
    let store = ArtifactStore::open(root.path().join("artifacts")).unwrap();
    let intent = PackageResolutionCoordinator::new(
        &policy(),
        &registry(),
        manager(),
        &mut backend,
        &mut fetch,
    )
    .resolve(&desired(), &target(), &store)
    .unwrap();
    let mut resolution = persisted(&intent, &store);
    let OfflineInstallRecipeV1::NodeArchive {
        install: Some(install),
        ..
    } = &mut resolution.recipe
    else {
        panic!("expected bound nvm recipe")
    };
    install.cache_relative_path = ".cache/bin/alternate/node.tar.xz".into();
    let resolution_reference = store
        .put(
            &serde_json::to_vec(&resolution).unwrap(),
            ContentSensitivity::Portable,
        )
        .unwrap();
    let tampered = ResolvedPackageIntent {
        declaration: intent.declaration,
        resolution: resolution_reference,
        artifacts: intent.artifacts,
    };

    assert!(matches!(
        tampered.load_and_validate(&target(), &manager(), &registry(), &store),
        Err(PackageResolutionError::InvalidNodeRequest)
    ));
}

#[test]
fn node_source_key_drift_invalidates_persisted_authority() {
    let base = registry();
    let changed = PackageSourceRegistry::builtin()
        .unwrap()
        .with_node_source_authority(
            &id("nodejs-nvm"),
            NodeSourceAuthorityV1 {
                release_key_fingerprints: BTreeSet::from([
                    "dd792f5973c6de52c432cbdac77abfa00ddbf2b7".into(),
                ]),
                nvm_script_releases: source_authority().nvm_script_releases,
            },
        )
        .unwrap();

    assert_ne!(
        base.source_definition_digest(&id("nodejs-nvm")).unwrap(),
        changed.source_definition_digest(&id("nodejs-nvm")).unwrap()
    );
}

#[test]
fn production_node_source_authority_rejects_unapproved_signers() {
    let arbitrary = NodeSourceAuthorityV1 {
        release_key_fingerprints: BTreeSet::from(["00".repeat(20)]),
        nvm_script_releases: source_authority().nvm_script_releases,
    };
    assert!(matches!(
        PackageSourceRegistry::builtin()
            .unwrap()
            .with_node_source_authority(&id("nodejs-nvm"), arbitrary),
        Err(PackageResolutionError::MutableSourceMetadata)
    ));
    assert!(
        PackageSourceRegistry::builtin()
            .unwrap()
            .with_node_source_authority(&id("nodejs-nvm"), source_authority())
            .is_ok()
    );

    let mut unknown_release = source_authority();
    unknown_release.nvm_script_releases =
        BTreeMap::from([("0.99.0".into(), content_digest(b"unapproved nvm script"))]);
    assert!(matches!(
        PackageSourceRegistry::builtin()
            .unwrap()
            .with_node_source_authority(&id("nodejs-nvm"), unknown_release),
        Err(PackageResolutionError::MutableSourceMetadata)
    ));
}

#[test]
fn node_backend_rejects_signature_or_checksum_mismatch() {
    let archive = b"exact node archive";
    let (bytes, sums, _, _, _) = release_fixture(archive);
    for (fingerprint, signed_payload) in [
        ("00".repeat(20), sums.clone()),
        (
            source_authority()
                .release_key_fingerprints
                .iter()
                .next()
                .unwrap()
                .clone(),
            b"different signed payload".to_vec(),
        ),
    ] {
        let mut host = FixtureHost {
            snapshot: Ok(host_snapshot()),
            calls: 0,
        };
        let mut verifier = FixtureVerifier {
            fingerprint,
            signed_payload,
            calls: 0,
        };
        let mut backend = NodeResolutionBackend::new(&mut host, &mut verifier);
        let mut fetch = FixtureFetch {
            bytes: bytes.clone(),
            contacted: Vec::new(),
        };
        let root = tempfile::tempdir().unwrap();
        let store = ArtifactStore::open(root.path().join("artifacts")).unwrap();

        assert!(
            PackageResolutionCoordinator::new(
                &policy(),
                &registry(),
                manager(),
                &mut backend,
                &mut fetch,
            )
            .resolve(&desired(), &target(), &store)
            .is_err()
        );
        assert_eq!(
            fetch.contacted.len(),
            2,
            "archive fetch must wait for signed metadata verification"
        );
    }
}

#[test]
fn node_resolution_is_deterministic_for_identical_authority_and_release_bytes() {
    let root = tempfile::tempdir().unwrap();
    let store = ArtifactStore::open(root.path().join("artifacts")).unwrap();
    let resolve = || {
        let archive = b"exact node archive";
        let (bytes, sums, _, _, _) = release_fixture(archive);
        let mut host = FixtureHost {
            snapshot: Ok(host_snapshot()),
            calls: 0,
        };
        let mut verifier = FixtureVerifier {
            fingerprint: source_authority()
                .release_key_fingerprints
                .into_iter()
                .next()
                .unwrap(),
            signed_payload: sums,
            calls: 0,
        };
        let mut backend = NodeResolutionBackend::new(&mut host, &mut verifier);
        let mut fetch = FixtureFetch {
            bytes,
            contacted: Vec::new(),
        };
        PackageResolutionCoordinator::new(
            &policy(),
            &registry(),
            manager(),
            &mut backend,
            &mut fetch,
        )
        .resolve(&desired(), &target(), &store)
        .unwrap()
    };

    assert_eq!(resolve(), resolve());
}

struct RejectingPreflightFetch {
    fetch_calls: usize,
    preflight_sets: Vec<Vec<String>>,
}

impl PackageFetch for RejectingPreflightFetch {
    fn preflight_locators(&mut self, locators: &[String]) -> Result<(), PackageResolutionError> {
        self.preflight_sets.push(locators.to_vec());
        Err(PackageResolutionError::UnapprovedArtifactLocation)
    }

    fn fetch_hop(
        &mut self,
        _request: &PackageFetchRequestV1,
        _locator: &str,
    ) -> Result<PackageFetchHopV1, PackageResolutionError> {
        self.fetch_calls += 1;
        panic!("all release locators must preflight before network")
    }
}

#[test]
fn node_backend_preflights_the_complete_release_set_before_network() {
    let mut host = FixtureHost {
        snapshot: Ok(host_snapshot()),
        calls: 0,
    };
    let mut verifier = FixtureVerifier {
        fingerprint: source_authority()
            .release_key_fingerprints
            .into_iter()
            .next()
            .unwrap(),
        signed_payload: Vec::new(),
        calls: 0,
    };
    let mut backend = NodeResolutionBackend::new(&mut host, &mut verifier);
    let mut fetch = RejectingPreflightFetch {
        fetch_calls: 0,
        preflight_sets: Vec::new(),
    };
    let root = tempfile::tempdir().unwrap();
    let store = ArtifactStore::open(root.path().join("artifacts")).unwrap();

    assert!(matches!(
        PackageResolutionCoordinator::new(
            &policy(),
            &registry(),
            manager(),
            &mut backend,
            &mut fetch,
        )
        .resolve(&desired(), &target(), &store),
        Err(PackageResolutionError::UnapprovedArtifactLocation)
    ));
    assert_eq!(fetch.fetch_calls, 0);
    assert_eq!(fetch.preflight_sets.len(), 1);
    assert_eq!(fetch.preflight_sets[0].len(), 3);
}

#[test]
fn node_backend_rejects_a_registered_custom_mirror_before_host_or_network() {
    let custom_registry =
        PackageSourceRegistry::from_controlled_sources(vec![ControlledPackageSourceV1 {
            source_id: id("nodejs-nvm"),
            manager: PackageManager::Nvm,
            canonical_repository: "https://mirror.example.invalid/node".into(),
            approved_artifact_roots: BTreeSet::new(),
            apt_source_authority: None,
        }])
        .unwrap()
        .with_commonkit_node_release_authority(&id("nodejs-nvm"))
        .unwrap();
    let mut host = FixtureHost {
        snapshot: Ok(host_snapshot()),
        calls: 0,
    };
    let mut verifier = FixtureVerifier {
        fingerprint: String::new(),
        signed_payload: Vec::new(),
        calls: 0,
    };
    let mut backend = NodeResolutionBackend::new(&mut host, &mut verifier);
    let mut fetch = FixtureFetch {
        bytes: BTreeMap::new(),
        contacted: Vec::new(),
    };
    let root = tempfile::tempdir().unwrap();
    let store = ArtifactStore::open(root.path().join("artifacts")).unwrap();

    assert!(matches!(
        PackageResolutionCoordinator::new(
            &policy(),
            &custom_registry,
            manager(),
            &mut backend,
            &mut fetch,
        )
        .resolve(&desired(), &target(), &store),
        Err(PackageResolutionError::InvalidNodeRequest)
    ));
    drop(backend);
    assert_eq!(host.calls, 0);
    assert!(fetch.contacted.is_empty());
}

#[test]
fn node_backend_rejects_wrong_archive_or_platform_without_fallback() {
    let archive = b"exact node archive";
    let (mut bytes, sums, archive_url, _, _) = release_fixture(archive);
    bytes.insert(archive_url, b"substituted archive".to_vec());
    let mut host = FixtureHost {
        snapshot: Ok(host_snapshot()),
        calls: 0,
    };
    let mut verifier = FixtureVerifier {
        fingerprint: source_authority()
            .release_key_fingerprints
            .into_iter()
            .next()
            .unwrap(),
        signed_payload: sums,
        calls: 0,
    };
    let mut backend = NodeResolutionBackend::new(&mut host, &mut verifier);
    let mut fetch = FixtureFetch {
        bytes,
        contacted: Vec::new(),
    };
    let root = tempfile::tempdir().unwrap();
    let store = ArtifactStore::open(root.path().join("artifacts")).unwrap();
    assert!(matches!(
        PackageResolutionCoordinator::new(
            &policy(),
            &registry(),
            manager(),
            &mut backend,
            &mut fetch,
        )
        .resolve(&desired(), &target(), &store),
        Err(PackageResolutionError::CorruptArtifact)
    ));

    let mut unsupported_target = target();
    unsupported_target.os = "windows".into();
    unsupported_target.arch = "amd64".into();
    unsupported_target.libc = None;
    let mut unsupported_snapshot = host_snapshot();
    unsupported_snapshot.target = unsupported_target.clone();
    let mut host = FixtureHost {
        snapshot: Ok(unsupported_snapshot),
        calls: 0,
    };
    let mut verifier = FixtureVerifier {
        fingerprint: String::new(),
        signed_payload: Vec::new(),
        calls: 0,
    };
    let mut backend = NodeResolutionBackend::new(&mut host, &mut verifier);
    let mut fetch = FixtureFetch {
        bytes: BTreeMap::new(),
        contacted: Vec::new(),
    };
    assert!(
        PackageResolutionCoordinator::new(
            &policy(),
            &registry(),
            manager(),
            &mut backend,
            &mut fetch,
        )
        .resolve(&desired(), &unsupported_target, &store)
        .is_err()
    );
    assert!(fetch.contacted.is_empty());
}

#[test]
fn node_backend_rejects_a_signed_manifest_for_the_wrong_exact_version() {
    let archive = b"exact node archive";
    let (mut bytes, _sums, _, sums_url, _) = release_fixture(archive);
    let wrong_sums = format!(
        "{:x}  node-v22.13.1-linux-x64.tar.xz\n",
        Sha256::digest(archive)
    )
    .into_bytes();
    bytes.insert(sums_url, wrong_sums.clone());
    let mut host = FixtureHost {
        snapshot: Ok(host_snapshot()),
        calls: 0,
    };
    let mut verifier = FixtureVerifier {
        fingerprint: source_authority()
            .release_key_fingerprints
            .into_iter()
            .next()
            .unwrap(),
        signed_payload: wrong_sums,
        calls: 0,
    };
    let mut backend = NodeResolutionBackend::new(&mut host, &mut verifier);
    let mut fetch = FixtureFetch {
        bytes,
        contacted: Vec::new(),
    };
    let root = tempfile::tempdir().unwrap();
    let store = ArtifactStore::open(root.path().join("artifacts")).unwrap();

    assert!(matches!(
        PackageResolutionCoordinator::new(
            &policy(),
            &registry(),
            manager(),
            &mut backend,
            &mut fetch,
        )
        .resolve(&desired(), &target(), &store),
        Err(PackageResolutionError::IncompleteNodeRelease)
    ));
    assert_eq!(fetch.contacted.len(), 2);
}

#[test]
fn nvm_environment_and_signature_command_are_closed_and_typed() {
    for variable in [
        "NPM_CONFIG_PREFIX",
        "PREFIX",
        "NVM_NODEJS_ORG_MIRROR",
        "NVM_IOJS_ORG_MIRROR",
        "NVM_REINSTALL_PACKAGES_FROM",
        "NVM_INSTALL_LATEST_NPM",
    ] {
        assert!(
            validate_nvm_environment(&BTreeMap::from([(variable.into(), "x".into())])).is_err()
        );
    }
    assert!(validate_nvm_environment(&BTreeMap::new()).is_ok());

    for variable in [
        "npm_config_prefix",
        "NPM_CONFIG_USERCONFIG",
        "npm_config_registry",
    ] {
        assert!(
            validate_nvm_environment_os(&BTreeMap::from([(
                OsString::from(variable),
                OsString::from("x"),
            )]))
            .is_err()
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        assert!(
            validate_nvm_environment_os(&BTreeMap::from([(
                OsString::from_vec(b"npm_config_\xff".to_vec()),
                OsString::from("x"),
            )]))
            .is_err()
        );
        assert!(
            validate_nvm_environment_os(&BTreeMap::from([(
                OsString::from("npm_config_prefix"),
                OsString::from_vec(vec![0xff]),
            )]))
            .is_err()
        );
    }

    let command = ProcessNodeReleaseSignatureVerifier::new("/usr/bin/gpgv").command_snapshot();
    assert_eq!(command.executable.to_string_lossy(), "/usr/bin/gpgv");
    assert_eq!(command.args[0], "--status-fd=1");
    assert_eq!(command.args[1], "--keyring");
    assert_eq!(command.args.last().unwrap(), "<private>/SHASUMS256.txt.asc");
    assert_eq!(command.environment.len(), 1);
}

fn process_host_fixture() -> (tempfile::TempDir, ProcessNodeRuntimeHost, PackageTargetV1) {
    let root = tempfile::tempdir().unwrap();
    let nvm_dir = root.path().join(".nvm");
    std::fs::create_dir_all(&nvm_dir).unwrap();
    std::fs::write(
        nvm_dir.join("nvm.sh"),
        include_bytes!("fixtures/node/nvm-v0.40.6.sh"),
    )
    .unwrap();
    let shell = root.path().join("bash");
    let keyring = root.path().join("node-release-keyring.kbx");
    let gpgv = root.path().join("gpgv");
    std::fs::write(&shell, b"bash fixture").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    std::fs::write(&keyring, b"keyring fixture").unwrap();
    std::fs::write(&gpgv, b"gpgv fixture").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&gpgv, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let mut target = target();
    target.manager_prefix = Some(nvm_dir.to_string_lossy().into_owned());
    (
        root,
        ProcessNodeRuntimeHost::new_with_gpgv(nvm_dir, shell, keyring, gpgv),
        target,
    )
}

#[test]
fn process_nvm_probe_requires_nvm_and_rejects_default_packages_or_prefix() {
    let (_root, mut host, target) = process_host_fixture();
    let snapshot = host.probe(&target).unwrap();
    assert_eq!(snapshot.manager.version, "0.40.6");
    assert_eq!(
        snapshot.nvm_script_digest.as_str(),
        COMMONKIT_NVM_SCRIPT_RELEASES[0].1,
    );

    let (root, mut host, target) = process_host_fixture();
    std::fs::write(root.path().join(".nvm/default-packages"), b"typescript\n").unwrap();
    assert!(host.probe(&target).is_err());

    let (root, mut host, target) = process_host_fixture();
    std::fs::write(root.path().join(".npmrc"), b"prefix=/tmp/escape\n").unwrap();
    assert!(host.probe(&target).is_err());

    let (root, mut host, target) = process_host_fixture();
    std::fs::remove_file(root.path().join(".nvm/nvm.sh")).unwrap();
    assert!(host.probe(&target).is_err());
}

#[test]
fn node_backend_rejects_an_unknown_nvm_release_before_fetch() {
    let mut snapshot = host_snapshot();
    let unknown_digest = content_digest(b"unknown nvm release");
    snapshot.manager.version = "0.99.0".into();
    snapshot.manager.executable_digest = unknown_digest.clone();
    snapshot.nvm_script_digest = unknown_digest;
    let manager = snapshot.manager.clone();
    let mut host = FixtureHost {
        snapshot: Ok(snapshot),
        calls: 0,
    };
    let mut verifier = FixtureVerifier {
        fingerprint: String::new(),
        signed_payload: Vec::new(),
        calls: 0,
    };
    let mut backend = NodeResolutionBackend::new(&mut host, &mut verifier);
    let mut fetch = FixtureFetch {
        bytes: BTreeMap::new(),
        contacted: Vec::new(),
    };
    let root = tempfile::tempdir().unwrap();
    let store = ArtifactStore::open(root.path().join("artifacts")).unwrap();

    assert!(matches!(
        PackageResolutionCoordinator::new(
            &policy(),
            &registry(),
            manager,
            &mut backend,
            &mut fetch,
        )
        .resolve(&desired(), &target(), &store),
        Err(PackageResolutionError::NodeAuthorityMismatch)
    ));
    assert!(fetch.contacted.is_empty());
}

#[test]
fn process_nvm_probe_rejects_ambiguous_or_executable_version_dispatches() {
    let valid = include_str!("fixtures/node/nvm-v0.40.6.sh");
    let cases = [
        format!("NVM_VERSION='9.9.9'\n{valid}"),
        format!("nvm_echo '9.9.9'\n{valid}"),
        format!("{valid}{valid}"),
        valid.replace("    ;;\n", ""),
        valid.replace("'0.40.6'", "\"$NVM_VERSION\""),
        valid.replace("'0.40.6'", "'$(printf 0.40.6)'"),
        valid.replace("'0.40.6'", "'0.40.6'; touch /tmp/spoof"),
        valid.replace("0.40.6", "0.040.6"),
        valid.replace("0.40.6", "0.40.5"),
        valid.replace("\"--version\" | \"-v\")", "\"--version\" | \"-v\") # spoof"),
    ];

    for script in cases {
        let (root, mut host, target) = process_host_fixture();
        std::fs::write(root.path().join(".nvm/nvm.sh"), script).unwrap();

        assert!(matches!(
            host.probe(&target),
            Err(NodeRuntimeHostError::UnsafeConfiguration(_))
        ));
    }
}

#[test]
fn process_nvm_probe_rejects_scripts_outside_the_commonkit_release_authority() {
    let valid = include_str!("fixtures/node/nvm-v0.40.6.sh");
    let cases = [
        format!("{valid}\n# altered after release\n"),
        format!("cat <<'SPOOF'\n{valid}SPOOF\n"),
        valid.replace("0.40.6", "0.99.0"),
    ];

    for script in cases {
        let (root, mut host, target) = process_host_fixture();
        std::fs::write(root.path().join(".nvm/nvm.sh"), script).unwrap();

        assert_eq!(
            host.probe(&target),
            Err(NodeRuntimeHostError::UnsafeConfiguration(
                "nvm.sh does not match a CommonKit-approved release".into(),
            )),
        );
    }
}

#[test]
fn checked_in_v2_schema_matches_and_requires_the_bound_nvm_recipe() {
    let checked_in: serde_json::Value = serde_json::from_slice(
        &std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../schemas/package-resolution-v2.schema.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let generated = package_resolution_v2_schema().unwrap();
    assert_eq!(checked_in, generated);

    let validator = jsonschema::validator_for(&generated).unwrap();
    let mut legacy_node = serde_json::json!({
        "schemaVersion": 2,
        "declaration": {
            "id": "node",
            "version": "22.14.0",
            "manager": "nvm",
            "source": "nodejs-nvm",
            "selector": { "type": "node_runtime" }
        },
        "target": {
            "os": "linux",
            "osVersion": "24.04",
            "arch": "amd64",
            "libc": "glibc",
            "managerPrefix": "/home/al/.nvm"
        },
        "manager": {
            "manager": "nvm",
            "version": "0.40.6",
            "executableDigest": content_digest(b"nvm"),
            "configDigest": content_digest(b"config")
        },
        "source": {
            "sourceId": "nodejs-nvm",
            "registryDefinitionDigest": content_digest(b"registry"),
            "canonicalRepository": "https://nodejs.org/dist",
            "signedMetadata": []
        },
        "before": { "installedVersions": [] },
        "closure": [],
        "artifacts": [],
        "recipe": { "type": "node_archive", "artifact_roles": [] }
    });
    assert!(!validator.is_valid(&legacy_node));
    legacy_node["recipe"]["install"] =
        serde_json::to_value(commonkit_adapters::NodeOfflineInstallRecipeV1 {
            node_version: "22.14.0".into(),
            archive_file_name: "node-v22.14.0-linux-x64.tar.xz".into(),
            cache_relative_path:
                ".cache/bin/node-v22.14.0-linux-x64/node-v22.14.0-linux-x64.tar.xz".into(),
            nvm_version: "0.40.6".into(),
            nvm_script_digest: content_digest(b"nvm"),
            shell_executable_digest: content_digest(b"shell"),
            offline: true,
            no_source_fallback: true,
            per_version_lock: true,
            install_latest_npm: false,
            migrate_packages: false,
        })
        .unwrap();
    assert!(validator.is_valid(&legacy_node));
    assert!(
        serde_json::from_value::<commonkit_adapters::PackageResolutionV1>(legacy_node.clone())
            .is_ok()
    );

    for malicious in [
        {
            let mut value = legacy_node.clone();
            value["schemaVersion"] = serde_json::json!(3);
            value
        },
        {
            let mut value = legacy_node.clone();
            value["recipe"]["install"] = serde_json::Value::Null;
            value
        },
        {
            let mut value = legacy_node.clone();
            value["manager"]["manager"] = serde_json::json!("apt");
            value
        },
        {
            let mut value = legacy_node.clone();
            value["declaration"]["manager"] = serde_json::json!("apt");
            value
        },
        {
            let mut value = legacy_node.clone();
            value["recipe"] = serde_json::json!({
                "type": "apt_archives",
                "artifact_roles": []
            });
            value
        },
        {
            let mut value = legacy_node.clone();
            value["recipe"]["install"]["offline"] = serde_json::json!(false);
            value
        },
        {
            let mut value = legacy_node.clone();
            value["recipe"]["install"]["noSourceFallback"] = serde_json::json!(false);
            value
        },
        {
            let mut value = legacy_node.clone();
            value["recipe"]["install"]["perVersionLock"] = serde_json::json!(false);
            value
        },
        {
            let mut value = legacy_node.clone();
            value["recipe"]["install"]["installLatestNpm"] = serde_json::json!(true);
            value
        },
        {
            let mut value = legacy_node.clone();
            value["recipe"]["install"]["migratePackages"] = serde_json::json!(true);
            value
        },
    ] {
        assert!(!validator.is_valid(&malicious), "{malicious}");
    }
}
