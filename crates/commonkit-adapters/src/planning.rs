use std::collections::BTreeSet;

use commonkit_contracts::{
    ContractError, Plan, PlanBindings, Sha256Digest, StableId, digest_domain_json,
};
use commonkit_core::{PlanBuildError, PlanDraft, build_plan};
use thiserror::Error;

use crate::{
    ArtifactError, ArtifactStore, DeclaredSideEffect, FileAdapter, FileAdapterError,
    MaterializedState, NormalizedResource, OwnershipError, OwnershipRules, ProviderContractError,
    ResolvedMaterializedState, ResolvedPackageIntent, ResourceProvenance, ResourceType,
    SshFileAdapter, SshFileAdapterError, SshFilesystemTransport, UnsupportedCapability,
    validate_ownership,
};
use commonkit_reconcile::Adapter;

pub trait ProviderResourcePlanner {
    fn adapter_id(&self) -> &StableId;
    fn route(&self) -> FilesystemPlannerRoute;

    fn register_provider_resource(
        &mut self,
        id: StableId,
        resource: &NormalizedResource,
        provider_artifacts: &ArtifactStore,
    ) -> Result<Option<commonkit_contracts::Operation>, ProviderPlanError>;
}

impl ProviderResourcePlanner for FileAdapter {
    fn adapter_id(&self) -> &StableId {
        Adapter::id(self)
    }

    fn route(&self) -> FilesystemPlannerRoute {
        FilesystemPlannerRoute::Local
    }

    fn register_provider_resource(
        &mut self,
        id: StableId,
        resource: &NormalizedResource,
        provider_artifacts: &ArtifactStore,
    ) -> Result<Option<commonkit_contracts::Operation>, ProviderPlanError> {
        match self.register_materialized_provider_resource(id, resource, provider_artifacts) {
            Ok(operation) => Ok(Some(operation)),
            Err(FileAdapterError::NoChange) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }
}

impl<T: SshFilesystemTransport + Send> ProviderResourcePlanner for SshFileAdapter<T> {
    fn adapter_id(&self) -> &StableId {
        Adapter::id(self)
    }

    fn route(&self) -> FilesystemPlannerRoute {
        FilesystemPlannerRoute::Ssh(self.target_capabilities())
    }

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilesystemPlannerRoute {
    Local,
    Ssh(crate::SshTargetCapabilities),
}

pub trait PackageResourcePlanner {
    fn adapter_id(&self) -> &StableId;

    fn register_package_resource(
        &mut self,
        id: StableId,
        intent: &ResolvedPackageIntent,
        provenance: &ResourceProvenance,
        provider_artifacts: &ArtifactStore,
    ) -> Result<Option<commonkit_contracts::Operation>, ProviderPlanError>;
}

pub enum ProviderPlannerRoute<'a> {
    Filesystem(&'a mut dyn ProviderResourcePlanner),
    Package(&'a mut dyn PackageResourcePlanner),
}

pub struct ProviderResourceRouter<'a> {
    filesystem: Option<&'a mut dyn ProviderResourcePlanner>,
    filesystem_route: Option<FilesystemPlannerRoute>,
    package: Option<&'a mut dyn PackageResourcePlanner>,
}

impl<'a> ProviderResourceRouter<'a> {
    pub fn new(routes: Vec<ProviderPlannerRoute<'a>>) -> Result<Self, ProviderPlanError> {
        let mut router = Self {
            filesystem: None,
            filesystem_route: None,
            package: None,
        };
        for route in routes {
            match route {
                ProviderPlannerRoute::Filesystem(planner) => {
                    let route = planner.route();
                    let valid_route = matches!(
                        (planner.adapter_id().as_str(), route),
                        ("files", FilesystemPlannerRoute::Local)
                            | ("ssh-files", FilesystemPlannerRoute::Ssh(_))
                    );
                    if !valid_route {
                        return Err(ProviderPlanError::InvalidResourcePlanner {
                            resource_type: ResourceType::Filesystem,
                            adapter_id: planner.adapter_id().clone(),
                        });
                    }
                    if router.filesystem.replace(planner).is_some() {
                        return Err(ProviderPlanError::DuplicateResourcePlanner(
                            ResourceType::Filesystem,
                        ));
                    }
                    router.filesystem_route = Some(route);
                }
                ProviderPlannerRoute::Package(planner) => {
                    if planner.adapter_id().as_str() != "packages" {
                        return Err(ProviderPlanError::InvalidResourcePlanner {
                            resource_type: ResourceType::Package,
                            adapter_id: planner.adapter_id().clone(),
                        });
                    }
                    if router.package.replace(planner).is_some() {
                        return Err(ProviderPlanError::DuplicateResourcePlanner(
                            ResourceType::Package,
                        ));
                    }
                }
            }
        }
        Ok(router)
    }

