use std::collections::{BTreeMap, BTreeSet};

use commonkit_contracts::{
    ContractError, PackageDeclaration, PackageManager, SchemaVersion, SecurityPolicy, Sha256Digest,
    StableId, digest_domain_json,
};
use commonkit_core::enforce_package_source_policy;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    ArtifactError, ArtifactStore, ContentReference, ContentSensitivity, MaterializedState,
    PackageDesiredIntent, ProviderContractError, ResolvedMaterializedState, ResourceIntent,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackageTargetV1 {
    pub os: String,
    pub os_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distro_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distro_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codename: Option<String>,
    pub arch: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub libc: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manager_prefix: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagerBindingV1 {
    pub manager: PackageManager,
    pub version: String,
    pub executable_digest: Sha256Digest,
    pub config_digest: Sha256Digest,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactEvidence {
    pub authority: StableId,
    pub metadata_digest: Sha256Digest,
    pub signature_digest: Sha256Digest,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceBindingV1 {
    pub source_id: StableId,
    pub registry_definition_digest: Sha256Digest,
    pub canonical_repository: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository_revision: Option<String>,
    pub signed_metadata: Vec<ArtifactEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackageObservationV1 {
    pub installed_versions: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackageResolutionProbeV1 {
    pub before: PackageObservationV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository_revision: Option<String>,
    pub signed_metadata: Vec<ArtifactEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResolvedPackage {
    pub declaration: PackageDeclaration,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackageArtifactV1 {
    pub role: StableId,
    pub content: ContentReference,
    pub upstream_checksum: Sha256Digest,
    pub size: u64,
    pub materialization_key: StableId,
    pub source_metadata_digest: Sha256Digest,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum OfflineInstallRecipeV1 {
    HomebrewBottle { artifact_roles: BTreeSet<StableId> },
    AptArchives { artifact_roles: BTreeSet<StableId> },
    NodeArchive { artifact_roles: BTreeSet<StableId> },
    RustToolchain { artifact_roles: BTreeSet<StableId> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackageResolutionV1 {
    pub schema_version: SchemaVersion,
    pub declaration: PackageDeclaration,
    pub target: PackageTargetV1,
    pub manager: ManagerBindingV1,
    pub source: SourceBindingV1,
    pub before: PackageObservationV1,
    pub closure: Vec<ResolvedPackage>,
    pub artifacts: Vec<PackageArtifactV1>,
    pub recipe: OfflineInstallRecipeV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResolvedPackageIntent {
    pub declaration: PackageDeclaration,
    pub resolution: ContentReference,
    pub artifacts: Vec<ContentReference>,
}

pub fn package_resolution_schema() -> Result<serde_json::Value, serde_json::Error> {
    let mut schema = serde_json::to_value(schema_for!(PackageResolutionV1))?;
    schema["$id"] = serde_json::Value::String(
        "https://schemas.commonkit.dev/v1/package-resolution.schema.json".into(),
    );
    Ok(schema)
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackageFetchRequestV1 {
    pub role: StableId,
    pub immutable_locator: String,
    pub upstream_checksum: Sha256Digest,
    pub size: u64,
    pub materialization_key: StableId,
    pub source_metadata_digest: Sha256Digest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackageResolutionDraftV1 {
    pub closure: Vec<ResolvedPackage>,
    pub artifacts: Vec<PackageFetchRequestV1>,
    pub recipe: OfflineInstallRecipeV1,
}

pub struct PackageResolutionRequestV1<'a> {
    pub desired: &'a PackageDesiredIntent,
    pub target: &'a PackageTargetV1,
    pub manager: &'a ManagerBindingV1,
    pub source_id: &'a StableId,
    pub canonical_repository: &'a str,
    pub registry_definition_digest: &'a Sha256Digest,
}

pub trait PackageFetch {
    fn fetch(&mut self, request: &PackageFetchRequestV1)
    -> Result<Vec<u8>, PackageResolutionError>;
}

pub trait PackageResolutionBackend {
    fn manager(&self) -> PackageManager;

    fn probe(
        &mut self,
        request: &PackageResolutionRequestV1<'_>,
    ) -> Result<PackageResolutionProbeV1, PackageResolutionError>;

    fn resolve_and_fetch(
        &mut self,
        request: &PackageResolutionRequestV1<'_>,
        source: &SourceBindingV1,
        fetch: &mut dyn PackageFetch,
    ) -> Result<PackageResolutionDraftV1, PackageResolutionError>;
}

/// Target-side package preparation seam. It deliberately has no fetch or
/// resolver capability and can consume only controller-persisted artifacts.
pub trait OfflinePackageBackend {
    fn manager(&self) -> PackageManager;

    fn prepare_offline(
        &mut self,
        resolution: &PackageResolutionV1,
        artifacts: &ArtifactStore,
    ) -> Result<(), PackageResolutionError>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct PackageSourceDefinitionV1<'a> {
    source_id: &'a StableId,
    manager: PackageManager,
    canonical_repository: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PackageSourceDefinition {
    source_id: StableId,
    manager: PackageManager,
    canonical_repository: String,
    digest: Sha256Digest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ControlledPackageSourceV1 {
    pub source_id: StableId,
    pub manager: PackageManager,
    pub canonical_repository: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageSourceRegistry {
    sources: BTreeMap<StableId, PackageSourceDefinition>,
}

impl PackageSourceRegistry {
    pub fn builtin() -> Result<Self, PackageResolutionError> {
        Self::from_controlled_sources(vec![
            controlled_source(
                "homebrew-core",
                PackageManager::Homebrew,
                "https://github.com/Homebrew/homebrew-core",
            )?,
            controlled_source(
                "homebrew_core",
                PackageManager::Homebrew,
                "https://github.com/Homebrew/homebrew-core",
            )?,
            controlled_source(
                "ubuntu-main",
                PackageManager::Apt,
                "https://archive.ubuntu.com/ubuntu",
            )?,
            controlled_source(
                "debian-main",
                PackageManager::Apt,
                "https://deb.debian.org/debian",
            )?,
            controlled_source("nodejs-fnm", PackageManager::Fnm, "https://nodejs.org/dist")?,
            controlled_source("nodejs-nvm", PackageManager::Nvm, "https://nodejs.org/dist")?,
            controlled_source(
                "rustup-official",
                PackageManager::Rustup,
                "https://static.rust-lang.org",
            )?,
        ])
    }

    pub fn from_controlled_sources(
        sources: Vec<ControlledPackageSourceV1>,
    ) -> Result<Self, PackageResolutionError> {
        let mut registry = BTreeMap::new();
        for source in sources {
            if !source.canonical_repository.starts_with("https://") {
                return Err(PackageResolutionError::MutableSourceMetadata);
            }
            let digest = digest_domain_json(
                "commonkit.package-source-registry-definition.v1",
                &PackageSourceDefinitionV1 {
                    source_id: &source.source_id,
                    manager: source.manager,
                    canonical_repository: &source.canonical_repository,
                },
            )?;
            if registry
                .insert(
                    source.source_id.clone(),
                    PackageSourceDefinition {
                        source_id: source.source_id,
                        manager: source.manager,
                        canonical_repository: source.canonical_repository,
                        digest,
                    },
                )
                .is_some()
            {
                return Err(PackageResolutionError::DuplicateSource);
            }
        }
        Ok(Self { sources: registry })
    }
}

fn controlled_source(
    source_id: &str,
    manager: PackageManager,
    canonical_repository: &str,
) -> Result<ControlledPackageSourceV1, PackageResolutionError> {
    Ok(ControlledPackageSourceV1 {
        source_id: StableId::parse(source_id)?,
        manager,
        canonical_repository: canonical_repository.into(),
    })
}

pub struct PackageResolutionCoordinator<'a> {
    policy: &'a SecurityPolicy,
    registry: &'a PackageSourceRegistry,
    manager: ManagerBindingV1,
    backend: &'a mut dyn PackageResolutionBackend,
    fetch: &'a mut dyn PackageFetch,
}

impl<'a> PackageResolutionCoordinator<'a> {
    pub fn new(
        policy: &'a SecurityPolicy,
        registry: &'a PackageSourceRegistry,
        manager: ManagerBindingV1,
        backend: &'a mut dyn PackageResolutionBackend,
        fetch: &'a mut dyn PackageFetch,
    ) -> Self {
        Self {
            policy,
            registry,
            manager,
            backend,
            fetch,
        }
    }

    pub fn resolve(
        &mut self,
        desired: &PackageDesiredIntent,
        target: &PackageTargetV1,
        store: &ArtifactStore,
    ) -> Result<ResolvedPackageIntent, PackageResolutionError> {
        let PackageDesiredIntent::Package { declaration } = desired;
        declaration.validate()?;
        enforce_package_source_policy(self.policy, declaration).map_err(|_| {
            PackageResolutionError::PackageSourceNotAllowed {
                source_id: declaration.source.clone(),
            }
        })?;
        validate_target(target)?;
        validate_manager(&self.manager)?;
        if self.manager.manager != declaration.manager
            || self.backend.manager() != declaration.manager
        {
            return Err(PackageResolutionError::ManagerMismatch);
        }
        let definition = self
            .registry
            .sources
            .get(&declaration.source)
            .ok_or_else(|| PackageResolutionError::UnknownSource {
                source_id: declaration.source.clone(),
            })?;
        if definition.manager != declaration.manager || definition.source_id != declaration.source {
            return Err(PackageResolutionError::SourceManagerMismatch);
        }
        let request = PackageResolutionRequestV1 {
            desired,
            target,
            manager: &self.manager,
            source_id: &definition.source_id,
            canonical_repository: &definition.canonical_repository,
            registry_definition_digest: &definition.digest,
        };
        let mut probe = self.backend.probe(&request)?;
        probe.signed_metadata.sort();
        if probe
            .signed_metadata
            .windows(2)
            .any(|pair| pair[0] == pair[1])
        {
            return Err(PackageResolutionError::DuplicateSourceMetadata);
        }
        let source = SourceBindingV1 {
            source_id: definition.source_id.clone(),
            registry_definition_digest: definition.digest.clone(),
            canonical_repository: definition.canonical_repository.clone(),
            repository_revision: probe.repository_revision,
            signed_metadata: probe.signed_metadata,
        };
        validate_source(&source)?;
        let source_metadata_digest = source.metadata_digest()?;
        let mut recording_fetch = RecordingPackageFetch {
            inner: self.fetch,
            fetched: Vec::new(),
        };
        let mut draft = self
            .backend
            .resolve_and_fetch(&request, &source, &mut recording_fetch)?;
        canonicalize_draft(&mut draft)?;
        if !draft
            .closure
            .iter()
            .any(|package| package.declaration == *declaration)
        {
            return Err(PackageResolutionError::MissingRootPackage);
        }
        validate_recipe_roles(
            &draft.recipe,
            draft.artifacts.iter().map(|artifact| artifact.role.clone()),
        )?;
        let artifacts = validate_and_persist_artifacts(
            &draft.artifacts,
            recording_fetch.fetched,
            &source_metadata_digest,
            store,
        )?;
        let resolution = PackageResolutionV1 {
            schema_version: SchemaVersion(1),
            declaration: declaration.clone(),
            target: target.clone(),
            manager: self.manager.clone(),
            source,
            before: probe.before,
            closure: draft.closure,
            artifacts,
            recipe: draft.recipe,
        };
        let resolution_bytes = serde_json::to_vec(&resolution)?;
        let resolution_reference = store.put(&resolution_bytes, ContentSensitivity::Portable)?;
        Ok(ResolvedPackageIntent {
            declaration: declaration.clone(),
            resolution: resolution_reference,
            artifacts: resolution
                .artifacts
                .iter()
                .map(|artifact| artifact.content.clone())
                .collect(),
        })
    }

    pub fn resolve_state(
        &mut self,
        state: &MaterializedState,
        target: &PackageTargetV1,
        store: &ArtifactStore,
    ) -> Result<ResolvedMaterializedState, PackageResolutionError> {
        state.verify()?;
        let mut resources = Vec::with_capacity(state.resources.len());
        for resource in &state.resources {
            let intent = match &resource.intent {
                ResourceIntent::Filesystem(intent) => ResourceIntent::Filesystem(intent.clone()),
                ResourceIntent::Package(desired) => {
                    ResourceIntent::ResolvedPackage(self.resolve(desired, target, store)?)
                }
                ResourceIntent::ResolvedPackage(_) => {
                    return Err(PackageResolutionError::ResolutionBindingMismatch);
                }
            };
            resources.push(crate::NormalizedResource {
                intent,
                provenance: resource.provenance.clone(),
            });
        }
        ResolvedMaterializedState::finalize(
            state.inputs.clone(),
            resources,
            state.declared_side_effects.clone(),
            state.unsupported.clone(),
            state.capabilities.clone(),
        )
        .map_err(Into::into)
    }
}

impl SourceBindingV1 {
    pub fn metadata_digest(&self) -> Result<Sha256Digest, ContractError> {
        digest_domain_json(
            "commonkit.package-source-metadata.v1",
            &(&self.repository_revision, &self.signed_metadata),
        )
    }
}

impl ResolvedPackageIntent {
    pub fn load_persisted(
        &self,
        store: &ArtifactStore,
    ) -> Result<PackageResolutionV1, PackageResolutionError> {
        let bytes = store.load(&self.resolution)?;
        let resolution: PackageResolutionV1 = serde_json::from_slice(&bytes)?;
        if resolution.schema_version != SchemaVersion(1)
            || resolution.declaration != self.declaration
        {
            return Err(PackageResolutionError::ResolutionBindingMismatch);
        }
        validate_persisted_resolution(&resolution)?;
        let expected = resolution
            .artifacts
            .iter()
            .map(|artifact| artifact.content.clone())
            .collect::<Vec<_>>();
        if expected != self.artifacts {
            return Err(PackageResolutionError::ArtifactSetMismatch);
        }
        let mut unique = BTreeSet::new();
        for artifact in &resolution.artifacts {
            if artifact.content.digest != artifact.upstream_checksum
                || artifact.content.bytes != artifact.size
                || !unique.insert((artifact.role.clone(), artifact.materialization_key.clone()))
            {
                return Err(PackageResolutionError::CorruptArtifact);
            }
            store.load(&artifact.content)?;
        }
        Ok(resolution)
    }

    pub fn load_and_validate(
        &self,
        target: &PackageTargetV1,
        manager: &ManagerBindingV1,
        registry: &PackageSourceRegistry,
        store: &ArtifactStore,
    ) -> Result<PackageResolutionV1, PackageResolutionError> {
        let resolution = self.load_persisted(store)?;
        if &resolution.target != target {
            return Err(PackageResolutionError::TargetBindingMismatch);
        }
        if &resolution.manager != manager {
            return Err(PackageResolutionError::ManagerBindingMismatch);
        }
        let definition = registry
            .sources
            .get(&resolution.source.source_id)
            .ok_or_else(|| PackageResolutionError::UnknownSource {
                source_id: resolution.source.source_id.clone(),
            })?;
        if definition.manager != resolution.manager.manager
            || definition.digest != resolution.source.registry_definition_digest
            || definition.canonical_repository != resolution.source.canonical_repository
        {
            return Err(PackageResolutionError::SourceBindingMismatch);
        }
        validate_source(&resolution.source)?;
        Ok(resolution)
    }
}

fn validate_persisted_resolution(
    resolution: &PackageResolutionV1,
) -> Result<(), PackageResolutionError> {
    resolution.declaration.validate()?;
    validate_target(&resolution.target)?;
    validate_manager(&resolution.manager)?;
    validate_source(&resolution.source)?;
    let source_metadata_digest = resolution.source.metadata_digest()?;
    let mut previous_closure = None;
    for package in &resolution.closure {
        package.declaration.validate()?;
        let key = (
            package.declaration.manager,
            package.declaration.id.as_str(),
            package.declaration.version.as_str(),
        );
        if previous_closure.is_some_and(|previous| previous >= key) {
            return Err(PackageResolutionError::DuplicateClosurePackage);
        }
        previous_closure = Some(key);
    }
    if !resolution
        .closure
        .iter()
        .any(|package| package.declaration == resolution.declaration)
    {
        return Err(PackageResolutionError::MissingRootPackage);
    }
    let mut keys = BTreeSet::new();
    for artifact in &resolution.artifacts {
        if artifact.source_metadata_digest != source_metadata_digest
            || !keys.insert(artifact.materialization_key.clone())
        {
            return Err(PackageResolutionError::CorruptArtifact);
        }
    }
    validate_recipe_roles(
        &resolution.recipe,
        resolution
            .artifacts
            .iter()
            .map(|artifact| artifact.role.clone()),
    )
}

struct RecordingPackageFetch<'a> {
    inner: &'a mut dyn PackageFetch,
    fetched: Vec<(PackageFetchRequestV1, Vec<u8>)>,
}

impl PackageFetch for RecordingPackageFetch<'_> {
    fn fetch(
        &mut self,
        request: &PackageFetchRequestV1,
    ) -> Result<Vec<u8>, PackageResolutionError> {
        validate_fetch_request(request)?;
        let bytes = self.inner.fetch(request)?;
        self.fetched.push((request.clone(), bytes.clone()));
        Ok(bytes)
    }
}

fn validate_target(target: &PackageTargetV1) -> Result<(), PackageResolutionError> {
    let required = [&target.os, &target.os_version, &target.arch];
    if required.iter().any(|value| value.trim().is_empty()) {
        return Err(PackageResolutionError::InvalidTarget);
    }
    Ok(())
}

fn validate_manager(manager: &ManagerBindingV1) -> Result<(), PackageResolutionError> {
    if manager.version.trim().is_empty()
        || manager.version.contains(char::is_whitespace)
        || manager
            .version
            .contains(['*', '^', '~', '<', '>', '=', ','])
        || contains_floating_word(&manager.version)
    {
        return Err(PackageResolutionError::InvalidManagerBinding);
    }
    Ok(())
}

fn validate_source(source: &SourceBindingV1) -> Result<(), PackageResolutionError> {
    let revision_is_immutable = source
        .repository_revision
        .as_deref()
        .is_some_and(|revision| {
            matches!(revision.len(), 40 | 64)
                && revision.chars().all(|character| {
                    character.is_ascii_hexdigit() && !character.is_ascii_uppercase()
                })
        });
    if !source.canonical_repository.starts_with("https://")
        || (!revision_is_immutable && source.signed_metadata.is_empty())
    {
        return Err(PackageResolutionError::MutableSourceMetadata);
    }
    Ok(())
}

fn validate_fetch_request(request: &PackageFetchRequestV1) -> Result<(), PackageResolutionError> {
    if !request.immutable_locator.starts_with("https://")
        || contains_floating_word(&request.immutable_locator)
    {
        return Err(PackageResolutionError::MutableArtifactLocator);
    }
    Ok(())
}

fn contains_floating_word(value: &str) -> bool {
    value
        .split(|character: char| !character.is_ascii_alphanumeric())
        .any(|part| {
            matches!(
                part.to_ascii_lowercase().as_str(),
                "latest" | "stable" | "default" | "node"
            )
        })
}

fn canonicalize_draft(draft: &mut PackageResolutionDraftV1) -> Result<(), PackageResolutionError> {
    for package in &draft.closure {
        package.declaration.validate()?;
    }
    draft.closure.sort_by(|left, right| {
        (
            &left.declaration.manager,
            &left.declaration.id,
            &left.declaration.version,
        )
            .cmp(&(
                &right.declaration.manager,
                &right.declaration.id,
                &right.declaration.version,
            ))
    });
    if draft.closure.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(PackageResolutionError::DuplicateClosurePackage);
    }
    draft.artifacts.sort();
    Ok(())
}

fn validate_and_persist_artifacts(
    expected: &[PackageFetchRequestV1],
    mut fetched: Vec<(PackageFetchRequestV1, Vec<u8>)>,
    source_metadata_digest: &Sha256Digest,
    store: &ArtifactStore,
) -> Result<Vec<PackageArtifactV1>, PackageResolutionError> {
    fetched.sort_by(|left, right| left.0.cmp(&right.0));
    if expected.windows(2).any(|pair| pair[0] == pair[1])
        || fetched.windows(2).any(|pair| pair[0].0 == pair[1].0)
    {
        return Err(PackageResolutionError::DuplicateArtifact);
    }
    if expected.len() != fetched.len()
        || expected
            .iter()
            .zip(&fetched)
            .any(|(left, right)| left != &right.0)
    {
        return Err(PackageResolutionError::ArtifactSetMismatch);
    }
    let mut artifacts = Vec::with_capacity(expected.len());
    let mut keys = BTreeSet::new();
    for (request, bytes) in &fetched {
        if !keys.insert(request.materialization_key.clone()) {
            return Err(PackageResolutionError::DuplicateArtifact);
        }
        if request.source_metadata_digest != *source_metadata_digest
            || u64::try_from(bytes.len()).ok() != Some(request.size)
            || Sha256Digest::parse(format!("sha256:{:x}", Sha256::digest(bytes)))?
                != request.upstream_checksum
        {
            return Err(PackageResolutionError::CorruptArtifact);
        }
    }
    for (request, bytes) in fetched {
        let content = store.put(&bytes, ContentSensitivity::Portable)?;
        if content.digest != request.upstream_checksum || content.bytes != request.size {
            return Err(PackageResolutionError::CorruptArtifact);
        }
        artifacts.push(PackageArtifactV1 {
            role: request.role,
            content,
            upstream_checksum: request.upstream_checksum,
            size: request.size,
            materialization_key: request.materialization_key,
            source_metadata_digest: request.source_metadata_digest,
        });
    }
    Ok(artifacts)
}

fn validate_recipe_roles(
    recipe: &OfflineInstallRecipeV1,
    roles: impl IntoIterator<Item = StableId>,
) -> Result<(), PackageResolutionError> {
    let expected = match recipe {
        OfflineInstallRecipeV1::HomebrewBottle { artifact_roles }
        | OfflineInstallRecipeV1::AptArchives { artifact_roles }
        | OfflineInstallRecipeV1::NodeArchive { artifact_roles }
        | OfflineInstallRecipeV1::RustToolchain { artifact_roles } => artifact_roles,
    };
    let actual = roles.into_iter().collect::<BTreeSet<_>>();
    if &actual != expected {
        return Err(PackageResolutionError::ArtifactSetMismatch);
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum PackageResolutionError {
    #[error("package source is not allowed by policy: {source_id}")]
    PackageSourceNotAllowed { source_id: StableId },
    #[error("package resolution backend is unavailable for {manager:?}")]
    ResolverUnavailable { manager: PackageManager },
    #[error("package fetch capability is unavailable")]
    FetchUnavailable,
    #[error("package target tuple is incomplete")]
    InvalidTarget,
    #[error("package manager binding is incomplete or floating")]
    InvalidManagerBinding,
    #[error("package manager binding does not match declaration/backend")]
    ManagerMismatch,
    #[error("controlled source registry has no entry for {source_id}")]
    UnknownSource { source_id: StableId },
    #[error("controlled source is not valid for the declared manager")]
    SourceManagerMismatch,
    #[error("package source lacks immutable repository metadata")]
    MutableSourceMetadata,
    #[error("package artifact locator is not immutable")]
    MutableArtifactLocator,
    #[error("package resolution omitted the requested root package")]
    MissingRootPackage,
    #[error("package resolution closure contains a duplicate")]
    DuplicateClosurePackage,
    #[error("package resolution artifact set does not exactly match fetched artifacts")]
    ArtifactSetMismatch,
    #[error("package resolution contains duplicate artifact references")]
    DuplicateArtifact,
    #[error("fetched package artifact failed digest, size, or metadata verification")]
    CorruptArtifact,
    #[error("controlled package source appears more than once")]
    DuplicateSource,
    #[error("persisted package resolution does not match its intent")]
    ResolutionBindingMismatch,
    #[error("persisted package target binding changed")]
    TargetBindingMismatch,
    #[error("persisted package manager binding changed")]
    ManagerBindingMismatch,
    #[error("persisted controlled source binding changed")]
    SourceBindingMismatch,
    #[error("package source metadata evidence contains a duplicate")]
    DuplicateSourceMetadata,
    #[error(transparent)]
    Artifact(#[from] ArtifactError),
    #[error(transparent)]
    Contract(#[from] ContractError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Provider(#[from] ProviderContractError),
}
