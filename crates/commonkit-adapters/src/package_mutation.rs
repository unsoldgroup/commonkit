//! Offline, forward-only mutation of controller-resolved package plans.

use commonkit_contracts::{
    Operation, OperationKind, PackageExitClassification, PackageManager,
    PackageOperationConsentBinding, PackageReceiptEvidence, PackageSelector, RecoveryCapability,
    ResourceRef, Risk, Sha256Digest, StableId, digest_domain_json,
};
use commonkit_core::{OperationDraft, finalize_operation};
use commonkit_reconcile::{Adapter, AdapterFailure, RecoveryObservation};
use thiserror::Error;

use crate::{
    ArtifactStore, NodeOfflineInstallRecipeV1, OfflineInstallRecipeV1, PackageObservationV1,
    PackageResolutionAuthority, PackageResolutionV1, PackageResourcePlanner, ProviderPlanError,
    ResolvedPackageIntent, ResourceProvenance,
};

/// The only target-side capability exposed to package mutation.
///
/// Implementations receive a validated, persisted resolution and the artifact
/// store that contains its exact bytes. They never receive a resolver or fetch
/// capability, which makes applying and recovering a durable plan offline.
pub trait PackageMutationBackend: Send {
    fn manager(&self) -> PackageManager;

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
    authority: PackageResolutionAuthority,
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
            authority,
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
            authority,
            artifacts,
            backend,
        }
    }

    fn resolution(&self, operation: &Operation) -> Result<PackageResolutionV1, AdapterFailure> {
        if operation.adapter_id != self.id {
            return Err(failure("package_adapter_mismatch"));
        }
        let (_, resolution) = self
            .authority
            .load_by_resolution_digest(&operation.payload_digest, &self.artifacts)
            .map_err(|_| failure("package_resolution_invalid"))?;
        ensure_supported(&resolution, self.backend.manager())?;
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
        let resolution = self.authority.validate(intent, provider_artifacts)?;
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
            .authority
            .load_by_resolution_digest(&operation.payload_digest, &self.artifacts)
            .map_err(|_| failure("package_resolution_invalid"))?;
        ensure_supported(&resolution, self.backend.manager())?;
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
                target_authority_digest: self.authority.digest().clone(),
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
    backend_manager: PackageManager,
) -> Result<(), AdapterFailure> {
    if resolution.manager.manager != backend_manager {
        return Err(failure("package_manager_mismatch"));
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

fn package_version_key(
    package: &crate::ResolvedPackage,
    resolution: &PackageResolutionV1,
) -> Option<String> {
    match resolution.manager.manager {
        PackageManager::Apt => {
            let Some(PackageSelector::AptBinary { architecture, .. }) =
                package.declaration.selector.as_ref()
            else {
                return None;
            };
            Some(format!(
                "{}:{}={}",
                package.declaration.id.as_str(),
                architecture.as_deref().unwrap_or("native"),
                package.declaration.version
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
