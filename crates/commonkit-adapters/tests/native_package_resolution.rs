#[cfg(unix)]
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
#[cfg(unix)]
use std::fs::{self, OpenOptions};
#[cfg(unix)]
use std::io::Read;
#[cfg(unix)]
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::time::Duration;

#[cfg(unix)]
use commonkit_adapters::{
    AptRepositoryConfigurationV1, AptResolutionBackend, AptResolutionSystemRequestV1,
    ArtifactStore, ControlledPackageSourceV1, NodeResolutionBackend, NodeRuntimeHost,
    PackageDesiredIntent, PackageDiscoveryFetchRequestV1, PackageFetch, PackageFetchHopV1,
    PackageFetchRequestV1, PackageFetchResultV1, PackageResolutionCoordinator,
    PackageResolutionError, PackageSourceRegistry, PackageTargetV1,
    ProcessAptResolutionCommandRunner, ProcessNodeReleaseSignatureVerifier, ProcessNodeRuntimeHost,
};
#[cfg(unix)]
use commonkit_contracts::{
    PackageDeclaration, PackageManager, PackageSelector, SecurityPolicy, StableId,
};
#[cfg(unix)]
use futures_util::StreamExt;
#[cfg(unix)]
use reqwest::header::LOCATION;

const OPT_IN_ENV: &str = "COMMONKIT_NATIVE_PACKAGE_RESOLUTION";

#[test]
fn native_resolution_harness_requires_an_exact_opt_in() {
    assert!(!native_harness_enabled(None));
    assert!(!native_harness_enabled(Some(OsStr::new("0"))));
    assert!(!native_harness_enabled(Some(OsStr::new("true"))));
    assert!(native_harness_enabled(Some(OsStr::new("1"))));
}

#[cfg(all(unix, not(target_os = "linux")))]
#[test]
fn native_linux_harness_remains_compile_checked_off_target() {
    let harness: fn() = run_linux_harness;
    assert_eq!(std::mem::size_of_val(&harness), std::mem::size_of::<fn()>());
}

#[test]
#[ignore = "requires explicit Linux package-resolution evidence opt-in"]
fn production_apt_and_node_resolvers_reopen_offline_without_target_mutation() {
    run_native_read_only_resolution_harness();
}

fn native_harness_enabled(value: Option<&OsStr>) -> bool {
    value == Some(OsStr::new("1"))
}

fn run_native_read_only_resolution_harness() {
    if !native_harness_enabled(std::env::var_os(OPT_IN_ENV).as_deref()) {
        return;
    }
    #[cfg(not(target_os = "linux"))]
    panic!("native package-resolution evidence requires Linux");
    #[cfg(target_os = "linux")]
    run_linux_harness();
}

