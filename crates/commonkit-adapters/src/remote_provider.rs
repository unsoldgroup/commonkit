use commonkit_contracts::{Sha256Digest, StableId};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    ArtifactError, ArtifactStore, ContentSensitivity, DesiredStateProvider, FilesystemIntent,
    MaterializedState, ProviderContext, ProviderFailure, ProviderWorkspace, SshFilesystemRequest,
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
        state.verify()?;
        self.validate_before_remote_contact(&state)?;

        let mut references = state
            .resources
            .iter()
            .filter_map(|resource| match &resource.intent {
                FilesystemIntent::File { content, .. } => Some(content),
                _ => None,
            })
            .collect::<Vec<_>>();
        references.sort_by(|left, right| left.digest.cmp(&right.digest));
        references.dedup_by(|left, right| left.digest == right.digest);

        let mut artifact_digests = Vec::with_capacity(references.len());
        for reference in references {
            let content = artifacts.load(reference)?;
            let digest = reference.digest.clone();
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
            target_id: context.target_id.clone(),
            provider_id: state.inputs.provider_id,
            provider_version: state.inputs.provider_version.to_string(),
            provider_inputs_digest: state.inputs.input_set_digest,
            materialized_state_digest: state.digest,
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
            matches!(
                &resource.intent,
                FilesystemIntent::File { content, .. }
                    if content.sensitivity != ContentSensitivity::Portable
            )
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
