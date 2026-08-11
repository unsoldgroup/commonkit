use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::Duration;

use commonkit_contracts::{PackageManager, StableId};
use commonkit_core::{RootAccess, TargetRoot};
use futures_util::StreamExt;
use sha2::{Digest, Sha256};

use crate::{
    AptResolutionBackend, AptSourceAuthorityV1, ArtifactStore, ContentSensitivity,
    LocalTargetFilesystem, NodeResolutionBackend, NodeRuntimeHost, PackageDiscoveryFetchRequestV1,
    PackageFetch, PackageFetchHopV1, PackageFetchRequestV1, PackageMutationArtifact,
    PackageMutationBackend, PackageMutationPhase, PackageResolutionBackend,
    PackageResolutionCoordinator, PackageResolutionError, PackageSourceRegistry,
    ProcessAptResolutionCommandRunner, ProcessNodeReleaseSignatureVerifier, ProcessNodeRuntimeHost,
    ProcessOfflinePackageBackend, SshFilesystemRequest, SshFilesystemResponse, TargetFilesystem,
    TargetFilesystemError, TargetPackageResolutionConfig, package_resolution_request_digest,
    package_resolution_response_digest,
};

const MAX_RESOLUTION_ARTIFACT_BYTES: u64 = 512 * 1024 * 1024;
const MAX_RESOLUTION_ARTIFACT_COUNT: usize = 128;

pub struct TargetHelper {
    roots: BTreeMap<StableId, (PathBuf, RootAccess)>,
    artifacts: ArtifactStore,
    receipts: PathBuf,
    package_resolution: Option<TargetPackageResolutionConfig>,
}

impl TargetHelper {
    pub fn open(roots: Vec<TargetRoot>, state_root: &Path) -> Result<Self, TargetFilesystemError> {
        Self::open_with_package_resolution(roots, state_root, None)
    }

    pub fn open_with_package_resolution(
        roots: Vec<TargetRoot>,
        state_root: &Path,
        package_resolution: Option<TargetPackageResolutionConfig>,
    ) -> Result<Self, TargetFilesystemError> {
        let mut mapped = BTreeMap::new();
        for root in roots {
            let path = PathBuf::from(&root.path);
            if mapped.insert(root.id, (path, root.access)).is_some() {
                return Err(TargetFilesystemError::InvalidSshConfig("duplicate root id"));
            }
        }
        let artifacts = ArtifactStore::open(state_root.join("artifacts"))
            .map_err(|_| TargetFilesystemError::RemoteArtifact)?;
        let receipts = state_root.join("receipts");
        std::fs::create_dir_all(&receipts)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&receipts, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(Self {
            roots: mapped,
            artifacts,
            receipts,
            package_resolution,
        })
    }

    fn root(&self, id: &StableId) -> Result<LocalTargetFilesystem, TargetFilesystemError> {
        let (path, access) = self
            .roots
            .get(id)
            .ok_or_else(|| TargetFilesystemError::UnknownRoot(id.clone()))?;
        LocalTargetFilesystem::open(path, *access)
    }

