//! CommonKit composition, policy, and planning primitives.

mod target;

use std::{collections::BTreeSet, sync::OnceLock};

pub use commonkit_contracts::*;
use regex::Regex;
pub use target::{
    CaseSensitivity, OperatingSystem, PlatformFacts, RootAccess, TargetInventory,
    TargetInventoryError, TargetRoot, TargetTransport,
};
use thiserror::Error;

#[derive(Debug, Clone)]
pub struct OperationDraft {
    pub adapter_id: StableId,
    pub kind: OperationKind,
    pub resource: ResourceRef,
    pub risk: Risk,
    pub requires_confirmation: bool,
    pub depends_on: Vec<Sha256Digest>,
    pub before_digest: Option<Sha256Digest>,
    pub after_digest: Option<Sha256Digest>,
    pub payload_digest: Sha256Digest,
    pub provenance: Option<OperationProvenance>,
    pub summary: String,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct OperationSemantic<'a> {
    adapter_id: &'a StableId,
    kind: OperationKind,
    resource: &'a ResourceRef,
    risk: Risk,
    requires_confirmation: bool,
    depends_on: &'a [Sha256Digest],
    before_digest: &'a Option<Sha256Digest>,
    after_digest: &'a Option<Sha256Digest>,
    payload_digest: &'a Sha256Digest,
    #[serde(skip_serializing_if = "Option::is_none")]
    provenance: Option<&'a OperationProvenance>,
}

pub fn finalize_operation(mut draft: OperationDraft) -> Result<Operation, ContractError> {
    draft
        .depends_on
        .sort_by(|left, right| left.as_str().cmp(right.as_str()));
    draft.depends_on.dedup();
    let id = digest_domain_json(
        "commonkit.operation.v1",
        &OperationSemantic {
            adapter_id: &draft.adapter_id,
            kind: draft.kind,
            resource: &draft.resource,
            risk: draft.risk,
            requires_confirmation: draft.requires_confirmation,
            depends_on: &draft.depends_on,
            before_digest: &draft.before_digest,
            after_digest: &draft.after_digest,
            payload_digest: &draft.payload_digest,
            provenance: draft.provenance.as_ref(),
        },
    )?;
    Ok(Operation {
        id,
        adapter_id: draft.adapter_id,
        kind: draft.kind,
        resource: draft.resource,
        risk: draft.risk,
        requires_confirmation: draft.requires_confirmation,
        depends_on: draft.depends_on,
        before_digest: draft.before_digest,
        after_digest: draft.after_digest,
        payload_digest: draft.payload_digest,
        provenance: draft.provenance,
        summary: draft.summary,
    })
}

#[derive(Debug, Clone)]
pub struct PlanDraft {
    pub target_id: StableId,
    pub desired_digest: Sha256Digest,
    pub observed_digest: Sha256Digest,
    pub policy_digest: Sha256Digest,
    pub bindings: PlanBindings,
    pub operations: Vec<Operation>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct PlanSemantic<'a> {
    target_id: &'a StableId,
    desired_digest: &'a Sha256Digest,
    observed_digest: &'a Sha256Digest,
    policy_digest: &'a Sha256Digest,
    bindings: &'a PlanBindings,
    operation_ids: Vec<&'a Sha256Digest>,
}

pub fn build_plan(mut draft: PlanDraft) -> Result<Plan, PlanBuildError> {
    let mut known = BTreeSet::new();
    for operation in &draft.operations {
        if !known.insert(operation.id.as_str().to_owned()) {
            return Err(PlanBuildError::DuplicateOperation(operation.id.clone()));
        }
    }
    for operation in &draft.operations {
        for dependency in &operation.depends_on {
            if !known.contains(dependency.as_str()) {
                return Err(PlanBuildError::MissingDependency(dependency.clone()));
            }
        }
    }

    let mut emitted = BTreeSet::new();
    let mut ordered = Vec::with_capacity(draft.operations.len());
    while !draft.operations.is_empty() {
        let next = draft
            .operations
            .iter()
            .enumerate()
            .filter(|(_, operation)| {
                operation
                    .depends_on
                    .iter()
                    .all(|dependency| emitted.contains(dependency.as_str()))
            })
            .min_by(|(_, left), (_, right)| operation_order(left, right))
            .map(|(index, _)| index)
            .ok_or(PlanBuildError::DependencyCycle)?;
        let operation = draft.operations.remove(next);
        emitted.insert(operation.id.as_str().to_owned());
        ordered.push(operation);
    }
    for operation in &ordered {
        let recomputed = finalize_operation(OperationDraft {
            adapter_id: operation.adapter_id.clone(),
            kind: operation.kind,
            resource: operation.resource.clone(),
            risk: operation.risk,
            requires_confirmation: operation.requires_confirmation,
            depends_on: operation.depends_on.clone(),
            before_digest: operation.before_digest.clone(),
            after_digest: operation.after_digest.clone(),
            payload_digest: operation.payload_digest.clone(),
            provenance: operation.provenance.clone(),
            summary: operation.summary.clone(),
        })?;
        if recomputed.id != operation.id {
            return Err(PlanBuildError::OperationIdMismatch(operation.id.clone()));
        }
    }
    let id = digest_domain_json(
        "commonkit.plan.v1",
        &PlanSemantic {
            target_id: &draft.target_id,
            desired_digest: &draft.desired_digest,
            observed_digest: &draft.observed_digest,
            policy_digest: &draft.policy_digest,
            bindings: &draft.bindings,
            operation_ids: ordered.iter().map(|operation| &operation.id).collect(),
        },
    )?;
    Ok(Plan {
        schema_version: SchemaVersion(SCHEMA_VERSION),
        contract_version: CONTRACT_VERSION.into(),
        id,
        target_id: draft.target_id,
        desired_digest: draft.desired_digest,
        observed_digest: draft.observed_digest,
        policy_digest: draft.policy_digest,
        bindings: draft.bindings,
        operations: ordered,
    })
}

