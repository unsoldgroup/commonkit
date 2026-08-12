//! Offline, forward-only mutation of controller-resolved package plans.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(unix)]
use cap_std::fs::Dir;
use commonkit_contracts::{
    Operation, OperationKind, PackageExitClassification, PackageManager,
    PackageOperationConsentBinding, PackageReceiptEvidence, PackageSelector, RecoveryCapability,
    ResourceRef, Risk, SecurityPolicy, Sha256Digest, StableId, digest_domain_json,
};
use commonkit_core::{OperationDraft, finalize_operation};
use commonkit_reconcile::{Adapter, AdapterFailure, RecoveryObservation};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    ArtifactStore, NodeOfflineInstallRecipeV1, NodeRuntimeHost, OfflineInstallRecipeV1,
    PackageObservationV1, PackageResolutionAuthority, PackageResolutionV1, PackageResourcePlanner,
    ProviderPlanError, ResolvedPackageIntent, ResourceProvenance,
    apt_resolution::apt_package_identity,
};

const CLOSED_NVM_PATH: &str = "/usr/bin:/bin";
const APT_LIVE_SAFETY_MARKER_PREFIX: &str = "commonkit-apt-live-safety=";

/// The only target-side capability exposed to package mutation.
///
/// Implementations receive a validated, persisted resolution and the artifact
/// store that contains its exact bytes. They never receive a resolver or fetch
/// capability, which makes applying and recovering a durable plan offline.
pub trait PackageMutationBackend: Send {
    fn manager(&self) -> PackageManager;

    fn supports_manager(&self, manager: PackageManager) -> bool {
        self.manager() == manager
    }

    fn supports_target(&self, _resolution: &PackageResolutionV1) -> bool {
        true
    }

    fn observe(
        &mut self,
        resolution: &PackageResolutionV1,
    ) -> Result<PackageObservationV1, PackageMutationError>;

    fn prepare_offline(
        &mut self,
        resolution: &PackageResolutionV1,
        artifacts: &ArtifactStore,
    ) -> Result<(), PackageMutationError>;

    fn apply_offline(
        &mut self,
        resolution: &PackageResolutionV1,
        artifacts: &ArtifactStore,
    ) -> Result<(), PackageMutationError>;

    fn verify_offline(
        &mut self,
        resolution: &PackageResolutionV1,
        artifacts: &ArtifactStore,
    ) -> Result<(), PackageMutationError>;
}

/// Closed dispatch for production package mutation. The adapter has one
/// stable route, while this registry permits only explicitly registered
/// manager backends to handle a persisted resolution.
pub struct PackageMutationBackendRegistry {
    backends: Vec<Box<dyn PackageMutationBackend>>,
}

impl PackageMutationBackendRegistry {
    pub fn new(
        backends: impl IntoIterator<Item = Box<dyn PackageMutationBackend>>,
    ) -> Result<Self, PackageMutationError> {
        let mut registered: Vec<Box<dyn PackageMutationBackend>> = Vec::new();
        for backend in backends {
            if registered.iter().any(|existing| {
                [PackageManager::Apt, PackageManager::Nvm]
                    .iter()
                    .any(|manager| {
                        existing.supports_manager(*manager) && backend.supports_manager(*manager)
                    })
            }) {
                return Err(PackageMutationError::Backend);
            }
            registered.push(backend);
        }
        Ok(Self {
            backends: registered,
        })
    }

    fn backend(
        &mut self,
        manager: PackageManager,
    ) -> Result<&mut Box<dyn PackageMutationBackend>, PackageMutationError> {
        self.backends
            .iter_mut()
            .find(|backend| backend.supports_manager(manager))
            .ok_or(PackageMutationError::UnsupportedRecipe)
    }
}

impl PackageMutationBackend for PackageMutationBackendRegistry {
    fn manager(&self) -> PackageManager {
        PackageManager::Apt
    }

    fn supports_manager(&self, manager: PackageManager) -> bool {
        self.backends
            .iter()
            .any(|backend| backend.supports_manager(manager))
    }

    fn observe(
        &mut self,
        resolution: &PackageResolutionV1,
    ) -> Result<PackageObservationV1, PackageMutationError> {
        self.backend(resolution.manager.manager)?
            .observe(resolution)
    }

    fn prepare_offline(
        &mut self,
        resolution: &PackageResolutionV1,
        artifacts: &ArtifactStore,
    ) -> Result<(), PackageMutationError> {
        self.backend(resolution.manager.manager)?
            .prepare_offline(resolution, artifacts)
    }

    fn apply_offline(
        &mut self,
        resolution: &PackageResolutionV1,
        artifacts: &ArtifactStore,
    ) -> Result<(), PackageMutationError> {
        self.backend(resolution.manager.manager)?
            .apply_offline(resolution, artifacts)
    }