    #[allow(clippy::too_many_arguments)]
    fn package_resolution(
        &self,
        root_id: StableId,
        request_id: StableId,
        request_nonce: commonkit_contracts::Sha256Digest,
        desired: crate::PackageDesiredIntent,
        target: crate::PackageTargetV1,
        manager_kind: PackageManager,
        policy: commonkit_contracts::SecurityPolicy,
        apt: Option<crate::AptResolutionConstraints>,
        target_identity_digest: commonkit_contracts::Sha256Digest,
        request_digest: commonkit_contracts::Sha256Digest,
    ) -> Result<SshFilesystemResponse, TargetFilesystemError> {
        let Some(config) = self.package_resolution.clone() else {
            return Ok(package_resolution_rejected(
                request_id,
                request_nonce,
                request_digest,
                target_identity_digest,
            ));
        };
        if !self.roots.contains_key(&root_id)
            || target_identity_digest != config.target_identity_digest
            || target != config.target
            || manager_kind != config.manager.manager
            || policy != config.policy
            || apt.as_ref() != expected_apt_constraints(config.apt.as_ref()).as_ref()
        {
            return Ok(package_resolution_rejected(
                request_id,
                request_nonce,
                request_digest,
                target_identity_digest,
            ));
        }
        let expected_request_digest = package_resolution_request_digest(
            &root_id,
            &desired,
            &target,
            manager_kind,
            &policy,
            &apt,
            &target_identity_digest,
            &request_nonce,
        )
        .map_err(|_| TargetFilesystemError::PackageResolutionRejected)?;
        if expected_request_digest != request_digest {
            return Ok(package_resolution_rejected(
                request_id,
                request_nonce,
                request_digest,
                target_identity_digest,
            ));
        }

        let registry = target_package_source_registry(&config)?;
        match config.manager.manager {
            PackageManager::Apt => {
                let repository = config
                    .apt
                    .clone()
                    .ok_or(TargetFilesystemError::PackageResolutionUnavailable)?;
                let canonical = apt_canonical_repository(&repository.source_id)?;
                let actual = ProcessAptResolutionCommandRunner::probe_manager_binding(
                    &config.target,
                    canonical,
                    &repository,
                )
                .map_err(|_| TargetFilesystemError::PackageResolutionUnavailable)?;
                if actual != config.manager {
                    return Err(TargetFilesystemError::PackageResolutionRejected);
                }
                let mut backend =
                    AptResolutionBackend::new(repository, ProcessAptResolutionCommandRunner);
                self.resolve_package_with_backend(
                    &config,
                    &registry,
                    &mut backend,
                    root_id,
                    request_id,
                    request_nonce,
                    desired,
                    target_identity_digest,
                    request_digest,
                )
            }
            PackageManager::Nvm => {
                let node = config
                    .node
                    .clone()
                    .ok_or(TargetFilesystemError::PackageResolutionUnavailable)?;
                let mut host = ProcessNodeRuntimeHost::new_with_gpgv(
                    node.nvm_dir,
                    node.shell_executable,
                    node.release_keyring,
                    node.gpgv_executable.clone(),
                );
                let actual = commonkit_adapters_node_probe(&mut host, &config.target)?;
                if actual.manager != config.manager {
                    return Err(TargetFilesystemError::PackageResolutionRejected);
                }
                let mut verifier = ProcessNodeReleaseSignatureVerifier::new_with_digest(
                    node.gpgv_executable,
                    Some(node.gpgv_executable_digest),
                );
                let mut backend = NodeResolutionBackend::new(&mut host, &mut verifier);
                self.resolve_package_with_backend(
                    &config,
                    &registry,
                    &mut backend,
                    root_id,
                    request_id,
                    request_nonce,
                    desired,
                    target_identity_digest,
                    request_digest,
                )
            }
            _ => Err(TargetFilesystemError::PackageResolutionUnavailable),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn resolve_package_with_backend(
        &self,
        config: &TargetPackageResolutionConfig,
        registry: &PackageSourceRegistry,
        backend: &mut dyn PackageResolutionBackend,
        _root_id: StableId,
        request_id: StableId,
        request_nonce: commonkit_contracts::Sha256Digest,
        desired: crate::PackageDesiredIntent,
        target_identity_digest: commonkit_contracts::Sha256Digest,
        request_digest: commonkit_contracts::Sha256Digest,
    ) -> Result<SshFilesystemResponse, TargetFilesystemError> {
        let mut fetch = TargetPackageFetch::new()
            .map_err(|_| TargetFilesystemError::PackageResolutionUnavailable)?;
        let mut coordinator = PackageResolutionCoordinator::new(
            &config.policy,
            registry,
            config.manager.clone(),
            backend,
            &mut fetch,
        );
        let intent = coordinator
            .resolve(&desired, &config.target, &self.artifacts)
            .map_err(|_| TargetFilesystemError::PackageResolutionFailed)?;
        let resolution_bytes = self
            .artifacts
            .load(&intent.resolution)
            .map_err(|_| TargetFilesystemError::RemoteArtifact)?;
        let resolution: crate::PackageResolutionV1 = serde_json::from_slice(&resolution_bytes)
            .map_err(|_| TargetFilesystemError::PackageResolutionFailed)?;
        if resolution.declaration
            != match &desired {
                crate::PackageDesiredIntent::Package { declaration } => declaration.clone(),
            }
            || resolution.target != config.target
            || resolution.manager != config.manager
        {
            return Err(TargetFilesystemError::PackageResolutionRejected);
        }
        let mut artifacts = Vec::with_capacity(intent.artifacts.len());
        if intent.artifacts.len() > MAX_RESOLUTION_ARTIFACT_COUNT {
            return Err(TargetFilesystemError::RemoteArtifact);
        }
        let mut total_artifact_bytes = 0u64;
        for reference in intent.artifacts {
            let bytes = self
                .artifacts
                .load(&reference)
                .map_err(|_| TargetFilesystemError::RemoteArtifact)?;
            if u64::try_from(bytes.len()).ok() != Some(reference.bytes)
                || reference.bytes > MAX_RESOLUTION_ARTIFACT_BYTES
            {
                return Err(TargetFilesystemError::RemoteArtifact);
            }
            total_artifact_bytes = total_artifact_bytes
                .checked_add(reference.bytes)
                .ok_or(TargetFilesystemError::RemoteArtifact)?;
            if total_artifact_bytes > MAX_RESOLUTION_ARTIFACT_BYTES {
                return Err(TargetFilesystemError::RemoteArtifact);
            }
            artifacts.push(PackageMutationArtifact { reference, bytes });
        }
        let response_digest = package_resolution_response_digest(
            &request_digest,
            &request_nonce,
            &target_identity_digest,
            &resolution,
            &artifacts,
        )
        .map_err(|_| TargetFilesystemError::PackageResolutionFailed)?;
        Ok(SshFilesystemResponse::PackageResolution {
            request_id,
            request_nonce,
            request_digest,
            target_identity_digest,
            response_digest,
            resolution,
            artifacts,
        })
    }

    pub fn dispatch(
        &self,
        request: SshFilesystemRequest,
    ) -> Result<SshFilesystemResponse, TargetFilesystemError> {
        match request {
            SshFilesystemRequest::ReadFile { root_id, path } => {
                Ok(match self.root(&root_id)?.read_file(&path)? {
                    Some(content) => SshFilesystemResponse::File { content },
                    None => SshFilesystemResponse::Absent,
                })
            }
            SshFilesystemRequest::InspectResource { root_id, path } => {
                Ok(SshFilesystemResponse::Resource {
                    resource: self.root(&root_id)?.inspect_resource(&path)?,
                })
            }
            SshFilesystemRequest::WriteFile {
                root_id,
                path,
                content,
            } => {
                self.root(&root_id)?.write_file(&path, &content)?;
                Ok(SshFilesystemResponse::Applied)
            }
            SshFilesystemRequest::WriteDirectory {
                root_id,
                path,
                mode,
            } => {
                self.root(&root_id)?.write_directory(&path, mode.as_ref())?;
                Ok(SshFilesystemResponse::Applied)
            }
            SshFilesystemRequest::WriteSymlink {
                root_id,
                path,
                target,
                target_kind,
            } => {
                self.root(&root_id)?
                    .write_symlink(&path, &target, target_kind)?;
                Ok(SshFilesystemResponse::Applied)
            }
            SshFilesystemRequest::Remove { root_id, path } => {
                self.root(&root_id)?.remove_resource(&path)?;
                Ok(SshFilesystemResponse::Applied)
            }
            SshFilesystemRequest::PackageMutation {
                root_id,
                phase,
                resolution,
                artifacts,
            } => {
                let (root_path, access) = self
                    .roots
                    .get(&root_id)
                    .ok_or_else(|| TargetFilesystemError::UnknownRoot(root_id.clone()))?;
                if phase != PackageMutationPhase::Observe && access != &RootAccess::ReadWrite {
                    return Err(TargetFilesystemError::ReadOnly);
                }
                for artifact in artifacts {
                    let stored = self
                        .artifacts
                        .put(&artifact.bytes, artifact.reference.sensitivity)
                        .map_err(|_| TargetFilesystemError::RemoteArtifact)?;
                    if stored.digest != artifact.reference.digest
                        || stored.bytes != artifact.reference.bytes
                    {
                        return Err(TargetFilesystemError::RemoteArtifact);
                    }
                }
                let mut backend = ProcessOfflinePackageBackend::new(root_path);
                match phase {
                    PackageMutationPhase::Observe => {
                        let observed = PackageMutationBackend::observe(&mut backend, &resolution)
                            .map_err(|_| TargetFilesystemError::PackageCommandFailed)?;
                        Ok(SshFilesystemResponse::PackageObserved {
                            installed_versions: observed.installed_versions,
                        })
                    }
                    PackageMutationPhase::Prepare => {
                        PackageMutationBackend::prepare_offline(
                            &mut backend,
                            &resolution,
                            &self.artifacts,
                        )
                        .map_err(|_| TargetFilesystemError::PackageCommandFailed)?;
                        Ok(SshFilesystemResponse::Applied)
                    }
                    PackageMutationPhase::Apply => {
                        PackageMutationBackend::apply_offline(
                            &mut backend,
                            &resolution,
                            &self.artifacts,
                        )
                        .map_err(|_| TargetFilesystemError::PackageCommandFailed)?;
                        Ok(SshFilesystemResponse::Applied)
                    }
                    PackageMutationPhase::Verify => {
                        PackageMutationBackend::verify_offline(
                            &mut backend,
                            &resolution,
                            &self.artifacts,
                        )
                        .map_err(|_| TargetFilesystemError::PackageCommandFailed)?;
                        Ok(SshFilesystemResponse::Applied)
                    }
                }
            }
            // A target resolver is intentionally a separate capability from
            // mutation. Until the verified target resolver is installed, the
            // helper rejects this request rather than authorizing a
            // controller-side resolution against the wrong machine.
            SshFilesystemRequest::PackageResolution {
                request_id,
                request_nonce,
                request_digest,
                target_identity_digest,
                root_id,
                desired,
                target,
                manager_kind,
                policy,
                apt,
            } => match self.package_resolution(
                root_id,
                request_id.clone(),
                request_nonce.clone(),
                desired,
                target,
                manager_kind,
                policy,
                apt,
                target_identity_digest.clone(),
                request_digest.clone(),
            ) {
                Ok(response) => Ok(response),
                Err(_) => Ok(package_resolution_rejected(
                    request_id,
                    request_nonce,
                    request_digest,
                    target_identity_digest,
                )),
            },
            SshFilesystemRequest::StageArtifact {
                digest, content, ..
            } => {
                let reference = self
                    .artifacts
                    .put(&content, ContentSensitivity::Portable)
                    .map_err(|_| TargetFilesystemError::RemoteArtifact)?;
                if reference.digest != digest {
                    return Err(TargetFilesystemError::RemoteArtifact);
                }
                Ok(SshFilesystemResponse::ArtifactStaged { digest })
            }
            SshFilesystemRequest::VerifyArtifact { digest, .. } => {
                self.artifacts
                    .verify_digest(&digest)
                    .map_err(|_| TargetFilesystemError::RemoteArtifact)?;
                Ok(SshFilesystemResponse::ArtifactVerified { digest })
            }
            SshFilesystemRequest::BindRecoveryReceipt {
                run_id,
                receipt_digest,
            } => {
                self.artifacts
                    .verify_digest(&receipt_digest)
                    .map_err(|_| TargetFilesystemError::RemoteArtifact)?;
                let path = self.receipts.join(format!("{}.digest", run_id.as_str()));
                let bytes = receipt_digest.as_str().as_bytes();
                let mut options = std::fs::OpenOptions::new();
                options.write(true).create_new(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                match options.open(&path) {
                    Ok(mut file) => {
                        use std::io::Write;
                        file.write_all(bytes)?;
                        file.sync_all()?;
                    }
                    Err(error)
                        if error.kind() == std::io::ErrorKind::AlreadyExists
                            && std::fs::read(&path)? == bytes => {}
                    Err(error) => return Err(error.into()),
                }
                Ok(SshFilesystemResponse::RecoveryReceiptBound { receipt_digest })
            }
            SshFilesystemRequest::RecoverRun {
                run_id,
                receipt_digest,
            } => {
                self.artifacts
                    .verify_digest(&receipt_digest)
                    .map_err(|_| TargetFilesystemError::RemoteArtifact)?;
                let bound =
                    std::fs::read(self.receipts.join(format!("{}.digest", run_id.as_str())))?;
                if bound != receipt_digest.as_str().as_bytes() {
                    return Err(TargetFilesystemError::RemoteArtifact);
                }
                Ok(SshFilesystemResponse::RecoveryReady { receipt_digest })
            }
        }
    }
}

fn package_resolution_rejected(
    request_id: StableId,
    request_nonce: commonkit_contracts::Sha256Digest,
    request_digest: commonkit_contracts::Sha256Digest,
    target_identity_digest: commonkit_contracts::Sha256Digest,
) -> SshFilesystemResponse {
    SshFilesystemResponse::PackageResolutionRejected {
        request_id,
        request_nonce,
        request_digest,
        target_identity_digest,
    }
}

fn expected_apt_constraints(
    repository: Option<&crate::AptRepositoryConfigurationV1>,
) -> Option<crate::AptResolutionConstraints> {
    repository.map(|repository| crate::AptResolutionConstraints {
        source_id: repository.source_id.clone(),
        suite: repository.suite.clone(),
        components: repository.components.clone(),
        signing_authority: repository.signing_authority.clone(),
    })
}

fn apt_canonical_repository(source_id: &StableId) -> Result<&'static str, TargetFilesystemError> {
    match source_id.as_str() {
        "ubuntu-main" => Ok("https://archive.ubuntu.com/ubuntu"),
        "debian-main" => Ok("https://deb.debian.org/debian"),
        _ => Err(TargetFilesystemError::PackageResolutionUnavailable),
    }
}

fn target_package_source_registry(
    config: &TargetPackageResolutionConfig,
) -> Result<PackageSourceRegistry, TargetFilesystemError> {
    match config.manager.manager {
        PackageManager::Apt => {
            let apt = config
                .apt
                .as_ref()
                .ok_or(TargetFilesystemError::PackageResolutionUnavailable)?;
            let key_metadata = std::fs::symlink_metadata(&apt.signed_by)
                .map_err(|_| TargetFilesystemError::PackageResolutionUnavailable)?;
            if key_metadata.file_type().is_symlink() || !key_metadata.is_file() {
                return Err(TargetFilesystemError::PackageResolutionUnavailable);
            }
            let key = std::fs::read(&apt.signed_by)
                .map_err(|_| TargetFilesystemError::PackageResolutionUnavailable)?;
            let key_digest = commonkit_contracts::Sha256Digest::parse(format!(
                "sha256:{:x}",
                Sha256::digest(&key)
            ))
            .map_err(|_| TargetFilesystemError::PackageResolutionUnavailable)?;
            PackageSourceRegistry::builtin()
                .and_then(|registry| {
                    registry.with_apt_source_authority(
                        &apt.source_id,
                        AptSourceAuthorityV1 {
                            suite: apt.suite.clone(),
                            components: apt.components.clone(),
                            signing_authority: apt.signing_authority.clone(),
                            signing_key_digest: key_digest,
                        },
                    )
                })
                .map_err(|_| TargetFilesystemError::PackageResolutionUnavailable)
        }
        PackageManager::Nvm => PackageSourceRegistry::builtin()
            .and_then(|registry| {
                registry
                    .with_commonkit_node_release_authority(&StableId::parse("nodejs-nvm").unwrap())
            })
            .map_err(|_| TargetFilesystemError::PackageResolutionUnavailable),
        _ => Err(TargetFilesystemError::PackageResolutionUnavailable),
    }
}

fn commonkit_adapters_node_probe(
    host: &mut ProcessNodeRuntimeHost,
    target: &crate::PackageTargetV1,
) -> Result<crate::NodeRuntimeHostSnapshotV1, TargetFilesystemError> {
    host.probe(target)
        .map_err(|_| TargetFilesystemError::PackageResolutionUnavailable)
}

struct TargetPackageFetch {
    runtime: tokio::runtime::Runtime,
    pinned_addresses: BTreeMap<String, SocketAddr>,
}

impl TargetPackageFetch {
    fn new() -> Result<Self, PackageResolutionError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| PackageResolutionError::FetchUnavailable)?;
        Ok(Self {
            runtime,
            pinned_addresses: BTreeMap::new(),
        })
    }

