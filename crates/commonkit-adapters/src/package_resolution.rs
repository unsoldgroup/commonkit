use std::collections::{BTreeMap, BTreeSet};

use commonkit_contracts::{
    ContractError, PackageDeclaration, PackageManager, PackageSelector, SchemaVersion,
    SecurityPolicy, Sha256Digest, StableId, digest_domain_json,
};
use commonkit_core::enforce_package_source_policy;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    ArtifactError, ArtifactStore, ContentReference, ContentSensitivity, MaterializedState,
    PackageDesiredIntent, ProviderContractError, ResolvedMaterializedState, ResourceIntent,
};

pub const COMMONKIT_NODE_RELEASE_KEY_FINGERPRINTS: &[&str] = &[
    "5be8a3f6c8a5c01d106c0ad820b1a390b168d356",
    "dd792f5973c6de52c432cbdac77abfa00ddbf2b7",
    "cc68f5a3106ff448322e48ed27f5e38d5b0a215f",
    "8fcca13fef1d0c2e91008e09770f7a9a5ae15600",
    "890c08db8579162fee0df9db8beab4dfcf555ef4",
    "c82fa3ae1cbedc6be46b9360c43cec45c17ab93c",
    "108f52b48db57bb0cc439b2997b01419bd92f80a",
    "a363a499291cbbc940dd62e41f10027af002f8b0",
    "71dcfd284a79c3b38668286bc97ec7a07ede3fc1",
    "86c8d74642e67846f8e120284daa80d1e737bc9f",
];