    fn verify_offline(
        &mut self,
        resolution: &PackageResolutionV1,
        artifacts: &ArtifactStore,
    ) -> Result<(), PackageMutationError> {
        self.backend(resolution.manager.manager)?
            .verify_offline(resolution, artifacts)
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum PackageMutationError {
    #[error("package mutation backend failed")]
    Backend,
    #[error("package mutation recipe is not approved")]
    UnsupportedRecipe,
}

/// CommonKit's package operation adapter.
///
/// The default backend is a trait object so the adapter can be registered in
/// the existing `Vec<Box<dyn Adapter>>` reconciliation seam without exposing
/// a generic implementation detail to callers.
pub struct PackageAdapter {
    id: StableId,
    authority: Option<PackageResolutionAuthority>,
    artifacts: ArtifactStore,
    backend: Box<dyn PackageMutationBackend>,
}

impl PackageAdapter {
    pub fn new(
        authority: PackageResolutionAuthority,
        artifacts: ArtifactStore,
        backend: Box<dyn PackageMutationBackend>,
    ) -> Self {
        Self {
            id: StableId::parse("packages").expect("static package adapter ID"),
            authority: Some(authority),
            artifacts,
            backend,
        }
    }

    pub fn with_id(
        id: StableId,
        authority: PackageResolutionAuthority,
        artifacts: ArtifactStore,
        backend: Box<dyn PackageMutationBackend>,
    ) -> Self {
        Self {
            id,
            authority: Some(authority),
            artifacts,
            backend,
        }
    }

    /// Opens a target-side adapter from the persisted resolution and artifact
    /// store only. The controller has already approved the plan; runtime
    /// apply/recovery still revalidates the immutable built-in registry and
    /// never receives provider or network capabilities.
    pub fn new_offline(artifacts: ArtifactStore, backend: Box<dyn PackageMutationBackend>) -> Self {
        Self {
            id: StableId::parse("packages").expect("static package adapter ID"),
            authority: None,
            artifacts,
            backend,
        }
    }

    fn resolution(&self, operation: &Operation) -> Result<PackageResolutionV1, AdapterFailure> {
        if operation.adapter_id != self.id {
            return Err(failure("package_adapter_mismatch"));
        }
        let (_, resolution) = self
            .load_resolution(operation)
            .map_err(|_| failure("package_resolution_invalid"))?;
        ensure_supported(&resolution, self.backend.as_ref())?;
        Ok(resolution)
    }

    fn observe_state(
        &mut self,
        operation: &Operation,
    ) -> Result<(PackageResolutionV1, PackageObservationV1), AdapterFailure> {
        let resolution = self.resolution(operation)?;
        let observed = self
            .backend
            .observe(&resolution)
            .map_err(|_| failure("package_observation_failed"))?;
        Ok((resolution, observed))
    }

    fn after_resolution(resolution: &PackageResolutionV1, observed: &PackageObservationV1) -> bool {
        if resolution.manager.manager == PackageManager::Apt
            && !valid_apt_observation(observed, &resolution.target.arch)
        {
            return false;
        }
        resolution.closure.iter().all(|package| {
            package_version_key(package, resolution)
                .is_some_and(|key| observed.installed_versions.contains(&key))
        })
    }

    fn package_digest(
        &mut self,
        operation: &Operation,
    ) -> Result<Option<Sha256Digest>, AdapterFailure> {
        let (resolution, observed) = self.observe_state(operation)?;
        digest_domain_json("commonkit.package-final-state.v1", &(resolution, observed))
            .map(Some)
            .map_err(|_| failure("package_digest_failed"))
    }

    fn load_resolution(
        &self,
        operation: &Operation,
    ) -> Result<(ResolvedPackageIntent, PackageResolutionV1), PackageMutationError> {
        if let Some(authority) = &self.authority {
            return authority
                .load_by_resolution_digest(&operation.payload_digest, &self.artifacts)
                .map_err(|_| PackageMutationError::Backend);
        }
        let bytes = self
            .artifacts
            .load_by_digest(&operation.payload_digest)
            .map_err(|_| PackageMutationError::Backend)?;
        let resolution: PackageResolutionV1 =
            serde_json::from_slice(&bytes).map_err(|_| PackageMutationError::Backend)?;
        let intent = ResolvedPackageIntent {
            declaration: resolution.declaration.clone(),
            resolution: crate::ContentReference {
                digest: operation.payload_digest.clone(),
                bytes: bytes.len() as u64,
                sensitivity: crate::ContentSensitivity::Portable,
            },
            artifacts: resolution
                .artifacts
                .iter()
                .map(|artifact| artifact.content.clone())
                .collect(),
        };
        let authority = runtime_authority(&resolution)?;
        let validated = authority
            .validate(&intent, &self.artifacts)
            .map_err(|_| PackageMutationError::Backend)?;
        Ok((intent, validated))
    }
}

impl PackageResourcePlanner for PackageAdapter {
    fn adapter_id(&self) -> &StableId {
        &self.id
    }

    fn register_package_resource(
        &mut self,
        id: StableId,
        intent: &ResolvedPackageIntent,
        provenance: &ResourceProvenance,
        provider_artifacts: &ArtifactStore,
    ) -> Result<Option<Operation>, ProviderPlanError> {
        let authority = self
            .authority
            .as_ref()
            .ok_or(ProviderPlanError::MissingPackageResolutionAuthority)?;
        let resolution = authority.validate(intent, provider_artifacts)?;
        let resolution_bytes = provider_artifacts.load(&intent.resolution)?;
        let stored_resolution = self
            .artifacts
            .put(&resolution_bytes, intent.resolution.sensitivity)?;
        if stored_resolution.digest != intent.resolution.digest {
            return Err(ProviderPlanError::PackageResolutionStaging);
        }
        for artifact in &resolution.artifacts {
            let bytes = provider_artifacts.load(&artifact.content)?;
            let stored = self.artifacts.put(&bytes, artifact.content.sensitivity)?;
            if stored.digest != artifact.content.digest {
                return Err(ProviderPlanError::PackageResolutionStaging);
            }
        }
        let desired_digest = crate::ResourceIntent::ResolvedPackage(intent.clone())
            .desired_digest()
            .map_err(ProviderPlanError::Contract)?;
        let operation = finalize_operation(OperationDraft {
            adapter_id: self.id.clone(),
            kind: OperationKind::Create,
            resource: ResourceRef {
                resource_type: StableId::parse("package").expect("static resource type"),
                resource_id: id,
                managed_path: None,
            },
            risk: Risk::Medium,
            requires_confirmation: true,
            recovery_capability: RecoveryCapability::ConvergeForwardOnly,
            depends_on: Vec::new(),
            before_digest: None,
            after_digest: Some(desired_digest),
            payload_digest: stored_resolution.digest,
            provenance: Some(provenance.clone()),
            summary: format!("install {}", resolution.declaration.id.as_str()),
        })
        .map_err(ProviderPlanError::Contract)?;
        Ok(Some(operation))
    }
}

impl Adapter for PackageAdapter {
    fn id(&self) -> &StableId {
        &self.id
    }

    fn supports_recovery(&self, capability: RecoveryCapability) -> bool {
        capability == RecoveryCapability::ConvergeForwardOnly
    }

    fn supports_offline_recovery(&self, _operation: &Operation) -> bool {
        true
    }

    fn preflight(&mut self, operation: &Operation) -> Result<(), AdapterFailure> {
        if operation.kind != OperationKind::Create
            || operation.recovery_capability != RecoveryCapability::ConvergeForwardOnly
        {
            return Err(failure("package_operation_unsupported"));
        }
        self.resolution(operation).map(|_| ())
    }

    fn package_authorization_binding(
        &self,
        operation: &Operation,
    ) -> Result<Option<(PackageOperationConsentBinding, PackageReceiptEvidence)>, AdapterFailure>
    {
        let (_, resolution) = self
            .load_resolution(operation)
            .map_err(|_| failure("package_resolution_invalid"))?;
        ensure_supported(&resolution, self.backend.as_ref())?;
        let manager_authority_digest = digest_domain_json(
            "commonkit.package-manager-authority.v1",
            &resolution.manager,
        )
        .map_err(|_| failure("package_authority_digest_failed"))?;
        Ok(Some((
            PackageOperationConsentBinding {
                operation_id: operation.id.clone(),
                resolution_digest: operation.payload_digest.clone(),
            },
            PackageReceiptEvidence {
                operation_id: operation.id.clone(),
                resolution_digest: operation.payload_digest.clone(),
                target_authority_digest: self
                    .authority
                    .as_ref()
                    .map(|authority| authority.digest().clone())
                    .unwrap_or_else(|| {
                        runtime_authority(&resolution)
                            .expect("validated authority")
                            .digest()
                            .clone()
                    }),
                manager: resolution.manager.manager,
                manager_authority_digest,
                source_id: resolution.source.source_id.clone(),
                source_authority_digest: resolution.source.registry_definition_digest.clone(),
                before_installed_versions: resolution.before.installed_versions.clone(),
                no_preimage_reason:
                    commonkit_contracts::PackageNoPreimageReason::AdditiveForwardOnly,
                exit_classification: PackageExitClassification::NotRun,
                final_digest: None,
            },
        )))
    }

    fn package_final_digest(
        &mut self,
        operation: &Operation,
    ) -> Result<Option<Sha256Digest>, AdapterFailure> {
        self.package_digest(operation)
    }

    fn observe_recovery(
        &mut self,
        operation: &Operation,
    ) -> Result<RecoveryObservation, AdapterFailure> {
        let (resolution, observed) = self.observe_state(operation)?;
        if resolution.manager.manager == PackageManager::Apt
            && !valid_apt_observation(&observed, &resolution.target.arch)
        {
            return Ok(RecoveryObservation::Other);
        }
        let before = if resolution.manager.manager == PackageManager::Apt {
            apt_before_observation(&resolution)
        } else {
            Some(resolution.before.clone())
        };
        let Some(before) = before else {
            return Ok(RecoveryObservation::Other);
        };
        Ok(
            if observed.installed_versions == before.installed_versions {
                RecoveryObservation::Before
            } else if Self::after_resolution(&resolution, &observed) {
                RecoveryObservation::After
            } else {
                RecoveryObservation::Other
            },
        )
    }

    fn prepare_recovery(&mut self, operation: &Operation) -> Result<(), AdapterFailure> {
        let resolution = self.resolution(operation)?;
        self.backend
            .prepare_offline(&resolution, &self.artifacts)
            .map_err(|_| failure("package_recovery_prepare_failed"))
    }

    fn converge_recovery(&mut self, operation: &Operation) -> Result<(), AdapterFailure> {
        let resolution = self.resolution(operation)?;
        self.backend
            .apply_offline(&resolution, &self.artifacts)
            .map_err(|_| failure("package_recovery_apply_failed"))
    }

    fn prepare(&mut self, operation: &Operation) -> Result<(), AdapterFailure> {
        let resolution = self.resolution(operation)?;
        self.backend
            .prepare_offline(&resolution, &self.artifacts)
            .map_err(|_| failure("package_prepare_failed"))
    }

    fn apply(&mut self, operation: &Operation) -> Result<(), AdapterFailure> {
        let resolution = self.resolution(operation)?;
        self.backend
            .apply_offline(&resolution, &self.artifacts)
            .map_err(|_| failure("package_apply_failed"))
    }

    fn verify(&mut self, operation: &Operation) -> Result<(), AdapterFailure> {
        let resolution = self.resolution(operation)?;
        self.backend
            .verify_offline(&resolution, &self.artifacts)
            .map_err(|_| failure("package_verify_failed"))
    }

    fn rollback(&mut self, _operation: &Operation) -> Result<(), AdapterFailure> {
        Err(failure("package_rollback_unsupported"))
    }
}

fn ensure_supported(
    resolution: &PackageResolutionV1,
    backend: &dyn PackageMutationBackend,
) -> Result<(), AdapterFailure> {
    if !backend.supports_manager(resolution.manager.manager) {
        return Err(failure("package_manager_mismatch"));
    }
    if resolution.manager.manager == PackageManager::Apt
        && !valid_apt_resolution_identity(resolution)
    {
        return Err(failure("package_apt_identity_invalid"));
    }
    if !backend.supports_target(resolution) {
        return Err(failure("package_target_unsupported"));
    }
    let approved = matches!(
        (&resolution.manager.manager, &resolution.recipe),
        (
            PackageManager::Apt,
            OfflineInstallRecipeV1::AptArchives { .. }
        ) | (
            PackageManager::Nvm,
            OfflineInstallRecipeV1::NodeArchive {
                install: Some(NodeOfflineInstallRecipeV1 {
                    offline: true,
                    no_source_fallback: true,
                    per_version_lock: true,
                    install_latest_npm: false,
                    migrate_packages: false,
                    ..
                }),
                ..
            },
        )
    );
    if approved {
        Ok(())
    } else {
        Err(failure("package_manager_unsupported"))
    }
}

fn valid_apt_resolution_identity(resolution: &PackageResolutionV1) -> bool {
    let mut identities = BTreeSet::new();
    let mut name_versions = BTreeMap::<(String, String), BTreeSet<String>>::new();
    for package in &resolution.closure {
        let Some(PackageSelector::AptBinary { name, architecture }) =
            package.declaration.selector.as_ref()
        else {
            return false;
        };
        let architecture = architecture.as_deref().unwrap_or(&resolution.target.arch);
        if architecture == "native"
            || (architecture != "all" && architecture != resolution.target.arch)
            || name.is_empty()
            || architecture.is_empty()
        {
            return false;
        }
        if !identities.insert((
            name.to_owned(),
            architecture.to_owned(),
            package.declaration.version.clone(),
        )) {
            return false;
        }
        name_versions
            .entry((name.to_owned(), package.declaration.version.clone()))
            .or_default()
            .insert(architecture.to_owned());
    }
    !name_versions.values().any(|architectures| {
        architectures.contains("all") && architectures.contains(&resolution.target.arch)
    })
}

fn valid_apt_observation(observed: &PackageObservationV1, target_architecture: &str) -> bool {
    let mut identities = BTreeMap::<(String, String), String>::new();
    let mut name_versions = BTreeMap::<(String, String), BTreeSet<String>>::new();
    for identity in &observed.installed_versions {
        let Some((name, architecture, version)) = parse_apt_observation_identity(identity) else {
            return false;
        };
        if architecture != "all" && architecture != target_architecture {
            return false;
        }
        let key = (name.to_owned(), architecture.to_owned());
        if identities
            .insert(key, version.to_owned())
            .is_some_and(|previous| previous != version)
        {
            return false;
        }
        name_versions
            .entry((name.to_owned(), version.to_owned()))
            .or_default()
            .insert(architecture.to_owned());
    }
    !name_versions.values().any(|architectures| {
        architectures.contains("all") && architectures.contains(target_architecture)
    })
}

fn apt_before_observation(resolution: &PackageResolutionV1) -> Option<PackageObservationV1> {
    let mut marker = None;
    let mut installed_versions = BTreeSet::new();
    for identity in &resolution.before.installed_versions {
        if let Some(digest) = identity.strip_prefix(APT_LIVE_SAFETY_MARKER_PREFIX) {
            if marker.replace(digest).is_some() || Sha256Digest::parse(digest).is_err() {
                return None;
            }
        } else {
            installed_versions.insert(identity.clone());
        }
    }
    marker?;
    let observed = PackageObservationV1 { installed_versions };
    valid_apt_observation(&observed, &resolution.target.arch).then_some(observed)
}

fn parse_apt_observation_identity(identity: &str) -> Option<(&str, &str, &str)> {
    if identity.chars().any(char::is_whitespace) || identity.matches('=').count() != 1 {
        return None;
    }
    let (name_architecture, version) = identity.split_once('=')?;
    let (name, architecture) = name_architecture.split_once(':')?;
    if name.is_empty()
        || architecture.is_empty()
        || version.is_empty()
        || name.contains(':')
        || architecture.contains(':')
    {
        return None;
    }
    Some((name, architecture, version))
}

fn runtime_authority(
    resolution: &PackageResolutionV1,
) -> Result<PackageResolutionAuthority, PackageMutationError> {
    // The persisted resolution carries the target-attested source-definition
    // digest. Rebuild the trusted built-in registry and enrich only the
    // matching source identity; never accept source IDs or repositories from
    // the payload as new authority.
    let registry = crate::PackageSourceRegistry::for_remote_resolution(
        &resolution.source,
        resolution.manager.manager,
    )
    .map_err(|_| PackageMutationError::Backend)?;
    let source = resolution.source.source_id.clone();
    let policy = SecurityPolicy {
        allowlists: [(
            StableId::parse("package_sources").unwrap(),
            [source.as_str().into()].into_iter().collect(),
        )]
        .into_iter()
        .collect(),
        ..SecurityPolicy::default()
    };
    PackageResolutionAuthority::new(&resolution.target, &resolution.manager, &registry, &policy)
        .map_err(|_| PackageMutationError::Backend)
}

/// Native target implementation for the two production offline recipes.
/// Every archive is loaded from the controller-persisted artifact store;
/// package commands are fixed and never receive a repository or URL.
pub struct ProcessOfflinePackageBackend {
    logical_root: PathBuf,
    process_root: PathBuf,
    package_resolution: Option<crate::TargetPackageResolutionConfig>,
    require_package_resolution: bool,
    #[cfg(unix)]
    _root_dir: Option<Dir>,
}

/// SSH counterpart to the native offline backend. It uses only the typed
/// package request in the closed filesystem protocol; arbitrary remote shell
/// commands are never exposed to the controller.
pub struct SshOfflinePackageBackend<T> {
    root_id: StableId,
    transport: T,
    target_platform: Option<(String, String)>,
    target_identity_digest: Option<Sha256Digest>,
}

impl<T> SshOfflinePackageBackend<T> {
    pub fn new(root_id: StableId, transport: T) -> Self {
        Self {
            root_id,
            transport,
            target_platform: None,
            target_identity_digest: None,
        }
    }

    pub fn with_target_platform(
        root_id: StableId,
        transport: T,
        operating_system: impl Into<String>,
        architecture: impl Into<String>,
        target_identity_digest: Sha256Digest,
    ) -> Self {
        Self {
            root_id,
            transport,
            target_platform: Some((operating_system.into(), architecture.into())),
            target_identity_digest: Some(target_identity_digest),
        }
    }

    fn request(
        &mut self,
        phase: crate::PackageMutationPhase,
        resolution: &PackageResolutionV1,
        artifacts: Option<&ArtifactStore>,
    ) -> Result<crate::SshFilesystemResponse, PackageMutationError>
    where
        T: crate::SshFilesystemTransport,
    {
        let transferred = match artifacts {
            Some(artifacts) => self.stage_artifacts(resolution, artifacts)?,
            None => Vec::new(),
        };
        self.transport
            .perform(crate::SshFilesystemRequest::PackageMutation {
                root_id: self.root_id.clone(),
                phase,
                resolution: resolution.clone(),
                artifacts: transferred,
                target_identity_digest: self.target_identity_digest.clone(),
            })
            .map_err(|_| PackageMutationError::Backend)
    }

    fn stage_artifacts(
        &mut self,
        resolution: &PackageResolutionV1,
        artifacts: &ArtifactStore,
    ) -> Result<Vec<crate::PackageMutationArtifact>, PackageMutationError>
    where
        T: crate::SshFilesystemTransport,
    {
        if resolution.artifacts.len() > crate::MAX_ARTIFACT_TRANSFER_COUNT {
            return Err(PackageMutationError::Backend);
        }
        let aggregate = resolution
            .artifacts
            .iter()
            .try_fold(0u64, |total, artifact| {
                total.checked_add(artifact.content.bytes)
            });
        if aggregate.is_none_or(|bytes| bytes > crate::MAX_ARTIFACT_TRANSFER_BYTES) {
            return Err(PackageMutationError::Backend);
        }
        let run_digest =
            digest_domain_json("commonkit.ssh-package-mutation-transfer.v1", resolution)
                .map_err(|_| PackageMutationError::Backend)?;
        let run_id = crate::artifact_transfer_id("mutation-run", &run_digest)
            .map_err(|_| PackageMutationError::Backend)?;
        let mut transferred = Vec::with_capacity(resolution.artifacts.len());
        for artifact in &resolution.artifacts {
            let reference = &artifact.content;
            if reference.bytes == 0
                || reference.bytes > crate::MAX_ARTIFACT_TRANSFER_BYTES
                || reference.sensitivity != crate::ContentSensitivity::Portable
            {
                return Err(PackageMutationError::Backend);
            }
            let transfer_id = crate::artifact_transfer_id("mutation", &reference.digest)
                .map_err(|_| PackageMutationError::Backend)?;
            let total_chunks = u32::try_from(
                reference
                    .bytes
                    .div_ceil(u64::from(crate::ARTIFACT_CHUNK_SIZE)),
            )
            .map_err(|_| PackageMutationError::Backend)?;
            for sequence in 0..total_chunks {
                let offset = u64::from(sequence) * u64::from(crate::ARTIFACT_CHUNK_SIZE);
                let content = artifacts
                    .read_chunk(reference, offset, crate::ARTIFACT_CHUNK_SIZE)
                    .map_err(|_| PackageMutationError::Backend)?;
                let expected_response_digest = crate::artifact_chunk_response_digest(
                    &run_id,
                    &transfer_id,
                    &reference.digest,
                    reference.bytes,
                    crate::ARTIFACT_CHUNK_SIZE,
                    sequence,
                    offset,
                    total_chunks,
                    &content,
                )
                .map_err(|_| PackageMutationError::Backend)?;
                match self
                    .transport
                    .perform(crate::SshFilesystemRequest::StageArtifactChunk {
                        run_id: run_id.clone(),
                        transfer_id: transfer_id.clone(),
                        digest: reference.digest.clone(),
                        byte_count: reference.bytes,
                        chunk_size: crate::ARTIFACT_CHUNK_SIZE,
                        sequence,
                        offset,
                        total_chunks,
                        content,
                    })
                    .map_err(|_| PackageMutationError::Backend)?
                {
                    crate::SshFilesystemResponse::ArtifactChunkStaged {
                        run_id: response_run,
                        transfer_id: response_transfer,
                        digest,
                        byte_count,
                        chunk_size,
                        sequence: response_sequence,
                        offset: response_offset,
                        total_chunks: response_total,
                        response_digest,
                    } if response_run == run_id
                        && response_transfer == transfer_id
                        && digest == reference.digest
                        && byte_count == reference.bytes
                        && chunk_size == crate::ARTIFACT_CHUNK_SIZE
                        && response_sequence == sequence
                        && response_offset == offset
                        && response_total == total_chunks
                        && response_digest == expected_response_digest => {}
                    _ => return Err(PackageMutationError::Backend),
                }
            }
            match self
                .transport
                .perform(crate::SshFilesystemRequest::VerifyArtifact {
                    run_id: run_id.clone(),
                    digest: reference.digest.clone(),
                })
                .map_err(|_| PackageMutationError::Backend)?
            {
                crate::SshFilesystemResponse::ArtifactVerified { digest }
                    if digest == reference.digest => {}
                _ => return Err(PackageMutationError::Backend),
            }
            transferred.push(crate::PackageMutationArtifact {
                reference: reference.clone(),
            });
        }
        Ok(transferred)
    }
}

impl<T: crate::SshFilesystemTransport + Send> PackageMutationBackend
    for SshOfflinePackageBackend<T>
{
    fn manager(&self) -> PackageManager {
        PackageManager::Apt
    }

    fn supports_manager(&self, manager: PackageManager) -> bool {
        matches!(manager, PackageManager::Apt | PackageManager::Nvm)
    }

    fn supports_target(&self, resolution: &PackageResolutionV1) -> bool {
        self.target_platform.as_ref().is_some_and(|(os, arch)| {
            package_target_matches_platform(
                &resolution.target,
                resolution.manager.manager,
                os,
                arch,
            )
        })
    }

    fn observe(
        &mut self,
        resolution: &PackageResolutionV1,
    ) -> Result<PackageObservationV1, PackageMutationError> {
        match self.request(crate::PackageMutationPhase::Observe, resolution, None)? {
            crate::SshFilesystemResponse::PackageObserved { installed_versions } => {
                Ok(PackageObservationV1 { installed_versions })
            }
            _ => Err(PackageMutationError::Backend),
        }
    }

    fn prepare_offline(
        &mut self,
        resolution: &PackageResolutionV1,
        artifacts: &ArtifactStore,
    ) -> Result<(), PackageMutationError> {
        self.request(
            crate::PackageMutationPhase::Prepare,
            resolution,
            Some(artifacts),
        )?
        .ensure_applied()
    }

    fn apply_offline(
        &mut self,
        resolution: &PackageResolutionV1,
        artifacts: &ArtifactStore,
    ) -> Result<(), PackageMutationError> {
        self.request(
            crate::PackageMutationPhase::Apply,
            resolution,
            Some(artifacts),
        )?
        .ensure_applied()
    }

    fn verify_offline(
        &mut self,
        resolution: &PackageResolutionV1,
        artifacts: &ArtifactStore,
    ) -> Result<(), PackageMutationError> {
        self.request(
            crate::PackageMutationPhase::Verify,
            resolution,
            Some(artifacts),
        )?
        .ensure_applied()
    }
}

trait PackageMutationResponseExt {
    fn ensure_applied(self) -> Result<(), PackageMutationError>;
}

impl PackageMutationResponseExt for crate::SshFilesystemResponse {
    fn ensure_applied(self) -> Result<(), PackageMutationError> {
        matches!(self, crate::SshFilesystemResponse::Applied)
            .then_some(())
            .ok_or(PackageMutationError::Backend)
    }
}

impl ProcessOfflinePackageBackend {
    pub fn new(target_root: impl Into<PathBuf>) -> Self {
        let target_root = target_root.into();
        Self {
            logical_root: target_root.clone(),
            process_root: target_root,
            package_resolution: None,
            require_package_resolution: false,
            #[cfg(unix)]
            _root_dir: None,
        }
    }

    /// Opens the offline backend with the target-local manager authority that
    /// was used to create the persisted resolution. Every mutation phase then
    /// re-probes that authority before it observes or changes packages.
    pub fn with_package_resolution(
        target_root: impl Into<PathBuf>,
        package_resolution: crate::TargetPackageResolutionConfig,
    ) -> Self {
        let backend = Self::new(target_root);
        backend.with_target_package_resolution(package_resolution)
    }

    /// Attaches the target-local manager authority to an already root-bound
    /// backend, as used by the remote helper after it opens its root handle.
    pub fn with_target_package_resolution(
        mut self,
        package_resolution: crate::TargetPackageResolutionConfig,
    ) -> Self {
        self.package_resolution = Some(package_resolution);
        self
    }

    /// Makes a production executor fail closed if no target-local manager
    /// authority was attached before a package phase starts.
    pub fn require_target_package_resolution(mut self) -> Self {
        self.require_package_resolution = true;
        self
    }

    #[cfg(unix)]
    pub fn new_with_bound_root(
        _target_root: impl Into<PathBuf>,
        root_handle: std::fs::File,
    ) -> Result<Self, PackageMutationError> {
        use std::os::fd::AsRawFd;

        let fd = root_handle.as_raw_fd();
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) } < 0 {
            return Err(PackageMutationError::Backend);
        }
        #[cfg(target_os = "linux")]
        let fd_path = "/proc/self/fd";
        #[cfg(not(target_os = "linux"))]
        let fd_path = "/dev/fd";
        let process_root = PathBuf::from(format!("{fd_path}/{fd}"));
        if !fs::metadata(&process_root)
            .map(|metadata| metadata.is_dir())
            .unwrap_or(false)
        {
            return Err(PackageMutationError::Backend);
        }
        Ok(Self {
            logical_root: _target_root.into(),
            process_root,
            package_resolution: None,
            require_package_resolution: false,
            _root_dir: Some(Dir::from_std_file(root_handle)),
        })
    }

