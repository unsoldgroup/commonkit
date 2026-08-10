use std::collections::BTreeMap;
use std::fmt;

use commonkit_contracts::{
    ContractError, PackageDeclaration, PackageManager, RecoveryCapability, Sha256Digest, StableId,
    digest_domain_json,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::artifacts::ContentReference;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct NormalizedManagedPath(String);

impl NormalizedManagedPath {
    pub fn parse(value: impl Into<String>) -> Result<Self, ResourceError> {
        let value = value.into();
        if value.is_empty()
            || value.starts_with('/')
            || value.ends_with('/')
            || value.contains('\\')
            || value.contains(':')
            || value
                .split('/')
                .any(|segment| segment.is_empty() || segment == "." || segment == "..")
        {
            return Err(ResourceError::UnsafeManagedPath(value));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn is_within(&self, root: &Self) -> bool {
        self == root
            || self
                .0
                .strip_prefix(&root.0)
                .is_some_and(|suffix| suffix.starts_with('/'))
    }
}

impl TryFrom<String> for NormalizedManagedPath {
    type Error = ResourceError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl From<NormalizedManagedPath> for String {
    fn from(value: NormalizedManagedPath) -> Self {
        value.0
    }
}

impl fmt::Display for NormalizedManagedPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u32", into = "u32")]
pub struct FileMode(u32);

impl FileMode {
    pub fn parse(value: u32) -> Result<Self, ResourceError> {
        if value <= 0o7777 {
            Ok(Self(value))
        } else {
            Err(ResourceError::InvalidFileMode(value))
        }
    }

    pub fn value(&self) -> u32 {
        self.0
    }
}

impl TryFrom<u32> for FileMode {
    type Error = ResourceError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl From<FileMode> for u32 {
    fn from(value: FileMode) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SafeSymlinkTarget(String);

impl SafeSymlinkTarget {
    pub fn parse(
        link: &NormalizedManagedPath,
        target: impl Into<String>,
    ) -> Result<Self, ResourceError> {
        let target = target.into();
        if target.is_empty()
            || target.starts_with('/')
            || target.ends_with('/')
            || target.contains('\\')
            || target.contains(':')
        {
            return Err(ResourceError::UnsafeSymlinkTarget(target));
        }
        let mut resolved: Vec<&str> = link.as_str().split('/').collect();
        resolved.pop();
        for segment in target.split('/') {
            match segment {
                "" | "." => return Err(ResourceError::UnsafeSymlinkTarget(target)),
                ".." => {
                    if resolved.pop().is_none() {
                        return Err(ResourceError::UnsafeSymlinkTarget(target));
                    }
                }
                value => resolved.push(value),
            }
        }
        if resolved.is_empty() {
            return Err(ResourceError::UnsafeSymlinkTarget(target));
        }
        Ok(Self(target))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn validate_for(&self, link: &NormalizedManagedPath) -> Result<(), ResourceError> {
        Self::parse(link, self.0.clone()).map(|_| ())
    }

    pub(crate) fn resolved_for(
        &self,
        link: &NormalizedManagedPath,
    ) -> Result<NormalizedManagedPath, ResourceError> {
        self.validate_for(link)?;
        let mut resolved: Vec<&str> = link.as_str().split('/').collect();
        resolved.pop();
        for segment in self.0.split('/') {
            if segment == ".." {
                resolved.pop();
            } else {
                resolved.push(segment);
            }
        }
        NormalizedManagedPath::parse(resolved.join("/"))
    }
}

impl TryFrom<String> for SafeSymlinkTarget {
    type Error = ResourceError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        // Deserialization cannot validate containment without its link path.
        if value.is_empty()
            || value.starts_with('/')
            || value.ends_with('/')
            || value.contains('\\')
            || value.contains(':')
        {
            Err(ResourceError::UnsafeSymlinkTarget(value))
        } else {
            Ok(Self(value))
        }
    }
}

impl From<SafeSymlinkTarget> for String {
    fn from(value: SafeSymlinkTarget) -> Self {
        value.0
    }
}

pub type ResourceProvenance = commonkit_contracts::OperationProvenance;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymlinkTargetKind {
    #[default]
    File,
    Directory,
}

impl SymlinkTargetKind {
    pub(crate) fn is_file(value: &Self) -> bool {
        *value == Self::File
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum FilesystemIntent {
    File {
        path: NormalizedManagedPath,
        content: ContentReference,
        mode: Option<FileMode>,
        expected_before: Option<Sha256Digest>,
    },
    Directory {
        path: NormalizedManagedPath,
        mode: Option<FileMode>,
        exact: bool,
    },
    Symlink {
        path: NormalizedManagedPath,
        target: SafeSymlinkTarget,
        #[serde(default, skip_serializing_if = "SymlinkTargetKind::is_file")]
        target_kind: SymlinkTargetKind,
        expected_before: Option<Sha256Digest>,
    },
    Remove {
        path: NormalizedManagedPath,
        expected_before: Option<Sha256Digest>,
    },
}

impl FilesystemIntent {
    pub fn path(&self) -> &NormalizedManagedPath {
        match self {
            Self::File { path, .. }
            | Self::Directory { path, .. }
            | Self::Symlink { path, .. }
            | Self::Remove { path, .. } => path,
        }
    }

    fn kind(&self) -> ResourceKind {
        match self {
            Self::File { .. } => ResourceKind::File,
            Self::Directory { .. } => ResourceKind::Directory,
            Self::Symlink { .. } => ResourceKind::Symlink,
            Self::Remove { .. } => ResourceKind::Remove,
        }
    }

    fn is_exact_directory(&self) -> bool {
        matches!(self, Self::Directory { exact: true, .. })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceType {
    Filesystem,
    Package,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "resourceType", rename_all = "snake_case", deny_unknown_fields)]
pub enum ResourceAddress {
    Filesystem {
        path: NormalizedManagedPath,
    },
    Package {
        manager: PackageManager,
        id: StableId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum PackageResourceIntent {
    Package {
        declaration: PackageDeclaration,
        resolution: ContentReference,
        artifacts: Vec<ContentReference>,
    },
}

impl PackageResourceIntent {
    pub fn new(
        declaration: PackageDeclaration,
        resolution: ContentReference,
        artifacts: Vec<ContentReference>,
    ) -> Result<Self, ResourceError> {
        declaration
            .validate()
            .map_err(|_| ResourceError::InvalidPackageDeclaration)?;
        if artifacts.is_empty() {
            return Err(ResourceError::MissingPackageArtifact);
        }
        Ok(Self::Package {
            declaration,
            resolution,
            artifacts,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ResourceIntent {
    Filesystem(FilesystemIntent),
    Package(PackageResourceIntent),
}

impl From<FilesystemIntent> for ResourceIntent {
    fn from(intent: FilesystemIntent) -> Self {
        Self::Filesystem(intent)
    }
}

impl From<PackageResourceIntent> for ResourceIntent {
    fn from(intent: PackageResourceIntent) -> Self {
        Self::Package(intent)
    }
}

impl ResourceIntent {
    pub fn resource_type(&self) -> ResourceType {
        match self {
            Self::Filesystem(_) => ResourceType::Filesystem,
            Self::Package(_) => ResourceType::Package,
        }
    }

    pub fn address(&self) -> ResourceAddress {
        match self {
            Self::Filesystem(intent) => ResourceAddress::Filesystem {
                path: intent.path().clone(),
            },
            Self::Package(PackageResourceIntent::Package { declaration, .. }) => {
                ResourceAddress::Package {
                    manager: declaration.manager,
                    id: declaration.id.clone(),
                }
            }
        }
    }

    pub fn sort_key(&self) -> ResourceAddress {
        self.address()
    }

    pub fn artifact_references(&self) -> Vec<&ContentReference> {
        match self {
            Self::Filesystem(FilesystemIntent::File { content, .. }) => vec![content],
            Self::Filesystem(_) => Vec::new(),
            Self::Package(PackageResourceIntent::Package {
                resolution,
                artifacts,
                ..
            }) => {
                let mut references = Vec::with_capacity(artifacts.len() + 1);
                references.push(resolution);
                references.extend(artifacts);
                references
            }
        }
    }

    pub fn desired_digest(&self) -> Result<Sha256Digest, ContractError> {
        match self {
            Self::Filesystem(intent) => {
                digest_domain_json("commonkit.filesystem-resource-desired.v1", intent)
            }
            Self::Package(intent) => {
                digest_domain_json("commonkit.package-resource-desired.v1", intent)
            }
        }
    }

    pub fn recovery_capability(&self) -> RecoveryCapability {
        match self {
            Self::Filesystem(_) => RecoveryCapability::ExactRollback,
            Self::Package(_) => RecoveryCapability::ConvergeForwardOnly,
        }
    }

    pub fn filesystem(&self) -> Option<&FilesystemIntent> {
        match self {
            Self::Filesystem(intent) => Some(intent),
            Self::Package(_) => None,
        }
    }

    pub fn filesystem_mut(&mut self) -> Option<&mut FilesystemIntent> {
        match self {
            Self::Filesystem(intent) => Some(intent),
            Self::Package(_) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NormalizedResource {
    pub intent: ResourceIntent,
    pub provenance: ResourceProvenance,
}

impl NormalizedResource {
    pub fn resource_type(&self) -> ResourceType {
        self.intent.resource_type()
    }

    pub fn address(&self) -> ResourceAddress {
        self.intent.address()
    }

    pub fn sort_key(&self) -> ResourceAddress {
        self.intent.sort_key()
    }

    pub fn artifact_references(&self) -> Vec<&ContentReference> {
        self.intent.artifact_references()
    }

    pub fn desired_digest(&self) -> Result<Sha256Digest, ContractError> {
        self.intent.desired_digest()
    }

    pub fn recovery_capability(&self) -> RecoveryCapability {
        self.intent.recovery_capability()
    }
}

#[derive(Debug, Clone)]
pub struct OwnershipRules {
    case_sensitive: bool,
    declared_roots: Vec<NormalizedManagedPath>,
    protected_roots: Vec<NormalizedManagedPath>,
}

impl OwnershipRules {
    pub fn new(
        case_sensitive: bool,
        declared_roots: Vec<NormalizedManagedPath>,
        protected_roots: Vec<NormalizedManagedPath>,
    ) -> Result<Self, OwnershipError> {
        if declared_roots.is_empty() {
            return Err(OwnershipError::NoDeclaredRoots);
        }
        Ok(Self {
            case_sensitive,
            declared_roots,
            protected_roots,
        })
    }

    pub(crate) fn authority_digest(&self) -> Result<Sha256Digest, ContractError> {
        let mut declared_roots = self
            .declared_roots
            .iter()
            .map(NormalizedManagedPath::as_str)
            .collect::<Vec<_>>();
        declared_roots.sort_unstable();
        declared_roots.dedup();
        let mut protected_roots = self
            .protected_roots
            .iter()
            .map(NormalizedManagedPath::as_str)
            .collect::<Vec<_>>();
        protected_roots.sort_unstable();
        protected_roots.dedup();
        digest_domain_json(
            "commonkit.ownership-authority.v1",
            &(self.case_sensitive, declared_roots, protected_roots),
        )
    }

    fn contains_path(&self, path: &NormalizedManagedPath, root: &NormalizedManagedPath) -> bool {
        if self.case_sensitive {
            path.is_within(root)
        } else {
            let path = path.as_str().to_lowercase();
            let root = root.as_str().to_lowercase();
            path == root
                || path
                    .strip_prefix(&root)
                    .is_some_and(|suffix| suffix.starts_with('/'))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResourceKind {
    File,
    Directory,
    Symlink,
    Remove,
}

pub fn validate_ownership(
    resources: &[NormalizedResource],
    rules: &OwnershipRules,
) -> Result<(), OwnershipError> {
    let mut addresses: BTreeMap<ResourceAddress, &NormalizedResource> = BTreeMap::new();
    let mut paths: BTreeMap<String, &NormalizedResource> = BTreeMap::new();
    let mut folded: BTreeMap<String, &NormalizedResource> = BTreeMap::new();

    for resource in resources {
        let address = resource.address();
        if addresses.insert(address.clone(), resource).is_some()
            && matches!(address, ResourceAddress::Package { .. })
        {
            return Err(OwnershipError::DuplicatePackage { address });
        }
        let Some(intent) = resource.intent.filesystem() else {
            let ResourceIntent::Package(PackageResourceIntent::Package {
                declaration,
                artifacts,
                ..
            }) = &resource.intent
            else {
                unreachable!("closed resource vocabulary")
            };
            declaration
                .validate()
                .map_err(|_| OwnershipError::InvalidPackageDeclaration)?;
            if artifacts.is_empty() {
                return Err(OwnershipError::MissingPackageArtifact { address });
            }
            continue;
        };
        let path = intent.path();
        if let FilesystemIntent::Symlink { target, .. } = intent {
            let resolved = target
                .resolved_for(path)
                .map_err(|_| OwnershipError::UnsafeSymlinkTarget { path: path.clone() })?;
            if rules
                .protected_roots
                .iter()
                .any(|root| rules.contains_path(&resolved, root))
            {
                return Err(OwnershipError::ProtectedPath { path: path.clone() });
            }
        }
        if !rules
            .declared_roots
            .iter()
            .any(|root| rules.contains_path(path, root))
        {
            return Err(OwnershipError::OutsideDeclaredRoot { path: path.clone() });
        }
        if rules.protected_roots.iter().any(|root| {
            rules.contains_path(path, root)
                || (rules.contains_path(root, path)
                    && !matches!(
                        intent,
                        FilesystemIntent::Directory {
                            exact: false,
                            mode: None,
                            ..
                        }
                    ))
        }) {
            return Err(OwnershipError::ProtectedPath { path: path.clone() });
        }
        if let Some(existing) = paths.insert(path.as_str().into(), resource) {
            let existing_kind = existing
                .intent
                .filesystem()
                .expect("filesystem path map only")
                .kind();
            let incoming_kind = intent.kind();
            return Err(
                if existing_kind == ResourceKind::Remove || incoming_kind == ResourceKind::Remove {
                    OwnershipError::RemovalConflict { path: path.clone() }
                } else if existing_kind != incoming_kind {
                    OwnershipError::TypeConflict { path: path.clone() }
                } else {
                    OwnershipError::DuplicatePath { path: path.clone() }
                },
            );
        }
        if !rules.case_sensitive {
            let key = path.as_str().to_lowercase();
            if let Some(existing) = folded.insert(key, resource)
                && existing
                    .intent
                    .filesystem()
                    .expect("filesystem folded map only")
                    .path()
                    != path
            {
                return Err(OwnershipError::CaseCollision {
                    first: existing
                        .intent
                        .filesystem()
                        .expect("filesystem folded map only")
                        .path()
                        .clone(),
                    second: path.clone(),
                });
            }
        }
    }

    let claims: Vec<&NormalizedResource> = paths.values().copied().collect();
    for exact in claims.iter().filter(|claim| {
        claim
            .intent
            .filesystem()
            .is_some_and(FilesystemIntent::is_exact_directory)
    }) {
        let exact_intent = exact.intent.filesystem().expect("filesystem claims only");
        for descendant in &claims {
            let descendant_intent = descendant
                .intent
                .filesystem()
                .expect("filesystem claims only");
            if exact_intent.path() != descendant_intent.path()
                && rules.contains_path(descendant_intent.path(), exact_intent.path())
                && exact.provenance.provider_id != descendant.provenance.provider_id
            {
                return Err(OwnershipError::ExactDirectoryConflict {
                    directory: exact_intent.path().clone(),
                    descendant: descendant_intent.path().clone(),
                });
            }
        }
    }
    Ok(())
}

pub fn materialized_resources_digest(
    resources: &[NormalizedResource],
) -> Result<Sha256Digest, ContractError> {
    let mut ordered: Vec<&NormalizedResource> = resources.iter().collect();
    ordered.sort_by(|left, right| {
        (left.sort_key(), left.provenance.provider_id.as_str())
            .cmp(&(right.sort_key(), right.provenance.provider_id.as_str()))
    });
    let domain = if ordered
        .iter()
        .all(|resource| resource.resource_type() == ResourceType::Filesystem)
    {
        "commonkit.materialized-filesystem.v1"
    } else {
        "commonkit.materialized-resources.v2"
    };
    digest_domain_json(domain, &ordered)
}

#[derive(Debug, Error)]
pub enum ResourceError {
    #[error("managed path is not a canonical portable relative path: {0}")]
    UnsafeManagedPath(String),
    #[error("symlink target escapes the managed root or is not portable: {0}")]
    UnsafeSymlinkTarget(String),
    #[error("file mode exceeds portable permission bits: {0:o}")]
    InvalidFileMode(u32),
    #[error("package declaration is not exact")]
    InvalidPackageDeclaration,
    #[error("package resource must bind at least one immutable artifact")]
    MissingPackageArtifact,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum OwnershipError {
    #[error("at least one target root must be declared")]
    NoDeclaredRoots,
    #[error("package declaration is not exact")]
    InvalidPackageDeclaration,
    #[error("package resource has no immutable artifact: {address:?}")]
    MissingPackageArtifact { address: ResourceAddress },
    #[error("multiple resources claim the same package identity: {address:?}")]
    DuplicatePackage { address: ResourceAddress },
    #[error("symlink target escapes the managed root at path: {path}")]
    UnsafeSymlinkTarget { path: NormalizedManagedPath },
    #[error("provider claim is outside declared roots: {path}")]
    OutsideDeclaredRoot { path: NormalizedManagedPath },
    #[error("provider claim targets protected CommonKit state: {path}")]
    ProtectedPath { path: NormalizedManagedPath },
    #[error("multiple resources claim the same path: {path}")]
    DuplicatePath { path: NormalizedManagedPath },
    #[error("resource types conflict at path: {path}")]
    TypeConflict { path: NormalizedManagedPath },
    #[error("removal conflicts with another resource at path: {path}")]
    RemovalConflict { path: NormalizedManagedPath },
    #[error("paths collide on a case-insensitive target: {first} and {second}")]
    CaseCollision {
        first: NormalizedManagedPath,
        second: NormalizedManagedPath,
    },
    #[error("exact directory {directory} conflicts with descendant claim {descendant}")]
    ExactDirectoryConflict {
        directory: NormalizedManagedPath,
        descendant: NormalizedManagedPath,
    },
}