pub const COMMONKIT_NVM_SCRIPT_RELEASES: &[(&str, &str)] = &[(
    "0.40.6",
    "sha256:baad94563757afa5147950e3dfa272f06b6307e1a7d92c553f8438959b5165fe",
)];

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
    pub source: SourceBindingV1,
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
    HomebrewBottle {
        artifact_roles: BTreeSet<StableId>,
    },
    AptArchives {
        artifact_roles: BTreeSet<StableId>,
    },
    NodeArchive {
        artifact_roles: BTreeSet<StableId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        install: Option<NodeOfflineInstallRecipeV1>,
    },
    RustToolchain {
        artifact_roles: BTreeSet<StableId>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NodeOfflineInstallRecipeV1 {
    pub node_version: String,
    pub archive_file_name: String,
    pub cache_relative_path: String,
    pub nvm_version: String,
    pub nvm_script_digest: Sha256Digest,
    pub shell_executable_digest: Sha256Digest,
    pub offline: bool,
    pub no_source_fallback: bool,
    pub per_version_lock: bool,
    pub install_latest_npm: bool,
    pub migrate_packages: bool,
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CompatibleResolvedPackageV1 {
    declaration: PackageDeclaration,
    #[serde(default, deserialize_with = "deserialize_present_source")]
    source: Option<SourceBindingV1>,
}

fn deserialize_present_source<'de, D>(deserializer: D) -> Result<Option<SourceBindingV1>, D::Error>
where
    D: Deserializer<'de>,
{
    SourceBindingV1::deserialize(deserializer).map(Some)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CompatiblePackageResolutionV1 {
    schema_version: SchemaVersion,
    declaration: PackageDeclaration,
    target: PackageTargetV1,
    manager: ManagerBindingV1,
    source: SourceBindingV1,
    before: PackageObservationV1,
    closure: Vec<CompatibleResolvedPackageV1>,
    artifacts: Vec<PackageArtifactV1>,
    recipe: OfflineInstallRecipeV1,
}

impl CompatiblePackageResolutionV1 {
    fn inherit_missing_parent_sources(self) -> PackageResolutionV1 {
        let source = self.source;
        PackageResolutionV1 {
            schema_version: self.schema_version,
            declaration: self.declaration,
            target: self.target,
            manager: self.manager,
            closure: self
                .closure
                .into_iter()
                .map(|package| ResolvedPackage {
                    declaration: package.declaration,
                    source: package.source.unwrap_or_else(|| source.clone()),
                })
                .collect(),
            source,
            before: self.before,
            artifacts: self.artifacts,
            recipe: self.recipe,
        }
    }
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
    // Early v1 artifacts omitted the closure-level source. The dual loader
    // inherits the top-level source and validates the strict current model.
    if let Some(required) = schema["$defs"]["ResolvedPackage"]["required"].as_array_mut() {
        required.retain(|field| field != "source");
    }
    if let Some(node_recipe) = schema["$defs"]["OfflineInstallRecipeV1"]["oneOf"]
        .as_array_mut()
        .and_then(|variants| variants.get_mut(2))
    {
        node_recipe["properties"]
            .as_object_mut()
            .map(|properties| properties.remove("install"));
    }
    schema["$defs"]
        .as_object_mut()
        .map(|definitions| definitions.remove("NodeOfflineInstallRecipeV1"));
    schema["$id"] = serde_json::Value::String(
        "https://schemas.commonkit.dev/v1/package-resolution.schema.json".into(),
    );
    Ok(schema)
}

pub fn package_resolution_v2_schema() -> Result<serde_json::Value, serde_json::Error> {
    let mut schema = serde_json::to_value(schema_for!(PackageResolutionV1))?;
    schema["properties"]["schemaVersion"] = serde_json::json!({
        "const": 2,
        "type": "integer"
    });
    schema["$defs"]["PackageManager"] = serde_json::json!({
        "const": "nvm",
        "type": "string"
    });
    let node_selector = schema["$defs"]["PackageSelector"]["oneOf"][2].take();
    schema["$defs"]["PackageSelector"] = node_selector;
    schema["$defs"]["PackageDeclaration"]["properties"]["selector"] = serde_json::json!({
        "$ref": "#/$defs/PackageSelector"
    });
    if let Some(required) = schema["$defs"]["PackageDeclaration"]["required"].as_array_mut() {
        required.push(serde_json::Value::String("selector".into()));
    }
    let mut node_recipe = schema["$defs"]["OfflineInstallRecipeV1"]["oneOf"][2].take();
    node_recipe["properties"]["install"] = serde_json::json!({
        "$ref": "#/$defs/NodeOfflineInstallRecipeV1"
    });
    if let Some(required) = node_recipe["required"].as_array_mut() {
        required.push(serde_json::Value::String("install".into()));
    }
    schema["$defs"]["OfflineInstallRecipeV1"] = node_recipe;
    for (property, value) in [
        ("offline", true),
        ("noSourceFallback", true),
        ("perVersionLock", true),
        ("installLatestNpm", false),
        ("migratePackages", false),
    ] {
        schema["$defs"]["NodeOfflineInstallRecipeV1"]["properties"][property] = serde_json::json!({
            "const": value,
            "type": "boolean"
        });
    }
    schema["$id"] = serde_json::Value::String(
        "https://schemas.commonkit.dev/v2/package-resolution.schema.json".into(),
    );
    schema["title"] = serde_json::Value::String("PackageResolutionV2".into());
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

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PackageDiscoveryFetchRequestV1 {
    pub role: StableId,
    pub immutable_locator: String,
    pub maximum_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageFetchResultV1 {
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackageFetchHopV1 {
    Complete(PackageFetchResultV1),
    Redirect { location: String },
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
    pub apt_source_authority: Option<&'a AptSourceAuthorityV1>,
    pub node_source_authority: Option<&'a NodeSourceAuthorityV1>,
}

pub trait PackageFetch {
    /// Validates a complete immutable locator set before the first response is
    /// requested. The coordinator's scoped wrapper enforces source roots.
    fn preflight_locators(&mut self, _locators: &[String]) -> Result<(), PackageResolutionError> {
        Ok(())
    }

    /// Fetches exactly one HTTP response. Implementations must disable
    /// automatic redirect following and return every redirect as a hop.
    fn fetch_hop(
        &mut self,
        request: &PackageFetchRequestV1,
        locator: &str,
    ) -> Result<PackageFetchHopV1, PackageResolutionError>;

    /// Fetches authenticated metadata whose digest is learned only after its
    /// signature has been verified. This remains a per-hop capability.
    fn fetch_discovery_hop(
        &mut self,
        request: &PackageDiscoveryFetchRequestV1,
        locator: &str,
    ) -> Result<PackageFetchHopV1, PackageResolutionError> {
        let placeholder = PackageFetchRequestV1 {
            role: request.role.clone(),
            immutable_locator: request.immutable_locator.clone(),
            upstream_checksum: Sha256Digest::parse(format!("sha256:{}", "0".repeat(64)))?,
            size: 0,
            materialization_key: request.role.clone(),
            source_metadata_digest: Sha256Digest::parse(format!("sha256:{}", "0".repeat(64)))?,
        };
        self.fetch_hop(&placeholder, locator)
    }
}

pub struct PackageFetchedResolutionV1 {
    pub probe: PackageResolutionProbeV1,
    pub draft: PackageResolutionDraftV1,
    pub fetched: Vec<(PackageFetchRequestV1, Vec<u8>)>,
}

pub trait PackageResolutionBackend {
    fn manager(&self) -> PackageManager;

    /// Pure/local capability validation performed for the entire desired
    /// state before any resolver receives network authority.
    fn preflight_resolution(
        &mut self,
        _request: &PackageResolutionRequestV1<'_>,
    ) -> Result<(), PackageResolutionError> {
        Ok(())
    }

    fn probe(
        &mut self,
        request: &PackageResolutionRequestV1<'_>,
    ) -> Result<PackageResolutionProbeV1, PackageResolutionError>;

    fn resolve(
        &mut self,
        request: &PackageResolutionRequestV1<'_>,
        source: &SourceBindingV1,
    ) -> Result<PackageResolutionDraftV1, PackageResolutionError>;

    /// Optional staged resolution for registries such as Node where signed
    /// metadata must be fetched before archive checksums are known.
    fn resolve_and_fetch(
        &mut self,
        _request: &PackageResolutionRequestV1<'_>,
        _fetch: &mut dyn PackageFetch,
    ) -> Result<Option<PackageFetchedResolutionV1>, PackageResolutionError> {
        Ok(None)
    }

    fn fetch_artifacts(
        &mut self,
        _request: &PackageResolutionRequestV1<'_>,
        _source: &SourceBindingV1,
        artifacts: &[PackageFetchRequestV1],
        fetch: &mut dyn PackageFetch,
    ) -> Result<(), PackageResolutionError> {
        for artifact in artifacts {
            match fetch.fetch_hop(artifact, &artifact.immutable_locator)? {
                PackageFetchHopV1::Complete(_) => {}
                PackageFetchHopV1::Redirect { .. } => {
                    return Err(PackageResolutionError::UnvalidatedRedirect);
                }
            }
        }
        Ok(())
    }
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
    approved_artifact_roots: &'a BTreeSet<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    apt_source_authority: Option<&'a AptSourceAuthorityV1>,
    #[serde(skip_serializing_if = "Option::is_none")]
    node_source_authority: Option<&'a NodeSourceAuthorityV1>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AptSourceAuthorityV1 {
    pub suite: String,
    pub components: BTreeSet<String>,
    pub signing_authority: StableId,
    pub signing_key_digest: Sha256Digest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NodeSourceAuthorityV1 {
    pub release_key_fingerprints: BTreeSet<String>,
    pub nvm_script_releases: BTreeMap<String, Sha256Digest>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PackageSourceDefinition {
    source_id: StableId,
    manager: PackageManager,
    canonical_repository: String,
    approved_artifact_roots: BTreeSet<String>,
    apt_source_authority: Option<AptSourceAuthorityV1>,
    node_source_authority: Option<NodeSourceAuthorityV1>,
    digest: Sha256Digest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ControlledPackageSourceV1 {
    pub source_id: StableId,
    pub manager: PackageManager,
    pub canonical_repository: String,
    pub approved_artifact_roots: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub apt_source_authority: Option<AptSourceAuthorityV1>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageSourceRegistry {
    sources: BTreeMap<StableId, PackageSourceDefinition>,
    digest: Sha256Digest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageResolutionAuthority {
    target: PackageTargetV1,
    manager: ManagerBindingV1,
    registry: PackageSourceRegistry,
    package_sources: BTreeSet<String>,
    digest: Sha256Digest,
}

fn package_source_definition_digest(
    source: &PackageSourceDefinition,
) -> Result<Sha256Digest, PackageResolutionError> {
    Ok(digest_domain_json(
        if source.node_source_authority.is_some() {
            "commonkit.package-source-registry-definition.v4"
        } else if source.apt_source_authority.is_some() {
            "commonkit.package-source-registry-definition.v2"
        } else {
            "commonkit.package-source-registry-definition.v1"
        },
        &PackageSourceDefinitionV1 {
            source_id: &source.source_id,
            manager: source.manager,
            canonical_repository: &source.canonical_repository,
            approved_artifact_roots: &source.approved_artifact_roots,
            apt_source_authority: source.apt_source_authority.as_ref(),
            node_source_authority: source.node_source_authority.as_ref(),
        },
    )?)
}

fn package_source_registry_digest<'a>(
    sources: impl Iterator<Item = &'a PackageSourceDefinition>,
) -> Result<Sha256Digest, PackageResolutionError> {
    let entries = sources
        .map(|source| PackageSourceDefinitionV1 {
            source_id: &source.source_id,
            manager: source.manager,
            canonical_repository: &source.canonical_repository,
            approved_artifact_roots: &source.approved_artifact_roots,
            apt_source_authority: source.apt_source_authority.as_ref(),
            node_source_authority: source.node_source_authority.as_ref(),
        })
        .collect::<Vec<_>>();
    let domain = if entries
        .iter()
        .any(|entry| entry.node_source_authority.is_some())
    {
        "commonkit.package-source-registry.v4"
    } else if entries
        .iter()
        .any(|entry| entry.apt_source_authority.is_some())
    {
        "commonkit.package-source-registry.v2"
    } else {
        "commonkit.package-source-registry.v1"
    };
    Ok(digest_domain_json(domain, &entries)?)
}

impl PackageResolutionAuthority {
    pub fn new(
        target: &PackageTargetV1,
        manager: &ManagerBindingV1,
        registry: &PackageSourceRegistry,
        policy: &SecurityPolicy,
    ) -> Result<Self, PackageResolutionError> {
        validate_target(target)?;
        validate_manager(manager)?;
        let package_sources = effective_package_sources(policy);
        let digest = digest_domain_json(
            "commonkit.package-resolution-authority.v1",
            &(target, manager, registry.digest(), &package_sources),
        )?;
        Ok(Self {
            target: target.clone(),
            manager: manager.clone(),
            registry: registry.clone(),
            package_sources,
            digest,
        })
    }

    pub fn digest(&self) -> &Sha256Digest {
        &self.digest
    }

    pub fn target(&self) -> &PackageTargetV1 {
        &self.target
    }

    pub fn manager(&self) -> &ManagerBindingV1 {
        &self.manager
    }

    pub fn load_by_resolution_digest(
        &self,
        resolution_digest: &Sha256Digest,
        store: &ArtifactStore,
    ) -> Result<(ResolvedPackageIntent, PackageResolutionV1), PackageResolutionError> {
        let bytes = store.load_by_digest(resolution_digest)?;
        let resolution = serde_json::from_slice::<PackageResolutionV1>(&bytes).or_else(|_| {
            serde_json::from_slice::<CompatiblePackageResolutionV1>(&bytes)
                .map(CompatiblePackageResolutionV1::inherit_missing_parent_sources)
        })?;
        let resolution_reference = ContentReference {
            digest: resolution_digest.clone(),
            bytes: bytes
                .len()
                .try_into()
                .map_err(|_| PackageResolutionError::CorruptArtifact)?,
            sensitivity: ContentSensitivity::Portable,
        };
        let intent = ResolvedPackageIntent {
            declaration: resolution.declaration.clone(),
            resolution: resolution_reference,
            artifacts: resolution
                .artifacts
                .iter()
                .map(|artifact| artifact.content.clone())
                .collect(),
        };
        let validated = self.validate(&intent, store)?;
        Ok((intent, validated))
    }

    pub fn validate(
        &self,
        intent: &ResolvedPackageIntent,
        store: &ArtifactStore,
    ) -> Result<PackageResolutionV1, PackageResolutionError> {
        let resolution =
            intent.load_and_validate(&self.target, &self.manager, &self.registry, store)?;
        enforce_current_package_sources(&self.package_sources, &resolution.declaration)?;
        for package in &resolution.closure {
            enforce_current_package_sources(&self.package_sources, &package.declaration)?;
        }
        Ok(resolution)
    }
}

fn effective_package_sources(policy: &SecurityPolicy) -> BTreeSet<String> {
    let key = StableId::parse("package_sources").expect("static stable ID");
    policy.allowlists.get(&key).cloned().unwrap_or_default()
}

fn enforce_current_package_sources(
    allowed: &BTreeSet<String>,
    declaration: &PackageDeclaration,
) -> Result<(), PackageResolutionError> {
    if allowed.contains(declaration.source.as_str()) {
        Ok(())
    } else {
        Err(PackageResolutionError::PackageSourceNotAllowed {
            source_id: declaration.source.clone(),
        })
    }
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
        for mut source in sources {
            source
                .approved_artifact_roots
                .insert(source.canonical_repository.clone());
            if !source.canonical_repository.starts_with("https://")
                || source
                    .approved_artifact_roots
                    .iter()
                    .any(|root| !root.starts_with("https://"))
            {
                return Err(PackageResolutionError::MutableSourceMetadata);
            }
            if source
                .apt_source_authority
                .as_ref()
                .is_some_and(|authority| {
                    source.manager != PackageManager::Apt
                        || authority.suite.is_empty()
                        || authority.components.is_empty()
                })
            {
                return Err(PackageResolutionError::MutableSourceMetadata);
            }
            let digest = digest_domain_json(
                if source.apt_source_authority.is_some() {
                    "commonkit.package-source-registry-definition.v2"
                } else {
                    "commonkit.package-source-registry-definition.v1"
                },
                &PackageSourceDefinitionV1 {
                    source_id: &source.source_id,
                    manager: source.manager,
                    canonical_repository: &source.canonical_repository,
                    approved_artifact_roots: &source.approved_artifact_roots,
                    apt_source_authority: source.apt_source_authority.as_ref(),
                    node_source_authority: None,
                },
            )?;
            if registry
                .insert(
                    source.source_id.clone(),
                    PackageSourceDefinition {
                        source_id: source.source_id,
                        manager: source.manager,
                        canonical_repository: source.canonical_repository,
                        approved_artifact_roots: source.approved_artifact_roots,
                        apt_source_authority: source.apt_source_authority,
                        node_source_authority: None,
                        digest,
                    },
                )
                .is_some()
            {
                return Err(PackageResolutionError::DuplicateSource);
            }
        }
        let entries = registry
            .values()
            .map(|source| PackageSourceDefinitionV1 {
                source_id: &source.source_id,
                manager: source.manager,
                canonical_repository: &source.canonical_repository,
                approved_artifact_roots: &source.approved_artifact_roots,
                apt_source_authority: source.apt_source_authority.as_ref(),
                node_source_authority: source.node_source_authority.as_ref(),
            })
            .collect::<Vec<_>>();
        let domain = if entries
            .iter()
            .any(|entry| entry.node_source_authority.is_some())
        {
            "commonkit.package-source-registry.v3"
        } else if entries
            .iter()
            .any(|entry| entry.apt_source_authority.is_some())
        {
            "commonkit.package-source-registry.v2"
        } else {
            "commonkit.package-source-registry.v1"
        };
        let digest = digest_domain_json(domain, &entries)?;
        Ok(Self {
            sources: registry,
            digest,
        })
    }

    pub fn source_definition_digest(
        &self,
        source_id: &StableId,
    ) -> Result<Sha256Digest, PackageResolutionError> {
        self.sources
            .get(source_id)
            .map(|source| source.digest.clone())
            .ok_or_else(|| PackageResolutionError::UnknownSource {
                source_id: source_id.clone(),
            })
    }

    pub fn with_apt_source_authority(
        mut self,
        source_id: &StableId,
        authority: AptSourceAuthorityV1,
    ) -> Result<Self, PackageResolutionError> {
        if authority.suite.is_empty() || authority.components.is_empty() {
            return Err(PackageResolutionError::MutableSourceMetadata);
        }
        let source = self.sources.get_mut(source_id).ok_or_else(|| {
            PackageResolutionError::UnknownSource {
                source_id: source_id.clone(),
            }
        })?;
        if source.manager != PackageManager::Apt {
            return Err(PackageResolutionError::SourceManagerMismatch);
        }
        source.apt_source_authority = Some(authority);
        source.digest = package_source_definition_digest(source)?;
        self.digest = package_source_registry_digest(self.sources.values())?;
        Ok(self)
    }

    pub fn with_node_source_authority(
        mut self,
        source_id: &StableId,
        authority: NodeSourceAuthorityV1,
    ) -> Result<Self, PackageResolutionError> {
        if !valid_node_release_fingerprints(&authority.release_key_fingerprints)
            || !valid_nvm_script_releases(&authority.nvm_script_releases)
        {
            return Err(PackageResolutionError::MutableSourceMetadata);
        }
        let source = self.sources.get_mut(source_id).ok_or_else(|| {
            PackageResolutionError::UnknownSource {
                source_id: source_id.clone(),
            }
        })?;
        if source.manager != PackageManager::Nvm {
            return Err(PackageResolutionError::SourceManagerMismatch);
        }
        source.node_source_authority = Some(authority);
        source.digest = package_source_definition_digest(source)?;
        self.digest = package_source_registry_digest(self.sources.values())?;
        Ok(self)
    }

    pub fn with_commonkit_node_release_authority(
        self,
        source_id: &StableId,
    ) -> Result<Self, PackageResolutionError> {
        self.with_node_source_authority(
            source_id,
            NodeSourceAuthorityV1 {
                release_key_fingerprints: COMMONKIT_NODE_RELEASE_KEY_FINGERPRINTS
                    .iter()
                    .map(|fingerprint| (*fingerprint).into())
                    .collect(),
                nvm_script_releases: commonkit_nvm_script_releases()?,
            },
        )
    }

    pub fn digest(&self) -> &Sha256Digest {
        &self.digest
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
        approved_artifact_roots: BTreeSet::new(),
        apt_source_authority: None,
    })
}

pub struct PackageResolutionCoordinator<'a> {
    policy: &'a SecurityPolicy,
    registry: &'a PackageSourceRegistry,
    manager: ManagerBindingV1,
    backend: &'a mut dyn PackageResolutionBackend,
    fetch: &'a mut dyn PackageFetch,
}

struct PreparedPackageResolution {
    desired: PackageDesiredIntent,
    definition: PackageSourceDefinition,
    source: SourceBindingV1,
    before: PackageObservationV1,
    draft: PackageResolutionDraftV1,
    fetched: Option<Vec<(PackageFetchRequestV1, Vec<u8>)>>,
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
        let prepared = self.prepare_resolution(desired, target)?;
        self.persist_prepared(prepared, target, store)
    }

    fn prepare_resolution(
        &mut self,
        desired: &PackageDesiredIntent,
        target: &PackageTargetV1,
    ) -> Result<PreparedPackageResolution, PackageResolutionError> {
        let definition = self.preflight_desired(desired, target)?;
        let PackageDesiredIntent::Package { declaration } = desired;
        let request = PackageResolutionRequestV1 {
            desired,
            target,
            manager: &self.manager,
            source_id: &definition.source_id,
            canonical_repository: &definition.canonical_repository,
            registry_definition_digest: &definition.digest,
            apt_source_authority: definition.apt_source_authority.as_ref(),
            node_source_authority: definition.node_source_authority.as_ref(),
        };
        self.backend.preflight_resolution(&request)?;
        let mut scoped_fetch = RecordingPackageFetch {
            inner: self.fetch,
            fetched: Vec::new(),
            approved_artifact_roots: &definition.approved_artifact_roots,
        };
        let fetched_resolution = self
            .backend
            .resolve_and_fetch(&request, &mut scoped_fetch)?;
        let (mut probe, mut draft, fetched) = if let Some(fetched) = fetched_resolution {
            (fetched.probe, fetched.draft, Some(fetched.fetched))
        } else {
            let probe = self.backend.probe(&request)?;
            let source = source_binding(&definition, &probe);
            validate_source(&source)?;
            let draft = self.backend.resolve(&request, &source)?;
            (probe, draft, None)
        };
        probe.signed_metadata.sort();
        if probe
            .signed_metadata
            .windows(2)
            .any(|pair| pair[0] == pair[1])
        {
            return Err(PackageResolutionError::DuplicateSourceMetadata);
        }
        let source = source_binding(&definition, &probe);
        validate_source(&source)?;
        canonicalize_draft(&mut draft)?;
        self.validate_closure(&draft.closure, declaration, &source)?;
        validate_recipe_roles(
            &draft.recipe,
            draft.artifacts.iter().map(|artifact| artifact.role.clone()),
        )?;
        Ok(PreparedPackageResolution {
            desired: desired.clone(),
            definition,
            source,
            before: probe.before,
            draft,
            fetched,
        })
    }

    fn persist_prepared(
        &mut self,
        prepared: PreparedPackageResolution,
        target: &PackageTargetV1,
        store: &ArtifactStore,
    ) -> Result<ResolvedPackageIntent, PackageResolutionError> {
        let PreparedPackageResolution {
            desired,
            definition,
            source,
            before,
            draft,
            fetched,
        } = prepared;
        let PackageDesiredIntent::Package { declaration } = &desired;
        let request = PackageResolutionRequestV1 {
            desired: &desired,
            target,
            manager: &self.manager,
            source_id: &definition.source_id,
            canonical_repository: &definition.canonical_repository,
            registry_definition_digest: &definition.digest,
            apt_source_authority: definition.apt_source_authority.as_ref(),
            node_source_authority: definition.node_source_authority.as_ref(),
        };
        let source_metadata_digest = source.metadata_digest()?;
        for artifact in &draft.artifacts {
            validate_fetch_request(artifact, &definition.approved_artifact_roots)?;
        }
        let fetched = if let Some(fetched) = fetched {
            fetched
        } else {
            let mut recording_fetch = RecordingPackageFetch {
                inner: self.fetch,
                fetched: Vec::new(),
                approved_artifact_roots: &definition.approved_artifact_roots,
            };
            self.backend.fetch_artifacts(
                &request,
                &source,
                &draft.artifacts,
                &mut recording_fetch,
            )?;
            recording_fetch.fetched
        };
        let artifacts = validate_and_persist_artifacts(
            &draft.artifacts,
            fetched,
            &source_metadata_digest,
            store,
        )?;
        let resolution = PackageResolutionV1 {
            schema_version: SchemaVersion(if self.manager.manager == PackageManager::Nvm {
                2
            } else {
                1
            }),
            declaration: declaration.clone(),
            target: target.clone(),
            manager: self.manager.clone(),
            source,
            before,
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
        for resource in &state.resources {
            if let ResourceIntent::Package(desired) = &resource.intent {
                let definition = self.preflight_desired(desired, target)?;
                let request = PackageResolutionRequestV1 {
                    desired,
                    target,
                    manager: &self.manager,
                    source_id: &definition.source_id,
                    canonical_repository: &definition.canonical_repository,
                    registry_definition_digest: &definition.digest,
                    apt_source_authority: definition.apt_source_authority.as_ref(),
                    node_source_authority: definition.node_source_authority.as_ref(),
                };
                self.backend.preflight_resolution(&request)?;
            }
        }
        let mut prepared = Vec::new();
        for resource in &state.resources {
            if let ResourceIntent::Package(desired) = &resource.intent {
                prepared.push(self.prepare_resolution(desired, target)?);
            }
        }
        let mut prepared = prepared.into_iter();
        let mut resources = Vec::with_capacity(state.resources.len());
        for resource in &state.resources {
            let intent = match &resource.intent {
                ResourceIntent::Filesystem(intent) => ResourceIntent::Filesystem(intent.clone()),
                ResourceIntent::Package(desired) => {
                    let prepared = prepared
                        .next()
                        .ok_or(PackageResolutionError::ResolutionBindingMismatch)?;
                    if &prepared.desired != desired {
                        return Err(PackageResolutionError::ResolutionBindingMismatch);
                    }
                    ResourceIntent::ResolvedPackage(self.persist_prepared(prepared, target, store)?)
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

    fn preflight_desired(
        &self,
        desired: &PackageDesiredIntent,
        target: &PackageTargetV1,
    ) -> Result<PackageSourceDefinition, PackageResolutionError> {
        let PackageDesiredIntent::Package { declaration } = desired;
        declaration.validate_for_resolution()?;
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
            })?
            .clone();
        if definition.manager != declaration.manager || definition.source_id != declaration.source {
            return Err(PackageResolutionError::SourceManagerMismatch);
        }
        Ok(definition)
    }

    fn validate_closure(
        &self,
        closure: &[ResolvedPackage],
        root: &PackageDeclaration,
        source: &SourceBindingV1,
    ) -> Result<(), PackageResolutionError> {
        if !closure.iter().any(|package| {
            resolved_root_matches(&package.declaration, root) && package.source == *source
        }) {
            return Err(PackageResolutionError::MissingRootPackage);
        }
        for package in closure {
            package.declaration.validate_for_resolution()?;
            enforce_package_source_policy(self.policy, &package.declaration).map_err(|_| {
                PackageResolutionError::PackageSourceNotAllowed {
                    source_id: package.declaration.source.clone(),
                }
            })?;
            let definition = self
                .registry
                .sources
                .get(&package.declaration.source)
                .ok_or_else(|| PackageResolutionError::UnknownSource {
                    source_id: package.declaration.source.clone(),
                })?;
            if package.declaration.manager != self.manager.manager
                || definition.manager != package.declaration.manager
                || package.source.source_id != definition.source_id
                || package.source.registry_definition_digest != definition.digest
                || package.source.canonical_repository != definition.canonical_repository
                || package.source != *source
            {
                return Err(PackageResolutionError::SourceBindingMismatch);
            }
            validate_source(&package.source)?;
        }
        Ok(())
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

fn source_binding(
    definition: &PackageSourceDefinition,
    probe: &PackageResolutionProbeV1,
) -> SourceBindingV1 {
    SourceBindingV1 {
        source_id: definition.source_id.clone(),
        registry_definition_digest: definition.digest.clone(),
        canonical_repository: definition.canonical_repository.clone(),
        repository_revision: probe.repository_revision.clone(),
        signed_metadata: probe.signed_metadata.clone(),
    }
}

impl ResolvedPackageIntent {
    pub fn load_persisted(
        &self,
        store: &ArtifactStore,
    ) -> Result<PackageResolutionV1, PackageResolutionError> {
        let bytes = store.load(&self.resolution)?;
        let resolution = serde_json::from_slice::<PackageResolutionV1>(&bytes).or_else(|_| {
            serde_json::from_slice::<CompatiblePackageResolutionV1>(&bytes)
                .map(CompatiblePackageResolutionV1::inherit_missing_parent_sources)
        })?;
        if !matches!(
            resolution.schema_version,
            SchemaVersion(1) | SchemaVersion(2)
        ) || resolution.declaration != self.declaration
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
    resolution.declaration.validate_for_resolution()?;
    if (resolution.declaration.manager == PackageManager::Nvm
        && resolution.schema_version != SchemaVersion(2))
        || (resolution.declaration.manager != PackageManager::Nvm
            && resolution.schema_version != SchemaVersion(1))
    {
        return Err(PackageResolutionError::ResolutionBindingMismatch);
    }
    validate_target(&resolution.target)?;
    validate_manager(&resolution.manager)?;
    validate_source(&resolution.source)?;
    if resolution.declaration.source != resolution.source.source_id {
        return Err(PackageResolutionError::SourceBindingMismatch);
    }
    if resolution.declaration.manager != resolution.manager.manager {
        return Err(PackageResolutionError::ManagerBindingMismatch);
    }
    let source_metadata_digest = resolution.source.metadata_digest()?;
    let mut previous_closure = None;
    for package in &resolution.closure {
        package.declaration.validate_for_resolution()?;
        if package.source != resolution.source
            || package.declaration.source != package.source.source_id
        {
            return Err(PackageResolutionError::SourceBindingMismatch);
        }
        if package.declaration.manager != resolution.manager.manager {
            return Err(PackageResolutionError::ManagerBindingMismatch);
        }
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
        .any(|package| resolved_root_matches(&package.declaration, &resolution.declaration))
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
    )?;
    let expected_node_recipe = if resolution.manager.manager == PackageManager::Nvm {
        let version = resolution
            .declaration
            .version
            .strip_prefix('v')
            .unwrap_or(&resolution.declaration.version);
        let platform = crate::node_resolution::node_platform(&resolution.target)?;
        let archive = format!("node-v{version}-{platform}.tar.xz");
        Some((
            version,
            archive.clone(),
            format!(".cache/bin/node-v{version}-{platform}/{archive}"),
        ))
    } else {
        None
    };
    match (&resolution.manager.manager, &resolution.recipe) {
        (
            PackageManager::Nvm,
            OfflineInstallRecipeV1::NodeArchive {
                install: Some(install),
                ..
            },
        ) if expected_node_recipe.as_ref().is_some_and(
            |(version, archive_file_name, cache_relative_path)| {
                install.node_version == *version
                    && install.archive_file_name == *archive_file_name
                    && install.cache_relative_path == *cache_relative_path
            },
        ) && install.nvm_version == resolution.manager.version
            && install.nvm_script_digest == resolution.manager.executable_digest
            && install.offline
            && install.no_source_fallback
            && install.per_version_lock
            && !install.install_latest_npm
            && !install.migrate_packages =>
        {
            Ok(())
        }
        (PackageManager::Nvm, _) => Err(PackageResolutionError::InvalidNodeRequest),
        (_, _) => Ok(()),
    }
}

fn resolved_root_matches(resolved: &PackageDeclaration, requested: &PackageDeclaration) -> bool {
    if resolved == requested {
        return true;
    }
    if resolved.id != requested.id
        || resolved.version != requested.version
        || resolved.manager != PackageManager::Apt
        || requested.manager != PackageManager::Apt
        || resolved.source != requested.source
    {
        return false;
    }
    let (
        Some(PackageSelector::AptBinary {
            name: resolved_name,
            architecture: Some(resolved_architecture),
        }),
        Some(PackageSelector::AptBinary {
            name: requested_name,
            architecture: Some(requested_architecture),
        }),
    ) = (&resolved.selector, &requested.selector)
    else {
        return false;
    };
    resolved_name == requested_name
        && (resolved_architecture == requested_architecture || resolved_architecture == "all")
}

struct RecordingPackageFetch<'a> {
    inner: &'a mut dyn PackageFetch,
    fetched: Vec<(PackageFetchRequestV1, Vec<u8>)>,
    approved_artifact_roots: &'a BTreeSet<String>,
}

impl PackageFetch for RecordingPackageFetch<'_> {
    fn preflight_locators(&mut self, locators: &[String]) -> Result<(), PackageResolutionError> {
        for locator in locators {
            validate_fetch_locator(locator, self.approved_artifact_roots)?;
        }
        self.inner.preflight_locators(locators)
    }

    fn fetch_hop(
        &mut self,
        request: &PackageFetchRequestV1,
        locator: &str,
    ) -> Result<PackageFetchHopV1, PackageResolutionError> {
        if locator != request.immutable_locator {
            return Err(PackageResolutionError::ResolutionBindingMismatch);
        }
        validate_fetch_request(request, self.approved_artifact_roots)?;
        let mut current = locator.to_owned();
        let mut visited = BTreeSet::new();
        for _ in 0..=10 {
            if !visited.insert(current.clone()) {
                return Err(PackageResolutionError::RedirectLoop);
            }
            let result = self.inner.fetch_hop(request, &current)?;
            match result {
                PackageFetchHopV1::Complete(result) => {
                    self.fetched.push((request.clone(), result.bytes.clone()));
                    return Ok(PackageFetchHopV1::Complete(result));
                }
                PackageFetchHopV1::Redirect { location } => {
                    validate_fetch_locator(&location, self.approved_artifact_roots)?;
                    current = location;
                }
            }
        }
        Err(PackageResolutionError::TooManyRedirects)
    }

    fn fetch_discovery_hop(
        &mut self,
        request: &PackageDiscoveryFetchRequestV1,
        locator: &str,
    ) -> Result<PackageFetchHopV1, PackageResolutionError> {
        if locator != request.immutable_locator {
            return Err(PackageResolutionError::ResolutionBindingMismatch);
        }
        validate_fetch_locator(locator, self.approved_artifact_roots)?;
        let mut current = locator.to_owned();
        let mut visited = BTreeSet::new();
        for _ in 0..=10 {
            if !visited.insert(current.clone()) {
                return Err(PackageResolutionError::RedirectLoop);
            }
            let result = self.inner.fetch_discovery_hop(request, &current)?;
            match result {
                PackageFetchHopV1::Complete(result) => {
                    if u64::try_from(result.bytes.len())
                        .ok()
                        .is_none_or(|size| size > request.maximum_bytes)
                    {
                        return Err(PackageResolutionError::CorruptArtifact);
                    }
                    return Ok(PackageFetchHopV1::Complete(result));
                }
                PackageFetchHopV1::Redirect { location } => {
                    validate_fetch_locator(&location, self.approved_artifact_roots)?;
                    current = location;
                }
            }
        }
        Err(PackageResolutionError::TooManyRedirects)
    }
}

fn valid_node_release_fingerprints(fingerprints: &BTreeSet<String>) -> bool {
    let approved = COMMONKIT_NODE_RELEASE_KEY_FINGERPRINTS
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    !fingerprints.is_empty()
        && fingerprints.iter().all(|fingerprint| {
            fingerprint.len() == 40
                && fingerprint.chars().all(|character| {
                    character.is_ascii_hexdigit() && !character.is_ascii_uppercase()
                })
                && approved.contains(fingerprint.as_str())
        })
}

fn valid_nvm_script_releases(releases: &BTreeMap<String, Sha256Digest>) -> bool {
    !releases.is_empty()
        && releases.iter().all(|(version, digest)| {
            COMMONKIT_NVM_SCRIPT_RELEASES
                .iter()
                .any(|(approved_version, approved_digest)| {
                    version == approved_version && digest.as_str() == *approved_digest
                })
        })
}

fn commonkit_nvm_script_releases() -> Result<BTreeMap<String, Sha256Digest>, PackageResolutionError>
{
    COMMONKIT_NVM_SCRIPT_RELEASES
        .iter()
        .map(|(version, digest)| Ok(((*version).into(), Sha256Digest::parse(*digest)?)))
        .collect()
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

fn validate_fetch_request(
    request: &PackageFetchRequestV1,
    approved_artifact_roots: &BTreeSet<String>,
) -> Result<(), PackageResolutionError> {
    validate_fetch_locator(&request.immutable_locator, approved_artifact_roots)
}

fn validate_fetch_locator(
    locator: &str,
    approved_artifact_roots: &BTreeSet<String>,
) -> Result<(), PackageResolutionError> {
    if !locator.starts_with("https://") || contains_floating_artifact_word(locator) {
        return Err(PackageResolutionError::MutableArtifactLocator);
    }
    if !approved_artifact_roots
        .iter()
        .any(|root| locator_is_within_root(locator, root))
    {
        return Err(PackageResolutionError::UnapprovedArtifactLocation);
    }
    Ok(())
}

fn contains_floating_artifact_word(value: &str) -> bool {
    value
        .split(|character: char| !character.is_ascii_alphanumeric())
        .any(|part| {
            matches!(
                part.to_ascii_lowercase().as_str(),
                "latest" | "stable" | "default" | "current" | "lts"
            )
        })
}

fn locator_is_within_root(locator: &str, root: &str) -> bool {
    let (Ok(locator), Ok(root)) = (reqwest::Url::parse(locator), reqwest::Url::parse(root)) else {
        return false;
    };
    if locator.scheme() != "https"
        || root.scheme() != "https"
        || locator.host_str() != root.host_str()
        || locator.port_or_known_default() != root.port_or_known_default()
    {
        return false;
    }
    let root_path = root.path().trim_end_matches('/');
    locator.path() == root_path
        || locator
            .path()
            .strip_prefix(root_path)
            .is_some_and(|suffix| suffix.starts_with('/'))
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
        package.declaration.validate_for_resolution()?;
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
        | OfflineInstallRecipeV1::NodeArchive { artifact_roles, .. }
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
    #[error("package artifact locator is outside the controlled source roots")]
    UnapprovedArtifactLocation,
    #[error("package fetch returned a redirect outside the validated per-hop fetch seam")]
    UnvalidatedRedirect,
    #[error("package artifact redirect chain contains a loop")]
    RedirectLoop,
    #[error("package artifact redirect chain exceeds the maximum hop count")]
    TooManyRedirects,
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
    #[error("APT resolution request is unsupported or incomplete")]
    InvalidAptRequest,
    #[error("APT resolver target or manager authority does not match the controller binding")]
    AptAuthorityMismatch,
    #[error("APT repository metadata lacks required signed snapshot evidence")]
    UnauthenticatedAptMetadata,
    #[error(
        "APT resolution would hold, remove, downgrade, replace, or leave an alternative unresolved"
    )]
    UnsafeAptTransaction,
    #[error(
        "APT resolution did not return one exact authenticated archive for every closure package"
    )]
    IncompleteAptClosure,
    #[error("APT resolver backend failed: {reason}")]
    AptBackend { reason: String },
    #[error("Node/nvm resolution request is unsupported or incomplete")]
    InvalidNodeRequest,
    #[error("Node/nvm target or manager authority does not match the controller binding")]
    NodeAuthorityMismatch,
    #[error("Node release metadata signature is not authorized")]
    UnauthenticatedNodeMetadata,
    #[error("Node release metadata omitted the exact target archive")]
    IncompleteNodeRelease,
    #[error("Node/nvm target platform has no controlled binary mapping")]
    UnsupportedNodeTarget,
    #[error("Node/nvm resolver backend failed: {reason}")]
    NodeBackend { reason: String },
    #[error(transparent)]
    Artifact(#[from] ArtifactError),
    #[error(transparent)]
    Contract(#[from] ContractError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Provider(#[from] ProviderContractError),
}