#[cfg(unix)]
fn run_linux_harness() {
    let config = NativeHarnessConfig::from_environment();
    let root = redacted(tempfile::tempdir(), "private evidence workspace creation");
    let store = redacted(
        ArtifactStore::open(root.path().join("artifacts")),
        "private artifact store creation",
    );
    let mut fetch = redacted(NativeHttpsFetch::new(), "scoped HTTPS client creation");

    let apt_target = config.apt_target();
    let apt_repository = config.apt_repository();
    let apt_manager = redacted(
        ProcessAptResolutionCommandRunner::probe_manager_binding(
            &apt_target,
            &config.apt_repository_url,
            &apt_repository,
        ),
        "APT manager authority probe",
    );
    let apt_authority = redacted(
        ProcessAptResolutionCommandRunner::probe_source_authority(
            &apt_target,
            &config.apt_repository_url,
            &apt_repository,
        ),
        "APT source authority probe",
    );
    let apt_registry = redacted(
        PackageSourceRegistry::from_controlled_sources(vec![ControlledPackageSourceV1 {
            source_id: config.apt_source_id.clone(),
            manager: PackageManager::Apt,
            canonical_repository: config.apt_repository_url.clone(),
            approved_artifact_roots: BTreeSet::new(),
            apt_source_authority: Some(apt_authority.clone()),
        }]),
        "APT source registry construction",
    );
    let apt_declaration = PackageDeclaration {
        id: stable_id("native-apt-resolution"),
        version: config.apt_version.clone(),
        manager: PackageManager::Apt,
        source: config.apt_source_id.clone(),
        selector: Some(PackageSelector::AptBinary {
            name: config.apt_package.clone(),
            architecture: Some(config.target_arch.clone()),
        }),
    };
    assert_read_only_apt_plan(
        &apt_declaration,
        &apt_target,
        &apt_manager,
        &apt_registry,
        &apt_authority,
        &apt_repository,
        &config.apt_repository_url,
    );
    let mut apt_backend =
        AptResolutionBackend::new(apt_repository, ProcessAptResolutionCommandRunner);
    let apt_policy = package_policy(&config.apt_source_id);
    let apt_desired = redacted(
        PackageDesiredIntent::new(apt_declaration),
        "APT desired intent validation",
    );
    let apt_intent = redacted(
        PackageResolutionCoordinator::new(
            &apt_policy,
            &apt_registry,
            apt_manager.clone(),
            &mut apt_backend,
            &mut fetch,
        )
        .resolve(&apt_desired, &apt_target, &store),
        "APT controlled resolution",
    );
    let apt_reopened = redacted(
        apt_intent.load_and_validate(&apt_target, &apt_manager, &apt_registry, &store),
        "APT offline reopen",
    );
    println!(
        "native-resolution manager=apt os={} distro={} arch={} resolution={} closure={} artifacts={}",
        apt_target.os,
        apt_target.distro_id.as_deref().unwrap_or("unknown"),
        apt_target.arch,
        apt_intent.resolution.digest,
        apt_reopened.closure.len(),
        apt_reopened.artifacts.len()
    );

    let isolated_home = root.path().join("node-home");
    let isolated_nvm = isolated_home.join(".nvm");
    redacted(
        fs::create_dir_all(&isolated_nvm),
        "isolated nvm workspace creation",
    );
    let nvm_script = redacted(
        read_regular_no_follow(&config.node_nvm_script),
        "pinned nvm script read",
    );
    redacted(
        fs::write(isolated_nvm.join("nvm.sh"), nvm_script),
        "isolated nvm script copy",
    );
    let node_target = config.node_target(&isolated_nvm);
    if let Some(existing_nvm) = &config.existing_nvm_dir {
        let mut old_target = node_target.clone();
        old_target.manager_prefix = Some(path_text(existing_nvm, "existing nvm path"));
        let mut existing_host = ProcessNodeRuntimeHost::new(
            existing_nvm,
            &config.node_shell,
            &config.node_release_keyring,
        );
        assert!(
            existing_host.probe(&old_target).is_err(),
            "the explicitly supplied unsafe nvm authority unexpectedly passed"
        );
    }
    let mut host_probe = ProcessNodeRuntimeHost::new(
        &isolated_nvm,
        &config.node_shell,
        &config.node_release_keyring,
    );
    let node_snapshot = redacted(host_probe.probe(&node_target), "nvm authority probe");
    let node_manager = node_snapshot.manager;
    let node_registry = redacted(
        PackageSourceRegistry::builtin().and_then(|registry| {
            registry.with_commonkit_node_release_authority(&config.node_source_id)
        }),
        "Node source registry construction",
    );
    let node_policy = package_policy(&config.node_source_id);
    let node_desired = redacted(
        PackageDesiredIntent::new(PackageDeclaration {
            id: stable_id("native-node-resolution"),
            version: config.node_version.clone(),
            manager: PackageManager::Nvm,
            source: config.node_source_id.clone(),
            selector: Some(PackageSelector::NodeRuntime {}),
        }),
        "Node desired intent validation",
    );
    let mut node_host = ProcessNodeRuntimeHost::new(
        &isolated_nvm,
        &config.node_shell,
        &config.node_release_keyring,
    );
    let mut verifier = ProcessNodeReleaseSignatureVerifier::new(&config.node_gpgv);
    let mut node_backend = NodeResolutionBackend::new(&mut node_host, &mut verifier);
    let node_intent = redacted(
        PackageResolutionCoordinator::new(
            &node_policy,
            &node_registry,
            node_manager.clone(),
            &mut node_backend,
            &mut fetch,
        )
        .resolve(&node_desired, &node_target, &store),
        "Node controlled resolution",
    );
    drop(node_backend);
    let node_reopened = redacted(
        node_intent.load_and_validate(&node_target, &node_manager, &node_registry, &store),
        "Node offline reopen",
    );
    println!(
        "native-resolution manager=nvm os={} arch={} resolution={} closure={} artifacts={}",
        node_target.os,
        node_target.arch,
        node_intent.resolution.digest,
        node_reopened.closure.len(),
        node_reopened.artifacts.len()
    );
}

