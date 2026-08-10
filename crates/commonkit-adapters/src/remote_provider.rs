use commonkit_contracts::{Sha256Digest, StableId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;

use crate::{
    ArtifactError, ArtifactStore, ContentSensitivity, DesiredStateProvider, MaterializedState,
    ProviderContext, ProviderFailure, ProviderWorkspace, SshFilesystemRequest,
    SshFilesystemResponse, SshFilesystemTransport, TargetFilesystemError,
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
        self.validate_before_remote_contact(state)?;

        let references = state
            .resources
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
            provider_id: state.inputs.provider_id.clone(),
            provider_version: state.inputs.provider_version.to_string(),
            provider_inputs_digest: state.inputs.input_set_digest.clone(),
            materialized_state_digest: state.digest.clone(),
            artifact_digests,
        })
    }

    fn validate_before_remote_contact(
        &self,
        state: &MaterializedState,
    ) -> Result<(), RemoteProviderStagingError> {
        if !state.unsupported.is_empty() {
            return Err(RemoteProviderStagingError::UnsupportedOutput);
        }
        if !state.declared_side_effects.is_empty() {
            return Err(RemoteProviderStagingError::UnplannedSideEffects);
        }
        if state.resources.iter().any(|resource| {
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
    Target(#[from] TargetFilesystemError),
}
