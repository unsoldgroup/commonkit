use std::collections::BTreeSet;

use commonkit_contracts::{
    ContractError, Plan, PlanBindings, Sha256Digest, StableId, digest_domain_json,
};
use commonkit_core::{PlanBuildError, PlanDraft, build_plan};
use thiserror::Error;

use crate::{
    ArtifactError, ArtifactStore, DeclaredSideEffect, FileAdapter, FileAdapterError,
    FilesystemIntent, MaterializedState, NormalizedResource, OwnershipError, OwnershipRules,
    ProviderContractError, SshFileAdapter, SshFileAdapterError, SshFilesystemTransport,
    validate_ownership,
};

pub trait ProviderResourcePlanner {
    fn register_provider_resource(
        &mut self,
        id: StableId,
        resource: &NormalizedResource,
        provider_artifacts: &ArtifactStore,
    ) -> Result<Option<commonkit_contracts::Operation>, ProviderPlanError>;
}

impl ProviderResourcePlanner for FileAdapter {
    fn register_provider_resource(
        &mut self,
        id: StableId,
        resource: &NormalizedResource,
        provider_artifacts: &ArtifactStore,
    ) -> Result<Option<commonkit_contracts::Operation>, ProviderPlanError> {
        match self.register_materialized_resource(id, resource.intent.clone(), provider_artifacts) {
            Ok(operation) => Ok(Some(operation)),
            Err(FileAdapterError::NoChange) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }
}

impl<T: SshFilesystemTransport> ProviderResourcePlanner for SshFileAdapter<T> {
    fn register_provider_resource(
        &mut self,
        id: StableId,
        resource: &NormalizedResource,
        provider_artifacts: &ArtifactStore,
    ) -> Result<Option<commonkit_contracts::Operation>, ProviderPlanError> {
        self.register_materialized_resource(id, resource, provider_artifacts)
            .map_err(Into::into)
    }
}

pub struct ProviderPlanRequest<'a> {
    pub target_id: StableId,
    pub target_identity_digest: Sha256Digest,
    pub composed_loadout_digest: Sha256Digest,
    pub observed_digest: Sha256Digest,
    pub policy_digest: Sha256Digest,
    pub ownership_rules: &'a OwnershipRules,
    pub mapped_side_effects: BTreeSet<DeclaredSideEffect>,
}

/// Computes and validates the durable provider bindings without registering
/// operations or touching a managed target. This is used immediately before
/// apply to reject approvals whose provider authority has gone stale.
pub fn provider_plan_bindings(
    request: ProviderPlanRequest<'_>,
    states: &[MaterializedState],
    provider_artifacts: &ArtifactStore,
) -> Result<PlanBindings, ProviderPlanError> {
    struct AuthorityOnlyPlanner;
    impl ProviderResourcePlanner for AuthorityOnlyPlanner {
        fn register_provider_resource(
            &mut self,
            _id: StableId,
            _resource: &NormalizedResource,
            _provider_artifacts: &ArtifactStore,
        ) -> Result<Option<commonkit_contracts::Operation>, ProviderPlanError> {
            Ok(None)
        }
    }
    let mut planner = AuthorityOnlyPlanner;
    Ok(build_provider_plan(request, states, provider_artifacts, &mut planner)?.bindings)
}

