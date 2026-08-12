use std::collections::BTreeMap;
#[cfg(unix)]
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::Duration;

use cap_std::fs::Dir;
#[cfg(unix)]
use cap_std::fs::OpenOptions;
use commonkit_contracts::{PackageManager, StableId};
use commonkit_core::{RootAccess, TargetRoot};
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
#[cfg(unix)]
use std::os::fd::AsRawFd;

use crate::target::TargetDirectoryIdentity;
#[cfg(unix)]
use crate::{
    ARTIFACT_CHUNK_SIZE, ContentReference, MAX_ARTIFACT_TRANSFER_BYTES,
    artifact_chunk_response_digest,
};
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
    roots: BTreeMap<StableId, (LocalTargetFilesystem, PathBuf, RootAccess)>,
    artifacts: ArtifactStore,
    receipts: PathBuf,
    #[cfg(unix)]
    staging: std::sync::Mutex<Dir>,
    package_resolution: Option<TargetPackageResolutionConfig>,
    protected_paths: Vec<PathBuf>,
    engram_executable: Option<PathBuf>,
}

struct ValidatedRoot {
    id: StableId,
    path: PathBuf,
    identity: TargetDirectoryIdentity,
    access: RootAccess,
}

impl TargetHelper {
    pub fn with_engram_executable(
        mut self,
        executable: Option<PathBuf>,
    ) -> Result<Self, TargetFilesystemError> {
        if let Some(path) = &executable {
            let metadata = std::fs::symlink_metadata(path)?;
            if !path.is_absolute() || metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(TargetFilesystemError::InvalidEngramExecutable);
            }
        }
        self.engram_executable = executable;
        Ok(self)
    }

    pub fn open(roots: Vec<TargetRoot>, state_root: &Path) -> Result<Self, TargetFilesystemError> {
        Self::open_with_package_resolution(roots, state_root, None)
    }

    pub fn open_with_package_resolution(
        roots: Vec<TargetRoot>,
        state_root: &Path,
        package_resolution: Option<TargetPackageResolutionConfig>,
    ) -> Result<Self, TargetFilesystemError> {
        Self::open_with_package_resolution_and_protected_paths(
            roots,
            state_root,
            package_resolution,
            Vec::new(),
        )
    }

    pub fn open_with_package_resolution_and_protected_paths(
        roots: Vec<TargetRoot>,
        state_root: &Path,
        package_resolution: Option<TargetPackageResolutionConfig>,
        protected_paths: Vec<PathBuf>,
    ) -> Result<Self, TargetFilesystemError> {
        let validated_roots =
            Self::validate_configuration_internal(&roots, state_root, &protected_paths)?;
        let mut mapped = BTreeMap::new();
        for root in validated_roots {
            let filesystem =
                LocalTargetFilesystem::open_nofollow(&root.path, root.access, root.identity)?;
            if mapped
                .insert(root.id, (filesystem, root.path, root.access))
                .is_some()
            {
                return Err(TargetFilesystemError::InvalidSshConfig("duplicate root id"));
            }
        }
        let artifacts = ArtifactStore::open(state_root.join("artifacts"))
            .map_err(|_| TargetFilesystemError::RemoteArtifact)?;
        let receipts = state_root.join("receipts");
        std::fs::create_dir_all(&receipts)?;
        let staging_store = ArtifactStore::open(state_root.join("artifact-staging"))
            .map_err(|_| TargetFilesystemError::RemoteArtifact)?;
        let staging = staging_store
            .clone_directory()
            .map_err(|_| TargetFilesystemError::RemoteArtifact)?;
        validate_staging_directory(&staging).map_err(|_| TargetFilesystemError::RemoteArtifact)?;
        let _lock =
            acquire_staging_lock(&staging).map_err(|_| TargetFilesystemError::RemoteArtifact)?;
        cleanup_staging(&staging).map_err(|_| TargetFilesystemError::RemoteArtifact)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&receipts, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(Self {
            roots: mapped,
            artifacts,
            receipts,
            #[cfg(unix)]
            staging: std::sync::Mutex::new(staging),
            package_resolution,
            protected_paths,
            engram_executable: None,
        })
    }

    pub fn validate_configuration(
        roots: &[TargetRoot],
        state_root: &Path,
        protected_paths: &[PathBuf],
    ) -> Result<(), TargetFilesystemError> {
        Self::validate_configuration_internal(roots, state_root, protected_paths).map(|_| ())
    }

    fn validate_configuration_internal(
        roots: &[TargetRoot],
        state_root: &Path,
        protected_paths: &[PathBuf],
    ) -> Result<Vec<ValidatedRoot>, TargetFilesystemError> {
        #[cfg(windows)]
        {
            let _ = (roots, state_root, protected_paths);
            return Err(TargetFilesystemError::InvalidSshConfig(
                "target helper filesystem roots are unsupported on Windows",
            ));
        }
        let state_identity = validate_secure_path(state_root, true, true)?;
        if std::fs::symlink_metadata(state_root).is_ok() && !owned_by_effective_user(state_root) {
            return Err(TargetFilesystemError::InvalidSshConfig(
                "state root has an unexpected owner",
            ));
        }
        let protected_identities = protected_paths
            .iter()
            .map(|path| {
                let allow_missing = path == state_root;
                validate_secure_path(path, false, allow_missing)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut ids = BTreeMap::new();
        let mut root_identities = Vec::with_capacity(roots.len());
        for root in roots {
            let path = PathBuf::from(&root.path);
            let validated = validate_secure_path_with_identity(&path, true, false)
                .map_err(|_| TargetFilesystemError::InvalidRoot)?;
            let identity = validated
                .directory_identity
                .ok_or(TargetFilesystemError::InvalidRoot)?;
            if root.access == RootAccess::ReadWrite && !owned_by_effective_user(&path) {
                return Err(TargetFilesystemError::InvalidRoot);
            }
            if ids.insert(root.id.clone(), ()).is_some() {
                return Err(TargetFilesystemError::InvalidSshConfig("duplicate root id"));
            }
            root_identities.push((path, validated.path, identity, root.access));
        }
        for (path, canonical_path, _, access) in &root_identities {
            if *access == RootAccess::ReadWrite
                && (protected_paths
                    .iter()
                    .any(|protected| path.starts_with(protected) || protected.starts_with(path))
                    || protected_identities.iter().any(|protected| {
                        canonical_path.starts_with(protected)
                            || protected.starts_with(canonical_path)
                    })
                    || path.starts_with(state_root)
                    || state_root.starts_with(path)
                    || canonical_path.starts_with(&state_identity)
                    || state_identity.starts_with(canonical_path))
            {
                return Err(TargetFilesystemError::InvalidSshConfig(
                    "writable root overlaps helper control path",
                ));
            }
        }
        Ok(root_identities
            .into_iter()
            .zip(roots.iter())
            .map(|((_, path, identity, access), root)| ValidatedRoot {
                id: root.id.clone(),
                path,
                identity,
                access,
            })
            .collect())
    }

    pub fn validate_path(
        path: &Path,
        expected_directory: bool,
        allow_missing_leaf: bool,
    ) -> Result<PathBuf, TargetFilesystemError> {
        validate_secure_path(path, expected_directory, allow_missing_leaf)
    }

    fn root(&self, id: &StableId) -> Result<&LocalTargetFilesystem, TargetFilesystemError> {
        let (filesystem, _, _) = self
            .roots
            .get(id)
            .ok_or_else(|| TargetFilesystemError::UnknownRoot(id.clone()))?;
        Ok(filesystem)
    }

    fn ensure_unprotected(
        &self,
        root_id: &StableId,
        path: &crate::NormalizedManagedPath,
    ) -> Result<(), TargetFilesystemError> {
        let (_, root, _) = self
            .roots
            .get(root_id)
            .ok_or_else(|| TargetFilesystemError::UnknownRoot(root_id.clone()))?;
        let absolute = root.join(path.as_str());
        if self
            .protected_paths
            .iter()
            .any(|protected| absolute.starts_with(protected) || protected.starts_with(&absolute))
        {
            return Err(TargetFilesystemError::InvalidSshConfig(
                "managed path overlaps helper control path",
            ));
        }
        Ok(())
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
            if reference.bytes > MAX_RESOLUTION_ARTIFACT_BYTES {
                return Err(TargetFilesystemError::RemoteArtifact);
            }
            total_artifact_bytes = total_artifact_bytes
                .checked_add(reference.bytes)
                .ok_or(TargetFilesystemError::RemoteArtifact)?;
            if total_artifact_bytes > MAX_RESOLUTION_ARTIFACT_BYTES {
                return Err(TargetFilesystemError::RemoteArtifact);
            }
            artifacts.push(PackageMutationArtifact { reference });
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

    #[cfg(unix)]
    fn transfer_paths(
        transfer_id: &StableId,
        digest: &commonkit_contracts::Sha256Digest,
    ) -> (PathBuf, PathBuf) {
        let stem = format!(
            "{}-{}",
            transfer_id.as_str(),
            digest.as_str().trim_start_matches("sha256:")
        );
        (
            PathBuf::from(format!("{stem}.part")),
            PathBuf::from(format!("{stem}.meta")),
        )
    }

    #[cfg(unix)]
    fn validate_chunk(
        byte_count: u64,
        chunk_size: u32,
        sequence: u32,
        offset: u64,
        total_chunks: u32,
        content_len: usize,
    ) -> Result<(), TargetFilesystemError> {
        Self::validate_chunk_shape(byte_count, chunk_size, sequence, offset, total_chunks)?;
        if content_len == 0 && byte_count != 0
            || content_len as u64 > u64::from(chunk_size)
            || offset
                .checked_add(content_len as u64)
                .is_none_or(|end| end > byte_count)
            || (sequence + 1 < total_chunks && content_len as u32 != chunk_size)
            || (sequence + 1 == total_chunks && content_len as u64 != byte_count - offset)
        {
            return Err(TargetFilesystemError::RemoteArtifact);
        }
        Ok(())
    }

    #[cfg(unix)]
    fn validate_chunk_shape(
        byte_count: u64,
        chunk_size: u32,
        sequence: u32,
        offset: u64,
        total_chunks: u32,
    ) -> Result<(), TargetFilesystemError> {
        if byte_count > MAX_ARTIFACT_TRANSFER_BYTES
            || chunk_size != ARTIFACT_CHUNK_SIZE
            || total_chunks == 0
            || total_chunks as u64 != byte_count.div_ceil(u64::from(chunk_size))
            || sequence >= total_chunks
            || offset > byte_count
            || offset != u64::from(sequence) * u64::from(chunk_size)
        {
            return Err(TargetFilesystemError::RemoteArtifact);
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    #[cfg(unix)]
    fn stage_artifact_chunk(
        &self,
        run_id: StableId,
        transfer_id: StableId,
        digest: commonkit_contracts::Sha256Digest,
        byte_count: u64,
        chunk_size: u32,
        sequence: u32,
        offset: u64,
        total_chunks: u32,
        content: Vec<u8>,
    ) -> Result<SshFilesystemResponse, TargetFilesystemError> {
        Self::validate_chunk(
            byte_count,
            chunk_size,
            sequence,
            offset,
            total_chunks,
            content.len(),
        )?;
        let staging = self
            .staging
            .lock()
            .map_err(|_| TargetFilesystemError::RemoteArtifact)?;
        let _lock =
            acquire_staging_lock(&staging).map_err(|_| TargetFilesystemError::RemoteArtifact)?;
        let (part, meta) = Self::transfer_paths(&transfer_id, &digest);
        let mut part_exists = match staging.symlink_metadata(&part) {
            Ok(metadata) if metadata.is_file() => true,
            Ok(_) => return Err(TargetFilesystemError::RemoteArtifact),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(_) => return Err(TargetFilesystemError::RemoteArtifact),
        };
        let reference = ContentReference {
            digest: digest.clone(),
            bytes: byte_count,
            sensitivity: ContentSensitivity::Portable,
        };
        if sequence + 1 == total_chunks
            && !part_exists
            && self.artifacts.verify_reference(&reference).is_ok()
        {
            let response_digest = artifact_chunk_response_digest(
                &run_id,
                &transfer_id,
                &digest,
                byte_count,
                chunk_size,
                sequence,
                offset,
                total_chunks,
                &content,
            )
            .map_err(|_| TargetFilesystemError::RemoteArtifact)?;
            return Ok(SshFilesystemResponse::ArtifactChunkStaged {
                run_id,
                transfer_id,
                digest,
                byte_count,
                chunk_size,
                sequence,
                offset,
                total_chunks,
                response_digest,
            });
        }
        let identity = format!("{digest}|{byte_count}|{chunk_size}|{total_chunks}");
        let metadata_matches = if part_exists {
            let mut meta_options = staging_open_options();
            meta_options.read(true);
            match staging.open_with(&meta, &meta_options) {
                Ok(meta_file) => {
                    let mut metadata_bytes = Vec::new();
                    meta_file.take(4097).read_to_end(&mut metadata_bytes)?;
                    metadata_bytes.starts_with(format!("{identity}|").as_bytes())
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
                Err(_) => return Err(TargetFilesystemError::RemoteArtifact),
            }
        } else {
            false
        };
        // A sequence-0 retry is the only operation allowed to recover an
        // interrupted initialization. Reset both halves under the transfer
        // lock so a stale/orphaned pair cannot consume the aggregate cap or
        // be mixed with a new transfer identity.
        if sequence == 0 && offset == 0 && (!part_exists || !metadata_matches) {
            if part_exists {
                staging.remove_file(&part)?;
            }
            let _ = staging.remove_file(&meta);
            part_exists = false;
        }
        let (part_count, staged_bytes) =
            staging_usage(&staging).map_err(|_| TargetFilesystemError::RemoteArtifact)?;
        let current_len = staging
            .symlink_metadata(&part)
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        if !part_exists && part_count >= STAGING_MAX_PARTS {
            return Err(TargetFilesystemError::RemoteArtifact);
        }
        if current_len == offset
            && staged_bytes
                .checked_add(content.len() as u64)
                .is_none_or(|total| total > STAGING_MAX_BYTES)
        {
            return Err(TargetFilesystemError::RemoteArtifact);
        }
        if sequence == 0 && offset == 0 && !part_exists {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use cap_std::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = staging.open_with(&part, &options)?;
            file.write_all(&content)?;
            file.sync_all()?;
            let mut meta_options = OpenOptions::new();
            meta_options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use cap_std::fs::OpenOptionsExt;
                meta_options.mode(0o600);
            }
            let mut meta_file = staging.open_with(&meta, &meta_options)?;
            meta_file.write_all(identity.as_bytes())?;
            meta_file.sync_all()?;
        } else {
            if !part_exists {
                return Err(TargetFilesystemError::RemoteArtifact);
            }
            if !metadata_matches {
                return Err(TargetFilesystemError::RemoteArtifact);
            }
            let mut part_options = staging_open_options();
            part_options.read(true).write(true);
            let mut file = staging.open_with(&part, &part_options)?;
            let current = file.metadata()?.len();
            if current < offset {
                return Err(TargetFilesystemError::RemoteArtifact);
            }
            if current == offset {
                file.seek(SeekFrom::Start(offset))?;
                file.write_all(&content)?;
                file.sync_all()?;
            } else {
                let mut existing = vec![0; content.len()];
                file.seek(SeekFrom::Start(offset))?;
                file.read_exact(&mut existing)?;
                if existing != content {
                    return Err(TargetFilesystemError::RemoteArtifact);
                }
            }
        }
        let mut touch_options = staging_open_options();
        touch_options.write(true).truncate(true);
        let mut meta_file = staging.open_with(&meta, &touch_options)?;
        meta_file.write_all(format!("{identity}|{sequence}").as_bytes())?;
        meta_file.sync_all()?;
        if sequence + 1 == total_chunks {
            let mut part_options = staging_open_options();
            part_options.read(true);
            let mut part_file = staging.open_with(&part, &part_options)?;
            self.artifacts
                .put_file_handle(&mut part_file, &reference)
                .map_err(|_| TargetFilesystemError::RemoteArtifact)?;
            let _ = staging.remove_file(&part);
            let _ = staging.remove_file(&meta);
        }
        let response_digest = artifact_chunk_response_digest(
            &run_id,
            &transfer_id,
            &digest,
            byte_count,
            chunk_size,
            sequence,
            offset,
            total_chunks,
            &content,
        )
        .map_err(|_| TargetFilesystemError::RemoteArtifact)?;
        Ok(SshFilesystemResponse::ArtifactChunkStaged {
            run_id,
            transfer_id,
            digest,
            byte_count,
            chunk_size,
            sequence,
            offset,
            total_chunks,
            response_digest,
        })
    }

    #[allow(clippy::too_many_arguments)]
    #[cfg(unix)]
    fn read_artifact_chunk(
        &self,
        request_id: StableId,
        transfer_id: StableId,
        digest: commonkit_contracts::Sha256Digest,
        byte_count: u64,
        chunk_size: u32,
        sequence: u32,
        offset: u64,
        total_chunks: u32,
    ) -> Result<SshFilesystemResponse, TargetFilesystemError> {
        Self::validate_chunk_shape(byte_count, chunk_size, sequence, offset, total_chunks)?;
        let expected = usize::try_from((byte_count - offset).min(u64::from(chunk_size)))
            .map_err(|_| TargetFilesystemError::RemoteArtifact)?;
        Self::validate_chunk(
            byte_count,
            chunk_size,
            sequence,
            offset,
            total_chunks,
            expected,
        )?;
        let reference = ContentReference {
            digest: digest.clone(),
            bytes: byte_count,
            sensitivity: ContentSensitivity::Portable,
        };
        let content = self
            .artifacts
            .read_chunk(&reference, offset, chunk_size)
            .map_err(|_| TargetFilesystemError::RemoteArtifact)?;
        Self::validate_chunk(
            byte_count,
            chunk_size,
            sequence,
            offset,
            total_chunks,
            content.len(),
        )?;
        let response_digest = artifact_chunk_response_digest(
            &request_id,
            &transfer_id,
            &digest,
            byte_count,
            chunk_size,
            sequence,
            offset,
            total_chunks,
            &content,
        )
        .map_err(|_| TargetFilesystemError::RemoteArtifact)?;
        Ok(SshFilesystemResponse::ArtifactChunk {
            request_id,
            transfer_id,
            digest,
            byte_count,
            chunk_size,
            sequence,
            offset,
            total_chunks,
            content,
            response_digest,
        })
    }

    pub fn dispatch(
        &self,
        request: SshFilesystemRequest,
    ) -> Result<SshFilesystemResponse, TargetFilesystemError> {
        match &request {
            SshFilesystemRequest::ListDirectory { root_id, path }
            | SshFilesystemRequest::ReadFile { root_id, path }
            | SshFilesystemRequest::InspectResource { root_id, path }
            | SshFilesystemRequest::WriteFile { root_id, path, .. }
            | SshFilesystemRequest::WriteDirectory { root_id, path, .. }
            | SshFilesystemRequest::WriteSymlink { root_id, path, .. }
            | SshFilesystemRequest::Remove { root_id, path } => {
                self.ensure_unprotected(root_id, path)?;
            }
            _ => {}
        }
        match request {
            SshFilesystemRequest::ListDirectory { root_id, path } => {
                Ok(SshFilesystemResponse::Directory {
                    entries: self.root(&root_id)?.list_directory(&path)?,
                })
            }
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
            SshFilesystemRequest::EngramSync {
                root_id,
                project_id,
                project_path,
                mode,
            } => {
                let executable = self
                    .engram_executable
                    .as_ref()
                    .ok_or(TargetFilesystemError::EngramExecutableUnavailable)?;
                let (_, root_path, _) = self
                    .roots
                    .get(&root_id)
                    .ok_or_else(|| TargetFilesystemError::UnknownRoot(root_id.clone()))?;
                let mut project = root_path.clone();
                for component in project_path.as_str().split('/') {
                    project.push(component);
                    let metadata = std::fs::symlink_metadata(&project)?;
                    if metadata.file_type().is_symlink() {
                        return Err(TargetFilesystemError::SymlinkEncountered(
                            project_path.to_string(),
                        ));
                    }
                    if !metadata.is_dir() {
                        return Err(TargetFilesystemError::NotDirectory(
                            project_path.to_string(),
                        ));
                    }
                }
                let mut command = std::process::Command::new(executable);
                command
                    .arg("sync")
                    .arg("--project")
                    .arg(project_id.as_str())
                    .current_dir(project)
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null());
                if mode == crate::EngramTargetSyncMode::Import {
                    command.arg("--import");
                }
                if !command.status()?.success() {
                    return Err(TargetFilesystemError::EngramCommandFailed);
                }
                Ok(SshFilesystemResponse::EngramSynced { mode })
            }
            SshFilesystemRequest::PackageMutation {
                root_id,
                phase,
                resolution,
                artifacts,
                target_identity_digest,
            } => {
                let config = self
                    .package_resolution
                    .as_ref()
                    .ok_or(TargetFilesystemError::PackageResolutionRejected)?;
                if target_identity_digest.as_ref() != Some(&config.target_identity_digest) {
                    return Err(TargetFilesystemError::PackageResolutionRejected);
                }
                let registry = target_package_source_registry(config)?;
                let authority = crate::PackageResolutionAuthority::new(
                    &config.target,
                    &config.manager,
                    &registry,
                    &config.policy,
                )
                .map_err(|_| TargetFilesystemError::PackageResolutionRejected)?;
                authority
                    .validate_resolution(&resolution)
                    .map_err(|_| TargetFilesystemError::PackageResolutionRejected)?;
                let expected_artifacts = resolution
                    .artifacts
                    .iter()
                    .map(|artifact| artifact.content.clone())
                    .collect::<Vec<_>>();
                let received_artifacts = artifacts
                    .iter()
                    .map(|artifact| artifact.reference.clone())
                    .collect::<Vec<_>>();
                if (phase == PackageMutationPhase::Observe && !received_artifacts.is_empty())
                    || (phase != PackageMutationPhase::Observe
                        && expected_artifacts != received_artifacts)
                {
                    return Err(TargetFilesystemError::RemoteArtifact);
                }
                let (filesystem, root_path, access) = self
                    .roots
                    .get(&root_id)
                    .ok_or_else(|| TargetFilesystemError::UnknownRoot(root_id.clone()))?;
                if phase != PackageMutationPhase::Observe && access != &RootAccess::ReadWrite {
                    return Err(TargetFilesystemError::ReadOnly);
                }
                for artifact in artifacts {
                    self.artifacts
                        .verify_digest(&artifact.reference.digest)
                        .map_err(|_| TargetFilesystemError::RemoteArtifact)?;
                    let metadata = self
                        .artifacts
                        .read_chunk(&artifact.reference, 0, 1)
                        .map_err(|_| TargetFilesystemError::RemoteArtifact)?;
                    if artifact.reference.bytes > MAX_RESOLUTION_ARTIFACT_BYTES
                        || metadata.is_empty() && artifact.reference.bytes != 0
                    {
                        return Err(TargetFilesystemError::RemoteArtifact);
                    }
                }
                let root_handle = filesystem
                    .clone_root_handle()
                    .map_err(|_| TargetFilesystemError::PackageCommandFailed)?;
                let mut backend =
                    ProcessOfflinePackageBackend::new_with_bound_root(root_path, root_handle)
                        .map_err(|_| TargetFilesystemError::PackageCommandFailed)?
                        .with_target_package_resolution(
                            self.package_resolution
                                .clone()
                                .ok_or(TargetFilesystemError::PackageCommandFailed)?,
                        );
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
            #[cfg(not(unix))]
            SshFilesystemRequest::StageArtifactChunk { .. }
            | SshFilesystemRequest::ReadArtifactChunk { .. } => {
                // Apt/NVM package resolution and mutation are Unix-only. Do
                // not create or touch the staging store on platforms without
                // the descriptor/lock guarantees used by this protocol.
                Err(TargetFilesystemError::PackageResolutionUnavailable)
            }
            #[cfg(unix)]
            SshFilesystemRequest::StageArtifactChunk {
                run_id,
                transfer_id,
                digest,
                byte_count,
                chunk_size,
                sequence,
                offset,
                total_chunks,
                content,
            } => self.stage_artifact_chunk(
                run_id,
                transfer_id,
                digest,
                byte_count,
                chunk_size,
                sequence,
                offset,
                total_chunks,
                content,
            ),
            #[cfg(unix)]
            SshFilesystemRequest::ReadArtifactChunk {
                request_id,
                transfer_id,
                digest,
                byte_count,
                chunk_size,
                sequence,
                offset,
                total_chunks,
            } => self.read_artifact_chunk(
                request_id,
                transfer_id,
                digest,
                byte_count,
                chunk_size,
                sequence,
                offset,
                total_chunks,
            ),
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

struct SecurePathValidation {
    path: PathBuf,
    directory_identity: Option<TargetDirectoryIdentity>,
}

fn validate_secure_path(
    path: &Path,
    expected_directory: bool,
    allow_missing_leaf: bool,
) -> Result<PathBuf, TargetFilesystemError> {
    Ok(validate_secure_path_with_identity(path, expected_directory, allow_missing_leaf)?.path)
}

fn validate_secure_path_with_identity(
    path: &Path,
    expected_directory: bool,
    allow_missing_leaf: bool,
) -> Result<SecurePathValidation, TargetFilesystemError> {
    if !path.is_absolute() {
        return Err(TargetFilesystemError::InvalidSshConfig(
            "target helper paths must be absolute",
        ));
    }
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                let Some((identity, directory_identity)) =
                    allowed_platform_alias(path, expected_directory)?
                else {
                    return Err(TargetFilesystemError::InvalidSshConfig(
                        "target helper path has an unsafe type",
                    ));
                };
                return Ok(SecurePathValidation {
                    path: identity,
                    directory_identity: Some(directory_identity),
                });
            }
            if (expected_directory && !metadata.is_dir())
                || (!expected_directory && !metadata.is_file() && !metadata.is_dir())
            {
                return Err(TargetFilesystemError::InvalidSshConfig(
                    "target helper path has an unsafe type",
                ));
            }
            validate_secure_metadata(&metadata, false)?;
            validate_secure_ancestors(path.parent())?;
            Ok(SecurePathValidation {
                path: std::fs::canonicalize(path).map_err(|_| {
                    TargetFilesystemError::InvalidSshConfig("cannot canonicalize path")
                })?,
                directory_identity: metadata
                    .is_dir()
                    .then(|| TargetDirectoryIdentity::from_metadata(&metadata)),
            })
        }
        Err(error) if allow_missing_leaf && error.kind() == std::io::ErrorKind::NotFound => {
            let parent = path
                .parent()
                .ok_or(TargetFilesystemError::InvalidSshConfig(
                    "target helper path has no parent",
                ))?;
            validate_secure_ancestors(Some(parent))?;
            Ok(SecurePathValidation {
                path: canonicalize_missing(path)?,
                directory_identity: None,
            })
        }
        Err(error) => Err(TargetFilesystemError::Io(error)),
    }
}

fn validate_secure_ancestors(start: Option<&Path>) -> Result<(), TargetFilesystemError> {
    let mut current = start.ok_or(TargetFilesystemError::InvalidSshConfig(
        "target helper path has no parent",
    ))?;
    loop {
        let metadata = std::fs::symlink_metadata(current)?;
        if metadata.file_type().is_symlink() {
            if allowed_platform_alias(current, true)?.is_none() {
                return Err(TargetFilesystemError::InvalidSshConfig(
                    "target helper path ancestor is unsafe",
                ));
            }
            let parent = current.parent();
            validate_secure_ancestors(parent)?;
            return Ok(());
        }
        if !metadata.is_dir() {
            return Err(TargetFilesystemError::InvalidSshConfig(
                "target helper path ancestor is unsafe",
            ));
        }
        validate_secure_metadata(&metadata, true)?;
        let Some(parent) = current.parent() else {
            return Ok(());
        };
        if parent == current {
            return Ok(());
        }
        current = parent;
    }
}

fn allowed_platform_alias(
    path: &Path,
    expected_directory: bool,
) -> Result<Option<(PathBuf, TargetDirectoryIdentity)>, TargetFilesystemError> {
    if !expected_directory {
        return Ok(None);
    }
    let expected = match path {
        p if p == Path::new("/tmp") => Path::new("/private/tmp"),
        p if p == Path::new("/var") => Path::new("/private/var"),
        _ => return Ok(None),
    };
    let identity = std::fs::canonicalize(path)
        .map_err(|_| TargetFilesystemError::InvalidSshConfig("cannot canonicalize path"))?;
    if identity != expected {
        return Ok(None);
    }
    let metadata = std::fs::symlink_metadata(&identity)?;
    if !metadata.is_dir() {
        return Ok(None);
    }
    validate_secure_metadata(&metadata, true)?;
    validate_secure_ancestors(identity.parent())?;
    Ok(Some((
        identity,
        TargetDirectoryIdentity::from_metadata(&metadata),
    )))
}

fn validate_secure_metadata(
    metadata: &std::fs::Metadata,
    allow_sticky_ancestor: bool,
) -> Result<(), TargetFilesystemError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let mode = metadata.permissions().mode();
        let sticky_root = allow_sticky_ancestor && mode & 0o1000 != 0 && metadata.uid() == 0;
        if (!sticky_root && mode & 0o022 != 0) || !owner_allowed(metadata.uid()) {
            return Err(TargetFilesystemError::InvalidSshConfig(
                "target helper path has unsafe ownership or mode",
            ));
        }
    }
    Ok(())
}

fn canonicalize_missing(path: &Path) -> Result<PathBuf, TargetFilesystemError> {
    if std::fs::symlink_metadata(path).is_ok() {
        return std::fs::canonicalize(path)
            .map_err(|_| TargetFilesystemError::InvalidSshConfig("cannot canonicalize path"));
    }
    let parent = path
        .parent()
        .ok_or(TargetFilesystemError::InvalidSshConfig(
            "target helper path has no parent",
        ))?;
    let name = path
        .file_name()
        .ok_or(TargetFilesystemError::InvalidSshConfig(
            "target helper path has no name",
        ))?;
    Ok(canonicalize_missing(parent)?.join(name))
}

fn owner_allowed(uid: u32) -> bool {
    #[cfg(unix)]
    {
        let effective = unsafe { libc::geteuid() };
        uid == effective || uid == 0
    }
    #[cfg(not(unix))]
    {
        let _ = uid;
        true
    }
}

fn owned_by_effective_user(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        std::fs::symlink_metadata(path)
            .map(|metadata| metadata.uid() == unsafe { libc::geteuid() })
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        true
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

#[cfg(unix)]
const STAGING_MAX_PARTS: usize = 128;
#[cfg(unix)]
const STAGING_MAX_BYTES: u64 = MAX_ARTIFACT_TRANSFER_BYTES;
const STAGING_STALE_AFTER_SECS: u64 = 24 * 60 * 60;

#[cfg(unix)]
struct StagingLock {
    file: cap_std::fs::File,
}

#[cfg(not(unix))]
struct StagingLock;

#[cfg(unix)]
fn acquire_staging_lock(staging: &Dir) -> Result<StagingLock, std::io::Error> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW).mode(0o600);
    }
    let file = staging.open_with(".lock", &options)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "unsafe staging lock",
        ));
    }
    #[cfg(unix)]
    {
        use cap_std::fs::{MetadataExt, PermissionsExt};
        if metadata.permissions().mode() & 0o777 != 0o600
            || metadata.uid() != unsafe { libc::geteuid() }
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "unsafe staging lock ownership",
            ));
        }
    }
    #[cfg(unix)]
    {
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(StagingLock { file })
}

