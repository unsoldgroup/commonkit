//! Offline, forward-only mutation of controller-resolved package plans.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

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
    ArtifactStore, NodeOfflineInstallRecipeV1, OfflineInstallRecipeV1, PackageObservationV1,
    PackageResolutionAuthority, PackageResolutionV1, PackageResourcePlanner, ProviderPlanError,
    ResolvedPackageIntent, ResourceProvenance, apt_resolution::apt_package_identity,
};

const CLOSED_NVM_PATH: &str = "/usr/bin:/bin";

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
        Ok(
            if observed.installed_versions == resolution.before.installed_versions {
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
    let registry =
        crate::PackageSourceRegistry::builtin().map_err(|_| PackageMutationError::Backend)?;
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
    target_root: PathBuf,
}

/// SSH counterpart to the native offline backend. It uses only the typed
/// package request in the closed filesystem protocol; arbitrary remote shell
/// commands are never exposed to the controller.
pub struct SshOfflinePackageBackend<T> {
    root_id: StableId,
    transport: T,
    target_platform: Option<(String, String)>,
}

impl<T> SshOfflinePackageBackend<T> {
    pub fn new(root_id: StableId, transport: T) -> Self {
        Self {
            root_id,
            transport,
            target_platform: None,
        }
    }

    pub fn with_target_platform(
        root_id: StableId,
        transport: T,
        operating_system: impl Into<String>,
        architecture: impl Into<String>,
    ) -> Self {
        Self {
            root_id,
            transport,
            target_platform: Some((operating_system.into(), architecture.into())),
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
        let _ = artifacts;
        let transferred = resolution
            .artifacts
            .iter()
            .map(|artifact| crate::PackageMutationArtifact {
                reference: artifact.content.clone(),
            })
            .collect::<Vec<_>>();
        self.transport
            .perform(crate::SshFilesystemRequest::PackageMutation {
                root_id: self.root_id.clone(),
                phase,
                resolution: resolution.clone(),
                artifacts: transferred,
            })
            .map_err(|_| PackageMutationError::Backend)
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
        Self {
            target_root: target_root.into(),
        }
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
        let nvm_dir = self.target_root.join(".nvm");
        let script = self.validate_nvm_script(resolution)?;
        let script_copy = script.materialize()?;
        let output = Command::new("/bin/bash")
            .arg("-c")
            .arg("set -eu; . \"$1\"; nvm ls --no-colors")
            .arg("commonkit")
            .arg(script_copy.path())
            .env_clear()
            .env("HOME", &self.target_root)
            .env("NVM_DIR", &nvm_dir)
            .env("PATH", CLOSED_NVM_PATH)
            .env("NVM_NO_SOURCE_FALLBACK", "1")
            .env("NVM_OFFLINE", "1")
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
        let script = self.target_root.join(".nvm/nvm.sh");
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
        let expected = self.target_root.join(".nvm");
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
                let cache = self
                    .target_root
                    .join(".nvm")
                    .join(&install.cache_relative_path);
                if let Some(parent) = cache.parent() {
                    fs::create_dir_all(parent).map_err(|_| PackageMutationError::Backend)?;
                }
                fs::write(&cache, archive).map_err(|_| PackageMutationError::Backend)?;
                let status = Command::new("/bin/bash")
                    .arg("-c")
                    .arg("set -eu; export NVM_NO_SOURCE_FALLBACK=1 NVM_OFFLINE=1; . \"$1\"; nvm install --offline \"$2\"")
                    .arg("commonkit")
                    .arg(script_copy.path())
                    .arg(&install.node_version)
                    .env_clear()
                    .env("HOME", &self.target_root)
                    .env("NVM_DIR", self.target_root.join(".nvm"))
                    .env("PATH", CLOSED_NVM_PATH)
                    .env("NVM_NO_SOURCE_FALLBACK", "1")
                    .env("NVM_OFFLINE", "1")
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
        let observed = self.observe(resolution)?;
        if PackageAdapter::after_resolution(resolution, &observed) {
            Ok(())
        } else {
            Err(PackageMutationError::Backend)
        }
    }
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
    let os_matches = match manager {
        PackageManager::Apt => operating_system.eq_ignore_ascii_case("linux"),
        PackageManager::Nvm => {
            operating_system.eq_ignore_ascii_case("linux")
                || operating_system.eq_ignore_ascii_case("macos")
                || operating_system.eq_ignore_ascii_case("darwin")
        }
        _ => false,
    };
    os_matches
        && target.os.eq_ignore_ascii_case(operating_system)
        && canonical_arch(&target.arch) == canonical_arch(architecture)
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
    use super::ValidatedNvmScript;
    use std::fs;

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
}