    #[cfg(not(unix))]
    pub fn new_with_bound_root(
        _target_root: impl Into<PathBuf>,
        _root_handle: std::fs::File,
    ) -> Result<Self, PackageMutationError> {
        Err(PackageMutationError::Backend)
    }

    fn revalidate_live_manager_binding(
        &self,
        resolution: &PackageResolutionV1,
    ) -> Result<(), PackageMutationError> {
        let Some(config) = &self.package_resolution else {
            if self.require_package_resolution {
                return Err(PackageMutationError::Backend);
            }
            // Legacy/test-only constructors do not carry target-local paths.
            // Production constructors are wired with the persisted target
            // configuration before they are exposed to a plan executor.
            return Ok(());
        };
        if !package_targets_match(&config.target, &resolution.target)
            || config.manager != resolution.manager
        {
            return Err(PackageMutationError::Backend);
        }
        match resolution.manager.manager {
            PackageManager::Apt => {
                let repository = config.apt.as_ref().ok_or(PackageMutationError::Backend)?;
                if repository.source_id != resolution.source.source_id {
                    return Err(PackageMutationError::Backend);
                }
                if resolution
                    .source
                    .signed_metadata
                    .iter()
                    .any(|evidence| evidence.authority != repository.signing_authority)
                {
                    return Err(PackageMutationError::Backend);
                }
                let actual = crate::ProcessAptResolutionCommandRunner::probe_manager_binding(
                    &config.target,
                    &resolution.source.canonical_repository,
                    repository,
                )
                .map_err(|_| PackageMutationError::Backend)?;
                (actual == resolution.manager)
                    .then_some(())
                    .ok_or(PackageMutationError::Backend)
            }
            PackageManager::Nvm => {
                let node = config.node.as_ref().ok_or(PackageMutationError::Backend)?;
                let mut host = crate::ProcessNodeRuntimeHost::new_with_gpgv(
                    node.nvm_dir.clone(),
                    node.shell_executable.clone(),
                    node.release_keyring.clone(),
                    node.gpgv_executable.clone(),
                );
                let actual = host
                    .probe(&config.target)
                    .map_err(|_| PackageMutationError::Backend)?;
                (actual.manager == resolution.manager)
                    .then_some(())
                    .ok_or(PackageMutationError::Backend)
            }
            _ => Err(PackageMutationError::UnsupportedRecipe),
        }
    }