#[cfg(unix)]
fn assert_read_only_apt_plan(
    declaration: &PackageDeclaration,
    target: &PackageTargetV1,
    manager: &commonkit_adapters::ManagerBindingV1,
    registry: &PackageSourceRegistry,
    authority: &commonkit_adapters::AptSourceAuthorityV1,
    repository: &AptRepositoryConfigurationV1,
    canonical_repository: &str,
) {
    let request = AptResolutionSystemRequestV1 {
        declaration: declaration.clone(),
        target: target.clone(),
        manager: manager.clone(),
        source_id: repository.source_id.clone(),
        canonical_repository: canonical_repository.into(),
        registry_definition_digest: redacted(
            registry.source_definition_digest(&repository.source_id),
            "APT registry digest lookup",
        ),
        source_authority: authority.clone(),
        repository: repository.clone(),
    };
    let commands = redacted(
        ProcessAptResolutionCommandRunner::command_snapshot(&request),
        "APT read-only command plan",
    );
    for command in commands {
        assert!(
            !command.args.iter().any(|argument| matches!(
                argument.as_str(),
                "install" | "remove" | "purge" | "upgrade" | "dist-upgrade"
            )),
            "native evidence command plan contains a target mutation verb"
        );
    }
}

#[cfg(unix)]
struct NativeHarnessConfig {
    target_os_version: String,
    target_distro_id: String,
    target_distro_version: String,
    target_codename: String,
    target_arch: String,
    apt_source_id: StableId,
    apt_repository_url: String,
    apt_suite: String,
    apt_components: BTreeSet<String>,
    apt_signing_key: PathBuf,
    apt_signing_authority: StableId,
    apt_package: String,
    apt_version: String,
    node_source_id: StableId,
    node_version: String,
    node_nvm_script: PathBuf,
    existing_nvm_dir: Option<PathBuf>,
    node_shell: PathBuf,
    node_release_keyring: PathBuf,
    node_gpgv: PathBuf,
}