    fn pin_url(&mut self, url: &reqwest::Url) -> Result<SocketAddr, PackageResolutionError> {
        validate_package_fetch_url(url)?;
        let key = url.as_str().to_owned();
        if let Some(address) = self.pinned_addresses.get(&key) {
            return Ok(*address);
        }
        let address = validated_package_fetch_addresses(url)?
            .into_iter()
            .next()
            .ok_or(PackageResolutionError::UnapprovedArtifactLocation)?;
        self.pinned_addresses.insert(key, address);
        Ok(address)
    }

    fn fetch_one(
        &mut self,
        locator: &str,
        maximum_bytes: u64,
    ) -> Result<PackageFetchHopV1, PackageResolutionError> {
        let url = reqwest::Url::parse(locator)
            .map_err(|_| PackageResolutionError::UnapprovedArtifactLocation)?;
        let address = self.pin_url(&url)?;
        let host = url
            .host_str()
            .ok_or(PackageResolutionError::UnapprovedArtifactLocation)?;
        let client = reqwest::Client::builder()
            .https_only(true)
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(120))
            .user_agent("commonkit-target-helper/1")
            .resolve(host, address)
            .build()
            .map_err(|_| PackageResolutionError::FetchUnavailable)?;
        let locator = locator.to_owned();
        self.runtime.block_on(async move {
            let url = reqwest::Url::parse(&locator)
                .map_err(|_| PackageResolutionError::UnapprovedArtifactLocation)?;
            let response = client
                .get(url.clone())
                .send()
                .await
                .map_err(|_| PackageResolutionError::FetchUnavailable)?;
            if response.status().is_redirection() {
                let location = response
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .ok_or(PackageResolutionError::UnvalidatedRedirect)?;
                let location = url
                    .join(location)
                    .map_err(|_| PackageResolutionError::UnvalidatedRedirect)?;
                validate_package_fetch_url(&location)
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
            Ok(PackageFetchHopV1::Complete(crate::PackageFetchResultV1 {
                bytes,
            }))
        })
    }
}

