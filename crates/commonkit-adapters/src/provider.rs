use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use commonkit_contracts::{ContractError, Sha256Digest, StableId, digest_domain_json};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::artifacts::ArtifactStore;
use super::resources::{NormalizedManagedPath, NormalizedResource};

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MaterializedState {
    pub inputs: ProviderInputs,
    pub resources: Vec<NormalizedResource>,
    pub declared_side_effects: Vec<DeclaredSideEffect>,
    pub unsupported: Vec<UnsupportedCapability>,
    pub digest: Sha256Digest,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MaterializedSemantic<'a> {
    inputs_digest: &'a Sha256Digest,
    resources: &'a [NormalizedResource],
    declared_side_effects: &'a [DeclaredSideEffect],
    unsupported: &'a [UnsupportedCapability],
}

impl MaterializedState {
    pub fn finalize(
        inputs: ProviderInputs,
        mut resources: Vec<NormalizedResource>,
        mut declared_side_effects: Vec<DeclaredSideEffect>,
        mut unsupported: Vec<UnsupportedCapability>,
    ) -> Result<Self, ProviderContractError> {
        inputs.verify()?;
        for resource in &resources {
            let provenance = &resource.provenance;
            if provenance.provider_id != inputs.provider_id
                || provenance.provider_version != inputs.provider_version.as_str()
                || provenance.input_digest != inputs.input_set_digest
            {
                return Err(ProviderContractError::InvalidResourceProvenance);
            }
        }
        resources.sort_by(|left, right| {
            (left.intent.path().as_str(), &left.provenance.source)
                .cmp(&(right.intent.path().as_str(), &right.provenance.source))
        });
        declared_side_effects.sort();
        declared_side_effects.dedup();
        unsupported.sort();
        unsupported.dedup();
        let digest = digest_domain_json(
            "commonkit.materialized-state.v1",
            &MaterializedSemantic {
                inputs_digest: &inputs.input_set_digest,
                resources: &resources,
                declared_side_effects: &declared_side_effects,
                unsupported: &unsupported,
            },
        )?;
        Ok(Self {
            inputs,
            resources,
            declared_side_effects,
            unsupported,
            digest,
        })
    }

    pub fn verify(&self) -> Result<(), ProviderContractError> {
        let rebuilt = Self::finalize(
            self.inputs.clone(),
            self.resources.clone(),
            self.declared_side_effects.clone(),
            self.unsupported.clone(),
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

/// Runtime-only capability that exposes an isolated provider destination.
/// It deliberately contains no live-target path or mutation interface.
#[derive(Debug, Clone)]
pub struct ProviderWorkspace {
    staging_root: PathBuf,
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
        for root in live_and_protected_roots {
            let root = root.canonicalize()?;
            if staging_root.starts_with(&root) || root.starts_with(&staging_root) {
                return Err(ProviderContractError::StagingOverlapsLiveRoot);
            }
        }
        set_private_directory(&staging_root)?;
        Ok(Self { staging_root })
    }

    pub fn staging_root(&self) -> &Path {
        &self.staging_root
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

#[cfg(unix)]
fn set_private_directory(path: &Path) -> Result<(), std::io::Error> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn set_private_directory(_path: &Path) -> Result<(), std::io::Error> {
    Ok(())
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
    #[error("provider staging root must be an existing directory")]
    InvalidStagingRoot,
    #[error("provider staging root overlaps a live or protected root")]
    StagingOverlapsLiveRoot,
    #[error(transparent)]
    Io(#[from] std::io::Error),
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