#[cfg(not(unix))]
fn acquire_staging_lock(_staging: &Dir) -> Result<StagingLock, std::io::Error> {
    Ok(StagingLock)
}

#[cfg(unix)]
impl Drop for StagingLock {
    fn drop(&mut self) {
        #[cfg(unix)]
        unsafe {
            libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

#[cfg(unix)]
fn staging_open_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    {
        use cap_std::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    options
}

fn cleanup_staging(staging: &Dir) -> Result<(), std::io::Error> {
    let now = cap_std::time::SystemClock::new(cap_std::ambient_authority()).now();
    let mut entries = Vec::new();
    for entry in staging.read_dir(".")? {
        entries.push(entry?);
    }
    for entry in &entries {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.ends_with(".part") {
            continue;
        }
        let metadata = staging.symlink_metadata(&*name)?;
        if !metadata.is_file() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "unsafe staging entry",
            ));
        }
        let meta_name = format!("{}.meta", name.trim_end_matches(".part"));
        let liveness = match staging.symlink_metadata(&meta_name) {
            Ok(metadata) if metadata.is_file() => Some(metadata),
            Ok(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "unsafe staging entry",
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        let stale = liveness
            .as_ref()
            .or(Some(&metadata))
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age.as_secs() > STAGING_STALE_AFTER_SECS);
        if stale {
            entry.remove_file()?;
            let _ = staging.remove_file(meta_name);
        }
    }
    for entry in entries {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.ends_with(".meta") {
            continue;
        }
        let part = format!("{}.part", name.trim_end_matches(".meta"));
        match staging.symlink_metadata(&part) {
            Ok(metadata) if metadata.is_file() => continue,
            Ok(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "unsafe staging entry",
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let metadata = staging.symlink_metadata(&*name)?;
        if !metadata.is_file() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "unsafe staging entry",
            ));
        }
        let stale = metadata
            .modified()
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age.as_secs() > STAGING_STALE_AFTER_SECS);
        if stale {
            entry.remove_file()?;
        }
    }
    Ok(())
}