impl PackageFetch for TargetPackageFetch {
    fn preflight_locators(&mut self, locators: &[String]) -> Result<(), PackageResolutionError> {
        for locator in locators {
            let url = reqwest::Url::parse(locator)
                .map_err(|_| PackageResolutionError::UnapprovedArtifactLocation)?;
            self.pin_url(&url)?;
        }
        Ok(())
    }

    fn fetch_hop(
        &mut self,
        request: &PackageFetchRequestV1,
        locator: &str,
    ) -> Result<PackageFetchHopV1, PackageResolutionError> {
        if request.size > MAX_RESOLUTION_ARTIFACT_BYTES {
            return Err(PackageResolutionError::CorruptArtifact);
        }
        self.fetch_one(locator, request.size)
    }

    fn fetch_discovery_hop(
        &mut self,
        request: &PackageDiscoveryFetchRequestV1,
        locator: &str,
    ) -> Result<PackageFetchHopV1, PackageResolutionError> {
        if request.maximum_bytes > MAX_RESOLUTION_ARTIFACT_BYTES {
            return Err(PackageResolutionError::CorruptArtifact);
        }
        self.fetch_one(locator, request.maximum_bytes)
    }
}

fn validate_package_fetch_url(url: &reqwest::Url) -> Result<(), PackageResolutionError> {
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(PackageResolutionError::UnapprovedArtifactLocation);
    }
    Ok(())
}