#[cfg(unix)]
impl NativeHarnessConfig {
    fn from_environment() -> Self {
        Self {
            target_os_version: required_text("COMMONKIT_NATIVE_TARGET_OS_VERSION"),
            target_distro_id: required_text("COMMONKIT_NATIVE_TARGET_DISTRO_ID"),
            target_distro_version: required_text("COMMONKIT_NATIVE_TARGET_DISTRO_VERSION"),
            target_codename: required_text("COMMONKIT_NATIVE_TARGET_CODENAME"),
            target_arch: required_text("COMMONKIT_NATIVE_TARGET_ARCH"),
            apt_source_id: required_id("COMMONKIT_NATIVE_APT_SOURCE_ID"),
            apt_repository_url: required_text("COMMONKIT_NATIVE_APT_REPOSITORY"),
            apt_suite: required_text("COMMONKIT_NATIVE_APT_SUITE"),
            apt_components: required_text("COMMONKIT_NATIVE_APT_COMPONENTS")
                .split(',')
                .map(str::trim)
                .map(str::to_owned)
                .collect(),
            apt_signing_key: required_path("COMMONKIT_NATIVE_APT_SIGNING_KEY"),
            apt_signing_authority: required_id("COMMONKIT_NATIVE_APT_SIGNING_AUTHORITY"),
            apt_package: required_text("COMMONKIT_NATIVE_APT_PACKAGE"),
            apt_version: required_text("COMMONKIT_NATIVE_APT_VERSION"),
            node_source_id: required_id("COMMONKIT_NATIVE_NODE_SOURCE_ID"),
            node_version: required_text("COMMONKIT_NATIVE_NODE_VERSION"),
            node_nvm_script: required_path("COMMONKIT_NATIVE_NVM_SCRIPT"),
            existing_nvm_dir: optional_path("COMMONKIT_NATIVE_EXISTING_NVM_DIR"),
            node_shell: required_path("COMMONKIT_NATIVE_NODE_SHELL"),
            node_release_keyring: required_path("COMMONKIT_NATIVE_NODE_RELEASE_KEYRING"),
            node_gpgv: required_path("COMMONKIT_NATIVE_NODE_GPGV"),
        }
    }

    fn apt_target(&self) -> PackageTargetV1 {
        PackageTargetV1 {
            os: "linux".into(),
            os_version: self.target_os_version.clone(),
            distro_id: Some(self.target_distro_id.clone()),
            distro_version: Some(self.target_distro_version.clone()),
            codename: Some(self.target_codename.clone()),
            arch: self.target_arch.clone(),
            libc: Some("glibc".into()),
            manager_prefix: None,
        }
    }

    fn node_target(&self, nvm_dir: &Path) -> PackageTargetV1 {
        PackageTargetV1 {
            manager_prefix: Some(path_text(nvm_dir, "isolated nvm path")),
            ..self.apt_target()
        }
    }

    fn apt_repository(&self) -> AptRepositoryConfigurationV1 {
        AptRepositoryConfigurationV1 {
            source_id: self.apt_source_id.clone(),
            suite: self.apt_suite.clone(),
            components: self.apt_components.clone(),
            signed_by: self.apt_signing_key.clone(),
            signing_authority: self.apt_signing_authority.clone(),
        }
    }
}

#[cfg(unix)]
struct NativeHttpsFetch {
    runtime: tokio::runtime::Runtime,
    client: reqwest::Client,
}

#[cfg(unix)]
impl NativeHttpsFetch {
    fn new() -> Result<Self, PackageResolutionError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| PackageResolutionError::FetchUnavailable)?;
        let client = reqwest::Client::builder()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(120))
            .user_agent("commonkit-native-resolution-evidence/1")
            .build()
            .map_err(|_| PackageResolutionError::FetchUnavailable)?;
        Ok(Self { runtime, client })
    }

    fn fetch_one(
        &mut self,
        locator: &str,
        maximum_bytes: u64,
    ) -> Result<PackageFetchHopV1, PackageResolutionError> {
        let client = self.client.clone();
        let locator = locator.to_owned();
        self.runtime.block_on(async move {
            let url = validated_https_url(&locator)?;
            let response = client
                .get(url.clone())
                .send()
                .await
                .map_err(|_| PackageResolutionError::FetchUnavailable)?;
            if response.status().is_redirection() {
                let location = response
                    .headers()
                    .get(LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .ok_or(PackageResolutionError::UnvalidatedRedirect)?;
                let location = url
                    .join(location)
                    .map_err(|_| PackageResolutionError::UnvalidatedRedirect)?;
                return Ok(PackageFetchHopV1::Redirect {
                    location: location.into(),
                });
            }
            if !response.status().is_success()
                || response
                    .content_length()
                    .is_some_and(|length| length > maximum_bytes)
            {
                return Err(PackageResolutionError::FetchUnavailable);
            }
            let mut bytes = Vec::new();
            let mut stream = response.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|_| PackageResolutionError::FetchUnavailable)?;
                let next = u64::try_from(bytes.len())
                    .ok()
                    .and_then(|length| length.checked_add(u64::try_from(chunk.len()).ok()?))
                    .ok_or(PackageResolutionError::CorruptArtifact)?;
                if next > maximum_bytes {
                    return Err(PackageResolutionError::CorruptArtifact);
                }
                bytes.extend_from_slice(&chunk);
            }
            Ok(PackageFetchHopV1::Complete(PackageFetchResultV1 { bytes }))
        })
    }
}