    fn nvm_shell_executable(&self) -> PathBuf {
        self.package_resolution
            .as_ref()
            .and_then(|config| config.node.as_ref())
            .map(|node| node.shell_executable.clone())
            .unwrap_or_else(|| PathBuf::from("/bin/bash"))
    }

    fn observe_apt(
        &self,
        resolution: &PackageResolutionV1,
    ) -> Result<PackageObservationV1, PackageMutationError> {
        let output = Command::new("/usr/bin/dpkg-query")
            .args(["-W", "-f=${Package}:${Architecture}=${Version}\\n"])
            .output()
            .map_err(|_| PackageMutationError::Backend)?;
        if !output.status.success() {
            return Err(PackageMutationError::Backend);
        }
        let observed = PackageObservationV1 {
            installed_versions: String::from_utf8_lossy(&output.stdout)
                .lines()
                .filter(|line| !line.is_empty())
                .map(str::to_owned)
                .collect(),
        };
        valid_apt_observation(&observed, &resolution.target.arch)
            .then_some(observed)
            .ok_or(PackageMutationError::Backend)
    }

    fn observe_nvm(
        &self,
        resolution: &PackageResolutionV1,
    ) -> Result<PackageObservationV1, PackageMutationError> {
        self.ensure_nvm_target(resolution)?;
        #[cfg(unix)]
        let bound_nvm = self.bound_nvm_dir()?;
        #[cfg(unix)]
        let nvm_dir = bound_nvm
            .as_ref()
            .map(|bound| bound.path.clone())
            .unwrap_or_else(|| self.process_root.join(".nvm"));
        #[cfg(not(unix))]
        let nvm_dir = self.process_root.join(".nvm");
        let script = self.validate_nvm_script(resolution)?;
        let script_copy = script.materialize()?;
        let mut command = Command::new(self.nvm_shell_executable());
        command
            .arg("-c")
            .arg("set -eu; . \"$1\"; nvm ls --no-colors")
            .arg("commonkit")
            .arg(script_copy.path())
            .env_clear()
            .env("HOME", &self.process_root)
            .env("NVM_DIR", &nvm_dir)
            .env("PATH", CLOSED_NVM_PATH)
            .env("NVM_NO_SOURCE_FALLBACK", "1")
            .env("NVM_OFFLINE", "1");
        let _bound_root = self.configure_nvm_command(&mut command)?;
        let output = command
            .output()
            .map_err(|_| PackageMutationError::Backend)?;
        if !output.status.success() {
            return Err(PackageMutationError::Backend);
        }
        let installed_versions = String::from_utf8_lossy(&output.stdout)
            .split_whitespace()
            .filter_map(|field| field.strip_prefix('v'))
            .filter(|version| version.chars().next().is_some_and(|c| c.is_ascii_digit()))
            .map(str::to_owned)
            .collect();
        Ok(PackageObservationV1 { installed_versions })
    }