fn validated_package_fetch_addresses(
    url: &reqwest::Url,
) -> Result<Vec<SocketAddr>, PackageResolutionError> {
    let host = url
        .host_str()
        .ok_or(PackageResolutionError::UnapprovedArtifactLocation)?;
    let port = url
        .port_or_known_default()
        .ok_or(PackageResolutionError::UnapprovedArtifactLocation)?;
    let addresses = if let Ok(address) = host.parse::<IpAddr>() {
        vec![SocketAddr::new(address, port)]
    } else {
        (host, port)
            .to_socket_addrs()
            .map_err(|_| PackageResolutionError::UnapprovedArtifactLocation)?
            .collect()
    };
    if addresses.is_empty()
        || addresses
            .iter()
            .any(|address| unsafe_destination(address.ip()))
    {
        return Err(PackageResolutionError::UnapprovedArtifactLocation);
    }
    Ok(addresses)
}

fn unsafe_destination(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => {
            address.is_loopback()
                || address.is_private()
                || address.is_link_local()
                || address.is_unspecified()
                || address.is_broadcast()
                || address.is_multicast()
                || address.octets()[0] >= 240
                || address.octets()[0] == 0
                || (address.octets()[0] == 100 && (64..=127).contains(&address.octets()[1]))
                || (address.octets()[0] == 192 && address.octets()[1] == 0)
                || (address.octets()[0] == 198 && address.octets()[1] == 18)
                || (address.octets()[0] == 198 && address.octets()[1] == 19)
                || (address.octets()[0] == 198
                    && address.octets()[1] == 51
                    && address.octets()[2] == 100)
                || (address.octets()[0] == 203
                    && address.octets()[1] == 0
                    && address.octets()[2] == 113)
        }
        IpAddr::V6(address) => {
            let segments = address.segments();
            address
                .to_ipv4_mapped()
                .is_some_and(|mapped| unsafe_destination(IpAddr::V4(mapped)))
                || address.is_loopback()
                || address.is_unspecified()
                || address.is_multicast()
                || (segments[0] & 0xfe00) == 0xfc00
                || (segments[0] & 0xffc0) == 0xfe80
        }
    }
}