#[cfg(unix)]
impl PackageFetch for NativeHttpsFetch {
    fn preflight_locators(&mut self, locators: &[String]) -> Result<(), PackageResolutionError> {
        for locator in locators {
            validated_https_url(locator)?;
        }
        Ok(())
    }

    fn fetch_hop(
        &mut self,
        request: &PackageFetchRequestV1,
        locator: &str,
    ) -> Result<PackageFetchHopV1, PackageResolutionError> {
        self.fetch_one(locator, request.size)
    }

    fn fetch_discovery_hop(
        &mut self,
        request: &PackageDiscoveryFetchRequestV1,
        locator: &str,
    ) -> Result<PackageFetchHopV1, PackageResolutionError> {
        self.fetch_one(locator, request.maximum_bytes)
    }
}

#[cfg(unix)]
fn validated_https_url(value: &str) -> Result<reqwest::Url, PackageResolutionError> {
    let url = reqwest::Url::parse(value)
        .map_err(|_| PackageResolutionError::UnapprovedArtifactLocation)?;
    if url.scheme() != "https"
        || url.username() != ""
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(PackageResolutionError::UnapprovedArtifactLocation);
    }
    Ok(url)
}

#[cfg(unix)]
fn required_text(name: &'static str) -> String {
    let value =
        std::env::var_os(name).unwrap_or_else(|| panic!("required harness input {name} is absent"));
    value
        .into_string()
        .unwrap_or_else(|_| panic!("required harness input {name} is not UTF-8"))
}

#[cfg(unix)]
fn required_path(name: &'static str) -> PathBuf {
    let path = PathBuf::from(required_text(name));
    assert!(
        path.is_absolute(),
        "required harness path {name} is not absolute"
    );
    path
}

#[cfg(unix)]
fn optional_path(name: &'static str) -> Option<PathBuf> {
    std::env::var_os(name).map(|value| {
        let path = PathBuf::from(value);
        assert!(
            path.is_absolute(),
            "optional harness path {name} is not absolute"
        );
        path
    })
}

#[cfg(unix)]
fn required_id(name: &'static str) -> StableId {
    redacted(
        StableId::parse(required_text(name)),
        "stable harness identifier",
    )
}

#[cfg(unix)]
fn stable_id(value: &str) -> StableId {
    StableId::parse(value).expect("static harness ID is valid")
}

#[cfg(unix)]
fn package_policy(source_id: &StableId) -> SecurityPolicy {
    SecurityPolicy {
        allowlists: BTreeMap::from([(
            stable_id("package_sources"),
            BTreeSet::from([source_id.as_str().to_owned()]),
        )]),
        ..SecurityPolicy::default()
    }
}

#[cfg(unix)]
fn path_text(path: &Path, stage: &'static str) -> String {
    path.to_str()
        .map(str::to_owned)
        .unwrap_or_else(|| panic!("{stage} is not UTF-8"))
}

#[cfg(unix)]
fn read_regular_no_follow(path: &Path) -> Result<Vec<u8>, std::io::Error> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::other("not a regular file"));
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

#[cfg(unix)]
fn redacted<T, E>(result: Result<T, E>, stage: &'static str) -> T {
    result.unwrap_or_else(|_| panic!("{stage} failed"))
}