    fn validate_nvm_script(
        &self,
        resolution: &PackageResolutionV1,
    ) -> Result<ValidatedNvmScript, PackageMutationError> {
        #[cfg(unix)]
        if let Some(root) = &self._root_dir {
            let nvm = crate::target::open_target_dir_nofollow(root, Path::new(".nvm"))
                .map_err(|_| PackageMutationError::Backend)?;
            let bytes = read_bound_regular_no_follow(&nvm, Path::new("nvm.sh"))?;
            let digest = Sha256Digest::parse(format!("sha256:{:x}", Sha256::digest(&bytes)))
                .map_err(|_| PackageMutationError::Backend)?;
            if digest != resolution.manager.executable_digest {
                return Err(PackageMutationError::Backend);
            }
            return Ok(ValidatedNvmScript { bytes });
        }
        let script = self.process_root.join(".nvm/nvm.sh");
        let bytes = read_regular_no_follow(&script)?;
        let digest = Sha256Digest::parse(format!("sha256:{:x}", Sha256::digest(&bytes)))
            .map_err(|_| PackageMutationError::Backend)?;
        if digest != resolution.manager.executable_digest {
            return Err(PackageMutationError::Backend);
        }
        Ok(ValidatedNvmScript { bytes })
    }

    fn ensure_nvm_target(
        &self,
        resolution: &PackageResolutionV1,
    ) -> Result<(), PackageMutationError> {
        if !self.nvm_target_matches(resolution) {
            return Err(PackageMutationError::Backend);
        }
        Ok(())
    }