    fn preflight(&self, resources: &[&NormalizedResource]) -> Result<(), ProviderPlanError> {
        for resource in resources {
            let missing = match resource.resource_type() {
                ResourceType::Filesystem => self.filesystem.is_none(),
                ResourceType::Package => self.package.is_none(),
            };
            if missing {
                return Err(ProviderPlanError::MissingResourcePlanner(
                    resource.resource_type(),
                ));
            }
        }
        Ok(())
    }

    fn register(
        &mut self,
        id: StableId,
        resource: &NormalizedResource,
        provider_artifacts: &ArtifactStore,
    ) -> Result<Option<commonkit_contracts::Operation>, ProviderPlanError> {
        let (declared_adapter, filesystem_route, operation) = match &resource.intent {
            crate::ResourceIntent::Filesystem(_) => {
                let planner = self.filesystem.as_deref_mut().ok_or(
                    ProviderPlanError::MissingResourcePlanner(ResourceType::Filesystem),
                )?;
                let adapter_id = planner.adapter_id().clone();
                let route = self
                    .filesystem_route
                    .expect("installed filesystem planner has a captured route");
                let operation =
                    planner.register_provider_resource(id.clone(), resource, provider_artifacts)?;
                (adapter_id, Some(route), operation)
            }
            crate::ResourceIntent::Package(intent) => {
                let _ = intent;
                return Err(ProviderPlanError::PackageResolutionRequired);
            }
            crate::ResourceIntent::ResolvedPackage(intent) => {
                let planner = self.package.as_deref_mut().ok_or(
                    ProviderPlanError::MissingResourcePlanner(ResourceType::Package),
                )?;
                let adapter_id = planner.adapter_id().clone();
                let operation = planner.register_package_resource(
                    id.clone(),
                    intent,
                    &resource.provenance,
                    provider_artifacts,
                )?;
                (adapter_id, None, operation)
            }
        };
        if let Some(operation) = &operation
            && operation.adapter_id != declared_adapter
        {
            return Err(ProviderPlanError::UnexpectedAdapterRoute {
                resource_type: resource.resource_type(),
                expected: declared_adapter,
                actual: operation.adapter_id.clone(),
            });
        }
        if let Some(operation) = &operation {
            validate_operation_binding(
                id,
                resource,
                filesystem_route,
                operation,
                provider_artifacts,
            )?;
        }
        Ok(operation)
    }
}

fn validate_operation_binding(
    id: StableId,
    resource: &NormalizedResource,
    filesystem_route: Option<FilesystemPlannerRoute>,
    operation: &commonkit_contracts::Operation,
    provider_artifacts: &ArtifactStore,
) -> Result<(), ProviderPlanError> {
    let (resource_type, managed_path, after_digest, payload_digest) = match &resource.intent {
        crate::ResourceIntent::Filesystem(intent) => {
            let (resource_type, after_digest, payload_digest) = match filesystem_route
                .expect("filesystem route was captured before registration")
            {
                FilesystemPlannerRoute::Local => crate::files::provider_operation_binding(intent)?,
                FilesystemPlannerRoute::Ssh(capabilities) => {
                    crate::ssh_files::provider_operation_binding(intent, capabilities)?
                }
            };
            (
                resource_type,
                Some(intent.path().as_str().to_owned()),
                after_digest,
                payload_digest,
            )
        }
        crate::ResourceIntent::Package(_) => {
            return Err(ProviderPlanError::PackageResolutionRequired);
        }
        crate::ResourceIntent::ResolvedPackage(intent) => {
            intent.load_persisted(provider_artifacts)?;
            let desired_digest = resource.desired_digest()?;
            (
                StableId::parse("package").expect("static resource type"),
                None,
                Some(desired_digest.clone()),
                desired_digest,
            )
        }
    };
    let valid = operation.resource.resource_id == id
        && operation.resource.resource_type == resource_type
        && operation.resource.managed_path == managed_path
        && operation.provenance.as_ref() == Some(&resource.provenance)
        && operation.after_digest == after_digest
        && operation.payload_digest == payload_digest
        && operation.recovery_capability == resource.recovery_capability();
    if !valid {
        return Err(ProviderPlanError::UnexpectedResourceBinding {
            resource_type: resource.resource_type(),
            resource_id: id,
        });
    }
    Ok(())
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
    authority_plan_bindings(request, states, provider_artifacts)
}

pub fn resolved_provider_plan_bindings(
    request: ProviderPlanRequest<'_>,
    states: &[ResolvedMaterializedState],
    provider_artifacts: &ArtifactStore,
) -> Result<PlanBindings, ProviderPlanError> {
    authority_plan_bindings(request, states, provider_artifacts)
}

fn authority_plan_bindings<S: ProviderPlanningState>(
    request: ProviderPlanRequest<'_>,
    states: &[S],
    provider_artifacts: &ArtifactStore,
) -> Result<PlanBindings, ProviderPlanError> {
    struct AuthorityOnlyFilesystemPlanner {
        id: StableId,
    }
    impl ProviderResourcePlanner for AuthorityOnlyFilesystemPlanner {
        fn adapter_id(&self) -> &StableId {
            &self.id
        }

        fn route(&self) -> FilesystemPlannerRoute {
            FilesystemPlannerRoute::Local
        }

        fn register_provider_resource(
            &mut self,
            _id: StableId,
            _resource: &NormalizedResource,
            _provider_artifacts: &ArtifactStore,
        ) -> Result<Option<commonkit_contracts::Operation>, ProviderPlanError> {
            Ok(None)
        }
    }
    struct AuthorityOnlyPackagePlanner {
        id: StableId,
    }
    impl PackageResourcePlanner for AuthorityOnlyPackagePlanner {
        fn adapter_id(&self) -> &StableId {
            &self.id
        }

        fn register_package_resource(
            &mut self,
            _id: StableId,
            _intent: &ResolvedPackageIntent,
            _provenance: &ResourceProvenance,
            _provider_artifacts: &ArtifactStore,
        ) -> Result<Option<commonkit_contracts::Operation>, ProviderPlanError> {
            Ok(None)
        }
    }
    let mut filesystem = AuthorityOnlyFilesystemPlanner {
        id: StableId::parse("files").expect("static adapter ID"),
    };
    let mut package = AuthorityOnlyPackagePlanner {
        id: StableId::parse("packages").expect("static adapter ID"),
    };
    let mut router = ProviderResourceRouter::new(vec![
        ProviderPlannerRoute::Filesystem(&mut filesystem),
        ProviderPlannerRoute::Package(&mut package),
    ])?;
    Ok(build_provider_plan_from_states(request, states, provider_artifacts, &mut router)?.bindings)
}

pub fn build_provider_plan<P: ProviderResourcePlanner>(
    request: ProviderPlanRequest<'_>,
    states: &[MaterializedState],
    provider_artifacts: &ArtifactStore,
    files: &mut P,
) -> Result<Plan, ProviderPlanError> {
    let mut router = ProviderResourceRouter::new(vec![ProviderPlannerRoute::Filesystem(files)])?;
    build_provider_plan_with_router(request, states, provider_artifacts, &mut router)
}

pub fn build_provider_plan_with_router(
    request: ProviderPlanRequest<'_>,
    states: &[MaterializedState],
    provider_artifacts: &ArtifactStore,
    router: &mut ProviderResourceRouter<'_>,
) -> Result<Plan, ProviderPlanError> {
    build_provider_plan_from_states(request, states, provider_artifacts, router)
}

pub fn build_resolved_provider_plan_with_router(
    request: ProviderPlanRequest<'_>,
    states: &[ResolvedMaterializedState],
    provider_artifacts: &ArtifactStore,
    router: &mut ProviderResourceRouter<'_>,
) -> Result<Plan, ProviderPlanError> {
    build_provider_plan_from_states(request, states, provider_artifacts, router)
}

trait ProviderPlanningState {
    fn verify_state(&self) -> Result<(), ProviderContractError>;
    fn inputs(&self) -> &crate::ProviderInputs;
    fn resources(&self) -> &[NormalizedResource];
    fn declared_side_effects(&self) -> &[DeclaredSideEffect];
    fn unsupported(&self) -> &[UnsupportedCapability];
    fn digest(&self) -> &Sha256Digest;
}

impl ProviderPlanningState for MaterializedState {
    fn verify_state(&self) -> Result<(), ProviderContractError> {
        self.verify()
    }
    fn inputs(&self) -> &crate::ProviderInputs {
        &self.inputs
    }
    fn resources(&self) -> &[NormalizedResource] {
        &self.resources
    }
    fn declared_side_effects(&self) -> &[DeclaredSideEffect] {
        &self.declared_side_effects
    }
    fn unsupported(&self) -> &[UnsupportedCapability] {
        &self.unsupported
    }
    fn digest(&self) -> &Sha256Digest {
        &self.digest
    }
}

impl ProviderPlanningState for ResolvedMaterializedState {
    fn verify_state(&self) -> Result<(), ProviderContractError> {
        self.verify()
    }
    fn inputs(&self) -> &crate::ProviderInputs {
        &self.inputs
    }
    fn resources(&self) -> &[NormalizedResource] {
        &self.resources
    }
    fn declared_side_effects(&self) -> &[DeclaredSideEffect] {
        &self.declared_side_effects
    }
    fn unsupported(&self) -> &[UnsupportedCapability] {
        &self.unsupported
    }
    fn digest(&self) -> &Sha256Digest {
        &self.digest
    }
}

fn build_provider_plan_from_states<S: ProviderPlanningState>(
    request: ProviderPlanRequest<'_>,
    states: &[S],
    provider_artifacts: &ArtifactStore,
    router: &mut ProviderResourceRouter<'_>,
) -> Result<Plan, ProviderPlanError> {
    let mut resources = Vec::new();
    let mut state_digests = Vec::new();
    let mut input_digests = Vec::new();
    let mut provider_ids = BTreeSet::new();
    for state in states {
        state.verify_state()?;
        if !provider_ids.insert(state.inputs().provider_id.as_str().to_owned()) {
            return Err(ProviderPlanError::DuplicateProvider(
                state.inputs().provider_id.clone(),
            ));
        }
        if let Some(unsupported) = state.unsupported().first() {
            return Err(ProviderPlanError::UnsupportedCapability {
                entry: unsupported.source.clone(),
                capability: unsupported.capability.clone(),
                remediation: unsupported.remediation.clone(),
            });
        }
        if let Some(effect) = state
            .declared_side_effects()
            .iter()
            .find(|effect| !request.mapped_side_effects.contains(*effect))
        {
            return Err(ProviderPlanError::UnmappedSideEffect(effect.clone()));
        }
        state_digests.push(state.digest().clone());
        input_digests.push(state.inputs().digest().clone());
        resources.extend(state.resources().iter().cloned());
    }
    validate_ownership(&resources, request.ownership_rules)?;
    let filesystem_only = resources
        .iter()
        .all(|resource| resource.resource_type() == ResourceType::Filesystem);
    state_digests.sort();
    input_digests.sort();
    let desired_digest = digest_domain_json(
        if filesystem_only {
            "commonkit.provider-desired-set.v1"
        } else {
            "commonkit.provider-desired-set.v2"
        },
        &state_digests,
    )?;
    let provider_inputs_digest =
        digest_domain_json("commonkit.provider-input-set.v1", &input_digests)?;
    let mut ordered: Vec<&NormalizedResource> = resources.iter().collect();
    ordered.sort_by(|left, right| {
        (
            left.sort_key(),
            left.provenance.provider_id.as_str(),
            left.provenance.source.as_str(),
        )
            .cmp(&(
                right.sort_key(),
                right.provenance.provider_id.as_str(),
                right.provenance.source.as_str(),
            ))
    });
    for resource in &ordered {
        match &resource.intent {
            crate::ResourceIntent::Package(_) => {
                return Err(ProviderPlanError::PackageResolutionRequired);
            }
            crate::ResourceIntent::ResolvedPackage(intent) => {
                intent.load_persisted(provider_artifacts)?;
            }
            crate::ResourceIntent::Filesystem(_) => {}
        }
    }
    let resource_map_digest = digest_domain_json(
        if filesystem_only {
            "commonkit.ownership-map.v1"
        } else {
            "commonkit.ownership-map.v2"
        },
        &ordered,
    )?;
    let ownership_map_digest = digest_domain_json(
        if filesystem_only {
            "commonkit.ownership-map.v2"
        } else {
            "commonkit.ownership-map.v3"
        },
        &(
            resource_map_digest,
            request.ownership_rules.authority_digest()?,
        ),
    )?;
    let mut artifact_references = ordered
        .iter()
        .flat_map(|resource| resource.artifact_references().into_iter().cloned())
        .collect::<Vec<_>>();
    router.preflight(&ordered)?;
    artifact_references.sort_by(|left, right| left.digest.cmp(&right.digest));
    artifact_references.dedup();
    for reference in &artifact_references {
        provider_artifacts.load(reference)?;
    }
    let artifact_set_digest = digest_domain_json(
        if filesystem_only {
            "commonkit.provider-artifact-set.v1"
        } else {
            "commonkit.provider-artifact-set.v2"
        },
        &artifact_references,
    )?;

    let mut operations = Vec::new();
    for resource in ordered {
        if let Some(operation) =
            router.register(resource_id(resource)?, resource, provider_artifacts)?
        {
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
    let digest = match resource.intent.filesystem() {
        Some(intent) => digest_domain_json(
            "commonkit.provider-resource-id.v1",
            &(
                &resource.provenance.provider_id,
                intent.path(),
                &resource.provenance.source,
            ),
        )?,
        None => digest_domain_json(
            "commonkit.provider-resource-id.v2",
            &(
                &resource.provenance.provider_id,
                resource.address(),
                &resource.provenance.source,
            ),
        )?,
    };
    StableId::parse(format!("resource-{}", &digest.as_str()[7..61]))
        .map_err(ProviderPlanError::Contract)
}

#[derive(Debug, Error)]
pub enum ProviderPlanError {
    #[error("provider appears more than once in the materialized set: {0}")]
    DuplicateProvider(StableId),
    #[error("resource planner is not installed for {0:?}")]
    MissingResourcePlanner(ResourceType),
    #[error("package desired intent must be resolved by the trusted controller before planning")]
    PackageResolutionRequired,
    #[error("more than one resource planner is installed for {0:?}")]
    DuplicateResourcePlanner(ResourceType),
    #[error("adapter {adapter_id} is not a fixed route for {resource_type:?}")]
    InvalidResourcePlanner {
        resource_type: ResourceType,
        adapter_id: StableId,
    },
    #[error("resource route for {resource_type:?} returned adapter {actual}, expected {expected}")]
    UnexpectedAdapterRoute {
        resource_type: ResourceType,
        expected: StableId,
        actual: StableId,
    },
    #[error(
        "operation returned for {resource_type:?} does not match routed resource {resource_id}"
    )]
    UnexpectedResourceBinding {
        resource_type: ResourceType,
        resource_id: StableId,
    },
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
    PackageResolution(#[from] crate::PackageResolutionError),
    #[error(transparent)]
    Plan(#[from] PlanBuildError),
    #[error(transparent)]
    Contract(#[from] ContractError),
}
