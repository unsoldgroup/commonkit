use commonkit_contracts::{Sha256Digest, StableId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;

use crate::{
    ArtifactError, ArtifactStore, ContentSensitivity, DesiredStateProvider, MaterializedState,
    ProviderContext, ProviderFailure, ProviderWorkspace, ResolvedMaterializedState,
    SshFilesystemRequest, SshFilesystemResponse, SshFilesystemTransport, TargetFilesystemError,
};

/// Durable provenance for provider artifacts copied to a remote target's private store.
/// Provider code always runs against controller-owned isolated staging; only immutable bytes cross
/// the target boundary, and live mutation remains the operation adapters' responsibility.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteMaterializationReceipt {
    pub target_id: StableId,
    pub provider_id: StableId,
    pub provider_version: String,
    pub provider_inputs_digest: Sha256Digest,
    pub materialized_state_digest: Sha256Digest,
    pub artifact_digests: Vec<Sha256Digest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_resolution_authority_digest: Option<Sha256Digest>,
}

pub struct RemoteProviderStager<'a, T> {
    transport: &'a mut T,
}

impl<'a, T: SshFilesystemTransport> RemoteProviderStager<'a, T> {
    pub fn new(transport: &'a mut T) -> Self {
        Self { transport }
    }

    pub fn materialize_and_stage<P: DesiredStateProvider + ?Sized>(
        &mut self,
        provider: &P,
        context: &ProviderContext,
        workspace: &ProviderWorkspace,
        artifacts: &ArtifactStore,
        run_id: StableId,
    ) -> Result<RemoteMaterializationReceipt, RemoteProviderStagingError> {
        let state = provider.materialize(context, workspace, artifacts)?;
        self.stage_materialized(&state, artifacts, context.target_id.clone(), run_id)
    }

    /// Stages an already materialized and ownership-validated provider result. This keeps SSH
    /// target selection on the same controller pipeline and never re-runs provider resolution.
    pub fn stage_materialized(
        &mut self,
        state: &MaterializedState,
        artifacts: &ArtifactStore,
        target_id: StableId,
        run_id: StableId,
    ) -> Result<RemoteMaterializationReceipt, RemoteProviderStagingError> {
        state.verify()?;
        self.stage_state(state, None, artifacts, target_id, run_id)
    }

    pub fn stage_resolved_materialized(
        &mut self,
        state: &ResolvedMaterializedState,
        package_authority: &crate::PackageResolutionAuthority,
        artifacts: &ArtifactStore,
        target_id: StableId,
        run_id: StableId,
    ) -> Result<RemoteMaterializationReceipt, RemoteProviderStagingError> {
        state.verify()?;
        for resource in &state.resources {
            if let crate::ResourceIntent::ResolvedPackage(intent) = &resource.intent {
                package_authority.validate(intent, artifacts)?;
            }
        }
        self.stage_state(
            state,
            Some(package_authority.digest().clone()),
            artifacts,
            target_id,
            run_id,
        )
    }

    fn stage_state<S: RemoteStagingState>(
        &mut self,
        state: &S,
        package_resolution_authority_digest: Option<Sha256Digest>,
        artifacts: &ArtifactStore,
        target_id: StableId,
        run_id: StableId,
    ) -> Result<RemoteMaterializationReceipt, RemoteProviderStagingError> {
        self.validate_before_remote_contact(state)?;

        let references = state
            .resources()
            .iter()
            .flat_map(|resource| resource.artifact_references())
            .collect::<Vec<_>>();
        let mut metadata_by_digest = BTreeMap::new();
        let mut content_by_digest = BTreeMap::new();
        for reference in references {
            if let Some(existing) = metadata_by_digest.get(&reference.digest)
                && *existing != reference
            {
                return Err(RemoteProviderStagingError::ConflictingArtifactReference {
                    digest: reference.digest.clone(),
                });
            }
            let content = artifacts.load(reference)?;
            metadata_by_digest.insert(reference.digest.clone(), reference);
            content_by_digest
                .entry(reference.digest.clone())
                .or_insert(content);
        }

        let mut artifact_digests = Vec::with_capacity(content_by_digest.len());
        for (digest, content) in content_by_digest {
            let staged = self
                .transport
                .perform(SshFilesystemRequest::StageArtifact {
                    run_id: run_id.clone(),
                    digest: digest.clone(),
                    content,
                })?;
            if staged
                != (SshFilesystemResponse::ArtifactStaged {
                    digest: digest.clone(),
                })
            {
                return Err(RemoteProviderStagingError::UnexpectedResponse);
            }
            let verified = self
                .transport
                .perform(SshFilesystemRequest::VerifyArtifact {
                    run_id: run_id.clone(),
                    digest: digest.clone(),
                })?;
            if verified
                != (SshFilesystemResponse::ArtifactVerified {
                    digest: digest.clone(),
                })
            {
                return Err(RemoteProviderStagingError::UnexpectedResponse);
            }
            artifact_digests.push(digest);
        }

        Ok(RemoteMaterializationReceipt {
            target_id,
            provider_id: state.inputs().provider_id.clone(),
            provider_version: state.inputs().provider_version.to_string(),
            provider_inputs_digest: state.inputs().input_set_digest.clone(),
            materialized_state_digest: state.digest().clone(),
            artifact_digests,
            package_resolution_authority_digest,
        })
    }

