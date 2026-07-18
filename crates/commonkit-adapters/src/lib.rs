//! Capability adapters supplied with CommonKit.

mod artifacts;
mod files;
mod resources;

pub use artifacts::{ArtifactError, ArtifactStore, ContentReference, ContentSensitivity};
pub use files::{FileAdapter, FileAdapterError, FileIntent, ManagedRelativePath};
pub use resources::{
    FileMode, FilesystemIntent, NormalizedManagedPath, NormalizedResource, OwnershipError,
    OwnershipRules, ResourceError, ResourceProvenance, SafeSymlinkTarget,
    materialized_resources_digest, validate_ownership,
};
