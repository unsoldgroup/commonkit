//! Capability adapters supplied with CommonKit.

mod apm;
mod artifacts;
mod chezmoi;
mod credentials;
mod files;
mod git_sync;
mod native;
mod planning;
mod pipeline;
mod provider;
mod remote_helper;
mod remote_provider;
mod resources;
mod service_lifecycle;
mod ssh;
mod target;

pub use apm::{ApmProvider, ApmProviderConfig};
pub use artifacts::{ArtifactError, ArtifactStore, ContentReference, ContentSensitivity};
pub use chezmoi::ChezmoiProvider;
pub use credentials::{
    BwsCommandError, BwsCommandRunner, BwsCredentialResolver, CredentialReadiness,
    CredentialReadinessInspector, CredentialReference, CredentialReferenceError,
    CredentialResolveError, CredentialResolver, FakeCredentialResolver,
    LocalCredentialReadinessInspector, LocalSensitiveFileStore, NativeWindowsCredentialReader,
    PlatformKeychain, PlatformKeychainCredentialResolver, PlatformSecretCommandError,
    PlatformSecretCommandRunner, ProcessBwsRunner, ProcessPlatformSecretCommandRunner, SecretValue,
    SensitiveFileError, WindowsCredentialManagerResolver, WindowsCredentialReader,
};
pub use files::{FileAdapter, FileAdapterError, FileIntent, ManagedRelativePath};
pub use git_sync::{
    FastForwardPolicy, GitCommandError, GitCommandOutput, GitCommandRunner, GitRepository,
    GitRevision, GitSyncDisposition, GitSyncError, GitSyncStatus, ProcessGitRunner,
};
pub use native::NativeProvider;
pub use planning::{ProviderPlanError, ProviderPlanRequest, build_provider_plan};
pub use pipeline::{ProviderPipeline, ProviderPipelineError, ProviderPipelineOutput};
pub use provider::{
    DeclaredSideEffect, DesiredStateProvider, ExactProviderVersion, MaterializedState,
    ProviderContext, ProviderContractError, ProviderFailure, ProviderInputs, ProviderWorkspace,
    UnsupportedCapability,
};
pub use remote_helper::TargetHelper;
pub use remote_provider::{
    RemoteMaterializationReceipt, RemoteProviderStager, RemoteProviderStagingError,
};
pub use resources::{
    FileMode, FilesystemIntent, NormalizedManagedPath, NormalizedResource, OwnershipError,
    OwnershipRules, ResourceError, ResourceProvenance, SafeSymlinkTarget,
    materialized_resources_digest, validate_ownership,
};
pub use service_lifecycle::{
    LifecycleCommand, ServiceAdapter, ServiceBackend, ServiceDesiredState, ServiceError,
    ServiceObservedState, ServiceSpec, ServiceStartMode,
};
pub use ssh::{
    OpenSshConfig, OpenSshTransport, ProcessOutput, ProcessRemoteRunner, RemoteProcessRunner,
};
pub use target::{
    LocalTargetFilesystem, SshFilesystemRequest, SshFilesystemResponse, SshFilesystemTransport,
    TargetFilesystem, TargetFilesystemError,
};