    fn validate_before_remote_contact<S: RemoteStagingState>(
        &self,
        state: &S,
    ) -> Result<(), RemoteProviderStagingError> {
        if !state.unsupported().is_empty() {
            return Err(RemoteProviderStagingError::UnsupportedOutput);
        }
        if !state.declared_side_effects().is_empty() {
            return Err(RemoteProviderStagingError::UnplannedSideEffects);
        }
        if state.resources().iter().any(|resource| {
            resource
                .artifact_references()
                .into_iter()
                .any(|content| content.sensitivity != ContentSensitivity::Portable)
        }) {
            return Err(RemoteProviderStagingError::SensitiveArtifact);
        }
        Ok(())
    }
}

trait RemoteStagingState {
    fn inputs(&self) -> &crate::ProviderInputs;
    fn resources(&self) -> &[crate::NormalizedResource];
    fn declared_side_effects(&self) -> &[crate::DeclaredSideEffect];
    fn unsupported(&self) -> &[crate::UnsupportedCapability];
    fn digest(&self) -> &Sha256Digest;
}

impl RemoteStagingState for MaterializedState {
    fn inputs(&self) -> &crate::ProviderInputs {
        &self.inputs
    }
    fn resources(&self) -> &[crate::NormalizedResource] {
        &self.resources
    }
    fn declared_side_effects(&self) -> &[crate::DeclaredSideEffect] {
        &self.declared_side_effects
    }
    fn unsupported(&self) -> &[crate::UnsupportedCapability] {
        &self.unsupported
    }
    fn digest(&self) -> &Sha256Digest {
        &self.digest
    }
}

impl RemoteStagingState for ResolvedMaterializedState {
    fn inputs(&self) -> &crate::ProviderInputs {
        &self.inputs
    }
    fn resources(&self) -> &[crate::NormalizedResource] {
        &self.resources
    }
    fn declared_side_effects(&self) -> &[crate::DeclaredSideEffect] {
        &self.declared_side_effects
    }
    fn unsupported(&self) -> &[crate::UnsupportedCapability] {
        &self.unsupported
    }
    fn digest(&self) -> &Sha256Digest {
        &self.digest
    }
}

#[derive(Debug, Error)]
pub enum RemoteProviderStagingError {
    #[error("provider output contains unsupported capabilities; no remote artifacts were staged")]
    UnsupportedOutput,
    #[error("artifact digest has conflicting immutable reference metadata: {digest}")]
    ConflictingArtifactReference { digest: Sha256Digest },
    #[error(
        "provider output declares side effects that have not been normalized into CommonKit operations"
    )]
    UnplannedSideEffects,
    #[error("target-local sensitive provider artifacts cannot be copied to a remote target")]
    SensitiveArtifact,
    #[error("remote target returned an unexpected artifact response")]
    UnexpectedResponse,
    #[error(transparent)]
    Provider(#[from] ProviderFailure),
    #[error(transparent)]
    ProviderContract(#[from] crate::ProviderContractError),
    #[error(transparent)]
    Artifact(#[from] ArtifactError),
    #[error(transparent)]
    PackageResolution(#[from] crate::PackageResolutionError),
    #[error(transparent)]
    Target(#[from] TargetFilesystemError),
}
