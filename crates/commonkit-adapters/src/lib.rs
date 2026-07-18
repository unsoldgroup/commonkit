//! Capability adapters supplied with CommonKit.

mod apm;
mod artifacts;
mod chezmoi;
mod files;
mod native;
mod planning;
mod provider;
mod resources;
mod target;

pub use apm::{ApmProvider, ApmProviderConfig};
pub use artifacts::{ArtifactError, ArtifactStore, ContentReference, ContentSensitivity};
pub use chezmoi::ChezmoiProvider;
pub use files::{FileAdapter, FileAdapterError, FileIntent, ManagedRelativePath};
pub use native::NativeProvider;
pub use planning::{ProviderPlanError, ProviderPlanRequest, build_provider_plan};
pub use provider::{
    DeclaredSideEffect, DesiredStateProvider, ExactProviderVersion, MaterializedState,
    ProviderContext, ProviderContractError, ProviderFailure, ProviderInputs, ProviderWorkspace,
    UnsupportedCapability,
};
pub use resources::{
    FileMode, FilesystemIntent, NormalizedManagedPath, NormalizedResource, OwnershipError,
    OwnershipRules, ResourceError, ResourceProvenance, SafeSymlinkTarget,
    materialized_resources_digest, validate_ownership,
};
pub use target::{
    LocalTargetFilesystem, SshFilesystemRequest, SshFilesystemResponse, SshFilesystemTransport,
    TargetFilesystem, TargetFilesystemError,
};