    fn nvm_target_matches(&self, resolution: &PackageResolutionV1) -> bool {
        let expected = self.logical_root.join(".nvm");
        resolution.target.manager_prefix.as_deref() == expected.to_str()
    }

    fn artifact_bytes(
        &self,
        resolution: &PackageResolutionV1,
        artifacts: &ArtifactStore,
        role: &str,
    ) -> Result<Vec<u8>, PackageMutationError> {
        let artifact = resolution
            .artifacts
            .iter()
            .find(|artifact| artifact.role.as_str() == role)
            .ok_or(PackageMutationError::UnsupportedRecipe)?;
        artifacts
            .load(&artifact.content)
            .map_err(|_| PackageMutationError::Backend)
    }
}

impl PackageMutationBackend for ProcessOfflinePackageBackend {
    fn manager(&self) -> PackageManager {
        PackageManager::Apt
    }

    fn supports_manager(&self, manager: PackageManager) -> bool {
        matches!(manager, PackageManager::Apt | PackageManager::Nvm)
    }

    fn supports_target(&self, resolution: &PackageResolutionV1) -> bool {
        package_target_matches_platform(
            &resolution.target,
            resolution.manager.manager,
            std::env::consts::OS,
            std::env::consts::ARCH,
        ) && (resolution.manager.manager != PackageManager::Nvm
            || self.nvm_target_matches(resolution))
    }

    fn observe(
        &mut self,
        resolution: &PackageResolutionV1,
    ) -> Result<PackageObservationV1, PackageMutationError> {
        self.revalidate_live_manager_binding(resolution)?;
        match resolution.manager.manager {
            PackageManager::Apt => self.observe_apt(resolution),
            PackageManager::Nvm => self.observe_nvm(resolution),
            _ => Err(PackageMutationError::UnsupportedRecipe),
        }
    }

    fn prepare_offline(
        &mut self,
        resolution: &PackageResolutionV1,
        artifacts: &ArtifactStore,
    ) -> Result<(), PackageMutationError> {
        self.revalidate_live_manager_binding(resolution)?;
        if resolution.manager.manager == PackageManager::Nvm {
            self.ensure_nvm_target(resolution)?;
        }
        match &resolution.recipe {
            OfflineInstallRecipeV1::AptArchives { artifact_roles }
            | OfflineInstallRecipeV1::NodeArchive { artifact_roles, .. } => {
                for artifact in &resolution.artifacts {
                    if artifact_roles.contains(&artifact.role) {
                        artifacts
                            .load(&artifact.content)
                            .map_err(|_| PackageMutationError::Backend)?;
                    }
                }
                Ok(())
            }
            _ => Err(PackageMutationError::UnsupportedRecipe),
        }
    }

    fn apply_offline(
        &mut self,
        resolution: &PackageResolutionV1,
        artifacts: &ArtifactStore,
    ) -> Result<(), PackageMutationError> {
        self.revalidate_live_manager_binding(resolution)?;
        match resolution.manager.manager {
            PackageManager::Apt => {
                let OfflineInstallRecipeV1::AptArchives { artifact_roles } = &resolution.recipe
                else {
                    return Err(PackageMutationError::UnsupportedRecipe);
                };
                let staging = tempfile::tempdir().map_err(|_| PackageMutationError::Backend)?;
                let mut archives = Vec::new();
                for artifact in &resolution.artifacts {
                    if artifact_roles.contains(&artifact.role) {
                        let path = staging.path().join(artifact.role.as_str());
                        fs::write(
                            &path,
                            artifacts
                                .load(&artifact.content)
                                .map_err(|_| PackageMutationError::Backend)?,
                        )
                        .map_err(|_| PackageMutationError::Backend)?;
                        archives.push(path);
                    }
                }
                let status = Command::new("/usr/bin/dpkg")
                    .arg("--install")
                    .args(&archives)
                    .status()
                    .map_err(|_| PackageMutationError::Backend)?;
                status
                    .success()
                    .then_some(())
                    .ok_or(PackageMutationError::Backend)
            }
            PackageManager::Nvm => {
                let OfflineInstallRecipeV1::NodeArchive {
                    install: Some(install),
                    ..
                } = &resolution.recipe
                else {
                    return Err(PackageMutationError::UnsupportedRecipe);
                };
                self.ensure_nvm_target(resolution)?;
                let script = self.validate_nvm_script(resolution)?;
                let script_copy = script.materialize()?;
                let archive = self.artifact_bytes(resolution, artifacts, "node-archive")?;
                self.write_nvm_cache(&install.cache_relative_path, &archive)?;
                #[cfg(unix)]
                let bound_nvm = self.bound_nvm_dir()?;
                #[cfg(unix)]
                let nvm_dir = bound_nvm
                    .as_ref()
                    .map(|bound| bound.path.clone())
                    .unwrap_or_else(|| self.process_root.join(".nvm"));
                #[cfg(not(unix))]
                let nvm_dir = self.process_root.join(".nvm");
                let mut command = Command::new(self.nvm_shell_executable());
                command
                    .arg("-c")
                    .arg("set -eu; export NVM_NO_SOURCE_FALLBACK=1 NVM_OFFLINE=1; . \"$1\"; nvm install --offline \"$2\"")
                    .arg("commonkit")
                    .arg(script_copy.path())
                    .arg(&install.node_version)
                    .env_clear()
                    .env("HOME", &self.process_root)
                    .env("NVM_DIR", nvm_dir)
                    .env("PATH", CLOSED_NVM_PATH)
                    .env("NVM_NO_SOURCE_FALLBACK", "1")
                    .env("NVM_OFFLINE", "1");
                let _bound_root = self.configure_nvm_command(&mut command)?;
                let status = command
                    .status()
                    .map_err(|_| PackageMutationError::Backend)?;
                status
                    .success()
                    .then_some(())
                    .ok_or(PackageMutationError::Backend)
            }
            _ => Err(PackageMutationError::UnsupportedRecipe),
        }
    }

