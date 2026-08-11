use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use commonkit_contracts::{ContractError, Sha256Digest, StableId, digest_domain_json};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::artifacts::ArtifactStore;
use super::resources::{
    NormalizedManagedPath, NormalizedResource, ProviderResourceIntent, ResourceProvenance,
    ResourceType,
};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ExactProviderVersion(String);

impl ExactProviderVersion {
    pub fn parse(value: impl Into<String>) -> Result<Self, ProviderContractError> {
        let value = value.into();
        if value.is_empty()
            || value.trim() != value
            || value.chars().any(char::is_whitespace)
            || value.contains(['*', '^', '~', '<', '>', '='])
        {
            return Err(ProviderContractError::NonExactVersion(value));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ExactProviderVersion {
    type Error = ProviderContractError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl From<ExactProviderVersion> for String {
    fn from(value: ExactProviderVersion) -> Self {
        value.0
    }
}

impl fmt::Display for ExactProviderVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderInputs {
    pub provider_id: StableId,
    pub provider_version: ExactProviderVersion,
    pub contract_version: String,
    pub input_digests: BTreeMap<String, Sha256Digest>,
    pub declared_features: Vec<String>,
    pub input_set_digest: Sha256Digest,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProviderInputsSemantic<'a> {
    provider_id: &'a StableId,
    provider_version: &'a ExactProviderVersion,
    contract_version: &'a str,
    input_digests: &'a BTreeMap<String, Sha256Digest>,
    declared_features: &'a [String],
}

impl ProviderInputs {
    pub fn new(
        provider_id: StableId,
        provider_version: ExactProviderVersion,
        contract_version: String,
        input_digests: BTreeMap<String, Sha256Digest>,
        mut declared_features: Vec<String>,
    ) -> Result<Self, ProviderContractError> {
        if contract_version.is_empty() || input_digests.is_empty() {
            return Err(ProviderContractError::IncompleteInputs);
        }
        declared_features.sort();
        declared_features.dedup();
        let input_set_digest = digest_domain_json(
            "commonkit.provider-inputs.v1",
            &ProviderInputsSemantic {
                provider_id: &provider_id,
                provider_version: &provider_version,
                contract_version: &contract_version,
                input_digests: &input_digests,
                declared_features: &declared_features,
            },
        )?;
        Ok(Self {
            provider_id,
            provider_version,
            contract_version,
            input_digests,
            declared_features,
            input_set_digest,
        })
    }

    pub fn digest(&self) -> &Sha256Digest {
        &self.input_set_digest
    }

    pub fn verify(&self) -> Result<(), ProviderContractError> {
        let rebuilt = Self::new(
            self.provider_id.clone(),
            self.provider_version.clone(),
            self.contract_version.clone(),
            self.input_digests.clone(),
            self.declared_features.clone(),
        )?;
        if &rebuilt == self {
            Ok(())
        } else {
            Err(ProviderContractError::InputDigestMismatch)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum DeclaredSideEffect {
    ServiceRestart { service: String },
    RelayReconfigure { relay: String },
    UnsupportedResource { resource_type: String },
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UnsupportedCapability {
    pub source: String,
    pub capability: String,
    pub remediation: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MaterializedState {
    pub inputs: ProviderInputs,
    pub resources: Vec<NormalizedResource>,
    pub declared_side_effects: Vec<DeclaredSideEffect>,
    pub unsupported: Vec<UnsupportedCapability>,
    #[serde(default)]
    pub capabilities: Vec<ProviderCapabilityResource>,
    pub digest: Sha256Digest,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProviderNormalizedResource {
    intent: ProviderResourceIntent,
    provenance: ResourceProvenance,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProviderMaterializedState {
    inputs: ProviderInputs,
    resources: Vec<ProviderNormalizedResource>,
    declared_side_effects: Vec<DeclaredSideEffect>,
    unsupported: Vec<UnsupportedCapability>,
    #[serde(default)]
    capabilities: Vec<ProviderCapabilityResource>,
    digest: Sha256Digest,
}

impl<'de> Deserialize<'de> for MaterializedState {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = ProviderMaterializedState::deserialize(deserializer)?;
        Ok(Self {
            inputs: wire.inputs,
            resources: wire
                .resources
                .into_iter()
                .map(|resource| NormalizedResource {
                    intent: resource.intent.into(),
                    provenance: resource.provenance,
                })
                .collect(),
            declared_side_effects: wire.declared_side_effects,
            unsupported: wire.unsupported,
            capabilities: wire.capabilities,
            digest: wire.digest,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProviderCapability {
    McpStreamableHttp {
        id: String,
        name: String,
        enabled: bool,
        url: String,
        headers: BTreeMap<String, String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderCapabilityResource {
    pub capability: ProviderCapability,
    pub provenance: super::resources::ResourceProvenance,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MaterializedSemantic<'a> {
    inputs_digest: &'a Sha256Digest,
    resources: &'a [NormalizedResource],
    declared_side_effects: &'a [DeclaredSideEffect],
    unsupported: &'a [UnsupportedCapability],
    #[serde(skip_serializing_if = "slice_is_empty")]
    capabilities: &'a [ProviderCapabilityResource],
}

fn slice_is_empty<T>(values: &[T]) -> bool {
    values.is_empty()
}

impl MaterializedState {
    pub fn finalize(
        inputs: ProviderInputs,
        resources: Vec<NormalizedResource>,
        declared_side_effects: Vec<DeclaredSideEffect>,
        unsupported: Vec<UnsupportedCapability>,
    ) -> Result<Self, ProviderContractError> {
        Self::finalize_with_capabilities(
            inputs,
            resources,
            declared_side_effects,
            unsupported,
            Vec::new(),
        )
    }

    pub fn finalize_with_capabilities(
        inputs: ProviderInputs,
        mut resources: Vec<NormalizedResource>,
        mut declared_side_effects: Vec<DeclaredSideEffect>,
        mut unsupported: Vec<UnsupportedCapability>,
        mut capabilities: Vec<ProviderCapabilityResource>,
    ) -> Result<Self, ProviderContractError> {
        inputs.verify()?;
        for resource in &resources {
            if matches!(
                resource.intent,
                super::resources::ResourceIntent::ResolvedPackage(_)
            ) {
                return Err(ProviderContractError::ControllerOwnedPackageResolution);
            }
            let provenance = &resource.provenance;
            if provenance.provider_id != inputs.provider_id
                || provenance.provider_version != inputs.provider_version.as_str()
                || provenance.input_digest != inputs.input_set_digest
            {
                return Err(ProviderContractError::InvalidResourceProvenance);
            }
        }
        resources.sort_by(|left, right| {
            (left.sort_key(), &left.provenance.source)
                .cmp(&(right.sort_key(), &right.provenance.source))
        });
        declared_side_effects.sort();
        declared_side_effects.dedup();
        unsupported.sort();
        unsupported.dedup();
        for capability in &capabilities {
            let provenance = &capability.provenance;
            if provenance.provider_id != inputs.provider_id
                || provenance.provider_version != inputs.provider_version.as_str()
                || provenance.input_digest != inputs.input_set_digest
            {
                return Err(ProviderContractError::InvalidResourceProvenance);
            }
        }
        capabilities.sort_by(|left, right| {
            let ProviderCapability::McpStreamableHttp { id: left_id, .. } = &left.capability;
            let ProviderCapability::McpStreamableHttp { id: right_id, .. } = &right.capability;
            (&left.provenance.source, left_id).cmp(&(&right.provenance.source, right_id))
        });
        capabilities.dedup();
        let digest_domain = if resources
            .iter()
            .all(|resource| resource.resource_type() == ResourceType::Filesystem)
        {
            "commonkit.materialized-state.v1"
        } else {
            "commonkit.materialized-state.v2"
        };
        let digest = digest_domain_json(
            digest_domain,
            &MaterializedSemantic {
                inputs_digest: &inputs.input_set_digest,
                resources: &resources,
                declared_side_effects: &declared_side_effects,
                unsupported: &unsupported,
                capabilities: &capabilities,
            },
        )?;
        Ok(Self {
            inputs,
            resources,
            declared_side_effects,
            unsupported,
            capabilities,
            digest,
        })
    }

    pub fn verify(&self) -> Result<(), ProviderContractError> {
        let rebuilt = Self::finalize_with_capabilities(
            self.inputs.clone(),
            self.resources.clone(),
            self.declared_side_effects.clone(),
            self.unsupported.clone(),
            self.capabilities.clone(),
        )?;
        if &rebuilt == self {
            Ok(())
        } else {
            Err(ProviderContractError::MaterializationDigestMismatch)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResolvedMaterializedState {
    pub inputs: ProviderInputs,
    pub resources: Vec<NormalizedResource>,
    pub declared_side_effects: Vec<DeclaredSideEffect>,
    pub unsupported: Vec<UnsupportedCapability>,
    #[serde(default)]
    pub capabilities: Vec<ProviderCapabilityResource>,
    pub digest: Sha256Digest,
}

impl ResolvedMaterializedState {
    pub fn finalize(
        inputs: ProviderInputs,
        mut resources: Vec<NormalizedResource>,
        mut declared_side_effects: Vec<DeclaredSideEffect>,
        mut unsupported: Vec<UnsupportedCapability>,
        mut capabilities: Vec<ProviderCapabilityResource>,
    ) -> Result<Self, ProviderContractError> {
        inputs.verify()?;
        for resource in &resources {
            if matches!(
                resource.intent,
                super::resources::ResourceIntent::Package(_)
            ) {
                return Err(ProviderContractError::UnresolvedPackageIntent);
            }
            let provenance = &resource.provenance;
            if provenance.provider_id != inputs.provider_id
                || provenance.provider_version != inputs.provider_version.as_str()
                || provenance.input_digest != inputs.input_set_digest
            {
                return Err(ProviderContractError::InvalidResourceProvenance);
            }
        }
        resources.sort_by(|left, right| {
            (left.sort_key(), &left.provenance.source)
                .cmp(&(right.sort_key(), &right.provenance.source))
        });
        declared_side_effects.sort();
        declared_side_effects.dedup();
        unsupported.sort();
        unsupported.dedup();
        capabilities.sort_by(|left, right| {
            let ProviderCapability::McpStreamableHttp { id: left_id, .. } = &left.capability;
            let ProviderCapability::McpStreamableHttp { id: right_id, .. } = &right.capability;
            (&left.provenance.source, left_id).cmp(&(&right.provenance.source, right_id))
        });
        capabilities.dedup();
        let digest = digest_domain_json(
            "commonkit.resolved-materialized-state.v1",
            &MaterializedSemantic {
                inputs_digest: &inputs.input_set_digest,
                resources: &resources,
                declared_side_effects: &declared_side_effects,
                unsupported: &unsupported,
                capabilities: &capabilities,
            },
        )?;
        Ok(Self {
            inputs,
            resources,
            declared_side_effects,
            unsupported,
            capabilities,
            digest,
        })
    }

    pub fn verify(&self) -> Result<(), ProviderContractError> {
        let rebuilt = Self::finalize(
            self.inputs.clone(),
            self.resources.clone(),
            self.declared_side_effects.clone(),
            self.unsupported.clone(),
            self.capabilities.clone(),
        )?;
        if &rebuilt == self {
            Ok(())
        } else {
            Err(ProviderContractError::MaterializationDigestMismatch)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderContext {
    pub target_id: StableId,
    pub platform: String,
    pub architecture: String,
    pub policy_digest: Sha256Digest,
    pub declared_roots: Vec<NormalizedManagedPath>,
    pub observed_fact_digests: BTreeMap<String, Sha256Digest>,
}

impl ProviderContext {
    /// Stable binding used by providers whose output can vary by target OS or
    /// architecture. This is target data, never controller process data.
    pub fn platform_facts_digest(&self) -> Result<Sha256Digest, ProviderContractError> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Facts<'a> {
            platform: &'a str,
            architecture: &'a str,
        }
        if self.platform.trim().is_empty() || self.architecture.trim().is_empty() {
            return Err(ProviderContractError::IncompleteInputs);
        }
        digest_domain_json(
            "commonkit.provider-target-platform.v1",
            &Facts {
                platform: &self.platform,
                architecture: &self.architecture,
            },
        )
        .map_err(Into::into)
    }
}

/// Runtime-only capability that exposes an isolated provider destination.
/// It deliberately contains no live-target path or mutation interface.
#[derive(Debug, Clone)]
pub struct ProviderWorkspace {
    staging_root: PathBuf,
    scratch_root: PathBuf,
}

impl ProviderWorkspace {
    pub fn open(
        staging_root: impl AsRef<Path>,
        live_and_protected_roots: &[PathBuf],
    ) -> Result<Self, ProviderContractError> {
        let staging_root = staging_root.as_ref().canonicalize()?;
        if !staging_root.is_dir() {
            return Err(ProviderContractError::InvalidStagingRoot);
        }
        let scratch_root = staging_root.with_extension("provider-scratch");
        for root in live_and_protected_roots {
            let root = root.canonicalize()?;
            if staging_root.starts_with(&root)
                || root.starts_with(&staging_root)
                || scratch_root.starts_with(&root)
                || root.starts_with(&scratch_root)
            {
                return Err(ProviderContractError::StagingOverlapsLiveRoot);
            }
        }
        set_private_directory(&staging_root)?;
        match fs::create_dir(&scratch_root) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                if fs::read_dir(&scratch_root)?.next().is_some() {
                    return Err(ProviderContractError::InvalidStagingRoot);
                }
            }
            Err(error) => return Err(error.into()),
        }
        set_private_directory(&scratch_root)?;
        Ok(Self {
            staging_root,
            scratch_root,
        })
    }

    pub fn staging_root(&self) -> &Path {
        &self.staging_root
    }

    pub fn scratch_root(&self) -> &Path {
        &self.scratch_root
    }
}

pub trait DesiredStateProvider {
    fn id(&self) -> &StableId;

    fn inspect_inputs(&self, context: &ProviderContext) -> Result<ProviderInputs, ProviderFailure>;

    fn materialize(
        &self,
        context: &ProviderContext,
        workspace: &ProviderWorkspace,
        artifacts: &ArtifactStore,
    ) -> Result<MaterializedState, ProviderFailure>;
}

fn set_private_directory(path: &Path) -> Result<(), commonkit_platform::PlatformError> {
    commonkit_platform::ensure_private_path(path, commonkit_platform::PrivatePathKind::Directory)
}

#[derive(Debug, Error)]
pub enum ProviderContractError {
    #[error("provider version must be exact, not a range: {0}")]
    NonExactVersion(String),
    #[error("provider inputs require a contract version and at least one named digest")]
    IncompleteInputs,
    #[error("provider input digest does not match its canonical inputs")]
    InputDigestMismatch,
    #[error("resource provenance does not match the provider input set")]
    InvalidResourceProvenance,
    #[error("materialization digest does not match its canonical state")]
    MaterializationDigestMismatch,
    #[error("provider output cannot contain controller-owned package resolution")]
    ControllerOwnedPackageResolution,
    #[error("resolved materialized state still contains a desired package intent")]
    UnresolvedPackageIntent,
    #[error("provider staging root must be an existing directory")]
    InvalidStagingRoot,
    #[error("provider staging root overlaps a live or protected root")]
    StagingOverlapsLiveRoot,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Platform(#[from] commonkit_platform::PlatformError),
    #[error(transparent)]
    Contract(#[from] ContractError),
}

#[derive(Debug, Error)]
pub enum ProviderFailure {
    #[error("provider input inspection failed: {0}")]
    Inspect(String),
    #[error("provider materialization failed: {0}")]
    Materialize(String),
    #[error(transparent)]
    Contract(#[from] ProviderContractError),
}
