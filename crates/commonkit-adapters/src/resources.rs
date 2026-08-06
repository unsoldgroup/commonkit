use std::collections::BTreeMap;
use std::fmt;

use commonkit_contracts::{ContractError, Sha256Digest, digest_domain_json};
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NormalizedResource {
    pub intent: FilesystemIntent,
    pub provenance: ResourceProvenance,
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
    let mut paths: BTreeMap<String, &NormalizedResource> = BTreeMap::new();
    let mut folded: BTreeMap<String, &NormalizedResource> = BTreeMap::new();

    for resource in resources {
        let path = resource.intent.path();
        if let FilesystemIntent::Symlink { target, .. } = &resource.intent {
            target
                .validate_for(path)
                .map_err(|_| OwnershipError::UnsafeSymlinkTarget { path: path.clone() })?;
        }
        if !rules.declared_roots.iter().any(|root| path.is_within(root)) {
            return Err(OwnershipError::OutsideDeclaredRoot { path: path.clone() });
        }
        if rules
            .protected_roots
            .iter()
            .any(|root| path.is_within(root))
        {
            return Err(OwnershipError::ProtectedPath { path: path.clone() });
        }
        if let Some(existing) = paths.insert(path.as_str().into(), resource) {
            let existing_kind = existing.intent.kind();
            let incoming_kind = resource.intent.kind();
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
                && existing.intent.path() != path
            {
                return Err(OwnershipError::CaseCollision {
                    first: existing.intent.path().clone(),
                    second: path.clone(),
                });
            }
        }
    }

    let claims: Vec<&NormalizedResource> = paths.values().copied().collect();
    for exact in claims
        .iter()
        .filter(|claim| claim.intent.is_exact_directory())
    {
        for descendant in &claims {
            if exact.intent.path() != descendant.intent.path()
                && descendant.intent.path().is_within(exact.intent.path())
                && exact.provenance.provider_id != descendant.provenance.provider_id
            {
                return Err(OwnershipError::ExactDirectoryConflict {
                    directory: exact.intent.path().clone(),
                    descendant: descendant.intent.path().clone(),
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
        (
            left.intent.path().as_str(),
            left.provenance.provider_id.as_str(),
        )
            .cmp(&(
                right.intent.path().as_str(),
                right.provenance.provider_id.as_str(),
            ))
    });
    digest_domain_json("commonkit.materialized-filesystem.v1", &ordered)
}

#[derive(Debug, Error)]
pub enum ResourceError {
    #[error("managed path is not a canonical portable relative path: {0}")]
    UnsafeManagedPath(String),
    #[error("symlink target escapes the managed root or is not portable: {0}")]
    UnsafeSymlinkTarget(String),
    #[error("file mode exceeds portable permission bits: {0:o}")]
    InvalidFileMode(u32),
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum OwnershipError {
    #[error("at least one target root must be declared")]
    NoDeclaredRoots,
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
