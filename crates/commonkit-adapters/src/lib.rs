//! Capability adapters supplied with CommonKit.

mod apm;
mod apt_resolution;
mod artifacts;
mod budget;
mod chezmoi;
mod credentials;
mod documentation;
mod engram;
mod files;
mod git_sync;
mod github_onboarding;
mod mcp_clients;
mod native;
mod package_resolution;
mod packages;
mod pipeline;
mod planning;
mod provider;
mod provider_sandbox;
mod provider_snapshot;
mod remote_helper;
mod remote_provider;
mod resources;
mod service_lifecycle;
mod ssh;
mod ssh_files;
mod target;

pub use apm::{ApmProvider, ApmProviderConfig, redacted_apm_diagnostic_summary};
pub use apt_resolution::{
    AptCommandSpecV1, AptRepositoryConfigurationV1, AptResolutionBackend,
    AptResolutionCommandError, AptResolutionCommandRunner, AptResolutionSnapshotV1,
    AptResolutionSystemRequestV1, AptResolvedArchiveV1, AptResolvedPackageV1,
    AptTransactionRisksV1, ProcessAptResolutionCommandRunner,
};
pub use artifacts::{ArtifactError, ArtifactStore, ContentReference, ContentSensitivity};
pub use budget::{
    BYTES_PER_TOKEN, BudgetEntry, BudgetError, CONTEXT_BUDGET_FIELD, ContextBudgetLedger,
    ContextClass, declared_limit, tokens_for_bytes,
};
pub use chezmoi::{ChezmoiProvider, TESTED_CHEZMOI_VERSION};
pub use credentials::{
    BwsCommandError, BwsCommandRunner, BwsCredentialResolver, CredentialReadiness,
    CredentialReadinessInspector, CredentialReference, CredentialReferenceError,
    CredentialResolveError, CredentialResolver, FakeCredentialResolver,
    LocalCredentialReadinessInspector, LocalSensitiveFileStore, NativeWindowsCredentialReader,
    PlatformKeychain, PlatformKeychainCredentialResolver, PlatformSecretCommandError,
    PlatformSecretCommandRunner, ProcessBwsRunner, ProcessPlatformSecretCommandRunner, SecretValue,
    SensitiveFileError, WindowsCredentialManagerResolver, WindowsCredentialReader,
};
pub use documentation::{
    DOCLING_VERSION, DocumentationIngestionError, DocumentationSectionCandidate,
    DocumentationSource, IngestionDisposition, IngestionResult, ProjectContextProposal,
    ProjectDocumentationRole, RichExtractionLimits, RichExtractionPlan, discover_project_context,
    extract_rich_document, fetch_web_snapshot, ingest_documentation, publish_documentation,
    validate_resolved_web_addresses,
};
pub use engram::{
    EngramChunkAdapter, EngramChunkDigest, EngramChunkMovement, EngramChunkSetDeclaration,
    EngramChunkSetState, EngramChunkSetStatus, EngramCommandOutput, EngramCommandRunner,
    EngramError, EngramGrant, EngramGrantState, EngramOwnerId, EngramProjectId,
    EngramReconciliationReceipt, EngramScope, EngramScopeAttestation, EngramTargetChunkMovement,
    EngramTargetChunkSetDeclaration, EngramTargetReconciliationReceipt, ProcessEngramCommandRunner,
};
pub use files::{FileAdapter, FileAdapterError, FileIntent, ManagedRelativePath};
pub use git_sync::{
    FastForwardPolicy, GitCommandError, GitCommandOutput, GitCommandRunner, GitRepository,
    GitRevision, GitSyncDisposition, GitSyncError, GitSyncStatus, ProcessGitRunner,
};
pub use github_onboarding::{
    GitHubOnboardingAdapter, GitHubOnboardingError, GitHubRepositoryObservation,
    GitHubRepositoryPlan, GitHubRepositoryRequest, GitHubTransport, RepositoryApplyDisposition,
    RepositoryApplyResult,
};
pub use mcp_clients::{McpClientMaterializationError, materialize_mcp_client_state};
pub use native::NativeProvider;
pub use package_resolution::{
    ArtifactEvidence, ControlledPackageSourceV1, ManagerBindingV1, OfflineInstallRecipeV1,
    OfflinePackageBackend, PackageArtifactV1, PackageFetch, PackageFetchHopV1,
    PackageFetchRequestV1, PackageFetchResultV1, PackageObservationV1, PackageResolutionAuthority,
    PackageResolutionBackend, PackageResolutionCoordinator, PackageResolutionDraftV1,
    PackageResolutionError, PackageResolutionProbeV1, PackageResolutionRequestV1,
    PackageResolutionV1, PackageSourceRegistry, PackageTargetV1, ResolvedPackage,
    ResolvedPackageIntent, SourceBindingV1, package_resolution_schema,
};
pub use packages::{
    PackageBackendEvidence, PackageCommandError, PackageCommandOutput, PackageCommandRunner,
    PackageDriftEntry, PackageDriftReport, PackageDriftState, PackageObserver,
    ProcessPackageCommandRunner, parse_backend_output,
};
pub use pipeline::{ProviderPipeline, ProviderPipelineError, ProviderPipelineOutput};
pub use planning::{
    FilesystemPlannerRoute, PackageResourcePlanner, ProviderPlanError, ProviderPlanRequest,
    ProviderPlannerRoute, ProviderResourcePlanner, ProviderResourceRouter, build_provider_plan,
    build_provider_plan_with_router, build_resolved_provider_plan_with_router,
    provider_plan_bindings, resolved_provider_plan_bindings,
};
pub use provider::{
    DeclaredSideEffect, DesiredStateProvider, ExactProviderVersion, MaterializedState,
    ProviderCapability, ProviderCapabilityResource, ProviderContext, ProviderContractError,
    ProviderFailure, ProviderInputs, ProviderWorkspace, ResolvedMaterializedState,
    UnsupportedCapability,
};
pub use remote_helper::TargetHelper;
pub use remote_provider::{
    RemoteMaterializationReceipt, RemoteProviderStager, RemoteProviderStagingError,
};
pub use resources::{
    FileMode, FilesystemIntent, NormalizedManagedPath, NormalizedResource, OwnershipError,
    OwnershipRules, PackageDesiredIntent, ProviderResourceIntent, ResourceAddress, ResourceError,
    ResourceIntent, ResourceProvenance, ResourceType, SafeSymlinkTarget, SymlinkTargetKind,
    materialized_resources_digest, validate_ownership,
};
pub use service_lifecycle::{
    LifecycleCommand, ServiceAdapter, ServiceBackend, ServiceDesiredState, ServiceError,
    ServiceObservedState, ServiceSpec, ServiceStartMode,
};
pub use ssh::{
    OpenSshConfig, OpenSshTransport, ProcessOutput, ProcessRemoteRunner, RemoteProcessRunner,
};
pub use ssh_files::{SshFileAdapter, SshFileAdapterError, SshTargetCapabilities};
pub use target::{
    EngramTargetRuntime, EngramTargetSyncMode, LocalTargetFilesystem, SshFilesystemRequest,
    SshFilesystemResponse, SshFilesystemTransport, SshTargetFilesystem, TargetFilesystem,
    TargetFilesystemError, TargetResource,
};