pub fn build_provider_plan<P: ProviderResourcePlanner + ?Sized>(
    request: ProviderPlanRequest<'_>,
    states: &[MaterializedState],
    provider_artifacts: &ArtifactStore,
    files: &mut P,
) -> Result<Plan, ProviderPlanError> {
    let mut resources = Vec::new();
    let mut state_digests = Vec::new();
    let mut input_digests = Vec::new();
    let mut provider_ids = BTreeSet::new();
    for state in states {
        state.verify()?;
        if !provider_ids.insert(state.inputs.provider_id.as_str().to_owned()) {
            return Err(ProviderPlanError::DuplicateProvider(
                state.inputs.provider_id.clone(),
            ));
        }
        if let Some(unsupported) = state.unsupported.first() {
            return Err(ProviderPlanError::UnsupportedCapability {
                entry: unsupported.source.clone(),
                capability: unsupported.capability.clone(),
                remediation: unsupported.remediation.clone(),
            });
        }
        if let Some(effect) = state
            .declared_side_effects
            .iter()
            .find(|effect| !request.mapped_side_effects.contains(*effect))
        {
            return Err(ProviderPlanError::UnmappedSideEffect(effect.clone()));
        }
        state_digests.push(state.digest.clone());
        input_digests.push(state.inputs.digest().clone());
        resources.extend(state.resources.iter().cloned());
    }
    validate_ownership(&resources, request.ownership_rules)?;
    state_digests.sort();
    input_digests.sort();
    let desired_digest = digest_domain_json("commonkit.provider-desired-set.v1", &state_digests)?;
    let provider_inputs_digest =
        digest_domain_json("commonkit.provider-input-set.v1", &input_digests)?;
    let mut ordered: Vec<&NormalizedResource> = resources.iter().collect();
    ordered.sort_by(|left, right| {
        (
            left.intent.path().as_str(),
            left.provenance.provider_id.as_str(),
            left.provenance.source.as_str(),
        )
            .cmp(&(
                right.intent.path().as_str(),
                right.provenance.provider_id.as_str(),
                right.provenance.source.as_str(),
            ))
    });
    let ownership_map_digest = digest_domain_json("commonkit.ownership-map.v1", &ordered)?;
    let mut artifact_references = ordered
        .iter()
        .filter_map(|resource| match &resource.intent {
            FilesystemIntent::File { content, .. } => Some(content.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    artifact_references.sort_by(|left, right| left.digest.cmp(&right.digest));
    artifact_references.dedup();
    for reference in &artifact_references {
        provider_artifacts.load(reference)?;
    }
    let artifact_set_digest =
        digest_domain_json("commonkit.provider-artifact-set.v1", &artifact_references)?;

    let mut operations = Vec::new();
    for resource in ordered {
        if let Some(operation) = files.register_provider_resource(
            resource_id(resource)?,
            resource,
            provider_artifacts,
        )? {
            operations.push(operation);
        }
    }
    build_plan(PlanDraft {
        target_id: request.target_id,
        desired_digest,
        observed_digest: request.observed_digest,
        policy_digest: request.policy_digest,
        bindings: PlanBindings {
            target_identity_digest: request.target_identity_digest,
            composed_loadout_digest: request.composed_loadout_digest,
            provider_inputs_digest,
            ownership_map_digest,
            artifact_set_digest,
        },
        operations,
    })
    .map_err(ProviderPlanError::Plan)
}

fn resource_id(resource: &NormalizedResource) -> Result<StableId, ProviderPlanError> {
    let digest = digest_domain_json(
        "commonkit.provider-resource-id.v1",
        &(
            &resource.provenance.provider_id,
            resource.intent.path(),
            &resource.provenance.source,
        ),
    )?;
    StableId::parse(format!("resource-{}", &digest.as_str()[7..61]))
        .map_err(ProviderPlanError::Contract)
}

#[derive(Debug, Error)]
pub enum ProviderPlanError {
    #[error("provider appears more than once in the materialized set: {0}")]
    DuplicateProvider(StableId),
    #[error("unsupported provider capability {capability} at {entry}: {remediation}")]
    UnsupportedCapability {
        entry: String,
        capability: String,
        remediation: String,
    },
    #[error("provider side effect has no typed adapter mapping: {0:?}")]
    UnmappedSideEffect(DeclaredSideEffect),
    #[error(transparent)]
    Provider(#[from] ProviderContractError),
    #[error(transparent)]
    Ownership(#[from] OwnershipError),
    #[error(transparent)]
    Artifact(#[from] ArtifactError),
    #[error(transparent)]
    Adapter(#[from] FileAdapterError),
    #[error(transparent)]
    RemoteAdapter(#[from] SshFileAdapterError),
    #[error(transparent)]
    Plan(#[from] PlanBuildError),
    #[error(transparent)]
    Contract(#[from] ContractError),
}