fn operation_order(left: &Operation, right: &Operation) -> std::cmp::Ordering {
    (
        left.adapter_id.as_str(),
        left.resource.resource_type.as_str(),
        left.resource.resource_id.as_str(),
        left.kind,
        left.id.as_str(),
    )
        .cmp(&(
            right.adapter_id.as_str(),
            right.resource.resource_type.as_str(),
            right.resource.resource_id.as_str(),
            right.kind,
            right.id.as_str(),
        ))
}

#[derive(Debug, Error)]
pub enum PlanBuildError {
    #[error(transparent)]
    Contract(#[from] ContractError),
    #[error("duplicate operation ID: {0}")]
    DuplicateOperation(Sha256Digest),
    #[error("operation dependency is missing: {0}")]
    MissingDependency(Sha256Digest),
    #[error("operation dependency graph contains a cycle")]
    DependencyCycle,
    #[error("operation semantic ID does not match its content: {0}")]
    OperationIdMismatch(Sha256Digest),
}

pub fn is_forbidden_path(candidate: &str) -> bool {
    static PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();
    PATTERNS
        .get_or_init(|| {
            [
                r"(^|/)auth\.json$",
                r"(^|/)hosts\.yml$",
                r"(?i)(^|/)credentials\.enc$",
                r"(?i)(^|/).*credentials.*\.json$",
                r"(?i)(^|/)client_secret.*\.json$",
                r"(?i)(^|/)token_cache\.json$",
                r"(?i)(^|/)\.encryption_key$",
                r"(^|/)\.env(?:\.|$)",
                r"(?i)(^|/)(?:credentials?|tokens?|secrets?)(?:/|$|\.(?:json|ya?ml|txt|enc)$)",
                r"(?:^|/)orchestration\.db(?:-(?:wal|shm))?$",
                r"(?:^|/)(?:state|logs|memories|goals)_\d+\.sqlite(?:-(?:wal|shm))?$",
                r"(?:^|/)(?:state|logs|memories|goals)(?:_[0-9]+)?\.(?:db|sqlite|sqlite3)(?:-(?:wal|shm))?$",
                r"(?:^|/)context-mode/sessions/",
                r"(?:^|/)orca-(?:devices|e2ee-keypair)\.json$",
                r"(?:^|/)(?:Cookies|Local Storage|Singleton[^/]*)$",
                r"\.(?:sock|token)$",
                r"(?i)\.(?:pem|key)$",
            ]
            .into_iter()
            .map(|pattern| Regex::new(pattern).expect("forbidden-path regex"))
            .collect()
        })
        .iter()
        .any(|pattern| pattern.is_match(candidate))
}

pub fn enforce_policy_floor(
    organization: &SecurityPolicy,
    effective: &SecurityPolicy,
) -> Result<(), PolicyViolation> {
    for denied_path in &organization.denied_paths {
        if !effective.denied_paths.contains(denied_path) {
            return Err(PolicyViolation::DenialRemoved {
                value: denied_path.clone(),
            });
        }
    }
    for (control, required) in &organization.required_controls {
        if *required && effective.required_controls.get(control) != Some(&true) {
            return Err(PolicyViolation::RequiredControlWeakened {
                control: control.clone(),
            });
        }
    }
    for (name, allowed) in &organization.allowlists {
        let Some(effective_allowed) = effective.allowlists.get(name) else {
            return Err(PolicyViolation::AllowlistWidened { name: name.clone() });
        };
        if !effective_allowed.is_subset(allowed) {
            return Err(PolicyViolation::AllowlistWidened { name: name.clone() });
        }
    }
    for (name, minimum) in &organization.minimums {
        if effective
            .minimums
            .get(name)
            .is_none_or(|value| value < minimum)
        {
            return Err(PolicyViolation::MinimumLowered { name: name.clone() });
        }
    }
    for (name, maximum) in &organization.maximums {
        if effective
            .maximums
            .get(name)
            .is_none_or(|value| value > maximum)
        {
            return Err(PolicyViolation::MaximumRaised { name: name.clone() });
        }
    }
    Ok(())
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PolicyViolation {
    #[error("organization denial was removed: {value}")]
    DenialRemoved { value: String },
    #[error("required organization control was weakened: {control}")]
    RequiredControlWeakened { control: StableId },
    #[error("organization allowlist was widened: {name}")]
    AllowlistWidened { name: StableId },
    #[error("organization minimum was lowered: {name}")]
    MinimumLowered { name: StableId },
    #[error("organization maximum was raised: {name}")]
    MaximumRaised { name: StableId },
}