    fn verify_offline(
        &mut self,
        resolution: &PackageResolutionV1,
        _artifacts: &ArtifactStore,
    ) -> Result<(), PackageMutationError> {
        self.revalidate_live_manager_binding(resolution)?;
        let observed = self.observe(resolution)?;
        if PackageAdapter::after_resolution(resolution, &observed) {
            Ok(())
        } else {
            Err(PackageMutationError::Backend)
        }
    }
}

impl ProcessOfflinePackageBackend {
    #[cfg(unix)]
    fn configure_nvm_command(
        &self,
        command: &mut Command,
    ) -> Result<Option<std::fs::File>, PackageMutationError> {
        let Some(root) = &self._root_dir else {
            return Ok(None);
        };
        use std::os::fd::AsRawFd;
        use std::os::unix::process::CommandExt;
        let handle = root
            .try_clone()
            .map_err(|_| PackageMutationError::Backend)?
            .into_std_file();
        let fd = handle.as_raw_fd();
        command.env("HOME", ".").env("NVM_DIR", "./.nvm");
        unsafe {
            command.pre_exec(move || {
                if libc::fchdir(fd) == 0 {
                    Ok(())
                } else {
                    Err(std::io::Error::last_os_error())
                }
            });
        }
        Ok(Some(handle))
    }

    #[cfg(unix)]
    fn bound_nvm_dir(&self) -> Result<Option<BoundNvmDir>, PackageMutationError> {
        let Some(root) = &self._root_dir else {
            return Ok(None);
        };
        let nvm = crate::target::open_target_dir_nofollow(root, Path::new(".nvm"))
            .map_err(|_| PackageMutationError::Backend)?;
        let handle = nvm
            .try_clone()
            .map_err(|_| PackageMutationError::Backend)?
            .into_std_file();
        let fd = std::os::fd::AsRawFd::as_raw_fd(&handle);
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) } < 0 {
            return Err(PackageMutationError::Backend);
        }
        #[cfg(target_os = "linux")]
        let fd_path = "/proc/self/fd";
        #[cfg(not(target_os = "linux"))]
        let fd_path = "/dev/fd";
        Ok(Some(BoundNvmDir {
            path: PathBuf::from(format!("{fd_path}/{fd}")),
            _handle: handle,
        }))
    }

    fn write_nvm_cache(
        &self,
        relative_path: &str,
        bytes: &[u8],
    ) -> Result<(), PackageMutationError> {
        #[cfg(unix)]
        if let Some(root) = &self._root_dir {
            let nvm = crate::target::open_target_dir_nofollow(root, Path::new(".nvm"))
                .map_err(|_| PackageMutationError::Backend)?;
            let relative = Path::new(relative_path);
            let parent = relative.parent().unwrap_or_else(|| Path::new("."));
            let parent = open_or_create_bound_dir(&nvm, parent)?;
            let leaf = relative
                .file_name()
                .filter(|name| *name != "." && *name != "..")
                .ok_or(PackageMutationError::Backend)?;
            let mut options = cap_std::fs::OpenOptions::new();
            options.write(true).create(true).truncate(true);
            use cap_std::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW);
            let mut file = parent
                .open_with(Path::new(leaf), &options)
                .map_err(|_| PackageMutationError::Backend)?;
            file.write_all(bytes)
                .and_then(|_| file.sync_all())
                .map_err(|_| PackageMutationError::Backend)?;
            return Ok(());
        }
        let cache = self.process_root.join(".nvm").join(relative_path);
        if let Some(parent) = cache.parent() {
            fs::create_dir_all(parent).map_err(|_| PackageMutationError::Backend)?;
        }
        fs::write(&cache, bytes).map_err(|_| PackageMutationError::Backend)
    }
}

#[cfg(unix)]
struct BoundNvmDir {
    path: PathBuf,
    _handle: std::fs::File,
}

#[cfg(unix)]
fn open_or_create_bound_dir(root: &Dir, relative: &Path) -> Result<Dir, PackageMutationError> {
    let mut current = root
        .try_clone()
        .map_err(|_| PackageMutationError::Backend)?;
    for component in relative.components() {
        let std::path::Component::Normal(name) = component else {
            if matches!(component, std::path::Component::CurDir) {
                continue;
            }
            return Err(PackageMutationError::Backend);
        };
        match crate::target::open_target_dir_nofollow(&current, Path::new(name)) {
            Ok(next) => current = next,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                current
                    .create_dir(name)
                    .map_err(|_| PackageMutationError::Backend)?;
                current = crate::target::open_target_dir_nofollow(&current, Path::new(name))
                    .map_err(|_| PackageMutationError::Backend)?;
            }
            Err(_) => return Err(PackageMutationError::Backend),
        }
    }
    Ok(current)
}

#[cfg(unix)]
fn read_bound_regular_no_follow(
    parent: &Dir,
    name: &Path,
) -> Result<Vec<u8>, PackageMutationError> {
    let metadata = parent
        .symlink_metadata(name)
        .map_err(|_| PackageMutationError::Backend)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(PackageMutationError::Backend);
    }
    let mut file = crate::target::open_target_file_nofollow(parent, name)
        .map_err(|_| PackageMutationError::Backend)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|_| PackageMutationError::Backend)?;
    Ok(bytes)
}

struct ValidatedNvmScript {
    bytes: Vec<u8>,
}

struct NvmScriptCopy {
    _directory: tempfile::TempDir,
    path: PathBuf,
}

impl ValidatedNvmScript {
    fn materialize(&self) -> Result<NvmScriptCopy, PackageMutationError> {
        let directory = tempfile::Builder::new()
            .prefix("commonkit-nvm-script-")
            .tempdir()
            .map_err(|_| PackageMutationError::Backend)?;
        let path = directory.path().join("nvm.sh");
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
            options.custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
            options.mode(0o600);
            let mut file = options
                .open(&path)
                .map_err(|_| PackageMutationError::Backend)?;
            file.write_all(&self.bytes)
                .and_then(|()| file.sync_all())
                .map_err(|_| PackageMutationError::Backend)?;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o400))
                .map_err(|_| PackageMutationError::Backend)?;
        }
        #[cfg(not(unix))]
        {
            let mut file = options
                .open(&path)
                .map_err(|_| PackageMutationError::Backend)?;
            file.write_all(&self.bytes)
                .and_then(|()| file.sync_all())
                .map_err(|_| PackageMutationError::Backend)?;
        }
        Ok(NvmScriptCopy {
            _directory: directory,
            path,
        })
    }
}