fn validate_staging_directory(staging: &Dir) -> Result<(), std::io::Error> {
    let metadata = staging.dir_metadata()?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "unsafe staging root",
        ));
    }
    #[cfg(unix)]
    {
        use cap_std::fs::{MetadataExt, PermissionsExt};
        let uid = unsafe { libc::geteuid() };
        if metadata.permissions().mode() & 0o777 != 0o700 || metadata.uid() != uid {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "unsafe staging root ownership",
            ));
        }
    }
    Ok(())
}

#[cfg(unix)]
fn staging_usage(staging: &Dir) -> Result<(usize, u64), std::io::Error> {
    let mut count = 0;
    let mut bytes: u64 = 0;
    for entry in staging.read_dir(".")? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.ends_with(".part") {
            continue;
        }
        let metadata = staging.symlink_metadata(&*name)?;
        if !metadata.is_file() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "unsafe staging entry",
            ));
        }
        count += 1;
        bytes = bytes
            .checked_add(metadata.len())
            .ok_or_else(|| std::io::Error::other("staging usage overflow"))?;
    }
    Ok((count, bytes))
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
            if apt
                .signing_key_digest
                .as_ref()
                .is_some_and(|expected| expected != &key_digest)
            {
                return Err(TargetFilesystemError::PackageResolutionUnavailable);
            }
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