impl NvmScriptCopy {
    fn path(&self) -> &Path {
        &self.path
    }
}

fn read_regular_no_follow(path: &Path) -> Result<Vec<u8>, PackageMutationError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| PackageMutationError::Backend)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(PackageMutationError::Backend);
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
    }
    let mut file = options
        .open(path)
        .map_err(|_| PackageMutationError::Backend)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|_| PackageMutationError::Backend)?;
    Ok(bytes)
}

fn package_target_matches_platform(
    target: &crate::PackageTargetV1,
    manager: PackageManager,
    operating_system: &str,
    architecture: &str,
) -> bool {
    let target_os = canonical_os(&target.os);
    let platform_os = canonical_os(operating_system);
    let os_matches = match manager {
        PackageManager::Apt => platform_os == "linux",
        PackageManager::Nvm => platform_os == "linux" || platform_os == "macos",
        _ => false,
    };
    os_matches
        && target_os == platform_os
        && canonical_arch(&target.arch) == canonical_arch(architecture)
}

fn package_targets_match(left: &crate::PackageTargetV1, right: &crate::PackageTargetV1) -> bool {
    canonical_os(&left.os) == canonical_os(&right.os)
        && left.os_version == right.os_version
        && left.distro_id == right.distro_id
        && left.distro_version == right.distro_version
        && left.codename == right.codename
        && canonical_arch(&left.arch) == canonical_arch(&right.arch)
        && left.libc == right.libc
        && left.manager_prefix == right.manager_prefix
}

fn canonical_os(operating_system: &str) -> String {
    let operating_system = operating_system.to_ascii_lowercase();
    if operating_system == "darwin" {
        "macos".into()
    } else {
        operating_system
    }
}

fn canonical_arch(architecture: &str) -> std::borrow::Cow<'_, str> {
    match architecture.to_ascii_lowercase().as_str() {
        "x86_64" | "amd64" => "x86_64".into(),
        "aarch64" | "arm64" => "aarch64".into(),
        "armv7" | "armv7l" => "arm".into(),
        other => other.to_owned().into(),
    }
}

fn package_version_key(
    package: &crate::ResolvedPackage,
    resolution: &PackageResolutionV1,
) -> Option<String> {
    match resolution.manager.manager {
        PackageManager::Apt => {
            let Some(PackageSelector::AptBinary { name, architecture }) =
                package.declaration.selector.as_ref()
            else {
                return None;
            };
            let architecture = architecture.as_deref().unwrap_or(&resolution.target.arch);
            Some(apt_package_identity(
                name,
                architecture,
                &package.declaration.version,
            ))
        }
        PackageManager::Nvm => Some(
            package
                .declaration
                .version
                .strip_prefix('v')
                .unwrap_or(&package.declaration.version)
                .to_owned(),
        ),
        PackageManager::Homebrew | PackageManager::Fnm | PackageManager::Rustup => None,
    }
}

fn failure(code: &'static str) -> AdapterFailure {
    AdapterFailure::new(code, "package operation rejected")
}

#[cfg(test)]
mod tests {
    use super::{
        PackageManager, ValidatedNvmScript, package_target_matches_platform, package_targets_match,
    };
    use crate::PackageTargetV1;
    use std::fs;

    fn target(os: &str, arch: &str) -> PackageTargetV1 {
        PackageTargetV1 {
            os: os.into(),
            os_version: "1".into(),
            distro_id: None,
            distro_version: None,
            codename: None,
            arch: arch.into(),
            libc: None,
            manager_prefix: None,
        }
    }

    #[test]
    fn local_package_target_matching_accepts_linux_architecture_aliases() {
        assert!(package_target_matches_platform(
            &target("linux", "amd64"),
            PackageManager::Apt,
            "linux",
            "x86_64",
        ));
        assert!(!package_target_matches_platform(
            &target("linux", "amd64"),
            PackageManager::Apt,
            "linux",
            "arm64",
        ));
    }

    #[test]
    fn local_package_target_matching_accepts_macos_darwin_aliases() {
        assert!(package_target_matches_platform(
            &target("darwin", "x86_64"),
            PackageManager::Nvm,
            "macos",
            "x86_64",
        ));
        assert!(!package_target_matches_platform(
            &target("darwin", "x86_64"),
            PackageManager::Nvm,
            "linux",
            "x86_64",
        ));
    }

    #[test]
    fn local_manager_binding_matching_accepts_aliases_but_rejects_foreign_targets() {
        assert!(package_targets_match(
            &target("linux", "x86_64"),
            &target("linux", "amd64"),
        ));
        assert!(package_targets_match(
            &target("darwin", "x86_64"),
            &target("macos", "x86_64"),
        ));
        assert!(!package_targets_match(
            &target("linux", "x86_64"),
            &target("linux", "arm64"),
        ));
    }

    #[test]
    fn validated_nvm_script_materializes_an_immutable_copy() {
        let bytes = b"validated nvm bytes\n".to_vec();
        let copy = ValidatedNvmScript {
            bytes: bytes.clone(),
        }
        .materialize()
        .unwrap();
        assert_eq!(fs::read(copy.path()).unwrap(), bytes);
        let metadata = fs::symlink_metadata(copy.path()).unwrap();
        assert!(metadata.is_file());
        assert!(!metadata.file_type().is_symlink());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(metadata.permissions().mode() & 0o222, 0);
        }
    }

    #[cfg(unix)]
    #[test]
    fn bound_nvm_cache_write_survives_root_path_swap() {
        use super::ProcessOfflinePackageBackend;
        use std::fs::OpenOptions;
        use std::os::unix::fs::OpenOptionsExt;

        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join(".nvm")).unwrap();
        let handle = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(root.path())
            .unwrap();
        let backend =
            ProcessOfflinePackageBackend::new_with_bound_root(root.path(), handle).unwrap();
        let replacement = tempfile::tempdir().unwrap();
        fs::rename(root.path(), replacement.path().join("original")).unwrap();
        fs::create_dir_all(root.path()).unwrap();

        backend
            .write_nvm_cache(".cache/node/archive", b"bound bytes")
            .unwrap();
        assert_eq!(
            fs::read(replacement.path().join("original/.nvm/.cache/node/archive")).unwrap(),
            b"bound bytes"
        );
        assert!(!root.path().join(".nvm/.cache/node/archive").exists());
    }

    #[cfg(unix)]
    #[test]
    fn bound_nvm_cache_write_rejects_a_preexisting_symlinked_parent() {
        use super::ProcessOfflinePackageBackend;
        use std::fs::OpenOptions;
        use std::os::unix::fs::OpenOptionsExt;
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join(".nvm")).unwrap();
        symlink(outside.path(), root.path().join(".nvm/.cache")).unwrap();
        let handle = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(root.path())
            .unwrap();
        let backend =
            ProcessOfflinePackageBackend::new_with_bound_root(root.path(), handle).unwrap();

        assert!(
            backend
                .write_nvm_cache(".cache/node/archive", b"must stay bound")
                .is_err()
        );
        assert!(!outside.path().join("node/archive").exists());
    }
}
